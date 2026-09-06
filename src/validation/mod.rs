//! Deterministic truth-known validation support.
//!
//! This module is deliberately outside the assembly pipeline. Generated truth
//! and read-origin records are written below `evaluation_truth/`; callers must
//! give assemblers only the paths declared in `assembler_input`.

mod evaluator;
mod generator;

pub use evaluator::{
    evaluate_dataset, validate_evaluation_result, DatasetBindingMode, DatasetBindingRecord,
    EvaluationConfig, EvaluationResult, JunctionEvidenceMode, JunctionEvidenceRecord,
};
pub use generator::{
    derive_seed, generate_dataset, DatasetManifest, GeneratorCase, GeneratorConfig, Seed256,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

const MAX_VALIDATION_FASTA_IDENTIFIER_BYTES: usize = 128;
const VALIDATION_COORDINATE_SYSTEM: &str = "zero_based_truth_coordinates; strand gives +1/-1 traversal and negative-strand read bases complement truth; circular coordinates are modulo molecule length";

/// Result type for validation tooling.
pub type ValidationResult<T> = std::result::Result<T, ValidationError>;

/// Typed failures from the simulator and evaluator.
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("validation configuration invalid: {0}")]
    Configuration(String),
    #[error("validation input invalid: {0}")]
    Input(String),
    #[error("validation integrity failure: {0}")]
    Integrity(String),
    #[error("validation resource limit: {0}")]
    Resource(String),
    #[error("validation I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("validation JSON failure: {0}")]
    Json(#[from] serde_json::Error),
}

/// Experimental validation-tool exit and diagnostic class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationExitClass {
    Configuration,
    Input,
    Integrity,
    Resource,
    Io,
    Json,
}

impl ValidationExitClass {
    /// Fixed single-record label for experimental validation CLI diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "validation_configuration",
            Self::Input => "validation_input",
            Self::Integrity => "validation_integrity",
            Self::Resource => "validation_resource",
            Self::Io => "validation_io",
            Self::Json => "validation_json",
        }
    }

    /// Process exit class. Values align with the stable CLI's broad classes.
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Configuration => 2,
            Self::Input | Self::Json => 3,
            Self::Resource => 5,
            Self::Integrity => 6,
            Self::Io => 8,
        }
    }

    const fn for_error(error: &ValidationError) -> Self {
        match error {
            ValidationError::Configuration(_) => Self::Configuration,
            ValidationError::Input(_) => Self::Input,
            ValidationError::Integrity(_) => Self::Integrity,
            ValidationError::Resource(_) => Self::Resource,
            ValidationError::Io(_) => Self::Io,
            ValidationError::Json(_) => Self::Json,
        }
    }
}

/// One control-safe, single-line validation CLI diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationDiagnostic {
    class: ValidationExitClass,
    message: String,
}

impl ValidationDiagnostic {
    /// Converts one typed validation error without retaining raw controls.
    pub fn from_error(error: &ValidationError) -> Self {
        Self {
            class: ValidationExitClass::for_error(error),
            message: sanitize_diagnostic(error.to_string()),
        }
    }

    /// Returns the typed exit and diagnostic class.
    pub const fn class(&self) -> ValidationExitClass {
        self.class
    }

    /// Returns sanitized diagnostic content without its code prefix.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ValidationDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "error[{}]: {}",
            self.class.as_str(),
            self.message
        )
    }
}

fn sanitize_diagnostic(message: String) -> String {
    if !message
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return message;
    }
    let mut sanitized = String::with_capacity(message.len());
    for character in message.chars() {
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            sanitized.extend(character.escape_default());
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

/// Molecular topology supplied by truth, not inferred from assembly output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthTopology {
    Linear,
    Circular,
}

/// Truth-only component role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthClass {
    Primary,
    Minor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FastaRecord {
    id: String,
    sequence: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FastaSnapshot {
    records: Vec<FastaRecord>,
    byte_length: u64,
    sha256: String,
}

/// Held directory identity used to keep a multi-file validation run beneath
/// one selected dataset root even if its pathname is concurrently replaced.
struct DirectoryAnchor {
    descriptor: OwnedFd,
    display_path: PathBuf,
}

/// Reads, hashes, and parses one bounded FASTA from the same owned byte snapshot.
///
/// The returned digest therefore identifies the exact bytes represented by
/// `records`, even if the caller-supplied path is replaced after this function
/// returns. Callers must not reopen the path to establish provenance for those
/// records.
fn read_fasta_snapshot(
    path: &Path,
    allow_empty: bool,
    max_bytes: u64,
    max_records: usize,
    label: &str,
) -> ValidationResult<FastaSnapshot> {
    let bytes = read_file_bounded(path, max_bytes, label)?;
    parse_fasta_snapshot(bytes, path, allow_empty, max_records, label)
}

fn read_fasta_snapshot_beneath(
    root: &DirectoryAnchor,
    relative: &Path,
    allow_empty: bool,
    max_bytes: u64,
    max_records: usize,
    label: &str,
) -> ValidationResult<FastaSnapshot> {
    let bytes = read_file_bounded_beneath(root, relative, max_bytes, label)?;
    let display_path = root.display_path.join(relative);
    parse_fasta_snapshot(bytes, &display_path, allow_empty, max_records, label)
}

fn parse_fasta_snapshot(
    bytes: Vec<u8>,
    path: &Path,
    allow_empty: bool,
    max_records: usize,
    label: &str,
) -> ValidationResult<FastaSnapshot> {
    let byte_length = u64::try_from(bytes.len())
        .map_err(|_| ValidationError::Resource(format!("{label} size exceeds u64")))?;
    let sha256 = sha256_bytes(&bytes);
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| ValidationError::Input(format!("{}: {error}", path.display())))?;
    let mut records = Vec::<FastaRecord>::new();
    let mut identifiers = BTreeSet::new();
    for (line_index, line) in text.lines().enumerate() {
        if let Some(header) = line.strip_prefix('>') {
            if records.len() == max_records {
                return Err(ValidationError::Resource(format!(
                    "{label} record count exceeds fixed cap {max_records}"
                )));
            }
            let id = header.split_ascii_whitespace().next().unwrap_or_default();
            if id.is_empty() {
                return Err(ValidationError::Input(format!(
                    "{}:{}: empty FASTA identifier",
                    path.display(),
                    line_index + 1
                )));
            }
            if id.len() > MAX_VALIDATION_FASTA_IDENTIFIER_BYTES {
                return Err(ValidationError::Resource(format!(
                    "{label} identifier length exceeds fixed cap {MAX_VALIDATION_FASTA_IDENTIFIER_BYTES} bytes"
                )));
            }
            if !id.bytes().all(|byte| (b'!'..=b'~').contains(&byte)) {
                return Err(ValidationError::Input(format!(
                    "{}:{}: FASTA identifier must use visible ASCII",
                    path.display(),
                    line_index + 1
                )));
            }
            if !identifiers.insert(id.to_owned()) {
                return Err(ValidationError::Input(format!(
                    "{}:{}: duplicate FASTA identifier {id:?}",
                    path.display(),
                    line_index + 1
                )));
            }
            records.push(FastaRecord {
                id: id.to_owned(),
                sequence: Vec::new(),
            });
        } else if !line.trim().is_empty() {
            let record = records.last_mut().ok_or_else(|| {
                ValidationError::Input(format!(
                    "{}:{}: sequence before FASTA header",
                    path.display(),
                    line_index + 1
                ))
            })?;
            for base in line.trim().bytes() {
                let normalized = base.to_ascii_uppercase();
                if !matches!(normalized, b'A' | b'C' | b'G' | b'T') {
                    return Err(ValidationError::Input(format!(
                        "{}:{}: evaluator accepts only A/C/G/T",
                        path.display(),
                        line_index + 1
                    )));
                }
                record.sequence.push(normalized);
            }
        }
    }
    if records.iter().any(|record| record.sequence.is_empty()) {
        return Err(ValidationError::Input(format!(
            "{}: empty FASTA record",
            path.display()
        )));
    }
    if records.is_empty() && !allow_empty {
        return Err(ValidationError::Input(format!(
            "{}: empty FASTA input",
            path.display()
        )));
    }
    Ok(FastaSnapshot {
        records,
        byte_length,
        sha256,
    })
}

fn write_fasta(path: &Path, records: &[FastaRecord]) -> ValidationResult<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    for record in records {
        writeln!(writer, ">{id}", id = record.id)?;
        for line in record.sequence.chunks(80) {
            writer.write_all(line)?;
            writer.write_all(b"\n")?;
        }
    }
    writer.flush()?;
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> ValidationResult<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(lower_hex(&digest.finalize()))
}

fn open_directory_anchor(path: &Path, label: &str) -> ValidationResult<DirectoryAnchor> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let descriptor = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| {
        ValidationError::Io(std::io::Error::other(format!(
            "cannot open {label} {}: {error}",
            path.display()
        )))
    })?;
    Ok(DirectoryAnchor {
        descriptor,
        display_path: path.to_owned(),
    })
}

fn opened_regular_file(
    descriptor: OwnedFd,
    max_bytes: u64,
    label: &str,
    display_path: &Path,
) -> ValidationResult<(File, u64)> {
    let file = File::from(descriptor);
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(ValidationError::Input(format!(
            "{label} is not a regular non-symlink file: {}",
            display_path.display()
        )));
    }
    if metadata.len() > max_bytes {
        return Err(ValidationError::Resource(format!(
            "{label} is {} bytes, above fixed cap {max_bytes}",
            metadata.len()
        )));
    }
    Ok((file, metadata.len()))
}

fn open_regular_file_nofollow(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> ValidationResult<(File, u64)> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    // NONBLOCK prevents a concurrently substituted FIFO from hanging before
    // its opened-handle type can be rejected. It has no effect on regular-file
    // reads. NOFOLLOW binds the final path component to a non-symlink handle.
    let descriptor = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| {
        ValidationError::Io(std::io::Error::other(format!(
            "cannot open {label} {} without following a symlink: {error}",
            path.display()
        )))
    })?;
    opened_regular_file(descriptor, max_bytes, label, path)
}

fn open_regular_file_beneath(
    root: &DirectoryAnchor,
    relative: &Path,
    max_bytes: u64,
    label: &str,
) -> ValidationResult<(File, u64)> {
    use rustix::fs::{openat, Mode, OFlags};

    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return Err(ValidationError::Integrity(format!(
            "unsafe relative {label} path: {relative:?}"
        )));
    }
    let mut components = relative.components().peekable();
    let mut directory: Option<OwnedFd> = None;
    while let Some(component) = components.next() {
        let Component::Normal(segment) = component else {
            return Err(ValidationError::Integrity(format!(
                "unsafe relative {label} path: {relative:?}"
            )));
        };
        let is_final = components.peek().is_none();
        let descriptor = match directory.as_ref() {
            Some(parent) => {
                if is_final {
                    openat(
                        parent,
                        segment,
                        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                } else {
                    openat(
                        parent,
                        segment,
                        OFlags::RDONLY
                            | OFlags::CLOEXEC
                            | OFlags::DIRECTORY
                            | OFlags::NOFOLLOW
                            | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                }
            }
            None => {
                if is_final {
                    openat(
                        &root.descriptor,
                        segment,
                        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                } else {
                    openat(
                        &root.descriptor,
                        segment,
                        OFlags::RDONLY
                            | OFlags::CLOEXEC
                            | OFlags::DIRECTORY
                            | OFlags::NOFOLLOW
                            | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                }
            }
        }
        .map_err(|error| {
            ValidationError::Io(std::io::Error::other(format!(
                "cannot open {label} {} beneath anchored directory {}: {error}",
                relative.display(),
                root.display_path.display()
            )))
        })?;
        if is_final {
            return opened_regular_file(
                descriptor,
                max_bytes,
                label,
                &root.display_path.join(relative),
            );
        }
        directory = Some(descriptor);
    }
    Err(ValidationError::Integrity(format!(
        "unsafe relative {label} path: {relative:?}"
    )))
}

#[cfg(test)]
fn sha256_file_exact_bounded(
    path: &Path,
    expected_bytes: u64,
    max_bytes: u64,
    label: &str,
) -> ValidationResult<String> {
    if expected_bytes > max_bytes {
        return Err(ValidationError::Resource(format!(
            "{label} declares {expected_bytes} bytes, above fixed cap {max_bytes}"
        )));
    }
    let (file, opened_bytes) = open_regular_file_nofollow(path, max_bytes, label)?;
    sha256_opened_file_exact_bounded(file, opened_bytes, expected_bytes, label)
}

fn sha256_file_exact_bounded_beneath(
    root: &DirectoryAnchor,
    relative: &Path,
    expected_bytes: u64,
    max_bytes: u64,
    label: &str,
) -> ValidationResult<String> {
    if expected_bytes > max_bytes {
        return Err(ValidationError::Resource(format!(
            "{label} declares {expected_bytes} bytes, above fixed cap {max_bytes}"
        )));
    }
    let (file, opened_bytes) = open_regular_file_beneath(root, relative, max_bytes, label)?;
    sha256_opened_file_exact_bounded(file, opened_bytes, expected_bytes, label)
}

fn sha256_opened_file_exact_bounded(
    mut file: File,
    opened_bytes: u64,
    expected_bytes: u64,
    label: &str,
) -> ValidationResult<String> {
    if opened_bytes != expected_bytes {
        return Err(ValidationError::Integrity(format!(
            "{label} byte count mismatch before hashing: expected {expected_bytes}, opened {opened_bytes}"
        )));
    }

    let mut digest = Sha256::new();
    let mut consumed = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    while consumed < expected_bytes {
        let remaining = expected_bytes - consumed;
        let requested = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded digest request fits the fixed buffer");
        let read = file.read(&mut buffer[..requested])?;
        if read == 0 {
            return Err(ValidationError::Integrity(format!(
                "{label} ended after {consumed} of {expected_bytes} declared bytes"
            )));
        }
        digest.update(&buffer[..read]);
        consumed = consumed
            .checked_add(u64::try_from(read).map_err(|_| {
                ValidationError::Resource(format!("{label} read count exceeds u64"))
            })?)
            .ok_or_else(|| ValidationError::Resource(format!("{label} byte count overflow")))?;
    }
    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 {
        return Err(ValidationError::Resource(format!(
            "{label} grew beyond its declared {expected_bytes} bytes while being hashed"
        )));
    }
    let final_metadata = file.metadata()?;
    if !final_metadata.is_file() || final_metadata.len() != expected_bytes {
        return Err(ValidationError::Integrity(format!(
            "{label} metadata changed while being hashed"
        )));
    }
    Ok(lower_hex(&digest.finalize()))
}

fn read_file_bounded(path: &Path, max_bytes: u64, label: &str) -> ValidationResult<Vec<u8>> {
    let (reader, opened_bytes) = open_regular_file_nofollow(path, max_bytes, label)?;
    read_opened_file_bounded(reader, opened_bytes, max_bytes, label)
}

fn read_file_bounded_beneath(
    root: &DirectoryAnchor,
    relative: &Path,
    max_bytes: u64,
    label: &str,
) -> ValidationResult<Vec<u8>> {
    let (reader, opened_bytes) = open_regular_file_beneath(root, relative, max_bytes, label)?;
    read_opened_file_bounded(reader, opened_bytes, max_bytes, label)
}

fn read_opened_file_bounded(
    mut reader: File,
    opened_bytes: u64,
    max_bytes: u64,
    label: &str,
) -> ValidationResult<Vec<u8>> {
    let capacity = usize::try_from(opened_bytes)
        .map_err(|_| ValidationError::Resource(format!("{label} byte count exceeds usize")))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|error| {
        ValidationError::Resource(format!("cannot allocate {label} buffer: {error}"))
    })?;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let next_length = bytes
            .len()
            .checked_add(read)
            .ok_or_else(|| ValidationError::Resource(format!("{label} size overflow")))?;
        if u64::try_from(next_length)
            .map_err(|_| ValidationError::Resource(format!("{label} size exceeds u64")))?
            > max_bytes
        {
            return Err(ValidationError::Resource(format!(
                "{label} grew above fixed cap {max_bytes} while being read"
            )));
        }
        bytes.try_reserve(read).map_err(|error| {
            ValidationError::Resource(format!("cannot grow {label} buffer: {error}"))
        })?;
        bytes.extend_from_slice(&chunk[..read]);
    }
    let observed_bytes = u64::try_from(bytes.len())
        .map_err(|_| ValidationError::Resource(format!("{label} size exceeds u64")))?;
    let final_metadata = reader.metadata()?;
    if observed_bytes != opened_bytes
        || !final_metadata.is_file()
        || final_metadata.len() != opened_bytes
    {
        return Err(ValidationError::Integrity(format!(
            "{label} changed while being read"
        )));
    }
    Ok(bytes)
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn decode_hex_32(value: &str) -> ValidationResult<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ValidationError::Configuration(
            "seed must be exactly 64 lowercase hexadecimal characters".to_owned(),
        ));
    }
    let mut bytes = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).expect("validated ASCII hex");
        bytes[index] = u8::from_str_radix(text, 16).map_err(|error| {
            ValidationError::Configuration(format!("invalid seed hexadecimal: {error}"))
        })?;
    }
    Ok(bytes)
}

fn checked_relative_path(path: &str) -> ValidationResult<PathBuf> {
    if path.is_empty()
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
        || path.starts_with('/')
        || path.ends_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(ValidationError::Integrity(format!(
            "unsafe or nonportable relative path in manifest: {path:?}"
        )));
    }
    let candidate = Path::new(path);
    if candidate.as_os_str().is_empty() || candidate.is_absolute() {
        return Err(ValidationError::Integrity(format!(
            "unsafe relative path in manifest: {path:?}"
        )));
    }
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        let Component::Normal(segment) = component else {
            return Err(ValidationError::Integrity(format!(
                "unsafe relative path in manifest: {path:?}"
            )));
        };
        normalized.push(segment);
    }
    if normalized.to_str() != Some(path) {
        return Err(ValidationError::Integrity(format!(
            "noncanonical relative path in manifest: {path:?}"
        )));
    }
    Ok(normalized)
}

fn reverse_complement(sequence: &[u8]) -> Vec<u8> {
    sequence
        .iter()
        .rev()
        .map(|base| match base {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            _ => unreachable!("validated DNA has only A/C/G/T"),
        })
        .collect()
}

fn decimal_u64(value: &str, field: &str) -> ValidationResult<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return Err(ValidationError::Integrity(format!(
            "{field} is not canonical unsigned decimal"
        )));
    }
    value.parse::<u64>().map_err(|error| {
        ValidationError::Integrity(format!("{field} is not a u64 decimal: {error}"))
    })
}

fn write_json(path: &Path, value: &impl Serialize) -> ValidationResult<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn write_checksum_manifest(root: &Path, relative_paths: &[String]) -> ValidationResult<()> {
    let mut paths = relative_paths.to_vec();
    paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    paths.dedup();
    let mut writer = BufWriter::new(File::create(root.join("manifest.sha256"))?);
    for relative in paths {
        let path = checked_relative_path(&relative)?;
        let digest = sha256_file(&root.join(path))?;
        writeln!(writer, "{digest}  {relative}")?;
    }
    writer.flush()?;
    Ok(())
}

fn staging_directory(destination: &Path, prefix: &str) -> ValidationResult<tempfile::TempDir> {
    if destination.file_name().is_none() {
        return Err(ValidationError::Configuration(
            "output must name a new directory".to_owned(),
        ));
    }
    if destination.try_exists()? {
        return Err(ValidationError::Configuration(format!(
            "output already exists: {}",
            destination.display()
        )));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(ValidationError::Configuration(format!(
            "output parent is not an existing directory: {}",
            parent.display()
        )));
    }
    Ok(tempfile::Builder::new().prefix(prefix).tempdir_in(parent)?)
}

fn commit_staging(staging: &tempfile::TempDir, destination: &Path) -> ValidationResult<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};

    renameat_with(
        CWD,
        staging.path(),
        CWD,
        destination,
        RenameFlags::NOREPLACE,
    )
    .map_err(|error| {
        ValidationError::Io(std::io::Error::other(format!(
            "cannot commit new output directory {}: {error}",
            destination.display()
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rejects_noncanonical_seed_and_paths() {
        assert!(decode_hex_32(&"A".repeat(64)).is_err());
        assert!(decode_hex_32("00").is_err());
        for accepted in [
            "dataset.json",
            "assembler_input/reads_R1.fastq.gz",
            "evaluation_truth/.retained-name_1-2",
            "a/.../b",
        ] {
            assert!(checked_relative_path(accepted).is_ok(), "{accepted:?}");
        }
        for rejected in [
            "",
            "../truth.fasta",
            "/truth.fasta",
            "trailing/",
            "truth//truth.fasta",
            "truth\\truth.fasta",
            "truth/tab\tname",
            "truth/new\nname",
            "truth/./truth.fasta",
            "truth/truth fasta",
            "café",
        ] {
            assert!(checked_relative_path(rejected).is_err(), "{rejected:?}");
        }
    }

    #[test]
    fn reverse_complement_round_trips() {
        let sequence = b"AACCGT";
        assert_eq!(reverse_complement(&reverse_complement(sequence)), sequence);
    }

    #[test]
    fn bounded_reader_accepts_limit_and_rejects_limit_plus_one() {
        let temporary = tempfile::TempDir::new().unwrap();
        let path = temporary.path().join("input");
        for length in [7usize, 8] {
            fs::write(&path, vec![b'A'; length]).unwrap();
            assert_eq!(
                read_file_bounded(&path, 8, "test input").unwrap().len(),
                length
            );
        }
        fs::write(&path, vec![b'A'; 9]).unwrap();
        let error = read_file_bounded(&path, 8, "test input").unwrap_err();
        assert!(matches!(error, ValidationError::Resource(_)));
        assert!(error.to_string().contains("above fixed cap 8"));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_does_not_follow_the_final_symlink() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::TempDir::new().unwrap();
        let target = temporary.path().join("target");
        let link = temporary.path().join("link");
        fs::write(&target, b"ACGT").unwrap();
        symlink(&target, &link).unwrap();
        assert!(read_file_bounded(&link, 8, "test input").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn anchored_reader_rejects_symlinked_descendants_and_survives_root_replacement() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path().join("dataset");
        let moved_root = temporary.path().join("dataset-original");
        let outside = temporary.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/artifact"), b"original").unwrap();
        fs::write(outside.join("artifact"), b"replacement").unwrap();
        symlink(&outside, root.join("redirect")).unwrap();

        let anchor = open_directory_anchor(&root, "test root").unwrap();
        assert!(read_file_bounded_beneath(
            &anchor,
            Path::new("redirect/artifact"),
            64,
            "test artifact"
        )
        .is_err());

        fs::rename(&root, &moved_root).unwrap();
        symlink(&outside, &root).unwrap();
        assert_eq!(
            read_file_bounded_beneath(&anchor, Path::new("nested/artifact"), 64, "test artifact")
                .unwrap(),
            b"original"
        );
    }

    #[test]
    fn exact_bounded_digest_enforces_opened_length_and_limit() {
        let temporary = tempfile::TempDir::new().unwrap();
        let path = temporary.path().join("artifact");
        fs::write(&path, b"ACGT").unwrap();
        assert_eq!(
            sha256_file_exact_bounded(&path, 4, 4, "test artifact").unwrap(),
            sha256_bytes(b"ACGT")
        );
        assert!(matches!(
            sha256_file_exact_bounded(&path, 3, 4, "test artifact"),
            Err(ValidationError::Integrity(_))
        ));
        assert!(matches!(
            sha256_file_exact_bounded(&path, 4, 3, "test artifact"),
            Err(ValidationError::Resource(_))
        ));
    }

    #[test]
    fn fasta_snapshot_digest_remains_bound_to_the_parsed_bytes() {
        let temporary = tempfile::TempDir::new().unwrap();
        let path = temporary.path().join("assembly.fasta");
        let original = b">first\nACGT\n";
        fs::write(&path, original).unwrap();

        let snapshot = read_fasta_snapshot(&path, false, 1024, 4, "test FASTA").unwrap();
        fs::write(&path, b">replacement\nTTTT\n").unwrap();

        assert_eq!(snapshot.byte_length, u64::try_from(original.len()).unwrap());
        assert_eq!(snapshot.sha256, sha256_bytes(original));
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.records[0].id, "first");
        assert_eq!(snapshot.records[0].sequence, b"ACGT");
        assert_ne!(snapshot.sha256, sha256_file(&path).unwrap());
    }

    #[test]
    fn validation_diagnostic_is_typed_control_safe_and_single_line() {
        let error = ValidationError::Configuration(
            "path\nforged\rfield\tvalue\u{1b}[31m\u{2028}next\u{2029}last".to_owned(),
        );
        let diagnostic = ValidationDiagnostic::from_error(&error);
        assert_eq!(diagnostic.class(), ValidationExitClass::Configuration);
        assert_eq!(diagnostic.class().exit_code(), 2);
        assert!(diagnostic
            .message()
            .contains("path\\nforged\\rfield\\tvalue"));
        assert!(diagnostic.message().contains("\\u{1b}"));
        assert!(diagnostic.message().contains("\\u{2028}"));
        assert!(diagnostic.message().contains("\\u{2029}"));
        assert!(diagnostic
            .to_string()
            .chars()
            .all(|character| !character.is_control()
                && !matches!(character, '\u{2028}' | '\u{2029}')));
        assert_eq!(diagnostic.to_string().lines().count(), 1);
    }

    #[test]
    fn validation_exit_policy_maps_every_error_family() {
        let cases = [
            (
                ValidationError::Configuration("configuration".to_owned()),
                ValidationExitClass::Configuration,
                2,
            ),
            (
                ValidationError::Input("input".to_owned()),
                ValidationExitClass::Input,
                3,
            ),
            (
                ValidationError::Resource("resource".to_owned()),
                ValidationExitClass::Resource,
                5,
            ),
            (
                ValidationError::Integrity("integrity".to_owned()),
                ValidationExitClass::Integrity,
                6,
            ),
            (
                ValidationError::Io(std::io::Error::other("I/O")),
                ValidationExitClass::Io,
                8,
            ),
        ];
        for (error, expected_class, expected_exit) in cases {
            let diagnostic = ValidationDiagnostic::from_error(&error);
            assert_eq!(diagnostic.class(), expected_class);
            assert_eq!(diagnostic.class().exit_code(), expected_exit);
        }
        let json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let diagnostic = ValidationDiagnostic::from_error(&ValidationError::Json(json));
        assert_eq!(diagnostic.class(), ValidationExitClass::Json);
        assert_eq!(diagnostic.class().exit_code(), 3);
    }
}
