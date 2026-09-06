//! Read-witness-constrained reconstruction of one experimental compacted child.
//!
//! Only an exact `(k + 1)`-mer row from [`super::transition_witness`] can
//! admit an adjacency. Higher-k children, multi-k relations, graph topology,
//! and probabilistic membership are not evidence. This module does not
//! recompact after exclusion and is not reachable from the stable CLI.

use super::compacted_dbg::{
    AuthenticatedCompactedGraph, CompactedEdgeStep, CompactedGraphResult, CompactedLink,
    CompactedTopology, CompactedUnitig, CompactedUnitigId, EdgeOrientation, UnitigOrientation,
};
use super::external_run::WideRunSupportUnit;
use super::transition_witness::{
    transition_endpoints, TransitionLedger, TransitionLedgerView, TransitionRow,
};
use super::wide_kmer::{
    canonical_code, encode_exact_bases, reverse_complement_code, validate_code, PackedKmer,
};
use crate::error::{ErrorCode, Result, VeritasmError};
use sha2::{Digest, Sha256};
use std::mem::size_of;

const RAW_GRAPH_DOMAIN: &[u8] = b"veritasm:read-witnessed-raw-graph:v2\0";
const CONSTRAINED_GRAPH_DOMAIN: &[u8] = b"veritasm:read-witnessed-constrained-graph:v1\0";
const CHILD_ROOT_DOMAIN: &[u8] = b"veritasm:multik-child:v1\0";
const SEGMENT_ID_DOMAIN: &[u8] = b"veritasm:read-witnessed-segment-id:v1\0";
const SEGMENT_PROVENANCE_DOMAIN: &[u8] = b"veritasm:read-witnessed-segment-provenance:v1\0";
const TRANSITION_SET_DOMAIN: &[u8] = b"veritasm:read-witnessed-segment-transitions:v1\0";
const AUTHENTICATED_WITNESSED_CHILD_DOMAIN: &[u8] = b"veritasm:authenticated-witnessed-child:v1\0";
const ACCOUNTING_MARGIN_BYTES: u64 = 16_384;

/// Explicit owned-payload limits for one whole-resident child transformation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconstructionLimits {
    pub max_input_edges: u64,
    pub max_topology_candidates: u64,
    pub max_decision_rows: u64,
    pub max_segments: u64,
    pub max_links: u64,
    pub max_output_bases: u64,
    /// Owned vector payload plus a fixed implementation margin, not RSS.
    pub max_accounted_bytes: u64,
    /// Bounds the independent validator's fixed byte scratch.
    pub max_verification_scratch_bytes: u64,
}

/// Whether the child binding and transition ledger share authenticated spool ancestry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SourceEquivalence {
    Unverified = 0,
    AuthenticatedSpoolDescriptor = 1,
}

/// Opaque common-source and child-configuration binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildSourceBinding {
    source_root: [u8; 32],
    child_graph_binding: [u8; 32],
    compacted_ancestry_root: [u8; 32],
    equivalence: SourceEquivalence,
}

impl ChildSourceBinding {
    /// Explicitly unverified construction for isolated experiments.
    pub fn unverified(
        child: &CompactedGraphResult,
        transitions: &TransitionLedgerView<'_>,
    ) -> Self {
        Self {
            source_root: transitions.source_root(),
            child_graph_binding: child.source_identity,
            compacted_ancestry_root: [0; 32],
            equivalence: SourceEquivalence::Unverified,
        }
    }

    fn authenticated(
        compacted: &AuthenticatedCompactedGraph,
        transitions: &TransitionLedgerView<'_>,
    ) -> Result<Self> {
        let child = compacted.checked_graph()?;
        if compacted.source_root() != transitions.source_root()
            || compacted.k() != transitions.k()
            || child.source_identity != compacted.retention_root()
            || child.stats.canonical_edges != compacted.retained_key_count()
            || child.stats.input_support != compacted.retained_support()
        {
            return integrity("compacted and transition capabilities have different ancestry");
        }
        Ok(Self {
            source_root: compacted.source_root(),
            child_graph_binding: compacted.retention_root(),
            compacted_ancestry_root: compacted.ancestry_root(),
            equivalence: SourceEquivalence::AuthenticatedSpoolDescriptor,
        })
    }

    pub const fn source_root(self) -> [u8; 32] {
        self.source_root
    }

    pub const fn child_graph_binding(self) -> [u8; 32] {
        self.child_graph_binding
    }

    pub const fn compacted_ancestry_root(self) -> [u8; 32] {
        self.compacted_ancestry_root
    }

    pub const fn equivalence(self) -> SourceEquivalence {
        self.equivalence
    }
}

/// Kinds that a tampered or future object might claim for an adjacency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum AdjacencyEvidenceKind {
    OriginalReadTransition = 0,
    HigherKChild = 1,
    MultiKRelation = 2,
    BloomMembership = 3,
    TopologyOnly = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OriginalReadTransitionEvidence {
    pub kind: AdjacencyEvidenceKind,
    pub accepted_window_occurrences: u64,
    pub distinct_supplied_fragment_instances: u64,
    pub sorted_event_frames_sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WitnessedSegmentId(pub [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessedSegment {
    pub id: WitnessedSegmentId,
    pub parent_unitig_id: CompactedUnitigId,
    pub parent_start_step: u64,
    pub parent_end_step_exclusive: u64,
    pub topology: CompactedTopology,
    pub sequence: Vec<u8>,
    pub steps: Vec<CompactedEdgeStep>,
    pub total_edge_support: u64,
    pub minimum_edge_support: u64,
    pub lower_median_edge_support: u64,
    pub maximum_edge_support: u64,
    pub internal_transition_rows: u64,
    pub sum_accepted_window_occurrences: u64,
    pub sum_distinct_supplied_fragment_instances: u64,
    pub ordered_transition_rows_sha256: [u8; 32],
    pub provenance_sha256: [u8; 32],
}

/// Multiple graph candidates with the same q-mer remain distinct rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransitionCandidateOrigin {
    UnitigInterior {
        parent_unitig_id: CompactedUnitigId,
        left_step: u64,
    },
    ClosedWalkClosure {
        parent_unitig_id: CompactedUnitigId,
    },
    RawCompactedLink(CompactedLink),
    LedgerRetainedEndpointsNoRawCandidate,
    LedgerEndpointNotRetained,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TransitionDecisionStatus {
    AdmittedOriginalRead = 0,
    ExcludedEndpointNotRetained = 1,
    ExcludedNoOriginalReadWitness = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TransitionDecision {
    pub canonical_qmer: PackedKmer,
    pub origin: TransitionCandidateOrigin,
    pub status: TransitionDecisionStatus,
    pub evidence: Option<OriginalReadTransitionEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WitnessedLink {
    pub from: WitnessedSegmentId,
    pub from_orientation: UnitigOrientation,
    pub to: WitnessedSegmentId,
    pub to_orientation: UnitigOrientation,
    pub overlap_bases: u8,
    pub canonical_qmer: PackedKmer,
    pub evidence: OriginalReadTransitionEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconstructionConservation {
    pub input_canonical_edges: u64,
    pub represented_canonical_edges: u64,
    pub input_edge_support: u64,
    pub represented_edge_support: u64,
    pub topology_candidates: u64,
    pub admitted_topology_candidates: u64,
    pub excluded_no_original_read_witness: u64,
    pub ledger_rows: u64,
    pub eligible_ledger_rows: u64,
    pub excluded_endpoint_not_retained: u64,
    pub segments: u64,
    pub linear_segments: u64,
    pub closed_segments: u64,
    pub witnessed_links: u64,
    pub output_bases: u64,
    pub accounted_peak_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessedChild {
    pub k: u8,
    pub q: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub support_unit: WideRunSupportUnit,
    pub source_binding: ChildSourceBinding,
    pub transition_root: [u8; 32],
    pub exact_edge_table_sha256: [u8; 32],
    pub raw_compacted_graph_root: [u8; 32],
    pub constrained_graph_root: [u8; 32],
    pub child_root: [u8; 32],
    pub segments: Vec<WitnessedSegment>,
    pub links: Vec<WitnessedLink>,
    pub decisions: Vec<TransitionDecision>,
    pub conservation: ReconstructionConservation,
}

/// Opaque source-backed witnessed child.
///
/// The complete materialized child is private and this type does not implement
/// `Clone`.  Read-only records are exposed through [`Self::view`]; a copied
/// record set is not accepted by any source-backed constructor.
///
/// ```compile_fail
/// use veritasm::experimental::evidence_reconstruction::AuthenticatedWitnessedChild;
/// fn duplicate(child: &AuthenticatedWitnessedChild) -> AuthenticatedWitnessedChild {
///     child.clone()
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Clone))]
pub struct AuthenticatedWitnessedChild {
    graph_ancestry_root: [u8; 32],
    transition_root: [u8; 32],
    authentication_root: [u8; 32],
    child: WitnessedChild,
}

/// Checked, borrowed reporting view over an opaque witnessed child.
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedWitnessedChildView<'a> {
    child: &'a WitnessedChild,
    graph_ancestry_root: [u8; 32],
    authentication_root: [u8; 32],
}

impl AuthenticatedWitnessedChild {
    pub fn view(&self) -> AuthenticatedWitnessedChildView<'_> {
        AuthenticatedWitnessedChildView {
            child: &self.child,
            graph_ancestry_root: self.graph_ancestry_root,
            authentication_root: self.authentication_root,
        }
    }

    pub fn validate_against_sources(
        &self,
        compacted: &AuthenticatedCompactedGraph,
        transitions: &TransitionLedger,
        limits: ReconstructionLimits,
    ) -> Result<()> {
        let transition_view = transitions.view()?;
        let binding = ChildSourceBinding::authenticated(compacted, &transition_view)?;
        if self.graph_ancestry_root != compacted.ancestry_root()
            || self.transition_root != transitions.transition_root()
            || self.child.source_binding != binding
        {
            return integrity("authenticated witnessed child has different source capabilities");
        }
        validate_no_unsupported_adjacencies(&self.child, &transition_view, limits)?;
        let replay_peak = checked_mul(
            self.child.conservation.accounted_peak_bytes,
            2,
            "authenticated reconstruction plus complete replay",
        )?;
        enforce_limit(
            replay_peak,
            limits.max_accounted_bytes,
            ErrorCode::ResourceMemory,
            "authenticated reconstruction-plus-replay accounted bytes",
        )?;
        let rebuilt = constrain_compacted_child_with_binding(
            compacted.checked_graph()?,
            &transition_view,
            binding,
            limits,
        )?;
        if rebuilt != self.child {
            return integrity(
                "authenticated witnessed child differs from complete source reconstruction",
            );
        }
        if authenticated_witnessed_child_root(
            compacted.source_root(),
            self.graph_ancestry_root,
            self.transition_root,
            self.child.child_root,
        ) != self.authentication_root
        {
            return integrity("authenticated witnessed-child root is inconsistent");
        }
        Ok(())
    }
}

impl AuthenticatedWitnessedChildView<'_> {
    pub const fn k(&self) -> u8 {
        self.child.k
    }

    pub const fn q(&self) -> u8 {
        self.child.q
    }

    pub const fn minimizer_length(&self) -> u8 {
        self.child.minimizer_length
    }

    pub const fn virtual_bucket_count(&self) -> u32 {
        self.child.virtual_bucket_count
    }

    pub const fn support_unit(&self) -> WideRunSupportUnit {
        self.child.support_unit
    }

    pub const fn source_equivalence(&self) -> SourceEquivalence {
        self.child.source_binding.equivalence
    }

    pub const fn source_root(&self) -> [u8; 32] {
        self.child.source_binding.source_root
    }

    pub const fn graph_ancestry_root(&self) -> [u8; 32] {
        self.graph_ancestry_root
    }

    pub const fn transition_root(&self) -> [u8; 32] {
        self.child.transition_root
    }

    pub const fn child_root(&self) -> [u8; 32] {
        self.child.child_root
    }

    pub const fn exact_edge_table_sha256(&self) -> [u8; 32] {
        self.child.exact_edge_table_sha256
    }

    pub const fn raw_compacted_graph_root(&self) -> [u8; 32] {
        self.child.raw_compacted_graph_root
    }

    pub const fn constrained_graph_root(&self) -> [u8; 32] {
        self.child.constrained_graph_root
    }

    pub const fn authentication_root(&self) -> [u8; 32] {
        self.authentication_root
    }

    pub fn segments(&self) -> &[WitnessedSegment] {
        &self.child.segments
    }

    pub fn links(&self) -> &[WitnessedLink] {
        &self.child.links
    }

    pub fn decisions(&self) -> &[TransitionDecision] {
        &self.child.decisions
    }

    pub const fn conservation(&self) -> ReconstructionConservation {
        self.child.conservation
    }
}

#[derive(Debug, Clone, Copy)]
struct ParentSegments {
    parent: CompactedUnitigId,
    first: usize,
    last: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PendingLink {
    from: usize,
    from_orientation: UnitigOrientation,
    to: usize,
    to_orientation: UnitigOrientation,
    canonical_qmer: PackedKmer,
    evidence: OriginalReadTransitionEvidence,
}

#[derive(Debug, Clone, Copy)]
struct EdgeIndex {
    key: PackedKmer,
    literal: PackedKmer,
    segment: usize,
    step: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SegmentIdIndex {
    id: WitnessedSegmentId,
    index: usize,
}

#[derive(Debug, Clone, Copy)]
struct AdmittedShape {
    edge_count: usize,
    topology_candidates: usize,
    decision_capacity: usize,
    segment_capacity: usize,
    pending_link_capacity: usize,
    output_base_bound: u64,
    accounted_peak_bytes: u64,
}

/// Constrain a freely materialized compacted child while explicitly recording
/// that no source-backed ancestry claim was established.
pub fn constrain_unverified_compacted_child(
    child: &CompactedGraphResult,
    transitions: &TransitionLedgerView<'_>,
    limits: ReconstructionLimits,
) -> Result<WitnessedChild> {
    constrain_compacted_child_with_binding(
        child,
        transitions,
        ChildSourceBinding::unverified(child, transitions),
        limits,
    )
}

fn constrain_compacted_child_with_binding(
    child: &CompactedGraphResult,
    transitions: &TransitionLedgerView<'_>,
    source_binding: ChildSourceBinding,
    limits: ReconstructionLimits,
) -> Result<WitnessedChild> {
    validate_input_scalars(child, transitions, source_binding)?;
    let shape = admit_shape(child, transitions, limits)?;
    validate_inputs(child, transitions, source_binding)?;
    let raw_compacted_graph_root = raw_graph_root(child)?;
    let mut segments = try_vec(shape.segment_capacity, "witnessed segments")?;
    let mut parents = try_vec(child.unitigs.len(), "witnessed parent ranges")?;
    let mut decisions = try_vec(shape.decision_capacity, "transition decisions")?;
    let mut pending_links = try_vec(shape.pending_link_capacity, "pending witnessed links")?;

    for unitig in &child.unitigs {
        split_unitig(
            child.k,
            unitig,
            transitions,
            &mut segments,
            &mut parents,
            &mut decisions,
            &mut pending_links,
        )?;
    }
    let edge_index = build_edge_index(&segments, shape.edge_count)?;
    for link in &child.links {
        let qmer = raw_link_qmer(child, *link)?;
        let row = transitions.find(qmer);
        decisions.push(decision(
            qmer,
            TransitionCandidateOrigin::RawCompactedLink(*link),
            row,
        ));
        if let Some(row) = row {
            let from_parent = find_parent(&parents, link.from)?;
            let to_parent = find_parent(&parents, link.to)?;
            pending_links.push(PendingLink {
                from: outgoing_segment(from_parent, link.from_orientation),
                from_orientation: link.from_orientation,
                to: incoming_segment(to_parent, link.to_orientation),
                to_orientation: link.to_orientation,
                canonical_qmer: qmer,
                evidence: row_evidence(row),
            });
        }
    }

    decisions.sort_unstable_by_key(|row| (row.canonical_qmer, row.origin));
    if decisions.windows(2).any(|pair| {
        (pair[0].canonical_qmer, pair[0].origin) >= (pair[1].canonical_qmer, pair[1].origin)
    }) {
        return invariant("duplicate transition candidate origin");
    }
    let (eligible_ledger_rows, excluded_endpoint_rows) = append_endpoint_decisions(
        child.k,
        transitions,
        &edge_index,
        &segments,
        &mut decisions,
        &mut pending_links,
    )?;
    decisions.sort_unstable_by_key(|row| (row.canonical_qmer, row.origin));
    normalize_pending_links(&mut pending_links);

    let constrained_graph_root = constrained_graph_root(
        child.k,
        source_binding,
        transitions.transition_root(),
        &segments,
        &pending_links,
        &decisions,
    )?;
    let child_root = child_root(
        child,
        source_binding,
        transitions.transition_root(),
        raw_compacted_graph_root,
        constrained_graph_root,
    );
    finalize_segments(child.k, child_root, transitions, &mut segments)?;
    let mut links = finalize_links(child.k, &segments, pending_links)?;
    links.sort_unstable();
    links.dedup();
    enforce_limit(
        u64_from_usize(links.len(), "witnessed links")?,
        limits.max_links,
        ErrorCode::ResourceRetainedKeys,
        "witnessed links",
    )?;

    finish_child(
        child,
        transitions,
        source_binding,
        limits,
        shape,
        raw_compacted_graph_root,
        constrained_graph_root,
        child_root,
        segments,
        links,
        decisions,
        eligible_ledger_rows,
        excluded_endpoint_rows,
    )
}

/// Constrain two opaque source-backed capabilities in one fail-closed operation.
///
/// A caller-modified `CompactedGraphResult` or copied transition-row table does
/// not satisfy this signature and cannot be promoted by recomputing hashes.
///
/// ```compile_fail
/// use veritasm::experimental::compacted_dbg::CompactedGraphResult;
/// use veritasm::experimental::evidence_reconstruction::{
///     constrain_authenticated_compacted_child, ReconstructionLimits,
/// };
/// use veritasm::experimental::transition_witness::TransitionLedger;
/// fn cannot_promote(
///     graph: &CompactedGraphResult,
///     transitions: &TransitionLedger,
///     limits: ReconstructionLimits,
/// ) {
///     let _ = constrain_authenticated_compacted_child(graph, transitions, limits);
/// }
/// ```
pub fn constrain_authenticated_compacted_child(
    compacted: &AuthenticatedCompactedGraph,
    transitions: &TransitionLedger,
    limits: ReconstructionLimits,
) -> Result<AuthenticatedWitnessedChild> {
    let transition_view = transitions.view()?;
    let binding = ChildSourceBinding::authenticated(compacted, &transition_view)?;
    let child = constrain_compacted_child_with_binding(
        compacted.checked_graph()?,
        &transition_view,
        binding,
        limits,
    )?;
    let graph_ancestry_root = compacted.ancestry_root();
    let transition_root = transitions.transition_root();
    let authentication_root = authenticated_witnessed_child_root(
        compacted.source_root(),
        graph_ancestry_root,
        transition_root,
        child.child_root,
    );
    let authenticated = AuthenticatedWitnessedChild {
        graph_ancestry_root,
        transition_root,
        authentication_root,
        child,
    };
    authenticated.validate_against_sources(compacted, transitions, limits)?;
    Ok(authenticated)
}

fn split_unitig(
    k: u8,
    unitig: &CompactedUnitig,
    transitions: &TransitionLedgerView<'_>,
    segments: &mut Vec<WitnessedSegment>,
    parents: &mut Vec<ParentSegments>,
    decisions: &mut Vec<TransitionDecision>,
    pending_links: &mut Vec<PendingLink>,
) -> Result<()> {
    let first = segments.len();
    let mut start = 0_usize;
    for left in 0..unitig.steps.len().saturating_sub(1) {
        let qmer = canonical_window(&unitig.sequence[left..left + usize::from(k) + 1])?;
        let row = transitions.find(qmer);
        decisions.push(decision(
            qmer,
            TransitionCandidateOrigin::UnitigInterior {
                parent_unitig_id: unitig.id,
                left_step: u64_from_usize(left, "unitig transition offset")?,
            },
            row,
        ));
        if row.is_none() {
            segments.push(make_segment(
                k,
                unitig,
                start,
                left + 1,
                CompactedTopology::Linear,
            )?);
            start = left + 1;
        }
    }

    let closure_row = if unitig.topology == CompactedTopology::ClosedWalk {
        let qmer = closure_qmer(k, unitig)?;
        let row = transitions.find(qmer);
        decisions.push(decision(
            qmer,
            TransitionCandidateOrigin::ClosedWalkClosure {
                parent_unitig_id: unitig.id,
            },
            row,
        ));
        Some((qmer, row))
    } else {
        None
    };
    let closed_survives = unitig.topology == CompactedTopology::ClosedWalk
        && start == 0
        && closure_row.is_some_and(|(_, row)| row.is_some());
    segments.push(make_segment(
        k,
        unitig,
        start,
        unitig.steps.len(),
        if closed_survives {
            CompactedTopology::ClosedWalk
        } else {
            CompactedTopology::Linear
        },
    )?);
    let last = segments
        .len()
        .checked_sub(1)
        .ok_or_else(|| invariant_error("unitig split produced no segment"))?;
    parents.push(ParentSegments {
        parent: unitig.id,
        first,
        last,
    });
    if let Some((qmer, Some(row))) = closure_row {
        pending_links.push(PendingLink {
            from: last,
            from_orientation: UnitigOrientation::Forward,
            to: first,
            to_orientation: UnitigOrientation::Forward,
            canonical_qmer: qmer,
            evidence: row_evidence(row),
        });
    }
    Ok(())
}

fn make_segment(
    k: u8,
    unitig: &CompactedUnitig,
    start: usize,
    end: usize,
    topology: CompactedTopology,
) -> Result<WitnessedSegment> {
    if start >= end || end > unitig.steps.len() {
        return invariant("witnessed segment has an invalid parent step range");
    }
    let sequence_end = end
        .checked_add(usize::from(k - 1))
        .ok_or_else(|| overflow("witnessed segment sequence range"))?;
    let source_sequence = unitig
        .sequence
        .get(start..sequence_end)
        .ok_or_else(|| invariant_error("witnessed segment sequence range is outside parent"))?;
    let source_steps = unitig
        .steps
        .get(start..end)
        .ok_or_else(|| invariant_error("witnessed segment step range is outside parent"))?;
    let sequence = try_copy_slice(source_sequence, "witnessed segment sequence")?;
    let steps = try_copy_slice(source_steps, "witnessed segment steps")?;
    let mut support = try_vec(source_steps.len(), "witnessed segment support scratch")?;
    for step in source_steps {
        support.push(step.support);
    }
    support.sort_unstable();
    let total_edge_support = support.iter().try_fold(0_u64, |total, value| {
        checked_add(total, *value, "witnessed segment support")
    })?;
    Ok(WitnessedSegment {
        id: WitnessedSegmentId([0; 32]),
        parent_unitig_id: unitig.id,
        parent_start_step: u64_from_usize(start, "parent start step")?,
        parent_end_step_exclusive: u64_from_usize(end, "parent end step")?,
        topology,
        sequence,
        steps,
        total_edge_support,
        minimum_edge_support: support[0],
        lower_median_edge_support: support[(support.len() - 1) / 2],
        maximum_edge_support: support[support.len() - 1],
        internal_transition_rows: 0,
        sum_accepted_window_occurrences: 0,
        sum_distinct_supplied_fragment_instances: 0,
        ordered_transition_rows_sha256: [0; 32],
        provenance_sha256: [0; 32],
    })
}

fn decision(
    canonical_qmer: PackedKmer,
    origin: TransitionCandidateOrigin,
    row: Option<&TransitionRow>,
) -> TransitionDecision {
    TransitionDecision {
        canonical_qmer,
        origin,
        status: if row.is_some() {
            TransitionDecisionStatus::AdmittedOriginalRead
        } else {
            TransitionDecisionStatus::ExcludedNoOriginalReadWitness
        },
        evidence: row.map(row_evidence),
    }
}

fn row_evidence(row: &TransitionRow) -> OriginalReadTransitionEvidence {
    OriginalReadTransitionEvidence {
        kind: AdjacencyEvidenceKind::OriginalReadTransition,
        accepted_window_occurrences: row.accepted_window_occurrences,
        distinct_supplied_fragment_instances: row.distinct_supplied_fragment_instances,
        sorted_event_frames_sha256: row.sorted_event_frames_sha256,
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_child(
    child: &CompactedGraphResult,
    transitions: &TransitionLedgerView<'_>,
    source_binding: ChildSourceBinding,
    limits: ReconstructionLimits,
    shape: AdmittedShape,
    raw_compacted_graph_root: [u8; 32],
    constrained_graph_root: [u8; 32],
    child_root: [u8; 32],
    segments: Vec<WitnessedSegment>,
    links: Vec<WitnessedLink>,
    decisions: Vec<TransitionDecision>,
    eligible_ledger_rows: u64,
    excluded_endpoint_rows: u64,
) -> Result<WitnessedChild> {
    let represented_edges = segments.iter().try_fold(0_u64, |total, segment| {
        checked_add(
            total,
            u64_from_usize(segment.steps.len(), "represented segment steps")?,
            "represented edge count",
        )
    })?;
    let represented_support = segments.iter().try_fold(0_u64, |total, segment| {
        checked_add(
            total,
            segment.total_edge_support,
            "represented edge support",
        )
    })?;
    let output_bases = segments.iter().try_fold(0_u64, |total, segment| {
        checked_add(
            total,
            u64_from_usize(segment.sequence.len(), "witnessed segment bases")?,
            "witnessed output bases",
        )
    })?;
    if output_bases > shape.output_base_bound {
        return invariant("witnessed output bases exceeded the admitted bound");
    }
    enforce_limit(
        output_bases,
        limits.max_output_bases,
        ErrorCode::ResourceOutputBytes,
        "witnessed output bases",
    )?;
    let admitted_topology = decisions
        .iter()
        .filter(|row| {
            is_raw_topology_origin(row.origin)
                && row.status == TransitionDecisionStatus::AdmittedOriginalRead
        })
        .count();
    let excluded_no_witness = decisions
        .iter()
        .filter(|row| {
            is_raw_topology_origin(row.origin)
                && row.status == TransitionDecisionStatus::ExcludedNoOriginalReadWitness
        })
        .count();
    let linear_segments = segments
        .iter()
        .filter(|segment| segment.topology == CompactedTopology::Linear)
        .count();
    let closed_segments = segments.len().saturating_sub(linear_segments);
    let conservation = ReconstructionConservation {
        input_canonical_edges: child.stats.canonical_edges,
        represented_canonical_edges: represented_edges,
        input_edge_support: child.stats.input_support,
        represented_edge_support: represented_support,
        topology_candidates: u64_from_usize(shape.topology_candidates, "topology candidates")?,
        admitted_topology_candidates: u64_from_usize(
            admitted_topology,
            "admitted topology candidates",
        )?,
        excluded_no_original_read_witness: u64_from_usize(
            excluded_no_witness,
            "excluded topology candidates",
        )?,
        ledger_rows: u64_from_usize(transitions.rows().len(), "transition ledger rows")?,
        eligible_ledger_rows,
        excluded_endpoint_not_retained: excluded_endpoint_rows,
        segments: u64_from_usize(segments.len(), "witnessed segments")?,
        linear_segments: u64_from_usize(linear_segments, "linear segments")?,
        closed_segments: u64_from_usize(closed_segments, "closed segments")?,
        witnessed_links: u64_from_usize(links.len(), "witnessed links")?,
        output_bases,
        accounted_peak_bytes: shape.accounted_peak_bytes,
    };
    let result = WitnessedChild {
        k: child.k,
        q: child.k + 1,
        minimizer_length: child.minimizer_length,
        virtual_bucket_count: child.virtual_bucket_count,
        support_unit: child.support_unit,
        source_binding,
        transition_root: transitions.transition_root(),
        exact_edge_table_sha256: child.exact_edge_table_sha256,
        raw_compacted_graph_root,
        constrained_graph_root,
        child_root,
        segments,
        links,
        decisions,
        conservation,
    };
    validate_no_unsupported_adjacencies(&result, transitions, limits)?;
    Ok(result)
}

fn is_raw_topology_origin(origin: TransitionCandidateOrigin) -> bool {
    matches!(
        origin,
        TransitionCandidateOrigin::UnitigInterior { .. }
            | TransitionCandidateOrigin::ClosedWalkClosure { .. }
            | TransitionCandidateOrigin::RawCompactedLink(_)
    )
}

fn append_endpoint_decisions(
    k: u8,
    transitions: &TransitionLedgerView<'_>,
    edges: &[EdgeIndex],
    segments: &[WitnessedSegment],
    decisions: &mut Vec<TransitionDecision>,
    links: &mut Vec<PendingLink>,
) -> Result<(u64, u64)> {
    let mut eligible = 0_u64;
    let mut excluded = 0_u64;
    for row in transitions.rows() {
        let (prefix, suffix) = transition_endpoints(row.canonical_qmer, k)?;
        let prefix = canonical_code(prefix, k)?;
        let suffix = canonical_code(suffix, k)?;
        let retained = find_edge(edges, prefix).is_some() && find_edge(edges, suffix).is_some();
        if retained {
            eligible = checked_add(eligible, 1, "eligible transition rows")?;
            if !has_topology_candidate(decisions, row.canonical_qmer) {
                decisions.push(TransitionDecision {
                    canonical_qmer: row.canonical_qmer,
                    origin: TransitionCandidateOrigin::LedgerRetainedEndpointsNoRawCandidate,
                    status: TransitionDecisionStatus::AdmittedOriginalRead,
                    evidence: Some(row_evidence(row)),
                });
                if let Some(link) = retained_endpoint_link(k, row, edges, segments)? {
                    links.push(link);
                }
            }
        } else {
            excluded = checked_add(excluded, 1, "excluded transition endpoints")?;
            decisions.push(TransitionDecision {
                canonical_qmer: row.canonical_qmer,
                origin: TransitionCandidateOrigin::LedgerEndpointNotRetained,
                status: TransitionDecisionStatus::ExcludedEndpointNotRetained,
                evidence: Some(row_evidence(row)),
            });
        }
    }
    Ok((eligible, excluded))
}

fn retained_endpoint_link(
    k: u8,
    row: &TransitionRow,
    edges: &[EdgeIndex],
    segments: &[WitnessedSegment],
) -> Result<Option<PendingLink>> {
    let (prefix, suffix) = transition_endpoints(row.canonical_qmer, k)?;
    let from = oriented_edge_placement(k, prefix, edges, segments)?;
    let to = oriented_edge_placement(k, suffix, edges, segments)?;
    if from.0 == to.0 && from.1 == to.1 && to.2 == from.2 + 1 {
        return Ok(None);
    }
    let from_len = segments
        .get(from.0)
        .ok_or_else(|| invariant_error("retained transition from-segment is missing"))?
        .steps
        .len();
    if from.2 + 1 != from_len || to.2 != 0 {
        return integrity(
            "retained source transition is neither internal nor on emitted segment boundaries",
        );
    }
    Ok(Some(PendingLink {
        from: from.0,
        from_orientation: from.1,
        to: to.0,
        to_orientation: to.1,
        canonical_qmer: row.canonical_qmer,
        evidence: row_evidence(row),
    }))
}

fn oriented_edge_placement(
    k: u8,
    literal: PackedKmer,
    edges: &[EdgeIndex],
    segments: &[WitnessedSegment],
) -> Result<(usize, UnitigOrientation, usize)> {
    let key = canonical_code(literal, k)?;
    let edge = find_edge(edges, key)
        .ok_or_else(|| invariant_error("retained endpoint lacks its canonical edge"))?;
    let segment = segments
        .get(edge.segment)
        .ok_or_else(|| invariant_error("retained endpoint segment is missing"))?;
    if literal == edge.literal {
        Ok((edge.segment, UnitigOrientation::Forward, edge.step))
    } else if literal == reverse_complement_code(edge.literal, k)? {
        let oriented_step = segment
            .steps
            .len()
            .checked_sub(edge.step + 1)
            .ok_or_else(|| invariant_error("reverse endpoint step underflow"))?;
        Ok((
            edge.segment,
            UnitigOrientation::ReverseComplement,
            oriented_step,
        ))
    } else {
        integrity("retained endpoint literal is outside its canonical edge orbit")
    }
}

fn has_topology_candidate(decisions: &[TransitionDecision], qmer: PackedKmer) -> bool {
    let start = decisions.partition_point(|row| row.canonical_qmer < qmer);
    decisions
        .get(start)
        .is_some_and(|row| row.canonical_qmer == qmer)
}

fn build_edge_index(segments: &[WitnessedSegment], count: usize) -> Result<Vec<EdgeIndex>> {
    let mut edges = try_vec(count, "reconstruction edge index")?;
    for (segment_index, segment) in segments.iter().enumerate() {
        for (step_index, step) in segment.steps.iter().enumerate() {
            let end = step_index + segment.sequence.len() - segment.steps.len() + 1;
            let literal = encode_exact_bases(
                segment
                    .sequence
                    .get(step_index..end)
                    .ok_or_else(|| invariant_error("segment step spelling range is invalid"))?,
            )?;
            edges.push(EdgeIndex {
                key: step.key,
                literal,
                segment: segment_index,
                step: step_index,
            });
        }
    }
    edges.sort_unstable_by_key(|row| row.key);
    if edges.windows(2).any(|pair| pair[0].key >= pair[1].key) {
        return integrity("compacted child repeats or misorders a canonical edge");
    }
    Ok(edges)
}

fn find_edge(edges: &[EdgeIndex], key: PackedKmer) -> Option<&EdgeIndex> {
    edges
        .binary_search_by_key(&key, |edge| edge.key)
        .ok()
        .map(|index| &edges[index])
}

fn find_parent(parents: &[ParentSegments], id: CompactedUnitigId) -> Result<ParentSegments> {
    parents
        .binary_search_by_key(&id, |parent| parent.parent)
        .ok()
        .and_then(|index| parents.get(index).copied())
        .ok_or_else(|| invariant_error("compacted link references a missing parent unitig"))
}

fn outgoing_segment(parent: ParentSegments, orientation: UnitigOrientation) -> usize {
    match orientation {
        UnitigOrientation::Forward => parent.last,
        UnitigOrientation::ReverseComplement => parent.first,
    }
}

fn incoming_segment(parent: ParentSegments, orientation: UnitigOrientation) -> usize {
    match orientation {
        UnitigOrientation::Forward => parent.first,
        UnitigOrientation::ReverseComplement => parent.last,
    }
}

fn finalize_segments(
    k: u8,
    child_root: [u8; 32],
    transitions: &TransitionLedgerView<'_>,
    segments: &mut [WitnessedSegment],
) -> Result<()> {
    for segment in segments {
        let mut row_digest = Sha256::new();
        row_digest.update(TRANSITION_SET_DOMAIN);
        row_digest.update(child_root);
        let transition_count = segment.sequence.len().saturating_sub(usize::from(k));
        row_digest
            .update(u64_from_usize(transition_count, "segment transition count")?.to_le_bytes());
        let mut occurrence_sum = 0_u64;
        let mut fragment_sum = 0_u64;
        for window in segment.sequence.windows(usize::from(k) + 1) {
            let qmer = canonical_window(window)?;
            let row = transitions.find(qmer).ok_or_else(|| {
                integrity_error("emitted segment contains an unwitnessed adjacency")
            })?;
            occurrence_sum = checked_add(
                occurrence_sum,
                row.accepted_window_occurrences,
                "segment transition occurrence sum",
            )?;
            fragment_sum = checked_add(
                fragment_sum,
                row.distinct_supplied_fragment_instances,
                "segment transition fragment sum",
            )?;
            hash_transition_row(&mut row_digest, row);
        }
        segment.internal_transition_rows =
            u64_from_usize(transition_count, "segment transition rows")?;
        segment.sum_accepted_window_occurrences = occurrence_sum;
        segment.sum_distinct_supplied_fragment_instances = fragment_sum;
        segment.ordered_transition_rows_sha256 = row_digest.finalize().into();
        segment.id = segment_id(k, child_root, segment);
        segment.provenance_sha256 = segment_provenance(k, child_root, segment);
    }
    Ok(())
}

fn finalize_links(
    k: u8,
    segments: &[WitnessedSegment],
    pending: Vec<PendingLink>,
) -> Result<Vec<WitnessedLink>> {
    let mut links = try_vec(pending.len(), "witnessed links")?;
    for link in pending {
        let from = segments
            .get(link.from)
            .ok_or_else(|| invariant_error("pending link from-index is outside segments"))?;
        let to = segments
            .get(link.to)
            .ok_or_else(|| invariant_error("pending link to-index is outside segments"))?;
        links.push(WitnessedLink {
            from: from.id,
            from_orientation: link.from_orientation,
            to: to.id,
            to_orientation: link.to_orientation,
            overlap_bases: k - 1,
            canonical_qmer: link.canonical_qmer,
            evidence: link.evidence,
        });
    }
    Ok(links)
}

fn reverse_orientation(orientation: UnitigOrientation) -> UnitigOrientation {
    match orientation {
        UnitigOrientation::Forward => UnitigOrientation::ReverseComplement,
        UnitigOrientation::ReverseComplement => UnitigOrientation::Forward,
    }
}

fn canonical_pending(link: PendingLink) -> PendingLink {
    let reverse = PendingLink {
        from: link.to,
        from_orientation: reverse_orientation(link.to_orientation),
        to: link.from,
        to_orientation: reverse_orientation(link.from_orientation),
        ..link
    };
    link.min(reverse)
}

fn normalize_pending_links(links: &mut Vec<PendingLink>) {
    for link in links.iter_mut() {
        *link = canonical_pending(*link);
    }
    links.sort_unstable();
    links.dedup();
}

fn canonical_window(window: &[u8]) -> Result<PackedKmer> {
    let length = u8::try_from(window.len())
        .map_err(|_| overflow("transition window length does not fit u8"))?;
    canonical_code(encode_exact_bases(window)?, length)
}

fn closure_qmer(k: u8, unitig: &CompactedUnitig) -> Result<PackedKmer> {
    let step_count = unitig.steps.len();
    let last_start = step_count
        .checked_sub(1)
        .ok_or_else(|| invariant_error("closed unitig has no exact-edge step"))?;
    let mut bytes = [0_u8; 127];
    let k_usize = usize::from(k);
    bytes[..k_usize].copy_from_slice(
        unitig
            .sequence
            .get(last_start..last_start + k_usize)
            .ok_or_else(|| invariant_error("closed unitig lacks its last k-mer spelling"))?,
    );
    bytes[k_usize] = *unitig
        .sequence
        .get(k_usize - 1)
        .ok_or_else(|| invariant_error("closed unitig lacks its first terminal base"))?;
    canonical_window(&bytes[..=k_usize])
}

fn raw_link_qmer(child: &CompactedGraphResult, link: CompactedLink) -> Result<PackedKmer> {
    let from = find_unitig(&child.unitigs, link.from)?;
    let to = find_unitig(&child.unitigs, link.to)?;
    boundary_qmer_bytes(
        child.k,
        &from.sequence,
        link.from_orientation,
        &to.sequence,
        link.to_orientation,
    )
}

fn boundary_qmer_bytes(
    k: u8,
    from: &[u8],
    from_orientation: UnitigOrientation,
    to: &[u8],
    to_orientation: UnitigOrientation,
) -> Result<PackedKmer> {
    let k_usize = usize::from(k);
    if from.len() < k_usize || to.len() < k_usize {
        return integrity("link endpoint sequence is shorter than k");
    }
    for offset in 0..k_usize - 1 {
        let left = oriented_base(from, from_orientation, from.len() - (k_usize - 1) + offset)?;
        let right = oriented_base(to, to_orientation, offset)?;
        if left != right {
            return integrity("link endpoint sequences lack the declared exact overlap");
        }
    }
    let mut bytes = [0_u8; 127];
    for (offset, slot) in bytes[..k_usize].iter_mut().enumerate() {
        *slot = oriented_base(from, from_orientation, from.len() - k_usize + offset)?;
    }
    bytes[k_usize] = oriented_base(to, to_orientation, k_usize - 1)?;
    canonical_window(&bytes[..=k_usize])
}

fn oriented_base(sequence: &[u8], orientation: UnitigOrientation, index: usize) -> Result<u8> {
    let base = match orientation {
        UnitigOrientation::Forward => *sequence
            .get(index)
            .ok_or_else(|| integrity_error("forward oriented-base offset is outside segment"))?,
        UnitigOrientation::ReverseComplement => {
            let source = sequence.len().checked_sub(index + 1).ok_or_else(|| {
                integrity_error("reverse oriented-base offset is outside segment")
            })?;
            complement(*sequence.get(source).ok_or_else(|| {
                integrity_error("reverse oriented-base source is outside segment")
            })?)?
        }
    };
    if matches!(base, b'A' | b'C' | b'G' | b'T') {
        Ok(base)
    } else {
        integrity("segment contains a non-ACGT byte")
    }
}

fn complement(base: u8) -> Result<u8> {
    match base {
        b'A' => Ok(b'T'),
        b'C' => Ok(b'G'),
        b'G' => Ok(b'C'),
        b'T' => Ok(b'A'),
        _ => integrity("cannot complement a non-ACGT byte"),
    }
}

fn find_unitig(unitigs: &[CompactedUnitig], id: CompactedUnitigId) -> Result<&CompactedUnitig> {
    unitigs
        .binary_search_by_key(&id, |unitig| unitig.id)
        .ok()
        .and_then(|index| unitigs.get(index))
        .ok_or_else(|| integrity_error("link references a missing compacted unitig"))
}

/// Independently decode every emitted segment window and GFA-style link.
///
/// This validator does not trust decision rows, compacted topology, higher-k
/// relations, Bloom membership, or stored q-mer fields to establish evidence.
pub fn validate_no_unsupported_adjacencies(
    child: &WitnessedChild,
    transitions: &TransitionLedgerView<'_>,
    limits: ReconstructionLimits,
) -> Result<()> {
    admit_validator(child, transitions, limits)?;
    if child.k != transitions.k()
        || child.q != transitions.q()
        || child.source_binding.source_root != transitions.source_root()
        || child.transition_root != transitions.transition_root()
    {
        return integrity("witnessed child and transition ledger ancestry disagree");
    }
    if !(3..=126).contains(&child.k) || child.q != child.k + 1 {
        return integrity("witnessed child k/q is outside reconstruction range");
    }
    validate_decisions(child, transitions)?;
    let id_index = build_segment_id_index(&child.segments)?;

    let mut previous_origin = None;
    let mut represented_edges = 0_u64;
    let mut represented_support = 0_u64;
    let mut output_bases = 0_u64;
    for segment in &child.segments {
        let origin = (
            segment.parent_unitig_id,
            segment.parent_start_step,
            segment.parent_end_step_exclusive,
        );
        if previous_origin.is_some_and(|prior| prior >= origin) {
            return integrity("witnessed segments are not strictly ordered by parent range");
        }
        previous_origin = Some(origin);
        validate_segment_bytes(child.k, child.child_root, segment, transitions)?;
        represented_edges = checked_add(
            represented_edges,
            u64_from_usize(segment.steps.len(), "validated segment steps")?,
            "validated represented edges",
        )?;
        represented_support = checked_add(
            represented_support,
            segment.total_edge_support,
            "validated represented support",
        )?;
        output_bases = checked_add(
            output_bases,
            u64_from_usize(segment.sequence.len(), "validated segment bases")?,
            "validated output bases",
        )?;
    }
    if child.links.windows(2).any(|pair| pair[0] >= pair[1]) {
        return integrity("witnessed links are not strictly ordered and unique");
    }
    for link in &child.links {
        if link.evidence.kind != AdjacencyEvidenceKind::OriginalReadTransition {
            return integrity("GFA transition claims a non-original-read evidence kind");
        }
        if link.overlap_bases != child.k - 1 {
            return integrity("witnessed link overlap length is not k - 1");
        }
        let from = segment_by_id(&child.segments, &id_index, link.from)?;
        let to = segment_by_id(&child.segments, &id_index, link.to)?;
        let decoded = boundary_qmer_bytes(
            child.k,
            &from.sequence,
            link.from_orientation,
            &to.sequence,
            link.to_orientation,
        )?;
        if decoded != link.canonical_qmer {
            return integrity("stored link q-mer differs from independently decoded adjacency");
        }
        let row = transitions
            .find(decoded)
            .ok_or_else(|| integrity_error("emitted GFA transition lacks an exact source row"))?;
        if link.evidence != row_evidence(row) {
            return integrity("emitted GFA transition evidence differs from exact source row");
        }
    }
    if child.segments.len() as u64 != child.conservation.segments
        || child.links.len() as u64 != child.conservation.witnessed_links
        || represented_edges != child.conservation.represented_canonical_edges
        || represented_support != child.conservation.represented_edge_support
        || output_bases != child.conservation.output_bases
        || child.conservation.input_canonical_edges != represented_edges
        || child.conservation.input_edge_support != represented_support
    {
        return integrity("witnessed child conservation counters disagree with emitted records");
    }
    let constrained = constrained_graph_root_from_result(child, &id_index)?;
    if constrained != child.constrained_graph_root {
        return integrity("constrained graph root does not authenticate emitted records");
    }
    let expected_child_root = witnessed_child_root(child);
    if expected_child_root != child.child_root {
        return integrity("child root does not authenticate complete ancestry");
    }
    let topology_candidates = child
        .decisions
        .iter()
        .filter(|row| is_raw_topology_origin(row.origin))
        .count() as u64;
    let admitted = child
        .decisions
        .iter()
        .filter(|row| {
            is_raw_topology_origin(row.origin)
                && row.status == TransitionDecisionStatus::AdmittedOriginalRead
        })
        .count() as u64;
    let no_witness = child
        .decisions
        .iter()
        .filter(|row| {
            is_raw_topology_origin(row.origin)
                && row.status == TransitionDecisionStatus::ExcludedNoOriginalReadWitness
        })
        .count() as u64;
    let endpoint_excluded = child
        .decisions
        .iter()
        .filter(|row| row.status == TransitionDecisionStatus::ExcludedEndpointNotRetained)
        .count() as u64;
    let linear = child
        .segments
        .iter()
        .filter(|segment| segment.topology == CompactedTopology::Linear)
        .count() as u64;
    if topology_candidates != child.conservation.topology_candidates
        || admitted != child.conservation.admitted_topology_candidates
        || no_witness != child.conservation.excluded_no_original_read_witness
        || admitted + no_witness != topology_candidates
        || endpoint_excluded != child.conservation.excluded_endpoint_not_retained
        || child.conservation.eligible_ledger_rows + endpoint_excluded
            != child.conservation.ledger_rows
        || child.conservation.ledger_rows != transitions.rows().len() as u64
        || linear != child.conservation.linear_segments
        || child.conservation.closed_segments + linear != child.conservation.segments
    {
        return integrity("decision or topology conservation counters are invalid");
    }
    Ok(())
}

fn admit_validator(
    child: &WitnessedChild,
    transitions: &TransitionLedgerView<'_>,
    limits: ReconstructionLimits,
) -> Result<()> {
    validate_limits(limits)?;
    if limits.max_verification_scratch_bytes < 127 {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            "independent adjacency verification requires 127 bytes of fixed scratch",
        ));
    }
    let edges = child.segments.iter().try_fold(0_u64, |total, segment| {
        checked_add(
            total,
            u64_from_usize(segment.steps.len(), "validator segment steps")?,
            "validator edge count",
        )
    })?;
    let bases = child.segments.iter().try_fold(0_u64, |total, segment| {
        checked_add(
            total,
            u64_from_usize(segment.sequence.len(), "validator segment bases")?,
            "validator output bases",
        )
    })?;
    let topology_candidates = child
        .decisions
        .iter()
        .filter(|row| is_raw_topology_origin(row.origin))
        .count();
    let parent_count = child
        .segments
        .iter()
        .enumerate()
        .filter(|(index, segment)| {
            *index == 0 || child.segments[index - 1].parent_unitig_id != segment.parent_unitig_id
        })
        .count();
    let closure_candidates = child
        .decisions
        .iter()
        .filter(|row| {
            matches!(
                row.origin,
                TransitionCandidateOrigin::ClosedWalkClosure { .. }
            )
        })
        .count();
    let raw_link_candidates = child
        .decisions
        .iter()
        .filter(|row| matches!(row.origin, TransitionCandidateOrigin::RawCompactedLink(_)))
        .count();
    let decision_capacity = topology_candidates
        .checked_add(transitions.rows().len())
        .ok_or_else(|| overflow("validator decision capacity"))?;
    let pending_capacity = closure_candidates
        .checked_add(raw_link_candidates)
        .and_then(|value| value.checked_add(transitions.rows().len()))
        .ok_or_else(|| overflow("validator pending-link capacity"))?;
    let output_bound = checked_mul(edges, u64::from(child.k), "validator output-base bound")?;
    let expected_construction_peak = projected_accounted_bytes(
        usize::try_from(edges).map_err(|_| overflow("validator edge count usize"))?,
        parent_count,
        decision_capacity,
        usize::try_from(edges).map_err(|_| overflow("validator segment capacity usize"))?,
        pending_capacity,
        output_bound,
    )?;
    enforce_limit(
        edges,
        limits.max_input_edges,
        ErrorCode::ResourceRetainedKeys,
        "validator edges",
    )?;
    enforce_limit(
        topology_candidates as u64,
        limits.max_topology_candidates,
        ErrorCode::ResourceRetainedKeys,
        "validator topology candidates",
    )?;
    enforce_limit(
        child.decisions.len() as u64,
        limits.max_decision_rows,
        ErrorCode::ResourceRetainedKeys,
        "validator decision rows",
    )?;
    enforce_limit(
        child.segments.len() as u64,
        limits.max_segments,
        ErrorCode::ResourceRetainedKeys,
        "validator segments",
    )?;
    enforce_limit(
        child.links.len() as u64,
        limits.max_links,
        ErrorCode::ResourceRetainedKeys,
        "validator links",
    )?;
    enforce_limit(
        bases,
        limits.max_output_bases,
        ErrorCode::ResourceOutputBytes,
        "validator output bases",
    )?;
    let nested = child.segments.iter().try_fold(0_u64, |total, segment| {
        checked_sum(&[
            total,
            segment.sequence.capacity() as u64,
            checked_bytes(
                segment.steps.capacity(),
                size_of::<CompactedEdgeStep>() as u64,
                "validator segment step capacity",
            )?,
        ])
    })?;
    let payload = checked_sum(&[
        checked_bytes(
            child.segments.capacity(),
            size_of::<WitnessedSegment>() as u64,
            "validator segment headers",
        )?,
        nested,
        checked_bytes(
            child.links.capacity(),
            size_of::<WitnessedLink>() as u64,
            "validator link capacity",
        )?,
        checked_bytes(
            child.decisions.capacity(),
            size_of::<TransitionDecision>() as u64,
            "validator decision capacity",
        )?,
        checked_bytes(
            child.segments.len(),
            size_of::<SegmentIdIndex>() as u64,
            "validator ID index",
        )?,
        checked_bytes(
            child.links.len(),
            size_of::<PendingLink>() as u64,
            "validator root link scratch",
        )?,
        checked_mul(edges, size_of::<u64>() as u64, "validator support scratch")?,
        limits.max_verification_scratch_bytes,
        ACCOUNTING_MARGIN_BYTES,
    ])?;
    enforce_limit(
        payload,
        limits.max_accounted_bytes,
        ErrorCode::ResourceMemory,
        "validator accounted bytes",
    )?;
    if child.conservation.accounted_peak_bytes != expected_construction_peak {
        return integrity("recorded reconstruction peak is not independently derivable");
    }
    Ok(())
}

fn validate_segment_bytes(
    k: u8,
    child_root: [u8; 32],
    segment: &WitnessedSegment,
    transitions: &TransitionLedgerView<'_>,
) -> Result<()> {
    if segment.steps.is_empty()
        || segment.sequence.len() != segment.steps.len() + usize::from(k) - 1
        || segment.parent_start_step >= segment.parent_end_step_exclusive
        || segment.parent_end_step_exclusive - segment.parent_start_step
            != u64_from_usize(segment.steps.len(), "segment range length")?
    {
        return integrity("witnessed segment length or parent range is invalid");
    }
    let mut total = 0_u64;
    let mut support = try_vec(segment.steps.len(), "segment validation support scratch")?;
    for (offset, step) in segment.steps.iter().enumerate() {
        validate_step(k, &segment.sequence[offset..offset + usize::from(k)], step)?;
        total = checked_add(total, step.support, "validated segment support")?;
        support.push(step.support);
    }
    support.sort_unstable();
    let minimum = support[0];
    let median = support[(support.len() - 1) / 2];
    let maximum = support[support.len() - 1];
    if total != segment.total_edge_support
        || minimum != segment.minimum_edge_support
        || median != segment.lower_median_edge_support
        || maximum != segment.maximum_edge_support
    {
        return integrity("witnessed segment support summary is invalid");
    }

    let mut digest = Sha256::new();
    digest.update(TRANSITION_SET_DOMAIN);
    digest.update(child_root);
    let count = segment.sequence.len() - usize::from(k);
    digest.update(u64_from_usize(count, "validated segment transitions")?.to_le_bytes());
    let mut occurrence_sum = 0_u64;
    let mut fragment_sum = 0_u64;
    for window in segment.sequence.windows(usize::from(k) + 1) {
        let decoded = canonical_window(window)?;
        let row = transitions
            .find(decoded)
            .ok_or_else(|| integrity_error("emitted segment contains an unwitnessed adjacency"))?;
        occurrence_sum = checked_add(
            occurrence_sum,
            row.accepted_window_occurrences,
            "validated segment occurrence sum",
        )?;
        fragment_sum = checked_add(
            fragment_sum,
            row.distinct_supplied_fragment_instances,
            "validated segment fragment sum",
        )?;
        hash_transition_row(&mut digest, row);
    }
    if segment.internal_transition_rows != count as u64
        || segment.sum_accepted_window_occurrences != occurrence_sum
        || segment.sum_distinct_supplied_fragment_instances != fragment_sum
        || segment.ordered_transition_rows_sha256 != <[u8; 32]>::from(digest.finalize())
        || segment.id != segment_id(k, child_root, segment)
        || segment.provenance_sha256 != segment_provenance(k, child_root, segment)
    {
        return integrity("witnessed segment evidence, identifier, or provenance is invalid");
    }
    Ok(())
}

fn validate_step(k: u8, literal_bytes: &[u8], step: &CompactedEdgeStep) -> Result<()> {
    if step.support == 0 {
        return integrity("witnessed exact edge has zero support");
    }
    let literal = encode_exact_bases(literal_bytes)?;
    let reverse = reverse_complement_code(literal, k)?;
    let canonical = literal.min(reverse);
    let orientation = if literal == reverse {
        EdgeOrientation::SelfReverseComplement
    } else if literal == canonical {
        EdgeOrientation::Canonical
    } else {
        EdgeOrientation::ReverseComplement
    };
    if step.key != canonical || step.orientation != orientation {
        return integrity("witnessed exact-edge step differs from segment bytes");
    }
    Ok(())
}

fn validate_decisions(
    child: &WitnessedChild,
    transitions: &TransitionLedgerView<'_>,
) -> Result<()> {
    if child.decisions.windows(2).any(|pair| pair[0] >= pair[1]) {
        return integrity("transition decisions are not strictly ordered and unique");
    }
    for decision in &child.decisions {
        validate_code(decision.canonical_qmer, child.q)?;
        match decision.status {
            TransitionDecisionStatus::AdmittedOriginalRead => {
                let row = transitions.find(decision.canonical_qmer).ok_or_else(|| {
                    integrity_error("admitted transition decision lacks exact source row")
                })?;
                if decision.evidence != Some(row_evidence(row))
                    || decision.evidence.is_some_and(|evidence| {
                        evidence.kind != AdjacencyEvidenceKind::OriginalReadTransition
                    })
                {
                    return integrity(
                        "admitted transition uses unsupported or mismatched evidence",
                    );
                }
            }
            TransitionDecisionStatus::ExcludedEndpointNotRetained => {
                let row = transitions.find(decision.canonical_qmer).ok_or_else(|| {
                    integrity_error("endpoint-excluded decision lacks exact source row")
                })?;
                if decision.origin != TransitionCandidateOrigin::LedgerEndpointNotRetained
                    || decision.evidence != Some(row_evidence(row))
                {
                    return integrity("endpoint-excluded decision is malformed");
                }
            }
            TransitionDecisionStatus::ExcludedNoOriginalReadWitness => {
                if transitions.find(decision.canonical_qmer).is_some()
                    || decision.evidence.is_some()
                    || decision.origin == TransitionCandidateOrigin::LedgerEndpointNotRetained
                {
                    return integrity("no-witness exclusion contradicts exact source rows");
                }
            }
        }
    }
    Ok(())
}

fn segment_by_id<'a>(
    segments: &'a [WitnessedSegment],
    index: &[SegmentIdIndex],
    id: WitnessedSegmentId,
) -> Result<&'a WitnessedSegment> {
    let segment_index = index
        .binary_search_by_key(&id, |row| row.id)
        .ok()
        .and_then(|position| index.get(position))
        .ok_or_else(|| integrity_error("witnessed link references a missing segment"))?
        .index;
    segments
        .get(segment_index)
        .ok_or_else(|| integrity_error("segment ID index contains an invalid position"))
}

fn validate_input_scalars(
    child: &CompactedGraphResult,
    transitions: &TransitionLedgerView<'_>,
    binding: ChildSourceBinding,
) -> Result<()> {
    if !(3..=126).contains(&child.k) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "evidence reconstruction k must be in 3..=126; received {}",
                child.k
            ),
        ));
    }
    if transitions.k() != child.k || transitions.q() != child.k + 1 {
        return integrity("compacted child and transition ledger k/q disagree");
    }
    if binding.source_root != transitions.source_root()
        || binding.child_graph_binding != child.source_identity
    {
        return integrity("source binding does not match child and transition inputs");
    }
    match binding.equivalence {
        SourceEquivalence::Unverified if binding.compacted_ancestry_root != [0; 32] => {
            return integrity("unverified child carries an authenticated graph-ancestry root");
        }
        SourceEquivalence::AuthenticatedSpoolDescriptor
            if binding.compacted_ancestry_root == [0; 32] =>
        {
            return integrity("source-backed child lacks its compacted-graph ancestry root");
        }
        _ => {}
    }
    if child.minimizer_length == 0
        || child.minimizer_length > child.k
        || child.virtual_bucket_count == 0
    {
        return integrity("compacted child scalar configuration is invalid");
    }
    Ok(())
}

fn validate_inputs(
    child: &CompactedGraphResult,
    transitions: &TransitionLedgerView<'_>,
    binding: ChildSourceBinding,
) -> Result<()> {
    validate_input_scalars(child, transitions, binding)?;
    if child
        .unitigs
        .windows(2)
        .any(|pair| pair[0].id >= pair[1].id)
        || child.links.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return integrity("compacted child records are not strictly ordered and unique");
    }
    let mut edge_count = 0_u64;
    let mut support = 0_u64;
    let mut output_bases = 0_u64;
    for unitig in &child.unitigs {
        validate_compacted_unitig(child, unitig)?;
        edge_count = checked_add(
            edge_count,
            u64_from_usize(unitig.steps.len(), "compacted unitig steps")?,
            "compacted child edges",
        )?;
        support = checked_add(support, unitig.total_support, "compacted child support")?;
        output_bases = checked_add(
            output_bases,
            u64_from_usize(unitig.sequence.len(), "compacted unitig bases")?,
            "compacted child bases",
        )?;
    }
    for link in &child.links {
        if link.overlap_bases != child.k - 1 {
            return integrity("compacted child link overlap is not k - 1");
        }
        let decoded = raw_link_qmer(child, *link)?;
        validate_code(decoded, child.k + 1)?;
    }
    if edge_count != child.stats.canonical_edges
        || edge_count != child.stats.represented_canonical_edges
        || support != child.stats.input_support
        || support != child.stats.represented_support
        || output_bases != child.stats.output_bases
        || child.links.len() as u64 != child.stats.canonical_links
        || child.stats.link_candidates < child.stats.canonical_links
    {
        return integrity("compacted child conservation metadata is invalid");
    }
    Ok(())
}

fn validate_compacted_unitig(child: &CompactedGraphResult, unitig: &CompactedUnitig) -> Result<()> {
    if unitig.steps.is_empty()
        || unitig.sequence.len() != unitig.steps.len() + usize::from(child.k) - 1
    {
        return integrity("compacted unitig sequence/step length is invalid");
    }
    let mut total = 0_u64;
    let mut support = try_vec(unitig.steps.len(), "compacted unitig support validation")?;
    for (offset, step) in unitig.steps.iter().enumerate() {
        validate_step(
            child.k,
            &unitig.sequence[offset..offset + usize::from(child.k)],
            step,
        )?;
        total = checked_add(total, step.support, "compacted unitig support")?;
        support.push(step.support);
    }
    support.sort_unstable();
    let minimum = support[0];
    let median = support[(support.len() - 1) / 2];
    let maximum = support[support.len() - 1];
    if total != unitig.total_support
        || minimum != unitig.minimum_support
        || median != unitig.lower_median_support
        || maximum != unitig.maximum_support
        || compacted_unitig_id(child.k, unitig) != unitig.id
        || compacted_unitig_provenance(child, unitig) != unitig.provenance_sha256
    {
        return integrity("compacted unitig support, identifier, or provenance is invalid");
    }
    if unitig.topology == CompactedTopology::ClosedWalk {
        let overlap = usize::from(child.k - 1);
        if unitig.sequence[..overlap] != unitig.sequence[unitig.sequence.len() - overlap..] {
            return integrity("closed compacted unitig lacks repeated terminal context");
        }
    }
    Ok(())
}

fn admit_shape(
    child: &CompactedGraphResult,
    transitions: &TransitionLedgerView<'_>,
    limits: ReconstructionLimits,
) -> Result<AdmittedShape> {
    validate_limits(limits)?;
    let edge_count = child.unitigs.iter().try_fold(0_usize, |total, unitig| {
        total
            .checked_add(unitig.steps.len())
            .ok_or_else(|| overflow("admitted compacted edge count"))
    })?;
    let interiors = edge_count.saturating_sub(child.unitigs.len());
    let closures = child
        .unitigs
        .iter()
        .filter(|unitig| unitig.topology == CompactedTopology::ClosedWalk)
        .count();
    let topology_candidates = interiors
        .checked_add(closures)
        .and_then(|value| value.checked_add(child.links.len()))
        .ok_or_else(|| overflow("topology candidate bound"))?;
    let decision_capacity = topology_candidates
        .checked_add(transitions.rows().len())
        .ok_or_else(|| overflow("decision row capacity"))?;
    let segment_capacity = edge_count;
    let pending_link_capacity = closures
        .checked_add(child.links.len())
        .and_then(|value| value.checked_add(transitions.rows().len()))
        .ok_or_else(|| overflow("pending link capacity"))?;
    let output_base_bound = checked_mul(
        u64_from_usize(edge_count, "edge count")?,
        u64::from(child.k),
        "witnessed output-base bound",
    )?;
    enforce_limit(
        edge_count as u64,
        limits.max_input_edges,
        ErrorCode::ResourceRetainedKeys,
        "input edges",
    )?;
    enforce_limit(
        topology_candidates as u64,
        limits.max_topology_candidates,
        ErrorCode::ResourceRetainedKeys,
        "topology candidates",
    )?;
    enforce_limit(
        decision_capacity as u64,
        limits.max_decision_rows,
        ErrorCode::ResourceRetainedKeys,
        "decision rows",
    )?;
    enforce_limit(
        segment_capacity as u64,
        limits.max_segments,
        ErrorCode::ResourceRetainedKeys,
        "segments",
    )?;
    enforce_limit(
        pending_link_capacity as u64,
        limits.max_links,
        ErrorCode::ResourceRetainedKeys,
        "pending links",
    )?;
    enforce_limit(
        output_base_bound,
        limits.max_output_bases,
        ErrorCode::ResourceOutputBytes,
        "output-base bound",
    )?;

    let accounted_peak_bytes = projected_accounted_bytes(
        edge_count,
        child.unitigs.len(),
        decision_capacity,
        segment_capacity,
        pending_link_capacity,
        output_base_bound,
    )?;
    enforce_limit(
        accounted_peak_bytes,
        limits.max_accounted_bytes,
        ErrorCode::ResourceMemory,
        "accounted reconstruction bytes",
    )?;
    Ok(AdmittedShape {
        edge_count,
        topology_candidates,
        decision_capacity,
        segment_capacity,
        pending_link_capacity,
        output_base_bound,
        accounted_peak_bytes,
    })
}

fn projected_accounted_bytes(
    edge_count: usize,
    parent_count: usize,
    decision_capacity: usize,
    segment_capacity: usize,
    pending_link_capacity: usize,
    output_base_bound: u64,
) -> Result<u64> {
    checked_sum(&[
        checked_bytes(
            edge_count,
            size_of::<EdgeIndex>() as u64,
            "edge index bytes",
        )?,
        checked_bytes(
            parent_count,
            size_of::<ParentSegments>() as u64,
            "parent range bytes",
        )?,
        checked_bytes(
            segment_capacity,
            size_of::<WitnessedSegment>() as u64,
            "segment header bytes",
        )?,
        checked_bytes(
            segment_capacity,
            size_of::<SegmentIdIndex>() as u64,
            "segment ID index bytes",
        )?,
        checked_bytes(
            edge_count,
            size_of::<CompactedEdgeStep>() as u64,
            "segment step bytes",
        )?,
        output_base_bound,
        checked_bytes(
            decision_capacity,
            size_of::<TransitionDecision>() as u64,
            "decision bytes",
        )?,
        checked_bytes(
            pending_link_capacity,
            size_of::<PendingLink>() as u64,
            "pending link bytes",
        )?,
        checked_bytes(
            pending_link_capacity,
            size_of::<WitnessedLink>() as u64,
            "final link bytes",
        )?,
        checked_bytes(edge_count, size_of::<u64>() as u64, "support scratch bytes")?,
        ACCOUNTING_MARGIN_BYTES,
    ])
}

fn validate_limits(limits: ReconstructionLimits) -> Result<()> {
    if limits.max_input_edges == 0
        || limits.max_topology_candidates == 0
        || limits.max_decision_rows == 0
        || limits.max_segments == 0
        || limits.max_links == 0
        || limits.max_output_bases == 0
        || limits.max_accounted_bytes == 0
        || limits.max_verification_scratch_bytes == 0
    {
        Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "all evidence-reconstruction limits must be nonzero",
        ))
    } else {
        Ok(())
    }
}

fn raw_graph_root(child: &CompactedGraphResult) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(RAW_GRAPH_DOMAIN);
    digest.update([child.k, child.minimizer_length, child.support_unit as u8]);
    digest.update(child.virtual_bucket_count.to_le_bytes());
    digest.update(child.source_identity);
    digest.update(child.exact_edge_table_sha256);
    digest.update((child.unitigs.len() as u64).to_le_bytes());
    for unitig in &child.unitigs {
        digest.update(unitig.id.0);
        digest.update([unitig.topology as u8]);
        hash_bytes(&mut digest, &unitig.sequence);
        hash_steps(&mut digest, &unitig.steps);
        digest.update(unitig.total_support.to_le_bytes());
        digest.update(unitig.minimum_support.to_le_bytes());
        digest.update(unitig.lower_median_support.to_le_bytes());
        digest.update(unitig.maximum_support.to_le_bytes());
        digest.update(unitig.provenance_sha256);
    }
    digest.update((child.links.len() as u64).to_le_bytes());
    for link in &child.links {
        hash_compacted_link(&mut digest, *link);
    }
    hash_compacted_stats(&mut digest, child);
    Ok(digest.finalize().into())
}

fn constrained_graph_root(
    k: u8,
    binding: ChildSourceBinding,
    transition_root: [u8; 32],
    segments: &[WitnessedSegment],
    links: &[PendingLink],
    decisions: &[TransitionDecision],
) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(CONSTRAINED_GRAPH_DOMAIN);
    digest.update([k, binding.equivalence as u8]);
    digest.update(binding.source_root);
    digest.update(binding.child_graph_binding);
    digest.update(binding.compacted_ancestry_root);
    digest.update(transition_root);
    digest.update((segments.len() as u64).to_le_bytes());
    for segment in segments {
        hash_segment_preimage(&mut digest, segment);
    }
    digest.update((links.len() as u64).to_le_bytes());
    for link in links {
        digest.update((link.from as u64).to_le_bytes());
        digest.update([link.from_orientation as u8]);
        digest.update((link.to as u64).to_le_bytes());
        digest.update([link.to_orientation as u8]);
        digest.update(link.canonical_qmer.to_be_bytes());
        hash_evidence(&mut digest, link.evidence);
    }
    digest.update((decisions.len() as u64).to_le_bytes());
    for decision in decisions {
        hash_decision(&mut digest, decision);
    }
    Ok(digest.finalize().into())
}

fn constrained_graph_root_from_result(
    child: &WitnessedChild,
    index: &[SegmentIdIndex],
) -> Result<[u8; 32]> {
    let mut pending = try_vec(child.links.len(), "constrained-root link scratch")?;
    for link in &child.links {
        let from = segment_index_by_id(index, link.from)?;
        let to = segment_index_by_id(index, link.to)?;
        pending.push(PendingLink {
            from,
            from_orientation: link.from_orientation,
            to,
            to_orientation: link.to_orientation,
            canonical_qmer: link.canonical_qmer,
            evidence: link.evidence,
        });
    }
    normalize_pending_links(&mut pending);
    constrained_graph_root(
        child.k,
        child.source_binding,
        child.transition_root,
        &child.segments,
        &pending,
        &child.decisions,
    )
}

fn child_root(
    child: &CompactedGraphResult,
    binding: ChildSourceBinding,
    transition_root: [u8; 32],
    raw_graph_root: [u8; 32],
    constrained_root: [u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(CHILD_ROOT_DOMAIN);
    digest.update(binding.source_root);
    digest.update(binding.child_graph_binding);
    digest.update(binding.compacted_ancestry_root);
    digest.update([binding.equivalence as u8, child.k, child.support_unit as u8]);
    digest.update([child.minimizer_length]);
    digest.update(child.virtual_bucket_count.to_le_bytes());
    digest.update(child.exact_edge_table_sha256);
    digest.update(raw_graph_root);
    digest.update(transition_root);
    digest.update(constrained_root);
    digest.finalize().into()
}

fn witnessed_child_root(child: &WitnessedChild) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(CHILD_ROOT_DOMAIN);
    digest.update(child.source_binding.source_root);
    digest.update(child.source_binding.child_graph_binding);
    digest.update(child.source_binding.compacted_ancestry_root);
    digest.update([
        child.source_binding.equivalence as u8,
        child.k,
        child.support_unit as u8,
    ]);
    digest.update([child.minimizer_length]);
    digest.update(child.virtual_bucket_count.to_le_bytes());
    digest.update(child.exact_edge_table_sha256);
    digest.update(child.raw_compacted_graph_root);
    digest.update(child.transition_root);
    digest.update(child.constrained_graph_root);
    digest.finalize().into()
}

fn authenticated_witnessed_child_root(
    source_root: [u8; 32],
    graph_ancestry_root: [u8; 32],
    transition_root: [u8; 32],
    child_root: [u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(AUTHENTICATED_WITNESSED_CHILD_DOMAIN);
    digest.update(source_root);
    digest.update(graph_ancestry_root);
    digest.update(transition_root);
    digest.update(child_root);
    digest.finalize().into()
}

fn segment_id(k: u8, child_root: [u8; 32], segment: &WitnessedSegment) -> WitnessedSegmentId {
    let mut digest = Sha256::new();
    digest.update(SEGMENT_ID_DOMAIN);
    digest.update(child_root);
    digest.update([k, segment.topology as u8]);
    hash_bytes(&mut digest, &segment.sequence);
    hash_steps(&mut digest, &segment.steps);
    WitnessedSegmentId(digest.finalize().into())
}

fn segment_provenance(k: u8, child_root: [u8; 32], segment: &WitnessedSegment) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SEGMENT_PROVENANCE_DOMAIN);
    digest.update(child_root);
    digest.update(segment.id.0);
    digest.update(segment.parent_unitig_id.0);
    digest.update(segment.parent_start_step.to_le_bytes());
    digest.update(segment.parent_end_step_exclusive.to_le_bytes());
    digest.update([k, segment.topology as u8]);
    hash_bytes(&mut digest, &segment.sequence);
    hash_steps(&mut digest, &segment.steps);
    digest.update(segment.total_edge_support.to_le_bytes());
    digest.update(segment.internal_transition_rows.to_le_bytes());
    digest.update(segment.sum_accepted_window_occurrences.to_le_bytes());
    digest.update(
        segment
            .sum_distinct_supplied_fragment_instances
            .to_le_bytes(),
    );
    digest.update(segment.ordered_transition_rows_sha256);
    digest.finalize().into()
}

fn compacted_unitig_id(k: u8, unitig: &CompactedUnitig) -> CompactedUnitigId {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:experimental-compacted-unitig:v1\0");
    digest.update([k, unitig.topology as u8]);
    hash_bytes(&mut digest, &unitig.sequence);
    CompactedUnitigId(digest.finalize().into())
}

fn compacted_unitig_provenance(child: &CompactedGraphResult, unitig: &CompactedUnitig) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:experimental-compacted-unitig-provenance:v2\0");
    digest.update(child.source_identity);
    digest.update([child.support_unit as u8, child.k, unitig.topology as u8]);
    hash_steps(&mut digest, &unitig.steps);
    digest.finalize().into()
}

fn hash_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn hash_steps(digest: &mut Sha256, steps: &[CompactedEdgeStep]) {
    digest.update((steps.len() as u64).to_le_bytes());
    for step in steps {
        digest.update(step.key.to_be_bytes());
        digest.update([step.orientation as u8]);
        digest.update(step.support.to_le_bytes());
    }
}

fn hash_transition_row(digest: &mut Sha256, row: &TransitionRow) {
    digest.update(row.canonical_qmer.to_be_bytes());
    digest.update(row.accepted_window_occurrences.to_le_bytes());
    digest.update(row.distinct_supplied_fragment_instances.to_le_bytes());
    digest.update(row.sorted_event_frames_sha256);
}

fn hash_evidence(digest: &mut Sha256, evidence: OriginalReadTransitionEvidence) {
    digest.update([evidence.kind as u8]);
    digest.update(evidence.accepted_window_occurrences.to_le_bytes());
    digest.update(evidence.distinct_supplied_fragment_instances.to_le_bytes());
    digest.update(evidence.sorted_event_frames_sha256);
}

fn hash_segment_preimage(digest: &mut Sha256, segment: &WitnessedSegment) {
    digest.update(segment.parent_unitig_id.0);
    digest.update(segment.parent_start_step.to_le_bytes());
    digest.update(segment.parent_end_step_exclusive.to_le_bytes());
    digest.update([segment.topology as u8]);
    hash_bytes(digest, &segment.sequence);
    hash_steps(digest, &segment.steps);
    digest.update(segment.total_edge_support.to_le_bytes());
    digest.update(segment.minimum_edge_support.to_le_bytes());
    digest.update(segment.lower_median_edge_support.to_le_bytes());
    digest.update(segment.maximum_edge_support.to_le_bytes());
}

fn hash_decision(digest: &mut Sha256, decision: &TransitionDecision) {
    digest.update(decision.canonical_qmer.to_be_bytes());
    match decision.origin {
        TransitionCandidateOrigin::UnitigInterior {
            parent_unitig_id,
            left_step,
        } => {
            digest.update([0]);
            digest.update(parent_unitig_id.0);
            digest.update(left_step.to_le_bytes());
        }
        TransitionCandidateOrigin::ClosedWalkClosure { parent_unitig_id } => {
            digest.update([1]);
            digest.update(parent_unitig_id.0);
        }
        TransitionCandidateOrigin::RawCompactedLink(link) => {
            digest.update([2]);
            hash_compacted_link(digest, link);
        }
        TransitionCandidateOrigin::LedgerRetainedEndpointsNoRawCandidate => digest.update([3]),
        TransitionCandidateOrigin::LedgerEndpointNotRetained => digest.update([4]),
    }
    digest.update([decision.status as u8]);
    match decision.evidence {
        Some(evidence) => {
            digest.update([1]);
            hash_evidence(digest, evidence);
        }
        None => digest.update([0]),
    }
}

fn hash_compacted_link(digest: &mut Sha256, link: CompactedLink) {
    digest.update(link.from.0);
    digest.update([link.from_orientation as u8]);
    digest.update(link.to.0);
    digest.update([link.to_orientation as u8, link.overlap_bases]);
}

fn hash_compacted_stats(digest: &mut Sha256, child: &CompactedGraphResult) {
    for value in [
        child.stats.canonical_edges,
        child.stats.represented_canonical_edges,
        child.stats.input_support,
        child.stats.represented_support,
        child.stats.oriented_handles,
        child.stats.literal_nodes,
        child.stats.boundary_nodes,
        child.stats.degree_boundary_nodes,
        child.stats.self_reverse_complement_nodes,
        child.stats.self_reverse_complement_edges,
        child.stats.raw_walks,
        child.stats.linear_unitigs,
        child.stats.closed_unitigs,
        child.stats.directed_boundary_transitions,
        child.stats.link_candidates,
        child.stats.canonical_links,
        child.stats.output_bases,
    ] {
        digest.update(value.to_le_bytes());
    }
}

fn build_segment_id_index(segments: &[WitnessedSegment]) -> Result<Vec<SegmentIdIndex>> {
    let mut index = try_vec(segments.len(), "segment ID verification index")?;
    for (position, segment) in segments.iter().enumerate() {
        index.push(SegmentIdIndex {
            id: segment.id,
            index: position,
        });
    }
    index.sort_unstable();
    if index.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return integrity("witnessed segment identifiers are not unique");
    }
    Ok(index)
}

fn segment_index_by_id(index: &[SegmentIdIndex], id: WitnessedSegmentId) -> Result<usize> {
    index
        .binary_search_by_key(&id, |row| row.id)
        .ok()
        .and_then(|position| index.get(position))
        .map(|row| row.index)
        .ok_or_else(|| integrity_error("witnessed link references a missing segment"))
}

fn try_vec<T>(capacity: usize, label: &'static str) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve {label}: {cause}"),
        )
    })?;
    if values.capacity() > capacity {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "allocator returned {label} capacity {} above admitted {capacity}",
                values.capacity()
            ),
        ));
    }
    Ok(values)
}

fn try_copy_slice<T: Copy>(source: &[T], label: &'static str) -> Result<Vec<T>> {
    let mut values = try_vec(source.len(), label)?;
    values.extend_from_slice(source);
    Ok(values)
}

fn checked_bytes(count: usize, width: u64, label: &'static str) -> Result<u64> {
    checked_mul(u64_from_usize(count, label)?, width, label)
}

fn checked_sum(values: &[u64]) -> Result<u64> {
    values.iter().try_fold(0_u64, |total, value| {
        checked_add(total, *value, "reconstruction resource sum")
    })
}

fn checked_add(left: u64, right: u64, label: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(label))
}

fn checked_mul(left: u64, right: u64, label: &'static str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| overflow(label))
}

fn u64_from_usize(value: usize, label: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| overflow(label))
}

fn enforce_limit(observed: u64, limit: u64, code: ErrorCode, label: &'static str) -> Result<()> {
    if observed <= limit {
        Ok(())
    } else {
        Err(VeritasmError::new(
            code,
            format!("{label} {observed} exceeds limit {limit}"),
        ))
    }
}

fn integrity<T>(context: impl Into<String>) -> Result<T> {
    Err(integrity_error(context))
}

fn integrity_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegrityArtifact, context)
}

fn invariant<T>(context: impl Into<String>) -> Result<T> {
    Err(invariant_error(context))
}

fn invariant_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

fn overflow(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
    use crate::experimental::compacted_dbg::{
        compact_external_counts, compact_retained_counts, CompactedGraphLimits,
    };
    use crate::experimental::external_reduce::ExternalPartitionLimits;
    use crate::experimental::external_reduce::{ExactSupportCount, ExternalPartitionResult};
    use crate::experimental::partitioned_dbg::{route_minimizer, select_minimizer};
    use crate::experimental::retention::{
        authenticate_spool_external_counts, retain_spool_authenticated_counts, RetentionLimits,
        RetentionRule,
    };
    use crate::experimental::spool_external::SpoolExternalOptions;
    use crate::experimental::transition_witness::{
        build_transition_ledger, TransitionLedger, TransitionLedgerLimits, TransitionLedgerOptions,
    };
    use crate::spool::create_spool;
    use proptest::prelude::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn graph_limits() -> CompactedGraphLimits {
        CompactedGraphLimits {
            max_canonical_edges: 100_000,
            max_oriented_handles: 200_000,
            max_literal_nodes: 400_000,
            max_unitigs: 100_000,
            max_link_candidates: 10_000_000,
            max_output_bases: 20_000_000,
            max_accounted_bytes: 1_000_000_000,
        }
    }

    fn retention_limits() -> RetentionLimits {
        RetentionLimits {
            max_raw_keys: 100_000,
            max_retained_keys: 100_000,
            max_accounted_bytes: 32 << 20,
        }
    }

    fn limits() -> ReconstructionLimits {
        ReconstructionLimits {
            max_input_edges: 100_000,
            max_topology_candidates: 1_000_000,
            max_decision_rows: 1_100_000,
            max_segments: 100_000,
            max_links: 1_000_000,
            max_output_bases: 20_000_000,
            max_accounted_bytes: 1_000_000_000,
            max_verification_scratch_bytes: 127,
        }
    }

    fn ledger_limits() -> TransitionLedgerLimits {
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

    fn ledger(k: u8, reads: &[Vec<u8>]) -> TransitionLedger {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("reads.fasta");
        let mut fasta = Vec::new();
        for (index, read) in reads.iter().enumerate() {
            fasta.extend_from_slice(format!(">read-{index}\n").as_bytes());
            fasta.extend_from_slice(read);
            fasta.push(b'\n');
        }
        fs::write(&input, fasta).unwrap();
        let scientific = ScientificConfig::resolve(
            3,
            Profile::RetainAll,
            SupportUnit::AcceptedWindowOccurrence,
            None,
            0,
            false,
        )
        .unwrap();
        let spool_limits = Limits {
            memory_budget_bytes: 64 << 20,
            max_spool_bytes: 64 << 20,
            max_temp_bytes: 128 << 20,
            ..Limits::default()
        };
        let spool = create_spool(
            &InputSpec::Single(vec![input]),
            &scientific,
            &spool_limits,
            directory.path(),
        )
        .unwrap();
        build_transition_ledger(
            &spool,
            &TransitionLedgerOptions {
                work_dir: directory.path().to_path_buf(),
                k,
                limits: ledger_limits(),
            },
        )
        .unwrap()
    }

    fn graph_from_sequences(k: u8, sequences: &[Vec<u8>]) -> CompactedGraphResult {
        let mut counts = BTreeMap::<PackedKmer, u64>::new();
        for sequence in sequences {
            for window in sequence.windows(usize::from(k)) {
                let key = canonical_window(window).unwrap_or_else(|_| {
                    canonical_code(encode_exact_bases(window).unwrap(), k).unwrap()
                });
                *counts.entry(key).or_default() += 1;
            }
        }
        graph_from_counts(k, counts)
    }

    fn graph_from_edges(k: u8, edges: &[&[u8]]) -> CompactedGraphResult {
        let mut counts = BTreeMap::<PackedKmer, u64>::new();
        for edge in edges {
            assert_eq!(edge.len(), usize::from(k));
            let key = canonical_code(encode_exact_bases(edge).unwrap(), k).unwrap();
            *counts.entry(key).or_default() += 1;
        }
        graph_from_counts(k, counts)
    }

    fn graph_from_counts(k: u8, counts: BTreeMap<PackedKmer, u64>) -> CompactedGraphResult {
        let minimizer_length = (k - 1).min(5);
        let virtual_bucket_count = 7;
        let mut rows = counts
            .into_iter()
            .map(|(key, support)| {
                let minimizer = select_minimizer(key, k, minimizer_length).unwrap().key;
                ExactSupportCount {
                    bucket_id: route_minimizer(minimizer, minimizer_length, virtual_bucket_count)
                        .unwrap(),
                    minimizer,
                    key,
                    support,
                }
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| (row.bucket_id, row.key));
        let support_events = rows.iter().map(|row| row.support).sum();
        let input = ExternalPartitionResult {
            k,
            minimizer_length,
            virtual_bucket_count,
            source_identity: [0x5a; 32],
            support_unit: WideRunSupportUnit::AcceptedWindowOccurrence,
            support_events,
            distinct_kmers: rows.len() as u64,
            edge_counts: rows,
            final_runs: Vec::new(),
            replacements: Vec::new(),
            run_files_created: 0,
            open_files_high_water: 0,
            temporary_bytes_final: 0,
            temporary_bytes_high_water: 0,
        };
        compact_external_counts(&input, graph_limits()).unwrap()
    }

    fn reconstruct(graph: &CompactedGraphResult, ledger: &TransitionLedger) -> WitnessedChild {
        let view = ledger.view().unwrap();
        constrain_unverified_compacted_child(graph, &view, limits()).unwrap()
    }

    fn spellings(length: usize) -> Vec<Vec<u8>> {
        let count = 4_usize.pow(length as u32);
        let mut result = Vec::with_capacity(count);
        for encoded in 0..count {
            let mut value = encoded;
            let mut sequence = vec![b'A'; length];
            for base in sequence.iter_mut().rev() {
                *base = b"ACGT"[value & 3];
                value >>= 2;
            }
            result.push(sequence);
        }
        result
    }

    fn all_qmers(k: u8) -> TransitionLedger {
        ledger(k, &spellings(usize::from(k) + 1))
    }

    fn path_sequence(graph: &CompactedGraphResult) -> Vec<u8> {
        graph
            .unitigs
            .iter()
            .max_by_key(|unitig| unitig.steps.len())
            .unwrap()
            .sequence
            .clone()
    }

    fn assert_only_original_read_evidence(child: &WitnessedChild) {
        assert!(child
            .links
            .iter()
            .all(|link| { link.evidence.kind == AdjacencyEvidenceKind::OriginalReadTransition }));
        assert!(child
            .decisions
            .iter()
            .filter_map(|row| row.evidence)
            .all(|evidence| evidence.kind == AdjacencyEvidenceKind::OriginalReadTransition));
    }

    #[test]
    fn missing_internal_transition_splits_without_dropping_edges() {
        let graph = graph_from_sequences(3, &[b"AACCG".to_vec()]);
        let path = path_sequence(&graph);
        assert_eq!(path.len(), 5);
        let evidence = ledger(3, &[path[..4].to_vec()]);
        let child = reconstruct(&graph, &evidence);
        assert_eq!(child.segments.len(), 2);
        assert_eq!(child.conservation.represented_canonical_edges, 3);
        assert_eq!(
            child
                .decisions
                .iter()
                .filter(|row| {
                    matches!(row.origin, TransitionCandidateOrigin::UnitigInterior { .. })
                        && row.status == TransitionDecisionStatus::ExcludedNoOriginalReadWitness
                })
                .count(),
            1
        );
        assert!(child
            .segments
            .iter()
            .all(|segment| segment.topology == CompactedTopology::Linear));
        assert_only_original_read_evidence(&child);
    }

    #[test]
    fn missing_boundary_cross_product_is_visible_and_not_emitted() {
        let graph = graph_from_edges(3, &[b"CAA", b"GAA", b"AAC", b"AAG"]);
        let evidence = ledger(3, &[b"CAAC".to_vec(), b"GAAG".to_vec()]);
        let child = reconstruct(&graph, &evidence);
        assert!(graph.links.len() >= 4);
        assert_eq!(child.links.len(), 2);
        assert!(child
            .decisions
            .iter()
            .any(|row| row.status == TransitionDecisionStatus::ExcludedNoOriginalReadWitness));
        assert_only_original_read_evidence(&child);
    }

    #[test]
    fn closed_walk_survives_only_with_interior_and_closure_witnesses() {
        let graph = graph_from_edges(3, &[b"ACA", b"CAC"]);
        let sequence = path_sequence(&graph);
        let complete = ledger(
            3,
            &[{
                let mut read = sequence.clone();
                read.push(sequence[2]);
                read
            }],
        );
        let closed = reconstruct(&graph, &complete);
        assert_eq!(closed.conservation.closed_segments, 1);
        assert_eq!(closed.links.len(), 1);

        let interior_only = ledger(3, &[sequence[..4].to_vec()]);
        let opened = reconstruct(&graph, &interior_only);
        assert_eq!(opened.conservation.closed_segments, 0);
        assert!(opened.links.is_empty());

        let closure = closure_qmer(3, &graph.unitigs[0]).unwrap();
        let closure_read = super::super::wide_kmer::decode_mer(closure, 4).unwrap();
        let closure_only = ledger(3, &[closure_read]);
        let split = reconstruct(&graph, &closure_only);
        assert_eq!(split.segments.len(), 2);
        assert_eq!(split.links.len(), 1);
    }

    #[test]
    fn endpoint_exclusion_and_iupac_boundary_are_explicit() {
        let graph = graph_from_sequences(3, &[b"AACCG".to_vec()]);
        let evidence = ledger(3, &[b"CCCCNAACC".to_vec()]);
        let child = reconstruct(&graph, &evidence);
        assert!(child
            .decisions
            .iter()
            .any(|row| { row.status == TransitionDecisionStatus::ExcludedEndpointNotRetained }));
        assert!(child.conservation.excluded_endpoint_not_retained > 0);
        assert!(evidence.window_stats().ambiguity_only > 0);
    }

    #[test]
    fn unverified_state_is_visible_and_root_bound() {
        let graph = graph_from_sequences(3, &[b"AAACG".to_vec()]);
        let evidence = ledger(3, &[b"AAACG".to_vec()]);
        let child = reconstruct(&graph, &evidence);
        assert_eq!(
            child.source_binding.equivalence(),
            SourceEquivalence::Unverified
        );
        let mut tampered = child.clone();
        tampered.source_binding.equivalence = SourceEquivalence::AuthenticatedSpoolDescriptor;
        assert_eq!(
            validate_no_unsupported_adjacencies(&tampered, &evidence.view().unwrap(), limits())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn validator_refuses_every_non_read_evidence_kind_and_tampering() {
        let graph = graph_from_edges(3, &[b"CAA", b"GAA", b"AAC", b"AAG"]);
        let evidence = ledger(3, &[b"CAAC".to_vec(), b"GAAG".to_vec()]);
        let child = reconstruct(&graph, &evidence);
        assert!(!child.links.is_empty());
        for kind in [
            AdjacencyEvidenceKind::HigherKChild,
            AdjacencyEvidenceKind::MultiKRelation,
            AdjacencyEvidenceKind::BloomMembership,
            AdjacencyEvidenceKind::TopologyOnly,
        ] {
            let mut tampered = child.clone();
            tampered.links[0].evidence.kind = kind;
            assert!(validate_no_unsupported_adjacencies(
                &tampered,
                &evidence.view().unwrap(),
                limits()
            )
            .is_err());
        }
        for mutate in [
            |value: &mut WitnessedChild| value.transition_root[0] ^= 1,
            |value: &mut WitnessedChild| value.raw_compacted_graph_root[0] ^= 1,
            |value: &mut WitnessedChild| value.child_root[0] ^= 1,
            |value: &mut WitnessedChild| value.segments[0].sequence[0] ^= 1,
            |value: &mut WitnessedChild| value.segments[0].provenance_sha256[0] ^= 1,
            |value: &mut WitnessedChild| value.links[0].canonical_qmer = PackedKmer::ZERO,
            |value: &mut WitnessedChild| value.decisions[0].canonical_qmer = PackedKmer::ZERO,
            |value: &mut WitnessedChild| value.conservation.represented_canonical_edges += 1,
            |value: &mut WitnessedChild| value.conservation.accounted_peak_bytes += 1,
        ] as [fn(&mut WitnessedChild); 9]
        {
            let mut tampered = child.clone();
            mutate(&mut tampered);
            assert!(validate_no_unsupported_adjacencies(
                &tampered,
                &evidence.view().unwrap(),
                limits()
            )
            .is_err());
        }
    }

    #[test]
    fn authenticated_spool_child_happy_path_and_tampering_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("source.fasta");
        fs::write(&input, b">left-1\nCAAC\n>left-2\nCAAC\n>right\nGAAG\n").unwrap();
        let scientific = ScientificConfig::resolve(
            3,
            Profile::RetainAll,
            SupportUnit::AcceptedWindowOccurrence,
            None,
            0,
            false,
        )
        .unwrap();
        let spool_limits = Limits {
            memory_budget_bytes: 64 << 20,
            max_spool_bytes: 64 << 20,
            max_temp_bytes: 128 << 20,
            ..Limits::default()
        };
        let spool = create_spool(
            &InputSpec::Single(vec![input]),
            &scientific,
            &spool_limits,
            directory.path(),
        )
        .unwrap();
        let external_options = SpoolExternalOptions {
            work_dir: directory.path().to_path_buf(),
            k: 3,
            minimizer_length: 2,
            virtual_bucket_count: 5,
            support_unit: SupportUnit::AcceptedWindowOccurrence,
            max_fragment_decode_bytes: 1 << 20,
            max_fragment_windows: 10_000,
            limits: ExternalPartitionLimits {
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
            },
        };
        let raw = authenticate_spool_external_counts(&spool, &external_options, retention_limits())
            .unwrap();
        let retained =
            retain_spool_authenticated_counts(&raw, RetentionRule::RetainAll, retention_limits())
                .unwrap();
        let graph = compact_retained_counts(&retained, graph_limits()).unwrap();
        let transition = build_transition_ledger(
            &spool,
            &TransitionLedgerOptions {
                work_dir: directory.path().to_path_buf(),
                k: 3,
                limits: ledger_limits(),
            },
        )
        .unwrap();
        let child = constrain_authenticated_compacted_child(&graph, &transition, limits()).unwrap();
        assert_eq!(
            child.view().source_equivalence(),
            SourceEquivalence::AuthenticatedSpoolDescriptor
        );
        child
            .validate_against_sources(&graph, &transition, limits())
            .unwrap();
        let authenticated_peak = child
            .view()
            .conservation()
            .accounted_peak_bytes
            .checked_mul(2)
            .unwrap();
        let mut exact_authenticated_limits = limits();
        exact_authenticated_limits.max_accounted_bytes = authenticated_peak;
        child
            .validate_against_sources(&graph, &transition, exact_authenticated_limits)
            .unwrap();
        exact_authenticated_limits.max_accounted_bytes = authenticated_peak - 1;
        assert_eq!(
            child
                .validate_against_sources(&graph, &transition, exact_authenticated_limits)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );

        let wrapper_mutations: &[fn(&mut AuthenticatedWitnessedChild)] = &[
            |value| value.graph_ancestry_root[0] ^= 1,
            |value| value.transition_root[0] ^= 1,
            |value| value.authentication_root[0] ^= 1,
        ];
        for (index, mutate) in wrapper_mutations.iter().enumerate() {
            let mut changed = child.clone();
            mutate(&mut changed);
            assert!(
                changed
                    .validate_against_sources(&graph, &transition, limits())
                    .is_err(),
                "authenticated wrapper mutation {index} was accepted"
            );
        }

        let child_mutations: &[fn(&mut AuthenticatedWitnessedChild)] = &[
            |value| value.child.k ^= 1,
            |value| value.child.q ^= 1,
            |value| value.child.minimizer_length ^= 1,
            |value| value.child.virtual_bucket_count += 1,
            |value| {
                value.child.support_unit = match value.child.support_unit {
                    WideRunSupportUnit::AcceptedWindowOccurrence => {
                        WideRunSupportUnit::SuppliedFragmentInstance
                    }
                    WideRunSupportUnit::SuppliedFragmentInstance => {
                        WideRunSupportUnit::AcceptedWindowOccurrence
                    }
                }
            },
            |value| value.child.source_binding.source_root[0] ^= 1,
            |value| value.child.source_binding.child_graph_binding[0] ^= 1,
            |value| value.child.source_binding.compacted_ancestry_root[0] ^= 1,
            |value| value.child.source_binding.equivalence = SourceEquivalence::Unverified,
            |value| value.child.transition_root[0] ^= 1,
            |value| value.child.exact_edge_table_sha256[0] ^= 1,
            |value| value.child.raw_compacted_graph_root[0] ^= 1,
            |value| value.child.constrained_graph_root[0] ^= 1,
            |value| value.child.child_root[0] ^= 1,
            |value| {
                value.child.segments.pop();
            },
            |value| {
                value.child.links.pop();
            },
            |value| {
                value.child.decisions.pop();
            },
        ];
        for (index, mutate) in child_mutations.iter().enumerate() {
            let mut changed = child.clone();
            mutate(&mut changed);
            assert!(
                changed
                    .validate_against_sources(&graph, &transition, limits())
                    .is_err(),
                "materialized child mutation {index} was accepted"
            );
        }

        let segment_mutations: &[fn(&mut AuthenticatedWitnessedChild)] = &[
            |value| value.child.segments[0].id.0[0] ^= 1,
            |value| value.child.segments[0].parent_unitig_id.0[0] ^= 1,
            |value| value.child.segments[0].parent_start_step += 1,
            |value| value.child.segments[0].parent_end_step_exclusive += 1,
            |value| {
                value.child.segments[0].topology = match value.child.segments[0].topology {
                    CompactedTopology::Linear => CompactedTopology::ClosedWalk,
                    CompactedTopology::ClosedWalk => CompactedTopology::Linear,
                }
            },
            |value| value.child.segments[0].sequence[0] ^= 1,
            |value| {
                let mut words = value.child.segments[0].steps[0].key.words();
                words[3] ^= 1;
                value.child.segments[0].steps[0].key = PackedKmer::from_words(words);
            },
            |value| {
                value.child.segments[0].steps[0].orientation =
                    match value.child.segments[0].steps[0].orientation {
                        EdgeOrientation::Canonical => EdgeOrientation::ReverseComplement,
                        EdgeOrientation::ReverseComplement => EdgeOrientation::Canonical,
                        EdgeOrientation::SelfReverseComplement => EdgeOrientation::Canonical,
                    }
            },
            |value| value.child.segments[0].steps[0].support += 1,
            |value| value.child.segments[0].total_edge_support += 1,
            |value| value.child.segments[0].minimum_edge_support += 1,
            |value| value.child.segments[0].lower_median_edge_support += 1,
            |value| value.child.segments[0].maximum_edge_support += 1,
            |value| value.child.segments[0].internal_transition_rows += 1,
            |value| value.child.segments[0].sum_accepted_window_occurrences += 1,
            |value| value.child.segments[0].sum_distinct_supplied_fragment_instances += 1,
            |value| value.child.segments[0].ordered_transition_rows_sha256[0] ^= 1,
            |value| value.child.segments[0].provenance_sha256[0] ^= 1,
        ];
        for (index, mutate) in segment_mutations.iter().enumerate() {
            let mut changed = child.clone();
            mutate(&mut changed);
            assert!(
                changed
                    .validate_against_sources(&graph, &transition, limits())
                    .is_err(),
                "segment or edge-step mutation {index} was accepted"
            );
        }

        let link_mutations: &[fn(&mut AuthenticatedWitnessedChild)] = &[
            |value| value.child.links[0].from.0[0] ^= 1,
            |value| {
                value.child.links[0].from_orientation = match value.child.links[0].from_orientation
                {
                    UnitigOrientation::Forward => UnitigOrientation::ReverseComplement,
                    UnitigOrientation::ReverseComplement => UnitigOrientation::Forward,
                }
            },
            |value| value.child.links[0].to.0[0] ^= 1,
            |value| {
                value.child.links[0].to_orientation = match value.child.links[0].to_orientation {
                    UnitigOrientation::Forward => UnitigOrientation::ReverseComplement,
                    UnitigOrientation::ReverseComplement => UnitigOrientation::Forward,
                }
            },
            |value| value.child.links[0].overlap_bases ^= 1,
            |value| {
                let mut words = value.child.links[0].canonical_qmer.words();
                words[3] ^= 1;
                value.child.links[0].canonical_qmer = PackedKmer::from_words(words);
            },
            |value| value.child.links[0].evidence.kind = AdjacencyEvidenceKind::TopologyOnly,
            |value| value.child.links[0].evidence.accepted_window_occurrences += 1,
            |value| {
                value.child.links[0]
                    .evidence
                    .distinct_supplied_fragment_instances += 1
            },
            |value| value.child.links[0].evidence.sorted_event_frames_sha256[0] ^= 1,
        ];
        for (index, mutate) in link_mutations.iter().enumerate() {
            let mut changed = child.clone();
            mutate(&mut changed);
            assert!(
                changed
                    .validate_against_sources(&graph, &transition, limits())
                    .is_err(),
                "link or link-evidence mutation {index} was accepted"
            );
        }

        let decision_mutations: &[fn(&mut AuthenticatedWitnessedChild)] = &[
            |value| {
                let mut words = value.child.decisions[0].canonical_qmer.words();
                words[3] ^= 1;
                value.child.decisions[0].canonical_qmer = PackedKmer::from_words(words);
            },
            |value| {
                value.child.decisions[0].origin =
                    TransitionCandidateOrigin::LedgerEndpointNotRetained
            },
            |value| {
                value.child.decisions[0].status =
                    TransitionDecisionStatus::ExcludedEndpointNotRetained
            },
            |value| value.child.decisions[0].evidence = None,
            |value| {
                value
                    .child
                    .decisions
                    .iter_mut()
                    .find_map(|row| row.evidence.as_mut())
                    .expect("fixture must contain admitted evidence")
                    .accepted_window_occurrences += 1
            },
        ];
        for (index, mutate) in decision_mutations.iter().enumerate() {
            let mut changed = child.clone();
            mutate(&mut changed);
            assert!(
                changed
                    .validate_against_sources(&graph, &transition, limits())
                    .is_err(),
                "decision or decision-evidence mutation {index} was accepted"
            );
        }

        let conservation_mutations: &[fn(&mut AuthenticatedWitnessedChild)] = &[
            |value| value.child.conservation.input_canonical_edges += 1,
            |value| value.child.conservation.represented_canonical_edges += 1,
            |value| value.child.conservation.input_edge_support += 1,
            |value| value.child.conservation.represented_edge_support += 1,
            |value| value.child.conservation.topology_candidates += 1,
            |value| value.child.conservation.admitted_topology_candidates += 1,
            |value| value.child.conservation.excluded_no_original_read_witness += 1,
            |value| value.child.conservation.ledger_rows += 1,
            |value| value.child.conservation.eligible_ledger_rows += 1,
            |value| value.child.conservation.excluded_endpoint_not_retained += 1,
            |value| value.child.conservation.segments += 1,
            |value| value.child.conservation.linear_segments += 1,
            |value| value.child.conservation.closed_segments += 1,
            |value| value.child.conservation.witnessed_links += 1,
            |value| value.child.conservation.output_bases += 1,
            |value| value.child.conservation.accounted_peak_bytes += 1,
        ];
        for (index, mutate) in conservation_mutations.iter().enumerate() {
            let mut changed = child.clone();
            mutate(&mut changed);
            assert!(
                changed
                    .validate_against_sources(&graph, &transition, limits())
                    .is_err(),
                "conservation mutation {index} was accepted"
            );
        }

        let mut copied = graph.checked_graph().unwrap().clone();
        copied
            .links
            .pop()
            .expect("branch graph must contain a link");
        copied.stats.canonical_links -= 1;
        let view = transition.view().unwrap();
        let unverified = constrain_unverified_compacted_child(&copied, &view, limits()).unwrap();
        assert_eq!(
            unverified.source_binding.equivalence(),
            SourceEquivalence::Unverified
        );

        let threshold_one = retain_spool_authenticated_counts(
            &raw,
            RetentionRule::InclusiveSupport { minimum_support: 1 },
            retention_limits(),
        )
        .unwrap();
        let threshold_one_graph = compact_retained_counts(&threshold_one, graph_limits()).unwrap();
        assert_ne!(graph.ancestry_root(), threshold_one_graph.ancestry_root());
        assert_eq!(
            child
                .validate_against_sources(&threshold_one_graph, &transition, limits())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        let threshold_one_child =
            constrain_authenticated_compacted_child(&threshold_one_graph, &transition, limits())
                .unwrap();
        threshold_one_child
            .validate_against_sources(&threshold_one_graph, &transition, limits())
            .unwrap();
        assert_eq!(
            threshold_one_child
                .validate_against_sources(&graph, &transition, limits())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let threshold_two = retain_spool_authenticated_counts(
            &raw,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            retention_limits(),
        )
        .unwrap();
        let threshold_two_graph = compact_retained_counts(&threshold_two, graph_limits()).unwrap();
        let threshold_two_child =
            constrain_authenticated_compacted_child(&threshold_two_graph, &transition, limits())
                .unwrap();
        assert_eq!(threshold_two_graph.retained_key_count(), 2);
        assert_eq!(
            threshold_two_child
                .view()
                .conservation()
                .input_canonical_edges,
            2
        );
        assert!(
            threshold_two_child
                .view()
                .conservation()
                .excluded_endpoint_not_retained
                > 0
        );
        threshold_two_child
            .validate_against_sources(&threshold_two_graph, &transition, limits())
            .unwrap();

        let other_directory = tempfile::tempdir().unwrap();
        let other_input = other_directory.path().join("other.fasta");
        fs::write(&other_input, b">other\nAAAACCCC\n").unwrap();
        let other_spool = create_spool(
            &InputSpec::Single(vec![other_input]),
            &scientific,
            &spool_limits,
            other_directory.path(),
        )
        .unwrap();
        let other_transition = build_transition_ledger(
            &other_spool,
            &TransitionLedgerOptions {
                work_dir: other_directory.path().to_path_buf(),
                k: 3,
                limits: ledger_limits(),
            },
        )
        .unwrap();
        assert_eq!(
            constrain_authenticated_compacted_child(&graph, &other_transition, limits())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn resource_limits_have_minus_exact_and_plus_one_boundaries() {
        let graph = graph_from_sequences(3, &[b"AACCG".to_vec()]);
        let evidence = ledger(3, &[b"AACCG".to_vec()]);
        let view = evidence.view().unwrap();
        let binding = ChildSourceBinding::unverified(&graph, &view);
        let edge_count = graph.stats.canonical_edges;
        let topology = graph
            .unitigs
            .iter()
            .map(|unitig| unitig.steps.len().saturating_sub(1))
            .sum::<usize>()
            + graph
                .unitigs
                .iter()
                .filter(|unitig| unitig.topology == CompactedTopology::ClosedWalk)
                .count()
            + graph.links.len();
        let decision_bound = topology + evidence.rows().len();
        let output_bound = edge_count * u64::from(graph.k);
        let mut exact = limits();
        exact.max_input_edges = edge_count;
        exact.max_topology_candidates = topology as u64;
        exact.max_decision_rows = decision_bound as u64;
        exact.max_segments = edge_count;
        exact.max_links = (graph.links.len()
            + graph
                .unitigs
                .iter()
                .filter(|unitig| unitig.topology == CompactedTopology::ClosedWalk)
                .count()
            + evidence.rows().len()) as u64;
        exact.max_output_bases = output_bound;
        let child = constrain_compacted_child_with_binding(&graph, &view, binding, exact).unwrap();
        let mut exact_memory = exact;
        exact_memory.max_accounted_bytes = child.conservation.accounted_peak_bytes;
        constrain_compacted_child_with_binding(&graph, &view, binding, exact_memory).unwrap();

        for mutate in [
            |value: &mut ReconstructionLimits| value.max_input_edges -= 1,
            |value: &mut ReconstructionLimits| value.max_topology_candidates -= 1,
            |value: &mut ReconstructionLimits| value.max_decision_rows -= 1,
            |value: &mut ReconstructionLimits| value.max_segments -= 1,
            |value: &mut ReconstructionLimits| value.max_links -= 1,
            |value: &mut ReconstructionLimits| value.max_output_bases -= 1,
            |value: &mut ReconstructionLimits| value.max_accounted_bytes -= 1,
            |value: &mut ReconstructionLimits| value.max_verification_scratch_bytes = 126,
        ] as [fn(&mut ReconstructionLimits); 8]
        {
            let mut below = exact_memory;
            mutate(&mut below);
            assert!(constrain_compacted_child_with_binding(&graph, &view, binding, below).is_err());
        }
        let mut plus_one = exact_memory;
        plus_one.max_input_edges += 1;
        plus_one.max_topology_candidates += 1;
        plus_one.max_decision_rows += 1;
        plus_one.max_segments += 1;
        plus_one.max_links += 1;
        plus_one.max_output_bases += 1;
        plus_one.max_accounted_bytes += 1;
        constrain_compacted_child_with_binding(&graph, &view, binding, plus_one).unwrap();

        for mutate in [
            |value: &mut ReconstructionLimits| value.max_input_edges = 0,
            |value: &mut ReconstructionLimits| value.max_topology_candidates = 0,
            |value: &mut ReconstructionLimits| value.max_decision_rows = 0,
            |value: &mut ReconstructionLimits| value.max_segments = 0,
            |value: &mut ReconstructionLimits| value.max_links = 0,
            |value: &mut ReconstructionLimits| value.max_output_bases = 0,
            |value: &mut ReconstructionLimits| value.max_accounted_bytes = 0,
            |value: &mut ReconstructionLimits| value.max_verification_scratch_bytes = 0,
        ] as [fn(&mut ReconstructionLimits); 8]
        {
            let mut invalid = limits();
            mutate(&mut invalid);
            assert_eq!(
                validate_no_unsupported_adjacencies(&child, &view, invalid)
                    .unwrap_err()
                    .code(),
                ErrorCode::ConfigurationInvalidLimit
            );
        }
    }

    #[test]
    fn exhaustive_small_strings_validate_against_complete_literal_qmer_oracle() {
        let evidence = all_qmers(3);
        let view = evidence.view().unwrap();
        for length in 3..=5 {
            for sequence in spellings(length) {
                let graph = graph_from_sequences(3, std::slice::from_ref(&sequence));
                let child = constrain_compacted_child_with_binding(
                    &graph,
                    &view,
                    ChildSourceBinding::unverified(&graph, &view),
                    limits(),
                )
                .unwrap();
                validate_no_unsupported_adjacencies(&child, &view, limits()).unwrap();
                assert_eq!(
                    child.conservation.input_canonical_edges,
                    child.conservation.represented_canonical_edges
                );
                assert_eq!(child.conservation.excluded_no_original_read_witness, 0);
            }
        }
    }

    #[test]
    fn complete_k5_branch_graph_accounts_for_every_raw_link_candidate() {
        let edge_spellings = spellings(5);
        let edge_slices = edge_spellings.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let graph = graph_from_edges(5, &edge_slices);
        let evidence = all_qmers(5);
        let child = reconstruct(&graph, &evidence);

        assert_eq!(graph.stats.canonical_edges, 512);
        assert_eq!(graph.unitigs.len(), 512);
        assert_eq!(graph.links.len(), 2_080);
        assert_eq!(evidence.rows().len(), 2_080);
        assert_eq!(child.conservation.topology_candidates, 2_080);
        assert_eq!(child.conservation.admitted_topology_candidates, 2_080);
        assert_eq!(child.conservation.excluded_no_original_read_witness, 0);
        assert_eq!(child.links.len(), 2_080);
        assert_eq!(child.decisions.len(), 2_080);
        for link in &graph.links {
            assert!(child.decisions.iter().any(|decision| {
                decision.origin == TransitionCandidateOrigin::RawCompactedLink(*link)
                    && decision.status == TransitionDecisionStatus::AdmittedOriginalRead
            }));
        }
        validate_no_unsupported_adjacencies(&child, &evidence.view().unwrap(), limits()).unwrap();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn randomized_single_bit_mutations_of_reported_digests_are_rejected(
            field in 0_usize..9,
            byte in 0_usize..32,
            bit in 0_u8..8,
        ) {
            let graph = graph_from_edges(3, &[b"CAA", b"GAA", b"AAC", b"AAG"]);
            let evidence = ledger(3, &[b"CAAC".to_vec(), b"GAAG".to_vec()]);
            let mut child = reconstruct(&graph, &evidence);
            let mask = 1_u8 << bit;
            match field {
                0 => child.transition_root[byte] ^= mask,
                1 => child.exact_edge_table_sha256[byte] ^= mask,
                2 => child.raw_compacted_graph_root[byte] ^= mask,
                3 => child.constrained_graph_root[byte] ^= mask,
                4 => child.child_root[byte] ^= mask,
                5 => child.segments[0].id.0[byte] ^= mask,
                6 => child.segments[0].provenance_sha256[byte] ^= mask,
                7 => child.links[0].evidence.sorted_event_frames_sha256[byte] ^= mask,
                8 => child
                    .decisions
                    .iter_mut()
                    .find_map(|row| row.evidence.as_mut())
                    .unwrap()
                    .sorted_event_frames_sha256[byte] ^= mask,
                _ => unreachable!(),
            }
            prop_assert!(validate_no_unsupported_adjacencies(
                &child,
                &evidence.view().unwrap(),
                limits(),
            )
            .is_err());
        }
    }

    #[test]
    fn scheduling_and_repeat_execution_are_byte_structurally_deterministic() {
        let graph = graph_from_edges(3, &[b"CAA", b"GAA", b"AAC", b"AAG"]);
        let evidence = ledger(3, &[b"CAAC".to_vec(), b"GAAG".to_vec()]);
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| reconstruct(&graph, &evidence));
        let four = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap()
            .install(|| reconstruct(&graph, &evidence));
        assert_eq!(one, four);
        assert_eq!(one, reconstruct(&graph, &evidence));
    }

    #[test]
    fn wide_k126_and_large_linear_child_avoid_quadratic_link_lookup() {
        let wide = (0..127)
            .map(|index| b"ACGTTGCA"[(index * 5 + index / 3) % 8])
            .collect::<Vec<_>>();
        let graph = graph_from_sequences(126, std::slice::from_ref(&wide));
        let evidence = ledger(126, std::slice::from_ref(&wide));
        let child = reconstruct(&graph, &evidence);
        assert_eq!(child.k, 126);
        validate_no_unsupported_adjacencies(&child, &evidence.view().unwrap(), limits()).unwrap();

        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut long = Vec::with_capacity(5_000);
        for _ in 0..5_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            long.push(b"ACGT"[(state & 3) as usize]);
        }
        let graph = graph_from_sequences(15, std::slice::from_ref(&long));
        let evidence = ledger(15, std::slice::from_ref(&long));
        let child = reconstruct(&graph, &evidence);
        validate_no_unsupported_adjacencies(&child, &evidence.view().unwrap(), limits()).unwrap();
        assert_eq!(
            child.conservation.input_canonical_edges,
            child.conservation.represented_canonical_edges
        );
    }
}
