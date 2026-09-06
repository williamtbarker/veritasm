use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use veritasm::{
    assemble, AssembleConfig, ErrorCode, InputSpec, Limits, Profile, ScientificConfig, SupportUnit,
    VeritasmError,
};

#[derive(Debug, Parser)]
#[command(
    name = "veritasm",
    version,
    about = "Evidence-first deterministic de novo unitig assembly from short reads",
    long_about = "VeritAsm builds exact compacted de Bruijn-graph unitigs from single-end or strictly synchronized paired-end FASTA/FASTQ. It reports algorithmic evidence and uncertainty; it does not perform organism detection or biological presence/absence calls.",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Assemble one exact single-k evidence bundle.
    Assemble(Box<AssembleArgs>),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliProfile {
    RetainAll,
    Thresholded,
    Custom,
}

impl From<CliProfile> for Profile {
    fn from(value: CliProfile) -> Self {
        match value {
            CliProfile::RetainAll => Self::RetainAll,
            CliProfile::Thresholded => Self::Thresholded,
            CliProfile::Custom => Self::Custom,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliSupportUnit {
    SuppliedFragmentInstance,
    AcceptedWindowOccurrence,
}

impl From<CliSupportUnit> for SupportUnit {
    fn from(value: CliSupportUnit) -> Self {
        match value {
            CliSupportUnit::SuppliedFragmentInstance => Self::SuppliedFragmentInstance,
            CliSupportUnit::AcceptedWindowOccurrence => Self::AcceptedWindowOccurrence,
        }
    }
}

#[derive(Debug, Args)]
struct AssembleArgs {
    /// Ordered single-end lanes; compression is detected from content.
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

    /// New result directory. Existing destinations are never replaced.
    #[arg(short = 'o', long, value_name = "DIR")]
    output_dir: PathBuf,

    /// Exact graph word size.
    #[arg(short = 'k', long, default_value_t = 31, value_parser = parse_u8_decimal)]
    k: u8,

    /// Stable retention profile.
    #[arg(long, value_enum, default_value_t = CliProfile::Thresholded)]
    profile: CliProfile,

    /// Unit counted by exact support.
    #[arg(long, value_enum, default_value_t = CliSupportUnit::SuppliedFragmentInstance)]
    support_unit: CliSupportUnit,

    /// Inclusive exact support threshold; profile-dependent when omitted.
    #[arg(long, value_parser = parse_u64_decimal)]
    min_support: Option<u64>,

    /// Reject FASTQ windows containing a base below this Phred+33 score.
    #[arg(long, default_value_t = 20, value_parser = parse_u8_decimal)]
    min_base_quality: u8,

    /// Disable exact construction-read remapping evidence.
    #[arg(long)]
    no_remap: bool,

    #[arg(long, default_value_t = 1 << 20, value_parser = parse_u64_decimal)]
    max_header_bytes: u64,
    #[arg(long, default_value_t = 10 << 20, value_parser = parse_u64_decimal)]
    max_read_bases: u64,
    #[arg(long, default_value_t = 24 << 20, value_parser = parse_u64_decimal)]
    max_record_bytes: u64,
    /// Maximum aggregate physical bytes consumed before decompression.
    #[arg(long, default_value_t = 1_u64 << 40, value_parser = parse_u64_decimal)]
    max_raw_transport_bytes: u64,
    #[arg(long, default_value_t = 1_u64 << 40, value_parser = parse_u64_decimal)]
    max_decoded_input_bytes: u64,
    /// Maximum aggregate concatenated gzip members across all sources.
    #[arg(long, default_value_t = 1_000_000, value_parser = parse_u64_decimal)]
    max_gzip_members: u64,
    #[arg(long, default_value_t = 4_096, value_parser = parse_u64_decimal)]
    batch_fragments: u64,
    #[arg(long, default_value_t = 512 << 20, value_parser = parse_u64_decimal)]
    memory_budget_bytes: u64,
    #[arg(long, default_value_t = 1_u64 << 40, value_parser = parse_u64_decimal)]
    max_spool_bytes: u64,
    #[arg(long, default_value_t = 2_u64 << 40, value_parser = parse_u64_decimal)]
    max_temp_bytes: u64,
    #[arg(long, default_value_t = 6, value_parser = parse_u8_decimal)]
    partition_prefix_bits: u8,
    #[arg(long, default_value_t = 1_048_576, value_parser = parse_u64_decimal)]
    sort_buffer_keys: u64,
    #[arg(long, default_value_t = 1_000_000, value_parser = parse_u64_decimal)]
    max_runs: u64,
    #[arg(long, default_value_t = 1 << 30, value_parser = parse_u64_decimal)]
    max_manifest_bytes: u64,
    #[arg(long, default_value_t = 10_000_000, value_parser = parse_u64_decimal)]
    max_retained_kmers: u64,
    #[arg(long, default_value_t = 10_000, value_parser = parse_u64_decimal)]
    max_mapping_candidates: u64,
    #[arg(long, default_value_t = 100_u64 << 30, value_parser = parse_u64_decimal)]
    max_staged_output_bytes: u64,
    #[arg(long, default_value_t = 1_000, value_parser = parse_u64_decimal)]
    html_max_unitig_rows: u64,

    /// Worker threads; omitted uses available parallelism.
    #[arg(short = 't', long, value_parser = parse_usize_decimal)]
    threads: Option<usize>,
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

fn run(cli: Cli) -> veritasm::Result<()> {
    match cli.command {
        Command::Assemble(args) => {
            let input = if args.single.is_empty() {
                InputSpec::Paired {
                    read1: args.read1,
                    read2: args.read2,
                }
            } else {
                InputSpec::Single(args.single)
            };
            let scientific = ScientificConfig::resolve(
                args.k,
                args.profile.into(),
                args.support_unit.into(),
                args.min_support,
                args.min_base_quality,
                !args.no_remap,
            )?;
            let limits = Limits {
                max_header_bytes: args.max_header_bytes,
                max_read_bases: args.max_read_bases,
                max_record_bytes: args.max_record_bytes,
                max_raw_transport_bytes: args.max_raw_transport_bytes,
                max_decoded_input_bytes: args.max_decoded_input_bytes,
                max_gzip_members: args.max_gzip_members,
                batch_fragments: args.batch_fragments,
                memory_budget_bytes: args.memory_budget_bytes,
                max_spool_bytes: args.max_spool_bytes,
                max_temp_bytes: args.max_temp_bytes,
                partition_prefix_bits: args.partition_prefix_bits,
                sort_buffer_keys: args.sort_buffer_keys,
                merge_fan_in: 16,
                max_count_open_files: 17,
                max_runs: args.max_runs,
                max_manifest_bytes: args.max_manifest_bytes,
                max_retained_kmers: args.max_retained_kmers,
                max_mapping_candidates: args.max_mapping_candidates,
                max_staged_output_bytes: args.max_staged_output_bytes,
                html_max_unitig_rows: args.html_max_unitig_rows,
            };
            let maximum_worker_threads = limits.maximum_worker_threads();
            let threads = args.threads.unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1)
                    .min(maximum_worker_threads)
            });
            let config = AssembleConfig {
                input,
                output_dir: args.output_dir,
                scientific,
                limits,
                threads,
            };
            assemble(&config)?;
            Ok(())
        }
    }
}

fn informational_clap_output(error: &clap::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    )
}

fn clap_invalid_argument(error: &clap::Error) -> Option<&str> {
    match error.get(ContextKind::InvalidArg) {
        Some(ContextValue::String(argument)) => Some(argument),
        _ => None,
    }
}

fn clap_parse_error(error: &clap::Error) -> VeritasmError {
    let invalid_argument = clap_invalid_argument(error);
    let (code, context) = match error.kind() {
        ErrorKind::ValueValidation if invalid_argument.is_some_and(|arg| arg.contains("--k")) => {
            (ErrorCode::ConfigurationInvalidK, "invalid value for --k")
        }
        ErrorKind::ValueValidation
            if invalid_argument.is_some_and(|arg| arg.contains("--min-support")) =>
        {
            (
                ErrorCode::ConfigurationInvalidSupport,
                "invalid value for --min-support",
            )
        }
        ErrorKind::ValueValidation => (
            ErrorCode::ConfigurationInvalidLimit,
            "invalid value for a numeric command-line option",
        ),
        ErrorKind::InvalidValue => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "invalid command-line option value",
        ),
        ErrorKind::UnknownArgument => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "unknown command-line argument",
        ),
        ErrorKind::InvalidSubcommand => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "invalid command-line subcommand",
        ),
        ErrorKind::ArgumentConflict => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "conflicting or repeated command-line arguments",
        ),
        ErrorKind::MissingRequiredArgument | ErrorKind::MissingSubcommand => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "missing required command-line argument",
        ),
        ErrorKind::TooManyValues | ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "invalid number of command-line values",
        ),
        ErrorKind::InvalidUtf8 => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "command-line argument is not valid UTF-8",
        ),
        ErrorKind::Io | ErrorKind::Format => (
            ErrorCode::InternalUnexpected,
            "command-line parser could not render its result",
        ),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "missing required command-line argument or subcommand",
        ),
        _ => (
            ErrorCode::ConfigurationUnsupportedCombination,
            "invalid command line",
        ),
    };
    VeritasmError::new(code, context)
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
    fn decimal_parser_rejects_signs_and_suffixes() {
        for invalid in ["", "+1", "-1", "1k", " 1", "1 "] {
            assert!(parse_u64_decimal(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(parse_u64_decimal("001").unwrap(), 1);
    }

    #[test]
    fn unknown_arguments_become_one_line_typed_errors() {
        let clap_error = Cli::try_parse_from(["veritasm", "--definitely-unknown"]).unwrap_err();
        let error = clap_parse_error(&clap_error);
        assert_eq!(error.code(), ErrorCode::ConfigurationUnsupportedCombination);
        assert_eq!(error.context(), "unknown command-line argument");
        assert!(!error.to_string().contains('\n'));
    }

    #[test]
    fn numeric_parse_errors_use_the_narrow_configuration_codes() {
        for (argument, expected) in [
            ("--k", ErrorCode::ConfigurationInvalidK),
            ("--min-support", ErrorCode::ConfigurationInvalidSupport),
            ("--threads", ErrorCode::ConfigurationInvalidLimit),
        ] {
            let clap_error = Cli::try_parse_from([
                "veritasm",
                "assemble",
                "-U",
                "reads.fastq",
                "-o",
                "result",
                argument,
                "not-a-number",
            ])
            .unwrap_err();
            assert_eq!(clap_parse_error(&clap_error).code(), expected);
        }
    }

    #[test]
    fn help_and_version_remain_informational() {
        for arguments in [
            &["veritasm", "--help"][..],
            &["veritasm", "--version"][..],
            &["veritasm", "assemble", "--help"][..],
        ] {
            let error = Cli::try_parse_from(arguments).unwrap_err();
            assert!(informational_clap_output(&error));
            assert_eq!(error.exit_code(), 0);
        }
    }
}
