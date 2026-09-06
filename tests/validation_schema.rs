use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(binary: &str, arguments: &[&str]) -> Output {
    Command::new(binary).args(arguments).output().unwrap()
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

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn validate_json_schema(
    instance: &Value,
    schema: &Value,
    root: &Value,
    path: &str,
) -> Result<(), String> {
    let schema_object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: boolean and non-object schemas are not supported"))?;
    for keyword in schema_object.keys() {
        if !matches!(
            keyword.as_str(),
            "$schema"
                | "$id"
                | "$defs"
                | "$ref"
                | "title"
                | "type"
                | "const"
                | "enum"
                | "oneOf"
                | "required"
                | "properties"
                | "additionalProperties"
                | "minProperties"
                | "minLength"
                | "pattern"
                | "minItems"
                | "maxItems"
                | "items"
                | "x-sort-key"
                | "x-invariants"
        ) {
            return Err(format!(
                "{path}: test validator does not implement schema keyword {keyword:?}"
            ));
        }
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("{path}: unsupported reference {reference:?}"))?;
        let target = root
            .get("$defs")
            .and_then(|definitions| definitions.get(name))
            .ok_or_else(|| format!("{path}: unresolved reference {reference:?}"))?;
        return validate_json_schema(instance, target, root, path);
    }
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = branches
            .iter()
            .filter(|branch| validate_json_schema(instance, branch, root, path).is_ok())
            .count();
        if matches != 1 {
            return Err(format!(
                "{path}: expected exactly one oneOf branch, got {matches}"
            ));
        }
    }
    if let Some(expected) = schema.get("const") {
        if instance != expected {
            return Err(format!("{path}: expected const {expected}, got {instance}"));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(instance) {
            return Err(format!(
                "{path}: value {instance} is outside enum {allowed:?}"
            ));
        }
    }
    if let Some(expected_type) = schema.get("type").and_then(Value::as_str) {
        let matches = match expected_type {
            "object" => instance.is_object(),
            "array" => instance.is_array(),
            "string" => instance.is_string(),
            "boolean" => instance.is_boolean(),
            "null" => instance.is_null(),
            other => return Err(format!("{path}: unsupported schema type {other:?}")),
        };
        if !matches {
            return Err(format!("{path}: expected {expected_type}, got {instance}"));
        }
    }
    if let Some(text) = instance.as_str() {
        if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64) {
            if text.chars().count() < usize::try_from(minimum).unwrap() {
                return Err(format!("{path}: string shorter than {minimum}"));
            }
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            if !matches_known_pattern(text, pattern)? {
                return Err(format!("{path}: {text:?} does not match {pattern:?}"));
            }
        }
    }
    if let Some(object) = instance.as_object() {
        if let Some(minimum) = schema.get("minProperties").and_then(Value::as_u64) {
            if object.len() < usize::try_from(minimum).unwrap() {
                return Err(format!(
                    "{path}: object has fewer than {minimum} properties"
                ));
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("{path}: missing required property {name:?}"));
                }
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            for name in object.keys() {
                if properties.is_none_or(|known| !known.contains_key(name)) {
                    return Err(format!("{path}: unexpected property {name:?}"));
                }
            }
        }
        if let Some(properties) = properties {
            for (name, child_schema) in properties {
                if let Some(child) = object.get(name) {
                    validate_json_schema(child, child_schema, root, &format!("{path}.{name}"))?;
                }
            }
        }
    }
    if let Some(array) = instance.as_array() {
        if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64) {
            if array.len() < usize::try_from(minimum).unwrap() {
                return Err(format!("{path}: array has fewer than {minimum} items"));
            }
        }
        if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64) {
            if array.len() > usize::try_from(maximum).unwrap() {
                return Err(format!("{path}: array has more than {maximum} items"));
            }
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in array.iter().enumerate() {
                validate_json_schema(item, item_schema, root, &format!("{path}[{index}]"))?;
            }
        }
    }
    Ok(())
}

fn matches_known_pattern(value: &str, pattern: &str) -> Result<bool, String> {
    let matches = match pattern {
        r"^[A-Za-z0-9._-]+$" => {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        }
        r"^[0-9a-f]{64}$" => {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        r"^v4-[0-9a-f]{64}$" => value
            .strip_prefix("v4-")
            .is_some_and(|digest| matches_known_pattern(digest, r"^[0-9a-f]{64}$") == Ok(true)),
        r"^(0|[1-9][0-9]*)$" => canonical_unsigned(value, false),
        r"^[1-9][0-9]*$" => canonical_unsigned(value, true),
        r"^(?!.*(?:^|/)\.{1,2}(?:/|$))(?:[A-Za-z0-9._-]+/)*[A-Za-z0-9._-]+$" => {
            !value.is_empty()
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
                })
                && !value.starts_with('/')
                && !value.ends_with('/')
                && !value
                    .split('/')
                    .any(|component| component.is_empty() || matches!(component, "." | ".."))
        }
        r"^eval-[0-9a-f]{64}$" => value
            .strip_prefix("eval-")
            .is_some_and(|digest| matches_known_pattern(digest, r"^[0-9a-f]{64}$") == Ok(true)),
        r"^[0-9]+\.[0-9]{6}$" => fixed_six_decimal(value, false),
        r"^-?[0-9]+\.[0-9]{6}$" => fixed_six_decimal(value, true),
        other => {
            return Err(format!(
                "test validator does not implement pattern {other:?}"
            ))
        }
    };
    Ok(matches)
}

fn canonical_unsigned(value: &str, positive: bool) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) && !value.starts_with('0')
        || (!positive && value == "0")
}

fn fixed_six_decimal(value: &str, signed: bool) -> bool {
    let unsigned = if signed {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    unsigned.split_once('.').is_some_and(|(whole, fraction)| {
        !whole.is_empty()
            && whole.bytes().all(|byte| byte.is_ascii_digit())
            && fraction.len() == 6
            && fraction.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn descriptor_columns(schema: &Value) -> Vec<&str> {
    schema["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| {
            column
                .as_str()
                .or_else(|| column.get("name").and_then(Value::as_str))
                .unwrap()
        })
        .collect()
}

fn validate_tsv_descriptor(path: &Path, schema: &Value) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(schema["encoding"], "UTF-8");
    assert_eq!(schema["line_ending"], "LF");
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.contains(&b'\r'));
    let text = std::str::from_utf8(&bytes).unwrap();
    let mut lines = text.lines();
    let columns = descriptor_columns(schema);
    assert_eq!(
        lines.next().unwrap().split('\t').collect::<Vec<_>>(),
        columns
    );
    for (row_index, line) in lines.enumerate() {
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            columns.len(),
            "{} row {}",
            path.display(),
            row_index + 2
        );
        assert_eq!(
            fields[0],
            schema["artifact_schema_version"].as_str().unwrap()
        );
        for (field, column) in fields.iter().zip(schema["columns"].as_array().unwrap()) {
            if column.is_object() {
                validate_tsv_field(field, column);
            }
        }
        if schema["artifact"] == "junctions.tsv" {
            assert!(schema["classifications"]
                .as_array()
                .unwrap()
                .contains(&Value::String(fields[3].to_owned())));
        }
    }
}

fn validate_tsv_field(field: &str, column: &Value) {
    match column["type"].as_str().unwrap() {
        "literal" => assert_eq!(field, column["value"].as_str().unwrap()),
        "enum" => assert!(column["values"]
            .as_array()
            .unwrap()
            .contains(&Value::String(field.to_owned()))),
        "u64_decimal" => assert!(canonical_unsigned(field, false)),
        "u8_decimal" => {
            assert!(canonical_unsigned(field, false));
            let value = field.parse::<u8>().unwrap();
            let minimum = column["minimum"].as_str().unwrap().parse::<u8>().unwrap();
            let maximum = column["maximum"].as_str().unwrap().parse::<u8>().unwrap();
            assert!((minimum..=maximum).contains(&value));
        }
        "opaque_ascii_identifier" | "truth_molecule_identifier" => {
            assert!(!field.is_empty() && field.bytes().all(|byte| (b'!'..=b'~').contains(&byte)))
        }
        "boolean" => assert!(matches!(field, "true" | "false")),
        "lowercase_sha256" => {
            assert!(matches_known_pattern(field, r"^[0-9a-f]{64}$").unwrap())
        }
        kind => panic!("unimplemented TSV descriptor type {kind:?}"),
    }
}

fn emit_validation_artifacts(root: &Path) -> (PathBuf, PathBuf) {
    let dataset = root.join("dataset");
    assert_success(&run(
        env!("CARGO_BIN_EXE_veritasm-simulate"),
        &[
            "--out",
            dataset.to_str().unwrap(),
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
            "--substitution-rate-ppm",
            "1000000",
            "--low-quality-rate-ppm",
            "1000000",
        ],
    ));
    let evaluation = root.join("evaluation");
    assert_success(&run(
        env!("CARGO_BIN_EXE_veritasm-evaluate"),
        &[
            "--dataset",
            dataset.join("dataset.json").to_str().unwrap(),
            "--assembly",
            dataset
                .join("evaluation_truth/truth.fasta")
                .to_str()
                .unwrap(),
            "--out",
            evaluation.to_str().unwrap(),
            "--junction-evidence",
            "all",
        ],
    ));
    (dataset, evaluation)
}

#[test]
fn emitted_json_and_tsv_artifacts_conform_to_shipped_schemas() {
    let temporary = TempDir::new().unwrap();
    let (dataset, evaluation) = emit_validation_artifacts(temporary.path());
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));

    let dataset_schema = json(&repository.join("schema/validation_dataset.schema.json"));
    let dataset_instance = json(&dataset.join("dataset.json"));
    validate_json_schema(&dataset_instance, &dataset_schema, &dataset_schema, "$").unwrap();

    let result_schema = json(&repository.join("schema/validation_result.schema.json"));
    let result_instance = json(&evaluation.join("result.json"));
    validate_json_schema(&result_instance, &result_schema, &result_schema, "$").unwrap();

    for (artifact, descriptor) in [
        (
            dataset.join("evaluation_truth/origins.tsv"),
            "schema/validation_origins.schema.json",
        ),
        (
            dataset.join("evaluation_truth/errors.tsv"),
            "schema/validation_errors.schema.json",
        ),
        (
            dataset.join("evaluation_truth/quality_events.tsv"),
            "schema/validation_quality_events.schema.json",
        ),
        (
            evaluation.join("alignments.tsv"),
            "schema/validation_alignments.schema.json",
        ),
        (
            evaluation.join("junctions.tsv"),
            "schema/validation_junctions.schema.json",
        ),
    ] {
        assert!(fs::read_to_string(&artifact).unwrap().lines().count() > 1);
        validate_tsv_descriptor(&artifact, &json(&repository.join(descriptor)));
    }

    let mut invalid = dataset_instance;
    invalid
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_owned(), Value::Bool(true));
    assert!(validate_json_schema(&invalid, &dataset_schema, &dataset_schema, "$").is_err());
}

#[test]
fn portable_dataset_path_schema_has_the_frozen_ascii_grammar() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let schema = json(&repository.join("schema/validation_dataset.schema.json"));
    let pattern = schema["$defs"]["relative_path"]["pattern"]
        .as_str()
        .unwrap();
    for accepted in [
        "dataset.json",
        "assembler_input/reads_R1.fastq.gz",
        "evaluation_truth/.retained-name_1-2",
        "a/.../b",
    ] {
        assert!(
            matches_known_pattern(accepted, pattern).unwrap(),
            "{accepted:?}"
        );
    }
    for rejected in [
        "",
        "/absolute",
        "trailing/",
        "a//b",
        "a/./b",
        "a/../b",
        "a\\b",
        "a b",
        "a\tb",
        "a\nb",
        "café",
    ] {
        assert!(
            !matches_known_pattern(rejected, pattern).unwrap(),
            "{rejected:?}"
        );
    }
}
