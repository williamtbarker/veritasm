use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn gzip_bytes(contents: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(contents).unwrap();
    encoder.finish().unwrap()
}

fn gzip_bytes_with_exact_transport_length(contents: &[u8], total: usize) -> Vec<u8> {
    fn empty_member_with_length(length: usize) -> Vec<u8> {
        let ordinary = gzip_bytes(b"");
        if length == ordinary.len() {
            return ordinary;
        }
        let mut zero_extra = GzBuilder::new()
            .extra(Vec::new())
            .write(Vec::new(), Compression::fast());
        zero_extra.write_all(b"").unwrap();
        let extra_overhead = zero_extra.finish().unwrap().len();
        let extra_length = length
            .checked_sub(extra_overhead)
            .expect("requested empty gzip member is not representable");
        assert!(extra_length <= usize::from(u16::MAX));
        let mut encoder = GzBuilder::new()
            .extra(vec![b'x'; extra_length])
            .write(Vec::new(), Compression::fast());
        encoder.write_all(b"").unwrap();
        let member = encoder.finish().unwrap();
        assert_eq!(member.len(), length);
        member
    }

    let payload = gzip_bytes(contents);
    let ordinary_empty_length = gzip_bytes(b"").len();
    let mut zero_extra = GzBuilder::new()
        .extra(Vec::new())
        .write(Vec::new(), Compression::fast());
    zero_extra.write_all(b"").unwrap();
    let extra_overhead = zero_extra.finish().unwrap().len();
    let maximum_member_length = extra_overhead + usize::from(u16::MAX);
    let mut remaining = total
        .checked_sub(payload.len())
        .expect("requested gzip transport length is too small");
    let mut bytes = Vec::with_capacity(total);
    while remaining > maximum_member_length {
        let after_maximum = remaining - maximum_member_length;
        let member_length =
            if after_maximum != ordinary_empty_length && after_maximum < extra_overhead {
                remaining - extra_overhead
            } else {
                maximum_member_length
            };
        bytes.extend_from_slice(&empty_member_with_length(member_length));
        remaining -= member_length;
    }
    if remaining != 0 {
        assert!(remaining == ordinary_empty_length || remaining >= extra_overhead);
        bytes.extend_from_slice(&empty_member_with_length(remaining));
    }
    bytes.extend_from_slice(&payload);
    assert_eq!(bytes.len(), total);
    bytes
}

fn write_gzip(path: &Path, contents: &[u8]) {
    fs::write(path, gzip_bytes(contents)).unwrap();
}

fn run(arguments: Vec<String>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_veritasm"))
        .args(arguments)
        .output()
        .unwrap()
}

fn run_with_stdin(arguments: Vec<String>, input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_veritasm"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child_stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || match child_stdin.write_all(&input) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    });
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap().unwrap();
    output
}

fn run_in(arguments: Vec<String>, current_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_veritasm"))
        .args(arguments)
        .current_dir(current_dir)
        .output()
        .unwrap()
}

fn single_args(input: &Path, output: &Path, extra: &[&str]) -> Vec<String> {
    let mut arguments = vec![
        "assemble".to_owned(),
        "-U".to_owned(),
        input.display().to_string(),
        "-o".to_owned(),
        output.display().to_string(),
        "-k".to_owned(),
        "5".to_owned(),
    ];
    arguments.extend(extra.iter().map(|argument| (*argument).to_owned()));
    arguments
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, exit_code: i32, error_code: &str) {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("error[{error_code}]:")),
        "expected {error_code:?} in stderr: {stderr}"
    );
}

#[test]
fn diagnostics_escape_control_characters_in_paths_and_stay_on_one_line() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fa");
    let hostile_parent = temporary.path().join("parent\nforged\tfield\u{1b}[31m");
    let bundle = hostile_parent.join("result");
    fs::write(&input, b">read-a\nAACGCTA\n").unwrap();
    fs::create_dir(&hostile_parent).unwrap();
    fs::create_dir(&bundle).unwrap();

    let result = run(single_args(&input, &bundle, &["--profile", "retain-all"]));

    assert_failure(&result, 7, "destination_existing");
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert_eq!(stderr.bytes().filter(|byte| *byte == b'\n').count(), 1);
    assert!(stderr.contains("\\n"));
    assert!(stderr.contains("\\t"));
    assert!(stderr.contains("\\u{1b}"));
    assert!(stderr[..stderr.len() - 1]
        .chars()
        .all(|character| !character.is_control()));
}

fn read_run(bundle: &Path) -> Value {
    serde_json::from_slice(&fs::read(bundle.join("run.json")).unwrap()).unwrap()
}

fn collect_regular_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, files);
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(relative, fs::read(path).unwrap());
            } else {
                panic!("unexpected non-regular bundle entry: {}", path.display());
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn lower_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").unwrap();
    }
    output
}

fn assert_manifest_integrity(bundle: &Path) {
    let files = collect_regular_files(bundle);
    for required in [
        "assembly.gfa",
        "pair_audit_summary.tsv",
        "pair_links.tsv",
        "report.html",
        "run.json",
        "schema/assembly_gfa.schema.json",
        "schema/manifest.json",
        "schema/pair_audit_summary.schema.json",
        "schema/pair_links.schema.json",
        "schema/run.schema.json",
        "schema/transform_summary.schema.json",
        "schema/unitig_evidence.schema.json",
        "transform_summary.tsv",
        "unitig_evidence.tsv",
        "unitigs.fasta",
    ] {
        assert!(files.contains_key(required), "bundle omitted {required}");
    }
    let manifest = String::from_utf8(files["manifest.sha256"].clone()).unwrap();
    assert!(manifest.ends_with('\n'));

    let mut listed = BTreeSet::new();
    let mut previous: Option<&str> = None;
    for line in manifest.lines() {
        let (digest, relative) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("invalid manifest line: {line:?}"));
        assert_eq!(digest.len(), 64);
        assert!(digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        if let Some(prior) = previous {
            assert!(prior < relative, "manifest is not strictly sorted");
        }
        previous = Some(relative);
        assert!(
            listed.insert(relative.to_owned()),
            "duplicate manifest path"
        );
        let bytes = files
            .get(relative)
            .unwrap_or_else(|| panic!("manifest lists missing file {relative:?}"));
        assert_eq!(
            digest,
            lower_sha256(bytes),
            "digest mismatch for {relative}"
        );
    }

    let expected = files
        .keys()
        .filter(|path| path.as_str() != "manifest.sha256")
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(listed, expected);
}

fn paired_args(read1: &[PathBuf], read2: &[PathBuf], output: &Path, extra: &[&str]) -> Vec<String> {
    let mut arguments = vec!["assemble".to_owned(), "-1".to_owned()];
    arguments.extend(read1.iter().map(|path| path.display().to_string()));
    arguments.push("-2".to_owned());
    arguments.extend(read2.iter().map(|path| path.display().to_string()));
    arguments.extend([
        "-o".to_owned(),
        output.display().to_string(),
        "-k".to_owned(),
        "5".to_owned(),
    ]);
    arguments.extend(extra.iter().map(|argument| (*argument).to_owned()));
    arguments
}

#[test]
fn assembles_plain_fasta_into_a_complete_evidence_bundle() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fa");
    let bundle = temporary.path().join("result");
    fs::write(&input, b">read-a\nAACGCTA\n>read-b\nAACGCTA\n").unwrap();

    let result = run(single_args(
        &input,
        &bundle,
        &["--profile", "retain-all", "--threads", "1"],
    ));
    assert_success(&result);

    let run = read_run(&bundle);
    assert_eq!(run["status"]["code"], "software_run_complete");
    assert_eq!(run["input"]["mode"], "single_end");
    assert_eq!(run["input"]["sources"][0]["format"], "fasta");
    assert_eq!(run["input"]["fragments_decimal"], "2");
    assert_eq!(run["input"]["reads_decimal"], "2");
    assert_eq!(run["input"]["inferred_mate_roles_decimal"], "0");
    let mapper = &run["read_audit"]["mapper"];
    assert_eq!(mapper["algorithm_id"], "literal_rarest_seed_zero_mismatch");
    assert_eq!(mapper["algorithm_version"], "1");
    assert_eq!(mapper["target_universe"], "all_emitted_linear_unitigs_only");
    assert_eq!(mapper["seed_length"], 15);
    assert_eq!(mapper["execution_status"], "executed");
    assert_eq!(mapper["linear_targets_decimal"], "1");
    assert_eq!(mapper["index_postings_decimal"], "0");
    assert!(
        mapper["accounted_index_bytes_decimal"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    assert!(fs::read_to_string(bundle.join("unitigs.fasta"))
        .unwrap()
        .contains("AACGCTA"));
    assert!(fs::read_to_string(bundle.join("assembly.gfa"))
        .unwrap()
        .starts_with("H\tVN:Z:1.0\tPN:Z:veritasm"));
    let html = fs::read_to_string(bundle.join("report.html")).unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("literal_rarest_seed_zero_mismatch"));
    assert!(html.contains("<th>Mapper literal seed length</th><td>15</td>"));
    assert!(!html.contains("<script src="));
    assert!(!html.contains("<link rel=\"stylesheet\""));
    assert_manifest_integrity(&bundle);
}

#[test]
fn even_k_self_reverse_complement_edge_cannot_create_a_hairpin_unitig() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("palindrome.fa");
    let one_thread_bundle = temporary.path().join("palindrome-t1");
    let four_thread_bundle = temporary.path().join("palindrome-t4");
    fs::write(&input, b">read-a\nTCGCGAC\n").unwrap();

    let arguments = |bundle: &Path, threads: &str| {
        vec![
            "assemble".to_owned(),
            "-U".to_owned(),
            input.display().to_string(),
            "-o".to_owned(),
            bundle.display().to_string(),
            "-k".to_owned(),
            "6".to_owned(),
            "--profile".to_owned(),
            "retain-all".to_owned(),
            "--min-base-quality".to_owned(),
            "0".to_owned(),
            "--no-remap".to_owned(),
            "--threads".to_owned(),
            threads.to_owned(),
        ]
    };
    let one_thread = run(arguments(&one_thread_bundle, "1"));
    let four_threads = run(arguments(&four_thread_bundle, "4"));
    assert_success(&one_thread);
    assert_success(&four_threads);
    assert_eq!(
        collect_regular_files(&one_thread_bundle),
        collect_regular_files(&four_thread_bundle)
    );

    let fasta = fs::read_to_string(one_thread_bundle.join("unitigs.fasta")).unwrap();
    let sequences = fasta
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('>'))
        .collect::<BTreeSet<_>>();
    assert_eq!(sequences, BTreeSet::from(["CGCGAC", "TCGCGA"]));
    assert!(!fasta.contains("GTCGCGAC"));

    let evidence = fs::read_to_string(one_thread_bundle.join("unitig_evidence.tsv")).unwrap();
    let rows = evidence.lines().skip(1).collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    for row in rows {
        let fields = row.split('\t').collect::<Vec<_>>();
        assert_eq!(fields[3], "6");
        assert_eq!(fields[5], "1");
        assert_eq!(fields[6], "1");
    }

    let gfa = fs::read_to_string(one_thread_bundle.join("assembly.gfa")).unwrap();
    assert_eq!(
        gfa.lines().filter(|line| line.starts_with("S\t")).count(),
        2
    );
    assert_eq!(
        gfa.lines().filter(|line| line.starts_with("L\t")).count(),
        2
    );
    let run = read_run(&one_thread_bundle);
    assert_eq!(run["graph"]["self_reverse_complement_keys_decimal"], "1");
    assert_eq!(run["graph"]["unitigs_decimal"], "2");
    let mapper = &run["read_audit"]["mapper"];
    assert_eq!(mapper["algorithm_id"], "literal_rarest_seed_zero_mismatch");
    assert_eq!(mapper["seed_length"], 15);
    assert_eq!(mapper["execution_status"], "not_requested");
    assert_eq!(mapper["linear_targets_decimal"], "0");
    assert_eq!(mapper["index_postings_decimal"], "0");
    assert_eq!(mapper["accounted_index_bytes_decimal"], "0");
    assert_manifest_integrity(&one_thread_bundle);
}

#[test]
fn accepts_stdin_once_and_rejects_stdin_reused_across_roles() {
    let temporary = TempDir::new().unwrap();
    let bundle = temporary.path().join("stdin-result");
    let result = run_with_stdin(
        single_args(
            Path::new("-"),
            &bundle,
            &["--profile", "retain-all", "--threads", "2"],
        ),
        b"@stdin-a\nAACGCTA\n+\nIIIIIII\n@stdin-b\nAACGCTA\n+\nIIIIIII\n",
    );
    assert_success(&result);
    let run = read_run(&bundle);
    assert_eq!(run["input"]["mode"], "single_end");
    assert_eq!(run["input"]["sources"][0]["format"], "fastq");
    assert_eq!(run["input"]["sources"][0]["records_decimal"], "2");
    assert_manifest_integrity(&bundle);

    let reused_bundle = temporary.path().join("stdin-reused-result");
    let intentionally_unread_stdin = vec![b'A'; 1024 * 1024];
    let reused = run_with_stdin(
        paired_args(
            &[PathBuf::from("-")],
            &[PathBuf::from("-")],
            &reused_bundle,
            &["--profile", "retain-all"],
        ),
        &intentionally_unread_stdin,
    );
    assert_failure(&reused, 4, "pair_physical_source_reuse");
    assert!(!reused_bundle.exists());
}

#[test]
fn active_destination_lease_fails_without_waiting_for_stdin_eof_or_creating_work() {
    let temporary = TempDir::new().unwrap();
    let bundle = temporary.path().join("result");
    let lock = temporary.path().join(".result.veritasm.lock");
    let lock_record = b"schema=veritasm-lock-v1\npid=999999\ndestination=result\n";
    fs::write(&lock, lock_record).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_veritasm"))
        .args(single_args(
            Path::new("-"),
            &bundle,
            &["--profile", "retain-all"],
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let held_stdin = child.stdin.take().unwrap();
    let mut exited = false;
    for _ in 0..500 {
        if child.try_wait().unwrap().is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !exited {
        child.kill().unwrap();
        panic!("assembler waited for stdin despite a preexisting destination lease");
    }
    drop(held_stdin);
    let result = child.wait_with_output().unwrap();
    assert_failure(&result, 7, "destination_locked");
    assert_eq!(fs::read(&lock).unwrap(), lock_record);
    assert!(!bundle.exists());
    let names = fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, [std::ffi::OsString::from(".result.veritasm.lock")]);
}

#[test]
fn two_complete_pipelines_share_one_early_lease_and_only_one_consumes_work() {
    let temporary = TempDir::new().unwrap();
    let competing_input = temporary.path().join("competing.fa");
    fs::write(&competing_input, b">competitor\nAACGCTA\n").unwrap();
    let bundle = temporary.path().join("result");
    let lock = temporary.path().join(".result.veritasm.lock");

    let mut first = Command::new(env!("CARGO_BIN_EXE_veritasm"))
        .args(single_args(
            Path::new("-"),
            &bundle,
            &["--profile", "retain-all", "--threads", "1"],
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first_stdin = first.stdin.take().unwrap();
    for attempt in 0..500 {
        if lock.exists() {
            break;
        }
        if let Some(status) = first.try_wait().unwrap() {
            panic!("first pipeline exited before holding its lease: {status}");
        }
        if attempt == 499 {
            first.kill().unwrap();
            panic!("first pipeline did not acquire its early lease");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let second = run(single_args(
        &competing_input,
        &bundle,
        &["--profile", "retain-all", "--threads", "1"],
    ));
    assert_failure(&second, 7, "destination_locked");
    assert!(!bundle.exists());

    first_stdin.write_all(b">winner\nAACGCTA\n").unwrap();
    drop(first_stdin);
    let first = first.wait_with_output().unwrap();
    assert_success(&first);
    assert_manifest_integrity(&bundle);
    assert!(!lock.exists());
}

fn padded_plain_fasta(total: usize) -> Vec<u8> {
    let record = b">raw-boundary\nAACGCTA\n";
    let mut bytes = vec![b'\n'; total.checked_sub(record.len()).unwrap()];
    bytes.extend_from_slice(record);
    bytes
}

#[test]
fn raw_transport_limit_boundaries_cover_file_stdin_plain_and_gzip() {
    const LIMIT: usize = 1 << 20;
    let temporary = TempDir::new().unwrap();
    for gzip in [false, true] {
        for stdin in [false, true] {
            for (label, total, succeeds) in [
                ("minus-one", LIMIT - 1, true),
                ("exact", LIMIT, true),
                ("plus-one", LIMIT + 1, false),
            ] {
                let logical = padded_plain_fasta(if gzip { 64 } else { total });
                let transport = if gzip {
                    gzip_bytes_with_exact_transport_length(&logical, total)
                } else {
                    logical
                };
                assert_eq!(transport.len(), total);
                let source = temporary
                    .path()
                    .join(format!("{label}-gzip-{gzip}-stdin-{stdin}.input"));
                if !stdin {
                    fs::write(&source, &transport).unwrap();
                }
                let bundle = temporary
                    .path()
                    .join(format!("{label}-gzip-{gzip}-stdin-{stdin}.result"));
                let arguments = single_args(
                    if stdin { Path::new("-") } else { &source },
                    &bundle,
                    &[
                        "--profile",
                        "retain-all",
                        "--threads",
                        "1",
                        "--max-raw-transport-bytes",
                        "1048576",
                    ],
                );
                let result = if stdin {
                    run_with_stdin(arguments, &transport)
                } else {
                    run(arguments)
                };
                if succeeds {
                    assert_success(&result);
                    let run = read_run(&bundle);
                    assert_eq!(
                        run["input"]["raw_transport_bytes_decimal"],
                        total.to_string()
                    );
                    assert_eq!(
                        run["parameters"]["limits"]["max_raw_transport_bytes_decimal"],
                        "1048576"
                    );
                    assert_eq!(run["schema_version"], "1.2");
                    assert_manifest_integrity(&bundle);
                } else {
                    assert_failure(&result, 3, "input_raw_transport_limit");
                    assert!(!bundle.exists());
                }
            }
        }
    }
}

#[test]
fn detects_concatenated_gzip_fastq_members_from_content() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.not-gz");
    let bundle = temporary.path().join("result");
    let mut transport = gzip_bytes(b"@read-a\nAACGCTA\n+\nIIIIIII\n");
    transport.extend(gzip_bytes(b"@read-b\nAACGCTA\n+\nIIIIIII\n"));
    fs::write(&input, transport).unwrap();

    let result = run(single_args(
        &input,
        &bundle,
        &[
            "--profile",
            "retain-all",
            "--threads",
            "2",
            "--max-gzip-members",
            "2",
        ],
    ));
    assert_success(&result);
    let run_document = read_run(&bundle);
    assert_eq!(run_document["input"]["sources"][0]["format"], "fastq");
    assert_eq!(run_document["input"]["sources"][0]["records_decimal"], "2");
    assert_eq!(run_document["input"]["fragments_decimal"], "2");
    assert_eq!(run_document["input"]["gzip_sources_decimal"], "1");
    assert_eq!(run_document["input"]["gzip_members_decimal"], "2");
    assert_eq!(
        run_document["parameters"]["limits"]["max_gzip_members_decimal"],
        "2"
    );
    assert_manifest_integrity(&bundle);

    let limited_bundle = temporary.path().join("limited-result");
    let limited = run(single_args(
        &input,
        &limited_bundle,
        &["--profile", "retain-all", "--max-gzip-members", "1"],
    ));
    assert_failure(&limited, 3, "input_gzip_member_limit");
    assert!(!limited_bundle.exists());
}

#[test]
fn detects_gzip_fasta_despite_a_fastq_filename() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("misleading.fastq");
    let bundle = temporary.path().join("result");
    write_gzip(&input, b">read-a\nAACGCTA\n>read-b\nAACGCTA\n");

    let result = run(single_args(
        &input,
        &bundle,
        &["--profile", "retain-all", "--threads", "1"],
    ));
    assert_success(&result);
    let run = read_run(&bundle);
    assert_eq!(run["input"]["sources"][0]["format"], "fasta");
    assert_eq!(run["input"]["sources"][0]["records_decimal"], "2");
    assert!(fs::read_to_string(bundle.join("unitigs.fasta"))
        .unwrap()
        .contains("AACGCTA"));
    assert_manifest_integrity(&bundle);
}

#[test]
fn accepts_strict_paired_plain_and_gzip_lanes() {
    let temporary = TempDir::new().unwrap();
    let r1a = temporary.path().join("lane-a-r1.fastq");
    let r2a = temporary.path().join("lane-a-r2.fastq");
    let r1b = temporary.path().join("lane-b-r1.data");
    let r2b = temporary.path().join("lane-b-r2.data");
    let bundle = temporary.path().join("result");
    fs::write(&r1a, b"@pair-a/1\nAACGCTA\n+\nIIIIIII\n").unwrap();
    fs::write(&r2a, b"@pair-a/2\nTACGGTA\n+\nIIIIIII\n").unwrap();
    write_gzip(&r1b, b"@pair-b 1:N:0:7\nGGTACCA\n+\nIIIIIII\n");
    write_gzip(&r2b, b"@pair-b 2:N:0:7\nTCCAAGG\n+\nIIIIIII\n");

    let result = run(paired_args(
        &[r1a, r1b],
        &[r2a, r2b],
        &bundle,
        &["--profile", "retain-all", "--threads", "2"],
    ));
    assert_success(&result);
    let run = read_run(&bundle);
    assert_eq!(run["input"]["mode"], "paired_end");
    assert_eq!(run["input"]["lane_count_decimal"], "2");
    assert_eq!(run["input"]["fragments_decimal"], "2");
    assert_eq!(run["input"]["reads_decimal"], "4");
    let labels = run["input"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|source| source["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            "lane-000000-R1",
            "lane-000000-R2",
            "lane-000001-R1",
            "lane-000001-R2"
        ]
    );
    assert_ne!(run["pair_audit"]["mode"], "not_paired_input");
    assert_manifest_integrity(&bundle);
}

#[test]
fn accepts_strict_paired_fasta() {
    let temporary = TempDir::new().unwrap();
    let read1 = temporary.path().join("reads-r1.fasta");
    let read2 = temporary.path().join("reads-r2.fasta");
    let bundle = temporary.path().join("paired-fasta-result");
    fs::write(&read1, b">pair-a/1\nAACGCTAGG\n>pair-b/1\nGGTACCAAT\n").unwrap();
    fs::write(&read2, b">pair-a/2\nCCTAGCGTT\n>pair-b/2\nATTGGTACC\n").unwrap();

    let result = run(paired_args(
        &[read1],
        &[read2],
        &bundle,
        &["--profile", "retain-all", "--threads", "2"],
    ));
    assert_success(&result);
    let run = read_run(&bundle);
    assert_eq!(run["input"]["mode"], "paired_end");
    assert_eq!(run["input"]["fragments_decimal"], "2");
    assert_eq!(run["input"]["reads_decimal"], "4");
    assert_eq!(run["input"]["sources"][0]["format"], "fasta");
    assert_eq!(run["input"]["sources"][1]["format"], "fasta");
    assert_manifest_integrity(&bundle);
}

#[test]
fn multiple_single_end_lanes_preserve_declared_order() {
    let temporary = TempDir::new().unwrap();
    let first = temporary.path().join("first.fa");
    let second = temporary.path().join("second.fq");
    let bundle = temporary.path().join("result");
    fs::write(&first, b">one\nAACGCTA\n").unwrap();
    fs::write(&second, b"@two\nGGTACCA\n+\nIIIIIII\n").unwrap();

    let result = run(vec![
        "assemble".to_owned(),
        "-U".to_owned(),
        first.display().to_string(),
        second.display().to_string(),
        "-o".to_owned(),
        bundle.display().to_string(),
        "-k".to_owned(),
        "5".to_owned(),
        "--profile".to_owned(),
        "retain-all".to_owned(),
        "--threads".to_owned(),
        "2".to_owned(),
    ]);
    assert_success(&result);
    let run = read_run(&bundle);
    assert_eq!(run["input"]["lane_count_decimal"], "2");
    assert_eq!(run["input"]["sources"][0]["label"], "lane-000000-S");
    assert_eq!(run["input"]["sources"][0]["format"], "fasta");
    assert_eq!(run["input"]["sources"][1]["label"], "lane-000001-S");
    assert_eq!(run["input"]["sources"][1]["format"], "fastq");
}

#[test]
fn complete_bundle_is_identical_across_thread_counts() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fastq");
    fs::write(
        &input,
        b"@a\nAACGCTAGG\n+\nIIIIIIIII\n@b\nAACGCTAGG\n+\nIIIIIIIII\n@c\nCCTAGCGTT\n+\nIIIIIIIII\n",
    )
    .unwrap();

    let mut baseline = None;
    for threads in ["1", "2", "4"] {
        let bundle = temporary.path().join(format!("result-{threads}"));
        let result = run(single_args(
            &input,
            &bundle,
            &["--profile", "retain-all", "--threads", threads],
        ));
        assert_success(&result);
        assert_manifest_integrity(&bundle);
        let files = collect_regular_files(&bundle);
        if let Some(expected) = &baseline {
            assert_eq!(&files, expected, "bundle differs at {threads} threads");
        } else {
            baseline = Some(files);
        }
    }
}

#[test]
fn paired_multilane_complete_bundle_is_identical_across_thread_counts() {
    let temporary = TempDir::new().unwrap();
    let r1a = temporary.path().join("lane-a-r1.fastq");
    let r2a = temporary.path().join("lane-a-r2.fastq");
    let r1b = temporary.path().join("lane-b-r1.fasta.gz-content");
    let r2b = temporary.path().join("lane-b-r2.fasta.gz-content");
    fs::write(
        &r1a,
        b"@pair-a/1\nAACGCTAGG\n+\nIIIIIIIII\n@pair-b/1\nGGTACCAAT\n+\nIIIIIIIII\n",
    )
    .unwrap();
    fs::write(
        &r2a,
        b"@pair-a/2\nCCTAGCGTT\n+\nIIIIIIIII\n@pair-b/2\nATTGGTACC\n+\nIIIIIIIII\n",
    )
    .unwrap();
    write_gzip(&r1b, b">pair-c/1\nTTACGGTCA\n>pair-d/1\nCGATTCGGA\n");
    write_gzip(&r2b, b">pair-c/2\nTGACCGTAA\n>pair-d/2\nTCCGAATCG\n");

    let read1 = [r1a, r1b];
    let read2 = [r2a, r2b];
    let mut baseline = None;
    for threads in ["1", "2", "4"] {
        let bundle = temporary.path().join(format!("paired-result-{threads}"));
        let result = run(paired_args(
            &read1,
            &read2,
            &bundle,
            &["--profile", "retain-all", "--threads", threads],
        ));
        assert_success(&result);
        let run = read_run(&bundle);
        assert_eq!(run["schema_version"], "1.2");
        assert_eq!(run["input"]["lane_count_decimal"], "2");
        assert_eq!(run["input"]["fragments_decimal"], "4");
        let lane_states = run["pair_audit"]["lane_state_counts"].as_array().unwrap();
        assert_eq!(lane_states.len(), 14);
        let expected_states = [
            "mate_ineligible",
            "mate_indeterminate_candidate_limit",
            "mate_unmapped",
            "mate_multiple_placement_groups",
            "same_linear_unitig",
            "endpoint_tie",
            "cross_unitig_observation",
        ];
        for lane in 0..2 {
            let rows =
                &lane_states[lane * expected_states.len()..(lane + 1) * expected_states.len()];
            assert_eq!(
                rows.iter()
                    .map(|row| row["state"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                expected_states
            );
            assert!(rows
                .iter()
                .all(|row| row["lane_ordinal"].as_u64().unwrap() == lane as u64));
            assert_eq!(
                rows.iter()
                    .map(|row| {
                        row["supplied_fragment_instances_decimal"]
                            .as_str()
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                    })
                    .sum::<u64>(),
                2
            );
        }
        let reconciliation = &run["pair_audit"]["reconciliation"];
        assert_eq!(reconciliation["lane_states_sum_to_lane_fragments"], true);
        assert_eq!(reconciliation["lane_state_sums_equal_global_states"], true);
        assert_eq!(
            reconciliation["lane_link_support_equals_lane_cross_unitig_state"],
            true
        );
        let pair_links = fs::read_to_string(bundle.join("pair_links.tsv")).unwrap();
        assert_eq!(
            pair_links.lines().next().unwrap(),
            "schema_version\tk\tlane_ordinal\tsegment_a\tend_a\tstrand_a\tend_distance_a\tmate_role_a\tsegment_b\tend_b\tstrand_b\tend_distance_b\tmate_role_b\tsupplied_fragment_instances\tstatus"
        );
        assert!(pair_links.lines().skip(1).all(|row| {
            let fields = row.split('\t').collect::<Vec<_>>();
            fields[0] == "1.1" && matches!(fields[2], "0" | "1")
        }));
        assert_manifest_integrity(&bundle);
        let files = collect_regular_files(&bundle);
        if let Some(expected) = &baseline {
            assert_eq!(
                &files, expected,
                "paired multi-lane bundle differs at {threads} threads"
            );
        } else {
            baseline = Some(files);
        }
    }
}

#[test]
fn rejects_destination_paths_with_dot_or_dotdot_components() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fa");
    fs::write(&input, b">read-a\nAACGCTA\n").unwrap();

    let dot_output = PathBuf::from("./dot-result");
    let dot = run_in(
        single_args(&input, &dot_output, &["--profile", "retain-all"]),
        temporary.path(),
    );
    assert_failure(&dot, 7, "destination_unsafe_path");
    assert!(!temporary.path().join("dot-result").exists());

    let dotdot_output = temporary
        .path()
        .join("unused-parent")
        .join("..")
        .join("dotdot-result");
    let dotdot = run(single_args(
        &input,
        &dotdot_output,
        &["--profile", "retain-all"],
    ));
    assert_failure(&dotdot, 7, "destination_unsafe_path");
    assert!(!temporary.path().join("dotdot-result").exists());
}

#[test]
fn reports_quality_and_iupac_window_exclusions_without_coercion() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fastq");
    let bundle = temporary.path().join("result");
    fs::write(&input, b"@mixed\nAACNTACGTACGT\n+\nII!IIIIII!III\n").unwrap();

    let result = run(single_args(
        &input,
        &bundle,
        &["--profile", "retain-all", "--min-base-quality", "20"],
    ));
    assert_success(&result);
    let run = read_run(&bundle);
    assert_eq!(run["windows"]["possible_decimal"], "9");
    assert_eq!(run["windows"]["accepted_decimal"], "1");
    assert_eq!(run["windows"]["ambiguity_only_decimal"], "1");
    assert_eq!(run["windows"]["quality_only_decimal"], "4");
    assert_eq!(run["windows"]["ambiguity_and_quality_decimal"], "3");
    assert_eq!(
        run["counting"]["observed_distinct_canonical_keys_decimal"],
        "1"
    );
}

#[test]
fn successful_empty_graph_is_explicit_and_has_zero_byte_fasta() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("singleton.fastq");
    let bundle = temporary.path().join("result");
    fs::write(&input, b"@one\nAACGCTA\n+\nIIIIIII\n").unwrap();

    let result = run(single_args(&input, &bundle, &["--threads", "1"]));
    assert_success(&result);
    let run = read_run(&bundle);
    assert_eq!(
        run["status"]["code"],
        "software_run_complete_no_unitigs_under_parameters"
    );
    assert_eq!(run["graph"]["unitigs_decimal"], "0");
    assert_eq!(fs::metadata(bundle.join("unitigs.fasta")).unwrap().len(), 0);
    assert_eq!(
        fs::read_to_string(bundle.join("assembly.gfa")).unwrap(),
        format!(
            "H\tVN:Z:1.0\tPN:Z:veritasm\tPV:Z:{}\tSC:Z:1.1\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_manifest_integrity(&bundle);
}

#[test]
fn rejects_malformed_fasta_fastq_and_nucleotide_input_without_partial_output() {
    let temporary = TempDir::new().unwrap();
    let cases: [(&str, &[u8], &str); 7] = [
        ("empty", b"", "input_empty"),
        ("unknown", b"not-fastx\n", "input_format"),
        ("empty-fasta", b">id\n", "input_fasta_structure"),
        (
            "short-quality",
            b"@id\nACGT\n+\nIII\n",
            "input_fastq_structure",
        ),
        (
            "plus-mismatch",
            b"@id\nACGT\n+other\nIIII\n",
            "input_fastq_structure",
        ),
        ("bad-base", b">id\nACZ\n", "input_nucleotide"),
        ("bad-quality", b"@id\nACG\n+\nII\0\n", "input_quality"),
    ];

    for (ordinal, (name, bytes, error_code)) in cases.iter().enumerate() {
        let input = temporary.path().join(format!("{ordinal}-{name}.data"));
        let bundle = temporary.path().join(format!("result-{ordinal}"));
        fs::write(&input, bytes).unwrap();
        let result = run(single_args(&input, &bundle, &["--profile", "retain-all"]));
        assert_failure(&result, 3, error_code);
        assert!(!bundle.exists(), "{name} left a partial destination");
    }
}

#[test]
fn cli_enforces_header_read_and_record_boundaries() {
    let temporary = TempDir::new().unwrap();

    for payload_length in [7_usize, 8, 9] {
        let input = temporary.path().join(format!("header-{payload_length}.fa"));
        let bundle = temporary
            .path()
            .join(format!("header-result-{payload_length}"));
        let mut bytes = vec![b'>'];
        bytes.extend(std::iter::repeat_n(b'x', payload_length));
        bytes.extend_from_slice(b"\nAACGT\n");
        fs::write(&input, bytes).unwrap();
        let result = run(single_args(
            &input,
            &bundle,
            &[
                "--profile",
                "retain-all",
                "--max-header-bytes",
                "8",
                "--threads",
                "1",
            ],
        ));
        if payload_length <= 8 {
            assert_success(&result);
            assert_manifest_integrity(&bundle);
        } else {
            assert_failure(&result, 3, "input_header");
            assert!(!bundle.exists());
        }
    }

    for read_length in [7_usize, 8, 9] {
        let input = temporary.path().join(format!("read-{read_length}.fa"));
        let bundle = temporary.path().join(format!("read-result-{read_length}"));
        let mut bytes = b">x\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'A', read_length));
        bytes.push(b'\n');
        fs::write(&input, bytes).unwrap();
        let result = run(single_args(
            &input,
            &bundle,
            &[
                "--profile",
                "retain-all",
                "--max-read-bases",
                "8",
                "--threads",
                "1",
            ],
        ));
        if read_length <= 8 {
            assert_success(&result);
            assert_manifest_integrity(&bundle);
        } else {
            assert_failure(&result, 3, "input_fasta_structure");
            assert!(!bundle.exists());
        }
    }

    for record_span in [11_usize, 12, 13] {
        let input = temporary.path().join(format!("record-{record_span}.fa"));
        let bundle = temporary
            .path()
            .join(format!("record-result-{record_span}"));
        let mut bytes = b">x\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'A', record_span - 4));
        bytes.push(b'\n');
        assert_eq!(bytes.len(), record_span);
        fs::write(&input, bytes).unwrap();
        let result = run(single_args(
            &input,
            &bundle,
            &[
                "--profile",
                "retain-all",
                "--max-record-bytes",
                "12",
                "--threads",
                "1",
            ],
        ));
        if record_span <= 12 {
            assert_success(&result);
            assert_manifest_integrity(&bundle);
        } else {
            assert_failure(&result, 3, "input_fasta_structure");
            assert!(!bundle.exists());
        }
    }
}

#[test]
fn rejects_corrupt_truncated_later_member_and_trailing_junk_gzip() {
    let temporary = TempDir::new().unwrap();
    let valid = gzip_bytes(b"@read\nAACGCTA\n+\nIIIIIII\n");
    let mut truncated = valid.clone();
    truncated.truncate(truncated.len() - 8);
    let mut corrupt = valid.clone();
    corrupt[12] ^= 0xff;
    let mut corrupt_later_member = valid.clone();
    let mut later_member = gzip_bytes(b"@later\nTTGCAAC\n+\nIIIIIII\n");
    let later_crc = later_member.len() - 8;
    later_member[later_crc] ^= 0x80;
    corrupt_later_member.extend_from_slice(&later_member);
    let mut trailing = valid;
    trailing.extend_from_slice(b"not another gzip member");

    for (ordinal, transport) in [corrupt, truncated, corrupt_later_member, trailing]
        .into_iter()
        .enumerate()
    {
        let input = temporary.path().join(format!("gzip-{ordinal}.data"));
        let bundle = temporary.path().join(format!("result-{ordinal}"));
        fs::write(&input, transport).unwrap();
        let result = run(single_args(&input, &bundle, &["--profile", "retain-all"]));
        assert_failure(&result, 3, "input_decompression");
        assert!(!bundle.exists());
    }
}

#[test]
fn rejects_invalid_paired_fasta_synchronization() {
    let temporary = TempDir::new().unwrap();
    let cases: [(&str, &[u8], &[u8], &str); 4] = [
        (
            "mismatched",
            b">alpha/1\nAACGT\n",
            b">beta/2\nAACGT\n",
            "pair_identity",
        ),
        (
            "reordered",
            b">alpha/1\nAACGT\n>beta/1\nAACGT\n",
            b">beta/2\nAACGT\n>alpha/2\nAACGT\n",
            "pair_identity",
        ),
        (
            "missing",
            b">first/1\nAACGT\n>second/1\nAACGT\n",
            b">first/2\nAACGT\n",
            "pair_order_or_cardinality",
        ),
        (
            "swapped",
            b">pair/2\nAACGT\n",
            b">pair/1\nAACGT\n",
            "pair_role",
        ),
    ];

    for (name, read1, read2, error_code) in cases {
        let r1 = temporary.path().join(format!("{name}-r1.fasta"));
        let r2 = temporary.path().join(format!("{name}-r2.fasta"));
        let bundle = temporary.path().join(format!("{name}-result"));
        write_pair(&r1, &r2, read1, read2);
        let result = run(paired_args(
            &[r1],
            &[r2],
            &bundle,
            &["--profile", "retain-all"],
        ));
        assert_failure(&result, 4, error_code);
        assert!(!bundle.exists());
    }
}

fn write_pair(path1: &Path, path2: &Path, read1: &[u8], read2: &[u8]) {
    fs::write(path1, read1).unwrap();
    fs::write(path2, read2).unwrap();
}

#[test]
fn rejects_mismatched_and_reordered_pair_identifiers() {
    let temporary = TempDir::new().unwrap();
    let cases: [(&str, &[u8], &[u8]); 2] = [
        (
            "mismatched",
            b"@alpha/1\nAACGT\n+\nIIIII\n",
            b"@beta/2\nAACGT\n+\nIIIII\n",
        ),
        (
            "reordered",
            b"@alpha/1\nAACGT\n+\nIIIII\n@beta/1\nAACGT\n+\nIIIII\n",
            b"@beta/2\nAACGT\n+\nIIIII\n@alpha/2\nAACGT\n+\nIIIII\n",
        ),
    ];
    for (ordinal, (name, r1_bytes, r2_bytes)) in cases.iter().enumerate() {
        let r1 = temporary.path().join(format!("{name}-r1.fastq"));
        let r2 = temporary.path().join(format!("{name}-r2.fastq"));
        let bundle = temporary.path().join(format!("result-{ordinal}"));
        write_pair(&r1, &r2, r1_bytes, r2_bytes);
        let result = run(paired_args(
            &[r1],
            &[r2],
            &bundle,
            &["--profile", "retain-all"],
        ));
        assert_failure(&result, 4, "pair_identity");
        assert!(!bundle.exists());
    }
}

#[test]
fn rejects_missing_mates_and_role_swaps() {
    let temporary = TempDir::new().unwrap();

    let missing_r1 = temporary.path().join("missing-r1.fastq");
    let missing_r2 = temporary.path().join("missing-r2.fastq");
    let missing_bundle = temporary.path().join("missing-result");
    write_pair(
        &missing_r1,
        &missing_r2,
        b"@first/1\nAACGT\n+\nIIIII\n@second/1\nAACGT\n+\nIIIII\n",
        b"@first/2\nAACGT\n+\nIIIII\n",
    );
    let missing = run(paired_args(
        &[missing_r1],
        &[missing_r2],
        &missing_bundle,
        &["--profile", "retain-all"],
    ));
    assert_failure(&missing, 4, "pair_order_or_cardinality");
    assert!(!missing_bundle.exists());

    let swapped_r1 = temporary.path().join("swapped-r1.fastq");
    let swapped_r2 = temporary.path().join("swapped-r2.fastq");
    let swapped_bundle = temporary.path().join("swapped-result");
    write_pair(
        &swapped_r1,
        &swapped_r2,
        b"@pair/2\nAACGT\n+\nIIIII\n",
        b"@pair/1\nAACGT\n+\nIIIII\n",
    );
    let swapped = run(paired_args(
        &[swapped_r1],
        &[swapped_r2],
        &swapped_bundle,
        &["--profile", "retain-all"],
    ));
    assert_failure(&swapped, 4, "pair_role");
    assert!(!swapped_bundle.exists());
}

#[test]
fn rejects_paired_format_mismatch_and_physical_source_reuse() {
    let temporary = TempDir::new().unwrap();
    let r1 = temporary.path().join("r1.fastx");
    let r2 = temporary.path().join("r2.fastx");
    fs::write(&r1, b">pair/1\nAACGT\n").unwrap();
    fs::write(&r2, b"@pair/2\nAACGT\n+\nIIIII\n").unwrap();
    let format_bundle = temporary.path().join("format-result");
    let format = run(paired_args(
        std::slice::from_ref(&r1),
        &[r2],
        &format_bundle,
        &["--profile", "retain-all"],
    ));
    assert_failure(&format, 4, "pair_format");
    assert!(!format_bundle.exists());

    let alias_bundle = temporary.path().join("alias-result");
    let alias = run(paired_args(
        std::slice::from_ref(&r1),
        std::slice::from_ref(&r1),
        &alias_bundle,
        &["--profile", "retain-all"],
    ));
    assert_failure(&alias, 4, "pair_physical_source_reuse");
    assert!(!alias_bundle.exists());

    let lane_bundle = temporary.path().join("lane-count-result");
    let lane_count = run(paired_args(
        &[r1.clone(), temporary.path().join("second-r1.fastx")],
        std::slice::from_ref(&r1),
        &lane_bundle,
        &["--profile", "retain-all"],
    ));
    assert_failure(&lane_count, 4, "pair_lane_count");
    assert!(!lane_bundle.exists());
}

#[test]
fn an_existing_destination_is_never_modified() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("corrupt.data");
    let bundle = temporary.path().join("existing-result");
    fs::write(&input, [0x1f, 0x8b, 0x08, 0x00, 0xff]).unwrap();
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("sentinel.txt"), b"keep this exactly\n").unwrap();
    fs::create_dir(bundle.join("nested")).unwrap();
    fs::write(bundle.join("nested/value.bin"), [0, 1, 2, 3]).unwrap();
    let before = collect_regular_files(&bundle);

    let result = run(single_args(&input, &bundle, &["--profile", "retain-all"]));
    assert_failure(&result, 7, "destination_existing");
    assert_eq!(collect_regular_files(&bundle), before);
}
