//! Deterministic engineering probe for the standalone indexed exact mapper.
//!
//! This is not a biological benchmark and has no release acceptance threshold.

use std::error::Error;
use std::hint::black_box;
use std::time::{Duration, Instant};
use veritasm::audit::ReadState;
use veritasm::indexed_mapper::{IndexedExactMapper, IndexedMapperConfig, MappingWork};
use veritasm::model::{AvailabilityU64, Topology, Unitig};

const TARGET_LENGTH: usize = 200_000;
const READ_LENGTH: usize = 100;
const READ_COUNT: usize = 400;

fn main() -> Result<(), Box<dyn Error>> {
    let target_sequence = deterministic_dna(TARGET_LENGTH);
    let unitigs = [test_unitig(&target_sequence)];
    let reads = sampled_reads(&target_sequence);

    let started = Instant::now();
    let mapper = IndexedExactMapper::new(
        &unitigs,
        IndexedMapperConfig {
            seed_length: 15,
            index_memory_budget_bytes: 64 << 20,
            placement_memory_budget_bytes: 8 << 20,
        },
    )?;
    let index_build = started.elapsed();

    let started = Instant::now();
    let mut indexed_groups = Vec::with_capacity(reads.len());
    let mut work = MappingWork::default();
    for read in &reads {
        let mapping = mapper.map(black_box(read), 10_000)?;
        if mapping.state == ReadState::IndeterminateCandidateLimit {
            return Err("unexpected candidate-limit event in profile fixture".into());
        }
        indexed_groups.push(
            mapping
                .placement_groups
                .expect("enumeration-complete mapping must carry groups")
                .into_iter()
                .map(|group| (group.start as usize, group.strand))
                .collect::<Vec<_>>(),
        );
        add_work(&mut work, mapping.work)?;
    }
    let indexed_queries = started.elapsed();

    let started = Instant::now();
    let brute_groups: Vec<_> = reads
        .iter()
        .map(|read| brute_force_groups(black_box(&target_sequence), black_box(read)))
        .collect();
    let brute_queries = started.elapsed();
    if indexed_groups != brute_groups {
        return Err("indexed and brute-force placement sets differ".into());
    }

    let exhaustive_intervals = READ_COUNT
        .checked_mul(2)
        .and_then(|count| count.checked_mul(TARGET_LENGTH - READ_LENGTH + 1))
        .ok_or("exhaustive interval count overflow")?;
    println!("fixture=deterministic_uniform_dna_v1");
    println!("target_bases={TARGET_LENGTH}");
    println!("reads={READ_COUNT}");
    println!("read_length={READ_LENGTH}");
    println!("seed_length={}", mapper.seed_length());
    println!("index_postings={}", mapper.posting_count());
    println!("index_build_ms={:.3}", milliseconds(index_build));
    println!("indexed_query_ms={:.3}", milliseconds(indexed_queries));
    println!("brute_query_ms={:.3}", milliseconds(brute_queries));
    println!("brute_intervals={exhaustive_intervals}");
    println!("selected_seed_hits={}", work.selected_seed_hits);
    println!("postings_examined={}", work.postings_examined);
    println!(
        "indexed_full_verifications={}",
        work.indexed_full_verifications
    );
    println!(
        "verified_placement_groups={}",
        work.verified_placement_groups_seen
    );
    println!(
        "query_speed_ratio_brute_over_indexed={:.3}",
        ratio(brute_queries, indexed_queries)
    );
    println!(
        "amortized_speed_ratio_brute_over_build_plus_indexed={:.3}",
        ratio(brute_queries, index_build + indexed_queries)
    );
    Ok(())
}

fn deterministic_dna(length: usize) -> Vec<u8> {
    let mut state = 0x4d59_5df4_d0f3_3173u64;
    (0..length)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            b"ACGT"[((state >> 62) & 3) as usize]
        })
        .collect()
}

fn sampled_reads(target: &[u8]) -> Vec<Vec<u8>> {
    let start_domain = target.len() - READ_LENGTH + 1;
    (0..READ_COUNT)
        .map(|ordinal| {
            let start = ordinal.wrapping_mul(7_919) % start_domain;
            target[start..start + READ_LENGTH].to_vec()
        })
        .collect()
}

fn brute_force_groups(target: &[u8], read: &[u8]) -> Vec<(usize, char)> {
    let reverse = reverse_complement(read);
    let mut groups = Vec::new();
    for start in 0..=target.len() - read.len() {
        let interval = &target[start..start + read.len()];
        if interval == read {
            groups.push((start, '+'));
        }
        if interval == reverse {
            groups.push((start, '-'));
        }
    }
    groups
}

fn reverse_complement(read: &[u8]) -> Vec<u8> {
    read.iter()
        .rev()
        .map(|base| match base {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            _ => unreachable!("fixture DNA contains only ACGT"),
        })
        .collect()
}

fn add_work(total: &mut MappingWork, current: MappingWork) -> Result<(), Box<dyn Error>> {
    total.seed_lookups = checked_add(total.seed_lookups, current.seed_lookups)?;
    total.selected_seed_hits = checked_add(total.selected_seed_hits, current.selected_seed_hits)?;
    total.postings_examined = checked_add(total.postings_examined, current.postings_examined)?;
    total.indexed_full_verifications = checked_add(
        total.indexed_full_verifications,
        current.indexed_full_verifications,
    )?;
    total.fallback_full_verifications = checked_add(
        total.fallback_full_verifications,
        current.fallback_full_verifications,
    )?;
    total.verified_placement_groups_seen = checked_add(
        total.verified_placement_groups_seen,
        current.verified_placement_groups_seen,
    )?;
    Ok(())
}

fn checked_add(left: u64, right: u64) -> Result<u64, Box<dyn Error>> {
    left.checked_add(right)
        .ok_or_else(|| "profile work-counter overflow".into())
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn ratio(numerator: Duration, denominator: Duration) -> f64 {
    numerator.as_secs_f64() / denominator.as_secs_f64()
}

fn test_unitig(sequence: &[u8]) -> Unitig {
    Unitig {
        id: "profile-target".to_owned(),
        sequence: sequence.to_vec(),
        topology: Topology::Linear,
        edge_steps: 1,
        canonical_kmers: 1,
        minimum_support: 1,
        lower_median_support: 1,
        maximum_support: 1,
        enumeration_complete_read_placements: AvailabilityU64::Value(0),
        single_group_read_instances: AvailabilityU64::Value(0),
        multi_group_read_instances_with_group: AvailabilityU64::Value(0),
        placement_enumeration_status: "profile_fixture",
        sequence_sha256: "0".repeat(64),
    }
}
