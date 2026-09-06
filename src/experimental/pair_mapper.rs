//! Authenticated exact linear-unitig placement production for `pair_path`.
//!
//! This module is experimental and deliberately absent from the stable CLI. It consumes every
//! fragment from one authenticated paired spool, enumerates exact full-read placements on the
//! immutable linear-unitig target universe, and separates complete fragment records into disjoint
//! calibration and replay inputs. Public source-backed use is possible only through the opaque
//! `authenticated_pair_graph` adapter. It does not traverse graph links, alter topology, or scaffold.

use crate::audit::{PlacementGroup, ReadState};
use crate::config::WORKER_STACK_BYTES;
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::experimental::authenticated_pair_graph::PairGraphAncestry;
use crate::experimental::pair_path::{
    authenticated_evidence_pair_from_pair_mapper, mapper_identity_sha256,
    placement_input_root_sha256, AuthenticatedPlacementEvidencePair,
    CompleteEnumerationCertificate, Direction, ExactPlacement, ExactPlacementGroup,
    MapperProvenance, PairPathGraph, PairPlacementEvidence, PlacementDomain,
    PlacementEvidenceInput, WorkLimits,
};
use crate::experimental::transition_witness::transition_source_root;
use crate::indexed_mapper::{
    IndexedExactMapper, IndexedMapperConfig, IndexedMapperPlan, MappingWork, ALGORITHM_ID,
    ALGORITHM_VERSION, TARGET_UNIVERSE,
};
use crate::model::{Fragment, MateRole, ReadRecord, Topology, Unitig};
use crate::spool::{MemoryBoundedNext, Spool};
use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::mem::size_of;

pub const PRODUCER_ALGORITHM_ID: &str = "authenticated_spool_exact_pair_mapper";
pub const PRODUCER_ALGORITHM_VERSION: &str = "experimental-2";
pub const SPLIT_ALGORITHM_VERSION: &str = "content_hash_threshold_v1";

const FIXED_MEMORY_ALLOWANCE: u64 = 1 << 20;
const PER_ALLOCATION_ALLOWANCE: u64 = 64;
const OUTPUT_GROUP_BOUND_BYTES: u64 = 128;
const OUTPUT_PAIR_BOUND_BYTES: u64 = 512;
const MAX_WORKER_THREADS: u16 = 1_024;
const MAX_SEED_LENGTH: usize = 31;
const MAX_MAPPING_CANDIDATES: u64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CalibrationSplitRule {
    pub salt: [u8; 32],
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PairMapperLimits {
    pub maximum_fragments: u64,
    pub maximum_reads: u64,
    pub maximum_bases: u64,
    pub maximum_graph_sequence_bases: u64,
    pub maximum_mapping_candidates_per_read: u64,
    pub maximum_admitted_mapping_operations: u64,
    pub maximum_placement_groups: u64,
    pub maximum_placements: u64,
    pub maximum_batch_fragments: u64,
    pub maximum_decoded_batch_bytes: u64,
    pub mapper_index_memory_bytes: u64,
    pub mapper_query_memory_bytes_per_worker: u64,
    pub result_memory_bytes: u64,
    pub total_accounted_memory_bytes: u64,
    pub maximum_worker_threads: u16,
    pub maximum_mapper_parameter_bytes: u64,
}

impl Default for PairMapperLimits {
    fn default() -> Self {
        Self {
            maximum_fragments: 1_000_000,
            maximum_reads: 2_000_000,
            maximum_bases: 100_000_000_000,
            maximum_graph_sequence_bases: 100_000_000_000,
            maximum_mapping_candidates_per_read: 10_000,
            maximum_admitted_mapping_operations: 1_000_000_000_000,
            maximum_placement_groups: 10_000_000,
            maximum_placements: 10_000_000,
            maximum_batch_fragments: 4_096,
            maximum_decoded_batch_bytes: 64 << 20,
            mapper_index_memory_bytes: 512 << 20,
            mapper_query_memory_bytes_per_worker: 64 << 20,
            result_memory_bytes: 1 << 30,
            total_accounted_memory_bytes: 4 << 30,
            maximum_worker_threads: 64,
            maximum_mapper_parameter_bytes: 4 << 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PairMapperConfig {
    pub expected_graph_root_sha256: [u8; 32],
    pub expected_common_source_root_sha256: [u8; 32],
    pub expected_pair_mapping_source_root_sha256: [u8; 32],
    pub expected_minimum_base_quality: u8,
    pub seed_length: usize,
    pub worker_threads: u16,
    pub split: CalibrationSplitRule,
    pub limits: PairMapperLimits,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct UnavailableReasonCounts {
    pub ineligible_ambiguity_or_quality: u64,
    pub indeterminate_candidate_limit: u64,
    pub unmapped_possible_graph_junction: u64,
}

impl UnavailableReasonCounts {
    pub fn total(self) -> Result<u64> {
        self.ineligible_ambiguity_or_quality
            .checked_add(self.indeterminate_candidate_limit)
            .and_then(|value| value.checked_add(self.unmapped_possible_graph_junction))
            .ok_or_else(|| overflow("pair-mapper unavailable-reason total overflow"))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PairMapperConservationTelemetry {
    pub authenticated_fragments: u64,
    pub authenticated_reads: u64,
    pub authenticated_bases: u64,
    pub calibration_fragments: u64,
    pub replay_fragments: u64,
    pub exact_single_group_reads: u64,
    pub exact_multiple_group_reads: u64,
    pub unavailable: UnavailableReasonCounts,
    pub placement_groups: u64,
    pub placements: u64,
    pub admitted_mapping_operations: u64,
    pub observed_mapping_work: MappingWorkTelemetry,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MappingWorkTelemetry {
    pub seed_lookups: u64,
    pub selected_seed_hits: u64,
    pub postings_examined: u64,
    pub indexed_full_verifications: u64,
    pub fallback_full_verifications: u64,
    pub verified_placement_groups_seen: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PairMapperExecutionTelemetry {
    pub worker_threads: u16,
    pub batches: u64,
    pub maximum_observed_batch_fragments: u64,
    pub maximum_observed_decoded_batch_bytes: u64,
    pub maximum_observed_result_bytes: u64,
    pub maximum_accounted_peak_bytes: u64,
    pub mapper_index_accounted_bytes: u64,
}

/// Opaque placement result bound to the authenticated spool and complete
/// witnessed-child-to-pair-graph ancestry.
#[derive(Debug)]
pub struct PairMapperResult {
    common_source_root_sha256: [u8; 32],
    pair_mapping_source_root_sha256: [u8; 32],
    source_equivalence_root_sha256: [u8; 32],
    compacted_graph_ancestry_root_sha256: [u8; 32],
    transition_root_sha256: [u8; 32],
    witnessed_child_root_sha256: [u8; 32],
    witnessed_child_authentication_root_sha256: [u8; 32],
    exact_pair_graph_root_sha256: [u8; 32],
    authenticated_pair_graph_root_sha256: [u8; 32],
    mapper_identity_sha256: [u8; 32],
    calibration_placement_root_sha256: [u8; 32],
    replay_placement_root_sha256: [u8; 32],
    evidence_pair: AuthenticatedPlacementEvidencePair,
    conservation: PairMapperConservationTelemetry,
    execution: PairMapperExecutionTelemetry,
}

impl PairMapperResult {
    pub const fn common_source_root_sha256(&self) -> [u8; 32] {
        self.common_source_root_sha256
    }

    pub const fn pair_mapping_source_root_sha256(&self) -> [u8; 32] {
        self.pair_mapping_source_root_sha256
    }

    pub const fn source_equivalence_root_sha256(&self) -> [u8; 32] {
        self.source_equivalence_root_sha256
    }

    pub const fn compacted_graph_ancestry_root_sha256(&self) -> [u8; 32] {
        self.compacted_graph_ancestry_root_sha256
    }

    pub const fn transition_root_sha256(&self) -> [u8; 32] {
        self.transition_root_sha256
    }

    pub const fn witnessed_child_root_sha256(&self) -> [u8; 32] {
        self.witnessed_child_root_sha256
    }

    pub const fn witnessed_child_authentication_root_sha256(&self) -> [u8; 32] {
        self.witnessed_child_authentication_root_sha256
    }

    pub const fn exact_pair_graph_root_sha256(&self) -> [u8; 32] {
        self.exact_pair_graph_root_sha256
    }

    pub const fn authenticated_pair_graph_root_sha256(&self) -> [u8; 32] {
        self.authenticated_pair_graph_root_sha256
    }

    pub const fn mapper_identity_sha256(&self) -> [u8; 32] {
        self.mapper_identity_sha256
    }

    pub const fn calibration_placement_root_sha256(&self) -> [u8; 32] {
        self.calibration_placement_root_sha256
    }

    pub const fn replay_placement_root_sha256(&self) -> [u8; 32] {
        self.replay_placement_root_sha256
    }

    pub const fn producer_root_sha256(&self) -> [u8; 32] {
        self.evidence_pair.producer_result_root_sha256()
    }

    pub(super) const fn authenticated_evidence_pair(&self) -> &AuthenticatedPlacementEvidencePair {
        &self.evidence_pair
    }

    pub const fn calibration_input(&self) -> &PlacementEvidenceInput {
        self.evidence_pair.calibration_input()
    }

    pub const fn replay_input(&self) -> &PlacementEvidenceInput {
        self.evidence_pair.replay_input()
    }

    pub const fn conservation(&self) -> PairMapperConservationTelemetry {
        self.conservation
    }

    pub const fn execution(&self) -> PairMapperExecutionTelemetry {
        self.execution
    }
}

#[derive(Debug)]
struct PendingFragment {
    fragment: Fragment,
}

#[derive(Debug)]
struct MappedFragment {
    pair: PairPlacementEvidence,
    calibration: bool,
    telemetry: FragmentTelemetry,
}

#[derive(Debug, Clone, Copy, Default)]
struct FragmentTelemetry {
    exact_single: u64,
    exact_multiple: u64,
    unavailable: UnavailableReasonCounts,
    groups: u64,
    placements: u64,
    mapping_work: MappingWorkTelemetry,
}

#[derive(Debug, Default)]
struct LaneReplayState {
    lane_index: usize,
    observed_in_lane: u64,
}

#[derive(Debug)]
struct ReadMapping {
    digest: [u8; 32],
    groups: Vec<ExactPlacementGroup>,
    unavailable: Option<UnavailableReason>,
    state: ReadState,
    work: MappingWorkTelemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnavailableReason {
    IneligibleAmbiguityOrQuality,
    IndeterminateCandidateLimit,
    UnmappedPossibleGraphJunction,
}

/// Canonical identity of the immutable spool registration and every source descriptor.
///
/// This helper does not replace `Spool::verify`; production verifies and exhausts the same opened
/// descriptor before returning evidence.
pub fn pair_mapping_source_root_sha256(spool: &Spool) -> Result<[u8; 32]> {
    let whole = decode_hex_32(&spool.sha256, "whole spool digest")?;
    let pretrailer = decode_hex_32(&spool.pretrailer_sha256, "spool pretrailer digest")?;
    let scientific = spool.scientific_config();
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-mapper-spool-source:v1\0");
    hasher.update(spool.schema_version().to_le_bytes());
    hasher.update(whole);
    hasher.update(pretrailer);
    hasher.update(spool.fragment_count.to_le_bytes());
    hasher.update(spool.read_count.to_le_bytes());
    hasher.update([spool.input_mode_tag()]);
    hasher.update([scientific.k]);
    hash_length_prefixed(&mut hasher, scientific.profile.as_str().as_bytes())?;
    hasher.update([scientific.support_unit.tag()]);
    hasher.update(scientific.min_support.to_le_bytes());
    hasher.update([scientific.min_base_quality]);
    hasher.update([u8::from(scientific.remap)]);
    hasher.update(usize_to_u64(spool.sources.len(), "spool source count")?.to_le_bytes());
    for source in &spool.sources {
        hasher.update(source.lane_ordinal.to_le_bytes());
        hasher.update([source.role.tag(), source.format.tag()]);
        hasher.update(decode_hex_32(
            &source.raw_transport_sha256,
            "source raw-transport digest",
        )?);
        hasher.update(decode_hex_32(
            &source.logical_decoded_sha256,
            "source logical-decoded digest",
        )?);
        hasher.update(source.records.to_le_bytes());
        hasher.update(source.bases.to_le_bytes());
    }
    Ok(hasher.finalize().into())
}

/// Produce exact pair placements after a crate-sealed pair-graph adapter has
/// authenticated the complete graph ancestry.
///
/// This raw boundary is crate-private. Public source-backed production is
/// exposed only by `authenticated_pair_graph::produce_authenticated_pair_placement_evidence`.
pub(super) fn produce_pair_placement_evidence_with_ancestry(
    spool: &Spool,
    graph: &PairPathGraph,
    unitigs: &[Unitig],
    config: PairMapperConfig,
    graph_ancestry: &PairGraphAncestry,
) -> Result<PairMapperResult> {
    validate_config(config)?;
    validate_paired_source_catalog(spool)?;
    let common_source_root = transition_source_root(spool)?;
    if common_source_root != config.expected_common_source_root_sha256 {
        return Err(integrity(
            "pair-mapper expected common source root mismatch",
        ));
    }
    let pair_mapping_source_root = pair_mapping_source_root_sha256(spool)?;
    if pair_mapping_source_root != config.expected_pair_mapping_source_root_sha256 {
        return Err(integrity(
            "pair-mapper expected pair-mapping source root mismatch",
        ));
    }
    if graph.graph_root_sha256() != config.expected_graph_root_sha256 {
        return Err(integrity("pair-mapper expected graph root mismatch"));
    }
    graph_ancestry.validate(common_source_root, graph.graph_root_sha256())?;
    if spool.scientific_config().min_base_quality != config.expected_minimum_base_quality {
        return Err(integrity(
            "pair-mapper expected minimum base quality mismatches the authenticated spool",
        ));
    }
    validate_graph_targets(graph, unitigs, config.limits)?;
    validate_registered_cardinalities(spool, config.limits)?;

    let mapper_plan = IndexedExactMapper::plan(unitigs, config.seed_length)?;
    if mapper_plan.resident_bound_bytes() > config.limits.mapper_index_memory_bytes {
        return Err(resource("pair-mapper index plan exceeds its memory budget"));
    }
    let mapper = IndexedExactMapper::build_planned(
        unitigs,
        IndexedMapperConfig {
            seed_length: config.seed_length,
            index_memory_budget_bytes: config.limits.mapper_index_memory_bytes,
            placement_memory_budget_bytes: config.limits.mapper_query_memory_bytes_per_worker,
        },
        mapper_plan,
    )?;
    let mapper_provenance = mapper_provenance(spool, config)?;
    let compatibility_limits = compatibility_limits(config.limits)?;
    let mapper_identity = mapper_identity_sha256(&mapper_provenance, compatibility_limits)?;

    let (mut batch, mut calibration_pairs, mut replay_pairs, initial_peak) =
        preallocate_producer_vectors(config, mapper.accounted_resident_bytes())?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(usize::from(config.worker_threads))
        .stack_size(WORKER_STACK_BYTES)
        .build()
        .map_err(|_| resource("cannot construct pair-mapper worker pool"))?;
    let mut iterator = spool.iter()?;
    let mut conservation = PairMapperConservationTelemetry::default();
    let mut execution = PairMapperExecutionTelemetry {
        worker_threads: config.worker_threads,
        mapper_index_accounted_bytes: mapper.accounted_resident_bytes(),
        maximum_accounted_peak_bytes: initial_peak,
        ..PairMapperExecutionTelemetry::default()
    };
    let mut batch_decoded_bytes = 0u64;
    let mut lane_replay = LaneReplayState::default();

    loop {
        if usize_to_u64(batch.len(), "pair-mapper batch fragment count")?
            == config.limits.maximum_batch_fragments
        {
            process_batch(
                &pool,
                &mapper,
                graph,
                common_source_root,
                pair_mapping_source_root,
                mapper_identity,
                config,
                mapper_plan,
                &mut batch,
                &mut batch_decoded_bytes,
                &mut calibration_pairs,
                &mut replay_pairs,
                &mut conservation,
                &mut execution,
            )?;
        }
        let available = config
            .limits
            .maximum_decoded_batch_bytes
            .checked_sub(batch_decoded_bytes)
            .ok_or_else(|| overflow("pair-mapper decoded-batch accounting underflow"))?;
        match iterator.next_with_decode_memory_limit(available)? {
            MemoryBoundedNext::Fragment {
                fragment,
                memory_bytes,
            } => {
                validate_fragment_shape(&fragment)?;
                admit_fragment_cardinality(
                    &fragment,
                    spool,
                    config.limits,
                    &mut conservation,
                    &mut lane_replay,
                )?;
                batch_decoded_bytes = checked_add(
                    batch_decoded_bytes,
                    memory_bytes,
                    "pair-mapper decoded-batch byte overflow",
                )?;
                if batch.len() == batch.capacity() {
                    return Err(invariant(
                        "pair-mapper preallocated batch exhausted before configured limit",
                    ));
                }
                batch.push(PendingFragment { fragment });
            }
            MemoryBoundedNext::RequiresMemory(required) => {
                if batch.is_empty() {
                    return Err(resource(
                        "one paired spool fragment exceeds the decoded-batch memory budget",
                    ));
                }
                process_batch(
                    &pool,
                    &mapper,
                    graph,
                    common_source_root,
                    pair_mapping_source_root,
                    mapper_identity,
                    config,
                    mapper_plan,
                    &mut batch,
                    &mut batch_decoded_bytes,
                    &mut calibration_pairs,
                    &mut replay_pairs,
                    &mut conservation,
                    &mut execution,
                )?;
                if required > config.limits.maximum_decoded_batch_bytes {
                    return Err(resource(
                        "one paired spool fragment exceeds the decoded-batch memory budget",
                    ));
                }
            }
            MemoryBoundedNext::End => {
                process_batch(
                    &pool,
                    &mapper,
                    graph,
                    common_source_root,
                    pair_mapping_source_root,
                    mapper_identity,
                    config,
                    mapper_plan,
                    &mut batch,
                    &mut batch_decoded_bytes,
                    &mut calibration_pairs,
                    &mut replay_pairs,
                    &mut conservation,
                    &mut execution,
                )?;
                if !iterator.is_authenticated() {
                    return Err(integrity(
                        "pair-mapper spool replay reached end without terminal authentication",
                    ));
                }
                validate_lane_replay_complete(spool, &lane_replay)?;
                break;
            }
        }
    }

    calibration_pairs.sort_unstable_by(pair_order);
    replay_pairs.sort_unstable_by(pair_order);
    validate_final_conservation(
        spool,
        &calibration_pairs,
        &replay_pairs,
        conservation,
        config.limits,
    )?;

    let calibration_read_root = subset_read_set_root(
        b"calibration",
        common_source_root,
        pair_mapping_source_root,
        graph.graph_root_sha256(),
        mapper_identity,
        config.split,
        &calibration_pairs,
    )?;
    let replay_read_root = subset_read_set_root(
        b"replay",
        common_source_root,
        pair_mapping_source_root,
        graph.graph_root_sha256(),
        mapper_identity,
        config.split,
        &replay_pairs,
    )?;
    let calibration = placement_input(
        graph,
        pair_mapping_source_root,
        calibration_read_root,
        mapper_identity,
        fallible_clone_mapper(&mapper_provenance)?,
        calibration_pairs,
    )?;
    let replay = placement_input(
        graph,
        pair_mapping_source_root,
        replay_read_root,
        mapper_identity,
        mapper_provenance,
        replay_pairs,
    )?;
    let result_bytes_before_validation = placement_result_bytes(
        &calibration.pairs,
        calibration.pairs.capacity(),
        &replay.pairs,
        replay.pairs.capacity(),
    )?;
    let validation_scratch = placement_validation_scratch_bound(&calibration.pairs)?
        .max(placement_validation_scratch_bound(&replay.pairs)?)
        .max(calibration_reuse_scratch_bound(
            calibration.pairs.len(),
            replay.pairs.len(),
        )?);
    let validation_peak = [
        mapper.accounted_resident_bytes(),
        checked_mul(
            u64::from(config.worker_threads),
            usize_to_u64(WORKER_STACK_BYTES, "pair-mapper worker stack bytes")?,
            "pair-mapper validation stack reservation overflow",
        )?,
        result_bytes_before_validation,
        checked_mul(
            usize_to_u64(batch.capacity(), "pair-mapper retained batch capacity")?,
            usize_to_u64(size_of::<PendingFragment>(), "pending-fragment size")?,
            "pair-mapper retained batch bytes overflow",
        )?,
        validation_scratch,
        FIXED_MEMORY_ALLOWANCE,
    ]
    .into_iter()
    .try_fold(0u64, |sum, value| {
        checked_add(sum, value, "pair-mapper validation peak overflow")
    })?;
    if validation_peak > config.limits.total_accounted_memory_bytes {
        return Err(resource(
            "pair-mapper compatibility validation exceeds total memory budget",
        ));
    }
    execution.maximum_accounted_peak_bytes =
        execution.maximum_accounted_peak_bytes.max(validation_peak);
    let calibration_root = placement_input_root_sha256(graph, &calibration, compatibility_limits)?;
    let replay_root = placement_input_root_sha256(graph, &replay, compatibility_limits)?;
    let producer_root = producer_root_sha256(
        graph.graph_root_sha256(),
        common_source_root,
        pair_mapping_source_root,
        graph_ancestry,
        mapper_identity,
        config.split,
        calibration_read_root,
        replay_read_root,
        calibration_root,
        replay_root,
        conservation,
    )?;
    let result_bytes = placement_result_bytes(
        &calibration.pairs,
        calibration.pairs.capacity(),
        &replay.pairs,
        replay.pairs.capacity(),
    )?;
    if result_bytes > config.limits.result_memory_bytes {
        return Err(resource(
            "pair-mapper final result exceeds its memory budget",
        ));
    }
    execution.maximum_observed_result_bytes =
        execution.maximum_observed_result_bytes.max(result_bytes);
    let evidence_pair =
        authenticated_evidence_pair_from_pair_mapper(calibration, replay, producer_root)?;
    Ok(PairMapperResult {
        common_source_root_sha256: common_source_root,
        pair_mapping_source_root_sha256: pair_mapping_source_root,
        source_equivalence_root_sha256: graph_ancestry.source_equivalence_root_sha256(),
        compacted_graph_ancestry_root_sha256: graph_ancestry.compacted_graph_ancestry_root_sha256(),
        transition_root_sha256: graph_ancestry.transition_root_sha256(),
        witnessed_child_root_sha256: graph_ancestry.witnessed_child_root_sha256(),
        witnessed_child_authentication_root_sha256: graph_ancestry
            .witnessed_child_authentication_root_sha256(),
        exact_pair_graph_root_sha256: graph_ancestry.pair_graph_root_sha256(),
        authenticated_pair_graph_root_sha256: graph_ancestry.authentication_root_sha256(),
        mapper_identity_sha256: mapper_identity,
        calibration_placement_root_sha256: calibration_root,
        replay_placement_root_sha256: replay_root,
        evidence_pair,
        conservation,
        execution,
    })
}

#[cfg(test)]
fn produce_pair_placement_evidence(
    spool: &Spool,
    graph: &PairPathGraph,
    unitigs: &[Unitig],
    config: PairMapperConfig,
) -> Result<PairMapperResult> {
    let source_root = transition_source_root(spool)?;
    let ancestry =
        PairGraphAncestry::for_unverified_mapper_test(source_root, graph.graph_root_sha256())?;
    produce_pair_placement_evidence_with_ancestry(spool, graph, unitigs, config, &ancestry)
}

type ProducerVectors = (
    Vec<PendingFragment>,
    Vec<PairPlacementEvidence>,
    Vec<PairPlacementEvidence>,
    u64,
);

fn preallocate_producer_vectors(
    config: PairMapperConfig,
    mapper_index_bytes: u64,
) -> Result<ProducerVectors> {
    let batch_slots = usize::try_from(config.limits.maximum_batch_fragments)
        .map_err(|_| resource("pair-mapper batch limit does not fit address space"))?;
    let result_slots = usize::try_from(config.limits.maximum_fragments)
        .map_err(|_| resource("pair-mapper fragment limit does not fit address space"))?;
    let pending_size = usize_to_u64(size_of::<PendingFragment>(), "pending-fragment size")?;
    let pair_size = usize_to_u64(
        size_of::<PairPlacementEvidence>(),
        "pair placement evidence size",
    )?;
    let requested_batch_bytes = checked_mul(
        config.limits.maximum_batch_fragments,
        pending_size,
        "pair-mapper requested batch-vector bytes overflow",
    )?;
    let requested_result_bytes = checked_mul(
        checked_mul(
            config.limits.maximum_fragments,
            2,
            "pair-mapper requested result slots overflow",
        )?,
        pair_size,
        "pair-mapper requested result-vector bytes overflow",
    )?;
    let requested_result_bytes = checked_add(
        requested_result_bytes,
        checked_mul(
            2,
            PER_ALLOCATION_ALLOWANCE,
            "pair-mapper result-vector allowance overflow",
        )?,
        "pair-mapper requested result-vector bytes overflow",
    )?;
    if requested_result_bytes > config.limits.result_memory_bytes {
        return Err(resource(
            "pair-mapper preallocated result vectors exceed result memory budget",
        ));
    }
    let worker_bytes = checked_mul(
        u64::from(config.worker_threads),
        checked_add(
            config.limits.mapper_query_memory_bytes_per_worker,
            usize_to_u64(WORKER_STACK_BYTES, "pair-mapper worker stack bytes")?,
            "pair-mapper preallocation per-worker bytes overflow",
        )?,
        "pair-mapper preallocation worker bytes overflow",
    )?;
    let requested_peak = [
        mapper_index_bytes,
        worker_bytes,
        requested_batch_bytes,
        requested_result_bytes,
        FIXED_MEMORY_ALLOWANCE,
    ]
    .into_iter()
    .try_fold(0u64, |sum, value| {
        checked_add(sum, value, "pair-mapper preallocation peak overflow")
    })?;
    if requested_peak > config.limits.total_accounted_memory_bytes {
        return Err(resource(
            "pair-mapper preallocated vectors exceed total memory budget",
        ));
    }

    let mut batch = Vec::new();
    batch
        .try_reserve_exact(batch_slots)
        .map_err(|_| resource("cannot preallocate pair-mapper decoded batch"))?;
    let mut calibration = Vec::new();
    calibration
        .try_reserve_exact(result_slots)
        .map_err(|_| resource("cannot preallocate pair-mapper calibration result"))?;
    let mut replay = Vec::new();
    replay
        .try_reserve_exact(result_slots)
        .map_err(|_| resource("cannot preallocate pair-mapper replay result"))?;
    let actual_batch_bytes = checked_mul(
        usize_to_u64(batch.capacity(), "pair-mapper actual batch capacity")?,
        pending_size,
        "pair-mapper actual batch-vector bytes overflow",
    )?;
    let actual_result_bytes = placement_result_bytes(
        &calibration,
        calibration.capacity(),
        &replay,
        replay.capacity(),
    )?;
    if actual_batch_bytes > requested_batch_bytes
        || actual_result_bytes > requested_result_bytes
        || batch.capacity() < batch_slots
        || calibration.capacity() < result_slots
        || replay.capacity() < result_slots
    {
        return Err(resource(
            "allocator capacity differs from pair-mapper preallocation admission",
        ));
    }
    let actual_peak = [
        mapper_index_bytes,
        worker_bytes,
        actual_batch_bytes,
        actual_result_bytes,
        FIXED_MEMORY_ALLOWANCE,
    ]
    .into_iter()
    .try_fold(0u64, |sum, value| {
        checked_add(sum, value, "pair-mapper actual preallocation peak overflow")
    })?;
    if actual_peak > config.limits.total_accounted_memory_bytes {
        return Err(resource(
            "pair-mapper actual preallocated vectors exceed total memory budget",
        ));
    }
    Ok((batch, calibration, replay, actual_peak))
}

#[allow(clippy::too_many_arguments)]
fn process_batch(
    pool: &rayon::ThreadPool,
    mapper: &IndexedExactMapper<'_>,
    graph: &PairPathGraph,
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    mapper_identity: [u8; 32],
    config: PairMapperConfig,
    mapper_plan: IndexedMapperPlan,
    batch: &mut Vec<PendingFragment>,
    batch_decoded_bytes: &mut u64,
    calibration_pairs: &mut Vec<PairPlacementEvidence>,
    replay_pairs: &mut Vec<PairPlacementEvidence>,
    conservation: &mut PairMapperConservationTelemetry,
    execution: &mut PairMapperExecutionTelemetry,
) -> Result<()> {
    if batch.is_empty() {
        if *batch_decoded_bytes != 0 {
            return Err(invariant(
                "empty pair-mapper batch retains decoded-memory accounting",
            ));
        }
        return Ok(());
    }
    let batch_fragments = usize_to_u64(batch.len(), "pair-mapper batch fragment count")?;
    let admitted_work = batch.iter().try_fold(0u64, |sum, pending| {
        let bound = fragment_mapping_work_bound(
            &pending.fragment,
            mapper_plan,
            config.seed_length,
            config.limits.maximum_mapping_candidates_per_read,
        )?;
        checked_add(sum, bound, "pair-mapper batch work-bound overflow")
    })?;
    let projected_work = checked_add(
        conservation.admitted_mapping_operations,
        admitted_work,
        "pair-mapper admitted work total overflow",
    )?;
    if projected_work > config.limits.maximum_admitted_mapping_operations {
        return Err(resource(
            "pair-mapper conservative mapping-work limit exceeded",
        ));
    }
    let current_result_bytes = placement_result_bytes(
        calibration_pairs,
        calibration_pairs.capacity(),
        replay_pairs,
        replay_pairs.capacity(),
    )?;
    let peak = accounted_batch_peak(
        mapper.accounted_resident_bytes(),
        config,
        batch,
        batch.capacity(),
        *batch_decoded_bytes,
        current_result_bytes,
    )?;
    if peak > config.limits.total_accounted_memory_bytes {
        return Err(resource(
            "pair-mapper decoded batch and worst-case results exceed total memory budget",
        ));
    }

    let mapped = pool.install(|| {
        batch
            .par_iter()
            .map(|pending| {
                map_fragment(
                    mapper,
                    graph,
                    common_source_root,
                    pair_mapping_source_root,
                    mapper_identity,
                    config,
                    &pending.fragment,
                )
            })
            .collect::<Result<Vec<_>>>()
    })?;
    if mapped.len() != batch.len() {
        return Err(invariant("pair-mapper parallel batch cardinality changed"));
    }
    for window in mapped.windows(2) {
        if window[0].pair.fragment_ordinal >= window[1].pair.fragment_ordinal {
            return Err(invariant(
                "pair-mapper parallel results are not in input ordinal order",
            ));
        }
    }
    for mapped_fragment in mapped {
        merge_fragment_telemetry(conservation, mapped_fragment.telemetry)?;
        if conservation.placement_groups > config.limits.maximum_placement_groups
            || conservation.placements > config.limits.maximum_placements
        {
            return Err(resource("pair-mapper placement cardinality limit exceeded"));
        }
        if mapped_fragment.calibration {
            conservation.calibration_fragments = checked_add(
                conservation.calibration_fragments,
                1,
                "pair-mapper calibration fragment count overflow",
            )?;
            if calibration_pairs.len() == calibration_pairs.capacity() {
                return Err(invariant(
                    "pair-mapper preallocated calibration result exhausted",
                ));
            }
            calibration_pairs.push(mapped_fragment.pair);
        } else {
            conservation.replay_fragments = checked_add(
                conservation.replay_fragments,
                1,
                "pair-mapper replay fragment count overflow",
            )?;
            if replay_pairs.len() == replay_pairs.capacity() {
                return Err(invariant(
                    "pair-mapper preallocated replay result exhausted",
                ));
            }
            replay_pairs.push(mapped_fragment.pair);
        }
        let result_bytes = placement_result_bytes(
            calibration_pairs,
            calibration_pairs.capacity(),
            replay_pairs,
            replay_pairs.capacity(),
        )?;
        if result_bytes > config.limits.result_memory_bytes {
            return Err(resource("pair-mapper result memory budget exceeded"));
        }
        execution.maximum_observed_result_bytes =
            execution.maximum_observed_result_bytes.max(result_bytes);
    }
    conservation.admitted_mapping_operations = projected_work;
    execution.batches = checked_add(execution.batches, 1, "pair-mapper batch counter overflow")?;
    execution.maximum_observed_batch_fragments = execution
        .maximum_observed_batch_fragments
        .max(batch_fragments);
    execution.maximum_observed_decoded_batch_bytes = execution
        .maximum_observed_decoded_batch_bytes
        .max(*batch_decoded_bytes);
    execution.maximum_accounted_peak_bytes = execution.maximum_accounted_peak_bytes.max(peak);
    batch.clear();
    *batch_decoded_bytes = 0;
    Ok(())
}

fn map_fragment(
    mapper: &IndexedExactMapper<'_>,
    graph: &PairPathGraph,
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    mapper_identity: [u8; 32],
    config: PairMapperConfig,
    fragment: &Fragment,
) -> Result<MappedFragment> {
    validate_fragment_shape(fragment)?;
    let r1 = fragment
        .reads
        .first()
        .ok_or_else(|| invariant("paired fragment lost R1"))?;
    let r2 = fragment
        .reads
        .get(1)
        .ok_or_else(|| invariant("paired fragment lost R2"))?;
    let r1_mapping = map_read(
        mapper,
        graph,
        common_source_root,
        pair_mapping_source_root,
        fragment.lane_ordinal,
        fragment.ordinal,
        r1,
        config,
    )?;
    let r2_mapping = map_read(
        mapper,
        graph,
        common_source_root,
        pair_mapping_source_root,
        fragment.lane_ordinal,
        fragment.ordinal,
        r2,
        config,
    )?;
    let fragment_identity = fragment_identity_sha256(
        common_source_root,
        pair_mapping_source_root,
        fragment,
        r1_mapping.digest,
        r2_mapping.digest,
    );
    let calibration = split_is_calibration(
        common_source_root,
        pair_mapping_source_root,
        graph.graph_root_sha256(),
        mapper_identity,
        config.split,
        fragment_identity,
    );
    let mut telemetry = FragmentTelemetry::default();
    record_read_telemetry(&mut telemetry, &r1_mapping)?;
    record_read_telemetry(&mut telemetry, &r2_mapping)?;
    let unavailable = u8::from(r1_mapping.unavailable.is_some())
        .checked_add(u8::from(r2_mapping.unavailable.is_some()))
        .ok_or_else(|| overflow("pair-mapper unavailable read count overflow"))?;
    Ok(MappedFragment {
        pair: PairPlacementEvidence {
            lane_ordinal: fragment.lane_ordinal,
            fragment_ordinal: fragment.ordinal,
            fragment_identity_sha256: fragment_identity,
            r1_read_sha256: r1_mapping.digest,
            r2_read_sha256: r2_mapping.digest,
            r1_groups: r1_mapping.groups,
            r2_groups: r2_mapping.groups,
            junction_spanning_reads_unavailable: unavailable,
        },
        calibration,
        telemetry,
    })
}

#[allow(clippy::too_many_arguments)]
fn map_read(
    mapper: &IndexedExactMapper<'_>,
    graph: &PairPathGraph,
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    lane_ordinal: u32,
    fragment_ordinal: u64,
    read: &ReadRecord,
    config: PairMapperConfig,
) -> Result<ReadMapping> {
    let digest = read_identity_sha256(
        common_source_root,
        pair_mapping_source_root,
        lane_ordinal,
        fragment_ordinal,
        read,
    )?;
    let Some(normalized) = eligible_normalized_read(
        &read.sequence,
        read.quality.as_deref(),
        config.expected_minimum_base_quality,
    )?
    else {
        return Ok(ReadMapping {
            digest,
            groups: Vec::new(),
            unavailable: Some(UnavailableReason::IneligibleAmbiguityOrQuality),
            state: ReadState::IneligibleAmbiguityOrQuality,
            work: MappingWorkTelemetry::default(),
        });
    };
    let mapping = mapper.map_with_memory_limit(
        &normalized,
        config.limits.maximum_mapping_candidates_per_read,
        config.limits.mapper_query_memory_bytes_per_worker,
    )?;
    let work = MappingWorkTelemetry::from(mapping.work);
    match (mapping.state, mapping.placement_groups) {
        (ReadState::IndeterminateCandidateLimit, None) => Ok(ReadMapping {
            digest,
            groups: Vec::new(),
            unavailable: Some(UnavailableReason::IndeterminateCandidateLimit),
            state: ReadState::IndeterminateCandidateLimit,
            work,
        }),
        (ReadState::Unmapped, Some(groups)) if groups.is_empty() => Ok(ReadMapping {
            digest,
            groups: Vec::new(),
            unavailable: Some(UnavailableReason::UnmappedPossibleGraphJunction),
            state: ReadState::Unmapped,
            work,
        }),
        (ReadState::SinglePlacementGroup, Some(groups)) if groups.len() == 1 => Ok(ReadMapping {
            digest,
            groups: convert_groups(graph, &normalized, groups)?,
            unavailable: None,
            state: ReadState::SinglePlacementGroup,
            work,
        }),
        (ReadState::MultiplePlacementGroups, Some(groups)) if groups.len() > 1 => Ok(ReadMapping {
            digest,
            groups: convert_groups(graph, &normalized, groups)?,
            unavailable: None,
            state: ReadState::MultiplePlacementGroups,
            work,
        }),
        _ => Err(invariant(
            "indexed exact mapper returned an incoherent state/group combination",
        )),
    }
}

fn convert_groups(
    graph: &PairPathGraph,
    normalized_read: &[u8],
    groups: Vec<PlacementGroup>,
) -> Result<Vec<ExactPlacementGroup>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(groups.len())
        .map_err(|_| resource("cannot allocate pair-mapper placement groups"))?;
    if output.capacity() > groups.len() {
        return Err(resource(
            "allocator exceeded pair-mapper placement-group admission",
        ));
    }
    let read_length = usize_to_u64(normalized_read.len(), "mapped read length")?;
    if groups.windows(2).any(|window| window[0] >= window[1]) {
        return Err(invariant(
            "indexed exact mapper placement groups lost canonical order",
        ));
    }
    for (ordinal, group) in groups.into_iter().enumerate() {
        let segment_index = graph
            .segment_index(&group.unitig_id)
            .ok_or_else(|| integrity("mapped unitig is absent from bound graph"))?;
        let segment = graph
            .segment_catalog()
            .get(
                usize::try_from(segment_index)
                    .map_err(|_| overflow("mapped segment index does not fit usize"))?,
            )
            .ok_or_else(|| integrity("mapped segment index is absent from graph catalog"))?;
        if segment.topology != Topology::Linear
            || group.start >= group.end
            || group.end > segment.length
            || group.end.checked_sub(group.start) != Some(read_length)
        {
            return Err(integrity(
                "indexed exact mapper placement violates bound graph coordinates",
            ));
        }
        let read_strand = match group.strand {
            '+' => Direction::Forward,
            '-' => Direction::Reverse,
            _ => return Err(integrity("indexed exact mapper emitted an invalid strand")),
        };
        let mut placements = Vec::new();
        placements
            .try_reserve_exact(1)
            .map_err(|_| resource("cannot allocate one exact pair-mapper placement"))?;
        if placements.capacity() > 1 {
            return Err(resource("allocator exceeded one-placement group admission"));
        }
        placements.push(ExactPlacement {
            segment_index,
            start: group.start,
            end: group.end,
            read_strand,
        });
        output.push(ExactPlacementGroup {
            group_ordinal: u32::try_from(ordinal)
                .map_err(|_| resource("pair-mapper placement group exceeds u32 identity space"))?,
            placements,
        });
    }
    Ok(output)
}

fn eligible_normalized_read(
    sequence: &[u8],
    quality: Option<&[u8]>,
    minimum_base_quality: u8,
) -> Result<Option<Vec<u8>>> {
    if sequence.is_empty() {
        return Err(integrity("authenticated spool contains an empty read"));
    }
    if let Some(quality) = quality {
        if quality.len() != sequence.len() {
            return Err(integrity(
                "authenticated spool read has unequal sequence and quality lengths",
            ));
        }
        for &value in quality {
            if !(33..=126).contains(&value) {
                return Err(integrity(
                    "authenticated spool read contains an invalid Phred+33 byte",
                ));
            }
            if value - 33 < minimum_base_quality {
                return Ok(None);
            }
        }
    }
    let mut normalized = Vec::new();
    normalized
        .try_reserve_exact(sequence.len())
        .map_err(|_| resource("cannot allocate normalized pair-mapper read"))?;
    if normalized.capacity() > sequence.len() {
        return Err(resource(
            "allocator exceeded normalized pair-mapper read admission",
        ));
    }
    for &base in sequence {
        let normalized_base = base.to_ascii_uppercase();
        if !matches!(normalized_base, b'A' | b'C' | b'G' | b'T') {
            return Ok(None);
        }
        normalized.push(normalized_base);
    }
    Ok(Some(normalized))
}

fn record_read_telemetry(fragment: &mut FragmentTelemetry, mapping: &ReadMapping) -> Result<()> {
    match mapping.state {
        ReadState::SinglePlacementGroup => {
            fragment.exact_single = checked_add(
                fragment.exact_single,
                1,
                "pair-mapper single-group read count overflow",
            )?;
        }
        ReadState::MultiplePlacementGroups => {
            fragment.exact_multiple = checked_add(
                fragment.exact_multiple,
                1,
                "pair-mapper multiple-group read count overflow",
            )?;
        }
        ReadState::IneligibleAmbiguityOrQuality => {
            fragment.unavailable.ineligible_ambiguity_or_quality = checked_add(
                fragment.unavailable.ineligible_ambiguity_or_quality,
                1,
                "pair-mapper ineligible read count overflow",
            )?;
        }
        ReadState::IndeterminateCandidateLimit => {
            fragment.unavailable.indeterminate_candidate_limit = checked_add(
                fragment.unavailable.indeterminate_candidate_limit,
                1,
                "pair-mapper candidate-limit read count overflow",
            )?;
        }
        ReadState::Unmapped => {
            fragment.unavailable.unmapped_possible_graph_junction = checked_add(
                fragment.unavailable.unmapped_possible_graph_junction,
                1,
                "pair-mapper unmapped read count overflow",
            )?;
        }
        ReadState::NotRequested => {
            return Err(invariant(
                "pair-mapper classified a requested read as not requested",
            ));
        }
    }
    fragment.groups = checked_add(
        fragment.groups,
        usize_to_u64(mapping.groups.len(), "pair-mapper read group count")?,
        "pair-mapper group count overflow",
    )?;
    let placements = mapping.groups.iter().try_fold(0u64, |sum, group| {
        checked_add(
            sum,
            usize_to_u64(group.placements.len(), "pair-mapper placement count")?,
            "pair-mapper placement count overflow",
        )
    })?;
    fragment.placements = checked_add(
        fragment.placements,
        placements,
        "pair-mapper placement count overflow",
    )?;
    fragment.mapping_work.add(mapping.work)
}

fn merge_fragment_telemetry(
    total: &mut PairMapperConservationTelemetry,
    fragment: FragmentTelemetry,
) -> Result<()> {
    total.exact_single_group_reads = checked_add(
        total.exact_single_group_reads,
        fragment.exact_single,
        "pair-mapper exact-single total overflow",
    )?;
    total.exact_multiple_group_reads = checked_add(
        total.exact_multiple_group_reads,
        fragment.exact_multiple,
        "pair-mapper exact-multiple total overflow",
    )?;
    total.unavailable.add(fragment.unavailable)?;
    total.placement_groups = checked_add(
        total.placement_groups,
        fragment.groups,
        "pair-mapper placement-group total overflow",
    )?;
    total.placements = checked_add(
        total.placements,
        fragment.placements,
        "pair-mapper placement total overflow",
    )?;
    total.observed_mapping_work.add(fragment.mapping_work)
}

impl UnavailableReasonCounts {
    fn add(&mut self, other: Self) -> Result<()> {
        self.ineligible_ambiguity_or_quality = checked_add(
            self.ineligible_ambiguity_or_quality,
            other.ineligible_ambiguity_or_quality,
            "pair-mapper ineligible total overflow",
        )?;
        self.indeterminate_candidate_limit = checked_add(
            self.indeterminate_candidate_limit,
            other.indeterminate_candidate_limit,
            "pair-mapper candidate-limit total overflow",
        )?;
        self.unmapped_possible_graph_junction = checked_add(
            self.unmapped_possible_graph_junction,
            other.unmapped_possible_graph_junction,
            "pair-mapper unmapped total overflow",
        )?;
        Ok(())
    }
}

impl MappingWorkTelemetry {
    fn add(&mut self, other: Self) -> Result<()> {
        self.seed_lookups = checked_add(
            self.seed_lookups,
            other.seed_lookups,
            "pair-mapper seed-lookup total overflow",
        )?;
        self.selected_seed_hits = checked_add(
            self.selected_seed_hits,
            other.selected_seed_hits,
            "pair-mapper selected-seed total overflow",
        )?;
        self.postings_examined = checked_add(
            self.postings_examined,
            other.postings_examined,
            "pair-mapper posting total overflow",
        )?;
        self.indexed_full_verifications = checked_add(
            self.indexed_full_verifications,
            other.indexed_full_verifications,
            "pair-mapper indexed-verification total overflow",
        )?;
        self.fallback_full_verifications = checked_add(
            self.fallback_full_verifications,
            other.fallback_full_verifications,
            "pair-mapper fallback-verification total overflow",
        )?;
        self.verified_placement_groups_seen = checked_add(
            self.verified_placement_groups_seen,
            other.verified_placement_groups_seen,
            "pair-mapper verified-group total overflow",
        )?;
        Ok(())
    }
}

fn compatibility_limits(limits: PairMapperLimits) -> Result<WorkLimits> {
    Ok(WorkLimits {
        maximum_pairs: limits.maximum_fragments,
        maximum_placements: limits.maximum_placements,
        maximum_placement_pairs_per_fragment: limits.maximum_placements,
        maximum_edges_per_path: 1,
        maximum_search_states_per_fragment: 1,
        maximum_search_arc_examinations_per_fragment: 1,
        maximum_path_reconstruction_elements_per_fragment: 1,
        maximum_target_paths_per_fragment: 1,
        maximum_compatible_paths_per_fragment: 1,
        maximum_worker_threads: 1,
        maximum_mapper_algorithm_bytes: usize_to_u64(
            PRODUCER_ALGORITHM_ID.len(),
            "pair-mapper algorithm ID length",
        )?,
        maximum_mapper_version_bytes: usize_to_u64(
            PRODUCER_ALGORITHM_VERSION.len(),
            "pair-mapper version length",
        )?,
        maximum_mapper_parameter_bytes: limits.maximum_mapper_parameter_bytes,
        maximum_graph_sequence_bases: limits.maximum_graph_sequence_bases,
        graph_memory_bytes: 1,
        search_memory_bytes_per_worker: 1,
        result_memory_bytes: limits.result_memory_bytes,
        analysis_memory_bytes: limits.total_accounted_memory_bytes,
    })
}

fn pair_order(left: &PairPlacementEvidence, right: &PairPlacementEvidence) -> std::cmp::Ordering {
    (
        left.lane_ordinal,
        left.fragment_ordinal,
        left.fragment_identity_sha256,
    )
        .cmp(&(
            right.lane_ordinal,
            right.fragment_ordinal,
            right.fragment_identity_sha256,
        ))
}

fn decode_hex_32(value: &str, context: &'static str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        return Err(integrity(context));
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_hex_nibble(pair[0]).ok_or_else(|| integrity(context))?;
        let low = decode_hex_nibble(pair[1]).ok_or_else(|| integrity(context))?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

const fn decode_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
fn lower_hex(value: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for &byte in value {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn hash_length_prefixed(hasher: &mut Sha256, value: &[u8]) -> Result<()> {
    hasher.update(usize_to_u64(value.len(), "pair-mapper hash field length")?.to_le_bytes());
    hasher.update(value);
    Ok(())
}

fn try_owned(value: &str, context: &'static str) -> Result<String> {
    let mut output = String::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| resource(context))?;
    output.push_str(value);
    Ok(output)
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn checked_mul(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| overflow(context))
}

fn usize_to_u64(value: usize, context: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| overflow(context))
}

fn config_error(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ConfigurationInvalidLimit, context)
}

fn resource(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn integrity(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegrityArtifact, context)
}

fn invariant(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
    use crate::experimental::transition_witness::transition_source_root;
    use crate::model::AvailabilityU64;
    use crate::spool::create_spool;
    use std::collections::BTreeSet;
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::{tempdir, TempDir};

    fn scientific() -> ScientificConfig {
        ScientificConfig::resolve(
            3,
            Profile::RetainAll,
            SupportUnit::SuppliedFragmentInstance,
            None,
            20,
            true,
        )
        .unwrap()
    }

    fn paired_spool(lanes: &[(&str, &str)]) -> (TempDir, Spool) {
        let directory = tempdir().unwrap();
        let mut read1 = Vec::new();
        let mut read2 = Vec::new();
        for (lane, (r1, r2)) in lanes.iter().enumerate() {
            let r1_path = directory.path().join(format!("lane-{lane}-r1.fastq"));
            let r2_path = directory.path().join(format!("lane-{lane}-r2.fastq"));
            fs::write(&r1_path, r1).unwrap();
            fs::write(&r2_path, r2).unwrap();
            read1.push(r1_path);
            read2.push(r2_path);
        }
        let spool = create_spool(
            &InputSpec::Paired { read1, read2 },
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        (directory, spool)
    }

    fn unitig(id: &str, sequence: &[u8]) -> Unitig {
        Unitig {
            id: id.to_owned(),
            sequence: sequence.to_vec(),
            topology: Topology::Linear,
            edge_steps: sequence.len() as u64,
            canonical_kmers: sequence.len() as u64,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: AvailabilityU64::NotAvailable("test"),
            single_group_read_instances: AvailabilityU64::NotAvailable("test"),
            multi_group_read_instances_with_group: AvailabilityU64::NotAvailable("test"),
            placement_enumeration_status: "test",
            sequence_sha256: lower_hex(&Sha256::digest(sequence).into()),
        }
    }

    fn pair_path_limits() -> WorkLimits {
        WorkLimits {
            maximum_pairs: 100,
            maximum_placements: 1_000,
            maximum_placement_pairs_per_fragment: 32,
            maximum_edges_per_path: 8,
            maximum_search_states_per_fragment: 512,
            maximum_search_arc_examinations_per_fragment: 2_048,
            maximum_path_reconstruction_elements_per_fragment: 16_384,
            maximum_target_paths_per_fragment: 128,
            maximum_compatible_paths_per_fragment: 16,
            maximum_worker_threads: 4,
            maximum_mapper_algorithm_bytes: 256,
            maximum_mapper_version_bytes: 128,
            maximum_mapper_parameter_bytes: 4 << 10,
            maximum_graph_sequence_bases: 1 << 20,
            graph_memory_bytes: 8 << 20,
            search_memory_bytes_per_worker: 1 << 20,
            result_memory_bytes: 32 << 20,
            analysis_memory_bytes: 64 << 20,
        }
    }

    fn graph(unitigs: &[Unitig]) -> PairPathGraph {
        PairPathGraph::from_unverified_compaction(3, unitigs, &[], pair_path_limits()).unwrap()
    }

    fn config(spool: &Spool, graph: &PairPathGraph) -> PairMapperConfig {
        PairMapperConfig {
            expected_graph_root_sha256: graph.graph_root_sha256(),
            expected_common_source_root_sha256: transition_source_root(spool).unwrap(),
            expected_pair_mapping_source_root_sha256: pair_mapping_source_root_sha256(spool)
                .unwrap(),
            expected_minimum_base_quality: spool.scientific_config().min_base_quality,
            seed_length: 3,
            worker_threads: 2,
            split: CalibrationSplitRule {
                salt: [19; 32],
                numerator: 1,
                denominator: 2,
            },
            limits: PairMapperLimits {
                maximum_fragments: 100,
                maximum_reads: 200,
                maximum_bases: 1 << 20,
                maximum_graph_sequence_bases: 1 << 20,
                maximum_mapping_candidates_per_read: 100,
                maximum_admitted_mapping_operations: 1_000_000_000,
                maximum_placement_groups: 100_000,
                maximum_placements: 100_000,
                maximum_batch_fragments: 3,
                maximum_decoded_batch_bytes: 8 << 20,
                mapper_index_memory_bytes: 8 << 20,
                mapper_query_memory_bytes_per_worker: 2 << 20,
                result_memory_bytes: 8 << 20,
                total_accounted_memory_bytes: 64 << 20,
                maximum_worker_threads: 4,
                maximum_mapper_parameter_bytes: 4 << 10,
            },
        }
    }

    fn all_pairs(result: &PairMapperResult) -> Vec<&PairPlacementEvidence> {
        let mut pairs: Vec<_> = result
            .calibration_input()
            .pairs
            .iter()
            .chain(&result.replay_input().pairs)
            .collect();
        pairs.sort_unstable_by_key(|pair| pair.fragment_ordinal);
        pairs
    }

    fn reverse_complement(sequence: &[u8]) -> Vec<u8> {
        sequence
            .iter()
            .rev()
            .map(|base| match base.to_ascii_uppercase() {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => b'N',
            })
            .collect()
    }

    fn literal_oracle(
        read: &[u8],
        graph: &PairPathGraph,
        unitigs: &[Unitig],
    ) -> Vec<(u32, u64, u64, Direction)> {
        let mut targets: Vec<_> = unitigs
            .iter()
            .filter(|unitig| unitig.topology == Topology::Linear)
            .collect();
        targets.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        let normalized: Vec<_> = read.iter().map(u8::to_ascii_uppercase).collect();
        let reverse = reverse_complement(&normalized);
        let mut output = Vec::new();
        for target in targets {
            if normalized.len() > target.sequence.len() {
                continue;
            }
            for start in 0..=target.sequence.len() - normalized.len() {
                let end = start + normalized.len();
                for (direction, query) in [
                    (Direction::Forward, normalized.as_slice()),
                    (Direction::Reverse, reverse.as_slice()),
                ] {
                    if target.sequence[start..end] == *query {
                        output.push((
                            graph.segment_index(&target.id).unwrap(),
                            start as u64,
                            end as u64,
                            direction,
                        ));
                    }
                }
            }
        }
        output
    }

    fn observed(groups: &[ExactPlacementGroup]) -> Vec<(u32, u64, u64, Direction)> {
        groups
            .iter()
            .flat_map(|group| &group.placements)
            .map(|placement| {
                (
                    placement.segment_index,
                    placement.start,
                    placement.end,
                    placement.read_strand,
                )
            })
            .collect()
    }

    #[test]
    fn exact_groups_equal_an_independent_literal_full_read_oracle() {
        let (_directory, spool) = paired_spool(&[(
            "@f0/1\nAAC\n+\nIII\n@f1/1\nACG\n+\nIII\n",
            "@f0/2\nCGT\n+\nIII\n@f1/2\nCCC\n+\nIII\n",
        )]);
        let unitigs = vec![unitig("z", b"TTTAACGGG"), unitig("a", b"AACGTTACG")];
        let graph = graph(&unitigs);
        let result =
            produce_pair_placement_evidence(&spool, &graph, &unitigs, config(&spool, &graph))
                .unwrap();
        let pairs = all_pairs(&result);
        let mut iterator = spool.iter().unwrap();
        for (pair, fragment) in pairs.into_iter().zip(&mut iterator) {
            let fragment = fragment.unwrap();
            let r1_oracle = literal_oracle(&fragment.reads[0].sequence, &graph, &unitigs);
            let r2_oracle = literal_oracle(&fragment.reads[1].sequence, &graph, &unitigs);
            assert_eq!(observed(&pair.r1_groups), r1_oracle);
            assert_eq!(observed(&pair.r2_groups), r2_oracle);
            let expected_unavailable =
                u8::from(r1_oracle.is_empty()) + u8::from(r2_oracle.is_empty());
            assert_eq!(
                pair.junction_spanning_reads_unavailable,
                expected_unavailable
            );
        }
        assert!(iterator.is_authenticated());
        assert_eq!(
            result.common_source_root_sha256(),
            transition_source_root(&spool).unwrap()
        );
        assert_eq!(
            result.pair_mapping_source_root_sha256(),
            result.calibration_input().library_source_root_sha256
        );
        assert_eq!(
            result.pair_mapping_source_root_sha256(),
            result.replay_input().library_source_root_sha256
        );
    }

    #[test]
    fn lane_fragment_mate_identities_and_split_are_exactly_conserved() {
        let lane = ("@same/1\nAAC\n+\nIII\n", "@same/2\nCGT\n+\nIII\n");
        let (_directory, spool) = paired_spool(&[lane, lane]);
        let unitigs = vec![unitig("u", b"AACGTTAAC")];
        let graph = graph(&unitigs);
        let result =
            produce_pair_placement_evidence(&spool, &graph, &unitigs, config(&spool, &graph))
                .unwrap();
        let pairs = all_pairs(&result);
        assert_eq!(pairs.len(), 2);
        assert_ne!(pairs[0].lane_ordinal, pairs[1].lane_ordinal);
        assert_ne!(
            pairs[0].fragment_identity_sha256,
            pairs[1].fragment_identity_sha256
        );
        assert_ne!(pairs[0].r1_read_sha256, pairs[1].r1_read_sha256);
        assert_eq!(
            result.conservation().calibration_fragments + result.conservation().replay_fragments,
            spool.fragment_count
        );
        let calibration: BTreeSet<_> = result
            .calibration_input()
            .pairs
            .iter()
            .map(|pair| pair.fragment_identity_sha256)
            .collect();
        assert!(result
            .replay_input()
            .pairs
            .iter()
            .all(|pair| !calibration.contains(&pair.fragment_identity_sha256)));
        assert_eq!(result.conservation().authenticated_reads, 4);
    }

    #[test]
    fn typed_unavailable_reasons_do_not_publish_partial_or_invented_groups() {
        let (_directory, spool) = paired_spool(&[(
            "@cap/1\nAAA\n+\nIII\n@bad/1\nNAA\n+\nIII\n",
            "@cap/2\nAAA\n+\nIII\n@bad/2\nCCC\n+\nIII\n",
        )]);
        let unitigs = vec![unitig("repeat", b"AAAAAAAAAA")];
        let graph = graph(&unitigs);
        let mut settings = config(&spool, &graph);
        settings.limits.maximum_mapping_candidates_per_read = 1;
        let result = produce_pair_placement_evidence(&spool, &graph, &unitigs, settings).unwrap();
        assert_eq!(
            result
                .conservation()
                .unavailable
                .indeterminate_candidate_limit,
            2
        );
        assert_eq!(
            result
                .conservation()
                .unavailable
                .ineligible_ambiguity_or_quality,
            1
        );
        assert_eq!(
            result
                .conservation()
                .unavailable
                .unmapped_possible_graph_junction,
            1
        );
        for pair in all_pairs(&result) {
            assert!(pair.r1_groups.is_empty());
            assert!(pair.r2_groups.is_empty());
            assert_eq!(pair.junction_spanning_reads_unavailable, 2);
        }
    }

    #[test]
    fn eligibility_matches_ambiguity_and_phred_boundaries() {
        assert_eq!(
            eligible_normalized_read(b"acg", Some(b"III"), 20).unwrap(),
            Some(b"ACG".to_vec())
        );
        assert_eq!(
            eligible_normalized_read(b"ANG", Some(b"III"), 20).unwrap(),
            None
        );
        assert_eq!(
            eligible_normalized_read(b"ACG", Some(b"I!I"), 20).unwrap(),
            None
        );
        assert_eq!(
            eligible_normalized_read(b"ACG", None, 93).unwrap(),
            Some(b"ACG".to_vec())
        );
        assert_eq!(
            eligible_normalized_read(b"ACG", Some(b"II"), 20)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn closed_graph_walks_are_outside_the_exact_mapper_domain() {
        let (_directory, spool) = paired_spool(&[("@f/1\nAAC\n+\nIII\n", "@f/2\nCGT\n+\nIII\n")]);
        let mut closed = unitig("closed", b"AACGTT");
        closed.topology = Topology::ClosedGraphWalk;
        let unitigs = vec![unitig("linear", b"GGGCCC"), closed];
        let graph = graph(&unitigs);
        let result =
            produce_pair_placement_evidence(&spool, &graph, &unitigs, config(&spool, &graph))
                .unwrap();
        let pairs = all_pairs(&result);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].r1_groups.is_empty());
        assert!(pairs[0].r2_groups.is_empty());
        assert_eq!(pairs[0].junction_spanning_reads_unavailable, 2);
        assert_eq!(
            result
                .conservation()
                .unavailable
                .unmapped_possible_graph_junction,
            2
        );
    }

    #[test]
    fn roots_reject_graph_source_and_authenticated_spool_mutation() {
        let (_directory, spool) = paired_spool(&[("@f/1\nAAC\n+\nIII\n", "@f/2\nCGT\n+\nIII\n")]);
        let unitigs = vec![unitig("u", b"AACGTT")];
        let graph = graph(&unitigs);
        let base = config(&spool, &graph);
        for mutate in [0usize, 1, 2] {
            let mut settings = base;
            match mutate {
                0 => settings.expected_graph_root_sha256[0] ^= 1,
                1 => settings.expected_common_source_root_sha256[0] ^= 1,
                _ => settings.expected_pair_mapping_source_root_sha256[0] ^= 1,
            }
            assert_eq!(
                produce_pair_placement_evidence(&spool, &graph, &unitigs, settings)
                    .unwrap_err()
                    .code(),
                ErrorCode::IntegrityArtifact
            );
        }
        #[cfg(unix)]
        fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let mut file = OpenOptions::new().write(true).open(spool.path()).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"X").unwrap();
        file.sync_all().unwrap();
        assert_eq!(
            produce_pair_placement_evidence(&spool, &graph, &unitigs, base)
                .unwrap_err()
                .code(),
            ErrorCode::IntegritySpool
        );
    }

    #[test]
    fn representative_graph_decode_work_and_result_caps_fail_closed() {
        let (_directory, spool) = paired_spool(&[("@f/1\nAAC\n+\nIII\n", "@f/2\nCGT\n+\nIII\n")]);
        let unitigs = vec![unitig("u", b"AACGTTAAC")];
        let graph = graph(&unitigs);
        let base = config(&spool, &graph);
        let mut graph_cap = base;
        graph_cap.limits.maximum_graph_sequence_bases = 1;
        let mut decode_cap = base;
        decode_cap.limits.maximum_decoded_batch_bytes = 1;
        let mut work_cap = base;
        work_cap.limits.maximum_admitted_mapping_operations = 1;
        let mut result_cap = base;
        result_cap.limits.result_memory_bytes = 1;
        for settings in [graph_cap, decode_cap, work_cap, result_cap] {
            assert_eq!(
                produce_pair_placement_evidence(&spool, &graph, &unitigs, settings)
                    .unwrap_err()
                    .code(),
                ErrorCode::ResourceMemory
            );
        }
    }

    #[test]
    fn preallocation_caps_accept_exact_boundary_and_reject_one_byte_less() {
        let (_directory, spool) = paired_spool(&[("@f/1\nAAC\n+\nIII\n", "@f/2\nCGT\n+\nIII\n")]);
        let unitigs = vec![unitig("u", b"AACGTTAAC")];
        let graph = graph(&unitigs);
        let mut settings = config(&spool, &graph);
        settings.worker_threads = 1;
        settings.limits.maximum_fragments = 1;
        settings.limits.maximum_batch_fragments = 1;
        let plan = IndexedExactMapper::plan(&unitigs, settings.seed_length).unwrap();
        let mapper = IndexedExactMapper::build_planned(
            &unitigs,
            IndexedMapperConfig {
                seed_length: settings.seed_length,
                index_memory_budget_bytes: settings.limits.mapper_index_memory_bytes,
                placement_memory_budget_bytes: settings.limits.mapper_query_memory_bytes_per_worker,
            },
            plan,
        )
        .unwrap();
        let (_, _, _, exact_peak) =
            preallocate_producer_vectors(settings, mapper.accounted_resident_bytes()).unwrap();
        settings.limits.total_accounted_memory_bytes = exact_peak;
        assert!(preallocate_producer_vectors(settings, mapper.accounted_resident_bytes()).is_ok());
        settings.limits.total_accounted_memory_bytes = exact_peak - 1;
        assert_eq!(
            preallocate_producer_vectors(settings, mapper.accounted_resident_bytes())
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );

        let pair_size = usize_to_u64(
            size_of::<PairPlacementEvidence>(),
            "test pair placement evidence size",
        )
        .unwrap();
        let exact_result_outer = pair_size * 2 + PER_ALLOCATION_ALLOWANCE * 2;
        settings.limits.total_accounted_memory_bytes = 64 << 20;
        settings.limits.result_memory_bytes = exact_result_outer;
        assert!(preallocate_producer_vectors(settings, mapper.accounted_resident_bytes()).is_ok());
        settings.limits.result_memory_bytes = exact_result_outer - 1;
        assert_eq!(
            preallocate_producer_vectors(settings, mapper.accounted_resident_bytes())
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn worker_batch_and_unitig_order_do_not_change_scientific_outputs() {
        let (_directory, spool) = paired_spool(&[(
            "@a/1\nAAC\n+\nIII\n@b/1\nACG\n+\nIII\n@c/1\nGTT\n+\nIII\n",
            "@a/2\nCGT\n+\nIII\n@b/2\nTTA\n+\nIII\n@c/2\nCCC\n+\nIII\n",
        )]);
        let unitigs = vec![unitig("z", b"TTTAACGGG"), unitig("a", b"AACGTTACG")];
        let graph = graph(&unitigs);
        let mut one = config(&spool, &graph);
        one.worker_threads = 1;
        one.limits.maximum_batch_fragments = 1;
        let first = produce_pair_placement_evidence(&spool, &graph, &unitigs, one).unwrap();
        let mut reversed = unitigs.clone();
        reversed.reverse();
        let mut four = config(&spool, &graph);
        four.worker_threads = 4;
        four.limits.maximum_batch_fragments = 3;
        let second = produce_pair_placement_evidence(&spool, &graph, &reversed, four).unwrap();
        assert_eq!(first.producer_root_sha256(), second.producer_root_sha256());
        assert_eq!(first.calibration_input(), second.calibration_input());
        assert_eq!(first.replay_input(), second.replay_input());
        assert_eq!(first.conservation(), second.conservation());
        assert_ne!(
            first.execution().worker_threads,
            second.execution().worker_threads
        );
        assert_ne!(first.execution().batches, second.execution().batches);
    }

    #[test]
    fn upstream_pair_identifier_mismatch_is_rejected_before_a_spool_exists() {
        let directory = tempdir().unwrap();
        let r1 = directory.path().join("r1.fastq");
        let r2 = directory.path().join("r2.fastq");
        fs::write(&r1, b"@left/1\nAAC\n+\nIII\n").unwrap();
        fs::write(&r2, b"@right/2\nCGT\n+\nIII\n").unwrap();
        let error = create_spool(
            &InputSpec::Paired {
                read1: vec![r1],
                read2: vec![r2],
            },
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PairIdentity);
    }
}

impl From<MappingWork> for MappingWorkTelemetry {
    fn from(value: MappingWork) -> Self {
        Self {
            seed_lookups: value.seed_lookups,
            selected_seed_hits: value.selected_seed_hits,
            postings_examined: value.postings_examined,
            indexed_full_verifications: value.indexed_full_verifications,
            fallback_full_verifications: value.fallback_full_verifications,
            verified_placement_groups_seen: value.verified_placement_groups_seen,
        }
    }
}

fn read_identity_sha256(
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    lane_ordinal: u32,
    fragment_ordinal: u64,
    read: &ReadRecord,
) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-mapper-read:v1\0");
    hasher.update(common_source_root);
    hasher.update(pair_mapping_source_root);
    hasher.update(lane_ordinal.to_le_bytes());
    hasher.update(fragment_ordinal.to_le_bytes());
    hasher.update([read.role.tag()]);
    hasher.update(read.normalized_id_digest);
    hash_length_prefixed(&mut hasher, &read.sequence)?;
    match &read.quality {
        Some(quality) => {
            hasher.update([1]);
            hash_length_prefixed(&mut hasher, quality)?;
        }
        None => {
            hasher.update([0]);
            hasher.update(0u64.to_le_bytes());
        }
    }
    Ok(hasher.finalize().into())
}

fn fragment_identity_sha256(
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    fragment: &Fragment,
    r1_digest: [u8; 32],
    r2_digest: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-mapper-fragment:v1\0");
    hasher.update(common_source_root);
    hasher.update(pair_mapping_source_root);
    hasher.update(fragment.lane_ordinal.to_le_bytes());
    hasher.update(fragment.ordinal.to_le_bytes());
    hasher.update([MateRole::R1.tag()]);
    hasher.update(r1_digest);
    hasher.update([MateRole::R2.tag()]);
    hasher.update(r2_digest);
    hasher.finalize().into()
}

fn split_is_calibration(
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    graph_root: [u8; 32],
    mapper_identity: [u8; 32],
    split: CalibrationSplitRule,
    fragment_identity: [u8; 32],
) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-mapper-split:v1\0");
    hasher.update(common_source_root);
    hasher.update(pair_mapping_source_root);
    hasher.update(graph_root);
    hasher.update(mapper_identity);
    hasher.update(split.salt);
    hasher.update(fragment_identity);
    let digest: [u8; 32] = hasher.finalize().into();
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&digest[..8]);
    let bucket = u64::from_be_bytes(prefix) % u64::from(split.denominator);
    bucket < u64::from(split.numerator)
}

fn subset_read_set_root(
    subset: &[u8],
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    graph_root: [u8; 32],
    mapper_identity: [u8; 32],
    split: CalibrationSplitRule,
    pairs: &[PairPlacementEvidence],
) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-mapper-subset:v1\0");
    hash_length_prefixed(&mut hasher, subset)?;
    hasher.update(common_source_root);
    hasher.update(pair_mapping_source_root);
    hasher.update(graph_root);
    hasher.update(mapper_identity);
    hash_split_rule(&mut hasher, split);
    hasher.update(usize_to_u64(pairs.len(), "pair-mapper subset pair count")?.to_le_bytes());
    for pair in pairs {
        hasher.update(pair.lane_ordinal.to_le_bytes());
        hasher.update(pair.fragment_ordinal.to_le_bytes());
        hasher.update(pair.fragment_identity_sha256);
        hasher.update(pair.r1_read_sha256);
        hasher.update(pair.r2_read_sha256);
    }
    Ok(hasher.finalize().into())
}

#[allow(clippy::too_many_arguments)]
fn producer_root_sha256(
    graph_root: [u8; 32],
    common_source_root: [u8; 32],
    pair_mapping_source_root: [u8; 32],
    graph_ancestry: &PairGraphAncestry,
    mapper_identity: [u8; 32],
    split: CalibrationSplitRule,
    calibration_read_root: [u8; 32],
    replay_read_root: [u8; 32],
    calibration_placement_root: [u8; 32],
    replay_placement_root: [u8; 32],
    telemetry: PairMapperConservationTelemetry,
) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-mapper-result:v2\0");
    hasher.update(graph_root);
    hasher.update(common_source_root);
    hasher.update(pair_mapping_source_root);
    hasher.update(graph_ancestry.source_equivalence_root_sha256());
    hasher.update(graph_ancestry.compacted_graph_ancestry_root_sha256());
    hasher.update(graph_ancestry.transition_root_sha256());
    hasher.update(graph_ancestry.witnessed_child_root_sha256());
    hasher.update(graph_ancestry.witnessed_child_authentication_root_sha256());
    hasher.update(graph_ancestry.pair_graph_root_sha256());
    hasher.update(graph_ancestry.authentication_root_sha256());
    hasher.update(mapper_identity);
    hash_split_rule(&mut hasher, split);
    hasher.update(calibration_read_root);
    hasher.update(replay_read_root);
    hasher.update(calibration_placement_root);
    hasher.update(replay_placement_root);
    for value in [
        telemetry.authenticated_fragments,
        telemetry.authenticated_reads,
        telemetry.authenticated_bases,
        telemetry.calibration_fragments,
        telemetry.replay_fragments,
        telemetry.exact_single_group_reads,
        telemetry.exact_multiple_group_reads,
        telemetry.unavailable.ineligible_ambiguity_or_quality,
        telemetry.unavailable.indeterminate_candidate_limit,
        telemetry.unavailable.unmapped_possible_graph_junction,
        telemetry.placement_groups,
        telemetry.placements,
        telemetry.admitted_mapping_operations,
        telemetry.observed_mapping_work.seed_lookups,
        telemetry.observed_mapping_work.selected_seed_hits,
        telemetry.observed_mapping_work.postings_examined,
        telemetry.observed_mapping_work.indexed_full_verifications,
        telemetry.observed_mapping_work.fallback_full_verifications,
        telemetry
            .observed_mapping_work
            .verified_placement_groups_seen,
    ] {
        hasher.update(value.to_le_bytes());
    }
    Ok(hasher.finalize().into())
}

fn hash_split_rule(hasher: &mut Sha256, split: CalibrationSplitRule) {
    hasher.update(SPLIT_ALGORITHM_VERSION.as_bytes());
    hasher.update(split.salt);
    hasher.update(split.numerator.to_le_bytes());
    hasher.update(split.denominator.to_le_bytes());
}

fn placement_input(
    graph: &PairPathGraph,
    library_source_root: [u8; 32],
    read_set_root: [u8; 32],
    mapper_identity: [u8; 32],
    mapper: MapperProvenance,
    pairs: Vec<PairPlacementEvidence>,
) -> Result<PlacementEvidenceInput> {
    Ok(PlacementEvidenceInput {
        library_source_root_sha256: library_source_root,
        read_set_root_sha256: read_set_root,
        mapper_identity_sha256: mapper_identity,
        placement_domain: PlacementDomain::LinearUnitigOnly,
        certificate: CompleteEnumerationCertificate {
            graph_root_sha256: graph.graph_root_sha256(),
            library_source_root_sha256: library_source_root,
            read_set_root_sha256: read_set_root,
            mapper_identity_sha256: mapper_identity,
            placement_domain: PlacementDomain::LinearUnitigOnly,
            supplied_fragment_instances: usize_to_u64(
                pairs.len(),
                "pair-mapper certificate fragment count",
            )?,
            complete_within_declared_domain: true,
        },
        mapper,
        pairs,
    })
}

fn mapper_provenance(spool: &Spool, config: PairMapperConfig) -> Result<MapperProvenance> {
    let mut parameters = String::new();
    append_parameter(&mut parameters, "producer=")?;
    append_parameter(&mut parameters, PRODUCER_ALGORITHM_ID)?;
    append_parameter(&mut parameters, ";producer_version=")?;
    append_parameter(&mut parameters, PRODUCER_ALGORITHM_VERSION)?;
    append_parameter(&mut parameters, ";mapper=")?;
    append_parameter(&mut parameters, ALGORITHM_ID)?;
    append_parameter(&mut parameters, ";mapper_version=")?;
    append_parameter(&mut parameters, ALGORITHM_VERSION)?;
    append_parameter(&mut parameters, ";target_universe=")?;
    append_parameter(&mut parameters, TARGET_UNIVERSE)?;
    append_parameter(&mut parameters, ";seed_length=")?;
    append_decimal(
        &mut parameters,
        usize_to_u64(config.seed_length, "pair-mapper seed length")?,
    )?;
    append_parameter(
        &mut parameters,
        ";match=zero_mismatch_full_read_verified;minimum_base_quality=",
    )?;
    append_decimal(
        &mut parameters,
        u64::from(spool.scientific_config().min_base_quality),
    )?;
    append_parameter(&mut parameters, ";maximum_graph_sequence_bases=")?;
    append_decimal(&mut parameters, config.limits.maximum_graph_sequence_bases)?;
    append_parameter(&mut parameters, ";maximum_mapping_candidates_per_read=")?;
    append_decimal(
        &mut parameters,
        config.limits.maximum_mapping_candidates_per_read,
    )?;
    append_parameter(
        &mut parameters,
        ";unavailable_policy=typed_exclusion_v1;split_algorithm=",
    )?;
    append_parameter(&mut parameters, SPLIT_ALGORITHM_VERSION)?;
    append_parameter(&mut parameters, ";split_numerator=")?;
    append_decimal(&mut parameters, u64::from(config.split.numerator))?;
    append_parameter(&mut parameters, ";split_denominator=")?;
    append_decimal(&mut parameters, u64::from(config.split.denominator))?;
    append_parameter(&mut parameters, ";split_salt=")?;
    append_hex(&mut parameters, &config.split.salt)?;
    if usize_to_u64(parameters.len(), "pair-mapper parameter length")?
        > config.limits.maximum_mapper_parameter_bytes
        || usize_to_u64(parameters.capacity(), "pair-mapper parameter capacity")?
            > config.limits.maximum_mapper_parameter_bytes
    {
        return Err(resource("pair-mapper parameters exceed their byte limit"));
    }
    Ok(MapperProvenance {
        algorithm_id: try_owned(PRODUCER_ALGORITHM_ID, "allocate pair-mapper algorithm ID")?,
        algorithm_version: try_owned(
            PRODUCER_ALGORITHM_VERSION,
            "allocate pair-mapper algorithm version",
        )?,
        parameters,
    })
}

fn append_parameter(output: &mut String, value: &str) -> Result<()> {
    output
        .try_reserve_exact(value.len())
        .map_err(|_| resource("cannot grow pair-mapper parameter serialization"))?;
    output.push_str(value);
    Ok(())
}

fn append_decimal(output: &mut String, mut value: u64) -> Result<()> {
    let mut storage = [0u8; 20];
    let mut cursor = storage.len();
    loop {
        cursor -= 1;
        storage[cursor] = b'0'
            + u8::try_from(value % 10).map_err(|_| {
                invariant("pair-mapper decimal digit does not fit its byte representation")
            })?;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let digits = std::str::from_utf8(&storage[cursor..])
        .map_err(|_| invariant("pair-mapper decimal serialization is not UTF-8"))?;
    append_parameter(output, digits)
}

fn append_hex(output: &mut String, value: &[u8; 32]) -> Result<()> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    output
        .try_reserve_exact(64)
        .map_err(|_| resource("cannot grow pair-mapper hexadecimal parameter"))?;
    for &byte in value {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Ok(())
}

fn fallible_clone_mapper(mapper: &MapperProvenance) -> Result<MapperProvenance> {
    Ok(MapperProvenance {
        algorithm_id: try_owned(&mapper.algorithm_id, "clone pair-mapper algorithm ID")?,
        algorithm_version: try_owned(
            &mapper.algorithm_version,
            "clone pair-mapper algorithm version",
        )?,
        parameters: try_owned(&mapper.parameters, "clone pair-mapper parameters")?,
    })
}

fn validate_config(config: PairMapperConfig) -> Result<()> {
    let limits = config.limits;
    if !(1..=MAX_SEED_LENGTH).contains(&config.seed_length)
        || config.worker_threads == 0
        || config.worker_threads > limits.maximum_worker_threads
        || config.worker_threads > MAX_WORKER_THREADS
        || config.split.numerator == 0
        || config.split.numerator >= config.split.denominator
        || limits.maximum_fragments == 0
        || limits.maximum_reads < 2
        || limits.maximum_bases == 0
        || limits.maximum_graph_sequence_bases == 0
        || !(1..=MAX_MAPPING_CANDIDATES).contains(&limits.maximum_mapping_candidates_per_read)
        || limits.maximum_admitted_mapping_operations == 0
        || limits.maximum_placement_groups == 0
        || limits.maximum_placements == 0
        || limits.maximum_batch_fragments == 0
        || limits.maximum_decoded_batch_bytes == 0
        || limits.mapper_index_memory_bytes == 0
        || limits.mapper_query_memory_bytes_per_worker == 0
        || limits.result_memory_bytes == 0
        || limits.total_accounted_memory_bytes == 0
        || limits.maximum_worker_threads == 0
        || limits.maximum_worker_threads > MAX_WORKER_THREADS
        || limits.maximum_mapper_parameter_bytes == 0
    {
        return Err(config_error(
            "pair-mapper configuration or resource limit is outside its experimental domain",
        ));
    }
    let stack_bytes = usize_to_u64(WORKER_STACK_BYTES, "pair-mapper worker stack bytes")?;
    let fixed = checked_add(
        limits.mapper_index_memory_bytes,
        checked_mul(
            u64::from(config.worker_threads),
            checked_add(
                limits.mapper_query_memory_bytes_per_worker,
                stack_bytes,
                "pair-mapper per-worker memory bound overflow",
            )?,
            "pair-mapper worker memory bound overflow",
        )?,
        "pair-mapper fixed memory bound overflow",
    )?;
    if fixed > limits.total_accounted_memory_bytes {
        return Err(resource(
            "pair-mapper index, query, and worker-stack reservations exceed total memory",
        ));
    }
    Ok(())
}

fn validate_paired_source_catalog(spool: &Spool) -> Result<()> {
    if spool.input_mode_tag() != 1 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationUnsupportedCombination,
            "pair-mapper requires an authenticated paired-end spool",
        ));
    }
    if spool.sources.is_empty() || spool.sources.len() % 2 != 0 {
        return Err(integrity(
            "paired spool source catalog does not contain complete mate lanes",
        ));
    }
    let mut source_records = 0u64;
    let mut fragment_records = 0u64;
    let mut source_bases = 0u64;
    for (lane, chunk) in spool.sources.chunks_exact(2).enumerate() {
        let expected_lane =
            u32::try_from(lane).map_err(|_| overflow("paired source lane ordinal exceeds u32"))?;
        if chunk[0].lane_ordinal != expected_lane
            || chunk[1].lane_ordinal != expected_lane
            || chunk[0].role != MateRole::R1
            || chunk[1].role != MateRole::R2
            || chunk[0].records != chunk[1].records
        {
            return Err(integrity(
                "paired spool source catalog has missing, reordered, or unequal mates",
            ));
        }
        source_records = checked_add(
            source_records,
            checked_add(
                chunk[0].records,
                chunk[1].records,
                "paired source read count overflow",
            )?,
            "paired source read count overflow",
        )?;
        fragment_records = checked_add(
            fragment_records,
            chunk[0].records,
            "paired source fragment count overflow",
        )?;
        source_bases = checked_add(
            source_bases,
            checked_add(
                chunk[0].bases,
                chunk[1].bases,
                "paired source base count overflow",
            )?,
            "paired source base count overflow",
        )?;
    }
    if source_records != spool.read_count
        || fragment_records != spool.fragment_count
        || source_bases != spool.stats.bases
    {
        return Err(integrity(
            "paired spool source cardinalities disagree with registered totals",
        ));
    }
    Ok(())
}

fn validate_registered_cardinalities(spool: &Spool, limits: PairMapperLimits) -> Result<()> {
    if spool.fragment_count > limits.maximum_fragments
        || spool.read_count > limits.maximum_reads
        || spool.stats.bases > limits.maximum_bases
    {
        return Err(resource(
            "registered paired spool exceeds pair-mapper cardinality limits",
        ));
    }
    if spool.read_count
        != spool
            .fragment_count
            .checked_mul(2)
            .ok_or_else(|| overflow("paired spool expected read count overflow"))?
    {
        return Err(integrity(
            "registered paired spool does not contain exactly two reads per fragment",
        ));
    }
    Ok(())
}

fn validate_fragment_shape(fragment: &Fragment) -> Result<()> {
    if fragment.reads.len() != 2
        || fragment.reads[0].role != MateRole::R1
        || fragment.reads[1].role != MateRole::R2
    {
        return Err(VeritasmError::new(
            ErrorCode::PairRole,
            "authenticated paired fragment does not contain exactly R1 then R2",
        ));
    }
    if fragment.reads[0].normalized_id_digest != fragment.reads[1].normalized_id_digest {
        return Err(VeritasmError::new(
            ErrorCode::PairIdentity,
            "authenticated paired fragment has mismatched normalized identifiers",
        ));
    }
    Ok(())
}

fn admit_fragment_cardinality(
    fragment: &Fragment,
    spool: &Spool,
    limits: PairMapperLimits,
    telemetry: &mut PairMapperConservationTelemetry,
    lane_replay: &mut LaneReplayState,
) -> Result<()> {
    if fragment.ordinal != telemetry.authenticated_fragments {
        return Err(integrity(
            "authenticated spool fragment ordinal is not contiguous",
        ));
    }
    if fragment.ordinal >= spool.fragment_count {
        return Err(integrity(
            "authenticated spool produced more fragments than registered",
        ));
    }
    while lane_replay.lane_index < spool.sources.len() / 2 {
        let registered = spool.sources[lane_replay.lane_index * 2].records;
        if lane_replay.observed_in_lane < registered {
            break;
        }
        if lane_replay.observed_in_lane != registered {
            return Err(integrity(
                "pair-mapper observed too many fragments in a registered lane",
            ));
        }
        lane_replay.lane_index += 1;
        lane_replay.observed_in_lane = 0;
    }
    let expected_lane = u32::try_from(lane_replay.lane_index)
        .map_err(|_| overflow("pair-mapper replay lane index exceeds u32"))?;
    if lane_replay.lane_index >= spool.sources.len() / 2 || fragment.lane_ordinal != expected_lane {
        return Err(integrity(
            "pair-mapper fragment lane differs from authenticated source cardinalities",
        ));
    }
    lane_replay.observed_in_lane = checked_add(
        lane_replay.observed_in_lane,
        1,
        "pair-mapper per-lane replay count overflow",
    )?;
    let fragment_bases = fragment.reads.iter().try_fold(0u64, |sum, read| {
        checked_add(
            sum,
            usize_to_u64(read.sequence.len(), "pair-mapper read base count")?,
            "pair-mapper fragment base count overflow",
        )
    })?;
    let fragments = checked_add(
        telemetry.authenticated_fragments,
        1,
        "pair-mapper fragment total overflow",
    )?;
    let reads = checked_add(
        telemetry.authenticated_reads,
        2,
        "pair-mapper read total overflow",
    )?;
    let bases = checked_add(
        telemetry.authenticated_bases,
        fragment_bases,
        "pair-mapper base total overflow",
    )?;
    if fragments > limits.maximum_fragments
        || reads > limits.maximum_reads
        || bases > limits.maximum_bases
    {
        return Err(resource(
            "authenticated spool replay exceeds pair-mapper cardinality limits",
        ));
    }
    telemetry.authenticated_fragments = fragments;
    telemetry.authenticated_reads = reads;
    telemetry.authenticated_bases = bases;
    Ok(())
}

fn validate_lane_replay_complete(spool: &Spool, lane_replay: &LaneReplayState) -> Result<()> {
    let mut lane_index = lane_replay.lane_index;
    let mut observed = lane_replay.observed_in_lane;
    while lane_index < spool.sources.len() / 2 {
        let registered = spool.sources[lane_index * 2].records;
        if observed != registered {
            return Err(integrity(
                "pair-mapper authenticated replay did not conserve per-lane fragments",
            ));
        }
        lane_index += 1;
        observed = 0;
    }
    Ok(())
}

fn validate_graph_targets(
    graph: &PairPathGraph,
    unitigs: &[Unitig],
    limits: PairMapperLimits,
) -> Result<()> {
    if graph.segment_catalog().len() != unitigs.len() {
        return Err(integrity(
            "pair-mapper target count differs from bound graph catalog",
        ));
    }
    let catalog_bases = graph
        .segment_catalog()
        .iter()
        .try_fold(0u64, |sum, segment| {
            checked_add(
                sum,
                segment.length,
                "pair-mapper graph catalog base total overflow",
            )
        })?;
    if catalog_bases > limits.maximum_graph_sequence_bases {
        return Err(resource(
            "pair-mapper graph sequence bases exceed their configured limit",
        ));
    }
    let verification_index_bytes = checked_add(
        checked_mul(
            usize_to_u64(unitigs.len(), "pair-mapper target verification count")?,
            usize_to_u64(size_of::<&Unitig>(), "unitig reference size")?,
            "pair-mapper target verification bytes overflow",
        )?,
        PER_ALLOCATION_ALLOWANCE,
        "pair-mapper target verification bytes overflow",
    )?;
    if verification_index_bytes > limits.total_accounted_memory_bytes {
        return Err(resource(
            "pair-mapper target verification index exceeds total memory budget",
        ));
    }
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(unitigs.len())
        .map_err(|_| resource("cannot allocate pair-mapper target verification index"))?;
    let actual_verification_bytes = checked_add(
        checked_mul(
            usize_to_u64(
                ordered.capacity(),
                "pair-mapper actual target verification capacity",
            )?,
            usize_to_u64(size_of::<&Unitig>(), "unitig reference size")?,
            "pair-mapper actual target verification bytes overflow",
        )?,
        PER_ALLOCATION_ALLOWANCE,
        "pair-mapper actual target verification bytes overflow",
    )?;
    if actual_verification_bytes > verification_index_bytes {
        return Err(resource(
            "allocator exceeded pair-mapper target verification admission",
        ));
    }
    ordered.extend(unitigs.iter());
    ordered.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    if ordered
        .windows(2)
        .any(|window| window[0].id == window[1].id)
    {
        return Err(integrity("pair-mapper target identifiers are not unique"));
    }
    let mut graph_bases = 0u64;
    for (segment, unitig) in graph.segment_catalog().iter().zip(ordered) {
        let length = usize_to_u64(unitig.sequence.len(), "pair-mapper target length")?;
        graph_bases = checked_add(
            graph_bases,
            length,
            "pair-mapper graph sequence-base total overflow",
        )?;
        let digest: [u8; 32] = Sha256::digest(&unitig.sequence).into();
        if segment.id != unitig.id
            || segment.length != length
            || segment.topology != unitig.topology
            || segment.sequence_sha256 != digest
            || !unitig
                .sequence
                .iter()
                .all(|base| matches!(base, b'A' | b'C' | b'G' | b'T'))
        {
            return Err(integrity(
                "pair-mapper target identity differs from bound graph catalog",
            ));
        }
    }
    if graph_bases != catalog_bases {
        return Err(integrity(
            "pair-mapper graph sequence-base total differs from its bound catalog",
        ));
    }
    Ok(())
}

fn fragment_mapping_work_bound(
    fragment: &Fragment,
    plan: IndexedMapperPlan,
    seed_length: usize,
    maximum_candidates: u64,
) -> Result<u64> {
    fragment.reads.iter().try_fold(0u64, |sum, read| {
        let read_length = usize_to_u64(read.sequence.len(), "pair-mapper work-bound read length")?;
        let postings = usize_to_u64(plan.posting_count(), "pair-mapper posting count")?;
        let targets = usize_to_u64(plan.linear_targets(), "pair-mapper linear target count")?;
        let verified_bound = maximum_candidates
            .checked_add(1)
            .ok_or_else(|| overflow("pair-mapper verified-group work bound overflow"))?;
        let bound = if read.sequence.len() < seed_length {
            let target_bases_bound = checked_add(
                postings,
                checked_mul(
                    targets,
                    usize_to_u64(seed_length, "pair-mapper seed length")?,
                    "pair-mapper short-read target-base bound overflow",
                )?,
                "pair-mapper short-read target-base bound overflow",
            )?;
            let intervals = checked_add(
                target_bases_bound,
                targets,
                "pair-mapper short-read interval bound overflow",
            )?;
            checked_add(
                checked_mul(
                    intervals,
                    2,
                    "pair-mapper fallback comparison bound overflow",
                )?,
                verified_bound,
                "pair-mapper fallback work bound overflow",
            )?
        } else {
            let q = usize_to_u64(seed_length, "pair-mapper seed length")?;
            let per_orientation = read_length
                .checked_sub(q)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| overflow("pair-mapper seed-lookup bound overflow"))?;
            let seed_lookups =
                checked_mul(per_orientation, 2, "pair-mapper seed-lookup bound overflow")?;
            // Each selected orientation can expose at most every posting. The work telemetry
            // counts selected hits, examined postings, and full verifications independently.
            let posting_work = checked_mul(
                postings,
                6,
                "pair-mapper indexed posting-work bound overflow",
            )?;
            checked_add(
                checked_add(
                    seed_lookups,
                    posting_work,
                    "pair-mapper indexed work bound overflow",
                )?,
                verified_bound,
                "pair-mapper indexed work bound overflow",
            )?
        };
        checked_add(sum, bound, "pair-mapper fragment work bound overflow")
    })
}

fn accounted_batch_peak(
    mapper_index_bytes: u64,
    config: PairMapperConfig,
    batch: &[PendingFragment],
    batch_capacity: usize,
    decoded_batch_bytes: u64,
    current_result_bytes: u64,
) -> Result<u64> {
    let batch_count = usize_to_u64(batch.len(), "pair-mapper batch count")?;
    let worker_stack_bytes = usize_to_u64(WORKER_STACK_BYTES, "pair-mapper stack bytes")?;
    let worker_bytes = checked_mul(
        u64::from(config.worker_threads),
        checked_add(
            config.limits.mapper_query_memory_bytes_per_worker,
            worker_stack_bytes,
            "pair-mapper per-worker peak overflow",
        )?,
        "pair-mapper worker peak overflow",
    )?;
    let normalization_bytes = batch.iter().try_fold(0u64, |sum, pending| {
        let reads = pending
            .fragment
            .reads
            .iter()
            .try_fold(0u64, |subtotal, read| {
                checked_add(
                    subtotal,
                    usize_to_u64(read.sequence.len(), "pair-mapper normalization bytes")?,
                    "pair-mapper normalization bytes overflow",
                )
            })?;
        checked_add(sum, reads, "pair-mapper normalization bytes overflow")
    })?;
    let per_fragment_result = checked_add(
        OUTPUT_PAIR_BOUND_BYTES,
        checked_mul(
            checked_mul(
                config.limits.maximum_mapping_candidates_per_read,
                2,
                "pair-mapper per-fragment candidate bound overflow",
            )?,
            OUTPUT_GROUP_BOUND_BYTES,
            "pair-mapper per-fragment output bound overflow",
        )?,
        "pair-mapper per-fragment output bound overflow",
    )?;
    let batch_result_bound = checked_mul(
        batch_count,
        per_fragment_result,
        "pair-mapper batch output bound overflow",
    )?;
    let batch_outer_bytes = checked_mul(
        usize_to_u64(batch_capacity, "pair-mapper batch capacity")?,
        usize_to_u64(size_of::<PendingFragment>(), "pending-fragment size")?,
        "pair-mapper batch outer allocation overflow",
    )?;
    [
        mapper_index_bytes,
        worker_bytes,
        decoded_batch_bytes,
        normalization_bytes,
        batch_result_bound,
        batch_outer_bytes,
        current_result_bytes,
        FIXED_MEMORY_ALLOWANCE,
    ]
    .into_iter()
    .try_fold(0u64, |sum, value| {
        checked_add(sum, value, "pair-mapper accounted peak overflow")
    })
}

fn placement_result_bytes(
    calibration: &[PairPlacementEvidence],
    calibration_capacity: usize,
    replay: &[PairPlacementEvidence],
    replay_capacity: usize,
) -> Result<u64> {
    let mut allocations = 2u64;
    let outer_capacity = calibration_capacity
        .checked_add(replay_capacity)
        .ok_or_else(|| overflow("pair-mapper outer result capacity overflow"))?;
    let mut bytes = checked_mul(
        usize_to_u64(outer_capacity, "pair-mapper result capacity")?,
        usize_to_u64(
            size_of::<PairPlacementEvidence>(),
            "pair placement evidence size",
        )?,
        "pair-mapper outer result bytes overflow",
    )?;
    for pair in calibration.iter().chain(replay) {
        for groups in [&pair.r1_groups, &pair.r2_groups] {
            bytes = checked_add(
                bytes,
                checked_mul(
                    usize_to_u64(groups.capacity(), "pair-mapper group capacity")?,
                    usize_to_u64(size_of::<ExactPlacementGroup>(), "exact group size")?,
                    "pair-mapper group bytes overflow",
                )?,
                "pair-mapper result bytes overflow",
            )?;
            allocations = checked_add(allocations, 1, "pair-mapper allocation count overflow")?;
            for group in groups {
                bytes = checked_add(
                    bytes,
                    checked_mul(
                        usize_to_u64(
                            group.placements.capacity(),
                            "pair-mapper placement capacity",
                        )?,
                        usize_to_u64(size_of::<ExactPlacement>(), "exact placement size")?,
                        "pair-mapper placement bytes overflow",
                    )?,
                    "pair-mapper result bytes overflow",
                )?;
                allocations = checked_add(allocations, 1, "pair-mapper allocation count overflow")?;
            }
        }
    }
    checked_add(
        bytes,
        checked_mul(
            allocations,
            PER_ALLOCATION_ALLOWANCE,
            "pair-mapper allocation allowance overflow",
        )?,
        "pair-mapper result allocation bound overflow",
    )
}

fn placement_validation_scratch_bound(pairs: &[PairPlacementEvidence]) -> Result<u64> {
    let pair_count = usize_to_u64(pairs.len(), "pair-mapper validation pair count")?;
    let fixed_per_pair = usize_to_u64(
        size_of::<(u32, u64)>()
            .checked_add(size_of::<[u8; 32]>())
            .and_then(|value| value.checked_add(size_of::<&PairPlacementEvidence>()))
            .ok_or_else(|| overflow("pair-mapper validation record-size overflow"))?,
        "pair-mapper validation record size",
    )?;
    let mut bytes = checked_mul(
        pair_count,
        fixed_per_pair,
        "pair-mapper validation fixed scratch overflow",
    )?;
    let mut allocations = 3u64;
    for pair in pairs {
        for groups in [&pair.r1_groups, &pair.r2_groups] {
            bytes = checked_add(
                bytes,
                checked_mul(
                    usize_to_u64(groups.len(), "pair-mapper validation group count")?,
                    usize_to_u64(size_of::<u32>(), "pair-mapper group identity size")?,
                    "pair-mapper group-identity scratch overflow",
                )?,
                "pair-mapper validation scratch overflow",
            )?;
            let placements = groups.iter().try_fold(0u64, |sum, group| {
                checked_add(
                    sum,
                    usize_to_u64(
                        group.placements.len(),
                        "pair-mapper validation placement count",
                    )?,
                    "pair-mapper validation placement count overflow",
                )
            })?;
            bytes = checked_add(
                bytes,
                checked_mul(
                    placements,
                    // `(segment, start, end, Direction)` has at most this conservative width.
                    40,
                    "pair-mapper validation placement scratch overflow",
                )?,
                "pair-mapper validation scratch overflow",
            )?;
            allocations = checked_add(
                allocations,
                2,
                "pair-mapper validation allocation count overflow",
            )?;
        }
    }
    checked_add(
        bytes,
        checked_mul(
            allocations,
            PER_ALLOCATION_ALLOWANCE,
            "pair-mapper validation allocation allowance overflow",
        )?,
        "pair-mapper validation scratch bound overflow",
    )
}

fn calibration_reuse_scratch_bound(calibration_pairs: usize, replay_pairs: usize) -> Result<u64> {
    let identities = calibration_pairs
        .checked_add(replay_pairs)
        .ok_or_else(|| overflow("pair-mapper reuse identity count overflow"))?;
    checked_add(
        checked_mul(
            usize_to_u64(identities, "pair-mapper reuse identity count")?,
            usize_to_u64(size_of::<[u8; 32]>(), "pair-mapper reuse identity size")?,
            "pair-mapper reuse identity bytes overflow",
        )?,
        checked_mul(
            2,
            PER_ALLOCATION_ALLOWANCE,
            "pair-mapper reuse allocation allowance overflow",
        )?,
        "pair-mapper reuse scratch bound overflow",
    )
}

fn validate_final_conservation(
    spool: &Spool,
    calibration: &[PairPlacementEvidence],
    replay: &[PairPlacementEvidence],
    telemetry: PairMapperConservationTelemetry,
    limits: PairMapperLimits,
) -> Result<()> {
    let supplied = checked_add(
        usize_to_u64(calibration.len(), "calibration fragment count")?,
        usize_to_u64(replay.len(), "replay fragment count")?,
        "pair-mapper split fragment count overflow",
    )?;
    let classified_reads = [
        telemetry.exact_single_group_reads,
        telemetry.exact_multiple_group_reads,
        telemetry.unavailable.total()?,
    ]
    .into_iter()
    .try_fold(0u64, |sum, value| {
        checked_add(sum, value, "pair-mapper classified read count overflow")
    })?;
    let observed_work = telemetry.observed_mapping_work.total()?;
    if telemetry.authenticated_fragments != spool.fragment_count
        || telemetry.authenticated_reads != spool.read_count
        || telemetry.authenticated_bases != spool.stats.bases
        || telemetry.calibration_fragments
            != usize_to_u64(calibration.len(), "calibration fragment count")?
        || telemetry.replay_fragments != usize_to_u64(replay.len(), "replay fragment count")?
        || supplied != spool.fragment_count
        || classified_reads != spool.read_count
        || telemetry.placement_groups != telemetry.placements
        || telemetry.placement_groups > limits.maximum_placement_groups
        || telemetry.placements > limits.maximum_placements
        || observed_work > telemetry.admitted_mapping_operations
    {
        return Err(integrity(
            "pair-mapper authentication, classification, placement, or work conservation failed",
        ));
    }
    for pairs in [calibration, replay] {
        if pairs.windows(2).any(|window| {
            (window[0].lane_ordinal, window[0].fragment_ordinal)
                >= (window[1].lane_ordinal, window[1].fragment_ordinal)
        }) {
            return Err(integrity(
                "pair-mapper subset pair coordinates are duplicate or unordered",
            ));
        }
    }
    let (mut left, mut right) = (0usize, 0usize);
    while left < calibration.len() && right < replay.len() {
        let calibration_key = (
            calibration[left].lane_ordinal,
            calibration[left].fragment_ordinal,
        );
        let replay_key = (replay[right].lane_ordinal, replay[right].fragment_ordinal);
        match calibration_key.cmp(&replay_key) {
            std::cmp::Ordering::Less => left += 1,
            std::cmp::Ordering::Greater => right += 1,
            std::cmp::Ordering::Equal => {
                return Err(integrity(
                    "pair-mapper calibration/replay coordinates overlap",
                ));
            }
        }
    }
    Ok(())
}

impl MappingWorkTelemetry {
    pub fn total(self) -> Result<u64> {
        [
            self.seed_lookups,
            self.selected_seed_hits,
            self.postings_examined,
            self.indexed_full_verifications,
            self.fallback_full_verifications,
            self.verified_placement_groups_seen,
        ]
        .into_iter()
        .try_fold(0u64, |sum, value| {
            checked_add(sum, value, "pair-mapper observed work total overflow")
        })
    }
}
