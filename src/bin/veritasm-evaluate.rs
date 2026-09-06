use clap::{error::ErrorKind, Parser, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use veritasm::validation::{
    evaluate_dataset, EvaluationConfig, JunctionEvidenceMode, ValidationDiagnostic, ValidationError,
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum JunctionEvidenceArg {
    All,
    NonCorrect,
    Summary,
}

impl From<JunctionEvidenceArg> for JunctionEvidenceMode {
    fn from(value: JunctionEvidenceArg) -> Self {
        match value {
            JunctionEvidenceArg::All => Self::All,
            JunctionEvidenceArg::NonCorrect => Self::NonCorrect,
            JunctionEvidenceArg::Summary => Self::Summary,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "veritasm-evaluate",
    about = "Evaluate FASTA assembly sequence against generated truth"
)]
struct Args {
    /// Generator dataset.json; truth is read only by this evaluator.
    #[arg(long)]
    dataset: PathBuf,
    /// Assembly FASTA. A zero-byte file is a valid empty assembly.
    #[arg(long)]
    assembly: PathBuf,
    /// New checksum-bearing evaluation result directory.
    #[arg(long)]
    out: PathBuf,
    /// Exact flank length on each side of an evaluated output adjacency.
    #[arg(long, default_value_t = 15)]
    junction_flank: usize,
    /// Maximum accepted primary-alignment edit rate in parts per million.
    #[arg(long, default_value_t = 150_000)]
    max_edit_rate_ppm: u32,
    /// Per-contig, per-truth-orientation dynamic-programming cell cap.
    #[arg(long, default_value_t = 50_000_000)]
    max_dp_cells: u64,
    /// Aggregate oriented truth bases inspected by the exact-substring fast path.
    #[arg(long, default_value_t = 1_000_000_000)]
    max_exact_alignment_scan_bases: u64,
    /// Upper bound on exact packed-index key comparisons.
    #[arg(long, default_value_t = 500_000_000)]
    max_junction_comparisons: u64,
    /// Per-adjacency junction evidence rows to materialize.
    #[arg(long, value_enum, default_value_t = JunctionEvidenceArg::NonCorrect)]
    junction_evidence: JunctionEvidenceArg,
    /// Conservative memory ceiling for exact truth-window occurrence indexes.
    #[arg(long, default_value_t = 536_870_912)]
    max_junction_index_bytes: u64,
    /// Maximum bytes permitted in the materialized junction TSV.
    #[arg(long, default_value_t = 134_217_728)]
    max_junction_evidence_bytes: u64,
    /// SHA-256 of the exact canonical dataset manifest.sha256 bytes, supplied externally.
    #[arg(long)]
    expected_dataset_content_root_sha256: Option<String>,
    /// Maximum admitted bytes for truth-read-ledger semantic replay state.
    #[arg(long, default_value_t = 402_653_184)]
    max_truth_evidence_replay_bytes: u64,
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
    let mut config = EvaluationConfig::new(args.dataset, args.assembly, args.out);
    config.junction_flank_length = args.junction_flank;
    config.max_edit_rate_ppm = args.max_edit_rate_ppm;
    config.max_dp_cells = args.max_dp_cells;
    config.max_exact_alignment_scan_bases = args.max_exact_alignment_scan_bases;
    config.max_junction_comparisons = args.max_junction_comparisons;
    config.junction_evidence_mode = args.junction_evidence.into();
    config.max_junction_index_bytes = args.max_junction_index_bytes;
    config.max_junction_evidence_bytes = args.max_junction_evidence_bytes;
    config.expected_dataset_content_root_sha256 = args.expected_dataset_content_root_sha256;
    config.max_truth_evidence_replay_bytes = args.max_truth_evidence_replay_bytes;
    match evaluate_dataset(&config) {
        Ok(result) => {
            println!("{}", result.evaluation_id);
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
