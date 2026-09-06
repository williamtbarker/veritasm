use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use veritasm::experimental::multik_bundle::PortfolioProfile;
use veritasm::experimental::multik_pipeline::{
    run_authenticated_multik_portfolio, MultiKPairEvidenceConfig, MultiKPortfolioConfig,
    MultiKResourceLimits,
};
use veritasm::experimental::retention::RetentionRule;
use veritasm::{ErrorCode, InputSpec, SupportUnit, VeritasmError};

#[derive(Debug, Parser)]
#[command(
    name = "veritasm-multik",
    version,
    about = "EXPERIMENTAL, UNQUALIFIED, RESEARCH-ONLY independent multi-k assembly portfolio",
    long_about = "EXPERIMENTAL, UNQUALIFIED, RESEARCH USE ONLY. Builds independently authenticated read-witnessed children from one immutable FASTA/FASTQ spool. It never splices or selects sequence across k and makes no sensitivity, accuracy, sample-content inference, clinical, production-readiness, or performance claim."
)]
struct Cli {
    /// Ordered single-end lanes; gzip is detected from content.
    #[arg(
        short = 'U',
        long,
        value_name = "FASTX",
        num_args = 1..,
        required_unless_present = "read1",
        conflicts_with_all = ["read1", "read2"]
    )]
    single: Vec<PathBuf>,

    /// Ordered read-1 lanes.
    #[arg(
        short = '1',
        long,
        value_name = "FASTX",
        num_args = 1..,
        requires = "read2",
        conflicts_with = "single"
    )]
    read1: Vec<PathBuf>,

    /// Ordered read-2 lanes corresponding one-for-one to --read1.
    #[arg(
        short = '2',
        long,
        value_name = "FASTX",
        num_args = 1..,
        requires = "read1",
        conflicts_with = "single"
    )]
    read2: Vec<PathBuf>,

    /// New result directory. Existing paths are never replaced.
    #[arg(short = 'o', long, value_name = "DIR")]
    output_dir: PathBuf,

    /// Strictly increasing unique child k values (repeat or use commas).
    #[arg(
        short = 'k',
        long = "k",
        value_name = "K",
        value_delimiter = ',',
        default_value = "21,31,51",
        value_parser = parse_u8_decimal
    )]
    ks: Vec<u8>,

    /// Exact-key retention rule applied independently to every child.
    #[arg(long, value_enum, default_value_t = CliRetention::RetainAll)]
    retention: CliRetention,

    /// Inclusive exact support threshold; required for inclusive-support.
    #[arg(long, value_parser = parse_u64_decimal)]
    min_support: Option<u64>,

    /// Unit counted by exact support.
    #[arg(long, value_enum, default_value_t = CliSupportUnit::SuppliedFragmentInstance)]
    support_unit: CliSupportUnit,

    /// Primary FASTA presentation; all child segments remain in segments.fasta.
    #[arg(long, value_enum, default_value_t = CliOutputProfile::DiversityPreserving)]
    output_profile: CliOutputProfile,

    /// Reject FASTQ windows containing a base below this Phred+33 score.
    #[arg(long, default_value_t = 20, value_parser = parse_u8_decimal)]
    min_base_quality: u8,

    /// Execution threads. This serial alpha accepts exactly 1.
    #[arg(short = 't', long, value_parser = parse_usize_decimal)]
    threads: Option<usize>,

    /// Aggregate modelled owned-payload envelope; not a process-RSS promise.
    #[arg(long, default_value_t = 512 << 20, value_parser = parse_u64_decimal)]
    memory_budget_bytes: u64,

    #[arg(long, default_value_t = 1_u64 << 40, value_parser = parse_u64_decimal)]
    max_spool_bytes: u64,

    #[arg(long, default_value_t = 2_u64 << 40, value_parser = parse_u64_decimal)]
    max_temp_bytes: u64,

    /// Maximum exact keys admitted per child.
    #[arg(long, default_value_t = 1_000_000, value_parser = parse_u64_decimal)]
    max_child_keys: u64,

    /// Maximum paired fragments admitted to the optional authenticated pair analysis.
    /// Larger paired inputs still assemble and report pair evidence as unavailable.
    #[arg(long, default_value_t = 8_192, value_parser = parse_u64_decimal)]
    max_pair_fragments: u64,

    /// Maximum canonical JSON bytes before private snapshot compression.
    #[arg(long, default_value_t = 64 << 20, value_parser = parse_u64_decimal)]
    max_child_snapshot_document_bytes: u64,

    /// Maximum stored bytes in one deterministic gzip child snapshot.
    #[arg(long, default_value_t = 8 << 20, value_parser = parse_u64_decimal)]
    max_child_snapshot_bytes: u64,

    /// Maximum aggregate encoded child snapshots loaded for final reporting.
    #[arg(long, default_value_t = 32 << 20, value_parser = parse_u64_decimal)]
    max_loaded_snapshot_bytes: u64,

    /// Maximum bytes written in private final-bundle staging.
    #[arg(long, default_value_t = 100_u64 << 30, value_parser = parse_u64_decimal)]
    max_staged_output_bytes: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliRetention {
    RetainAll,
    InclusiveSupport,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliSupportUnit {
    SuppliedFragmentInstance,
    AcceptedWindowOccurrence,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliOutputProfile {
    DiversityPreserving,
    ExactAgreementConsensus,
}

fn run(cli: Cli) -> veritasm::Result<()> {
    let input = if cli.single.is_empty() {
        InputSpec::Paired {
            read1: cli.read1,
            read2: cli.read2,
        }
    } else {
        InputSpec::Single(cli.single)
    };
    let retention_rule = match (cli.retention, cli.min_support) {
        (CliRetention::RetainAll, None | Some(1)) => RetentionRule::RetainAll,
        (CliRetention::RetainAll, Some(_)) => {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationProfileConflict,
                "retain-all permits only an omitted --min-support or exactly 1",
            ));
        }
        (CliRetention::InclusiveSupport, Some(minimum_support)) if minimum_support > 0 => {
            RetentionRule::InclusiveSupport { minimum_support }
        }
        (CliRetention::InclusiveSupport, _) => {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidSupport,
                "inclusive-support requires --min-support of at least 1",
            ));
        }
    };
    let support_unit = match cli.support_unit {
        CliSupportUnit::SuppliedFragmentInstance => SupportUnit::SuppliedFragmentInstance,
        CliSupportUnit::AcceptedWindowOccurrence => SupportUnit::AcceptedWindowOccurrence,
    };
    let profile = match cli.output_profile {
        CliOutputProfile::DiversityPreserving => PortfolioProfile::DiversityPreserving,
        CliOutputProfile::ExactAgreementConsensus => PortfolioProfile::ExactAgreementConsensus,
    };
    let mut limits = MultiKResourceLimits {
        max_aggregate_accounted_memory_bytes: cli.memory_budget_bytes,
        ..MultiKResourceLimits::default()
    };
    partition_component_memory(&mut limits, cli.memory_budget_bytes);
    configure_pair_fragment_limit(&mut limits, cli.max_pair_fragments)?;
    limits.stable_input_and_spool.memory_budget_bytes = cli.memory_budget_bytes;
    limits.stable_input_and_spool.max_spool_bytes = cli.max_spool_bytes;
    limits.stable_input_and_spool.max_temp_bytes = cli.max_temp_bytes;
    limits.max_aggregate_temp_bytes = cli.max_temp_bytes;
    limits.external_counts.max_temp_bytes = cli.max_temp_bytes;
    limits.external_counts.max_distinct_kmers = cli.max_child_keys;
    limits.retention.max_raw_keys = cli.max_child_keys;
    limits.retention.max_retained_keys = cli.max_child_keys;
    limits.compacted_graph.max_canonical_edges = cli.max_child_keys;
    limits.reconstruction.max_input_edges = cli.max_child_keys;
    limits.transition_ledger.max_temp_bytes = cli.max_temp_bytes;
    limits.bundle.max_snapshot_document_bytes = cli.max_child_snapshot_document_bytes;
    limits.bundle.max_snapshot_bytes = cli.max_child_snapshot_bytes;
    limits.bundle.max_loaded_snapshot_bytes = cli.max_loaded_snapshot_bytes;
    limits.bundle.max_staged_output_bytes = cli.max_staged_output_bytes;
    let worker_threads = resolve_serial_threads(cli.threads)?;
    eprintln!("veritasm-multik: experimental; unqualified; research use only; no sample-content or performance claim");
    run_authenticated_multik_portfolio(&MultiKPortfolioConfig {
        input,
        output_dir: cli.output_dir,
        ks: cli.ks,
        support_unit,
        retention_rule,
        min_base_quality: cli.min_base_quality,
        profile,
        worker_threads,
        minimizer_length_ceiling: 15,
        virtual_bucket_count: 256,
        pair_evidence: MultiKPairEvidenceConfig::default(),
        limits,
    })?;
    Ok(())
}

fn configure_pair_fragment_limit(
    limits: &mut MultiKResourceLimits,
    maximum_fragments: u64,
) -> veritasm::Result<()> {
    if maximum_fragments < 2 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "--max-pair-fragments must be at least 2",
        ));
    }
    let maximum_reads = maximum_fragments.checked_mul(2).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            "--max-pair-fragments overflows the paired-read bound",
        )
    })?;
    limits.pair_mapper.maximum_fragments = maximum_fragments;
    limits.pair_mapper.maximum_reads = maximum_reads;
    limits.pair_mapper.maximum_batch_fragments = limits
        .pair_mapper
        .maximum_batch_fragments
        .min(maximum_fragments);
    limits.pair_graph_adapter.pair_path.maximum_pairs = maximum_fragments;
    Ok(())
}

fn resolve_serial_threads(requested: Option<usize>) -> veritasm::Result<usize> {
    match requested {
        None | Some(1) => Ok(1),
        Some(value) => Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!(
                "experimental multi-k execution is serial; --threads must equal 1, received {value}"
            ),
        )),
    }
}

fn partition_component_memory(limits: &mut MultiKResourceLimits, aggregate: u64) {
    // Fractions deliberately leave four sixteenths unassigned in the largest
    // live modelled phase. Config validation uses checked overlap formulas.
    let sixteenth = aggregate / 16;
    limits.external_counts.max_memory_bytes = sixteenth.saturating_mul(8);
    limits.retention.max_accounted_bytes = sixteenth.saturating_mul(2);
    limits.compacted_graph.max_accounted_bytes = sixteenth.saturating_mul(3);
    limits.transition_ledger.max_memory_bytes = sixteenth.saturating_mul(5);
    limits.reconstruction.max_accounted_bytes = sixteenth.saturating_mul(3);
    limits.reconstruction.max_verification_scratch_bytes = sixteenth;
    limits.max_child_report_accounted_bytes = sixteenth.saturating_mul(2);
    limits.bundle.max_loaded_report_accounted_bytes = sixteenth.saturating_mul(4);
    limits.bundle.max_presentation_accounted_bytes = sixteenth.saturating_mul(2);
    limits.bundle.max_validation_scratch_bytes = sixteenth.max(1);
    limits.pair_graph_adapter.maximum_accounted_bytes = sixteenth.saturating_mul(2);
    limits.pair_graph_adapter.pair_path.graph_memory_bytes = sixteenth.max(4_096);
    limits
        .pair_graph_adapter
        .pair_path
        .search_memory_bytes_per_worker = (sixteenth / 2).max(1);
    limits.pair_graph_adapter.pair_path.result_memory_bytes = sixteenth.saturating_mul(6);
    limits.pair_graph_adapter.pair_path.analysis_memory_bytes = sixteenth.saturating_mul(8);
    limits.pair_mapper.mapper_index_memory_bytes = sixteenth.saturating_mul(2);
    limits.pair_mapper.mapper_query_memory_bytes_per_worker = (sixteenth / 2).max(1);
    limits.pair_mapper.result_memory_bytes = sixteenth.saturating_mul(2);
    limits.pair_mapper.total_accounted_memory_bytes = sixteenth.saturating_mul(8);
    limits.external_counts.sort_buffer_bytes = limits
        .external_counts
        .sort_buffer_bytes
        .min(limits.external_counts.max_memory_bytes / 8)
        .max(128);
    limits.transition_ledger.sort_buffer_bytes = limits
        .transition_ledger
        .sort_buffer_bytes
        .min(limits.transition_ledger.max_memory_bytes / 4)
        .max(128);
    limits.transition_ledger.max_fragment_decode_bytes = limits
        .transition_ledger
        .max_fragment_decode_bytes
        .min(limits.transition_ledger.max_memory_bytes / 4)
        .max(1);
    let window_allowance = limits.external_counts.max_memory_bytes / (4 * 32);
    limits.transition_ledger.max_fragment_windows = limits
        .transition_ledger
        .max_fragment_windows
        .min(window_allowance)
        .max(1);
}

fn parse_u64_decimal(value: &str) -> std::result::Result<u64, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("expected an unsigned base-10 integer without signs or suffixes".to_owned());
    }
    value
        .parse()
        .map_err(|_| "unsigned integer is out of range".to_owned())
}

fn parse_u8_decimal(value: &str) -> std::result::Result<u8, String> {
    parse_u64_decimal(value).and_then(|parsed| {
        u8::try_from(parsed).map_err(|_| "unsigned integer is out of range for u8".to_owned())
    })
}

fn parse_usize_decimal(value: &str) -> std::result::Result<usize, String> {
    parse_u64_decimal(value).and_then(|parsed| {
        usize::try_from(parsed).map_err(|_| "unsigned integer is out of range".to_owned())
    })
}

fn informational_clap_output(error: &clap::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    )
}

fn clap_parse_error(error: &clap::Error) -> VeritasmError {
    let invalid = match error.get(ContextKind::InvalidArg) {
        Some(ContextValue::String(argument)) => Some(argument.as_str()),
        _ => None,
    };
    let code = match error.kind() {
        ErrorKind::ValueValidation if invalid.is_some_and(|arg| arg.contains("--k")) => {
            ErrorCode::ConfigurationInvalidK
        }
        ErrorKind::ValueValidation if invalid.is_some_and(|arg| arg.contains("--min-support")) => {
            ErrorCode::ConfigurationInvalidSupport
        }
        ErrorKind::ValueValidation => ErrorCode::ConfigurationInvalidLimit,
        _ => ErrorCode::ConfigurationUnsupportedCombination,
    };
    VeritasmError::new(code, "invalid veritasm-multik command line")
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) if informational_clap_output(&error) => {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => return exit_with_error(&clap_parse_error(&error)),
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => exit_with_error(&error),
    }
}

fn exit_with_error(error: &VeritasmError) -> ExitCode {
    eprintln!("{error}");
    ExitCode::from(error.code().exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_parser_is_strict() {
        for invalid in ["", "+1", "-1", "1k", " 1", "1 "] {
            assert!(parse_u64_decimal(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(parse_u64_decimal("001").unwrap(), 1);
    }

    #[test]
    fn malformed_k_is_typed() {
        let error = Cli::try_parse_from([
            "veritasm-multik",
            "-U",
            "reads.fastq",
            "-o",
            "result",
            "-k",
            "not-a-number",
        ])
        .unwrap_err();
        assert_eq!(
            clap_parse_error(&error).code(),
            ErrorCode::ConfigurationInvalidK
        );
    }

    #[test]
    fn thread_option_is_never_silently_ignored() {
        assert_eq!(resolve_serial_threads(None).unwrap(), 1);
        assert_eq!(resolve_serial_threads(Some(1)).unwrap(), 1);
        assert_eq!(
            resolve_serial_threads(Some(2)).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        assert_eq!(
            resolve_serial_threads(Some(0)).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
    }

    #[test]
    fn pair_fragment_limit_propagates_exactly_and_rejects_invalid_bounds() {
        for maximum in [8_192, 8_193] {
            let mut limits = MultiKResourceLimits::default();
            configure_pair_fragment_limit(&mut limits, maximum).unwrap();
            assert_eq!(limits.pair_mapper.maximum_fragments, maximum);
            assert_eq!(limits.pair_mapper.maximum_reads, maximum * 2);
            assert_eq!(limits.pair_graph_adapter.pair_path.maximum_pairs, maximum);
        }
        let mut limits = MultiKResourceLimits::default();
        assert_eq!(
            configure_pair_fragment_limit(&mut limits, 1)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        assert_eq!(
            configure_pair_fragment_limit(&mut limits, u64::MAX)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceIntegerOverflow
        );
    }
}
