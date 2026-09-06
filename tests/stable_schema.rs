use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

type CheckResult<T = ()> = Result<T, String>;

const UNITIG_HEADER: &str = "schema_version\tunitig_id\tlength_bases\tk\ttopology\tedge_steps\tcanonical_kmers\tsupport_unit\tretention_min_support\tminimum_represented_key_support\tlower_median_represented_key_support\tmaximum_represented_key_support\tenumeration_complete_read_placements\tsingle_group_read_instances\tmulti_group_read_instances_with_group\tplacement_enumeration_status\tsequence_sha256";
const PAIR_LINK_HEADER: &str = "schema_version\tk\tlane_ordinal\tsegment_a\tend_a\tstrand_a\tend_distance_a\tmate_role_a\tsegment_b\tend_b\tstrand_b\tend_distance_b\tmate_role_b\tsupplied_fragment_instances\tstatus";
const PAIR_SUMMARY_HEADER: &str = "schema_version\tk\tstate\tsupplied_fragment_instances";
const TRANSFORM_HEADER: &str = "schema_version\tstage_order\talgorithm_id\talgorithm_version\tsupport_unit\tparameters_json\tinput_distinct_canonical_keys\toutput_distinct_canonical_keys\tinput_support_mass\toutput_support_mass\tremoved_key_count\tremoved_support_mass\tdecision_set_sha256\tpre_state_sha256\tpost_state_sha256\tstatus";

#[derive(Clone, Debug)]
struct FastaRecord {
    id: String,
    sequence: Vec<u8>,
    line_lengths: Vec<usize>,
    fields: BTreeMap<String, String>,
}

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

fn emit_stable_bundle(root: &Path) -> PathBuf {
    let input = root.join("reads.fasta");
    fs::write(
        &input,
        b">a1\nAACGCTA\n>a2\nAACGCTA\n>b1\nTTGGCAT\n>b2\nTTGGCAT\n",
    )
    .unwrap();
    let bundle = root.join("bundle");
    assert_success(&run(
        env!("CARGO_BIN_EXE_veritasm"),
        &[
            "assemble",
            "-U",
            input.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
            "-k",
            "5",
            "--profile",
            "retain-all",
            "--threads",
            "2",
        ],
    ));
    bundle
}

fn emit_linked_stable_bundle(root: &Path) -> PathBuf {
    let input = root.join("linked-reads.fasta");
    fs::write(&input, b">read-a\nTCGCGAC\n").unwrap();
    let bundle = root.join("linked-bundle");
    assert_success(&run(
        env!("CARGO_BIN_EXE_veritasm"),
        &[
            "assemble",
            "-U",
            input.to_str().unwrap(),
            "-o",
            bundle.to_str().unwrap(),
            "-k",
            "6",
            "--profile",
            "retain-all",
            "--min-base-quality",
            "0",
            "--no-remap",
            "--threads",
            "2",
        ],
    ));
    bundle
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn schema_children(schema: &Map<String, Value>) -> CheckResult<Vec<&Value>> {
    let mut children = Vec::new();
    for keyword in schema.keys() {
        if !matches!(
            keyword.as_str(),
            "$schema"
                | "$id"
                | "$defs"
                | "$ref"
                | "title"
                | "description"
                | "type"
                | "const"
                | "enum"
                | "oneOf"
                | "allOf"
                | "if"
                | "then"
                | "else"
                | "required"
                | "properties"
                | "additionalProperties"
                | "minProperties"
                | "minLength"
                | "pattern"
                | "minimum"
                | "maximum"
                | "minItems"
                | "maxItems"
                | "prefixItems"
                | "items"
                | "x-property-order"
                | "x-sort-key"
                | "x-conditional-row-sets"
                | "x-invariant"
                | "x-invariants"
                | "x-maximum-decimal"
        ) {
            return Err(format!(
                "test validator does not implement schema keyword {keyword:?}"
            ));
        }
    }
    for object_keyword in ["$defs", "properties"] {
        if let Some(values) = schema.get(object_keyword) {
            let object = values
                .as_object()
                .ok_or_else(|| format!("{object_keyword} must be an object"))?;
            children.extend(object.values());
        }
    }
    for array_keyword in ["oneOf", "allOf", "prefixItems"] {
        if let Some(values) = schema.get(array_keyword) {
            let array = values
                .as_array()
                .ok_or_else(|| format!("{array_keyword} must be an array"))?;
            children.extend(array);
        }
    }
    for schema_keyword in ["if", "then", "else", "items", "additionalProperties"] {
        if let Some(child) = schema.get(schema_keyword) {
            if child.is_object() || child.is_boolean() {
                children.push(child);
            } else {
                return Err(format!("{schema_keyword} must be a schema"));
            }
        }
    }
    Ok(children)
}

fn check_supported_schema(schema: &Value, path: &str) -> CheckResult {
    check_supported_schema_node(schema, schema, path)
}

fn check_supported_schema_node(schema: &Value, root: &Value, path: &str) -> CheckResult {
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object or boolean"))?;
    if let Some(expected_type) = object.get("type").and_then(Value::as_str) {
        if !matches!(
            expected_type,
            "object" | "array" | "string" | "boolean" | "integer" | "null"
        ) {
            return Err(format!(
                "{path}: test validator does not implement schema type {expected_type:?}"
            ));
        }
    }
    if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
        matches_known_pattern("", pattern)?;
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("{path}: unsupported reference {reference:?}"))?;
        if root
            .get("$defs")
            .and_then(|definitions| definitions.get(name))
            .is_none()
        {
            return Err(format!("{path}: unresolved reference {reference:?}"));
        }
    }
    for child in schema_children(object)? {
        check_supported_schema_node(child, root, path)?;
    }
    Ok(())
}

fn validate_json_schema(instance: &Value, schema: &Value, root: &Value, path: &str) -> CheckResult {
    if let Some(allowed) = schema.as_bool() {
        return if allowed {
            Ok(())
        } else {
            Err(format!("{path}: rejected by false schema"))
        };
    }
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object or boolean"))?;

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("{path}: unsupported reference {reference:?}"))?;
        let target = root
            .get("$defs")
            .and_then(|definitions| definitions.get(name))
            .ok_or_else(|| format!("{path}: unresolved reference {reference:?}"))?;
        validate_json_schema(instance, target, root, path)?;
    }
    if let Some(expected) = object.get("const") {
        if instance != expected {
            return Err(format!("{path}: expected const {expected}, got {instance}"));
        }
    }
    if let Some(allowed) = object.get("enum") {
        let allowed = allowed
            .as_array()
            .ok_or_else(|| format!("{path}: enum must be an array"))?;
        if !allowed.contains(instance) {
            return Err(format!("{path}: {instance} is outside enum {allowed:?}"));
        }
    }
    if let Some(expected_type) = object.get("type").and_then(Value::as_str) {
        let matches = match expected_type {
            "object" => instance.is_object(),
            "array" => instance.is_array(),
            "string" => instance.is_string(),
            "boolean" => instance.is_boolean(),
            "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
            "null" => instance.is_null(),
            other => return Err(format!("{path}: unsupported schema type {other:?}")),
        };
        if !matches {
            return Err(format!("{path}: expected {expected_type}, got {instance}"));
        }
    }
    if let Some(branches) = object.get("allOf") {
        for branch in branches
            .as_array()
            .ok_or_else(|| format!("{path}: allOf must be an array"))?
        {
            validate_json_schema(instance, branch, root, path)?;
        }
    }
    if let Some(branches) = object.get("oneOf") {
        let matches = branches
            .as_array()
            .ok_or_else(|| format!("{path}: oneOf must be an array"))?
            .iter()
            .filter(|branch| validate_json_schema(instance, branch, root, path).is_ok())
            .count();
        if matches != 1 {
            return Err(format!(
                "{path}: expected exactly one oneOf branch, got {matches}"
            ));
        }
    }
    if let Some(condition) = object.get("if") {
        let condition_matches = validate_json_schema(instance, condition, root, path).is_ok();
        if condition_matches {
            if let Some(consequence) = object.get("then") {
                validate_json_schema(instance, consequence, root, path)?;
            }
        } else if let Some(alternative) = object.get("else") {
            validate_json_schema(instance, alternative, root, path)?;
        }
    }

    if let Some(text) = instance.as_str() {
        if let Some(minimum) = object.get("minLength").and_then(Value::as_u64) {
            if u64::try_from(text.chars().count()).unwrap() < minimum {
                return Err(format!("{path}: string shorter than {minimum}"));
            }
        }
        if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
            if !matches_known_pattern(text, pattern)? {
                return Err(format!("{path}: {text:?} does not match {pattern:?}"));
            }
        }
        if let Some(maximum) = object.get("x-maximum-decimal").and_then(Value::as_str) {
            let value = text
                .parse::<u128>()
                .map_err(|_| format!("{path}: decimal string does not fit u128"))?;
            let maximum = maximum
                .parse::<u128>()
                .map_err(|_| format!("{path}: invalid x-maximum-decimal"))?;
            if value > maximum {
                return Err(format!("{path}: decimal value exceeds {maximum}"));
            }
        }
    }
    if instance.as_i64().is_some() || instance.as_u64().is_some() {
        let value = instance
            .as_i64()
            .map(i128::from)
            .or_else(|| instance.as_u64().map(i128::from))
            .unwrap();
        if let Some(minimum) = object.get("minimum").and_then(Value::as_i64) {
            if value < i128::from(minimum) {
                return Err(format!("{path}: integer is below {minimum}"));
            }
        }
        if let Some(maximum) = object.get("maximum").and_then(Value::as_u64) {
            if value < 0 || u128::try_from(value).unwrap() > u128::from(maximum) {
                return Err(format!("{path}: integer is above {maximum}"));
            }
        }
    }
    if let Some(instance_object) = instance.as_object() {
        if let Some(minimum) = object.get("minProperties").and_then(Value::as_u64) {
            if u64::try_from(instance_object.len()).unwrap() < minimum {
                return Err(format!("{path}: too few object properties"));
            }
        }
        let properties = object.get("properties").and_then(Value::as_object);
        if let Some(required) = object.get("required") {
            for name in required
                .as_array()
                .ok_or_else(|| format!("{path}: required must be an array"))?
                .iter()
                .map(|name| {
                    name.as_str()
                        .ok_or_else(|| format!("{path}: required name must be a string"))
                })
            {
                let name = name?;
                if !instance_object.contains_key(name) {
                    return Err(format!("{path}: missing required property {name:?}"));
                }
            }
        }
        if let Some(additional) = object.get("additionalProperties") {
            for (name, child) in instance_object {
                if properties.is_some_and(|known| known.contains_key(name)) {
                    continue;
                }
                validate_json_schema(child, additional, root, &format!("{path}.{name}"))?;
            }
        }
        if let Some(properties) = properties {
            for (name, child_schema) in properties {
                if let Some(child) = instance_object.get(name) {
                    validate_json_schema(child, child_schema, root, &format!("{path}.{name}"))?;
                }
            }
        }
    }
    if let Some(array) = instance.as_array() {
        if let Some(minimum) = object.get("minItems").and_then(Value::as_u64) {
            if u64::try_from(array.len()).unwrap() < minimum {
                return Err(format!("{path}: too few array items"));
            }
        }
        if let Some(maximum) = object.get("maxItems").and_then(Value::as_u64) {
            if u64::try_from(array.len()).unwrap() > maximum {
                return Err(format!("{path}: too many array items"));
            }
        }
        let prefix_count = if let Some(prefix) = object.get("prefixItems") {
            let prefix = prefix
                .as_array()
                .ok_or_else(|| format!("{path}: prefixItems must be an array"))?;
            for (index, child_schema) in prefix.iter().enumerate() {
                if let Some(child) = array.get(index) {
                    validate_json_schema(child, child_schema, root, &format!("{path}[{index}]"))?;
                }
            }
            prefix.len()
        } else {
            0
        };
        if let Some(item_schema) = object.get("items") {
            for (index, child) in array.iter().enumerate().skip(prefix_count) {
                validate_json_schema(child, item_schema, root, &format!("{path}[{index}]"))?;
            }
        }
    }
    Ok(())
}

fn matches_known_pattern(value: &str, pattern: &str) -> CheckResult<bool> {
    let result = match pattern {
        r"^(0|[1-9][0-9]{0,19})$" => canonical_decimal(value, true, 20),
        r"^[1-9][0-9]{0,19}$" => canonical_decimal(value, false, 20),
        r"^[0-9a-f]{64}$" => is_lower_sha256(value),
        r"^v4-[0-9a-f]{64}$" => value.strip_prefix("v4-").is_some_and(is_lower_sha256),
        r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$" => semver_like(value),
        r"^lane-[0-9]{6}-(S|R1|R2)$" => lane_label(value),
        r"^\{.*\}$" => value.starts_with('{') && value.ends_with('}') && !value.contains('\n'),
        other => {
            return Err(format!(
                "test validator does not implement schema pattern {other:?}"
            ))
        }
    };
    Ok(result)
}

fn canonical_decimal(value: &str, allow_zero: bool, maximum_digits: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_digits
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && ((allow_zero && value == "0") || !value.starts_with('0'))
}

fn semver_like(value: &str) -> bool {
    let core_end = value.find(['-', '+']).unwrap_or(value.len());
    let (core, suffix) = value.split_at(core_end);
    let mut parts = core.split('.');
    let core_ok = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && parts.next().is_none();
    core_ok
        && (suffix.is_empty()
            || (suffix.len() > 1
                && suffix[1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))))
}

fn lane_label(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("lane-") else {
        return false;
    };
    let Some((ordinal, role)) = rest.split_once('-') else {
        return false;
    };
    ordinal.len() == 6
        && ordinal.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(role, "S" | "R1" | "R2")
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_u64(value: &str, context: &str) -> CheckResult<u64> {
    if !canonical_decimal(value, true, 20) {
        return Err(format!("{context}: not a canonical u64 decimal"));
    }
    value
        .parse::<u64>()
        .map_err(|_| format!("{context}: outside u64"))
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").unwrap();
    }
    output
}

fn lower_sha256(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

fn reverse_complement(sequence: &[u8]) -> CheckResult<Vec<u8>> {
    sequence
        .iter()
        .rev()
        .map(|base| match base {
            b'A' => Ok(b'T'),
            b'C' => Ok(b'G'),
            b'G' => Ok(b'C'),
            b'T' => Ok(b'A'),
            _ => Err("non-ACGT sequence in oriented GFA check".to_owned()),
        })
        .collect()
}

fn parse_fasta(path: &Path) -> CheckResult<Vec<FastaRecord>> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(b"\n") || bytes.contains(&b'\r') {
        return Err("FASTA must use LF and end with LF".to_owned());
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let mut records = Vec::<FastaRecord>::new();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix('>') {
            let mut tokens = header.split(' ');
            let id = tokens.next().unwrap_or_default();
            if !id.starts_with("utg-") || !is_lower_sha256(&id[4..]) {
                return Err(format!("invalid FASTA unitig ID {id:?}"));
            }
            let mut fields = BTreeMap::new();
            let mut observed_names = Vec::new();
            for token in tokens {
                let (name, value) = token
                    .split_once('=')
                    .ok_or_else(|| format!("invalid FASTA header field {token:?}"))?;
                observed_names.push(name);
                if fields.insert(name.to_owned(), value.to_owned()).is_some() {
                    return Err(format!("duplicate FASTA header field {name:?}"));
                }
            }
            let expected = [
                "schema_version",
                "length_bases",
                "k",
                "topology",
                "edge_steps",
                "canonical_kmers",
                "support_unit",
                "retention_min_support",
                "minimum_represented_key_support",
                "lower_median_represented_key_support",
                "maximum_represented_key_support",
                "placement_enumeration_status",
                "sequence_sha256",
            ];
            if observed_names != expected {
                return Err("FASTA header fields differ from the stable ordered grammar".to_owned());
            }
            records.push(FastaRecord {
                id: id.to_owned(),
                sequence: Vec::new(),
                line_lengths: Vec::new(),
                fields,
            });
        } else {
            let record = records
                .last_mut()
                .ok_or_else(|| "FASTA sequence precedes first header".to_owned())?;
            if line.is_empty()
                || line.len() > 80
                || !line.bytes().all(|base| b"ACGT".contains(&base))
            {
                return Err("invalid FASTA sequence line".to_owned());
            }
            record.sequence.extend_from_slice(line.as_bytes());
            record.line_lengths.push(line.len());
        }
    }
    for record in &records {
        validate_fasta_record(record)?;
    }
    let mut sorted = records.clone();
    sorted.sort_by(|left, right| {
        right
            .sequence
            .len()
            .cmp(&left.sequence.len())
            .then_with(|| left.sequence.cmp(&right.sequence))
            .then_with(|| left.id.cmp(&right.id))
    });
    if sorted.iter().map(|record| &record.id).collect::<Vec<_>>()
        != records.iter().map(|record| &record.id).collect::<Vec<_>>()
    {
        return Err("FASTA records are not in the declared total order".to_owned());
    }
    Ok(records)
}

fn field<'a>(record: &'a FastaRecord, name: &str) -> CheckResult<&'a str> {
    record
        .fields
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("{}: missing {name}", record.id))
}

fn validate_fasta_record(record: &FastaRecord) -> CheckResult {
    if record.line_lengths.is_empty()
        || record.line_lengths[..record.line_lengths.len() - 1]
            .iter()
            .any(|length| *length != 80)
    {
        return Err(format!("{}: FASTA is not wrapped at 80 bases", record.id));
    }
    let k = parse_u64(field(record, "k")?, "FASTA k")?;
    if !(3..=63).contains(&k) {
        return Err("FASTA k outside stable bounds".to_owned());
    }
    let length = parse_u64(field(record, "length_bases")?, "FASTA length")?;
    let steps = parse_u64(field(record, "edge_steps")?, "FASTA edge_steps")?;
    let canonical = parse_u64(field(record, "canonical_kmers")?, "FASTA canonical_kmers")?;
    if length != u64::try_from(record.sequence.len()).unwrap()
        || length != steps + k - 1
        || steps != canonical
        || steps > 1_000_000_000
        || canonical > 500_000_000
    {
        return Err(format!(
            "{}: inconsistent FASTA length/step fields",
            record.id
        ));
    }
    if field(record, "schema_version")? != "1.1"
        || !matches!(field(record, "topology")?, "linear" | "closed_graph_walk")
        || !matches!(
            field(record, "support_unit")?,
            "supplied_fragment_instance" | "accepted_window_occurrence"
        )
    {
        return Err(format!("{}: invalid stable FASTA enum", record.id));
    }
    let minimum = parse_u64(
        field(record, "minimum_represented_key_support")?,
        "minimum support",
    )?;
    let median = parse_u64(
        field(record, "lower_median_represented_key_support")?,
        "median support",
    )?;
    let maximum = parse_u64(
        field(record, "maximum_represented_key_support")?,
        "maximum support",
    )?;
    if minimum == 0 || minimum > median || median > maximum {
        return Err(format!("{}: invalid support order", record.id));
    }
    let sequence_digest = lower_sha256(&record.sequence);
    if field(record, "sequence_sha256")? != sequence_digest {
        return Err(format!("{}: FASTA sequence digest mismatch", record.id));
    }
    let topology_tag = if field(record, "topology")? == "linear" {
        0
    } else {
        1
    };
    let mut identity = Sha256::new();
    identity.update(b"veritasm:unitig:v1\0");
    identity.update([u8::try_from(k).unwrap(), topology_tag]);
    identity.update(length.to_le_bytes());
    identity.update(&record.sequence);
    let expected_id = format!("utg-{}", lower_hex(&identity.finalize()));
    if record.id != expected_id {
        return Err(format!("{}: content-derived unitig ID mismatch", record.id));
    }
    Ok(())
}

fn descriptor_columns(schema: &Value) -> CheckResult<Vec<&str>> {
    schema
        .get("columns")
        .and_then(Value::as_array)
        .ok_or_else(|| "TSV descriptor has no columns array".to_owned())?
        .iter()
        .map(|column| {
            column
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "TSV descriptor column has no name".to_owned())
        })
        .collect()
}

fn validate_tsv_field(value: &str, column: &Value, context: &str) -> CheckResult {
    let kind = column
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context}: descriptor type is missing"))?;
    let minimum = || {
        column
            .get("minimum")
            .and_then(Value::as_str)
            .map_or(Ok(0), |text| parse_u64(text, "descriptor minimum"))
    };
    match kind {
        "literal" => {
            if column.get("value").and_then(Value::as_str) != Some(value) {
                return Err(format!("{context}: literal mismatch"));
            }
        }
        "enum" | "ordered_enum" => {
            let allowed = column
                .get("values")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("{context}: enum has no values"))?;
            if !allowed.iter().any(|item| item.as_str() == Some(value)) {
                return Err(format!("{context}: value outside enum"));
            }
        }
        "u64_decimal" => {
            let parsed = parse_u64(value, context)?;
            if parsed < minimum()? {
                return Err(format!("{context}: value below minimum"));
            }
        }
        "u32_decimal" => {
            let parsed = parse_u64(value, context)?;
            let maximum = column
                .get("maximum")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{context}: u32 maximum is missing"))?
                .parse::<u64>()
                .map_err(|_| format!("{context}: invalid u32 maximum"))?;
            if parsed < minimum()? || parsed > maximum || parsed > u64::from(u32::MAX) {
                return Err(format!("{context}: value outside u32 descriptor bounds"));
            }
        }
        "u8_decimal" => {
            let parsed = parse_u64(value, context)?;
            let maximum = column
                .get("maximum")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{context}: u8 maximum is missing"))?
                .parse::<u64>()
                .map_err(|_| format!("{context}: invalid u8 maximum"))?;
            if parsed < minimum()? || parsed > maximum || parsed > u64::from(u8::MAX) {
                return Err(format!("{context}: value outside u8 descriptor bounds"));
            }
        }
        "u64_decimal_or_NA" => {
            if value != "NA" {
                let parsed = parse_u64(value, context)?;
                if parsed < minimum()? {
                    return Err(format!("{context}: value below minimum"));
                }
            }
        }
        "stable_content_derived_identifier" => {
            if !value.strip_prefix("utg-").is_some_and(is_lower_sha256) {
                return Err(format!("{context}: invalid unitig identifier"));
            }
        }
        "lowercase_sha256" => {
            if !is_lower_sha256(value) {
                return Err(format!("{context}: invalid lowercase SHA-256"));
            }
        }
        "canonical_json_object" => {
            let parsed: Value = serde_json::from_str(value)
                .map_err(|error| format!("{context}: invalid embedded JSON: {error}"))?;
            let object = parsed
                .as_object()
                .ok_or_else(|| format!("{context}: embedded JSON is not an object"))?;
            let mut keys = object.keys().collect::<Vec<_>>();
            let observed = keys.clone();
            keys.sort();
            if observed != keys || serde_json::to_string(&parsed).unwrap() != value {
                return Err(format!("{context}: embedded JSON is not canonical"));
            }
        }
        other => {
            return Err(format!(
                "{context}: unsupported TSV descriptor type {other:?}"
            ))
        }
    }
    Ok(())
}

fn parse_tsv(path: &Path, descriptor: &Path) -> CheckResult<Vec<Vec<String>>> {
    let schema = json(descriptor);
    if schema
        .get("descriptor_schema_version")
        .and_then(Value::as_str)
        != Some("1.0")
        || schema.get("encoding").and_then(Value::as_str) != Some("UTF-8")
        || schema.get("line_ending").and_then(Value::as_str) != Some("LF")
        || schema.get("delimiter").and_then(Value::as_str) != Some("TAB")
        || schema.get("header").and_then(Value::as_bool) != Some(true)
    {
        return Err("unsupported stable TSV descriptor envelope".to_owned());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if !bytes.ends_with(b"\n") || bytes.contains(&b'\r') {
        return Err(format!(
            "{}: TSV must use LF and end with LF",
            path.display()
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let mut lines = text.lines();
    let columns = descriptor_columns(&schema)?;
    let header = lines
        .next()
        .ok_or_else(|| format!("{}: TSV has no header", path.display()))?;
    if header.split('\t').collect::<Vec<_>>() != columns {
        return Err(format!("{}: TSV header mismatch", path.display()));
    }
    let column_descriptors = schema["columns"].as_array().unwrap();
    let mut rows = Vec::new();
    for (row_index, line) in lines.enumerate() {
        let values = line.split('\t').collect::<Vec<_>>();
        if values.len() != columns.len() {
            return Err(format!(
                "{} row {}: expected {} columns, found {}",
                path.display(),
                row_index + 2,
                columns.len(),
                values.len()
            ));
        }
        for (column_index, (value, column)) in values.iter().zip(column_descriptors).enumerate() {
            validate_tsv_field(
                value,
                column,
                &format!("{}:{}:{}", path.display(), row_index + 2, column_index + 1),
            )?;
        }
        rows.push(values.into_iter().map(str::to_owned).collect());
    }
    Ok(rows)
}

fn oriented_sequence(record: &FastaRecord, orientation: &str) -> CheckResult<Vec<u8>> {
    match orientation {
        "+" => Ok(record.sequence.clone()),
        "-" => reverse_complement(&record.sequence),
        _ => Err("invalid GFA orientation".to_owned()),
    }
}

fn orientation_rank(value: &str) -> CheckResult<u8> {
    match value {
        "+" => Ok(0),
        "-" => Ok(1),
        _ => Err(format!("invalid orientation {value:?}")),
    }
}

fn validate_gfa(path: &Path, records: &[FastaRecord]) -> CheckResult<usize> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if !bytes.ends_with(b"\n") || bytes.contains(&b'\r') {
        return Err("GFA must use LF and end with LF".to_owned());
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| "GFA has no header".to_owned())?
        .split('\t')
        .collect::<Vec<_>>();
    if header.len() != 5
        || header[0..3] != ["H", "VN:Z:1.0", "PN:Z:veritasm"]
        || header[3] != format!("PV:Z:{}", env!("CARGO_PKG_VERSION"))
        || header[4] != "SC:Z:1.1"
    {
        return Err("invalid stable GFA header".to_owned());
    }
    let by_id = records
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut segment_index = 0usize;
    let mut saw_link = false;
    let mut previous_link: Option<(String, u8, String, u8, u64)> = None;
    let mut link_count = 0usize;
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.first().copied() {
            Some("S") => {
                if saw_link || fields.len() != 7 || segment_index >= records.len() {
                    return Err("invalid or out-of-order GFA segment".to_owned());
                }
                let record = &records[segment_index];
                let expected = [
                    "S".to_owned(),
                    record.id.clone(),
                    String::from_utf8(record.sequence.clone()).unwrap(),
                    format!("TP:Z:{}", field(record, "topology")?),
                    format!("ES:i:{}", field(record, "edge_steps")?),
                    format!("CK:i:{}", field(record, "canonical_kmers")?),
                    format!(
                        "SH:H:{}",
                        field(record, "sequence_sha256")?.to_ascii_uppercase()
                    ),
                ];
                if fields != expected.iter().map(String::as_str).collect::<Vec<_>>() {
                    return Err(format!("{}: GFA segment disagrees with FASTA", record.id));
                }
                segment_index += 1;
            }
            Some("L") => {
                saw_link = true;
                link_count += 1;
                if segment_index != records.len() || fields.len() != 6 {
                    return Err("GFA links must follow every segment".to_owned());
                }
                let from = by_id
                    .get(fields[1])
                    .ok_or_else(|| "GFA link has unknown source segment".to_owned())?;
                let to = by_id
                    .get(fields[3])
                    .ok_or_else(|| "GFA link has unknown target segment".to_owned())?;
                let from_rank = orientation_rank(fields[2])?;
                let to_rank = orientation_rank(fields[4])?;
                let overlap = fields[5]
                    .strip_suffix('M')
                    .ok_or_else(|| "GFA link overlap is not an M operation".to_owned())?;
                let overlap = parse_u64(overlap, "GFA overlap")?;
                let expected_overlap = parse_u64(field(from, "k")?, "GFA k")? - 1;
                if overlap != expected_overlap || field(from, "k")? != field(to, "k")? {
                    return Err("GFA link overlap differs from k-1".to_owned());
                }
                let from_sequence = oriented_sequence(from, fields[2])?;
                let to_sequence = oriented_sequence(to, fields[4])?;
                let overlap_usize = usize::try_from(overlap).unwrap();
                if from_sequence[from_sequence.len() - overlap_usize..]
                    != to_sequence[..overlap_usize]
                {
                    return Err("GFA link sequence overlap mismatch".to_owned());
                }
                let key = (
                    fields[1].to_owned(),
                    from_rank,
                    fields[3].to_owned(),
                    to_rank,
                    overlap,
                );
                let reverse_complement_key = (
                    fields[3].to_owned(),
                    1 - to_rank,
                    fields[1].to_owned(),
                    1 - from_rank,
                    overlap,
                );
                if key > reverse_complement_key {
                    return Err("GFA link is not the canonical relation representative".to_owned());
                }
                if previous_link
                    .as_ref()
                    .is_some_and(|previous| previous >= &key)
                {
                    return Err("GFA links are not strictly sorted and unique".to_owned());
                }
                previous_link = Some(key);
            }
            _ => return Err("GFA contains an unsupported record type".to_owned()),
        }
    }
    if segment_index != records.len() {
        return Err("GFA segment count differs from FASTA".to_owned());
    }
    Ok(link_count)
}

fn json_decimal(value: &Value, pointer: &str) -> CheckResult<u64> {
    let text = value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("run.json {pointer} is not a decimal string"))?;
    parse_u64(text, pointer)
}

fn validate_unitig_evidence(
    rows: &[Vec<String>],
    records: &[FastaRecord],
    run_record: &Value,
) -> CheckResult {
    if rows.len() != records.len() {
        return Err("unitig evidence row count differs from FASTA".to_owned());
    }
    let k = run_record["parameters"]["k"]
        .as_u64()
        .ok_or_else(|| "run k is not an integer".to_owned())?;
    let support_unit = run_record["parameters"]["support_unit"]
        .as_str()
        .ok_or_else(|| "run support unit is not a string".to_owned())?;
    let threshold = run_record["parameters"]["retention_min_support_decimal"]
        .as_str()
        .ok_or_else(|| "run threshold is not a string".to_owned())?;
    let global_status = run_record["read_audit"]["global_enumeration_status"]
        .as_str()
        .ok_or_else(|| "run read-audit status is not a string".to_owned())?;
    let mut placement_sum = 0u64;
    let mut single_sum = 0u64;
    let mut multi_sum = 0u64;
    for (row, record) in rows.iter().zip(records) {
        if row[0] != "1.1"
            || row[1] != record.id
            || row[2] != field(record, "length_bases")?
            || row[3] != k.to_string()
            || row[4] != field(record, "topology")?
            || row[5] != field(record, "edge_steps")?
            || row[6] != field(record, "canonical_kmers")?
            || row[7] != support_unit
            || row[8] != threshold
            || row[9] != field(record, "minimum_represented_key_support")?
            || row[10] != field(record, "lower_median_represented_key_support")?
            || row[11] != field(record, "maximum_represented_key_support")?
            || row[15] != field(record, "placement_enumeration_status")?
            || row[16] != field(record, "sequence_sha256")?
        {
            return Err(format!("{}: FASTA/evidence disagreement", record.id));
        }
        let expected_status = if row[4] == "closed_graph_walk" {
            "unavailable_closed_walk_audit_unsupported"
        } else {
            match global_status {
                "placement_enumeration_complete" => "placement_enumeration_complete",
                "indeterminate_candidate_limit" => "indeterminate_candidate_limit",
                "unavailable_remap_disabled" => "unavailable_remap_disabled",
                _ => return Err("unknown global placement status".to_owned()),
            }
        };
        if row[15] != expected_status {
            return Err(format!("{}: invalid row placement status", record.id));
        }
        if expected_status == "placement_enumeration_complete" {
            placement_sum = placement_sum
                .checked_add(parse_u64(&row[12], "unitig placement total")?)
                .ok_or_else(|| "unitig placement sum overflow".to_owned())?;
            single_sum = single_sum
                .checked_add(parse_u64(&row[13], "unitig single total")?)
                .ok_or_else(|| "unitig single sum overflow".to_owned())?;
            multi_sum = multi_sum
                .checked_add(parse_u64(&row[14], "unitig multi total")?)
                .ok_or_else(|| "unitig multi sum overflow".to_owned())?;
        } else if row[12..=14] != ["NA", "NA", "NA"] {
            return Err(format!(
                "{}: unavailable audit fields are not NA",
                record.id
            ));
        }
    }
    let totals = &run_record["read_audit"]["placement_totals"];
    if global_status == "placement_enumeration_complete" {
        for (name, observed) in [
            ("enumeration_complete_read_placements", placement_sum),
            ("single_group_read_instances", single_sum),
            ("multi_group_read_instance_unitig_memberships", multi_sum),
        ] {
            let expected = totals[name]
                .as_str()
                .ok_or_else(|| format!("run placement total {name} must be numeric"))?;
            if parse_u64(expected, name)? != observed {
                return Err(format!("run placement total {name} does not reconcile"));
            }
        }
    } else {
        let expected_reason = if global_status == "unavailable_remap_disabled" {
            "remap_disabled"
        } else {
            "candidate_limit"
        };
        for name in [
            "enumeration_complete_read_placements",
            "single_group_read_instances",
            "multi_group_read_instance_unitig_memberships",
        ] {
            if totals[name]["status"] != "not_available"
                || totals[name]["reason"] != expected_reason
            {
                return Err(format!(
                    "run placement total {name} has an invalid NA state"
                ));
            }
        }
    }
    Ok(())
}

fn end_rank(value: &str) -> CheckResult<u8> {
    match value {
        "L" => Ok(0),
        "R" => Ok(1),
        _ => Err(format!("invalid segment end {value:?}")),
    }
}

fn role_rank(value: &str) -> CheckResult<u8> {
    match value {
        "R1" => Ok(0),
        "R2" => Ok(1),
        _ => Err(format!("invalid pair role {value:?}")),
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PairEndpoint {
    segment: String,
    end: u8,
    strand: u8,
    distance: u64,
    role: u8,
}

fn validate_pair_artifacts(
    links: &[Vec<String>],
    summary: &[Vec<String>],
    records: &[FastaRecord],
    run_record: &Value,
) -> CheckResult {
    let k = run_record["parameters"]["k"]
        .as_u64()
        .ok_or_else(|| "run k is not an integer".to_owned())?;
    let by_id = records
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let paired_lanes = run_record["input"]["sources"]
        .as_array()
        .ok_or_else(|| "run input sources is not an array".to_owned())?
        .iter()
        .filter_map(|source| {
            matches!(source["role"].as_str(), Some("R1" | "R2"))
                .then(|| source["lane_ordinal"].as_u64())
                .flatten()
        })
        .collect::<BTreeSet<_>>();
    let mut link_support = 0u64;
    let mut prior_key = None;
    for row in links {
        if row[1] != k.to_string() || row[3] == row[8] {
            return Err("pair-link k or distinct-segment invariant failed".to_owned());
        }
        let lane = parse_u64(&row[2], "pair-link lane")?;
        if !paired_lanes.contains(&lane) {
            return Err("pair link names a non-paired input lane".to_owned());
        }
        let first_record = by_id
            .get(row[3].as_str())
            .ok_or_else(|| "pair link source ID is absent from FASTA".to_owned())?;
        let second_record = by_id
            .get(row[8].as_str())
            .ok_or_else(|| "pair link target ID is absent from FASTA".to_owned())?;
        if field(first_record, "topology")? != "linear"
            || field(second_record, "topology")? != "linear"
        {
            return Err("pair link references a closed graph walk".to_owned());
        }
        let first = PairEndpoint {
            segment: row[3].clone(),
            end: end_rank(&row[4])?,
            strand: orientation_rank(&row[5])?,
            distance: parse_u64(&row[6], "pair-link first end distance")?,
            role: role_rank(&row[7])?,
        };
        let second = PairEndpoint {
            segment: row[8].clone(),
            end: end_rank(&row[9])?,
            strand: orientation_rank(&row[10])?,
            distance: parse_u64(&row[11], "pair-link second end distance")?,
            role: role_rank(&row[12])?,
        };
        if first.distance >= u64::try_from(first_record.sequence.len()).unwrap()
            || second.distance >= u64::try_from(second_record.sequence.len()).unwrap()
            || first > second
        {
            return Err("pair-link endpoint bounds or swap canonicalization failed".to_owned());
        }
        let key = (lane, first, second);
        if prior_key.as_ref().is_some_and(|prior| prior >= &key) {
            return Err("pair-link rows are not strictly sorted and unique".to_owned());
        }
        prior_key = Some(key);
        link_support = link_support
            .checked_add(parse_u64(&row[13], "pair-link support")?)
            .ok_or_else(|| "pair-link support overflow".to_owned())?;
    }

    let mode = run_record["input"]["mode"]
        .as_str()
        .ok_or_else(|| "run input mode is missing".to_owned())?;
    let remap = run_record["parameters"]["remap"]
        .as_bool()
        .ok_or_else(|| "run remap flag is missing".to_owned())?;
    let expected_states: &[&str] = match (mode, remap) {
        ("single_end", _) => &["not_paired_input"],
        ("paired_end", false) => &["remap_not_requested"],
        ("paired_end", true) => &[
            "mate_ineligible",
            "mate_indeterminate_candidate_limit",
            "mate_unmapped",
            "mate_multiple_placement_groups",
            "same_linear_unitig",
            "endpoint_tie",
            "cross_unitig_observation",
        ],
        _ => return Err("invalid input mode".to_owned()),
    };
    if summary.len() != expected_states.len() {
        return Err("pair summary has the wrong conditional row count".to_owned());
    }
    let mut summary_total = 0u64;
    let mut cross_support = 0u64;
    for (row, expected_state) in summary.iter().zip(expected_states) {
        if row[1] != k.to_string() || row[2] != *expected_state {
            return Err("pair summary state order or k mismatch".to_owned());
        }
        let count = parse_u64(&row[3], "pair summary count")?;
        summary_total = summary_total
            .checked_add(count)
            .ok_or_else(|| "pair summary count overflow".to_owned())?;
        if row[2] == "cross_unitig_observation" {
            cross_support = count;
        }
    }
    if summary_total != json_decimal(run_record, "/input/fragments_decimal")?
        || cross_support != link_support
        || json_decimal(run_record, "/pair_audit/link_support_decimal")? != link_support
        || json_decimal(run_record, "/pair_audit/link_groups_decimal")?
            != u64::try_from(links.len()).unwrap()
    {
        return Err("pair artifact totals do not reconcile".to_owned());
    }
    let run_states = run_record["pair_audit"]["state_counts"]
        .as_array()
        .ok_or_else(|| "run pair state counts is not an array".to_owned())?;
    if run_states.len() != summary.len() {
        return Err("run pair states differ from TSV".to_owned());
    }
    for (run_state, row) in run_states.iter().zip(summary) {
        if run_state["state"] != row[2]
            || run_state["supplied_fragment_instances_decimal"] != row[3]
        {
            return Err("run pair state differs from TSV".to_owned());
        }
    }
    Ok(())
}

fn validate_transform_artifact(rows: &[Vec<String>], run_record: &Value) -> CheckResult {
    if rows.len() != 2 || rows[0][1] != "0" || rows[1][1] != "1" {
        return Err("transform summary does not contain exact ordered stages 0 and 1".to_owned());
    }
    let run_rows = run_record["transformations"]
        .as_array()
        .ok_or_else(|| "run transformations is not an array".to_owned())?;
    if run_rows.len() != rows.len() {
        return Err("run transformation count differs from TSV".to_owned());
    }
    let names = [
        "stage_order",
        "algorithm_id",
        "algorithm_version",
        "support_unit",
        "parameters_json",
        "input_distinct_canonical_keys_decimal",
        "output_distinct_canonical_keys_decimal",
        "input_support_mass_decimal",
        "output_support_mass_decimal",
        "removed_key_count_decimal",
        "removed_support_mass_decimal",
        "decision_set_sha256",
        "pre_state_sha256",
        "post_state_sha256",
        "status",
    ];
    for (row, run_row) in rows.iter().zip(run_rows) {
        for (index, name) in names.iter().enumerate() {
            let expected = if *name == "stage_order" {
                run_row[*name].as_u64().map(|value| value.to_string())
            } else {
                run_row[*name].as_str().map(str::to_owned)
            }
            .ok_or_else(|| format!("run transformation field {name} has the wrong type"))?;
            if row[index + 1] != expected {
                return Err(format!("transform TSV differs from run field {name}"));
            }
        }
    }
    Ok(())
}

fn validate_run_semantics(
    run_record: &Value,
    records: &[FastaRecord],
    graph_links: usize,
) -> CheckResult {
    for (observed, maximum) in [
        (
            "/input/raw_transport_bytes_decimal",
            "/parameters/limits/max_raw_transport_bytes_decimal",
        ),
        (
            "/input/gzip_members_decimal",
            "/parameters/limits/max_gzip_members_decimal",
        ),
    ] {
        if json_decimal(run_record, observed)? > json_decimal(run_record, maximum)? {
            return Err(format!(
                "run observed field {observed} exceeds effective limit {maximum}"
            ));
        }
    }
    let unitig_count = u64::try_from(records.len()).unwrap();
    let linear_count = u64::try_from(
        records
            .iter()
            .filter(|record| record.fields["topology"] == "linear")
            .count(),
    )
    .unwrap();
    let closed_count = unitig_count - linear_count;
    let represented_keys = records.iter().try_fold(0u64, |sum, record| {
        sum.checked_add(parse_u64(
            field(record, "canonical_kmers")?,
            "represented k-mers",
        )?)
        .ok_or_else(|| "represented k-mer total overflow".to_owned())
    })?;
    for (pointer, expected) in [
        ("/graph/unitigs_decimal", unitig_count),
        ("/graph/linear_unitigs_decimal", linear_count),
        ("/graph/closed_graph_walks_decimal", closed_count),
        (
            "/graph/graph_links_decimal",
            u64::try_from(graph_links).unwrap(),
        ),
        ("/graph/retained_canonical_keys_decimal", represented_keys),
        (
            "/counting/retained_distinct_canonical_keys_decimal",
            represented_keys,
        ),
    ] {
        if json_decimal(run_record, pointer)? != expected {
            return Err(format!("run.json {pointer} does not reconcile"));
        }
    }
    let status = run_record["status"]["code"]
        .as_str()
        .ok_or_else(|| "run status code is missing".to_owned())?;
    if (records.is_empty() && status != "software_run_complete_no_unitigs_under_parameters")
        || (!records.is_empty() && status != "software_run_complete")
    {
        return Err("run status disagrees with the emitted unitig count".to_owned());
    }
    let possible = json_decimal(run_record, "/windows/possible_decimal")?;
    let classified = [
        "/windows/accepted_decimal",
        "/windows/ambiguity_only_decimal",
        "/windows/quality_only_decimal",
        "/windows/ambiguity_and_quality_decimal",
    ]
    .into_iter()
    .try_fold(0u64, |sum, pointer| {
        sum.checked_add(json_decimal(run_record, pointer)?)
            .ok_or_else(|| "window partition total overflow".to_owned())
    })?;
    if possible != classified {
        return Err("run window partition does not reconcile".to_owned());
    }
    for pair in [
        ("/input/fragments_decimal", "/spool/fragments_decimal"),
        ("/input/reads_decimal", "/spool/reads_decimal"),
        (
            "/counting/retained_support_mass_decimal",
            "/graph/retained_support_mass_decimal",
        ),
    ] {
        if json_decimal(run_record, pair.0)? != json_decimal(run_record, pair.1)? {
            return Err(format!("run fields {} and {} disagree", pair.0, pair.1));
        }
    }
    let k = run_record["parameters"]["k"]
        .as_u64()
        .ok_or_else(|| "run k is not an integer".to_owned())?;
    let support_unit = run_record["parameters"]["support_unit"]
        .as_str()
        .ok_or_else(|| "run support unit is missing".to_owned())?;
    let threshold = run_record["parameters"]["retention_min_support_decimal"]
        .as_str()
        .ok_or_else(|| "run retention threshold is missing".to_owned())?;
    for record in records {
        if field(record, "k")? != k.to_string()
            || field(record, "support_unit")? != support_unit
            || field(record, "retention_min_support")? != threshold
        {
            return Err(format!("{}: FASTA/run parameter mismatch", record.id));
        }
    }

    let sources = run_record["input"]["sources"]
        .as_array()
        .ok_or_else(|| "run sources is not an array".to_owned())?;
    let mut previous_source = None;
    let mut source_reads = 0u64;
    let mut source_bases = 0u64;
    for source in sources {
        let lane = source["lane_ordinal"]
            .as_u64()
            .ok_or_else(|| "source lane ordinal is not an integer".to_owned())?;
        let role = source["role"]
            .as_str()
            .ok_or_else(|| "source role is missing".to_owned())?;
        let role_order = match role {
            "S" => 0,
            "R1" => 1,
            "R2" => 2,
            _ => return Err("invalid source role".to_owned()),
        };
        let key = (lane, role_order);
        if previous_source.is_some_and(|previous| previous >= key) {
            return Err("input sources are not strictly sorted".to_owned());
        }
        previous_source = Some(key);
        if source["label"] != format!("lane-{lane:06}-{role}") {
            return Err("source label disagrees with lane and role".to_owned());
        }
        source_reads = source_reads
            .checked_add(parse_u64(
                source["records_decimal"]
                    .as_str()
                    .ok_or_else(|| "source records are missing".to_owned())?,
                "source records",
            )?)
            .ok_or_else(|| "source record total overflow".to_owned())?;
        source_bases = source_bases
            .checked_add(parse_u64(
                source["bases_decimal"]
                    .as_str()
                    .ok_or_else(|| "source bases are missing".to_owned())?,
                "source bases",
            )?)
            .ok_or_else(|| "source base total overflow".to_owned())?;
    }
    if source_reads != json_decimal(run_record, "/input/reads_decimal")?
        || source_bases != json_decimal(run_record, "/input/bases_decimal")?
    {
        return Err("input source totals do not reconcile".to_owned());
    }

    let remap = run_record["parameters"]["remap"]
        .as_bool()
        .ok_or_else(|| "run remap flag is missing".to_owned())?;
    let input_mode = run_record["input"]["mode"]
        .as_str()
        .ok_or_else(|| "run input mode is missing".to_owned())?;
    let expected_roles: &[&str] = if input_mode == "single_end" {
        &["S"]
    } else {
        &["R1", "R2"]
    };
    let expected_states: &[&str] = if remap {
        &[
            "ineligible_ambiguity_or_quality",
            "indeterminate_candidate_limit",
            "unmapped",
            "single_placement_group",
            "multiple_placement_groups",
        ]
    } else {
        &["not_requested"]
    };
    let state_counts = run_record["read_audit"]["state_counts"]
        .as_array()
        .ok_or_else(|| "read state counts is not an array".to_owned())?;
    if state_counts.len() != expected_roles.len() * expected_states.len() {
        return Err("read state count has the wrong conditional row set".to_owned());
    }
    let mut candidate_limit = 0u64;
    for (role_index, role) in expected_roles.iter().enumerate() {
        let mut role_total = 0u64;
        for (state_index, state) in expected_states.iter().enumerate() {
            let row = &state_counts[role_index * expected_states.len() + state_index];
            if row["mate_role"] != *role || row["state"] != *state {
                return Err("read state rows are not in the conditional order".to_owned());
            }
            let count = parse_u64(
                row["read_instances_decimal"]
                    .as_str()
                    .ok_or_else(|| "read state count is missing".to_owned())?,
                "read state count",
            )?;
            role_total = role_total
                .checked_add(count)
                .ok_or_else(|| "read role total overflow".to_owned())?;
            if *state == "indeterminate_candidate_limit" {
                candidate_limit = candidate_limit
                    .checked_add(count)
                    .ok_or_else(|| "candidate-limit total overflow".to_owned())?;
            }
        }
        let expected_role_total = if input_mode == "single_end" {
            json_decimal(run_record, "/input/reads_decimal")?
        } else {
            json_decimal(run_record, "/input/fragments_decimal")?
        };
        if role_total != expected_role_total {
            return Err(format!("read state rows for {role} do not reconcile"));
        }
    }
    let expected_global = if !remap {
        "unavailable_remap_disabled"
    } else if candidate_limit > 0 {
        "indeterminate_candidate_limit"
    } else {
        "placement_enumeration_complete"
    };
    if run_record["read_audit"]["global_enumeration_status"] != expected_global {
        return Err("global read enumeration status does not reconcile".to_owned());
    }
    Ok(())
}

fn scientific_digest(bundle: &Path) -> CheckResult<String> {
    let mut digest = Sha256::new();
    for relative in [
        "assembly.gfa",
        "pair_audit_summary.tsv",
        "pair_links.tsv",
        "transform_summary.tsv",
        "unitig_evidence.tsv",
        "unitigs.fasta",
    ] {
        let bytes = fs::read(bundle.join(relative)).map_err(|error| error.to_string())?;
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(lower_sha256(&bytes).as_bytes());
        digest.update(b"\n");
    }
    Ok(lower_hex(&digest.finalize()))
}

fn first_line(path: &Path) -> CheckResult<String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    text.lines()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} is empty", path.display()))
}

fn validate_stable_bundle(bundle: &Path) -> CheckResult {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in [
        "schema/run.schema.json",
        "schema/assembly_gfa.schema.json",
        "schema/unitig_evidence.schema.json",
        "schema/pair_links.schema.json",
        "schema/pair_audit_summary.schema.json",
        "schema/transform_summary.schema.json",
        "schema/manifest.json",
    ] {
        let emitted = fs::read(bundle.join(relative)).map_err(|error| error.to_string())?;
        let source = fs::read(repository.join(relative)).map_err(|error| error.to_string())?;
        if emitted != source {
            return Err(format!(
                "emitted schema {relative} differs from source schema"
            ));
        }
    }

    let schema = json(&bundle.join("schema/run.schema.json"));
    check_supported_schema(&schema, "$")?;
    if schema.get("$schema").and_then(Value::as_str)
        != Some("https://json-schema.org/draft/2020-12/schema")
        || schema.get("$id").and_then(Value::as_str)
            != Some("https://veritasm.invalid/schema/run-1.2.json")
        || schema.get("title").and_then(Value::as_str)
            != Some("VeritAsm deterministic run record 1.2")
    {
        return Err("run schema draft/id/title envelope is not the frozen 1.2 contract".to_owned());
    }
    let run_record = json(&bundle.join("run.json"));
    validate_json_schema(&run_record, &schema, &schema, "$")?;

    if first_line(&bundle.join("unitig_evidence.tsv"))? != UNITIG_HEADER
        || first_line(&bundle.join("pair_links.tsv"))? != PAIR_LINK_HEADER
        || first_line(&bundle.join("pair_audit_summary.tsv"))? != PAIR_SUMMARY_HEADER
        || first_line(&bundle.join("transform_summary.tsv"))? != TRANSFORM_HEADER
    {
        return Err("one or more stable TSV literal headers changed".to_owned());
    }
    let records = parse_fasta(&bundle.join("unitigs.fasta"))?;
    let graph_links = validate_gfa(&bundle.join("assembly.gfa"), &records)?;
    let unitig_rows = parse_tsv(
        &bundle.join("unitig_evidence.tsv"),
        &bundle.join("schema/unitig_evidence.schema.json"),
    )?;
    let pair_links = parse_tsv(
        &bundle.join("pair_links.tsv"),
        &bundle.join("schema/pair_links.schema.json"),
    )?;
    let pair_summary = parse_tsv(
        &bundle.join("pair_audit_summary.tsv"),
        &bundle.join("schema/pair_audit_summary.schema.json"),
    )?;
    let transforms = parse_tsv(
        &bundle.join("transform_summary.tsv"),
        &bundle.join("schema/transform_summary.schema.json"),
    )?;
    validate_run_semantics(&run_record, &records, graph_links)?;
    validate_unitig_evidence(&unitig_rows, &records, &run_record)?;
    validate_pair_artifacts(&pair_links, &pair_summary, &records, &run_record)?;
    validate_transform_artifact(&transforms, &run_record)?;
    if run_record["scientific_artifacts_digest"] != scientific_digest(bundle)? {
        return Err("scientific artifact digest does not reconcile".to_owned());
    }
    Ok(())
}

fn write_json(path: &Path, value: &Value) {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    let mut entries = fs::read_dir(source)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        let target = destination.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            fs::copy(path, target).unwrap();
        }
    }
}

fn assert_schema_rejects(instance: &Value, schema: &Value, label: &str) {
    assert!(
        validate_json_schema(instance, schema, schema, "$").is_err(),
        "stable run schema accepted {label} mutation"
    );
}

#[test]
fn emitted_stable_bundle_passes_independent_schema_and_cross_artifact_checks() {
    let temporary = TempDir::new().unwrap();
    let bundle = emit_stable_bundle(temporary.path());
    validate_stable_bundle(&bundle).unwrap();

    let linked = emit_linked_stable_bundle(temporary.path());
    let gfa = fs::read_to_string(linked.join("assembly.gfa")).unwrap();
    assert_eq!(
        gfa.lines().filter(|line| line.starts_with("L\t")).count(),
        2
    );
    validate_stable_bundle(&linked).unwrap();
}

#[test]
fn stable_run_schema_rejects_name_enum_bound_missing_extra_and_conditional_mutations() {
    let temporary = TempDir::new().unwrap();
    let bundle = emit_stable_bundle(temporary.path());
    let schema = json(&bundle.join("schema/run.schema.json"));
    check_supported_schema(&schema, "$").unwrap();
    let valid = json(&bundle.join("run.json"));
    validate_json_schema(&valid, &schema, &schema, "$").unwrap();

    let mut changed = valid.clone();
    changed["software"]["name"] = Value::String("not-veritasm".to_owned());
    assert_schema_rejects(&changed, &schema, "software name");

    let mut changed = valid.clone();
    changed["status"]["code"] = Value::String("complete".to_owned());
    assert_schema_rejects(&changed, &schema, "status enum");

    let mut changed = valid.clone();
    changed["parameters"]["k"] = Value::from(64);
    assert_schema_rejects(&changed, &schema, "k upper bound");

    let mut changed = valid.clone();
    changed.as_object_mut().unwrap().remove("graph");
    assert_schema_rejects(&changed, &schema, "missing required field");

    let mut changed = valid.clone();
    changed
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_owned(), Value::Bool(true));
    assert_schema_rejects(&changed, &schema, "additional property");

    let mut changed = valid;
    changed["status"]["code"] =
        Value::String("software_run_complete_no_unitigs_under_parameters".to_owned());
    assert_schema_rejects(&changed, &schema, "code/message conditional");
}

#[test]
fn test_schema_validator_fails_closed_on_unknown_schema_surface() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let valid = json(&repository.join("schema/run.schema.json"));

    let mut unknown_keyword = valid.clone();
    unknown_keyword
        .as_object_mut()
        .unwrap()
        .insert("unevaluatedProperties".to_owned(), Value::Bool(false));
    let error = check_supported_schema(&unknown_keyword, "$").unwrap_err();
    assert!(error.contains("does not implement schema keyword"));

    let mut unknown_pattern = valid.clone();
    unknown_pattern["$defs"]["sha256"]["pattern"] = Value::String(".*".to_owned());
    let error = check_supported_schema(&unknown_pattern, "$").unwrap_err();
    assert!(error.contains("does not implement schema pattern"));

    let mut unresolved_reference = valid;
    unresolved_reference["properties"]["spool"]["properties"]["sha256"]["$ref"] =
        Value::String("#/$defs/not_present".to_owned());
    let error = check_supported_schema(&unresolved_reference, "$").unwrap_err();
    assert!(error.contains("unresolved reference"));
}

#[test]
fn semantic_checker_rejects_observations_above_effective_transport_limits() {
    let temporary = TempDir::new().unwrap();
    let bundle = emit_stable_bundle(temporary.path());

    for (ordinal, (observed_pointer, maximum_pointer)) in [
        (
            "/input/raw_transport_bytes_decimal",
            "/parameters/limits/max_raw_transport_bytes_decimal",
        ),
        (
            "/input/gzip_members_decimal",
            "/parameters/limits/max_gzip_members_decimal",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mutated = temporary
            .path()
            .join(format!("transport-limit-mutation-{ordinal}"));
        copy_tree(&bundle, &mutated);
        let mut run_record = json(&mutated.join("run.json"));
        let maximum = json_decimal(&run_record, maximum_pointer).unwrap();
        let invalid = maximum.checked_add(1).unwrap();
        *run_record.pointer_mut(observed_pointer).unwrap() = Value::String(invalid.to_string());
        let schema = json(&mutated.join("schema/run.schema.json"));
        assert!(validate_json_schema(&run_record, &schema, &schema, "$").is_ok());
        write_json(&mutated.join("run.json"), &run_record);
        assert!(validate_stable_bundle(&mutated).is_err());
    }
}

#[test]
fn semantic_checker_rejects_na_order_and_cross_artifact_identifier_mutations() {
    let temporary = TempDir::new().unwrap();
    let bundle = emit_stable_bundle(temporary.path());

    let na_bundle = temporary.path().join("na-mutation");
    copy_tree(&bundle, &na_bundle);
    let mut run_record = json(&na_bundle.join("run.json"));
    run_record["read_audit"]["placement_totals"]["enumeration_complete_read_placements"] =
        serde_json::json!({"status": "not_available", "reason": "remap_disabled"});
    let schema = json(&na_bundle.join("schema/run.schema.json"));
    assert!(validate_json_schema(&run_record, &schema, &schema, "$").is_ok());
    write_json(&na_bundle.join("run.json"), &run_record);
    assert!(validate_stable_bundle(&na_bundle).is_err());

    let order_bundle = temporary.path().join("order-mutation");
    copy_tree(&bundle, &order_bundle);
    let fasta_path = order_bundle.join("unitigs.fasta");
    let text = fs::read_to_string(&fasta_path).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 4);
    fs::write(
        &fasta_path,
        format!("{}\n{}\n{}\n{}\n", lines[2], lines[3], lines[0], lines[1]),
    )
    .unwrap();
    assert!(validate_stable_bundle(&order_bundle).is_err());

    let id_bundle = temporary.path().join("id-mutation");
    copy_tree(&bundle, &id_bundle);
    let evidence_path = id_bundle.join("unitig_evidence.tsv");
    let evidence = fs::read_to_string(&evidence_path).unwrap();
    let original_id = evidence.lines().nth(1).unwrap().split('\t').nth(1).unwrap();
    let replacement = format!("utg-{}", "0".repeat(64));
    fs::write(
        &evidence_path,
        evidence.replacen(original_id, &replacement, 1),
    )
    .unwrap();
    assert!(validate_stable_bundle(&id_bundle).is_err());

    let linked_bundle = emit_linked_stable_bundle(temporary.path());
    let gfa_bundle = temporary.path().join("gfa-mutation");
    copy_tree(&linked_bundle, &gfa_bundle);
    let gfa_path = gfa_bundle.join("assembly.gfa");
    let gfa = fs::read_to_string(&gfa_path).unwrap();
    let original_sequence = gfa
        .lines()
        .find(|line| line.starts_with("S\t"))
        .unwrap()
        .split('\t')
        .nth(2)
        .unwrap();
    let mut replacement_sequence = original_sequence.as_bytes().to_vec();
    let last = replacement_sequence.last_mut().unwrap();
    *last = if *last == b'A' { b'C' } else { b'A' };
    let replacement_sequence = String::from_utf8(replacement_sequence).unwrap();
    fs::write(
        &gfa_path,
        gfa.replacen(original_sequence, &replacement_sequence, 1),
    )
    .unwrap();
    assert!(validate_stable_bundle(&gfa_bundle).is_err());
}
