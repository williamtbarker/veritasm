use clap::{error::ErrorKind, Parser};
use std::path::PathBuf;
use std::process::ExitCode;
use veritasm::validation::{
    generate_dataset, GeneratorCase, GeneratorConfig, Seed256, ValidationDiagnostic,
    ValidationError,
};

#[derive(Debug, Parser)]
#[command(
    name = "veritasm-simulate",
    about = "Generate deterministic truth-known short-read validation data"
)]
struct Args {
    /// New output directory; its parent must already exist.
    #[arg(long)]
    out: PathBuf,
    /// Built-in case, including error-pe or the explicit qc-censoring-control.
    #[arg(long, value_parser = parse_case)]
    case: GeneratorCase,
    /// Replicate used by the documented SHA-256 seed derivation.
    #[arg(long, default_value_t = 0)]
    replicate: u32,
    /// Optional explicit 256-bit lowercase hexadecimal seed.
    #[arg(long, value_parser = parse_seed)]
    seed_hex: Option<Seed256>,
    /// Exact number of supplied fragment instances.
    #[arg(long, default_value_t = 200)]
    fragments: u64,
    #[arg(long, default_value_t = 2_000)]
    truth_length: usize,
    #[arg(long, default_value_t = 100)]
    read_length: usize,
    #[arg(long, default_value_t = 250)]
    insert_length: usize,
    /// Per-base substitution probability in parts per million.
    #[arg(long)]
    substitution_rate_ppm: Option<u32>,
    /// Independent per-base probability of Q10 instead of Q40, in parts per million.
    #[arg(long)]
    low_quality_rate_ppm: Option<u32>,
    /// Optional independent 256-bit seed for the quality stream.
    #[arg(long, value_parser = parse_seed)]
    quality_seed_hex: Option<Seed256>,
    /// Emit deterministic content-detectable gzip FASTQ.
    #[arg(long)]
    gzip: bool,
}

fn parse_case(value: &str) -> Result<GeneratorCase, String> {
    value
        .parse::<GeneratorCase>()
        .map_err(|error| error.to_string())
}

fn parse_seed(value: &str) -> Result<Seed256, String> {
    Seed256::from_hex(value).map_err(|error| error.to_string())
}

fn main() -> ExitCode {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            return exit_with_error(&ValidationError::Configuration(format!(
                "command-line parsing failed: {error}"
            )));
        }
    };
    let mut config = GeneratorConfig::new(args.out, args.case);
    config.replicate = args.replicate;
    config.explicit_seed = args.seed_hex;
    config.fragments = args.fragments;
    config.truth_length = args.truth_length;
    config.read_length = args.read_length;
    config.insert_length = args.insert_length;
    config.gzip = args.gzip;
    if let Some(rate) = args.substitution_rate_ppm {
        config.substitution_rate_ppm = rate;
    }
    if let Some(rate) = args.low_quality_rate_ppm {
        config.low_quality_rate_ppm = rate;
    }
    config.explicit_quality_seed = args.quality_seed_hex;
    match generate_dataset(&config) {
        Ok(manifest) => {
            println!("{}", manifest.dataset_id);
            ExitCode::SUCCESS
        }
        Err(error) => exit_with_error(&error),
    }
}

fn exit_with_error(error: &ValidationError) -> ExitCode {
    let diagnostic = ValidationDiagnostic::from_error(error);
    eprintln!("{diagnostic}");
    ExitCode::from(diagnostic.class().exit_code())
}
