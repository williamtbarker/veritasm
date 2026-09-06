//! Conservative, experimental paired-end constraints over an existing compact graph.
//!
//! This module is deliberately disconnected from stable assembly.  Its placement
//! domain is [`PlacementDomain::LinearUnitigOnly`]: a caller must quantify reads
//! which need graph-link-spanning placement, and those reads never enter model
//! fitting or path support.  Pair evidence annotates one existing path only after
//! exhaustive bounded enumeration; it never changes topology or spells sequence.

use crate::config::WORKER_STACK_BYTES;
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::model::{GraphLink, Topology, Unitig};
use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::mem::size_of;

pub const ALGORITHM_ID: &str = "linear_unitig_pair_path_evidence";
pub const ALGORITHM_VERSION: &str = "experimental-3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Forward,
    Reverse,
}

impl Direction {
    const fn flip(self) -> Self {
        match self {
            Self::Forward => Self::Reverse,
            Self::Reverse => Self::Forward,
        }
    }

    fn from_symbol(value: char) -> Result<Self> {
        match value {
            '+' => Ok(Self::Forward),
            '-' => Ok(Self::Reverse),
            _ => Err(integrity("pair-path orientation is neither plus nor minus")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementDomain {
    LinearUnitigOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct OrientedSegment {
    pub segment_index: u32,
    pub direction: Direction,
}

impl OrientedSegment {
    const fn flip(self) -> Self {
        Self {
            segment_index: self.segment_index,
            direction: self.direction.flip(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactPlacement {
    pub segment_index: u32,
    pub start: u64,
    pub end: u64,
    pub read_strand: Direction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactPlacementGroup {
    pub group_ordinal: u32,
    pub placements: Vec<ExactPlacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairPlacementEvidence {
    pub lane_ordinal: u32,
    pub fragment_ordinal: u64,
    /// Immutable supplied-fragment-instance identity. The producer must bind
    /// lane, ordinal, both reads, and their immutable source into this digest.
    pub fragment_identity_sha256: [u8; 32],
    pub r1_read_sha256: [u8; 32],
    pub r2_read_sha256: [u8; 32],
    pub r1_groups: Vec<ExactPlacementGroup>,
    pub r2_groups: Vec<ExactPlacementGroup>,
    /// Number of reads the upstream linear-unitig-only mapper could not
    /// evaluate because an exact placement may cross a graph link.
    pub junction_spanning_reads_unavailable: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MapperProvenance {
    pub algorithm_id: String,
    pub algorithm_version: String,
    /// Exact UTF-8 parameter serialization; bytes are bound without claiming
    /// arbitrary JSON canonicalization.
    pub parameters: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompleteEnumerationCertificate {
    pub graph_root_sha256: [u8; 32],
    /// Declared pre-split library/placement-producer lineage shared by
    /// calibration and replay subsets. This is source-authenticated only when
    /// enclosed by [`AuthenticatedPlacementEvidencePair`].
    pub library_source_root_sha256: [u8; 32],
    pub read_set_root_sha256: [u8; 32],
    pub mapper_identity_sha256: [u8; 32],
    pub placement_domain: PlacementDomain,
    pub supplied_fragment_instances: u64,
    /// Complete only inside `placement_domain`; this cannot assert complete
    /// junction-spanning placement under `LinearUnitigOnly`.
    pub complete_within_declared_domain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementEvidenceInput {
    /// A caller assertion in this freely constructible type; not an
    /// authenticated source claim by itself.
    pub library_source_root_sha256: [u8; 32],
    pub read_set_root_sha256: [u8; 32],
    pub mapper: MapperProvenance,
    pub mapper_identity_sha256: [u8; 32],
    pub placement_domain: PlacementDomain,
    pub certificate: CompleteEnumerationCertificate,
    pub pairs: Vec<PairPlacementEvidence>,
}

/// Immutable evidence pair emitted by the in-crate exact placement producer.
///
/// [`PlacementEvidenceInput`] is intentionally a public, freely constructible
/// interchange type and is therefore *not* authenticated by a self-consistent
/// digest alone. This capability has private fields and no public constructor:
/// only the trusted `pair_mapper` producer can create it after freezing both
/// disjoint subsets and its complete producer-result root.
#[derive(Debug)]
pub struct AuthenticatedPlacementEvidencePair {
    calibration: PlacementEvidenceInput,
    replay: PlacementEvidenceInput,
    producer_result_root_sha256: [u8; 32],
}

impl AuthenticatedPlacementEvidencePair {
    pub const fn calibration_input(&self) -> &PlacementEvidenceInput {
        &self.calibration
    }

    pub const fn replay_input(&self) -> &PlacementEvidenceInput {
        &self.replay
    }

    pub const fn producer_result_root_sha256(&self) -> [u8; 32] {
        self.producer_result_root_sha256
    }
}

/// Crate-sealed handoff from the exact pair mapper. Keeping this constructor
/// crate-private is the authentication boundary; the digest is evidence bound
/// by that producer, not a credential an external caller can self-issue.
pub(super) fn authenticated_evidence_pair_from_pair_mapper(
    calibration: PlacementEvidenceInput,
    replay: PlacementEvidenceInput,
    producer_result_root_sha256: [u8; 32],
) -> Result<AuthenticatedPlacementEvidencePair> {
    validate_declared_evidence_pair(&calibration, &replay)?;
    if producer_result_root_sha256 == [0; 32] {
        return Err(integrity(
            "authenticated pair-mapper producer root is the unset digest",
        ));
    }
    if calibration.read_set_root_sha256 == replay.read_set_root_sha256 {
        return Err(integrity(
            "authenticated calibration and replay subset roots are not distinct",
        ));
    }
    if calibration_reuse_telemetry(&calibration, &replay)?.reused_fragment_instances != 0 {
        return Err(integrity(
            "authenticated calibration and replay subsets reuse a fragment identity",
        ));
    }
    Ok(AuthenticatedPlacementEvidencePair {
        calibration,
        replay,
        producer_result_root_sha256,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum LibraryOrientation {
    #[serde(rename = "FR")]
    Fr,
    #[serde(rename = "RF")]
    Rf,
    #[serde(rename = "FF")]
    Ff,
    #[serde(rename = "RR")]
    Rr,
}

impl LibraryOrientation {
    const fn index(self) -> usize {
        match self {
            Self::Fr => 0,
            Self::Rf => 1,
            Self::Ff => 2,
            Self::Rr => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelConfig {
    pub minimum_anchors: u64,
    pub minimum_dominant_anchors: u64,
    pub dominance_numerator: u64,
    pub dominance_denominator: u64,
    pub maximum_span_p90_minus_p10: u64,
    pub maximum_inner_p90_minus_p10: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WorkLimits {
    pub maximum_pairs: u64,
    pub maximum_placements: u64,
    pub maximum_placement_pairs_per_fragment: u64,
    pub maximum_edges_per_path: u32,
    pub maximum_search_states_per_fragment: u64,
    pub maximum_search_arc_examinations_per_fragment: u64,
    pub maximum_path_reconstruction_elements_per_fragment: u64,
    pub maximum_target_paths_per_fragment: u64,
    pub maximum_compatible_paths_per_fragment: u64,
    pub maximum_worker_threads: u16,
    pub maximum_mapper_algorithm_bytes: u64,
    pub maximum_mapper_version_bytes: u64,
    pub maximum_mapper_parameter_bytes: u64,
    /// Maximum total sequence bases inspected and content-hashed while the
    /// immutable graph view is constructed. The sequences are caller-owned
    /// and are not retained by [`PairPathGraph`].
    pub maximum_graph_sequence_bases: u64,
    pub graph_memory_bytes: u64,
    pub search_memory_bytes_per_worker: u64,
    pub result_memory_bytes: u64,
    pub analysis_memory_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PairPathConfig {
    pub model: ModelConfig,
    pub limits: WorkLimits,
    pub minimum_distinct_fragments_for_path: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AnchorLedger {
    pub included: u64,
    pub no_placement: u64,
    pub nonunique_placement: u64,
    pub junction_spanning_unavailable: u64,
    pub cross_component: u64,
    pub different_unitig: u64,
    pub non_linear_unitig: u64,
    pub coincident_start: u64,
}

impl AnchorLedger {
    pub fn total(self) -> Result<u64> {
        [
            self.included,
            self.no_placement,
            self.nonunique_placement,
            self.junction_spanning_unavailable,
            self.cross_component,
            self.different_unitig,
            self.non_linear_unitig,
            self.coincident_start,
        ]
        .into_iter()
        .try_fold(0u64, |sum, value| {
            sum.checked_add(value)
                .ok_or_else(|| overflow("pair-path anchor ledger overflow"))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UnsignedDistribution {
    pub observations: u64,
    pub minimum: u64,
    pub p10: u64,
    pub median: u64,
    pub p90: u64,
    pub maximum: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SignedDistribution {
    pub observations: u64,
    pub minimum: i64,
    pub p10: i64,
    pub median: i64,
    pub p90: i64,
    pub maximum: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneModelAvailability {
    Available,
    InsufficientAnchors,
    NoUniqueDominantOrientation,
    InsufficientDominantAnchors,
    DominanceBelowThreshold,
    SpanIntervalTooWide,
    InnerIntervalTooWide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrozenLaneModel {
    pub lane_ordinal: u32,
    pub ledger: AnchorLedger,
    pub orientation_counts: [u64; 4],
    pub dominant_orientation: Option<LibraryOrientation>,
    pub outer_span: Option<UnsignedDistribution>,
    pub inner_gap: Option<SignedDistribution>,
    pub availability: LaneModelAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrozenLibraryModels {
    pub placement_domain: PlacementDomain,
    pub quantile_rule: &'static str,
    pub config: ModelConfig,
    pub lanes: Vec<FrozenLaneModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CanonicalGraphPath {
    pub handles: Vec<OrientedSegment>,
    pub link_indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatiblePathEvidence {
    pub path: CanonicalGraphPath,
    pub compatible_placement_observations: u64,
    pub minimum_outer_span: u64,
    pub maximum_outer_span: u64,
    pub minimum_inner_gap: i64,
    pub maximum_inner_gap: i64,
}

/// Conservative locus key used only to prevent an incompatible or incomplete
/// fragment from disappearing during path aggregation. Directions are omitted
/// deliberately: uncertainty on either reciprocal orientation blocks every
/// otherwise-supported path between the same unordered endpoint segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EndpointDomain {
    pub first_segment_index: u32,
    pub second_segment_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateAttributionScope {
    /// The decision has no defensible graph locus (for example, no placement).
    None,
    /// Every potentially affected unordered endpoint domain was enumerated.
    ExactEndpointDomains,
    /// A placement-combination cap prevented locus enumeration. Every emitted
    /// aggregate path is therefore conservatively indeterminate.
    GlobalIndeterminate,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ReplayLedger {
    pub placement_group_pairs: u64,
    pub placement_pairs: u64,
    pub cross_component_placement_pairs: u64,
    pub search_states_created: u64,
    pub search_states: u64,
    pub search_arcs_examined: u64,
    pub path_reconstruction_elements: u64,
    pub target_paths: u64,
    pub orientation_conflicts: u64,
    pub geometry_conflicts: u64,
    pub non_linear_topology_conflicts: u64,
    pub compatible_path_observations: u64,
    pub geometry_pruned_searches: u64,
    pub geometry_pruned_extensions: u64,
    pub placement_combination_cap_hits: u64,
    pub search_depth_cap_hits: u64,
    pub search_state_cap_hits: u64,
    pub search_arc_cap_hits: u64,
    pub path_reconstruction_cap_hits: u64,
    pub target_path_cap_hits: u64,
    pub compatible_path_cap_hits: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbstentionReason {
    LaneModelUnavailable,
    NoPlacement,
    JunctionSpanningPlacementUnavailable,
    PlacementCombinationLimit,
    CrossComponent,
    NonLinearTopology,
    SearchDepthLimit,
    SearchStateLimit,
    SearchArcWorkLimit,
    PathReconstructionWorkLimit,
    SearchPathCountLimit,
    CompatiblePathCountLimit,
    NoCompatiblePath,
    OrientationConflict,
    GeometryConflict,
    MultipleCompatiblePaths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairDecisionStatus {
    SupportedUniqueExistingPath,
    TrivialWithinUnitig,
    Abstained,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairPathDecision {
    pub lane_ordinal: u32,
    pub fragment_ordinal: u64,
    pub fragment_identity_sha256: [u8; 32],
    pub status: PairDecisionStatus,
    pub reason: Option<AbstentionReason>,
    /// Always false. This experimental observation cannot by itself authorize
    /// reconstruction, graph mutation, sequence spelling, or scaffold joins.
    pub authorizes_reconstruction: bool,
    pub aggregate_attribution_scope: AggregateAttributionScope,
    pub affected_endpoint_domains: Vec<EndpointDomain>,
    pub ledger: ReplayLedger,
    pub compatible_paths: Vec<CompatiblePathEvidence>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DecisionSummary {
    pub supplied_fragment_instances: u64,
    pub junction_spanning_reads_unavailable: u64,
    pub supported_unique_existing_path: u64,
    pub trivial_within_unitig: u64,
    pub abstained: u64,
    pub indeterminate: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PlacementInputCounts {
    pub fragment_instances: u64,
    pub placement_groups: u64,
    pub placements: u64,
    pub junction_spanning_reads_unavailable: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CalibrationReuseTelemetry {
    pub calibration_fragment_instances: u64,
    pub replay_fragment_instances: u64,
    pub reused_fragment_instances: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregatePathAvailability {
    AvailableForExperimentalConstraint,
    BelowMinimumDistinctFragments,
    Contradicted,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AggregatePathEvidence {
    pub path: CanonicalGraphPath,
    pub distinct_supporting_fragments: u64,
    pub distinct_contradicting_fragments: u64,
    pub distinct_indeterminate_fragments: u64,
    pub minimum_required_supporting_fragments: u64,
    pub availability: AggregatePathAvailability,
    /// `true` is only an experimental constraint signal. It never authorizes
    /// sequence synthesis, topology mutation, or a scaffold join.
    pub available_for_experimental_constraint: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairPathResult {
    pub algorithm_id: &'static str,
    pub algorithm_version: &'static str,
    pub placement_domain: PlacementDomain,
    pub library_source_root_sha256: [u8; 32],
    pub mapper_identity_sha256: [u8; 32],
    pub graph_root_sha256: [u8; 32],
    pub calibration_placement_root_sha256: [u8; 32],
    pub replay_placement_root_sha256: [u8; 32],
    pub graph_k: u8,
    pub graph_sequence_bases_hashed: u64,
    pub config: PairPathConfig,
    pub worker_threads: u16,
    pub graph_accounted_allocation_bytes: u64,
    pub calibration_input_counts: PlacementInputCounts,
    pub replay_input_counts: PlacementInputCounts,
    pub calibration_reuse: CalibrationReuseTelemetry,
    pub models: FrozenLibraryModels,
    pub decisions: Vec<PairPathDecision>,
    pub aggregate_paths: Vec<AggregatePathEvidence>,
    pub summary: DecisionSummary,
    /// Domain-separated digest over every evidence-bearing field in this
    /// result except this digest itself.
    pub result_root_sha256: [u8; 32],
}

/// Opaque result of analyzing an authenticated pair-mapper capability.
///
/// The freely constructible [`PairPathResult`] remains an unverified report.
/// This wrapper is the source-backed capability and cannot be forged or
/// mutated through the safe public API.
#[derive(Debug, Serialize)]
pub struct AuthenticatedPairPathResult {
    result: PairPathResult,
    placement_producer_result_root_sha256: [u8; 32],
    authenticated_result_root_sha256: [u8; 32],
}

impl AuthenticatedPairPathResult {
    pub const fn result(&self) -> &PairPathResult {
        &self.result
    }

    pub const fn placement_producer_result_root_sha256(&self) -> [u8; 32] {
        self.placement_producer_result_root_sha256
    }

    pub const fn authenticated_result_root_sha256(&self) -> [u8; 32] {
        self.authenticated_result_root_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphCatalogEntry {
    pub segment_index: u32,
    pub id: String,
    pub length: u64,
    pub topology: Topology,
    pub sequence_sha256: [u8; 32],
    pub component_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CanonicalLinkCatalogEntry {
    pub link_index: u32,
    pub from: OrientedSegment,
    pub to: OrientedSegment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Arc {
    to: OrientedSegment,
    link_index: u32,
}

/// Freely constructible, explicitly unverified directed view of exact compacted
/// unitigs and their exact `k-1` links.
///
/// This is a useful topology value for fixtures and independent experiments,
/// but possession of it proves neither registered-source ancestry nor
/// read-witnessed adjacency.  Only `authenticated_pair_graph` can promote a
/// witnessed-child capability into the distinct source-backed adapter used by
/// the authenticated pair mapper.
#[derive(Debug)]
pub struct PairPathGraph {
    k: u8,
    graph_root_sha256: [u8; 32],
    segments: Vec<GraphCatalogEntry>,
    canonical_links: Vec<CanonicalLinkCatalogEntry>,
    outgoing: Vec<Vec<Arc>>,
    sequence_bases_hashed: u64,
    accounted_allocation_bytes: u64,
}

impl PairPathGraph {
    /// Build an unverified graph from caller-owned materialized records.
    pub fn from_unverified_compaction(
        k: u8,
        unitigs: &[Unitig],
        links: &[GraphLink],
        limits: WorkLimits,
    ) -> Result<Self> {
        validate_limits(PairPathConfig {
            model: ModelConfig {
                minimum_anchors: 10,
                minimum_dominant_anchors: 10,
                dominance_numerator: 9,
                dominance_denominator: 10,
                maximum_span_p90_minus_p10: u64::MAX,
                maximum_inner_p90_minus_p10: u64::MAX,
            },
            limits,
            minimum_distinct_fragments_for_path: 2,
        })?;
        if !(3..=127).contains(&k) {
            return Err(config_error("pair-path graph k must be between 3 and 127"));
        }
        if u32::try_from(unitigs.len()).is_err() || u32::try_from(links.len()).is_err() {
            return Err(resource_error("pair-path graph exceeds u32 identity space"));
        }
        let unitig_count = u64::try_from(unitigs.len())
            .map_err(|_| overflow("pair-path unitig count does not fit u64"))?;
        let link_count = u64::try_from(links.len())
            .map_err(|_| overflow("pair-path link count does not fit u64"))?;
        let minimum_graph_bytes = 4096u64
            .checked_add(
                unitig_count
                    .checked_mul(512)
                    .ok_or_else(|| overflow("pair-path unitig record estimate overflow"))?,
            )
            .and_then(|value| value.checked_add(link_count.checked_mul(256)?))
            .ok_or_else(|| overflow("pair-path minimum graph estimate overflow"))?;
        if minimum_graph_bytes > limits.graph_memory_bytes {
            return Err(resource_error(
                "pair-path graph record count exceeds its memory budget",
            ));
        }
        let sequence_bases_hashed = unitigs.iter().try_fold(0u64, |sum, unitig| {
            checked_add(
                sum,
                usize_to_u64(unitig.sequence.len(), "pair-path unitig sequence length")?,
                "pair-path graph sequence-base count overflow",
            )
        })?;
        if sequence_bases_hashed > limits.maximum_graph_sequence_bases {
            return Err(resource_error(
                "pair-path graph sequence/hash-work limit exceeded",
            ));
        }
        let id_bytes = unitigs.iter().try_fold(0u64, |sum, unitig| {
            let length = u64::try_from(unitig.id.len())
                .map_err(|_| overflow("pair-path identifier length does not fit u64"))?;
            sum.checked_add(length)
                .ok_or_else(|| overflow("pair-path identifier bytes overflow"))
        })?;
        let required = minimum_graph_bytes
            .checked_add(id_bytes)
            .ok_or_else(|| overflow("pair-path graph memory estimate overflow"))?;
        if required > limits.graph_memory_bytes {
            return Err(resource_error("pair-path graph memory budget exceeded"));
        }

        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(unitigs.len())
            .map_err(|_| resource_error("cannot allocate ordered pair-path unitig references"))?;
        ordered.extend(unitigs.iter());
        ordered.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        if ordered
            .windows(2)
            .any(|window| window[0].id == window[1].id)
        {
            return Err(integrity(
                "pair-path graph has duplicate unitig identifiers",
            ));
        }
        let overlap = usize::from(k - 1);
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(ordered.len())
            .map_err(|_| resource_error("cannot allocate pair-path segments"))?;
        for (segment_offset, &unitig) in ordered.iter().enumerate() {
            if unitig.sequence.len() < usize::from(k)
                || !unitig
                    .sequence
                    .iter()
                    .all(|base| matches!(base, b'A' | b'C' | b'G' | b'T'))
            {
                return Err(integrity(
                    "pair-path unitig is shorter than k or contains non-ACGT",
                ));
            }
            segments.push(GraphCatalogEntry {
                segment_index: u32::try_from(segment_offset)
                    .map_err(|_| overflow("pair-path segment index does not fit u32"))?,
                id: try_clone_string(&unitig.id, "clone pair-path unitig identifier")?,
                length: u64::try_from(unitig.sequence.len())
                    .map_err(|_| overflow("pair-path unitig length does not fit u64"))?,
                topology: unitig.topology,
                sequence_sha256: Sha256::digest(&unitig.sequence).into(),
                component_index: 0,
            });
        }
        let handle_count = segments
            .len()
            .checked_mul(2)
            .ok_or_else(|| overflow("pair-path handle count overflow"))?;
        let mut outgoing = Vec::new();
        outgoing
            .try_reserve_exact(handle_count)
            .map_err(|_| resource_error("cannot allocate pair-path adjacency table"))?;
        outgoing.resize_with(handle_count, Vec::new);
        let mut parents = Vec::new();
        parents
            .try_reserve_exact(segments.len())
            .map_err(|_| resource_error("cannot allocate pair-path component parents"))?;
        parents.extend(0..segments.len());
        let mut ranks = Vec::new();
        ranks
            .try_reserve_exact(segments.len())
            .map_err(|_| resource_error("cannot allocate pair-path component ranks"))?;
        ranks.resize(segments.len(), 0u8);

        // Stable GraphLink stores one canonical representative of a physical
        // reciprocal pair.  Normalize again here so caller order and an
        // explicitly supplied reciprocal cannot change path identities.
        let mut normalized_links = Vec::new();
        normalized_links
            .try_reserve_exact(links.len())
            .map_err(|_| resource_error("cannot allocate normalized pair-path links"))?;
        for link in links {
            let from_index = segment_index(&segments, &link.from_segment)?;
            let to_index = segment_index(&segments, &link.to_segment)?;
            let from_direction = Direction::from_symbol(link.from_orientation)?;
            let to_direction = Direction::from_symbol(link.to_orientation)?;
            let forward = (from_index, from_direction, to_index, to_direction);
            let reciprocal = (
                to_index,
                to_direction.flip(),
                from_index,
                from_direction.flip(),
            );
            normalized_links.push(forward.min(reciprocal));
        }
        normalized_links.sort_unstable();
        normalized_links.dedup();
        let graph_root_sha256 = graph_content_root(k, &segments, &normalized_links)?;
        let mut canonical_links = Vec::new();
        canonical_links
            .try_reserve_exact(normalized_links.len())
            .map_err(|_| resource_error("cannot allocate canonical pair-path link catalog"))?;
        for (link_offset, &(from_index, from_direction, to_index, to_direction)) in
            normalized_links.iter().enumerate()
        {
            let from_unitig = ordered
                .get(u32_to_usize(from_index)?)
                .copied()
                .ok_or_else(|| integrity("normalized pair-path link source is absent"))?;
            let to_unitig = ordered
                .get(u32_to_usize(to_index)?)
                .copied()
                .ok_or_else(|| integrity("normalized pair-path link target is absent"))?;
            validate_overlap(
                from_unitig,
                from_direction,
                to_unitig,
                to_direction,
                overlap,
            )?;
            union(
                &mut parents,
                &mut ranks,
                u32_to_usize(from_index)?,
                u32_to_usize(to_index)?,
            );
            let link_index = u32::try_from(link_offset)
                .map_err(|_| overflow("pair-path link index does not fit u32"))?;
            canonical_links.push(CanonicalLinkCatalogEntry {
                link_index,
                from: OrientedSegment {
                    segment_index: from_index,
                    direction: from_direction,
                },
                to: OrientedSegment {
                    segment_index: to_index,
                    direction: to_direction,
                },
            });
            insert_arc(
                &mut outgoing,
                OrientedSegment {
                    segment_index: from_index,
                    direction: from_direction,
                },
                Arc {
                    to: OrientedSegment {
                        segment_index: to_index,
                        direction: to_direction,
                    },
                    link_index,
                },
            )?;
            insert_arc(
                &mut outgoing,
                OrientedSegment {
                    segment_index: to_index,
                    direction: to_direction,
                }
                .flip(),
                Arc {
                    to: OrientedSegment {
                        segment_index: from_index,
                        direction: from_direction,
                    }
                    .flip(),
                    link_index,
                },
            )?;
        }
        let mut roots = Vec::new();
        roots
            .try_reserve_exact(segments.len())
            .map_err(|_| resource_error("cannot allocate pair-path component roots"))?;
        for index in 0..segments.len() {
            roots.push(find(&mut parents, index));
        }
        let mut distinct_roots = Vec::new();
        distinct_roots
            .try_reserve_exact(roots.len())
            .map_err(|_| resource_error("cannot allocate distinct pair-path components"))?;
        distinct_roots.extend(roots.iter().copied());
        distinct_roots.sort_unstable();
        distinct_roots.dedup();
        for (index, root) in roots.into_iter().enumerate() {
            let component = distinct_roots
                .binary_search(&root)
                .map_err(|_| invariant("pair-path component root disappeared"))?;
            segments[index].component_index = u32::try_from(component)
                .map_err(|_| resource_error("pair-path component index does not fit u32"))?;
        }
        for arcs in &mut outgoing {
            arcs.sort_unstable();
            arcs.dedup();
        }
        let accounted_allocation_bytes = graph_resident_bytes(
            segments.capacity(),
            &segments,
            canonical_links.capacity(),
            outgoing.capacity(),
            &outgoing,
        )?;
        if accounted_allocation_bytes > limits.graph_memory_bytes {
            return Err(resource_error(
                "pair-path resident graph exceeds its memory budget",
            ));
        }
        Ok(Self {
            k,
            graph_root_sha256,
            segments,
            canonical_links,
            outgoing,
            sequence_bases_hashed,
            accounted_allocation_bytes,
        })
    }

    #[cfg(test)]
    fn from_compaction(
        k: u8,
        unitigs: &[Unitig],
        links: &[GraphLink],
        limits: WorkLimits,
    ) -> Result<Self> {
        Self::from_unverified_compaction(k, unitigs, links, limits)
    }

    pub fn segment_index(&self, id: &str) -> Option<u32> {
        self.segments
            .binary_search_by(|segment| segment.id.as_str().cmp(id))
            .ok()
            .and_then(|index| u32::try_from(index).ok())
    }

    pub fn segment_id(&self, index: u32) -> Option<&str> {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.segments.get(index))
            .map(|segment| segment.id.as_str())
    }

    pub const fn k(&self) -> u8 {
        self.k
    }

    pub const fn graph_root_sha256(&self) -> [u8; 32] {
        self.graph_root_sha256
    }

    pub fn segment_catalog(&self) -> &[GraphCatalogEntry] {
        &self.segments
    }

    pub fn link_catalog(&self) -> &[CanonicalLinkCatalogEntry] {
        &self.canonical_links
    }

    /// Conservative bytes held by the returned graph's owned vector and
    /// string capacities. Allocator metadata and process RSS are not included.
    pub const fn accounted_allocation_bytes(&self) -> u64 {
        self.accounted_allocation_bytes
    }

    /// Total caller-owned graph bases inspected and content-hashed at graph
    /// construction. These bytes are work accounting, not retained memory.
    pub const fn sequence_bases_hashed(&self) -> u64 {
        self.sequence_bases_hashed
    }

    fn segment(&self, index: u32) -> Result<&GraphCatalogEntry> {
        self.segments
            .get(u32_to_usize(index)?)
            .ok_or_else(|| integrity("pair-path placement references a missing segment"))
    }

    fn arcs(&self, handle: OrientedSegment) -> Result<&[Arc]> {
        let slot = u32_to_usize(handle.segment_index)?
            .checked_mul(2)
            .and_then(|value| {
                value.checked_add(usize::from(handle.direction == Direction::Reverse))
            })
            .ok_or_else(|| overflow("pair-path handle slot overflow"))?;
        self.outgoing
            .get(slot)
            .map(Vec::as_slice)
            .ok_or_else(|| integrity("pair-path handle is out of range"))
    }
}

#[derive(Default)]
struct LaneAccumulator {
    ledger: AnchorLedger,
    spans: [Vec<u64>; 4],
    inners: [Vec<i64>; 4],
}

/// Analyze freely constructible, caller-supplied placement evidence.
///
/// All structural certificates and roots are recomputed, but neither the input
/// nor returned report is an authenticated capability. The crate-private
/// authenticated analyzer is reserved for evidence emitted through
/// `authenticated_pair_graph`.
pub fn analyze_unverified_pair_paths(
    graph: &PairPathGraph,
    calibration_input: &PlacementEvidenceInput,
    replay_input: &PlacementEvidenceInput,
    config: PairPathConfig,
    worker_threads: usize,
) -> Result<PairPathResult> {
    analyze_pair_paths_inner(
        graph,
        calibration_input,
        replay_input,
        config,
        worker_threads,
    )
}

#[cfg(test)]
fn analyze_pair_paths(
    graph: &PairPathGraph,
    calibration_input: &PlacementEvidenceInput,
    replay_input: &PlacementEvidenceInput,
    config: PairPathConfig,
    worker_threads: usize,
) -> Result<PairPathResult> {
    analyze_unverified_pair_paths(
        graph,
        calibration_input,
        replay_input,
        config,
        worker_threads,
    )
}

/// Analyze evidence issued by the in-crate exact pair mapper.
pub(super) fn analyze_authenticated_pair_paths(
    graph: &PairPathGraph,
    evidence: &AuthenticatedPlacementEvidencePair,
    config: PairPathConfig,
    worker_threads: usize,
) -> Result<AuthenticatedPairPathResult> {
    let result = analyze_pair_paths_inner(
        graph,
        evidence.calibration_input(),
        evidence.replay_input(),
        config,
        worker_threads,
    )?;
    let producer_root = evidence.producer_result_root_sha256();
    let authenticated_root = authenticated_pair_path_result_root(&result, producer_root);
    let authenticated = AuthenticatedPairPathResult {
        result,
        placement_producer_result_root_sha256: producer_root,
        authenticated_result_root_sha256: authenticated_root,
    };
    validate_authenticated_pair_path_result(&authenticated)?;
    Ok(authenticated)
}

fn analyze_pair_paths_inner(
    graph: &PairPathGraph,
    calibration_input: &PlacementEvidenceInput,
    replay_input: &PlacementEvidenceInput,
    config: PairPathConfig,
    worker_threads: usize,
) -> Result<PairPathResult> {
    validate_limits(config)?;
    if worker_threads == 0 || worker_threads > usize::from(config.limits.maximum_worker_threads) {
        return Err(config_error(
            "pair-path worker count is zero or exceeds its configured maximum",
        ));
    }
    if graph.accounted_allocation_bytes() > config.limits.graph_memory_bytes {
        return Err(resource_error(
            "retained pair-path graph exceeds the analysis graph budget",
        ));
    }
    let analysis_bytes = graph
        .accounted_allocation_bytes()
        .checked_add(config.limits.result_memory_bytes)
        .and_then(|value| {
            value.checked_add(
                config
                    .limits
                    .search_memory_bytes_per_worker
                    .checked_mul(u64::try_from(worker_threads).ok()?)?,
            )
        })
        .and_then(|value| {
            value.checked_add(
                u64::try_from(WORKER_STACK_BYTES)
                    .ok()?
                    .checked_mul(u64::try_from(worker_threads).ok()?)?,
            )
        })
        .ok_or_else(|| overflow("pair-path analysis resident-memory estimate overflow"))?;
    if analysis_bytes > config.limits.analysis_memory_bytes {
        return Err(resource_error(
            "pair-path graph, result, and worker memory exceed the analysis budget",
        ));
    }
    let (calibration_root, calibration_counts) =
        validate_evidence_input(graph, calibration_input, config.limits)?;
    let (replay_root, replay_counts) = validate_evidence_input(graph, replay_input, config.limits)?;
    if calibration_input.library_source_root_sha256 != replay_input.library_source_root_sha256
        || calibration_input.mapper != replay_input.mapper
        || calibration_input.mapper_identity_sha256 != replay_input.mapper_identity_sha256
        || calibration_input.placement_domain != replay_input.placement_domain
    {
        return Err(integrity(
            "calibration and replay do not share one declared library lineage and mapper semantics",
        ));
    }
    if checked_add(
        calibration_counts.fragment_instances,
        replay_counts.fragment_instances,
        "combined pair-path input fragment count overflow",
    )? > config.limits.maximum_pairs
        || checked_add(
            calibration_counts.placements,
            replay_counts.placements,
            "combined pair-path input placement count overflow",
        )? > config.limits.maximum_placements
    {
        return Err(resource_error(
            "combined calibration and replay input caps exceeded",
        ));
    }
    let models = freeze_models(graph, &calibration_input.pairs, config.model)?;
    let decisions = replay(graph, &replay_input.pairs, &models, config, worker_threads)?;
    let mut summary =
        decisions
            .iter()
            .try_fold(DecisionSummary::default(), |mut value, decision| {
                value.supplied_fragment_instances = checked_add(
                    value.supplied_fragment_instances,
                    1,
                    "pair decision total overflow",
                )?;
                match decision.status {
                    PairDecisionStatus::SupportedUniqueExistingPath => {
                        value.supported_unique_existing_path = checked_add(
                            value.supported_unique_existing_path,
                            1,
                            "supported pair count overflow",
                        )?
                    }
                    PairDecisionStatus::TrivialWithinUnitig => {
                        value.trivial_within_unitig = checked_add(
                            value.trivial_within_unitig,
                            1,
                            "trivial within-unitig pair count overflow",
                        )?
                    }
                    PairDecisionStatus::Abstained => {
                        value.abstained =
                            checked_add(value.abstained, 1, "abstained pair count overflow")?
                    }
                    PairDecisionStatus::Indeterminate => {
                        value.indeterminate = checked_add(
                            value.indeterminate,
                            1,
                            "indeterminate pair count overflow",
                        )?
                    }
                }
                Ok(value)
            })?;
    summary.junction_spanning_reads_unavailable = replay_counts.junction_spanning_reads_unavailable;
    let calibration_reuse = calibration_reuse_telemetry(calibration_input, replay_input)?;
    let aggregate_paths =
        aggregate_path_evidence(&decisions, config.minimum_distinct_fragments_for_path)?;
    let mut result = PairPathResult {
        algorithm_id: ALGORITHM_ID,
        algorithm_version: ALGORITHM_VERSION,
        placement_domain: PlacementDomain::LinearUnitigOnly,
        library_source_root_sha256: calibration_input.library_source_root_sha256,
        mapper_identity_sha256: calibration_input.mapper_identity_sha256,
        graph_root_sha256: graph.graph_root_sha256(),
        calibration_placement_root_sha256: calibration_root,
        replay_placement_root_sha256: replay_root,
        graph_k: graph.k(),
        graph_sequence_bases_hashed: graph.sequence_bases_hashed(),
        config,
        worker_threads: u16::try_from(worker_threads)
            .map_err(|_| overflow("pair-path worker count does not fit u16"))?,
        graph_accounted_allocation_bytes: graph.accounted_allocation_bytes(),
        calibration_input_counts: calibration_counts,
        replay_input_counts: replay_counts,
        calibration_reuse,
        models,
        decisions,
        aggregate_paths,
        summary,
        result_root_sha256: [0; 32],
    };
    result.result_root_sha256 = pair_path_result_root_sha256(&result)?;
    validate_pair_path_result(&result)?;
    Ok(result)
}

pub fn mapper_identity_sha256(mapper: &MapperProvenance, limits: WorkLimits) -> Result<[u8; 32]> {
    validate_mapper(mapper, limits)?;
    mapper_identity_after_validation(mapper)
}

fn mapper_identity_after_validation(mapper: &MapperProvenance) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-path-mapper:v1\0");
    hash_length_prefixed(&mut hasher, mapper.algorithm_id.as_bytes())?;
    hash_length_prefixed(&mut hasher, mapper.algorithm_version.as_bytes())?;
    hash_length_prefixed(&mut hasher, mapper.parameters.as_bytes())?;
    Ok(hasher.finalize().into())
}

/// Recompute the domain-separated pair-path result root using the explicit
/// little-endian, length-prefixed field encoding in this function and its
/// helpers. The stored root field is deliberately excluded.
pub fn pair_path_result_root_sha256(result: &PairPathResult) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-path-result:v1\0");
    hash_length_prefixed(&mut hasher, result.algorithm_id.as_bytes())?;
    hash_length_prefixed(&mut hasher, result.algorithm_version.as_bytes())?;
    hasher.update([placement_domain_tag(result.placement_domain)]);
    hasher.update(result.library_source_root_sha256);
    hasher.update(result.mapper_identity_sha256);
    hasher.update(result.graph_root_sha256);
    hasher.update(result.calibration_placement_root_sha256);
    hasher.update(result.replay_placement_root_sha256);
    hasher.update([result.graph_k]);
    hasher.update(result.graph_sequence_bases_hashed.to_le_bytes());
    hash_pair_path_config(&mut hasher, result.config);
    hasher.update(result.worker_threads.to_le_bytes());
    hasher.update(result.graph_accounted_allocation_bytes.to_le_bytes());
    hash_input_counts(&mut hasher, result.calibration_input_counts);
    hash_input_counts(&mut hasher, result.replay_input_counts);
    for value in [
        result.calibration_reuse.calibration_fragment_instances,
        result.calibration_reuse.replay_fragment_instances,
        result.calibration_reuse.reused_fragment_instances,
    ] {
        hasher.update(value.to_le_bytes());
    }
    hash_frozen_models(&mut hasher, &result.models)?;
    hasher.update(
        usize_to_u64(result.decisions.len(), "pair-path result decision count")?.to_le_bytes(),
    );
    for decision in &result.decisions {
        hash_decision(&mut hasher, decision)?;
    }
    hasher.update(
        usize_to_u64(result.aggregate_paths.len(), "pair-path aggregate count")?.to_le_bytes(),
    );
    for aggregate in &result.aggregate_paths {
        hash_graph_path(&mut hasher, &aggregate.path)?;
        for value in [
            aggregate.distinct_supporting_fragments,
            aggregate.distinct_contradicting_fragments,
            aggregate.distinct_indeterminate_fragments,
            aggregate.minimum_required_supporting_fragments,
        ] {
            hasher.update(value.to_le_bytes());
        }
        hasher.update([aggregate_availability_tag(aggregate.availability)]);
        hasher.update([u8::from(aggregate.available_for_experimental_constraint)]);
    }
    for value in [
        result.summary.supplied_fragment_instances,
        result.summary.junction_spanning_reads_unavailable,
        result.summary.supported_unique_existing_path,
        result.summary.trivial_within_unitig,
        result.summary.abstained,
        result.summary.indeterminate,
    ] {
        hasher.update(value.to_le_bytes());
    }
    Ok(hasher.finalize().into())
}

fn authenticated_pair_path_result_root(
    result: &PairPathResult,
    producer_result_root_sha256: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:authenticated-pair-path-result:v1\0");
    hasher.update(producer_result_root_sha256);
    hasher.update(result.result_root_sha256);
    hasher.finalize().into()
}

/// Recompute and validate the roots carried by an opaque authenticated result.
pub fn validate_authenticated_pair_path_result(
    authenticated: &AuthenticatedPairPathResult,
) -> Result<()> {
    validate_pair_path_result(authenticated.result())?;
    if authenticated.placement_producer_result_root_sha256 == [0; 32]
        || authenticated.authenticated_result_root_sha256
            != authenticated_pair_path_result_root(
                authenticated.result(),
                authenticated.placement_producer_result_root_sha256,
            )
    {
        return Err(integrity(
            "authenticated pair-path result root is inconsistent",
        ));
    }
    Ok(())
}

/// Verify the typed invariants and stored content root of a pair-path result.
pub fn validate_pair_path_result(result: &PairPathResult) -> Result<()> {
    if result.algorithm_id != ALGORITHM_ID
        || result.algorithm_version != ALGORITHM_VERSION
        || result.placement_domain != PlacementDomain::LinearUnitigOnly
        || result.models.placement_domain != PlacementDomain::LinearUnitigOnly
        || result.models.config != result.config.model
        || !(3..=127).contains(&result.graph_k)
        || result.worker_threads == 0
        || result.worker_threads > result.config.limits.maximum_worker_threads
        || result.graph_sequence_bases_hashed > result.config.limits.maximum_graph_sequence_bases
        || result.graph_accounted_allocation_bytes > result.config.limits.graph_memory_bytes
    {
        return Err(integrity("pair-path result metadata is inconsistent"));
    }
    validate_limits(result.config)?;
    let analysis_bytes = result
        .graph_accounted_allocation_bytes
        .checked_add(result.config.limits.result_memory_bytes)
        .and_then(|value| {
            value.checked_add(
                result
                    .config
                    .limits
                    .search_memory_bytes_per_worker
                    .checked_mul(u64::from(result.worker_threads))?,
            )
        })
        .and_then(|value| {
            value.checked_add(
                u64::try_from(WORKER_STACK_BYTES)
                    .ok()?
                    .checked_mul(u64::from(result.worker_threads))?,
            )
        })
        .ok_or_else(|| overflow("pair-path result analysis-memory estimate overflow"))?;
    if analysis_bytes > result.config.limits.analysis_memory_bytes
        || result.calibration_reuse.calibration_fragment_instances
            != result.calibration_input_counts.fragment_instances
        || result.calibration_reuse.replay_fragment_instances
            != result.replay_input_counts.fragment_instances
        || result.calibration_reuse.reused_fragment_instances
            > result
                .calibration_reuse
                .calibration_fragment_instances
                .min(result.calibration_reuse.replay_fragment_instances)
    {
        return Err(integrity("pair-path result accounting is inconsistent"));
    }
    let mut recomputed_summary = DecisionSummary {
        supplied_fragment_instances: usize_to_u64(
            result.decisions.len(),
            "pair-path result decision count",
        )?,
        junction_spanning_reads_unavailable: result
            .replay_input_counts
            .junction_spanning_reads_unavailable,
        ..DecisionSummary::default()
    };
    for decision in &result.decisions {
        validate_decision_accounting(decision, result.config.limits)?;
        let counter = match decision.status {
            PairDecisionStatus::SupportedUniqueExistingPath => {
                &mut recomputed_summary.supported_unique_existing_path
            }
            PairDecisionStatus::TrivialWithinUnitig => {
                &mut recomputed_summary.trivial_within_unitig
            }
            PairDecisionStatus::Abstained => &mut recomputed_summary.abstained,
            PairDecisionStatus::Indeterminate => &mut recomputed_summary.indeterminate,
        };
        *counter = checked_add(*counter, 1, "pair-path result summary overflow")?;
    }
    if result.summary != recomputed_summary
        || result.replay_input_counts.fragment_instances
            != result.summary.supplied_fragment_instances
        || result.decisions.windows(2).any(|window| {
            (window[0].lane_ordinal, window[0].fragment_ordinal)
                >= (window[1].lane_ordinal, window[1].fragment_ordinal)
        })
        || result.decisions.iter().any(|decision| {
            decision.authorizes_reconstruction
                || decision
                    .affected_endpoint_domains
                    .windows(2)
                    .any(|window| window[0] >= window[1])
                || match decision.aggregate_attribution_scope {
                    AggregateAttributionScope::None
                    | AggregateAttributionScope::GlobalIndeterminate => {
                        !decision.affected_endpoint_domains.is_empty()
                    }
                    AggregateAttributionScope::ExactEndpointDomains => {
                        decision.affected_endpoint_domains.is_empty()
                    }
                }
                || decision.compatible_paths.iter().any(|evidence| {
                    evidence.path.handles.is_empty()
                        || evidence.path.handles.len()
                            != evidence.path.link_indices.len().saturating_add(1)
                })
        })
    {
        return Err(integrity("pair-path result decisions are inconsistent"));
    }
    let recomputed_aggregates = aggregate_path_evidence(
        &result.decisions,
        result.config.minimum_distinct_fragments_for_path,
    )?;
    if result.aggregate_paths != recomputed_aggregates {
        return Err(integrity("pair-path aggregate result is inconsistent"));
    }
    let recomputed = pair_path_result_root_sha256(result)?;
    if recomputed != result.result_root_sha256 {
        return Err(integrity("pair-path result content root does not match"));
    }
    Ok(())
}

fn validate_decision_accounting(decision: &PairPathDecision, limits: WorkLimits) -> Result<()> {
    let ledger = decision.ledger;
    let compatible_count = usize_to_u64(
        decision.compatible_paths.len(),
        "pair-path compatible result count",
    )?;
    let compatible_observations =
        decision
            .compatible_paths
            .iter()
            .try_fold(0u64, |sum, evidence| {
                if evidence.compatible_placement_observations == 0
                    || evidence.minimum_outer_span > evidence.maximum_outer_span
                    || evidence.minimum_inner_gap > evidence.maximum_inner_gap
                {
                    return Err(integrity(
                        "pair-path compatible evidence accounting is inconsistent",
                    ));
                }
                checked_add(
                    sum,
                    evidence.compatible_placement_observations,
                    "pair-path compatible observation validation overflow",
                )
            })?;
    let cap_hits = [
        ledger.placement_combination_cap_hits,
        ledger.search_depth_cap_hits,
        ledger.search_state_cap_hits,
        ledger.search_arc_cap_hits,
        ledger.path_reconstruction_cap_hits,
        ledger.target_path_cap_hits,
        ledger.compatible_path_cap_hits,
    ];
    if cap_hits.into_iter().any(|hits| hits > 1)
        || cap_hits.into_iter().sum::<u64>() > 1
        || ledger.search_states_created > limits.maximum_search_states_per_fragment
        || ledger.search_states > ledger.search_states_created
        || ledger.search_arcs_examined > limits.maximum_search_arc_examinations_per_fragment
        || ledger.path_reconstruction_elements
            > limits.maximum_path_reconstruction_elements_per_fragment
        || ledger.target_paths > limits.maximum_target_paths_per_fragment
        || compatible_count > limits.maximum_compatible_paths_per_fragment
        || ledger.cross_component_placement_pairs > ledger.placement_pairs
        || ledger.compatible_path_observations > ledger.target_paths
        || compatible_observations != ledger.compatible_path_observations
        || (ledger.placement_combination_cap_hits == 0
            && ledger.placement_pairs > limits.maximum_placement_pairs_per_fragment)
        || decision
            .compatible_paths
            .windows(2)
            .any(|window| window[0].path >= window[1].path)
    {
        return Err(integrity(
            "pair-path decision work accounting is inconsistent",
        ));
    }
    let status_reason_valid = match decision.status {
        PairDecisionStatus::SupportedUniqueExistingPath => {
            decision.reason.is_none()
                && decision.compatible_paths.len() == 1
                && !decision.compatible_paths[0].path.link_indices.is_empty()
                && no_adverse_evidence(ledger)
        }
        PairDecisionStatus::TrivialWithinUnitig => {
            decision.reason.is_none()
                && decision.compatible_paths.len() == 1
                && decision.compatible_paths[0].path.link_indices.is_empty()
                && no_adverse_evidence(ledger)
        }
        PairDecisionStatus::Abstained => decision.reason.is_some(),
        PairDecisionStatus::Indeterminate => {
            matches!(
                decision.reason,
                Some(
                    AbstentionReason::PlacementCombinationLimit
                        | AbstentionReason::SearchDepthLimit
                        | AbstentionReason::SearchStateLimit
                        | AbstentionReason::SearchArcWorkLimit
                        | AbstentionReason::PathReconstructionWorkLimit
                        | AbstentionReason::SearchPathCountLimit
                        | AbstentionReason::CompatiblePathCountLimit
                )
            )
        }
    };
    let reason_ledger_valid = match decision.reason {
        None => true,
        Some(AbstentionReason::LaneModelUnavailable)
        | Some(AbstentionReason::NoPlacement)
        | Some(AbstentionReason::JunctionSpanningPlacementUnavailable) => true,
        Some(AbstentionReason::PlacementCombinationLimit) => {
            ledger.placement_combination_cap_hits == 1
        }
        Some(AbstentionReason::CrossComponent) => ledger.cross_component_placement_pairs != 0,
        Some(AbstentionReason::NonLinearTopology) => ledger.non_linear_topology_conflicts != 0,
        Some(AbstentionReason::SearchDepthLimit) => ledger.search_depth_cap_hits == 1,
        Some(AbstentionReason::SearchStateLimit) => ledger.search_state_cap_hits == 1,
        Some(AbstentionReason::SearchArcWorkLimit) => ledger.search_arc_cap_hits == 1,
        Some(AbstentionReason::PathReconstructionWorkLimit) => {
            ledger.path_reconstruction_cap_hits == 1
        }
        Some(AbstentionReason::SearchPathCountLimit) => ledger.target_path_cap_hits == 1,
        Some(AbstentionReason::CompatiblePathCountLimit) => ledger.compatible_path_cap_hits == 1,
        Some(AbstentionReason::NoCompatiblePath) => decision.compatible_paths.is_empty(),
        Some(AbstentionReason::OrientationConflict) => ledger.orientation_conflicts != 0,
        Some(AbstentionReason::GeometryConflict) => {
            ledger.geometry_conflicts != 0
                || ledger.geometry_pruned_searches != 0
                || ledger.geometry_pruned_extensions != 0
        }
        Some(AbstentionReason::MultipleCompatiblePaths) => decision.compatible_paths.len() > 1,
    };
    if !status_reason_valid
        || !reason_ledger_valid
        || (decision.aggregate_attribution_scope == AggregateAttributionScope::GlobalIndeterminate
            && decision.status != PairDecisionStatus::Indeterminate)
    {
        return Err(integrity(
            "pair-path decision status is inconsistent with its evidence",
        ));
    }
    Ok(())
}

const fn no_adverse_evidence(ledger: ReplayLedger) -> bool {
    ledger.cross_component_placement_pairs == 0
        && ledger.orientation_conflicts == 0
        && ledger.geometry_conflicts == 0
        && ledger.non_linear_topology_conflicts == 0
        && ledger.geometry_pruned_searches == 0
        && ledger.geometry_pruned_extensions == 0
        && ledger.placement_combination_cap_hits == 0
        && ledger.search_depth_cap_hits == 0
        && ledger.search_state_cap_hits == 0
        && ledger.search_arc_cap_hits == 0
        && ledger.path_reconstruction_cap_hits == 0
        && ledger.target_path_cap_hits == 0
        && ledger.compatible_path_cap_hits == 0
}

fn hash_pair_path_config(hasher: &mut Sha256, config: PairPathConfig) {
    let model = config.model;
    for value in [
        model.minimum_anchors,
        model.minimum_dominant_anchors,
        model.dominance_numerator,
        model.dominance_denominator,
        model.maximum_span_p90_minus_p10,
        model.maximum_inner_p90_minus_p10,
    ] {
        hasher.update(value.to_le_bytes());
    }
    let limits = config.limits;
    for value in [
        limits.maximum_pairs,
        limits.maximum_placements,
        limits.maximum_placement_pairs_per_fragment,
        u64::from(limits.maximum_edges_per_path),
        limits.maximum_search_states_per_fragment,
        limits.maximum_search_arc_examinations_per_fragment,
        limits.maximum_path_reconstruction_elements_per_fragment,
        limits.maximum_target_paths_per_fragment,
        limits.maximum_compatible_paths_per_fragment,
        u64::from(limits.maximum_worker_threads),
        limits.maximum_mapper_algorithm_bytes,
        limits.maximum_mapper_version_bytes,
        limits.maximum_mapper_parameter_bytes,
        limits.maximum_graph_sequence_bases,
        limits.graph_memory_bytes,
        limits.search_memory_bytes_per_worker,
        limits.result_memory_bytes,
        limits.analysis_memory_bytes,
        config.minimum_distinct_fragments_for_path,
    ] {
        hasher.update(value.to_le_bytes());
    }
}

fn hash_input_counts(hasher: &mut Sha256, counts: PlacementInputCounts) {
    for value in [
        counts.fragment_instances,
        counts.placement_groups,
        counts.placements,
        counts.junction_spanning_reads_unavailable,
    ] {
        hasher.update(value.to_le_bytes());
    }
}

fn hash_frozen_models(hasher: &mut Sha256, models: &FrozenLibraryModels) -> Result<()> {
    hasher.update([placement_domain_tag(models.placement_domain)]);
    hash_length_prefixed(hasher, models.quantile_rule.as_bytes())?;
    for value in [
        models.config.minimum_anchors,
        models.config.minimum_dominant_anchors,
        models.config.dominance_numerator,
        models.config.dominance_denominator,
        models.config.maximum_span_p90_minus_p10,
        models.config.maximum_inner_p90_minus_p10,
    ] {
        hasher.update(value.to_le_bytes());
    }
    hasher.update(usize_to_u64(models.lanes.len(), "pair-path result lane count")?.to_le_bytes());
    for lane in &models.lanes {
        hasher.update(lane.lane_ordinal.to_le_bytes());
        hash_anchor_ledger(hasher, lane.ledger);
        for count in lane.orientation_counts {
            hasher.update(count.to_le_bytes());
        }
        match lane.dominant_orientation {
            Some(orientation) => {
                hasher.update([1, library_orientation_tag(orientation)]);
            }
            None => hasher.update([0]),
        }
        hash_optional_unsigned_distribution(hasher, lane.outer_span);
        hash_optional_signed_distribution(hasher, lane.inner_gap);
        hasher.update([lane_model_availability_tag(lane.availability)]);
    }
    Ok(())
}

fn hash_anchor_ledger(hasher: &mut Sha256, ledger: AnchorLedger) {
    for value in [
        ledger.included,
        ledger.no_placement,
        ledger.nonunique_placement,
        ledger.junction_spanning_unavailable,
        ledger.cross_component,
        ledger.different_unitig,
        ledger.non_linear_unitig,
        ledger.coincident_start,
    ] {
        hasher.update(value.to_le_bytes());
    }
}

fn hash_optional_unsigned_distribution(
    hasher: &mut Sha256,
    distribution: Option<UnsignedDistribution>,
) {
    let Some(distribution) = distribution else {
        hasher.update([0]);
        return;
    };
    hasher.update([1]);
    for value in [
        distribution.observations,
        distribution.minimum,
        distribution.p10,
        distribution.median,
        distribution.p90,
        distribution.maximum,
    ] {
        hasher.update(value.to_le_bytes());
    }
}

fn hash_optional_signed_distribution(
    hasher: &mut Sha256,
    distribution: Option<SignedDistribution>,
) {
    let Some(distribution) = distribution else {
        hasher.update([0]);
        return;
    };
    hasher.update([1]);
    hasher.update(distribution.observations.to_le_bytes());
    for value in [
        distribution.minimum,
        distribution.p10,
        distribution.median,
        distribution.p90,
        distribution.maximum,
    ] {
        hasher.update(value.to_le_bytes());
    }
}

fn hash_decision(hasher: &mut Sha256, decision: &PairPathDecision) -> Result<()> {
    hasher.update(decision.lane_ordinal.to_le_bytes());
    hasher.update(decision.fragment_ordinal.to_le_bytes());
    hasher.update(decision.fragment_identity_sha256);
    hasher.update([decision_status_tag(decision.status)]);
    match decision.reason {
        Some(reason) => hasher.update([1, abstention_reason_tag(reason)]),
        None => hasher.update([0]),
    }
    hasher.update([u8::from(decision.authorizes_reconstruction)]);
    hasher.update([attribution_scope_tag(decision.aggregate_attribution_scope)]);
    hasher.update(
        usize_to_u64(
            decision.affected_endpoint_domains.len(),
            "pair-path endpoint-domain result count",
        )?
        .to_le_bytes(),
    );
    for domain in &decision.affected_endpoint_domains {
        hasher.update(domain.first_segment_index.to_le_bytes());
        hasher.update(domain.second_segment_index.to_le_bytes());
    }
    hash_replay_ledger(hasher, decision.ledger);
    hasher.update(
        usize_to_u64(
            decision.compatible_paths.len(),
            "pair-path compatible result count",
        )?
        .to_le_bytes(),
    );
    for evidence in &decision.compatible_paths {
        hash_graph_path(hasher, &evidence.path)?;
        hasher.update(evidence.compatible_placement_observations.to_le_bytes());
        hasher.update(evidence.minimum_outer_span.to_le_bytes());
        hasher.update(evidence.maximum_outer_span.to_le_bytes());
        hasher.update(evidence.minimum_inner_gap.to_le_bytes());
        hasher.update(evidence.maximum_inner_gap.to_le_bytes());
    }
    Ok(())
}

fn hash_replay_ledger(hasher: &mut Sha256, ledger: ReplayLedger) {
    for value in [
        ledger.placement_group_pairs,
        ledger.placement_pairs,
        ledger.cross_component_placement_pairs,
        ledger.search_states_created,
        ledger.search_states,
        ledger.search_arcs_examined,
        ledger.path_reconstruction_elements,
        ledger.target_paths,
        ledger.orientation_conflicts,
        ledger.geometry_conflicts,
        ledger.non_linear_topology_conflicts,
        ledger.compatible_path_observations,
        ledger.geometry_pruned_searches,
        ledger.geometry_pruned_extensions,
        ledger.placement_combination_cap_hits,
        ledger.search_depth_cap_hits,
        ledger.search_state_cap_hits,
        ledger.search_arc_cap_hits,
        ledger.path_reconstruction_cap_hits,
        ledger.target_path_cap_hits,
        ledger.compatible_path_cap_hits,
    ] {
        hasher.update(value.to_le_bytes());
    }
}

fn hash_graph_path(hasher: &mut Sha256, path: &CanonicalGraphPath) -> Result<()> {
    hasher.update(usize_to_u64(path.handles.len(), "pair-path result handle count")?.to_le_bytes());
    for handle in &path.handles {
        hasher.update(handle.segment_index.to_le_bytes());
        hasher.update([direction_tag(handle.direction)]);
    }
    hasher.update(
        usize_to_u64(path.link_indices.len(), "pair-path result link count")?.to_le_bytes(),
    );
    for link in &path.link_indices {
        hasher.update(link.to_le_bytes());
    }
    Ok(())
}

/// Validate and hash a public placement interchange record. The root detects
/// mutation and inconsistent certificate fields; it does not authenticate the
/// caller or source. See [`AuthenticatedPlacementEvidencePair`].
pub fn placement_input_root_sha256(
    graph: &PairPathGraph,
    input: &PlacementEvidenceInput,
    limits: WorkLimits,
) -> Result<[u8; 32]> {
    validate_evidence_input(graph, input, limits).map(|(root, _)| root)
}

fn placement_input_root_after_validation(
    graph: &PairPathGraph,
    input: &PlacementEvidenceInput,
    limits: WorkLimits,
) -> Result<[u8; 32]> {
    let mapper_identity = mapper_identity_sha256(&input.mapper, limits)?;
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(input.pairs.len())
        .map_err(|_| resource_error("cannot allocate canonical placement-input pair index"))?;
    ordered.extend(input.pairs.iter());
    ordered.sort_unstable_by_key(|pair| {
        (
            pair.lane_ordinal,
            pair.fragment_ordinal,
            pair.fragment_identity_sha256,
        )
    });
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-path-placement-input:v2\0");
    hasher.update(graph.graph_root_sha256());
    hasher.update(input.certificate.graph_root_sha256);
    hasher.update(input.library_source_root_sha256);
    hasher.update(input.certificate.library_source_root_sha256);
    hasher.update(input.read_set_root_sha256);
    hasher.update(input.certificate.read_set_root_sha256);
    hasher.update(mapper_identity);
    hasher.update(input.mapper_identity_sha256);
    hasher.update(input.certificate.mapper_identity_sha256);
    hasher.update([placement_domain_tag(input.placement_domain)]);
    hasher.update([placement_domain_tag(input.certificate.placement_domain)]);
    hasher.update([u8::from(input.certificate.complete_within_declared_domain)]);
    hasher.update(input.certificate.supplied_fragment_instances.to_le_bytes());
    hasher.update(usize_to_u64(ordered.len(), "placement-input pair count")?.to_le_bytes());
    for pair in ordered {
        hasher.update(pair.lane_ordinal.to_le_bytes());
        hasher.update(pair.fragment_ordinal.to_le_bytes());
        hasher.update(pair.fragment_identity_sha256);
        hasher.update(pair.r1_read_sha256);
        hasher.update(pair.r2_read_sha256);
        hasher.update([pair.junction_spanning_reads_unavailable]);
        hash_groups(&mut hasher, &pair.r1_groups)?;
        hash_groups(&mut hasher, &pair.r2_groups)?;
    }
    Ok(hasher.finalize().into())
}

fn freeze_models(
    graph: &PairPathGraph,
    pairs: &[PairPlacementEvidence],
    config: ModelConfig,
) -> Result<FrozenLibraryModels> {
    let mut lane_ordinals = Vec::new();
    lane_ordinals
        .try_reserve_exact(pairs.len())
        .map_err(|_| resource_error("cannot allocate pair-path lane ordinals"))?;
    lane_ordinals.extend(pairs.iter().map(|pair| pair.lane_ordinal));
    lane_ordinals.sort_unstable();
    lane_ordinals.dedup();
    let mut lanes = Vec::new();
    lanes
        .try_reserve_exact(lane_ordinals.len())
        .map_err(|_| resource_error("cannot allocate pair-path lane accumulators"))?;
    lanes.extend(
        lane_ordinals
            .into_iter()
            .map(|ordinal| (ordinal, LaneAccumulator::default())),
    );
    for pair in pairs {
        let lane_index = lanes
            .binary_search_by_key(&pair.lane_ordinal, |(ordinal, _)| *ordinal)
            .map_err(|_| invariant("pre-registered pair-path lane disappeared"))?;
        let lane = &mut lanes
            .get_mut(lane_index)
            .ok_or_else(|| invariant("pair-path lane index is out of range"))?
            .1;
        let disposition = anchor_geometry(graph, pair)?;
        let counter = match disposition {
            AnchorDisposition::Included {
                orientation,
                span,
                inner,
            } => {
                lane.spans[orientation.index()]
                    .try_reserve(1)
                    .map_err(|_| resource_error("cannot allocate pair-path model span"))?;
                lane.inners[orientation.index()]
                    .try_reserve(1)
                    .map_err(|_| resource_error("cannot allocate pair-path model inner gap"))?;
                lane.spans[orientation.index()].push(span);
                lane.inners[orientation.index()].push(inner);
                &mut lane.ledger.included
            }
            AnchorDisposition::NoPlacement => &mut lane.ledger.no_placement,
            AnchorDisposition::Nonunique => &mut lane.ledger.nonunique_placement,
            AnchorDisposition::JunctionUnavailable => {
                &mut lane.ledger.junction_spanning_unavailable
            }
            AnchorDisposition::CrossComponent => &mut lane.ledger.cross_component,
            AnchorDisposition::DifferentUnitig => &mut lane.ledger.different_unitig,
            AnchorDisposition::NonLinear => &mut lane.ledger.non_linear_unitig,
            AnchorDisposition::Coincident => &mut lane.ledger.coincident_start,
        };
        *counter = checked_add(*counter, 1, "pair-path anchor disposition overflow")?;
    }
    let mut frozen = Vec::new();
    frozen
        .try_reserve_exact(lanes.len())
        .map_err(|_| resource_error("cannot allocate frozen lane models"))?;
    for (lane_ordinal, mut lane) in lanes {
        for values in &mut lane.spans {
            values.sort_unstable();
        }
        for values in &mut lane.inners {
            values.sort_unstable();
        }
        let counts = [
            usize_to_u64(lane.spans[0].len(), "FR anchor count")?,
            usize_to_u64(lane.spans[1].len(), "RF anchor count")?,
            usize_to_u64(lane.spans[2].len(), "FF anchor count")?,
            usize_to_u64(lane.spans[3].len(), "RR anchor count")?,
        ];
        let counted_anchors = counts.into_iter().try_fold(0u64, |sum, count| {
            checked_add(sum, count, "pair-path orientation-count sum overflow")
        })?;
        if counted_anchors != lane.ledger.included {
            return Err(invariant(
                "pair-path orientations do not reconcile with included anchors",
            ));
        }
        let maximum = *counts.iter().max().unwrap_or(&0);
        let mut leader = None;
        let mut tied = false;
        for (index, count) in counts.iter().copied().enumerate() {
            if count == maximum && maximum != 0 && leader.replace(index).is_some() {
                tied = true;
            }
        }
        let dominant = if tied {
            None
        } else {
            leader.map(orientation_from_index)
        };
        let span = dominant
            .map(|value| summarize_unsigned(&lane.spans[value.index()]))
            .transpose()?
            .flatten();
        let inner = dominant
            .map(|value| summarize_signed(&lane.inners[value.index()]))
            .transpose()?
            .flatten();
        let dominance = dominant.is_some()
            && ratio_at_least(
                maximum,
                lane.ledger.included,
                config.dominance_numerator,
                config.dominance_denominator,
            );
        let span_width = span
            .map(|distribution| distribution.p90 - distribution.p10)
            .unwrap_or(0);
        let inner_width = inner
            .map(|distribution| i128::from(distribution.p90) - i128::from(distribution.p10))
            .unwrap_or(0);
        let availability = if lane.ledger.included < config.minimum_anchors {
            LaneModelAvailability::InsufficientAnchors
        } else if dominant.is_none() {
            LaneModelAvailability::NoUniqueDominantOrientation
        } else if maximum < config.minimum_dominant_anchors {
            LaneModelAvailability::InsufficientDominantAnchors
        } else if !dominance {
            LaneModelAvailability::DominanceBelowThreshold
        } else if span.is_none() || inner.is_none() {
            return Err(invariant(
                "dominant pair-path distribution summary is absent",
            ));
        } else if span_width > config.maximum_span_p90_minus_p10 {
            LaneModelAvailability::SpanIntervalTooWide
        } else if inner_width > i128::from(config.maximum_inner_p90_minus_p10) {
            LaneModelAvailability::InnerIntervalTooWide
        } else {
            LaneModelAvailability::Available
        };
        if lane.ledger.total()? == 0 {
            return Err(invariant("empty lane entered pair-path model"));
        }
        frozen.push(FrozenLaneModel {
            lane_ordinal,
            ledger: lane.ledger,
            orientation_counts: counts,
            dominant_orientation: dominant,
            outer_span: span,
            inner_gap: inner,
            availability,
        });
    }
    let frozen_pairs = frozen.iter().try_fold(0u64, |sum, lane| {
        checked_add(
            sum,
            lane.ledger.total()?,
            "pair-path frozen lane total overflow",
        )
    })?;
    if frozen_pairs != usize_to_u64(pairs.len(), "pair-path supplied pair count")? {
        return Err(invariant(
            "pair-path frozen lane ledgers do not reconcile with supplied pairs",
        ));
    }
    Ok(FrozenLibraryModels {
        placement_domain: PlacementDomain::LinearUnitigOnly,
        quantile_rule: "empirical_nearest_rank_p10_p50_p90",
        config,
        lanes: frozen,
    })
}

#[derive(Debug, Clone, Copy)]
enum AnchorDisposition {
    Included {
        orientation: LibraryOrientation,
        span: u64,
        inner: i64,
    },
    NoPlacement,
    Nonunique,
    JunctionUnavailable,
    CrossComponent,
    DifferentUnitig,
    NonLinear,
    Coincident,
}

fn anchor_geometry(
    graph: &PairPathGraph,
    pair: &PairPlacementEvidence,
) -> Result<AnchorDisposition> {
    if pair.junction_spanning_reads_unavailable != 0 {
        return Ok(AnchorDisposition::JunctionUnavailable);
    }
    let r1 = collect_placements(&pair.r1_groups)?;
    let r2 = collect_placements(&pair.r2_groups)?;
    if r1.is_empty() || r2.is_empty() {
        return Ok(AnchorDisposition::NoPlacement);
    }
    if pair.r1_groups.len() != 1 || pair.r2_groups.len() != 1 || r1.len() != 1 || r2.len() != 1 {
        return Ok(AnchorDisposition::Nonunique);
    }
    let left_record = graph.segment(r1[0].segment_index)?;
    let right_record = graph.segment(r2[0].segment_index)?;
    if left_record.component_index != right_record.component_index {
        return Ok(AnchorDisposition::CrossComponent);
    }
    if r1[0].segment_index != r2[0].segment_index {
        return Ok(AnchorDisposition::DifferentUnitig);
    }
    if left_record.topology != Topology::Linear {
        return Ok(AnchorDisposition::NonLinear);
    }
    if r1[0].start == r2[0].start {
        return Ok(AnchorDisposition::Coincident);
    }
    let (left, right) = if r1[0].start < r2[0].start {
        (r1[0], r2[0])
    } else {
        (r2[0], r1[0])
    };
    let span = left
        .end
        .max(right.end)
        .checked_sub(left.start)
        .ok_or_else(|| invariant("anchor span underflow"))?;
    let inner128 = i128::from(right.start) - i128::from(left.end);
    let inner =
        i64::try_from(inner128).map_err(|_| integrity("anchor inner gap does not fit i64"))?;
    Ok(AnchorDisposition::Included {
        orientation: library_orientation(left.read_strand, right.read_strand),
        span,
        inner,
    })
}

fn replay(
    graph: &PairPathGraph,
    pairs: &[PairPlacementEvidence],
    models: &FrozenLibraryModels,
    config: PairPathConfig,
    worker_threads: usize,
) -> Result<Vec<PairPathDecision>> {
    if worker_threads == 0 || worker_threads > usize::from(config.limits.maximum_worker_threads) {
        return Err(config_error(
            "pair-path worker count is zero or exceeds its configured maximum",
        ));
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_threads)
        .stack_size(WORKER_STACK_BYTES)
        .build()
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::InternalUnexpected,
                format!("create pair-path worker pool: {cause}"),
            )
        })?;
    let mut decisions: Vec<PairPathDecision> = pool.install(|| {
        pairs
            .par_iter()
            .try_fold(Vec::new, |mut batch, pair| {
                batch
                    .try_reserve(1)
                    .map_err(|_| resource_error("cannot allocate pair-path worker result"))?;
                batch.push(replay_one(graph, pair, models, config.limits)?);
                Ok(batch)
            })
            .try_reduce(Vec::new, |mut left, right| {
                left.try_reserve(right.len())
                    .map_err(|_| resource_error("cannot merge pair-path worker results"))?;
                left.extend(right);
                Ok(left)
            })
    })?;
    decisions.sort_unstable_by_key(|value| (value.lane_ordinal, value.fragment_ordinal));
    Ok(decisions)
}

#[derive(Debug, Clone, Copy)]
struct SearchState {
    handle: OrientedSegment,
    spelled_length: u64,
    parent_index: Option<usize>,
    incoming_link: Option<u32>,
    depth: u32,
}

fn replay_one(
    graph: &PairPathGraph,
    pair: &PairPlacementEvidence,
    models: &FrozenLibraryModels,
    limits: WorkLimits,
) -> Result<PairPathDecision> {
    let Some(lane) = models
        .lanes
        .binary_search_by_key(&pair.lane_ordinal, |value| value.lane_ordinal)
        .ok()
        .map(|index| &models.lanes[index])
    else {
        return Ok(decision(
            pair,
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::LaneModelUnavailable),
            ReplayLedger::default(),
            Vec::new(),
        ));
    };
    if lane.availability != LaneModelAvailability::Available {
        return Ok(decision(
            pair,
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::LaneModelUnavailable),
            ReplayLedger::default(),
            Vec::new(),
        ));
    }
    if pair.junction_spanning_reads_unavailable != 0 {
        return Ok(decision(
            pair,
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::JunctionSpanningPlacementUnavailable),
            ReplayLedger::default(),
            Vec::new(),
        ));
    }
    let r1 = collect_placements(&pair.r1_groups)?;
    let r2 = collect_placements(&pair.r2_groups)?;
    if r1.is_empty() || r2.is_empty() {
        return Ok(decision(
            pair,
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::NoPlacement),
            ReplayLedger::default(),
            Vec::new(),
        ));
    }
    let group_combinations = usize_to_u64(pair.r1_groups.len(), "R1 group count")?
        .checked_mul(usize_to_u64(pair.r2_groups.len(), "R2 group count")?)
        .ok_or_else(|| overflow("pair-path placement group pair count overflow"))?;
    let combinations = usize_to_u64(r1.len(), "R1 placement count")?
        .checked_mul(usize_to_u64(r2.len(), "R2 placement count")?)
        .ok_or_else(|| overflow("pair-path placement pair count overflow"))?;
    let mut ledger = ReplayLedger {
        placement_group_pairs: group_combinations,
        placement_pairs: combinations,
        ..ReplayLedger::default()
    };
    if combinations > limits.maximum_placement_pairs_per_fragment {
        ledger.placement_combination_cap_hits = 1;
        return Ok(decision_with_attribution(
            pair,
            PairDecisionStatus::Indeterminate,
            Some(AbstentionReason::PlacementCombinationLimit),
            AggregateAttributionScope::GlobalIndeterminate,
            Vec::new(),
            ledger,
            Vec::new(),
        ));
    }
    let affected_endpoint_domains = endpoint_domains(&r1, &r2, combinations)?;
    let non_linear_placements = r1
        .iter()
        .chain(r2.iter())
        .try_fold(0u64, |count, placement| {
            if graph.segment(placement.segment_index)?.topology == Topology::Linear {
                Ok(count)
            } else {
                checked_add(count, 1, "pair-path non-linear placement count overflow")
            }
        })?;
    if non_linear_placements != 0 {
        ledger.non_linear_topology_conflicts = non_linear_placements;
        return Ok(decision_with_attribution(
            pair,
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::NonLinearTopology),
            AggregateAttributionScope::ExactEndpointDomains,
            affected_endpoint_domains,
            ledger,
            Vec::new(),
        ));
    }
    let orientation = lane
        .dominant_orientation
        .ok_or_else(|| invariant("available lane has no orientation"))?;
    let span = lane
        .outer_span
        .ok_or_else(|| invariant("available lane has no span interval"))?;
    let inner = lane
        .inner_gap
        .ok_or_else(|| invariant("available lane has no inner interval"))?;
    ledger.placement_pairs = 0;
    let compatible_capacity = usize::try_from(limits.maximum_compatible_paths_per_fragment)
        .map_err(|_| resource_error("compatible path limit does not fit usize"))?;
    let mut compatible = Vec::new();
    compatible
        .try_reserve_exact(compatible_capacity)
        .map_err(|_| resource_error("cannot allocate compatible pair-path table"))?;
    for first in &r1 {
        for second in &r2 {
            ledger.placement_pairs =
                checked_add(ledger.placement_pairs, 1, "placement pair ledger overflow")?;
            if graph.segment(first.segment_index)?.component_index
                != graph.segment(second.segment_index)?.component_index
            {
                ledger.cross_component_placement_pairs = checked_add(
                    ledger.cross_component_placement_pairs,
                    1,
                    "cross-component pair ledger overflow",
                )?;
                continue;
            }
            for direction in [Direction::Forward, Direction::Reverse] {
                if let Some(reason) = enumerate(
                    graph,
                    first,
                    second,
                    direction,
                    orientation,
                    span,
                    inner,
                    limits,
                    &mut ledger,
                    &mut compatible,
                )? {
                    return Ok(decision_with_attribution(
                        pair,
                        PairDecisionStatus::Indeterminate,
                        Some(reason),
                        AggregateAttributionScope::ExactEndpointDomains,
                        affected_endpoint_domains,
                        ledger,
                        compatible,
                    ));
                }
            }
        }
    }
    let paths = compatible;
    let (status, reason) = if ledger.non_linear_topology_conflicts != 0 {
        (
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::NonLinearTopology),
        )
    } else if ledger.cross_component_placement_pairs != 0 {
        (
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::CrossComponent),
        )
    } else if ledger.orientation_conflicts != 0 {
        (
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::OrientationConflict),
        )
    } else if ledger.geometry_conflicts != 0
        || ledger.geometry_pruned_searches != 0
        || ledger.geometry_pruned_extensions != 0
    {
        (
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::GeometryConflict),
        )
    } else if paths.len() == 1 && paths[0].path.link_indices.is_empty() {
        (PairDecisionStatus::TrivialWithinUnitig, None)
    } else if paths.len() == 1 {
        (PairDecisionStatus::SupportedUniqueExistingPath, None)
    } else if paths.len() > 1 {
        (
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::MultipleCompatiblePaths),
        )
    } else {
        (
            PairDecisionStatus::Abstained,
            Some(AbstentionReason::NoCompatiblePath),
        )
    };
    Ok(decision_with_attribution(
        pair,
        status,
        reason,
        AggregateAttributionScope::ExactEndpointDomains,
        affected_endpoint_domains,
        ledger,
        paths,
    ))
}

#[allow(clippy::too_many_arguments)]
fn enumerate(
    graph: &PairPathGraph,
    source: &ExactPlacement,
    target: &ExactPlacement,
    source_direction: Direction,
    expected_orientation: LibraryOrientation,
    span_interval: UnsignedDistribution,
    inner_interval: SignedDistribution,
    limits: WorkLimits,
    ledger: &mut ReplayLedger,
    compatible: &mut Vec<CompatiblePathEvidence>,
) -> Result<Option<AbstentionReason>> {
    let source_anchor = orient_placement(graph, source, source_direction)?;
    let source_record = graph.segment(source.segment_index)?;
    let Some(maximum_target_unitig_start) =
        maximum_target_unitig_start(graph, target, source_anchor, span_interval, inner_interval)?
    else {
        ledger.geometry_pruned_searches = checked_add(
            ledger.geometry_pruned_searches,
            1,
            "pair-path geometry-pruned search ledger overflow",
        )?;
        return Ok(None);
    };
    if ledger.search_states_created == limits.maximum_search_states_per_fragment {
        ledger.search_state_cap_hits = checked_add(
            ledger.search_state_cap_hits,
            1,
            "pair-path state-cap hit ledger overflow",
        )?;
        return Ok(Some(AbstentionReason::SearchStateLimit));
    }
    let remaining_states = limits
        .maximum_search_states_per_fragment
        .checked_sub(ledger.search_states_created)
        .ok_or_else(|| invariant("pair-path created states exceed their cap"))?;
    let arena_capacity = usize::try_from(remaining_states)
        .map_err(|_| resource_error("pair-path remaining-state count does not fit usize"))?;
    let mut arena = Vec::new();
    arena
        .try_reserve_exact(arena_capacity)
        .map_err(|_| resource_error("cannot allocate pair-path parent-state arena"))?;
    let mut queue = VecDeque::new();
    queue
        .try_reserve(arena_capacity)
        .map_err(|_| resource_error("cannot allocate pair-path search queue"))?;
    let initial_handle = OrientedSegment {
        segment_index: source.segment_index,
        direction: source_direction,
    };
    arena.push(SearchState {
        handle: initial_handle,
        spelled_length: source_record.length,
        parent_index: None,
        incoming_link: None,
        depth: 0,
    });
    queue.push_back(0);
    ledger.search_states_created = checked_add(
        ledger.search_states_created,
        1,
        "pair-path created-state ledger overflow",
    )?;
    while let Some(state_index) = queue.pop_front() {
        if ledger.search_states == limits.maximum_search_states_per_fragment {
            ledger.search_state_cap_hits = checked_add(
                ledger.search_state_cap_hits,
                1,
                "pair-path state-cap hit ledger overflow",
            )?;
            return Ok(Some(AbstentionReason::SearchStateLimit));
        }
        ledger.search_states = checked_add(
            ledger.search_states,
            1,
            "pair-path search-state ledger overflow",
        )?;
        let state = *arena
            .get(state_index)
            .ok_or_else(|| invariant("pair-path queued state is absent from its arena"))?;
        let current = graph.segment(state.handle.segment_index)?;
        let current_start = state
            .spelled_length
            .checked_sub(current.length)
            .ok_or_else(|| invariant("pair-path spelled length is below current unitig length"))?;
        if state.handle.segment_index == target.segment_index {
            let target_anchor = orient_placement(graph, target, state.handle.direction)?;
            let target_start = current_start
                .checked_add(target_anchor.start)
                .ok_or_else(|| overflow("pair-path target start overflow"))?;
            if target_start > source_anchor.start {
                if ledger.target_paths == limits.maximum_target_paths_per_fragment {
                    ledger.target_path_cap_hits = checked_add(
                        ledger.target_path_cap_hits,
                        1,
                        "pair-path target-path cap ledger overflow",
                    )?;
                    return Ok(Some(AbstentionReason::SearchPathCountLimit));
                }
                ledger.target_paths = checked_add(
                    ledger.target_paths,
                    1,
                    "pair-path target-path ledger overflow",
                )?;
                let path_orientation =
                    library_orientation(source_anchor.strand, target_anchor.strand);
                if path_orientation != expected_orientation {
                    ledger.orientation_conflicts = checked_add(
                        ledger.orientation_conflicts,
                        1,
                        "pair-path orientation-conflict ledger overflow",
                    )?;
                } else {
                    let target_end = current_start
                        .checked_add(target_anchor.end)
                        .ok_or_else(|| overflow("pair-path target end overflow"))?;
                    let outer = source_anchor
                        .end
                        .max(target_end)
                        .checked_sub(source_anchor.start)
                        .ok_or_else(|| invariant("pair-path outer span underflow"))?;
                    let inner128 = i128::from(target_start) - i128::from(source_anchor.end);
                    let inner = i64::try_from(inner128)
                        .map_err(|_| resource_error("pair-path inner gap does not fit i64"))?;
                    if outer < span_interval.p10
                        || outer > span_interval.p90
                        || inner < inner_interval.p10
                        || inner > inner_interval.p90
                    {
                        ledger.geometry_conflicts = checked_add(
                            ledger.geometry_conflicts,
                            1,
                            "pair-path geometry-conflict ledger overflow",
                        )?;
                    } else {
                        let Some(key) = reconstruct_canonical_path(
                            &arena,
                            state_index,
                            compatible.len(),
                            limits,
                            ledger,
                        )?
                        else {
                            return Ok(Some(AbstentionReason::PathReconstructionWorkLimit));
                        };
                        let entry = match compatible
                            .binary_search_by(|evidence| evidence.path.cmp(&key))
                        {
                            Ok(index) => compatible
                                .get_mut(index)
                                .ok_or_else(|| invariant("compatible path index disappeared"))?,
                            Err(insertion_index) => {
                                if usize_to_u64(compatible.len(), "compatible path count")?
                                    >= limits.maximum_compatible_paths_per_fragment
                                {
                                    ledger.compatible_path_cap_hits = checked_add(
                                        ledger.compatible_path_cap_hits,
                                        1,
                                        "pair-path compatible-path cap ledger overflow",
                                    )?;
                                    return Ok(Some(AbstentionReason::CompatiblePathCountLimit));
                                }
                                compatible.insert(
                                    insertion_index,
                                    CompatiblePathEvidence {
                                        path: key,
                                        compatible_placement_observations: 0,
                                        minimum_outer_span: outer,
                                        maximum_outer_span: outer,
                                        minimum_inner_gap: inner,
                                        maximum_inner_gap: inner,
                                    },
                                );
                                compatible.get_mut(insertion_index).ok_or_else(|| {
                                    invariant("inserted compatible pair path is absent")
                                })?
                            }
                        };
                        ledger.compatible_path_observations = checked_add(
                            ledger.compatible_path_observations,
                            1,
                            "pair-path compatible-observation ledger overflow",
                        )?;
                        entry.compatible_placement_observations = checked_add(
                            entry.compatible_placement_observations,
                            1,
                            "compatible path observation count overflow",
                        )?;
                        entry.minimum_outer_span = entry.minimum_outer_span.min(outer);
                        entry.maximum_outer_span = entry.maximum_outer_span.max(outer);
                        entry.minimum_inner_gap = entry.minimum_inner_gap.min(inner);
                        entry.maximum_inner_gap = entry.maximum_inner_gap.max(inner);
                    }
                }
            }
        }
        let edges = state.depth;
        let arcs = graph.arcs(state.handle)?;
        if edges == limits.maximum_edges_per_path {
            let overlap = u64::from(graph.k - 1);
            let can_extend = !arcs.is_empty()
                && state
                    .spelled_length
                    .checked_sub(overlap)
                    .is_some_and(|next_start| next_start <= maximum_target_unitig_start);
            if can_extend {
                ledger.search_depth_cap_hits = checked_add(
                    ledger.search_depth_cap_hits,
                    1,
                    "pair-path depth-cap hit ledger overflow",
                )?;
                return Ok(Some(AbstentionReason::SearchDepthLimit));
            }
            continue;
        }
        for arc in arcs {
            if ledger.search_arcs_examined == limits.maximum_search_arc_examinations_per_fragment {
                ledger.search_arc_cap_hits = checked_add(
                    ledger.search_arc_cap_hits,
                    1,
                    "pair-path arc-work cap hit ledger overflow",
                )?;
                return Ok(Some(AbstentionReason::SearchArcWorkLimit));
            }
            ledger.search_arcs_examined = checked_add(
                ledger.search_arcs_examined,
                1,
                "pair-path examined-arc ledger overflow",
            )?;
            let next_start = state
                .spelled_length
                .checked_sub(u64::from(graph.k - 1))
                .ok_or_else(|| invariant("pair-path current unitig is shorter than overlap"))?;
            if next_start > maximum_target_unitig_start {
                ledger.geometry_pruned_extensions = checked_add(
                    ledger.geometry_pruned_extensions,
                    1,
                    "pair-path geometry-pruned extension ledger overflow",
                )?;
                continue;
            }
            let next = graph.segment(arc.to.segment_index)?;
            if next.topology != Topology::Linear {
                ledger.non_linear_topology_conflicts = checked_add(
                    ledger.non_linear_topology_conflicts,
                    1,
                    "pair-path non-linear traversal count overflow",
                )?;
                continue;
            }
            let contribution = next
                .length
                .checked_sub(u64::from(graph.k - 1))
                .ok_or_else(|| invariant("pair-path next unitig is shorter than k-1 overlap"))?;
            let spelled_length = state
                .spelled_length
                .checked_add(contribution)
                .ok_or_else(|| overflow("pair-path spelled length overflow"))?;
            if ledger.search_states_created == limits.maximum_search_states_per_fragment {
                ledger.search_state_cap_hits = checked_add(
                    ledger.search_state_cap_hits,
                    1,
                    "pair-path state-cap hit ledger overflow",
                )?;
                return Ok(Some(AbstentionReason::SearchStateLimit));
            }
            let next_index = arena.len();
            arena.push(SearchState {
                handle: arc.to,
                spelled_length,
                parent_index: Some(state_index),
                incoming_link: Some(arc.link_index),
                depth: state
                    .depth
                    .checked_add(1)
                    .ok_or_else(|| overflow("pair-path state depth overflow"))?,
            });
            queue.push_back(next_index);
            ledger.search_states_created = checked_add(
                ledger.search_states_created,
                1,
                "pair-path created-state ledger overflow",
            )?;
        }
    }
    Ok(None)
}

#[derive(Clone, Copy)]
struct OrientedPlacement {
    start: u64,
    end: u64,
    strand: Direction,
}

fn orient_placement(
    graph: &PairPathGraph,
    placement: &ExactPlacement,
    direction: Direction,
) -> Result<OrientedPlacement> {
    let length = graph.segment(placement.segment_index)?.length;
    match direction {
        Direction::Forward => Ok(OrientedPlacement {
            start: placement.start,
            end: placement.end,
            strand: placement.read_strand,
        }),
        Direction::Reverse => Ok(OrientedPlacement {
            start: length
                .checked_sub(placement.end)
                .ok_or_else(|| integrity("reverse placement end exceeds unitig"))?,
            end: length
                .checked_sub(placement.start)
                .ok_or_else(|| integrity("reverse placement start exceeds unitig"))?,
            strand: placement.read_strand.flip(),
        }),
    }
}

fn maximum_target_unitig_start(
    graph: &PairPathGraph,
    target: &ExactPlacement,
    source: OrientedPlacement,
    span: UnsignedDistribution,
    inner: SignedDistribution,
) -> Result<Option<u64>> {
    let source_read_span = source
        .end
        .checked_sub(source.start)
        .ok_or_else(|| invariant("oriented source placement is reversed"))?;
    if source_read_span > span.p90 {
        return Ok(None);
    }
    let outer_right = i128::from(source.start) + i128::from(span.p90);
    let inner_right = i128::from(source.end) + i128::from(inner.p90);
    let mut maximum = None;
    for direction in [Direction::Forward, Direction::Reverse] {
        let target = orient_placement(graph, target, direction)?;
        let by_outer = outer_right - i128::from(target.end);
        let by_inner = inner_right - i128::from(target.start);
        let bound = by_outer.min(by_inner);
        if bound >= 0 {
            let bound = u64::try_from(bound)
                .map_err(|_| overflow("pair-path geometry bound does not fit u64"))?;
            maximum = Some(maximum.map_or(bound, |current: u64| current.max(bound)));
        }
    }
    Ok(maximum)
}

fn validate_mapper(mapper: &MapperProvenance, limits: WorkLimits) -> Result<()> {
    if mapper.algorithm_id.is_empty() || mapper.algorithm_version.is_empty() {
        return Err(integrity("mapper algorithm identity is empty"));
    }
    if usize_to_u64(mapper.algorithm_id.len(), "mapper algorithm length")?
        > limits.maximum_mapper_algorithm_bytes
        || usize_to_u64(mapper.algorithm_version.len(), "mapper version length")?
            > limits.maximum_mapper_version_bytes
        || usize_to_u64(mapper.parameters.len(), "mapper parameter length")?
            > limits.maximum_mapper_parameter_bytes
    {
        return Err(resource_error(
            "mapper provenance exceeds its declared byte limits",
        ));
    }
    Ok(())
}

fn validate_declared_evidence_pair(
    calibration: &PlacementEvidenceInput,
    replay: &PlacementEvidenceInput,
) -> Result<()> {
    for input in [calibration, replay] {
        let mapper_identity = mapper_identity_after_validation(&input.mapper)?;
        if input.certificate.library_source_root_sha256 != input.library_source_root_sha256
            || input.certificate.read_set_root_sha256 != input.read_set_root_sha256
            || input.mapper_identity_sha256 != mapper_identity
            || input.certificate.mapper_identity_sha256 != mapper_identity
            || input.placement_domain != PlacementDomain::LinearUnitigOnly
            || input.certificate.placement_domain != input.placement_domain
            || !input.certificate.complete_within_declared_domain
            || input.certificate.supplied_fragment_instances
                != usize_to_u64(input.pairs.len(), "certified fragment count")?
        {
            return Err(integrity(
                "mapper-produced placement input metadata is inconsistent",
            ));
        }
    }
    if calibration.certificate.graph_root_sha256 != replay.certificate.graph_root_sha256
        || calibration.library_source_root_sha256 != replay.library_source_root_sha256
        || calibration.mapper != replay.mapper
        || calibration.mapper_identity_sha256 != replay.mapper_identity_sha256
        || calibration.placement_domain != replay.placement_domain
    {
        return Err(integrity(
            "mapper-produced calibration and replay evidence have different lineage or semantics",
        ));
    }
    Ok(())
}

fn validate_evidence_input(
    graph: &PairPathGraph,
    input: &PlacementEvidenceInput,
    limits: WorkLimits,
) -> Result<([u8; 32], PlacementInputCounts)> {
    validate_mapper(&input.mapper, limits)?;
    let mapper_identity = mapper_identity_sha256(&input.mapper, limits)?;
    let certificate = &input.certificate;
    if certificate.graph_root_sha256 != graph.graph_root_sha256()
        || certificate.library_source_root_sha256 != input.library_source_root_sha256
        || certificate.read_set_root_sha256 != input.read_set_root_sha256
        || input.mapper_identity_sha256 != mapper_identity
        || certificate.mapper_identity_sha256 != mapper_identity
        || input.placement_domain != PlacementDomain::LinearUnitigOnly
        || certificate.placement_domain != input.placement_domain
        || !certificate.complete_within_declared_domain
        || certificate.supplied_fragment_instances
            != usize_to_u64(input.pairs.len(), "certified fragment count")?
    {
        return Err(integrity(
            "placement input does not match its complete linear-unitig enumeration certificate",
        ));
    }
    let counts = validate_pairs(graph, &input.pairs, limits)?;
    let root = placement_input_root_after_validation(graph, input, limits)?;
    Ok((root, counts))
}

fn calibration_reuse_telemetry(
    calibration: &PlacementEvidenceInput,
    replay: &PlacementEvidenceInput,
) -> Result<CalibrationReuseTelemetry> {
    let mut calibration_ids = Vec::new();
    calibration_ids
        .try_reserve_exact(calibration.pairs.len())
        .map_err(|_| resource_error("cannot allocate calibration fragment identity index"))?;
    calibration_ids.extend(
        calibration
            .pairs
            .iter()
            .map(|pair| pair.fragment_identity_sha256),
    );
    calibration_ids.sort_unstable();
    let mut replay_ids = Vec::new();
    replay_ids
        .try_reserve_exact(replay.pairs.len())
        .map_err(|_| resource_error("cannot allocate replay fragment identity index"))?;
    replay_ids.extend(
        replay
            .pairs
            .iter()
            .map(|pair| pair.fragment_identity_sha256),
    );
    replay_ids.sort_unstable();
    let (mut left, mut right, mut reused) = (0usize, 0usize, 0u64);
    while left < calibration_ids.len() && right < replay_ids.len() {
        match calibration_ids[left].cmp(&replay_ids[right]) {
            std::cmp::Ordering::Less => left += 1,
            std::cmp::Ordering::Greater => right += 1,
            std::cmp::Ordering::Equal => {
                reused = checked_add(reused, 1, "calibration-reuse count overflow")?;
                left += 1;
                right += 1;
            }
        }
    }
    Ok(CalibrationReuseTelemetry {
        calibration_fragment_instances: usize_to_u64(
            calibration.pairs.len(),
            "calibration fragment count",
        )?,
        replay_fragment_instances: usize_to_u64(replay.pairs.len(), "replay fragment count")?,
        reused_fragment_instances: reused,
    })
}

fn endpoint_domains(
    r1: &[&ExactPlacement],
    r2: &[&ExactPlacement],
    combinations: u64,
) -> Result<Vec<EndpointDomain>> {
    let capacity = usize::try_from(combinations)
        .map_err(|_| resource_error("pair-path endpoint-domain count does not fit usize"))?;
    let mut domains = Vec::new();
    domains
        .try_reserve_exact(capacity)
        .map_err(|_| resource_error("cannot allocate pair-path endpoint domains"))?;
    for first in r1 {
        for second in r2 {
            let (first_segment_index, second_segment_index) =
                if first.segment_index <= second.segment_index {
                    (first.segment_index, second.segment_index)
                } else {
                    (second.segment_index, first.segment_index)
                };
            domains.push(EndpointDomain {
                first_segment_index,
                second_segment_index,
            });
        }
    }
    domains.sort_unstable();
    domains.dedup();
    Ok(domains)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AggregateObservationKind {
    Support,
    Contradiction,
    Indeterminate,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AggregateObservation<'a> {
    path: &'a CanonicalGraphPath,
    fragment_identity_sha256: [u8; 32],
    kind: AggregateObservationKind,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DomainBlocker {
    domain: EndpointDomain,
    fragment_identity_sha256: [u8; 32],
    kind: AggregateObservationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DomainBlockerCounts {
    domain: EndpointDomain,
    contradictions: u64,
    indeterminate: u64,
}

fn aggregate_path_evidence(
    decisions: &[PairPathDecision],
    minimum_distinct_fragments: u64,
) -> Result<Vec<AggregatePathEvidence>> {
    let observation_count = decisions.iter().try_fold(0usize, |sum, decision| {
        sum.checked_add(decision.compatible_paths.len())
            .ok_or_else(|| overflow("aggregate path observation count overflow"))
    })?;
    let mut observations = Vec::new();
    observations
        .try_reserve_exact(observation_count)
        .map_err(|_| resource_error("cannot allocate aggregate path observations"))?;
    let blocker_count = decisions.iter().try_fold(0usize, |sum, decision| {
        sum.checked_add(decision.affected_endpoint_domains.len())
            .ok_or_else(|| overflow("aggregate endpoint blocker count overflow"))
    })?;
    let mut blockers = Vec::new();
    blockers
        .try_reserve_exact(blocker_count)
        .map_err(|_| resource_error("cannot allocate aggregate endpoint blockers"))?;
    let mut global_indeterminate = Vec::new();
    global_indeterminate
        .try_reserve_exact(decisions.len())
        .map_err(|_| resource_error("cannot allocate global aggregate uncertainty"))?;
    for decision in decisions {
        let kind = match decision.status {
            PairDecisionStatus::SupportedUniqueExistingPath => AggregateObservationKind::Support,
            PairDecisionStatus::TrivialWithinUnitig => continue,
            PairDecisionStatus::Abstained => AggregateObservationKind::Contradiction,
            PairDecisionStatus::Indeterminate => AggregateObservationKind::Indeterminate,
        };
        match (kind, decision.aggregate_attribution_scope) {
            (AggregateObservationKind::Support, _) => {}
            (_, AggregateAttributionScope::ExactEndpointDomains) => {
                for &domain in &decision.affected_endpoint_domains {
                    blockers.push(DomainBlocker {
                        domain,
                        fragment_identity_sha256: decision.fragment_identity_sha256,
                        kind,
                    });
                }
            }
            (
                AggregateObservationKind::Indeterminate,
                AggregateAttributionScope::GlobalIndeterminate,
            )
            | (AggregateObservationKind::Indeterminate, AggregateAttributionScope::None) => {
                global_indeterminate.push(decision.fragment_identity_sha256);
            }
            (
                AggregateObservationKind::Contradiction,
                AggregateAttributionScope::GlobalIndeterminate,
            ) => {
                // A globally ambiguous contradictory locus cannot safely be
                // attributed as a contradiction to one path. Treat it as
                // global uncertainty instead of silently dropping it.
                global_indeterminate.push(decision.fragment_identity_sha256);
            }
            (AggregateObservationKind::Contradiction, AggregateAttributionScope::None) => {}
        }
        for compatible in &decision.compatible_paths {
            if compatible.path.link_indices.is_empty() {
                continue;
            }
            observations.push(AggregateObservation {
                path: &compatible.path,
                fragment_identity_sha256: decision.fragment_identity_sha256,
                kind,
            });
        }
    }
    observations.sort_unstable();
    blockers.sort_unstable();
    global_indeterminate.sort_unstable();
    global_indeterminate.dedup();
    let mut blocker_counts = Vec::new();
    blocker_counts
        .try_reserve_exact(blockers.len())
        .map_err(|_| resource_error("cannot allocate aggregate blocker summaries"))?;
    let mut blocker_begin = 0usize;
    while blocker_begin < blockers.len() {
        let domain = blockers[blocker_begin].domain;
        let mut blocker_end = blocker_begin;
        let (mut contradictions, mut indeterminate) = (0u64, 0u64);
        while blocker_end < blockers.len() && blockers[blocker_end].domain == domain {
            let identity = blockers[blocker_end].fragment_identity_sha256;
            let mut identity_end = blocker_end + 1;
            let mut identity_kind = blockers[blocker_end].kind;
            while identity_end < blockers.len()
                && blockers[identity_end].domain == domain
                && blockers[identity_end].fragment_identity_sha256 == identity
            {
                identity_kind = identity_kind.max(blockers[identity_end].kind);
                identity_end += 1;
            }
            match identity_kind {
                AggregateObservationKind::Support => {
                    return Err(invariant("support entered aggregate endpoint blockers"));
                }
                AggregateObservationKind::Contradiction => {
                    contradictions = checked_add(
                        contradictions,
                        1,
                        "aggregate endpoint contradiction count overflow",
                    )?;
                }
                AggregateObservationKind::Indeterminate => {
                    indeterminate = checked_add(
                        indeterminate,
                        1,
                        "aggregate endpoint indeterminate count overflow",
                    )?;
                }
            }
            blocker_end = identity_end;
        }
        blocker_counts.push(DomainBlockerCounts {
            domain,
            contradictions,
            indeterminate,
        });
        blocker_begin = blocker_end;
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(observations.len())
        .map_err(|_| resource_error("cannot allocate aggregate path evidence"))?;
    let mut begin = 0usize;
    while begin < observations.len() {
        let mut end = begin + 1;
        while end < observations.len() && observations[end].path == observations[begin].path {
            end += 1;
        }
        let mut support = 0u64;
        let mut previous_identity = None;
        for observation in &observations[begin..end] {
            if previous_identity == Some(observation.fragment_identity_sha256) {
                continue;
            }
            previous_identity = Some(observation.fragment_identity_sha256);
            if observation.kind == AggregateObservationKind::Support {
                support = checked_add(support, 1, "aggregate supporting fragment overflow")?;
            }
        }
        let domain = path_endpoint_domain(observations[begin].path)?;
        let (contradiction, domain_indeterminate) = blocker_counts
            .binary_search_by_key(&domain, |counts| counts.domain)
            .ok()
            .and_then(|index| blocker_counts.get(index))
            .map_or((0, 0), |counts| {
                (counts.contradictions, counts.indeterminate)
            });
        let indeterminate = checked_add(
            domain_indeterminate,
            usize_to_u64(
                global_indeterminate.len(),
                "global aggregate indeterminate count",
            )?,
            "aggregate indeterminate fragment overflow",
        )?;
        let availability = if indeterminate != 0 {
            AggregatePathAvailability::Indeterminate
        } else if contradiction != 0 {
            AggregatePathAvailability::Contradicted
        } else if support < minimum_distinct_fragments {
            AggregatePathAvailability::BelowMinimumDistinctFragments
        } else {
            AggregatePathAvailability::AvailableForExperimentalConstraint
        };
        output.push(AggregatePathEvidence {
            path: try_clone_path(observations[begin].path)?,
            distinct_supporting_fragments: support,
            distinct_contradicting_fragments: contradiction,
            distinct_indeterminate_fragments: indeterminate,
            minimum_required_supporting_fragments: minimum_distinct_fragments,
            availability,
            available_for_experimental_constraint: availability
                == AggregatePathAvailability::AvailableForExperimentalConstraint,
        });
        begin = end;
    }
    Ok(output)
}

fn path_endpoint_domain(path: &CanonicalGraphPath) -> Result<EndpointDomain> {
    let first = path
        .handles
        .first()
        .ok_or_else(|| invariant("aggregate pair path has no first handle"))?
        .segment_index;
    let second = path
        .handles
        .last()
        .ok_or_else(|| invariant("aggregate pair path has no last handle"))?
        .segment_index;
    let (first_segment_index, second_segment_index) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    Ok(EndpointDomain {
        first_segment_index,
        second_segment_index,
    })
}

fn try_clone_path(path: &CanonicalGraphPath) -> Result<CanonicalGraphPath> {
    Ok(CanonicalGraphPath {
        handles: try_copy_exact(&path.handles, "clone aggregate pair-path handles")?,
        link_indices: try_copy_exact(&path.link_indices, "clone aggregate pair-path links")?,
    })
}

fn validate_pairs(
    graph: &PairPathGraph,
    pairs: &[PairPlacementEvidence],
    limits: WorkLimits,
) -> Result<PlacementInputCounts> {
    if usize_to_u64(pairs.len(), "pair-path pair count")? > limits.maximum_pairs {
        return Err(resource_error("pair-path supplied pair limit exceeded"));
    }
    let mut pair_keys = Vec::new();
    pair_keys
        .try_reserve_exact(pairs.len())
        .map_err(|_| resource_error("cannot allocate pair-path pair identity index"))?;
    let mut fragment_identities = Vec::new();
    fragment_identities
        .try_reserve_exact(pairs.len())
        .map_err(|_| resource_error("cannot allocate pair-path fragment identity index"))?;
    let mut counts = PlacementInputCounts {
        fragment_instances: usize_to_u64(pairs.len(), "pair-path pair count")?,
        ..PlacementInputCounts::default()
    };
    for pair in pairs {
        if pair.junction_spanning_reads_unavailable > 2 {
            return Err(integrity(
                "junction-spanning unavailable read count exceeds two",
            ));
        }
        counts.junction_spanning_reads_unavailable = checked_add(
            counts.junction_spanning_reads_unavailable,
            u64::from(pair.junction_spanning_reads_unavailable),
            "junction-spanning unavailable count overflow",
        )?;
        pair_keys.push((pair.lane_ordinal, pair.fragment_ordinal));
        fragment_identities.push(pair.fragment_identity_sha256);
        for groups in [&pair.r1_groups, &pair.r2_groups] {
            let mut expected_read_span = None;
            let group_count = usize_to_u64(groups.len(), "pair-path placement group count")?;
            if group_count > limits.maximum_placements {
                return Err(resource_error("pair-path placement group limit exceeded"));
            }
            let placement_count = groups.iter().try_fold(0usize, |sum, group| {
                sum.checked_add(group.placements.len())
                    .ok_or_else(|| overflow("pair-path placement count overflow"))
            })?;
            let placement_count_u64 = usize_to_u64(placement_count, "pair-path placement count")?;
            counts.placement_groups = checked_add(
                counts.placement_groups,
                group_count,
                "pair-path placement group count overflow",
            )?;
            if counts
                .placements
                .checked_add(placement_count_u64)
                .ok_or_else(|| overflow("pair-path total placements overflow"))?
                > limits.maximum_placements
            {
                return Err(resource_error("pair-path total placement limit exceeded"));
            }
            let mut group_ids = Vec::new();
            group_ids
                .try_reserve_exact(groups.len())
                .map_err(|_| resource_error("cannot allocate placement group identity index"))?;
            let mut placements = Vec::new();
            placements
                .try_reserve_exact(placement_count)
                .map_err(|_| resource_error("cannot allocate exact placement identity index"))?;
            for group in groups {
                group_ids.push(group.group_ordinal);
                if group.placements.is_empty() {
                    return Err(integrity("exact placement group is empty"));
                }
                for placement in &group.placements {
                    validate_placement(graph, placement)?;
                    let read_span = placement
                        .end
                        .checked_sub(placement.start)
                        .ok_or_else(|| integrity("exact placement read span underflow"))?;
                    match expected_read_span {
                        Some(expected) if expected != read_span => {
                            return Err(integrity(
                                "exact placements for one read disagree on read span",
                            ));
                        }
                        None => expected_read_span = Some(read_span),
                        Some(_) => {}
                    }
                    let key = (
                        placement.segment_index,
                        placement.start,
                        placement.end,
                        placement.read_strand,
                    );
                    placements.push(key);
                }
            }
            group_ids.sort_unstable();
            if group_ids.windows(2).any(|window| window[0] == window[1]) {
                return Err(integrity("duplicate exact placement group ordinal"));
            }
            placements.sort_unstable();
            if placements.windows(2).any(|window| window[0] == window[1]) {
                return Err(integrity("duplicate exact placement across groups"));
            }
            counts.placements = checked_add(
                counts.placements,
                placement_count_u64,
                "pair-path total placements overflow",
            )?;
        }
    }
    pair_keys.sort_unstable();
    if pair_keys.windows(2).any(|window| window[0] == window[1]) {
        return Err(integrity("duplicate pair-path lane and fragment identity"));
    }
    fragment_identities.sort_unstable();
    if fragment_identities
        .windows(2)
        .any(|window| window[0] == window[1])
    {
        return Err(integrity(
            "duplicate immutable fragment identity in placement input",
        ));
    }
    Ok(counts)
}

fn validate_placement(graph: &PairPathGraph, placement: &ExactPlacement) -> Result<()> {
    let segment = graph.segment(placement.segment_index)?;
    if placement.start >= placement.end || placement.end > segment.length {
        return Err(integrity("pair-path placement coordinates are invalid"));
    }
    Ok(())
}

fn validate_limits(config: PairPathConfig) -> Result<()> {
    let model = config.model;
    let limits = config.limits;
    if model.minimum_anchors < 10
        || model.minimum_dominant_anchors < 10
        || model.dominance_denominator == 0
        || model.dominance_numerator > model.dominance_denominator
        || u128::from(model.dominance_numerator) * 2 <= u128::from(model.dominance_denominator)
    {
        return Err(config_error(
            "pair-path model requires at least ten anchors and a dominance fraction in (1/2,1]",
        ));
    }
    if limits.maximum_pairs == 0
        || limits.maximum_placements == 0
        || limits.maximum_placement_pairs_per_fragment == 0
        || limits.maximum_edges_per_path == 0
        || limits.maximum_search_states_per_fragment == 0
        || limits.maximum_search_arc_examinations_per_fragment == 0
        || limits.maximum_path_reconstruction_elements_per_fragment == 0
        || limits.maximum_target_paths_per_fragment == 0
        || limits.maximum_compatible_paths_per_fragment == 0
        || limits.maximum_worker_threads == 0
        || limits.maximum_mapper_algorithm_bytes == 0
        || limits.maximum_mapper_version_bytes == 0
        || limits.maximum_mapper_parameter_bytes == 0
        || limits.maximum_graph_sequence_bases == 0
        || limits.graph_memory_bytes == 0
        || limits.search_memory_bytes_per_worker == 0
        || limits.result_memory_bytes == 0
        || limits.analysis_memory_bytes == 0
        || config.minimum_distinct_fragments_for_path < 2
    {
        return Err(config_error("pair-path work limits must be nonzero"));
    }
    let path_storage = 256u64
        .checked_add(
            u64::from(limits.maximum_edges_per_path)
                .checked_mul(32)
                .ok_or_else(|| overflow("pair-path path-storage estimate overflow"))?,
        )
        .ok_or_else(|| overflow("pair-path path-storage estimate overflow"))?;
    let required_search = limits
        .maximum_search_states_per_fragment
        .checked_mul(128)
        .and_then(|value| {
            value
                .checked_add(path_storage.checked_mul(2)?)?
                .checked_add(
                    limits
                        .maximum_compatible_paths_per_fragment
                        .checked_mul(path_storage)?,
                )
        })
        .ok_or_else(|| overflow("pair-path search estimate overflow"))?;
    if required_search > limits.search_memory_bytes_per_worker {
        return Err(config_error(
            "pair-path search caps exceed per-worker memory budget",
        ));
    }
    let model_and_input_index = limits
        .maximum_placements
        .checked_mul(256)
        .and_then(|value| value.checked_add(limits.maximum_pairs.checked_mul(384)?))
        .ok_or_else(|| overflow("pair-path model memory estimate overflow"))?;
    let required_results = limits
        .maximum_pairs
        .checked_mul(
            512u64
                .checked_add(
                    limits
                        .maximum_placement_pairs_per_fragment
                        .checked_mul(128)
                        .ok_or_else(|| overflow("pair-path endpoint-domain estimate overflow"))?,
                )
                .ok_or_else(|| overflow("pair-path result estimate overflow"))?
                .checked_add(
                    limits
                        .maximum_compatible_paths_per_fragment
                        .checked_mul(path_storage)
                        .and_then(|value| value.checked_mul(3))
                        .ok_or_else(|| overflow("pair-path result estimate overflow"))?,
                )
                .ok_or_else(|| overflow("pair-path result estimate overflow"))?,
        )
        .ok_or_else(|| overflow("pair-path result estimate overflow"))?
        .checked_add(model_and_input_index)
        .ok_or_else(|| overflow("pair-path combined result estimate overflow"))?;
    if required_results > limits.result_memory_bytes || limits.graph_memory_bytes < 4096 {
        return Err(config_error(
            "pair-path result or graph memory budget is too small for configured caps",
        ));
    }
    Ok(())
}

fn collect_placements(groups: &[ExactPlacementGroup]) -> Result<Vec<&ExactPlacement>> {
    let count = groups.iter().try_fold(0usize, |sum, group| {
        sum.checked_add(group.placements.len())
            .ok_or_else(|| overflow("pair-path placement count overflow"))
    })?;
    let mut placements = Vec::new();
    placements
        .try_reserve_exact(count)
        .map_err(|_| resource_error("cannot allocate pair-path placement references"))?;
    for group in groups {
        placements.extend(group.placements.iter());
    }
    placements.sort_unstable_by_key(|placement| {
        (
            placement.segment_index,
            placement.start,
            placement.end,
            placement.read_strand,
        )
    });
    Ok(placements)
}

fn library_orientation(left: Direction, right: Direction) -> LibraryOrientation {
    match (left, right) {
        (Direction::Forward, Direction::Reverse) => LibraryOrientation::Fr,
        (Direction::Reverse, Direction::Forward) => LibraryOrientation::Rf,
        (Direction::Forward, Direction::Forward) => LibraryOrientation::Ff,
        (Direction::Reverse, Direction::Reverse) => LibraryOrientation::Rr,
    }
}

fn orientation_from_index(index: usize) -> LibraryOrientation {
    [
        LibraryOrientation::Fr,
        LibraryOrientation::Rf,
        LibraryOrientation::Ff,
        LibraryOrientation::Rr,
    ][index]
}

fn summarize_unsigned(values: &[u64]) -> Result<Option<UnsignedDistribution>> {
    if values.is_empty() {
        return Ok(None);
    }
    Ok(Some(UnsignedDistribution {
        observations: usize_to_u64(values.len(), "pair-path unsigned summary count")?,
        minimum: values[0],
        p10: nearest(values, 10)?,
        median: nearest(values, 50)?,
        p90: nearest(values, 90)?,
        maximum: values[values.len() - 1],
    }))
}

fn summarize_signed(values: &[i64]) -> Result<Option<SignedDistribution>> {
    if values.is_empty() {
        return Ok(None);
    }
    Ok(Some(SignedDistribution {
        observations: usize_to_u64(values.len(), "pair-path signed summary count")?,
        minimum: values[0],
        p10: nearest(values, 10)?,
        median: nearest(values, 50)?,
        p90: nearest(values, 90)?,
        maximum: values[values.len() - 1],
    }))
}

fn nearest<T: Copy>(values: &[T], percentile: u64) -> Result<T> {
    let count = u64::try_from(values.len())
        .map_err(|_| overflow("pair-path quantile count does not fit u64"))?;
    let rank = (u128::from(count) * u128::from(percentile)).div_ceil(100);
    let index = usize::try_from(rank - 1)
        .map_err(|_| overflow("pair-path quantile index does not fit usize"))?;
    values
        .get(index)
        .copied()
        .ok_or_else(|| invariant("pair-path quantile index is out of bounds"))
}

fn ratio_at_least(part: u64, whole: u64, numerator: u64, denominator: u64) -> bool {
    whole != 0
        && u128::from(part) * u128::from(denominator) >= u128::from(whole) * u128::from(numerator)
}

fn reconstruct_canonical_path(
    arena: &[SearchState],
    terminal_index: usize,
    compatible_paths: usize,
    limits: WorkLimits,
    ledger: &mut ReplayLedger,
) -> Result<Option<CanonicalGraphPath>> {
    let terminal = arena
        .get(terminal_index)
        .ok_or_else(|| invariant("pair-path terminal state is absent from its arena"))?;
    let handle_count = u64::from(terminal.depth)
        .checked_add(1)
        .ok_or_else(|| overflow("pair-path reconstructed handle count overflow"))?;
    let raw_elements = handle_count
        .checked_add(u64::from(terminal.depth))
        .ok_or_else(|| overflow("pair-path reconstructed element count overflow"))?;
    // One reverse parent walk, one in-place order reversal, one reciprocal
    // construction, one worst-case canonical comparison, and a conservative
    // upper bound for every lexicographic comparison in the sorted compatible
    // path table lookup that follows reconstruction.
    let table_comparisons = if compatible_paths == 0 {
        0
    } else {
        u64::from(usize::BITS - compatible_paths.leading_zeros())
    };
    let passes = table_comparisons
        .checked_add(4)
        .ok_or_else(|| overflow("pair-path reconstruction pass count overflow"))?;
    let work_elements = raw_elements
        .checked_mul(passes)
        .ok_or_else(|| overflow("pair-path reconstruction work overflow"))?;
    let next_work = ledger
        .path_reconstruction_elements
        .checked_add(work_elements)
        .ok_or_else(|| overflow("pair-path reconstruction ledger overflow"))?;
    if next_work > limits.maximum_path_reconstruction_elements_per_fragment {
        ledger.path_reconstruction_cap_hits = checked_add(
            ledger.path_reconstruction_cap_hits,
            1,
            "pair-path reconstruction cap-hit ledger overflow",
        )?;
        return Ok(None);
    }
    ledger.path_reconstruction_elements = next_work;

    let handle_capacity = usize::try_from(handle_count)
        .map_err(|_| resource_error("pair-path reconstructed handle count does not fit usize"))?;
    let link_capacity = usize::try_from(terminal.depth)
        .map_err(|_| resource_error("pair-path reconstructed link count does not fit usize"))?;
    let mut handles = Vec::new();
    handles
        .try_reserve_exact(handle_capacity)
        .map_err(|_| resource_error("cannot allocate reconstructed pair-path handles"))?;
    let mut links = Vec::new();
    links
        .try_reserve_exact(link_capacity)
        .map_err(|_| resource_error("cannot allocate reconstructed pair-path links"))?;
    let mut cursor = Some(terminal_index);
    while let Some(index) = cursor {
        let state = arena
            .get(index)
            .ok_or_else(|| invariant("pair-path parent state is absent from its arena"))?;
        handles.push(state.handle);
        match (state.parent_index, state.incoming_link) {
            (Some(parent), Some(link)) => {
                links.push(link);
                cursor = Some(parent);
            }
            (None, None) => cursor = None,
            _ => return Err(invariant("pair-path parent/link state is inconsistent")),
        }
    }
    if handles.len() != handle_capacity || links.len() != link_capacity {
        return Err(invariant(
            "pair-path reconstructed depth disagrees with its parent chain",
        ));
    }
    handles.reverse();
    links.reverse();
    let mut reverse_handles = Vec::new();
    reverse_handles
        .try_reserve_exact(handles.len())
        .map_err(|_| resource_error("cannot allocate reciprocal reconstructed handles"))?;
    reverse_handles.extend(handles.iter().rev().map(|handle| handle.flip()));
    let mut reverse_links = Vec::new();
    reverse_links
        .try_reserve_exact(links.len())
        .map_err(|_| resource_error("cannot allocate reciprocal reconstructed links"))?;
    reverse_links.extend(links.iter().rev().copied());
    let forward = (handles, links);
    let reverse = (reverse_handles, reverse_links);
    let (handles, link_indices) = if reverse < forward { reverse } else { forward };
    Ok(Some(CanonicalGraphPath {
        handles,
        link_indices,
    }))
}

fn try_copy_exact<T: Copy>(values: &[T], context: &'static str) -> Result<Vec<T>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(values.len())
        .map_err(|_| resource_error(context))?;
    copy.extend_from_slice(values);
    Ok(copy)
}

fn try_clone_string(value: &str, context: &'static str) -> Result<String> {
    let mut copy = String::new();
    copy.try_reserve_exact(value.len())
        .map_err(|_| resource_error(context))?;
    copy.push_str(value);
    Ok(copy)
}

fn graph_content_root(
    k: u8,
    segments: &[GraphCatalogEntry],
    canonical_links: &[(u32, Direction, u32, Direction)],
) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:experimental-pair-path-graph:v3\0");
    hasher.update([k]);
    hasher.update(usize_to_u64(segments.len(), "pair-path graph-root unitig count")?.to_le_bytes());
    for segment in segments {
        hash_length_prefixed(&mut hasher, segment.id.as_bytes())?;
        hasher.update([segment.topology.tag()]);
        hasher.update(segment.length.to_le_bytes());
        hasher.update(segment.sequence_sha256);
    }
    hasher.update(
        usize_to_u64(canonical_links.len(), "pair-path graph-root link count")?.to_le_bytes(),
    );
    for &(from, from_direction, to, to_direction) in canonical_links {
        hasher.update(from.to_le_bytes());
        hasher.update([direction_tag(from_direction)]);
        hasher.update(to.to_le_bytes());
        hasher.update([direction_tag(to_direction)]);
    }
    Ok(hasher.finalize().into())
}

fn hash_length_prefixed(hasher: &mut Sha256, value: &[u8]) -> Result<()> {
    hasher.update(usize_to_u64(value.len(), "pair-path hash field length")?.to_le_bytes());
    hasher.update(value);
    Ok(())
}

fn hash_groups(hasher: &mut Sha256, groups: &[ExactPlacementGroup]) -> Result<()> {
    let mut ordered_groups = Vec::new();
    ordered_groups
        .try_reserve_exact(groups.len())
        .map_err(|_| resource_error("cannot allocate canonical placement group index"))?;
    ordered_groups.extend(groups.iter());
    ordered_groups.sort_unstable_by_key(|group| group.group_ordinal);
    hasher.update(usize_to_u64(ordered_groups.len(), "placement group hash count")?.to_le_bytes());
    for group in ordered_groups {
        hasher.update(group.group_ordinal.to_le_bytes());
        let mut placements = Vec::new();
        placements
            .try_reserve_exact(group.placements.len())
            .map_err(|_| resource_error("cannot allocate canonical placement hash index"))?;
        placements.extend(group.placements.iter());
        placements.sort_unstable_by_key(|placement| {
            (
                placement.segment_index,
                placement.start,
                placement.end,
                placement.read_strand,
            )
        });
        hasher.update(usize_to_u64(placements.len(), "placement hash count")?.to_le_bytes());
        for placement in placements {
            hasher.update(placement.segment_index.to_le_bytes());
            hasher.update(placement.start.to_le_bytes());
            hasher.update(placement.end.to_le_bytes());
            hasher.update([direction_tag(placement.read_strand)]);
        }
    }
    Ok(())
}

const fn placement_domain_tag(domain: PlacementDomain) -> u8 {
    match domain {
        PlacementDomain::LinearUnitigOnly => 0,
    }
}

const fn direction_tag(direction: Direction) -> u8 {
    match direction {
        Direction::Forward => 0,
        Direction::Reverse => 1,
    }
}

const fn library_orientation_tag(orientation: LibraryOrientation) -> u8 {
    orientation.index() as u8
}

const fn lane_model_availability_tag(availability: LaneModelAvailability) -> u8 {
    match availability {
        LaneModelAvailability::Available => 0,
        LaneModelAvailability::InsufficientAnchors => 1,
        LaneModelAvailability::NoUniqueDominantOrientation => 2,
        LaneModelAvailability::InsufficientDominantAnchors => 3,
        LaneModelAvailability::DominanceBelowThreshold => 4,
        LaneModelAvailability::SpanIntervalTooWide => 5,
        LaneModelAvailability::InnerIntervalTooWide => 6,
    }
}

const fn abstention_reason_tag(reason: AbstentionReason) -> u8 {
    match reason {
        AbstentionReason::LaneModelUnavailable => 0,
        AbstentionReason::NoPlacement => 1,
        AbstentionReason::JunctionSpanningPlacementUnavailable => 2,
        AbstentionReason::PlacementCombinationLimit => 3,
        AbstentionReason::CrossComponent => 4,
        AbstentionReason::NonLinearTopology => 5,
        AbstentionReason::SearchDepthLimit => 6,
        AbstentionReason::SearchStateLimit => 7,
        AbstentionReason::SearchArcWorkLimit => 8,
        AbstentionReason::PathReconstructionWorkLimit => 9,
        AbstentionReason::SearchPathCountLimit => 10,
        AbstentionReason::CompatiblePathCountLimit => 11,
        AbstentionReason::NoCompatiblePath => 12,
        AbstentionReason::OrientationConflict => 13,
        AbstentionReason::GeometryConflict => 14,
        AbstentionReason::MultipleCompatiblePaths => 15,
    }
}

const fn decision_status_tag(status: PairDecisionStatus) -> u8 {
    match status {
        PairDecisionStatus::SupportedUniqueExistingPath => 0,
        PairDecisionStatus::TrivialWithinUnitig => 1,
        PairDecisionStatus::Abstained => 2,
        PairDecisionStatus::Indeterminate => 3,
    }
}

const fn attribution_scope_tag(scope: AggregateAttributionScope) -> u8 {
    match scope {
        AggregateAttributionScope::None => 0,
        AggregateAttributionScope::ExactEndpointDomains => 1,
        AggregateAttributionScope::GlobalIndeterminate => 2,
    }
}

const fn aggregate_availability_tag(availability: AggregatePathAvailability) -> u8 {
    match availability {
        AggregatePathAvailability::AvailableForExperimentalConstraint => 0,
        AggregatePathAvailability::BelowMinimumDistinctFragments => 1,
        AggregatePathAvailability::Contradicted => 2,
        AggregatePathAvailability::Indeterminate => 3,
    }
}

fn graph_resident_bytes(
    segment_capacity: usize,
    segments: &[GraphCatalogEntry],
    link_catalog_capacity: usize,
    outgoing_capacity: usize,
    outgoing: &[Vec<Arc>],
) -> Result<u64> {
    let segment_records = usize_to_u64(segment_capacity, "pair-path segment capacity")?
        .checked_mul(usize_to_u64(
            size_of::<GraphCatalogEntry>(),
            "pair-path segment-record size",
        )?)
        .ok_or_else(|| overflow("pair-path segment bytes overflow"))?;
    let identifier_bytes = segments.iter().try_fold(0u64, |sum, segment| {
        checked_add(
            sum,
            usize_to_u64(segment.id.capacity(), "pair-path identifier capacity")?,
            "pair-path identifier capacity sum overflow",
        )
    })?;
    let outer_adjacency = usize_to_u64(outgoing_capacity, "pair-path adjacency capacity")?
        .checked_mul(usize_to_u64(
            size_of::<Vec<Arc>>(),
            "pair-path adjacency-record size",
        )?)
        .ok_or_else(|| overflow("pair-path adjacency bytes overflow"))?;
    let arc_bytes = outgoing.iter().try_fold(0u64, |sum, arcs| {
        let bytes = usize_to_u64(arcs.capacity(), "pair-path arc capacity")?
            .checked_mul(usize_to_u64(size_of::<Arc>(), "pair-path arc size")?)
            .ok_or_else(|| overflow("pair-path arc bytes overflow"))?;
        checked_add(sum, bytes, "pair-path arc capacity sum overflow")
    })?;
    let link_catalog = usize_to_u64(link_catalog_capacity, "pair-path link catalog capacity")?
        .checked_mul(usize_to_u64(
            size_of::<CanonicalLinkCatalogEntry>(),
            "pair-path link-catalog record size",
        )?)
        .ok_or_else(|| overflow("pair-path link-catalog bytes overflow"))?;
    [
        segment_records,
        identifier_bytes,
        link_catalog,
        outer_adjacency,
        arc_bytes,
    ]
    .into_iter()
    .try_fold(0u64, |sum, bytes| {
        checked_add(sum, bytes, "pair-path resident graph bytes overflow")
    })
}

fn decision(
    pair: &PairPlacementEvidence,
    status: PairDecisionStatus,
    reason: Option<AbstentionReason>,
    ledger: ReplayLedger,
    compatible_paths: Vec<CompatiblePathEvidence>,
) -> PairPathDecision {
    decision_with_attribution(
        pair,
        status,
        reason,
        AggregateAttributionScope::None,
        Vec::new(),
        ledger,
        compatible_paths,
    )
}

fn decision_with_attribution(
    pair: &PairPlacementEvidence,
    status: PairDecisionStatus,
    reason: Option<AbstentionReason>,
    aggregate_attribution_scope: AggregateAttributionScope,
    affected_endpoint_domains: Vec<EndpointDomain>,
    ledger: ReplayLedger,
    compatible_paths: Vec<CompatiblePathEvidence>,
) -> PairPathDecision {
    PairPathDecision {
        lane_ordinal: pair.lane_ordinal,
        fragment_ordinal: pair.fragment_ordinal,
        fragment_identity_sha256: pair.fragment_identity_sha256,
        status,
        reason,
        authorizes_reconstruction: false,
        aggregate_attribution_scope,
        affected_endpoint_domains,
        ledger,
        compatible_paths,
    }
}

fn segment_index(segments: &[GraphCatalogEntry], id: &str) -> Result<u32> {
    segments
        .binary_search_by(|segment| segment.id.as_str().cmp(id))
        .ok()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| integrity("pair-path link references an absent unitig"))
}

fn insert_arc(outgoing: &mut [Vec<Arc>], from: OrientedSegment, arc: Arc) -> Result<()> {
    let slot = u32_to_usize(from.segment_index)?
        .checked_mul(2)
        .and_then(|value| value.checked_add(usize::from(from.direction == Direction::Reverse)))
        .ok_or_else(|| overflow("pair-path arc slot overflow"))?;
    outgoing
        .get_mut(slot)
        .ok_or_else(|| integrity("pair-path arc source is out of range"))?
        .try_reserve(1)
        .map_err(|_| resource_error("cannot allocate pair-path arc"))?;
    outgoing[slot].push(arc);
    Ok(())
}

fn validate_overlap(
    from: &Unitig,
    from_direction: Direction,
    to: &Unitig,
    to_direction: Direction,
    overlap: usize,
) -> Result<()> {
    for offset in 0..overlap {
        let left = oriented_base(
            &from.sequence,
            from_direction,
            from.sequence.len() - overlap + offset,
        )?;
        let right = oriented_base(&to.sequence, to_direction, offset)?;
        if left != right {
            return Err(integrity("pair-path link lacks its exact k-1 overlap"));
        }
    }
    Ok(())
}

fn oriented_base(sequence: &[u8], direction: Direction, index: usize) -> Result<u8> {
    match direction {
        Direction::Forward => sequence
            .get(index)
            .copied()
            .ok_or_else(|| integrity("oriented base index is out of range")),
        Direction::Reverse => {
            let source = sequence
                .len()
                .checked_sub(index + 1)
                .ok_or_else(|| integrity("reverse base index is out of range"))?;
            match sequence[source] {
                b'A' => Ok(b'T'),
                b'C' => Ok(b'G'),
                b'G' => Ok(b'C'),
                b'T' => Ok(b'A'),
                _ => Err(integrity("unitig contains non-ACGT")),
            }
        }
    }
}

fn find(parents: &mut [usize], mut value: usize) -> usize {
    let mut root = value;
    while parents[root] != root {
        root = parents[root];
    }
    while parents[value] != value {
        let next = parents[value];
        parents[value] = root;
        value = next;
    }
    root
}

fn union(parents: &mut [usize], ranks: &mut [u8], left: usize, right: usize) {
    let left_root = find(parents, left);
    let right_root = find(parents, right);
    if left_root == right_root {
        return;
    }
    match ranks[left_root].cmp(&ranks[right_root]) {
        std::cmp::Ordering::Less => parents[left_root] = right_root,
        std::cmp::Ordering::Greater => parents[right_root] = left_root,
        std::cmp::Ordering::Equal => {
            parents[right_root] = left_root;
            ranks[left_root] = ranks[left_root].saturating_add(1);
        }
    }
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}
fn usize_to_u64(value: usize, context: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| overflow(context))
}
fn u32_to_usize(value: u32) -> Result<usize> {
    usize::try_from(value).map_err(|_| overflow("pair-path u32 index does not fit usize"))
}
fn config_error(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ConfigurationInvalidLimit, context)
}
fn integrity(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegrityArtifact, context)
}
fn invariant(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}
fn resource_error(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AvailabilityU64;

    fn limits() -> WorkLimits {
        WorkLimits {
            maximum_pairs: 1_000,
            maximum_placements: 10_000,
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

    fn config() -> PairPathConfig {
        PairPathConfig {
            model: ModelConfig {
                minimum_anchors: 10,
                minimum_dominant_anchors: 10,
                dominance_numerator: 9,
                dominance_denominator: 10,
                maximum_span_p90_minus_p10: 10,
                maximum_inner_p90_minus_p10: 10,
            },
            limits: limits(),
            minimum_distinct_fragments_for_path: 2,
        }
    }

    fn evidence_input(
        graph: &PairPathGraph,
        pairs: &[PairPlacementEvidence],
        config: PairPathConfig,
    ) -> PlacementEvidenceInput {
        let mapper = MapperProvenance {
            algorithm_id: "test_exact_linear_mapper".to_owned(),
            algorithm_version: "1".to_owned(),
            parameters: "exact=true".to_owned(),
        };
        let mapper_identity_sha256 = mapper_identity_sha256(&mapper, config.limits).unwrap();
        let library_source_root_sha256 = [70; 32];
        let read_set_root_sha256 = [71; 32];
        PlacementEvidenceInput {
            library_source_root_sha256,
            read_set_root_sha256,
            mapper_identity_sha256,
            placement_domain: PlacementDomain::LinearUnitigOnly,
            certificate: CompleteEnumerationCertificate {
                graph_root_sha256: graph.graph_root_sha256(),
                library_source_root_sha256,
                read_set_root_sha256,
                mapper_identity_sha256,
                placement_domain: PlacementDomain::LinearUnitigOnly,
                supplied_fragment_instances: u64::try_from(pairs.len()).unwrap(),
                complete_within_declared_domain: true,
            },
            mapper,
            pairs: pairs.to_vec(),
        }
    }

    fn analyze_pair_paths(
        graph: &PairPathGraph,
        pairs: &[PairPlacementEvidence],
        config: PairPathConfig,
        worker_threads: usize,
    ) -> Result<PairPathResult> {
        let input = evidence_input(graph, pairs, config);
        super::analyze_pair_paths(graph, &input, &input, config, worker_threads)
    }

    fn unitig(id: &str, length: usize) -> Unitig {
        unitig_with_topology(id, length, Topology::Linear)
    }

    fn unitig_with_topology(id: &str, length: usize, topology: Topology) -> Unitig {
        Unitig {
            id: id.to_owned(),
            sequence: vec![b'A'; length],
            topology,
            edge_steps: length as u64,
            canonical_kmers: length as u64,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: AvailabilityU64::NotAvailable("test"),
            single_group_read_instances: AvailabilityU64::NotAvailable("test"),
            multi_group_read_instances_with_group: AvailabilityU64::NotAvailable("test"),
            placement_enumeration_status: "test",
            sequence_sha256: "test".to_owned(),
        }
    }

    fn link(from: &str, to: &str) -> GraphLink {
        GraphLink {
            from_segment: from.to_owned(),
            from_orientation: '+',
            to_segment: to.to_owned(),
            to_orientation: '+',
        }
    }

    fn graph(branch: bool) -> PairPathGraph {
        let unitigs = vec![
            unitig("a", 30),
            unitig("b", 10),
            unitig("c", 10),
            unitig("cal", 200),
            unitig("d", 30),
        ];
        let mut links = vec![link("a", "b"), link("b", "d")];
        if branch {
            links.push(link("a", "c"));
            links.push(link("c", "d"));
        }
        PairPathGraph::from_compaction(3, &unitigs, &links, limits()).unwrap()
    }

    fn placement(
        segment_index: u32,
        start: u64,
        end: u64,
        read_strand: Direction,
    ) -> ExactPlacement {
        ExactPlacement {
            segment_index,
            start,
            end,
            read_strand,
        }
    }

    fn group(group_ordinal: u32, placements: Vec<ExactPlacement>) -> ExactPlacementGroup {
        ExactPlacementGroup {
            group_ordinal,
            placements,
        }
    }

    fn pair(
        lane_ordinal: u32,
        fragment_ordinal: u64,
        r1: ExactPlacement,
        r2: ExactPlacement,
    ) -> PairPlacementEvidence {
        PairPlacementEvidence {
            lane_ordinal,
            fragment_ordinal,
            fragment_identity_sha256: [u8::try_from(fragment_ordinal % 251).unwrap(); 32],
            r1_read_sha256: [11; 32],
            r2_read_sha256: [22; 32],
            r1_groups: vec![group(0, vec![r1])],
            r2_groups: vec![group(0, vec![r2])],
            junction_spanning_reads_unavailable: 0,
        }
    }

    fn calibration_pairs(
        graph: &PairPathGraph,
        lane: u32,
        first_ordinal: u64,
        orientation: LibraryOrientation,
    ) -> Vec<PairPlacementEvidence> {
        let cal = graph.segment_index("cal").unwrap();
        let (left, right) = match orientation {
            LibraryOrientation::Fr => (Direction::Forward, Direction::Reverse),
            LibraryOrientation::Rf => (Direction::Reverse, Direction::Forward),
            LibraryOrientation::Ff => (Direction::Forward, Direction::Forward),
            LibraryOrientation::Rr => (Direction::Reverse, Direction::Reverse),
        };
        (0..10)
            .map(|offset| {
                let start = if offset < 5 { 10 } else { 11 };
                pair(
                    lane,
                    first_ordinal + offset,
                    placement(cal, start, start + 8, left),
                    placement(cal, 68, 76, right),
                )
            })
            .collect()
    }

    fn test_pair(graph: &PairPathGraph, lane: u32, ordinal: u64) -> PairPlacementEvidence {
        pair(
            lane,
            ordinal,
            placement(graph.segment_index("a").unwrap(), 0, 8, Direction::Forward),
            placement(
                graph.segment_index("d").unwrap(),
                22,
                30,
                Direction::Reverse,
            ),
        )
    }

    fn decision_for(result: &PairPathResult, ordinal: u64) -> &PairPathDecision {
        result
            .decisions
            .iter()
            .find(|decision| decision.fragment_ordinal == ordinal)
            .unwrap()
    }

    fn assert_scientific_result_equal(left: &PairPathResult, right: &PairPathResult) {
        assert_eq!(left.graph_root_sha256, right.graph_root_sha256);
        assert_eq!(
            left.calibration_placement_root_sha256,
            right.calibration_placement_root_sha256
        );
        assert_eq!(
            left.replay_placement_root_sha256,
            right.replay_placement_root_sha256
        );
        assert_eq!(left.models, right.models);
        assert_eq!(left.decisions, right.decisions);
        assert_eq!(left.aggregate_paths, right.aggregate_paths);
        assert_eq!(left.summary, right.summary);
    }

    #[test]
    fn exact_k_minus_one_distance_supports_one_existing_path() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        pairs.push(test_pair(&graph, 0, 100));
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        let decision = decision_for(&result, 100);
        assert_eq!(
            decision.status,
            PairDecisionStatus::SupportedUniqueExistingPath
        );
        assert_eq!(decision.reason, None);
        assert_eq!(decision.compatible_paths.len(), 1);
        let path = &decision.compatible_paths[0];
        assert_eq!(path.path.handles.len(), 3);
        assert_eq!(path.path.link_indices.len(), 2);
        assert_eq!((path.minimum_outer_span, path.maximum_outer_span), (66, 66));
        assert_eq!((path.minimum_inner_gap, path.maximum_inner_gap), (50, 50));
        assert_eq!(
            decision_for(&result, 0).status,
            PairDecisionStatus::TrivialWithinUnitig
        );
        assert_eq!(result.summary.trivial_within_unitig, 10);
    }

    #[test]
    fn equal_length_branch_paths_remain_an_explicit_conflict() {
        let graph = graph(true);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        pairs.push(test_pair(&graph, 0, 100));
        let result = analyze_pair_paths(&graph, &pairs, config(), 2).unwrap();
        let decision = decision_for(&result, 100);
        assert_eq!(decision.status, PairDecisionStatus::Abstained);
        assert_eq!(
            decision.reason,
            Some(AbstentionReason::MultipleCompatiblePaths)
        );
        assert_eq!(decision.compatible_paths.len(), 2);
    }

    #[test]
    fn multimapping_can_support_only_when_all_compatible_observations_share_one_path() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let mut multi = test_pair(&graph, 0, 100);
        let a = graph.segment_index("a").unwrap();
        multi
            .r1_groups
            .push(group(1, vec![placement(a, 1, 9, Direction::Forward)]));
        pairs.push(multi);
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        let decision = decision_for(&result, 100);
        assert_eq!(
            decision.status,
            PairDecisionStatus::SupportedUniqueExistingPath
        );
        assert_eq!(decision.ledger.placement_group_pairs, 2);
        assert_eq!(decision.ledger.placement_pairs, 2);
        assert_eq!(decision.compatible_paths.len(), 1);
        assert_eq!(
            decision.compatible_paths[0].compatible_placement_observations,
            2
        );
        assert_eq!(result.models.lanes[0].ledger.nonunique_placement, 1);

        let mut reordered = pairs;
        reordered.last_mut().unwrap().r1_groups.reverse();
        assert_scientific_result_equal(
            &analyze_pair_paths(&graph, &reordered, config(), 4).unwrap(),
            &result,
        );
    }

    #[test]
    fn geometry_pruning_finishes_a_cycle_without_false_depth_indeterminacy() {
        let unitigs = vec![unitig("cal", 100), unitig("x", 30)];
        let graph =
            PairPathGraph::from_compaction(3, &unitigs, &[link("x", "x")], limits()).unwrap();
        let cal = graph.segment_index("cal").unwrap();
        let x = graph.segment_index("x").unwrap();
        let mut pairs: Vec<_> = (0..10)
            .map(|ordinal| {
                pair(
                    0,
                    ordinal,
                    placement(cal, 10, 18, Direction::Forward),
                    placement(cal, 30, 38, Direction::Reverse),
                )
            })
            .collect();
        pairs.push(pair(
            0,
            100,
            placement(x, 0, 8, Direction::Forward),
            placement(x, 20, 28, Direction::Reverse),
        ));
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        let decision = decision_for(&result, 100);
        assert_eq!(decision.status, PairDecisionStatus::Abstained);
        assert_eq!(decision.reason, Some(AbstentionReason::OrientationConflict));
        assert_ne!(decision.reason, Some(AbstentionReason::SearchDepthLimit));
        assert_eq!(decision.compatible_paths[0].path.link_indices.len(), 0);
        assert_ne!(decision.ledger.geometry_pruned_extensions, 0);
    }

    #[test]
    fn a_reachable_path_beyond_the_depth_cap_is_indeterminate() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        pairs.push(test_pair(&graph, 0, 100));
        let mut bounded = config();
        bounded.limits.maximum_edges_per_path = 1;
        let per_state = 256 + 32;
        bounded.limits.search_memory_bytes_per_worker =
            (bounded.limits.maximum_search_states_per_fragment
                + bounded.limits.maximum_compatible_paths_per_fragment)
                * per_state;
        bounded.limits.result_memory_bytes = 32 << 20;
        let result = analyze_pair_paths(&graph, &pairs, bounded, 1).unwrap();
        let decision = decision_for(&result, 100);
        assert_eq!(decision.status, PairDecisionStatus::Indeterminate);
        assert_eq!(decision.reason, Some(AbstentionReason::SearchDepthLimit));
        assert_eq!(decision.ledger.search_depth_cap_hits, 1);
    }

    #[test]
    fn target_path_count_exhaustion_is_indeterminate_not_unique() {
        let graph = graph(true);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        pairs.push(test_pair(&graph, 0, 100));
        let mut bounded = config();
        bounded.limits.maximum_target_paths_per_fragment = 1;
        let result = analyze_pair_paths(&graph, &pairs, bounded, 1).unwrap();
        let decision = decision_for(&result, 100);
        assert_eq!(decision.status, PairDecisionStatus::Indeterminate);
        assert_eq!(
            decision.reason,
            Some(AbstentionReason::SearchPathCountLimit)
        );
        assert_eq!(decision.ledger.target_path_cap_hits, 1);
    }

    #[test]
    fn central_quantiles_retain_but_do_not_fit_one_tail_outlier() {
        let graph = graph(false);
        let cal = graph.segment_index("cal").unwrap();
        let mut pairs: Vec<_> = (0..10)
            .map(|ordinal| {
                pair(
                    0,
                    ordinal,
                    placement(cal, 10, 18, Direction::Forward),
                    placement(cal, 68, 76, Direction::Reverse),
                )
            })
            .collect();
        pairs.push(pair(
            0,
            10,
            placement(cal, 10, 18, Direction::Forward),
            placement(cal, 152, 160, Direction::Reverse),
        ));
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        let model = &result.models.lanes[0];
        let spans = model.outer_span.unwrap();
        assert_eq!((spans.p10, spans.p90, spans.maximum), (66, 66, 150));
        assert_eq!(model.availability, LaneModelAvailability::Available);
        assert_eq!(
            decision_for(&result, 10).reason,
            Some(AbstentionReason::GeometryConflict)
        );
    }

    #[test]
    fn lanes_never_pool_and_input_order_and_thread_count_do_not_change_results() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 9, 100, LibraryOrientation::Fr);
        pairs.extend(calibration_pairs(&graph, 2, 0, LibraryOrientation::Rf));
        let expected = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        pairs.reverse();
        let actual = analyze_pair_paths(&graph, &pairs, config(), 4).unwrap();
        assert_scientific_result_equal(&actual, &expected);
        assert_eq!(expected.worker_threads, 1);
        assert_eq!(actual.worker_threads, 4);
        assert_eq!(actual.models.lanes[0].lane_ordinal, 2);
        assert_eq!(
            actual.models.lanes[0].dominant_orientation,
            Some(LibraryOrientation::Rf)
        );
        assert_eq!(actual.models.lanes[1].lane_ordinal, 9);
        assert_eq!(
            actual.models.lanes[1].dominant_orientation,
            Some(LibraryOrientation::Fr)
        );
    }

    #[test]
    fn every_orientation_is_inferred_in_coordinate_order() {
        let graph = graph(false);
        let orientations = [
            LibraryOrientation::Fr,
            LibraryOrientation::Rf,
            LibraryOrientation::Ff,
            LibraryOrientation::Rr,
        ];
        let mut pairs = Vec::new();
        for (lane, orientation) in orientations.into_iter().enumerate() {
            pairs.extend(calibration_pairs(
                &graph,
                u32::try_from(lane).unwrap(),
                u64::try_from(lane).unwrap() * 100,
                orientation,
            ));
        }
        let result = analyze_pair_paths(&graph, &pairs, config(), 4).unwrap();
        assert_eq!(result.models.lanes.len(), 4);
        for (lane, orientation) in orientations.into_iter().enumerate() {
            assert_eq!(
                result.models.lanes[lane].dominant_orientation,
                Some(orientation)
            );
            assert_eq!(
                result.models.lanes[lane].availability,
                LaneModelAvailability::Available
            );
        }
    }

    #[test]
    fn linear_unitig_placement_domain_quantifies_junction_unavailability() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        pairs.push(PairPlacementEvidence {
            lane_ordinal: 0,
            fragment_ordinal: 100,
            fragment_identity_sha256: [100; 32],
            r1_read_sha256: [11; 32],
            r2_read_sha256: [22; 32],
            r1_groups: Vec::new(),
            r2_groups: Vec::new(),
            junction_spanning_reads_unavailable: 1,
        });
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        assert_eq!(result.placement_domain, PlacementDomain::LinearUnitigOnly);
        assert_eq!(result.summary.junction_spanning_reads_unavailable, 1);
        assert_eq!(
            result.models.lanes[0].ledger.junction_spanning_unavailable,
            1
        );
        assert_eq!(
            decision_for(&result, 100).reason,
            Some(AbstentionReason::JunctionSpanningPlacementUnavailable)
        );
    }

    #[test]
    fn canonical_link_identity_is_independent_of_order_and_reciprocal_duplication() {
        let unitigs = vec![unitig("a", 30), unitig("b", 30), unitig("cal", 200)];
        let forward = link("a", "b");
        let reciprocal = GraphLink {
            from_segment: "b".to_owned(),
            from_orientation: '-',
            to_segment: "a".to_owned(),
            to_orientation: '-',
        };
        let first = PairPathGraph::from_compaction(
            3,
            &unitigs,
            &[forward.clone(), reciprocal.clone()],
            limits(),
        )
        .unwrap();
        let second =
            PairPathGraph::from_compaction(3, &unitigs, &[reciprocal, forward], limits()).unwrap();
        assert_eq!(first.outgoing, second.outgoing);
        assert_eq!(first.graph_root_sha256(), second.graph_root_sha256());
        assert_eq!(first.link_catalog(), second.link_catalog());
        assert_eq!(
            first.accounted_allocation_bytes(),
            second.accounted_allocation_bytes()
        );
    }

    #[test]
    fn corrupt_or_over_limit_evidence_fails_closed() {
        let graph = graph(false);
        let a = graph.segment_index("a").unwrap();
        let invalid = pair(
            0,
            0,
            placement(a, 8, 8, Direction::Forward),
            placement(a, 20, 28, Direction::Reverse),
        );
        assert_eq!(
            analyze_pair_paths(&graph, &[invalid], config(), 1)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut limited = config();
        limited.limits.maximum_pairs = 1;
        limited.limits.result_memory_bytes = 32 << 20;
        let pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        assert_eq!(
            analyze_pair_paths(&graph, &pairs, limited, 1)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn graph_and_placement_provenance_are_content_bound_and_order_invariant() {
        let graph = graph(false);
        let pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let input = evidence_input(&graph, &pairs, config());
        let first = placement_input_root_sha256(&graph, &input, limits()).unwrap();
        let mut reordered = input.clone();
        reordered.pairs.reverse();
        for pair in &mut reordered.pairs {
            pair.r1_groups.reverse();
        }
        let second = placement_input_root_sha256(&graph, &reordered, limits()).unwrap();
        assert_eq!(first, second);
        reordered.pairs[0].r1_read_sha256[0] ^= 1;
        assert_ne!(
            first,
            placement_input_root_sha256(&graph, &reordered, limits()).unwrap()
        );

        let one = PairPathGraph::from_compaction(3, &[unitig("x", 20)], &[], limits()).unwrap();
        let mut changed = unitig("x", 20);
        changed.sequence[10] = b'C';
        let two = PairPathGraph::from_compaction(3, &[changed], &[], limits()).unwrap();
        assert_ne!(one.graph_root_sha256(), two.graph_root_sha256());
        assert_ne!(
            one.segment_catalog()[0].sequence_sha256,
            two.segment_catalog()[0].sequence_sha256
        );
    }

    #[test]
    fn certificate_tampering_and_incomplete_enumeration_fail_closed() {
        let graph = graph(false);
        let pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let mut calibration = evidence_input(&graph, &pairs, config());
        let replay = calibration.clone();
        calibration.certificate.graph_root_sha256[0] ^= 1;
        assert_eq!(
            super::analyze_pair_paths(&graph, &calibration, &replay, config(), 1)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        calibration = replay.clone();
        calibration.certificate.complete_within_declared_domain = false;
        assert_eq!(
            super::analyze_pair_paths(&graph, &calibration, &replay, config(), 1)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        calibration = replay.clone();
        calibration.mapper.parameters.push('x');
        assert_eq!(
            super::analyze_pair_paths(&graph, &calibration, &replay, config(), 1)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn calibration_and_replay_are_separate_and_reuse_is_reported() {
        let graph = graph(false);
        let calibration_pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let replay_pairs = vec![test_pair(&graph, 0, 100)];
        let calibration = evidence_input(&graph, &calibration_pairs, config());
        let replay = evidence_input(&graph, &replay_pairs, config());
        let result = super::analyze_pair_paths(&graph, &calibration, &replay, config(), 2).unwrap();
        assert_eq!(result.calibration_reuse.calibration_fragment_instances, 10);
        assert_eq!(result.calibration_reuse.replay_fragment_instances, 1);
        assert_eq!(result.calibration_reuse.reused_fragment_instances, 0);
        assert_ne!(
            result.calibration_placement_root_sha256,
            result.replay_placement_root_sha256
        );
    }

    #[test]
    fn individual_support_never_authorizes_and_aggregate_requires_two_fragments() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        pairs.push(test_pair(&graph, 0, 100));
        let one = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        assert!(!decision_for(&one, 100).authorizes_reconstruction);
        assert_eq!(one.aggregate_paths.len(), 1);
        assert_eq!(
            one.aggregate_paths[0].availability,
            AggregatePathAvailability::BelowMinimumDistinctFragments
        );
        assert!(!one.aggregate_paths[0].available_for_experimental_constraint);

        pairs.push(test_pair(&graph, 0, 101));
        let two = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        assert_eq!(two.aggregate_paths.len(), 1);
        assert_eq!(two.aggregate_paths[0].distinct_supporting_fragments, 2);
        assert_eq!(
            two.aggregate_paths[0].availability,
            AggregatePathAvailability::AvailableForExperimentalConstraint
        );
        assert!(two.aggregate_paths[0].available_for_experimental_constraint);
    }

    #[test]
    fn any_cross_component_or_geometry_alternative_forces_abstention() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let mut cross = test_pair(&graph, 0, 100);
        cross.r1_groups.push(group(
            1,
            vec![placement(
                graph.segment_index("cal").unwrap(),
                0,
                8,
                Direction::Forward,
            )],
        ));
        pairs.push(cross);
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        let decision = decision_for(&result, 100);
        assert_eq!(decision.reason, Some(AbstentionReason::CrossComponent));
        assert_eq!(decision.compatible_paths.len(), 1);
        assert_ne!(decision.ledger.cross_component_placement_pairs, 0);
        assert_eq!(
            result.aggregate_paths[0].availability,
            AggregatePathAvailability::Contradicted
        );
        assert_eq!(
            result.aggregate_paths[0].distinct_contradicting_fragments,
            1
        );

        pairs.pop();
        let mut geometry = test_pair(&graph, 0, 101);
        geometry.r1_groups.push(group(
            1,
            vec![placement(
                graph.segment_index("a").unwrap(),
                10,
                18,
                Direction::Forward,
            )],
        ));
        pairs.push(geometry);
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        let decision = decision_for(&result, 101);
        assert_eq!(decision.reason, Some(AbstentionReason::GeometryConflict));
        assert_eq!(decision.compatible_paths.len(), 1);
        assert_ne!(decision.ledger.geometry_conflicts, 0);

        pairs.pop();
        let mut orientation = test_pair(&graph, 0, 102);
        orientation.r1_groups.push(group(
            1,
            vec![placement(
                graph.segment_index("a").unwrap(),
                0,
                8,
                Direction::Reverse,
            )],
        ));
        pairs.push(orientation);
        let result = analyze_pair_paths(&graph, &pairs, config(), 1).unwrap();
        let decision = decision_for(&result, 102);
        assert_eq!(decision.reason, Some(AbstentionReason::OrientationConflict));
        assert_eq!(decision.compatible_paths.len(), 1);
        assert_ne!(decision.ledger.orientation_conflicts, 0);
    }

    #[test]
    fn k_domain_matches_exact_graph_substrate() {
        assert!(PairPathGraph::from_compaction(127, &[unitig("k127", 127)], &[], limits()).is_ok());
        assert_eq!(
            PairPathGraph::from_compaction(128, &[unitig("k128", 128)], &[], limits())
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
    }

    #[test]
    fn component_indexing_handles_a_long_chain_without_quadratic_union_find() {
        let unitigs: Vec<_> = (0..512)
            .map(|index| unitig(&format!("u{index:04}"), 8))
            .collect();
        let links: Vec<_> = (0..511)
            .map(|index| link(&format!("u{index:04}"), &format!("u{:04}", index + 1)))
            .collect();
        let graph = PairPathGraph::from_compaction(3, &unitigs, &links, limits()).unwrap();
        assert!(graph
            .segment_catalog()
            .iter()
            .all(|segment| segment.component_index == 0));
    }

    #[test]
    fn analysis_budget_readmits_the_retained_graph() {
        let graph = graph(false);
        let pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let mut bounded = config();
        bounded.limits.analysis_memory_bytes = bounded.limits.result_memory_bytes;
        let input = evidence_input(&graph, &pairs, bounded);
        assert_eq!(
            super::analyze_pair_paths(&graph, &input, &input, bounded, 1)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn same_component_geometry_pruning_is_an_adverse_alternative() {
        let unitigs = vec![
            unitig("z", 200),
            unitig("a", 30),
            unitig("b", 10),
            unitig("cal", 200),
            unitig("d", 30),
        ];
        let graph = PairPathGraph::from_compaction(
            3,
            &unitigs,
            &[link("z", "a"), link("a", "b"), link("b", "d")],
            limits(),
        )
        .unwrap();
        let calibration = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let a = graph.segment_index("a").unwrap();
        let z = graph.segment_index("z").unwrap();
        let d = graph.segment_index("d").unwrap();
        let mut probe = pair(
            0,
            100,
            placement(a, 0, 8, Direction::Forward),
            placement(d, 22, 30, Direction::Reverse),
        );
        probe
            .r1_groups
            .push(group(1, vec![placement(z, 0, 8, Direction::Forward)]));
        let calibration_input = evidence_input(&graph, &calibration, config());
        let replay_input = evidence_input(&graph, &[probe], config());
        let result =
            super::analyze_pair_paths(&graph, &calibration_input, &replay_input, config(), 1)
                .unwrap();
        let decision = &result.decisions[0];
        assert_eq!(decision.status, PairDecisionStatus::Abstained);
        assert_eq!(decision.reason, Some(AbstentionReason::GeometryConflict));
        assert_eq!(decision.compatible_paths.len(), 1);
        assert_ne!(decision.ledger.geometry_pruned_extensions, 0);
    }

    #[test]
    fn non_linear_endpoints_and_traversals_never_support_replay() {
        let endpoint_graph = PairPathGraph::from_compaction(
            3,
            &[
                unitig_with_topology("a", 30, Topology::ClosedGraphWalk),
                unitig("b", 10),
                unitig("cal", 200),
                unitig("d", 30),
            ],
            &[link("a", "b"), link("b", "d")],
            limits(),
        )
        .unwrap();
        let calibration = calibration_pairs(&endpoint_graph, 0, 0, LibraryOrientation::Fr);
        let replay = [test_pair(&endpoint_graph, 0, 100)];
        let result = super::analyze_pair_paths(
            &endpoint_graph,
            &evidence_input(&endpoint_graph, &calibration, config()),
            &evidence_input(&endpoint_graph, &replay, config()),
            config(),
            1,
        )
        .unwrap();
        assert_eq!(result.decisions[0].status, PairDecisionStatus::Abstained);
        assert_eq!(
            result.decisions[0].reason,
            Some(AbstentionReason::NonLinearTopology)
        );

        let traversal_graph = PairPathGraph::from_compaction(
            3,
            &[
                unitig("a", 30),
                unitig_with_topology("b", 10, Topology::ClosedGraphWalk),
                unitig("cal", 200),
                unitig("d", 30),
            ],
            &[link("a", "b"), link("b", "d")],
            limits(),
        )
        .unwrap();
        let calibration = calibration_pairs(&traversal_graph, 0, 0, LibraryOrientation::Fr);
        let replay = [test_pair(&traversal_graph, 0, 101)];
        let result = super::analyze_pair_paths(
            &traversal_graph,
            &evidence_input(&traversal_graph, &calibration, config()),
            &evidence_input(&traversal_graph, &replay, config()),
            config(),
            1,
        )
        .unwrap();
        assert_eq!(result.decisions[0].status, PairDecisionStatus::Abstained);
        assert_eq!(
            result.decisions[0].reason,
            Some(AbstentionReason::NonLinearTopology)
        );
        assert_ne!(result.decisions[0].ledger.non_linear_topology_conflicts, 0);

        let closed = endpoint_graph.segment_index("a").unwrap();
        let zero_link = [pair(
            0,
            102,
            placement(closed, 1, 9, Direction::Forward),
            placement(closed, 20, 28, Direction::Reverse),
        )];
        let result = super::analyze_pair_paths(
            &endpoint_graph,
            &evidence_input(
                &endpoint_graph,
                &calibration_pairs(&endpoint_graph, 0, 0, LibraryOrientation::Fr),
                config(),
            ),
            &evidence_input(&endpoint_graph, &zero_link, config()),
            config(),
            1,
        )
        .unwrap();
        assert_eq!(result.decisions[0].status, PairDecisionStatus::Abstained);
        assert_eq!(
            result.decisions[0].reason,
            Some(AbstentionReason::NonLinearTopology)
        );
    }

    #[test]
    fn pure_same_endpoint_contradiction_blocks_aggregate_availability() {
        let graph = graph(false);
        let calibration = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let a = graph.segment_index("a").unwrap();
        let d = graph.segment_index("d").unwrap();
        let replay = [
            test_pair(&graph, 0, 100),
            test_pair(&graph, 0, 101),
            pair(
                0,
                102,
                placement(a, 10, 18, Direction::Forward),
                placement(d, 22, 30, Direction::Reverse),
            ),
        ];
        let result = super::analyze_pair_paths(
            &graph,
            &evidence_input(&graph, &calibration, config()),
            &evidence_input(&graph, &replay, config()),
            config(),
            1,
        )
        .unwrap();
        assert!(result.decisions[2].compatible_paths.is_empty());
        assert_eq!(
            result.decisions[2].reason,
            Some(AbstentionReason::GeometryConflict)
        );
        assert_eq!(result.aggregate_paths.len(), 1);
        assert_eq!(
            result.aggregate_paths[0].availability,
            AggregatePathAvailability::Contradicted
        );
        assert_eq!(result.aggregate_paths[0].distinct_supporting_fragments, 2);
        assert_eq!(
            result.aggregate_paths[0].distinct_contradicting_fragments,
            1
        );
        assert!(!result.aggregate_paths[0].available_for_experimental_constraint);
    }

    #[test]
    fn unattributable_placement_cap_makes_every_candidate_indeterminate() {
        let graph = graph(false);
        let mut bounded = config();
        bounded.limits.maximum_placement_pairs_per_fragment = 1;
        let calibration = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let mut exhausted = test_pair(&graph, 0, 102);
        let a = graph.segment_index("a").unwrap();
        exhausted
            .r1_groups
            .push(group(1, vec![placement(a, 1, 9, Direction::Forward)]));
        let replay = [
            test_pair(&graph, 0, 100),
            test_pair(&graph, 0, 101),
            exhausted,
        ];
        let result = super::analyze_pair_paths(
            &graph,
            &evidence_input(&graph, &calibration, bounded),
            &evidence_input(&graph, &replay, bounded),
            bounded,
            1,
        )
        .unwrap();
        assert_eq!(
            result.decisions[2].aggregate_attribution_scope,
            AggregateAttributionScope::GlobalIndeterminate
        );
        assert_eq!(
            result.aggregate_paths[0].availability,
            AggregatePathAvailability::Indeterminate
        );
        assert_eq!(
            result.aggregate_paths[0].distinct_indeterminate_fragments,
            1
        );
    }

    #[test]
    fn placement_root_api_rejects_invalid_certificate_and_read_spans() {
        let graph = graph(false);
        let pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let mut input = evidence_input(&graph, &pairs, config());
        input.certificate.complete_within_declared_domain = false;
        assert_eq!(
            placement_input_root_sha256(&graph, &input, limits())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        input = evidence_input(&graph, &pairs, config());
        input.certificate.read_set_root_sha256[0] ^= 1;
        assert_eq!(
            placement_input_root_sha256(&graph, &input, limits())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut inconsistent = test_pair(&graph, 0, 100);
        let a = graph.segment_index("a").unwrap();
        inconsistent
            .r1_groups
            .push(group(1, vec![placement(a, 1, 10, Direction::Forward)]));
        let input = evidence_input(&graph, &[inconsistent], config());
        assert_eq!(
            placement_input_root_sha256(&graph, &input, limits())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn graph_sequence_hash_work_has_an_exact_boundary() {
        let mut exact = limits();
        exact.maximum_graph_sequence_bases = 10;
        let graph = PairPathGraph::from_compaction(3, &[unitig("x", 10)], &[], exact).unwrap();
        assert_eq!(graph.sequence_bases_hashed(), 10);
        let mut below = exact;
        below.maximum_graph_sequence_bases = 9;
        assert_eq!(
            PairPathGraph::from_compaction(3, &[unitig("x", 10)], &[], below)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );

        let mut no_record_capacity = limits();
        no_record_capacity.graph_memory_bytes = 4096;
        assert_eq!(
            PairPathGraph::from_compaction(
                3,
                &[unitig("x", usize::from(u8::MAX))],
                &[],
                no_record_capacity,
            )
            .unwrap_err()
            .code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn complete_result_root_is_deterministic_and_rejects_tampering() {
        let graph = graph(false);
        let mut pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        pairs.push(test_pair(&graph, 0, 100));
        let result = analyze_pair_paths(&graph, &pairs, config(), 2).unwrap();
        validate_pair_path_result(&result).unwrap();
        assert_eq!(
            result.result_root_sha256,
            pair_path_result_root_sha256(&result).unwrap()
        );

        let mut reordered = pairs;
        reordered.reverse();
        let same = analyze_pair_paths(&graph, &reordered, config(), 2).unwrap();
        assert_eq!(result.result_root_sha256, same.result_root_sha256);

        let mut changed_worker = analyze_pair_paths(&graph, &reordered, config(), 1).unwrap();
        assert_ne!(result.result_root_sha256, changed_worker.result_root_sha256);
        changed_worker.worker_threads = 2;
        assert_eq!(
            validate_pair_path_result(&changed_worker)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut tampered = result.clone();
        tampered.decisions[0].ledger.search_states ^= 1;
        assert_eq!(
            validate_pair_path_result(&tampered).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
        tampered = result;
        tampered.config.model.maximum_span_p90_minus_p10 += 1;
        assert_eq!(
            validate_pair_path_result(&tampered).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        let result = analyze_pair_paths(&graph, &reordered, config(), 2).unwrap();
        let mut self_consistent_bad_ledger = result.clone();
        self_consistent_bad_ledger.decisions[0]
            .ledger
            .search_arcs_examined = self_consistent_bad_ledger
            .config
            .limits
            .maximum_search_arc_examinations_per_fragment
            + 1;
        self_consistent_bad_ledger.result_root_sha256 =
            pair_path_result_root_sha256(&self_consistent_bad_ledger).unwrap();
        assert_eq!(
            validate_pair_path_result(&self_consistent_bad_ledger)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut self_consistent_bad_aggregate = result;
        self_consistent_bad_aggregate.aggregate_paths[0].distinct_supporting_fragments += 1;
        self_consistent_bad_aggregate.result_root_sha256 =
            pair_path_result_root_sha256(&self_consistent_bad_aggregate).unwrap();
        assert_eq!(
            validate_pair_path_result(&self_consistent_bad_aggregate)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn cross_library_and_cross_mapper_model_transfer_fail_closed() {
        let graph = graph(false);
        let calibration_pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let replay_pairs = [test_pair(&graph, 0, 100)];
        let calibration = evidence_input(&graph, &calibration_pairs, config());

        let mut other_library = evidence_input(&graph, &replay_pairs, config());
        other_library.library_source_root_sha256[0] ^= 1;
        other_library.certificate.library_source_root_sha256 =
            other_library.library_source_root_sha256;
        placement_input_root_sha256(&graph, &other_library, limits()).unwrap();
        assert_eq!(
            super::analyze_pair_paths(&graph, &calibration, &other_library, config(), 1)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        assert_eq!(
            authenticated_evidence_pair_from_pair_mapper(
                calibration.clone(),
                other_library,
                [99; 32]
            )
            .unwrap_err()
            .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut other_mapper = evidence_input(&graph, &replay_pairs, config());
        other_mapper.mapper.parameters.push_str(";different=true");
        let other_identity = mapper_identity_sha256(&other_mapper.mapper, limits()).unwrap();
        other_mapper.mapper_identity_sha256 = other_identity;
        other_mapper.certificate.mapper_identity_sha256 = other_identity;
        placement_input_root_sha256(&graph, &other_mapper, limits()).unwrap();
        assert_eq!(
            super::analyze_pair_paths(&graph, &calibration, &other_mapper, config(), 1)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn opaque_mapper_capability_binds_disjoint_inputs_and_authenticated_result() {
        let graph = graph(false);
        let calibration_pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let replay_pairs = [test_pair(&graph, 0, 100)];
        let calibration = evidence_input(&graph, &calibration_pairs, config());
        let mut replay = evidence_input(&graph, &replay_pairs, config());
        replay.read_set_root_sha256[0] ^= 1;
        replay.certificate.read_set_root_sha256 = replay.read_set_root_sha256;
        let capability =
            authenticated_evidence_pair_from_pair_mapper(calibration, replay, [99; 32]).unwrap();
        let authenticated =
            analyze_authenticated_pair_paths(&graph, &capability, config(), 2).unwrap();
        validate_authenticated_pair_path_result(&authenticated).unwrap();
        assert_eq!(
            authenticated.placement_producer_result_root_sha256(),
            [99; 32]
        );
        assert_eq!(
            authenticated.result().decisions[0].status,
            PairDecisionStatus::SupportedUniqueExistingPath
        );

        let mut tampered = authenticated;
        tampered.authenticated_result_root_sha256[0] ^= 1;
        assert_eq!(
            validate_authenticated_pair_path_result(&tampered)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn explicit_arc_and_path_reconstruction_work_caps_fail_indeterminate() {
        let graph = graph(false);
        let calibration_pairs = calibration_pairs(&graph, 0, 0, LibraryOrientation::Fr);
        let replay_pairs = [test_pair(&graph, 0, 100)];

        let mut arc_limited = config();
        arc_limited
            .limits
            .maximum_search_arc_examinations_per_fragment = 1;
        let result = super::analyze_pair_paths(
            &graph,
            &evidence_input(&graph, &calibration_pairs, arc_limited),
            &evidence_input(&graph, &replay_pairs, arc_limited),
            arc_limited,
            1,
        )
        .unwrap();
        assert_eq!(
            result.decisions[0].status,
            PairDecisionStatus::Indeterminate
        );
        assert_eq!(
            result.decisions[0].reason,
            Some(AbstentionReason::SearchArcWorkLimit)
        );
        assert_eq!(result.decisions[0].ledger.search_arcs_examined, 1);
        assert_eq!(result.decisions[0].ledger.search_arc_cap_hits, 1);

        let mut reconstruction_limited = config();
        reconstruction_limited
            .limits
            .maximum_path_reconstruction_elements_per_fragment = 19;
        let result = super::analyze_pair_paths(
            &graph,
            &evidence_input(&graph, &calibration_pairs, reconstruction_limited),
            &evidence_input(&graph, &replay_pairs, reconstruction_limited),
            reconstruction_limited,
            1,
        )
        .unwrap();
        assert_eq!(
            result.decisions[0].status,
            PairDecisionStatus::Indeterminate
        );
        assert_eq!(
            result.decisions[0].reason,
            Some(AbstentionReason::PathReconstructionWorkLimit)
        );
        assert_eq!(result.decisions[0].ledger.path_reconstruction_elements, 0);
        assert_eq!(result.decisions[0].ledger.path_reconstruction_cap_hits, 1);

        let mut exact = config();
        exact.limits.maximum_search_arc_examinations_per_fragment = 2;
        exact
            .limits
            .maximum_path_reconstruction_elements_per_fragment = 20;
        let result = super::analyze_pair_paths(
            &graph,
            &evidence_input(&graph, &calibration_pairs, exact),
            &evidence_input(&graph, &replay_pairs, exact),
            exact,
            1,
        )
        .unwrap();
        assert_eq!(
            result.decisions[0].status,
            PairDecisionStatus::SupportedUniqueExistingPath
        );
        assert_eq!(result.decisions[0].ledger.search_arcs_examined, 2);
        assert_eq!(result.decisions[0].ledger.path_reconstruction_elements, 20);
    }

    #[test]
    fn exhaustive_small_dag_oracle_matches_every_compatible_path() {
        const EDGES: [(&str, &str); 6] = [
            ("a", "b"),
            ("a", "c"),
            ("a", "d"),
            ("b", "c"),
            ("b", "d"),
            ("c", "d"),
        ];
        for mask in 0u8..(1 << EDGES.len()) {
            let selected: Vec<_> = EDGES
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, &(from, to))| link(from, to))
                .collect();
            let graph = PairPathGraph::from_compaction(
                3,
                &[
                    unitig("a", 3),
                    unitig("b", 3),
                    unitig("c", 3),
                    unitig("d", 3),
                    unitig("cal", 20),
                ],
                &selected,
                limits(),
            )
            .unwrap();
            let a = graph.segment_index("a").unwrap();
            let d = graph.segment_index("d").unwrap();
            let cal = graph.segment_index("cal").unwrap();
            for desired_edges in 1u64..=3 {
                let calibration: Vec<_> = (0..10)
                    .map(|ordinal| {
                        pair(
                            0,
                            ordinal,
                            placement(cal, 0, 1, Direction::Forward),
                            placement(
                                cal,
                                desired_edges + 2,
                                desired_edges + 3,
                                Direction::Reverse,
                            ),
                        )
                    })
                    .collect();
                let replay = [pair(
                    0,
                    100,
                    placement(a, 0, 1, Direction::Forward),
                    placement(d, 2, 3, Direction::Reverse),
                )];
                let result = super::analyze_pair_paths(
                    &graph,
                    &evidence_input(&graph, &calibration, config()),
                    &evidence_input(&graph, &replay, config()),
                    config(),
                    1,
                )
                .unwrap();
                let mut observed: Vec<_> = result.decisions[0]
                    .compatible_paths
                    .iter()
                    .map(|evidence| evidence.path.clone())
                    .collect();
                observed.sort_unstable();
                let start = OrientedSegment {
                    segment_index: a,
                    direction: Direction::Forward,
                };
                let mut expected = Vec::new();
                oracle_exact_depth_paths(
                    &graph,
                    start,
                    d,
                    u32::try_from(desired_edges).unwrap(),
                    &mut vec![start],
                    &mut Vec::new(),
                    &mut expected,
                );
                expected.sort_unstable();
                assert_eq!(observed, expected, "mask={mask:06b}, depth={desired_edges}");
            }
        }
    }

    fn oracle_exact_depth_paths(
        graph: &PairPathGraph,
        current: OrientedSegment,
        target_segment: u32,
        desired_edges: u32,
        handles: &mut Vec<OrientedSegment>,
        links: &mut Vec<u32>,
        output: &mut Vec<CanonicalGraphPath>,
    ) {
        if current.segment_index == target_segment && links.len() == desired_edges as usize {
            let reverse_handles: Vec<_> =
                handles.iter().rev().map(|handle| handle.flip()).collect();
            let reverse_links: Vec<_> = links.iter().rev().copied().collect();
            let forward = (handles.clone(), links.clone());
            let reverse = (reverse_handles, reverse_links);
            let (handles, link_indices) = if reverse < forward { reverse } else { forward };
            output.push(CanonicalGraphPath {
                handles,
                link_indices,
            });
            return;
        }
        if links.len() >= desired_edges as usize {
            return;
        }
        for arc in graph.arcs(current).unwrap() {
            handles.push(arc.to);
            links.push(arc.link_index);
            oracle_exact_depth_paths(
                graph,
                arc.to,
                target_segment,
                desired_edges,
                handles,
                links,
                output,
            );
            links.pop();
            handles.pop();
        }
    }

    #[test]
    fn hundred_thousand_segment_chain_uses_linear_parent_arena_work() {
        const SEGMENTS: usize = 100_000;
        let mut chain_config = config();
        chain_config.limits.maximum_pairs = 11;
        chain_config.limits.maximum_placements = 22;
        chain_config.limits.maximum_placement_pairs_per_fragment = 1;
        chain_config.limits.maximum_edges_per_path = u32::try_from(SEGMENTS - 1).unwrap();
        chain_config.limits.maximum_search_states_per_fragment =
            u64::try_from(SEGMENTS + 2).unwrap();
        chain_config
            .limits
            .maximum_search_arc_examinations_per_fragment = u64::try_from(SEGMENTS - 1).unwrap();
        chain_config
            .limits
            .maximum_path_reconstruction_elements_per_fragment =
            u64::try_from(SEGMENTS).unwrap() * 8;
        chain_config.limits.maximum_target_paths_per_fragment = 1;
        chain_config.limits.maximum_compatible_paths_per_fragment = 1;
        chain_config.limits.maximum_graph_sequence_bases = 1 << 20;
        chain_config.limits.graph_memory_bytes = 256 << 20;
        chain_config.limits.search_memory_bytes_per_worker = 32 << 20;
        chain_config.limits.result_memory_bytes = 128 << 20;
        chain_config.limits.analysis_memory_bytes = 512 << 20;

        let calibration_length = SEGMENTS + 10;
        let mut unitigs = Vec::new();
        unitigs.try_reserve_exact(SEGMENTS + 1).unwrap();
        unitigs.push(unitig("cal", calibration_length));
        unitigs.extend((0..SEGMENTS).map(|index| unitig(&format!("u{index:06}"), 3)));
        let mut links = Vec::new();
        links.try_reserve_exact(SEGMENTS - 1).unwrap();
        links.extend(
            (0..SEGMENTS - 1)
                .map(|index| link(&format!("u{index:06}"), &format!("u{:06}", index + 1))),
        );
        let graph =
            PairPathGraph::from_compaction(3, &unitigs, &links, chain_config.limits).unwrap();
        drop(unitigs);
        drop(links);

        let cal = graph.segment_index("cal").unwrap();
        let last_offset = u64::try_from(SEGMENTS).unwrap() + 1;
        let calibration_pairs: Vec<_> = (0..10)
            .map(|ordinal| {
                pair(
                    0,
                    ordinal,
                    placement(cal, 0, 1, Direction::Forward),
                    placement(cal, last_offset, last_offset + 1, Direction::Reverse),
                )
            })
            .collect();
        let replay_pairs = [pair(
            0,
            100,
            placement(
                graph.segment_index("u000000").unwrap(),
                0,
                1,
                Direction::Forward,
            ),
            placement(
                graph.segment_index("u099999").unwrap(),
                2,
                3,
                Direction::Reverse,
            ),
        )];
        let result = super::analyze_pair_paths(
            &graph,
            &evidence_input(&graph, &calibration_pairs, chain_config),
            &evidence_input(&graph, &replay_pairs, chain_config),
            chain_config,
            1,
        )
        .unwrap();
        let decision = &result.decisions[0];
        assert_eq!(
            decision.status,
            PairDecisionStatus::SupportedUniqueExistingPath
        );
        assert_eq!(decision.compatible_paths[0].path.handles.len(), SEGMENTS);
        assert_eq!(
            decision.ledger.search_states_created,
            u64::try_from(SEGMENTS + 1).unwrap()
        );
        assert_eq!(
            decision.ledger.search_arcs_examined,
            u64::try_from(SEGMENTS - 1).unwrap()
        );
        assert_eq!(
            decision.ledger.path_reconstruction_elements,
            u64::try_from(SEGMENTS * 8 - 4).unwrap()
        );
    }
}
