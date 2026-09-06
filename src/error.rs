//! Stable error codes shared by the library and command-line interface.

use std::fmt;

/// Result type used throughout VeritAsm.
pub type Result<T> = std::result::Result<T, VeritasmError>;

/// Stable machine-facing error codes from the current CLI contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    ConfigurationInvalidK,
    ConfigurationProfileConflict,
    ConfigurationInvalidSupport,
    ConfigurationInvalidLimit,
    ConfigurationUnsupportedCombination,
    InputOpen,
    InputRead,
    InputEmpty,
    InputFormat,
    InputHeader,
    InputFastaStructure,
    InputFastqStructure,
    InputNucleotide,
    InputQuality,
    InputDecompression,
    InputRawTransportLimit,
    InputDecodedLimit,
    InputGzipMemberLimit,
    PairLaneCount,
    PairFormat,
    PairIdentity,
    PairRole,
    PairOrderOrCardinality,
    PairPhysicalSourceReuse,
    ResourceMemory,
    ResourceSpoolBytes,
    ResourceTemporaryBytes,
    ResourceOpenFiles,
    ResourceRunCount,
    ResourceManifestBytes,
    ResourceRetainedKeys,
    ResourceMappingCandidates,
    ResourceOutputBytes,
    ResourceIntegerOverflow,
    IntegritySpool,
    IntegrityCountRun,
    IntegrityManifest,
    IntegrityOrdinalCoverage,
    IntegritySchema,
    IntegrityArtifact,
    DestinationUnsafePath,
    DestinationInputCollision,
    DestinationExisting,
    DestinationLocked,
    DestinationNoReplaceUnsupported,
    CommitWrite,
    CommitFlush,
    CommitSync,
    CommitReopen,
    CommitValidate,
    CommitManifest,
    CommitRenameNoReplace,
    InternalInvariant,
    InternalUnexpected,
}

impl ErrorCode {
    /// Stable underscore-separated serialization.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigurationInvalidK => "configuration_invalid_k",
            Self::ConfigurationProfileConflict => "configuration_profile_conflict",
            Self::ConfigurationInvalidSupport => "configuration_invalid_support",
            Self::ConfigurationInvalidLimit => "configuration_invalid_limit",
            Self::ConfigurationUnsupportedCombination => "configuration_unsupported_combination",
            Self::InputOpen => "input_open",
            Self::InputRead => "input_read",
            Self::InputEmpty => "input_empty",
            Self::InputFormat => "input_format",
            Self::InputHeader => "input_header",
            Self::InputFastaStructure => "input_fasta_structure",
            Self::InputFastqStructure => "input_fastq_structure",
            Self::InputNucleotide => "input_nucleotide",
            Self::InputQuality => "input_quality",
            Self::InputDecompression => "input_decompression",
            Self::InputRawTransportLimit => "input_raw_transport_limit",
            Self::InputDecodedLimit => "input_decoded_limit",
            Self::InputGzipMemberLimit => "input_gzip_member_limit",
            Self::PairLaneCount => "pair_lane_count",
            Self::PairFormat => "pair_format",
            Self::PairIdentity => "pair_identity",
            Self::PairRole => "pair_role",
            Self::PairOrderOrCardinality => "pair_order_or_cardinality",
            Self::PairPhysicalSourceReuse => "pair_physical_source_reuse",
            Self::ResourceMemory => "resource_memory",
            Self::ResourceSpoolBytes => "resource_spool_bytes",
            Self::ResourceTemporaryBytes => "resource_temporary_bytes",
            Self::ResourceOpenFiles => "resource_open_files",
            Self::ResourceRunCount => "resource_run_count",
            Self::ResourceManifestBytes => "resource_manifest_bytes",
            Self::ResourceRetainedKeys => "resource_retained_keys",
            Self::ResourceMappingCandidates => "resource_mapping_candidates",
            Self::ResourceOutputBytes => "resource_output_bytes",
            Self::ResourceIntegerOverflow => "resource_integer_overflow",
            Self::IntegritySpool => "integrity_spool",
            Self::IntegrityCountRun => "integrity_count_run",
            Self::IntegrityManifest => "integrity_manifest",
            Self::IntegrityOrdinalCoverage => "integrity_ordinal_coverage",
            Self::IntegritySchema => "integrity_schema",
            Self::IntegrityArtifact => "integrity_artifact",
            Self::DestinationUnsafePath => "destination_unsafe_path",
            Self::DestinationInputCollision => "destination_input_collision",
            Self::DestinationExisting => "destination_existing",
            Self::DestinationLocked => "destination_locked",
            Self::DestinationNoReplaceUnsupported => "destination_no_replace_unsupported",
            Self::CommitWrite => "commit_write",
            Self::CommitFlush => "commit_flush",
            Self::CommitSync => "commit_sync",
            Self::CommitReopen => "commit_reopen",
            Self::CommitValidate => "commit_validate",
            Self::CommitManifest => "commit_manifest",
            Self::CommitRenameNoReplace => "commit_rename_no_replace",
            Self::InternalInvariant => "internal_invariant",
            Self::InternalUnexpected => "internal_unexpected",
        }
    }

    /// Stable process exit class.
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::ConfigurationInvalidK
            | Self::ConfigurationProfileConflict
            | Self::ConfigurationInvalidSupport
            | Self::ConfigurationInvalidLimit
            | Self::ConfigurationUnsupportedCombination => 2,
            Self::InputOpen
            | Self::InputRead
            | Self::InputEmpty
            | Self::InputFormat
            | Self::InputHeader
            | Self::InputFastaStructure
            | Self::InputFastqStructure
            | Self::InputNucleotide
            | Self::InputQuality
            | Self::InputDecompression
            | Self::InputRawTransportLimit
            | Self::InputDecodedLimit
            | Self::InputGzipMemberLimit => 3,
            Self::PairLaneCount
            | Self::PairFormat
            | Self::PairIdentity
            | Self::PairRole
            | Self::PairOrderOrCardinality
            | Self::PairPhysicalSourceReuse => 4,
            Self::ResourceMemory
            | Self::ResourceSpoolBytes
            | Self::ResourceTemporaryBytes
            | Self::ResourceOpenFiles
            | Self::ResourceRunCount
            | Self::ResourceManifestBytes
            | Self::ResourceRetainedKeys
            | Self::ResourceMappingCandidates
            | Self::ResourceOutputBytes
            | Self::ResourceIntegerOverflow => 5,
            Self::IntegritySpool
            | Self::IntegrityCountRun
            | Self::IntegrityManifest
            | Self::IntegrityOrdinalCoverage
            | Self::IntegritySchema
            | Self::IntegrityArtifact => 6,
            Self::DestinationUnsafePath
            | Self::DestinationInputCollision
            | Self::DestinationExisting
            | Self::DestinationLocked
            | Self::DestinationNoReplaceUnsupported => 7,
            Self::CommitWrite
            | Self::CommitFlush
            | Self::CommitSync
            | Self::CommitReopen
            | Self::CommitValidate
            | Self::CommitManifest
            | Self::CommitRenameNoReplace => 8,
            Self::InternalInvariant | Self::InternalUnexpected => 70,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A typed error with a stable code and bounded human-readable context.
#[derive(Debug)]
pub struct VeritasmError {
    code: ErrorCode,
    context: String,
}

impl VeritasmError {
    pub fn new(code: ErrorCode, context: impl Into<String>) -> Self {
        Self {
            code,
            context: sanitize_context(context.into()),
        }
    }

    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn context(&self) -> &str {
        &self.context
    }
}

/// Keep the CLI's one-record-per-error contract even when an operating-system
/// path or error string contains terminal control characters.  Error context
/// is diagnostic rather than a lossless path serialization; callers retain
/// the original typed path for filesystem operations.
fn sanitize_context(context: String) -> String {
    if !context
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return context;
    }

    let mut sanitized = String::with_capacity(context.len());
    for character in context.chars() {
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            sanitized.extend(character.escape_default());
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

impl fmt::Display for VeritasmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "error[{}]: {}", self.code, self.context)
    }
}

impl std::error::Error for VeritasmError {}

pub(crate) fn overflow(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_always_safe_for_one_line_diagnostics() {
        let error = VeritasmError::new(
            ErrorCode::InputOpen,
            "path\nforged\rline\tfield\u{1b}[31m\u{2028}next\u{2029}last",
        );

        assert_eq!(
            error.context(),
            "path\\nforged\\rline\\tfield\\u{1b}[31m\\u{2028}next\\u{2029}last"
        );
        assert!(error
            .context()
            .chars()
            .all(|character| !character.is_control()
                && !matches!(character, '\u{2028}' | '\u{2029}')));
        assert_eq!(error.to_string().lines().count(), 1);
    }

    #[test]
    fn ordinary_context_is_preserved_exactly() {
        let error = VeritasmError::new(ErrorCode::InputOpen, "lane-000000-S: file not found");
        assert_eq!(error.context(), "lane-000000-S: file not found");
    }

    #[test]
    fn transport_envelope_failures_have_distinct_stable_codes() {
        assert_eq!(
            ErrorCode::InputRawTransportLimit.as_str(),
            "input_raw_transport_limit"
        );
        assert_eq!(ErrorCode::InputRawTransportLimit.exit_code(), 3);
        assert_eq!(
            ErrorCode::InputGzipMemberLimit.as_str(),
            "input_gzip_member_limit"
        );
        assert_eq!(ErrorCode::InputGzipMemberLimit.exit_code(), 3);
        assert_ne!(
            ErrorCode::InputRawTransportLimit,
            ErrorCode::InputDecodedLimit
        );
        assert_ne!(
            ErrorCode::InputGzipMemberLimit,
            ErrorCode::InputDecompression
        );
    }
}
