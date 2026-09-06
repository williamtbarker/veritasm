use super::generator::{
    dataset_id_from_parameters, validate_generator_parameters, DatasetManifest, FileRecord,
    GeneratorCase, TruthMoleculeRecord,
};
use super::{
    checked_relative_path, commit_staging, decimal_u64, open_directory_anchor, read_fasta_snapshot,
    read_fasta_snapshot_beneath, read_file_bounded_beneath, reverse_complement, sha256_bytes,
    sha256_file, sha256_file_exact_bounded_beneath, staging_directory, write_checksum_manifest,
    write_json, DirectoryAnchor, FastaRecord, TruthClass, TruthTopology, ValidationError,
    ValidationResult, VALIDATION_COORDINATE_SYSTEM,
};
use flate2::read::MultiGzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const EVALUATOR_VERSION: &str = "4";
const GENERATOR_VERSION: &str = "4";
const VALIDATION_PLAN_VERSION: &str = "veritasm-validation-v1";
const VALIDATION_RNG_NAME: &str = "sha256-counter-v1";
const VALIDATION_RNG_BYTE_ORDER: &str = "counter_u64_le_output_words_u64_le";
const MAX_GENERATED_FRAGMENTS: u64 = 100_000;
const MAX_GENERATED_TRUTH_LENGTH: u64 = 2_000_000;
const MAX_GENERATED_READ_LENGTH: u64 = 10_000;
const MAX_GENERATED_READ_BASES: u64 = 25_000_000;
const ERROR_RATE_SCALE: u64 = 1_000_000;
const MAX_DATASET_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DATASET_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DATASET_FILE_RECORDS: usize = 1_024;
const MAX_TOTAL_DATASET_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ASSEMBLER_INPUT_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_TRUTH_FASTA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ASSEMBLY_FASTA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRUTH_FASTA_RECORDS: usize = 256;
const MAX_ASSEMBLY_FASTA_RECORDS: usize = 10_000;
const MAX_DECODED_READ_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DATASET_CHECKSUM_MANIFEST_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_TRUTH_EVIDENCE_REPLAY_BYTES: u64 = 384 * 1024 * 1024;
const MAX_TOTAL_ALIGNMENT_DP_CELLS: u64 = 250_000_000;
const MAX_RETAINED_ALIGNMENT_ROWS: usize = 10_000;
const MAX_JUNCTION_FLANK_LENGTH: usize = 31;
const DEFAULT_MAX_JUNCTION_INDEX_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MAX_JUNCTION_EVIDENCE_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_MAX_EXACT_ALIGNMENT_SCAN_BASES: u64 = 1_000_000_000;
const MAX_EXACT_ALIGNMENT_PREFIX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RAW_EVALUATION_TABLE_BYTES: u64 = 128 * 1024 * 1024;
const PROJECTED_HEADER_BYTES: u64 = 4 * 1024;
const PROJECTED_ROW_FIXED_BYTES: u64 = 512;
const JUNCTION_INDEX_FIXED_BYTES: u64 = 64 * 1024;
const INPUT_MANIFEST_PATH: &str = "assembler_input/input_manifest.json";
const TRUTH_FASTA_PATH: &str = "evaluation_truth/truth.fasta";
const ORIGINS_PATH: &str = "evaluation_truth/origins.tsv";
const ERRORS_PATH: &str = "evaluation_truth/errors.tsv";
const QUALITY_EVENTS_PATH: &str = "evaluation_truth/quality_events.tsv";
const DATASET_CHECKSUM_MANIFEST_PATH: &str = "manifest.sha256";
const ORIGINS_HEADER: &str = "schema_version\tfragment_ordinal\tread_id\tmate_role\tmolecule_id\ttopology\tstrand\tfirst_truth_base_zero_based\ttruth_step\tread_length\touter_fragment_start_zero_based\touter_fragment_span\twraps_origin\tpre_error_sequence_sha256\tobserved_sequence_sha256\tquality_phred_sequence_sha256";
const ERRORS_HEADER: &str = "schema_version\tread_id\tobserved_offset_zero_based\ttruth_coordinate_zero_based\terror_kind\ttruth_base\tobserved_base";
const QUALITY_EVENTS_HEADER: &str =
    "schema_version\tread_id\tobserved_offset_zero_based\tevent_kind\tquality_phred";

#[derive(Debug, Clone, Copy)]
struct EvaluationWorkLimits {
    max_total_alignment_dp_cells: u64,
    max_retained_alignment_rows: usize,
}

const EVALUATION_WORK_LIMITS: EvaluationWorkLimits = EvaluationWorkLimits {
    max_total_alignment_dp_cells: MAX_TOTAL_ALIGNMENT_DP_CELLS,
    max_retained_alignment_rows: MAX_RETAINED_ALIGNMENT_ROWS,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssemblerInputManifest {
    schema_version: String,
    dataset_id: String,
    mode: String,
    fragments_decimal: String,
    reads_decimal: String,
    read_files: Vec<AssemblerInputFile>,
    truth_exclusion: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssemblerInputFile {
    role: String,
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, Copy)]
enum TruthLayout {
    Single(TruthTopology),
    LinearPrimaryMinor,
}

#[derive(Debug, Clone, Copy)]
struct CaseContract {
    mode: &'static str,
    read_count_per_fragment: u64,
    truth_layout: TruthLayout,
}

/// Configuration for deterministic truth-aware evaluation.
#[derive(Debug, Clone)]
pub struct EvaluationConfig {
    pub dataset_manifest: PathBuf,
    pub assembly_fasta: PathBuf,
    pub output_directory: PathBuf,
    pub junction_flank_length: usize,
    pub max_edit_rate_ppm: u32,
    pub max_dp_cells: u64,
    pub max_exact_alignment_scan_bases: u64,
    pub max_junction_comparisons: u64,
    pub junction_evidence_mode: JunctionEvidenceMode,
    pub max_junction_index_bytes: u64,
    pub max_junction_evidence_bytes: u64,
    /// Expected SHA-256 of the exact canonical dataset `manifest.sha256` bytes.
    ///
    /// When absent, the result is explicitly development-unbound and is not qualification
    /// eligible. The expected value must come from outside the dataset directory.
    pub expected_dataset_content_root_sha256: Option<String>,
    /// Maximum admitted heap projection for read/origin/event evidence replay.
    pub max_truth_evidence_replay_bytes: u64,
}

impl EvaluationConfig {
    /// Constructs a conservative default evaluator configuration.
    pub fn new(
        dataset_manifest: PathBuf,
        assembly_fasta: PathBuf,
        output_directory: PathBuf,
    ) -> Self {
        Self {
            dataset_manifest,
            assembly_fasta,
            output_directory,
            junction_flank_length: 15,
            max_edit_rate_ppm: 150_000,
            max_dp_cells: 50_000_000,
            max_exact_alignment_scan_bases: DEFAULT_MAX_EXACT_ALIGNMENT_SCAN_BASES,
            max_junction_comparisons: 500_000_000,
            junction_evidence_mode: JunctionEvidenceMode::NonCorrect,
            max_junction_index_bytes: DEFAULT_MAX_JUNCTION_INDEX_BYTES,
            max_junction_evidence_bytes: DEFAULT_MAX_JUNCTION_EVIDENCE_BYTES,
            expected_dataset_content_root_sha256: None,
            max_truth_evidence_replay_bytes: DEFAULT_MAX_TRUTH_EVIDENCE_REPLAY_BYTES,
        }
    }
}

/// Selects which per-adjacency junction rows are materialized.
///
/// Every mode evaluates every eligible adjacency and produces identical aggregate metrics and a
/// digest of the complete logical row stream. Selection changes only the rows written to
/// `junctions.tsv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JunctionEvidenceMode {
    /// Emit every eligible adjacency.
    All,
    /// Emit false and indeterminate adjacencies, aggregating correct adjacencies.
    NonCorrect,
    /// Emit no per-adjacency rows and retain exact aggregate metrics only.
    Summary,
}

impl JunctionEvidenceMode {
    /// Stable machine-readable mode name used in evaluation identities.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::NonCorrect => "non_correct",
            Self::Summary => "summary",
        }
    }

    const fn emits(self, class: &JunctionClass) -> bool {
        match self {
            Self::All => true,
            Self::NonCorrect => !matches!(class, JunctionClass::Correct),
            Self::Summary => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatioMetric {
    pub numerator_decimal: String,
    pub denominator_decimal: String,
    pub value_decimal: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QvMetric {
    pub value_decimal: Option<String>,
    pub status: String,
    pub evaluated_columns_decimal: String,
    pub observed_errors_decimal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseMetrics {
    pub alignment_metric_scope: String,
    pub matches_decimal: String,
    pub mismatches_decimal: String,
    pub inserted_bases_decimal: String,
    pub deleted_bases_decimal: String,
    pub evaluated_alignment_columns_decimal: String,
    pub unaligned_assembly_bases_decimal: String,
    pub error_rate: RatioMetric,
    pub consensus_accuracy: RatioMetric,
    pub qv: QvMetric,
    pub recovery: RecoveryMetrics,
    pub aligned_duplication_ratio: RatioMetric,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryMetrics {
    pub compatible_placement_status: String,
    pub exact_unambiguous_alignment_records_decimal: String,
    pub exact_ambiguous_alignment_records_decimal: String,
    pub non_exact_alignment_records_decimal: String,
    pub truth_bases_decimal: String,
    pub unique_coordinate_covered_truth_bases_decimal: String,
    pub compatible_covered_truth_bases_lower_bound_decimal: String,
    pub compatible_covered_truth_bases_upper_bound_decimal: String,
    pub unique_coordinate_truth_genome_fraction: RatioMetric,
    pub compatible_truth_genome_fraction_lower_bound: RatioMetric,
    pub compatible_truth_genome_fraction_upper_bound: RatioMetric,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JunctionMetrics {
    pub flank_length_decimal: String,
    pub assembly_adjacencies_decimal: String,
    pub eligible_adjacencies_decimal: String,
    pub correct_decimal: String,
    pub false_decimal: String,
    pub indeterminate_ambiguous_flank_decimal: String,
    pub indeterminate_unmapped_flank_decimal: String,
    pub not_evaluated_short_flank_decimal: String,
    pub primary_minor_chimera_decimal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoleculeMetric {
    pub molecule_id: String,
    pub truth_class: TruthClass,
    pub topology: TruthTopology,
    pub truth_bases_decimal: String,
    pub compatible_placement_status: String,
    pub unique_coordinate_covered_bases_decimal: String,
    pub compatible_covered_bases_lower_bound_decimal: String,
    pub compatible_covered_bases_upper_bound_decimal: String,
    pub unique_coordinate_truth_genome_fraction: RatioMetric,
    pub compatible_truth_genome_fraction_lower_bound: RatioMetric,
    pub compatible_truth_genome_fraction_upper_bound: RatioMetric,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatorRecord {
    pub name: String,
    pub version: String,
    pub alignment_algorithm: String,
    pub junction_algorithm: String,
    pub max_edit_rate_ppm_decimal: String,
    pub max_dp_cells_decimal: String,
    pub max_exact_alignment_scan_bases_decimal: String,
    pub exact_alignment_scan_bases_decimal: String,
    pub max_junction_comparisons_decimal: String,
    pub junction_key_comparisons_decimal: String,
    pub max_junction_flank_length_decimal: String,
    pub max_junction_index_bytes_decimal: String,
    pub junction_index_execution_status: String,
    pub junction_index_projected_bytes_decimal: String,
    pub junction_index_capacity_bytes_decimal: String,
    pub max_junction_evidence_bytes_decimal: String,
    pub max_exact_alignment_prefix_bytes_decimal: String,
    pub max_dataset_manifest_bytes_decimal: String,
    pub max_dataset_artifact_bytes_decimal: String,
    pub max_truth_fasta_bytes_decimal: String,
    pub max_assembly_fasta_bytes_decimal: String,
    pub max_truth_evidence_replay_bytes_decimal: String,
    pub projected_truth_evidence_replay_bytes_decimal: String,
    pub tie_breaking: String,
}

/// External binding state for the dataset content root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetBindingMode {
    /// Internal consistency only; ineligible for an admitted qualification scorecard.
    DevelopmentUnbound,
    /// Exact canonical content root matched an externally supplied digest.
    ExternallyBound,
}

impl DatasetBindingMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DevelopmentUnbound => "development_unbound",
            Self::ExternallyBound => "externally_bound",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetBindingRecord {
    pub mode: DatasetBindingMode,
    pub qualification_admission: String,
    pub expected_dataset_content_root_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationInputRecord {
    pub dataset_id: String,
    pub dataset_manifest_sha256: String,
    pub dataset_content_root_sha256: String,
    pub dataset_binding: DatasetBindingRecord,
    pub truth_fasta_sha256: String,
    pub assembly_fasta_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationStatus {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblySummary {
    pub records_decimal: String,
    pub bases_decimal: String,
    pub accepted_alignment_records_decimal: String,
    pub unaligned_records_decimal: String,
    pub empty_output: bool,
}

/// Describes the complete logical junction evaluation and its selected materialized rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JunctionEvidenceRecord {
    pub mode: JunctionEvidenceMode,
    pub logical_rows_decimal: String,
    pub emitted_rows_decimal: String,
    pub omitted_rows_decimal: String,
    pub omitted_correct_rows_decimal: String,
    pub logical_rows_sha256: String,
    pub artifact_bytes_decimal: String,
}

/// Machine-readable summary in an evaluator result bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationResult {
    pub schema_version: String,
    pub evaluation_id: String,
    pub evaluator: EvaluatorRecord,
    pub inputs: EvaluationInputRecord,
    pub status: EvaluationStatus,
    pub assembly: AssemblySummary,
    pub base_metrics: BaseMetrics,
    pub junction_metrics: JunctionMetrics,
    pub junction_evidence: JunctionEvidenceRecord,
    pub per_molecule: Vec<MoleculeMetric>,
    pub raw_alignments_sha256: String,
    pub raw_junctions_sha256: String,
    pub limitations: Vec<String>,
}

/// Validate cross-field invariants that JSON Schema cannot express.
///
/// The evaluator calls this before committing a result. Consumers can use it
/// after separately checking the shipped structural JSON Schema; this is not
/// a replacement for schema, digest, or manifest validation.
pub fn validate_evaluation_result(result: &EvaluationResult) -> ValidationResult<()> {
    let field = |value: &str, name: &str| decimal_u64(value, name);
    let evaluator = &result.evaluator;
    if result.schema_version != "veritasm-validation-result-v4"
        || evaluator.name != "veritasm-evaluate"
        || evaluator.version != EVALUATOR_VERSION
        || evaluator.alignment_algorithm
            != "exact_compatible_coordinate_bounds_with_primary_alignment_diagnostic_v3"
        || evaluator.junction_algorithm != "exact_packed_window_occurrence_index_v2"
        || evaluator.tie_breaking
            != "edit_distance,descending_matches,indels,molecule_manifest_order,strand_plus_before_minus,oriented_start"
    {
        return Err(ValidationError::Integrity(
            "evaluator identity or algorithm descriptor differs from version 4".to_owned(),
        ));
    }
    let max_edit_rate_ppm = field(
        &evaluator.max_edit_rate_ppm_decimal,
        "maximum edit rate ppm",
    )?;
    let max_dp_cells = field(&evaluator.max_dp_cells_decimal, "maximum DP cells")?;
    let max_scan = field(
        &evaluator.max_exact_alignment_scan_bases_decimal,
        "max exact-alignment scan bases",
    )?;
    let observed_scan = field(
        &evaluator.exact_alignment_scan_bases_decimal,
        "exact-alignment scan bases",
    )?;
    let max_comparisons = field(
        &evaluator.max_junction_comparisons_decimal,
        "max junction comparisons",
    )?;
    let observed_comparisons = field(
        &evaluator.junction_key_comparisons_decimal,
        "junction key comparisons",
    )?;
    let max_flank = field(
        &evaluator.max_junction_flank_length_decimal,
        "maximum junction flank length",
    )?;
    let max_index = field(
        &evaluator.max_junction_index_bytes_decimal,
        "max junction index bytes",
    )?;
    let projected_index = field(
        &evaluator.junction_index_projected_bytes_decimal,
        "projected junction index bytes",
    )?;
    let capacity_index = field(
        &evaluator.junction_index_capacity_bytes_decimal,
        "junction index capacity bytes",
    )?;
    let max_evidence = field(
        &evaluator.max_junction_evidence_bytes_decimal,
        "max junction evidence bytes",
    )?;
    let artifact_bytes = field(
        &result.junction_evidence.artifact_bytes_decimal,
        "junction evidence artifact bytes",
    )?;
    let max_prefix = field(
        &evaluator.max_exact_alignment_prefix_bytes_decimal,
        "maximum exact-alignment prefix bytes",
    )?;
    let max_dataset_manifest = field(
        &evaluator.max_dataset_manifest_bytes_decimal,
        "maximum dataset manifest bytes",
    )?;
    let max_dataset_artifact = field(
        &evaluator.max_dataset_artifact_bytes_decimal,
        "maximum dataset artifact bytes",
    )?;
    let max_truth_fasta = field(
        &evaluator.max_truth_fasta_bytes_decimal,
        "maximum truth FASTA bytes",
    )?;
    let max_assembly_fasta = field(
        &evaluator.max_assembly_fasta_bytes_decimal,
        "maximum assembly FASTA bytes",
    )?;
    let max_truth_evidence_replay = field(
        &evaluator.max_truth_evidence_replay_bytes_decimal,
        "maximum truth-evidence replay bytes",
    )?;
    let projected_truth_evidence_replay = field(
        &evaluator.projected_truth_evidence_replay_bytes_decimal,
        "projected truth-evidence replay bytes",
    )?;
    if max_edit_rate_ppm > ERROR_RATE_SCALE
        || max_dp_cells == 0
        || max_dp_cells > MAX_TOTAL_ALIGNMENT_DP_CELLS
        || max_scan == 0
        || max_comparisons == 0
        || max_index == 0
        || max_evidence == 0
        || max_flank != MAX_JUNCTION_FLANK_LENGTH as u64
        || max_prefix != MAX_EXACT_ALIGNMENT_PREFIX_BYTES
        || max_dataset_manifest != MAX_DATASET_MANIFEST_BYTES
        || max_dataset_artifact != MAX_DATASET_ARTIFACT_BYTES
        || max_truth_fasta != MAX_TRUTH_FASTA_BYTES
        || max_assembly_fasta != MAX_ASSEMBLY_FASTA_BYTES
        || max_truth_evidence_replay == 0
        || projected_truth_evidence_replay > max_truth_evidence_replay
        || observed_scan > max_scan
        || observed_comparisons > max_comparisons
        || projected_index > max_index
        || capacity_index > projected_index
        || capacity_index > max_index
        || artifact_bytes > max_evidence
    {
        return Err(ValidationError::Integrity(
            "evaluator configuration or resource provenance violates the version-4 contract"
                .to_owned(),
        ));
    }
    let records = field(&result.assembly.records_decimal, "assembly records")?;
    let assembly_bases = field(&result.assembly.bases_decimal, "assembly bases")?;
    let accepted = field(
        &result.assembly.accepted_alignment_records_decimal,
        "accepted alignment records",
    )?;
    let unaligned = field(
        &result.assembly.unaligned_records_decimal,
        "unaligned assembly records",
    )?;
    let expected_empty = records == 0;
    let expected_status_code = if expected_empty {
        "evaluation_complete_empty_assembly"
    } else {
        "evaluation_complete"
    };
    let expected_status_message = if expected_empty {
        "Evaluation completed: the assembly contained no records; this is not biological absence."
    } else {
        "Evaluation completed under the recorded truth and metric rules."
    };
    if records > MAX_ASSEMBLY_FASTA_RECORDS as u64
        || assembly_bases > MAX_ASSEMBLY_FASTA_BYTES
        || accepted.checked_add(unaligned) != Some(records)
        || result.assembly.empty_output != expected_empty
        || (assembly_bases == 0) != expected_empty
        || records > assembly_bases
        || result.status.code != expected_status_code
        || result.status.message != expected_status_message
    {
        return Err(ValidationError::Integrity(
            "assembly result counts or empty-output state do not reconcile".to_owned(),
        ));
    }
    if expected_empty && observed_scan != 0 {
        return Err(ValidationError::Integrity(
            "an empty assembly cannot consume exact-alignment scan work".to_owned(),
        ));
    }

    let base = &result.base_metrics;
    if base.alignment_metric_scope != "deterministic_primary_alignment_diagnostic_only" {
        return Err(ValidationError::Integrity(
            "base-alignment metric scope differs from evaluator version 3".to_owned(),
        ));
    }
    let matches = field(&base.matches_decimal, "matching alignment bases")?;
    let mismatches = field(&base.mismatches_decimal, "mismatching alignment bases")?;
    let insertions = field(&base.inserted_bases_decimal, "inserted alignment bases")?;
    let deletions = field(&base.deleted_bases_decimal, "deleted alignment bases")?;
    let columns = field(
        &base.evaluated_alignment_columns_decimal,
        "evaluated alignment columns",
    )?;
    let unaligned_bases = field(
        &base.unaligned_assembly_bases_decimal,
        "unaligned assembly bases",
    )?;
    let errors = mismatches
        .checked_add(insertions)
        .and_then(|value| value.checked_add(deletions))
        .ok_or_else(|| ValidationError::Integrity("base-error count overflow".to_owned()))?;
    let expected_columns = matches
        .checked_add(mismatches)
        .and_then(|value| value.checked_add(insertions))
        .and_then(|value| value.checked_add(deletions))
        .ok_or_else(|| ValidationError::Integrity("alignment-column count overflow".to_owned()))?;
    let aligned_query_bases = matches
        .checked_add(mismatches)
        .and_then(|value| value.checked_add(insertions))
        .ok_or_else(|| {
            ValidationError::Integrity("aligned query-base count overflow".to_owned())
        })?;
    let maximum_retained_alignment_columns =
        (MAX_RAW_EVALUATION_TABLE_BYTES - PROJECTED_HEADER_BYTES) / 2;
    if columns > maximum_retained_alignment_columns
        || expected_columns != columns
        || aligned_query_bases.checked_add(unaligned_bases) != Some(assembly_bases)
        || accepted > aligned_query_bases
        || (accepted == 0) != (aligned_query_bases == 0)
        || (accepted == 0) != (columns == 0)
        || unaligned > unaligned_bases
        || (unaligned == 0) != (unaligned_bases == 0)
        || field(&base.error_rate.numerator_decimal, "error-rate numerator")? != errors
        || field(
            &base.error_rate.denominator_decimal,
            "error-rate denominator",
        )? != columns
        || field(
            &base.consensus_accuracy.numerator_decimal,
            "consensus-accuracy numerator",
        )? != matches
        || field(
            &base.consensus_accuracy.denominator_decimal,
            "consensus-accuracy denominator",
        )? != columns
        || field(&base.qv.evaluated_columns_decimal, "QV evaluated columns")? != columns
        || field(&base.qv.observed_errors_decimal, "QV observed errors")? != errors
    {
        return Err(ValidationError::Integrity(
            "base metrics, ratios, QV, and assembly bases do not reconcile".to_owned(),
        ));
    }
    validate_ratio_metric(
        &base.error_rate,
        errors,
        columns,
        if columns == 0 {
            "not_available_no_aligned_bases"
        } else {
            "measured"
        },
        "error rate",
    )?;
    validate_ratio_metric(
        &base.consensus_accuracy,
        matches,
        columns,
        if columns == 0 {
            "not_available_no_aligned_bases"
        } else {
            "measured"
        },
        "consensus accuracy",
    )?;
    if base.qv != qv(errors, columns) {
        return Err(ValidationError::Integrity(
            "QV value, status, or counts do not reconcile".to_owned(),
        ));
    }

    let junctions = &result.junction_metrics;
    let flank = field(&junctions.flank_length_decimal, "junction flank length")?;
    let assembly_adjacencies = field(
        &junctions.assembly_adjacencies_decimal,
        "assembly adjacencies",
    )?;
    let eligible = field(
        &junctions.eligible_adjacencies_decimal,
        "eligible adjacencies",
    )?;
    let correct = field(&junctions.correct_decimal, "correct junctions")?;
    let false_junctions = field(&junctions.false_decimal, "false junctions")?;
    let ambiguous = field(
        &junctions.indeterminate_ambiguous_flank_decimal,
        "ambiguous junctions",
    )?;
    let unmapped = field(
        &junctions.indeterminate_unmapped_flank_decimal,
        "unmapped junctions",
    )?;
    let short = field(
        &junctions.not_evaluated_short_flank_decimal,
        "short-flank adjacencies",
    )?;
    let chimeras = field(
        &junctions.primary_minor_chimera_decimal,
        "primary/minor chimeras",
    )?;
    let classified = correct
        .checked_add(false_junctions)
        .and_then(|value| value.checked_add(ambiguous))
        .and_then(|value| value.checked_add(unmapped));
    if flank == 0
        || flank > max_flank
        || classified != Some(eligible)
        || eligible.checked_add(short) != Some(assembly_adjacencies)
        || assembly_bases.checked_sub(records) != Some(assembly_adjacencies)
        || chimeras > false_junctions
    {
        return Err(ValidationError::Integrity(
            "junction metric counts do not reconcile".to_owned(),
        ));
    }
    match evaluator.junction_index_execution_status.as_str() {
        "built"
            if eligible != 0
                && projected_index != 0
                && capacity_index != 0
                && observed_comparisons != 0 => {}
        "built_empty_truth_window_universe"
            if eligible != 0
                && projected_index == JUNCTION_INDEX_FIXED_BYTES
                && capacity_index == JUNCTION_INDEX_FIXED_BYTES
                && observed_comparisons == 0 => {}
        "not_required_no_eligible_adjacencies"
            if eligible == 0
                && projected_index == 0
                && capacity_index == 0
                && observed_comparisons == 0 => {}
        _ => {
            return Err(ValidationError::Integrity(
                "junction index status, eligibility, byte, and work accounting do not reconcile"
                    .to_owned(),
            ));
        }
    }

    let evidence = &result.junction_evidence;
    let logical = field(&evidence.logical_rows_decimal, "logical junction rows")?;
    let emitted = field(&evidence.emitted_rows_decimal, "emitted junction rows")?;
    let omitted = field(&evidence.omitted_rows_decimal, "omitted junction rows")?;
    let omitted_correct = field(
        &evidence.omitted_correct_rows_decimal,
        "omitted correct junction rows",
    )?;
    let mode_valid = match evidence.mode {
        JunctionEvidenceMode::All => emitted == logical && omitted == 0 && omitted_correct == 0,
        JunctionEvidenceMode::NonCorrect => {
            omitted == correct
                && omitted_correct == correct
                && emitted.checked_add(correct) == Some(logical)
        }
        JunctionEvidenceMode::Summary => {
            emitted == 0 && omitted == logical && omitted_correct == correct
        }
    };
    if logical != eligible
        || emitted.checked_add(omitted) != Some(logical)
        || omitted_correct > omitted
        || !mode_valid
    {
        return Err(ValidationError::Integrity(
            "junction evidence counts or selection mode do not reconcile".to_owned(),
        ));
    }
    if result.per_molecule.len() > MAX_TRUTH_FASTA_RECORDS {
        return Err(ValidationError::Integrity(
            "per-molecule metrics exceed the evaluator truth-record cap".to_owned(),
        ));
    }
    let recovery = &base.recovery;
    let exact_unambiguous = field(
        &recovery.exact_unambiguous_alignment_records_decimal,
        "exact unambiguous recovery alignment records",
    )?;
    let exact_ambiguous = field(
        &recovery.exact_ambiguous_alignment_records_decimal,
        "exact ambiguous recovery alignment records",
    )?;
    let non_exact = field(
        &recovery.non_exact_alignment_records_decimal,
        "non-exact recovery alignment records",
    )?;
    let recovery_accounted = exact_unambiguous
        .checked_add(exact_ambiguous)
        .and_then(|value| value.checked_add(non_exact));
    let unique_covered = field(
        &recovery.unique_coordinate_covered_truth_bases_decimal,
        "unique-coordinate covered truth bases",
    )?;
    let lower_covered = field(
        &recovery.compatible_covered_truth_bases_lower_bound_decimal,
        "compatible lower-bound covered truth bases",
    )?;
    let upper_covered = field(
        &recovery.compatible_covered_truth_bases_upper_bound_decimal,
        "compatible upper-bound covered truth bases",
    )?;
    let expected_recovery_status = if non_exact == 0 {
        "complete_exact_compatible_placement_universe"
    } else {
        "not_available_non_exact_compatible_placement_universe"
    };
    if recovery_accounted != Some(accepted)
        || unique_covered > lower_covered
        || lower_covered > upper_covered
        || recovery.compatible_placement_status != expected_recovery_status
    {
        return Err(ValidationError::Integrity(
            "recovery placement classes, bounds, or availability do not reconcile".to_owned(),
        ));
    }
    let mut total_truth_bases = 0u64;
    let mut total_unique_covered = 0u64;
    let mut total_lower_covered = 0u64;
    let mut total_upper_covered = 0u64;
    let mut prior_molecule_id: Option<&str> = None;
    for molecule in &result.per_molecule {
        let truth = field(&molecule.truth_bases_decimal, "molecule truth bases")?;
        let unique = field(
            &molecule.unique_coordinate_covered_bases_decimal,
            "molecule unique-coordinate covered bases",
        )?;
        let lower = field(
            &molecule.compatible_covered_bases_lower_bound_decimal,
            "molecule compatible lower-bound covered bases",
        )?;
        let upper = field(
            &molecule.compatible_covered_bases_upper_bound_decimal,
            "molecule compatible upper-bound covered bases",
        )?;
        if truth == 0
            || unique > lower
            || lower > upper
            || upper > truth
            || molecule.compatible_placement_status != expected_recovery_status
            || prior_molecule_id
                .is_some_and(|prior| prior.as_bytes() >= molecule.molecule_id.as_bytes())
        {
            return Err(ValidationError::Integrity(
                "molecule recovery bounds, availability, or ordering do not reconcile".to_owned(),
            ));
        }
        for (actual, numerator, label) in [
            (
                &molecule.unique_coordinate_truth_genome_fraction,
                unique,
                "molecule unique-coordinate truth genome fraction",
            ),
            (
                &molecule.compatible_truth_genome_fraction_lower_bound,
                lower,
                "molecule compatible truth genome fraction lower bound",
            ),
            (
                &molecule.compatible_truth_genome_fraction_upper_bound,
                upper,
                "molecule compatible truth genome fraction upper bound",
            ),
        ] {
            if actual != &availability_ratio(numerator, truth, expected_recovery_status)? {
                return Err(ValidationError::Integrity(format!(
                    "{label} value, status, or counts do not reconcile"
                )));
            }
        }
        total_truth_bases = total_truth_bases.checked_add(truth).ok_or_else(|| {
            ValidationError::Integrity("total truth-base count overflow".to_owned())
        })?;
        total_unique_covered = total_unique_covered.checked_add(unique).ok_or_else(|| {
            ValidationError::Integrity("total unique-coordinate count overflow".to_owned())
        })?;
        total_lower_covered = total_lower_covered.checked_add(lower).ok_or_else(|| {
            ValidationError::Integrity("total compatible lower-bound count overflow".to_owned())
        })?;
        total_upper_covered = total_upper_covered.checked_add(upper).ok_or_else(|| {
            ValidationError::Integrity("total compatible upper-bound count overflow".to_owned())
        })?;
        prior_molecule_id = Some(&molecule.molecule_id);
    }
    if total_truth_bases == 0
        || total_truth_bases > MAX_TRUTH_FASTA_BYTES
        || field(
            &recovery.truth_bases_decimal,
            "recovery truth-base denominator",
        )? != total_truth_bases
        || unique_covered != total_unique_covered
        || lower_covered != total_lower_covered
        || upper_covered != total_upper_covered
        || field(
            &base.aligned_duplication_ratio.numerator_decimal,
            "duplication-ratio numerator",
        )? != matches
        || field(
            &base.aligned_duplication_ratio.denominator_decimal,
            "duplication-ratio denominator",
        )? != upper_covered
    {
        return Err(ValidationError::Integrity(
            "global recovery or duplication counts do not reconcile".to_owned(),
        ));
    }
    for (actual, numerator, label) in [
        (
            &recovery.unique_coordinate_truth_genome_fraction,
            unique_covered,
            "global unique-coordinate truth genome fraction",
        ),
        (
            &recovery.compatible_truth_genome_fraction_lower_bound,
            lower_covered,
            "global compatible truth genome fraction lower bound",
        ),
        (
            &recovery.compatible_truth_genome_fraction_upper_bound,
            upper_covered,
            "global compatible truth genome fraction upper bound",
        ),
    ] {
        if actual != &availability_ratio(numerator, total_truth_bases, expected_recovery_status)? {
            return Err(ValidationError::Integrity(format!(
                "{label} value, status, or counts do not reconcile"
            )));
        }
    }
    if base.aligned_duplication_ratio
        != duplication_ratio(matches, upper_covered, exact_ambiguous, non_exact)?
    {
        return Err(ValidationError::Integrity(
            "aligned duplication availability or value does not reconcile".to_owned(),
        ));
    }
    let binding = &result.inputs.dataset_binding;
    if !is_lower_sha256(&result.inputs.dataset_manifest_sha256)
        || !is_lower_sha256(&result.inputs.dataset_content_root_sha256)
        || !is_lower_sha256(&result.inputs.truth_fasta_sha256)
        || !is_lower_sha256(&result.inputs.assembly_fasta_sha256)
    {
        return Err(ValidationError::Integrity(
            "evaluation input digests are not canonical lowercase SHA-256".to_owned(),
        ));
    }
    match binding.mode {
        DatasetBindingMode::DevelopmentUnbound
            if binding.expected_dataset_content_root_sha256.is_none()
                && binding.qualification_admission == "ineligible_development_unbound" => {}
        DatasetBindingMode::ExternallyBound
            if binding
                .expected_dataset_content_root_sha256
                .as_deref()
                == Some(result.inputs.dataset_content_root_sha256.as_str())
                && binding.qualification_admission == "eligible_externally_bound" => {}
        _ => {
            return Err(ValidationError::Integrity(
                "dataset binding mode, expected content root, and qualification admission do not reconcile"
                    .to_owned(),
            ))
        }
    }
    let expected_evaluation_id = evaluation_id_from_values(
        &result.inputs.dataset_id,
        &result.inputs.dataset_manifest_sha256,
        &result.inputs.dataset_content_root_sha256,
        binding.mode,
        binding.expected_dataset_content_root_sha256.as_deref(),
        &result.inputs.truth_fasta_sha256,
        &result.inputs.assembly_fasta_sha256,
        flank,
        max_edit_rate_ppm,
        max_dp_cells,
        max_scan,
        max_comparisons,
        result.junction_evidence.mode,
        max_index,
        max_evidence,
        max_truth_evidence_replay,
    );
    if result.evaluation_id != expected_evaluation_id {
        return Err(ValidationError::Integrity(
            "evaluation ID does not match the recorded inputs and evaluator configuration"
                .to_owned(),
        ));
    }
    Ok(())
}

struct TruthMolecule {
    id: String,
    class: TruthClass,
    topology: TruthTopology,
    sequence: Vec<u8>,
}

struct EvidenceRead {
    rank: usize,
    role: &'static str,
    sequence: Vec<u8>,
    quality_phred: Vec<u8>,
}

struct LedgerOrigin {
    fragment_ordinal: u64,
    molecule_index: usize,
    first_truth_base: usize,
    truth_step: i8,
    outer_fragment_start: usize,
    outer_fragment_span: usize,
}

struct DatasetContentBinding {
    content_root_sha256: String,
    mode: DatasetBindingMode,
    expected_content_root_sha256: Option<String>,
}

struct TruthEvidenceAudit {
    projected_replay_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strand {
    Forward,
    Reverse,
}

impl Strand {
    fn symbol(self) -> char {
        match self {
            Self::Forward => '+',
            Self::Reverse => '-',
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Forward => 0,
            Self::Reverse => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignmentOp {
    Match,
    Mismatch,
    Insertion,
    Deletion,
}

impl AlignmentOp {
    fn cigar(self) -> char {
        match self {
            Self::Match => '=',
            Self::Mismatch => 'X',
            Self::Insertion => 'I',
            Self::Deletion => 'D',
        }
    }
}

struct AlignmentCandidate {
    molecule_index: usize,
    strand: Strand,
    oriented_reference_start: usize,
    operations: Vec<AlignmentOp>,
    edits: u64,
    matches: u64,
    mismatches: u64,
    insertions: u64,
    deletions: u64,
}

impl AlignmentCandidate {
    fn columns(&self) -> u64 {
        self.matches + self.mismatches + self.insertions + self.deletions
    }

    fn reference_bases(&self) -> u64 {
        self.matches + self.mismatches + self.deletions
    }

    fn accepted(&self, max_edit_rate_ppm: u32) -> bool {
        u128::from(self.edits) * u128::from(ERROR_RATE_SCALE)
            <= u128::from(self.columns()) * u128::from(max_edit_rate_ppm)
    }
}

struct AlignmentRow {
    contig_id: String,
    contig_length: usize,
    status: &'static str,
    candidate: Option<AlignmentCandidate>,
    equally_best_targets: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ExactPlacement {
    molecule_index: usize,
    strand_rank: u8,
    start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JunctionClass {
    Correct,
    False,
    IndeterminateAmbiguous,
    IndeterminateUnmapped,
}

impl JunctionClass {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Correct => "correct_truth_adjacency",
            Self::False => "false_junction",
            Self::IndeterminateAmbiguous => "indeterminate_ambiguous_flank",
            Self::IndeterminateUnmapped => "indeterminate_unmapped_flank",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlacementSummary {
    count: u64,
    unique: Option<ExactPlacement>,
}

struct JunctionRow {
    boundary: usize,
    class: JunctionClass,
    compatible: PlacementSummary,
    left: PlacementSummary,
    right: PlacementSummary,
    primary_minor_chimera: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PackedOccurrence {
    key: u128,
    molecule_index: u16,
    strand_rank: u8,
    start: u32,
}

struct PackedWindowIndex {
    occurrences: Vec<PackedOccurrence>,
}

#[derive(Default)]
struct JunctionWork {
    key_comparisons: u64,
}

struct JunctionEvaluation {
    counts: JunctionCounts,
    evidence: JunctionEvidenceRecord,
    key_comparisons: u64,
    index_execution_status: &'static str,
    index_projected_bytes: u64,
    index_capacity_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct ExactAlignmentPlan {
    molecule_index: usize,
    strand: Strand,
    oriented_reference_start: usize,
    equally_best_targets: usize,
}

struct AlignmentPreflight {
    plans: Vec<Option<ExactAlignmentPlan>>,
    exact_scan_bases: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MoleculeCoverageMasks {
    unique: Vec<bool>,
    lower: Vec<bool>,
    upper: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecoveryCoverage {
    molecules: Vec<MoleculeCoverageMasks>,
    exact_unambiguous_records: u64,
    exact_ambiguous_records: u64,
    non_exact_records: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CoordinateInterval {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlacementCoordinateSet {
    intervals: [CoordinateInterval; 2],
    len: u8,
}

#[derive(Debug, Default)]
struct MonotoneIntervalUnion {
    current: Option<CoordinateInterval>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CompatibleRecoveryWork {
    placement_visits: u64,
    coordinate_mask_operations: u64,
}

#[derive(Debug)]
enum LowerCoordinateCoverage {
    Linear(CoordinateInterval),
    Circular {
        compatible: Vec<bool>,
        complement_union: MonotoneIntervalUnion,
    },
}

#[derive(Debug)]
struct LowerCoordinateCandidate {
    molecule_index: usize,
    coverage: LowerCoordinateCoverage,
}

#[derive(Clone, Copy)]
struct CompatiblePlacement<'a> {
    molecule_index: usize,
    coordinate_set: PlacementCoordinateSet,
    oriented_start: usize,
    query_length: usize,
    truth: &'a TruthMolecule,
    strand: Strand,
}

#[derive(Debug)]
enum LowerCoordinateState {
    Unseen,
    Candidate(LowerCoordinateCandidate),
    Impossible,
}

/// Evaluates an assembly against generator truth and commits a new result bundle.
pub fn evaluate_dataset(config: &EvaluationConfig) -> ValidationResult<EvaluationResult> {
    validate_config(config)?;
    let dataset_root_path = config.dataset_manifest.parent().ok_or_else(|| {
        ValidationError::Configuration("dataset manifest has no parent directory".to_owned())
    })?;
    let dataset_root_path = if dataset_root_path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dataset_root_path
    };
    let dataset_manifest_name = config.dataset_manifest.file_name().ok_or_else(|| {
        ValidationError::Configuration("dataset manifest has no filename".to_owned())
    })?;
    if dataset_manifest_name != "dataset.json" {
        return Err(ValidationError::Configuration(
            "dataset manifest filename must be exactly dataset.json".to_owned(),
        ));
    }
    let dataset_root = open_directory_anchor(dataset_root_path, "dataset root")?;
    let dataset_bytes = read_file_bounded_beneath(
        &dataset_root,
        Path::new(dataset_manifest_name),
        MAX_DATASET_MANIFEST_BYTES,
        "dataset manifest",
    )?;
    let dataset: DatasetManifest = serde_json::from_slice(&dataset_bytes)?;
    let content_binding = verify_dataset_content_binding(
        &dataset,
        &dataset_bytes,
        &dataset_root,
        config.expected_dataset_content_root_sha256.as_deref(),
    )?;
    validate_dataset_manifest(&dataset, &dataset_root)?;
    let (truths, truth_sha256) = load_truth(&dataset, &dataset_root)?;
    let truth_evidence = validate_truth_evidence(config, &dataset, &dataset_root, &truths)?;
    let assembly_snapshot = read_fasta_snapshot(
        &config.assembly_fasta,
        true,
        MAX_ASSEMBLY_FASTA_BYTES,
        MAX_ASSEMBLY_FASTA_RECORDS,
        "assembly FASTA",
    )?;
    let assembly_records = assembly_snapshot.records;
    let assembly_sha256 = assembly_snapshot.sha256;
    let alignment_preflight = preflight_evaluation_work(
        &assembly_records,
        &truths,
        config.max_dp_cells,
        config.max_exact_alignment_scan_bases,
        EVALUATION_WORK_LIMITS,
    )?;
    let dataset_sha256 = sha256_bytes(&dataset_bytes);

    let mut alignment_rows = Vec::new();
    alignment_rows
        .try_reserve_exact(assembly_records.len())
        .map_err(|error| {
            ValidationError::Resource(format!("cannot allocate alignment rows: {error}"))
        })?;
    for (contig, exact_plan) in assembly_records.iter().zip(&alignment_preflight.plans) {
        alignment_rows.push(align_contig(
            contig,
            &truths,
            config.max_edit_rate_ppm,
            config.max_dp_cells,
            *exact_plan,
        )?);
    }
    let mut exact_alignment_scan_bases = alignment_preflight.exact_scan_bases;
    let recovery = evaluate_compatible_recovery(
        &assembly_records,
        &alignment_rows,
        &truths,
        &mut exact_alignment_scan_bases,
        config.max_exact_alignment_scan_bases,
    )?;
    enforce_alignment_table_limit(&alignment_rows, &truths)?;

    let staging = staging_directory(&config.output_directory, ".veritasm-evaluate-")?;
    let root = staging.path();
    write_alignment_rows(&root.join("alignments.tsv"), &alignment_rows, &truths)?;
    let junction_evaluation = evaluate_junctions(
        &assembly_records,
        &truths,
        config,
        &root.join("junctions.tsv"),
    )?;
    let raw_alignments_sha256 = sha256_file(&root.join("alignments.tsv"))?;
    let raw_junctions_sha256 = sha256_file(&root.join("junctions.tsv"))?;

    let evaluation_id = evaluation_id(
        &dataset.dataset_id,
        &dataset_sha256,
        &truth_sha256,
        &assembly_sha256,
        &content_binding,
        config,
    );
    let result = summarize(
        config,
        &dataset,
        &truths,
        &assembly_records,
        &alignment_rows,
        recovery,
        junction_evaluation,
        exact_alignment_scan_bases,
        evaluation_id,
        dataset_sha256,
        truth_sha256,
        assembly_sha256,
        raw_alignments_sha256,
        raw_junctions_sha256,
        content_binding,
        truth_evidence,
    )?;
    validate_evaluation_result(&result)?;
    write_json(&root.join("result.json"), &result)?;
    write_checksum_manifest(
        root,
        &[
            "alignments.tsv".to_owned(),
            "junctions.tsv".to_owned(),
            "result.json".to_owned(),
        ],
    )?;
    commit_staging(&staging, &config.output_directory)?;
    Ok(result)
}

fn validate_config(config: &EvaluationConfig) -> ValidationResult<()> {
    if !(1..=MAX_JUNCTION_FLANK_LENGTH).contains(&config.junction_flank_length) {
        return Err(ValidationError::Configuration(format!(
            "junction flank length must be between 1 and {MAX_JUNCTION_FLANK_LENGTH}"
        )));
    }
    if config.max_edit_rate_ppm > ERROR_RATE_SCALE as u32 {
        return Err(ValidationError::Configuration(
            "maximum edit rate must be at most 1000000 ppm".to_owned(),
        ));
    }
    if config.max_dp_cells == 0 || config.max_dp_cells > MAX_TOTAL_ALIGNMENT_DP_CELLS {
        return Err(ValidationError::Configuration(format!(
            "maximum DP cells must be between 1 and {MAX_TOTAL_ALIGNMENT_DP_CELLS}"
        )));
    }
    if config.max_exact_alignment_scan_bases == 0 {
        return Err(ValidationError::Configuration(
            "maximum exact-alignment scan bases must be positive".to_owned(),
        ));
    }
    if config.max_junction_comparisons == 0 {
        return Err(ValidationError::Configuration(
            "maximum junction comparisons must be positive".to_owned(),
        ));
    }
    if config.max_junction_index_bytes == 0 || config.max_junction_evidence_bytes == 0 {
        return Err(ValidationError::Configuration(
            "junction index and evidence byte limits must be positive".to_owned(),
        ));
    }
    if config.max_truth_evidence_replay_bytes == 0 {
        return Err(ValidationError::Configuration(
            "truth-evidence replay byte limit must be positive".to_owned(),
        ));
    }
    if config
        .expected_dataset_content_root_sha256
        .as_deref()
        .is_some_and(|digest| !is_lower_sha256(digest))
    {
        return Err(ValidationError::Configuration(
            "expected dataset content root must be exactly 64 lowercase hexadecimal characters"
                .to_owned(),
        ));
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn verify_dataset_content_binding(
    dataset: &DatasetManifest,
    dataset_bytes: &[u8],
    root: &DirectoryAnchor,
    expected_content_root_sha256: Option<&str>,
) -> ValidationResult<DatasetContentBinding> {
    if dataset.files.len() > MAX_DATASET_FILE_RECORDS {
        return Err(ValidationError::Resource(format!(
            "dataset file record count {} exceeds fixed cap {MAX_DATASET_FILE_RECORDS}",
            dataset.files.len()
        )));
    }
    let manifest_bytes = read_file_bounded_beneath(
        root,
        Path::new(DATASET_CHECKSUM_MANIFEST_PATH),
        MAX_DATASET_CHECKSUM_MANIFEST_BYTES,
        "dataset checksum manifest",
    )?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(dataset.files.len().saturating_add(1))
        .map_err(|error| {
            ValidationError::Resource(format!(
                "cannot allocate dataset checksum inventory: {error}"
            ))
        })?;
    for record in &dataset.files {
        checked_relative_path(&record.path)?;
        if !is_lower_sha256(&record.sha256) {
            return Err(ValidationError::Integrity(format!(
                "dataset artifact has a noncanonical SHA-256: {:?}",
                record.path
            )));
        }
        entries.push((record.path.as_str(), record.sha256.as_str()));
    }
    let dataset_sha256 = sha256_bytes(dataset_bytes);
    entries.push(("dataset.json", dataset_sha256.as_str()));
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    if entries.windows(2).any(|window| window[0].0 == window[1].0) {
        return Err(ValidationError::Integrity(
            "dataset checksum inventory contains duplicate paths".to_owned(),
        ));
    }
    let projected_bytes = entries.iter().try_fold(0usize, |total, (path, _)| {
        total.checked_add(64 + 2 + path.len() + 1).ok_or_else(|| {
            ValidationError::Resource(
                "canonical dataset checksum manifest length overflow".to_owned(),
            )
        })
    })?;
    if u64::try_from(projected_bytes).map_err(|_| {
        ValidationError::Resource(
            "canonical dataset checksum manifest length exceeds u64".to_owned(),
        )
    })? > MAX_DATASET_CHECKSUM_MANIFEST_BYTES
    {
        return Err(ValidationError::Resource(format!(
            "canonical dataset checksum manifest exceeds fixed cap {MAX_DATASET_CHECKSUM_MANIFEST_BYTES}"
        )));
    }
    let mut canonical = String::new();
    canonical
        .try_reserve_exact(projected_bytes)
        .map_err(|error| {
            ValidationError::Resource(format!(
                "cannot allocate canonical dataset checksum manifest: {error}"
            ))
        })?;
    for (path, digest) in entries {
        writeln!(&mut canonical, "{digest}  {path}")
            .expect("writing canonical checksum entries to String cannot fail");
    }
    if manifest_bytes != canonical.as_bytes() {
        return Err(ValidationError::Integrity(
            "dataset checksum manifest is not the exact canonical sorted complete inventory"
                .to_owned(),
        ));
    }
    let content_root_sha256 = sha256_bytes(&manifest_bytes);
    let mode = if let Some(expected) = expected_content_root_sha256 {
        if expected != content_root_sha256 {
            return Err(ValidationError::Integrity(format!(
                "dataset content root differs from externally supplied expectation: expected {expected}, observed {content_root_sha256}"
            )));
        }
        DatasetBindingMode::ExternallyBound
    } else {
        DatasetBindingMode::DevelopmentUnbound
    };
    Ok(DatasetContentBinding {
        content_root_sha256,
        mode,
        expected_content_root_sha256: expected_content_root_sha256.map(str::to_owned),
    })
}

fn validate_dataset_manifest(
    dataset: &DatasetManifest,
    root: &DirectoryAnchor,
) -> ValidationResult<()> {
    if dataset.schema_version != "veritasm-validation-dataset-v2" {
        return Err(ValidationError::Integrity(format!(
            "unsupported dataset schema version {:?}",
            dataset.schema_version
        )));
    }
    if dataset.evaluation_truth.namespace != "evaluation_only" {
        return Err(ValidationError::Integrity(
            "truth namespace must be evaluation_only".to_owned(),
        ));
    }
    validate_dataset_contract_metadata(dataset)?;

    let contract = case_contract(&dataset.case_id)?;
    if dataset.assembler_input.mode != contract.mode {
        return Err(ValidationError::Integrity(format!(
            "case {:?} requires assembler input mode {:?}, found {:?}",
            dataset.case_id, contract.mode, dataset.assembler_input.mode
        )));
    }
    if dataset.assembler_input.manifest_path != INPUT_MANIFEST_PATH {
        return Err(ValidationError::Integrity(format!(
            "assembler input manifest path must be {INPUT_MANIFEST_PATH:?}"
        )));
    }
    for (actual, expected, label) in [
        (
            dataset.evaluation_truth.fasta_path.as_str(),
            TRUTH_FASTA_PATH,
            "truth FASTA",
        ),
        (
            dataset.evaluation_truth.origins_path.as_str(),
            ORIGINS_PATH,
            "origin ledger",
        ),
        (
            dataset.evaluation_truth.errors_path.as_str(),
            ERRORS_PATH,
            "error ledger",
        ),
        (
            dataset.evaluation_truth.quality_events_path.as_str(),
            QUALITY_EVENTS_PATH,
            "quality-event ledger",
        ),
    ] {
        if actual != expected {
            return Err(ValidationError::Integrity(format!(
                "{label} path must be {expected:?}"
            )));
        }
    }

    let mut paths = BTreeSet::new();
    let mut previous_path: Option<&str> = None;
    if dataset.files.len() > MAX_DATASET_FILE_RECORDS {
        return Err(ValidationError::Resource(format!(
            "dataset file record count {} exceeds fixed cap {MAX_DATASET_FILE_RECORDS}",
            dataset.files.len()
        )));
    }
    let mut declared_artifact_bytes = 0u64;
    for file in &dataset.files {
        if !paths.insert(file.path.as_str()) {
            return Err(ValidationError::Integrity(format!(
                "duplicate dataset file path {:?}",
                file.path
            )));
        }
        if previous_path.is_some_and(|previous| previous.as_bytes() > file.path.as_bytes()) {
            return Err(ValidationError::Integrity(
                "dataset file paths are not unsigned-byte lexicographically sorted".to_owned(),
            ));
        }
        checked_relative_path(&file.path)?;
        match file.visibility.as_str() {
            "assembler_input" if !file.path.starts_with("assembler_input/") => {
                return Err(ValidationError::Integrity(format!(
                    "assembler_input artifact is outside its namespace: {:?}",
                    file.path
                )));
            }
            "evaluation_only" if !file.path.starts_with("evaluation_truth/") => {
                return Err(ValidationError::Integrity(format!(
                    "evaluation_only artifact is outside its namespace: {:?}",
                    file.path
                )));
            }
            "assembler_input" | "evaluation_only" => {}
            _ => {
                return Err(ValidationError::Integrity(format!(
                    "invalid dataset artifact visibility {:?}: {:?}",
                    file.path, file.visibility
                )));
            }
        }
        let declared_bytes = decimal_u64(&file.bytes_decimal, "file bytes_decimal")?;
        if declared_bytes > MAX_DATASET_ARTIFACT_BYTES {
            return Err(ValidationError::Resource(format!(
                "dataset artifact {:?} declares {declared_bytes} bytes, above fixed cap {MAX_DATASET_ARTIFACT_BYTES}",
                file.path
            )));
        }
        declared_artifact_bytes = declared_artifact_bytes
            .checked_add(declared_bytes)
            .ok_or_else(|| {
                ValidationError::Resource("dataset artifact byte total overflow".to_owned())
            })?;
        if declared_artifact_bytes > MAX_TOTAL_DATASET_ARTIFACT_BYTES {
            return Err(ValidationError::Resource(format!(
                "declared dataset artifact bytes {declared_artifact_bytes} exceed fixed aggregate cap {MAX_TOTAL_DATASET_ARTIFACT_BYTES}"
            )));
        }
        previous_path = Some(file.path.as_str());
    }

    require_file_contract(
        dataset,
        INPUT_MANIFEST_PATH,
        "assembler_input_manifest",
        "assembler_input",
    )?;
    require_file_contract(
        dataset,
        TRUTH_FASTA_PATH,
        "truth_sequences",
        "evaluation_only",
    )?;
    require_file_contract(
        dataset,
        ORIGINS_PATH,
        "read_origin_ledger",
        "evaluation_only",
    )?;
    require_file_contract(
        dataset,
        ERRORS_PATH,
        "error_event_ledger",
        "evaluation_only",
    )?;
    require_file_contract(
        dataset,
        QUALITY_EVENTS_PATH,
        "quality_event_ledger",
        "evaluation_only",
    )?;

    let expected_read_paths = usize::try_from(contract.read_count_per_fragment)
        .expect("read count per fragment is one or two");
    if dataset.assembler_input.read_paths.len() != expected_read_paths {
        return Err(ValidationError::Integrity(format!(
            "assembler input mode {:?} requires {expected_read_paths} read path(s)",
            dataset.assembler_input.mode
        )));
    }
    let extension = if dataset.generator.parameters.output_compression == "gzip" {
        "fastq.gz"
    } else {
        "fastq"
    };
    let expected_generated_read_paths = if contract.mode == "single_end" {
        vec![format!("assembler_input/reads_SE.{extension}")]
    } else {
        vec![
            format!("assembler_input/reads_R1.{extension}"),
            format!("assembler_input/reads_R2.{extension}"),
        ]
    };
    if dataset.assembler_input.read_paths != expected_generated_read_paths {
        return Err(ValidationError::Integrity(
            "assembler-input read paths differ from the exact generator-v4 role/compression paths"
                .to_owned(),
        ));
    }
    let mut logical_paths = BTreeSet::from([
        INPUT_MANIFEST_PATH,
        TRUTH_FASTA_PATH,
        ORIGINS_PATH,
        ERRORS_PATH,
        QUALITY_EVENTS_PATH,
    ]);
    for (index, read_path) in dataset.assembler_input.read_paths.iter().enumerate() {
        if !logical_paths.insert(read_path.as_str()) {
            return Err(ValidationError::Integrity(format!(
                "duplicate or overlapping logical dataset path {read_path:?}"
            )));
        }
        let expected_role = match (contract.mode, index) {
            ("single_end", 0) => "single_end_reads",
            ("paired_end", 0) => "read_1",
            ("paired_end", 1) => "read_2",
            _ => unreachable!("mode and path cardinality validated"),
        };
        require_file_contract(dataset, read_path, expected_role, "assembler_input")?;
    }
    for file in &dataset.files {
        if !logical_paths.contains(file.path.as_str()) {
            return Err(ValidationError::Integrity(format!(
                "undeclared dataset artifact {:?}",
                file.path
            )));
        }
    }

    validate_declared_counts(dataset, contract)?;
    validate_truth_layout(dataset, contract.truth_layout)?;

    for file in &dataset.files {
        verify_file_record(root, file)?;
    }
    validate_assembler_input_manifest(dataset, root, contract)?;
    Ok(())
}

fn validate_dataset_contract_metadata(dataset: &DatasetManifest) -> ValidationResult<()> {
    if dataset.tier != "tier_1_truth_known_computational"
        || dataset.role != "development_qualification_not_release_scorecard"
    {
        return Err(ValidationError::Integrity(
            "dataset tier or role differs from the validation-v1 contract".to_owned(),
        ));
    }
    if dataset.generator.name != "veritasm-simulate"
        || dataset.generator.version != GENERATOR_VERSION
        || dataset.generator.plan_version != VALIDATION_PLAN_VERSION
        || dataset.generator.rng_name != VALIDATION_RNG_NAME
        || dataset.generator.rng_byte_order != VALIDATION_RNG_BYTE_ORDER
        || dataset.generator.rng_streams.derivation != "sha256-domain-separated-stream-v1"
    {
        return Err(ValidationError::Integrity(
            "generator identity differs from the validation-v1 contract".to_owned(),
        ));
    }
    let replicate = decimal_u64(&dataset.replicate_decimal, "dataset replicate_decimal")?;
    if replicate > u64::from(u32::MAX) {
        return Err(ValidationError::Integrity(
            "dataset replicate exceeds generator u32 range".to_owned(),
        ));
    }
    if dataset.generator.seed_hex.len() != 64
        || !dataset
            .generator
            .seed_hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ValidationError::Integrity(
            "generator seed is not 256-bit lowercase hexadecimal".to_owned(),
        ));
    }
    match dataset.generator.seed_source.as_str() {
        "explicit" => {}
        "sha256_plan_case_replicate" => {
            let mut digest = Sha256::new();
            digest.update(VALIDATION_PLAN_VERSION.as_bytes());
            digest.update([0]);
            digest.update(dataset.case_id.as_bytes());
            digest.update([0]);
            digest.update(replicate.to_string().as_bytes());
            if super::lower_hex(&digest.finalize()) != dataset.generator.seed_hex {
                return Err(ValidationError::Integrity(
                    "derived generator seed differs from case and replicate".to_owned(),
                ));
            }
        }
        _ => {
            return Err(ValidationError::Integrity(format!(
                "unsupported generator seed source {:?}",
                dataset.generator.seed_source
            )));
        }
    }
    let master_seed = super::decode_hex_32(&dataset.generator.seed_hex)?;
    for (label, actual) in [
        (
            b"layout".as_slice(),
            dataset.generator.rng_streams.layout_seed_hex.as_str(),
        ),
        (
            b"error".as_slice(),
            dataset.generator.rng_streams.error_seed_hex.as_str(),
        ),
    ] {
        if derived_stream_seed_hex(master_seed, label) != actual {
            return Err(ValidationError::Integrity(
                "generator layout/error stream seed differs from the master seed".to_owned(),
            ));
        }
    }
    match dataset.generator.rng_streams.quality_seed_source.as_str() {
        "derived_from_master_seed"
            if derived_stream_seed_hex(master_seed, b"quality")
                == dataset.generator.rng_streams.quality_seed_hex => {}
        "explicit" => {
            super::decode_hex_32(&dataset.generator.rng_streams.quality_seed_hex)?;
        }
        _ => {
            return Err(ValidationError::Integrity(
                "generator quality stream seed or source is inconsistent".to_owned(),
            ));
        }
    }
    let expected_dataset_id = dataset_id_from_parameters(
        &dataset.case_id,
        &dataset.replicate_decimal,
        &dataset.generator.seed_hex,
        &dataset.generator.seed_source,
        &dataset.generator.rng_streams.layout_seed_hex,
        &dataset.generator.rng_streams.error_seed_hex,
        &dataset.generator.rng_streams.quality_seed_hex,
        &dataset.generator.rng_streams.quality_seed_source,
        &dataset.generator.parameters,
    );
    if dataset.dataset_id != expected_dataset_id {
        return Err(ValidationError::Integrity(
            "dataset ID differs from the full canonical generator-parameter commitment".to_owned(),
        ));
    }

    let generator_case = GeneratorCase::from_str(&dataset.case_id).map_err(|error| {
        ValidationError::Integrity(format!("declared generator case is invalid: {error}"))
    })?;
    let shared_parameters =
        validate_generator_parameters(generator_case, &dataset.generator.parameters).map_err(
            |error| match error {
                ValidationError::Resource(message) => ValidationError::Integrity(format!(
                    "declared generator-v4 tuple is not generator-feasible: {message}"
                )),
                ValidationError::Configuration(message) | ValidationError::Integrity(message) => {
                    ValidationError::Integrity(format!(
                        "declared generator-v4 parameter contract is invalid: {message}"
                    ))
                }
                other => other,
            },
        )?;

    let fragments = decimal_u64(
        &dataset.generator.parameters.fragments_decimal,
        "generator fragments_decimal",
    )?;
    let truth_length = decimal_u64(
        &dataset.generator.parameters.truth_length_decimal,
        "generator truth_length_decimal",
    )?;
    let read_length = decimal_u64(
        &dataset.generator.parameters.read_length_decimal,
        "generator read_length_decimal",
    )?;
    let insert_length = decimal_u64(
        &dataset.generator.parameters.insert_length_decimal,
        "generator insert_length_decimal",
    )?;
    let substitution_rate = decimal_u64(
        &dataset.generator.parameters.substitution_rate_ppm_decimal,
        "generator substitution_rate_ppm_decimal",
    )?;
    let low_quality_rate = decimal_u64(
        &dataset.generator.parameters.low_quality_rate_ppm_decimal,
        "generator low_quality_rate_ppm_decimal",
    )?;
    if fragments > MAX_GENERATED_FRAGMENTS
        || truth_length > MAX_GENERATED_TRUTH_LENGTH
        || read_length > MAX_GENERATED_READ_LENGTH
    {
        return Err(ValidationError::Integrity(
            "generator size parameters exceed frozen generator bounds".to_owned(),
        ));
    }
    if truth_length == 0 || read_length == 0 {
        return Err(ValidationError::Integrity(
            "generator truth and read lengths must be positive".to_owned(),
        ));
    }
    if substitution_rate > ERROR_RATE_SCALE || low_quality_rate > ERROR_RATE_SCALE {
        return Err(ValidationError::Integrity(
            "generator substitution or low-quality rate exceeds one million ppm".to_owned(),
        ));
    }
    if dataset.generator.parameters.error_model
        != "independent_substitution_bernoulli_per_observed_base_v1"
        || dataset.generator.parameters.default_quality_phred_decimal != "40"
        || dataset.generator.parameters.low_quality_phred_decimal != "10"
    {
        return Err(ValidationError::Integrity(
            "generator error or quality value model differs from version 4".to_owned(),
        ));
    }
    if !matches!(
        dataset.generator.parameters.output_compression.as_str(),
        "plain" | "gzip"
    ) {
        return Err(ValidationError::Integrity(
            "generator output compression differs from version 4".to_owned(),
        ));
    }
    for molecule in &dataset.evaluation_truth.molecules {
        let molecule_length = decimal_u64(
            &molecule.length_bases_decimal,
            "truth molecule length_bases_decimal",
        )?;
        if molecule_length != truth_length {
            return Err(ValidationError::Integrity(
                "truth molecule length differs from generator truth length".to_owned(),
            ));
        }
    }
    if shared_parameters.fragments != fragments
        || shared_parameters.truth_length as u64 != truth_length
        || shared_parameters.read_length as u64 != read_length
        || shared_parameters.insert_length as u64 != insert_length
        || u64::from(shared_parameters.substitution_rate_ppm) != substitution_rate
        || u64::from(shared_parameters.low_quality_rate_ppm) != low_quality_rate
    {
        return Err(ValidationError::Integrity(
            "shared generator parameter interpretation does not reconcile".to_owned(),
        ));
    }
    let (expected_fraction, expected_orientation, reads_per_fragment) =
        match dataset.case_id.as_str() {
            "linear-se" => ("not_applicable", "not_applicable", 1u64),
            "mixture-pe" => (
                "0.1_exact_count_rounded_down",
                "inward_FR_with_random_truth_strand",
                2,
            ),
            "linear-pe" | "circular-pe" | "error-pe" | "qc-censoring-control" => {
                ("not_applicable", "inward_FR_with_random_truth_strand", 2)
            }
            _ => {
                return Err(ValidationError::Integrity(format!(
                    "unsupported validation case ID {:?}",
                    dataset.case_id
                )));
            }
        };
    if read_length > truth_length
        || (reads_per_fragment == 2
            && (insert_length < read_length || insert_length > truth_length))
    {
        return Err(ValidationError::Integrity(
            "generator read or paired-insert span is inconsistent with truth length".to_owned(),
        ));
    }
    let generated_read_bases = fragments
        .checked_mul(reads_per_fragment)
        .and_then(|reads| reads.checked_mul(read_length))
        .ok_or_else(|| {
            ValidationError::Resource("generated read-base count overflow".to_owned())
        })?;
    if generated_read_bases > MAX_GENERATED_READ_BASES {
        return Err(ValidationError::Integrity(
            "generator read-base count exceeds frozen generator bound".to_owned(),
        ));
    }
    if dataset.generator.parameters.mixture_minor_fragment_fraction != expected_fraction
        || dataset.generator.parameters.library_orientation != expected_orientation
    {
        return Err(ValidationError::Integrity(
            "generator mixture or library metadata differs from the case contract".to_owned(),
        ));
    }
    let expected_quality_model = if dataset.case_id == "qc-censoring-control" {
        if low_quality_rate != 0 {
            return Err(ValidationError::Integrity(
                "qc-censoring-control requires a zero independent low-quality rate".to_owned(),
            ));
        }
        "qc_censoring_substitution_q10_else_q40_v1"
    } else {
        "independent_bernoulli_q10_else_q40_v1"
    };
    if dataset.generator.parameters.quality_model != expected_quality_model {
        return Err(ValidationError::Integrity(
            "generator quality model differs from the case contract".to_owned(),
        ));
    }
    if dataset.assembler_input.contract.is_empty() {
        return Err(ValidationError::Integrity(
            "dataset input contract must not be empty".to_owned(),
        ));
    }
    if dataset.evaluation_truth.coordinate_system != VALIDATION_COORDINATE_SYSTEM {
        return Err(ValidationError::Integrity(
            "truth coordinate system differs from the generator-v4 contract".to_owned(),
        ));
    }
    Ok(())
}

fn derived_stream_seed_hex(master_seed: [u8; 32], stream: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:validation-rng-stream:v1\0");
    digest.update(master_seed);
    digest.update(stream);
    super::lower_hex(&digest.finalize())
}

fn case_contract(case_id: &str) -> ValidationResult<CaseContract> {
    match case_id {
        "linear-se" => Ok(CaseContract {
            mode: "single_end",
            read_count_per_fragment: 1,
            truth_layout: TruthLayout::Single(TruthTopology::Linear),
        }),
        "linear-pe" | "error-pe" | "qc-censoring-control" => Ok(CaseContract {
            mode: "paired_end",
            read_count_per_fragment: 2,
            truth_layout: TruthLayout::Single(TruthTopology::Linear),
        }),
        "circular-pe" => Ok(CaseContract {
            mode: "paired_end",
            read_count_per_fragment: 2,
            truth_layout: TruthLayout::Single(TruthTopology::Circular),
        }),
        "mixture-pe" => Ok(CaseContract {
            mode: "paired_end",
            read_count_per_fragment: 2,
            truth_layout: TruthLayout::LinearPrimaryMinor,
        }),
        _ => Err(ValidationError::Integrity(format!(
            "unsupported validation case ID {case_id:?}"
        ))),
    }
}

fn require_file_contract(
    dataset: &DatasetManifest,
    path: &str,
    expected_role: &str,
    expected_visibility: &str,
) -> ValidationResult<()> {
    let record = dataset
        .files
        .iter()
        .find(|record| record.path == path)
        .ok_or_else(|| {
            ValidationError::Integrity(format!("dataset files omit declared path {path:?}"))
        })?;
    if record.role != expected_role || record.visibility != expected_visibility {
        return Err(ValidationError::Integrity(format!(
            "dataset artifact {path:?} must have role {expected_role:?} and visibility {expected_visibility:?}"
        )));
    }
    Ok(())
}

fn validate_declared_counts(
    dataset: &DatasetManifest,
    contract: CaseContract,
) -> ValidationResult<()> {
    let fragments = decimal_u64(
        &dataset.assembler_input.fragments_decimal,
        "assembler input fragments_decimal",
    )?;
    let generated_fragments = decimal_u64(
        &dataset.generator.parameters.fragments_decimal,
        "generator fragments_decimal",
    )?;
    if fragments != generated_fragments {
        return Err(ValidationError::Integrity(
            "generator and assembler-input fragment counts differ".to_owned(),
        ));
    }
    let reads = decimal_u64(
        &dataset.assembler_input.reads_decimal,
        "assembler input reads_decimal",
    )?;
    let expected_reads = fragments
        .checked_mul(contract.read_count_per_fragment)
        .ok_or_else(|| ValidationError::Resource("declared read count overflow".to_owned()))?;
    if reads != expected_reads {
        return Err(ValidationError::Integrity(format!(
            "assembler input mode {:?} requires {expected_reads} reads for {fragments} fragments, found {reads}",
            dataset.assembler_input.mode
        )));
    }
    Ok(())
}

fn validate_truth_layout(dataset: &DatasetManifest, layout: TruthLayout) -> ValidationResult<()> {
    let molecules = &dataset.evaluation_truth.molecules;
    let mut identifiers = BTreeSet::new();
    for molecule in molecules {
        if !valid_evidence_identifier(&molecule.molecule_id) {
            return Err(ValidationError::Integrity(format!(
                "truth molecule ID {:?} does not match [A-Za-z0-9._-]+",
                molecule.molecule_id
            )));
        }
        if !identifiers.insert(molecule.molecule_id.as_str()) {
            return Err(ValidationError::Integrity(
                "duplicate truth molecule ID".to_owned(),
            ));
        }
    }
    match layout {
        TruthLayout::Single(topology) => {
            if molecules.len() != 1
                || molecules[0].truth_class != TruthClass::Primary
                || molecules[0].topology != topology
            {
                return Err(ValidationError::Integrity(format!(
                    "case {:?} requires exactly one primary {topology:?} truth molecule",
                    dataset.case_id
                )));
            }
        }
        TruthLayout::LinearPrimaryMinor => {
            let primary = molecules
                .iter()
                .filter(|molecule| molecule.truth_class == TruthClass::Primary)
                .count();
            let minor = molecules
                .iter()
                .filter(|molecule| molecule.truth_class == TruthClass::Minor)
                .count();
            if molecules.len() != 2
                || primary != 1
                || minor != 1
                || molecules
                    .iter()
                    .any(|molecule| molecule.topology != TruthTopology::Linear)
            {
                return Err(ValidationError::Integrity(
                    "mixture-pe requires exactly one primary and one minor linear truth molecule"
                        .to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn valid_evidence_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_assembler_input_manifest(
    dataset: &DatasetManifest,
    root: &DirectoryAnchor,
    contract: CaseContract,
) -> ValidationResult<()> {
    let relative = checked_relative_path(&dataset.assembler_input.manifest_path)?;
    let bytes = read_file_bounded_beneath(
        root,
        &relative,
        MAX_ASSEMBLER_INPUT_MANIFEST_BYTES,
        "assembler input manifest",
    )?;
    let file_record = dataset
        .files
        .iter()
        .find(|record| record.path == dataset.assembler_input.manifest_path)
        .expect("assembler input manifest file contract validated");
    let expected_bytes = decimal_u64(
        &file_record.bytes_decimal,
        "assembler input manifest bytes_decimal",
    )?;
    if u64::try_from(bytes.len()).map_err(|_| {
        ValidationError::Resource("assembler input manifest size exceeds u64".to_owned())
    })? != expected_bytes
        || sha256_bytes(&bytes) != file_record.sha256
    {
        return Err(ValidationError::Integrity(
            "assembler input manifest changed during validation".to_owned(),
        ));
    }
    let manifest: AssemblerInputManifest = serde_json::from_slice(&bytes)?;
    if manifest.schema_version != "veritasm-assembler-input-v1" {
        return Err(ValidationError::Integrity(format!(
            "unsupported assembler input schema version {:?}",
            manifest.schema_version
        )));
    }
    if manifest.dataset_id != dataset.dataset_id || manifest.mode != dataset.assembler_input.mode {
        return Err(ValidationError::Integrity(
            "assembler input manifest identity or mode differs from dataset manifest".to_owned(),
        ));
    }
    if manifest.truth_exclusion.is_empty() {
        return Err(ValidationError::Integrity(
            "assembler input manifest truth_exclusion must not be empty".to_owned(),
        ));
    }
    let manifest_fragments = decimal_u64(
        &manifest.fragments_decimal,
        "input manifest fragments_decimal",
    )?;
    let manifest_reads = decimal_u64(&manifest.reads_decimal, "input manifest reads_decimal")?;
    let outer_fragments = decimal_u64(
        &dataset.assembler_input.fragments_decimal,
        "assembler input fragments_decimal",
    )?;
    let outer_reads = decimal_u64(
        &dataset.assembler_input.reads_decimal,
        "assembler input reads_decimal",
    )?;
    if manifest_fragments != outer_fragments || manifest_reads != outer_reads {
        return Err(ValidationError::Integrity(
            "assembler input manifest counts differ from dataset manifest".to_owned(),
        ));
    }
    if manifest.read_files.len() != dataset.assembler_input.read_paths.len() {
        return Err(ValidationError::Integrity(
            "assembler input manifest read-file cardinality differs from dataset manifest"
                .to_owned(),
        ));
    }
    let mut input_paths = BTreeSet::new();
    for (index, (file, expected_path)) in manifest
        .read_files
        .iter()
        .zip(&dataset.assembler_input.read_paths)
        .enumerate()
    {
        if !input_paths.insert(file.path.as_str()) {
            return Err(ValidationError::Integrity(
                "duplicate read path in assembler input manifest".to_owned(),
            ));
        }
        let expected_role = match (contract.mode, index) {
            ("single_end", 0) => "S",
            ("paired_end", 0) => "R1",
            ("paired_end", 1) => "R2",
            _ => unreachable!("mode and path cardinality validated"),
        };
        if file.path != *expected_path || file.role != expected_role {
            return Err(ValidationError::Integrity(format!(
                "assembler input read {index} must have role {expected_role:?} and match the declared path"
            )));
        }
        let expected_sha256 = dataset
            .files
            .iter()
            .find(|record| record.path == file.path)
            .expect("declared read file contract validated")
            .sha256
            .as_str();
        if file.sha256 != expected_sha256 {
            return Err(ValidationError::Integrity(format!(
                "assembler input read hash differs for {:?}",
                file.path
            )));
        }
    }
    Ok(())
}

fn verify_file_record(root: &DirectoryAnchor, record: &FileRecord) -> ValidationResult<()> {
    let relative = checked_relative_path(&record.path)?;
    let expected_bytes = decimal_u64(&record.bytes_decimal, "file bytes_decimal")?;
    let actual = sha256_file_exact_bounded_beneath(
        root,
        &relative,
        expected_bytes,
        MAX_DATASET_ARTIFACT_BYTES,
        "dataset artifact",
    )?;
    if actual != record.sha256 {
        return Err(ValidationError::Integrity(format!(
            "dataset artifact SHA-256 mismatch: {:?}",
            record.path
        )));
    }
    Ok(())
}

fn load_truth(
    dataset: &DatasetManifest,
    root: &DirectoryAnchor,
) -> ValidationResult<(Vec<TruthMolecule>, String)> {
    let relative = checked_relative_path(&dataset.evaluation_truth.fasta_path)?;
    let snapshot = read_fasta_snapshot_beneath(
        root,
        &relative,
        false,
        MAX_TRUTH_FASTA_BYTES,
        MAX_TRUTH_FASTA_RECORDS,
        "truth FASTA",
    )?;
    let file_record = dataset
        .files
        .iter()
        .find(|record| record.path == dataset.evaluation_truth.fasta_path)
        .ok_or_else(|| {
            ValidationError::Integrity("dataset files omit the truth FASTA".to_owned())
        })?;
    let expected_bytes = decimal_u64(&file_record.bytes_decimal, "truth FASTA bytes_decimal")?;
    if snapshot.byte_length != expected_bytes || snapshot.sha256 != file_record.sha256 {
        return Err(ValidationError::Integrity(
            "truth FASTA changed before the evaluator snapshot".to_owned(),
        ));
    }
    if snapshot.records.len() != dataset.evaluation_truth.molecules.len() {
        return Err(ValidationError::Integrity(
            "truth FASTA record count differs from dataset manifest".to_owned(),
        ));
    }
    let truths = snapshot
        .records
        .into_iter()
        .zip(&dataset.evaluation_truth.molecules)
        .map(|(record, manifest)| {
            if record.id != manifest.molecule_id {
                return Err(ValidationError::Integrity(format!(
                    "truth FASTA record order differs from molecule manifest: found {:?}, expected {:?}",
                    record.id, manifest.molecule_id
                )));
            }
            validate_truth_record(manifest, &record)?;
            Ok(TruthMolecule {
                id: record.id,
                class: manifest.truth_class,
                topology: manifest.topology,
                sequence: record.sequence,
            })
        })
        .collect::<ValidationResult<Vec<_>>>()?;
    Ok((truths, snapshot.sha256))
}

fn validate_truth_evidence(
    config: &EvaluationConfig,
    dataset: &DatasetManifest,
    root: &DirectoryAnchor,
    truths: &[TruthMolecule],
) -> ValidationResult<TruthEvidenceAudit> {
    let generator_case = GeneratorCase::from_str(&dataset.case_id).map_err(|error| {
        ValidationError::Integrity(format!("declared generator case is invalid: {error}"))
    })?;
    let parameters = validate_generator_parameters(generator_case, &dataset.generator.parameters)?;
    let fragments = parameters.fragments;
    let read_length = parameters.read_length;
    let insert_length = parameters.insert_length;
    let paired = parameters.paired;
    let reads_per_fragment = if paired { 2usize } else { 1usize };
    let expected_reads = usize::try_from(parameters.read_count)
        .map_err(|_| ValidationError::Resource("evidence read count exceeds usize".to_owned()))?;
    let read_length_u64 = u64::try_from(read_length)
        .map_err(|_| ValidationError::Resource("evidence read length exceeds u64".to_owned()))?;
    let total_read_bases = parameters
        .read_count
        .checked_mul(read_length_u64)
        .ok_or_else(|| ValidationError::Resource("evidence read-base count overflow".to_owned()))?;
    let declared_replay_snapshot_bytes = dataset
        .assembler_input
        .read_paths
        .iter()
        .map(String::as_str)
        .chain([ORIGINS_PATH, ERRORS_PATH, QUALITY_EVENTS_PATH])
        .try_fold(0u64, |total, path| {
            let record = dataset
                .files
                .iter()
                .find(|record| record.path == path)
                .expect("truth-evidence artifact contract validated");
            total
                .checked_add(decimal_u64(
                    &record.bytes_decimal,
                    "truth-evidence replay artifact bytes_decimal",
                )?)
                .ok_or_else(|| {
                    ValidationError::Resource(
                        "truth-evidence replay artifact byte total overflow".to_owned(),
                    )
                })
        })?;
    // Below the generator fragment cap every generated identifier has a fixed 12-digit ordinal.
    // Decoded FASTQ bytes coexist with copied sequence/quality vectors during parsing, so admit
    // them separately from both the stored artifact snapshots and retained read bases.
    let fastq_identifier_bytes = if paired { 17u64 } else { 15u64 };
    let decoded_fastq_bytes_per_read = read_length_u64
        .checked_mul(2)
        .and_then(|bases| bases.checked_add(fastq_identifier_bytes + 6))
        .ok_or_else(|| {
            ValidationError::Resource("decoded FASTQ byte projection overflow".to_owned())
        })?;
    let decoded_fastq_bytes = parameters
        .read_count
        .checked_mul(decoded_fastq_bytes_per_read)
        .ok_or_else(|| {
            ValidationError::Resource("decoded FASTQ byte projection overflow".to_owned())
        })?;
    let retained_truth_bases = truths.iter().try_fold(0u64, |total, truth| {
        total
            .checked_add(u64::try_from(truth.sequence.len()).map_err(|_| {
                ValidationError::Resource("truth sequence length exceeds u64".to_owned())
            })?)
            .ok_or_else(|| {
                ValidationError::Resource("retained truth-base projection overflow".to_owned())
            })
    })?;
    let per_read_state = u64::try_from(
        std::mem::size_of::<EvidenceRead>()
            .saturating_add(std::mem::size_of::<LedgerOrigin>())
            .saturating_add(256),
    )
    .map_err(|_| ValidationError::Resource("per-read replay state exceeds u64".to_owned()))?;
    let retained_read_base_bytes = total_read_bases.checked_mul(2).ok_or_else(|| {
        ValidationError::Resource("sequence/quality replay byte projection overflow".to_owned())
    })?;
    let projected_replay_bytes = declared_replay_snapshot_bytes
        .checked_add(decoded_fastq_bytes)
        .and_then(|total| total.checked_add(retained_read_base_bytes))
        .and_then(|total| total.checked_add(retained_truth_bases))
        .and_then(|total| total.checked_add(read_length_u64))
        .and_then(|total| {
            parameters
                .read_count
                .checked_mul(per_read_state)
                .and_then(|state| total.checked_add(state))
        })
        .ok_or_else(|| {
            ValidationError::Resource("truth-evidence replay byte projection overflow".to_owned())
        })?;
    if projected_replay_bytes > config.max_truth_evidence_replay_bytes {
        return Err(ValidationError::Resource(format!(
            "projected truth-evidence replay state {projected_replay_bytes} bytes exceeds configured limit {}",
            config.max_truth_evidence_replay_bytes
        )));
    }

    let mut reads = BTreeMap::<String, EvidenceRead>::new();
    for (file_index, relative) in dataset.assembler_input.read_paths.iter().enumerate() {
        let role = match (paired, file_index) {
            (false, 0) => "S",
            (true, 0) => "R1",
            (true, 1) => "R2",
            _ => unreachable!("read path cardinality was validated"),
        };
        let parsed = read_evidence_fastq(dataset, root, relative, role, read_length, fragments)?;
        for (ordinal, (read_id, sequence, quality_phred)) in parsed.into_iter().enumerate() {
            let rank = ordinal
                .checked_mul(reads_per_fragment)
                .and_then(|value| value.checked_add(if paired { file_index } else { 0 }))
                .ok_or_else(|| ValidationError::Resource("read rank overflow".to_owned()))?;
            if reads
                .insert(
                    read_id,
                    EvidenceRead {
                        rank,
                        role,
                        sequence,
                        quality_phred,
                    },
                )
                .is_some()
            {
                return Err(ValidationError::Integrity(
                    "duplicate read identifier in assembler-input FASTQ".to_owned(),
                ));
            }
        }
    }
    if reads.len() != expected_reads {
        return Err(ValidationError::Integrity(format!(
            "assembler-input FASTQ contains {} reads, expected {expected_reads}",
            reads.len()
        )));
    }

    let truth_by_id = truths
        .iter()
        .enumerate()
        .map(|(index, truth)| (truth.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let origin_bytes = read_declared_artifact(dataset, root, ORIGINS_PATH, "origin ledger")?;
    let mut origin_rows = TsvCursor::new(
        &origin_bytes,
        ORIGINS_PATH,
        ORIGINS_HEADER,
        16,
        expected_reads,
    )?;
    let mut origins = Vec::new();
    origins.try_reserve_exact(expected_reads).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate validated origin ledger: {error}"))
    })?;
    for rank in 0..expected_reads {
        let fields = origin_rows.next::<16>()?.ok_or_else(|| {
            ValidationError::Integrity(format!(
                "origin ledger contains {} rows, expected {expected_reads}",
                origin_rows.rows_seen()
            ))
        })?;
        let line = rank + 2;
        if fields[0] != "2.0" {
            return ledger_integrity(ORIGINS_PATH, line, "schema_version must be 2.0");
        }
        let fragment_ordinal = ledger_u64(fields[1], ORIGINS_PATH, line, "fragment_ordinal")?;
        let expected_ordinal = u64::try_from(rank / reads_per_fragment)
            .map_err(|_| ValidationError::Resource("origin rank exceeds u64".to_owned()))?;
        let expected_role = if paired {
            if rank % 2 == 0 {
                "R1"
            } else {
                "R2"
            }
        } else {
            "S"
        };
        let expected_id = expected_read_id(expected_ordinal, expected_role);
        if fragment_ordinal != expected_ordinal
            || fields[2] != expected_id
            || fields[3] != expected_role
        {
            return ledger_integrity(
                ORIGINS_PATH,
                line,
                "rows must be ordered by fragment with the exact generated read ID and mate role",
            );
        }
        let read = reads.get(fields[2]).ok_or_else(|| {
            ValidationError::Integrity(format!(
                "{ORIGINS_PATH}:{line}: read_id is absent from assembler-input FASTQ"
            ))
        })?;
        if read.rank != rank || read.role != fields[3] {
            return ledger_integrity(
                ORIGINS_PATH,
                line,
                "origin order or mate role differs from assembler-input FASTQ",
            );
        }
        let molecule_index = *truth_by_id.get(fields[4]).ok_or_else(|| {
            ValidationError::Integrity(format!(
                "{ORIGINS_PATH}:{line}: molecule_id is absent from truth FASTA"
            ))
        })?;
        let truth = &truths[molecule_index];
        if fields[5] != topology_name(truth.topology) {
            return ledger_integrity(ORIGINS_PATH, line, "topology differs from truth manifest");
        }
        let truth_step = match (fields[6], fields[8]) {
            ("+", "1") => 1,
            ("-", "-1") => -1,
            _ => {
                return ledger_integrity(
                    ORIGINS_PATH,
                    line,
                    "strand and truth_step must be matching +/- and +/-1 values",
                )
            }
        };
        let first_truth_base =
            ledger_usize(fields[7], ORIGINS_PATH, line, "first_truth_base_zero_based")?;
        if ledger_usize(fields[9], ORIGINS_PATH, line, "read_length")? != read_length {
            return ledger_integrity(
                ORIGINS_PATH,
                line,
                "read_length differs from generator parameters",
            );
        }
        let outer_fragment_start = ledger_usize(
            fields[10],
            ORIGINS_PATH,
            line,
            "outer_fragment_start_zero_based",
        )?;
        let outer_fragment_span =
            ledger_usize(fields[11], ORIGINS_PATH, line, "outer_fragment_span")?;
        let expected_span = if paired { insert_length } else { read_length };
        if outer_fragment_span != expected_span
            || outer_fragment_start >= truth.sequence.len()
            || (truth.topology == TruthTopology::Linear
                && outer_fragment_start
                    .checked_add(outer_fragment_span)
                    .is_none_or(|end| end > truth.sequence.len()))
        {
            return ledger_integrity(
                ORIGINS_PATH,
                line,
                "outer-fragment geometry contradicts generator parameters or truth topology",
            );
        }
        let pre_error = replay_ledger_origin(
            truth,
            first_truth_base,
            truth_step,
            read_length,
            ORIGINS_PATH,
            line,
        )?;
        let wraps_origin = match fields[12] {
            "true" => true,
            "false" => false,
            _ => return ledger_integrity(ORIGINS_PATH, line, "wraps_origin must be true or false"),
        };
        let expected_wraps = truth.topology == TruthTopology::Circular
            && if truth_step == 1 {
                first_truth_base
                    .checked_add(read_length)
                    .is_none_or(|end| end > truth.sequence.len())
            } else {
                read_length > first_truth_base + 1
            };
        if wraps_origin != expected_wraps
            || sha256_bytes(&pre_error) != fields[13]
            || sha256_bytes(&read.sequence) != fields[14]
            || sha256_bytes(&read.quality_phred) != fields[15]
        {
            return ledger_integrity(
                ORIGINS_PATH,
                line,
                "origin geometry or sequence/quality digest does not match truth and FASTQ",
            );
        }
        origins.push(LedgerOrigin {
            fragment_ordinal,
            molecule_index,
            first_truth_base,
            truth_step,
            outer_fragment_start,
            outer_fragment_span,
        });
    }
    if origin_rows.next::<16>()?.is_some() {
        return Err(ValidationError::Integrity(format!(
            "origin ledger contains more than {expected_reads} rows"
        )));
    }

    validate_fragment_geometry(dataset, truths, &origins, paired)?;
    drop(origin_bytes);
    validate_error_and_quality_ledgers(dataset, root, truths, &reads, &origins, read_length)?;
    Ok(TruthEvidenceAudit {
        projected_replay_bytes,
    })
}

fn read_declared_artifact(
    dataset: &DatasetManifest,
    root: &DirectoryAnchor,
    relative: &str,
    label: &str,
) -> ValidationResult<Vec<u8>> {
    let record = dataset
        .files
        .iter()
        .find(|record| record.path == relative)
        .expect("required dataset artifact contract was validated");
    let expected_bytes = decimal_u64(&record.bytes_decimal, "dataset artifact bytes_decimal")?;
    let bytes = read_file_bounded_beneath(
        root,
        &checked_relative_path(relative)?,
        MAX_DATASET_ARTIFACT_BYTES,
        label,
    )?;
    if u64::try_from(bytes.len())
        .map_err(|_| ValidationError::Resource(format!("{label} size exceeds u64")))?
        != expected_bytes
        || sha256_bytes(&bytes) != record.sha256
    {
        return Err(ValidationError::Integrity(format!(
            "{label} changed before its semantic-validation snapshot"
        )));
    }
    Ok(bytes)
}

type ParsedFastqRecord = (String, Vec<u8>, Vec<u8>);

fn read_evidence_fastq(
    dataset: &DatasetManifest,
    root: &DirectoryAnchor,
    relative: &str,
    role: &str,
    read_length: usize,
    fragments: u64,
) -> ValidationResult<Vec<ParsedFastqRecord>> {
    let stored = read_declared_artifact(dataset, root, relative, "assembler-input FASTQ")?;
    let gzip_magic = stored.starts_with(&[0x1f, 0x8b]);
    let expected_gzip = dataset.generator.parameters.output_compression == "gzip";
    if gzip_magic != expected_gzip {
        return Err(ValidationError::Integrity(format!(
            "assembler-input FASTQ {relative:?} compression contradicts generator parameters"
        )));
    }
    let decoded = if gzip_magic {
        let mut bytes = Vec::new();
        MultiGzDecoder::new(Cursor::new(stored))
            .take(MAX_DECODED_READ_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).map_err(|_| {
            ValidationError::Resource("decoded assembler-input FASTQ size exceeds u64".to_owned())
        })? > MAX_DECODED_READ_FILE_BYTES
        {
            return Err(ValidationError::Resource(format!(
                "decoded assembler-input FASTQ exceeds fixed cap {MAX_DECODED_READ_FILE_BYTES}"
            )));
        }
        bytes
    } else {
        stored
    };
    let expected_records = usize::try_from(fragments)
        .map_err(|_| ValidationError::Resource("fragment count exceeds usize".to_owned()))?;
    if expected_records == 0 {
        if decoded.is_empty() {
            return Ok(Vec::new());
        }
        return Err(ValidationError::Integrity(format!(
            "assembler-input FASTQ {relative:?} is nonempty for a zero-fragment dataset"
        )));
    }
    if !decoded.ends_with(b"\n") || decoded.contains(&b'\r') || decoded.contains(&0) {
        return Err(ValidationError::Integrity(format!(
            "assembler-input FASTQ {relative:?} must use LF records with a final newline and no NUL"
        )));
    }
    let expected_lines = expected_records
        .checked_mul(4)
        .ok_or_else(|| ValidationError::Resource("FASTQ line count overflow".to_owned()))?;
    if decoded.iter().filter(|byte| **byte == b'\n').count() != expected_lines {
        return Err(ValidationError::Integrity(format!(
            "assembler-input FASTQ {relative:?} record count differs from fragments_decimal"
        )));
    }
    let mut records = Vec::new();
    records
        .try_reserve_exact(expected_records)
        .map_err(|error| {
            ValidationError::Resource(format!("cannot allocate FASTQ validation records: {error}"))
        })?;
    let mut lines = decoded.split(|byte| *byte == b'\n');
    for ordinal in 0..expected_records {
        let record = [
            lines.next().expect("validated FASTQ line count"),
            lines.next().expect("validated FASTQ line count"),
            lines.next().expect("validated FASTQ line count"),
            lines.next().expect("validated FASTQ line count"),
        ];
        let id_bytes = record[0].strip_prefix(b"@").ok_or_else(|| {
            ValidationError::Integrity(format!(
                "assembler-input FASTQ {relative:?} record {} header lacks @",
                ordinal + 1
            ))
        })?;
        let id = std::str::from_utf8(id_bytes).map_err(|error| {
            ValidationError::Integrity(format!(
                "assembler-input FASTQ {relative:?} record {} identifier is not UTF-8: {error}",
                ordinal + 1
            ))
        })?;
        let expected_id = expected_read_id(
            u64::try_from(ordinal)
                .map_err(|_| ValidationError::Resource("FASTQ ordinal exceeds u64".to_owned()))?,
            role,
        );
        if id != expected_id
            || record[2] != b"+"
            || record[1].len() != read_length
            || record[3].len() != read_length
            || !record[1]
                .iter()
                .all(|base| matches!(base, b'A' | b'C' | b'G' | b'T'))
        {
            return Err(ValidationError::Integrity(format!(
                "assembler-input FASTQ {relative:?} record {} violates the generated-read contract",
                ordinal + 1
            )));
        }
        let quality_phred = record[3]
            .iter()
            .map(|byte| byte.checked_sub(33))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                ValidationError::Integrity(format!(
                    "assembler-input FASTQ {relative:?} contains quality below Phred 0"
                ))
            })?;
        records.push((id.to_owned(), record[1].to_vec(), quality_phred));
    }
    if lines.next() != Some(&b""[..]) || lines.next().is_some() {
        return Err(ValidationError::Integrity(format!(
            "assembler-input FASTQ {relative:?} has an invalid final record boundary"
        )));
    }
    Ok(records)
}

fn expected_read_id(fragment_ordinal: u64, role: &str) -> String {
    match role {
        "S" => format!("v4_{fragment_ordinal:012}"),
        "R1" => format!("v4_{fragment_ordinal:012}/1"),
        "R2" => format!("v4_{fragment_ordinal:012}/2"),
        _ => unreachable!("validated read role"),
    }
}

struct TsvCursor<'a> {
    path: &'static str,
    lines: std::str::SplitTerminator<'a, char>,
    columns: usize,
    max_rows: usize,
    rows_seen: usize,
}

impl<'a> TsvCursor<'a> {
    fn new(
        bytes: &'a [u8],
        path: &'static str,
        expected_header: &str,
        columns: usize,
        max_rows: usize,
    ) -> ValidationResult<Self> {
        if !bytes.ends_with(b"\n") || bytes.contains(&b'\r') || bytes.contains(&0) {
            return Err(ValidationError::Integrity(format!(
                "{path} must be UTF-8 TSV with LF endings, a final newline, and no NUL"
            )));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|error| ValidationError::Integrity(format!("{path} is not UTF-8: {error}")))?;
        let mut lines = text.split_terminator('\n');
        if lines.next() != Some(expected_header) {
            return Err(ValidationError::Integrity(format!(
                "{path}:1: header differs from the frozen descriptor"
            )));
        }
        Ok(Self {
            path,
            lines,
            columns,
            max_rows,
            rows_seen: 0,
        })
    }

    fn next<const COLUMNS: usize>(&mut self) -> ValidationResult<Option<[&'a str; COLUMNS]>> {
        if COLUMNS != self.columns {
            return Err(ValidationError::Integrity(format!(
                "{} internal TSV column contract mismatch",
                self.path
            )));
        }
        let Some(line) = self.lines.next() else {
            return Ok(None);
        };
        if self.rows_seen == self.max_rows {
            return Err(ValidationError::Resource(format!(
                "{} row count exceeds semantic cap {}",
                self.path, self.max_rows
            )));
        }
        let line_number = self.rows_seen.saturating_add(2);
        self.rows_seen = self.rows_seen.saturating_add(1);
        if line.bytes().filter(|byte| *byte == b'\t').count() != self.columns.saturating_sub(1) {
            return Err(ValidationError::Integrity(format!(
                "{}:{line_number}: expected {} columns",
                self.path, self.columns
            )));
        }
        let mut fields = [""; COLUMNS];
        let mut observed = 0usize;
        for field in line.split('\t') {
            if observed == COLUMNS {
                return Err(ValidationError::Integrity(format!(
                    "{}:{line_number}: expected {} columns",
                    self.path, self.columns
                )));
            }
            fields[observed] = field;
            observed += 1;
        }
        if observed != COLUMNS {
            return Err(ValidationError::Integrity(format!(
                "{}:{line_number}: expected {} columns, found {observed}",
                self.path, self.columns
            )));
        }
        Ok(Some(fields))
    }

    const fn rows_seen(&self) -> usize {
        self.rows_seen
    }
}

fn ledger_integrity<T>(path: &str, line: usize, message: &str) -> ValidationResult<T> {
    Err(ValidationError::Integrity(format!(
        "{path}:{line}: {message}"
    )))
}

fn ledger_u64(value: &str, path: &str, line: usize, field: &str) -> ValidationResult<u64> {
    decimal_u64(value, field).map_err(|_| {
        ValidationError::Integrity(format!(
            "{path}:{line}: {field} is not canonical unsigned decimal"
        ))
    })
}

fn ledger_usize(value: &str, path: &str, line: usize, field: &str) -> ValidationResult<usize> {
    usize::try_from(ledger_u64(value, path, line, field)?)
        .map_err(|_| ValidationError::Resource(format!("{path}:{line}: {field} exceeds usize")))
}

fn replay_ledger_origin(
    truth: &TruthMolecule,
    first: usize,
    step: i8,
    length: usize,
    path: &str,
    line: usize,
) -> ValidationResult<Vec<u8>> {
    if truth.sequence.is_empty() || first >= truth.sequence.len() || !matches!(step, -1 | 1) {
        return ledger_integrity(path, line, "origin coordinate or step is outside truth");
    }
    let mut sequence = Vec::new();
    sequence.try_reserve_exact(length).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate origin replay: {error}"))
    })?;
    for offset in 0..length {
        let coordinate = ledger_coordinate(truth, first, step, offset).ok_or_else(|| {
            ValidationError::Integrity(format!(
                "{path}:{line}: read traversal exceeds linear truth"
            ))
        })?;
        let base = truth.sequence[coordinate];
        sequence.push(if step == 1 {
            base
        } else {
            match base {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => unreachable!("truth FASTA accepts only ACGT"),
            }
        });
    }
    Ok(sequence)
}

fn ledger_coordinate(
    truth: &TruthMolecule,
    first: usize,
    step: i8,
    offset: usize,
) -> Option<usize> {
    match (truth.topology, step) {
        (TruthTopology::Circular, 1) => first
            .checked_add(offset % truth.sequence.len())
            .map(|value| value % truth.sequence.len()),
        (TruthTopology::Circular, -1) => {
            let retreat = offset % truth.sequence.len();
            Some(if retreat <= first {
                first - retreat
            } else {
                truth.sequence.len() - (retreat - first)
            })
        }
        (TruthTopology::Linear, 1) => first
            .checked_add(offset)
            .filter(|coordinate| *coordinate < truth.sequence.len()),
        (TruthTopology::Linear, -1) => first.checked_sub(offset),
        _ => None,
    }
}

fn validate_fragment_geometry(
    dataset: &DatasetManifest,
    truths: &[TruthMolecule],
    origins: &[LedgerOrigin],
    paired: bool,
) -> ValidationResult<()> {
    let reads_per_fragment = if paired { 2 } else { 1 };
    let mut minor_fragments = 0usize;
    for fragment in origins.chunks_exact(reads_per_fragment) {
        let first = &fragment[0];
        let truth = &truths[first.molecule_index];
        if truth.class == TruthClass::Minor {
            minor_fragments += 1;
        }
        let expected_forward_first = first.outer_fragment_start;
        let expected_reverse_first =
            (first.outer_fragment_start + first.outer_fragment_span - 1) % truth.sequence.len();
        if paired {
            let second = &fragment[1];
            if first.fragment_ordinal != second.fragment_ordinal
                || first.molecule_index != second.molecule_index
                || first.outer_fragment_start != second.outer_fragment_start
                || first.outer_fragment_span != second.outer_fragment_span
                || !matches!((first.truth_step, second.truth_step), (1, -1) | (-1, 1))
                || (first.truth_step == 1
                    && (first.first_truth_base != expected_forward_first
                        || second.first_truth_base != expected_reverse_first))
                || (first.truth_step == -1
                    && (first.first_truth_base != expected_reverse_first
                        || second.first_truth_base != expected_forward_first))
            {
                return Err(ValidationError::Integrity(
                    "paired origin rows contradict inward-FR fragment geometry".to_owned(),
                ));
            }
        } else if !((first.truth_step == 1 && first.first_truth_base == expected_forward_first)
            || (first.truth_step == -1 && first.first_truth_base == expected_reverse_first))
        {
            return Err(ValidationError::Integrity(
                "single-end origin row contradicts outer-fragment geometry".to_owned(),
            ));
        }
    }
    let expected_minor = if dataset.case_id == "mixture-pe" {
        origins.len() / reads_per_fragment / 10
    } else {
        0
    };
    if minor_fragments != expected_minor {
        return Err(ValidationError::Integrity(format!(
            "origin molecule assignments contain {minor_fragments} minor fragments, expected {expected_minor}"
        )));
    }
    Ok(())
}

fn validate_error_and_quality_ledgers(
    dataset: &DatasetManifest,
    root: &DirectoryAnchor,
    truths: &[TruthMolecule],
    reads: &BTreeMap<String, EvidenceRead>,
    origins: &[LedgerOrigin],
    read_length: usize,
) -> ValidationResult<()> {
    let total_read_bases = origins.len().checked_mul(read_length).ok_or_else(|| {
        ValidationError::Resource("truth-ledger read-base count overflow".to_owned())
    })?;
    let error_bytes = read_declared_artifact(dataset, root, ERRORS_PATH, "error ledger")?;
    let quality_bytes =
        read_declared_artifact(dataset, root, QUALITY_EVENTS_PATH, "quality-event ledger")?;
    let mut error_rows = TsvCursor::new(
        &error_bytes,
        ERRORS_PATH,
        ERRORS_HEADER,
        7,
        total_read_bases,
    )?;
    let mut quality_rows = TsvCursor::new(
        &quality_bytes,
        QUALITY_EVENTS_PATH,
        QUALITY_EVENTS_HEADER,
        5,
        total_read_bases,
    )?;
    let mut next_error = error_rows.next::<7>()?;
    let mut next_quality = quality_rows.next::<5>()?;
    let paired = dataset.assembler_input.mode == "paired_end";
    let reads_per_fragment = if paired { 2 } else { 1 };
    let expected_quality_kind = if dataset.case_id == "qc-censoring-control" {
        "qc_censoring_substitution_linked_q10"
    } else {
        "independent_low_quality_q10"
    };
    let mut error_events = 0usize;
    let mut quality_events = 0usize;

    for (rank, origin) in origins.iter().enumerate() {
        let role = if paired {
            if rank % reads_per_fragment == 0 {
                "R1"
            } else {
                "R2"
            }
        } else {
            "S"
        };
        let read_id = expected_read_id(origin.fragment_ordinal, role);
        let read = reads.get(&read_id).ok_or_else(|| {
            ValidationError::Integrity(format!(
                "origin replay names a read absent from assembler-input FASTQ: {read_id:?}"
            ))
        })?;
        let pre_error = replay_ledger_origin(
            &truths[origin.molecule_index],
            origin.first_truth_base,
            origin.truth_step,
            read_length,
            ORIGINS_PATH,
            rank + 2,
        )?;
        for (offset, &pre_error_base) in pre_error.iter().enumerate() {
            let mut error_observed_base = None;
            if let Some(fields) = next_error {
                if fields[1] == read_id {
                    let line = error_rows.rows_seen() + 1;
                    let event_offset =
                        ledger_usize(fields[2], ERRORS_PATH, line, "observed_offset_zero_based")?;
                    if event_offset < offset {
                        return ledger_integrity(
                            ERRORS_PATH,
                            line,
                            "rows are duplicated or not strictly ordered by read emission and offset",
                        );
                    }
                    if event_offset == offset {
                        if fields[0] != "2.0" || fields[4] != "substitution" {
                            return ledger_integrity(
                                ERRORS_PATH,
                                line,
                                "schema_version/error_kind differs from the frozen substitution ledger",
                            );
                        }
                        let declared_coordinate = ledger_usize(
                            fields[3],
                            ERRORS_PATH,
                            line,
                            "truth_coordinate_zero_based",
                        )?;
                        let expected_coordinate = ledger_coordinate(
                            &truths[origin.molecule_index],
                            origin.first_truth_base,
                            origin.truth_step,
                            offset,
                        )
                        .expect("validated origin replay established coordinate");
                        let truth_base =
                            exact_ledger_base(fields[5], ERRORS_PATH, line, "truth_base")?;
                        let observed_base =
                            exact_ledger_base(fields[6], ERRORS_PATH, line, "observed_base")?;
                        if declared_coordinate != expected_coordinate
                            || truth_base != pre_error_base
                            || truth_base == observed_base
                        {
                            return ledger_integrity(
                                ERRORS_PATH,
                                line,
                                "substitution does not reconcile to the declared read origin",
                            );
                        }
                        error_observed_base = Some(observed_base);
                        error_events = error_events.checked_add(1).ok_or_else(|| {
                            ValidationError::Resource("error-event count overflow".to_owned())
                        })?;
                        next_error = error_rows.next::<7>()?;
                    }
                }
            }

            let mut has_quality_event = false;
            if let Some(fields) = next_quality {
                if fields[1] == read_id {
                    let line = quality_rows.rows_seen() + 1;
                    let event_offset = ledger_usize(
                        fields[2],
                        QUALITY_EVENTS_PATH,
                        line,
                        "observed_offset_zero_based",
                    )?;
                    if event_offset < offset {
                        return ledger_integrity(
                            QUALITY_EVENTS_PATH,
                            line,
                            "rows are duplicated or not strictly ordered by read emission and offset",
                        );
                    }
                    if event_offset == offset {
                        if fields[0] != "1.0"
                            || fields[3] != expected_quality_kind
                            || fields[4] != "10"
                        {
                            return ledger_integrity(
                                QUALITY_EVENTS_PATH,
                                line,
                                "quality event differs from the frozen case-specific Q10 ledger",
                            );
                        }
                        has_quality_event = true;
                        quality_events = quality_events.checked_add(1).ok_or_else(|| {
                            ValidationError::Resource("quality-event count overflow".to_owned())
                        })?;
                        next_quality = quality_rows.next::<5>()?;
                    }
                }
            }

            if dataset.case_id == "qc-censoring-control"
                && error_observed_base.is_some() != has_quality_event
            {
                return Err(ValidationError::Integrity(format!(
                    "qc-censoring-control quality-event offsets differ from substitution offsets at read {read_id:?}, offset {offset}"
                )));
            }
            let expected_base = error_observed_base.unwrap_or(pre_error_base);
            if read.sequence[offset] != expected_base {
                return Err(ValidationError::Integrity(format!(
                    "error ledger does not reproduce observed sequence for read {read_id:?}"
                )));
            }
            let expected_quality = if has_quality_event { 10 } else { 40 };
            if read.quality_phred[offset] != expected_quality {
                return Err(ValidationError::Integrity(format!(
                    "quality-event ledger does not reproduce observed qualities for read {read_id:?}"
                )));
            }
        }
        if next_error.is_some_and(|fields| fields[1] == read_id) {
            return ledger_integrity(
                ERRORS_PATH,
                error_rows.rows_seen() + 1,
                "observed offset exceeds read length",
            );
        }
        if next_quality.is_some_and(|fields| fields[1] == read_id) {
            return ledger_integrity(
                QUALITY_EVENTS_PATH,
                quality_rows.rows_seen() + 1,
                "observed offset exceeds read length",
            );
        }
    }
    if next_error.is_some() {
        return ledger_integrity(
            ERRORS_PATH,
            error_rows.rows_seen() + 1,
            "read_id is absent, duplicated, or not ordered by origins.tsv emission order",
        );
    }
    if next_quality.is_some() {
        return ledger_integrity(
            QUALITY_EVENTS_PATH,
            quality_rows.rows_seen() + 1,
            "read_id is absent, duplicated, or not ordered by origins.tsv emission order",
        );
    }
    if error_rows.next::<7>()?.is_some() || quality_rows.next::<5>()?.is_some() {
        return Err(ValidationError::Integrity(
            "event ledger cursor did not terminate at the authenticated snapshot boundary"
                .to_owned(),
        ));
    }
    let substitution_rate = decimal_u64(
        &dataset.generator.parameters.substitution_rate_ppm_decimal,
        "generator substitution_rate_ppm_decimal",
    )?;
    if (substitution_rate == 0 && error_events != 0)
        || (substitution_rate == ERROR_RATE_SCALE && error_events != total_read_bases)
    {
        return Err(ValidationError::Integrity(
            "error-ledger cardinality contradicts a deterministic zero or one-million-ppm substitution rate"
                .to_owned(),
        ));
    }
    let low_quality_rate = decimal_u64(
        &dataset.generator.parameters.low_quality_rate_ppm_decimal,
        "generator low_quality_rate_ppm_decimal",
    )?;
    if dataset.case_id != "qc-censoring-control"
        && ((low_quality_rate == 0 && quality_events != 0)
            || (low_quality_rate == ERROR_RATE_SCALE && quality_events != total_read_bases))
    {
        return Err(ValidationError::Integrity(
            "quality-event cardinality contradicts a deterministic zero or one-million-ppm low-quality rate"
                .to_owned(),
        ));
    }
    Ok(())
}

fn exact_ledger_base(value: &str, path: &str, line: usize, field: &str) -> ValidationResult<u8> {
    let bytes = value.as_bytes();
    if bytes.len() == 1 && matches!(bytes[0], b'A' | b'C' | b'G' | b'T') {
        Ok(bytes[0])
    } else {
        ledger_integrity(
            path,
            line,
            &format!("{field} must be exactly one A/C/G/T base"),
        )
    }
}

fn validate_truth_record(
    manifest: &TruthMoleculeRecord,
    fasta: &FastaRecord,
) -> ValidationResult<()> {
    let length = decimal_u64(&manifest.length_bases_decimal, "truth molecule length")?;
    if length
        != u64::try_from(fasta.sequence.len()).map_err(|_| {
            ValidationError::Resource("truth sequence length exceeds u64".to_owned())
        })?
    {
        return Err(ValidationError::Integrity(format!(
            "truth length mismatch for {:?}",
            fasta.id
        )));
    }
    if sha256_bytes(&fasta.sequence) != manifest.sequence_sha256 {
        return Err(ValidationError::Integrity(format!(
            "truth sequence SHA-256 mismatch for {:?}",
            fasta.id
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct AlignmentDimensions {
    reference_length: usize,
    rows: usize,
    columns: usize,
    cells: usize,
    cells_u64: u64,
}

fn alignment_dimensions(
    query_length: usize,
    truth_length: usize,
    topology: TruthTopology,
) -> ValidationResult<AlignmentDimensions> {
    let reference_length = if topology == TruthTopology::Circular {
        truth_length
            .checked_add(query_length.saturating_sub(1))
            .ok_or_else(|| {
                ValidationError::Resource("circular alignment reference length overflow".to_owned())
            })?
    } else {
        truth_length
    };
    let rows = query_length
        .checked_add(1)
        .ok_or_else(|| ValidationError::Resource("alignment row count overflow".to_owned()))?;
    let columns = reference_length
        .checked_add(1)
        .ok_or_else(|| ValidationError::Resource("alignment column count overflow".to_owned()))?;
    let cells = rows
        .checked_mul(columns)
        .ok_or_else(|| ValidationError::Resource("alignment DP cell count overflow".to_owned()))?;
    let cells_u64 = u64::try_from(cells)
        .map_err(|_| ValidationError::Resource("alignment DP cells exceed u64".to_owned()))?;
    Ok(AlignmentDimensions {
        reference_length,
        rows,
        columns,
        cells,
        cells_u64,
    })
}

fn preflight_evaluation_work(
    contigs: &[FastaRecord],
    truths: &[TruthMolecule],
    max_dp_cells: u64,
    max_exact_scan_bases: u64,
    limits: EvaluationWorkLimits,
) -> ValidationResult<AlignmentPreflight> {
    if contigs.len() > limits.max_retained_alignment_rows {
        return Err(ValidationError::Resource(format!(
            "alignment row count {} exceeds fixed cap {}",
            contigs.len(),
            limits.max_retained_alignment_rows
        )));
    }

    let mut plans = Vec::new();
    plans.try_reserve_exact(contigs.len()).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate alignment plans: {error}"))
    })?;
    let mut total_cells = 0u64;
    let mut exact_scan_bases = 0u64;
    for contig in contigs {
        let exact = exact_alignment_plan(
            &contig.sequence,
            truths,
            &mut exact_scan_bases,
            max_exact_scan_bases,
        )?;
        if exact.is_some() {
            plans.push(exact);
            continue;
        }
        for truth in truths {
            let cells =
                alignment_dimensions(contig.sequence.len(), truth.sequence.len(), truth.topology)?
                    .cells_u64;
            if cells > max_dp_cells {
                return Err(ValidationError::Resource(format!(
                    "alignment requires {cells} DP cells, above max {max_dp_cells}"
                )));
            }
            let both_strands = cells.checked_mul(2).ok_or_else(|| {
                ValidationError::Resource("alignment DP work overflow".to_owned())
            })?;
            total_cells = total_cells.checked_add(both_strands).ok_or_else(|| {
                ValidationError::Resource("aggregate alignment DP work overflow".to_owned())
            })?;
            if total_cells > limits.max_total_alignment_dp_cells {
                return Err(ValidationError::Resource(format!(
                    "aggregate alignment DP work {total_cells} cells exceeds fixed cap {}",
                    limits.max_total_alignment_dp_cells
                )));
            }
        }
        plans.push(None);
    }
    Ok(AlignmentPreflight {
        plans,
        exact_scan_bases,
    })
}

fn exact_alignment_plan(
    query: &[u8],
    truths: &[TruthMolecule],
    scan_bases: &mut u64,
    max_scan_bases: u64,
) -> ValidationResult<Option<ExactAlignmentPlan>> {
    let prefix_bytes = u64::try_from(query.len())
        .map_err(|_| {
            ValidationError::Resource("exact-alignment query length exceeds u64".to_owned())
        })?
        .checked_mul(u64::try_from(std::mem::size_of::<usize>()).expect("usize size fits u64"))
        .ok_or_else(|| {
            ValidationError::Resource("exact-alignment prefix bytes overflow".to_owned())
        })?;
    if prefix_bytes > MAX_EXACT_ALIGNMENT_PREFIX_BYTES {
        return Ok(None);
    }
    let prefix = kmp_prefix(query)?;
    let mut best = None;
    let mut equally_best_targets = 0usize;
    for (molecule_index, truth) in truths.iter().enumerate() {
        for strand in [Strand::Forward, Strand::Reverse] {
            if let Some(start) =
                exact_oriented_start(query, &prefix, truth, strand, scan_bases, max_scan_bases)?
            {
                equally_best_targets = equally_best_targets.checked_add(1).ok_or_else(|| {
                    ValidationError::Resource("exact-alignment target count overflow".to_owned())
                })?;
                if best.is_none() {
                    best = Some((molecule_index, strand, start));
                }
            }
        }
    }
    Ok(best.map(
        |(molecule_index, strand, oriented_reference_start)| ExactAlignmentPlan {
            molecule_index,
            strand,
            oriented_reference_start,
            equally_best_targets,
        },
    ))
}

fn kmp_prefix(query: &[u8]) -> ValidationResult<Vec<usize>> {
    let mut prefix = Vec::new();
    prefix.try_reserve_exact(query.len()).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate exact-alignment prefix: {error}"))
    })?;
    exact_alignment_prefix_capacity_bytes(prefix.capacity())?;
    for index in 0..query.len() {
        let mut border = if index == 0 { 0 } else { prefix[index - 1] };
        while border > 0 && query[index] != query[border] {
            border = prefix[border - 1];
        }
        if index > 0 && query[index] == query[border] {
            border += 1;
        }
        prefix.push(border);
    }
    Ok(prefix)
}

fn exact_alignment_prefix_capacity_bytes(capacity: usize) -> ValidationResult<u64> {
    let bytes = u64::try_from(capacity)
        .map_err(|_| {
            ValidationError::Resource("exact-alignment prefix capacity exceeds u64".to_owned())
        })?
        .checked_mul(u64::try_from(std::mem::size_of::<usize>()).expect("type size fits u64"))
        .ok_or_else(|| {
            ValidationError::Resource("exact-alignment prefix capacity bytes overflow".to_owned())
        })?;
    if bytes > MAX_EXACT_ALIGNMENT_PREFIX_BYTES {
        return Err(ValidationError::Resource(format!(
            "exact-alignment prefix post-reservation capacity requires {bytes} bytes, above fixed cap {MAX_EXACT_ALIGNMENT_PREFIX_BYTES}"
        )));
    }
    Ok(bytes)
}

fn exact_oriented_start(
    query: &[u8],
    prefix: &[usize],
    truth: &TruthMolecule,
    strand: Strand,
    scan_bases: &mut u64,
    max_scan_bases: u64,
) -> ValidationResult<Option<usize>> {
    if truth.topology == TruthTopology::Linear && query.len() > truth.sequence.len() {
        return Ok(None);
    }
    let search_length = if truth.topology == TruthTopology::Circular {
        truth
            .sequence
            .len()
            .checked_add(query.len().saturating_sub(1))
            .ok_or_else(|| {
                ValidationError::Resource(
                    "circular exact-alignment search length overflow".to_owned(),
                )
            })?
    } else {
        truth.sequence.len()
    };
    let mut matched = 0usize;
    for position in 0..search_length {
        *scan_bases = checked_increment(*scan_bases, "exact-alignment scan-base count overflow")?;
        if *scan_bases > max_scan_bases {
            return Err(ValidationError::Resource(format!(
                "exact-alignment scan bases exceed max {max_scan_bases}"
            )));
        }
        let base = oriented_base(&truth.sequence, position % truth.sequence.len(), strand);
        while matched > 0 && base != query[matched] {
            matched = prefix[matched - 1];
        }
        if base == query[matched] {
            matched += 1;
        }
        if matched == query.len() {
            let start = position + 1 - query.len();
            if truth.topology == TruthTopology::Linear || start < truth.sequence.len() {
                return Ok(Some(start));
            }
            matched = prefix[matched - 1];
        }
    }
    Ok(None)
}

fn evaluate_compatible_recovery(
    assemblies: &[FastaRecord],
    rows: &[AlignmentRow],
    truths: &[TruthMolecule],
    scan_bases: &mut u64,
    max_scan_bases: u64,
) -> ValidationResult<RecoveryCoverage> {
    let (recovery, work) = evaluate_compatible_recovery_with_work(
        assemblies,
        rows,
        truths,
        scan_bases,
        max_scan_bases,
    )?;
    let _bounded_linear_work = (work.placement_visits, work.coordinate_mask_operations);
    Ok(recovery)
}

fn evaluate_compatible_recovery_with_work(
    assemblies: &[FastaRecord],
    rows: &[AlignmentRow],
    truths: &[TruthMolecule],
    scan_bases: &mut u64,
    max_scan_bases: u64,
) -> ValidationResult<(RecoveryCoverage, CompatibleRecoveryWork)> {
    // Exact placements are stack-resident interval pairs. Monotone unions and
    // interval intersections avoid materializing query coordinates per hit;
    // the work object makes the remaining linear mask touches falsifiable.
    if assemblies.len() != rows.len() {
        return Err(ValidationError::Integrity(
            "assembly and alignment row counts differ during recovery evaluation".to_owned(),
        ));
    }
    let mut molecules = Vec::new();
    molecules.try_reserve_exact(truths.len()).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate recovery masks: {error}"))
    })?;
    for truth in truths {
        molecules.push(MoleculeCoverageMasks {
            unique: try_false_mask(truth.sequence.len(), "unique-coordinate recovery mask")?,
            lower: try_false_mask(truth.sequence.len(), "compatible lower-bound recovery mask")?,
            upper: try_false_mask(truth.sequence.len(), "compatible upper-bound recovery mask")?,
        });
    }
    let mut exact_unambiguous_records = 0u64;
    let mut exact_ambiguous_records = 0u64;
    let mut non_exact_records = 0u64;
    let mut work = CompatibleRecoveryWork::default();
    for (assembly, row) in assemblies.iter().zip(rows) {
        let Some(candidate) = &row.candidate else {
            continue;
        };
        if candidate.edits != 0
            || candidate.mismatches != 0
            || candidate.insertions != 0
            || candidate.deletions != 0
        {
            non_exact_records = checked_increment(
                non_exact_records,
                "non-exact recovery alignment count overflow",
            )?;
            continue;
        }

        let prefix = kmp_prefix(&assembly.sequence)?;
        let mut first_coordinate_set: Option<(usize, PlacementCoordinateSet)> = None;
        let mut lower_state = LowerCoordinateState::Unseen;
        let mut has_distinct_placement = false;
        for (molecule_index, truth) in truths.iter().enumerate() {
            for strand in [Strand::Forward, Strand::Reverse] {
                let mut upper_union = MonotoneIntervalUnion::default();
                {
                    let upper = &mut molecules[molecule_index].upper;
                    for_each_exact_oriented_start(
                        &assembly.sequence,
                        &prefix,
                        truth,
                        strand,
                        scan_bases,
                        max_scan_bases,
                        |start| {
                            work.placement_visits = checked_increment(
                                work.placement_visits,
                                "exact compatible placement count overflow",
                            )?;
                            let coordinate_set = placement_coordinate_set(
                                start,
                                assembly.sequence.len(),
                                truth,
                                strand,
                            )?;
                            let interval_end =
                                start.checked_add(assembly.sequence.len()).ok_or_else(|| {
                                    ValidationError::Resource(
                                        "exact compatible interval overflow".to_owned(),
                                    )
                                })?;
                            upper_union.push_and_mark(
                                CoordinateInterval {
                                    start,
                                    end: interval_end,
                                },
                                upper,
                                truth.topology,
                                strand,
                                true,
                                &mut work,
                            )?;

                            match first_coordinate_set {
                                None => {
                                    first_coordinate_set = Some((molecule_index, coordinate_set));
                                }
                                Some((first_molecule, first_set)) => {
                                    if first_molecule != molecule_index
                                        || first_set != coordinate_set
                                    {
                                        has_distinct_placement = true;
                                    }
                                }
                            }
                            observe_lower_coordinate_placement(
                                &mut lower_state,
                                CompatiblePlacement {
                                    molecule_index,
                                    coordinate_set,
                                    oriented_start: start,
                                    query_length: assembly.sequence.len(),
                                    truth,
                                    strand,
                                },
                                &mut work,
                            )
                        },
                    )?;
                    upper_union.finish(upper, truth.topology, strand, true, &mut work)?;
                }
                finish_lower_coordinate_scan(
                    &mut lower_state,
                    molecule_index,
                    truth,
                    strand,
                    &mut work,
                )?;
            }
        }
        let Some((first_molecule, first_coordinates)) = first_coordinate_set else {
            return Err(ValidationError::Integrity(
                "zero-edit accepted alignment has no exact compatible coordinate placement"
                    .to_owned(),
            ));
        };
        if has_distinct_placement {
            exact_ambiguous_records = checked_increment(
                exact_ambiguous_records,
                "ambiguous exact recovery alignment count overflow",
            )?;
        } else {
            exact_unambiguous_records = checked_increment(
                exact_unambiguous_records,
                "unambiguous exact recovery alignment count overflow",
            )?;
            mark_coordinate_set(
                &mut molecules[first_molecule].unique,
                first_coordinates,
                true,
                &mut work,
            )?;
        }
        apply_lower_coordinate_candidate(lower_state, &mut molecules, &mut work)?;
    }
    Ok((
        RecoveryCoverage {
            molecules,
            exact_unambiguous_records,
            exact_ambiguous_records,
            non_exact_records,
        },
        work,
    ))
}

impl MonotoneIntervalUnion {
    fn push_and_mark(
        &mut self,
        interval: CoordinateInterval,
        mask: &mut [bool],
        topology: TruthTopology,
        strand: Strand,
        value: bool,
        work: &mut CompatibleRecoveryWork,
    ) -> ValidationResult<()> {
        if interval.start >= interval.end {
            return Err(ValidationError::Integrity(
                "exact compatible interval is empty or reversed".to_owned(),
            ));
        }
        let Some(current) = self.current else {
            self.current = Some(interval);
            return Ok(());
        };
        if interval.start < current.start {
            return Err(ValidationError::Integrity(
                "exact compatible placement starts are not monotone".to_owned(),
            ));
        }
        if interval.start <= current.end {
            self.current = Some(CoordinateInterval {
                start: current.start,
                end: current.end.max(interval.end),
            });
            return Ok(());
        }
        mark_oriented_interval(mask, current, topology, strand, value, work)?;
        self.current = Some(interval);
        Ok(())
    }

    fn finish(
        &mut self,
        mask: &mut [bool],
        topology: TruthTopology,
        strand: Strand,
        value: bool,
        work: &mut CompatibleRecoveryWork,
    ) -> ValidationResult<()> {
        if let Some(interval) = self.current.take() {
            mark_oriented_interval(mask, interval, topology, strand, value, work)?;
        }
        Ok(())
    }
}

fn placement_coordinate_set(
    oriented_start: usize,
    query_length: usize,
    truth: &TruthMolecule,
    strand: Strand,
) -> ValidationResult<PlacementCoordinateSet> {
    let truth_length = truth.sequence.len();
    if truth_length == 0 || query_length == 0 {
        return Err(ValidationError::Integrity(
            "exact compatible placement uses an empty truth or query".to_owned(),
        ));
    }
    if truth.topology == TruthTopology::Linear {
        let oriented_end = oriented_start.checked_add(query_length).ok_or_else(|| {
            ValidationError::Resource("linear compatible-coordinate range overflow".to_owned())
        })?;
        if oriented_end > truth_length {
            return Err(ValidationError::Integrity(
                "linear exact compatible placement exceeds its truth molecule".to_owned(),
            ));
        }
        return Ok(single_coordinate_interval(oriented_interval_to_forward(
            CoordinateInterval {
                start: oriented_start,
                end: oriented_end,
            },
            truth_length,
            strand,
        )?));
    }

    let covered = query_length.min(truth_length);
    if covered == truth_length {
        return Ok(single_coordinate_interval(CoordinateInterval {
            start: 0,
            end: truth_length,
        }));
    }
    let start = oriented_start % truth_length;
    let end = start.checked_add(covered).ok_or_else(|| {
        ValidationError::Resource("circular compatible-coordinate range overflow".to_owned())
    })?;
    if end <= truth_length {
        Ok(single_coordinate_interval(oriented_interval_to_forward(
            CoordinateInterval { start, end },
            truth_length,
            strand,
        )?))
    } else {
        coordinate_interval_pair(
            oriented_interval_to_forward(
                CoordinateInterval {
                    start,
                    end: truth_length,
                },
                truth_length,
                strand,
            )?,
            oriented_interval_to_forward(
                CoordinateInterval {
                    start: 0,
                    end: end - truth_length,
                },
                truth_length,
                strand,
            )?,
        )
    }
}

fn single_coordinate_interval(interval: CoordinateInterval) -> PlacementCoordinateSet {
    PlacementCoordinateSet {
        intervals: [interval, CoordinateInterval { start: 0, end: 0 }],
        len: 1,
    }
}

fn coordinate_interval_pair(
    mut first: CoordinateInterval,
    mut second: CoordinateInterval,
) -> ValidationResult<PlacementCoordinateSet> {
    if first.start >= first.end || second.start >= second.end {
        return Err(ValidationError::Integrity(
            "circular compatible-coordinate interval is empty".to_owned(),
        ));
    }
    if first.start > second.start {
        std::mem::swap(&mut first, &mut second);
    }
    if first.end >= second.start {
        Ok(single_coordinate_interval(CoordinateInterval {
            start: first.start,
            end: first.end.max(second.end),
        }))
    } else {
        Ok(PlacementCoordinateSet {
            intervals: [first, second],
            len: 2,
        })
    }
}

fn oriented_interval_to_forward(
    interval: CoordinateInterval,
    truth_length: usize,
    strand: Strand,
) -> ValidationResult<CoordinateInterval> {
    if interval.start >= interval.end || interval.end > truth_length {
        return Err(ValidationError::Integrity(
            "oriented compatible-coordinate interval is outside its truth molecule".to_owned(),
        ));
    }
    Ok(match strand {
        Strand::Forward => interval,
        Strand::Reverse => CoordinateInterval {
            start: truth_length - interval.end,
            end: truth_length - interval.start,
        },
    })
}

fn observe_lower_coordinate_placement(
    state: &mut LowerCoordinateState,
    placement: CompatiblePlacement<'_>,
    work: &mut CompatibleRecoveryWork,
) -> ValidationResult<()> {
    let CompatiblePlacement {
        molecule_index,
        coordinate_set,
        oriented_start,
        query_length,
        truth,
        strand,
    } = placement;
    if matches!(state, LowerCoordinateState::Unseen) {
        let coverage = match truth.topology {
            TruthTopology::Linear => {
                LowerCoordinateCoverage::Linear(sole_coordinate_interval(coordinate_set)?)
            }
            TruthTopology::Circular => {
                let compatible =
                    try_true_mask(truth.sequence.len(), "circular compatible lower-bound mask")?;
                add_coordinate_mask_operations(
                    work,
                    truth.sequence.len(),
                    "circular compatible-mask initialization work overflow",
                )?;
                LowerCoordinateCoverage::Circular {
                    compatible,
                    complement_union: MonotoneIntervalUnion::default(),
                }
            }
        };
        *state = LowerCoordinateState::Candidate(LowerCoordinateCandidate {
            molecule_index,
            coverage,
        });
    }

    let LowerCoordinateState::Candidate(candidate) = state else {
        return Ok(());
    };
    if candidate.molecule_index != molecule_index {
        *state = LowerCoordinateState::Impossible;
        return Ok(());
    }
    match &mut candidate.coverage {
        LowerCoordinateCoverage::Linear(intersection) => {
            let observed = sole_coordinate_interval(coordinate_set)?;
            intersection.start = intersection.start.max(observed.start);
            intersection.end = intersection.end.min(observed.end);
            if intersection.start > intersection.end {
                intersection.start = intersection.end;
            }
        }
        LowerCoordinateCoverage::Circular {
            compatible,
            complement_union,
        } => {
            let truth_length = truth.sequence.len();
            let covered = query_length.min(truth_length);
            if covered < truth_length {
                let complement_start = oriented_start.checked_add(covered).ok_or_else(|| {
                    ValidationError::Resource(
                        "circular compatible complement start overflow".to_owned(),
                    )
                })?;
                let complement_end = oriented_start.checked_add(truth_length).ok_or_else(|| {
                    ValidationError::Resource(
                        "circular compatible complement end overflow".to_owned(),
                    )
                })?;
                complement_union.push_and_mark(
                    CoordinateInterval {
                        start: complement_start,
                        end: complement_end,
                    },
                    compatible,
                    TruthTopology::Circular,
                    strand,
                    false,
                    work,
                )?;
            }
        }
    }
    Ok(())
}

fn finish_lower_coordinate_scan(
    state: &mut LowerCoordinateState,
    molecule_index: usize,
    truth: &TruthMolecule,
    strand: Strand,
    work: &mut CompatibleRecoveryWork,
) -> ValidationResult<()> {
    let LowerCoordinateState::Candidate(candidate) = state else {
        return Ok(());
    };
    if candidate.molecule_index != molecule_index {
        return Ok(());
    }
    if let LowerCoordinateCoverage::Circular {
        compatible,
        complement_union,
    } = &mut candidate.coverage
    {
        complement_union.finish(compatible, truth.topology, strand, false, work)?;
    }
    Ok(())
}

fn apply_lower_coordinate_candidate(
    state: LowerCoordinateState,
    molecules: &mut [MoleculeCoverageMasks],
    work: &mut CompatibleRecoveryWork,
) -> ValidationResult<()> {
    let LowerCoordinateState::Candidate(candidate) = state else {
        return Ok(());
    };
    let lower = &mut molecules
        .get_mut(candidate.molecule_index)
        .ok_or_else(|| {
            ValidationError::Integrity(
                "compatible lower-bound candidate names a missing truth molecule".to_owned(),
            )
        })?
        .lower;
    match candidate.coverage {
        LowerCoordinateCoverage::Linear(interval) => {
            mark_forward_interval(lower, interval, true, work)?;
        }
        LowerCoordinateCoverage::Circular { compatible, .. } => {
            if compatible.len() != lower.len() {
                return Err(ValidationError::Integrity(
                    "circular compatible lower-bound mask has the wrong length".to_owned(),
                ));
            }
            add_coordinate_mask_operations(
                work,
                compatible.len(),
                "circular compatible-mask merge work overflow",
            )?;
            for (coordinate, covered) in compatible.into_iter().enumerate() {
                if covered {
                    lower[coordinate] = true;
                }
            }
        }
    }
    Ok(())
}

fn sole_coordinate_interval(
    coordinate_set: PlacementCoordinateSet,
) -> ValidationResult<CoordinateInterval> {
    if coordinate_set.len != 1 {
        return Err(ValidationError::Integrity(
            "linear compatible placement does not have one coordinate interval".to_owned(),
        ));
    }
    Ok(coordinate_set.intervals[0])
}

fn mark_coordinate_set(
    mask: &mut [bool],
    coordinate_set: PlacementCoordinateSet,
    value: bool,
    work: &mut CompatibleRecoveryWork,
) -> ValidationResult<()> {
    let interval_count = usize::from(coordinate_set.len);
    if interval_count == 0 || interval_count > coordinate_set.intervals.len() {
        return Err(ValidationError::Integrity(
            "compatible placement has an invalid coordinate-interval count".to_owned(),
        ));
    }
    for interval in coordinate_set.intervals.into_iter().take(interval_count) {
        mark_forward_interval(mask, interval, value, work)?;
    }
    Ok(())
}

fn mark_oriented_interval(
    mask: &mut [bool],
    interval: CoordinateInterval,
    topology: TruthTopology,
    strand: Strand,
    value: bool,
    work: &mut CompatibleRecoveryWork,
) -> ValidationResult<()> {
    let truth_length = mask.len();
    if truth_length == 0 || interval.start >= interval.end {
        return Err(ValidationError::Integrity(
            "compatible coverage interval uses an empty truth or range".to_owned(),
        ));
    }
    if topology == TruthTopology::Linear {
        let forward = oriented_interval_to_forward(interval, truth_length, strand)?;
        return mark_forward_interval(mask, forward, value, work);
    }

    let length = interval.end.checked_sub(interval.start).ok_or_else(|| {
        ValidationError::Resource("circular compatible interval length underflow".to_owned())
    })?;
    if length >= truth_length {
        return mark_forward_interval(
            mask,
            CoordinateInterval {
                start: 0,
                end: truth_length,
            },
            value,
            work,
        );
    }
    let start = interval.start % truth_length;
    let end = start.checked_add(length).ok_or_else(|| {
        ValidationError::Resource("circular compatible interval projection overflow".to_owned())
    })?;
    if end <= truth_length {
        let forward =
            oriented_interval_to_forward(CoordinateInterval { start, end }, truth_length, strand)?;
        mark_forward_interval(mask, forward, value, work)
    } else {
        let first = oriented_interval_to_forward(
            CoordinateInterval {
                start,
                end: truth_length,
            },
            truth_length,
            strand,
        )?;
        let second = oriented_interval_to_forward(
            CoordinateInterval {
                start: 0,
                end: end - truth_length,
            },
            truth_length,
            strand,
        )?;
        mark_forward_interval(mask, first, value, work)?;
        mark_forward_interval(mask, second, value, work)
    }
}

fn mark_forward_interval(
    mask: &mut [bool],
    interval: CoordinateInterval,
    value: bool,
    work: &mut CompatibleRecoveryWork,
) -> ValidationResult<()> {
    if interval.start > interval.end || interval.end > mask.len() {
        return Err(ValidationError::Integrity(
            "forward compatible-coordinate interval is outside its mask".to_owned(),
        ));
    }
    let length = interval.end - interval.start;
    add_coordinate_mask_operations(work, length, "compatible-coordinate mask work overflow")?;
    mask[interval.start..interval.end].fill(value);
    Ok(())
}

fn add_coordinate_mask_operations(
    work: &mut CompatibleRecoveryWork,
    operations: usize,
    context: &str,
) -> ValidationResult<()> {
    work.coordinate_mask_operations = work
        .coordinate_mask_operations
        .checked_add(
            u64::try_from(operations).map_err(|_| ValidationError::Resource(context.to_owned()))?,
        )
        .ok_or_else(|| ValidationError::Resource(context.to_owned()))?;
    Ok(())
}

fn try_true_mask(length: usize, label: &str) -> ValidationResult<Vec<bool>> {
    let mut mask = Vec::new();
    mask.try_reserve_exact(length)
        .map_err(|error| ValidationError::Resource(format!("cannot allocate {label}: {error}")))?;
    mask.resize(length, true);
    Ok(mask)
}

fn try_false_mask(length: usize, label: &str) -> ValidationResult<Vec<bool>> {
    let mut mask = Vec::new();
    mask.try_reserve_exact(length)
        .map_err(|error| ValidationError::Resource(format!("cannot allocate {label}: {error}")))?;
    mask.resize(length, false);
    Ok(mask)
}

fn for_each_exact_oriented_start<F>(
    query: &[u8],
    prefix: &[usize],
    truth: &TruthMolecule,
    strand: Strand,
    scan_bases: &mut u64,
    max_scan_bases: u64,
    mut visit: F,
) -> ValidationResult<()>
where
    F: FnMut(usize) -> ValidationResult<()>,
{
    if truth.topology == TruthTopology::Linear && query.len() > truth.sequence.len() {
        return Ok(());
    }
    let search_length = if truth.topology == TruthTopology::Circular {
        truth
            .sequence
            .len()
            .checked_add(query.len().saturating_sub(1))
            .ok_or_else(|| {
                ValidationError::Resource(
                    "circular compatible-placement search length overflow".to_owned(),
                )
            })?
    } else {
        truth.sequence.len()
    };
    let mut matched = 0usize;
    for position in 0..search_length {
        *scan_bases =
            checked_increment(*scan_bases, "compatible-placement scan-base count overflow")?;
        if *scan_bases > max_scan_bases {
            return Err(ValidationError::Resource(format!(
                "exact-alignment and compatible-placement scan bases exceed max {max_scan_bases}"
            )));
        }
        let base = oriented_base(&truth.sequence, position % truth.sequence.len(), strand);
        while matched > 0 && base != query[matched] {
            matched = prefix[matched - 1];
        }
        if base == query[matched] {
            matched += 1;
        }
        if matched == query.len() {
            let start = position + 1 - query.len();
            if truth.topology == TruthTopology::Linear || start < truth.sequence.len() {
                visit(start)?;
            }
            matched = prefix[matched - 1];
        }
    }
    Ok(())
}

fn align_contig(
    contig: &FastaRecord,
    truths: &[TruthMolecule],
    max_edit_rate_ppm: u32,
    max_dp_cells: u64,
    exact_plan: Option<ExactAlignmentPlan>,
) -> ValidationResult<AlignmentRow> {
    if let Some(plan) = exact_plan {
        let mut operations = Vec::new();
        operations
            .try_reserve_exact(contig.sequence.len())
            .map_err(|error| {
                ValidationError::Resource(format!(
                    "cannot allocate exact-alignment operations: {error}"
                ))
            })?;
        operations.resize(contig.sequence.len(), AlignmentOp::Match);
        let matches = u64::try_from(contig.sequence.len()).map_err(|_| {
            ValidationError::Resource("exact-alignment match count exceeds u64".to_owned())
        })?;
        return Ok(AlignmentRow {
            contig_id: contig.id.clone(),
            contig_length: contig.sequence.len(),
            status: "accepted_primary_alignment",
            candidate: Some(AlignmentCandidate {
                molecule_index: plan.molecule_index,
                strand: plan.strand,
                oriented_reference_start: plan.oriented_reference_start,
                operations,
                edits: 0,
                matches,
                mismatches: 0,
                insertions: 0,
                deletions: 0,
            }),
            equally_best_targets: plan.equally_best_targets,
        });
    }
    let candidate_count = truths.len().checked_mul(2).ok_or_else(|| {
        ValidationError::Resource("alignment candidate count overflow".to_owned())
    })?;
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(candidate_count)
        .map_err(|error| {
            ValidationError::Resource(format!("cannot allocate alignment candidates: {error}"))
        })?;
    for (molecule_index, truth) in truths.iter().enumerate() {
        candidates.push(fitting_alignment(
            &contig.sequence,
            truth,
            molecule_index,
            Strand::Forward,
            max_dp_cells,
        )?);
        candidates.push(fitting_alignment(
            &contig.sequence,
            truth,
            molecule_index,
            Strand::Reverse,
            max_dp_cells,
        )?);
    }
    candidates.sort_by(compare_candidates);
    let best_signature = candidates
        .first()
        .map(candidate_score_signature)
        .ok_or_else(|| {
            ValidationError::Integrity("evaluator received no truth molecule".to_owned())
        })?;
    let equally_best_targets = candidates
        .iter()
        .filter(|candidate| candidate_score_signature(candidate) == best_signature)
        .count();
    let best = candidates.remove(0);
    let accepted = best.accepted(max_edit_rate_ppm);
    Ok(AlignmentRow {
        contig_id: contig.id.clone(),
        contig_length: contig.sequence.len(),
        status: if accepted {
            "accepted_primary_alignment"
        } else {
            "unaligned_edit_rate_exceeded"
        },
        candidate: accepted.then_some(best),
        equally_best_targets,
    })
}

fn compare_candidates(left: &AlignmentCandidate, right: &AlignmentCandidate) -> Ordering {
    candidate_sort_key(left).cmp(&candidate_sort_key(right))
}

fn candidate_score_signature(candidate: &AlignmentCandidate) -> (u64, std::cmp::Reverse<u64>, u64) {
    (
        candidate.edits,
        std::cmp::Reverse(candidate.matches),
        candidate.insertions + candidate.deletions,
    )
}

fn candidate_sort_key(
    candidate: &AlignmentCandidate,
) -> (u64, std::cmp::Reverse<u64>, u64, usize, u8, usize) {
    (
        candidate.edits,
        std::cmp::Reverse(candidate.matches),
        candidate.insertions + candidate.deletions,
        candidate.molecule_index,
        candidate.strand.rank(),
        candidate.oriented_reference_start,
    )
}

/// Compact rolling-row score for the fitting-alignment dynamic program.
///
/// At one fixed DP cell `(query_prefix, reference_prefix)`, a path's indel
/// count is
///
/// `2 * edits + 2 * matches - query_prefix - (reference_prefix - start)`.
///
/// Consequently, after edit distance and descending matches are tied,
/// minimizing indels is exactly the same as minimizing `start`. Encoding
/// `(edits, query_length - matches, start)` is therefore sufficient inside a
/// cell. Endpoints are at different reference prefixes, so their true indel
/// counts are decoded and compared explicitly before traceback.
#[derive(Debug, Clone, Copy)]
struct AlignmentScoreEncoding {
    start_radix: u64,
    match_radix: u64,
    match_weight: u64,
    edit_weight: u64,
    maximum_key: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AlignmentEndpointScore {
    edits: u64,
    matches: u64,
    indels: u64,
    start: usize,
    end: usize,
}

impl AlignmentEndpointScore {
    fn sort_key(self) -> (u64, std::cmp::Reverse<u64>, u64, usize, usize) {
        (
            self.edits,
            std::cmp::Reverse(self.matches),
            self.indels,
            self.start,
            self.end,
        )
    }
}

impl AlignmentScoreEncoding {
    fn new(
        query_length: usize,
        maximum_start: usize,
        maximum_edits: usize,
    ) -> ValidationResult<Self> {
        let query_length = u64::try_from(query_length).map_err(|_| {
            ValidationError::Resource("alignment query length exceeds u64".to_owned())
        })?;
        let maximum_start = u64::try_from(maximum_start).map_err(|_| {
            ValidationError::Resource("alignment start coordinate exceeds u64".to_owned())
        })?;
        let maximum_edits = u64::try_from(maximum_edits).map_err(|_| {
            ValidationError::Resource("alignment edit bound exceeds u64".to_owned())
        })?;
        let start_radix = maximum_start.checked_add(1).ok_or_else(|| {
            ValidationError::Resource("alignment start radix overflow".to_owned())
        })?;
        let match_radix = query_length.checked_add(1).ok_or_else(|| {
            ValidationError::Resource("alignment match radix overflow".to_owned())
        })?;
        let match_weight = start_radix;
        let edit_weight = match_radix.checked_mul(match_weight).ok_or_else(|| {
            ValidationError::Resource("alignment score weight overflow".to_owned())
        })?;
        let maximum_key = maximum_edits
            .checked_mul(edit_weight)
            .and_then(|value| value.checked_add(query_length.checked_mul(match_weight)?))
            .and_then(|value| value.checked_add(maximum_start))
            .ok_or_else(|| {
                ValidationError::Resource("alignment score encoding overflow".to_owned())
            })?;
        if maximum_key == u64::MAX {
            return Err(ValidationError::Resource(
                "alignment score encoding exhausts its sentinel".to_owned(),
            ));
        }
        Ok(Self {
            start_radix,
            match_radix,
            match_weight,
            edit_weight,
            maximum_key,
        })
    }

    fn initial(self, query_length: usize, start: usize) -> ValidationResult<u64> {
        let query_length = u64::try_from(query_length).map_err(|_| {
            ValidationError::Resource("alignment query length exceeds u64".to_owned())
        })?;
        let start = u64::try_from(start).map_err(|_| {
            ValidationError::Resource("alignment start coordinate exceeds u64".to_owned())
        })?;
        query_length
            .checked_mul(self.match_weight)
            .and_then(|value| value.checked_add(start))
            .filter(|value| *value <= self.maximum_key)
            .ok_or_else(|| ValidationError::Resource("initial alignment score overflow".to_owned()))
    }

    fn add_edit(self, score: u64) -> ValidationResult<u64> {
        if score == u64::MAX {
            return Ok(u64::MAX);
        }
        score
            .checked_add(self.edit_weight)
            .filter(|value| *value <= self.maximum_key)
            .ok_or_else(|| ValidationError::Resource("alignment score overflow".to_owned()))
    }

    fn add_match(self, score: u64) -> ValidationResult<u64> {
        if score == u64::MAX {
            return Ok(u64::MAX);
        }
        score
            .checked_sub(self.match_weight)
            .ok_or_else(|| ValidationError::Integrity("alignment match score underflow".to_owned()))
    }

    fn endpoint(
        self,
        score: u64,
        query_length: usize,
        end: usize,
    ) -> ValidationResult<Option<AlignmentEndpointScore>> {
        if score == u64::MAX {
            return Ok(None);
        }
        let start_u64 = score % self.start_radix;
        let fields = score / self.start_radix;
        let inverse_matches = fields % self.match_radix;
        let edits = fields / self.match_radix;
        let query_length_u64 = u64::try_from(query_length).map_err(|_| {
            ValidationError::Resource("alignment query length exceeds u64".to_owned())
        })?;
        let matches = query_length_u64
            .checked_sub(inverse_matches)
            .ok_or_else(|| {
                ValidationError::Integrity(
                    "decoded alignment matches exceed query length".to_owned(),
                )
            })?;
        let start = usize::try_from(start_u64).map_err(|_| {
            ValidationError::Resource("alignment start coordinate exceeds usize".to_owned())
        })?;
        let reference_bases = end.checked_sub(start).ok_or_else(|| {
            ValidationError::Integrity("alignment endpoint precedes its start".to_owned())
        })?;
        let twice_edits_and_matches = edits
            .checked_add(matches)
            .and_then(|value| value.checked_mul(2))
            .ok_or_else(|| {
                ValidationError::Resource("alignment indel derivation overflow".to_owned())
            })?;
        let consumed = query_length_u64
            .checked_add(u64::try_from(reference_bases).map_err(|_| {
                ValidationError::Resource("alignment reference span exceeds u64".to_owned())
            })?)
            .ok_or_else(|| {
                ValidationError::Resource("alignment consumed-base count overflow".to_owned())
            })?;
        let indels = twice_edits_and_matches
            .checked_sub(consumed)
            .ok_or_else(|| {
                ValidationError::Integrity("decoded alignment indel count underflow".to_owned())
            })?;
        Ok(Some(AlignmentEndpointScore {
            edits,
            matches,
            indels,
            start,
            end,
        }))
    }
}

fn filled_alignment_score_row(length: usize, value: u64) -> ValidationResult<Vec<u64>> {
    let mut row = Vec::new();
    row.try_reserve_exact(length).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate alignment score row: {error}"))
    })?;
    row.resize(length, value);
    Ok(row)
}

fn fitting_alignment(
    query: &[u8],
    truth: &TruthMolecule,
    molecule_index: usize,
    strand: Strand,
    max_dp_cells: u64,
) -> ValidationResult<AlignmentCandidate> {
    let dimensions = alignment_dimensions(query.len(), truth.sequence.len(), truth.topology)?;
    if dimensions.cells_u64 > max_dp_cells {
        return Err(ValidationError::Resource(format!(
            "alignment requires {} DP cells, above max {max_dp_cells}",
            dimensions.cells_u64
        )));
    }
    let oriented = if strand == Strand::Forward {
        truth.sequence.clone()
    } else {
        reverse_complement(&truth.sequence)
    };
    let reference = if truth.topology == TruthTopology::Circular {
        (0..dimensions.reference_length)
            .map(|index| oriented[index % oriented.len()])
            .collect::<Vec<_>>()
    } else {
        oriented
    };
    let max_score = query
        .len()
        .checked_add(reference.len())
        .ok_or_else(|| ValidationError::Resource("alignment score bound overflow".to_owned()))?;
    if max_score >= (u32::MAX / 4) as usize {
        return Err(ValidationError::Resource(
            "alignment dimensions exceed u32 score model".to_owned(),
        ));
    }
    let maximum_start = truth.sequence.len().saturating_sub(1);
    let encoding = AlignmentScoreEncoding::new(query.len(), maximum_start, max_score)?;
    let infinity = u64::MAX;
    let mut trace = Vec::new();
    trace
        .try_reserve_exact(dimensions.cells)
        .map_err(|error| ValidationError::Resource(format!("cannot allocate DP trace: {error}")))?;
    trace.resize(dimensions.cells, 0u8);
    let rows = dimensions.rows;
    let columns = dimensions.columns;
    let mut previous = filled_alignment_score_row(columns, infinity)?;
    let mut current = filled_alignment_score_row(columns, infinity)?;
    for (column, score) in previous.iter_mut().enumerate() {
        if column <= maximum_start {
            *score = encoding.initial(query.len(), column)?;
        }
    }
    for row in 1..rows {
        current.fill(infinity);
        current[0] = encoding.add_edit(previous[0])?;
        trace[row * columns] = 2;
        for column in 1..columns {
            let diagonal = if query[row - 1] == reference[column - 1] {
                encoding.add_match(previous[column - 1])?
            } else {
                encoding.add_edit(previous[column - 1])?
            };
            let insertion = encoding.add_edit(previous[column])?;
            let deletion = encoding.add_edit(current[column - 1])?;
            let (score, direction) = if diagonal <= insertion && diagonal <= deletion {
                (diagonal, 1)
            } else if insertion <= deletion {
                (insertion, 2)
            } else {
                (deletion, 3)
            };
            current[column] = score;
            trace[row * columns + column] = direction;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let endpoint = previous
        .iter()
        .enumerate()
        .skip(1)
        .try_fold(
            None,
            |best: Option<AlignmentEndpointScore>, (column, score)| {
                let Some(candidate) = encoding.endpoint(*score, query.len(), column)? else {
                    return Ok(best);
                };
                Ok::<_, ValidationError>(Some(match best {
                    Some(current) if current.sort_key() <= candidate.sort_key() => current,
                    _ => candidate,
                }))
            },
        )?
        .ok_or_else(|| ValidationError::Integrity("empty alignment reference".to_owned()))?;
    let mut column = endpoint.end;
    let mut row = query.len();
    let reference_bases = endpoint.end.checked_sub(endpoint.start).ok_or_else(|| {
        ValidationError::Integrity("alignment endpoint precedes its start".to_owned())
    })?;
    let expected_insertions = endpoint
        .edits
        .checked_add(endpoint.matches)
        .and_then(|value| value.checked_sub(u64::try_from(reference_bases).ok()?))
        .ok_or_else(|| {
            ValidationError::Integrity("optimized alignment insertion count underflow".to_owned())
        })?;
    let expected_deletions = endpoint
        .indels
        .checked_sub(expected_insertions)
        .ok_or_else(|| {
            ValidationError::Integrity("optimized alignment deletion count underflow".to_owned())
        })?;
    let operation_count = query
        .len()
        .checked_add(usize::try_from(expected_deletions).map_err(|_| {
            ValidationError::Resource("alignment operation count exceeds usize".to_owned())
        })?)
        .ok_or_else(|| {
            ValidationError::Resource("alignment operation count overflow".to_owned())
        })?;
    let mut operations = Vec::new();
    operations
        .try_reserve_exact(operation_count)
        .map_err(|error| {
            ValidationError::Resource(format!("cannot allocate alignment traceback: {error}"))
        })?;
    while row > 0 {
        match trace[row * columns + column] {
            1 => {
                operations.push(if query[row - 1] == reference[column - 1] {
                    AlignmentOp::Match
                } else {
                    AlignmentOp::Mismatch
                });
                row -= 1;
                column -= 1;
            }
            2 => {
                operations.push(AlignmentOp::Insertion);
                row -= 1;
            }
            3 => {
                operations.push(AlignmentOp::Deletion);
                column -= 1;
            }
            _ => {
                return Err(ValidationError::Integrity(
                    "invalid fitting-alignment traceback".to_owned(),
                ));
            }
        }
    }
    operations.reverse();
    let mut matches = 0u64;
    let mut mismatches = 0u64;
    let mut insertions = 0u64;
    let mut deletions = 0u64;
    for operation in &operations {
        match operation {
            AlignmentOp::Match => matches += 1,
            AlignmentOp::Mismatch => mismatches += 1,
            AlignmentOp::Insertion => insertions += 1,
            AlignmentOp::Deletion => deletions += 1,
        }
    }
    let edits = mismatches + insertions + deletions;
    if edits != endpoint.edits
        || matches != endpoint.matches
        || insertions + deletions != endpoint.indels
        || insertions != expected_insertions
        || deletions != expected_deletions
        || column != endpoint.start
    {
        return Err(ValidationError::Integrity(
            "alignment traceback does not reconcile to the optimized score".to_owned(),
        ));
    }
    Ok(AlignmentCandidate {
        molecule_index,
        strand,
        oriented_reference_start: column,
        operations,
        edits,
        matches,
        mismatches,
        insertions,
        deletions,
    })
}

fn evaluate_junctions(
    contigs: &[FastaRecord],
    truths: &[TruthMolecule],
    config: &EvaluationConfig,
    path: &Path,
) -> ValidationResult<JunctionEvaluation> {
    let flank = config.junction_flank_length;
    let combined_length = flank.checked_mul(2).ok_or_else(|| {
        ValidationError::Resource("combined junction flank length overflow".to_owned())
    })?;
    let index_required = contigs
        .iter()
        .any(|contig| contig.sequence.len() >= combined_length);
    let (
        flank_index,
        combined_index,
        index_execution_status,
        index_projected_bytes,
        index_capacity_bytes,
    ) = if index_required {
        let flank_records = occurrence_count(truths, flank)?;
        let combined_records = occurrence_count(truths, combined_length)?;
        let total_records = flank_records.checked_add(combined_records).ok_or_else(|| {
            ValidationError::Resource("junction index record count overflow".to_owned())
        })?;
        let projected = total_records
            .checked_mul(
                u64::try_from(std::mem::size_of::<PackedOccurrence>()).expect("type size fits u64"),
            )
            .and_then(|bytes| bytes.checked_mul(2))
            .and_then(|bytes| bytes.checked_add(JUNCTION_INDEX_FIXED_BYTES))
            .ok_or_else(|| {
                ValidationError::Resource("junction index byte estimate overflow".to_owned())
            })?;
        if projected > config.max_junction_index_bytes {
            return Err(ValidationError::Resource(format!(
                "junction index projects {projected} bytes, above max {}",
                config.max_junction_index_bytes
            )));
        }
        let flank_index = PackedWindowIndex::build(truths, flank, flank_records)?;
        let combined_index = PackedWindowIndex::build(truths, combined_length, combined_records)?;
        let capacity = flank_index
            .capacity_bytes()?
            .checked_add(combined_index.capacity_bytes()?)
            .and_then(|bytes| bytes.checked_add(JUNCTION_INDEX_FIXED_BYTES))
            .ok_or_else(|| {
                ValidationError::Resource("junction index capacity byte count overflow".to_owned())
            })?;
        if capacity > config.max_junction_index_bytes {
            return Err(ValidationError::Resource(format!(
                "junction index post-reservation capacity requires {capacity} bytes, above max {}",
                config.max_junction_index_bytes
            )));
        }
        let execution_status = if total_records == 0 {
            "built_empty_truth_window_universe"
        } else {
            "built"
        };
        (
            Some(flank_index),
            Some(combined_index),
            execution_status,
            projected,
            capacity,
        )
    } else {
        (None, None, "not_required_no_eligible_adjacencies", 0, 0)
    };
    let mut writer = JunctionEvidenceWriter::new(
        path,
        config.junction_evidence_mode,
        config.max_junction_evidence_bytes,
    )?;
    let mut work = JunctionWork::default();
    let mut counts = JunctionCounts::default();
    for contig in contigs {
        let adjacencies = contig.sequence.len().saturating_sub(1);
        counts.assembly =
            checked_increment_by_usize(counts.assembly, adjacencies, "adjacency count overflow")?;
        for boundary in 1..contig.sequence.len() {
            if boundary < flank || contig.sequence.len() - boundary < flank {
                counts.short = checked_increment(counts.short, "short-junction count overflow")?;
                continue;
            }
            counts.eligible =
                checked_increment(counts.eligible, "eligible-junction count overflow")?;
            let flank_index = flank_index.as_ref().ok_or_else(|| {
                ValidationError::Integrity(
                    "eligible junction has no constructed flank index".to_owned(),
                )
            })?;
            let combined_index = combined_index.as_ref().ok_or_else(|| {
                ValidationError::Integrity(
                    "eligible junction has no constructed combined-flank index".to_owned(),
                )
            })?;
            let left = flank_index.lookup(
                encode_exact_window(&contig.sequence[boundary - flank..boundary]),
                &mut work,
                config.max_junction_comparisons,
            )?;
            let right = flank_index.lookup(
                encode_exact_window(&contig.sequence[boundary..boundary + flank]),
                &mut work,
                config.max_junction_comparisons,
            )?;
            let compatible = combined_index.lookup(
                encode_exact_window(&contig.sequence[boundary - flank..boundary + flank]),
                &mut work,
                config.max_junction_comparisons,
            )?;
            let (class, chimera) = if compatible.count != 0 {
                counts.correct =
                    checked_increment(counts.correct, "correct-junction count overflow")?;
                (JunctionClass::Correct, false)
            } else if left.count == 0 || right.count == 0 {
                counts.indeterminate_unmapped = checked_increment(
                    counts.indeterminate_unmapped,
                    "unmapped-junction count overflow",
                )?;
                (JunctionClass::IndeterminateUnmapped, false)
            } else if left.count != 1 || right.count != 1 {
                counts.indeterminate_ambiguous = checked_increment(
                    counts.indeterminate_ambiguous,
                    "ambiguous-junction count overflow",
                )?;
                (JunctionClass::IndeterminateAmbiguous, false)
            } else {
                counts.false_junctions =
                    checked_increment(counts.false_junctions, "false-junction count overflow")?;
                let left_class = truths[left
                    .unique
                    .expect("count one has unique placement")
                    .molecule_index]
                    .class;
                let right_class = truths[right
                    .unique
                    .expect("count one has unique placement")
                    .molecule_index]
                    .class;
                let chimera = matches!(
                    (left_class, right_class),
                    (TruthClass::Primary, TruthClass::Minor)
                        | (TruthClass::Minor, TruthClass::Primary)
                );
                if chimera {
                    counts.chimeras = checked_increment(counts.chimeras, "chimera count overflow")?;
                }
                (JunctionClass::False, chimera)
            };
            writer.observe(
                &contig.id,
                &JunctionRow {
                    boundary,
                    class,
                    compatible,
                    left,
                    right,
                    primary_minor_chimera: chimera,
                },
                truths,
            )?;
        }
    }
    validate_junction_count_invariants(&counts)?;
    let evidence = writer.finish(counts.eligible, counts.correct)?;
    Ok(JunctionEvaluation {
        counts,
        evidence,
        key_comparisons: work.key_comparisons,
        index_execution_status,
        index_projected_bytes,
        index_capacity_bytes,
    })
}

#[derive(Default)]
struct JunctionCounts {
    assembly: u64,
    eligible: u64,
    correct: u64,
    false_junctions: u64,
    indeterminate_ambiguous: u64,
    indeterminate_unmapped: u64,
    short: u64,
    chimeras: u64,
}

impl PackedWindowIndex {
    fn build(
        truths: &[TruthMolecule],
        window_length: usize,
        record_count: u64,
    ) -> ValidationResult<Self> {
        let capacity = usize::try_from(record_count).map_err(|_| {
            ValidationError::Resource("junction index record count exceeds usize".to_owned())
        })?;
        let mut occurrences = Vec::new();
        occurrences.try_reserve_exact(capacity).map_err(|error| {
            ValidationError::Resource(format!(
                "cannot allocate junction occurrence index: {error}"
            ))
        })?;
        for (molecule_index, truth) in truths.iter().enumerate() {
            let molecule_index = u16::try_from(molecule_index).map_err(|_| {
                ValidationError::Resource("truth molecule index exceeds u16".to_owned())
            })?;
            for (strand_rank, strand) in [(0u8, Strand::Forward), (1u8, Strand::Reverse)] {
                append_window_occurrences(
                    truth,
                    molecule_index,
                    strand_rank,
                    strand,
                    window_length,
                    &mut occurrences,
                )?;
            }
        }
        if occurrences.len() != capacity {
            return Err(ValidationError::Integrity(
                "junction index record count changed during construction".to_owned(),
            ));
        }
        occurrences.sort_unstable();
        Ok(Self { occurrences })
    }

    fn capacity_bytes(&self) -> ValidationResult<u64> {
        u64::try_from(self.occurrences.capacity())
            .map_err(|_| {
                ValidationError::Resource(
                    "junction index occurrence capacity exceeds u64".to_owned(),
                )
            })?
            .checked_mul(
                u64::try_from(std::mem::size_of::<PackedOccurrence>()).expect("type size fits u64"),
            )
            .ok_or_else(|| {
                ValidationError::Resource(
                    "junction index occurrence capacity bytes overflow".to_owned(),
                )
            })
    }

    fn lookup(
        &self,
        key: u128,
        work: &mut JunctionWork,
        max_comparisons: u64,
    ) -> ValidationResult<PlacementSummary> {
        let start = self.partition_point(key, false, work, max_comparisons)?;
        let end = self.partition_point(key, true, work, max_comparisons)?;
        let count = u64::try_from(end - start).map_err(|_| {
            ValidationError::Resource("junction placement count exceeds u64".to_owned())
        })?;
        let unique = if count == 1 {
            let occurrence = self.occurrences[start];
            Some(ExactPlacement {
                molecule_index: usize::from(occurrence.molecule_index),
                strand_rank: occurrence.strand_rank,
                start: usize::try_from(occurrence.start)
                    .expect("u32 fits usize on supported targets"),
            })
        } else {
            None
        };
        Ok(PlacementSummary { count, unique })
    }

    fn partition_point(
        &self,
        key: u128,
        include_equal: bool,
        work: &mut JunctionWork,
        max_comparisons: u64,
    ) -> ValidationResult<usize> {
        let mut left = 0usize;
        let mut right = self.occurrences.len();
        while left < right {
            work.key_comparisons = checked_increment(
                work.key_comparisons,
                "junction key-comparison count overflow",
            )?;
            if work.key_comparisons > max_comparisons {
                return Err(ValidationError::Resource(format!(
                    "junction index key comparisons exceed max {max_comparisons}"
                )));
            }
            let middle = left + (right - left) / 2;
            let observed = self.occurrences[middle].key;
            if observed < key || (include_equal && observed == key) {
                left = middle + 1;
            } else {
                right = middle;
            }
        }
        Ok(left)
    }
}

fn occurrence_count(truths: &[TruthMolecule], window_length: usize) -> ValidationResult<u64> {
    truths.iter().try_fold(0u64, |total, truth| {
        let starts = match truth.topology {
            TruthTopology::Linear => truth
                .sequence
                .len()
                .saturating_sub(window_length.saturating_sub(1)),
            TruthTopology::Circular => truth.sequence.len(),
        };
        let both_strands = u64::try_from(starts)
            .map_err(|_| ValidationError::Resource("junction start count exceeds u64".to_owned()))?
            .checked_mul(2)
            .ok_or_else(|| ValidationError::Resource("junction start count overflow".to_owned()))?;
        total.checked_add(both_strands).ok_or_else(|| {
            ValidationError::Resource("junction index record count overflow".to_owned())
        })
    })
}

fn append_window_occurrences(
    truth: &TruthMolecule,
    molecule_index: u16,
    strand_rank: u8,
    strand: Strand,
    window_length: usize,
    output: &mut Vec<PackedOccurrence>,
) -> ValidationResult<()> {
    let starts = match truth.topology {
        TruthTopology::Linear => truth
            .sequence
            .len()
            .saturating_sub(window_length.saturating_sub(1)),
        TruthTopology::Circular => truth.sequence.len(),
    };
    if starts == 0 {
        return Ok(());
    }
    let mut key = 0u128;
    for offset in 0..window_length {
        let position = if truth.topology == TruthTopology::Circular {
            offset % truth.sequence.len()
        } else {
            offset
        };
        key = (key << 2)
            | u128::from(exact_base_bits(oriented_base(
                &truth.sequence,
                position,
                strand,
            )));
    }
    let mask = (1u128 << (2 * window_length)) - 1;
    for start in 0..starts {
        output.push(PackedOccurrence {
            key,
            molecule_index,
            strand_rank,
            start: u32::try_from(start).map_err(|_| {
                ValidationError::Resource("junction placement start exceeds u32".to_owned())
            })?,
        });
        if start + 1 != starts {
            let next = start.checked_add(window_length).ok_or_else(|| {
                ValidationError::Resource("junction rolling-window position overflow".to_owned())
            })?;
            let position = if truth.topology == TruthTopology::Circular {
                next % truth.sequence.len()
            } else {
                next
            };
            key = ((key << 2) & mask)
                | u128::from(exact_base_bits(oriented_base(
                    &truth.sequence,
                    position,
                    strand,
                )));
        }
    }
    Ok(())
}

fn encode_exact_window(sequence: &[u8]) -> u128 {
    sequence.iter().fold(0u128, |key, base| {
        (key << 2) | u128::from(exact_base_bits(*base))
    })
}

fn exact_base_bits(base: u8) -> u8 {
    match base {
        b'A' => 0,
        b'C' => 1,
        b'G' => 2,
        b'T' => 3,
        _ => unreachable!("validation FASTA parser accepts only A/C/G/T"),
    }
}

fn oriented_base(sequence: &[u8], position: usize, strand: Strand) -> u8 {
    match strand {
        Strand::Forward => sequence[position],
        Strand::Reverse => match sequence[sequence.len() - 1 - position] {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            _ => unreachable!("validation FASTA parser accepts only A/C/G/T"),
        },
    }
}

struct JunctionEvidenceWriter {
    writer: BufWriter<File>,
    mode: JunctionEvidenceMode,
    max_bytes: u64,
    artifact_bytes: u64,
    logical_rows: u64,
    emitted_rows: u64,
    omitted_correct_rows: u64,
    logical_digest: Sha256,
    row_buffer: String,
}

impl JunctionEvidenceWriter {
    fn new(path: &Path, mode: JunctionEvidenceMode, max_bytes: u64) -> ValidationResult<Self> {
        const HEADER: &[u8] = b"schema_version\tcontig_id\tboundary_zero_based\tclassification\tcompatible_placement_count\tleft_flank_placement_count\tright_flank_placement_count\tprimary_minor_chimera\tcompatible_unique_placement\tleft_flank_unique_placement\tright_flank_unique_placement\n";
        let header_bytes = u64::try_from(HEADER.len()).expect("header length fits u64");
        if header_bytes > max_bytes {
            return Err(ValidationError::Resource(format!(
                "junction evidence header requires {header_bytes} bytes, above max {max_bytes}"
            )));
        }
        let mut writer = BufWriter::new(File::create(path)?);
        writer.write_all(HEADER)?;
        let mut row_buffer = String::new();
        row_buffer.try_reserve_exact(2_048).map_err(|error| {
            ValidationError::Resource(format!("cannot allocate junction row buffer: {error}"))
        })?;
        Ok(Self {
            writer,
            mode,
            max_bytes,
            artifact_bytes: header_bytes,
            logical_rows: 0,
            emitted_rows: 0,
            omitted_correct_rows: 0,
            logical_digest: Sha256::new(),
            row_buffer,
        })
    }

    fn observe(
        &mut self,
        contig_id: &str,
        row: &JunctionRow,
        truths: &[TruthMolecule],
    ) -> ValidationResult<()> {
        self.row_buffer.clear();
        write!(
            self.row_buffer,
            "2.0\t{contig_id}\t{}\t{}\t{}\t{}\t{}\t{}\t",
            row.boundary,
            row.class.as_str(),
            row.compatible.count,
            row.left.count,
            row.right.count,
            row.primary_minor_chimera,
        )
        .expect("writing to String cannot fail");
        write_unique_placement(&mut self.row_buffer, row.compatible.unique, truths);
        self.row_buffer.push('\t');
        write_unique_placement(&mut self.row_buffer, row.left.unique, truths);
        self.row_buffer.push('\t');
        write_unique_placement(&mut self.row_buffer, row.right.unique, truths);
        self.row_buffer.push('\n');

        self.logical_digest.update(self.row_buffer.as_bytes());
        self.logical_rows =
            checked_increment(self.logical_rows, "logical junction-row count overflow")?;
        if self.mode.emits(&row.class) {
            let row_bytes = u64::try_from(self.row_buffer.len()).map_err(|_| {
                ValidationError::Resource("junction row byte count exceeds u64".to_owned())
            })?;
            let next_bytes = self.artifact_bytes.checked_add(row_bytes).ok_or_else(|| {
                ValidationError::Resource("junction evidence byte count overflow".to_owned())
            })?;
            if next_bytes > self.max_bytes {
                return Err(ValidationError::Resource(format!(
                    "junction evidence requires more than {} bytes in {} mode",
                    self.max_bytes,
                    self.mode.as_str()
                )));
            }
            self.writer.write_all(self.row_buffer.as_bytes())?;
            self.artifact_bytes = next_bytes;
            self.emitted_rows =
                checked_increment(self.emitted_rows, "emitted junction-row count overflow")?;
        } else if row.class == JunctionClass::Correct {
            self.omitted_correct_rows = checked_increment(
                self.omitted_correct_rows,
                "omitted correct junction-row count overflow",
            )?;
        }
        Ok(())
    }

    fn finish(
        mut self,
        expected_logical_rows: u64,
        expected_correct_rows: u64,
    ) -> ValidationResult<JunctionEvidenceRecord> {
        self.writer.flush()?;
        if self.logical_rows != expected_logical_rows {
            return Err(ValidationError::Integrity(
                "logical junction evidence rows do not equal eligible junctions".to_owned(),
            ));
        }
        let expected_emitted_rows = match self.mode {
            JunctionEvidenceMode::All => expected_logical_rows,
            JunctionEvidenceMode::NonCorrect => expected_logical_rows
                .checked_sub(expected_correct_rows)
                .ok_or_else(|| {
                    ValidationError::Integrity(
                        "correct junction rows exceed logical junction rows".to_owned(),
                    )
                })?,
            JunctionEvidenceMode::Summary => 0,
        };
        let expected_omitted_correct_rows = match self.mode {
            JunctionEvidenceMode::All => 0,
            JunctionEvidenceMode::NonCorrect | JunctionEvidenceMode::Summary => {
                expected_correct_rows
            }
        };
        if self.emitted_rows != expected_emitted_rows
            || self.omitted_correct_rows != expected_omitted_correct_rows
        {
            return Err(ValidationError::Integrity(
                "junction evidence mode counters do not reconcile".to_owned(),
            ));
        }
        let omitted_rows = self
            .logical_rows
            .checked_sub(self.emitted_rows)
            .ok_or_else(|| {
                ValidationError::Integrity("emitted junction rows exceed logical rows".to_owned())
            })?;
        Ok(JunctionEvidenceRecord {
            mode: self.mode,
            logical_rows_decimal: self.logical_rows.to_string(),
            emitted_rows_decimal: self.emitted_rows.to_string(),
            omitted_rows_decimal: omitted_rows.to_string(),
            omitted_correct_rows_decimal: self.omitted_correct_rows.to_string(),
            logical_rows_sha256: super::lower_hex(&self.logical_digest.finalize()),
            artifact_bytes_decimal: self.artifact_bytes.to_string(),
        })
    }
}

fn write_unique_placement(
    output: &mut String,
    placement: Option<ExactPlacement>,
    truths: &[TruthMolecule],
) {
    if let Some(placement) = placement {
        write!(
            output,
            "{}:{}:{}",
            truths[placement.molecule_index].id,
            if placement.strand_rank == 0 { '+' } else { '-' },
            placement.start
        )
        .expect("writing to String cannot fail");
    } else {
        output.push_str("NA");
    }
}

fn validate_junction_count_invariants(counts: &JunctionCounts) -> ValidationResult<()> {
    let classified = counts
        .correct
        .checked_add(counts.false_junctions)
        .and_then(|value| value.checked_add(counts.indeterminate_ambiguous))
        .and_then(|value| value.checked_add(counts.indeterminate_unmapped))
        .ok_or_else(|| {
            ValidationError::Resource("classified junction count overflow".to_owned())
        })?;
    let accounted = counts.eligible.checked_add(counts.short).ok_or_else(|| {
        ValidationError::Resource("accounted adjacency count overflow".to_owned())
    })?;
    if classified != counts.eligible
        || accounted != counts.assembly
        || counts.chimeras > counts.false_junctions
    {
        return Err(ValidationError::Integrity(
            "junction metric counters do not reconcile".to_owned(),
        ));
    }
    Ok(())
}

fn checked_increment(value: u64, context: &str) -> ValidationResult<u64> {
    value
        .checked_add(1)
        .ok_or_else(|| ValidationError::Resource(context.to_owned()))
}

fn checked_increment_by_usize(
    value: u64,
    increment: usize,
    context: &str,
) -> ValidationResult<u64> {
    value
        .checked_add(
            u64::try_from(increment).map_err(|_| ValidationError::Resource(context.to_owned()))?,
        )
        .ok_or_else(|| ValidationError::Resource(context.to_owned()))
}

#[cfg(test)]
fn exact_placements(needle: &[u8], truths: &[TruthMolecule]) -> BTreeSet<ExactPlacement> {
    let mut placements = BTreeSet::new();
    for (molecule_index, truth) in truths.iter().enumerate() {
        for (strand_rank, oriented) in [truth.sequence.clone(), reverse_complement(&truth.sequence)]
            .into_iter()
            .enumerate()
        {
            let start_count = if truth.topology == TruthTopology::Circular {
                truth.sequence.len()
            } else if needle.len() <= truth.sequence.len() {
                truth.sequence.len() - needle.len() + 1
            } else {
                0
            };
            let starts = 0..start_count;
            for start in starts {
                let matches = (0..needle.len()).all(|offset| {
                    let position = start + offset;
                    let observed = if position < oriented.len() {
                        oriented[position]
                    } else {
                        oriented[position % oriented.len()]
                    };
                    observed == needle[offset]
                });
                if matches {
                    placements.insert(ExactPlacement {
                        molecule_index,
                        strand_rank: strand_rank as u8,
                        start,
                    });
                }
            }
        }
    }
    placements
}

#[allow(clippy::too_many_arguments)]
fn summarize(
    config: &EvaluationConfig,
    dataset: &DatasetManifest,
    truths: &[TruthMolecule],
    assemblies: &[FastaRecord],
    rows: &[AlignmentRow],
    recovery_coverage: RecoveryCoverage,
    junction_evaluation: JunctionEvaluation,
    exact_alignment_scan_bases: u64,
    evaluation_id: String,
    dataset_sha256: String,
    truth_sha256: String,
    assembly_sha256: String,
    raw_alignments_sha256: String,
    raw_junctions_sha256: String,
    content_binding: DatasetContentBinding,
    truth_evidence: TruthEvidenceAudit,
) -> ValidationResult<EvaluationResult> {
    let JunctionEvaluation {
        counts: junctions,
        evidence: junction_evidence,
        key_comparisons,
        index_execution_status,
        index_projected_bytes,
        index_capacity_bytes,
    } = junction_evaluation;
    let mut matches = 0u64;
    let mut mismatches = 0u64;
    let mut insertions = 0u64;
    let mut deletions = 0u64;
    let mut accepted = 0u64;
    let mut unaligned_records = 0u64;
    let mut unaligned_bases = 0u64;
    for row in rows {
        let Some(candidate) = &row.candidate else {
            unaligned_records += 1;
            unaligned_bases = unaligned_bases
                .checked_add(u64::try_from(row.contig_length).map_err(|_| {
                    ValidationError::Resource("contig length exceeds u64".to_owned())
                })?)
                .ok_or_else(|| {
                    ValidationError::Resource("unaligned base count overflow".to_owned())
                })?;
            continue;
        };
        accepted += 1;
        matches = checked_sum(matches, candidate.matches, "match count")?;
        mismatches = checked_sum(mismatches, candidate.mismatches, "mismatch count")?;
        insertions = checked_sum(insertions, candidate.insertions, "insertion count")?;
        deletions = checked_sum(deletions, candidate.deletions, "deletion count")?;
    }
    let truth_bases = truths.iter().try_fold(0u64, |total, truth| {
        checked_sum(
            total,
            u64::try_from(truth.sequence.len())
                .map_err(|_| ValidationError::Resource("truth length exceeds u64".to_owned()))?,
            "truth base count",
        )
    })?;
    let exact_accounted = recovery_coverage
        .exact_unambiguous_records
        .checked_add(recovery_coverage.exact_ambiguous_records)
        .and_then(|value| value.checked_add(recovery_coverage.non_exact_records))
        .ok_or_else(|| ValidationError::Resource("recovery record count overflow".to_owned()))?;
    if exact_accounted != accepted {
        return Err(ValidationError::Integrity(
            "compatible-recovery record classes do not reconcile to accepted alignments".to_owned(),
        ));
    }
    let recovery_status = if recovery_coverage.non_exact_records == 0 {
        "complete_exact_compatible_placement_universe"
    } else {
        "not_available_non_exact_compatible_placement_universe"
    };
    let mut unique_covered = 0u64;
    let mut lower_covered = 0u64;
    let mut upper_covered = 0u64;
    for masks in &recovery_coverage.molecules {
        unique_covered = checked_sum(
            unique_covered,
            mask_count(&masks.unique, "unique-coordinate coverage")?,
            "unique-coordinate coverage",
        )?;
        lower_covered = checked_sum(
            lower_covered,
            mask_count(&masks.lower, "compatible lower-bound coverage")?,
            "compatible lower-bound coverage",
        )?;
        upper_covered = checked_sum(
            upper_covered,
            mask_count(&masks.upper, "compatible upper-bound coverage")?,
            "compatible upper-bound coverage",
        )?;
    }
    let errors = checked_sum(
        checked_sum(mismatches, insertions, "alignment error count")?,
        deletions,
        "alignment error count",
    )?;
    let columns = checked_sum(
        checked_sum(matches, mismatches, "alignment column count")?,
        checked_sum(insertions, deletions, "alignment column count")?,
        "alignment column count",
    )?;
    let correct_columns = columns.checked_sub(errors).ok_or_else(|| {
        ValidationError::Integrity("alignment errors exceed evaluated columns".to_owned())
    })?;
    let assembly_bases = assemblies.iter().try_fold(0u64, |total, record| {
        checked_sum(
            total,
            u64::try_from(record.sequence.len())
                .map_err(|_| ValidationError::Resource("assembly length exceeds u64".to_owned()))?,
            "assembly base count",
        )
    })?;
    let mut per_molecule = truths
        .iter()
        .zip(&recovery_coverage.molecules)
        .map(|(truth, masks)| {
            let unique = mask_count(&masks.unique, "molecule unique-coordinate coverage")?;
            let lower = mask_count(&masks.lower, "molecule compatible lower-bound coverage")?;
            let upper = mask_count(&masks.upper, "molecule compatible upper-bound coverage")?;
            let truth_length = u64::try_from(truth.sequence.len())
                .map_err(|_| ValidationError::Resource("truth length exceeds u64".to_owned()))?;
            Ok(MoleculeMetric {
                molecule_id: truth.id.clone(),
                truth_class: truth.class,
                topology: truth.topology,
                truth_bases_decimal: truth_length.to_string(),
                compatible_placement_status: recovery_status.to_owned(),
                unique_coordinate_covered_bases_decimal: unique.to_string(),
                compatible_covered_bases_lower_bound_decimal: lower.to_string(),
                compatible_covered_bases_upper_bound_decimal: upper.to_string(),
                unique_coordinate_truth_genome_fraction: availability_ratio(
                    unique,
                    truth_length,
                    recovery_status,
                )?,
                compatible_truth_genome_fraction_lower_bound: availability_ratio(
                    lower,
                    truth_length,
                    recovery_status,
                )?,
                compatible_truth_genome_fraction_upper_bound: availability_ratio(
                    upper,
                    truth_length,
                    recovery_status,
                )?,
            })
        })
        .collect::<ValidationResult<Vec<_>>>()?;
    per_molecule.sort_by(|left, right| {
        left.molecule_id
            .as_bytes()
            .cmp(right.molecule_id.as_bytes())
    });
    let status = if assemblies.is_empty() {
        EvaluationStatus {
            code: "evaluation_complete_empty_assembly".to_owned(),
            message: "Evaluation completed: the assembly contained no records; this is not biological absence.".to_owned(),
        }
    } else {
        EvaluationStatus {
            code: "evaluation_complete".to_owned(),
            message: "Evaluation completed under the recorded truth and metric rules.".to_owned(),
        }
    };
    Ok(EvaluationResult {
        schema_version: "veritasm-validation-result-v4".to_owned(),
        evaluation_id,
        evaluator: EvaluatorRecord {
            name: "veritasm-evaluate".to_owned(),
            version: EVALUATOR_VERSION.to_owned(),
            alignment_algorithm: "exact_compatible_coordinate_bounds_with_primary_alignment_diagnostic_v3".to_owned(),
            junction_algorithm: "exact_packed_window_occurrence_index_v2".to_owned(),
            max_edit_rate_ppm_decimal: config.max_edit_rate_ppm.to_string(),
            max_dp_cells_decimal: config.max_dp_cells.to_string(),
            max_exact_alignment_scan_bases_decimal: config
                .max_exact_alignment_scan_bases
                .to_string(),
            exact_alignment_scan_bases_decimal: exact_alignment_scan_bases.to_string(),
            max_junction_comparisons_decimal: config.max_junction_comparisons.to_string(),
            junction_key_comparisons_decimal: key_comparisons.to_string(),
            max_junction_flank_length_decimal: MAX_JUNCTION_FLANK_LENGTH.to_string(),
            max_junction_index_bytes_decimal: config.max_junction_index_bytes.to_string(),
            junction_index_execution_status: index_execution_status.to_owned(),
            junction_index_projected_bytes_decimal: index_projected_bytes.to_string(),
            junction_index_capacity_bytes_decimal: index_capacity_bytes.to_string(),
            max_junction_evidence_bytes_decimal: config.max_junction_evidence_bytes.to_string(),
            max_exact_alignment_prefix_bytes_decimal: MAX_EXACT_ALIGNMENT_PREFIX_BYTES.to_string(),
            max_dataset_manifest_bytes_decimal: MAX_DATASET_MANIFEST_BYTES.to_string(),
            max_dataset_artifact_bytes_decimal: MAX_DATASET_ARTIFACT_BYTES.to_string(),
            max_truth_fasta_bytes_decimal: MAX_TRUTH_FASTA_BYTES.to_string(),
            max_assembly_fasta_bytes_decimal: MAX_ASSEMBLY_FASTA_BYTES.to_string(),
            max_truth_evidence_replay_bytes_decimal: config
                .max_truth_evidence_replay_bytes
                .to_string(),
            projected_truth_evidence_replay_bytes_decimal: truth_evidence
                .projected_replay_bytes
                .to_string(),
            tie_breaking: "edit_distance,descending_matches,indels,molecule_manifest_order,strand_plus_before_minus,oriented_start".to_owned(),
        },
        inputs: EvaluationInputRecord {
            dataset_id: dataset.dataset_id.clone(),
            dataset_manifest_sha256: dataset_sha256,
            dataset_content_root_sha256: content_binding.content_root_sha256,
            dataset_binding: DatasetBindingRecord {
                mode: content_binding.mode,
                qualification_admission: if content_binding.mode
                    == DatasetBindingMode::ExternallyBound
                {
                    "eligible_externally_bound"
                } else {
                    "ineligible_development_unbound"
                }
                .to_owned(),
                expected_dataset_content_root_sha256: content_binding
                    .expected_content_root_sha256,
            },
            truth_fasta_sha256: truth_sha256,
            assembly_fasta_sha256: assembly_sha256,
        },
        status,
        assembly: AssemblySummary {
            records_decimal: assemblies.len().to_string(),
            bases_decimal: assembly_bases.to_string(),
            accepted_alignment_records_decimal: accepted.to_string(),
            unaligned_records_decimal: unaligned_records.to_string(),
            empty_output: assemblies.is_empty(),
        },
        base_metrics: BaseMetrics {
            alignment_metric_scope: "deterministic_primary_alignment_diagnostic_only".to_owned(),
            matches_decimal: matches.to_string(),
            mismatches_decimal: mismatches.to_string(),
            inserted_bases_decimal: insertions.to_string(),
            deleted_bases_decimal: deletions.to_string(),
            evaluated_alignment_columns_decimal: columns.to_string(),
            unaligned_assembly_bases_decimal: unaligned_bases.to_string(),
            error_rate: ratio(errors, columns, if columns == 0 { "not_available_no_aligned_bases" } else { "measured" })?,
            consensus_accuracy: ratio(correct_columns, columns, if columns == 0 { "not_available_no_aligned_bases" } else { "measured" })?,
            qv: qv(errors, columns),
            recovery: RecoveryMetrics {
                compatible_placement_status: recovery_status.to_owned(),
                exact_unambiguous_alignment_records_decimal: recovery_coverage
                    .exact_unambiguous_records
                    .to_string(),
                exact_ambiguous_alignment_records_decimal: recovery_coverage
                    .exact_ambiguous_records
                    .to_string(),
                non_exact_alignment_records_decimal: recovery_coverage
                    .non_exact_records
                    .to_string(),
                truth_bases_decimal: truth_bases.to_string(),
                unique_coordinate_covered_truth_bases_decimal: unique_covered.to_string(),
                compatible_covered_truth_bases_lower_bound_decimal: lower_covered.to_string(),
                compatible_covered_truth_bases_upper_bound_decimal: upper_covered.to_string(),
                unique_coordinate_truth_genome_fraction: availability_ratio(
                    unique_covered,
                    truth_bases,
                    recovery_status,
                )?,
                compatible_truth_genome_fraction_lower_bound: availability_ratio(
                    lower_covered,
                    truth_bases,
                    recovery_status,
                )?,
                compatible_truth_genome_fraction_upper_bound: availability_ratio(
                    upper_covered,
                    truth_bases,
                    recovery_status,
                )?,
            },
            aligned_duplication_ratio: duplication_ratio(
                matches,
                upper_covered,
                recovery_coverage.exact_ambiguous_records,
                recovery_coverage.non_exact_records,
            )?,
        },
        junction_metrics: JunctionMetrics {
            flank_length_decimal: config.junction_flank_length.to_string(),
            assembly_adjacencies_decimal: junctions.assembly.to_string(),
            eligible_adjacencies_decimal: junctions.eligible.to_string(),
            correct_decimal: junctions.correct.to_string(),
            false_decimal: junctions.false_junctions.to_string(),
            indeterminate_ambiguous_flank_decimal: junctions.indeterminate_ambiguous.to_string(),
            indeterminate_unmapped_flank_decimal: junctions.indeterminate_unmapped.to_string(),
            not_evaluated_short_flank_decimal: junctions.short.to_string(),
            primary_minor_chimera_decimal: junctions.chimeras.to_string(),
        },
        junction_evidence,
        per_molecule,
        raw_alignments_sha256,
        raw_junctions_sha256,
        limitations: vec![
            "Exact contigs use an exact-substring alignment fast path; edited long contigs still require a separately frozen scalable approximate aligner.".to_owned(),
            "The evaluator reports one deterministic primary alignment per contig and is not a replacement for an independent large-genome evaluator.".to_owned(),
            "Recovery uses unique-coordinate coverage plus compatible-coordinate lower and upper bounds; the primary alignment row does not select recovery coordinates.".to_owned(),
            "Duplication is unavailable when exact placements are ambiguous or when a non-exact compatible placement universe is not enumerated.".to_owned(),
            "Junctions require exact flanks; repeat-ambiguous or unmapped flanks are indeterminate rather than false.".to_owned(),
            "Non-all junction evidence modes retain exact metrics and a logical-row digest but intentionally omit selected per-adjacency rows.".to_owned(),
            "Truth-known synthetic results do not establish real-sample or biological detection performance.".to_owned(),
            "Minor-component metrics are local truth comparisons and do not establish global haplotype phasing.".to_owned(),
            "Dataset identity is a parameter commitment, not an authenticity signature or proof of independent dataset provenance.".to_owned(),
            "Ledger validation proves internal truth/read/event consistency; it does not independently replay every stochastic RNG draw from the recorded seeds.".to_owned(),
        ],
    })
}

#[cfg(test)]
fn forward_coordinate(position: usize, truth: &TruthMolecule, strand: Strand) -> usize {
    let modulo = position % truth.sequence.len();
    match strand {
        Strand::Forward => modulo,
        Strand::Reverse => truth.sequence.len() - 1 - modulo,
    }
}

fn checked_sum(left: u64, right: u64, label: &str) -> ValidationResult<u64> {
    left.checked_add(right)
        .ok_or_else(|| ValidationError::Resource(format!("{label} overflow")))
}

fn mask_count(mask: &[bool], label: &str) -> ValidationResult<u64> {
    u64::try_from(mask.iter().filter(|value| **value).count())
        .map_err(|_| ValidationError::Resource(format!("{label} exceeds u64")))
}

fn ratio(numerator: u64, denominator: u64, status: &str) -> ValidationResult<RatioMetric> {
    let value_decimal = if denominator == 0 {
        None
    } else {
        let scaled = u128::from(numerator)
            .checked_mul(1_000_000)
            .ok_or_else(|| ValidationError::Resource("ratio scaling overflow".to_owned()))?
            / u128::from(denominator);
        Some(format!("{}.{:06}", scaled / 1_000_000, scaled % 1_000_000))
    };
    Ok(RatioMetric {
        numerator_decimal: numerator.to_string(),
        denominator_decimal: denominator.to_string(),
        value_decimal,
        status: status.to_owned(),
    })
}

fn unavailable_ratio(numerator: u64, denominator: u64, status: &str) -> RatioMetric {
    RatioMetric {
        numerator_decimal: numerator.to_string(),
        denominator_decimal: denominator.to_string(),
        value_decimal: None,
        status: status.to_owned(),
    }
}

fn availability_ratio(
    numerator: u64,
    denominator: u64,
    recovery_status: &str,
) -> ValidationResult<RatioMetric> {
    if recovery_status == "complete_exact_compatible_placement_universe" {
        ratio(
            numerator,
            denominator,
            "measured_exact_compatible_placements",
        )
    } else {
        Ok(unavailable_ratio(numerator, denominator, recovery_status))
    }
}

fn duplication_ratio(
    matches: u64,
    compatible_upper_covered: u64,
    ambiguous_exact_records: u64,
    non_exact_records: u64,
) -> ValidationResult<RatioMetric> {
    if non_exact_records != 0 {
        Ok(unavailable_ratio(
            matches,
            compatible_upper_covered,
            "not_available_non_exact_compatible_placement_universe",
        ))
    } else if ambiguous_exact_records != 0 {
        Ok(unavailable_ratio(
            matches,
            compatible_upper_covered,
            "not_available_ambiguous_exact_compatible_placements",
        ))
    } else if compatible_upper_covered == 0 {
        Ok(unavailable_ratio(
            matches,
            compatible_upper_covered,
            "not_available_no_covered_truth_bases",
        ))
    } else {
        ratio(
            matches,
            compatible_upper_covered,
            "measured_exact_unambiguous_placements",
        )
    }
}

fn validate_ratio_metric(
    actual: &RatioMetric,
    numerator: u64,
    denominator: u64,
    status: &str,
    label: &str,
) -> ValidationResult<()> {
    let expected = ratio(numerator, denominator, status)?;
    if actual != &expected {
        return Err(ValidationError::Integrity(format!(
            "{label} value, status, or counts do not reconcile"
        )));
    }
    Ok(())
}

fn qv(errors: u64, columns: u64) -> QvMetric {
    if columns == 0 {
        QvMetric {
            value_decimal: None,
            status: "not_available_no_aligned_bases".to_owned(),
            evaluated_columns_decimal: columns.to_string(),
            observed_errors_decimal: errors.to_string(),
        }
    } else if errors == 0 {
        QvMetric {
            value_decimal: Some(format_qv_decimal(u128::from(columns) + 1, 1)),
            status: "lower_bound_zero_observed_errors".to_owned(),
            evaluated_columns_decimal: columns.to_string(),
            observed_errors_decimal: errors.to_string(),
        }
    } else {
        QvMetric {
            value_decimal: Some(format_qv_decimal(u128::from(columns), u128::from(errors))),
            status: "measured".to_owned(),
            evaluated_columns_decimal: columns.to_string(),
            observed_errors_decimal: errors.to_string(),
        }
    }
}

/// Approximates `10 * log10(numerator / denominator)` with six fractional
/// decimal digits using only checked integer arithmetic.
///
/// The ratio must be at least one. A truncated Q63 binary logarithm is divided
/// by the nearest Q63 representation of `log2(10)`, then the resulting fixed-
/// point approximation is rounded to a micro-QV (half upward). This freezes
/// output bytes across Rust versions and platforms instead of depending on the
/// implementation precision of `f64` transcendental functions. It is not an
/// arbitrary-precision, correctly rounded implementation of real `log10`.
fn format_qv_decimal(numerator: u128, denominator: u128) -> String {
    const FRACTION_BITS: u32 = 63;
    const ONE_Q63: u128 = 1u128 << FRACTION_BITS;
    const TWO_Q63: u128 = 2u128 << FRACTION_BITS;
    const LOG2_10_Q63: u128 = 30_639_378_698_826_356_221;
    const MICRO_QV_SCALE: u128 = 10_000_000;

    debug_assert!(denominator > 0);
    debug_assert!(numerator >= denominator);
    let numerator_bits = 128 - numerator.leading_zeros();
    let denominator_bits = 128 - denominator.leading_zeros();
    let mut exponent = numerator_bits - denominator_bits;
    let mut scaled_denominator = denominator
        .checked_shl(exponent)
        .expect("QV normalization shift remains inside u128");
    if scaled_denominator > numerator {
        exponent -= 1;
        scaled_denominator >>= 1;
    }

    let shifted_numerator = numerator
        .checked_shl(FRACTION_BITS)
        .expect("QV numerator and Q63 scale remain inside u128");
    let mut normalized = shifted_numerator / scaled_denominator;
    debug_assert!((ONE_Q63..TWO_Q63).contains(&normalized));
    let mut fractional_log2 = 0u128;
    for bit in 1..=FRACTION_BITS {
        normalized = normalized
            .checked_mul(normalized)
            .expect("a normalized Q63 square remains inside u128")
            >> FRACTION_BITS;
        if normalized >= TWO_Q63 {
            normalized >>= 1;
            fractional_log2 |= 1u128 << (FRACTION_BITS - bit);
        }
    }

    let log2_q63 = u128::from(exponent)
        .checked_mul(ONE_Q63)
        .and_then(|integer| integer.checked_add(fractional_log2))
        .expect("QV log2 representation remains inside u128");
    let scaled = log2_q63
        .checked_mul(MICRO_QV_SCALE)
        .expect("QV decimal scaling remains inside u128");
    let micro_qv = scaled
        .checked_add(LOG2_10_Q63 / 2)
        .expect("QV rounding remains inside u128")
        / LOG2_10_Q63;
    format!("{}.{:06}", micro_qv / 1_000_000, micro_qv % 1_000_000)
}

fn enforce_alignment_table_limit(
    alignment_rows: &[AlignmentRow],
    truths: &[TruthMolecule],
) -> ValidationResult<()> {
    let mut alignment_bytes = PROJECTED_HEADER_BYTES;
    for row in alignment_rows {
        let identifier_bytes = u64::try_from(row.contig_id.len())
            .map_err(|_| ValidationError::Resource("contig ID length exceeds u64".to_owned()))?;
        let operation_bytes = row.candidate.as_ref().map_or(Ok(0u64), |candidate| {
            u64::try_from(candidate.operations.len())
                .map_err(|_| {
                    ValidationError::Resource("alignment operation count exceeds u64".to_owned())
                })?
                .checked_mul(2)
                .ok_or_else(|| {
                    ValidationError::Resource("projected CIGAR bytes overflow".to_owned())
                })
        })?;
        let truth_identifier_bytes = row
            .candidate
            .as_ref()
            .map(|candidate| truths[candidate.molecule_index].id.len())
            .unwrap_or(0);
        let truth_identifier_bytes = u64::try_from(truth_identifier_bytes)
            .map_err(|_| ValidationError::Resource("truth ID length exceeds u64".to_owned()))?;
        alignment_bytes = alignment_bytes
            .checked_add(PROJECTED_ROW_FIXED_BYTES)
            .and_then(|bytes| bytes.checked_add(identifier_bytes))
            .and_then(|bytes| bytes.checked_add(operation_bytes))
            .and_then(|bytes| bytes.checked_add(truth_identifier_bytes))
            .ok_or_else(|| {
                ValidationError::Resource("projected alignment table bytes overflow".to_owned())
            })?;
        if alignment_bytes > MAX_RAW_EVALUATION_TABLE_BYTES {
            return Err(ValidationError::Resource(format!(
                "projected alignment table bytes {alignment_bytes} exceed fixed cap {MAX_RAW_EVALUATION_TABLE_BYTES}"
            )));
        }
    }

    Ok(())
}

fn write_alignment_rows(
    path: &Path,
    rows: &[AlignmentRow],
    truths: &[TruthMolecule],
) -> ValidationResult<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(b"schema_version\tcontig_id\tcontig_length\tstatus\tmolecule_id\ttruth_class\ttopology\tstrand\toriented_reference_start\treference_bases_consumed\tmatches\tmismatches\tinserted_bases\tdeleted_bases\tedit_distance\talignment_columns\tequally_best_molecule_strands\tcigar\n")?;
    for row in rows {
        if let Some(candidate) = &row.candidate {
            let truth = &truths[candidate.molecule_index];
            writeln!(
                writer,
                "1.0\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                row.contig_id,
                row.contig_length,
                row.status,
                truth.id,
                truth_class_name(truth.class),
                topology_name(truth.topology),
                candidate.strand.symbol(),
                candidate.oriented_reference_start,
                candidate.reference_bases(),
                candidate.matches,
                candidate.mismatches,
                candidate.insertions,
                candidate.deletions,
                candidate.edits,
                candidate.columns(),
                row.equally_best_targets,
                cigar(&candidate.operations)
            )?;
        } else {
            writeln!(
                writer,
                "1.0\t{}\t{}\t{}\tNA\tNA\tNA\tNA\tNA\tNA\tNA\tNA\tNA\tNA\tNA\tNA\t{}\tNA",
                row.contig_id, row.contig_length, row.status, row.equally_best_targets
            )?;
        }
    }
    writer.flush()?;
    Ok(())
}

fn cigar(operations: &[AlignmentOp]) -> String {
    let mut output = String::new();
    let mut index = 0;
    while index < operations.len() {
        let operation = operations[index];
        let mut end = index + 1;
        while end < operations.len() && operations[end] == operation {
            end += 1;
        }
        output.push_str(&(end - index).to_string());
        output.push(operation.cigar());
        index = end;
    }
    output
}

fn truth_class_name(class: TruthClass) -> &'static str {
    match class {
        TruthClass::Primary => "primary",
        TruthClass::Minor => "minor",
    }
}

fn topology_name(topology: TruthTopology) -> &'static str {
    match topology {
        TruthTopology::Linear => "linear",
        TruthTopology::Circular => "circular",
    }
}

fn evaluation_id(
    dataset_id: &str,
    dataset_sha256: &str,
    truth_sha256: &str,
    assembly_sha256: &str,
    content_binding: &DatasetContentBinding,
    config: &EvaluationConfig,
) -> String {
    evaluation_id_from_values(
        dataset_id,
        dataset_sha256,
        &content_binding.content_root_sha256,
        content_binding.mode,
        content_binding.expected_content_root_sha256.as_deref(),
        truth_sha256,
        assembly_sha256,
        config.junction_flank_length as u64,
        u64::from(config.max_edit_rate_ppm),
        config.max_dp_cells,
        config.max_exact_alignment_scan_bases,
        config.max_junction_comparisons,
        config.junction_evidence_mode,
        config.max_junction_index_bytes,
        config.max_junction_evidence_bytes,
        config.max_truth_evidence_replay_bytes,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluation_id_from_values(
    dataset_id: &str,
    dataset_sha256: &str,
    dataset_content_root_sha256: &str,
    dataset_binding_mode: DatasetBindingMode,
    expected_dataset_content_root_sha256: Option<&str>,
    truth_sha256: &str,
    assembly_sha256: &str,
    junction_flank_length: u64,
    max_edit_rate_ppm: u64,
    max_dp_cells: u64,
    max_exact_alignment_scan_bases: u64,
    max_junction_comparisons: u64,
    junction_evidence_mode: JunctionEvidenceMode,
    max_junction_index_bytes: u64,
    max_junction_evidence_bytes: u64,
    max_truth_evidence_replay_bytes: u64,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:validation-evaluation:v4\0");
    digest.update(dataset_id.as_bytes());
    digest.update([0]);
    digest.update(dataset_sha256.as_bytes());
    digest.update([0]);
    digest.update(dataset_content_root_sha256.as_bytes());
    digest.update([0]);
    digest.update(dataset_binding_mode.as_str().as_bytes());
    digest.update([0]);
    digest.update(
        expected_dataset_content_root_sha256
            .unwrap_or("not_supplied")
            .as_bytes(),
    );
    digest.update([0]);
    digest.update(truth_sha256.as_bytes());
    digest.update([0]);
    digest.update(assembly_sha256.as_bytes());
    digest.update([0]);
    digest.update(junction_flank_length.to_string().as_bytes());
    digest.update([0]);
    digest.update(max_edit_rate_ppm.to_string().as_bytes());
    digest.update([0]);
    digest.update(max_dp_cells.to_string().as_bytes());
    digest.update([0]);
    digest.update(max_exact_alignment_scan_bases.to_string().as_bytes());
    digest.update([0]);
    digest.update(max_junction_comparisons.to_string().as_bytes());
    digest.update([0]);
    digest.update(junction_evidence_mode.as_str().as_bytes());
    digest.update([0]);
    digest.update(max_junction_index_bytes.to_string().as_bytes());
    digest.update([0]);
    digest.update(max_junction_evidence_bytes.to_string().as_bytes());
    digest.update([0]);
    digest.update(max_truth_evidence_replay_bytes.to_string().as_bytes());
    format!("eval-{}", super::lower_hex(&digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_cursor_enforces_exact_columns_and_inclusive_row_cap() {
        let bytes = b"a\tb\n0\t1\n2\t3\n";
        let mut exact = TsvCursor::new(bytes, "test.tsv", "a\tb", 2, 2).unwrap();
        assert_eq!(exact.next::<2>().unwrap(), Some(["0", "1"]));
        assert_eq!(exact.next::<2>().unwrap(), Some(["2", "3"]));
        assert_eq!(exact.next::<2>().unwrap(), None);

        let mut below = TsvCursor::new(bytes, "test.tsv", "a\tb", 2, 1).unwrap();
        assert_eq!(below.next::<2>().unwrap(), Some(["0", "1"]));
        assert!(matches!(
            below.next::<2>(),
            Err(ValidationError::Resource(_))
        ));

        let malformed = b"a\tb\n0\t1\textra\n";
        let mut malformed = TsvCursor::new(malformed, "test.tsv", "a\tb", 2, 1).unwrap();
        assert!(matches!(
            malformed.next::<2>(),
            Err(ValidationError::Integrity(_))
        ));
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct OracleAlignmentScore {
        edits: u64,
        matches: u64,
        indels: u64,
        start: usize,
    }

    impl OracleAlignmentScore {
        fn sort_key(self) -> (u64, std::cmp::Reverse<u64>, u64, usize) {
            (
                self.edits,
                std::cmp::Reverse(self.matches),
                self.indels,
                self.start,
            )
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enumerate_fitting_paths(
        query: &[u8],
        reference: &[u8],
        query_position: usize,
        reference_position: usize,
        score: OracleAlignmentScore,
        best: &mut Option<OracleAlignmentScore>,
    ) {
        if query_position == query.len() {
            if reference_position != 0 {
                if best.is_none_or(|current| score.sort_key() < current.sort_key()) {
                    *best = Some(score);
                }
            } else if reference_position < reference.len() {
                // The implementation excludes endpoint column zero. A final
                // deletion is the only way an all-insertion path can reach an
                // admitted endpoint there.
                enumerate_fitting_paths(
                    query,
                    reference,
                    query_position,
                    reference_position + 1,
                    OracleAlignmentScore {
                        edits: score.edits + 1,
                        indels: score.indels + 1,
                        ..score
                    },
                    best,
                );
            }
            return;
        }

        enumerate_fitting_paths(
            query,
            reference,
            query_position + 1,
            reference_position,
            OracleAlignmentScore {
                edits: score.edits + 1,
                indels: score.indels + 1,
                ..score
            },
            best,
        );
        if reference_position == reference.len() {
            return;
        }

        let is_match = query[query_position] == reference[reference_position];
        enumerate_fitting_paths(
            query,
            reference,
            query_position + 1,
            reference_position + 1,
            OracleAlignmentScore {
                edits: score.edits + u64::from(!is_match),
                matches: score.matches + u64::from(is_match),
                ..score
            },
            best,
        );
        if query_position != 0 {
            // Row zero is initialized independently at every legal start;
            // it does not contain charged deletion transitions.
            enumerate_fitting_paths(
                query,
                reference,
                query_position,
                reference_position + 1,
                OracleAlignmentScore {
                    edits: score.edits + 1,
                    indels: score.indels + 1,
                    ..score
                },
                best,
            );
        }
    }

    fn exhaustive_fitting_oracle(
        query: &[u8],
        truth: &TruthMolecule,
        strand: Strand,
    ) -> OracleAlignmentScore {
        let oriented = if strand == Strand::Forward {
            truth.sequence.clone()
        } else {
            reverse_complement(&truth.sequence)
        };
        let reference_length = if truth.topology == TruthTopology::Circular {
            truth.sequence.len() + query.len().saturating_sub(1)
        } else {
            truth.sequence.len()
        };
        let reference = (0..reference_length)
            .map(|index| oriented[index % oriented.len()])
            .collect::<Vec<_>>();
        let mut best = None;
        for start in 0..truth.sequence.len() {
            enumerate_fitting_paths(
                query,
                &reference,
                0,
                start,
                OracleAlignmentScore {
                    edits: 0,
                    matches: 0,
                    indels: 0,
                    start,
                },
                &mut best,
            );
        }
        best.expect("nonempty query and truth have at least one fitting path")
    }

    fn truth(sequence: &[u8], topology: TruthTopology) -> TruthMolecule {
        TruthMolecule {
            id: "mol".to_owned(),
            class: TruthClass::Primary,
            topology,
            sequence: sequence.to_vec(),
        }
    }

    fn compatible_recovery(
        assemblies: &[FastaRecord],
        truths: &[TruthMolecule],
    ) -> RecoveryCoverage {
        let mut planning_scan = 0;
        let plans = assemblies
            .iter()
            .map(|assembly| {
                exact_alignment_plan(&assembly.sequence, truths, &mut planning_scan, u64::MAX)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let rows = assemblies
            .iter()
            .zip(plans)
            .map(|(assembly, plan)| {
                align_contig(assembly, truths, 150_000, 1_000_000, plan).unwrap()
            })
            .collect::<Vec<_>>();
        evaluate_compatible_recovery(assemblies, &rows, truths, &mut planning_scan, u64::MAX)
            .unwrap()
    }

    fn exact_rows(assemblies: &[FastaRecord], truths: &[TruthMolecule]) -> Vec<AlignmentRow> {
        let mut planning_scan = 0;
        assemblies
            .iter()
            .map(|assembly| {
                let plan =
                    exact_alignment_plan(&assembly.sequence, truths, &mut planning_scan, u64::MAX)
                        .unwrap();
                align_contig(assembly, truths, 150_000, 1_000_000, plan).unwrap()
            })
            .collect()
    }

    fn brute_compatible_recovery(
        assemblies: &[FastaRecord],
        truths: &[TruthMolecule],
    ) -> RecoveryCoverage {
        let mut molecules = truths
            .iter()
            .map(|truth| MoleculeCoverageMasks {
                unique: vec![false; truth.sequence.len()],
                lower: vec![false; truth.sequence.len()],
                upper: vec![false; truth.sequence.len()],
            })
            .collect::<Vec<_>>();
        let mut exact_unambiguous_records = 0_u64;
        let mut exact_ambiguous_records = 0_u64;
        for assembly in assemblies {
            let placements = exact_placements(&assembly.sequence, truths);
            assert!(!placements.is_empty());
            let mut first: Option<(usize, BTreeSet<usize>)> = None;
            let mut intersection_molecule = None;
            let mut intersection = BTreeSet::new();
            let mut distinct = false;
            for placement in placements {
                let strand = if placement.strand_rank == 0 {
                    Strand::Forward
                } else {
                    Strand::Reverse
                };
                let truth = &truths[placement.molecule_index];
                let coordinates = (0..assembly.sequence.len())
                    .map(|offset| forward_coordinate(placement.start + offset, truth, strand))
                    .collect::<BTreeSet<_>>();
                for &coordinate in &coordinates {
                    molecules[placement.molecule_index].upper[coordinate] = true;
                }
                match &first {
                    None => {
                        first = Some((placement.molecule_index, coordinates.clone()));
                        intersection_molecule = Some(placement.molecule_index);
                        intersection = coordinates;
                    }
                    Some((first_molecule, first_coordinates)) => {
                        if *first_molecule != placement.molecule_index
                            || first_coordinates != &coordinates
                        {
                            distinct = true;
                        }
                        if intersection_molecule == Some(placement.molecule_index) {
                            intersection.retain(|coordinate| coordinates.contains(coordinate));
                        } else {
                            intersection.clear();
                            intersection_molecule = None;
                        }
                    }
                }
            }
            let (first_molecule, first_coordinates) = first.unwrap();
            if distinct {
                exact_ambiguous_records += 1;
            } else {
                exact_unambiguous_records += 1;
                for coordinate in first_coordinates {
                    molecules[first_molecule].unique[coordinate] = true;
                }
            }
            if let Some(molecule_index) = intersection_molecule {
                for coordinate in &intersection {
                    molecules[molecule_index].lower[*coordinate] = true;
                }
            }
        }
        RecoveryCoverage {
            molecules,
            exact_unambiguous_records,
            exact_ambiguous_records,
            non_exact_records: 0,
        }
    }

    fn coverage_counts(
        recovery: &RecoveryCoverage,
        truths: &[TruthMolecule],
    ) -> Vec<(String, TruthClass, u64, u64, u64)> {
        let mut counts = truths
            .iter()
            .zip(&recovery.molecules)
            .map(|(truth, masks)| {
                (
                    truth.id.clone(),
                    truth.class,
                    mask_count(&masks.unique, "test unique").unwrap(),
                    mask_count(&masks.lower, "test lower").unwrap(),
                    mask_count(&masks.upper, "test upper").unwrap(),
                )
            })
            .collect::<Vec<_>>();
        counts.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        counts
    }

    fn recovery_metric_signature(
        recovery: &RecoveryCoverage,
        truths: &[TruthMolecule],
        diagnostic_matching_bases: u64,
    ) -> Vec<u8> {
        let class_counts = [TruthClass::Primary, TruthClass::Minor]
            .into_iter()
            .map(|class| {
                let mut truth_bases = 0u64;
                let mut unique = 0u64;
                let mut lower = 0u64;
                let mut upper = 0u64;
                for (truth, masks) in truths.iter().zip(&recovery.molecules) {
                    if truth.class == class {
                        truth_bases += u64::try_from(truth.sequence.len()).unwrap();
                        unique += mask_count(&masks.unique, "test class unique").unwrap();
                        lower += mask_count(&masks.lower, "test class lower").unwrap();
                        upper += mask_count(&masks.upper, "test class upper").unwrap();
                    }
                }
                (class, truth_bases, unique, lower, upper)
            })
            .collect::<Vec<_>>();
        let (truth_bases, unique, lower, upper) = class_counts.iter().fold(
            (0u64, 0u64, 0u64, 0u64),
            |totals, (_, truth, unique, lower, upper)| {
                (
                    totals.0 + truth,
                    totals.1 + unique,
                    totals.2 + lower,
                    totals.3 + upper,
                )
            },
        );
        let status = if recovery.non_exact_records == 0 {
            "complete_exact_compatible_placement_universe"
        } else {
            "not_available_non_exact_compatible_placement_universe"
        };
        serde_json::to_vec(&(
            recovery.exact_unambiguous_records,
            recovery.exact_ambiguous_records,
            recovery.non_exact_records,
            coverage_counts(recovery, truths),
            class_counts,
            (
                truth_bases,
                unique,
                lower,
                upper,
                availability_ratio(unique, truth_bases, status).unwrap(),
                availability_ratio(lower, truth_bases, status).unwrap(),
                availability_ratio(upper, truth_bases, status).unwrap(),
                duplication_ratio(
                    diagnostic_matching_bases,
                    upper,
                    recovery.exact_ambiguous_records,
                    recovery.non_exact_records,
                )
                .unwrap(),
            ),
        ))
        .unwrap()
    }

    fn placements(needle: &[u8], truths: &[TruthMolecule]) -> BTreeSet<ExactPlacement> {
        exact_placements(needle, truths)
    }

    fn sequence_from_code(mut code: usize, length: usize) -> Vec<u8> {
        let mut sequence = vec![b'A'; length];
        for base in sequence.iter_mut().rev() {
            *base = match code & 3 {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                _ => b'T',
            };
            code >>= 2;
        }
        sequence
    }

    #[test]
    fn evidence_identifiers_exclude_delimiter_and_control_ambiguity() {
        for accepted in ["A", "mol-1", "sample_2.segment"] {
            assert!(valid_evidence_identifier(accepted));
        }
        for rejected in ["", "mol:1", "mol 1", "mol\t1", "møl"] {
            assert!(!valid_evidence_identifier(rejected));
        }
    }

    fn indexed_placements(needle: &[u8], truths: &[TruthMolecule]) -> PlacementSummary {
        let count = occurrence_count(truths, needle.len()).unwrap();
        let index = PackedWindowIndex::build(truths, needle.len(), count).unwrap();
        let mut work = JunctionWork::default();
        index
            .lookup(encode_exact_window(needle), &mut work, u64::MAX)
            .unwrap()
    }

    #[test]
    fn fitting_alignment_counts_substitution_insertion_and_deletion() {
        let reference = truth(b"AACCGGTT", TruthTopology::Linear);
        let substitution =
            fitting_alignment(b"AACTGGTT", &reference, 0, Strand::Forward, 1_000).unwrap();
        assert_eq!(substitution.mismatches, 1);
        assert_eq!(substitution.edits, 1);

        let insertion =
            fitting_alignment(b"AACCTGGTT", &reference, 0, Strand::Forward, 1_000).unwrap();
        assert_eq!(insertion.insertions, 1);
        assert_eq!(insertion.edits, 1);

        let deletion_reference = truth(b"ACGTACGT", TruthTopology::Linear);
        let deletion =
            fitting_alignment(b"ACGACGT", &deletion_reference, 0, Strand::Forward, 1_000).unwrap();
        assert_eq!(deletion.deletions, 1);
        assert_eq!(deletion.edits, 1);
    }

    #[test]
    fn fitting_alignment_optimizes_published_ties_before_molecule_order() {
        let mut first = truth(b"AC", TruthTopology::Linear);
        first.id = "first".to_owned();
        let mut second = truth(b"CA", TruthTopology::Linear);
        second.id = "second".to_owned();
        let contig = FastaRecord {
            id: "query".to_owned(),
            sequence: b"AA".to_vec(),
        };

        let first_candidate =
            fitting_alignment(&contig.sequence, &first, 0, Strand::Forward, 100).unwrap();
        assert_eq!(cigar(&first_candidate.operations), "1=1X");
        assert_eq!(first_candidate.edits, 1);
        assert_eq!(first_candidate.matches, 1);
        assert_eq!(first_candidate.insertions + first_candidate.deletions, 0);
        assert_eq!(first_candidate.oriented_reference_start, 0);

        let row = align_contig(&contig, &[first, second], 1_000_000, 100, None).unwrap();
        let selected = row.candidate.unwrap();
        assert_eq!(selected.molecule_index, 0);
        assert_eq!(selected.mismatches, 1);
        assert_eq!(selected.insertions, 0);
        assert_eq!(selected.deletions, 0);
        assert_eq!(row.equally_best_targets, 2);
    }

    #[test]
    fn fitting_alignment_matches_exhaustive_full_score_oracle() {
        let sequences = (1..=3)
            .flat_map(|length| {
                (0..1usize << length).map(move |code| {
                    (0..length)
                        .rev()
                        .map(|shift| if code & (1 << shift) == 0 { b'A' } else { b'C' })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        for topology in [TruthTopology::Linear, TruthTopology::Circular] {
            for truth_sequence in &sequences {
                let reference = truth(truth_sequence, topology);
                for query in &sequences {
                    for strand in [Strand::Forward, Strand::Reverse] {
                        let expected = exhaustive_fitting_oracle(query, &reference, strand);
                        let actual =
                            fitting_alignment(query, &reference, 0, strand, 10_000).unwrap();
                        assert_eq!(
                            OracleAlignmentScore {
                                edits: actual.edits,
                                matches: actual.matches,
                                indels: actual.insertions + actual.deletions,
                                start: actual.oriented_reference_start,
                            },
                            expected,
                            "topology={topology:?} strand={strand:?} truth={truth_sequence:?} query={query:?} cigar={}",
                            cigar(&actual.operations)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn circular_alignment_accepts_rotation_and_seam() {
        let reference = truth(b"AACCGGTT", TruthTopology::Circular);
        let candidate =
            fitting_alignment(b"GGTTAACC", &reference, 0, Strand::Forward, 1_000).unwrap();
        assert_eq!(candidate.edits, 0);
        assert_eq!(candidate.oriented_reference_start, 4);
        let placements = placements(b"GTTA", &[reference]);
        assert!(!placements.is_empty());
    }

    #[test]
    fn circularity_is_not_assumed_for_linear_truth() {
        let circular = truth(b"AACCGGTT", TruthTopology::Circular);
        let linear = truth(b"AACCGGTT", TruthTopology::Linear);
        assert!(!placements(b"GGTTAACC", &[circular]).is_empty());
        assert!(placements(b"GGTTAACC", &[linear]).is_empty());
    }

    #[test]
    fn repetitive_truth_is_counted_without_materializing_lookup_results() {
        let sequence = [b'A'; 32];
        let truths = [truth(&sequence, TruthTopology::Linear)];
        let indexed = indexed_placements(b"A", &truths);
        assert_eq!(indexed.count, 32);
        assert_eq!(indexed.unique, None);
    }

    #[test]
    fn duplicate_truth_self_assembly_has_complete_upper_recovery_and_na_duplication() {
        let mut first = truth(b"AACCGTTA", TruthTopology::Linear);
        first.id = "copy-a".to_owned();
        let mut second = truth(b"AACCGTTA", TruthTopology::Linear);
        second.id = "copy-b".to_owned();
        second.class = TruthClass::Minor;
        let truths = [first, second];
        let assemblies = [
            FastaRecord {
                id: "copy-a".to_owned(),
                sequence: b"AACCGTTA".to_vec(),
            },
            FastaRecord {
                id: "copy-b".to_owned(),
                sequence: b"AACCGTTA".to_vec(),
            },
        ];
        let recovery = compatible_recovery(&assemblies, &truths);
        assert_eq!(recovery.exact_unambiguous_records, 0);
        assert_eq!(recovery.exact_ambiguous_records, 2);
        assert_eq!(
            coverage_counts(&recovery, &truths),
            vec![
                ("copy-a".to_owned(), TruthClass::Primary, 0, 0, 8),
                ("copy-b".to_owned(), TruthClass::Minor, 0, 0, 8),
            ]
        );
        let duplication = duplication_ratio(16, 16, 2, 0).unwrap();
        assert_eq!(duplication.value_decimal, None);
        assert_eq!(
            duplication.status,
            "not_available_ambiguous_exact_compatible_placements"
        );
    }

    #[test]
    fn repeated_starts_produce_compatible_bounds_not_first_start_coverage() {
        let truths = [truth(b"AAAAA", TruthTopology::Linear)];
        let assemblies = [FastaRecord {
            id: "repeat".to_owned(),
            sequence: b"AAA".to_vec(),
        }];
        let recovery = compatible_recovery(&assemblies, &truths);
        assert_eq!(recovery.exact_ambiguous_records, 1);
        assert_eq!(
            coverage_counts(&recovery, &truths),
            vec![("mol".to_owned(), TruthClass::Primary, 0, 1, 5)]
        );
    }

    #[test]
    fn interval_recovery_matches_brute_coordinate_materialization_exhaustively() {
        for topology in [TruthTopology::Linear, TruthTopology::Circular] {
            for truth_length in 1..=4 {
                for encoded in 0..4_usize.pow(truth_length as u32) {
                    let sequence = sequence_from_code(encoded, truth_length);
                    let truth = truth(&sequence, topology);
                    let queries = match topology {
                        TruthTopology::Linear => (0..truth_length)
                            .flat_map(|start| {
                                (start + 1..=truth_length)
                                    .map(|end| sequence[start..end].to_vec())
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>(),
                        TruthTopology::Circular => (0..truth_length)
                            .flat_map(|start| {
                                (1..=truth_length + 2)
                                    .map(|length| {
                                        (0..length)
                                            .map(|offset| sequence[(start + offset) % truth_length])
                                            .collect::<Vec<_>>()
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>(),
                    };
                    for query in queries {
                        let assemblies = [FastaRecord {
                            id: "query".to_owned(),
                            sequence: query,
                        }];
                        assert_eq!(
                            compatible_recovery(&assemblies, std::slice::from_ref(&truth)),
                            brute_compatible_recovery(&assemblies, std::slice::from_ref(&truth)),
                            "topology={topology:?} truth={sequence:?} query={:?}",
                            assemblies[0].sequence
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn homopolymer_compatible_recovery_work_is_linear_in_scanned_truth() {
        let truth_length = 200_000_usize;
        let query_length = truth_length / 2;
        let truths = [truth(&vec![b'A'; truth_length], TruthTopology::Linear)];
        let assemblies = [FastaRecord {
            id: "homopolymer".to_owned(),
            sequence: vec![b'A'; query_length],
        }];
        let rows = exact_rows(&assemblies, &truths);
        let mut scan_bases = 0_u64;
        let (recovery, work) = evaluate_compatible_recovery_with_work(
            &assemblies,
            &rows,
            &truths,
            &mut scan_bases,
            u64::MAX,
        )
        .unwrap();

        assert_eq!(
            coverage_counts(&recovery, &truths),
            vec![(
                "mol".to_owned(),
                TruthClass::Primary,
                0,
                0,
                u64::try_from(truth_length).unwrap(),
            )]
        );
        assert_eq!(recovery.exact_unambiguous_records, 0);
        assert_eq!(recovery.exact_ambiguous_records, 1);
        assert_eq!(
            work.placement_visits,
            u64::try_from(truth_length - query_length + 1).unwrap()
        );
        assert_eq!(
            work.coordinate_mask_operations,
            u64::try_from(truth_length).unwrap()
        );
        assert_eq!(scan_bases, u64::try_from(truth_length * 2).unwrap());
        assert!(work.coordinate_mask_operations <= scan_bases);
    }

    #[test]
    fn truth_order_permutation_preserves_aggregate_and_copy_bound_metrics() {
        let mut primary = truth(b"AACCGTTA", TruthTopology::Linear);
        primary.id = "primary".to_owned();
        let mut minor = truth(b"AACCGTTA", TruthTopology::Linear);
        minor.id = "minor".to_owned();
        minor.class = TruthClass::Minor;
        let assemblies = [FastaRecord {
            id: "query".to_owned(),
            sequence: b"AACCGTTA".to_vec(),
        }];
        let forward_truths = [primary, minor];
        let mut reverse_truths = [
            truth(b"AACCGTTA", TruthTopology::Linear),
            truth(b"AACCGTTA", TruthTopology::Linear),
        ];
        reverse_truths[0].id = "minor".to_owned();
        reverse_truths[0].class = TruthClass::Minor;
        reverse_truths[1].id = "primary".to_owned();
        let forward = compatible_recovery(&assemblies, &forward_truths);
        let reverse = compatible_recovery(&assemblies, &reverse_truths);
        assert_eq!(
            recovery_metric_signature(&forward, &forward_truths, 8),
            recovery_metric_signature(&reverse, &reverse_truths, 8)
        );
    }

    #[test]
    fn packed_occurrence_index_matches_exhaustive_oracle() {
        for topology in [TruthTopology::Linear, TruthTopology::Circular] {
            for truth_code in 0..256 {
                let sequence = sequence_from_code(truth_code, 4);
                let truths = [truth(&sequence, topology)];
                for needle_length in 1..=3 {
                    for needle_code in 0..(1usize << (2 * needle_length)) {
                        let needle = sequence_from_code(needle_code, needle_length);
                        let exhaustive = placements(&needle, &truths);
                        let indexed = indexed_placements(&needle, &truths);
                        assert_eq!(
                            indexed.count,
                            u64::try_from(exhaustive.len()).unwrap(),
                            "topology={topology:?} truth={sequence:?} needle={needle:?}"
                        );
                        assert_eq!(
                            indexed.unique,
                            (exhaustive.len() == 1).then(|| *exhaustive.iter().next().unwrap()),
                            "topology={topology:?} truth={sequence:?} needle={needle:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn packed_occurrence_index_matches_longer_circular_queries() {
        let truths = [truth(b"ACG", TruthTopology::Circular)];
        for needle in [b"ACGAC".as_slice(), b"CGACGA", b"TTTT"] {
            let exhaustive = placements(needle, &truths);
            let indexed = indexed_placements(needle, &truths);
            assert_eq!(indexed.count, u64::try_from(exhaustive.len()).unwrap());
            assert_eq!(
                indexed.unique,
                (exhaustive.len() == 1).then(|| *exhaustive.iter().next().unwrap())
            );
        }
    }

    #[test]
    fn exact_alignment_plan_preserves_target_ties_and_circular_start() {
        let truths = [
            truth(b"AACCGGTT", TruthTopology::Circular),
            truth(b"GGTTAACC", TruthTopology::Linear),
        ];
        let mut scan_bases = 0;
        let plan = exact_alignment_plan(b"GGTTAACC", &truths, &mut scan_bases, u64::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(plan.molecule_index, 0);
        assert_eq!(plan.strand, Strand::Forward);
        assert_eq!(plan.oriented_reference_start, 4);
        assert!(plan.equally_best_targets >= 2);
    }

    #[test]
    fn exact_alignment_prefix_capacity_enforces_the_inclusive_byte_boundary() {
        let element_bytes = u64::try_from(std::mem::size_of::<usize>()).unwrap();
        let boundary = usize::try_from(MAX_EXACT_ALIGNMENT_PREFIX_BYTES / element_bytes).unwrap();
        assert_eq!(
            exact_alignment_prefix_capacity_bytes(boundary).unwrap(),
            MAX_EXACT_ALIGNMENT_PREFIX_BYTES
        );
        assert!(matches!(
            exact_alignment_prefix_capacity_bytes(boundary + 1),
            Err(ValidationError::Resource(_))
        ));
    }

    #[test]
    fn empty_denominators_are_explicitly_not_available() {
        let metric = ratio(0, 0, "not_available").unwrap();
        assert_eq!(metric.value_decimal, None);
        let qv = qv(0, 0);
        assert_eq!(qv.value_decimal, None);
        assert_eq!(qv.status, "not_available_no_aligned_bases");
    }

    #[test]
    fn qv_decimal_rounding_is_integer_defined_at_binary_float_boundaries() {
        for (errors, columns, expected) in [
            (919_999, 1_209_421, "1.187902"),
            (690_886, 1_997_595, "4.611011"),
            (281_222, 1_626_003, "7.620720"),
            (1, 2, "3.010300"),
        ] {
            let metric = qv(errors, columns);
            assert_eq!(metric.value_decimal.as_deref(), Some(expected));
            assert_eq!(metric.status, "measured");
        }
        assert_eq!(qv(0, 1).value_decimal.as_deref(), Some("3.010300"));
        assert_eq!(qv(0, u64::MAX).value_decimal.as_deref(), Some("192.659197"));
    }
}
