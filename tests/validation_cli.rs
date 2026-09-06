use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use veritasm::validation::{
    validate_evaluation_result, DatasetBindingMode, EvaluationResult, JunctionEvidenceMode,
};

fn simulate(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_veritasm-simulate"))
        .args(arguments)
        .output()
        .unwrap()
}

fn evaluate(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_veritasm-evaluate"))
        .args(arguments)
        .output()
        .unwrap()
}

fn assemble(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_veritasm"))
        .args(arguments)
        .output()
        .unwrap()
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

fn assert_safe_diagnostic(output: &Output, exit_code: i32, diagnostic_code: &str) {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(stderr.starts_with(&format!("error[{diagnostic_code}]: ")));
    assert_eq!(stderr.bytes().filter(|byte| *byte == b'\n').count(), 1);
    assert!(stderr.ends_with('\n'));
    assert!(stderr[..stderr.len() - 1]
        .chars()
        .all(|character| !character.is_control() && !matches!(character, '\u{2028}' | '\u{2029}')));
}

fn generate(root: &Path, case: &str) -> PathBuf {
    let output = root.join(case);
    let result = simulate(&[
        "--out",
        output.to_str().unwrap(),
        "--case",
        case,
        "--fragments",
        "40",
        "--truth-length",
        "400",
        "--read-length",
        "30",
        "--insert-length",
        "80",
    ]);
    assert_success(&result);
    output
}

fn fasta_sequences(path: &Path) -> Vec<Vec<u8>> {
    let text = fs::read_to_string(path).unwrap();
    let mut records = Vec::<Vec<u8>>::new();
    for line in text.lines() {
        if line.starts_with('>') {
            records.push(Vec::new());
        } else {
            records
                .last_mut()
                .unwrap()
                .extend_from_slice(line.as_bytes());
        }
    }
    records
}

fn write_fasta(path: &Path, id: &str, sequence: &[u8]) {
    let mut text = format!(">{id}\n");
    for chunk in sequence.chunks(80) {
        text.push_str(std::str::from_utf8(chunk).unwrap());
        text.push('\n');
    }
    fs::write(path, text).unwrap();
}

fn write_single_base_records(path: &Path, records: usize) {
    let mut text = String::with_capacity(records * 12);
    for ordinal in 0..records {
        writeln!(&mut text, ">c{ordinal}\nA").unwrap();
    }
    fs::write(path, text).unwrap();
}

fn write_repeated_sequence_records(path: &Path, records: usize, sequence: &[u8]) {
    let mut text = String::with_capacity(records * (sequence.len() + 12));
    let sequence = std::str::from_utf8(sequence).unwrap();
    for ordinal in 0..records {
        writeln!(&mut text, ">c{ordinal}\n{sequence}").unwrap();
    }
    fs::write(path, text).unwrap();
}

fn write_json_value(path: &Path, value: &Value) {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
}

fn file_record_mut<'a>(manifest: &'a mut Value, relative: &str) -> &'a mut Value {
    manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["path"] == relative)
        .unwrap()
}

fn rewrite_declared_artifact(dataset: &Path, manifest: &mut Value, relative: &str, bytes: &[u8]) {
    fs::write(dataset.join(relative), bytes).unwrap();
    let record = file_record_mut(manifest, relative);
    record["bytes_decimal"] = Value::String(bytes.len().to_string());
    record["sha256"] = Value::String(lower_sha256(bytes));
}

fn refresh_dataset_checksum_manifest(dataset: &Path, manifest: &Value) -> String {
    let mut rows = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                record["path"].as_str().unwrap().to_owned(),
                record["sha256"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    rows.push((
        "dataset.json".to_owned(),
        lower_sha256(&fs::read(dataset.join("dataset.json")).unwrap()),
    ));
    rows.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut bytes = String::new();
    for (path, digest) in rows {
        writeln!(&mut bytes, "{digest}  {path}").unwrap();
    }
    fs::write(dataset.join("manifest.sha256"), bytes.as_bytes()).unwrap();
    lower_sha256(bytes.as_bytes())
}

fn assert_invalid_dataset_manifest(
    dataset: &Path,
    assembly: &Path,
    output: &Path,
    manifest: &Value,
    expected: &str,
) {
    write_json_value(&dataset.join("dataset.json"), manifest);
    refresh_dataset_checksum_manifest(dataset, manifest);
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        output.to_str().unwrap(),
    ]);
    assert!(!evaluation.status.success());
    let stderr = String::from_utf8_lossy(&evaluation.stderr);
    assert!(
        stderr.contains(expected),
        "expected {expected:?} in stderr: {stderr}"
    );
    assert!(!output.exists());
}

fn result(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path.join("result.json")).unwrap()).unwrap()
}

fn lower_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::new();
    for byte in digest {
        write!(&mut output, "{byte:02x}").unwrap();
    }
    output
}

fn assert_manifest(bundle: &Path) {
    let text = fs::read_to_string(bundle.join("manifest.sha256")).unwrap();
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        let (digest, relative) = line.split_once("  ").unwrap();
        assert_eq!(
            digest,
            lower_sha256(&fs::read(bundle.join(relative)).unwrap())
        );
        assert!(rows.insert(relative, digest).is_none());
    }
    assert_eq!(
        rows.keys().copied().collect::<Vec<_>>(),
        ["alignments.tsv", "junctions.tsv", "result.json"]
    );
}

#[test]
fn generator_v4_covers_all_built_in_cases_and_ledger_shapes() {
    let temporary = TempDir::new().unwrap();
    for case in [
        "linear-se",
        "linear-pe",
        "circular-pe",
        "mixture-pe",
        "error-pe",
        "qc-censoring-control",
    ] {
        let dataset = generate(temporary.path(), case);
        let manifest: Value =
            serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
        assert_eq!(manifest["schema_version"], "veritasm-validation-dataset-v2");
        assert_eq!(manifest["case_id"], case);
        assert_eq!(manifest["generator"]["version"], "4");
        let dataset_id = manifest["dataset_id"].as_str().unwrap();
        assert!(dataset_id.starts_with("v4-"));
        assert_eq!(dataset_id.len(), 67);
        assert!(dataset_id[3..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_eq!(
            manifest["generator"]["parameters"]["output_compression"],
            "plain"
        );
        assert_eq!(manifest["generator"]["rng_name"], "sha256-counter-v1");
        assert_eq!(
            manifest["generator"]["seed_hex"].as_str().unwrap().len(),
            64
        );
        assert!(dataset.join("manifest.sha256").is_file());

        let origins = fs::read_to_string(dataset.join("evaluation_truth/origins.tsv")).unwrap();
        let expected_reads = if case == "linear-se" { 40 } else { 80 };
        assert_eq!(origins.lines().count(), expected_reads + 1);
        assert!(origins.lines().skip(1).all(|row| row
            .split('\t')
            .nth(2)
            .is_some_and(|read_id| read_id.starts_with("v4_"))));
        assert!(dataset
            .join("evaluation_truth/quality_events.tsv")
            .is_file());
        let input_manifest =
            fs::read_to_string(dataset.join("assembler_input/input_manifest.json")).unwrap();
        assert!(!input_manifest.contains("evaluation_truth"));
        assert!(!input_manifest.contains("mol-0001"));

        let empty_assembly = temporary.path().join(format!("{case}-empty.fasta"));
        fs::write(&empty_assembly, []).unwrap();
        let evaluation = temporary.path().join(format!("{case}-empty-evaluation"));
        assert_success(&evaluate(&[
            "--dataset",
            dataset.join("dataset.json").to_str().unwrap(),
            "--assembly",
            empty_assembly.to_str().unwrap(),
            "--out",
            evaluation.to_str().unwrap(),
        ]));
    }

    let zero_unused_insert = temporary.path().join("linear-se-zero-unused-insert");
    assert_success(&simulate(&[
        "--out",
        zero_unused_insert.to_str().unwrap(),
        "--case",
        "linear-se",
        "--fragments",
        "1",
        "--truth-length",
        "100",
        "--read-length",
        "30",
        "--insert-length",
        "0",
    ]));
    let empty_assembly = temporary.path().join("linear-se-zero-unused-insert.fasta");
    fs::write(&empty_assembly, []).unwrap();
    assert_success(&evaluate(&[
        "--dataset",
        zero_unused_insert.join("dataset.json").to_str().unwrap(),
        "--assembly",
        empty_assembly.to_str().unwrap(),
        "--out",
        temporary
            .path()
            .join("linear-se-zero-unused-insert-evaluation")
            .to_str()
            .unwrap(),
    ]));
}

#[test]
fn evaluator_content_root_binding_distinguishes_development_from_qualification() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "linear-se");
    let assembly = temporary.path().join("empty-binding-assembly.fasta");
    fs::write(&assembly, []).unwrap();
    let expected_root = lower_sha256(&fs::read(dataset.join("manifest.sha256")).unwrap());

    let bound = temporary.path().join("bound-evaluation");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        bound.to_str().unwrap(),
        "--expected-dataset-content-root-sha256",
        &expected_root,
    ]));
    let bound_result = result(&bound);
    assert_eq!(
        bound_result["inputs"]["dataset_content_root_sha256"],
        expected_root
    );
    assert_eq!(
        bound_result["inputs"]["dataset_binding"]["mode"],
        "externally_bound"
    );
    assert_eq!(
        bound_result["inputs"]["dataset_binding"]["qualification_admission"],
        "eligible_externally_bound"
    );
    assert_eq!(
        bound_result["inputs"]["dataset_binding"]["expected_dataset_content_root_sha256"],
        expected_root
    );

    let noncanonical_root = format!("A{}", &expected_root[1..]);
    let invalid_expected_output = temporary.path().join("invalid-expected-root");
    let invalid_expected = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        invalid_expected_output.to_str().unwrap(),
        "--expected-dataset-content-root-sha256",
        &noncanonical_root,
    ]);
    assert_safe_diagnostic(&invalid_expected, 2, "validation_configuration");
    assert!(!invalid_expected_output.exists());

    let wrong_root = "0".repeat(64);
    let wrong_expected_output = temporary.path().join("wrong-expected-root");
    let wrong_expected = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        wrong_expected_output.to_str().unwrap(),
        "--expected-dataset-content-root-sha256",
        &wrong_root,
    ]);
    assert_safe_diagnostic(&wrong_expected, 6, "validation_integrity");
    assert!(String::from_utf8_lossy(&wrong_expected.stderr)
        .contains("differs from externally supplied expectation"));
    assert!(!wrong_expected_output.exists());

    let mut rewritten_manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    rewritten_manifest["limitations"]
        .as_array_mut()
        .unwrap()
        .push(Value::String(
            "Internally coherent metadata rewrite used by the external-root kill test.".to_owned(),
        ));
    write_json_value(&dataset.join("dataset.json"), &rewritten_manifest);
    let rewritten_root = refresh_dataset_checksum_manifest(&dataset, &rewritten_manifest);
    assert_ne!(rewritten_root, expected_root);

    let development = temporary.path().join("development-unbound-evaluation");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        development.to_str().unwrap(),
    ]));
    let development_result = result(&development);
    assert_eq!(
        development_result["inputs"]["dataset_binding"]["mode"],
        "development_unbound"
    );
    assert_eq!(
        development_result["inputs"]["dataset_binding"]["qualification_admission"],
        "ineligible_development_unbound"
    );
    assert_eq!(
        development_result["inputs"]["dataset_binding"]["expected_dataset_content_root_sha256"],
        Value::Null
    );

    let coherent_rewrite_output = temporary.path().join("coherent-rewrite-bound-evaluation");
    let coherent_rewrite = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        coherent_rewrite_output.to_str().unwrap(),
        "--expected-dataset-content-root-sha256",
        &expected_root,
    ]);
    assert_safe_diagnostic(&coherent_rewrite, 6, "validation_integrity");
    assert!(String::from_utf8_lossy(&coherent_rewrite.stderr)
        .contains("differs from externally supplied expectation"));
    assert!(!coherent_rewrite_output.exists());

    let rebound = temporary
        .path()
        .join("rewritten-externally-bound-evaluation");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        rebound.to_str().unwrap(),
        "--expected-dataset-content-root-sha256",
        &rewritten_root,
    ]));

    let alternate_manifest = dataset.join("renamed-dataset.json");
    fs::copy(dataset.join("dataset.json"), &alternate_manifest).unwrap();
    let renamed_output = temporary.path().join("renamed-manifest-evaluation");
    let renamed = evaluate(&[
        "--dataset",
        alternate_manifest.to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        renamed_output.to_str().unwrap(),
    ]);
    assert_safe_diagnostic(&renamed, 2, "validation_configuration");
    assert!(
        String::from_utf8_lossy(&renamed.stderr).contains("filename must be exactly dataset.json")
    );
    assert!(!renamed_output.exists());
}

#[test]
fn evaluator_rejects_noncanonical_checksum_manifest_and_admits_replay_memory_boundaries() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "error-pe");
    let assembly = temporary.path().join("empty-replay-assembly.fasta");
    fs::write(&assembly, []).unwrap();

    let baseline_output = temporary.path().join("replay-baseline");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        baseline_output.to_str().unwrap(),
    ]));
    let baseline = result(&baseline_output);
    let projected = baseline["evaluator"]["projected_truth_evidence_replay_bytes_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(projected > 1);

    let below = (projected - 1).to_string();
    let below_output = temporary.path().join("replay-below-limit");
    let rejected = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        below_output.to_str().unwrap(),
        "--max-truth-evidence-replay-bytes",
        &below,
    ]);
    assert_safe_diagnostic(&rejected, 5, "validation_resource");
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("projected truth-evidence replay state")
    );
    assert!(!below_output.exists());

    for (name, limit) in [
        ("replay-exact-limit", projected),
        ("replay-plus-one-limit", projected + 1),
    ] {
        let output = temporary.path().join(name);
        let limit = limit.to_string();
        assert_success(&evaluate(&[
            "--dataset",
            dataset.join("dataset.json").to_str().unwrap(),
            "--assembly",
            assembly.to_str().unwrap(),
            "--out",
            output.to_str().unwrap(),
            "--max-truth-evidence-replay-bytes",
            &limit,
        ]));
        let value = result(&output);
        assert_eq!(
            value["evaluator"]["projected_truth_evidence_replay_bytes_decimal"],
            projected.to_string()
        );
        assert_eq!(
            value["evaluator"]["max_truth_evidence_replay_bytes_decimal"],
            limit
        );
    }

    let noncanonical = generate(temporary.path(), "linear-pe");
    let manifest_path = noncanonical.join("manifest.sha256");
    let mut manifest_bytes = fs::read(&manifest_path).unwrap();
    manifest_bytes.push(b'\n');
    fs::write(&manifest_path, manifest_bytes).unwrap();
    let trailing_output = temporary.path().join("trailing-manifest-evaluation");
    let trailing = evaluate(&[
        "--dataset",
        noncanonical.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        trailing_output.to_str().unwrap(),
    ]);
    assert_safe_diagnostic(&trailing, 6, "validation_integrity");
    assert!(String::from_utf8_lossy(&trailing.stderr)
        .contains("exact canonical sorted complete inventory"));
    assert!(!trailing_output.exists());
}

#[test]
fn every_generator_v4_truth_fasta_reaches_complete_self_recovery_bounds() {
    let temporary = TempDir::new().unwrap();
    for case in [
        "linear-se",
        "linear-pe",
        "circular-pe",
        "mixture-pe",
        "error-pe",
        "qc-censoring-control",
    ] {
        let dataset = generate(temporary.path(), case);
        let evaluation = temporary
            .path()
            .join(format!("{case}-self-truth-evaluation"));
        assert_success(&evaluate(&[
            "--dataset",
            dataset.join("dataset.json").to_str().unwrap(),
            "--assembly",
            dataset
                .join("evaluation_truth/truth.fasta")
                .to_str()
                .unwrap(),
            "--out",
            evaluation.to_str().unwrap(),
        ]));
        let value = result(&evaluation);
        let recovery = &value["base_metrics"]["recovery"];
        assert_eq!(
            recovery["compatible_placement_status"], "complete_exact_compatible_placement_universe",
            "{case}"
        );
        assert_eq!(
            recovery["compatible_truth_genome_fraction_lower_bound"]["value_decimal"], "1.000000",
            "{case}"
        );
        assert_eq!(
            recovery["compatible_truth_genome_fraction_upper_bound"]["value_decimal"], "1.000000",
            "{case}"
        );
    }
}

#[test]
fn validation_cli_diagnostics_escape_controls_and_use_typed_exits() {
    let temporary = TempDir::new().unwrap();
    let hostile = temporary
        .path()
        .join("result\nforged\tfield\u{1b}[31m\u{2028}next");
    fs::create_dir(&hostile).unwrap();
    let simulation = simulate(&["--out", hostile.to_str().unwrap(), "--case", "linear-se"]);
    assert_safe_diagnostic(&simulation, 2, "validation_configuration");
    let stderr = std::str::from_utf8(&simulation.stderr).unwrap();
    assert!(stderr.contains("\\n"));
    assert!(stderr.contains("\\t"));
    assert!(stderr.contains("\\u{1b}"));
    assert!(stderr.contains("\\u{2028}"));

    let dataset = generate(temporary.path(), "linear-se");
    let assembly = temporary.path().join("empty.fasta");
    fs::write(&assembly, []).unwrap();
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        hostile.to_str().unwrap(),
    ]);
    assert_safe_diagnostic(&evaluation, 2, "validation_configuration");
    assert!(std::str::from_utf8(&evaluation.stderr)
        .unwrap()
        .contains("\\n"));

    let parse_failure = simulate(&[
        "--out",
        temporary.path().join("unused").to_str().unwrap(),
        "--case",
        "invalid\nforged\u{1b}[31m",
    ]);
    assert_safe_diagnostic(&parse_failure, 2, "validation_configuration");
    assert!(std::str::from_utf8(&parse_failure.stderr)
        .unwrap()
        .contains("\\n"));
}

#[test]
fn evaluator_accepts_circular_rotation_and_scores_the_seam() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "circular-pe");
    let truth = fasta_sequences(&dataset.join("evaluation_truth/truth.fasta")).remove(0);
    let rotated = [&truth[173..], &truth[..173]].concat();
    let assembly = temporary.path().join("circular-assembly.fasta");
    write_fasta(&assembly, "rotated", &rotated);
    let evaluation = temporary.path().join("circular-evaluation");
    let output = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        evaluation.to_str().unwrap(),
        "--junction-flank",
        "12",
    ]);
    assert_success(&output);
    let value = result(&evaluation);
    assert_eq!(value["base_metrics"]["matches_decimal"], "400");
    assert_eq!(value["base_metrics"]["mismatches_decimal"], "0");
    assert_eq!(
        value["base_metrics"]["recovery"]["compatible_truth_genome_fraction_upper_bound"]
            ["value_decimal"],
        "1.000000"
    );
    assert_eq!(value["junction_metrics"]["false_decimal"], "0");
    assert_eq!(value["junction_metrics"]["correct_decimal"], "377");
    let baseline: EvaluationResult =
        serde_json::from_slice(&fs::read(evaluation.join("result.json")).unwrap()).unwrap();
    validate_evaluation_result(&baseline).unwrap();

    let mut false_empty = baseline.clone();
    false_empty.assembly.records_decimal = "0".to_owned();
    false_empty.assembly.accepted_alignment_records_decimal = "0".to_owned();
    false_empty.assembly.unaligned_records_decimal = "0".to_owned();
    false_empty.assembly.empty_output = true;
    false_empty.status.code = "evaluation_complete_empty_assembly".to_owned();
    false_empty.status.message =
        "Evaluation completed: the assembly contained no records; this is not biological absence."
            .to_owned();
    assert!(validate_evaluation_result(&false_empty).is_err());

    let mut false_unaligned = baseline.clone();
    false_unaligned.assembly.accepted_alignment_records_decimal = "0".to_owned();
    false_unaligned.assembly.unaligned_records_decimal = "1".to_owned();
    assert!(validate_evaluation_result(&false_unaligned).is_err());

    let mut false_zero_coverage = baseline.clone();
    false_zero_coverage
        .base_metrics
        .recovery
        .compatible_truth_genome_fraction_upper_bound
        .numerator_decimal = "0".to_owned();
    assert!(validate_evaluation_result(&false_zero_coverage).is_err());

    let mut forged_dataset_id = baseline.clone();
    forged_dataset_id.inputs.dataset_id = "forged-dataset".to_owned();
    assert!(validate_evaluation_result(&forged_dataset_id).is_err());
    let mut forged_truth_hash = baseline.clone();
    forged_truth_hash.inputs.truth_fasta_sha256 = "0".repeat(64);
    assert!(validate_evaluation_result(&forged_truth_hash).is_err());

    let mut forged_adjacencies = baseline;
    forged_adjacencies
        .junction_metrics
        .assembly_adjacencies_decimal = "400".to_owned();
    forged_adjacencies
        .junction_metrics
        .not_evaluated_short_flank_decimal = "23".to_owned();
    assert!(validate_evaluation_result(&forged_adjacencies).is_err());
    assert_manifest(&evaluation);
}

#[test]
fn evaluator_reports_empty_output_without_fabricating_zero_denominators() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "linear-se");
    let assembly = temporary.path().join("empty.fasta");
    fs::write(&assembly, []).unwrap();
    let evaluation = temporary.path().join("empty-evaluation");
    let output = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        evaluation.to_str().unwrap(),
        "--max-junction-index-bytes",
        "1",
    ]);
    assert_success(&output);
    let value = result(&evaluation);
    assert_eq!(
        value["status"]["code"],
        "evaluation_complete_empty_assembly"
    );
    assert_eq!(value["assembly"]["empty_output"], true);
    assert_eq!(
        value["evaluator"]["junction_index_execution_status"],
        "not_required_no_eligible_adjacencies"
    );
    assert_eq!(
        value["evaluator"]["junction_index_projected_bytes_decimal"],
        "0"
    );
    assert_eq!(
        value["evaluator"]["junction_index_capacity_bytes_decimal"],
        "0"
    );
    assert_eq!(
        value["base_metrics"]["error_rate"]["value_decimal"],
        Value::Null
    );
    assert_eq!(
        value["base_metrics"]["recovery"]["compatible_truth_genome_fraction_upper_bound"]
            ["value_decimal"],
        "0.000000"
    );
    assert_eq!(value["base_metrics"]["qv"]["value_decimal"], Value::Null);
    assert_eq!(
        fs::read_to_string(evaluation.join("alignments.tsv"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let typed: EvaluationResult =
        serde_json::from_slice(&fs::read(evaluation.join("result.json")).unwrap()).unwrap();
    validate_evaluation_result(&typed).unwrap();
    let mut forged_resource = typed.clone();
    forged_resource
        .evaluator
        .junction_index_capacity_bytes_decimal = "1".to_owned();
    assert!(validate_evaluation_result(&forged_resource).is_err());
    let mut forged_status = typed.clone();
    forged_status.status.code = "evaluation_complete".to_owned();
    assert!(validate_evaluation_result(&forged_status).is_err());
    let mut forged_status_message = typed.clone();
    forged_status_message.status.message = "misleading success".to_owned();
    assert!(validate_evaluation_result(&forged_status_message).is_err());
    let mut forged_base = typed.clone();
    forged_base.base_metrics.matches_decimal = "1".to_owned();
    assert!(validate_evaluation_result(&forged_base).is_err());
    let mut forged_index_status = typed.clone();
    forged_index_status
        .junction_metrics
        .assembly_adjacencies_decimal = "1".to_owned();
    forged_index_status
        .junction_metrics
        .eligible_adjacencies_decimal = "1".to_owned();
    forged_index_status.junction_metrics.correct_decimal = "1".to_owned();
    forged_index_status.junction_evidence.logical_rows_decimal = "1".to_owned();
    forged_index_status.junction_evidence.omitted_rows_decimal = "1".to_owned();
    forged_index_status
        .junction_evidence
        .omitted_correct_rows_decimal = "1".to_owned();
    assert!(validate_evaluation_result(&forged_index_status).is_err());
    let mut forged_ratio_value = typed.clone();
    forged_ratio_value.base_metrics.error_rate.value_decimal = Some("0.000000".to_owned());
    assert!(validate_evaluation_result(&forged_ratio_value).is_err());
    let mut forged_ratio_status = typed.clone();
    forged_ratio_status.base_metrics.error_rate.status = "measured".to_owned();
    assert!(validate_evaluation_result(&forged_ratio_status).is_err());
    let mut forged_qv = typed.clone();
    forged_qv.base_metrics.qv.value_decimal = Some("99.000000".to_owned());
    assert!(validate_evaluation_result(&forged_qv).is_err());
    let mut forged_molecule_ratio = typed.clone();
    forged_molecule_ratio.per_molecule[0]
        .compatible_truth_genome_fraction_upper_bound
        .value_decimal = Some("1.000000".to_owned());
    assert!(validate_evaluation_result(&forged_molecule_ratio).is_err());
    let mut forged_flank = typed.clone();
    forged_flank.junction_metrics.flank_length_decimal = "32".to_owned();
    assert!(validate_evaluation_result(&forged_flank).is_err());
    let mut forged_edit_limit = typed.clone();
    forged_edit_limit.evaluator.max_edit_rate_ppm_decimal = "1000001".to_owned();
    assert!(validate_evaluation_result(&forged_edit_limit).is_err());
    let mut forged_dp_limit = typed.clone();
    forged_dp_limit.evaluator.max_dp_cells_decimal = "250000001".to_owned();
    assert!(validate_evaluation_result(&forged_dp_limit).is_err());
    let mut forged_identity = typed.clone();
    forged_identity.inputs.assembly_fasta_sha256 = "0".repeat(64);
    assert!(validate_evaluation_result(&forged_identity).is_err());
    let mut forged_mode_identity = typed.clone();
    forged_mode_identity.junction_evidence.mode = JunctionEvidenceMode::All;
    assert!(validate_evaluation_result(&forged_mode_identity).is_err());
    let mut forged_binding = typed.clone();
    forged_binding.inputs.dataset_binding.mode = DatasetBindingMode::ExternallyBound;
    forged_binding
        .inputs
        .dataset_binding
        .qualification_admission = "eligible_externally_bound".to_owned();
    forged_binding
        .inputs
        .dataset_binding
        .expected_dataset_content_root_sha256 =
        Some(forged_binding.inputs.dataset_content_root_sha256.clone());
    assert!(validate_evaluation_result(&forged_binding).is_err());
    let mut forged_evidence = typed;
    forged_evidence.junction_evidence.emitted_rows_decimal = "1".to_owned();
    assert!(validate_evaluation_result(&forged_evidence).is_err());
    assert_manifest(&evaluation);
}

#[test]
fn evaluator_counts_base_error_and_nonadjacent_false_junction() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "linear-se");
    let truth = fasta_sequences(&dataset.join("evaluation_truth/truth.fasta")).remove(0);

    let mut erroneous = truth.clone();
    erroneous[200] = match erroneous[200] {
        b'A' => b'C',
        _ => b'A',
    };
    let error_assembly = temporary.path().join("error-assembly.fasta");
    write_fasta(&error_assembly, "one-substitution", &erroneous);
    let error_evaluation = temporary.path().join("error-evaluation");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        error_assembly.to_str().unwrap(),
        "--out",
        error_evaluation.to_str().unwrap(),
    ]));
    let error_result = result(&error_evaluation);
    assert_eq!(error_result["base_metrics"]["mismatches_decimal"], "1");
    assert_eq!(
        error_result["base_metrics"]["error_rate"]["numerator_decimal"],
        "1"
    );
    assert_eq!(
        error_result["base_metrics"]["recovery"]["compatible_placement_status"],
        "not_available_non_exact_compatible_placement_universe"
    );
    assert_eq!(
        error_result["base_metrics"]["recovery"]["compatible_truth_genome_fraction_upper_bound"]
            ["value_decimal"],
        Value::Null
    );
    assert_eq!(
        error_result["base_metrics"]["aligned_duplication_ratio"]["value_decimal"],
        Value::Null
    );

    let chimera = [&truth[60..120], &truth[260..320]].concat();
    let chimera_assembly = temporary.path().join("chimera-assembly.fasta");
    write_fasta(&chimera_assembly, "nonadjacent", &chimera);
    let chimera_evaluation = temporary.path().join("chimera-evaluation");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        chimera_assembly.to_str().unwrap(),
        "--out",
        chimera_evaluation.to_str().unwrap(),
        "--junction-flank",
        "12",
        "--max-edit-rate-ppm",
        "1000000",
    ]));
    let chimera_result = result(&chimera_evaluation);
    let false_adjacencies = chimera_result["junction_metrics"]["false_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(false_adjacencies >= 1);
    let junction_rows = fs::read_to_string(chimera_evaluation.join("junctions.tsv")).unwrap();
    assert!(junction_rows
        .lines()
        .any(|line| line.starts_with("2.0\tnonadjacent\t60\tfalse_junction\t")));
}

#[test]
fn junction_evidence_modes_preserve_metrics_and_logical_digest() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "linear-se");
    let truth = fasta_sequences(&dataset.join("evaluation_truth/truth.fasta")).remove(0);
    let mut assembly_sequence = truth.clone();
    let middle = assembly_sequence.len() / 2;
    assembly_sequence[middle] = match assembly_sequence[middle] {
        b'A' => b'C',
        _ => b'A',
    };
    let assembly = temporary.path().join("mode-comparison.fasta");
    write_fasta(&assembly, "mode-comparison", &assembly_sequence);

    let mut results = Vec::new();
    for mode in ["all", "non-correct", "summary"] {
        let output = temporary.path().join(format!("evaluation-{mode}"));
        assert_success(&evaluate(&[
            "--dataset",
            dataset.join("dataset.json").to_str().unwrap(),
            "--assembly",
            assembly.to_str().unwrap(),
            "--out",
            output.to_str().unwrap(),
            "--junction-evidence",
            mode,
        ]));
        results.push(result(&output));
    }
    let repeated_all = temporary.path().join("evaluation-all-repeat");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        repeated_all.to_str().unwrap(),
        "--junction-evidence",
        "all",
    ]));
    for artifact in [
        "alignments.tsv",
        "junctions.tsv",
        "result.json",
        "manifest.sha256",
    ] {
        assert_eq!(
            fs::read(temporary.path().join("evaluation-all").join(artifact)).unwrap(),
            fs::read(repeated_all.join(artifact)).unwrap(),
            "nondeterministic evaluator artifact: {artifact}"
        );
    }
    for value in &results[1..] {
        assert_eq!(value["junction_metrics"], results[0]["junction_metrics"]);
        assert_eq!(
            value["junction_evidence"]["logical_rows_sha256"],
            results[0]["junction_evidence"]["logical_rows_sha256"]
        );
        assert_eq!(
            value["junction_evidence"]["logical_rows_decimal"],
            results[0]["junction_evidence"]["logical_rows_decimal"]
        );
    }
    assert_eq!(
        results[0]["junction_evidence"]["emitted_rows_decimal"],
        results[0]["junction_evidence"]["logical_rows_decimal"]
    );
    let logical_rows = results[0]["junction_evidence"]["logical_rows_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let correct_rows = results[0]["junction_metrics"]["correct_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert_eq!(results[0]["junction_evidence"]["omitted_rows_decimal"], "0");
    assert_eq!(
        results[0]["junction_evidence"]["omitted_correct_rows_decimal"],
        "0"
    );
    assert_eq!(
        results[1]["junction_evidence"]["emitted_rows_decimal"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        logical_rows - correct_rows
    );
    assert_eq!(
        results[1]["junction_evidence"]["omitted_rows_decimal"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        correct_rows
    );
    assert_eq!(
        results[1]["junction_evidence"]["omitted_correct_rows_decimal"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        correct_rows
    );
    assert_eq!(results[2]["junction_evidence"]["emitted_rows_decimal"], "0");
    assert_eq!(
        results[2]["junction_evidence"]["omitted_rows_decimal"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        logical_rows
    );
    assert_eq!(
        results[2]["junction_evidence"]["omitted_correct_rows_decimal"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        correct_rows
    );
}

#[test]
fn evaluator_rejects_tampered_truth_before_creating_output() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "linear-pe");
    let truth_path = dataset.join("evaluation_truth/truth.fasta");
    let mut bytes = fs::read(&truth_path).unwrap();
    bytes.push(b'\n');
    fs::write(&truth_path, bytes).unwrap();
    let assembly = temporary.path().join("empty.fasta");
    fs::write(&assembly, []).unwrap();
    let evaluation = temporary.path().join("must-not-exist");
    let output = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        evaluation.to_str().unwrap(),
    ]);
    assert_safe_diagnostic(&output, 6, "validation_integrity");
    assert!(String::from_utf8_lossy(&output.stderr).contains("mismatch"));
    assert!(!evaluation.exists());
}

#[test]
fn generated_single_and_paired_reads_are_accepted_without_truth_arguments() {
    let temporary = TempDir::new().unwrap();
    let single = generate(temporary.path(), "linear-se");
    let single_bundle = temporary.path().join("single-bundle");
    assert_success(&assemble(&[
        "assemble",
        "--single",
        single
            .join("assembler_input/reads_SE.fastq")
            .to_str()
            .unwrap(),
        "--output-dir",
        single_bundle.to_str().unwrap(),
        "--k",
        "15",
        "--profile",
        "retain-all",
        "--min-base-quality",
        "0",
        "--threads",
        "1",
    ]));
    assert!(single_bundle.join("unitigs.fasta").is_file());

    let paired = generate(temporary.path(), "linear-pe");
    let paired_bundle = temporary.path().join("paired-bundle");
    assert_success(&assemble(&[
        "assemble",
        "--read1",
        paired
            .join("assembler_input/reads_R1.fastq")
            .to_str()
            .unwrap(),
        "--read2",
        paired
            .join("assembler_input/reads_R2.fastq")
            .to_str()
            .unwrap(),
        "--output-dir",
        paired_bundle.to_str().unwrap(),
        "--k",
        "15",
        "--profile",
        "retain-all",
        "--min-base-quality",
        "0",
        "--threads",
        "1",
    ]));
    let evaluation = temporary.path().join("paired-evaluation");
    assert_success(&evaluate(&[
        "--dataset",
        paired.join("dataset.json").to_str().unwrap(),
        "--assembly",
        paired_bundle.join("unitigs.fasta").to_str().unwrap(),
        "--out",
        evaluation.to_str().unwrap(),
        "--junction-flank",
        "10",
    ]));
    let evaluation_result = result(&evaluation);
    assert!(evaluation_result["assembly"]["records_decimal"] != "0");
    assert_eq!(
        evaluation_result["base_metrics"]["unaligned_assembly_bases_decimal"],
        "0"
    );
}

#[test]
fn validation_tools_refuse_existing_outputs_and_resource_overruns() {
    let temporary = TempDir::new().unwrap();
    let existing = temporary.path().join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("sentinel"), b"preserve").unwrap();
    let simulation = simulate(&["--out", existing.to_str().unwrap(), "--case", "linear-se"]);
    assert_safe_diagnostic(&simulation, 2, "validation_configuration");
    assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"preserve");

    let dataset = generate(temporary.path(), "linear-se");
    let mut truth = fasta_sequences(&dataset.join("evaluation_truth/truth.fasta")).remove(0);
    let middle = truth.len() / 2;
    truth[middle] = match truth[middle] {
        b'A' => b'C',
        _ => b'A',
    };
    let assembly = temporary.path().join("truth.fasta");
    write_fasta(&assembly, "truth", &truth);
    let limited = temporary.path().join("limited");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        limited.to_str().unwrap(),
        "--max-dp-cells",
        "10",
    ]);
    assert_safe_diagnostic(&evaluation, 5, "validation_resource");
    assert!(String::from_utf8_lossy(&evaluation.stderr).contains("resource limit"));
    assert!(!limited.exists());
    assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"preserve");

    let invalid_dp_cap = temporary.path().join("invalid-dp-cap");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        invalid_dp_cap.to_str().unwrap(),
        "--max-dp-cells",
        "250000001",
    ]);
    assert_safe_diagnostic(&evaluation, 2, "validation_configuration");
    assert!(String::from_utf8_lossy(&evaluation.stderr)
        .contains("maximum DP cells must be between 1 and 250000000"));
    assert!(!invalid_dp_cap.exists());

    let exact_truth = fasta_sequences(&dataset.join("evaluation_truth/truth.fasta")).remove(0);
    let exact_assembly = temporary.path().join("exact-truth.fasta");
    write_fasta(&exact_assembly, "exact", &exact_truth);
    let evidence_limited = temporary.path().join("evidence-limited");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        exact_assembly.to_str().unwrap(),
        "--out",
        evidence_limited.to_str().unwrap(),
        "--junction-evidence",
        "all",
        "--max-junction-evidence-bytes",
        "300",
    ]);
    assert_safe_diagnostic(&evaluation, 5, "validation_resource");
    assert!(String::from_utf8_lossy(&evaluation.stderr).contains("junction evidence"));
    assert!(!evidence_limited.exists());
    assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"preserve");

    let scan_limited = temporary.path().join("scan-limited");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        exact_assembly.to_str().unwrap(),
        "--out",
        scan_limited.to_str().unwrap(),
        "--max-exact-alignment-scan-bases",
        "1",
    ]);
    assert_safe_diagnostic(&evaluation, 5, "validation_resource");
    assert!(String::from_utf8_lossy(&evaluation.stderr).contains("exact-alignment scan bases"));
    assert!(!scan_limited.exists());
    assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"preserve");

    let oversized_simulation = temporary.path().join("oversized-simulation");
    let simulation = simulate(&[
        "--out",
        oversized_simulation.to_str().unwrap(),
        "--case",
        "linear-se",
        "--truth-length",
        "2000001",
    ]);
    assert_safe_diagnostic(&simulation, 5, "validation_resource");
    assert!(String::from_utf8_lossy(&simulation.stderr).contains("resource limit"));
    assert!(!oversized_simulation.exists());

    let oversized_assembly = temporary.path().join("oversized.fasta");
    fs::File::create(&oversized_assembly)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let oversized_evaluation = temporary.path().join("oversized-evaluation");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        oversized_assembly.to_str().unwrap(),
        "--out",
        oversized_evaluation.to_str().unwrap(),
    ]);
    assert_safe_diagnostic(&evaluation, 5, "validation_resource");
    let stderr = String::from_utf8_lossy(&evaluation.stderr);
    assert!(stderr.contains("resource limit"));
    assert!(stderr.contains("assembly FASTA"));
    assert!(!oversized_evaluation.exists());

    let short_dataset = temporary.path().join("short-linear-truth");
    let simulation = simulate(&[
        "--out",
        short_dataset.to_str().unwrap(),
        "--case",
        "linear-se",
        "--fragments",
        "1",
        "--truth-length",
        "1",
        "--read-length",
        "1",
    ]);
    assert_success(&simulation);
    let short_assembly = temporary.path().join("short-truth-long-assembly.fasta");
    write_fasta(&short_assembly, "longer-than-truth", b"ACGT");
    let short_evaluation = temporary.path().join("short-truth-evaluation");
    let evaluation = evaluate(&[
        "--dataset",
        short_dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        short_assembly.to_str().unwrap(),
        "--out",
        short_evaluation.to_str().unwrap(),
        "--junction-flank",
        "2",
    ]);
    assert_success(&evaluation);
    let value = result(&short_evaluation);
    assert_eq!(
        value["evaluator"]["junction_index_execution_status"],
        "built_empty_truth_window_universe"
    );
    assert_eq!(value["evaluator"]["junction_key_comparisons_decimal"], "0");
    assert_eq!(
        value["junction_metrics"]["eligible_adjacencies_decimal"],
        "1"
    );
    assert_eq!(
        value["junction_metrics"]["indeterminate_unmapped_flank_decimal"],
        "1"
    );
    assert_manifest(&short_evaluation);
}

#[test]
fn evaluator_rejects_truth_separation_manifest_semantic_conflicts() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "linear-pe");
    let original: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    let assembly = temporary.path().join("empty-for-manifest-checks.fasta");
    fs::write(&assembly, []).unwrap();

    let mut invalid = original.clone();
    file_record_mut(&mut invalid, "evaluation_truth/truth.fasta")["visibility"] =
        Value::String("assembler_input".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("invalid-truth-visibility"),
        &invalid,
        "assembler_input artifact is outside its namespace",
    );

    let mut invalid = original.clone();
    file_record_mut(&mut invalid, "assembler_input/input_manifest.json")["visibility"] =
        Value::String("evaluation_only".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("invalid-input-visibility"),
        &invalid,
        "evaluation_only artifact is outside its namespace",
    );

    let mut invalid = original.clone();
    let first_read = invalid["assembler_input"]["read_paths"][0]
        .as_str()
        .unwrap()
        .to_owned();
    file_record_mut(&mut invalid, &first_read)["role"] = Value::String("read_2".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("invalid-read-role"),
        &invalid,
        "role \"read_1\"",
    );

    let mut invalid = original.clone();
    let first_read = invalid["assembler_input"]["read_paths"][0]
        .as_str()
        .unwrap()
        .to_owned();
    file_record_mut(&mut invalid, &first_read)["visibility"] =
        Value::String("evaluation_only".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("invalid-read-visibility"),
        &invalid,
        "evaluation_only artifact is outside its namespace",
    );

    let mut invalid = original.clone();
    let first_read = invalid["assembler_input"]["read_paths"][0].clone();
    invalid["assembler_input"]["read_paths"][1] = first_read;
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("duplicate-read-path"),
        &invalid,
        "exact generator-v4 role/compression paths",
    );

    let mut invalid = original.clone();
    invalid["assembler_input"]["read_paths"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("read-path-cardinality-conflict"),
        &invalid,
        "requires 2 read path(s)",
    );

    let mut invalid = original.clone();
    invalid["assembler_input"]["mode"] = Value::String("single_end".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("case-mode-conflict"),
        &invalid,
        "requires assembler input mode",
    );

    let mut invalid = original.clone();
    invalid["assembler_input"]["reads_decimal"] = Value::String("79".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("read-count-conflict"),
        &invalid,
        "requires 80 reads",
    );

    let mut invalid = original.clone();
    invalid["evaluation_truth"]["molecules"][0]["truth_class"] = Value::String("minor".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("truth-role-conflict"),
        &invalid,
        "exactly one primary Linear truth molecule",
    );

    let mut invalid = original.clone();
    invalid["evaluation_truth"]["molecules"][0]["topology"] = Value::String("circular".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("truth-topology-conflict"),
        &invalid,
        "exactly one primary Linear truth molecule",
    );

    let mut invalid = original.clone();
    invalid["generator"]["rng_name"] = Value::String("unfrozen-rng".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("generator-identity-conflict"),
        &invalid,
        "generator identity differs",
    );

    let mut invalid = original.clone();
    invalid["evaluation_truth"]["coordinate_system"] =
        Value::String("zero_based_but_orientation_unspecified".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("coordinate-system-conflict"),
        &invalid,
        "truth coordinate system differs from the generator-v4 contract",
    );

    let mut invalid = original.clone();
    invalid["generator"]["parameters"]["truth_length_decimal"] = Value::String("399".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("generator-truth-length-conflict"),
        &invalid,
        "full canonical generator-parameter commitment",
    );

    let mut invalid = original.clone();
    let template = invalid["files"][0].clone();
    let files = invalid["files"].as_array_mut().unwrap();
    while files.len() <= 1_024 {
        let mut record = template.clone();
        record["path"] = Value::String(format!("evaluation_truth/extra-{:04}", files.len()));
        record["role"] = Value::String("supplementary_evidence".to_owned());
        record["visibility"] = Value::String("evaluation_only".to_owned());
        record["bytes_decimal"] = Value::String("0".to_owned());
        record["sha256"] = Value::String("0".repeat(64));
        files.push(record);
    }
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("dataset-file-record-limit"),
        &invalid,
        "dataset file record count 1025 exceeds fixed cap 1024",
    );

    let mut invalid = original.clone();
    invalid["files"][0]["bytes_decimal"] = Value::String((256u64 * 1024 * 1024).to_string());
    invalid["files"][1]["bytes_decimal"] = Value::String((256u64 * 1024 * 1024).to_string());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("dataset-artifact-byte-limit"),
        &invalid,
        "exceed fixed aggregate cap 536870912",
    );

    write_json_value(&dataset.join("dataset.json"), &original);
}

#[test]
fn evaluator_rejects_checksum_refreshed_malformed_or_semantically_fabricated_truth_ledgers() {
    let temporary = TempDir::new().unwrap();
    let assembly = temporary.path().join("empty-ledger-audit.fasta");
    fs::write(&assembly, []).unwrap();

    let make_dense_error = |name: &str, case: &str| {
        let dataset = temporary.path().join(name);
        let result = simulate(&[
            "--out",
            dataset.to_str().unwrap(),
            "--case",
            case,
            "--fragments",
            "4",
            "--truth-length",
            "100",
            "--read-length",
            "20",
            "--insert-length",
            "40",
            "--substitution-rate-ppm",
            "1000000",
        ]);
        assert_success(&result);
        dataset
    };

    let dataset = make_dense_error("bad-origin-header", "error-pe");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    let origins_path = "evaluation_truth/origins.tsv";
    let mut origins = fs::read(dataset.join(origins_path)).unwrap();
    origins[0] = b'X';
    rewrite_declared_artifact(&dataset, &mut manifest, origins_path, &origins);
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("bad-origin-header-evaluation"),
        &manifest,
        "header differs from the frozen descriptor",
    );

    let dataset = make_dense_error("inconsistent-pair-geometry", "error-pe");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    let origins = fs::read_to_string(dataset.join(origins_path)).unwrap();
    let mut lines = origins.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut first_fields = lines[1].split('\t').map(str::to_owned).collect::<Vec<_>>();
    let outer_start = first_fields[10].parse::<usize>().unwrap();
    first_fields[10] = if outer_start == 0 {
        "1".to_owned()
    } else {
        (outer_start - 1).to_string()
    };
    lines[1] = first_fields.join("\t");
    let rewritten = format!("{}\n", lines.join("\n"));
    rewrite_declared_artifact(&dataset, &mut manifest, origins_path, rewritten.as_bytes());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary
            .path()
            .join("inconsistent-pair-geometry-evaluation"),
        &manifest,
        "paired origin rows contradict inward-FR fragment geometry",
    );

    let dataset = make_dense_error("reordered-errors", "error-pe");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    let errors_path = "evaluation_truth/errors.tsv";
    let errors = fs::read_to_string(dataset.join(errors_path)).unwrap();
    let mut lines = errors.lines().map(str::to_owned).collect::<Vec<_>>();
    lines.swap(1, 2);
    let rewritten = format!("{}\n", lines.join("\n"));
    rewrite_declared_artifact(&dataset, &mut manifest, errors_path, rewritten.as_bytes());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("reordered-errors-evaluation"),
        &manifest,
        "error ledger does not reproduce observed sequence",
    );

    let dataset = make_dense_error("invalid-error-base", "error-pe");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    let errors = fs::read_to_string(dataset.join(errors_path)).unwrap();
    let mut lines = errors.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut fields = lines[1].split('\t').map(str::to_owned).collect::<Vec<_>>();
    fields[6] = "N".to_owned();
    lines[1] = fields.join("\t");
    let rewritten = format!("{}\n", lines.join("\n"));
    rewrite_declared_artifact(&dataset, &mut manifest, errors_path, rewritten.as_bytes());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("invalid-error-base-evaluation"),
        &manifest,
        "observed_base must be exactly one A/C/G/T base",
    );

    let dataset = make_dense_error("missing-error", "error-pe");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    let errors = fs::read_to_string(dataset.join(errors_path)).unwrap();
    let mut lines = errors.lines().collect::<Vec<_>>();
    lines.remove(1);
    let rewritten = format!("{}\n", lines.join("\n"));
    rewrite_declared_artifact(&dataset, &mut manifest, errors_path, rewritten.as_bytes());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("missing-error-evaluation"),
        &manifest,
        "does not reproduce observed sequence",
    );

    let dataset = make_dense_error("missing-linked-quality", "qc-censoring-control");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    let quality_path = "evaluation_truth/quality_events.tsv";
    let quality = fs::read_to_string(dataset.join(quality_path)).unwrap();
    let mut lines = quality.lines().collect::<Vec<_>>();
    lines.remove(1);
    let rewritten = format!("{}\n", lines.join("\n"));
    rewrite_declared_artifact(&dataset, &mut manifest, quality_path, rewritten.as_bytes());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("missing-linked-quality-evaluation"),
        &manifest,
        "quality-event offsets differ from substitution offsets",
    );

    let dataset = generate(temporary.path(), "linear-pe");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    manifest["generator"]["parameters"]["substitution_rate_ppm_decimal"] =
        Value::String("1".to_owned());
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("uncommitted-parameter-evaluation"),
        &manifest,
        "full canonical generator-parameter commitment",
    );
}

#[test]
fn evaluator_replays_content_detected_gzip_and_compression_changes_dataset_identity() {
    let temporary = TempDir::new().unwrap();
    let plain = temporary.path().join("identity-plain");
    let gzip = temporary.path().join("identity-gzip");
    let common = [
        "--case",
        "error-pe",
        "--fragments",
        "4",
        "--truth-length",
        "100",
        "--read-length",
        "20",
        "--insert-length",
        "40",
    ];
    let mut plain_arguments = vec!["--out", plain.to_str().unwrap()];
    plain_arguments.extend(common);
    assert_success(&simulate(&plain_arguments));
    let mut gzip_arguments = vec!["--out", gzip.to_str().unwrap()];
    gzip_arguments.extend(common);
    gzip_arguments.push("--gzip");
    assert_success(&simulate(&gzip_arguments));

    let plain_manifest: Value =
        serde_json::from_slice(&fs::read(plain.join("dataset.json")).unwrap()).unwrap();
    let gzip_manifest: Value =
        serde_json::from_slice(&fs::read(gzip.join("dataset.json")).unwrap()).unwrap();
    assert_ne!(plain_manifest["dataset_id"], gzip_manifest["dataset_id"]);
    assert_eq!(
        gzip_manifest["generator"]["parameters"]["output_compression"],
        "gzip"
    );
    let empty = temporary.path().join("empty-gzip-evaluation.fasta");
    fs::write(&empty, []).unwrap();
    assert_success(&evaluate(&[
        "--dataset",
        gzip.join("dataset.json").to_str().unwrap(),
        "--assembly",
        empty.to_str().unwrap(),
        "--out",
        temporary.path().join("gzip-evaluation").to_str().unwrap(),
    ]));
}

#[test]
fn evaluator_accepts_empty_fastq_only_for_zero_declared_fragments() {
    let temporary = TempDir::new().unwrap();
    let dataset = temporary.path().join("zero-fragments");
    assert_success(&simulate(&[
        "--out",
        dataset.to_str().unwrap(),
        "--case",
        "linear-se",
        "--fragments",
        "0",
        "--truth-length",
        "100",
        "--read-length",
        "20",
        "--insert-length",
        "0",
    ]));
    assert!(fs::read(dataset.join("assembler_input/reads_SE.fastq"))
        .unwrap()
        .is_empty());
    let assembly = temporary.path().join("zero-fragments-empty-assembly.fasta");
    fs::write(&assembly, []).unwrap();
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        temporary
            .path()
            .join("zero-fragments-evaluation")
            .to_str()
            .unwrap(),
    ]));
}

#[test]
fn evaluator_rejects_truth_fasta_order_drift_from_manifest_order() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "mixture-pe");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dataset.join("dataset.json")).unwrap()).unwrap();
    manifest["evaluation_truth"]["molecules"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    let assembly = temporary.path().join("empty-for-truth-order.fasta");
    fs::write(&assembly, []).unwrap();
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("truth-order-conflict"),
        &manifest,
        "truth FASTA record order differs from molecule manifest",
    );
}

#[test]
fn evaluator_rejects_input_manifest_semantic_conflicts_after_hash_verification() {
    let temporary = TempDir::new().unwrap();
    let dataset = generate(temporary.path(), "linear-pe");
    let dataset_path = dataset.join("dataset.json");
    let mut dataset_manifest: Value =
        serde_json::from_slice(&fs::read(&dataset_path).unwrap()).unwrap();
    let input_path = dataset.join("assembler_input/input_manifest.json");
    let mut input_manifest: Value =
        serde_json::from_slice(&fs::read(&input_path).unwrap()).unwrap();
    input_manifest["read_files"][0]["role"] = Value::String("R2".to_owned());
    let mut input_bytes = serde_json::to_vec_pretty(&input_manifest).unwrap();
    input_bytes.push(b'\n');
    fs::write(&input_path, &input_bytes).unwrap();
    let input_record =
        file_record_mut(&mut dataset_manifest, "assembler_input/input_manifest.json");
    input_record["bytes_decimal"] = Value::String(input_bytes.len().to_string());
    input_record["sha256"] = Value::String(lower_sha256(&input_bytes));

    let assembly = temporary.path().join("empty-for-input-manifest.fasta");
    fs::write(&assembly, []).unwrap();
    assert_invalid_dataset_manifest(
        &dataset,
        &assembly,
        &temporary.path().join("input-manifest-role-conflict"),
        &dataset_manifest,
        "must have role \"R1\"",
    );
}

#[test]
fn evaluator_rejects_many_small_records_and_aggregate_work_before_alignment() {
    let temporary = TempDir::new().unwrap();
    let dataset = temporary.path().join("large-truth-dataset");
    assert_success(&simulate(&[
        "--out",
        dataset.to_str().unwrap(),
        "--case",
        "linear-se",
        "--fragments",
        "1",
        "--truth-length",
        "20000",
        "--read-length",
        "30",
    ]));

    let too_many = temporary.path().join("too-many-small-records.fasta");
    write_single_base_records(&too_many, 10_001);
    let too_many_output = temporary.path().join("too-many-small-records-output");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        too_many.to_str().unwrap(),
        "--out",
        too_many_output.to_str().unwrap(),
    ]);
    assert!(!evaluation.status.success());
    assert!(String::from_utf8_lossy(&evaluation.stderr)
        .contains("assembly FASTA record count exceeds fixed cap 10000"));
    assert!(!too_many_output.exists());

    let excessive_work = temporary.path().join("aggregate-work-small-records.fasta");
    write_repeated_sequence_records(&excessive_work, 10_000, &[b'A'; 30]);
    let excessive_work_output = temporary.path().join("aggregate-work-small-records-output");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        excessive_work.to_str().unwrap(),
        "--out",
        excessive_work_output.to_str().unwrap(),
    ]);
    assert!(!evaluation.status.success());
    assert!(String::from_utf8_lossy(&evaluation.stderr).contains("aggregate alignment DP work"));
    assert!(!excessive_work_output.exists());
}

#[test]
fn evaluator_streams_more_than_the_v1_junction_row_limit() {
    let temporary = TempDir::new().unwrap();
    let dataset = temporary.path().join("large-linear-se");
    assert_success(&simulate(&[
        "--out",
        dataset.to_str().unwrap(),
        "--case",
        "linear-se",
        "--fragments",
        "1",
        "--truth-length",
        "250030",
        "--read-length",
        "30",
    ]));
    let assembly = dataset.join("evaluation_truth/truth.fasta");
    let output = temporary.path().join("large-junction-evaluation");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        assembly.to_str().unwrap(),
        "--out",
        output.to_str().unwrap(),
    ]);
    assert_success(&evaluation);
    let value = result(&output);
    assert_eq!(value["base_metrics"]["matches_decimal"], "250030");
    assert_eq!(
        value["junction_metrics"]["eligible_adjacencies_decimal"],
        "250001"
    );
    assert_eq!(value["junction_metrics"]["correct_decimal"], "250001");
    assert_eq!(value["junction_evidence"]["logical_rows_decimal"], "250001");
    assert_eq!(value["junction_evidence"]["emitted_rows_decimal"], "0");

    let long_identifier = temporary.path().join("long-identifier.fasta");
    write_fasta(&long_identifier, &"x".repeat(129), b"A");
    let output = temporary.path().join("long-identifier-output");
    let evaluation = evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        long_identifier.to_str().unwrap(),
        "--out",
        output.to_str().unwrap(),
    ]);
    assert!(!evaluation.status.success());
    assert!(String::from_utf8_lossy(&evaluation.stderr)
        .contains("assembly FASTA identifier length exceeds fixed cap 128 bytes"));
    assert!(!output.exists());
}

#[test]
fn evaluator_accepts_one_megabase_exact_linear_truth_in_summary_mode() {
    let temporary = TempDir::new().unwrap();
    let dataset = temporary.path().join("one-megabase-linear-se");
    assert_success(&simulate(&[
        "--out",
        dataset.to_str().unwrap(),
        "--case",
        "linear-se",
        "--fragments",
        "1",
        "--truth-length",
        "1000000",
        "--read-length",
        "30",
    ]));
    let output = temporary.path().join("one-megabase-evaluation");
    assert_success(&evaluate(&[
        "--dataset",
        dataset.join("dataset.json").to_str().unwrap(),
        "--assembly",
        dataset
            .join("evaluation_truth/truth.fasta")
            .to_str()
            .unwrap(),
        "--out",
        output.to_str().unwrap(),
        "--junction-evidence",
        "summary",
    ]));
    let value = result(&output);
    assert_eq!(value["base_metrics"]["matches_decimal"], "1000000");
    assert_eq!(value["base_metrics"]["mismatches_decimal"], "0");
    assert_eq!(
        value["junction_metrics"]["assembly_adjacencies_decimal"],
        "999999"
    );
    assert_eq!(
        value["junction_metrics"]["eligible_adjacencies_decimal"],
        "999971"
    );
    assert_eq!(value["junction_metrics"]["correct_decimal"], "999971");
    assert_eq!(
        value["junction_metrics"]["not_evaluated_short_flank_decimal"],
        "28"
    );
    assert_eq!(value["junction_evidence"]["logical_rows_decimal"], "999971");
    assert_eq!(value["junction_evidence"]["emitted_rows_decimal"], "0");
    let scan_bases = value["evaluator"]["exact_alignment_scan_bases_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let max_scan_bases = value["evaluator"]["max_exact_alignment_scan_bases_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(scan_bases > 0);
    assert!(scan_bases <= max_scan_bases);
    let projected_index_bytes = value["evaluator"]["junction_index_projected_bytes_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let capacity_index_bytes = value["evaluator"]["junction_index_capacity_bytes_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let max_index_bytes = value["evaluator"]["max_junction_index_bytes_decimal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(capacity_index_bytes <= projected_index_bytes);
    assert!(capacity_index_bytes <= max_index_bytes);
    assert_manifest(&output);
}

#[test]
fn validation_schema_files_are_valid_json_with_distinct_ids_or_artifacts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        "schema/validation_alignments.schema.json",
        "schema/validation_dataset.schema.json",
        "schema/validation_errors.schema.json",
        "schema/validation_experiment.schema.json",
        "schema/validation_junctions.schema.json",
        "schema/validation_origins.schema.json",
        "schema/validation_quality_events.schema.json",
        "schema/validation_result.schema.json",
    ];
    let mut identities = BTreeMap::new();
    for relative in files {
        let value: Value = serde_json::from_slice(&fs::read(root.join(relative)).unwrap()).unwrap();
        let identity = value
            .get("$id")
            .or_else(|| value.get("artifact"))
            .and_then(Value::as_str)
            .unwrap()
            .to_owned();
        assert!(identities.insert(identity, relative).is_none());
    }
}
