use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use tempfile::TempDir;

const SINGLE_FASTA: &[u8] =
    b">read-a\nAACGTTGCAACGTT\n>read-b\nAACGTTGCAACGTT\n>read-c\nCGTTGCAACGTTAA\n";

fn gzip_bytes(contents: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(contents).unwrap();
    encoder.finish().unwrap()
}

fn run(arguments: Vec<OsString>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_veritasm-multik"))
        .args(arguments)
        .output()
        .unwrap()
}

fn single_args(input: &Path, output: &Path, extra: &[&str]) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("-U"),
        input.as_os_str().to_owned(),
        OsString::from("-o"),
        output.as_os_str().to_owned(),
        OsString::from("-k"),
        OsString::from("3,5"),
    ];
    arguments.extend(extra.iter().map(OsString::from));
    arguments
}

fn paired_args(
    read1: &[PathBuf],
    read2: &[PathBuf],
    output: &Path,
    extra: &[&str],
) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("-1")];
    arguments.extend(read1.iter().map(|path| path.as_os_str().to_owned()));
    arguments.push(OsString::from("-2"));
    arguments.extend(read2.iter().map(|path| path.as_os_str().to_owned()));
    arguments.push(OsString::from("-o"));
    arguments.push(output.as_os_str().to_owned());
    arguments.push(OsString::from("-k"));
    arguments.push(OsString::from("3,5"));
    arguments.extend(extra.iter().map(OsString::from));
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
    assert!(
        output.stdout.is_empty(),
        "successful bundle command wrote unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
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
        output.stdout.is_empty(),
        "failed bundle command wrote unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let expected = format!("error[{error_code}]:");
    assert!(
        stderr.lines().any(|line| line.starts_with(&expected)),
        "expected {expected:?} in stderr: {stderr}"
    );
    assert_eq!(
        stderr.matches("error[").count(),
        1,
        "failure emitted more than one machine diagnostic: {stderr}"
    );
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
    let mut output = String::with_capacity(64);
    for byte in digest {
        write!(&mut output, "{byte:02x}").unwrap();
    }
    output
}

fn assert_complete_bundle(bundle: &Path) -> BTreeMap<String, Vec<u8>> {
    veritasm::bundle::verify_bundle_manifest(bundle).unwrap();
    let files = collect_regular_files(bundle);
    for required in [
        "adjacency_evidence.tsv",
        "assembly.gfa",
        "contigs.fasta",
        "manifest.sha256",
        "pair_evidence.jsonl",
        "pair_evidence.tsv",
        "profile_decisions.tsv",
        "report.html",
        "run.json",
        "schema/multik_run.schema.json",
        "segment_evidence.tsv",
        "segments.fasta",
        "transition_decisions.tsv",
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
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "manifest digest is not lowercase hexadecimal: {digest:?}"
        );
        assert!(!Path::new(relative).is_absolute());
        assert!(Path::new(relative)
            .components()
            .all(|component| matches!(component, Component::Normal(_))));
        if let Some(prior) = previous {
            assert!(prior < relative, "manifest paths are not strictly sorted");
        }
        previous = Some(relative);
        assert!(
            listed.insert(relative.to_owned()),
            "duplicate manifest path"
        );
        let bytes = files
            .get(relative)
            .unwrap_or_else(|| panic!("manifest names missing file {relative:?}"));
        assert_eq!(
            lower_sha256(bytes),
            digest,
            "digest mismatch for {relative}"
        );
    }
    let expected = files
        .keys()
        .filter(|relative| relative.as_str() != "manifest.sha256")
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        listed, expected,
        "manifest does not bind the exact file tree"
    );

    let run: Value = serde_json::from_slice(&files["run.json"]).unwrap();
    assert_eq!(run["schema"], "veritasm-experimental-multik-run-v2");
    assert_eq!(run["status"], "experimental");
    assert_eq!(run["qualification"], "unqualified");
    assert_eq!(run["intended_use"], "research_use_only");
    assert_pair_evidence_artifacts(&run, &files);
    let schema: Value = serde_json::from_slice(&files["schema/multik_run.schema.json"]).unwrap();
    assert_eq!(
        schema["properties"]["schema"]["const"],
        "veritasm-experimental-multik-run-v2"
    );
    let report = String::from_utf8(files["report.html"].clone()).unwrap();
    assert!(report.starts_with("<!doctype html>"));
    assert!(!report.contains("src=\"http"));
    assert!(!report.contains("href=\"http"));
    assert!(files["assembly.gfa"].starts_with(b"H\tVN:Z:1.0"));
    files
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn assert_pair_evidence_artifacts(run: &Value, files: &BTreeMap<String, Vec<u8>>) {
    let (state, placement_domain, expects_document) = match run["input_mode"].as_str() {
        Some("single_end") => ("not_applicable_single_end", "not_applicable", false),
        Some("paired_end") => (
            "authenticated_linear_unitig_only_unqualified",
            "linear_unitig_only",
            true,
        ),
        mode => panic!("unexpected multi-k input mode: {mode:?}"),
    };
    assert_eq!(run["pair_evidence"], state);
    assert_eq!(run["quality_correction"], "disabled_unqualified");
    let children = run["children"].as_array().unwrap();
    assert_eq!(
        run["totals"]["children"].as_u64().unwrap(),
        children.len() as u64
    );

    let jsonl = std::str::from_utf8(&files["pair_evidence.jsonl"]).unwrap();
    assert!(jsonl.ends_with('\n'));
    let documents = jsonl
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(documents.len(), children.len());

    let tsv = std::str::from_utf8(&files["pair_evidence.tsv"]).unwrap();
    assert!(tsv.ends_with('\n'));
    let mut tsv_lines = tsv.lines();
    let header = tsv_lines.next().unwrap().split('\t').collect::<Vec<_>>();
    assert_eq!(header.first(), Some(&"k"));
    assert_eq!(header.get(1), Some(&"state"));
    assert_eq!(header.get(2), Some(&"placement_domain"));
    assert_eq!(header.get(17), Some(&"sequence_modified"));
    let tsv_rows = tsv_lines
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(tsv_rows.len(), children.len());

    for ((child, document), row) in children.iter().zip(&documents).zip(&tsv_rows) {
        let pair = &child["pair_evidence"];
        let child_k = child["k"].as_u64().unwrap();
        let report_root = pair["report_root"].as_str().unwrap();
        assert_eq!(pair["state"], state);
        assert_eq!(pair["placement_domain"], placement_domain);
        assert_eq!(pair["changes_sequence_or_graph"], false);
        assert!(is_lower_hex_digest(report_root));
        assert_eq!(document["k"], child_k);
        assert_eq!(document["state"], state);
        assert_eq!(document["report_root"], report_root);
        assert_eq!(document["evidence"].is_object(), expects_document);
        assert_eq!(document["evidence"].is_null(), !expects_document);

        assert_eq!(row.len(), header.len());
        assert_eq!(row[0].parse::<u64>().unwrap(), child_k);
        assert_eq!(row[1], state);
        assert_eq!(row[2], placement_domain);
        assert_eq!(row[3], report_root);
        assert_eq!(row[17], "false");
        assert_eq!(
            &row[18..],
            ["experimental", "unqualified", "research_use_only"]
        );

        if expects_document {
            assert!(pair["exact_pair_graph_root"]
                .as_str()
                .is_some_and(is_lower_hex_digest));
            assert!(pair["placement_producer_root"]
                .as_str()
                .is_some_and(is_lower_hex_digest));
            assert!(pair["authenticated_path_result_root"]
                .as_str()
                .is_some_and(is_lower_hex_digest));
            assert_eq!(
                pair["calibration_fragments"].as_u64().unwrap()
                    + pair["replay_fragments"].as_u64().unwrap(),
                pair["authenticated_fragments"].as_u64().unwrap()
            );
        } else {
            assert!(pair["exact_pair_graph_root"].is_null());
            assert!(pair["placement_producer_root"].is_null());
            assert!(pair["authenticated_path_result_root"].is_null());
            assert_eq!(pair["authenticated_fragments"], 0);
            assert_eq!(pair["lane_models"].as_array().unwrap().len(), 0);
        }
    }
}

fn read_run(bundle: &Path) -> Value {
    serde_json::from_slice(&fs::read(bundle.join("run.json")).unwrap()).unwrap()
}

fn fasta_sequences(bytes: &[u8]) -> Vec<Vec<u8>> {
    let text = std::str::from_utf8(bytes).unwrap();
    let mut sequences = Vec::new();
    let mut current = None::<Vec<u8>>;
    for line in text.lines() {
        if line.starts_with('>') {
            if let Some(sequence) = current.replace(Vec::new()) {
                sequences.push(sequence);
            }
        } else if !line.is_empty() {
            current
                .as_mut()
                .expect("FASTA sequence appeared before a header")
                .extend_from_slice(line.as_bytes());
        }
    }
    if let Some(sequence) = current {
        sequences.push(sequence);
    }
    sequences.sort();
    sequences
}

fn lock_path(output: &Path) -> PathBuf {
    let name = output.file_name().unwrap().to_string_lossy();
    output.with_file_name(format!(".{name}.veritasm.lock"))
}

fn assert_no_failed_run_residue(parent: &Path, output: &Path) {
    assert!(!output.exists(), "failed run published its destination");
    assert!(
        !lock_path(output).exists(),
        "failed run leaked its lease file"
    );
    for entry in fs::read_dir(parent).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".veritasm-multik-work-")
                && !name.starts_with(".veritasm-multik-stage-"),
            "failed run leaked owned temporary entry {name:?}"
        );
    }
}

#[test]
fn single_end_plain_and_content_gzip_are_valid_and_sequence_equivalent() {
    let temporary = TempDir::new().unwrap();
    let plain = temporary.path().join("reads.fasta");
    let compressed_without_gzip_suffix = temporary.path().join("reads.transport");
    fs::write(&plain, SINGLE_FASTA).unwrap();
    fs::write(&compressed_without_gzip_suffix, gzip_bytes(SINGLE_FASTA)).unwrap();

    let plain_bundle = temporary.path().join("plain-result");
    let gzip_bundle = temporary.path().join("gzip-result");
    let plain_result = run(single_args(
        &plain,
        &plain_bundle,
        &["--retention", "retain-all"],
    ));
    let gzip_result = run(single_args(
        &compressed_without_gzip_suffix,
        &gzip_bundle,
        &["--retention", "retain-all"],
    ));
    assert_success(&plain_result);
    assert_success(&gzip_result);
    let plain_files = assert_complete_bundle(&plain_bundle);
    let gzip_files = assert_complete_bundle(&gzip_bundle);

    assert_eq!(read_run(&plain_bundle)["input_mode"], "single_end");
    assert_eq!(read_run(&gzip_bundle)["input_mode"], "single_end");
    let plain_sequences = fasta_sequences(&plain_files["segments.fasta"]);
    let gzip_sequences = fasta_sequences(&gzip_files["segments.fasta"]);
    assert!(!plain_sequences.is_empty());
    assert_eq!(plain_sequences, gzip_sequences);
}

#[test]
fn synchronized_paired_multilane_fastq_succeeds_with_explicit_pair_scope() {
    let temporary = TempDir::new().unwrap();
    let lane1_r1 = temporary.path().join("lane1-r1.fastq");
    let lane1_r2 = temporary.path().join("lane1-r2.fastq");
    let lane2_r1 = temporary.path().join("lane2-r1.fastq");
    let lane2_r2 = temporary.path().join("lane2-r2.fastq");
    fs::write(
        &lane1_r1,
        b"@lane1-a/1\nAACGTTGCAACGTT\n+\nIIIIIIIIIIIIII\n@lane1-b/1\nCGTTGCAACGTTAA\n+\nIIIIIIIIIIIIII\n",
    )
    .unwrap();
    fs::write(
        &lane1_r2,
        b"@lane1-a/2\nTTGCAACGTTGCAA\n+\nIIIIIIIIIIIIII\n@lane1-b/2\nAACGTTGCAACGTT\n+\nIIIIIIIIIIIIII\n",
    )
    .unwrap();
    fs::write(
        &lane2_r1,
        gzip_bytes(
            b"@lane2-a/1\nGCAACGTTAACGTT\n+\nIIIIIIIIIIIIII\n@lane2-b/1\nAACGTTGCAACGTT\n+\nIIIIIIIIIIIIII\n",
        ),
    )
    .unwrap();
    fs::write(
        &lane2_r2,
        gzip_bytes(
            b"@lane2-a/2\nAACGTTAACGTTGC\n+\nIIIIIIIIIIIIII\n@lane2-b/2\nCGTTGCAACGTTAA\n+\nIIIIIIIIIIIIII\n",
        ),
    )
    .unwrap();

    let bundle = temporary.path().join("paired-result");
    let result = run(paired_args(
        &[lane1_r1, lane2_r1],
        &[lane1_r2, lane2_r2],
        &bundle,
        &["--retention", "retain-all"],
    ));
    assert_success(&result);
    assert_complete_bundle(&bundle);
    let run = read_run(&bundle);
    assert_eq!(run["input_mode"], "paired_end");
    assert_eq!(
        run["pair_evidence"],
        "authenticated_linear_unitig_only_unqualified"
    );
    for child in run["children"].as_array().unwrap() {
        assert_eq!(
            child["pair_evidence"]["authenticated_fragments"], 4,
            "every supplied paired fragment must enter the authenticated ledger"
        );
    }
}

#[test]
fn paired_input_above_optional_evidence_cap_still_assembles_with_explicit_unavailability() {
    let temporary = TempDir::new().unwrap();
    let read1 = temporary.path().join("over-cap-r1.fastq");
    let read2 = temporary.path().join("over-cap-r2.fastq");
    let mut r1_bytes = Vec::new();
    let mut r2_bytes = Vec::new();
    for ordinal in 0..8_193_u64 {
        writeln!(r1_bytes, "@pair-{ordinal}/1\nAACGTT\n+\nIIIIII").unwrap();
        writeln!(r2_bytes, "@pair-{ordinal}/2\nAACGTT\n+\nIIIIII").unwrap();
    }
    fs::write(&read1, r1_bytes).unwrap();
    fs::write(&read2, r2_bytes).unwrap();

    let bundle = temporary.path().join("over-cap-result");
    let result = run(vec![
        OsString::from("-1"),
        read1.as_os_str().to_owned(),
        OsString::from("-2"),
        read2.as_os_str().to_owned(),
        OsString::from("-o"),
        bundle.as_os_str().to_owned(),
        OsString::from("-k"),
        OsString::from("3"),
    ]);
    assert_success(&result);
    veritasm::bundle::verify_bundle_manifest(&bundle).unwrap();
    let run = read_run(&bundle);
    assert_eq!(run["input_mode"], "paired_end");
    assert_eq!(run["pair_evidence"], "unavailable_resource_limit");
    for child in run["children"].as_array().unwrap() {
        let pair = &child["pair_evidence"];
        assert_eq!(pair["state"], "unavailable_resource_limit");
        assert_eq!(pair["placement_domain"], "unavailable_resource_limit");
        assert_eq!(pair["supplied_fragments"], 8_193);
        assert_eq!(pair["configured_fragment_limit"], 8_192);
        assert_eq!(pair["authenticated_fragments"], 0);
        assert_eq!(pair["changes_sequence_or_graph"], false);
    }
    assert!(!fasta_sequences(&fs::read(bundle.join("segments.fasta")).unwrap()).is_empty());
}

#[test]
fn paired_input_rejects_identity_order_cardinality_and_corrupt_gzip_atomically() {
    struct FailureCase {
        name: &'static str,
        read1: &'static [u8],
        read2: &'static [u8],
        exit_code: i32,
        error_code: &'static str,
        corrupt_gzip: bool,
    }

    let temporary = TempDir::new().unwrap();
    let cases = [
        FailureCase {
            name: "identity",
            read1: b"@alpha/1\nAACGTT\n+\nIIIIII\n",
            read2: b"@beta/2\nAACGTT\n+\nIIIIII\n",
            exit_code: 4,
            error_code: "pair_identity",
            corrupt_gzip: false,
        },
        FailureCase {
            name: "order",
            read1: b"@alpha/1\nAACGTT\n+\nIIIIII\n@beta/1\nCGTTGC\n+\nIIIIII\n",
            read2: b"@beta/2\nCGTTGC\n+\nIIIIII\n@alpha/2\nAACGTT\n+\nIIIIII\n",
            exit_code: 4,
            error_code: "pair_identity",
            corrupt_gzip: false,
        },
        FailureCase {
            name: "cardinality",
            read1: b"@alpha/1\nAACGTT\n+\nIIIIII\n@beta/1\nCGTTGC\n+\nIIIIII\n",
            read2: b"@alpha/2\nAACGTT\n+\nIIIIII\n",
            exit_code: 4,
            error_code: "pair_order_or_cardinality",
            corrupt_gzip: false,
        },
        FailureCase {
            name: "gzip",
            read1: b"@alpha/1\nAACGTT\n+\nIIIIII\n",
            read2: b"@alpha/2\nAACGTT\n+\nIIIIII\n",
            exit_code: 3,
            error_code: "input_decompression",
            corrupt_gzip: true,
        },
    ];

    for case in cases {
        let r1 = temporary.path().join(format!("{}-r1.fastq", case.name));
        let r2 = temporary.path().join(format!("{}-r2.transport", case.name));
        let bundle = temporary.path().join(format!("{}-result", case.name));
        fs::write(&r1, case.read1).unwrap();
        if case.corrupt_gzip {
            let mut transport = gzip_bytes(case.read2);
            transport.truncate(transport.len() - 5);
            fs::write(&r2, transport).unwrap();
        } else {
            fs::write(&r2, case.read2).unwrap();
        }
        let result = run(paired_args(
            &[r1],
            &[r2],
            &bundle,
            &["--retention", "retain-all"],
        ));
        assert_failure(&result, case.exit_code, case.error_code);
        assert_no_failed_run_residue(temporary.path(), &bundle);
    }
}

#[test]
fn destination_lease_and_existing_result_are_fail_closed() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fasta");
    fs::write(&input, SINGLE_FASTA).unwrap();

    let existing = temporary.path().join("existing-result");
    fs::create_dir(&existing).unwrap();
    fs::create_dir(existing.join("nested")).unwrap();
    fs::write(existing.join("sentinel"), b"keep exactly\n").unwrap();
    fs::write(existing.join("nested/record"), b"also keep\n").unwrap();
    let before = collect_regular_files(&existing);
    let result = run(single_args(
        &input,
        &existing,
        &["--retention", "retain-all"],
    ));
    assert_failure(&result, 7, "destination_existing");
    assert_eq!(collect_regular_files(&existing), before);
    assert!(!lock_path(&existing).exists());

    let locked = temporary.path().join("locked-result");
    let lock = lock_path(&locked);
    let lock_record = b"schema=veritasm-lock-v1\npid=999999\ndestination=locked-result\n";
    fs::write(&lock, lock_record).unwrap();
    let never_opened = temporary.path().join("does-not-exist.fasta");
    let locked_result = run(single_args(
        &never_opened,
        &locked,
        &["--retention", "retain-all"],
    ));
    assert_failure(&locked_result, 7, "destination_locked");
    assert_eq!(fs::read(&lock).unwrap(), lock_record);
    assert!(!locked.exists());
    assert!(
        fs::read_dir(temporary.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".veritasm-multik-work-")),
        "lease rejection must precede private work creation"
    );
}

#[test]
fn concurrent_writers_share_one_early_destination_lease() {
    let temporary = TempDir::new().unwrap();
    let competing_input = temporary.path().join("competing.fasta");
    fs::write(&competing_input, SINGLE_FASTA).unwrap();
    let bundle = temporary.path().join("contended-result");
    let lock = lock_path(&bundle);

    let mut first = Command::new(env!("CARGO_BIN_EXE_veritasm-multik"))
        .args(single_args(
            Path::new("-"),
            &bundle,
            &["--retention", "retain-all"],
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
            panic!("first writer exited before acquiring its lease: {status}");
        }
        if attempt == 499 {
            first.kill().unwrap();
            panic!("first writer did not acquire its early destination lease");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let second = run(single_args(
        &competing_input,
        &bundle,
        &["--retention", "retain-all"],
    ));
    assert_failure(&second, 7, "destination_locked");
    assert!(!bundle.exists());

    first_stdin.write_all(SINGLE_FASTA).unwrap();
    drop(first_stdin);
    let first = first.wait_with_output().unwrap();
    assert_success(&first);
    assert_complete_bundle(&bundle);
    assert!(!lock.exists());
    assert!(
        fs::read_dir(temporary.path()).unwrap().all(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            !name.starts_with(".veritasm-multik-work-")
                && !name.starts_with(".veritasm-multik-stage-")
        }),
        "completed contention test leaked owned temporary state"
    );
}

#[test]
fn default_and_explicit_serial_execution_are_byte_deterministic_and_parallel_is_rejected() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fasta");
    fs::write(&input, SINGLE_FASTA).unwrap();
    let default_bundle = temporary.path().join("default-threads");
    let explicit_bundle = temporary.path().join("explicit-one");

    let default = run(single_args(
        &input,
        &default_bundle,
        &["--retention", "retain-all"],
    ));
    let explicit = run(single_args(
        &input,
        &explicit_bundle,
        &["--retention", "retain-all", "--threads", "1"],
    ));
    assert_success(&default);
    assert_success(&explicit);
    let default_files = assert_complete_bundle(&default_bundle);
    let explicit_files = assert_complete_bundle(&explicit_bundle);
    assert_eq!(default_files, explicit_files);
    let default_run = read_run(&default_bundle);
    assert_eq!(default_run["execution"]["execution_threads"], 1);
    assert_eq!(default_run["profile"], "diversity_preserving");
    assert_eq!(default_run["support_unit"], "supplied_fragment_instance");
    for child in default_run["children"].as_array().unwrap() {
        assert_eq!(child["retention_rule"], "retain_all");
        assert!(child["retention_minimum_support"].is_null());
    }

    for threads in ["0", "2", "4"] {
        let output = temporary.path().join(format!("rejected-{threads}"));
        let missing_input = temporary.path().join(format!("missing-{threads}.fasta"));
        let result = run(single_args(
            &missing_input,
            &output,
            &["--retention", "retain-all", "--threads", threads],
        ));
        assert_failure(&result, 2, "configuration_invalid_limit");
        assert_no_failed_run_residue(temporary.path(), &output);
    }
}

#[test]
fn retention_support_and_presentation_arguments_are_recorded_and_conflicts_are_typed() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fasta");
    fs::write(&input, SINGLE_FASTA).unwrap();
    let bundle = temporary.path().join("thresholded-result");
    let result = run(single_args(
        &input,
        &bundle,
        &[
            "--retention",
            "inclusive-support",
            "--min-support",
            "2",
            "--support-unit",
            "accepted-window-occurrence",
            "--output-profile",
            "exact-agreement-consensus",
        ],
    ));
    assert_success(&result);
    let files = assert_complete_bundle(&bundle);
    let run_record: Value = serde_json::from_slice(&files["run.json"]).unwrap();
    assert_eq!(run_record["profile"], "exact_agreement_consensus");
    assert_eq!(run_record["support_unit"], "accepted_window_occurrence");
    assert_eq!(run_record["children"].as_array().unwrap().len(), 2);
    for (child, expected_k) in run_record["children"]
        .as_array()
        .unwrap()
        .iter()
        .zip([3, 5])
    {
        assert_eq!(child["k"], expected_k);
        assert_eq!(child["retention_rule"], "inclusive_support");
        assert_eq!(child["retention_minimum_support"], 2);
    }
    assert_eq!(
        fasta_sequences(&files["contigs.fasta"]).len() as u64,
        run_record["totals"]["exact_agreement_presentations"]
            .as_u64()
            .unwrap()
    );

    let invalid_cases: [(&str, &[&str], &str); 4] = [
        (
            "retain-all-threshold",
            &["--retention", "retain-all", "--min-support", "2"],
            "configuration_profile_conflict",
        ),
        (
            "missing-threshold",
            &["--retention", "inclusive-support"],
            "configuration_invalid_support",
        ),
        (
            "unsorted-k",
            &["--retention", "retain-all", "-k", "5,3"],
            "configuration_invalid_k",
        ),
        (
            "duplicate-k",
            &["--retention", "retain-all", "-k", "3,3"],
            "configuration_invalid_k",
        ),
    ];
    for (name, extra, error_code) in invalid_cases {
        let output = temporary.path().join(format!("invalid-{name}"));
        let missing_input = temporary.path().join(format!("missing-{name}.fasta"));
        let result = run(single_args(&missing_input, &output, extra));
        assert_failure(&result, 2, error_code);
        assert_no_failed_run_residue(temporary.path(), &output);
    }
}

#[test]
fn resource_boundaries_are_typed_and_never_publish_partial_bundles() {
    let temporary = TempDir::new().unwrap();
    let input = temporary.path().join("reads.fasta");
    fs::write(&input, SINGLE_FASTA).unwrap();

    let exact_minimum = temporary.path().join("exact-minimum-byte-limits");
    let exact_result = run(single_args(
        &input,
        &exact_minimum,
        &[
            "--retention",
            "retain-all",
            "--max-spool-bytes",
            "1048576",
            "--max-temp-bytes",
            "1048576",
            "--max-staged-output-bytes",
            "1048576",
        ],
    ));
    assert_success(&exact_result);
    assert_complete_bundle(&exact_minimum);
    let exact_run = read_run(&exact_minimum);
    assert_eq!(
        exact_run["execution"]["max_aggregate_temp_bytes"],
        1_048_576
    );
    assert!(
        exact_run["execution"]["max_final_staging_bytes"]
            .as_u64()
            .unwrap()
            <= 1_048_576
    );

    let failures: [(&str, &[&str], i32, &str); 5] = [
        (
            "temp-below-domain",
            &["--retention", "retain-all", "--max-temp-bytes", "1048575"],
            2,
            "configuration_invalid_limit",
        ),
        (
            "memory-below-domain",
            &[
                "--retention",
                "retain-all",
                "--memory-budget-bytes",
                "33554431",
            ],
            2,
            "configuration_invalid_limit",
        ),
        (
            "child-key-cap",
            &["--retention", "retain-all", "--max-child-keys", "1"],
            5,
            "resource_retained_keys",
        ),
        (
            "child-snapshot-cap",
            &[
                "--retention",
                "retain-all",
                "--max-child-snapshot-bytes",
                "1",
            ],
            5,
            "resource_output_bytes",
        ),
        (
            "loaded-snapshot-cap",
            &[
                "--retention",
                "retain-all",
                "--max-loaded-snapshot-bytes",
                "1",
            ],
            5,
            "resource_memory",
        ),
    ];
    for (name, extra, exit_code, error_code) in failures {
        let output = temporary.path().join(format!("failed-{name}"));
        let result = run(single_args(&input, &output, extra));
        assert_failure(&result, exit_code, error_code);
        assert_no_failed_run_residue(temporary.path(), &output);
    }
}
