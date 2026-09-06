use super::{
    commit_staging, decimal_u64, decode_hex_32, lower_hex, sha256_bytes, sha256_file,
    staging_directory, write_checksum_manifest, write_fasta, write_json, FastaRecord, TruthClass,
    TruthTopology, ValidationError, ValidationResult, VALIDATION_COORDINATE_SYSTEM,
};
use flate2::{read::MultiGzDecoder, write::GzEncoder, Compression, GzBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const PLAN_VERSION: &str = "veritasm-validation-v1";
const GENERATOR_VERSION: &str = "4";
const RNG_NAME: &str = "sha256-counter-v1";
const RNG_STREAM_DERIVATION: &str = "sha256-domain-separated-stream-v1";
const DEFAULT_QUALITY_PHRED: u8 = 40;
const LOW_QUALITY_PHRED: u8 = 10;
const MAX_GENERATED_FRAGMENTS: u64 = 100_000;
const MAX_GENERATED_TRUTH_LENGTH: usize = 2_000_000;
const MAX_GENERATED_READ_LENGTH: usize = 10_000;
const MAX_GENERATED_READ_BASES: u64 = 25_000_000;
const MAX_GENERATED_OUTPUT_ESTIMATE_BYTES: u64 = 256 * 1024 * 1024;
const ESTIMATED_FASTQ_BYTES_PER_READ: u64 = 64;
const ESTIMATED_ORIGIN_BYTES_PER_READ: u64 = 512;
const ESTIMATED_ERROR_BYTES_PER_EVENT: u64 = 256;
const ESTIMATED_QUALITY_EVENT_BYTES_PER_EVENT: u64 = 192;
const ESTIMATED_FIXED_OUTPUT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GenerationSize {
    read_count: u64,
}

/// Built-in generator cases in the first executable validation slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GeneratorCase {
    LinearSe,
    LinearPe,
    CircularPe,
    MixturePe,
    ErrorPe,
    QcCensoringControl,
}

impl GeneratorCase {
    /// Stable case identifier used in seed derivation and manifests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LinearSe => "linear-se",
            Self::LinearPe => "linear-pe",
            Self::CircularPe => "circular-pe",
            Self::MixturePe => "mixture-pe",
            Self::ErrorPe => "error-pe",
            Self::QcCensoringControl => "qc-censoring-control",
        }
    }

    fn paired(self) -> bool {
        !matches!(self, Self::LinearSe)
    }

    fn circular(self) -> bool {
        matches!(self, Self::CircularPe)
    }

    fn quality_model(self) -> &'static str {
        if matches!(self, Self::QcCensoringControl) {
            "qc_censoring_substitution_q10_else_q40_v1"
        } else {
            "independent_bernoulli_q10_else_q40_v1"
        }
    }
}

impl fmt::Display for GeneratorCase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for GeneratorCase {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "linear-se" => Ok(Self::LinearSe),
            "linear-pe" => Ok(Self::LinearPe),
            "circular-pe" => Ok(Self::CircularPe),
            "mixture-pe" => Ok(Self::MixturePe),
            "error-pe" => Ok(Self::ErrorPe),
            "qc-censoring-control" => Ok(Self::QcCensoringControl),
            _ => Err(ValidationError::Configuration(format!(
                "unknown generator case {value:?}; expected linear-se, linear-pe, circular-pe, mixture-pe, error-pe, or qc-censoring-control"
            ))),
        }
    }
}

/// Complete deterministic 256-bit simulator seed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Seed256([u8; 32]);

impl Seed256 {
    /// Parses the stable lowercase hexadecimal representation.
    pub fn from_hex(value: &str) -> ValidationResult<Self> {
        Ok(Self(decode_hex_32(value)?))
    }

    /// Returns all 256 seed bits as lowercase hexadecimal.
    pub fn to_hex(self) -> String {
        lower_hex(&self.0)
    }
}

impl fmt::Debug for Seed256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Seed256")
            .field(&self.to_hex())
            .finish()
    }
}

/// Derives the pre-registered seed for a case and replicate.
pub fn derive_seed(case: GeneratorCase, replicate: u32) -> Seed256 {
    let mut digest = Sha256::new();
    digest.update(PLAN_VERSION.as_bytes());
    digest.update([0]);
    digest.update(case.as_str().as_bytes());
    digest.update([0]);
    digest.update(replicate.to_string().as_bytes());
    Seed256(digest.finalize().into())
}

fn derive_stream_seed(seed: Seed256, stream: &[u8]) -> Seed256 {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:validation-rng-stream:v1\0");
    digest.update(seed.0);
    digest.update(stream);
    Seed256(digest.finalize().into())
}

/// Configuration for one deterministic generated dataset.
#[derive(Debug, Clone)]
pub struct GeneratorConfig {
    pub output_directory: PathBuf,
    pub case: GeneratorCase,
    pub replicate: u32,
    pub explicit_seed: Option<Seed256>,
    pub fragments: u64,
    pub truth_length: usize,
    pub read_length: usize,
    pub insert_length: usize,
    pub substitution_rate_ppm: u32,
    pub low_quality_rate_ppm: u32,
    pub explicit_quality_seed: Option<Seed256>,
    pub gzip: bool,
}

impl GeneratorConfig {
    /// Constructs a small default case suitable for development validation.
    pub fn new(output_directory: PathBuf, case: GeneratorCase) -> Self {
        Self {
            output_directory,
            case,
            replicate: 0,
            explicit_seed: None,
            fragments: 200,
            truth_length: 2_000,
            read_length: 100,
            insert_length: 250,
            substitution_rate_ppm: if matches!(
                case,
                GeneratorCase::ErrorPe | GeneratorCase::QcCensoringControl
            ) {
                20_000
            } else {
                0
            },
            low_quality_rate_ppm: if matches!(case, GeneratorCase::ErrorPe) {
                20_000
            } else {
                0
            },
            explicit_quality_seed: None,
            gzip: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    pub path: String,
    pub role: String,
    pub visibility: String,
    pub bytes_decimal: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TruthMoleculeRecord {
    pub molecule_id: String,
    pub truth_class: TruthClass,
    pub topology: TruthTopology,
    pub length_bases_decimal: String,
    pub sequence_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorRecord {
    pub name: String,
    pub version: String,
    pub plan_version: String,
    pub rng_name: String,
    pub rng_byte_order: String,
    pub seed_hex: String,
    pub seed_source: String,
    pub rng_streams: RngStreamsRecord,
    pub parameters: GeneratorParameters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RngStreamsRecord {
    pub derivation: String,
    pub layout_seed_hex: String,
    pub error_seed_hex: String,
    pub quality_seed_hex: String,
    pub quality_seed_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorParameters {
    pub fragments_decimal: String,
    pub truth_length_decimal: String,
    pub read_length_decimal: String,
    pub insert_length_decimal: String,
    pub substitution_rate_ppm_decimal: String,
    pub low_quality_rate_ppm_decimal: String,
    pub error_model: String,
    pub quality_model: String,
    pub default_quality_phred_decimal: String,
    pub low_quality_phred_decimal: String,
    pub mixture_minor_fragment_fraction: String,
    pub library_orientation: String,
    pub output_compression: String,
}

/// Canonical, generator-feasible interpretation of the version-4 parameter record.
///
/// Both generation and evaluation use this validator. This prevents an evaluator from admitting
/// a declared version-4 tuple that the generator would reject before consuming RNG state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ValidatedGeneratorParameters {
    pub fragments: u64,
    pub truth_length: usize,
    pub read_length: usize,
    pub insert_length: usize,
    pub substitution_rate_ppm: u32,
    pub low_quality_rate_ppm: u32,
    pub paired: bool,
    pub read_count: u64,
}

pub(super) fn validate_generator_parameters(
    case: GeneratorCase,
    parameters: &GeneratorParameters,
) -> ValidationResult<ValidatedGeneratorParameters> {
    let fragments = decimal_u64(&parameters.fragments_decimal, "generator fragments_decimal")?;
    let truth_length_u64 = decimal_u64(
        &parameters.truth_length_decimal,
        "generator truth_length_decimal",
    )?;
    let read_length_u64 = decimal_u64(
        &parameters.read_length_decimal,
        "generator read_length_decimal",
    )?;
    let insert_length_u64 = decimal_u64(
        &parameters.insert_length_decimal,
        "generator insert_length_decimal",
    )?;
    let substitution_rate_u64 = decimal_u64(
        &parameters.substitution_rate_ppm_decimal,
        "generator substitution_rate_ppm_decimal",
    )?;
    let low_quality_rate_u64 = decimal_u64(
        &parameters.low_quality_rate_ppm_decimal,
        "generator low_quality_rate_ppm_decimal",
    )?;

    if parameters.error_model != "independent_substitution_bernoulli_per_observed_base_v1"
        || parameters.default_quality_phred_decimal != "40"
        || parameters.low_quality_phred_decimal != "10"
    {
        return Err(ValidationError::Integrity(
            "generator error or quality value model differs from version 4".to_owned(),
        ));
    }
    if !matches!(parameters.output_compression.as_str(), "plain" | "gzip") {
        return Err(ValidationError::Integrity(
            "generator output compression differs from version 4".to_owned(),
        ));
    }
    let expected_quality_model = if matches!(case, GeneratorCase::QcCensoringControl) {
        "qc_censoring_substitution_q10_else_q40_v1"
    } else {
        "independent_bernoulli_q10_else_q40_v1"
    };
    let expected_fraction = if matches!(case, GeneratorCase::MixturePe) {
        "0.1_exact_count_rounded_down"
    } else {
        "not_applicable"
    };
    let expected_orientation = if case.paired() {
        "inward_FR_with_random_truth_strand"
    } else {
        "not_applicable"
    };
    if parameters.quality_model != expected_quality_model
        || parameters.mixture_minor_fragment_fraction != expected_fraction
        || parameters.library_orientation != expected_orientation
    {
        return Err(ValidationError::Integrity(
            "generator quality, mixture, or library model differs from the case contract"
                .to_owned(),
        ));
    }

    if fragments > MAX_GENERATED_FRAGMENTS {
        return Err(ValidationError::Resource(format!(
            "fragments exceeds generator cap {MAX_GENERATED_FRAGMENTS}"
        )));
    }
    if truth_length_u64 == 0 || read_length_u64 == 0 {
        return Err(ValidationError::Configuration(
            "truth length and read length must be positive".to_owned(),
        ));
    }
    if truth_length_u64 > MAX_GENERATED_TRUTH_LENGTH as u64 {
        return Err(ValidationError::Resource(format!(
            "truth length exceeds generator cap {MAX_GENERATED_TRUTH_LENGTH} bases"
        )));
    }
    if read_length_u64 > MAX_GENERATED_READ_LENGTH as u64 {
        return Err(ValidationError::Resource(format!(
            "read length exceeds generator cap {MAX_GENERATED_READ_LENGTH} bases"
        )));
    }
    if substitution_rate_u64 > 1_000_000 || low_quality_rate_u64 > 1_000_000 {
        return Err(ValidationError::Configuration(
            "substitution and low-quality rates must be at most 1000000 ppm".to_owned(),
        ));
    }
    if matches!(case, GeneratorCase::QcCensoringControl) && low_quality_rate_u64 != 0 {
        return Err(ValidationError::Configuration(
            "qc-censoring-control derives Q10 events from substitutions and requires low-quality-rate-ppm=0"
                .to_owned(),
        ));
    }

    let truth_length = usize::try_from(truth_length_u64)
        .map_err(|_| ValidationError::Resource("truth length exceeds usize".to_owned()))?;
    let read_length = usize::try_from(read_length_u64)
        .map_err(|_| ValidationError::Resource("read length exceeds usize".to_owned()))?;
    let insert_length = usize::try_from(insert_length_u64)
        .map_err(|_| ValidationError::Resource("insert length exceeds usize".to_owned()))?;
    let substitution_rate_ppm = u32::try_from(substitution_rate_u64)
        .map_err(|_| ValidationError::Resource("substitution rate exceeds u32".to_owned()))?;
    let low_quality_rate_ppm = u32::try_from(low_quality_rate_u64)
        .map_err(|_| ValidationError::Resource("low-quality rate exceeds u32".to_owned()))?;
    let span = if case.paired() {
        if insert_length < read_length {
            return Err(ValidationError::Configuration(
                "paired insert length must be at least read length".to_owned(),
            ));
        }
        insert_length
    } else {
        read_length
    };
    if span > truth_length {
        return Err(ValidationError::Configuration(
            "read or insert span exceeds truth length".to_owned(),
        ));
    }

    let read_count = generation_size_from_values(
        case,
        fragments,
        truth_length_u64,
        read_length_u64,
        substitution_rate_u64,
        low_quality_rate_u64,
    )?;
    Ok(ValidatedGeneratorParameters {
        fragments,
        truth_length,
        read_length,
        insert_length,
        substitution_rate_ppm,
        low_quality_rate_ppm,
        paired: case.paired(),
        read_count,
    })
}

/// Commits every generator parameter and RNG identity field to one unambiguous dataset ID.
///
/// Each field is encoded as a length-prefixed name followed by a length-prefixed value. The field
/// order below is part of generator version 4's frozen identity contract.
#[allow(clippy::too_many_arguments)]
pub(super) fn dataset_id_from_parameters(
    case_id: &str,
    replicate_decimal: &str,
    seed_hex: &str,
    seed_source: &str,
    layout_seed_hex: &str,
    error_seed_hex: &str,
    quality_seed_hex: &str,
    quality_seed_source: &str,
    parameters: &GeneratorParameters,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:validation-dataset-parameters:v4\0");
    for (name, value) in [
        ("case_id", case_id),
        ("replicate_decimal", replicate_decimal),
        ("generator_name", "veritasm-simulate"),
        ("generator_version", GENERATOR_VERSION),
        ("plan_version", PLAN_VERSION),
        ("rng_name", RNG_NAME),
        ("rng_byte_order", "counter_u64_le_output_words_u64_le"),
        ("seed_hex", seed_hex),
        ("seed_source", seed_source),
        ("rng_stream_derivation", RNG_STREAM_DERIVATION),
        ("layout_seed_hex", layout_seed_hex),
        ("error_seed_hex", error_seed_hex),
        ("quality_seed_hex", quality_seed_hex),
        ("quality_seed_source", quality_seed_source),
        ("fragments_decimal", &parameters.fragments_decimal),
        ("truth_length_decimal", &parameters.truth_length_decimal),
        ("read_length_decimal", &parameters.read_length_decimal),
        ("insert_length_decimal", &parameters.insert_length_decimal),
        (
            "substitution_rate_ppm_decimal",
            &parameters.substitution_rate_ppm_decimal,
        ),
        (
            "low_quality_rate_ppm_decimal",
            &parameters.low_quality_rate_ppm_decimal,
        ),
        ("error_model", &parameters.error_model),
        ("quality_model", &parameters.quality_model),
        (
            "default_quality_phred_decimal",
            &parameters.default_quality_phred_decimal,
        ),
        (
            "low_quality_phred_decimal",
            &parameters.low_quality_phred_decimal,
        ),
        (
            "mixture_minor_fragment_fraction",
            &parameters.mixture_minor_fragment_fraction,
        ),
        ("library_orientation", &parameters.library_orientation),
        ("output_compression", &parameters.output_compression),
    ] {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    format!("v4-{}", lower_hex(&digest.finalize()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblerInputRecord {
    pub mode: String,
    pub fragments_decimal: String,
    pub reads_decimal: String,
    pub manifest_path: String,
    pub read_paths: Vec<String>,
    pub contract: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TruthRecord {
    pub namespace: String,
    pub fasta_path: String,
    pub origins_path: String,
    pub errors_path: String,
    pub quality_events_path: String,
    pub coordinate_system: String,
    pub molecules: Vec<TruthMoleculeRecord>,
}

/// Dataset identity and checksums emitted by generator version 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetManifest {
    pub schema_version: String,
    pub dataset_id: String,
    pub tier: String,
    pub role: String,
    pub case_id: String,
    pub replicate_decimal: String,
    pub generator: GeneratorRecord,
    pub assembler_input: AssemblerInputRecord,
    pub evaluation_truth: TruthRecord,
    pub files: Vec<FileRecord>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InputOnlyManifest<'a> {
    schema_version: &'static str,
    dataset_id: &'a str,
    mode: &'a str,
    fragments_decimal: String,
    reads_decimal: String,
    read_files: Vec<InputFile<'a>>,
    truth_exclusion: &'static str,
}

#[derive(Debug, Serialize)]
struct InputFile<'a> {
    role: &'a str,
    path: &'a str,
    sha256: String,
}

struct OriginRow {
    fragment_ordinal: u64,
    read_id: String,
    mate_role: &'static str,
    molecule_id: String,
    topology: TruthTopology,
    strand: char,
    first_truth_base: usize,
    truth_step: i8,
    read_length: usize,
    outer_fragment_start: usize,
    outer_fragment_span: usize,
    wraps_origin: bool,
    pre_error_sha256: String,
    observed_sha256: String,
    quality_sha256: String,
}

struct ErrorRow {
    read_id: String,
    observed_offset: usize,
    truth_coordinate: usize,
    truth_base: u8,
    observed_base: u8,
}

struct QualityEventRow {
    read_id: String,
    observed_offset: usize,
    quality_phred: u8,
    event_kind: &'static str,
}

struct Molecule {
    id: String,
    class: TruthClass,
    topology: TruthTopology,
    sequence: Vec<u8>,
}

enum FastqOutput {
    Plain(BufWriter<File>),
    Gzip(GzEncoder<BufWriter<File>>),
}

impl FastqOutput {
    fn create(path: &Path, gzip: bool) -> ValidationResult<Self> {
        let writer = BufWriter::new(File::create(path)?);
        if gzip {
            Ok(Self::Gzip(
                GzBuilder::new().mtime(0).write(writer, Compression::fast()),
            ))
        } else {
            Ok(Self::Plain(writer))
        }
    }

    fn finish(self) -> ValidationResult<()> {
        match self {
            Self::Plain(mut writer) => writer.flush()?,
            Self::Gzip(writer) => writer.finish()?.flush()?,
        }
        Ok(())
    }
}

impl Write for FastqOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(writer) => writer.write(bytes),
            Self::Gzip(writer) => writer.write(bytes),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(writer) => writer.flush(),
            Self::Gzip(writer) => writer.flush(),
        }
    }
}

/// Generates one complete, checksum-bearing dataset in a new directory.
pub fn generate_dataset(config: &GeneratorConfig) -> ValidationResult<DatasetManifest> {
    let parameters = parameters_from_config(config);
    let generation_size = validate_config(config)?;
    let seed = config
        .explicit_seed
        .unwrap_or_else(|| derive_seed(config.case, config.replicate));
    let seed_hex = seed.to_hex();
    let layout_seed = derive_stream_seed(seed, b"layout");
    let error_seed = derive_stream_seed(seed, b"error");
    let quality_seed = config
        .explicit_quality_seed
        .unwrap_or_else(|| derive_stream_seed(seed, b"quality"));
    let quality_seed_hex = quality_seed.to_hex();
    let seed_source = if config.explicit_seed.is_some() {
        "explicit"
    } else {
        "sha256_plan_case_replicate"
    };
    let quality_seed_source = if config.explicit_quality_seed.is_some() {
        "explicit"
    } else {
        "derived_from_master_seed"
    };
    let dataset_id = dataset_id_from_parameters(
        config.case.as_str(),
        &config.replicate.to_string(),
        &seed_hex,
        seed_source,
        &layout_seed.to_hex(),
        &error_seed.to_hex(),
        &quality_seed_hex,
        quality_seed_source,
        &parameters,
    );
    let staging = staging_directory(&config.output_directory, ".veritasm-simulate-")?;
    let root = staging.path();
    fs::create_dir(root.join("assembler_input"))?;
    fs::create_dir(root.join("evaluation_truth"))?;

    let mut layout_rng = CounterRng::new(layout_seed);
    let mut error_rng = CounterRng::new(error_seed);
    let mut quality_rng = CounterRng::new(quality_seed);
    let molecules = build_molecules(config, &mut layout_rng)?;
    let mut truth_records = Vec::new();
    truth_records
        .try_reserve_exact(molecules.len())
        .map_err(|error| {
            ValidationError::Resource(format!("cannot allocate truth FASTA records: {error}"))
        })?;
    for molecule in &molecules {
        truth_records.push(FastaRecord {
            id: molecule.id.clone(),
            sequence: try_clone_bytes(&molecule.sequence, "truth FASTA sequence")?,
        });
    }
    write_fasta(&root.join("evaluation_truth/truth.fasta"), &truth_records)?;

    let extension = if config.gzip { "fastq.gz" } else { "fastq" };
    let read1_relative = if config.case.paired() {
        format!("assembler_input/reads_R1.{extension}")
    } else {
        format!("assembler_input/reads_SE.{extension}")
    };
    let read2_relative = format!("assembler_input/reads_R2.{extension}");
    let mut read1 = FastqOutput::create(&root.join(&read1_relative), config.gzip)?;
    let mut read2 = config
        .case
        .paired()
        .then(|| FastqOutput::create(&root.join(&read2_relative), config.gzip))
        .transpose()?;
    let read_count = usize::try_from(generation_size.read_count)
        .map_err(|_| ValidationError::Resource("read count exceeds usize".to_owned()))?;
    let mut origins = Vec::new();
    origins.try_reserve_exact(read_count).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate origin ledger: {error}"))
    })?;
    let mut errors = Vec::new();
    let mut quality_events = Vec::new();
    let assignments = molecule_assignments(config, molecules.len(), &mut layout_rng)?;

    for (index, molecule_index) in assignments.into_iter().enumerate() {
        let ordinal = u64::try_from(index)
            .map_err(|_| ValidationError::Resource("fragment ordinal exceeds u64".to_owned()))?;
        let molecule = &molecules[molecule_index];
        let span = if config.case.paired() {
            config.insert_length
        } else {
            config.read_length
        };
        let start = sample_outer_start(molecule, span, &mut layout_rng)?;
        let reverse_fragment = layout_rng.uniform(2)? == 1;

        let first_r1 = if reverse_fragment {
            (start + span - 1) % molecule.sequence.len()
        } else {
            start
        };
        let step_r1 = if reverse_fragment { -1 } else { 1 };
        let first_role = if config.case.paired() { "R1" } else { "S" };
        emit_read(
            &mut read1,
            config,
            &mut error_rng,
            &mut quality_rng,
            &mut origins,
            &mut errors,
            &mut quality_events,
            ordinal,
            first_role,
            config.case.paired().then_some(1),
            molecule,
            first_r1,
            step_r1,
            start,
            span,
        )?;

        if let Some(writer) = &mut read2 {
            let first_r2 = if reverse_fragment {
                start
            } else {
                (start + span - 1) % molecule.sequence.len()
            };
            let step_r2 = if reverse_fragment { 1 } else { -1 };
            emit_read(
                writer,
                config,
                &mut error_rng,
                &mut quality_rng,
                &mut origins,
                &mut errors,
                &mut quality_events,
                ordinal,
                "R2",
                Some(2),
                molecule,
                first_r2,
                step_r2,
                start,
                span,
            )?;
        }
    }
    read1.finish()?;
    if let Some(writer) = read2 {
        writer.finish()?;
    }

    write_origins(&root.join("evaluation_truth/origins.tsv"), &origins)?;
    write_errors(&root.join("evaluation_truth/errors.tsv"), &errors)?;
    write_quality_events(
        &root.join("evaluation_truth/quality_events.tsv"),
        &quality_events,
    )?;
    verify_generated_reads(
        root,
        config,
        &molecules,
        &origins,
        &errors,
        &quality_events,
        &read1_relative,
        config.case.paired().then_some(read2_relative.as_str()),
    )?;

    let mode = if config.case.paired() {
        "paired_end"
    } else {
        "single_end"
    };
    let reads = config
        .fragments
        .checked_mul(if config.case.paired() { 2 } else { 1 })
        .ok_or_else(|| ValidationError::Resource("read count overflow".to_owned()))?;
    let mut read_paths = vec![read1_relative.clone()];
    if config.case.paired() {
        read_paths.push(read2_relative.clone());
    }
    let input_files = read_paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            Ok(InputFile {
                role: if !config.case.paired() {
                    "S"
                } else if index == 0 {
                    "R1"
                } else {
                    "R2"
                },
                path,
                sha256: sha256_file(&root.join(path))?,
            })
        })
        .collect::<ValidationResult<Vec<_>>>()?;
    let input_manifest = InputOnlyManifest {
        schema_version: "veritasm-assembler-input-v1",
        dataset_id: &dataset_id,
        mode,
        fragments_decimal: config.fragments.to_string(),
        reads_decimal: reads.to_string(),
        read_files: input_files,
        truth_exclusion:
            "Only read_files are assembler inputs; no reference sequence or origin ledger is declared here.",
    };
    write_json(
        &root.join("assembler_input/input_manifest.json"),
        &input_manifest,
    )?;

    let mut relative_paths = vec![
        "assembler_input/input_manifest.json".to_owned(),
        read1_relative.clone(),
        "evaluation_truth/errors.tsv".to_owned(),
        "evaluation_truth/origins.tsv".to_owned(),
        "evaluation_truth/quality_events.tsv".to_owned(),
        "evaluation_truth/truth.fasta".to_owned(),
    ];
    if config.case.paired() {
        relative_paths.push(read2_relative.clone());
    }
    relative_paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    let files = relative_paths
        .iter()
        .map(|relative| {
            let metadata = fs::metadata(root.join(relative))?;
            Ok(FileRecord {
                path: relative.clone(),
                role: file_role(relative).to_owned(),
                visibility: if relative.starts_with("assembler_input/") {
                    "assembler_input"
                } else {
                    "evaluation_only"
                }
                .to_owned(),
                bytes_decimal: metadata.len().to_string(),
                sha256: sha256_file(&root.join(relative))?,
            })
        })
        .collect::<ValidationResult<Vec<_>>>()?;
    let truth_molecules = molecules
        .iter()
        .map(|molecule| TruthMoleculeRecord {
            molecule_id: molecule.id.clone(),
            truth_class: molecule.class,
            topology: molecule.topology,
            length_bases_decimal: molecule.sequence.len().to_string(),
            sequence_sha256: sha256_bytes(&molecule.sequence),
        })
        .collect();
    let manifest = DatasetManifest {
        schema_version: "veritasm-validation-dataset-v2".to_owned(),
        dataset_id,
        tier: "tier_1_truth_known_computational".to_owned(),
        role: "development_qualification_not_release_scorecard".to_owned(),
        case_id: config.case.to_string(),
        replicate_decimal: config.replicate.to_string(),
        generator: GeneratorRecord {
            name: "veritasm-simulate".to_owned(),
            version: GENERATOR_VERSION.to_owned(),
            plan_version: PLAN_VERSION.to_owned(),
            rng_name: RNG_NAME.to_owned(),
            rng_byte_order: "counter_u64_le_output_words_u64_le".to_owned(),
            seed_hex,
            seed_source: seed_source.to_owned(),
            rng_streams: RngStreamsRecord {
                derivation: RNG_STREAM_DERIVATION.to_owned(),
                layout_seed_hex: layout_seed.to_hex(),
                error_seed_hex: error_seed.to_hex(),
                quality_seed_hex,
                quality_seed_source: quality_seed_source.to_owned(),
            },
            parameters,
        },
        assembler_input: AssemblerInputRecord {
            mode: mode.to_owned(),
            fragments_decimal: config.fragments.to_string(),
            reads_decimal: reads.to_string(),
            manifest_path: "assembler_input/input_manifest.json".to_owned(),
            read_paths,
            contract: "Pass only the declared read_paths to an assembler; truth and ledgers are evaluator-only.".to_owned(),
        },
        evaluation_truth: TruthRecord {
            namespace: "evaluation_only".to_owned(),
            fasta_path: "evaluation_truth/truth.fasta".to_owned(),
            origins_path: "evaluation_truth/origins.tsv".to_owned(),
            errors_path: "evaluation_truth/errors.tsv".to_owned(),
            quality_events_path: "evaluation_truth/quality_events.tsv".to_owned(),
            coordinate_system: VALIDATION_COORDINATE_SYSTEM.to_owned(),
            molecules: truth_molecules,
        },
        files,
        limitations: vec![
            "Synthetic substitutions and binary Q10/Q40 qualities are not a sequencing-platform model.".to_owned(),
            "The qc-censoring-control case intentionally assigns Q10 to every substituted base and is not retained-error qualification evidence.".to_owned(),
            "Generated recovery is not evidence of organism detection, absence, or assay sensitivity.".to_owned(),
            "The current executable slice is not the full pre-registered validation matrix.".to_owned(),
        ],
    };
    write_json(&root.join("dataset.json"), &manifest)?;
    relative_paths.push("dataset.json".to_owned());
    write_checksum_manifest(root, &relative_paths)?;
    commit_staging(&staging, &config.output_directory)?;
    Ok(manifest)
}

fn parameters_from_config(config: &GeneratorConfig) -> GeneratorParameters {
    GeneratorParameters {
        fragments_decimal: config.fragments.to_string(),
        truth_length_decimal: config.truth_length.to_string(),
        read_length_decimal: config.read_length.to_string(),
        insert_length_decimal: config.insert_length.to_string(),
        substitution_rate_ppm_decimal: config.substitution_rate_ppm.to_string(),
        low_quality_rate_ppm_decimal: config.low_quality_rate_ppm.to_string(),
        error_model: "independent_substitution_bernoulli_per_observed_base_v1".to_owned(),
        quality_model: config.case.quality_model().to_owned(),
        default_quality_phred_decimal: DEFAULT_QUALITY_PHRED.to_string(),
        low_quality_phred_decimal: LOW_QUALITY_PHRED.to_string(),
        mixture_minor_fragment_fraction: if matches!(config.case, GeneratorCase::MixturePe) {
            "0.1_exact_count_rounded_down"
        } else {
            "not_applicable"
        }
        .to_owned(),
        library_orientation: if config.case.paired() {
            "inward_FR_with_random_truth_strand"
        } else {
            "not_applicable"
        }
        .to_owned(),
        output_compression: if config.gzip { "gzip" } else { "plain" }.to_owned(),
    }
}

fn validate_config(config: &GeneratorConfig) -> ValidationResult<GenerationSize> {
    let validated = validate_generator_parameters(config.case, &parameters_from_config(config))?;
    Ok(GenerationSize {
        read_count: validated.read_count,
    })
}

fn generation_size_from_values(
    case: GeneratorCase,
    fragments: u64,
    truth_length: u64,
    read_length: u64,
    substitution_rate_ppm: u64,
    low_quality_rate_ppm: u64,
) -> ValidationResult<u64> {
    let mates = if case.paired() { 2 } else { 1 };
    let read_count = fragments
        .checked_mul(mates)
        .ok_or_else(|| ValidationError::Resource("generated read count overflow".to_owned()))?;
    let read_bases = read_count.checked_mul(read_length).ok_or_else(|| {
        ValidationError::Resource("generated read-base count overflow".to_owned())
    })?;
    if read_bases > MAX_GENERATED_READ_BASES {
        return Err(ValidationError::Resource(format!(
            "generated read bases {read_bases} exceed cap {MAX_GENERATED_READ_BASES}"
        )));
    }
    let molecule_count = if matches!(case, GeneratorCase::MixturePe) {
        2
    } else {
        1
    };
    let truth_bases = truth_length.checked_mul(molecule_count).ok_or_else(|| {
        ValidationError::Resource("generated truth-base count overflow".to_owned())
    })?;
    let fastq_bytes = read_bases
        .checked_mul(2)
        .and_then(|bytes| {
            read_count
                .checked_mul(ESTIMATED_FASTQ_BYTES_PER_READ)
                .and_then(|overhead| bytes.checked_add(overhead))
        })
        .ok_or_else(|| ValidationError::Resource("FASTQ size estimate overflow".to_owned()))?;
    let origin_bytes = read_count
        .checked_mul(ESTIMATED_ORIGIN_BYTES_PER_READ)
        .ok_or_else(|| {
            ValidationError::Resource("origin-ledger size estimate overflow".to_owned())
        })?;
    let error_bytes = if substitution_rate_ppm == 0 {
        0
    } else {
        read_bases
            .checked_mul(ESTIMATED_ERROR_BYTES_PER_EVENT)
            .ok_or_else(|| {
                ValidationError::Resource("error-ledger size estimate overflow".to_owned())
            })?
    };
    let quality_event_bytes = if low_quality_rate_ppm == 0
        && !matches!(case, GeneratorCase::QcCensoringControl)
    {
        0
    } else {
        read_bases
            .checked_mul(ESTIMATED_QUALITY_EVENT_BYTES_PER_EVENT)
            .ok_or_else(|| {
                ValidationError::Resource("quality-event-ledger size estimate overflow".to_owned())
            })?
    };
    let estimated_output_bytes = [
        fastq_bytes,
        origin_bytes,
        error_bytes,
        quality_event_bytes,
        truth_bases,
        ESTIMATED_FIXED_OUTPUT_BYTES,
    ]
    .into_iter()
    .try_fold(0u64, |total, value| total.checked_add(value))
    .ok_or_else(|| ValidationError::Resource("total output size estimate overflow".to_owned()))?;
    if estimated_output_bytes > MAX_GENERATED_OUTPUT_ESTIMATE_BYTES {
        return Err(ValidationError::Resource(format!(
            "conservative generated output estimate {estimated_output_bytes} bytes exceeds cap {MAX_GENERATED_OUTPUT_ESTIMATE_BYTES}"
        )));
    }
    Ok(read_count)
}

fn build_molecules(
    config: &GeneratorConfig,
    rng: &mut CounterRng,
) -> ValidationResult<Vec<Molecule>> {
    let primary = random_dna(config.truth_length, rng)?;
    let topology = if config.case.circular() {
        TruthTopology::Circular
    } else {
        TruthTopology::Linear
    };
    let count = if matches!(config.case, GeneratorCase::MixturePe) {
        2
    } else {
        1
    };
    let mut molecules = Vec::new();
    molecules.try_reserve_exact(count).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate truth molecules: {error}"))
    })?;
    let minor = if matches!(config.case, GeneratorCase::MixturePe) {
        let mut minor = try_clone_bytes(&primary, "minor truth sequence")?;
        for position in (17..minor.len()).step_by(50) {
            minor[position] = mutate_base(minor[position], rng)?;
        }
        Some(minor)
    } else {
        None
    };
    molecules.push(Molecule {
        id: "mol-0001".to_owned(),
        class: TruthClass::Primary,
        topology,
        sequence: primary,
    });
    if let Some(minor) = minor {
        molecules.push(Molecule {
            id: "mol-0002".to_owned(),
            class: TruthClass::Minor,
            topology: TruthTopology::Linear,
            sequence: minor,
        });
    }
    Ok(molecules)
}

fn molecule_assignments(
    config: &GeneratorConfig,
    molecule_count: usize,
    rng: &mut CounterRng,
) -> ValidationResult<Vec<usize>> {
    let count = usize::try_from(config.fragments)
        .map_err(|_| ValidationError::Resource("fragment count exceeds usize".to_owned()))?;
    let mut assignments = Vec::new();
    assignments.try_reserve_exact(count).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate fragment assignments: {error}"))
    })?;
    assignments.resize(count, 0);
    if matches!(config.case, GeneratorCase::MixturePe) {
        if molecule_count != 2 {
            return Err(ValidationError::Integrity(
                "mixture case must contain two truth molecules".to_owned(),
            ));
        }
        let minor_count = count / 10;
        assignments[count - minor_count..].fill(1);
        rng.shuffle(&mut assignments)?;
    }
    Ok(assignments)
}

fn random_dna(length: usize, rng: &mut CounterRng) -> ValidationResult<Vec<u8>> {
    let mut sequence = Vec::new();
    sequence.try_reserve_exact(length).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate truth sequence: {error}"))
    })?;
    for _ in 0..length {
        sequence.push(match rng.next_u64() & 3 {
            0 => b'A',
            1 => b'C',
            2 => b'G',
            _ => b'T',
        });
    }
    Ok(sequence)
}

fn try_clone_bytes(source: &[u8], label: &str) -> ValidationResult<Vec<u8>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(source.len())
        .map_err(|error| ValidationError::Resource(format!("cannot allocate {label}: {error}")))?;
    output.extend_from_slice(source);
    Ok(output)
}

fn mutate_base(base: u8, rng: &mut CounterRng) -> ValidationResult<u8> {
    let alternatives = match base {
        b'A' => b"CGT",
        b'C' => b"AGT",
        b'G' => b"ACT",
        b'T' => b"ACG",
        _ => unreachable!("generated truth is A/C/G/T"),
    };
    Ok(alternatives[rng.uniform(alternatives.len())?])
}

fn sample_outer_start(
    molecule: &Molecule,
    span: usize,
    rng: &mut CounterRng,
) -> ValidationResult<usize> {
    let choices = if molecule.topology == TruthTopology::Circular {
        molecule.sequence.len()
    } else {
        molecule.sequence.len() - span + 1
    };
    rng.uniform(choices)
}

#[allow(clippy::too_many_arguments)]
fn emit_read(
    writer: &mut FastqOutput,
    config: &GeneratorConfig,
    error_rng: &mut CounterRng,
    quality_rng: &mut CounterRng,
    origins: &mut Vec<OriginRow>,
    errors: &mut Vec<ErrorRow>,
    quality_events: &mut Vec<QualityEventRow>,
    ordinal: u64,
    mate_role: &'static str,
    mate_number: Option<u8>,
    molecule: &Molecule,
    first_coordinate: usize,
    step: i8,
    outer_start: usize,
    outer_span: usize,
) -> ValidationResult<()> {
    let read_id = if let Some(mate_number) = mate_number {
        format!("v4_{ordinal:012}/{mate_number}")
    } else {
        format!("v4_{ordinal:012}")
    };
    let pre_error = truth_read(molecule, first_coordinate, step, config.read_length)?;
    let mut observed = try_clone_bytes(&pre_error, "observed read sequence")?;
    let mut qualities = Vec::new();
    qualities
        .try_reserve_exact(observed.len())
        .map_err(|error| {
            ValidationError::Resource(format!("cannot allocate read qualities: {error}"))
        })?;
    qualities.resize(observed.len(), DEFAULT_QUALITY_PHRED);
    for offset in 0..observed.len() {
        let substituted = error_rng.uniform(1_000_000)? < config.substitution_rate_ppm as usize;
        if substituted {
            let truth_base = observed[offset];
            observed[offset] = mutate_base(truth_base, error_rng)?;
            try_push(
                errors,
                ErrorRow {
                    read_id: read_id.clone(),
                    observed_offset: offset,
                    truth_coordinate: coordinate_at(
                        first_coordinate,
                        step,
                        offset,
                        molecule.sequence.len(),
                        molecule.topology,
                    )?,
                    truth_base,
                    observed_base: observed[offset],
                },
                "error ledger",
            )?;
        }
        let quality_event_kind = if matches!(config.case, GeneratorCase::QcCensoringControl) {
            substituted.then_some("qc_censoring_substitution_linked_q10")
        } else if quality_rng.uniform(1_000_000)? < config.low_quality_rate_ppm as usize {
            Some("independent_low_quality_q10")
        } else {
            None
        };
        if let Some(event_kind) = quality_event_kind {
            qualities[offset] = LOW_QUALITY_PHRED;
            try_push(
                quality_events,
                QualityEventRow {
                    read_id: read_id.clone(),
                    observed_offset: offset,
                    quality_phred: qualities[offset],
                    event_kind,
                },
                "quality-event ledger",
            )?;
        }
    }
    writeln!(writer, "@{read_id}")?;
    writer.write_all(&observed)?;
    writer.write_all(b"\n+\n")?;
    for quality in &qualities {
        writer.write_all(&[quality + 33])?;
    }
    writer.write_all(b"\n")?;

    let wraps_origin = if molecule.topology == TruthTopology::Circular {
        if step > 0 {
            first_coordinate + config.read_length > molecule.sequence.len()
        } else {
            config.read_length > first_coordinate + 1
        }
    } else {
        false
    };
    try_push(
        origins,
        OriginRow {
            fragment_ordinal: ordinal,
            read_id,
            mate_role,
            molecule_id: molecule.id.clone(),
            topology: molecule.topology,
            strand: if step > 0 { '+' } else { '-' },
            first_truth_base: first_coordinate,
            truth_step: step,
            read_length: config.read_length,
            outer_fragment_start: outer_start,
            outer_fragment_span: outer_span,
            wraps_origin,
            pre_error_sha256: sha256_bytes(&pre_error),
            observed_sha256: sha256_bytes(&observed),
            quality_sha256: sha256_bytes(&qualities),
        },
        "origin ledger",
    )?;
    Ok(())
}

fn truth_read(
    molecule: &Molecule,
    first: usize,
    step: i8,
    length: usize,
) -> ValidationResult<Vec<u8>> {
    let mut read = Vec::new();
    read.try_reserve_exact(length).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate truth-derived read: {error}"))
    })?;
    for offset in 0..length {
        let coordinate = coordinate_at(
            first,
            step,
            offset,
            molecule.sequence.len(),
            molecule.topology,
        )?;
        let truth_base = molecule.sequence[coordinate];
        read.push(if step > 0 {
            truth_base
        } else {
            complement_exact_base(truth_base)
        });
    }
    Ok(read)
}

/// Replays a declared origin without calling the generator's coordinate or orientation helpers.
///
/// This deliberately duplicates the coordinate arithmetic as a qualification oracle. Sharing
/// `truth_read` or `coordinate_at` here would let one defect make both generation and verification
/// agree.
fn independent_origin_replay(
    forward_truth: &[u8],
    topology: TruthTopology,
    first: usize,
    step: i8,
    length: usize,
) -> ValidationResult<Vec<u8>> {
    if forward_truth.is_empty() || first >= forward_truth.len() || !matches!(step, -1 | 1) {
        return Err(ValidationError::Integrity(
            "independent origin replay received invalid geometry".to_owned(),
        ));
    }
    let mut replay = Vec::new();
    replay.try_reserve_exact(length).map_err(|error| {
        ValidationError::Resource(format!(
            "cannot allocate independent origin replay: {error}"
        ))
    })?;
    for offset in 0..length {
        let coordinate =
            independent_origin_coordinate(forward_truth.len(), topology, first, step, offset)?;
        let base = forward_truth[coordinate];
        replay.push(if step == 1 {
            base
        } else {
            match base {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => {
                    return Err(ValidationError::Integrity(
                        "independent origin replay encountered non-ACGT truth".to_owned(),
                    ))
                }
            }
        });
    }
    Ok(replay)
}

fn independent_origin_coordinate(
    truth_length: usize,
    topology: TruthTopology,
    first: usize,
    step: i8,
    offset: usize,
) -> ValidationResult<usize> {
    if truth_length == 0 || first >= truth_length || !matches!(step, -1 | 1) {
        return Err(ValidationError::Integrity(
            "independent origin coordinate received invalid geometry".to_owned(),
        ));
    }
    match (topology, step) {
        (TruthTopology::Circular, 1) => first
            .checked_add(offset % truth_length)
            .map(|coordinate| coordinate % truth_length)
            .ok_or_else(|| {
                ValidationError::Resource("independent circular coordinate overflow".to_owned())
            }),
        (TruthTopology::Circular, -1) => {
            let retreat = offset % truth_length;
            Ok(if retreat <= first {
                first - retreat
            } else {
                truth_length - (retreat - first)
            })
        }
        (TruthTopology::Linear, 1) => first
            .checked_add(offset)
            .filter(|coordinate| *coordinate < truth_length)
            .ok_or_else(|| {
                ValidationError::Integrity(
                    "independent forward origin exceeds linear truth".to_owned(),
                )
            }),
        (TruthTopology::Linear, -1) => first.checked_sub(offset).ok_or_else(|| {
            ValidationError::Integrity("independent reverse origin exceeds linear truth".to_owned())
        }),
        _ => unreachable!("step validated as plus or minus one"),
    }
}

fn complement_exact_base(base: u8) -> u8 {
    match base {
        b'A' => b'T',
        b'C' => b'G',
        b'G' => b'C',
        b'T' => b'A',
        _ => unreachable!("generated truth is A/C/G/T"),
    }
}

fn try_push<T>(values: &mut Vec<T>, value: T, label: &str) -> ValidationResult<()> {
    if values.len() == values.capacity() {
        values
            .try_reserve(1)
            .map_err(|error| ValidationError::Resource(format!("cannot grow {label}: {error}")))?;
    }
    values.push(value);
    Ok(())
}

fn coordinate_at(
    first: usize,
    step: i8,
    offset: usize,
    length: usize,
    topology: TruthTopology,
) -> ValidationResult<usize> {
    if first >= length || !matches!(step, -1 | 1) {
        return Err(ValidationError::Integrity(
            "invalid truth-origin coordinate or step".to_owned(),
        ));
    }
    if step > 0 {
        let coordinate = first
            .checked_add(offset)
            .ok_or_else(|| ValidationError::Resource("truth coordinate overflow".to_owned()))?;
        if topology == TruthTopology::Circular {
            Ok(coordinate % length)
        } else if coordinate < length {
            Ok(coordinate)
        } else {
            Err(ValidationError::Integrity(
                "linear read origin exceeds truth".to_owned(),
            ))
        }
    } else if topology == TruthTopology::Circular {
        Ok((first + length - (offset % length)) % length)
    } else {
        first.checked_sub(offset).ok_or_else(|| {
            ValidationError::Integrity("linear reverse read origin exceeds truth".to_owned())
        })
    }
}

fn write_origins(path: &Path, rows: &[OriginRow]) -> ValidationResult<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(b"schema_version\tfragment_ordinal\tread_id\tmate_role\tmolecule_id\ttopology\tstrand\tfirst_truth_base_zero_based\ttruth_step\tread_length\touter_fragment_start_zero_based\touter_fragment_span\twraps_origin\tpre_error_sequence_sha256\tobserved_sequence_sha256\tquality_phred_sequence_sha256\n")?;
    for row in rows {
        writeln!(
            writer,
            "2.0\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.fragment_ordinal,
            row.read_id,
            row.mate_role,
            row.molecule_id,
            topology_name(row.topology),
            row.strand,
            row.first_truth_base,
            row.truth_step,
            row.read_length,
            row.outer_fragment_start,
            row.outer_fragment_span,
            row.wraps_origin,
            row.pre_error_sha256,
            row.observed_sha256,
            row.quality_sha256
        )?;
    }
    writer.flush()?;
    Ok(())
}

fn write_errors(path: &Path, rows: &[ErrorRow]) -> ValidationResult<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(b"schema_version\tread_id\tobserved_offset_zero_based\ttruth_coordinate_zero_based\terror_kind\ttruth_base\tobserved_base\n")?;
    for row in rows {
        writeln!(
            writer,
            "2.0\t{}\t{}\t{}\tsubstitution\t{}\t{}",
            row.read_id,
            row.observed_offset,
            row.truth_coordinate,
            char::from(row.truth_base),
            char::from(row.observed_base)
        )?;
    }
    writer.flush()?;
    Ok(())
}

fn write_quality_events(path: &Path, rows: &[QualityEventRow]) -> ValidationResult<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(
        b"schema_version\tread_id\tobserved_offset_zero_based\tevent_kind\tquality_phred\n",
    )?;
    for row in rows {
        writeln!(
            writer,
            "1.0\t{}\t{}\t{}\t{}",
            row.read_id, row.observed_offset, row.event_kind, row.quality_phred
        )?;
    }
    writer.flush()?;
    Ok(())
}

struct GeneratedFastqRecord {
    id: String,
    sequence: Vec<u8>,
    quality: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn verify_generated_reads(
    root: &Path,
    config: &GeneratorConfig,
    molecules: &[Molecule],
    origins: &[OriginRow],
    errors: &[ErrorRow],
    quality_events: &[QualityEventRow],
    read1_relative: &str,
    read2_relative: Option<&str>,
) -> ValidationResult<()> {
    let first = read_generated_fastq(&root.join(read1_relative), config.gzip)?;
    let second = read2_relative
        .map(|relative| read_generated_fastq(&root.join(relative), config.gzip))
        .transpose()?;
    let expected_fragments = usize::try_from(config.fragments)
        .map_err(|_| ValidationError::Resource("fragment count exceeds usize".to_owned()))?;
    if first.len() != expected_fragments
        || second
            .as_ref()
            .is_some_and(|records| records.len() != expected_fragments)
    {
        return Err(ValidationError::Integrity(
            "generated FASTQ record count does not match configuration".to_owned(),
        ));
    }
    if let Some(second) = &second {
        for (left, right) in first.iter().zip(second) {
            let left_id = left.id.strip_suffix("/1").ok_or_else(|| {
                ValidationError::Integrity("generated R1 identifier lacks /1".to_owned())
            })?;
            let right_id = right.id.strip_suffix("/2").ok_or_else(|| {
                ValidationError::Integrity("generated R2 identifier lacks /2".to_owned())
            })?;
            if left_id != right_id {
                return Err(ValidationError::Integrity(
                    "generated mates are not synchronized".to_owned(),
                ));
            }
        }
    } else if first.iter().any(|record| record.id.contains('/')) {
        return Err(ValidationError::Integrity(
            "generated single-end identifier exposes a mate role".to_owned(),
        ));
    }

    let mut observed_by_id = BTreeMap::new();
    for record in first.iter().chain(second.iter().flatten()) {
        if observed_by_id.insert(record.id.as_str(), record).is_some() {
            return Err(ValidationError::Integrity(
                "duplicate generated read identifier".to_owned(),
            ));
        }
    }
    if observed_by_id.len() != origins.len() {
        return Err(ValidationError::Integrity(
            "origin ledger row count does not match generated reads".to_owned(),
        ));
    }
    let mut errors_by_read = BTreeMap::<&str, Vec<&ErrorRow>>::new();
    for error in errors {
        errors_by_read
            .entry(error.read_id.as_str())
            .or_default()
            .push(error);
    }
    let mut quality_events_by_read = BTreeMap::<&str, Vec<&QualityEventRow>>::new();
    for event in quality_events {
        quality_events_by_read
            .entry(event.read_id.as_str())
            .or_default()
            .push(event);
    }
    let molecules_by_id = molecules
        .iter()
        .map(|molecule| (molecule.id.as_str(), molecule))
        .collect::<BTreeMap<_, _>>();
    for origin in origins {
        let record = observed_by_id.get(origin.read_id.as_str()).ok_or_else(|| {
            ValidationError::Integrity("origin ledger names missing read".to_owned())
        })?;
        let molecule = molecules_by_id
            .get(origin.molecule_id.as_str())
            .ok_or_else(|| {
                ValidationError::Integrity("origin ledger names missing molecule".to_owned())
            })?;
        let pre_error = independent_origin_replay(
            &molecule.sequence,
            molecule.topology,
            origin.first_truth_base,
            origin.truth_step,
            origin.read_length,
        )?;
        if sha256_bytes(&pre_error) != origin.pre_error_sha256
            || sha256_bytes(&record.sequence) != origin.observed_sha256
        {
            return Err(ValidationError::Integrity(
                "generated read does not match origin sequence digests".to_owned(),
            ));
        }
        let mut expected = pre_error.clone();
        let mut expected_quality = vec![DEFAULT_QUALITY_PHRED; expected.len()];
        let mut prior_offset = None;
        let mut error_offsets = BTreeSet::new();
        for error in errors_by_read
            .get(origin.read_id.as_str())
            .into_iter()
            .flat_map(|rows| rows.iter().copied())
        {
            if error.observed_offset >= expected.len()
                || prior_offset.is_some_and(|prior| error.observed_offset <= prior)
            {
                return Err(ValidationError::Integrity(
                    "error ledger offsets are invalid or not strictly increasing".to_owned(),
                ));
            }
            let coordinate = independent_origin_coordinate(
                molecule.sequence.len(),
                molecule.topology,
                origin.first_truth_base,
                origin.truth_step,
                error.observed_offset,
            )?;
            if coordinate != error.truth_coordinate
                || expected[error.observed_offset] != error.truth_base
                || error.truth_base == error.observed_base
            {
                return Err(ValidationError::Integrity(
                    "error event does not reconcile to read origin".to_owned(),
                ));
            }
            expected[error.observed_offset] = error.observed_base;
            error_offsets.insert(error.observed_offset);
            prior_offset = Some(error.observed_offset);
        }
        let mut prior_quality_offset = None;
        let mut quality_offsets = BTreeSet::new();
        for event in quality_events_by_read
            .get(origin.read_id.as_str())
            .into_iter()
            .flat_map(|rows| rows.iter().copied())
        {
            if event.observed_offset >= expected_quality.len()
                || prior_quality_offset.is_some_and(|prior| event.observed_offset <= prior)
                || event.quality_phred != LOW_QUALITY_PHRED
            {
                return Err(ValidationError::Integrity(
                    "quality-event offsets or values are invalid".to_owned(),
                ));
            }
            let event_valid = if matches!(config.case, GeneratorCase::QcCensoringControl) {
                event.event_kind == "qc_censoring_substitution_linked_q10"
                    && error_offsets.contains(&event.observed_offset)
            } else {
                event.event_kind == "independent_low_quality_q10"
            };
            if !event_valid {
                return Err(ValidationError::Integrity(
                    "quality event contradicts the declared quality model".to_owned(),
                ));
            }
            expected_quality[event.observed_offset] = event.quality_phred;
            quality_offsets.insert(event.observed_offset);
            prior_quality_offset = Some(event.observed_offset);
        }
        if matches!(config.case, GeneratorCase::QcCensoringControl)
            && quality_offsets != error_offsets
        {
            return Err(ValidationError::Integrity(
                "qc-censoring-control quality events do not equal substitution offsets".to_owned(),
            ));
        }
        let decoded_quality = record
            .quality
            .iter()
            .map(|byte| byte.checked_sub(33))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| ValidationError::Integrity("generated quality below Q0".to_owned()))?;
        if expected != record.sequence
            || expected_quality != decoded_quality
            || sha256_bytes(&decoded_quality) != origin.quality_sha256
        {
            return Err(ValidationError::Integrity(
                "generated FASTQ does not replay from origin and error ledgers".to_owned(),
            ));
        }
    }
    Ok(())
}

fn read_generated_fastq(path: &Path, gzip: bool) -> ValidationResult<Vec<GeneratedFastqRecord>> {
    let stored = fs::read(path)?;
    let mut decoded = Vec::new();
    if gzip {
        MultiGzDecoder::new(Cursor::new(stored)).read_to_end(&mut decoded)?;
    } else {
        decoded = stored;
    }
    if decoded.is_empty() {
        return Ok(Vec::new());
    }
    let text = std::str::from_utf8(&decoded)
        .map_err(|error| ValidationError::Integrity(format!("generated FASTQ UTF-8: {error}")))?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() % 4 != 0 {
        return Err(ValidationError::Integrity(
            "generated FASTQ is not four-line structured".to_owned(),
        ));
    }
    lines
        .chunks_exact(4)
        .map(|record| {
            let id = record[0].strip_prefix('@').ok_or_else(|| {
                ValidationError::Integrity("generated FASTQ header lacks @".to_owned())
            })?;
            if record[2] != "+" || record[1].len() != record[3].len() {
                return Err(ValidationError::Integrity(
                    "generated FASTQ sequence/quality structure is invalid".to_owned(),
                ));
            }
            Ok(GeneratedFastqRecord {
                id: id.to_owned(),
                sequence: record[1].as_bytes().to_vec(),
                quality: record[3].as_bytes().to_vec(),
            })
        })
        .collect()
}

fn topology_name(topology: TruthTopology) -> &'static str {
    match topology {
        TruthTopology::Linear => "linear",
        TruthTopology::Circular => "circular",
    }
}

fn file_role(path: &str) -> &'static str {
    match path {
        "assembler_input/input_manifest.json" => "assembler_input_manifest",
        path if path.contains("reads_SE") => "single_end_reads",
        path if path.contains("reads_R1") => "read_1",
        path if path.contains("reads_R2") => "read_2",
        "evaluation_truth/truth.fasta" => "truth_sequences",
        "evaluation_truth/origins.tsv" => "read_origin_ledger",
        "evaluation_truth/errors.tsv" => "error_event_ledger",
        "evaluation_truth/quality_events.tsv" => "quality_event_ledger",
        _ => "unknown",
    }
}

struct CounterRng {
    seed: Seed256,
    counter: u64,
    block: [u8; 32],
    offset: usize,
}

impl CounterRng {
    fn new(seed: Seed256) -> Self {
        Self {
            seed,
            counter: 0,
            block: [0; 32],
            offset: 32,
        }
    }

    fn refill(&mut self) {
        let mut digest = Sha256::new();
        digest.update(b"veritasm:validation-rng:v1\0");
        digest.update(self.seed.0);
        digest.update(self.counter.to_le_bytes());
        self.block = digest.finalize().into();
        self.counter = self
            .counter
            .checked_add(1)
            .expect("generator RNG counter exhausted");
        self.offset = 0;
    }

    fn next_u64(&mut self) -> u64 {
        if self.offset > 24 {
            self.refill();
        }
        let bytes: [u8; 8] = self.block[self.offset..self.offset + 8]
            .try_into()
            .expect("eight RNG bytes are available");
        self.offset += 8;
        u64::from_le_bytes(bytes)
    }

    fn uniform(&mut self, upper: usize) -> ValidationResult<usize> {
        if upper == 0 {
            return Err(ValidationError::Configuration(
                "cannot sample an empty range".to_owned(),
            ));
        }
        let upper_u64 = u64::try_from(upper)
            .map_err(|_| ValidationError::Resource("sample range exceeds u64".to_owned()))?;
        loop {
            let value = self.next_u64();
            if let Some(bounded) = unbiased_bounded_word(value, upper_u64) {
                return usize::try_from(bounded)
                    .map_err(|_| ValidationError::Resource("sample exceeds usize".to_owned()));
            }
        }
    }

    fn shuffle<T>(&mut self, values: &mut [T]) -> ValidationResult<()> {
        for index in (1..values.len()).rev() {
            let selected = self.uniform(index + 1)?;
            values.swap(index, selected);
        }
        Ok(())
    }
}

fn unbiased_bounded_word(value: u64, upper: u64) -> Option<u64> {
    debug_assert!(upper > 0);
    let threshold = u64::MAX - (u64::MAX % upper);
    (value < threshold).then_some(value % upper)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn frozen_seed_vector_matches_contract() {
        assert_eq!(
            derive_seed(GeneratorCase::LinearSe, 0).to_hex(),
            "18c9415808bbd749e6a02b8d9888a9a697318e67705667a66f7f167a9d466c05"
        );
    }

    #[test]
    fn dataset_identity_commits_every_generator_parameter() {
        let parameters = GeneratorParameters {
            fragments_decimal: "10".to_owned(),
            truth_length_decimal: "100".to_owned(),
            read_length_decimal: "20".to_owned(),
            insert_length_decimal: "40".to_owned(),
            substitution_rate_ppm_decimal: "1".to_owned(),
            low_quality_rate_ppm_decimal: "2".to_owned(),
            error_model: "independent_substitution_bernoulli_per_observed_base_v1".to_owned(),
            quality_model: "independent_bernoulli_q10_else_q40_v1".to_owned(),
            default_quality_phred_decimal: "40".to_owned(),
            low_quality_phred_decimal: "10".to_owned(),
            mixture_minor_fragment_fraction: "not_applicable".to_owned(),
            library_orientation: "inward_FR_with_random_truth_strand".to_owned(),
            output_compression: "plain".to_owned(),
        };
        let id = |case: &str,
                  replicate: &str,
                  master: &str,
                  quality: &str,
                  values: &GeneratorParameters| {
            dataset_id_from_parameters(
                case,
                replicate,
                master,
                "explicit",
                &"1".repeat(64),
                &"2".repeat(64),
                quality,
                "explicit",
                values,
            )
        };
        let baseline = id(
            "linear-pe",
            "0",
            &"0".repeat(64),
            &"3".repeat(64),
            &parameters,
        );
        assert_eq!(
            baseline,
            "v4-6c869a6adcce48d7c0c98c024ecc82bf655ad90844fe2ff45fc0bf479ba8cd79"
        );
        assert_ne!(
            baseline,
            id(
                "circular-pe",
                "0",
                &"0".repeat(64),
                &"3".repeat(64),
                &parameters
            )
        );
        assert_ne!(
            baseline,
            id(
                "linear-pe",
                "1",
                &"0".repeat(64),
                &"3".repeat(64),
                &parameters
            )
        );
        assert_ne!(
            baseline,
            id(
                "linear-pe",
                "0",
                &"4".repeat(64),
                &"3".repeat(64),
                &parameters
            )
        );
        assert_ne!(
            baseline,
            id(
                "linear-pe",
                "0",
                &"0".repeat(64),
                &"5".repeat(64),
                &parameters
            )
        );
        for changed in [
            dataset_id_from_parameters(
                "linear-pe",
                "0",
                &"0".repeat(64),
                "sha256_plan_case_replicate",
                &"1".repeat(64),
                &"2".repeat(64),
                &"3".repeat(64),
                "explicit",
                &parameters,
            ),
            dataset_id_from_parameters(
                "linear-pe",
                "0",
                &"0".repeat(64),
                "explicit",
                &"4".repeat(64),
                &"2".repeat(64),
                &"3".repeat(64),
                "explicit",
                &parameters,
            ),
            dataset_id_from_parameters(
                "linear-pe",
                "0",
                &"0".repeat(64),
                "explicit",
                &"1".repeat(64),
                &"4".repeat(64),
                &"3".repeat(64),
                "explicit",
                &parameters,
            ),
            dataset_id_from_parameters(
                "linear-pe",
                "0",
                &"0".repeat(64),
                "explicit",
                &"1".repeat(64),
                &"2".repeat(64),
                &"3".repeat(64),
                "derived_from_master_seed",
                &parameters,
            ),
        ] {
            assert_ne!(baseline, changed);
        }
        macro_rules! assert_parameter_bound {
            ($field:ident, $replacement:expr) => {{
                let mut changed = parameters.clone();
                changed.$field = $replacement.to_owned();
                assert_ne!(
                    baseline,
                    id("linear-pe", "0", &"0".repeat(64), &"3".repeat(64), &changed),
                    stringify!($field)
                );
            }};
        }
        assert_parameter_bound!(fragments_decimal, "11");
        assert_parameter_bound!(truth_length_decimal, "101");
        assert_parameter_bound!(read_length_decimal, "21");
        assert_parameter_bound!(insert_length_decimal, "41");
        assert_parameter_bound!(substitution_rate_ppm_decimal, "3");
        assert_parameter_bound!(low_quality_rate_ppm_decimal, "4");
        assert_parameter_bound!(error_model, "changed");
        assert_parameter_bound!(quality_model, "changed");
        assert_parameter_bound!(default_quality_phred_decimal, "39");
        assert_parameter_bound!(low_quality_phred_decimal, "9");
        assert_parameter_bound!(mixture_minor_fragment_fraction, "changed");
        assert_parameter_bound!(library_orientation, "changed");
        assert_parameter_bound!(output_compression, "gzip");
    }

    #[test]
    fn bounded_sampling_rejects_incomplete_u64_tail() {
        assert_eq!(unbiased_bounded_word(u64::MAX, 3), None);
        let mut counts = [0usize; 3];
        for word in 0..9 {
            counts[unbiased_bounded_word(word, 3).unwrap() as usize] += 1;
        }
        assert_eq!(counts, [3, 3, 3]);
    }

    #[test]
    fn substitution_sampling_excludes_truth_and_reaches_each_alternative() {
        for truth in b"ACGT" {
            let mut rng = CounterRng::new(derive_seed(GeneratorCase::ErrorPe, u32::from(*truth)));
            let mut observed = std::collections::BTreeSet::new();
            for _ in 0..64 {
                let mutation = mutate_base(*truth, &mut rng).unwrap();
                assert_ne!(mutation, *truth);
                observed.insert(mutation);
            }
            assert_eq!(observed.len(), 3);
        }
    }

    #[test]
    fn generator_is_byte_deterministic_and_separates_truth() {
        let temporary = TempDir::new().unwrap();
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        let mut config = GeneratorConfig::new(first.clone(), GeneratorCase::ErrorPe);
        config.fragments = 24;
        config.truth_length = 500;
        config.read_length = 40;
        config.insert_length = 100;
        config.substitution_rate_ppm = 100_000;
        let first_manifest = generate_dataset(&config).unwrap();
        config.output_directory = second.clone();
        let second_manifest = generate_dataset(&config).unwrap();
        assert_eq!(first_manifest, second_manifest);

        for file in &first_manifest.files {
            assert_eq!(
                fs::read(first.join(&file.path)).unwrap(),
                fs::read(second.join(&file.path)).unwrap(),
                "{}",
                file.path
            );
        }
        let input_manifest =
            fs::read_to_string(first.join("assembler_input/input_manifest.json")).unwrap();
        assert!(!input_manifest.contains("truth.fasta"));
        assert!(!input_manifest.contains("mol-0001"));
        let reads = fs::read_to_string(first.join("assembler_input/reads_R1.fastq")).unwrap();
        assert!(!reads.contains("mol-"));
        assert!(!reads.contains("linear"));
    }

    #[test]
    fn circular_origin_coordinates_round_trip() {
        let molecule = Molecule {
            id: "mol".to_owned(),
            class: TruthClass::Primary,
            topology: TruthTopology::Circular,
            sequence: b"ACGTTGCA".to_vec(),
        };
        assert_eq!(truth_read(&molecule, 6, 1, 5).unwrap(), b"CAACG");
        assert_eq!(truth_read(&molecule, 1, -1, 5).unwrap(), b"GTTGC");
    }

    #[test]
    fn independent_origin_replay_covers_linear_circular_strand_and_wrap_geometries() {
        let truth = b"ACGTTGCA";
        for (topology, first, step, length) in [
            (TruthTopology::Linear, 1, 1, 5),
            (TruthTopology::Linear, 6, -1, 5),
            (TruthTopology::Circular, 6, 1, 5),
            (TruthTopology::Circular, 1, -1, 5),
        ] {
            let molecule = Molecule {
                id: "mol".to_owned(),
                class: TruthClass::Primary,
                topology,
                sequence: truth.to_vec(),
            };
            assert_eq!(
                truth_read(&molecule, first, step, length).unwrap(),
                independent_origin_replay(truth, topology, first, step, length).unwrap()
            );
        }
    }

    fn emitted_single_read(
        temporary: &TempDir,
        config: GeneratorConfig,
    ) -> (GeneratedFastqRecord, Vec<ErrorRow>, Vec<QualityEventRow>) {
        let output = temporary.path().join(format!("{}.fastq", config.case));
        let mut writer = FastqOutput::create(&output, false).unwrap();
        let motif = b"ACGTTGCATGCAACGTACGATCGTACGT";
        let sequence = motif
            .iter()
            .copied()
            .cycle()
            .take(config.truth_length)
            .collect::<Vec<_>>();
        let molecule = Molecule {
            id: "mol".to_owned(),
            class: TruthClass::Primary,
            topology: TruthTopology::Linear,
            sequence,
        };
        let seed = derive_seed(config.case, 71);
        let mut error_rng = CounterRng::new(derive_stream_seed(seed, b"error"));
        let mut quality_rng = CounterRng::new(derive_stream_seed(seed, b"quality"));
        let mut origins = Vec::new();
        let mut errors = Vec::new();
        let mut quality_events = Vec::new();
        emit_read(
            &mut writer,
            &config,
            &mut error_rng,
            &mut quality_rng,
            &mut origins,
            &mut errors,
            &mut quality_events,
            0,
            "S",
            None,
            &molecule,
            3,
            1,
            3,
            config.insert_length,
        )
        .unwrap();
        writer.finish().unwrap();
        let record = read_generated_fastq(&output, false).unwrap().remove(0);
        (record, errors, quality_events)
    }

    fn accepted_windows_with_error(
        record: &GeneratedFastqRecord,
        errors: &[ErrorRow],
        k: usize,
        minimum_quality: u8,
    ) -> usize {
        let error_offsets = errors
            .iter()
            .map(|error| error.observed_offset)
            .collect::<BTreeSet<_>>();
        let qualities = record
            .quality
            .iter()
            .map(|value| value - 33)
            .collect::<Vec<_>>();
        (0..=record.sequence.len() - k)
            .filter(|start| {
                qualities[*start..*start + k]
                    .iter()
                    .all(|quality| *quality >= minimum_quality)
                    && (*start..*start + k).any(|offset| error_offsets.contains(&offset))
            })
            .count()
    }

    #[test]
    fn error_profile_retains_injected_errors_while_qc_control_censors_them() {
        let retained_directory = TempDir::new().unwrap();
        let mut retained = GeneratorConfig::new(PathBuf::from("unused"), GeneratorCase::ErrorPe);
        retained.truth_length = 20_000;
        retained.read_length = 10_000;
        retained.insert_length = 12_000;
        let (retained_read, retained_errors, retained_quality_events) =
            emitted_single_read(&retained_directory, retained);
        let retained_error_offsets = retained_errors
            .iter()
            .map(|event| event.observed_offset)
            .collect::<BTreeSet<_>>();
        let retained_quality_offsets = retained_quality_events
            .iter()
            .map(|event| event.observed_offset)
            .collect::<BTreeSet<_>>();
        assert!(!retained_error_offsets.is_empty());
        assert!(!retained_quality_offsets.is_empty());
        assert!(
            retained_error_offsets
                .difference(&retained_quality_offsets)
                .next()
                .is_some(),
            "the default error profile must retain at least one Q40 substitution"
        );
        assert!(
            retained_quality_offsets
                .difference(&retained_error_offsets)
                .next()
                .is_some(),
            "the default error profile must emit at least one Q10 correct base"
        );
        assert!(accepted_windows_with_error(&retained_read, &retained_errors, 21, 20) > 0);

        let censoring_directory = TempDir::new().unwrap();
        let mut censoring =
            GeneratorConfig::new(PathBuf::from("unused"), GeneratorCase::QcCensoringControl);
        censoring.truth_length = 20_000;
        censoring.read_length = 10_000;
        censoring.insert_length = 12_000;
        let (censored_read, censored_errors, censoring_events) =
            emitted_single_read(&censoring_directory, censoring);
        assert!(!censored_errors.is_empty());
        assert_eq!(censoring_events.len(), censored_errors.len());
        assert_eq!(
            accepted_windows_with_error(&censored_read, &censored_errors, 21, 20),
            0
        );
    }

    fn accepted_q20_windows(records: &[GeneratedFastqRecord], k: usize) -> usize {
        records
            .iter()
            .map(|record| {
                let qualities = record
                    .quality
                    .iter()
                    .map(|value| value - 33)
                    .collect::<Vec<_>>();
                (0..=record.sequence.len() - k)
                    .filter(|start| {
                        qualities[*start..*start + k]
                            .iter()
                            .all(|quality| *quality >= 20)
                    })
                    .count()
            })
            .sum()
    }

    #[test]
    fn quality_seed_changes_only_quality_dependent_outputs() {
        let temporary = TempDir::new().unwrap();
        let master = derive_seed(GeneratorCase::ErrorPe, 9);
        let mut first =
            GeneratorConfig::new(temporary.path().join("quality-a"), GeneratorCase::ErrorPe);
        first.explicit_seed = Some(master);
        first.explicit_quality_seed = Some(derive_stream_seed(master, b"quality-a"));
        first.fragments = 32;
        first.truth_length = 500;
        first.read_length = 40;
        first.insert_length = 100;
        first.substitution_rate_ppm = 250_000;
        first.low_quality_rate_ppm = 50_000;
        generate_dataset(&first).unwrap();
        let mut second = first.clone();
        second.output_directory = temporary.path().join("quality-b");
        second.explicit_quality_seed = Some(derive_stream_seed(master, b"quality-b"));
        generate_dataset(&second).unwrap();

        assert_eq!(
            fs::read(first.output_directory.join("evaluation_truth/errors.tsv")).unwrap(),
            fs::read(second.output_directory.join("evaluation_truth/errors.tsv")).unwrap()
        );
        assert_eq!(
            fs::read(first.output_directory.join("evaluation_truth/truth.fasta")).unwrap(),
            fs::read(second.output_directory.join("evaluation_truth/truth.fasta")).unwrap()
        );
        let first_reads = read_generated_fastq(
            &first
                .output_directory
                .join("assembler_input/reads_R1.fastq"),
            false,
        )
        .unwrap();
        let second_reads = read_generated_fastq(
            &second
                .output_directory
                .join("assembler_input/reads_R1.fastq"),
            false,
        )
        .unwrap();
        assert!(first_reads
            .iter()
            .zip(&second_reads)
            .all(|(left, right)| left.id == right.id && left.sequence == right.sequence));
        assert!(first_reads
            .iter()
            .zip(&second_reads)
            .any(|(left, right)| left.quality != right.quality));
        assert_ne!(
            accepted_q20_windows(&first_reads, 21),
            accepted_q20_windows(&second_reads, 21)
        );
    }

    #[test]
    fn negative_strand_error_ledger_uses_pre_error_read_orientation() {
        let temporary = TempDir::new().unwrap();
        let output = temporary.path().join("negative.fastq");
        let mut writer = FastqOutput::create(&output, false).unwrap();
        let mut config = GeneratorConfig::new(
            temporary.path().join("unused-dataset"),
            GeneratorCase::ErrorPe,
        );
        config.read_length = 5;
        config.substitution_rate_ppm = 1_000_000;
        config.low_quality_rate_ppm = 0;
        let molecule = Molecule {
            id: "mol".to_owned(),
            class: TruthClass::Primary,
            topology: TruthTopology::Circular,
            sequence: b"ACGTTGCA".to_vec(),
        };
        let mut origins = Vec::new();
        let mut errors = Vec::new();
        let mut quality_events = Vec::new();
        let seed = derive_seed(GeneratorCase::ErrorPe, 19);
        let mut error_rng = CounterRng::new(derive_stream_seed(seed, b"error"));
        let mut quality_rng = CounterRng::new(derive_stream_seed(seed, b"quality"));
        emit_read(
            &mut writer,
            &config,
            &mut error_rng,
            &mut quality_rng,
            &mut origins,
            &mut errors,
            &mut quality_events,
            0,
            "S",
            None,
            &molecule,
            1,
            -1,
            0,
            5,
        )
        .unwrap();
        writer.finish().unwrap();

        assert_eq!(origins.len(), 1);
        assert_eq!(origins[0].strand, '-');
        assert_eq!(origins[0].truth_step, -1);
        assert_eq!(origins[0].pre_error_sha256, sha256_bytes(b"GTTGC"));
        assert_eq!(errors.len(), 5);
        assert_eq!(
            errors
                .iter()
                .map(|error| error.truth_base)
                .collect::<Vec<_>>(),
            b"GTTGC"
        );
        assert_eq!(
            errors
                .iter()
                .map(|error| error.truth_coordinate)
                .collect::<Vec<_>>(),
            [1, 0, 7, 6, 5]
        );
        assert!(errors
            .iter()
            .all(|error| error.truth_base != error.observed_base));
    }

    #[test]
    fn opposite_truth_strands_compact_to_one_reverse_complement_orbit() {
        use crate::compact::compact_graph;
        use crate::dna::scan_canonical_kmers;
        use crate::graph::ExactGraph;
        use crate::model::KmerCount;

        let molecule = Molecule {
            id: "mol".to_owned(),
            class: TruthClass::Primary,
            topology: TruthTopology::Linear,
            sequence: b"AACGCTA".to_vec(),
        };
        let forward = truth_read(&molecule, 0, 1, molecule.sequence.len()).unwrap();
        let reverse = truth_read(
            &molecule,
            molecule.sequence.len() - 1,
            -1,
            molecule.sequence.len(),
        )
        .unwrap();
        assert_eq!(forward, b"AACGCTA");
        assert_eq!(reverse, b"TAGCGTT");

        let mut counts = BTreeMap::new();
        for sequence in [forward, reverse] {
            for key in scan_canonical_kmers(&sequence, None, 5, 0).unwrap().kmers {
                *counts.entry(key).or_insert(0u64) += 1;
            }
        }
        let retained = counts
            .into_iter()
            .map(|(key, support)| KmerCount { key, support })
            .collect::<Vec<_>>();
        let graph = ExactGraph::from_sorted_retained(5, &retained, 64 << 20).unwrap();
        let result = compact_graph(&graph, 64 << 20).unwrap();

        assert_eq!(result.unitigs.len(), 1);
        assert_eq!(result.unitigs[0].sequence, b"AACGCTA");
        assert_eq!(result.unitigs[0].minimum_support, 2);
        assert_eq!(result.unitigs[0].maximum_support, 2);
    }

    #[test]
    fn mixture_has_exact_floor_ten_percent_minor_fragments() {
        let seed = derive_seed(GeneratorCase::MixturePe, 0);
        let mut rng = CounterRng::new(seed);
        let config = GeneratorConfig {
            output_directory: PathBuf::from("unused"),
            case: GeneratorCase::MixturePe,
            replicate: 0,
            explicit_seed: None,
            fragments: 23,
            truth_length: 500,
            read_length: 40,
            insert_length: 100,
            substitution_rate_ppm: 0,
            low_quality_rate_ppm: 0,
            explicit_quality_seed: None,
            gzip: false,
        };
        let assignments = molecule_assignments(&config, 2, &mut rng).unwrap();
        assert_eq!(assignments.iter().filter(|value| **value == 1).count(), 2);
    }

    #[test]
    fn gzip_generation_passes_origin_and_error_replay() {
        let temporary = TempDir::new().unwrap();
        let mut config =
            GeneratorConfig::new(temporary.path().join("gzip-error"), GeneratorCase::ErrorPe);
        config.fragments = 12;
        config.truth_length = 300;
        config.read_length = 30;
        config.insert_length = 70;
        config.substitution_rate_ppm = 250_000;
        config.gzip = true;
        let manifest = generate_dataset(&config).unwrap();
        for path in &manifest.assembler_input.read_paths {
            assert_eq!(
                &fs::read(config.output_directory.join(path)).unwrap()[..2],
                b"\x1f\x8b"
            );
        }
    }

    #[test]
    fn generator_caps_have_explicit_boundaries() {
        let mut config = GeneratorConfig::new(PathBuf::from("unused"), GeneratorCase::LinearSe);
        config.fragments = 0;
        config.read_length = 1;
        config.insert_length = 1;

        config.truth_length = MAX_GENERATED_TRUTH_LENGTH - 1;
        assert!(validate_config(&config).is_ok());
        config.truth_length = MAX_GENERATED_TRUTH_LENGTH;
        assert!(validate_config(&config).is_ok());
        config.truth_length = MAX_GENERATED_TRUTH_LENGTH + 1;
        let error = validate_config(&config).unwrap_err();
        assert!(matches!(error, ValidationError::Resource(_)));
        assert!(error
            .to_string()
            .contains("truth length exceeds generator cap"));

        config.truth_length = MAX_GENERATED_TRUTH_LENGTH;
        config.read_length = MAX_GENERATED_READ_LENGTH;
        assert!(validate_config(&config).is_ok());
        config.read_length = MAX_GENERATED_READ_LENGTH + 1;
        let error = validate_config(&config).unwrap_err();
        assert!(matches!(error, ValidationError::Resource(_)));
        assert!(error
            .to_string()
            .contains("read length exceeds generator cap"));

        config.read_length = 1;
        config.fragments = MAX_GENERATED_FRAGMENTS;
        assert!(validate_config(&config).is_ok());
        config.fragments = MAX_GENERATED_FRAGMENTS + 1;
        let error = validate_config(&config).unwrap_err();
        assert!(matches!(error, ValidationError::Resource(_)));
        assert!(error
            .to_string()
            .contains("fragments exceeds generator cap"));
    }

    #[test]
    fn generator_and_manifest_paths_share_one_feasibility_contract() {
        let assert_same = |config: &GeneratorConfig| {
            let generated = validate_config(config)
                .map(|size| size.read_count)
                .map_err(|error| error.to_string());
            let declared =
                validate_generator_parameters(config.case, &parameters_from_config(config))
                    .map(|parameters| parameters.read_count)
                    .map_err(|error| error.to_string());
            assert_eq!(generated, declared);
        };

        for case in [
            GeneratorCase::LinearSe,
            GeneratorCase::LinearPe,
            GeneratorCase::CircularPe,
            GeneratorCase::MixturePe,
            GeneratorCase::ErrorPe,
            GeneratorCase::QcCensoringControl,
        ] {
            assert_same(&GeneratorConfig::new(PathBuf::from("unused"), case));
        }

        let mut invalid = GeneratorConfig::new(PathBuf::from("unused"), GeneratorCase::LinearPe);
        invalid.fragments = MAX_GENERATED_FRAGMENTS + 1;
        assert_same(&invalid);
        invalid.fragments = 1;
        invalid.truth_length = MAX_GENERATED_TRUTH_LENGTH + 1;
        assert_same(&invalid);
        invalid.truth_length = 100;
        invalid.read_length = 40;
        invalid.insert_length = 39;
        assert_same(&invalid);
        invalid.insert_length = 101;
        assert_same(&invalid);
        invalid.insert_length = 80;
        invalid.substitution_rate_ppm = 1_000_001;
        assert_same(&invalid);

        let mut conservative =
            GeneratorConfig::new(PathBuf::from("unused"), GeneratorCase::ErrorPe);
        conservative.fragments = 10_000;
        conservative.truth_length = 1_000;
        conservative.read_length = 100;
        conservative.insert_length = 250;
        conservative.substitution_rate_ppm = 1;
        assert_same(&conservative);

        let mut noncanonical = parameters_from_config(&GeneratorConfig::new(
            PathBuf::from("unused"),
            GeneratorCase::LinearSe,
        ));
        noncanonical.fragments_decimal = "01".to_owned();
        assert!(validate_generator_parameters(GeneratorCase::LinearSe, &noncanonical).is_err());
        noncanonical.fragments_decimal = "200".to_owned();
        noncanonical.error_model = "unfrozen".to_owned();
        assert!(validate_generator_parameters(GeneratorCase::LinearSe, &noncanonical).is_err());
    }

    #[test]
    fn generator_rejects_read_base_and_conservative_output_overruns() {
        let mut reads = GeneratorConfig::new(PathBuf::from("unused"), GeneratorCase::LinearPe);
        reads.fragments = MAX_GENERATED_FRAGMENTS;
        reads.truth_length = 1_000;
        reads.read_length = 126;
        reads.insert_length = 126;
        let error = validate_config(&reads).unwrap_err();
        assert!(matches!(error, ValidationError::Resource(_)));
        assert!(error.to_string().contains("generated read bases"));

        let mut ledger = GeneratorConfig::new(PathBuf::from("unused"), GeneratorCase::ErrorPe);
        ledger.fragments = 10_000;
        ledger.truth_length = 1_000;
        ledger.read_length = 100;
        ledger.insert_length = 250;
        ledger.substitution_rate_ppm = 1;
        let error = validate_config(&ledger).unwrap_err();
        assert!(matches!(error, ValidationError::Resource(_)));
        assert!(error
            .to_string()
            .contains("conservative generated output estimate"));
    }
}
