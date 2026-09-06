//! Narrow exact-substring evaluator for deterministic development fixtures.
//!
//! This is not a general assembly evaluator. It is suitable only when every
//! expected assembly sequence is an error-free substring of one of the supplied
//! linear truth records (in either orientation). It deliberately does not make
//! approximate alignments, resolve repeats, score circular seams, or infer
//! biological origin.

use clap::Parser;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(about = "Evaluate exact unitig substrings against linear fixture truth")]
struct Args {
    #[arg(long)]
    truth: PathBuf,
    #[arg(long)]
    assembly: PathBuf,
}

#[derive(Debug, Serialize)]
struct Evaluation {
    evaluator: &'static str,
    evaluator_version: &'static str,
    applicability: &'static str,
    truth_sha256: String,
    assembly_sha256: String,
    truth_records: usize,
    truth_bases: u64,
    assembly_records: usize,
    assembly_bases: u64,
    exact_compatible_records: usize,
    exact_compatible_bases: u64,
    non_substring_records: usize,
    non_substring_bases: u64,
    truth_bases_covered: u64,
    truth_fraction_numerator: u64,
    truth_fraction_denominator: u64,
    aligned_duplication_numerator: u64,
    aligned_duplication_denominator: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let truth_bytes = fs::read(&args.truth)?;
    let assembly_bytes = fs::read(&args.assembly)?;
    let truth = parse_fasta(&truth_bytes, &args.truth)?;
    let assembly = parse_fasta(&assembly_bytes, &args.assembly)?;

    let truth_bases = total_bases(&truth)?;
    let assembly_bases = total_bases(&assembly)?;
    let mut covered = truth
        .iter()
        .map(|sequence| vec![false; sequence.len()])
        .collect::<Vec<_>>();
    let mut compatible_records = 0usize;
    let mut compatible_bases = 0u64;

    for contig in &assembly {
        let mut compatible = false;
        for (truth_index, sequence) in truth.iter().enumerate() {
            for start in exact_starts(sequence, contig) {
                compatible = true;
                covered[truth_index][start..start + contig.len()].fill(true);
            }
            let reverse = reverse_complement(sequence);
            for reverse_start in exact_starts(&reverse, contig) {
                compatible = true;
                let start = sequence.len() - (reverse_start + contig.len());
                covered[truth_index][start..start + contig.len()].fill(true);
            }
        }
        if compatible {
            compatible_records += 1;
            compatible_bases = compatible_bases
                .checked_add(u64::try_from(contig.len())?)
                .ok_or("compatible-base count overflow")?;
        }
    }

    let mut truth_bases_covered = 0_u64;
    for row in &covered {
        let value = u64::try_from(row.iter().filter(|covered| **covered).count())?;
        truth_bases_covered = truth_bases_covered
            .checked_add(value)
            .ok_or("covered-base count overflow")?;
    }
    let non_substring_records = assembly.len() - compatible_records;
    let non_substring_bases = assembly_bases
        .checked_sub(compatible_bases)
        .ok_or("non-substring base subtraction underflow")?;
    let evaluation = Evaluation {
        evaluator: "veritasm/examples/evaluate_exact_truth.rs",
        evaluator_version: "1",
        applicability: "Exact error-free substrings of linear fixture truth only; no approximate alignment, circular-seam, repeat-resolution, phasing, or biological interpretation.",
        truth_sha256: lower_hex(&Sha256::digest(&truth_bytes)),
        assembly_sha256: lower_hex(&Sha256::digest(&assembly_bytes)),
        truth_records: truth.len(),
        truth_bases,
        assembly_records: assembly.len(),
        assembly_bases,
        exact_compatible_records: compatible_records,
        exact_compatible_bases: compatible_bases,
        non_substring_records,
        non_substring_bases,
        truth_bases_covered,
        truth_fraction_numerator: truth_bases_covered,
        truth_fraction_denominator: truth_bases,
        aligned_duplication_numerator: compatible_bases,
        aligned_duplication_denominator: truth_bases_covered,
    };
    serde_json::to_writer_pretty(std::io::stdout().lock(), &evaluation)?;
    println!();
    Ok(())
}

fn parse_fasta(bytes: &[u8], path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let text = std::str::from_utf8(bytes)?;
    let mut records = Vec::<Vec<u8>>::new();
    for line in text.lines() {
        if let Some(identifier) = line.strip_prefix('>') {
            if identifier.trim().is_empty() {
                return Err(format!("{}: empty FASTA identifier", path.display()).into());
            }
            records.push(Vec::new());
        } else if !line.trim().is_empty() {
            let sequence = records
                .last_mut()
                .ok_or_else(|| format!("{}: sequence before FASTA header", path.display()))?;
            for base in line.trim().bytes() {
                let normalized = base.to_ascii_uppercase();
                if !matches!(normalized, b'A' | b'C' | b'G' | b'T') {
                    return Err(
                        format!("{}: non-ACGT truth/evaluation base", path.display()).into(),
                    );
                }
                sequence.push(normalized);
            }
        }
    }
    if records.is_empty() || records.iter().any(Vec::is_empty) {
        return Err(format!("{}: empty FASTA input or record", path.display()).into());
    }
    Ok(records)
}

fn total_bases(records: &[Vec<u8>]) -> Result<u64, Box<dyn std::error::Error>> {
    records.iter().try_fold(0u64, |total, sequence| {
        let length = u64::try_from(sequence.len())?;
        total
            .checked_add(length)
            .ok_or_else(|| "FASTA base count overflow".into())
    })
}

fn exact_starts<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(move |(start, window)| (window == needle).then_some(start))
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
            _ => unreachable!("parser permits only ACGT"),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_positions_include_overlaps() {
        assert_eq!(exact_starts(b"AAAA", b"AA").collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn reverse_complement_is_correct() {
        assert_eq!(reverse_complement(b"AACGT"), b"ACGTT");
    }
}
