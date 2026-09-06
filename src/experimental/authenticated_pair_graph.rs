//! Source-authenticated adapter from a read-witnessed child to paired mapping.
//!
//! [`PairPathGraph`] remains a freely constructible, explicitly unverified
//! topology value. This module is the only public route to source-backed pair
//! placement: it replays the witnessed child against its exact compacted graph
//! and transition ledger, deterministically materializes mapper targets, and
//! seals every ancestry root into an opaque adapter.
//!
//! External callers cannot invoke the raw mapper boundary:
//!
//! ```compile_fail
//! use veritasm::experimental::pair_mapper::produce_pair_placement_evidence_with_ancestry;
//! ```
//!
//! ```compile_fail
//! use veritasm::experimental::authenticated_pair_graph::PairGraphAncestry;
//! ```
//!
//! A freely constructed pair-path graph also cannot be promoted through the
//! authenticated producer:
//!
//! ```compile_fail
//! use veritasm::experimental::authenticated_pair_graph::produce_authenticated_pair_placement_evidence;
//! use veritasm::experimental::pair_mapper::PairMapperConfig;
//! use veritasm::experimental::pair_path::PairPathGraph;
//! use veritasm::spool::Spool;
//! fn promote(spool: &Spool, graph: &PairPathGraph, config: PairMapperConfig) {
//!     let _ = produce_authenticated_pair_placement_evidence(spool, graph, config);
//! }
//! ```
//!
//! Nor can they construct or duplicate the opaque adapter:
//!
//! ```compile_fail
//! use veritasm::experimental::authenticated_pair_graph::AuthenticatedPairGraph;
//! fn duplicate(graph: &AuthenticatedPairGraph) -> AuthenticatedPairGraph {
//!     graph.clone()
//! }
//! ```
//!
//! ```compile_fail
//! use veritasm::experimental::authenticated_pair_graph::AuthenticatedPairGraph;
//! fn replace_root(graph: &mut AuthenticatedPairGraph) {
//!     graph.authentication_root_sha256 = [7; 32];
//! }
//! ```

use super::compacted_dbg::{AuthenticatedCompactedGraph, CompactedTopology, UnitigOrientation};
use super::evidence_reconstruction::{
    AuthenticatedWitnessedChild, ReconstructionLimits, SourceEquivalence, WitnessedSegment,
};
use super::pair_mapper::{
    produce_pair_placement_evidence_with_ancestry, PairMapperConfig, PairMapperResult,
};
use super::pair_path::{
    analyze_authenticated_pair_paths as analyze_sealed_pair_paths, AuthenticatedPairPathResult,
    PairPathConfig, PairPathGraph, WorkLimits,
};
use super::transition_witness::{transition_source_root, TransitionLedger};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::model::{AvailabilityU64, GraphLink, Topology, Unitig};
use crate::spool::Spool;
use sha2::{Digest, Sha256};
use std::mem::size_of;

pub const ALGORITHM_ID: &str = "authenticated_witnessed_child_pair_graph_adapter";
pub const ALGORITHM_VERSION: &str = "experimental-1";

const ACCOUNTING_MARGIN_BYTES: u64 = 16_384;
const WITNESSED_ID_HEX_BYTES: u64 = 64;
const PAIR_GRAPH_SCRATCH_BYTES_PER_SEGMENT: u64 = 128;
const PAIR_GRAPH_SCRATCH_BYTES_PER_LINK: u64 = 128;
const AUTHENTICATED_PAIR_GRAPH_DOMAIN: &[u8] = b"veritasm:authenticated-pair-graph-adapter:v1\0";

/// Crate-visible type, but module-private minting authority, required by the
/// raw exact pair mapper.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PairGraphAncestry {
    source_root_sha256: [u8; 32],
    source_equivalence_root_sha256: [u8; 32],
    compacted_graph_ancestry_root_sha256: [u8; 32],
    transition_root_sha256: [u8; 32],
    witnessed_child_root_sha256: [u8; 32],
    witnessed_child_authentication_root_sha256: [u8; 32],
    pair_graph_root_sha256: [u8; 32],
    authentication_root_sha256: [u8; 32],
}

impl PairGraphAncestry {
    #[allow(clippy::too_many_arguments)]
    fn new(
        source_root_sha256: [u8; 32],
        source_equivalence_root_sha256: [u8; 32],
        compacted_graph_ancestry_root_sha256: [u8; 32],
        transition_root_sha256: [u8; 32],
        witnessed_child_root_sha256: [u8; 32],
        witnessed_child_authentication_root_sha256: [u8; 32],
        pair_graph_root_sha256: [u8; 32],
        authentication_root_sha256: [u8; 32],
    ) -> Result<Self> {
        let value = Self {
            source_root_sha256,
            source_equivalence_root_sha256,
            compacted_graph_ancestry_root_sha256,
            transition_root_sha256,
            witnessed_child_root_sha256,
            witnessed_child_authentication_root_sha256,
            pair_graph_root_sha256,
            authentication_root_sha256,
        };
        value.validate(source_root_sha256, pair_graph_root_sha256)?;
        Ok(value)
    }

    pub(super) fn validate(
        &self,
        expected_source_root_sha256: [u8; 32],
        expected_pair_graph_root_sha256: [u8; 32],
    ) -> Result<()> {
        if [
            self.source_root_sha256,
            self.source_equivalence_root_sha256,
            self.compacted_graph_ancestry_root_sha256,
            self.transition_root_sha256,
            self.witnessed_child_root_sha256,
            self.witnessed_child_authentication_root_sha256,
            self.pair_graph_root_sha256,
            self.authentication_root_sha256,
        ]
        .contains(&[0; 32])
        {
            return Err(integrity("pair-graph ancestry contains an unset digest"));
        }
        if self.source_root_sha256 != expected_source_root_sha256
            || self.pair_graph_root_sha256 != expected_pair_graph_root_sha256
        {
            return Err(integrity(
                "pair-graph ancestry differs from the mapper source or exact graph",
            ));
        }
        let expected = pair_graph_authentication_root_sha256(
            self.source_root_sha256,
            self.source_equivalence_root_sha256,
            self.compacted_graph_ancestry_root_sha256,
            self.transition_root_sha256,
            self.witnessed_child_root_sha256,
            self.witnessed_child_authentication_root_sha256,
            self.pair_graph_root_sha256,
        );
        if expected != self.authentication_root_sha256 {
            return Err(integrity(
                "pair-graph ancestry authentication root is inconsistent",
            ));
        }
        Ok(())
    }

    pub(super) const fn source_equivalence_root_sha256(&self) -> [u8; 32] {
        self.source_equivalence_root_sha256
    }

    pub(super) const fn compacted_graph_ancestry_root_sha256(&self) -> [u8; 32] {
        self.compacted_graph_ancestry_root_sha256
    }

    pub(super) const fn transition_root_sha256(&self) -> [u8; 32] {
        self.transition_root_sha256
    }

    pub(super) const fn witnessed_child_root_sha256(&self) -> [u8; 32] {
        self.witnessed_child_root_sha256
    }

    pub(super) const fn witnessed_child_authentication_root_sha256(&self) -> [u8; 32] {
        self.witnessed_child_authentication_root_sha256
    }

    pub(super) const fn pair_graph_root_sha256(&self) -> [u8; 32] {
        self.pair_graph_root_sha256
    }

    pub(super) const fn authentication_root_sha256(&self) -> [u8; 32] {
        self.authentication_root_sha256
    }

    #[cfg(test)]
    pub(super) fn for_unverified_mapper_test(
        source_root_sha256: [u8; 32],
        pair_graph_root_sha256: [u8; 32],
    ) -> Result<Self> {
        let derive = |domain: &[u8]| -> [u8; 32] {
            let mut hasher = Sha256::new();
            hasher.update(domain);
            hasher.update(source_root_sha256);
            hasher.update(pair_graph_root_sha256);
            hasher.finalize().into()
        };
        let source_equivalence_root = derive(b"veritasm:test-pair-source-equivalence:v1\0");
        let compacted_ancestry_root = derive(b"veritasm:test-pair-compacted-ancestry:v1\0");
        let transition_root = derive(b"veritasm:test-pair-transition:v1\0");
        let child_root = derive(b"veritasm:test-pair-child:v1\0");
        let child_authentication_root = derive(b"veritasm:test-pair-child-authentication:v1\0");
        let authentication_root = pair_graph_authentication_root_sha256(
            source_root_sha256,
            source_equivalence_root,
            compacted_ancestry_root,
            transition_root,
            child_root,
            child_authentication_root,
            pair_graph_root_sha256,
        );
        Self::new(
            source_root_sha256,
            source_equivalence_root,
            compacted_ancestry_root,
            transition_root,
            child_root,
            child_authentication_root,
            pair_graph_root_sha256,
            authentication_root,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn pair_graph_authentication_root_sha256(
    source_root_sha256: [u8; 32],
    source_equivalence_root_sha256: [u8; 32],
    compacted_graph_ancestry_root_sha256: [u8; 32],
    transition_root_sha256: [u8; 32],
    witnessed_child_root_sha256: [u8; 32],
    witnessed_child_authentication_root_sha256: [u8; 32],
    pair_graph_root_sha256: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(AUTHENTICATED_PAIR_GRAPH_DOMAIN);
    hasher.update(ALGORITHM_ID.as_bytes());
    hasher.update([0]);
    hasher.update(ALGORITHM_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(source_root_sha256);
    hasher.update(source_equivalence_root_sha256);
    hasher.update(compacted_graph_ancestry_root_sha256);
    hasher.update(transition_root_sha256);
    hasher.update(witnessed_child_root_sha256);
    hasher.update(witnessed_child_authentication_root_sha256);
    hasher.update(pair_graph_root_sha256);
    hasher.finalize().into()
}

/// Finite cardinality and owned-payload bounds for one adapter construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairGraphAdapterLimits {
    pub maximum_segments: u64,
    pub maximum_links: u64,
    pub maximum_sequence_bases: u64,
    /// Conservative admission for adapter-owned targets, temporary links, and
    /// the configured pair-graph allowance. This is not a process-RSS claim.
    pub maximum_accounted_bytes: u64,
    pub pair_path: WorkLimits,
}

/// Opaque source-backed pair-graph and exact mapper-target capability.
///
/// The complete graph, target sequences, and ancestry token are private. The
/// type has no public constructor, no mutation method, and intentionally does
/// not implement `Clone`.
#[derive(Debug)]
pub struct AuthenticatedPairGraph {
    source_root_sha256: [u8; 32],
    source_equivalence_root_sha256: [u8; 32],
    compacted_graph_ancestry_root_sha256: [u8; 32],
    transition_root_sha256: [u8; 32],
    witnessed_child_root_sha256: [u8; 32],
    witnessed_child_authentication_root_sha256: [u8; 32],
    authentication_root_sha256: [u8; 32],
    graph: PairPathGraph,
    mapper_targets: Vec<Unitig>,
    ancestry: PairGraphAncestry,
    segment_count: u64,
    link_count: u64,
    sequence_bases: u64,
    adapter_owned_bytes: u64,
    admitted_construction_bytes: u64,
    limits: PairGraphAdapterLimits,
}

impl AuthenticatedPairGraph {
    pub const fn source_root_sha256(&self) -> [u8; 32] {
        self.source_root_sha256
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
        self.graph.graph_root_sha256()
    }

    pub const fn authentication_root_sha256(&self) -> [u8; 32] {
        self.authentication_root_sha256
    }

    pub const fn segment_count(&self) -> u64 {
        self.segment_count
    }

    pub const fn link_count(&self) -> u64 {
        self.link_count
    }

    pub const fn sequence_bases(&self) -> u64 {
        self.sequence_bases
    }

    pub const fn adapter_owned_bytes(&self) -> u64 {
        self.adapter_owned_bytes
    }

    pub const fn admitted_construction_bytes(&self) -> u64 {
        self.admitted_construction_bytes
    }

    pub const fn limits(&self) -> PairGraphAdapterLimits {
        self.limits
    }

    fn validate_integrity(&self) -> Result<()> {
        self.ancestry
            .validate(self.source_root_sha256, self.graph.graph_root_sha256())?;
        if self.source_equivalence_root_sha256 != self.ancestry.source_equivalence_root_sha256()
            || self.compacted_graph_ancestry_root_sha256
                != self.ancestry.compacted_graph_ancestry_root_sha256()
            || self.transition_root_sha256 != self.ancestry.transition_root_sha256()
            || self.witnessed_child_root_sha256 != self.ancestry.witnessed_child_root_sha256()
            || self.witnessed_child_authentication_root_sha256
                != self.ancestry.witnessed_child_authentication_root_sha256()
            || self.authentication_root_sha256 != self.ancestry.authentication_root_sha256()
        {
            return Err(integrity(
                "authenticated pair graph differs from its sealed ancestry token",
            ));
        }
        let segment_count = usize_to_u64(
            self.mapper_targets.len(),
            "authenticated pair-graph segment count",
        )?;
        let link_count = usize_to_u64(
            self.graph.link_catalog().len(),
            "authenticated pair-graph link count",
        )?;
        let sequence_bases = self.mapper_targets.iter().try_fold(0u64, |sum, target| {
            checked_add(
                sum,
                usize_to_u64(
                    target.sequence.len(),
                    "authenticated pair-graph target length",
                )?,
                "authenticated pair-graph sequence total overflow",
            )
        })?;
        if segment_count != self.segment_count
            || link_count != self.link_count
            || sequence_bases != self.sequence_bases
            || self.segment_count > self.limits.maximum_segments
            || self.link_count > self.limits.maximum_links
            || self.sequence_bases > self.limits.maximum_sequence_bases
        {
            return Err(integrity(
                "authenticated pair-graph cardinality or sequence accounting changed",
            ));
        }
        validate_mapper_targets(&self.graph, &self.mapper_targets)?;
        let owned = checked_add(
            checked_add(
                unitig_owned_bytes(&self.mapper_targets, self.mapper_targets.capacity())?,
                self.graph.accounted_allocation_bytes(),
                "authenticated pair-graph owned bytes overflow",
            )?,
            checked_add(
                usize_to_u64(size_of::<Self>(), "authenticated pair-graph header bytes")?,
                ACCOUNTING_MARGIN_BYTES,
                "authenticated pair-graph margin overflow",
            )?,
            "authenticated pair-graph owned bytes overflow",
        )?;
        if owned != self.adapter_owned_bytes
            || owned > self.limits.maximum_accounted_bytes
            || self.admitted_construction_bytes > self.limits.maximum_accounted_bytes
        {
            return Err(integrity(
                "authenticated pair-graph owned-memory accounting changed",
            ));
        }
        Ok(())
    }
}

/// Replay and authenticate the complete witnessed child before adapting it to
/// the exact linear-unitig pair-mapping domain.
pub fn adapt_authenticated_witnessed_child(
    child: &AuthenticatedWitnessedChild,
    compacted: &AuthenticatedCompactedGraph,
    transitions: &TransitionLedger,
    reconstruction_limits: ReconstructionLimits,
    limits: PairGraphAdapterLimits,
) -> Result<AuthenticatedPairGraph> {
    validate_limits(limits)?;
    child.validate_against_sources(compacted, transitions, reconstruction_limits)?;
    let view = child.view();
    if view.source_equivalence() != SourceEquivalence::AuthenticatedSpoolDescriptor
        || view.source_root() != compacted.source_root()
        || view.source_root() != transitions.source_root()
        || view.graph_ancestry_root() != compacted.ancestry_root()
        || view.transition_root() != transitions.transition_root()
    {
        return Err(integrity(
            "witnessed child, compacted graph, and transitions have different ancestry",
        ));
    }

    let segment_count = usize_to_u64(view.segments().len(), "witnessed segment count")?;
    let link_count = usize_to_u64(view.links().len(), "witnessed link count")?;
    let sequence_bases = view.segments().iter().try_fold(0u64, |sum, segment| {
        checked_add(
            sum,
            usize_to_u64(segment.sequence.len(), "witnessed segment length")?,
            "witnessed sequence-base total overflow",
        )
    })?;
    if segment_count > limits.maximum_segments
        || link_count > limits.maximum_links
        || sequence_bases > limits.maximum_sequence_bases
        || sequence_bases > limits.pair_path.maximum_graph_sequence_bases
    {
        return Err(resource(
            "witnessed child exceeds authenticated pair-graph cardinality limits",
        ));
    }
    let projected_unitig_bytes = projected_unitig_bytes(segment_count, sequence_bases)?;
    let projected_link_bytes = projected_link_bytes(link_count)?;
    let projected_pair_graph_scratch = checked_add(
        checked_mul(
            segment_count,
            PAIR_GRAPH_SCRATCH_BYTES_PER_SEGMENT,
            "pair-graph segment scratch admission overflow",
        )?,
        checked_mul(
            link_count,
            PAIR_GRAPH_SCRATCH_BYTES_PER_LINK,
            "pair-graph link scratch admission overflow",
        )?,
        "pair-graph scratch admission overflow",
    )?;
    let admitted_construction_bytes = [
        projected_unitig_bytes,
        projected_link_bytes,
        limits.pair_path.graph_memory_bytes,
        projected_pair_graph_scratch,
        usize_to_u64(
            size_of::<AuthenticatedPairGraph>(),
            "pair-graph adapter header bytes",
        )?,
        ACCOUNTING_MARGIN_BYTES,
    ]
    .into_iter()
    .try_fold(0u64, |sum, value| {
        checked_add(sum, value, "pair-graph construction admission overflow")
    })?;
    if admitted_construction_bytes > limits.maximum_accounted_bytes {
        return Err(resource(
            "authenticated pair-graph construction exceeds its memory budget",
        ));
    }

    let mut mapper_targets = Vec::new();
    mapper_targets
        .try_reserve_exact(view.segments().len())
        .map_err(|_| resource("cannot allocate authenticated pair-graph mapper targets"))?;
    for segment in view.segments() {
        mapper_targets.push(mapper_target(segment)?);
    }
    mapper_targets.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    if unitig_owned_bytes(&mapper_targets, mapper_targets.capacity())? > projected_unitig_bytes {
        return Err(resource(
            "allocator exceeded authenticated pair-graph target admission",
        ));
    }

    let mut materialized_links = Vec::new();
    materialized_links
        .try_reserve_exact(view.links().len())
        .map_err(|_| resource("cannot allocate authenticated pair-graph links"))?;
    for link in view.links() {
        if link.overlap_bases != view.k() - 1 {
            return Err(integrity(
                "witnessed child link does not have the exact k-minus-one overlap",
            ));
        }
        materialized_links.push(GraphLink {
            from_segment: lower_hex_digest(&link.from.0)?,
            from_orientation: orientation_symbol(link.from_orientation),
            to_segment: lower_hex_digest(&link.to.0)?,
            to_orientation: orientation_symbol(link.to_orientation),
        });
    }
    materialized_links.sort_unstable();
    let actual_link_bytes =
        graph_link_owned_bytes(&materialized_links, materialized_links.capacity())?;
    if actual_link_bytes > projected_link_bytes {
        return Err(resource(
            "allocator exceeded authenticated pair-graph link admission",
        ));
    }

    let graph = PairPathGraph::from_unverified_compaction(
        view.k(),
        &mapper_targets,
        &materialized_links,
        limits.pair_path,
    )?;
    if graph.segment_catalog().len() != view.segments().len()
        || graph.link_catalog().len() != view.links().len()
    {
        return Err(integrity(
            "pair-graph adapter did not conserve witnessed segments and links",
        ));
    }
    let source_root_sha256 = view.source_root();
    let source_equivalence_root_sha256 = compacted.source_equivalence_root();
    let compacted_graph_ancestry_root_sha256 = compacted.ancestry_root();
    let transition_root_sha256 = transitions.transition_root();
    let witnessed_child_root_sha256 = view.child_root();
    let witnessed_child_authentication_root_sha256 = view.authentication_root();
    let pair_graph_root_sha256 = graph.graph_root_sha256();
    let authentication_root_sha256 = pair_graph_authentication_root_sha256(
        source_root_sha256,
        source_equivalence_root_sha256,
        compacted_graph_ancestry_root_sha256,
        transition_root_sha256,
        witnessed_child_root_sha256,
        witnessed_child_authentication_root_sha256,
        pair_graph_root_sha256,
    );
    let ancestry = PairGraphAncestry::new(
        source_root_sha256,
        source_equivalence_root_sha256,
        compacted_graph_ancestry_root_sha256,
        transition_root_sha256,
        witnessed_child_root_sha256,
        witnessed_child_authentication_root_sha256,
        pair_graph_root_sha256,
        authentication_root_sha256,
    )?;
    let adapter_owned_bytes = checked_add(
        checked_add(
            unitig_owned_bytes(&mapper_targets, mapper_targets.capacity())?,
            graph.accounted_allocation_bytes(),
            "authenticated pair-graph owned bytes overflow",
        )?,
        checked_add(
            usize_to_u64(
                size_of::<AuthenticatedPairGraph>(),
                "authenticated pair-graph header bytes",
            )?,
            ACCOUNTING_MARGIN_BYTES,
            "authenticated pair-graph margin overflow",
        )?,
        "authenticated pair-graph owned bytes overflow",
    )?;
    if adapter_owned_bytes > limits.maximum_accounted_bytes {
        return Err(resource(
            "authenticated pair-graph retained data exceeds its memory budget",
        ));
    }
    let adapter = AuthenticatedPairGraph {
        source_root_sha256,
        source_equivalence_root_sha256,
        compacted_graph_ancestry_root_sha256,
        transition_root_sha256,
        witnessed_child_root_sha256,
        witnessed_child_authentication_root_sha256,
        authentication_root_sha256,
        graph,
        mapper_targets,
        ancestry,
        segment_count,
        link_count,
        sequence_bases,
        adapter_owned_bytes,
        admitted_construction_bytes,
        limits,
    };
    adapter.validate_integrity()?;
    Ok(adapter)
}

/// Produce paired placements only after the spool and every graph-ancestry
/// root agree with the opaque adapter.
pub fn produce_authenticated_pair_placement_evidence(
    spool: &Spool,
    graph: &AuthenticatedPairGraph,
    config: PairMapperConfig,
) -> Result<PairMapperResult> {
    graph.validate_integrity()?;
    let replay_source_root = transition_source_root(spool)?;
    if replay_source_root != graph.source_root_sha256
        || config.expected_common_source_root_sha256 != graph.source_root_sha256
        || config.expected_graph_root_sha256 != graph.graph.graph_root_sha256()
    {
        return Err(integrity(
            "paired spool, mapper configuration, and authenticated graph roots differ",
        ));
    }
    let result = produce_pair_placement_evidence_with_ancestry(
        spool,
        &graph.graph,
        &graph.mapper_targets,
        config,
        &graph.ancestry,
    )?;
    validate_pair_mapper_binding(graph, &result)?;
    Ok(result)
}

/// Analyze paired paths only when the placement result is bound to this exact
/// authenticated graph capability.
pub fn analyze_authenticated_pair_paths(
    graph: &AuthenticatedPairGraph,
    placements: &PairMapperResult,
    config: PairPathConfig,
    worker_threads: usize,
) -> Result<AuthenticatedPairPathResult> {
    graph.validate_integrity()?;
    validate_pair_mapper_binding(graph, placements)?;
    analyze_sealed_pair_paths(
        &graph.graph,
        placements.authenticated_evidence_pair(),
        config,
        worker_threads,
    )
}

fn validate_pair_mapper_binding(
    graph: &AuthenticatedPairGraph,
    placements: &PairMapperResult,
) -> Result<()> {
    if placements.common_source_root_sha256() != graph.source_root_sha256
        || placements.source_equivalence_root_sha256() != graph.source_equivalence_root_sha256
        || placements.compacted_graph_ancestry_root_sha256()
            != graph.compacted_graph_ancestry_root_sha256
        || placements.transition_root_sha256() != graph.transition_root_sha256
        || placements.witnessed_child_root_sha256() != graph.witnessed_child_root_sha256
        || placements.witnessed_child_authentication_root_sha256()
            != graph.witnessed_child_authentication_root_sha256
        || placements.exact_pair_graph_root_sha256() != graph.graph.graph_root_sha256()
        || placements.authenticated_pair_graph_root_sha256() != graph.authentication_root_sha256
        || placements.calibration_input().certificate.graph_root_sha256
            != graph.graph.graph_root_sha256()
        || placements.replay_input().certificate.graph_root_sha256
            != graph.graph.graph_root_sha256()
    {
        return Err(integrity(
            "pair-mapper result is not bound to the supplied authenticated graph",
        ));
    }
    Ok(())
}

fn mapper_target(segment: &WitnessedSegment) -> Result<Unitig> {
    let mut sequence = Vec::new();
    sequence
        .try_reserve_exact(segment.sequence.len())
        .map_err(|_| resource("cannot allocate authenticated mapper-target sequence"))?;
    sequence.extend_from_slice(&segment.sequence);
    let sequence_digest: [u8; 32] = Sha256::digest(&sequence).into();
    let edge_steps = usize_to_u64(segment.steps.len(), "witnessed segment edge-step count")?;
    Ok(Unitig {
        id: lower_hex_digest(&segment.id.0)?,
        sequence,
        topology: match segment.topology {
            CompactedTopology::Linear => Topology::Linear,
            CompactedTopology::ClosedWalk => Topology::ClosedGraphWalk,
        },
        edge_steps,
        canonical_kmers: edge_steps,
        minimum_support: segment.minimum_edge_support,
        lower_median_support: segment.lower_median_edge_support,
        maximum_support: segment.maximum_edge_support,
        enumeration_complete_read_placements: AvailabilityU64::NotAvailable(
            "not evaluated before authenticated pair mapping",
        ),
        single_group_read_instances: AvailabilityU64::NotAvailable(
            "not evaluated before authenticated pair mapping",
        ),
        multi_group_read_instances_with_group: AvailabilityU64::NotAvailable(
            "not evaluated before authenticated pair mapping",
        ),
        placement_enumeration_status: "not_evaluated_before_authenticated_pair_mapping",
        sequence_sha256: lower_hex_digest(&sequence_digest)?,
    })
}

fn validate_mapper_targets(graph: &PairPathGraph, targets: &[Unitig]) -> Result<()> {
    if graph.segment_catalog().len() != targets.len() {
        return Err(integrity(
            "authenticated mapper-target count differs from pair graph",
        ));
    }
    if targets
        .windows(2)
        .any(|window| window[0].id >= window[1].id)
    {
        return Err(integrity(
            "authenticated mapper targets are duplicate or unordered",
        ));
    }
    for (catalog, target) in graph.segment_catalog().iter().zip(targets) {
        let sequence_digest: [u8; 32] = Sha256::digest(&target.sequence).into();
        if catalog.id != target.id
            || catalog.length
                != usize_to_u64(target.sequence.len(), "authenticated mapper-target length")?
            || catalog.topology != target.topology
            || catalog.sequence_sha256 != sequence_digest
            || !hex_digest_matches(&target.sequence_sha256, &sequence_digest)
        {
            return Err(integrity(
                "authenticated mapper target differs from exact pair graph",
            ));
        }
    }
    Ok(())
}

fn validate_limits(limits: PairGraphAdapterLimits) -> Result<()> {
    if limits.maximum_segments == 0
        || limits.maximum_links == 0
        || limits.maximum_sequence_bases == 0
        || limits.maximum_accounted_bytes == 0
    {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "authenticated pair-graph limits must all be positive",
        ));
    }
    Ok(())
}

fn projected_unitig_bytes(segments: u64, sequence_bases: u64) -> Result<u64> {
    let per_segment = checked_add(
        usize_to_u64(size_of::<Unitig>(), "unitig header size")?,
        checked_mul(
            WITNESSED_ID_HEX_BYTES,
            2,
            "unitig identifier and sequence-digest bytes overflow",
        )?,
        "projected unitig bytes overflow",
    )?;
    checked_add(
        checked_mul(segments, per_segment, "projected unitig bytes overflow")?,
        sequence_bases,
        "projected unitig bytes overflow",
    )
}

fn projected_link_bytes(links: u64) -> Result<u64> {
    let per_link = checked_add(
        usize_to_u64(size_of::<GraphLink>(), "graph-link header size")?,
        checked_mul(
            WITNESSED_ID_HEX_BYTES,
            2,
            "graph-link identifier bytes overflow",
        )?,
        "projected graph-link bytes overflow",
    )?;
    checked_mul(links, per_link, "projected graph-link bytes overflow")
}

fn unitig_owned_bytes(unitigs: &[Unitig], outer_capacity: usize) -> Result<u64> {
    let mut bytes = checked_mul(
        usize_to_u64(outer_capacity, "mapper-target vector capacity")?,
        usize_to_u64(size_of::<Unitig>(), "mapper-target header size")?,
        "mapper-target vector bytes overflow",
    )?;
    for unitig in unitigs {
        for capacity in [
            unitig.id.capacity(),
            unitig.sequence.capacity(),
            unitig.sequence_sha256.capacity(),
        ] {
            bytes = checked_add(
                bytes,
                usize_to_u64(capacity, "mapper-target owned capacity")?,
                "mapper-target owned bytes overflow",
            )?;
        }
    }
    Ok(bytes)
}

fn graph_link_owned_bytes(links: &[GraphLink], outer_capacity: usize) -> Result<u64> {
    let mut bytes = checked_mul(
        usize_to_u64(outer_capacity, "graph-link vector capacity")?,
        usize_to_u64(size_of::<GraphLink>(), "graph-link header size")?,
        "graph-link vector bytes overflow",
    )?;
    for link in links {
        bytes = checked_add(
            bytes,
            checked_add(
                usize_to_u64(link.from_segment.capacity(), "graph-link source capacity")?,
                usize_to_u64(link.to_segment.capacity(), "graph-link target capacity")?,
                "graph-link identifier capacity overflow",
            )?,
            "graph-link owned bytes overflow",
        )?;
    }
    Ok(bytes)
}

fn lower_hex_digest(value: &[u8; 32]) -> Result<String> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::new();
    output
        .try_reserve_exact(
            usize::try_from(WITNESSED_ID_HEX_BYTES).map_err(|_| {
                overflow("witnessed identifier hex length does not fit address space")
            })?,
        )
        .map_err(|_| resource("cannot allocate witnessed identifier hex"))?;
    for &byte in value {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Ok(output)
}

fn hex_digest_matches(encoded: &str, digest: &[u8; 32]) -> bool {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    encoded.len() == 64
        && encoded
            .as_bytes()
            .chunks_exact(2)
            .zip(digest)
            .all(|(pair, byte)| {
                pair[0] == DIGITS[usize::from(byte >> 4)]
                    && pair[1] == DIGITS[usize::from(byte & 0x0f)]
            })
}

const fn orientation_symbol(orientation: UnitigOrientation) -> char {
    match orientation {
        UnitigOrientation::Forward => '+',
        UnitigOrientation::ReverseComplement => '-',
    }
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

fn resource(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn integrity(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegrityArtifact, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
    use crate::experimental::compacted_dbg::{compact_retained_counts, CompactedGraphLimits};
    use crate::experimental::evidence_reconstruction::constrain_authenticated_compacted_child;
    use crate::experimental::external_reduce::ExternalPartitionLimits;
    use crate::experimental::pair_mapper::{
        pair_mapping_source_root_sha256, CalibrationSplitRule, PairMapperLimits,
    };
    use crate::experimental::pair_path::{ModelConfig, PlacementDomain};
    use crate::experimental::retention::{
        retain_spool_external_counts, RetentionLimits, RetentionRule,
    };
    use crate::experimental::spool_external::SpoolExternalOptions;
    use crate::experimental::transition_witness::{
        build_transition_ledger, TransitionLedgerLimits, TransitionLedgerOptions,
    };
    use crate::spool::create_spool;
    use std::fs;
    use tempfile::{tempdir, TempDir};

    struct Fixture {
        _directory: TempDir,
        spool: Spool,
        compacted: AuthenticatedCompactedGraph,
        transitions: TransitionLedger,
        child: AuthenticatedWitnessedChild,
    }

    fn scientific() -> ScientificConfig {
        ScientificConfig::resolve(
            3,
            Profile::RetainAll,
            SupportUnit::AcceptedWindowOccurrence,
            None,
            0,
            false,
        )
        .unwrap()
    }

    fn spool_limits() -> Limits {
        Limits {
            memory_budget_bytes: 64 << 20,
            max_spool_bytes: 64 << 20,
            max_temp_bytes: 128 << 20,
            ..Limits::default()
        }
    }

    fn external_limits() -> ExternalPartitionLimits {
        ExternalPartitionLimits {
            max_segments: 100,
            max_input_bases: 1_000_000,
            max_windows: 1_000_000,
            max_distinct_kmers: 1_000_000,
            max_memory_bytes: 32 << 20,
            sort_buffer_bytes: 4 * 1024,
            io_buffer_bytes: 128,
            max_temp_bytes: 96 << 20,
            max_run_files: 1_000,
            merge_fan_in: 2,
            max_open_files: 3,
        }
    }

    fn retention_limits() -> RetentionLimits {
        RetentionLimits {
            max_raw_keys: 100_000,
            max_retained_keys: 100_000,
            max_accounted_bytes: 32 << 20,
        }
    }

    fn compacted_limits() -> CompactedGraphLimits {
        CompactedGraphLimits {
            max_canonical_edges: 100_000,
            max_oriented_handles: 200_000,
            max_literal_nodes: 400_000,
            max_unitigs: 100_000,
            max_link_candidates: 10_000_000,
            max_output_bases: 20_000_000,
            max_accounted_bytes: 512 << 20,
        }
    }

    fn reconstruction_limits() -> ReconstructionLimits {
        ReconstructionLimits {
            max_input_edges: 100_000,
            max_topology_candidates: 1_000_000,
            max_decision_rows: 1_100_000,
            max_segments: 100_000,
            max_links: 1_000_000,
            max_output_bases: 20_000_000,
            max_accounted_bytes: 512 << 20,
            max_verification_scratch_bytes: 127,
        }
    }

    fn transition_limits() -> TransitionLedgerLimits {
        TransitionLedgerLimits {
            max_events: 1_000_000,
            max_rows: 100_000,
            max_fragment_decode_bytes: 4 << 20,
            max_fragment_windows: 1_000_000,
            max_memory_bytes: 32 << 20,
            sort_buffer_bytes: 128 * 1024,
            max_temp_bytes: 64 << 20,
            max_run_files: 1_000,
            merge_fan_in: 4,
            max_open_files: 5,
        }
    }

    fn pair_path_limits() -> WorkLimits {
        WorkLimits {
            maximum_pairs: 100,
            maximum_placements: 10_000,
            maximum_placement_pairs_per_fragment: 100,
            maximum_edges_per_path: 64,
            maximum_search_states_per_fragment: 10_000,
            maximum_search_arc_examinations_per_fragment: 100_000,
            maximum_path_reconstruction_elements_per_fragment: 100_000,
            maximum_target_paths_per_fragment: 1_000,
            maximum_compatible_paths_per_fragment: 100,
            maximum_worker_threads: 4,
            maximum_mapper_algorithm_bytes: 256,
            maximum_mapper_version_bytes: 128,
            maximum_mapper_parameter_bytes: 4 << 10,
            maximum_graph_sequence_bases: 20_000_000,
            graph_memory_bytes: 64 << 20,
            search_memory_bytes_per_worker: 8 << 20,
            result_memory_bytes: 128 << 20,
            analysis_memory_bytes: 320 << 20,
        }
    }

    fn adapter_limits() -> PairGraphAdapterLimits {
        PairGraphAdapterLimits {
            maximum_segments: 100_000,
            maximum_links: 1_000_000,
            maximum_sequence_bases: 20_000_000,
            maximum_accounted_bytes: 256 << 20,
            pair_path: pair_path_limits(),
        }
    }

    fn write_paired_inputs(directory: &TempDir, variant: u8) -> InputSpec {
        let read1 = directory.path().join("r1.fastq");
        let read2 = directory.path().join("r2.fastq");
        let (r1_tail, r2_tail) = if variant == 0 {
            ("TTAACGTT", "AACGGTTA")
        } else {
            ("CCGTAACC", "GGTTAACG")
        };
        fs::write(
            &read1,
            format!(
                "@f0/1\nAACGTTAC\n+\nIIIIIIII\n@f1/1\nACGTAACC\n+\nIIIIIIII\n@f2/1\n{r1_tail}\n+\nIIIIIIII\n@f3/1\nCGTTAACG\n+\nIIIIIIII\n"
            ),
        )
        .unwrap();
        fs::write(
            &read2,
            format!(
                "@f0/2\nGTAACGTT\n+\nIIIIIIII\n@f1/2\nGGTTAACG\n+\nIIIIIIII\n@f2/2\n{r2_tail}\n+\nIIIIIIII\n@f3/2\nCGTACCGT\n+\nIIIIIIII\n"
            ),
        )
        .unwrap();
        InputSpec::Paired {
            read1: vec![read1],
            read2: vec![read2],
        }
    }

    fn write_single_input(directory: &TempDir) -> InputSpec {
        let reads = directory.path().join("reads.fastq");
        fs::write(
            &reads,
            b"@s0\nAACGTTAC\n+\nIIIIIIII\n@s1\nGTAACGTT\n+\nIIIIIIII\n",
        )
        .unwrap();
        InputSpec::Single(vec![reads])
    }

    fn fixture(paired: bool, variant: u8) -> Fixture {
        let directory = tempdir().unwrap();
        let input = if paired {
            write_paired_inputs(&directory, variant)
        } else {
            write_single_input(&directory)
        };
        let spool = create_spool(&input, &scientific(), &spool_limits(), directory.path()).unwrap();
        let external_options = SpoolExternalOptions {
            work_dir: directory.path().to_path_buf(),
            k: 3,
            minimizer_length: 2,
            virtual_bucket_count: 5,
            support_unit: SupportUnit::AcceptedWindowOccurrence,
            max_fragment_decode_bytes: 1 << 20,
            max_fragment_windows: 10_000,
            limits: external_limits(),
        };
        let retained = retain_spool_external_counts(
            &spool,
            &external_options,
            RetentionRule::RetainAll,
            retention_limits(),
        )
        .unwrap();
        let compacted = compact_retained_counts(&retained, compacted_limits()).unwrap();
        let transitions = build_transition_ledger(
            &spool,
            &TransitionLedgerOptions {
                work_dir: directory.path().to_path_buf(),
                k: 3,
                limits: transition_limits(),
            },
        )
        .unwrap();
        let child = constrain_authenticated_compacted_child(
            &compacted,
            &transitions,
            reconstruction_limits(),
        )
        .unwrap();
        Fixture {
            _directory: directory,
            spool,
            compacted,
            transitions,
            child,
        }
    }

    fn adapt(fixture: &Fixture) -> AuthenticatedPairGraph {
        adapt_authenticated_witnessed_child(
            &fixture.child,
            &fixture.compacted,
            &fixture.transitions,
            reconstruction_limits(),
            adapter_limits(),
        )
        .unwrap()
    }

    fn compacted_with_rule(fixture: &Fixture, rule: RetentionRule) -> AuthenticatedCompactedGraph {
        let external_options = SpoolExternalOptions {
            work_dir: fixture._directory.path().to_path_buf(),
            k: 3,
            minimizer_length: 2,
            virtual_bucket_count: 5,
            support_unit: SupportUnit::AcceptedWindowOccurrence,
            max_fragment_decode_bytes: 1 << 20,
            max_fragment_windows: 10_000,
            limits: external_limits(),
        };
        let retained = retain_spool_external_counts(
            &fixture.spool,
            &external_options,
            rule,
            retention_limits(),
        )
        .unwrap();
        compact_retained_counts(&retained, compacted_limits()).unwrap()
    }

    fn mapper_config(
        spool: &Spool,
        graph: &AuthenticatedPairGraph,
        worker_threads: u16,
    ) -> PairMapperConfig {
        PairMapperConfig {
            expected_graph_root_sha256: graph.exact_pair_graph_root_sha256(),
            expected_common_source_root_sha256: graph.source_root_sha256(),
            expected_pair_mapping_source_root_sha256: pair_mapping_source_root_sha256(spool)
                .unwrap(),
            expected_minimum_base_quality: spool.scientific_config().min_base_quality,
            seed_length: 3,
            worker_threads,
            split: CalibrationSplitRule {
                salt: [19; 32],
                numerator: 1,
                denominator: 2,
            },
            limits: PairMapperLimits {
                maximum_fragments: 100,
                maximum_reads: 200,
                maximum_bases: 1 << 20,
                maximum_graph_sequence_bases: 20_000_000,
                maximum_mapping_candidates_per_read: 100_000,
                maximum_admitted_mapping_operations: 1_000_000_000,
                maximum_placement_groups: 100_000,
                maximum_placements: 100_000,
                maximum_batch_fragments: 3,
                maximum_decoded_batch_bytes: 8 << 20,
                mapper_index_memory_bytes: 32 << 20,
                mapper_query_memory_bytes_per_worker: 8 << 20,
                result_memory_bytes: 32 << 20,
                total_accounted_memory_bytes: 192 << 20,
                maximum_worker_threads: 4,
                maximum_mapper_parameter_bytes: 4 << 10,
            },
        }
    }

    fn analysis_config() -> PairPathConfig {
        PairPathConfig {
            model: ModelConfig {
                minimum_anchors: 10,
                minimum_dominant_anchors: 10,
                dominance_numerator: 3,
                dominance_denominator: 5,
                maximum_span_p90_minus_p10: u64::MAX,
                maximum_inner_p90_minus_p10: u64::MAX,
            },
            limits: pair_path_limits(),
            minimum_distinct_fragments_for_path: 2,
        }
    }

    #[test]
    fn paired_source_maps_only_through_the_authenticated_adapter() {
        let fixture = fixture(true, 0);
        let graph = adapt(&fixture);
        assert_eq!(
            graph.source_root_sha256(),
            fixture.transitions.source_root()
        );
        assert_eq!(
            graph.segment_count(),
            fixture.child.view().segments().len() as u64
        );
        assert_eq!(
            graph.link_count(),
            fixture.child.view().links().len() as u64
        );
        let placements = produce_authenticated_pair_placement_evidence(
            &fixture.spool,
            &graph,
            mapper_config(&fixture.spool, &graph, 2),
        )
        .unwrap();
        assert_eq!(
            placements.common_source_root_sha256(),
            graph.source_root_sha256()
        );
        assert_eq!(
            placements.source_equivalence_root_sha256(),
            graph.source_equivalence_root_sha256()
        );
        assert_eq!(
            placements.compacted_graph_ancestry_root_sha256(),
            graph.compacted_graph_ancestry_root_sha256()
        );
        assert_eq!(
            placements.transition_root_sha256(),
            graph.transition_root_sha256()
        );
        assert_eq!(
            placements.witnessed_child_root_sha256(),
            graph.witnessed_child_root_sha256()
        );
        assert_eq!(
            placements.witnessed_child_authentication_root_sha256(),
            graph.witnessed_child_authentication_root_sha256()
        );
        assert_eq!(
            placements.exact_pair_graph_root_sha256(),
            graph.exact_pair_graph_root_sha256()
        );
        assert_eq!(
            placements.authenticated_pair_graph_root_sha256(),
            graph.authentication_root_sha256()
        );
        assert_eq!(placements.conservation().authenticated_fragments, 4);
        assert_eq!(placements.conservation().authenticated_reads, 8);
        let analysis =
            analyze_authenticated_pair_paths(&graph, &placements, analysis_config(), 2).unwrap();
        assert_eq!(
            analysis.result().placement_domain,
            PlacementDomain::LinearUnitigOnly
        );
        assert_eq!(
            analysis.result().graph_root_sha256,
            graph.exact_pair_graph_root_sha256()
        );
    }

    #[test]
    fn adapter_and_mapping_roots_are_deterministic_across_workers() {
        let fixture = fixture(true, 0);
        let first_graph = adapt(&fixture);
        let second_graph = adapt(&fixture);
        assert_eq!(
            first_graph.authentication_root_sha256(),
            second_graph.authentication_root_sha256()
        );
        assert_eq!(
            first_graph.exact_pair_graph_root_sha256(),
            second_graph.exact_pair_graph_root_sha256()
        );
        let first = produce_authenticated_pair_placement_evidence(
            &fixture.spool,
            &first_graph,
            mapper_config(&fixture.spool, &first_graph, 1),
        )
        .unwrap();
        let second = produce_authenticated_pair_placement_evidence(
            &fixture.spool,
            &second_graph,
            mapper_config(&fixture.spool, &second_graph, 4),
        )
        .unwrap();
        assert_eq!(first.producer_root_sha256(), second.producer_root_sha256());
        assert_eq!(first.calibration_input(), second.calibration_input());
        assert_eq!(first.replay_input(), second.replay_input());
        assert_ne!(
            first.execution().worker_threads,
            second.execution().worker_threads
        );
    }

    #[test]
    fn root_target_and_configuration_tampering_fail_closed() {
        let fixture = fixture(true, 0);
        let mut graph = adapt(&fixture);
        graph.authentication_root_sha256[0] ^= 1;
        assert_eq!(
            graph.validate_integrity().unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        let mut graph = adapt(&fixture);
        graph.ancestry.transition_root_sha256[0] ^= 1;
        assert_eq!(
            graph.validate_integrity().unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        let mut graph = adapt(&fixture);
        graph.mapper_targets[0].sequence[0] = if graph.mapper_targets[0].sequence[0] == b'A' {
            b'C'
        } else {
            b'A'
        };
        assert_eq!(
            graph.validate_integrity().unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        let graph = adapt(&fixture);
        let mut config = mapper_config(&fixture.spool, &graph, 1);
        config.expected_graph_root_sha256[0] ^= 1;
        assert_eq!(
            produce_authenticated_pair_placement_evidence(&fixture.spool, &graph, config)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn cross_source_capabilities_and_spools_are_rejected() {
        let first = fixture(true, 0);
        let second = fixture(true, 1);
        let same_source_different_retention = compacted_with_rule(
            &first,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
        );
        assert_eq!(
            adapt_authenticated_witnessed_child(
                &first.child,
                &same_source_different_retention,
                &first.transitions,
                reconstruction_limits(),
                adapter_limits(),
            )
            .unwrap_err()
            .code(),
            ErrorCode::IntegrityArtifact
        );
        assert_eq!(
            adapt_authenticated_witnessed_child(
                &first.child,
                &first.compacted,
                &second.transitions,
                reconstruction_limits(),
                adapter_limits(),
            )
            .unwrap_err()
            .code(),
            ErrorCode::IntegrityArtifact
        );
        let graph = adapt(&first);
        let config = mapper_config(&second.spool, &graph, 1);
        assert_eq!(
            produce_authenticated_pair_placement_evidence(&second.spool, &graph, config)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let first_graph = adapt(&first);
        let placements = produce_authenticated_pair_placement_evidence(
            &first.spool,
            &first_graph,
            mapper_config(&first.spool, &first_graph, 1),
        )
        .unwrap();
        let second_graph = adapt(&second);
        assert_eq!(
            analyze_authenticated_pair_paths(&second_graph, &placements, analysis_config(), 1,)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn single_end_source_can_be_reconstructed_but_not_pair_mapped() {
        let fixture = fixture(false, 0);
        let graph = adapt(&fixture);
        let error = produce_authenticated_pair_placement_evidence(
            &fixture.spool,
            &graph,
            mapper_config(&fixture.spool, &graph, 1),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigurationUnsupportedCombination);
    }

    #[test]
    fn adapter_cardinality_and_memory_limits_fail_before_publication() {
        let fixture = fixture(true, 0);
        let baseline = adapt(&fixture);
        let exact_segments = baseline.segment_count();
        let exact_bytes = baseline.admitted_construction_bytes();
        let mut exact_limits = adapter_limits();
        exact_limits.maximum_segments = exact_segments;
        exact_limits.maximum_accounted_bytes = exact_bytes;
        assert!(adapt_authenticated_witnessed_child(
            &fixture.child,
            &fixture.compacted,
            &fixture.transitions,
            reconstruction_limits(),
            exact_limits,
        )
        .is_ok());

        let mut limits = adapter_limits();
        limits.maximum_segments = exact_segments - 1;
        assert_eq!(
            adapt_authenticated_witnessed_child(
                &fixture.child,
                &fixture.compacted,
                &fixture.transitions,
                reconstruction_limits(),
                limits,
            )
            .unwrap_err()
            .code(),
            ErrorCode::ResourceMemory
        );
        let mut limits = adapter_limits();
        limits.maximum_accounted_bytes = exact_bytes - 1;
        assert_eq!(
            adapt_authenticated_witnessed_child(
                &fixture.child,
                &fixture.compacted,
                &fixture.transitions,
                reconstruction_limits(),
                limits,
            )
            .unwrap_err()
            .code(),
            ErrorCode::ResourceMemory
        );
    }
}
