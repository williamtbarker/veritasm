//! Opaque-capability orchestration for the experimental multi-k portfolio.
//!
//! Each sorted child is reconstructed independently from the same immutable
//! spool.  No cross-k edge, sequence splice, vote, or preferred-child selection is
//! implemented.  The stable assembler does not call this module.

use super::authenticated_pair_graph::{
    adapt_authenticated_witnessed_child, analyze_authenticated_pair_paths,
    produce_authenticated_pair_placement_evidence, AuthenticatedPairGraph, PairGraphAdapterLimits,
};
use super::compacted_dbg::{
    compact_retained_counts, AuthenticatedCompactedGraph, CompactedGraphLimits, CompactedTopology,
    EdgeOrientation, UnitigOrientation,
};
use super::evidence_reconstruction::{
    constrain_authenticated_compacted_child, AdjacencyEvidenceKind, AuthenticatedWitnessedChild,
    AuthenticatedWitnessedChildView, OriginalReadTransitionEvidence, ReconstructionLimits,
    SourceEquivalence as ReconstructionSourceEquivalence, TransitionCandidateOrigin,
    TransitionDecisionStatus,
};
use super::external_reduce::ExternalPartitionLimits;
use super::external_run::WideRunSupportUnit;
use super::multik_bundle::{
    write_unverified_child_snapshot, write_unverified_multik_bundle_with_lease, ChildSnapshotRef,
    DisabledCapabilityState, MultiKBundleLimits, MultiKBundleOutcome, PairEvidenceState,
    PortfolioProfile, ReportAdjacency, ReportConservation, ReportEdgeOrientation, ReportEdgeStep,
    ReportLink, ReportOrientation, ReportPairEvidence, ReportPairEvidencePayload,
    ReportPairLaneModel, ReportRetention, ReportRetentionRule, ReportSegment, ReportTopology,
    ReportTransitionDecision, ReportTransitionEvidence, UnverifiedChildSnapshotPayload,
    UnverifiedExecutionReport, UnverifiedMultiKBundleData, SNAPSHOT_CODEC_MEMORY_BYTES,
};
use super::pair_mapper::{
    pair_mapping_source_root_sha256, CalibrationSplitRule, PairMapperConfig, PairMapperLimits,
    PairMapperResult,
};
use super::pair_path::{
    validate_authenticated_pair_path_result, AggregatePathAvailability,
    AuthenticatedPairPathResult, LaneModelAvailability, LibraryOrientation, ModelConfig,
    PairPathConfig, WorkLimits,
};
use super::retention::{
    authenticate_spool_external_counts, retain_spool_authenticated_counts, RetainedCountArtifact,
    RetentionLimits, RetentionRule,
};
use super::spool_external::SpoolExternalOptions;
use super::transition_witness::{
    build_transition_ledger, validate_transition_k_values, TransitionLedger,
    TransitionLedgerLimits, TransitionLedgerOptions,
};
use crate::bundle::RunLease;
use crate::config::{AssembleConfig, InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
use crate::error::{ErrorCode, Result, VeritasmError};
use crate::spool::{create_spool, Spool};
use serde::Serialize;
use std::fs;
use std::io::{self, Write};
use std::mem::size_of;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// All explicit resource envelopes used by one portfolio run.
#[derive(Debug, Clone)]
pub struct MultiKResourceLimits {
    /// Maximum sum of component-owned payload limits in every modelled live
    /// phase. This is deliberately not described as a process-RSS bound.
    pub max_aggregate_accounted_memory_bytes: u64,
    /// Accounted owned bytes admitted for the one-way child reporting copy.
    pub max_child_report_accounted_bytes: u64,
    /// Aggregate live temporary bytes across the immutable spool, private run
    /// files, accumulated child snapshots, and final sibling staging.
    pub max_aggregate_temp_bytes: u64,
    pub stable_input_and_spool: Limits,
    pub external_counts: ExternalPartitionLimits,
    pub retention: RetentionLimits,
    pub compacted_graph: CompactedGraphLimits,
    pub transition_ledger: TransitionLedgerLimits,
    pub reconstruction: ReconstructionLimits,
    pub pair_graph_adapter: PairGraphAdapterLimits,
    pub pair_mapper: PairMapperLimits,
    pub bundle: MultiKBundleLimits,
}

impl Default for MultiKResourceLimits {
    fn default() -> Self {
        let stable = Limits::default();
        Self {
            max_aggregate_accounted_memory_bytes: stable.memory_budget_bytes,
            max_child_report_accounted_bytes: 64 << 20,
            max_aggregate_temp_bytes: stable.max_temp_bytes,
            external_counts: ExternalPartitionLimits {
                max_segments: 100_000_000,
                max_input_bases: 100_000_000_000,
                max_windows: 100_000_000_000,
                max_distinct_kmers: 1_000_000,
                max_memory_bytes: 256 << 20,
                sort_buffer_bytes: 32 << 20,
                io_buffer_bytes: 64 << 10,
                max_temp_bytes: stable.max_temp_bytes,
                max_run_files: 512,
                merge_fan_in: 16,
                max_open_files: 17,
            },
            retention: RetentionLimits {
                max_raw_keys: 1_000_000,
                max_retained_keys: 1_000_000,
                max_accounted_bytes: 64 << 20,
            },
            compacted_graph: CompactedGraphLimits {
                max_canonical_edges: 1_000_000,
                max_oriented_handles: 2_000_000,
                max_literal_nodes: 2_000_000,
                max_unitigs: 1_000_000,
                max_link_candidates: 4_000_000,
                max_output_bases: 2_000_000_000,
                max_accounted_bytes: 96 << 20,
            },
            transition_ledger: TransitionLedgerLimits {
                max_events: 100_000_000_000,
                max_rows: 1_000_000,
                max_fragment_decode_bytes: 16 << 20,
                max_fragment_windows: 500_000,
                max_memory_bytes: 160 << 20,
                sort_buffer_bytes: 32 << 20,
                max_temp_bytes: stable.max_temp_bytes,
                max_run_files: 512,
                merge_fan_in: 16,
                max_open_files: 17,
            },
            reconstruction: ReconstructionLimits {
                max_input_edges: 1_000_000,
                max_topology_candidates: 10_000_000,
                max_decision_rows: 20_000_000,
                max_segments: 1_000_000,
                max_links: 10_000_000,
                max_output_bases: 2_000_000_000,
                max_accounted_bytes: 96 << 20,
                max_verification_scratch_bytes: 32 << 20,
            },
            pair_graph_adapter: PairGraphAdapterLimits {
                maximum_segments: 100_000,
                maximum_links: 200_000,
                maximum_sequence_bases: 100_000_000,
                maximum_accounted_bytes: 64 << 20,
                pair_path: WorkLimits {
                    maximum_pairs: 8_192,
                    maximum_placements: 65_536,
                    maximum_placement_pairs_per_fragment: 16,
                    maximum_edges_per_path: 32,
                    maximum_search_states_per_fragment: 256,
                    maximum_search_arc_examinations_per_fragment: 2_048,
                    maximum_path_reconstruction_elements_per_fragment: 8_192,
                    maximum_target_paths_per_fragment: 32,
                    maximum_compatible_paths_per_fragment: 4,
                    maximum_worker_threads: 1,
                    maximum_mapper_algorithm_bytes: 256,
                    maximum_mapper_version_bytes: 128,
                    maximum_mapper_parameter_bytes: 4 << 10,
                    maximum_graph_sequence_bases: 100_000_000,
                    graph_memory_bytes: 32 << 20,
                    search_memory_bytes_per_worker: 16 << 20,
                    result_memory_bytes: 192 << 20,
                    analysis_memory_bytes: 256 << 20,
                },
            },
            pair_mapper: PairMapperLimits {
                maximum_fragments: 8_192,
                maximum_reads: 16_384,
                maximum_bases: 100_000_000,
                maximum_graph_sequence_bases: 100_000_000,
                maximum_mapping_candidates_per_read: 256,
                maximum_admitted_mapping_operations: 100_000_000_000,
                maximum_placement_groups: 65_536,
                maximum_placements: 65_536,
                maximum_batch_fragments: 1_024,
                maximum_decoded_batch_bytes: 32 << 20,
                mapper_index_memory_bytes: 64 << 20,
                mapper_query_memory_bytes_per_worker: 16 << 20,
                result_memory_bytes: 64 << 20,
                total_accounted_memory_bytes: 256 << 20,
                maximum_worker_threads: 1,
                maximum_mapper_parameter_bytes: 4 << 10,
            },
            bundle: MultiKBundleLimits::default(),
            stable_input_and_spool: stable,
        }
    }
}

/// Scientific parameters for the paired evidence-only analysis. Paired input
/// always runs this plane; single-end input records it as not applicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiKPairEvidenceConfig {
    pub seed_length: usize,
    pub calibration_split: CalibrationSplitRule,
    pub model: ModelConfig,
    pub minimum_distinct_fragments_for_path: u64,
}

impl Default for MultiKPairEvidenceConfig {
    fn default() -> Self {
        Self {
            seed_length: 15,
            calibration_split: CalibrationSplitRule {
                salt: [0x5d; 32],
                numerator: 1,
                denominator: 4,
            },
            model: ModelConfig {
                minimum_anchors: 10,
                minimum_dominant_anchors: 10,
                dominance_numerator: 9,
                dominance_denominator: 10,
                maximum_span_p90_minus_p10: 1_000,
                maximum_inner_p90_minus_p10: 1_000,
            },
            minimum_distinct_fragments_for_path: 2,
        }
    }
}

/// Configuration for the separate experimental portfolio executable.
#[derive(Debug, Clone)]
pub struct MultiKPortfolioConfig {
    pub input: InputSpec,
    pub output_dir: PathBuf,
    /// Must be strictly increasing and unique; current integrated range is 3..=63.
    pub ks: Vec<u8>,
    pub support_unit: SupportUnit,
    pub retention_rule: RetentionRule,
    pub min_base_quality: u8,
    pub profile: PortfolioProfile,
    /// Execution is deliberately serial in this alpha. The only accepted
    /// value is one; requests are never silently ignored.
    pub worker_threads: usize,
    pub minimizer_length_ceiling: u8,
    pub virtual_bucket_count: u32,
    pub pair_evidence: MultiKPairEvidenceConfig,
    pub limits: MultiKResourceLimits,
}

/// Successful, atomically committed experimental portfolio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiKPortfolioOutcome {
    pub destination: PathBuf,
    pub parent_root: [u8; 32],
    pub operational_root: [u8; 32],
    pub manifest_sha256: [u8; 32],
    pub children: u64,
    pub segments: u64,
}

impl MultiKPortfolioConfig {
    /// Validate every scalar and cross-field resource choice before a
    /// destination lease is acquired or input is opened.
    pub fn validate(&self) -> Result<()> {
        if self.ks.is_empty()
            || self.ks.iter().any(|k| !(3..=63).contains(k))
            || self.ks.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(config_error(
                ErrorCode::ConfigurationInvalidK,
                "integrated multi-k children require strictly increasing unique k values in 3..=63",
            ));
        }
        validate_transition_k_values(&self.ks)?;
        if self.minimizer_length_ceiling == 0 || self.virtual_bucket_count == 0 {
            return Err(config_error(
                ErrorCode::ConfigurationInvalidLimit,
                "multi-k minimizer ceiling and virtual bucket count must be nonzero",
            ));
        }
        if matches!(
            self.retention_rule,
            RetentionRule::InclusiveSupport { minimum_support: 0 }
        ) {
            return Err(config_error(
                ErrorCode::ConfigurationInvalidSupport,
                "inclusive retention requires minimum support of at least one",
            ));
        }
        if self.worker_threads != 1 {
            return Err(config_error(
                ErrorCode::ConfigurationInvalidLimit,
                "experimental multi-k execution is serial; worker threads must equal one",
            ));
        }
        if self.ks.len() > usize::from(self.limits.bundle.max_children) {
            return Err(config_error(
                ErrorCode::ConfigurationInvalidLimit,
                "multi-k child count exceeds the configured bundle limit",
            ));
        }
        let scientific = spool_scientific_config(self)?;
        AssembleConfig {
            input: self.input.clone(),
            output_dir: self.output_dir.clone(),
            scientific,
            limits: self.limits.stable_input_and_spool.clone(),
            threads: self.worker_threads,
        }
        .validate()?;
        for &k in &self.ks {
            self.limits.stable_input_and_spool.validate(k)?;
        }
        let paired = matches!(self.input, InputSpec::Paired { .. });
        if paired {
            validate_pair_configuration(self.pair_evidence, &self.limits)?;
        }
        validate_resource_limits(&self.limits, paired)?;
        Ok(())
    }
}

/// Build and atomically publish an experimental authenticated portfolio.
///
/// This establishes software derivation invariants only. It makes no
/// sensitivity, accuracy, sample-content inference, clinical, or performance claim.
pub fn run_authenticated_multik_portfolio(
    config: &MultiKPortfolioConfig,
) -> Result<MultiKPortfolioOutcome> {
    config.validate()?;
    // This exact cooperative lease precedes input metadata/open/read and all
    // run work. It is transferred unchanged to the no-replace bundle commit.
    let lease = RunLease::acquire(&config.output_dir)?;
    let parent = lease.destination().parent().ok_or_else(|| {
        config_error(
            ErrorCode::DestinationUnsafePath,
            "canonical multi-k destination has no parent directory",
        )
    })?;
    let work = tempfile::Builder::new()
        .prefix(".veritasm-multik-work-")
        .tempdir_in(parent)
        .map_err(|cause| {
            config_error(
                ErrorCode::ResourceTemporaryBytes,
                format!("cannot create private multi-k work directory: {cause}"),
            )
        })?;
    let result = run_in_work(config, lease, work.path());
    match result {
        Ok(outcome) => {
            // The bundle is already committed. Cleanup is best effort because
            // a post-commit error would incorrectly report overall failure.
            drop(work);
            Ok(outcome)
        }
        Err(primary) => match work.close() {
            Ok(()) => Err(primary),
            Err(cleanup) => Err(config_error(
                ErrorCode::ResourceTemporaryBytes,
                format!(
                    "multi-k run failed ({primary}); private work cleanup also failed: {cleanup}"
                ),
            )),
        },
    }
}

fn run_in_work(
    config: &MultiKPortfolioConfig,
    lease: RunLease,
    work: &Path,
) -> Result<MultiKPortfolioOutcome> {
    let scientific = spool_scientific_config(config)?;
    let spool = create_spool(
        &config.input,
        &scientific,
        &config.limits.stable_input_and_spool,
        work,
    )?;
    spool.verify()?;
    let source_root = super::transition_witness::transition_source_root(&spool)?;
    let pair_evidence = pair_evidence_state(
        &config.input,
        spool.fragment_count(),
        config.limits.pair_mapper.maximum_fragments,
    );
    let spool_bytes = spool.registered_byte_len();
    enforce_aggregate_temp(
        spool_bytes,
        config.limits.max_aggregate_temp_bytes,
        "immutable spool",
    )?;
    let mut snapshot_bytes = 0_u64;
    let mut snapshots = Vec::new();
    snapshots
        .try_reserve_exact(config.ks.len())
        .map_err(|cause| {
            config_error(
                ErrorCode::ResourceMemory,
                format!("cannot reserve multi-k child snapshot references: {cause}"),
            )
        })?;
    let child_context = ChildBuildContext {
        config,
        spool: &spool,
        parent_work: work,
        source_root,
        pair_evidence,
        spool_bytes,
    };
    for &k in &config.ks {
        let snapshot = build_one_child(&child_context, k, snapshot_bytes)?;
        snapshot_bytes = checked_add(
            snapshot_bytes,
            snapshot.byte_len(),
            "aggregate child-snapshot bytes",
        )?;
        enforce_aggregate_temp(
            checked_add(spool_bytes, snapshot_bytes, "spool plus child snapshots")?,
            config.limits.max_aggregate_temp_bytes,
            "spool plus accumulated child snapshots",
        )?;
        snapshots.push(snapshot);
    }

    let live_before_staging = checked_add(
        spool_bytes,
        snapshot_bytes,
        "spool plus snapshots before final staging",
    )?;
    let available_staging = config
        .limits
        .max_aggregate_temp_bytes
        .checked_sub(live_before_staging)
        .ok_or_else(|| {
            config_error(
                ErrorCode::ResourceTemporaryBytes,
                "spool and snapshots exceed the aggregate temporary-byte ceiling",
            )
        })?;
    let mut bundle_limits = config.limits.bundle;
    bundle_limits.max_staged_output_bytes =
        bundle_limits.max_staged_output_bytes.min(available_staging);
    if bundle_limits.max_staged_output_bytes == 0 {
        return Err(config_error(
            ErrorCode::ResourceTemporaryBytes,
            "aggregate temporary-byte ceiling leaves no final staging allowance",
        ));
    }

    let data = UnverifiedMultiKBundleData {
        source_root,
        input_mode: config.input.mode_name().to_owned(),
        support_unit: config.support_unit.as_str().to_owned(),
        min_base_quality: config.min_base_quality,
        profile: config.profile,
        pair_evidence,
        quality_correction: DisabledCapabilityState::DisabledUnqualified,
        execution: UnverifiedExecutionReport {
            execution_threads: 1,
            max_aggregate_accounted_memory_bytes: config
                .limits
                .max_aggregate_accounted_memory_bytes,
            projected_aggregate_accounted_memory_bytes: aggregate_memory_projection(
                &config.limits,
                matches!(config.input, InputSpec::Paired { .. }),
            )?,
            max_aggregate_temp_bytes: config.limits.max_aggregate_temp_bytes,
            max_final_staging_bytes: bundle_limits.max_staged_output_bytes,
        },
        children: snapshots,
        limits: bundle_limits,
    };
    let outcome = write_unverified_multik_bundle_with_lease(lease, &data)?;
    Ok(portfolio_outcome(outcome))
}

struct ChildBuildContext<'a> {
    config: &'a MultiKPortfolioConfig,
    spool: &'a Spool,
    parent_work: &'a Path,
    source_root: [u8; 32],
    pair_evidence: PairEvidenceState,
    spool_bytes: u64,
}

fn build_one_child(
    context: &ChildBuildContext<'_>,
    k: u8,
    prior_snapshot_bytes: u64,
) -> Result<ChildSnapshotRef> {
    let config = context.config;
    let spool = context.spool;
    let parent_work = context.parent_work;
    let source_root = context.source_root;
    let pair_evidence = context.pair_evidence;
    let spool_bytes = context.spool_bytes;
    let child_work = parent_work.join(format!("child-k{k:03}"));
    fs::create_dir(&child_work).map_err(|cause| {
        config_error(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot create private k={k} work directory: {cause}"),
        )
    })?;
    fs::set_permissions(&child_work, fs::Permissions::from_mode(0o700)).map_err(|cause| {
        config_error(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot set private k={k} work permissions: {cause}"),
        )
    })?;
    let minimizer_length = k.min(config.minimizer_length_ceiling);
    let prior_live_temp = checked_add(
        spool_bytes,
        prior_snapshot_bytes,
        "spool plus prior child snapshots",
    )?;
    enforce_aggregate_temp(
        prior_live_temp,
        config.limits.max_aggregate_temp_bytes,
        "spool plus prior child snapshots",
    )?;
    let aggregate_without_snapshots = config
        .limits
        .max_aggregate_temp_bytes
        .checked_sub(prior_snapshot_bytes)
        .ok_or_else(|| {
            config_error(
                ErrorCode::ResourceTemporaryBytes,
                "prior child snapshots exceed aggregate temporary bytes",
            )
        })?;
    let mut external_limits = config.limits.external_counts;
    // The spool bridge includes the already-live spool in its own temporary
    // accounting. Reserve accumulated snapshots by reducing its ceiling.
    external_limits.max_temp_bytes = external_limits
        .max_temp_bytes
        .min(aggregate_without_snapshots);
    let external_options = SpoolExternalOptions {
        work_dir: child_work.clone(),
        k,
        minimizer_length,
        virtual_bucket_count: config.virtual_bucket_count,
        support_unit: config.support_unit,
        max_fragment_decode_bytes: config.limits.transition_ledger.max_fragment_decode_bytes,
        max_fragment_windows: config.limits.transition_ledger.max_fragment_windows,
        limits: external_limits,
    };
    let raw =
        authenticate_spool_external_counts(spool, &external_options, config.limits.retention)?;
    if raw.source_equivalence().common_source_root() != source_root {
        return Err(config_error(
            ErrorCode::IntegrityArtifact,
            "child raw counts disagree with the common spool source root",
        ));
    }
    let retained =
        retain_spool_authenticated_counts(&raw, config.retention_rule, config.limits.retention)?;
    retained.validate_against_authenticated_raw(&raw)?;
    let retention_report = report_retention(config.retention_rule, &retained)?;
    drop(raw);

    let compacted = compact_retained_counts(&retained, config.limits.compacted_graph)?;
    drop(retained);
    let available_transition_temp = config
        .limits
        .max_aggregate_temp_bytes
        .checked_sub(prior_live_temp)
        .ok_or_else(|| {
            config_error(
                ErrorCode::ResourceTemporaryBytes,
                "spool and prior snapshots leave no transition-run allowance",
            )
        })?;
    if available_transition_temp == 0 {
        return Err(config_error(
            ErrorCode::ResourceTemporaryBytes,
            "spool and prior snapshots leave no transition-run allowance",
        ));
    }
    let mut transition_limits = config.limits.transition_ledger;
    // The transition builder accounts only its private runs, so the parent
    // reserves the spool and snapshots explicitly.
    transition_limits.max_temp_bytes = transition_limits
        .max_temp_bytes
        .min(available_transition_temp);
    let transition_options = TransitionLedgerOptions {
        work_dir: child_work.clone(),
        k,
        limits: transition_limits,
    };
    let transitions = build_transition_ledger(spool, &transition_options)?;
    let witnessed = constrain_authenticated_compacted_child(
        &compacted,
        &transitions,
        config.limits.reconstruction,
    )?;
    witnessed.validate_against_sources(&compacted, &transitions, config.limits.reconstruction)?;
    let mut payload = report_payload(
        retention_report,
        &compacted,
        &transitions,
        &witnessed,
        config.limits.max_child_report_accounted_bytes,
    )?;
    if pair_evidence == PairEvidenceState::UnavailableResourceLimit {
        payload.pair_evidence = ReportPairEvidence::unavailable_resource_limit(
            source_root,
            spool.fragment_count(),
            config.limits.pair_mapper.maximum_fragments,
        )?;
    }
    let pair_adapter =
        if pair_evidence == PairEvidenceState::AuthenticatedLinearUnitigOnlyUnqualified {
            Some(adapt_authenticated_witnessed_child(
                &witnessed,
                &compacted,
                &transitions,
                config.limits.reconstruction,
                config.limits.pair_graph_adapter,
            )?)
        } else {
            None
        };
    // The adapter is an owned, sealed derivative. Original graph capabilities
    // are released before mapper/index/result memory is admitted.
    drop(witnessed);
    drop(transitions);
    drop(compacted);
    if let Some(adapter) = pair_adapter {
        let mapper_config = PairMapperConfig {
            expected_graph_root_sha256: adapter.exact_pair_graph_root_sha256(),
            expected_common_source_root_sha256: adapter.source_root_sha256(),
            expected_pair_mapping_source_root_sha256: pair_mapping_source_root_sha256(spool)?,
            expected_minimum_base_quality: config.min_base_quality,
            seed_length: config.pair_evidence.seed_length,
            worker_threads: 1,
            split: config.pair_evidence.calibration_split,
            limits: config.limits.pair_mapper,
        };
        let placements =
            produce_authenticated_pair_placement_evidence(spool, &adapter, mapper_config)?;
        let path_config = PairPathConfig {
            model: config.pair_evidence.model,
            limits: config.limits.pair_graph_adapter.pair_path,
            minimum_distinct_fragments_for_path: config
                .pair_evidence
                .minimum_distinct_fragments_for_path,
        };
        let paths = analyze_authenticated_pair_paths(&adapter, &placements, path_config, 1)?;
        payload.pair_evidence = report_authenticated_pair_evidence(
            &adapter,
            &placements,
            &paths,
            mapper_config,
            &payload.segments,
            config.limits.bundle.max_pair_document_bytes,
        )?;
        let report_bytes = actual_report_payload_bytes(&payload)?;
        enforce_aggregate_memory(
            report_bytes,
            config.limits.max_child_report_accounted_bytes,
            "materialized child report including paired evidence",
        )?;
    }
    let snapshot_allowance = config
        .limits
        .max_aggregate_temp_bytes
        .checked_sub(prior_live_temp)
        .ok_or_else(|| {
            config_error(
                ErrorCode::ResourceTemporaryBytes,
                "aggregate temporary-byte ceiling leaves no child-snapshot allowance",
            )
        })?
        .min(config.limits.bundle.max_snapshot_bytes);
    if snapshot_allowance == 0 {
        return Err(config_error(
            ErrorCode::ResourceTemporaryBytes,
            "aggregate temporary-byte ceiling leaves no child-snapshot allowance",
        ));
    }
    let snapshot = write_unverified_child_snapshot(
        &child_work,
        &payload,
        config.limits.bundle.max_snapshot_document_bytes,
        snapshot_allowance,
        config.limits.bundle.max_validation_scratch_bytes,
    )?;
    // Complete graph/evidence capabilities and the reporting copy are dropped
    // here before the next k begins; only a bounded private snapshot remains.
    drop(payload);
    Ok(snapshot)
}

fn portfolio_outcome(outcome: MultiKBundleOutcome) -> MultiKPortfolioOutcome {
    MultiKPortfolioOutcome {
        destination: outcome.destination,
        parent_root: outcome.parent_root,
        operational_root: outcome.operational_root,
        manifest_sha256: outcome.manifest_sha256,
        children: outcome.children,
        segments: outcome.segments,
    }
}

const fn pair_evidence_state(
    input: &InputSpec,
    supplied_fragments: u64,
    configured_fragment_limit: u64,
) -> PairEvidenceState {
    match input {
        InputSpec::Single(_) => PairEvidenceState::NotApplicableSingleEnd,
        InputSpec::Paired { .. } if supplied_fragments > configured_fragment_limit => {
            PairEvidenceState::UnavailableResourceLimit
        }
        InputSpec::Paired { .. } => PairEvidenceState::AuthenticatedLinearUnitigOnlyUnqualified,
    }
}

fn report_retention(
    rule: RetentionRule,
    retained: &RetainedCountArtifact,
) -> Result<ReportRetention> {
    if retained.rule() != rule {
        return Err(config_error(
            ErrorCode::IntegrityArtifact,
            "retained capability reports a different rule than the portfolio",
        ));
    }
    let (rule, minimum_support) = match rule {
        RetentionRule::RetainAll => (ReportRetentionRule::RetainAll, None),
        RetentionRule::InclusiveSupport { minimum_support } => {
            (ReportRetentionRule::InclusiveSupport, Some(minimum_support))
        }
    };
    Ok(ReportRetention {
        rule,
        minimum_support,
        raw_keys: retained.raw_key_count(),
        retained_keys: retained.retained_key_count(),
        discarded_keys: retained.discarded_key_count(),
        raw_support: retained.raw_support(),
        retained_support: retained.retained_support(),
        discarded_support: retained.discarded_support(),
        decision_ledger_root: retained.decision_ledger_root(),
        retained_table_root: retained.retained_table_root(),
        retention_root: retained.retention_root(),
    })
}

#[derive(Serialize)]
struct PairGraphDocument {
    adapter_algorithm_id: &'static str,
    adapter_algorithm_version: &'static str,
    source_root_sha256: [u8; 32],
    source_equivalence_root_sha256: [u8; 32],
    compacted_graph_ancestry_root_sha256: [u8; 32],
    transition_root_sha256: [u8; 32],
    witnessed_child_root_sha256: [u8; 32],
    witnessed_child_authentication_root_sha256: [u8; 32],
    exact_pair_graph_root_sha256: [u8; 32],
    authentication_root_sha256: [u8; 32],
    segment_count: u64,
    link_count: u64,
    sequence_bases: u64,
    adapter_owned_bytes: u64,
    admitted_construction_bytes: u64,
    maximum_accounted_bytes: u64,
}

#[derive(Serialize)]
struct PairSegmentIndexDocument {
    segment_index: u32,
    /// Hex in FASTA/GFA is the lowercase encoding of these exact bytes.
    child_segment_id_sha256: [u8; 32],
}

#[derive(Serialize)]
struct AuthenticatedPairDocument<'a> {
    schema: &'static str,
    status: &'static str,
    qualification: &'static str,
    intended_use: &'static str,
    placement_domain: &'static str,
    changes_sequence_or_graph: bool,
    graph: PairGraphDocument,
    segment_index: &'a [PairSegmentIndexDocument],
    mapper_config: PairMapperConfig,
    mapper_conservation: super::pair_mapper::PairMapperConservationTelemetry,
    mapper_execution: super::pair_mapper::PairMapperExecutionTelemetry,
    placement_producer_root_sha256: [u8; 32],
    path_analysis: &'a AuthenticatedPairPathResult,
    limitations: [&'static str; 4],
}

fn report_authenticated_pair_evidence(
    graph: &AuthenticatedPairGraph,
    placements: &PairMapperResult,
    paths: &AuthenticatedPairPathResult,
    mapper_config: PairMapperConfig,
    child_segments: &[ReportSegment],
    maximum_document_bytes: u64,
) -> Result<ReportPairEvidence> {
    validate_authenticated_pair_path_result(paths)?;
    let result = paths.result();
    if maximum_document_bytes == 0
        || graph.source_root_sha256() != placements.common_source_root_sha256()
        || graph.source_equivalence_root_sha256() != placements.source_equivalence_root_sha256()
        || graph.compacted_graph_ancestry_root_sha256()
            != placements.compacted_graph_ancestry_root_sha256()
        || graph.transition_root_sha256() != placements.transition_root_sha256()
        || graph.witnessed_child_root_sha256() != placements.witnessed_child_root_sha256()
        || graph.witnessed_child_authentication_root_sha256()
            != placements.witnessed_child_authentication_root_sha256()
        || graph.exact_pair_graph_root_sha256() != placements.exact_pair_graph_root_sha256()
        || graph.authentication_root_sha256() != placements.authenticated_pair_graph_root_sha256()
        || placements.producer_root_sha256() != paths.placement_producer_result_root_sha256()
        || placements.mapper_identity_sha256() != result.mapper_identity_sha256
        || placements.calibration_placement_root_sha256()
            != result.calibration_placement_root_sha256
        || placements.replay_placement_root_sha256() != result.replay_placement_root_sha256
        || graph.exact_pair_graph_root_sha256() != result.graph_root_sha256
        || placements.pair_mapping_source_root_sha256() != result.library_source_root_sha256
        || result.config
            != (PairPathConfig {
                model: result.config.model,
                limits: graph.limits().pair_path,
                minimum_distinct_fragments_for_path: result
                    .config
                    .minimum_distinct_fragments_for_path,
            })
        || mapper_config.expected_graph_root_sha256 != graph.exact_pair_graph_root_sha256()
        || mapper_config.expected_common_source_root_sha256 != graph.source_root_sha256()
        || mapper_config.expected_pair_mapping_source_root_sha256
            != placements.pair_mapping_source_root_sha256()
        || mapper_config.expected_minimum_base_quality > 93
        || graph.segment_count()
            != u64_from_usize(child_segments.len(), "paired child segment count")?
    {
        return Err(config_error(
            ErrorCode::IntegrityArtifact,
            "authenticated pair capabilities or reporting configuration disagree",
        ));
    }

    let mut ordered_ids = Vec::new();
    ordered_ids
        .try_reserve_exact(child_segments.len())
        .map_err(|cause| memory_error("pair-report segment index", cause))?;
    ordered_ids.extend(child_segments.iter().map(|segment| segment.id));
    ordered_ids.sort_unstable();
    if ordered_ids.windows(2).any(|window| window[0] >= window[1]) {
        return Err(config_error(
            ErrorCode::IntegrityArtifact,
            "pair-report child segment identifiers are not unique",
        ));
    }
    let mut segment_index = Vec::new();
    segment_index
        .try_reserve_exact(ordered_ids.len())
        .map_err(|cause| memory_error("pair-report segment catalog", cause))?;
    for (index, id) in ordered_ids.into_iter().enumerate() {
        segment_index.push(PairSegmentIndexDocument {
            segment_index: u32::try_from(index).map_err(|_| {
                config_error(
                    ErrorCode::ResourceIntegerOverflow,
                    "pair-report segment index exceeds u32",
                )
            })?,
            child_segment_id_sha256: id,
        });
    }

    let graph_document = PairGraphDocument {
        adapter_algorithm_id: super::authenticated_pair_graph::ALGORITHM_ID,
        adapter_algorithm_version: super::authenticated_pair_graph::ALGORITHM_VERSION,
        source_root_sha256: graph.source_root_sha256(),
        source_equivalence_root_sha256: graph.source_equivalence_root_sha256(),
        compacted_graph_ancestry_root_sha256: graph.compacted_graph_ancestry_root_sha256(),
        transition_root_sha256: graph.transition_root_sha256(),
        witnessed_child_root_sha256: graph.witnessed_child_root_sha256(),
        witnessed_child_authentication_root_sha256: graph
            .witnessed_child_authentication_root_sha256(),
        exact_pair_graph_root_sha256: graph.exact_pair_graph_root_sha256(),
        authentication_root_sha256: graph.authentication_root_sha256(),
        segment_count: graph.segment_count(),
        link_count: graph.link_count(),
        sequence_bases: graph.sequence_bases(),
        adapter_owned_bytes: graph.adapter_owned_bytes(),
        admitted_construction_bytes: graph.admitted_construction_bytes(),
        maximum_accounted_bytes: graph.limits().maximum_accounted_bytes,
    };
    let document = AuthenticatedPairDocument {
        schema: "veritasm-experimental-authenticated-pair-evidence-v1",
        status: "experimental",
        qualification: "unqualified",
        intended_use: "research_use_only",
        placement_domain: "linear_unitig_only",
        changes_sequence_or_graph: false,
        graph: graph_document,
        segment_index: &segment_index,
        mapper_config,
        mapper_conservation: placements.conservation(),
        mapper_execution: placements.execution(),
        placement_producer_root_sha256: placements.producer_root_sha256(),
        path_analysis: paths,
        limitations: [
            "exact_placements_are_complete_only_within_linear_unitig_targets",
            "unmapped_reads_may_span_graph_junctions_and_are_explicitly_unavailable",
            "available_paths_are_annotations_not_sequence_or_graph_edits",
            "local_path_evidence_does_not_establish_global_haplotype_phase",
        ],
    };
    let authenticated_document_json = bounded_json(&document, maximum_document_bytes)?;

    let mut lane_models = Vec::new();
    lane_models
        .try_reserve_exact(result.models.lanes.len())
        .map_err(|cause| memory_error("pair-report lane models", cause))?;
    for lane in &result.models.lanes {
        lane_models.push(ReportPairLaneModel {
            lane_ordinal: lane.lane_ordinal,
            availability: pair_model_availability(lane.availability).to_owned(),
            calibration_included_anchors: lane.ledger.included,
            dominant_orientation: lane
                .dominant_orientation
                .map(pair_library_orientation)
                .map(str::to_owned),
            outer_span_p10: lane.outer_span.map(|distribution| distribution.p10),
            outer_span_p90: lane.outer_span.map(|distribution| distribution.p90),
            inner_gap_p10: lane.inner_gap.map(|distribution| distribution.p10),
            inner_gap_p90: lane.inner_gap.map(|distribution| distribution.p90),
        });
    }
    let available_aggregate_constraints = result
        .aggregate_paths
        .iter()
        .filter(|path| {
            path.availability == AggregatePathAvailability::AvailableForExperimentalConstraint
                && path.available_for_experimental_constraint
        })
        .try_fold(0_u64, |count, _| {
            checked_add(count, 1, "pair available-constraint count overflow")
        })?;
    let conservation = placements.conservation();
    let summary = result.summary;
    ReportPairEvidence::from_payload(ReportPairEvidencePayload {
        state: PairEvidenceState::AuthenticatedLinearUnitigOnlyUnqualified,
        placement_domain: "linear_unitig_only".to_owned(),
        source_root: graph.source_root_sha256(),
        pair_mapping_source_root: placements.pair_mapping_source_root_sha256(),
        source_equivalence_root: graph.source_equivalence_root_sha256(),
        compacted_graph_ancestry_root: graph.compacted_graph_ancestry_root_sha256(),
        transition_root: graph.transition_root_sha256(),
        witnessed_child_root: graph.witnessed_child_root_sha256(),
        witnessed_child_authentication_root: graph.witnessed_child_authentication_root_sha256(),
        exact_pair_graph_root: graph.exact_pair_graph_root_sha256(),
        authenticated_pair_graph_root: graph.authentication_root_sha256(),
        mapper_identity_root: placements.mapper_identity_sha256(),
        calibration_placement_root: placements.calibration_placement_root_sha256(),
        replay_placement_root: placements.replay_placement_root_sha256(),
        placement_producer_root: placements.producer_root_sha256(),
        pair_path_result_root: result.result_root_sha256,
        authenticated_pair_path_result_root: paths.authenticated_result_root_sha256(),
        graph_segments: graph.segment_count(),
        graph_links: graph.link_count(),
        graph_sequence_bases: graph.sequence_bases(),
        supplied_fragments: conservation.authenticated_fragments,
        configured_fragment_limit: mapper_config.limits.maximum_fragments,
        authenticated_fragments: conservation.authenticated_fragments,
        authenticated_reads: conservation.authenticated_reads,
        calibration_fragments: conservation.calibration_fragments,
        replay_fragments: conservation.replay_fragments,
        unavailable_ineligible_reads: conservation.unavailable.ineligible_ambiguity_or_quality,
        unavailable_candidate_limit_reads: conservation.unavailable.indeterminate_candidate_limit,
        unavailable_possible_graph_junction_reads: conservation
            .unavailable
            .unmapped_possible_graph_junction,
        placement_groups: conservation.placement_groups,
        placements: conservation.placements,
        supported_unique_existing_paths: summary.supported_unique_existing_path,
        trivial_within_unitig: summary.trivial_within_unitig,
        abstained: summary.abstained,
        indeterminate: summary.indeterminate,
        available_aggregate_constraints,
        decision_rows: u64_from_usize(result.decisions.len(), "pair decision rows")?,
        aggregate_path_rows: u64_from_usize(
            result.aggregate_paths.len(),
            "pair aggregate-path rows",
        )?,
        lane_models,
        authenticated_document_json,
    })
}

fn pair_model_availability(value: LaneModelAvailability) -> &'static str {
    match value {
        LaneModelAvailability::Available => "available",
        LaneModelAvailability::InsufficientAnchors => "insufficient_anchors",
        LaneModelAvailability::NoUniqueDominantOrientation => "no_unique_dominant_orientation",
        LaneModelAvailability::InsufficientDominantAnchors => "insufficient_dominant_anchors",
        LaneModelAvailability::DominanceBelowThreshold => "dominance_below_threshold",
        LaneModelAvailability::SpanIntervalTooWide => "span_interval_too_wide",
        LaneModelAvailability::InnerIntervalTooWide => "inner_interval_too_wide",
    }
}

fn pair_library_orientation(value: LibraryOrientation) -> &'static str {
    match value {
        LibraryOrientation::Fr => "FR",
        LibraryOrientation::Rf => "RF",
        LibraryOrientation::Ff => "FF",
        LibraryOrientation::Rr => "RR",
    }
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    maximum: u64,
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let new_length = self
            .bytes
            .len()
            .checked_add(input.len())
            .ok_or_else(|| io::Error::other("pair JSON length overflow"))?;
        if u64::try_from(new_length).map_err(|_| io::Error::other("pair JSON length overflow"))?
            > self.maximum
        {
            return Err(io::Error::other("pair JSON byte limit exceeded"));
        }
        self.bytes
            .try_reserve(input.len())
            .map_err(|_| io::Error::other("cannot reserve pair JSON bytes"))?;
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bounded_json(value: &impl Serialize, maximum: u64) -> Result<String> {
    let mut writer = BoundedJsonWriter {
        bytes: Vec::new(),
        maximum,
    };
    serde_json::to_writer(&mut writer, value).map_err(|cause| {
        config_error(
            ErrorCode::ResourceOutputBytes,
            format!("cannot render bounded authenticated pair document: {cause}"),
        )
    })?;
    String::from_utf8(writer.bytes).map_err(|cause| {
        config_error(
            ErrorCode::InternalInvariant,
            format!("authenticated pair JSON is not UTF-8: {cause}"),
        )
    })
}

fn report_payload(
    retention: ReportRetention,
    compacted: &AuthenticatedCompactedGraph,
    transitions: &TransitionLedger,
    witnessed: &AuthenticatedWitnessedChild,
    max_accounted_bytes: u64,
) -> Result<UnverifiedChildSnapshotPayload> {
    let view = witnessed.view();
    if view.source_equivalence() != ReconstructionSourceEquivalence::AuthenticatedSpoolDescriptor
        || view.source_root() != compacted.source_root()
        || view.transition_root() != transitions.transition_root()
        || view.graph_ancestry_root() != compacted.ancestry_root()
        || view.k() != compacted.k()
        || view.k() != transitions.k()
    {
        return Err(config_error(
            ErrorCode::IntegrityArtifact,
            "report conversion received incompatible authenticated capabilities",
        ));
    }
    let projected = projected_report_payload_bytes(&view, transitions)?;
    enforce_aggregate_memory(
        projected,
        max_accounted_bytes,
        "one-child report-copy owned payload",
    )?;
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(view.segments().len())
        .map_err(|cause| memory_error("report segments", cause))?;
    for segment in view.segments() {
        let mut sequence = Vec::new();
        sequence
            .try_reserve_exact(segment.sequence.len())
            .map_err(|cause| memory_error("report segment sequence", cause))?;
        sequence.extend_from_slice(&segment.sequence);
        let mut steps = Vec::new();
        steps
            .try_reserve_exact(segment.steps.len())
            .map_err(|cause| memory_error("report exact-edge steps", cause))?;
        for step in &segment.steps {
            steps.push(ReportEdgeStep {
                canonical_kmer: step.key.to_be_bytes(),
                orientation: match step.orientation {
                    EdgeOrientation::Canonical => ReportEdgeOrientation::Canonical,
                    EdgeOrientation::ReverseComplement => ReportEdgeOrientation::ReverseComplement,
                    EdgeOrientation::SelfReverseComplement => {
                        ReportEdgeOrientation::SelfReverseComplement
                    }
                },
                support: step.support,
            });
        }
        segments.push(ReportSegment {
            id: segment.id.0,
            parent_unitig_id: segment.parent_unitig_id.0,
            parent_start_step: segment.parent_start_step,
            parent_end_step_exclusive: segment.parent_end_step_exclusive,
            topology: match segment.topology {
                CompactedTopology::Linear => ReportTopology::Linear,
                CompactedTopology::ClosedWalk => ReportTopology::ClosedWalk,
            },
            sequence,
            edge_steps: u64_from_usize(segment.steps.len(), "report segment edge steps")?,
            steps,
            total_edge_support: segment.total_edge_support,
            minimum_edge_support: segment.minimum_edge_support,
            lower_median_edge_support: segment.lower_median_edge_support,
            maximum_edge_support: segment.maximum_edge_support,
            internal_transition_rows: segment.internal_transition_rows,
            sum_accepted_window_occurrences: segment.sum_accepted_window_occurrences,
            sum_distinct_supplied_fragment_instances: segment
                .sum_distinct_supplied_fragment_instances,
            ordered_transition_rows_sha256: segment.ordered_transition_rows_sha256,
            provenance_sha256: segment.provenance_sha256,
        });
    }
    segments.sort_by(report_segment_order);

    let mut links = Vec::new();
    links
        .try_reserve_exact(view.links().len())
        .map_err(|cause| memory_error("report links", cause))?;
    for link in view.links() {
        links.push(ReportLink {
            from: link.from.0,
            from_orientation: report_orientation(link.from_orientation),
            to: link.to.0,
            to_orientation: report_orientation(link.to_orientation),
            overlap_bases: link.overlap_bases,
            canonical_qmer: link.canonical_qmer.to_be_bytes(),
            evidence: report_evidence(link.evidence)?,
        });
    }
    links.sort_by(report_link_order);

    let mut adjacency_rows = Vec::new();
    adjacency_rows
        .try_reserve_exact(transitions.rows().len())
        .map_err(|cause| memory_error("report adjacency rows", cause))?;
    for row in transitions.rows() {
        adjacency_rows.push(ReportAdjacency {
            canonical_qmer: row.canonical_qmer.to_be_bytes(),
            evidence: ReportTransitionEvidence {
                accepted_window_occurrences: row.accepted_window_occurrences,
                distinct_supplied_fragment_instances: row.distinct_supplied_fragment_instances,
                sorted_event_frames_sha256: row.sorted_event_frames_sha256,
            },
        });
    }

    let mut transition_decisions = Vec::new();
    transition_decisions
        .try_reserve_exact(view.decisions().len())
        .map_err(|cause| memory_error("report transition decisions", cause))?;
    for decision in view.decisions() {
        transition_decisions.push(ReportTransitionDecision {
            canonical_qmer: decision.canonical_qmer.to_be_bytes(),
            origin: report_decision_origin(decision.origin),
            status: report_decision_status(decision.status).to_owned(),
            evidence: decision.evidence.map(report_evidence).transpose()?,
        });
    }
    transition_decisions.sort_by(|left, right| {
        left.canonical_qmer
            .cmp(&right.canonical_qmer)
            .then_with(|| left.origin.cmp(&right.origin))
    });

    let conservation = view.conservation();
    let payload = UnverifiedChildSnapshotPayload {
        schema: "veritasm-experimental-multik-child-snapshot-v2".to_owned(),
        k: view.k(),
        q: view.q(),
        minimizer_length: view.minimizer_length(),
        virtual_bucket_count: view.virtual_bucket_count(),
        support_unit: wide_support_unit(view.support_unit()).to_owned(),
        source_root: view.source_root(),
        source_equivalence_root: compacted.source_equivalence_root(),
        retention,
        transition_root: view.transition_root(),
        exact_edge_table_sha256: view.exact_edge_table_sha256(),
        raw_compacted_graph_root: view.raw_compacted_graph_root(),
        constrained_graph_root: view.constrained_graph_root(),
        child_root: view.child_root(),
        child_authentication_root: view.authentication_root(),
        segments,
        links,
        adjacency_rows,
        transition_decisions,
        conservation: ReportConservation {
            input_canonical_edges: conservation.input_canonical_edges,
            represented_canonical_edges: conservation.represented_canonical_edges,
            input_edge_support: conservation.input_edge_support,
            represented_edge_support: conservation.represented_edge_support,
            topology_candidates: conservation.topology_candidates,
            admitted_topology_candidates: conservation.admitted_topology_candidates,
            excluded_no_original_read_witness: conservation.excluded_no_original_read_witness,
            ledger_rows: conservation.ledger_rows,
            eligible_ledger_rows: conservation.eligible_ledger_rows,
            excluded_endpoint_not_retained: conservation.excluded_endpoint_not_retained,
            segments: conservation.segments,
            linear_segments: conservation.linear_segments,
            closed_segments: conservation.closed_segments,
            witnessed_links: conservation.witnessed_links,
            output_bases: conservation.output_bases,
            accounted_peak_bytes: conservation.accounted_peak_bytes,
        },
        pair_evidence: ReportPairEvidence::not_applicable_single_end(view.source_root())?,
    };
    let actual = actual_report_payload_bytes(&payload)?;
    enforce_aggregate_memory(
        actual,
        max_accounted_bytes,
        "materialized one-child report-copy owned payload",
    )?;
    Ok(payload)
}

fn report_evidence(evidence: OriginalReadTransitionEvidence) -> Result<ReportTransitionEvidence> {
    if evidence.kind != AdjacencyEvidenceKind::OriginalReadTransition {
        return Err(config_error(
            ErrorCode::IntegrityArtifact,
            "unsupported evidence kind reached the authenticated report boundary",
        ));
    }
    Ok(ReportTransitionEvidence {
        accepted_window_occurrences: evidence.accepted_window_occurrences,
        distinct_supplied_fragment_instances: evidence.distinct_supplied_fragment_instances,
        sorted_event_frames_sha256: evidence.sorted_event_frames_sha256,
    })
}

fn report_orientation(orientation: UnitigOrientation) -> ReportOrientation {
    match orientation {
        UnitigOrientation::Forward => ReportOrientation::Forward,
        UnitigOrientation::ReverseComplement => ReportOrientation::ReverseComplement,
    }
}

fn report_decision_status(status: TransitionDecisionStatus) -> &'static str {
    match status {
        TransitionDecisionStatus::AdmittedOriginalRead => "admitted_original_read",
        TransitionDecisionStatus::ExcludedEndpointNotRetained => "excluded_endpoint_not_retained",
        TransitionDecisionStatus::ExcludedNoOriginalReadWitness => {
            "excluded_no_original_read_witness"
        }
    }
}

fn report_decision_origin(origin: TransitionCandidateOrigin) -> String {
    match origin {
        TransitionCandidateOrigin::UnitigInterior {
            parent_unitig_id,
            left_step,
        } => format!(
            "unitig_interior:{}:{left_step}",
            hex_bytes(parent_unitig_id.0)
        ),
        TransitionCandidateOrigin::ClosedWalkClosure { parent_unitig_id } => {
            format!("closed_walk_closure:{}", hex_bytes(parent_unitig_id.0))
        }
        TransitionCandidateOrigin::RawCompactedLink(link) => format!(
            "raw_compacted_link:{}:{}:{}:{}:{}",
            hex_bytes(link.from.0),
            orientation_name(link.from_orientation),
            hex_bytes(link.to.0),
            orientation_name(link.to_orientation),
            link.overlap_bases,
        ),
        TransitionCandidateOrigin::LedgerRetainedEndpointsNoRawCandidate => {
            "ledger_retained_endpoints_no_raw_candidate".to_owned()
        }
        TransitionCandidateOrigin::LedgerEndpointNotRetained => {
            "ledger_endpoint_not_retained".to_owned()
        }
    }
}

fn orientation_name(orientation: UnitigOrientation) -> &'static str {
    match orientation {
        UnitigOrientation::Forward => "forward",
        UnitigOrientation::ReverseComplement => "reverse_complement",
    }
}

fn wide_support_unit(unit: WideRunSupportUnit) -> &'static str {
    match unit {
        WideRunSupportUnit::SuppliedFragmentInstance => "supplied_fragment_instance",
        WideRunSupportUnit::AcceptedWindowOccurrence => "accepted_window_occurrence",
    }
}

fn report_segment_order(left: &ReportSegment, right: &ReportSegment) -> std::cmp::Ordering {
    right
        .sequence
        .len()
        .cmp(&left.sequence.len())
        .then_with(|| left.sequence.cmp(&right.sequence))
        .then_with(|| left.id.cmp(&right.id))
}

fn report_link_order(left: &ReportLink, right: &ReportLink) -> std::cmp::Ordering {
    left.from
        .cmp(&right.from)
        .then_with(|| left.from_orientation.cmp(&right.from_orientation))
        .then_with(|| left.to.cmp(&right.to))
        .then_with(|| left.to_orientation.cmp(&right.to_orientation))
        .then_with(|| left.canonical_qmer.cmp(&right.canonical_qmer))
}

fn hex_bytes(bytes: [u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn spool_scientific_config(config: &MultiKPortfolioConfig) -> Result<ScientificConfig> {
    let first_k = config.ks.first().copied().ok_or_else(|| {
        config_error(
            ErrorCode::ConfigurationInvalidK,
            "multi-k portfolio requires at least one k",
        )
    })?;
    let (profile, minimum) = match config.retention_rule {
        RetentionRule::RetainAll => (Profile::RetainAll, Some(1)),
        RetentionRule::InclusiveSupport { minimum_support } => {
            (Profile::Custom, Some(minimum_support))
        }
    };
    ScientificConfig::resolve(
        first_k,
        profile,
        config.support_unit,
        minimum,
        config.min_base_quality,
        false,
    )
}

fn validate_pair_configuration(
    config: MultiKPairEvidenceConfig,
    limits: &MultiKResourceLimits,
) -> Result<()> {
    let model = config.model;
    let work = limits.pair_graph_adapter.pair_path;
    let mapper = limits.pair_mapper;
    if !(1..=31).contains(&config.seed_length)
        || config.calibration_split.numerator == 0
        || config.calibration_split.numerator >= config.calibration_split.denominator
        || model.minimum_anchors < 10
        || model.minimum_dominant_anchors < 10
        || model.dominance_denominator == 0
        || model.dominance_numerator > model.dominance_denominator
        || u128::from(model.dominance_numerator) * 2 <= u128::from(model.dominance_denominator)
        || config.minimum_distinct_fragments_for_path < 2
        || config.minimum_distinct_fragments_for_path > work.maximum_pairs
        || work.maximum_pairs == 0
        || work.maximum_placements == 0
        || work.maximum_placement_pairs_per_fragment == 0
        || work.maximum_edges_per_path == 0
        || work.maximum_search_states_per_fragment == 0
        || work.maximum_search_arc_examinations_per_fragment == 0
        || work.maximum_path_reconstruction_elements_per_fragment == 0
        || work.maximum_target_paths_per_fragment == 0
        || work.maximum_compatible_paths_per_fragment == 0
        || work.maximum_worker_threads == 0
        || work.maximum_graph_sequence_bases == 0
        || work.graph_memory_bytes < 4_096
        || work.search_memory_bytes_per_worker == 0
        || work.result_memory_bytes == 0
        || work.analysis_memory_bytes == 0
        || mapper.maximum_worker_threads < 1
    {
        return Err(config_error(
            ErrorCode::ConfigurationInvalidLimit,
            "paired-evidence scientific parameters or work limits are invalid",
        ));
    }
    let path_storage = checked_add(
        256,
        checked_mul(
            u64::from(work.maximum_edges_per_path),
            32,
            "pair path-storage admission overflow",
        )?,
        "pair path-storage admission overflow",
    )?;
    let required_search = checked_add(
        checked_mul(
            work.maximum_search_states_per_fragment,
            128,
            "pair search-state admission overflow",
        )?,
        checked_add(
            checked_mul(path_storage, 2, "pair path scratch admission overflow")?,
            checked_mul(
                work.maximum_compatible_paths_per_fragment,
                path_storage,
                "pair compatible-path scratch admission overflow",
            )?,
            "pair search scratch admission overflow",
        )?,
        "pair search admission overflow",
    )?;
    let model_and_input_index = checked_add(
        checked_mul(
            work.maximum_placements,
            256,
            "pair placement-index admission overflow",
        )?,
        checked_mul(work.maximum_pairs, 384, "pair model admission overflow")?,
        "pair model/index admission overflow",
    )?;
    let per_pair = checked_add(
        checked_add(
            512,
            checked_mul(
                work.maximum_placement_pairs_per_fragment,
                128,
                "pair endpoint-domain admission overflow",
            )?,
            "pair result admission overflow",
        )?,
        checked_mul(
            checked_mul(
                work.maximum_compatible_paths_per_fragment,
                path_storage,
                "pair compatible-path result admission overflow",
            )?,
            3,
            "pair compatible-path result admission overflow",
        )?,
        "pair result admission overflow",
    )?;
    let required_results = checked_add(
        checked_mul(
            work.maximum_pairs,
            per_pair,
            "pair result admission overflow",
        )?,
        model_and_input_index,
        "pair combined result admission overflow",
    )?;
    let stack_bytes = u64::try_from(crate::config::WORKER_STACK_BYTES).map_err(|_| {
        config_error(
            ErrorCode::ResourceIntegerOverflow,
            "pair worker-stack size does not fit u64",
        )
    })?;
    let required_analysis = checked_sum(
        &[
            work.graph_memory_bytes,
            work.result_memory_bytes,
            work.search_memory_bytes_per_worker,
            stack_bytes,
        ],
        "pair analysis admission overflow",
    )?;
    let mapper_worst_batch = projected_pair_mapper_worst_batch(mapper, stack_bytes)?;
    let required_mapper_reads = mapper.maximum_fragments.checked_mul(2).ok_or_else(|| {
        config_error(
            ErrorCode::ResourceIntegerOverflow,
            "paired fragment limit overflows required-read bound",
        )
    })?;
    if required_search > work.search_memory_bytes_per_worker
        || required_results > work.result_memory_bytes
        || required_analysis > work.analysis_memory_bytes
        || mapper_worst_batch > mapper.total_accounted_memory_bytes
        || work.maximum_pairs > mapper.maximum_fragments
        || work.maximum_placements > mapper.maximum_placements
        || mapper.maximum_reads < required_mapper_reads
        || mapper.maximum_batch_fragments > mapper.maximum_fragments
        || limits.pair_graph_adapter.maximum_sequence_bases > work.maximum_graph_sequence_bases
    {
        return Err(config_error(
            ErrorCode::ConfigurationInvalidLimit,
            "paired-evidence component limits do not close under their declared work bounds",
        ));
    }
    Ok(())
}

fn projected_pair_mapper_worst_batch(mapper: PairMapperLimits, stack_bytes: u64) -> Result<u64> {
    let mapper_fixed = checked_sum(
        &[
            mapper.mapper_index_memory_bytes,
            mapper.mapper_query_memory_bytes_per_worker,
            stack_bytes,
        ],
        "pair mapper fixed admission overflow",
    )?;
    let mapper_per_fragment_output = checked_add(
        512,
        checked_mul(
            checked_mul(
                mapper.maximum_mapping_candidates_per_read,
                2,
                "pair mapper candidate admission overflow",
            )?,
            128,
            "pair mapper candidate admission overflow",
        )?,
        "pair mapper per-fragment admission overflow",
    )?;
    checked_sum(
        &[
            mapper_fixed,
            mapper.maximum_decoded_batch_bytes,
            checked_mul(
                mapper.maximum_batch_fragments,
                mapper_per_fragment_output,
                "pair mapper batch-result admission overflow",
            )?,
            checked_mul(
                mapper.maximum_batch_fragments,
                64,
                "pair mapper batch-header admission overflow",
            )?,
            mapper.result_memory_bytes,
            1 << 20,
        ],
        "pair mapper worst-batch admission overflow",
    )
}

fn validate_resource_limits(limits: &MultiKResourceLimits, paired: bool) -> Result<()> {
    if limits.max_aggregate_accounted_memory_bytes == 0
        || limits.max_child_report_accounted_bytes == 0
        || limits.max_aggregate_temp_bytes == 0
    {
        return Err(invalid_limit("aggregate"));
    }
    let external = limits.external_counts;
    if external.max_segments == 0
        || external.max_input_bases == 0
        || external.max_windows == 0
        || external.max_distinct_kmers == 0
        || external.max_memory_bytes == 0
        || external.sort_buffer_bytes < 128
        || external.io_buffer_bytes == 0
        || external.io_buffer_bytes > 64 << 20
        || external.max_temp_bytes == 0
        || external.max_run_files == 0
        || external.merge_fan_in < 2
        || external.max_open_files < 3
        || external
            .merge_fan_in
            .checked_add(1)
            .is_none_or(|required| required > external.max_open_files)
    {
        return Err(invalid_limit("external-count"));
    }
    let retention = limits.retention;
    if retention.max_raw_keys == 0
        || retention.max_retained_keys == 0
        || retention.max_accounted_bytes == 0
    {
        return Err(invalid_limit("retention"));
    }
    let graph = limits.compacted_graph;
    if graph.max_canonical_edges == 0
        || graph.max_oriented_handles == 0
        || graph.max_literal_nodes == 0
        || graph.max_unitigs == 0
        || graph.max_link_candidates == 0
        || graph.max_output_bases == 0
        || graph.max_accounted_bytes == 0
    {
        return Err(invalid_limit("compacted-graph"));
    }
    let transition = limits.transition_ledger;
    if transition.max_events == 0
        || transition.max_rows == 0
        || transition.max_fragment_decode_bytes == 0
        || transition.max_fragment_windows == 0
        || transition.max_memory_bytes == 0
        || transition.sort_buffer_bytes < 128
        || transition.max_temp_bytes == 0
        || transition.max_run_files == 0
        || transition.merge_fan_in < 2
        || transition.max_open_files < transition.merge_fan_in.saturating_add(1).max(2)
    {
        return Err(invalid_limit("transition-ledger"));
    }
    let reconstruction = limits.reconstruction;
    if reconstruction.max_input_edges == 0
        || reconstruction.max_topology_candidates == 0
        || reconstruction.max_decision_rows == 0
        || reconstruction.max_segments == 0
        || reconstruction.max_links == 0
        || reconstruction.max_output_bases == 0
        || reconstruction.max_accounted_bytes == 0
        || reconstruction.max_verification_scratch_bytes == 0
    {
        return Err(invalid_limit("reconstruction"));
    }
    if paired {
        let adapter = limits.pair_graph_adapter;
        let mapper = limits.pair_mapper;
        if adapter.maximum_segments == 0
            || adapter.maximum_links == 0
            || adapter.maximum_sequence_bases == 0
            || adapter.maximum_accounted_bytes == 0
            || mapper.maximum_fragments == 0
            || mapper.maximum_reads < 2
            || mapper.maximum_bases == 0
            || mapper.maximum_graph_sequence_bases == 0
            || mapper.maximum_mapping_candidates_per_read == 0
            || mapper.maximum_admitted_mapping_operations == 0
            || mapper.maximum_placement_groups == 0
            || mapper.maximum_placements == 0
            || mapper.maximum_batch_fragments == 0
            || mapper.maximum_decoded_batch_bytes == 0
            || mapper.mapper_index_memory_bytes == 0
            || mapper.mapper_query_memory_bytes_per_worker == 0
            || mapper.result_memory_bytes == 0
            || mapper.total_accounted_memory_bytes == 0
            || mapper.maximum_worker_threads == 0
            || mapper.maximum_mapper_parameter_bytes == 0
        {
            return Err(invalid_limit("paired-evidence"));
        }
    }
    let bundle = limits.bundle;
    if bundle.max_children == 0
        || bundle.max_segments == 0
        || bundle.max_links == 0
        || bundle.max_adjacency_rows == 0
        || bundle.max_transition_decisions == 0
        || bundle.max_pair_decisions == 0
        || bundle.max_pair_aggregate_paths == 0
        || bundle.max_pair_lanes == 0
        || bundle.max_pair_document_bytes == 0
        || bundle.max_output_bases == 0
        || bundle.max_snapshot_document_bytes == 0
        || bundle.max_snapshot_bytes == 0
        || bundle.max_loaded_snapshot_bytes == 0
        || bundle.max_loaded_report_accounted_bytes == 0
        || bundle.max_presentation_accounted_bytes == 0
        || bundle.max_validation_scratch_bytes == 0
        || bundle.max_staged_output_bytes == 0
        || bundle.max_manifest_bytes == 0
    {
        return Err(invalid_limit("bundle"));
    }
    if limits.stable_input_and_spool.max_temp_bytes > limits.max_aggregate_temp_bytes {
        return Err(config_error(
            ErrorCode::ConfigurationInvalidLimit,
            "stable spool construction temporary limit exceeds the aggregate temporary ceiling",
        ));
    }
    let maximum_phase = aggregate_memory_projection(limits, paired)?;
    enforce_aggregate_memory(
        maximum_phase,
        limits.max_aggregate_accounted_memory_bytes,
        "declared multi-k phase overlap",
    )?;
    Ok(())
}

fn aggregate_memory_projection(limits: &MultiKResourceLimits, paired: bool) -> Result<u64> {
    let external = limits.external_counts;
    let retention = limits.retention;
    let graph = limits.compacted_graph;
    let transition = limits.transition_ledger;
    let reconstruction = limits.reconstruction;
    let bundle = limits.bundle;
    let graph_transition_reconstruction_report = checked_sum(
        &[
            graph.max_accounted_bytes,
            transition.max_memory_bytes,
            reconstruction.max_accounted_bytes,
            reconstruction.max_verification_scratch_bytes,
            limits.max_child_report_accounted_bytes,
            bundle.max_validation_scratch_bytes,
        ],
        "aggregate graph/transition/reconstruction/report phase",
    )?;
    let descriptor_decode_scratch = checked_add(
        bundle.max_snapshot_bytes,
        SNAPSHOT_CODEC_MEMORY_BYTES,
        "aggregate descriptor-bound decode scratch",
    )?;
    let final_reporting_scratch = bundle
        .max_presentation_accounted_bytes
        .max(bundle.max_validation_scratch_bytes)
        .max(descriptor_decode_scratch);
    let final_reporting = checked_add(
        bundle.max_loaded_report_accounted_bytes,
        final_reporting_scratch,
        "aggregate final-reporting phase",
    )?;
    let snapshot_serialization = checked_add(
        limits.max_child_report_accounted_bytes,
        bundle
            .max_validation_scratch_bytes
            .max(SNAPSHOT_CODEC_MEMORY_BYTES),
        "aggregate compressed child-snapshot serialization phase",
    )?;
    let mut maximum = limits
        .stable_input_and_spool
        .memory_budget_bytes
        .max(external.max_memory_bytes)
        .max(checked_add(
            external.max_memory_bytes,
            retention.max_accounted_bytes,
            "aggregate raw-plus-retention phase",
        )?)
        .max(checked_add(
            retention.max_accounted_bytes,
            graph.max_accounted_bytes,
            "aggregate retention-plus-compaction phase",
        )?)
        .max(checked_add(
            graph.max_accounted_bytes,
            transition.max_memory_bytes,
            "aggregate graph-plus-transition phase",
        )?)
        .max(graph_transition_reconstruction_report)
        .max(snapshot_serialization)
        .max(final_reporting);
    if paired {
        let pair_adapter = limits.pair_graph_adapter.maximum_accounted_bytes;
        let pair_graph_construction = checked_sum(
            &[
                graph.max_accounted_bytes,
                transition.max_memory_bytes,
                reconstruction.max_accounted_bytes,
                reconstruction.max_verification_scratch_bytes,
                limits.max_child_report_accounted_bytes,
                pair_adapter,
            ],
            "aggregate authenticated pair-graph construction phase",
        )?;
        let pair_mapping = checked_sum(
            &[
                limits.max_child_report_accounted_bytes,
                pair_adapter,
                limits.pair_mapper.total_accounted_memory_bytes,
            ],
            "aggregate paired placement phase",
        )?;
        let pair_analysis_and_report = checked_sum(
            &[
                limits.max_child_report_accounted_bytes,
                pair_adapter,
                limits.pair_mapper.result_memory_bytes,
                limits.pair_graph_adapter.pair_path.analysis_memory_bytes,
            ],
            "aggregate paired path-analysis and reporting phase",
        )?;
        maximum = maximum
            .max(pair_graph_construction)
            .max(pair_mapping)
            .max(pair_analysis_and_report);
    }
    Ok(maximum)
}

fn projected_report_payload_bytes(
    view: &AuthenticatedWitnessedChildView<'_>,
    transitions: &TransitionLedger,
) -> Result<u64> {
    // `try_reserve_exact` is permitted to over-allocate. A factor of two is
    // admitted before allocation, and the actual capacities are checked after
    // construction. The fixed margin covers headers and short strings.
    let mut bytes = 16_384_u64;
    bytes = checked_add(
        bytes,
        checked_mul(
            u64_from_usize(view.segments().len(), "report segment count")?,
            size_of::<ReportSegment>() as u64,
            "projected report segment headers",
        )?,
        "projected report bytes",
    )?;
    for segment in view.segments() {
        bytes = checked_add(
            bytes,
            u64_from_usize(segment.sequence.len(), "report segment bases")?,
            "projected report sequence bytes",
        )?;
        bytes = checked_add(
            bytes,
            checked_mul(
                u64_from_usize(segment.steps.len(), "report edge-step count")?,
                size_of::<ReportEdgeStep>() as u64,
                "projected report edge-step bytes",
            )?,
            "projected report bytes",
        )?;
    }
    for (count, width, label) in [
        (
            view.links().len(),
            size_of::<ReportLink>(),
            "projected report links",
        ),
        (
            transitions.rows().len(),
            size_of::<ReportAdjacency>(),
            "projected report adjacency rows",
        ),
        (
            view.decisions().len(),
            size_of::<ReportTransitionDecision>() + 256,
            "projected report decisions and strings",
        ),
    ] {
        bytes = checked_add(
            bytes,
            checked_mul(u64_from_usize(count, label)?, width as u64, label)?,
            "projected report bytes",
        )?;
    }
    checked_mul(bytes, 2, "projected report allocation allowance")
}

fn actual_report_payload_bytes(payload: &UnverifiedChildSnapshotPayload) -> Result<u64> {
    let mut bytes = checked_add(
        16_384,
        checked_mul(
            u64_from_usize(payload.segments.capacity(), "report segment capacity")?,
            size_of::<ReportSegment>() as u64,
            "report segment allocation bytes",
        )?,
        "materialized report bytes",
    )?;
    for segment in &payload.segments {
        bytes = checked_add(
            bytes,
            u64_from_usize(segment.sequence.capacity(), "report sequence capacity")?,
            "materialized report sequence bytes",
        )?;
        bytes = checked_add(
            bytes,
            checked_mul(
                u64_from_usize(segment.steps.capacity(), "report step capacity")?,
                size_of::<ReportEdgeStep>() as u64,
                "report step allocation bytes",
            )?,
            "materialized report bytes",
        )?;
    }
    bytes = checked_add(
        bytes,
        checked_mul(
            u64_from_usize(payload.links.capacity(), "report link capacity")?,
            size_of::<ReportLink>() as u64,
            "report link allocation bytes",
        )?,
        "materialized report bytes",
    )?;
    bytes = checked_add(
        bytes,
        checked_mul(
            u64_from_usize(
                payload.adjacency_rows.capacity(),
                "report adjacency capacity",
            )?,
            size_of::<ReportAdjacency>() as u64,
            "report adjacency allocation bytes",
        )?,
        "materialized report bytes",
    )?;
    bytes = checked_add(
        bytes,
        checked_mul(
            u64_from_usize(
                payload.transition_decisions.capacity(),
                "report decision capacity",
            )?,
            size_of::<ReportTransitionDecision>() as u64,
            "report decision allocation bytes",
        )?,
        "materialized report bytes",
    )?;
    for decision in &payload.transition_decisions {
        bytes = checked_add(
            bytes,
            u64_from_usize(decision.origin.capacity(), "report origin capacity")?,
            "materialized report origin bytes",
        )?;
        bytes = checked_add(
            bytes,
            u64_from_usize(decision.status.capacity(), "report status capacity")?,
            "materialized report status bytes",
        )?;
    }
    Ok(bytes)
}

fn enforce_aggregate_memory(observed: u64, maximum: u64, label: &'static str) -> Result<()> {
    if observed > maximum {
        return Err(config_error(
            ErrorCode::ResourceMemory,
            format!("{label} requires {observed} accounted bytes, exceeding limit {maximum}"),
        ));
    }
    Ok(())
}

fn enforce_aggregate_temp(observed: u64, maximum: u64, label: &'static str) -> Result<()> {
    if observed > maximum {
        return Err(config_error(
            ErrorCode::ResourceTemporaryBytes,
            format!("{label} requires {observed} temporary bytes, exceeding limit {maximum}"),
        ));
    }
    Ok(())
}

fn checked_sum(values: &[u64], context: &'static str) -> Result<u64> {
    values
        .iter()
        .try_fold(0_u64, |sum, value| checked_add(sum, *value, context))
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| config_error(ErrorCode::ResourceIntegerOverflow, context))
}

fn checked_mul(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_mul(right)
        .ok_or_else(|| config_error(ErrorCode::ResourceIntegerOverflow, context))
}

fn invalid_limit(component: &'static str) -> VeritasmError {
    config_error(
        ErrorCode::ConfigurationInvalidLimit,
        format!("invalid experimental {component} resource envelope"),
    )
}

fn u64_from_usize(value: usize, label: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        config_error(
            ErrorCode::ResourceIntegerOverflow,
            format!("{label} does not fit u64"),
        )
    })
}

fn memory_error(label: &'static str, cause: std::collections::TryReserveError) -> VeritasmError {
    config_error(
        ErrorCode::ResourceMemory,
        format!("cannot reserve {label}: {cause}"),
    )
}

fn config_error(code: ErrorCode, context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(code, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn config(input: PathBuf, output_dir: PathBuf) -> MultiKPortfolioConfig {
        MultiKPortfolioConfig {
            input: InputSpec::Single(vec![input]),
            output_dir,
            ks: vec![3, 5],
            support_unit: SupportUnit::SuppliedFragmentInstance,
            retention_rule: RetentionRule::RetainAll,
            min_base_quality: 0,
            profile: PortfolioProfile::DiversityPreserving,
            worker_threads: 1,
            minimizer_length_ceiling: 3,
            virtual_bucket_count: 8,
            pair_evidence: MultiKPairEvidenceConfig::default(),
            limits: MultiKResourceLimits::default(),
        }
    }

    fn read_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(base: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let mut entries = fs::read_dir(path)
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let file_type = entry.file_type().unwrap();
                if file_type.is_dir() {
                    visit(base, &entry.path(), output);
                } else if file_type.is_file() {
                    output.insert(
                        entry.path().strip_prefix(base).unwrap().to_path_buf(),
                        fs::read(entry.path()).unwrap(),
                    );
                } else {
                    panic!("unexpected non-regular test bundle entry");
                }
            }
        }
        let mut output = BTreeMap::new();
        visit(root, root, &mut output);
        output
    }

    #[test]
    fn malformed_config_is_rejected_before_input_io() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("missing.fastq");
        let mut candidate = config(missing.clone(), parent.path().join("result-a"));
        candidate.ks = vec![5, 3];
        assert_eq!(
            candidate.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidK
        );

        let mut candidate = config(missing.clone(), parent.path().join("result-b"));
        candidate.retention_rule = RetentionRule::InclusiveSupport { minimum_support: 0 };
        assert_eq!(
            candidate.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidSupport
        );

        let mut candidate = config(missing.clone(), parent.path().join("result-c"));
        candidate.worker_threads = 4;
        assert_eq!(
            candidate.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );

        let mut candidate = config(missing, parent.path().join("result-d"));
        candidate.limits.bundle.max_children = 1;
        assert_eq!(
            candidate.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
    }

    #[test]
    fn aggregate_memory_projection_has_exact_boundaries() {
        let mut limits = MultiKResourceLimits::default();
        let required = aggregate_memory_projection(&limits, false).unwrap();
        limits.max_aggregate_accounted_memory_bytes = required - 1;
        assert_eq!(
            validate_resource_limits(&limits, false).unwrap_err().code(),
            ErrorCode::ResourceMemory
        );
        limits.max_aggregate_accounted_memory_bytes = required;
        validate_resource_limits(&limits, false).unwrap();
        limits.max_aggregate_accounted_memory_bytes = required + 1;
        validate_resource_limits(&limits, false).unwrap();
    }

    #[test]
    fn paired_preflight_has_exact_mapper_and_aggregate_boundaries_before_input_io() {
        let stack_bytes = u64::try_from(crate::config::WORKER_STACK_BYTES).unwrap();
        let mut limits = MultiKResourceLimits::default();
        let required_mapper =
            projected_pair_mapper_worst_batch(limits.pair_mapper, stack_bytes).unwrap();
        limits.pair_mapper.total_accounted_memory_bytes = required_mapper - 1;
        assert_eq!(
            validate_pair_configuration(MultiKPairEvidenceConfig::default(), &limits)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        limits.pair_mapper.total_accounted_memory_bytes = required_mapper;
        validate_pair_configuration(MultiKPairEvidenceConfig::default(), &limits).unwrap();
        limits.pair_mapper.total_accounted_memory_bytes = required_mapper + 1;
        validate_pair_configuration(MultiKPairEvidenceConfig::default(), &limits).unwrap();

        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("result");
        let mut candidate = config(parent.path().join("missing-r1.fastq"), output.clone());
        candidate.input = InputSpec::Paired {
            read1: vec![parent.path().join("missing-r1.fastq")],
            read2: vec![parent.path().join("missing-r2.fastq")],
        };
        candidate.limits = limits;
        let required_aggregate = aggregate_memory_projection(&candidate.limits, true).unwrap();
        candidate.limits.max_aggregate_accounted_memory_bytes = required_aggregate - 1;
        assert_eq!(
            run_authenticated_multik_portfolio(&candidate)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
        assert!(!output.exists());

        candidate.limits.max_aggregate_accounted_memory_bytes = required_aggregate;
        assert_eq!(
            run_authenticated_multik_portfolio(&candidate)
                .unwrap_err()
                .code(),
            ErrorCode::InputOpen
        );
        assert!(!output.exists());
    }

    #[test]
    fn pair_resource_availability_is_explicit_at_8192_8193_boundary() {
        let paired = InputSpec::Paired {
            read1: vec![PathBuf::from("r1.fastq")],
            read2: vec![PathBuf::from("r2.fastq")],
        };
        assert_eq!(
            pair_evidence_state(&paired, 8_192, 8_192),
            PairEvidenceState::AuthenticatedLinearUnitigOnlyUnqualified
        );
        assert_eq!(
            pair_evidence_state(&paired, 8_193, 8_192),
            PairEvidenceState::UnavailableResourceLimit
        );
        assert_eq!(
            pair_evidence_state(
                &InputSpec::Single(vec![PathBuf::from("reads.fastq")]),
                u64::MAX,
                2
            ),
            PairEvidenceState::NotApplicableSingleEnd
        );
    }

    #[test]
    fn aggregate_temp_check_has_exact_boundaries() {
        assert!(enforce_aggregate_temp(9, 10, "test").is_ok());
        assert!(enforce_aggregate_temp(10, 10, "test").is_ok());
        assert_eq!(
            enforce_aggregate_temp(11, 10, "test").unwrap_err().code(),
            ErrorCode::ResourceTemporaryBytes
        );
    }

    #[test]
    fn destination_lease_precedes_missing_input_and_preserves_existing_data() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("existing");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"preserve\n").unwrap();
        let candidate = config(parent.path().join("missing.fastq"), destination.clone());
        assert_eq!(
            run_authenticated_multik_portfolio(&candidate)
                .unwrap_err()
                .code(),
            ErrorCode::DestinationExisting
        );
        assert_eq!(
            fs::read(destination.join("sentinel")).unwrap(),
            b"preserve\n"
        );
    }

    #[test]
    fn input_failure_does_not_publish_output() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let candidate = config(parent.path().join("missing.fastq"), destination.clone());
        assert_eq!(
            run_authenticated_multik_portfolio(&candidate)
                .unwrap_err()
                .code(),
            ErrorCode::InputOpen
        );
        assert!(!destination.exists());
    }

    #[test]
    fn real_portfolio_is_byte_deterministic_and_explicitly_serial() {
        use sha2::{Digest, Sha256};

        let parent = tempfile::tempdir().unwrap();
        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/reads.fasta");
        let mut results = Vec::new();
        for ordinal in 0..3 {
            let output = parent.path().join(format!("result-{ordinal}"));
            let mut candidate = config(input.clone(), output.clone());
            candidate.minimizer_length_ceiling = 15;
            candidate.virtual_bucket_count = 256;
            candidate.limits.external_counts.max_distinct_kmers = 10_000;
            candidate.limits.retention.max_raw_keys = 10_000;
            candidate.limits.retention.max_retained_keys = 10_000;
            candidate.limits.compacted_graph.max_canonical_edges = 10_000;
            candidate.limits.reconstruction.max_input_edges = 10_000;
            if ordinal == 2 {
                // An operational envelope must change its own identity while
                // leaving the scientific evidence and sequences unchanged.
                candidate.limits.max_aggregate_accounted_memory_bytes += 1;
            }
            let outcome = run_authenticated_multik_portfolio(&candidate).unwrap();
            let manifest_digest: [u8; 32] =
                Sha256::digest(fs::read(output.join("manifest.sha256")).unwrap()).into();
            assert_eq!(outcome.manifest_sha256, manifest_digest);
            let run: serde_json::Value =
                serde_json::from_slice(&fs::read(output.join("run.json")).unwrap()).unwrap();
            assert_eq!(run["execution"]["execution_threads"], 1);
            assert_eq!(
                run["execution"]["accounting_scope"],
                "modelled_owned_payload_not_process_rss"
            );
            assert_eq!(run["pair_evidence"], "not_applicable_single_end");
            assert_eq!(run["quality_correction"], "disabled_unqualified");
            results.push((
                outcome.parent_root,
                outcome.operational_root,
                outcome.manifest_sha256,
                read_tree(&output),
            ));
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(
            hex_bytes(results[0].0),
            "d2116a292eed6855b87e1984d45e7cb69c0a7f1a59d1e108474894636295d25b"
        );
        // Keep the scientific golden above, but do not freeze implementation
        // resource defaults into an unrelated scientific regression. Complete
        // repeated-run bytes are checked above. Here we test the separation
        // between scientific and operational identity explicitly.
        assert_eq!(results[0].0, results[2].0);
        assert_ne!(results[0].1, results[2].1);
        assert_ne!(results[0].2, results[2].2);
        let scientific_files = |tree: &BTreeMap<PathBuf, Vec<u8>>| {
            tree.iter()
                .filter(|(path, _)| {
                    !matches!(
                        path.to_str(),
                        Some("run.json" | "report.html" | "manifest.sha256")
                    )
                })
                .map(|(path, bytes)| (path.clone(), bytes.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(
            scientific_files(&results[0].3),
            scientific_files(&results[2].3)
        );
    }
}
