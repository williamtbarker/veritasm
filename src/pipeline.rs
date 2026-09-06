//! End-to-end stable assembly state machine.

use crate::audit::{AuditConfig, AuditInputMode};
use crate::bundle::{write_bundle_with_lease, BundleData, BundleInputMode, RunLease};
use crate::compact::compact_graph;
use crate::config::{AssembleConfig, InputSpec, WORKER_STACK_BYTES};
use crate::count::{CountOptions, CountResult, CountWriter, HistogramBin};
use crate::dna::{scan_fragment, FragmentScan};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::graph::ExactGraph;
use crate::model::{Fragment, SourceSummary, TransformRecord, WindowStats};
use crate::pairs::audit_and_summarize_spool_stream;
use crate::spool::{create_spool, MemoryBoundedNext};
use crate::transform::build_transform_records;
use rayon::prelude::*;
use std::fs;
use std::path::Path;

const PIPELINE_FIXED_RESIDENT_ALLOWANCE: u64 = 64 << 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub destination: std::path::PathBuf,
    pub scientific_artifacts_digest: String,
    pub supplied_fragments: u64,
    pub observed_distinct_canonical_keys: u64,
    pub retained_distinct_canonical_keys: u64,
    pub emitted_unitigs: u64,
}

/// Run the exact stable vertical slice and atomically commit its evidence bundle.
pub fn assemble(config: &AssembleConfig) -> Result<RunOutcome> {
    config.validate()?;
    // Reserve the final destination before input metadata/open/read work and
    // before creating a run work directory. The exact lease is transferred to
    // the bundle transaction; there is no second, late lock acquisition.
    let lease = RunLease::acquire(&config.output_dir)?;
    let parent = lease.destination().parent().ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::DestinationUnsafePath,
            "canonical destination has no parent directory",
        )
    })?;
    let work = tempfile::Builder::new()
        .prefix(".veritasm-work-")
        .tempdir_in(parent)
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!("cannot create private work directory: {cause}"),
            )
        })?;

    let mut spool = create_spool(
        &config.input,
        &config.scientific,
        &config.limits,
        work.path(),
    )?;
    let spool_bytes = fs::metadata(spool.path())
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::IntegritySpool,
                format!("cannot inspect completed spool: {cause}"),
            )
        })?
        .len();

    let count_options =
        CountOptions::from_config(work.path(), &config.scientific, &config.limits, spool_bytes)?;
    let mut counter = CountWriter::new(count_options)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.threads)
        .stack_size(WORKER_STACK_BYTES)
        .build()
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                format!("cannot create requested worker pool: {cause}"),
            )
        })?;
    let mut windows = WindowStats::default();
    scan_spool_in_batches(&spool, config, &pool, &mut counter, &mut windows)?;
    // Worker stacks and scheduler state do not overlap the exact-count
    // summary, graph, compaction, audit, or output phases.
    drop(pool);
    let counts = counter.finish(spool.fragment_count)?;
    let transformations = build_transform_records(&counts, &config.scientific)?;

    let CountResult {
        observed_distinct,
        observed_support_mass,
        retained_distinct,
        retained_support_mass,
        histogram,
        retained,
        ..
    } = counts;
    let resident_metadata_bytes = pipeline_metadata_bytes(&spool, &histogram, &transformations)?;
    let graph_phase_budget = remaining_phase_budget(
        config.limits.memory_budget_bytes,
        resident_metadata_bytes,
        "exact graph and count-summary metadata",
    )?;
    let graph =
        ExactGraph::from_sorted_retained_owned(config.scientific.k, retained, graph_phase_budget)?;
    let graph_stats = graph.stats();
    let mut compaction = compact_graph(&graph, graph_phase_budget)?;
    drop(graph);

    // Exact run files are no longer needed once their retained full keys have
    // been consumed into the graph. This is a run-owned private directory.
    remove_private_count_dir(work.path())?;

    let audit_mode = input_audit_mode(&config.input);
    let audited = audit_and_summarize_spool_stream(
        spool.iter()?,
        &mut compaction.unitigs,
        AuditConfig {
            input_mode: audit_mode,
            remap: config.scientific.remap,
            min_base_quality: config.scientific.min_base_quality,
            max_mapping_candidates: config.limits.max_mapping_candidates,
            memory_budget_bytes: config.limits.memory_budget_bytes,
        },
    )?;

    let sources = std::mem::take(&mut spool.sources);
    let supplied_fragments = spool.fragment_count;
    let supplied_reads = spool.read_count;
    let supplied_bases = spool.stats.bases;
    let raw_transport_bytes = spool.stats.raw_transport_bytes;
    let decoded_input_bytes = spool.stats.decoded_input_bytes;
    let inferred_mate_roles = spool.stats.inferred_mate_roles;
    let gzip_sources = spool.stats.gzip_sources;
    let gzip_members = spool.stats.gzip_members;
    let spool_sha256 = std::mem::take(&mut spool.sha256);
    drop(spool);

    let input_mode = match config.input {
        InputSpec::Single(_) => BundleInputMode::SingleEnd,
        InputSpec::Paired { .. } => BundleInputMode::PairedEnd,
    };
    let bundle_data = BundleData {
        input_mode,
        sources,
        supplied_fragment_instances: supplied_fragments,
        supplied_read_instances: supplied_reads,
        supplied_bases,
        raw_transport_bytes,
        decoded_input_bytes,
        inferred_mate_roles,
        gzip_sources,
        gzip_members,
        spool_sha256,
        spool_bytes,
        scientific: config.scientific.clone(),
        limits: config.limits.clone(),
        windows,
        observed_distinct_canonical_keys: observed_distinct,
        observed_support_mass,
        retained_distinct_canonical_keys: retained_distinct,
        retained_support_mass,
        support_histogram: histogram,
        oriented_handles: graph_stats.oriented_handles,
        self_reverse_complement_keys: graph_stats.self_reverse_complement_keys,
        unitigs: compaction.unitigs,
        graph_links: compaction.links,
        mapper: audited.audit.mapper,
        read_state_counts: audited.audit.state_counts,
        pair_state_counts: audited.pairs.state_counts,
        pair_lane_state_counts: audited.pairs.lane_state_counts,
        pair_links: audited.pairs.links,
        transformations,
    };
    let emitted_unitigs = bundle_data.unitigs.len() as u64;
    let outcome = write_bundle_with_lease(lease, &bundle_data)?;
    Ok(RunOutcome {
        destination: outcome.destination,
        scientific_artifacts_digest: outcome.scientific_artifacts_digest,
        supplied_fragments,
        observed_distinct_canonical_keys: observed_distinct,
        retained_distinct_canonical_keys: retained_distinct,
        emitted_unitigs,
    })
}

fn scan_spool_in_batches(
    spool: &crate::spool::Spool,
    config: &AssembleConfig,
    pool: &rayon::ThreadPool,
    counter: &mut CountWriter,
    total_windows: &mut WindowStats,
) -> Result<()> {
    let maximum_fragments = usize::try_from(config.limits.batch_fragments)
        .map_err(|_| overflow("batch-fragments does not fit usize"))?;
    let memory_share = config.limits.memory_budget_bytes / 2;
    // Keep the queue allocation fixed so a `push` cannot transiently grow it
    // after the next fragment has already been decoded.  The configured value
    // remains an upper bound; 4,096 is the stable internal scheduling cap.
    let batch_capacity = maximum_fragments.min(4_096);
    let mut batch = Vec::new();
    batch.try_reserve_exact(batch_capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve scan batch queue: {cause}"),
        )
    })?;
    let batch_slot_bytes = u64::try_from(batch.capacity())
        .map_err(|_| overflow("scan batch capacity does not fit u64"))?
        .checked_mul(
            u64::try_from(std::mem::size_of::<Fragment>())
                .map_err(|_| overflow("fragment slot size does not fit u64"))?,
        )
        .ok_or_else(|| overflow("scan batch queue byte accounting"))?;
    let fragment_share = memory_share.checked_sub(batch_slot_bytes).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            "scan batch queue exceeds its memory share",
        )
    })?;
    let mut batch_bytes = 0_u64;
    let mut fragments = spool.iter()?;
    loop {
        if batch.len() == batch_capacity {
            process_batch(&batch, config, pool, counter, total_windows)?;
            batch.clear();
            batch_bytes = 0;
        }
        let available = fragment_share
            .checked_sub(batch_bytes)
            .ok_or_else(|| overflow("scan batch available-memory accounting"))?;
        match fragments.next_with_memory_limit(available)? {
            MemoryBoundedNext::Fragment {
                fragment,
                memory_bytes,
            } => {
                batch_bytes = batch_bytes
                    .checked_add(memory_bytes)
                    .ok_or_else(|| overflow("scan batch byte accounting overflow"))?;
                batch.push(fragment);
            }
            MemoryBoundedNext::RequiresMemory(_) if !batch.is_empty() => {
                process_batch(&batch, config, pool, counter, total_windows)?;
                batch.clear();
                batch_bytes = 0;
            }
            MemoryBoundedNext::RequiresMemory(required) => {
                let total_required = required
                    .checked_add(batch_slot_bytes)
                    .ok_or_else(|| overflow("scan admission total byte estimate"))?;
                return Err(VeritasmError::new(
                    ErrorCode::ResourceMemory,
                    format!(
                        "one spooled fragment and the fixed batch queue require an estimated {total_required} allocator bytes, exceeding the {memory_share}-byte scanning memory share"
                    ),
                ));
            }
            MemoryBoundedNext::End => break,
        }
    }
    if !batch.is_empty() {
        process_batch(&batch, config, pool, counter, total_windows)?;
    }
    Ok(())
}

fn process_batch(
    batch: &[Fragment],
    config: &AssembleConfig,
    pool: &rayon::ThreadPool,
    counter: &mut CountWriter,
    total_windows: &mut WindowStats,
) -> Result<()> {
    let scans: Result<Vec<FragmentScan>> = pool.install(|| {
        batch
            .par_iter()
            .map(|fragment| {
                scan_fragment(
                    fragment,
                    config.scientific.k,
                    config.scientific.min_base_quality,
                    config.scientific.support_unit,
                )
            })
            .collect()
    });
    for (fragment, scan) in batch.iter().zip(scans?) {
        add_windows(total_windows, &scan.stats)?;
        counter.observe_fragment(fragment.ordinal, &scan.kmers)?;
    }
    Ok(())
}

fn add_windows(total: &mut WindowStats, value: &WindowStats) -> Result<()> {
    total.possible = checked_add(total.possible, value.possible, "possible windows")?;
    total.accepted = checked_add(total.accepted, value.accepted, "accepted windows")?;
    total.ambiguity_only = checked_add(
        total.ambiguity_only,
        value.ambiguity_only,
        "ambiguity-only windows",
    )?;
    total.quality_only = checked_add(
        total.quality_only,
        value.quality_only,
        "quality-only windows",
    )?;
    total.ambiguity_and_quality = checked_add(
        total.ambiguity_and_quality,
        value.ambiguity_and_quality,
        "ambiguity-and-quality windows",
    )?;
    Ok(())
}

fn checked_add(left: u64, right: u64, label: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(label))
}

fn remaining_phase_budget(total: u64, resident: u64, label: &'static str) -> Result<u64> {
    total.checked_sub(resident).filter(|remaining| *remaining > 0).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "resident {label} requires an estimated {resident} allocator bytes, leaving no phase allocation from the {total}-byte memory budget"
            ),
        )
    })
}

fn capacity_bytes<T>(capacity: usize, label: &'static str) -> Result<u64> {
    let bytes = capacity
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(|| overflow("pipeline capacity byte accounting overflow"))?;
    let _ = label;
    u64::try_from(bytes).map_err(|_| overflow("pipeline capacity bytes do not fit u64"))
}

fn add_string_capacity(total: &mut u64, value: &String, label: &'static str) -> Result<()> {
    let bytes = u64::try_from(value.capacity())
        .map_err(|_| overflow("pipeline string capacity does not fit u64"))?;
    *total = checked_add(*total, bytes, label)?;
    Ok(())
}

fn pipeline_metadata_bytes(
    spool: &crate::spool::Spool,
    histogram: &Vec<HistogramBin>,
    transformations: &Vec<TransformRecord>,
) -> Result<u64> {
    let mut total = PIPELINE_FIXED_RESIDENT_ALLOWANCE;
    total = checked_add(
        total,
        capacity_bytes::<SourceSummary>(spool.sources.capacity(), "source summaries")?,
        "pipeline source-summary allocation",
    )?;
    for source in &spool.sources {
        add_string_capacity(
            &mut total,
            &source.raw_transport_sha256,
            "source raw-digest allocation",
        )?;
        add_string_capacity(
            &mut total,
            &source.logical_decoded_sha256,
            "source logical-digest allocation",
        )?;
    }
    add_string_capacity(&mut total, &spool.sha256, "spool digest allocation")?;
    add_string_capacity(
        &mut total,
        &spool.pretrailer_sha256,
        "spool pretrailer-digest allocation",
    )?;
    total = checked_add(
        total,
        capacity_bytes::<HistogramBin>(histogram.capacity(), "support histogram")?,
        "pipeline support-histogram allocation",
    )?;
    total = checked_add(
        total,
        capacity_bytes::<TransformRecord>(transformations.capacity(), "transformation journal")?,
        "pipeline transformation-journal allocation",
    )?;
    for transform in transformations {
        add_string_capacity(
            &mut total,
            &transform.parameters_json,
            "transformation parameter allocation",
        )?;
        add_string_capacity(
            &mut total,
            &transform.decision_set_sha256,
            "transformation decision-digest allocation",
        )?;
        add_string_capacity(
            &mut total,
            &transform.pre_state_sha256,
            "transformation pre-state allocation",
        )?;
        add_string_capacity(
            &mut total,
            &transform.post_state_sha256,
            "transformation post-state allocation",
        )?;
    }
    Ok(total)
}

fn input_audit_mode(input: &InputSpec) -> AuditInputMode {
    match input {
        InputSpec::Single(_) => AuditInputMode::SingleEnd,
        InputSpec::Paired { .. } => AuditInputMode::PairedEnd,
    }
}

fn remove_private_count_dir(work: &Path) -> Result<()> {
    let count = work.join("count");
    if count.parent() != Some(work)
        || count.file_name().and_then(|name| name.to_str()) != Some("count")
    {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            "refusing to remove an unexpected count path",
        ));
    }
    fs::remove_dir_all(&count).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot remove private exact-count directory: {cause}"),
        )
    })
}
