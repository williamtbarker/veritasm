//! PRESERVED WORK IN PROGRESS: not declared in experimental/mod.rs and not
//! compiled or exercised by Cargo. See docs/RESUME.md before integrating.
//!
//! Evidence-preserving recompaction of a read-witness-constrained child.
//!
//! This module joins only uniquely forced paths of links already admitted by
//! [`super::evidence_reconstruction`]. It neither creates graph adjacencies nor
//! authorizes a biological path through a branch. The stable CLI does not call
//! this module.

use super::compacted_dbg::{
    AuthenticatedCompactedGraph, CompactedTopology, EdgeOrientation, UnitigOrientation,
};
use super::evidence_reconstruction::{
    constrain_authenticated_compacted_child, validate_no_unsupported_adjacencies,
    AdjacencyEvidenceKind, AuthenticatedWitnessedChild, AuthenticatedWitnessedChildView,
    OriginalReadTransitionEvidence, ReconstructionLimits, SourceEquivalence, WitnessedChild,
    WitnessedLink, WitnessedSegment, WitnessedSegmentId,
};
use super::transition_witness::{TransitionLedger, TransitionLedgerView, TransitionRow};
use super::wide_kmer::{canonical_code, encode_exact_bases, PackedKmer};
use crate::error::{ErrorCode, Result, VeritasmError};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::mem::size_of;

const PATH_ID_DOMAIN: &[u8] = b"veritasm:witnessed-recompacted-path:v1\0";
const PATH_EVIDENCE_DOMAIN: &[u8] = b"veritasm:witnessed-recompacted-evidence:v1\0";
const PATH_SET_DOMAIN: &[u8] = b"veritasm:witnessed-recompacted-set:v1\0";
const AUTHENTICATED_PATH_SET_DOMAIN: &[u8] =
    b"veritasm:authenticated-witnessed-recompacted-set:v1\0";
const ACCOUNTING_MARGIN_BYTES: u64 = 16_384;

/// Explicit owned-payload and verifier limits for one witnessed recompaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WitnessedRecompactionLimits {
    pub max_input_segments: u64,
    pub max_input_links: u64,
    pub max_expanded_arcs: u64,
    pub max_output_paths: u64,
    pub max_selected_links: u64,
    pub max_output_bases: u64,
    /// Construction payload plus a fixed implementation margin, not RSS.
    pub max_accounted_bytes: u64,
    /// Additional reconstruction and path-build payload allowed to verification.
    pub max_verification_scratch_bytes: u64,
}

/// Whether the path set came from an opaque authenticated capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PathSourceEquivalence {
    /// A report and ledger were checked, but source-backed ancestry was not proved.
    UnverifiedReport = 0,
    /// An opaque source-backed witnessed child was independently replayed.
    AuthenticatedCapability = 1,
}

/// Exact identifier for one oriented path through witnessed segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WitnessedPathId(pub [u8; 32]);

/// Presentation topology of one recompacted path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum WitnessedPathTopology {
    /// A linear sequence with no asserted terminal closure.
    Linear = 0,
    /// A residual unique cycle whose closure is an exact witnessed link.
    ClosedWalkCandidate = 1,
}

/// One source segment and the orientation used in a path spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OrientedWitnessedSegment {
    pub segment_id: WitnessedSegmentId,
    pub orientation: UnitigOrientation,
}

/// One selected source link in the traversal orientation used by a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WitnessedPathJoin {
    pub source_link_index: u64,
    pub from: WitnessedSegmentId,
    pub from_orientation: UnitigOrientation,
    pub to: WitnessedSegmentId,
    pub to_orientation: UnitigOrientation,
    pub overlap_bases: u8,
    pub canonical_qmer: PackedKmer,
    pub evidence: OriginalReadTransitionEvidence,
}

/// A source link deliberately not selected into a uniquely forced path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PreservedWitnessedLink {
    pub source_link_index: u64,
    pub from_path: WitnessedPathId,
    pub to_path: WitnessedPathId,
    pub link: WitnessedLink,
}

/// One evidence-bearing path. Segment records in the source child remain immutable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessedPath {
    pub id: WitnessedPathId,
    pub topology: WitnessedPathTopology,
    pub sequence: Vec<u8>,
    pub members: Vec<OrientedWitnessedSegment>,
    pub joins: Vec<WitnessedPathJoin>,
    pub canonical_edge_steps: u64,
    pub total_edge_support: u64,
    pub minimum_edge_support: u64,
    pub lower_median_edge_support: u64,
    pub maximum_edge_support: u64,
    /// Counts adjacency positions. Repeated q-mers are counted repeatedly.
    pub transition_rows: u64,
    pub minimum_transition_occurrences: Option<u64>,
    pub minimum_transition_fragment_instances: Option<u64>,
    /// A positional sum, not a union of fragment identities.
    pub sum_transition_occurrences: u64,
    /// A positional sum, not a count of distinct molecules across the path.
    pub sum_transition_fragment_instances: u64,
    pub ordered_transition_evidence_sha256: [u8; 32],
}

/// Conservation and bounded-resource evidence for one path set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WitnessedPathConservation {
    pub input_segments: u64,
    pub represented_segments: u64,
    pub input_links: u64,
    pub selected_links: u64,
    pub preserved_links: u64,
    pub output_paths: u64,
    pub linear_paths: u64,
    pub closed_walk_candidates: u64,
    pub canonical_edge_steps: u64,
    pub represented_edge_steps: u64,
    pub input_segment_bases: u64,
    pub output_path_bases: u64,
    pub spelled_joins: u64,
    pub saved_overlap_bases: u64,
    pub orientation_boundary_segments: u64,
    pub repeated_orbit_boundaries: u64,
    pub accounted_peak_bytes: u64,
}

/// Complete deterministic report. It is not itself an authenticated capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessedPathSet {
    pub k: u8,
    pub source_equivalence: PathSourceEquivalence,
    pub source_root: [u8; 32],
    pub transition_root: [u8; 32],
    pub witnessed_child_root: [u8; 32],
    /// Zero for the explicitly unverified report path.
    pub witnessed_child_authentication_root: [u8; 32],
    pub path_set_root: [u8; 32],
    pub paths: Vec<WitnessedPath>,
    pub preserved_links: Vec<PreservedWitnessedLink>,
    pub conservation: WitnessedPathConservation,
}

/// Opaque source-backed witnessed path capability.
///
/// Serialized [`WitnessedPathSet`] fields cannot be promoted back into this
/// type. The only public constructor consumes an opaque witnessed child and
/// independently validates it against its compacted graph and transition
/// ledger.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Clone))]
pub struct AuthenticatedWitnessedPathSet {
    source_root: [u8; 32],
    graph_ancestry_root: [u8; 32],
    transition_root: [u8; 32],
    witnessed_child_authentication_root: [u8; 32],
    authentication_root: [u8; 32],
    path_set: WitnessedPathSet,
}

/// Checked reporting view over an opaque witnessed path capability.
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedWitnessedPathSetView<'a> {
    path_set: &'a WitnessedPathSet,
    graph_ancestry_root: [u8; 32],
    authentication_root: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
struct WitnessedSource<'a> {
    k: u8,
    source_root: [u8; 32],
    transition_root: [u8; 32],
    child_root: [u8; 32],
    child_authentication_root: [u8; 32],
    segments: &'a [WitnessedSegment],
    links: &'a [WitnessedLink],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SegmentIndex {
    id: WitnessedSegmentId,
    index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Handle {
    segment: usize,
    orientation: UnitigOrientation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Arc {
    from: Handle,
    to: Handle,
    source_link_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SegmentPathIndex {
    segment_id: WitnessedSegmentId,
    path_id: WitnessedPathId,
}

#[derive(Debug, Clone, Copy)]
struct AdmittedShape {
    segments: usize,
    links: usize,
    expanded_arc_capacity: usize,
    handles: usize,
    edge_steps: u64,
    input_bases: u64,
    accounted_peak_bytes: u64,
}

struct PathBuildContext<'source, 'ledger, 'borrow> {
    source: WitnessedSource<'source>,
    transitions: &'borrow TransitionLedgerView<'ledger>,
    segment_index: &'borrow [SegmentIndex],
    arcs: &'borrow [Arc],
    limits: WitnessedRecompactionLimits,
}

struct PathBuildState<'a> {
    selected_source_links: &'a mut [bool],
    claimed: &'a mut [bool],
    support_scratch: &'a mut Vec<u64>,
    paths: &'a mut Vec<WitnessedPath>,
}

impl AuthenticatedWitnessedPathSet {
    /// Borrow the immutable report after checking its authentication root.
    pub fn view(&self) -> Result<AuthenticatedWitnessedPathSetView<'_>> {
        if self.path_set.source_equivalence != PathSourceEquivalence::AuthenticatedCapability
            || self.path_set.source_root != self.source_root
            || self.path_set.transition_root != self.transition_root
            || self.path_set.witnessed_child_authentication_root
                != self.witnessed_child_authentication_root
            || path_set_root(&self.path_set) != self.path_set.path_set_root
            || authenticated_path_set_root(
                self.source_root,
                self.graph_ancestry_root,
                self.transition_root,
                self.witnessed_child_authentication_root,
                self.path_set.path_set_root,
            ) != self.authentication_root
        {
            return integrity("authenticated witnessed path wrapper is inconsistent");
        }
        Ok(AuthenticatedWitnessedPathSetView {
            path_set: &self.path_set,
            graph_ancestry_root: self.graph_ancestry_root,
            authentication_root: self.authentication_root,
        })
    }

    /// Rebuild the witnessed child and the complete path set from opaque
    /// sources, then require byte-structural equality with this capability.
    pub fn validate_against_sources(
        &self,
        witnessed: &AuthenticatedWitnessedChild,
        compacted: &AuthenticatedCompactedGraph,
        transitions: &TransitionLedger,
        reconstruction_limits: ReconstructionLimits,
        limits: WitnessedRecompactionLimits,
    ) -> Result<()> {
        validate_limits(limits)?;
        witnessed.validate_against_sources(compacted, transitions, reconstruction_limits)?;
        let witnessed_view = witnessed.view();
        if self.source_root != compacted.source_root()
            || self.graph_ancestry_root != compacted.ancestry_root()
            || self.transition_root != transitions.transition_root()
            || self.witnessed_child_authentication_root != witnessed_view.authentication_root()
            || self.path_set.witnessed_child_root != witnessed_view.child_root()
        {
            return integrity("authenticated witnessed path set has different source capabilities");
        }
        self.view()?;

        let required_scratch = checked_add(
            witnessed_view.conservation().accounted_peak_bytes,
            self.path_set.conservation.accounted_peak_bytes,
            "witnessed path verification scratch",
        )?;
        enforce_limit(
            required_scratch,
            limits.max_verification_scratch_bytes,
            ErrorCode::ResourceMemory,
            "witnessed path verification scratch",
        )?;

        let rebuilt_witnessed =
            constrain_authenticated_compacted_child(compacted, transitions, reconstruction_limits)?;
        if rebuilt_witnessed.view().authentication_root()
            != self.witnessed_child_authentication_root
        {
            return integrity("independent witnessed-child replay changed authentication root");
        }
        let transition_view = transitions.view()?;
        let expected = build_path_set(
            source_from_authenticated(rebuilt_witnessed.view()),
            &transition_view,
            PathSourceEquivalence::AuthenticatedCapability,
            limits,
        )?;
        if expected != self.path_set {
            return integrity("witnessed path set differs from independent source replay");
        }
        Ok(())
    }
}

impl AuthenticatedWitnessedPathSetView<'_> {
    pub const fn k(&self) -> u8 {
        self.path_set.k
    }

    pub const fn source_root(&self) -> [u8; 32] {
        self.path_set.source_root
    }

    pub const fn transition_root(&self) -> [u8; 32] {
        self.path_set.transition_root
    }

    pub const fn witnessed_child_root(&self) -> [u8; 32] {
        self.path_set.witnessed_child_root
    }

    pub const fn witnessed_child_authentication_root(&self) -> [u8; 32] {
        self.path_set.witnessed_child_authentication_root
    }

    pub const fn graph_ancestry_root(&self) -> [u8; 32] {
        self.graph_ancestry_root
    }

    pub const fn path_set_root(&self) -> [u8; 32] {
        self.path_set.path_set_root
    }

    pub const fn authentication_root(&self) -> [u8; 32] {
        self.authentication_root
    }

    pub fn paths(&self) -> &[WitnessedPath] {
        &self.path_set.paths
    }

    pub fn preserved_links(&self) -> &[PreservedWitnessedLink] {
        &self.path_set.preserved_links
    }

    pub const fn conservation(&self) -> WitnessedPathConservation {
        self.path_set.conservation
    }
}

/// Build a clearly unverified research report from copied child records.
///
/// Complete adjacency validation is still required, but this return type
/// cannot be used where an opaque source-backed capability is required.
pub fn recompact_unverified_witnessed_child(
    child: &WitnessedChild,
    transitions: &TransitionLedgerView<'_>,
    reconstruction_limits: ReconstructionLimits,
    limits: WitnessedRecompactionLimits,
) -> Result<WitnessedPathSet> {
    validate_no_unsupported_adjacencies(child, transitions, reconstruction_limits)?;
    build_path_set(
        WitnessedSource {
            k: child.k,
            source_root: child.source_binding.source_root(),
            transition_root: child.transition_root,
            child_root: child.child_root,
            child_authentication_root: [0; 32],
            segments: &child.segments,
            links: &child.links,
        },
        transitions,
        PathSourceEquivalence::UnverifiedReport,
        limits,
    )
}

/// Independently rebuild and compare a clearly unverified path report.
pub fn validate_unverified_witnessed_paths(
    paths: &WitnessedPathSet,
    child: &WitnessedChild,
    transitions: &TransitionLedgerView<'_>,
    reconstruction_limits: ReconstructionLimits,
    limits: WitnessedRecompactionLimits,
) -> Result<()> {
    if paths.source_equivalence != PathSourceEquivalence::UnverifiedReport
        || paths.witnessed_child_authentication_root != [0; 32]
    {
        return integrity("unverified validator received an authenticated provenance claim");
    }
    let expected =
        recompact_unverified_witnessed_child(child, transitions, reconstruction_limits, limits)?;
    if expected != *paths {
        return integrity("unverified witnessed path report differs from complete rebuild");
    }
    Ok(())
}

/// Build an opaque witnessed path capability and immediately exercise the
/// independent source-replay validator.
pub fn recompact_authenticated_witnessed_child(
    witnessed: &AuthenticatedWitnessedChild,
    compacted: &AuthenticatedCompactedGraph,
    transitions: &TransitionLedger,
    reconstruction_limits: ReconstructionLimits,
    limits: WitnessedRecompactionLimits,
) -> Result<AuthenticatedWitnessedPathSet> {
    witnessed.validate_against_sources(compacted, transitions, reconstruction_limits)?;
    let witnessed_view = witnessed.view();
    let transition_view = transitions.view()?;
    let path_set = build_path_set(
        source_from_authenticated(witnessed_view),
        &transition_view,
        PathSourceEquivalence::AuthenticatedCapability,
        limits,
    )?;
    let authentication_root = authenticated_path_set_root(
        compacted.source_root(),
        compacted.ancestry_root(),
        transitions.transition_root(),
        witnessed_view.authentication_root(),
        path_set.path_set_root,
    );
    let result = AuthenticatedWitnessedPathSet {
        source_root: compacted.source_root(),
        graph_ancestry_root: compacted.ancestry_root(),
        transition_root: transitions.transition_root(),
        witnessed_child_authentication_root: witnessed_view.authentication_root(),
        authentication_root,
        path_set,
    };
    result.validate_against_sources(
        witnessed,
        compacted,
        transitions,
        reconstruction_limits,
        limits,
    )?;
    Ok(result)
}

fn source_from_authenticated(child: AuthenticatedWitnessedChildView<'_>) -> WitnessedSource<'_> {
    WitnessedSource {
        k: child.k(),
        source_root: child.source_root(),
        transition_root: child.transition_root(),
        child_root: child.child_root(),
        child_authentication_root: child.authentication_root(),
        segments: child.segments(),
        links: child.links(),
    }
}

fn build_path_set(
    source: WitnessedSource<'_>,
    transitions: &TransitionLedgerView<'_>,
    source_equivalence: PathSourceEquivalence,
    limits: WitnessedRecompactionLimits,
) -> Result<WitnessedPathSet> {
    if source.k != transitions.k()
        || source.transition_root != transitions.transition_root()
        || source.source_root != transitions.source_root()
    {
        return integrity("witnessed child and transition ledger ancestry disagree");
    }
    if source_equivalence == PathSourceEquivalence::UnverifiedReport
        && source.child_authentication_root != [0; 32]
    {
        return integrity("unverified recompaction cannot carry an authentication root");
    }
    let shape = admit_shape(source, limits)?;
    let segment_index = build_segment_index(source.segments)?;
    let arcs = build_arcs(source, &segment_index, shape.expanded_arc_capacity)?;
    enforce_limit(
        u64_from_usize(arcs.len(), "expanded witnessed arcs")?,
        limits.max_expanded_arcs,
        ErrorCode::ResourceRetainedKeys,
        "expanded witnessed arcs",
    )?;

    let mut in_degree = zeroed_vec(shape.handles, "witnessed handle indegrees")?;
    let mut out_degree = zeroed_vec(shape.handles, "witnessed handle outdegrees")?;
    for arc in &arcs {
        let from = handle_offset(arc.from, shape.segments)?;
        let to = handle_offset(arc.to, shape.segments)?;
        out_degree[from] = out_degree[from]
            .checked_add(1)
            .ok_or_else(|| overflow("witnessed handle outdegree"))?;
        in_degree[to] = in_degree[to]
            .checked_add(1)
            .ok_or_else(|| overflow("witnessed handle indegree"))?;
    }

    let mut orientation_boundary = zeroed_vec(shape.segments, "orientation boundaries")?;
    for (index, segment) in source.segments.iter().enumerate() {
        orientation_boundary[index] = segment_orientation_is_ambiguous(segment)?;
    }
    let mut next = none_vec(shape.handles, "witnessed continuation successors")?;
    let mut previous = none_vec(shape.handles, "witnessed continuation predecessors")?;
    for (arc_index, arc) in arcs.iter().enumerate() {
        let from = handle_offset(arc.from, shape.segments)?;
        let to = handle_offset(arc.to, shape.segments)?;
        if out_degree[from] == 1
            && in_degree[to] == 1
            && arc.from.segment != arc.to.segment
            && !orientation_boundary[arc.from.segment]
            && !orientation_boundary[arc.to.segment]
        {
            if next[from].replace(arc_index).is_some()
                || previous[to].replace(arc_index).is_some()
            {
                return invariant("unique continuation arrays received a duplicate arc");
            }
        }
    }

    let mut claimed = zeroed_vec(shape.segments, "claimed witnessed segment orbits")?;
    let mut visit_epoch = zeroed_vec(shape.segments, "path-local visit epochs")?;
    let mut epoch = 0_u64;
    let mut selected_source_links = zeroed_vec(shape.links, "selected source links")?;
    let mut paths = try_vec(shape.segments, "witnessed recompacted paths")?;
    let mut members = try_vec(shape.segments, "path member scratch")?;
    let mut support_scratch = try_vec(
        usize_from_u64(shape.edge_steps, "path support scratch")?,
        "path support scratch",
    )?;
    let mut repeated_orbit_boundaries = 0_u64;

    for handle_index in 0..shape.handles {
        let start = handle_from_offset(handle_index, shape.segments)?;
        if claimed[start.segment]
            || previous[handle_index].is_some()
            || next[handle_index].is_none()
        {
            continue;
        }
        epoch = next_epoch(epoch, &mut visit_epoch)?;
        let repeated = collect_path_members(
            start,
            &arcs,
            &next,
            &claimed,
            &mut visit_epoch,
            epoch,
            &mut members,
        )?;
        repeated_orbit_boundaries = checked_add(
            repeated_orbit_boundaries,
            u64::from(repeated),
            "repeated orbit boundaries",
        )?;
        emit_path(
            WitnessedPathTopology::Linear,
            &members,
            &PathBuildContext {
                source,
                transitions,
                segment_index: &segment_index,
                arcs: &arcs,
                limits,
            },
            PathBuildState {
                selected_source_links: &mut selected_source_links,
                claimed: &mut claimed,
                support_scratch: &mut support_scratch,
                paths: &mut paths,
            },
        )?;
    }

    for handle_index in 0..shape.handles {
        let start = handle_from_offset(handle_index, shape.segments)?;
        if claimed[start.segment] || next[handle_index].is_none() {
            continue;
        }
        epoch = next_epoch(epoch, &mut visit_epoch)?;
        let (closed, repeated) = collect_residual_cycle(
            start,
            &arcs,
            &next,
            &claimed,
            &mut visit_epoch,
            epoch,
            &mut members,
        )?;
        repeated_orbit_boundaries = checked_add(
            repeated_orbit_boundaries,
            u64::from(repeated),
            "repeated orbit boundaries",
        )?;
        emit_path(
            if closed {
                WitnessedPathTopology::ClosedWalkCandidate
            } else {
                WitnessedPathTopology::Linear
            },
            &members,
            &PathBuildContext {
                source,
                transitions,
                segment_index: &segment_index,
                arcs: &arcs,
                limits,
            },
            PathBuildState {
                selected_source_links: &mut selected_source_links,
                claimed: &mut claimed,
                support_scratch: &mut support_scratch,
                paths: &mut paths,
            },
        )?;
    }

    for segment in 0..shape.segments {
        if claimed[segment] {
            continue;
        }
        members.clear();
        members.push(Handle {
            segment,
            orientation: UnitigOrientation::Forward,
        });
        emit_path(
            WitnessedPathTopology::Linear,
            &members,
            &PathBuildContext {
                source,
                transitions,
                segment_index: &segment_index,
                arcs: &arcs,
                limits,
            },
            PathBuildState {
                selected_source_links: &mut selected_source_links,
                claimed: &mut claimed,
                support_scratch: &mut support_scratch,
                paths: &mut paths,
            },
        )?;
    }

    paths.sort_unstable_by_key(|path| path.id);
    if paths.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return integrity("witnessed recompacted path identifiers are not unique");
    }
    enforce_limit(
        u64_from_usize(paths.len(), "witnessed recompacted paths")?,
        limits.max_output_paths,
        ErrorCode::ResourceRetainedKeys,
        "witnessed recompacted paths",
    )?;

    let path_index = build_segment_path_index(&paths, shape.segments)?;
    let mut preserved_links = try_vec(shape.links, "preserved witnessed links")?;
    for (source_link_index, link) in source.links.iter().copied().enumerate() {
        if !selected_source_links[source_link_index] {
            preserved_links.push(PreservedWitnessedLink {
                source_link_index: u64_from_usize(source_link_index, "source link index")?,
                from_path: find_path_for_segment(&path_index, link.from)?,
                to_path: find_path_for_segment(&path_index, link.to)?,
                link,
            });
        }
    }
    preserved_links.sort_unstable();

    let selected_links = selected_source_links.iter().filter(|selected| **selected).count();
    enforce_limit(
        u64_from_usize(selected_links, "selected witnessed links")?,
        limits.max_selected_links,
        ErrorCode::ResourceRetainedKeys,
        "selected witnessed links",
    )?;
    let output_bases = paths.iter().try_fold(0_u64, |total, path| {
        checked_add(
            total,
            u64_from_usize(path.sequence.len(), "recompacted path bases")?,
            "recompacted output bases",
        )
    })?;
    enforce_limit(
        output_bases,
        limits.max_output_bases,
        ErrorCode::ResourceOutputBytes,
        "recompacted output bases",
    )?;
    let represented_segments = paths.iter().try_fold(0_u64, |total, path| {
        checked_add(
            total,
            u64_from_usize(path.members.len(), "recompacted path members")?,
            "represented witnessed segments",
        )
    })?;
    let represented_edges = paths.iter().try_fold(0_u64, |total, path| {
        checked_add(total, path.canonical_edge_steps, "represented exact edges")
    })?;
    let closed_walk_candidates = paths
        .iter()
        .filter(|path| path.topology == WitnessedPathTopology::ClosedWalkCandidate)
        .count();
    let spelled_joins = paths.iter().try_fold(0_u64, |total, path| {
        checked_add(
            total,
            u64_from_usize(path.members.len().saturating_sub(1), "spelled path joins")?,
            "spelled path joins",
        )
    })?;
    let saved_overlap_bases = checked_mul(
        spelled_joins,
        u64::from(source.k - 1),
        "saved witnessed overlaps",
    )?;
    if checked_add(output_bases, saved_overlap_bases, "path base conservation")?
        != shape.input_bases
        || represented_segments != u64_from_usize(shape.segments, "input segments")?
        || represented_edges != shape.edge_steps
        || selected_links + preserved_links.len() != shape.links
    {
        return invariant("witnessed recompaction conservation failed");
    }
    let conservation = WitnessedPathConservation {
        input_segments: u64_from_usize(shape.segments, "input segments")?,
        represented_segments,
        input_links: u64_from_usize(shape.links, "input links")?,
        selected_links: u64_from_usize(selected_links, "selected links")?,
        preserved_links: u64_from_usize(preserved_links.len(), "preserved links")?,
        output_paths: u64_from_usize(paths.len(), "output paths")?,
        linear_paths: u64_from_usize(paths.len() - closed_walk_candidates, "linear paths")?,
        closed_walk_candidates: u64_from_usize(
            closed_walk_candidates,
            "closed-walk candidates",
        )?,
        canonical_edge_steps: shape.edge_steps,
        represented_edge_steps: represented_edges,
        input_segment_bases: shape.input_bases,
        output_path_bases: output_bases,
        spelled_joins,
        saved_overlap_bases,
        orientation_boundary_segments: u64_from_usize(
            orientation_boundary.iter().filter(|boundary| **boundary).count(),
            "orientation-boundary segments",
        )?,
        repeated_orbit_boundaries,
        accounted_peak_bytes: shape.accounted_peak_bytes,
    };
    let mut result = WitnessedPathSet {
        k: source.k,
        source_equivalence,
        source_root: source.source_root,
        transition_root: source.transition_root,
        witnessed_child_root: source.child_root,
        witnessed_child_authentication_root: source.child_authentication_root,
        path_set_root: [0; 32],
        paths,
        preserved_links,
        conservation,
    };
    result.path_set_root = path_set_root(&result);
    Ok(result)
}

fn admit_shape(
    source: WitnessedSource<'_>,
    limits: WitnessedRecompactionLimits,
) -> Result<AdmittedShape> {
    validate_limits(limits)?;
    if !(3..=126).contains(&source.k) {
        return integrity("witnessed recompaction k is outside 3..=126");
    }
    let segments = source.segments.len();
    let links = source.links.len();
    enforce_limit(
        u64_from_usize(segments, "witnessed input segments")?,
        limits.max_input_segments,
        ErrorCode::ResourceRetainedKeys,
        "witnessed input segments",
    )?;
    enforce_limit(
        u64_from_usize(links, "witnessed input links")?,
        limits.max_input_links,
        ErrorCode::ResourceRetainedKeys,
        "witnessed input links",
    )?;
    let expanded_arc_capacity = links
        .checked_mul(2)
        .ok_or_else(|| overflow("expanded witnessed arc capacity"))?;
    enforce_limit(
        u64_from_usize(expanded_arc_capacity, "expanded witnessed arc capacity")?,
        limits.max_expanded_arcs,
        ErrorCode::ResourceRetainedKeys,
        "expanded witnessed arc capacity",
    )?;
    let handles = segments
        .checked_mul(2)
        .ok_or_else(|| overflow("oriented witnessed handle capacity"))?;
    let (edge_steps, input_bases) = source.segments.iter().try_fold(
        (0_u64, 0_u64),
        |(edges, bases), segment| {
            Ok((
                checked_add(
                    edges,
                    u64_from_usize(segment.steps.len(), "witnessed exact-edge steps")?,
                    "witnessed exact-edge steps",
                )?,
                checked_add(
                    bases,
                    u64_from_usize(segment.sequence.len(), "witnessed segment bases")?,
                    "witnessed segment bases",
                )?,
            ))
        },
    )?;
    enforce_limit(
        input_bases,
        limits.max_output_bases,
        ErrorCode::ResourceOutputBytes,
        "witnessed recompaction input-base bound",
    )?;
    let accounted_peak_bytes = projected_accounted_bytes(
        segments,
        links,
        expanded_arc_capacity,
        handles,
        edge_steps,
        input_bases,
    )?;
    enforce_limit(
        accounted_peak_bytes,
        limits.max_accounted_bytes,
        ErrorCode::ResourceMemory,
        "witnessed recompaction accounted bytes",
    )?;
    Ok(AdmittedShape {
        segments,
        links,
        expanded_arc_capacity,
        handles,
        edge_steps,
        input_bases,
        accounted_peak_bytes,
    })
}

fn projected_accounted_bytes(
    segments: usize,
    links: usize,
    expanded_arcs: usize,
    handles: usize,
    edge_steps: u64,
    input_bases: u64,
) -> Result<u64> {
    checked_sum(&[
        checked_bytes(segments, size_of::<SegmentIndex>(), "segment ID index")?,
        checked_bytes(expanded_arcs, size_of::<Arc>(), "expanded arcs")?,
        checked_bytes(handles, size_of::<usize>(), "handle indegrees")?,
        checked_bytes(handles, size_of::<usize>(), "handle outdegrees")?,
        checked_bytes(
            handles,
            size_of::<Option<usize>>(),
            "handle successors",
        )?,
        checked_bytes(
            handles,
            size_of::<Option<usize>>(),
            "handle predecessors",
        )?,
        checked_bytes(segments, size_of::<bool>(), "orientation boundaries")?,
        checked_bytes(segments, size_of::<bool>(), "claimed segment orbits")?,
        checked_bytes(segments, size_of::<u64>(), "path visit epochs")?,
        checked_bytes(links, size_of::<bool>(), "selected source links")?,
        checked_bytes(segments, size_of::<WitnessedPath>(), "path headers")?,
        checked_bytes(
            segments,
            size_of::<OrientedWitnessedSegment>(),
            "path member payload",
        )?,
        checked_bytes(
            segments,
            size_of::<WitnessedPathJoin>(),
            "path join payload",
        )?,
        input_bases,
        checked_mul(
            edge_steps,
            size_of::<u64>() as u64,
            "path support scratch",
        )?,
        checked_bytes(segments, size_of::<Handle>(), "path handle scratch")?,
        checked_mul(
            checked_bytes(
                segments,
                size_of::<OrientedWitnessedSegment>(),
                "canonical path scratch",
            )?,
            3,
            "canonical path scratch",
        )?,
        checked_bytes(
            segments,
            size_of::<SegmentPathIndex>(),
            "segment-to-path index",
        )?,
        checked_bytes(
            links,
            size_of::<PreservedWitnessedLink>(),
            "preserved link payload",
        )?,
        ACCOUNTING_MARGIN_BYTES,
    ])
}

fn validate_limits(limits: WitnessedRecompactionLimits) -> Result<()> {
    if limits.max_input_segments == 0
        || limits.max_input_links == 0
        || limits.max_expanded_arcs == 0
        || limits.max_output_paths == 0
        || limits.max_selected_links == 0
        || limits.max_output_bases == 0
        || limits.max_accounted_bytes == 0
        || limits.max_verification_scratch_bytes == 0
    {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "witnessed recompaction limits must all be greater than zero",
        ));
    }
    Ok(())
}

fn build_segment_index(segments: &[WitnessedSegment]) -> Result<Vec<SegmentIndex>> {
    let mut index = try_vec(segments.len(), "witnessed segment ID index")?;
    for (position, segment) in segments.iter().enumerate() {
        index.push(SegmentIndex {
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

fn segment_position(index: &[SegmentIndex], id: WitnessedSegmentId) -> Result<usize> {
    index
        .binary_search_by_key(&id, |row| row.id)
        .ok()
        .and_then(|position| index.get(position))
        .map(|row| row.index)
        .ok_or_else(|| integrity_error("witnessed link references a missing segment"))
}

fn build_arcs(
    source: WitnessedSource<'_>,
    segment_index: &[SegmentIndex],
    capacity: usize,
) -> Result<Vec<Arc>> {
    let mut arcs = try_vec(capacity, "expanded witnessed arcs")?;
    for (source_link_index, link) in source.links.iter().enumerate() {
        let forward = Arc {
            from: Handle {
                segment: segment_position(segment_index, link.from)?,
                orientation: link.from_orientation,
            },
            to: Handle {
                segment: segment_position(segment_index, link.to)?,
                orientation: link.to_orientation,
            },
            source_link_index,
        };
        let reverse = Arc {
            from: reverse_handle(forward.to),
            to: reverse_handle(forward.from),
            source_link_index,
        };
        arcs.push(forward);
        arcs.push(reverse);
    }
    arcs.sort_unstable();
    arcs.dedup();
    Ok(arcs)
}

fn reverse_handle(handle: Handle) -> Handle {
    Handle {
        segment: handle.segment,
        orientation: reverse_orientation(handle.orientation),
    }
}

fn reverse_orientation(orientation: UnitigOrientation) -> UnitigOrientation {
    match orientation {
        UnitigOrientation::Forward => UnitigOrientation::ReverseComplement,
        UnitigOrientation::ReverseComplement => UnitigOrientation::Forward,
    }
}

fn handle_offset(handle: Handle, segments: usize) -> Result<usize> {
    if handle.segment >= segments {
        return integrity("oriented handle references a missing segment");
    }
    handle
        .segment
        .checked_mul(2)
        .and_then(|value| {
            value.checked_add(match handle.orientation {
                UnitigOrientation::Forward => 0,
                UnitigOrientation::ReverseComplement => 1,
            })
        })
        .ok_or_else(|| overflow("oriented handle offset"))
}

fn handle_from_offset(offset: usize, segments: usize) -> Result<Handle> {
    if offset >= segments.saturating_mul(2) {
        return invariant("oriented handle offset exceeds admitted shape");
    }
    Ok(Handle {
        segment: offset / 2,
        orientation: if offset & 1 == 0 {
            UnitigOrientation::Forward
        } else {
            UnitigOrientation::ReverseComplement
        },
    })
}

fn segment_orientation_is_ambiguous(segment: &WitnessedSegment) -> Result<bool> {
    if segment.topology != CompactedTopology::Linear
        || segment
            .steps
            .iter()
            .any(|step| step.orientation == EdgeOrientation::SelfReverseComplement)
    {
        return Ok(true);
    }
    for index in 0..segment.sequence.len() {
        if segment.sequence[index]
            != complement(segment.sequence[segment.sequence.len() - index - 1])?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn collect_path_members(
    start: Handle,
    arcs: &[Arc],
    next: &[Option<usize>],
    claimed: &[bool],
    visit_epoch: &mut [u64],
    epoch: u64,
    members: &mut Vec<Handle>,
) -> Result<bool> {
    members.clear();
    let segments = claimed.len();
    let mut current = start;
    loop {
        if claimed[current.segment] || visit_epoch[current.segment] == epoch {
            return Ok(true);
        }
        visit_epoch[current.segment] = epoch;
        members.push(current);
        let offset = handle_offset(current, segments)?;
        let Some(arc_index) = next[offset] else {
            return Ok(false);
        };
        let target = arcs
            .get(arc_index)
            .ok_or_else(|| invariant_error("continuation references a missing arc"))?
            .to;
        if claimed[target.segment] || visit_epoch[target.segment] == epoch {
            return Ok(true);
        }
        current = target;
    }
}

fn collect_residual_cycle(
    start: Handle,
    arcs: &[Arc],
    next: &[Option<usize>],
    claimed: &[bool],
    visit_epoch: &mut [u64],
    epoch: u64,
    members: &mut Vec<Handle>,
) -> Result<(bool, bool)> {
    members.clear();
    let segments = claimed.len();
    let mut current = start;
    loop {
        if claimed[current.segment] || visit_epoch[current.segment] == epoch {
            return Ok((false, true));
        }
        visit_epoch[current.segment] = epoch;
        members.push(current);
        let offset = handle_offset(current, segments)?;
        let Some(arc_index) = next[offset] else {
            return Ok((false, false));
        };
        let target = arcs
            .get(arc_index)
            .ok_or_else(|| invariant_error("cycle continuation references a missing arc"))?
            .to;
        if target == start {
            return Ok((true, false));
        }
        if claimed[target.segment] || visit_epoch[target.segment] == epoch {
            return Ok((false, true));
        }
        current = target;
    }
}

fn next_epoch(epoch: u64, visit_epoch: &mut [u64]) -> Result<u64> {
    match epoch.checked_add(1) {
        Some(next) => Ok(next),
        None => {
            visit_epoch.fill(0);
            Ok(1)
        }
    }
}

fn emit_path(
    topology: WitnessedPathTopology,
    handles: &[Handle],
    context: &PathBuildContext<'_, '_, '_>,
    state: PathBuildState<'_>,
) -> Result<()> {
    if handles.is_empty() {
        return invariant("witnessed recompaction attempted to emit an empty path");
    }
    let mut members = try_vec(handles.len(), "canonical path members")?;
    for handle in handles {
        let segment = context
            .source
            .segments
            .get(handle.segment)
            .ok_or_else(|| invariant_error("path handle references a missing segment"))?;
        members.push(OrientedWitnessedSegment {
            segment_id: segment.id,
            orientation: handle.orientation,
        });
    }
    members = canonicalize_members(members, topology)?;

    for member in &members {
        let segment = segment_position(context.segment_index, member.segment_id)?;
        if state.claimed[segment] {
            return invariant("one witnessed segment orbit appears in multiple paths");
        }
        state.claimed[segment] = true;
    }

    let mut joins = try_vec(members.len(), "witnessed path joins")?;
    for pair in members.windows(2) {
        joins.push(path_join(pair[0], pair[1], context)?);
    }
    if topology == WitnessedPathTopology::ClosedWalkCandidate {
        let first = *members
            .first()
            .ok_or_else(|| invariant_error("closed path lacks its first member"))?;
        let last = *members
            .last()
            .ok_or_else(|| invariant_error("closed path lacks its last member"))?;
        joins.push(path_join(last, first, context)?);
    }
    for join in &joins {
        let index = usize_from_u64(join.source_link_index, "selected source link index")?;
        let selected = state
            .selected_source_links
            .get_mut(index)
            .ok_or_else(|| invariant_error("path join references a missing source link"))?;
        if std::mem::replace(selected, true) {
            return invariant("one canonical witnessed link was selected more than once");
        }
    }

    let sequence = spell_members(&members, context)?;
    let mut evidence_digest = Sha256::new();
    evidence_digest.update(PATH_EVIDENCE_DOMAIN);
    evidence_digest.update(context.source.child_root);
    let transition_count = members.iter().try_fold(0_u64, |total, member| {
        let segment = source_segment(context, member.segment_id)?;
        checked_add(
            total,
            u64_from_usize(
                segment.sequence.len().saturating_sub(usize::from(context.source.k)),
                "segment-internal transition rows",
            )?,
            "path transition rows",
        )
    })?;
    let transition_count = checked_add(
        transition_count,
        u64_from_usize(joins.len(), "path join rows")?,
        "path transition rows",
    )?;
    evidence_digest.update(transition_count.to_le_bytes());

    state.support_scratch.clear();
    let mut total_edge_support = 0_u64;
    let mut transition_summary = TransitionSummary::default();
    for member in &members {
        let segment = source_segment(context, member.segment_id)?;
        for step in &segment.steps {
            state.support_scratch.push(step.support);
            total_edge_support = checked_add(
                total_edge_support,
                step.support,
                "recompacted path edge support",
            )?;
        }
        hash_member_transitions(
            segment,
            member.orientation,
            context,
            &mut evidence_digest,
            &mut transition_summary,
        )?;
    }
    for join in &joins {
        let row = context
            .transitions
            .find(join.canonical_qmer)
            .ok_or_else(|| integrity_error("selected path join lacks an exact transition row"))?;
        if join.evidence != row_evidence(row) {
            return integrity("selected path join evidence differs from the exact transition row");
        }
        hash_transition_row(&mut evidence_digest, row);
        transition_summary.add(row)?;
    }
    if transition_summary.rows != transition_count {
        return invariant("path transition summary did not conserve adjacency positions");
    }

    state.support_scratch.sort_unstable();
    let minimum_edge_support = *state
        .support_scratch
        .first()
        .ok_or_else(|| invariant_error("witnessed path has no exact-edge support"))?;
    let lower_median_edge_support = state.support_scratch[(state.support_scratch.len() - 1) / 2];
    let maximum_edge_support = *state
        .support_scratch
        .last()
        .ok_or_else(|| invariant_error("witnessed path has no maximum edge support"))?;
    let canonical_edge_steps = u64_from_usize(
        state.support_scratch.len(),
        "recompacted path exact-edge steps",
    )?;
    let ordered_transition_evidence_sha256 = evidence_digest.finalize().into();
    let id = witnessed_path_id(
        context.source.child_root,
        topology,
        &sequence,
        &members,
        &joins,
        ordered_transition_evidence_sha256,
    );
    state.paths.push(WitnessedPath {
        id,
        topology,
        sequence,
        members,
        joins,
        canonical_edge_steps,
        total_edge_support,
        minimum_edge_support,
        lower_median_edge_support,
        maximum_edge_support,
        transition_rows: transition_summary.rows,
        minimum_transition_occurrences: transition_summary.minimum_occurrences,
        minimum_transition_fragment_instances: transition_summary.minimum_fragments,
        sum_transition_occurrences: transition_summary.sum_occurrences,
        sum_transition_fragment_instances: transition_summary.sum_fragments,
        ordered_transition_evidence_sha256,
    });
    enforce_limit(
        u64_from_usize(state.paths.len(), "witnessed output paths")?,
        context.limits.max_output_paths,
        ErrorCode::ResourceRetainedKeys,
        "witnessed output paths",
    )
}

#[derive(Debug, Default)]
struct TransitionSummary {
    rows: u64,
    minimum_occurrences: Option<u64>,
    minimum_fragments: Option<u64>,
    sum_occurrences: u64,
    sum_fragments: u64,
}

impl TransitionSummary {
    fn add(&mut self, row: &TransitionRow) -> Result<()> {
        self.rows = checked_add(self.rows, 1, "path transition rows")?;
        self.minimum_occurrences = Some(
            self.minimum_occurrences
                .map_or(row.accepted_window_occurrences, |current| {
                    current.min(row.accepted_window_occurrences)
                }),
        );
        self.minimum_fragments = Some(
            self.minimum_fragments
                .map_or(row.distinct_supplied_fragment_instances, |current| {
                    current.min(row.distinct_supplied_fragment_instances)
                }),
        );
        self.sum_occurrences = checked_add(
            self.sum_occurrences,
            row.accepted_window_occurrences,
            "path transition occurrence sum",
        )?;
        self.sum_fragments = checked_add(
            self.sum_fragments,
            row.distinct_supplied_fragment_instances,
            "path transition fragment sum",
        )?;
        Ok(())
    }
}

fn canonicalize_members(
    members: Vec<OrientedWitnessedSegment>,
    topology: WitnessedPathTopology,
) -> Result<Vec<OrientedWitnessedSegment>> {
    let reverse = reverse_members(&members)?;
    match topology {
        WitnessedPathTopology::Linear => {
            if reverse < members {
                Ok(reverse)
            } else {
                Ok(members)
            }
        }
        WitnessedPathTopology::ClosedWalkCandidate => {
            let forward = rotate_members(&members, minimum_rotation(&members)?)?;
            let reverse = rotate_members(&reverse, minimum_rotation(&reverse)?)?;
            if reverse < forward {
                Ok(reverse)
            } else {
                Ok(forward)
            }
        }
    }
}

fn reverse_members(
    members: &[OrientedWitnessedSegment],
) -> Result<Vec<OrientedWitnessedSegment>> {
    let mut reverse = try_vec(members.len(), "reverse path members")?;
    for member in members.iter().rev() {
        reverse.push(OrientedWitnessedSegment {
            segment_id: member.segment_id,
            orientation: reverse_orientation(member.orientation),
        });
    }
    Ok(reverse)
}

fn minimum_rotation(values: &[OrientedWitnessedSegment]) -> Result<usize> {
    if values.is_empty() {
        return invariant("cannot rotate an empty witnessed path");
    }
    let length = values.len();
    let mut left = 0_usize;
    let mut right = 1_usize;
    let mut offset = 0_usize;
    while left < length && right < length && offset < length {
        let left_value = values[(left + offset) % length];
        let right_value = values[(right + offset) % length];
        match left_value.cmp(&right_value) {
            Ordering::Equal => offset += 1,
            Ordering::Greater => {
                left = left
                    .checked_add(offset + 1)
                    .ok_or_else(|| overflow("minimum-rotation left offset"))?;
                if left == right {
                    left += 1;
                }
                offset = 0;
            }
            Ordering::Less => {
                right = right
                    .checked_add(offset + 1)
                    .ok_or_else(|| overflow("minimum-rotation right offset"))?;
                if left == right {
                    right += 1;
                }
                offset = 0;
            }
        }
    }
    Ok(left.min(right) % length)
}

fn rotate_members(
    members: &[OrientedWitnessedSegment],
    start: usize,
) -> Result<Vec<OrientedWitnessedSegment>> {
    if members.is_empty() || start >= members.len() {
        return invariant("canonical path rotation is outside its members");
    }
    let mut rotated = try_vec(members.len(), "rotated path members")?;
    rotated.extend_from_slice(&members[start..]);
    rotated.extend_from_slice(&members[..start]);
    Ok(rotated)
}

fn path_join(
    from: OrientedWitnessedSegment,
    to: OrientedWitnessedSegment,
    context: &PathBuildContext<'_, '_, '_>,
) -> Result<WitnessedPathJoin> {
    let from_position = segment_position(context.segment_index, from.segment_id)?;
    let to_position = segment_position(context.segment_index, to.segment_id)?;
    let key = (
        Handle {
            segment: from_position,
            orientation: from.orientation,
        },
        Handle {
            segment: to_position,
            orientation: to.orientation,
        },
    );
    let start = context
        .arcs
        .partition_point(|arc| (arc.from, arc.to) < key);
    let matching = context
        .arcs
        .get(start..)
        .unwrap_or_default()
        .iter()
        .take_while(|arc| (arc.from, arc.to) == key)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return integrity("selected path adjacency is not one unique witnessed arc");
    }
    let arc = matching[0];
    let source_link = *context
        .source
        .links
        .get(arc.source_link_index)
        .ok_or_else(|| invariant_error("expanded arc references a missing source link"))?;
    let decoded = boundary_qmer(
        context.source.k,
        source_segment(context, from.segment_id)?,
        from.orientation,
        source_segment(context, to.segment_id)?,
        to.orientation,
    )?;
    if decoded != source_link.canonical_qmer
        || source_link.overlap_bases != context.source.k - 1
        || source_link.evidence.kind != AdjacencyEvidenceKind::OriginalReadTransition
    {
        return integrity("selected path adjacency differs from its witnessed source link");
    }
    let row = context
        .transitions
        .find(decoded)
        .ok_or_else(|| integrity_error("selected path adjacency lacks an exact transition row"))?;
    if source_link.evidence != row_evidence(row) {
        return integrity("selected source link evidence differs from exact transition evidence");
    }
    Ok(WitnessedPathJoin {
        source_link_index: u64_from_usize(arc.source_link_index, "source link index")?,
        from: from.segment_id,
        from_orientation: from.orientation,
        to: to.segment_id,
        to_orientation: to.orientation,
        overlap_bases: source_link.overlap_bases,
        canonical_qmer: decoded,
        evidence: source_link.evidence,
    })
}

fn source_segment<'a>(
    context: &PathBuildContext<'a, '_, '_>,
    id: WitnessedSegmentId,
) -> Result<&'a WitnessedSegment> {
    let position = segment_position(context.segment_index, id)?;
    context
        .source
        .segments
        .get(position)
        .ok_or_else(|| invariant_error("segment index references a missing source segment"))
}

fn spell_members(
    members: &[OrientedWitnessedSegment],
    context: &PathBuildContext<'_, '_, '_>,
) -> Result<Vec<u8>> {
    let overlap = usize::from(context.source.k - 1);
    let output_length = members.iter().try_fold(0_usize, |total, member| {
        let length = source_segment(context, member.segment_id)?.sequence.len();
        total
            .checked_add(length)
            .ok_or_else(|| overflow("witnessed path spelling length"))
    })?;
    let output_length = output_length
        .checked_sub(
            members
                .len()
                .saturating_sub(1)
                .checked_mul(overlap)
                .ok_or_else(|| overflow("witnessed path overlap length"))?,
        )
        .ok_or_else(|| invariant_error("witnessed path overlap exceeds member bases"))?;
    enforce_limit(
        u64_from_usize(output_length, "witnessed path spelling length")?,
        context.limits.max_output_bases,
        ErrorCode::ResourceOutputBytes,
        "one witnessed path spelling",
    )?;
    let mut sequence = try_vec(output_length, "witnessed path sequence")?;
    for (member_index, member) in members.iter().enumerate() {
        let segment = source_segment(context, member.segment_id)?;
        if member_index > 0 {
            for offset in 0..overlap {
                let left = *sequence
                    .get(sequence.len() - overlap + offset)
                    .ok_or_else(|| invariant_error("path prefix overlap is unavailable"))?;
                let right = oriented_base(&segment.sequence, member.orientation, offset)?;
                if left != right {
                    return integrity("selected witnessed link lacks its declared exact overlap");
                }
            }
        }
        for offset in if member_index == 0 { 0 } else { overlap }..segment.sequence.len() {
            sequence.push(oriented_base(&segment.sequence, member.orientation, offset)?);
        }
    }
    if sequence.len() != output_length {
        return invariant("witnessed path spelling length changed during construction");
    }
    Ok(sequence)
}

fn hash_member_transitions(
    segment: &WitnessedSegment,
    orientation: UnitigOrientation,
    context: &PathBuildContext<'_, '_, '_>,
    digest: &mut Sha256,
    summary: &mut TransitionSummary,
) -> Result<()> {
    let q = usize::from(context.source.k) + 1;
    for start in 0..=segment.sequence.len().saturating_sub(q) {
        let decoded = oriented_window_qmer(&segment.sequence, orientation, start, q)?;
        let row = context
            .transitions
            .find(decoded)
            .ok_or_else(|| integrity_error("path member contains an unwitnessed adjacency"))?;
        hash_transition_row(digest, row);
        summary.add(row)?;
    }
    Ok(())
}

fn boundary_qmer(
    k: u8,
    from: &WitnessedSegment,
    from_orientation: UnitigOrientation,
    to: &WitnessedSegment,
    to_orientation: UnitigOrientation,
) -> Result<PackedKmer> {
    let k = usize::from(k);
    if from.sequence.len() < k || to.sequence.len() < k {
        return integrity("witnessed link endpoint is shorter than k");
    }
    let mut bytes = [0_u8; 127];
    for offset in 0..k {
        bytes[offset] = oriented_base(
            &from.sequence,
            from_orientation,
            from.sequence.len() - k + offset,
        )?;
    }
    for offset in 0..k - 1 {
        if bytes[offset + 1] != oriented_base(&to.sequence, to_orientation, offset)? {
            return integrity("witnessed link endpoints lack a k - 1 overlap");
        }
    }
    bytes[k] = oriented_base(&to.sequence, to_orientation, k - 1)?;
    canonical_window(&bytes[..=k])
}

fn oriented_window_qmer(
    sequence: &[u8],
    orientation: UnitigOrientation,
    start: usize,
    length: usize,
) -> Result<PackedKmer> {
    if length > 127 || start.saturating_add(length) > sequence.len() {
        return integrity("oriented transition window is outside its segment");
    }
    let mut bytes = [0_u8; 127];
    for (offset, slot) in bytes[..length].iter_mut().enumerate() {
        *slot = oriented_base(sequence, orientation, start + offset)?;
    }
    canonical_window(&bytes[..length])
}

fn canonical_window(window: &[u8]) -> Result<PackedKmer> {
    let length = u8::try_from(window.len())
        .map_err(|_| overflow("transition window length does not fit u8"))?;
    canonical_code(encode_exact_bases(window)?, length)
}

fn oriented_base(
    sequence: &[u8],
    orientation: UnitigOrientation,
    index: usize,
) -> Result<u8> {
    let base = match orientation {
        UnitigOrientation::Forward => *sequence
            .get(index)
            .ok_or_else(|| integrity_error("forward path offset is outside its segment"))?,
        UnitigOrientation::ReverseComplement => {
            let source = sequence
                .len()
                .checked_sub(index + 1)
                .ok_or_else(|| integrity_error("reverse path offset is outside its segment"))?;
            complement(*sequence.get(source).ok_or_else(|| {
                integrity_error("reverse path source offset is outside its segment")
            })?)?
        }
    };
    if matches!(base, b'A' | b'C' | b'G' | b'T') {
        Ok(base)
    } else {
        integrity("witnessed path segment contains a non-ACGT byte")
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

fn row_evidence(row: &TransitionRow) -> OriginalReadTransitionEvidence {
    OriginalReadTransitionEvidence {
        kind: AdjacencyEvidenceKind::OriginalReadTransition,
        accepted_window_occurrences: row.accepted_window_occurrences,
        distinct_supplied_fragment_instances: row.distinct_supplied_fragment_instances,
        sorted_event_frames_sha256: row.sorted_event_frames_sha256,
    }
}
