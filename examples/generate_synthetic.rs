//! Deterministic truth-known short-read fixtures for development and benchmarking.
//!
//! The generator is deliberately simple and is not a sequencing simulator. Its
//! truth is suitable for parser, graph, determinism, and regression tests only.

use clap::{Parser, ValueEnum};
use flate2::{write::GzEncoder, Compression, GzBuilder};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum Scenario {
    Basic,
    Repeat,
    Circular,
    Uneven,
    Mixture,
    HostContaminated,
}

#[derive(Debug, Parser)]
#[command(about = "Generate deterministic non-biological truth-known short reads")]
struct Args {
    /// New output directory.
    #[arg(long)]
    out: PathBuf,
    /// Recorded xorshift64 seed.
    #[arg(long, default_value_t = 7_643_211)]
    seed: u64,
    /// Number of supplied fragment instances.
    #[arg(long, default_value_t = 2_000)]
    fragments: u64,
    #[arg(long, value_enum, default_value_t = Scenario::Basic)]
    scenario: Scenario,
    /// Emit one single-end stream instead of synchronized mates.
    #[arg(long)]
    single_end: bool,
    #[arg(long, default_value_t = 100)]
    read_length: usize,
    #[arg(long, default_value_t = 250)]
    insert_length: usize,
    /// Write plain FASTQ rather than deterministic gzip.
    #[arg(long)]
    plain: bool,
}

#[derive(Debug, Serialize)]
struct Manifest {
    generator: &'static str,
    generator_version: &'static str,
    seed_decimal: String,
    scenario: Scenario,
    fragments_decimal: String,
    mode: &'static str,
    read_length: usize,
    insert_length: usize,
    model_limitations: &'static str,
    files: Vec<FileDigest>,
}

#[derive(Debug, Serialize)]
struct FileDigest {
    path: String,
    sha256: String,
}

enum Output {
    Plain(BufWriter<File>),
    Gzip(GzEncoder<BufWriter<File>>),
}

impl Output {
    fn create(path: &Path, plain: bool) -> std::io::Result<Self> {
        let writer = BufWriter::new(File::create(path)?);
        if plain {
            Ok(Self::Plain(writer))
        } else {
            Ok(Self::Gzip(
                GzBuilder::new().mtime(0).write(writer, Compression::fast()),
            ))
        }
    }

    fn finish(self) -> std::io::Result<()> {
        match self {
            Self::Plain(mut writer) => writer.flush(),
            Self::Gzip(writer) => writer.finish()?.flush(),
        }
    }
}

impl Write for Output {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.read_length == 0 || args.insert_length < args.read_length {
        return Err("require read-length>0 and insert-length>=read-length".into());
    }
    if args.out.exists() {
        return Err(format!("output already exists: {}", args.out.display()).into());
    }
    fs::create_dir(&args.out)?;

    let mut rng = XorShift64::new(args.seed);
    let mut target_a = random_dna(6_000, &mut rng);
    if matches!(args.scenario, Scenario::Repeat) {
        let repeated = target_a[500..900].to_vec();
        target_a[3_500..3_900].copy_from_slice(&repeated);
    }
    let mut target_b = target_a.clone();
    for position in (47..target_b.len()).step_by(97) {
        target_b[position] = mutate_base(target_b[position]);
    }
    let background = random_dna(30_000, &mut rng);

    let truth_path = args.out.join("truth.fasta");
    write_truth(
        &truth_path,
        args.scenario,
        &target_a,
        &target_b,
        &background,
    )?;

    let suffix = if args.plain { "fastq" } else { "fastq.gz" };
    let read1_path = args.out.join(format!("reads_R1.{suffix}"));
    let read2_path = args.out.join(format!("reads_R2.{suffix}"));
    let mut read1 = Output::create(&read1_path, args.plain)?;
    let mut read2 = (!args.single_end)
        .then(|| Output::create(&read2_path, args.plain))
        .transpose()?;
    let qualities = vec![b'I'; args.read_length];

    for ordinal in 0..args.fragments {
        let source = choose_source(args.scenario, &target_a, &target_b, &background, &mut rng);
        let circular = matches!(args.scenario, Scenario::Circular);
        let span = if args.single_end {
            args.read_length
        } else {
            args.insert_length
        };
        let start = choose_start(args.scenario, source.len(), span, circular, &mut rng)?;
        let first = slice_sequence(source, start, args.read_length, circular);
        write_fastq(&mut read1, ordinal, 1, &first, &qualities)?;
        if let Some(writer) = &mut read2 {
            let mate_start = (start + args.insert_length - args.read_length) % source.len();
            let mate = slice_sequence(source, mate_start, args.read_length, circular);
            write_fastq(writer, ordinal, 2, &reverse_complement(&mate), &qualities)?;
        }
    }
    read1.finish()?;
    if let Some(writer) = read2 {
        writer.finish()?;
    }

    let mut paths = vec![truth_path, read1_path];
    if !args.single_end {
        paths.push(read2_path);
    }
    paths.sort();
    let files = paths
        .iter()
        .map(|path| {
            Ok(FileDigest {
                path: path
                    .file_name()
                    .expect("generated path has a file name")
                    .to_string_lossy()
                    .into_owned(),
                sha256: sha256_file(path)?,
            })
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let manifest = Manifest {
        generator: "veritasm/examples/generate_synthetic.rs",
        generator_version: "1",
        seed_decimal: args.seed.to_string(),
        scenario: args.scenario,
        fragments_decimal: args.fragments.to_string(),
        mode: if args.single_end {
            "single_end"
        } else {
            "paired_end"
        },
        read_length: args.read_length,
        insert_length: args.insert_length,
        model_limitations: "Exact substitution-free reads from a deterministic xorshift64 genome model; not a biological or platform simulator.",
        files,
    };
    let manifest_path = args.out.join("dataset.json");
    let mut writer = BufWriter::new(File::create(manifest_path)?);
    serde_json::to_writer_pretty(&mut writer, &manifest)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn choose_source<'a>(
    scenario: Scenario,
    target_a: &'a [u8],
    target_b: &'a [u8],
    background: &'a [u8],
    rng: &mut XorShift64,
) -> &'a [u8] {
    match scenario {
        Scenario::Mixture if rng.next() % 10 == 0 => target_b,
        Scenario::HostContaminated if rng.next() % 100 != 0 => background,
        _ => target_a,
    }
}

fn choose_start(
    scenario: Scenario,
    length: usize,
    span: usize,
    circular: bool,
    rng: &mut XorShift64,
) -> Result<usize, Box<dyn std::error::Error>> {
    if span > length && !circular {
        return Err("read/insert span exceeds a linear source sequence".into());
    }
    if circular {
        return Ok(rng.next() as usize % length);
    }
    let choices = length - span + 1;
    if matches!(scenario, Scenario::Uneven) && rng.next() % 5 != 0 {
        Ok(rng.next() as usize % choices.div_ceil(4).max(1))
    } else {
        Ok(rng.next() as usize % choices)
    }
}

fn slice_sequence(sequence: &[u8], start: usize, length: usize, circular: bool) -> Vec<u8> {
    if !circular || start + length <= sequence.len() {
        return sequence[start..start + length].to_vec();
    }
    (0..length)
        .map(|offset| sequence[(start + offset) % sequence.len()])
        .collect()
}

fn write_fastq(
    writer: &mut Output,
    ordinal: u64,
    role: u8,
    sequence: &[u8],
    quality: &[u8],
) -> std::io::Result<()> {
    writeln!(writer, "@synthetic_{ordinal:012}/{role}")?;
    writer.write_all(sequence)?;
    writer.write_all(b"\n+\n")?;
    writer.write_all(quality)?;
    writer.write_all(b"\n")
}

fn write_truth(
    path: &Path,
    scenario: Scenario,
    target_a: &[u8],
    target_b: &[u8],
    background: &[u8],
) -> std::io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    write_fasta_record(&mut writer, "target_a", target_a)?;
    if matches!(scenario, Scenario::Mixture) {
        write_fasta_record(&mut writer, "target_b_minor_0.1_nominal", target_b)?;
    }
    if matches!(scenario, Scenario::HostContaminated) {
        write_fasta_record(&mut writer, "background_0.99_nominal", background)?;
    }
    writer.flush()
}

fn write_fasta_record(writer: &mut impl Write, id: &str, sequence: &[u8]) -> std::io::Result<()> {
    writeln!(writer, ">{id}")?;
    for line in sequence.chunks(80) {
        writer.write_all(line)?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

fn random_dna(length: usize, rng: &mut XorShift64) -> Vec<u8> {
    (0..length)
        .map(|_| match rng.next() & 3 {
            0 => b'A',
            1 => b'C',
            2 => b'G',
            _ => b'T',
        })
        .collect()
}

fn mutate_base(base: u8) -> u8 {
    match base {
        b'A' => b'C',
        b'C' => b'G',
        b'G' => b'T',
        b'T' => b'A',
        _ => unreachable!(),
    }
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
            _ => unreachable!(),
        })
        .collect()
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(lower_hex(&Sha256::digest(bytes)))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}
