//! Exact deterministic compaction of externally reduced wide k-mer counts.
//!
//! This experimental component consumes the verified materialized rows from
//! ExternalPartitionResult. Full packed strings are the only graph identities;
//! routing minimizers and buckets remain provenance. The stable CLI does not
//! call this module.

use super::external_reduce::ExternalPartitionResult;
use super::external_run::WideRunSupportUnit;
use super::partitioned_dbg::{route_minimizer, select_minimizer};
use super::retention::{RetainedCountArtifact, RetentionRule, SourceEquivalence};
use super::wide_kmer::{
    canonical_code, decode_mer, prefix_code, reverse_complement_code, suffix_code, terminal_base,
    validate_code, validate_k, PackedKmer,
};
use crate::error::{ErrorCode, Result, VeritasmError};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::mem::size_of;

const OWNED_ALLOCATION_MARGIN_BYTES: u64 = 16_384;
const NESTED_VECTOR_ALLOWANCE_BYTES: u64 = 64;
const AUTHENTICATED_GRAPH_DOMAIN: &[u8] = b"veritasm:authenticated-compacted-graph:v1\0";
const AUTHENTICATED_GRAPH_OPERATIONAL_DOMAIN: &[u8] =
    b"veritasm:authenticated-compacted-graph-operational:v1\0";
const COMPACTED_RESULT_DOMAIN: &[u8] = b"veritasm:complete-compacted-graph:v2\0";

/// Explicit resource bounds for one experimental compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactedGraphLimits {
    pub max_canonical_edges: u64,
    pub max_oriented_handles: u64,
    pub max_literal_nodes: u64,
    pub max_unitigs: u64,
    /// Bounds link candidates before reverse-complement deduplication.
    pub max_link_candidates: u64,
    pub max_output_bases: u64,
    /// Conservative owned-payload admission, not a process RSS promise.
    pub max_accounted_bytes: u64,
}

/// Orientation of one literal view of a canonical exact k-mer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum EdgeOrientation {
    /// The canonical packed spelling.
    Canonical = 0,
    /// The reverse complement of a non-fixed canonical spelling.
    ReverseComplement = 1,
    /// One orientation-fixed spelling equal to its reverse complement.
    SelfReverseComplement = 2,
}

impl EdgeOrientation {
    fn reverse(self) -> Self {
        match self {
            Self::Canonical => Self::ReverseComplement,
            Self::ReverseComplement => Self::Canonical,
            Self::SelfReverseComplement => Self::SelfReverseComplement,
        }
    }
}

/// Orientation of an emitted unitig sequence in one graph relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum UnitigOrientation {
    Forward = 0,
    ReverseComplement = 1,
}

impl UnitigOrientation {
    fn reverse(self) -> Self {
        match self {
            Self::Forward => Self::ReverseComplement,
            Self::ReverseComplement => Self::Forward,
        }
    }
}

/// Whether a compacted record is linear or a closed one-in/one-out walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum CompactedTopology {
    Linear = 0,
    ClosedWalk = 1,
}

/// Exact identifier derived from k, topology, and emitted sequence bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompactedUnitigId(pub [u8; 32]);

/// One oriented canonical-edge step in an emitted unitig.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactedEdgeStep {
    pub key: PackedKmer,
    pub orientation: EdgeOrientation,
    pub support: u64,
}

/// One exact compacted record with complete edge-level provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedUnitig {
    pub id: CompactedUnitigId,
    pub topology: CompactedTopology,
    pub sequence: Vec<u8>,
    pub steps: Vec<CompactedEdgeStep>,
    pub total_support: u64,
    pub minimum_support: u64,
    pub lower_median_support: u64,
    pub maximum_support: u64,
    /// V2 digest over source identity, typed support unit, and ordered steps.
    pub provenance_sha256: [u8; 32],
}

/// One exact overlap relation, canonicalized with its reverse complement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompactedLink {
    pub from: CompactedUnitigId,
    pub from_orientation: UnitigOrientation,
    pub to: CompactedUnitigId,
    pub to_orientation: UnitigOrientation,
    pub overlap_bases: u8,
}

/// Conservation and topology evidence for the compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactedGraphStats {
    pub canonical_edges: u64,
    pub represented_canonical_edges: u64,
    pub input_support: u64,
    pub represented_support: u64,
    pub oriented_handles: u64,
    pub literal_nodes: u64,
    pub boundary_nodes: u64,
    pub degree_boundary_nodes: u64,
    pub self_reverse_complement_nodes: u64,
    pub self_reverse_complement_edges: u64,
    pub raw_walks: u64,
    pub linear_unitigs: u64,
    pub closed_unitigs: u64,
    pub directed_boundary_transitions: u64,
    pub link_candidates: u64,
    pub canonical_links: u64,
    pub output_bases: u64,
    pub accounted_peak_bytes: u64,
}

/// Complete isolated result. Record vectors have deterministic total orders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedGraphResult {
    pub k: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub source_identity: [u8; 32],
    pub support_unit: WideRunSupportUnit,
    pub exact_edge_table_sha256: [u8; 32],
    pub unitigs: Vec<CompactedUnitig>,
    pub links: Vec<CompactedLink>,
    pub stats: CompactedGraphStats,
}

/// Opaque retained-count-to-compacted-graph capability.
///
/// It can only be minted by [`compact_retained_counts`].  The complete graph
/// remains privately owned; callers receive a checked shared view and cannot
/// promote a modified [`CompactedGraphResult`] back into this type.
///
/// ```compile_fail
/// use veritasm::experimental::compacted_dbg::AuthenticatedCompactedGraph;
/// fn remove_link(graph: &mut AuthenticatedCompactedGraph) {
///     graph.graph.links.clear();
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Clone))]
pub struct AuthenticatedCompactedGraph {
    source_equivalence: SourceEquivalence,
    retention_rule: RetentionRule,
    retention_root: [u8; 32],
    retained_table_root: [u8; 32],
    retention_operational_root: [u8; 32],
    retained_key_count: u64,
    retained_support: u64,
    graph_root: [u8; 32],
    ancestry_root: [u8; 32],
    operational_root: [u8; 32],
    limits: CompactedGraphLimits,
    graph: CompactedGraphResult,
}

impl AuthenticatedCompactedGraph {
    pub const fn source_root(&self) -> [u8; 32] {
        self.source_equivalence.common_source_root()
    }

    pub const fn source_equivalence_root(&self) -> [u8; 32] {
        self.source_equivalence.root()
    }

    pub const fn retention_rule(&self) -> RetentionRule {
        self.retention_rule
    }

    pub const fn retention_root(&self) -> [u8; 32] {
        self.retention_root
    }

    pub const fn retained_table_root(&self) -> [u8; 32] {
        self.retained_table_root
    }

    pub const fn graph_root(&self) -> [u8; 32] {
        self.graph_root
    }

    pub const fn ancestry_root(&self) -> [u8; 32] {
        self.ancestry_root
    }

    pub const fn operational_root(&self) -> [u8; 32] {
        self.operational_root
    }

    pub const fn k(&self) -> u8 {
        self.graph.k
    }

    pub const fn support_unit(&self) -> WideRunSupportUnit {
        self.graph.support_unit
    }

    pub const fn retained_key_count(&self) -> u64 {
        self.retained_key_count
    }

    pub const fn retained_support(&self) -> u64 {
        self.retained_support
    }

    /// Validate the complete private preimage before lending the graph.
    pub fn checked_graph(&self) -> Result<&CompactedGraphResult> {
        self.validate_invariants()?;
        Ok(&self.graph)
    }

    fn validate_invariants(&self) -> Result<()> {
        if self.graph.source_identity != self.retention_root
            || self.graph.stats.canonical_edges != self.retained_key_count
            || self.graph.stats.represented_canonical_edges != self.retained_key_count
            || self.graph.stats.input_support != self.retained_support
            || self.graph.stats.represented_support != self.retained_support
            || compacted_result_root(&self.graph) != self.graph_root
        {
            return invariant("authenticated compacted graph lost retained-table equivalence");
        }
        let ancestry = authenticated_graph_root(
            self.source_equivalence,
            self.retention_rule,
            self.retention_root,
            self.retained_table_root,
            self.retained_key_count,
            self.retained_support,
            self.graph_root,
        );
        if ancestry != self.ancestry_root {
            return invariant("authenticated compacted graph ancestry root is inconsistent");
        }
        if authenticated_graph_operational_root(
            self.ancestry_root,
            self.retention_operational_root,
            self.limits,
            self.graph.stats.accounted_peak_bytes,
        ) != self.operational_root
        {
            return invariant("authenticated compacted graph operational root is inconsistent");
        }
        Ok(())
    }
}

/// Compact one opaque retained table without exposing a promotable mutable
/// adapter at the source-backed boundary.
pub fn compact_retained_counts(
    retained: &RetainedCountArtifact,
    limits: CompactedGraphLimits,
) -> Result<AuthenticatedCompactedGraph> {
    let input = retained.retained_counts()?;
    let graph = compact_external_counts(input, limits)?;
    let source_equivalence = retained.source_equivalence();
    let retention_rule = retained.rule();
    let retention_root = retained.retention_root();
    let retained_table_root = retained.retained_table_root();
    let retention_operational_root = retained.operational_root();
    let retained_key_count = retained.retained_key_count();
    let retained_support = retained.retained_support();
    let graph_root = compacted_result_root(&graph);
    let ancestry_root = authenticated_graph_root(
        source_equivalence,
        retention_rule,
        retention_root,
        retained_table_root,
        retained_key_count,
        retained_support,
        graph_root,
    );
    let operational_root = authenticated_graph_operational_root(
        ancestry_root,
        retention_operational_root,
        limits,
        graph.stats.accounted_peak_bytes,
    );
    let authenticated = AuthenticatedCompactedGraph {
        source_equivalence,
        retention_rule,
        retention_root,
        retained_table_root,
        retention_operational_root,
        retained_key_count,
        retained_support,
        graph_root,
        ancestry_root,
        operational_root,
        limits,
        graph,
    };
    authenticated.validate_invariants()?;
    Ok(authenticated)
}

#[derive(Debug, Clone, Copy)]
struct CanonicalEdge {
    key: PackedKmer,
    minimizer: PackedKmer,
    bucket_id: u32,
    support: u64,
}

#[derive(Debug, Clone, Copy)]
struct Handle {
    canonical_index: usize,
    orientation: EdgeOrientation,
    spelling: PackedKmer,
    source: PackedKmer,
    target: PackedKmer,
}

struct HandleBuild {
    handles: Vec<Handle>,
    mate_indices: Vec<usize>,
    endpoints: Vec<Endpoint>,
    self_reverse_edges: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum IncidenceDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Endpoint {
    node: PackedKmer,
    direction: IncidenceDirection,
    handle: usize,
}

#[derive(Debug, Clone, Copy)]
struct LiteralNode {
    code: PackedKmer,
    incoming: [usize; 4],
    incoming_len: u8,
    outgoing: [usize; 4],
    outgoing_len: u8,
    reverse_fixed: bool,
    incident_fixed_edge: bool,
}

impl LiteralNode {
    fn boundary(self) -> bool {
        self.incoming_len != 1
            || self.outgoing_len != 1
            || self.reverse_fixed
            || self.incident_fixed_edge
    }

    fn degree_boundary(self) -> bool {
        self.incoming_len != 1 || self.outgoing_len != 1
    }

    fn incoming(self) -> impl Iterator<Item = usize> {
        self.incoming
            .into_iter()
            .take(usize::from(self.incoming_len))
    }

    fn outgoing(self) -> impl Iterator<Item = usize> {
        self.outgoing
            .into_iter()
            .take(usize::from(self.outgoing_len))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawWalk {
    first: usize,
    len: usize,
    topology: CompactedTopology,
}

#[derive(Debug)]
struct OrbitPlan {
    representative: Vec<usize>,
    topology: CompactedTopology,
}

#[derive(Debug, Clone, Copy)]
struct RawMapping {
    unitig_index: usize,
    forward: bool,
    reverse: bool,
    orbit_representative: bool,
}

/// Construct exact compacted records from verified external exact counts.
///
/// The borrowed external result is not re-read from its temporary run paths.
/// This function validates every materialized row, recomputes its routing
/// owner, and binds the complete normalized table into
/// exact_edge_table_sha256. The caller-owned result is excluded from the
/// allocation estimate.
pub fn compact_external_counts(
    input: &ExternalPartitionResult,
    limits: CompactedGraphLimits,
) -> Result<CompactedGraphResult> {
    validate_scalar_configuration(input.k, input.minimizer_length, input.virtual_bucket_count)?;
    let edge_count = usize_from_u64(input.distinct_kmers, "canonical edge count")?;
    if input.edge_counts.len() != edge_count {
        return invariant("external distinct-kmer count differs from its materialized edge table");
    }
    enforce_limit(
        input.distinct_kmers,
        limits.max_canonical_edges,
        "experimental compacted canonical edges",
    )?;
    let base_admission = admit_base_phase(edge_count, input.k, limits)?;

    let mut edges = try_vec(edge_count, "canonical-edge table")?;
    let mut previous_route = None;
    let mut occurrence_total = 0_u64;
    for row in &input.edge_counts {
        if row.support == 0 {
            return invariant("external edge table contains zero exact support");
        }
        validate_code(row.key, input.k)?;
        let canonical = canonical_code(row.key, input.k)?;
        if canonical != row.key {
            return invariant("external edge table contains a noncanonical exact key");
        }
        let owner = select_minimizer(row.key, input.k, input.minimizer_length)?;
        let bucket = route_minimizer(
            owner.key,
            input.minimizer_length,
            input.virtual_bucket_count,
        )?;
        if row.minimizer != owner.key || row.bucket_id != bucket {
            return invariant(
                "external edge table contains an incorrect minimizer owner or bucket",
            );
        }
        let route_order = (row.bucket_id, row.key);
        if previous_route.is_some_and(|previous| previous >= route_order) {
            return invariant(
                "external edge table is not strictly ordered by bucket and exact key",
            );
        }
        previous_route = Some(route_order);
        occurrence_total = checked_add(
            occurrence_total,
            row.support,
            "external exact-support total overflow",
        )?;
        edges.push(CanonicalEdge {
            key: row.key,
            minimizer: row.minimizer,
            bucket_id: row.bucket_id,
            support: row.support,
        });
    }
    if occurrence_total != input.support_events {
        return invariant("external edge table does not conserve authenticated support events");
    }
    edges.sort_unstable_by_key(|edge| edge.key);
    if edges.windows(2).any(|pair| pair[0].key == pair[1].key) {
        return invariant("external edge table repeats a complete canonical key");
    }
    let exact_edge_table_sha256 = edge_table_digest(input, &edges);

    let handle_build = build_handles_and_endpoints(input.k, &edges, limits)?;
    let handles = handle_build.handles;
    let mate_indices = handle_build.mate_indices;
    let endpoints = handle_build.endpoints;
    let self_reverse_edges = handle_build.self_reverse_edges;
    let nodes = build_nodes(input.k, &handles, endpoints, limits)?;
    let (walks, walk_handles, handle_owners) = extract_raw_walks(&handles, &nodes)?;
    let walk_mates = derive_walk_mates(&walks, &walk_handles, &mate_indices, &handle_owners)?;
    let (orbits, mappings) = build_orbits(
        &walks,
        &walk_handles,
        &walk_mates,
        &mate_indices,
        edges.len(),
        limits,
    )?;
    let (output_bases, link_candidate_bound, directed_transitions) =
        exact_output_bounds(input.k, &orbits, &mappings, &handle_owners, &nodes)?;
    let accounted_peak_bytes = admit_output_phase(
        base_admission,
        edge_count,
        handles.len(),
        nodes.len(),
        orbits.len(),
        output_bases,
        link_candidate_bound,
        limits,
    )?;

    let mut unitigs = emit_unitigs(input, &edges, &handles, &orbits)?;
    let mut links = derive_links(
        input.k,
        &nodes,
        &walks,
        &handle_owners,
        &mappings,
        &unitigs,
        link_candidate_bound,
    )?;
    unitigs.sort_unstable_by_key(|unitig| unitig.id);
    if unitigs.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return invariant("experimental compacted unitig identifier collision");
    }
    links.sort_unstable();
    links.dedup();
    validate_link_overlaps(input.k, &unitigs, &links)?;
    validate_conservation(&edges, &unitigs)?;

    let boundary_nodes = nodes.iter().filter(|node| node.boundary()).count();
    let degree_boundary_nodes = nodes.iter().filter(|node| node.degree_boundary()).count();
    let reverse_fixed_nodes = nodes.iter().filter(|node| node.reverse_fixed).count();
    let linear_unitigs = unitigs
        .iter()
        .filter(|unitig| unitig.topology == CompactedTopology::Linear)
        .count();
    let closed_unitigs = unitigs.len().saturating_sub(linear_unitigs);
    Ok(CompactedGraphResult {
        k: input.k,
        minimizer_length: input.minimizer_length,
        virtual_bucket_count: input.virtual_bucket_count,
        source_identity: input.source_identity,
        support_unit: input.support_unit,
        exact_edge_table_sha256,
        stats: CompactedGraphStats {
            canonical_edges: u64_from_usize(edges.len(), "canonical edge count")?,
            represented_canonical_edges: u64_from_usize(edges.len(), "represented edge count")?,
            input_support: occurrence_total,
            represented_support: occurrence_total,
            oriented_handles: u64_from_usize(handles.len(), "oriented handle count")?,
            literal_nodes: u64_from_usize(nodes.len(), "literal node count")?,
            boundary_nodes: u64_from_usize(boundary_nodes, "boundary node count")?,
            degree_boundary_nodes: u64_from_usize(
                degree_boundary_nodes,
                "degree-boundary node count",
            )?,
            self_reverse_complement_nodes: u64_from_usize(
                reverse_fixed_nodes,
                "self-reverse-complement node count",
            )?,
            self_reverse_complement_edges: u64_from_usize(
                self_reverse_edges,
                "self-reverse-complement edge count",
            )?,
            raw_walks: u64_from_usize(walks.len(), "raw walk count")?,
            linear_unitigs: u64_from_usize(linear_unitigs, "linear unitig count")?,
            closed_unitigs: u64_from_usize(closed_unitigs, "closed unitig count")?,
            directed_boundary_transitions: directed_transitions,
            link_candidates: u64_from_usize(link_candidate_bound, "link candidate count")?,
            canonical_links: u64_from_usize(links.len(), "canonical link count")?,
            output_bases,
            accounted_peak_bytes,
        },
        unitigs,
        links,
    })
}

fn build_handles_and_endpoints(
    k: u8,
    edges: &[CanonicalEdge],
    limits: CompactedGraphLimits,
) -> Result<HandleBuild> {
    let mut handle_count = 0_usize;
    for edge in edges {
        let reverse = reverse_complement_code(edge.key, k)?;
        handle_count = handle_count
            .checked_add(if reverse == edge.key { 1 } else { 2 })
            .ok_or_else(|| overflow("oriented handle count overflow"))?;
    }
    enforce_limit(
        u64_from_usize(handle_count, "oriented handle count")?,
        limits.max_oriented_handles,
        "experimental compacted oriented handles",
    )?;
    let mut handles = try_vec(handle_count, "oriented handle table")?;
    let mut self_reverse = 0_usize;
    for (canonical_index, edge) in edges.iter().enumerate() {
        let reverse = reverse_complement_code(edge.key, k)?;
        handles.push(make_handle(
            canonical_index,
            EdgeOrientation::Canonical,
            edge.key,
            k,
        )?);
        if reverse == edge.key {
            self_reverse = self_reverse
                .checked_add(1)
                .ok_or_else(|| overflow("self-reverse-complement edge count overflow"))?;
            handles
                .last_mut()
                .ok_or_else(|| invariant_error("new oriented handle disappeared"))?
                .orientation = EdgeOrientation::SelfReverseComplement;
        } else {
            handles.push(make_handle(
                canonical_index,
                EdgeOrientation::ReverseComplement,
                reverse,
                k,
            )?);
        }
    }
    handles.sort_unstable_by(|left, right| {
        left.spelling
            .cmp(&right.spelling)
            .then_with(|| left.orientation.cmp(&right.orientation))
    });
    if handles
        .windows(2)
        .any(|pair| pair[0].spelling == pair[1].spelling)
    {
        return invariant("two oriented handles have the same complete literal spelling");
    }
    if handles.len() != handle_count {
        return invariant("oriented handle materialization differs from its admitted count");
    }

    let mut mates = try_vec(handles.len(), "oriented-handle mate table")?;
    for handle in &handles {
        let reverse = reverse_complement_code(handle.spelling, k)?;
        let index = handles
            .binary_search_by_key(&reverse, |candidate| candidate.spelling)
            .map_err(|_| invariant_error("oriented handle lacks its reverse-complement mate"))?;
        let mate = handles[index];
        if mate.canonical_index != handle.canonical_index
            || mate.orientation != handle.orientation.reverse()
            || mate.source != reverse_complement_code(handle.target, k - 1)?
            || mate.target != reverse_complement_code(handle.source, k - 1)?
        {
            return invariant("oriented-handle reverse-complement involution failed");
        }
        mates.push(index);
    }
    for (index, mate) in mates.iter().copied().enumerate() {
        if mates.get(mate).copied() != Some(index) {
            return invariant("oriented-handle mate table is not an involution");
        }
    }

    let endpoint_capacity = checked_mul_usize(handles.len(), 2, "endpoint count")?;
    let mut endpoints = try_vec(endpoint_capacity, "literal endpoint table")?;
    for (handle_index, handle) in handles.iter().enumerate() {
        endpoints.push(Endpoint {
            node: handle.source,
            direction: IncidenceDirection::Outgoing,
            handle: handle_index,
        });
        endpoints.push(Endpoint {
            node: handle.target,
            direction: IncidenceDirection::Incoming,
            handle: handle_index,
        });
    }
    endpoints.sort_unstable();
    Ok(HandleBuild {
        handles,
        mate_indices: mates,
        endpoints,
        self_reverse_edges: self_reverse,
    })
}

fn make_handle(
    canonical_index: usize,
    orientation: EdgeOrientation,
    spelling: PackedKmer,
    k: u8,
) -> Result<Handle> {
    Ok(Handle {
        canonical_index,
        orientation,
        spelling,
        source: prefix_code(spelling, k)?,
        target: suffix_code(spelling, k)?,
    })
}

fn build_nodes(
    k: u8,
    handles: &[Handle],
    endpoints: Vec<Endpoint>,
    limits: CompactedGraphLimits,
) -> Result<Vec<LiteralNode>> {
    let node_count = if endpoints.is_empty() {
        0
    } else {
        1_usize
            .checked_add(
                endpoints
                    .windows(2)
                    .filter(|pair| pair[0].node != pair[1].node)
                    .count(),
            )
            .ok_or_else(|| overflow("literal node count overflow"))?
    };
    enforce_limit(
        u64_from_usize(node_count, "literal node count")?,
        limits.max_literal_nodes,
        "experimental compacted literal nodes",
    )?;
    let mut nodes = try_vec(node_count, "literal node table")?;
    let mut cursor = 0_usize;
    while cursor < endpoints.len() {
        let code = endpoints[cursor].node;
        let mut node = LiteralNode {
            code,
            incoming: [0; 4],
            incoming_len: 0,
            outgoing: [0; 4],
            outgoing_len: 0,
            reverse_fixed: reverse_complement_code(code, k - 1)? == code,
            incident_fixed_edge: false,
        };
        while cursor < endpoints.len() && endpoints[cursor].node == code {
            let endpoint = endpoints[cursor];
            let handle = handles
                .get(endpoint.handle)
                .ok_or_else(|| invariant_error("literal endpoint references a missing handle"))?;
            node.incident_fixed_edge |=
                handle.orientation == EdgeOrientation::SelfReverseComplement;
            match endpoint.direction {
                IncidenceDirection::Incoming => {
                    let slot = usize::from(node.incoming_len);
                    if slot >= 4 {
                        return invariant("literal DNA node has more than four incoming edges");
                    }
                    node.incoming[slot] = endpoint.handle;
                    node.incoming_len += 1;
                }
                IncidenceDirection::Outgoing => {
                    let slot = usize::from(node.outgoing_len);
                    if slot >= 4 {
                        return invariant("literal DNA node has more than four outgoing edges");
                    }
                    node.outgoing[slot] = endpoint.handle;
                    node.outgoing_len += 1;
                }
            }
            cursor += 1;
        }
        nodes.push(node);
    }
    if nodes.len() != node_count {
        return invariant("literal node materialization differs from its admitted count");
    }
    Ok(nodes)
}

fn extract_raw_walks(
    handles: &[Handle],
    nodes: &[LiteralNode],
) -> Result<(Vec<RawWalk>, Vec<usize>, Vec<usize>)> {
    let mut used = try_vec(handles.len(), "oriented-handle visited table")?;
    used.resize(handles.len(), false);
    let mut walk_handles = try_vec(handles.len(), "flat raw-walk handle table")?;
    let mut walks = try_vec(handles.len(), "raw-walk range table")?;

    for node in nodes.iter().copied().filter(|node| node.boundary()) {
        for first in node.outgoing() {
            if used[first] {
                continue;
            }
            let start = walk_handles.len();
            let mut current = first;
            loop {
                if used[current] {
                    return invariant("boundary walk entered an already consumed oriented handle");
                }
                used[current] = true;
                walk_handles.push(current);
                let target = handles[current].target;
                let target_node = find_node(nodes, target)?;
                if target_node.boundary() {
                    break;
                }
                if target_node.incoming_len != 1 || target_node.outgoing_len != 1 {
                    return invariant("non-boundary literal node is not one-in/one-out");
                }
                current = target_node.outgoing[0];
            }
            walks.push(RawWalk {
                first: start,
                len: walk_handles.len() - start,
                topology: CompactedTopology::Linear,
            });
        }
    }

    for first in 0..handles.len() {
        if used[first] {
            continue;
        }
        let source_node = find_node(nodes, handles[first].source)?;
        if source_node.boundary() {
            return invariant("boundary-sourced handle survived boundary walk extraction");
        }
        let start = walk_handles.len();
        let mut current = first;
        loop {
            if used[current] {
                return invariant("residual cycle entered an already consumed handle");
            }
            used[current] = true;
            walk_handles.push(current);
            let target_node = find_node(nodes, handles[current].target)?;
            if target_node.boundary()
                || target_node.incoming_len != 1
                || target_node.outgoing_len != 1
            {
                return invariant("residual component is not a boundary-free one-in/one-out cycle");
            }
            let next = target_node.outgoing[0];
            if next == first {
                break;
            }
            current = next;
            if walk_handles.len() - start > handles.len() {
                return invariant("residual cycle exceeded the oriented handle count");
            }
        }
        walks.push(RawWalk {
            first: start,
            len: walk_handles.len() - start,
            topology: CompactedTopology::ClosedWalk,
        });
    }
    if used.iter().any(|used| !*used) || walk_handles.len() != handles.len() {
        return invariant("raw walks do not partition every oriented handle exactly once");
    }
    let mut owners = try_vec(handles.len(), "oriented-handle raw-walk ownership table")?;
    owners.resize(handles.len(), usize::MAX);
    for (walk_index, walk) in walks.iter().enumerate() {
        for &handle in walk_slice(*walk, &walk_handles)? {
            if owners[handle] != usize::MAX {
                return invariant("one oriented handle belongs to multiple raw walks");
            }
            owners[handle] = walk_index;
        }
    }
    if owners.contains(&usize::MAX) {
        return invariant("one or more oriented handles lack raw-walk ownership");
    }
    Ok((walks, walk_handles, owners))
}

fn derive_walk_mates(
    walks: &[RawWalk],
    flat: &[usize],
    handle_mates: &[usize],
    owners: &[usize],
) -> Result<Vec<usize>> {
    let mut result = try_vec(walks.len(), "raw-walk mate table")?;
    for walk in walks {
        let values = walk_slice(*walk, flat)?;
        let last = *values
            .last()
            .ok_or_else(|| invariant_error("raw walk is empty"))?;
        let mate_first = *handle_mates
            .get(last)
            .ok_or_else(|| invariant_error("raw walk references a missing handle mate"))?;
        let mate_index = *owners
            .get(mate_first)
            .ok_or_else(|| invariant_error("raw-walk mate has no owner"))?;
        let mate = *walks
            .get(mate_index)
            .ok_or_else(|| invariant_error("raw-walk mate index is out of range"))?;
        let mate_values = walk_slice(mate, flat)?;
        if walk.topology != mate.topology
            || values.len() != mate_values.len()
            || !reverse_walk_matches(values, mate_values, handle_mates, walk.topology)
        {
            return invariant("raw walk and reverse-complement mate do not agree");
        }
        result.push(mate_index);
    }
    for (index, mate) in result.iter().copied().enumerate() {
        if result.get(mate).copied() != Some(index) {
            return invariant("raw-walk mate relation is not an involution");
        }
    }
    Ok(result)
}

fn build_orbits(
    walks: &[RawWalk],
    flat: &[usize],
    walk_mates: &[usize],
    handle_mates: &[usize],
    canonical_edges: usize,
    limits: CompactedGraphLimits,
) -> Result<(Vec<OrbitPlan>, Vec<RawMapping>)> {
    let orbit_count = walk_mates
        .iter()
        .enumerate()
        .filter(|(index, mate)| *index <= **mate)
        .count();
    if orbit_count > canonical_edges {
        return invariant("unitig orbit count exceeds canonical exact-edge count");
    }
    enforce_limit(
        u64_from_usize(orbit_count, "unitig count")?,
        limits.max_unitigs,
        "experimental compacted unitigs",
    )?;
    let mut orbits = try_vec(orbit_count, "unitig orbit table")?;
    let mut mappings = try_vec(walks.len(), "raw-walk orientation mapping table")?;
    mappings.resize(
        walks.len(),
        RawMapping {
            unitig_index: usize::MAX,
            forward: false,
            reverse: false,
            orbit_representative: false,
        },
    );
    for (walk_index, walk) in walks.iter().copied().enumerate() {
        let mate = walk_mates[walk_index];
        if walk_index > mate {
            continue;
        }
        let values = walk_slice(walk, flat)?;
        let representative = match walk.topology {
            CompactedTopology::Linear => canonical_linear(values, handle_mates)?,
            CompactedTopology::ClosedWalk => canonical_cycle(values, handle_mates)?,
        };
        let unitig_index = orbits.len();
        orbits.push(OrbitPlan {
            representative,
            topology: walk.topology,
        });
        for raw_index in [walk_index, mate] {
            let raw = walk_slice(walks[raw_index], flat)?;
            let forward = match walk.topology {
                CompactedTopology::Linear => raw == orbits[unitig_index].representative,
                CompactedTopology::ClosedWalk => {
                    cycle_equal(raw, &orbits[unitig_index].representative)
                }
            };
            let reverse = reverse_walk_matches(
                &orbits[unitig_index].representative,
                raw,
                handle_mates,
                walk.topology,
            );
            if !forward && !reverse {
                return invariant("raw walk matches neither orientation of its unitig orbit");
            }
            mappings[raw_index] = RawMapping {
                unitig_index,
                forward,
                reverse,
                orbit_representative: raw_index == walk_index,
            };
        }
    }
    if mappings
        .iter()
        .any(|mapping| mapping.unitig_index == usize::MAX)
    {
        return invariant("one or more raw walks lack a unitig-orbit mapping");
    }
    if orbits.len() != orbit_count {
        return invariant("unitig orbit materialization differs from its admitted count");
    }
    Ok((orbits, mappings))
}

fn canonical_linear(values: &[usize], mates: &[usize]) -> Result<Vec<usize>> {
    let ordering = compare_forward_reverse(values, mates)?;
    let mut representative = try_vec(values.len(), "canonical linear walk")?;
    if ordering != Ordering::Greater {
        representative.extend_from_slice(values);
    } else {
        for &handle in values.iter().rev() {
            representative.push(mates[handle]);
        }
    }
    Ok(representative)
}

fn canonical_cycle(values: &[usize], mates: &[usize]) -> Result<Vec<usize>> {
    if values.is_empty() {
        return invariant("cannot canonicalize an empty cycle");
    }
    let forward_start = minimum_rotation(values.len(), |position| values[position]);
    let reverse_start = minimum_rotation(values.len(), |position| {
        mates[values[values.len() - 1 - position]]
    });
    let choose_reverse = compare_rotations(
        values.len(),
        |position| values[(forward_start + position) % values.len()],
        |position| {
            let logical = (reverse_start + position) % values.len();
            mates[values[values.len() - 1 - logical]]
        },
    ) == Ordering::Greater;
    let mut representative = try_vec(values.len(), "canonical closed walk")?;
    for position in 0..values.len() {
        let value = if choose_reverse {
            let logical = (reverse_start + position) % values.len();
            mates[values[values.len() - 1 - logical]]
        } else {
            values[(forward_start + position) % values.len()]
        };
        representative.push(value);
    }
    Ok(representative)
}

fn minimum_rotation<F>(length: usize, value: F) -> usize
where
    F: Fn(usize) -> usize,
{
    minimum_rotation_with_comparisons(length, value).0
}

fn minimum_rotation_with_comparisons<F>(length: usize, value: F) -> (usize, usize)
where
    F: Fn(usize) -> usize,
{
    if length <= 1 {
        return (0, 0);
    }
    let mut first = 0_usize;
    let mut second = 1_usize;
    let mut offset = 0_usize;
    let mut comparisons = 0_usize;
    while first < length && second < length && offset < length {
        comparisons = comparisons.saturating_add(1);
        let left = value((first + offset) % length);
        let right = value((second + offset) % length);
        match left.cmp(&right) {
            Ordering::Equal => offset += 1,
            Ordering::Greater => {
                first += offset + 1;
                if first == second {
                    first += 1;
                }
                offset = 0;
            }
            Ordering::Less => {
                second += offset + 1;
                if first == second {
                    second += 1;
                }
                offset = 0;
            }
        }
    }
    (first.min(second) % length, comparisons)
}

fn compare_forward_reverse(values: &[usize], mates: &[usize]) -> Result<Ordering> {
    for (position, &left) in values.iter().enumerate() {
        let source = values
            .len()
            .checked_sub(position + 1)
            .ok_or_else(|| invariant_error("reverse walk position underflow"))?;
        let right = mates[values[source]];
        let ordering = left.cmp(&right);
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn compare_rotations<L, R>(length: usize, left: L, right: R) -> Ordering
where
    L: Fn(usize) -> usize,
    R: Fn(usize) -> usize,
{
    for position in 0..length {
        let ordering = left(position).cmp(&right(position));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn reverse_walk_matches(
    values: &[usize],
    candidate: &[usize],
    mates: &[usize],
    topology: CompactedTopology,
) -> bool {
    if values.len() != candidate.len() {
        return false;
    }
    match topology {
        CompactedTopology::Linear => candidate
            .iter()
            .copied()
            .eq(values.iter().rev().map(|handle| mates[*handle])),
        CompactedTopology::ClosedWalk => {
            if candidate.is_empty() {
                return values.is_empty();
            }
            let candidate_start = minimum_rotation(candidate.len(), |position| candidate[position]);
            let reverse_start = minimum_rotation(values.len(), |position| {
                mates[values[values.len() - 1 - position]]
            });
            (0..candidate.len()).all(|position| {
                let candidate_value = candidate[(candidate_start + position) % candidate.len()];
                let logical = (reverse_start + position) % values.len();
                candidate_value == mates[values[values.len() - 1 - logical]]
            })
        }
    }
}

fn cycle_equal(left: &[usize], right: &[usize]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    if left.is_empty() {
        return true;
    }
    let left_start = minimum_rotation(left.len(), |position| left[position]);
    let right_start = minimum_rotation(right.len(), |position| right[position]);
    (0..left.len()).all(|position| {
        left[(left_start + position) % left.len()] == right[(right_start + position) % right.len()]
    })
}

fn exact_output_bounds(
    k: u8,
    orbits: &[OrbitPlan],
    mappings: &[RawMapping],
    handle_owners: &[usize],
    nodes: &[LiteralNode],
) -> Result<(u64, usize, u64)> {
    let mut output_bases = 0_u64;
    for orbit in orbits {
        let length = orbit
            .representative
            .len()
            .checked_add(usize::from(k - 1))
            .ok_or_else(|| overflow("unitig step count overflow"))?;
        output_bases = checked_add(
            output_bases,
            u64_from_usize(length, "unitig edge-step count")?,
            "unitig edge-step total overflow",
        )?;
    }

    let mut transitions = 0_u64;
    let mut candidates = 0_usize;
    for node in nodes.iter().copied().filter(|node| node.boundary()) {
        let directed = usize::from(node.incoming_len)
            .checked_mul(usize::from(node.outgoing_len))
            .ok_or_else(|| overflow("boundary transition count overflow"))?;
        transitions = checked_add(
            transitions,
            u64_from_usize(directed, "directed boundary transitions")?,
            "directed boundary transition total overflow",
        )?;
        for incoming in node.incoming() {
            for outgoing in node.outgoing() {
                let left =
                    mappings
                        .get(*handle_owners.get(incoming).ok_or_else(|| {
                            invariant_error("incoming handle ownership is missing")
                        })?)
                        .ok_or_else(|| invariant_error("incoming raw-walk mapping is missing"))?;
                let right =
                    mappings
                        .get(*handle_owners.get(outgoing).ok_or_else(|| {
                            invariant_error("outgoing handle ownership is missing")
                        })?)
                        .ok_or_else(|| invariant_error("outgoing raw-walk mapping is missing"))?;
                candidates = candidates
                    .checked_add(orientation_count(*left) * orientation_count(*right))
                    .ok_or_else(|| overflow("link candidate count overflow"))?;
            }
        }
    }
    candidates = candidates
        .checked_add(
            orbits
                .iter()
                .filter(|orbit| orbit.topology == CompactedTopology::ClosedWalk)
                .count(),
        )
        .ok_or_else(|| overflow("closed-walk link candidate count overflow"))?;
    Ok((output_bases, candidates, transitions))
}

fn emit_unitigs(
    input: &ExternalPartitionResult,
    edges: &[CanonicalEdge],
    handles: &[Handle],
    orbits: &[OrbitPlan],
) -> Result<Vec<CompactedUnitig>> {
    let mut edge_owner = try_vec(edges.len(), "canonical-edge unitig ownership table")?;
    edge_owner.resize(edges.len(), false);
    let mut support_scratch = try_vec(edges.len(), "unitig support-order scratch")?;
    let mut unitigs = try_vec(orbits.len(), "compacted unitig table")?;
    for orbit in orbits {
        if orbit.representative.is_empty() {
            return invariant("unitig orbit has an empty representative");
        }
        let first_handle = handles
            .get(orbit.representative[0])
            .ok_or_else(|| invariant_error("unitig representative starts outside handle table"))?;
        let mut sequence = decode_mer(first_handle.spelling, input.k)?;
        let sequence_length = orbit
            .representative
            .len()
            .checked_add(usize::from(input.k - 1))
            .ok_or_else(|| overflow("unitig sequence length overflow"))?;
        sequence
            .try_reserve_exact(sequence_length.saturating_sub(sequence.len()))
            .map_err(|cause| {
                memory_error(format!(
                    "cannot reserve experimental unitig sequence: {cause}"
                ))
            })?;
        if sequence.capacity() > sequence_length {
            return Err(memory_error(format!(
                "allocator returned experimental unitig sequence capacity {} above admitted length {sequence_length}",
                sequence.capacity()
            )));
        }
        let mut steps = try_vec(orbit.representative.len(), "unitig exact-edge steps")?;
        support_scratch.clear();
        let mut total_occurrences = 0_u64;
        let mut previous_target = None;
        for &handle_index in &orbit.representative {
            let handle = handles.get(handle_index).ok_or_else(|| {
                invariant_error("unitig representative references missing handle")
            })?;
            if previous_target.is_some_and(|target| target != handle.source) {
                return invariant("adjacent unitig handles lack their literal k-1 overlap");
            }
            if !steps.is_empty() {
                sequence.push(terminal_base(handle.spelling, input.k)?);
            }
            previous_target = Some(handle.target);
            let edge = edges.get(handle.canonical_index).ok_or_else(|| {
                invariant_error("unitig handle references missing canonical edge")
            })?;
            if edge_owner[handle.canonical_index] {
                return invariant("one canonical edge is represented more than once");
            }
            edge_owner[handle.canonical_index] = true;
            support_scratch.push(edge.support);
            total_occurrences = checked_add(
                total_occurrences,
                edge.support,
                "unitig occurrence-support total overflow",
            )?;
            steps.push(CompactedEdgeStep {
                key: edge.key,
                orientation: handle.orientation,
                support: edge.support,
            });
        }
        if sequence.len() != sequence_length {
            return invariant(
                "unitig sequence length does not reconcile with its exact edge steps",
            );
        }
        if orbit.topology == CompactedTopology::ClosedWalk {
            let last = handles[*orbit
                .representative
                .last()
                .ok_or_else(|| invariant_error("closed representative is empty"))?];
            if last.target != first_handle.source {
                return invariant("closed unitig representative does not return to its first node");
            }
            let overlap = usize::from(input.k - 1);
            if sequence[..overlap] != sequence[sequence.len() - overlap..] {
                return invariant("closed unitig sequence lacks its repeated terminal context");
            }
        }
        support_scratch.sort_unstable();
        let minimum_support = support_scratch[0];
        let lower_median_support = support_scratch[(support_scratch.len() - 1) / 2];
        let maximum_support = support_scratch[support_scratch.len() - 1];
        let id = unitig_id(input.k, orbit.topology, &sequence);
        let provenance_sha256 = unitig_provenance_digest(
            input.source_identity,
            input.support_unit,
            input.k,
            orbit.topology,
            &steps,
        );
        unitigs.push(CompactedUnitig {
            id,
            topology: orbit.topology,
            sequence,
            steps,
            total_support: total_occurrences,
            minimum_support,
            lower_median_support,
            maximum_support,
            provenance_sha256,
        });
    }
    if edge_owner.iter().any(|owned| !*owned) {
        return invariant("one or more canonical edges lack a compacted unitig owner");
    }
    Ok(unitigs)
}

fn derive_links(
    k: u8,
    nodes: &[LiteralNode],
    walks: &[RawWalk],
    handle_owners: &[usize],
    mappings: &[RawMapping],
    unitigs: &[CompactedUnitig],
    candidate_bound: usize,
) -> Result<Vec<CompactedLink>> {
    let mut links = try_vec(candidate_bound, "compacted link candidate table")?;
    for node in nodes.iter().copied().filter(|node| node.boundary()) {
        for incoming in node.incoming() {
            for outgoing in node.outgoing() {
                let incoming_walk = *handle_owners
                    .get(incoming)
                    .ok_or_else(|| invariant_error("incoming boundary handle lacks an owner"))?;
                let outgoing_walk = *handle_owners
                    .get(outgoing)
                    .ok_or_else(|| invariant_error("outgoing boundary handle lacks an owner"))?;
                let from = *mappings
                    .get(incoming_walk)
                    .ok_or_else(|| invariant_error("incoming boundary walk lacks a mapping"))?;
                let to = *mappings
                    .get(outgoing_walk)
                    .ok_or_else(|| invariant_error("outgoing boundary walk lacks a mapping"))?;
                for from_orientation in mapping_orientations(from) {
                    for to_orientation in mapping_orientations(to) {
                        let candidate = CompactedLink {
                            from: unitigs[from.unitig_index].id,
                            from_orientation,
                            to: unitigs[to.unitig_index].id,
                            to_orientation,
                            overlap_bases: k - 1,
                        };
                        links.push(canonical_link(candidate));
                    }
                }
            }
        }
    }
    for (walk_index, walk) in walks.iter().enumerate() {
        if walk.topology != CompactedTopology::ClosedWalk {
            continue;
        }
        let mapping = mappings[walk_index];
        // Emit one candidate per reverse-complement orbit, not per raw cycle.
        if !mapping.orbit_representative {
            continue;
        }
        links.push(canonical_link(CompactedLink {
            from: unitigs[mapping.unitig_index].id,
            from_orientation: UnitigOrientation::Forward,
            to: unitigs[mapping.unitig_index].id,
            to_orientation: UnitigOrientation::Forward,
            overlap_bases: k - 1,
        }));
    }
    if links.len() != candidate_bound {
        return invariant("materialized link candidates differ from their admitted exact bound");
    }
    Ok(links)
}

fn mapping_orientations(mapping: RawMapping) -> impl Iterator<Item = UnitigOrientation> {
    [
        mapping.forward.then_some(UnitigOrientation::Forward),
        mapping
            .reverse
            .then_some(UnitigOrientation::ReverseComplement),
    ]
    .into_iter()
    .flatten()
}

fn orientation_count(mapping: RawMapping) -> usize {
    usize::from(mapping.forward) + usize::from(mapping.reverse)
}

fn canonical_link(link: CompactedLink) -> CompactedLink {
    let reverse = CompactedLink {
        from: link.to,
        from_orientation: link.to_orientation.reverse(),
        to: link.from,
        to_orientation: link.from_orientation.reverse(),
        overlap_bases: link.overlap_bases,
    };
    link.min(reverse)
}

fn validate_link_overlaps(
    k: u8,
    unitigs: &[CompactedUnitig],
    links: &[CompactedLink],
) -> Result<()> {
    let overlap = usize::from(k - 1);
    for link in links {
        let from = find_unitig(unitigs, link.from)?;
        let to = find_unitig(unitigs, link.to)?;
        for offset in 0..overlap {
            let from_base = oriented_base(
                &from.sequence,
                link.from_orientation,
                from.sequence.len() - overlap + offset,
            )?;
            let to_base = oriented_base(&to.sequence, link.to_orientation, offset)?;
            if from_base != to_base {
                return invariant("compacted graph link lacks its declared literal k-1 overlap");
            }
        }
    }
    Ok(())
}

fn find_unitig(unitigs: &[CompactedUnitig], id: CompactedUnitigId) -> Result<&CompactedUnitig> {
    unitigs
        .binary_search_by_key(&id, |unitig| unitig.id)
        .ok()
        .and_then(|index| unitigs.get(index))
        .ok_or_else(|| invariant_error("compacted link references a missing unitig"))
}

fn oriented_base(sequence: &[u8], orientation: UnitigOrientation, index: usize) -> Result<u8> {
    let base = match orientation {
        UnitigOrientation::Forward => *sequence
            .get(index)
            .ok_or_else(|| invariant_error("unitig oriented-base index is out of range"))?,
        UnitigOrientation::ReverseComplement => {
            let source = sequence
                .len()
                .checked_sub(index + 1)
                .ok_or_else(|| invariant_error("reverse unitig index is out of range"))?;
            complement(sequence[source])?
        }
    };
    if matches!(base, b'A' | b'C' | b'G' | b'T') {
        Ok(base)
    } else {
        invariant("emitted unitig contains a non-ACGT byte")
    }
}

fn complement(base: u8) -> Result<u8> {
    match base {
        b'A' => Ok(b'T'),
        b'C' => Ok(b'G'),
        b'G' => Ok(b'C'),
        b'T' => Ok(b'A'),
        _ => invariant("cannot complement a non-ACGT unitig base"),
    }
}

fn validate_conservation(edges: &[CanonicalEdge], unitigs: &[CompactedUnitig]) -> Result<()> {
    let represented = unitigs.iter().try_fold(0_usize, |total, unitig| {
        total
            .checked_add(unitig.steps.len())
            .ok_or_else(|| overflow("represented exact-edge count overflow"))
    })?;
    if represented != edges.len() {
        return invariant("compacted unitigs do not conserve canonical exact edges");
    }
    let input_support = edges.iter().try_fold(0_u64, |total, edge| {
        checked_add(total, edge.support, "input support overflow")
    })?;
    let represented_support = unitigs.iter().try_fold(0_u64, |total, unitig| {
        checked_add(
            total,
            unitig.total_support,
            "represented exact-support total overflow",
        )
    })?;
    if represented_support != input_support {
        return invariant("compacted unitigs do not conserve exact support");
    }
    Ok(())
}

fn admit_base_phase(canonical_edges: usize, k: u8, limits: CompactedGraphLimits) -> Result<u64> {
    let handles = checked_mul_usize(canonical_edges, 2, "admitted handle bound")?;
    let endpoints = checked_mul_usize(handles, 2, "admitted endpoint bound")?;
    let nested = checked_bytes(
        canonical_edges.min(handles),
        NESTED_VECTOR_ALLOWANCE_BYTES,
        "unitig representative nested allocations",
    )?;
    let scratch = checked_sum(
        &[
            checked_bytes(
                canonical_edges,
                size_of::<CanonicalEdge>() as u64,
                "canonical edges",
            )?,
            checked_bytes(handles, size_of::<Handle>() as u64, "oriented handles")?,
            checked_bytes(handles, size_of::<usize>() as u64, "handle mates")?,
            checked_bytes(endpoints, size_of::<Endpoint>() as u64, "literal endpoints")?,
            checked_bytes(endpoints, size_of::<LiteralNode>() as u64, "literal nodes")?,
            checked_bytes(handles, size_of::<bool>() as u64, "visited flags")?,
            checked_bytes(handles, size_of::<usize>() as u64, "flat raw walks")?,
            checked_bytes(handles, size_of::<RawWalk>() as u64, "raw-walk ranges")?,
            checked_bytes(handles, size_of::<usize>() as u64, "handle owners")?,
            checked_bytes(handles, size_of::<usize>() as u64, "walk mates")?,
            checked_bytes(handles, size_of::<RawMapping>() as u64, "raw mappings")?,
            checked_bytes(
                canonical_edges.min(handles),
                size_of::<OrbitPlan>() as u64,
                "unitig orbit records",
            )?,
            checked_bytes(handles, size_of::<usize>() as u64, "unitig representatives")?,
            checked_bytes(usize::from(k) * 2, 1, "packed graph operation scratch")?,
            nested,
            OWNED_ALLOCATION_MARGIN_BYTES,
        ],
        "experimental compaction base admission",
    )?;
    enforce_limit(
        scratch,
        limits.max_accounted_bytes,
        "experimental compaction accounted bytes",
    )?;
    Ok(scratch)
}

#[allow(clippy::too_many_arguments)]
fn admit_output_phase(
    base_bytes: u64,
    canonical_edges: usize,
    handles: usize,
    nodes: usize,
    unitigs: usize,
    output_bases: u64,
    link_candidates: usize,
    limits: CompactedGraphLimits,
) -> Result<u64> {
    enforce_limit(
        u64_from_usize(handles, "oriented handle count")?,
        limits.max_oriented_handles,
        "experimental compacted oriented handles",
    )?;
    enforce_limit(
        u64_from_usize(nodes, "literal node count")?,
        limits.max_literal_nodes,
        "experimental compacted literal nodes",
    )?;
    enforce_limit(
        u64_from_usize(unitigs, "unitig count")?,
        limits.max_unitigs,
        "experimental compacted unitigs",
    )?;
    enforce_limit(
        u64_from_usize(link_candidates, "link candidate count")?,
        limits.max_link_candidates,
        "experimental compacted link candidates",
    )?;
    enforce_limit(
        output_bases,
        limits.max_output_bases,
        "experimental compacted output bases",
    )?;
    let output_bases_usize = usize_from_u64(output_bases, "output base count")?;
    let output = checked_sum(
        &[
            checked_bytes(
                unitigs,
                size_of::<CompactedUnitig>() as u64,
                "unitig records",
            )?,
            checked_bytes(output_bases_usize, 1, "unitig sequence bytes")?,
            checked_bytes(
                canonical_edges,
                size_of::<CompactedEdgeStep>() as u64,
                "unitig exact-edge provenance",
            )?,
            checked_bytes(
                canonical_edges,
                size_of::<bool>() as u64,
                "edge ownership flags",
            )?,
            checked_bytes(
                canonical_edges,
                size_of::<u64>() as u64,
                "support-order scratch",
            )?,
            checked_bytes(
                link_candidates,
                size_of::<CompactedLink>() as u64,
                "link candidates",
            )?,
            checked_bytes(
                unitigs,
                NESTED_VECTOR_ALLOWANCE_BYTES * 2,
                "unitig nested allocations",
            )?,
        ],
        "experimental compaction output admission",
    )?;
    let peak = checked_add(
        base_bytes,
        output,
        "experimental compaction peak-byte overflow",
    )?;
    enforce_limit(
        peak,
        limits.max_accounted_bytes,
        "experimental compaction accounted bytes",
    )?;
    Ok(peak)
}

fn compacted_result_root(graph: &CompactedGraphResult) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(COMPACTED_RESULT_DOMAIN);
    digest.update([graph.k, graph.minimizer_length, graph.support_unit as u8]);
    digest.update(graph.virtual_bucket_count.to_le_bytes());
    digest.update(graph.source_identity);
    digest.update(graph.exact_edge_table_sha256);
    digest.update((graph.unitigs.len() as u64).to_le_bytes());
    for unitig in &graph.unitigs {
        digest.update(unitig.id.0);
        digest.update([unitig.topology as u8]);
        digest.update((unitig.sequence.len() as u64).to_le_bytes());
        digest.update(&unitig.sequence);
        digest.update((unitig.steps.len() as u64).to_le_bytes());
        for step in &unitig.steps {
            digest.update(step.key.to_be_bytes());
            digest.update([step.orientation as u8]);
            digest.update(step.support.to_le_bytes());
        }
        digest.update(unitig.total_support.to_le_bytes());
        digest.update(unitig.minimum_support.to_le_bytes());
        digest.update(unitig.lower_median_support.to_le_bytes());
        digest.update(unitig.maximum_support.to_le_bytes());
        digest.update(unitig.provenance_sha256);
    }
    digest.update((graph.links.len() as u64).to_le_bytes());
    for link in &graph.links {
        digest.update(link.from.0);
        digest.update([link.from_orientation as u8]);
        digest.update(link.to.0);
        digest.update([link.to_orientation as u8, link.overlap_bases]);
    }
    for value in [
        graph.stats.canonical_edges,
        graph.stats.represented_canonical_edges,
        graph.stats.input_support,
        graph.stats.represented_support,
        graph.stats.oriented_handles,
        graph.stats.literal_nodes,
        graph.stats.boundary_nodes,
        graph.stats.degree_boundary_nodes,
        graph.stats.self_reverse_complement_nodes,
        graph.stats.self_reverse_complement_edges,
        graph.stats.raw_walks,
        graph.stats.linear_unitigs,
        graph.stats.closed_unitigs,
        graph.stats.directed_boundary_transitions,
        graph.stats.link_candidates,
        graph.stats.canonical_links,
        graph.stats.output_bases,
    ] {
        digest.update(value.to_le_bytes());
    }
    digest.finalize().into()
}

fn authenticated_graph_root(
    source_equivalence: SourceEquivalence,
    retention_rule: RetentionRule,
    retention_root: [u8; 32],
    retained_table_root: [u8; 32],
    retained_key_count: u64,
    retained_support: u64,
    graph_root: [u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(AUTHENTICATED_GRAPH_DOMAIN);
    digest.update(source_equivalence.common_source_root());
    digest.update(source_equivalence.root());
    match retention_rule {
        RetentionRule::RetainAll => digest.update([0, 0, 0, 0, 0, 0, 0, 0, 0]),
        RetentionRule::InclusiveSupport { minimum_support } => {
            digest.update([1]);
            digest.update(minimum_support.to_le_bytes());
        }
    }
    digest.update(retention_root);
    digest.update(retained_table_root);
    digest.update(retained_key_count.to_le_bytes());
    digest.update(retained_support.to_le_bytes());
    digest.update(graph_root);
    digest.finalize().into()
}

fn authenticated_graph_operational_root(
    ancestry_root: [u8; 32],
    retention_operational_root: [u8; 32],
    limits: CompactedGraphLimits,
    accounted_peak_bytes: u64,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(AUTHENTICATED_GRAPH_OPERATIONAL_DOMAIN);
    digest.update(ancestry_root);
    digest.update(retention_operational_root);
    for value in [
        limits.max_canonical_edges,
        limits.max_oriented_handles,
        limits.max_literal_nodes,
        limits.max_unitigs,
        limits.max_link_candidates,
        limits.max_output_bases,
        limits.max_accounted_bytes,
        accounted_peak_bytes,
    ] {
        digest.update(value.to_le_bytes());
    }
    digest.finalize().into()
}

fn edge_table_digest(input: &ExternalPartitionResult, edges: &[CanonicalEdge]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:experimental-compacted-edge-table:v1\0");
    digest.update(input.source_identity);
    digest.update([input.k, input.minimizer_length]);
    digest.update(input.virtual_bucket_count.to_le_bytes());
    digest.update([input.support_unit as u8]);
    digest.update(input.support_events.to_le_bytes());
    digest.update((edges.len() as u64).to_le_bytes());
    for edge in edges {
        digest.update(edge.key.to_be_bytes());
        digest.update(edge.minimizer.to_be_bytes());
        digest.update(edge.bucket_id.to_le_bytes());
        digest.update(edge.support.to_le_bytes());
    }
    digest.finalize().into()
}

fn unitig_id(k: u8, topology: CompactedTopology, sequence: &[u8]) -> CompactedUnitigId {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:experimental-compacted-unitig:v1\0");
    digest.update([k, topology as u8]);
    digest.update((sequence.len() as u64).to_le_bytes());
    digest.update(sequence);
    CompactedUnitigId(digest.finalize().into())
}

fn unitig_provenance_digest(
    source_identity: [u8; 32],
    support_unit: WideRunSupportUnit,
    k: u8,
    topology: CompactedTopology,
    steps: &[CompactedEdgeStep],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:experimental-compacted-unitig-provenance:v2\0");
    digest.update(source_identity);
    digest.update([support_unit as u8, k, topology as u8]);
    digest.update((steps.len() as u64).to_le_bytes());
    for step in steps {
        digest.update(step.key.to_be_bytes());
        digest.update([step.orientation as u8]);
        digest.update(step.support.to_le_bytes());
    }
    digest.finalize().into()
}

fn validate_scalar_configuration(
    k: u8,
    minimizer_length: u8,
    virtual_bucket_count: u32,
) -> Result<()> {
    validate_k(k)?;
    if minimizer_length == 0 || minimizer_length > k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental compacted minimizer length must be in 1..={k}; received {minimizer_length}"
            ),
        ));
    }
    if virtual_bucket_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "experimental compacted graph requires at least one virtual bucket",
        ));
    }
    Ok(())
}

fn find_node(nodes: &[LiteralNode], code: PackedKmer) -> Result<LiteralNode> {
    nodes
        .binary_search_by_key(&code, |node| node.code)
        .ok()
        .and_then(|index| nodes.get(index).copied())
        .ok_or_else(|| invariant_error("oriented handle endpoint lacks a literal node"))
}

fn walk_slice(walk: RawWalk, flat: &[usize]) -> Result<&[usize]> {
    let end = walk
        .first
        .checked_add(walk.len)
        .ok_or_else(|| overflow("raw-walk range overflow"))?;
    flat.get(walk.first..end)
        .ok_or_else(|| invariant_error("raw-walk range is outside flat handle storage"))
}

fn try_vec<T>(capacity: usize, label: &'static str) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|cause| memory_error(format!("cannot reserve experimental {label}: {cause}")))?;
    if values.capacity() > capacity {
        return Err(memory_error(format!(
            "allocator returned experimental {label} capacity {} above admitted capacity {capacity}",
            values.capacity()
        )));
    }
    Ok(values)
}

fn checked_mul_usize(left: usize, right: usize, label: &'static str) -> Result<usize> {
    left.checked_mul(right)
        .ok_or_else(|| overflow(format!("{label} overflow")))
}

fn checked_bytes(count: usize, item: u64, label: &'static str) -> Result<u64> {
    u64_from_usize(count, label)?
        .checked_mul(item)
        .ok_or_else(|| overflow(format!("{label} byte count overflow")))
}

fn checked_sum(values: &[u64], label: &'static str) -> Result<u64> {
    values.iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(*value)
            .ok_or_else(|| overflow(format!("{label} overflow")))
    })
}

fn checked_add(left: u64, right: u64, label: &'static str) -> Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| overflow(label.to_owned()))
}

fn usize_from_u64(value: u64, label: &'static str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| memory_error(format!("experimental {label} {value} does not fit usize")))
}

fn u64_from_usize(value: usize, label: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| overflow(format!("experimental {label} does not fit u64")))
}

fn enforce_limit(observed: u64, limit: u64, label: &'static str) -> Result<()> {
    if observed <= limit {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("{label} {observed} exceeds configured limit {limit}"),
        ))
    }
}

fn memory_error(context: String) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn overflow(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context.into())
}

fn invariant<T>(context: impl Into<String>) -> Result<T> {
    Err(invariant_error(context))
}

fn invariant_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
    use crate::experimental::external_reduce::ExactSupportCount;
    use crate::experimental::external_reduce::ExternalPartitionLimits;
    use crate::experimental::retention::{
        authenticate_spool_external_counts, retain_spool_authenticated_counts,
        RetainedCountArtifact, RetentionLimits,
    };
    use crate::experimental::spool_external::SpoolExternalOptions;
    use crate::spool::create_spool;
    use proptest::prelude::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    fn generous_limits() -> CompactedGraphLimits {
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

    fn authenticated_retained_counts(fasta: &[u8]) -> RetainedCountArtifact {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("source.fasta");
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
        let options = SpoolExternalOptions {
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
        let retention_limits = RetentionLimits {
            max_raw_keys: 100_000,
            max_retained_keys: 100_000,
            max_accounted_bytes: 32 << 20,
        };
        let raw = authenticate_spool_external_counts(&spool, &options, retention_limits).unwrap();
        retain_spool_authenticated_counts(&raw, RetentionRule::RetainAll, retention_limits).unwrap()
    }

    fn authenticated_branch_retained_counts() -> RetainedCountArtifact {
        authenticated_retained_counts(b">left\nCAAC\n>right\nGAAG\n")
    }

    fn authenticated_branch_graph() -> AuthenticatedCompactedGraph {
        let retained = authenticated_branch_retained_counts();
        compact_retained_counts(&retained, generous_limits()).unwrap()
    }

    fn reverse(sequence: &[u8]) -> Vec<u8> {
        sequence
            .iter()
            .rev()
            .map(|base| match base {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => panic!("test sequence must be exact DNA"),
            })
            .collect()
    }

    fn result_from_sequences(k: u8, sequences: &[Vec<u8>]) -> ExternalPartitionResult {
        let mut counts = BTreeMap::<PackedKmer, u64>::new();
        for sequence in sequences {
            for window in sequence.windows(usize::from(k)) {
                let key = canonical_code(
                    super::super::wide_kmer::encode_exact_bases(window).unwrap(),
                    k,
                )
                .unwrap();
                *counts.entry(key).or_default() += 1;
            }
        }
        result_from_counts(k, counts)
    }

    fn result_from_edges(k: u8, sequences: &[&[u8]]) -> ExternalPartitionResult {
        let mut counts = BTreeMap::<PackedKmer, u64>::new();
        for sequence in sequences {
            assert_eq!(sequence.len(), usize::from(k));
            let key = canonical_code(
                super::super::wide_kmer::encode_exact_bases(sequence).unwrap(),
                k,
            )
            .unwrap();
            *counts.entry(key).or_default() += 1;
        }
        result_from_counts(k, counts)
    }

    fn result_from_counts(k: u8, counts: BTreeMap<PackedKmer, u64>) -> ExternalPartitionResult {
        let minimizer_length = (k - 1).min(5);
        let virtual_bucket_count = 7;
        let mut edge_counts = counts
            .into_iter()
            .map(|(key, support)| {
                let owner = select_minimizer(key, k, minimizer_length).unwrap();
                ExactSupportCount {
                    bucket_id: route_minimizer(owner.key, minimizer_length, virtual_bucket_count)
                        .unwrap(),
                    minimizer: owner.key,
                    key,
                    support,
                }
            })
            .collect::<Vec<_>>();
        edge_counts.sort_by_key(|row| (row.bucket_id, row.key));
        let support_events = edge_counts.iter().map(|row| row.support).sum();
        ExternalPartitionResult {
            k,
            minimizer_length,
            virtual_bucket_count,
            source_identity: [0x5a; 32],
            support_unit: WideRunSupportUnit::AcceptedWindowOccurrence,
            support_events,
            distinct_kmers: edge_counts.len() as u64,
            edge_counts,
            final_runs: Vec::new(),
            replacements: Vec::new(),
            run_files_created: 0,
            open_files_high_water: 0,
            temporary_bytes_final: 0,
            temporary_bytes_high_water: 0,
        }
    }

    fn normalized_unitigs(result: &CompactedGraphResult) -> Vec<(CompactedTopology, Vec<u8>)> {
        let mut rows = result
            .unitigs
            .iter()
            .map(|unitig| (unitig.topology, unitig.sequence.clone()))
            .collect::<Vec<_>>();
        rows.sort();
        rows
    }

    fn assert_literal_evidence(input: &ExternalPartitionResult, output: &CompactedGraphResult) {
        let support_by_key = input
            .edge_counts
            .iter()
            .map(|row| (row.key, row.support))
            .collect::<BTreeMap<_, _>>();
        let k = usize::from(input.k);
        let mut represented = BTreeMap::<PackedKmer, u64>::new();
        for unitig in &output.unitigs {
            assert_eq!(unitig.steps.len(), unitig.sequence.len() - k + 1);

            let mut id_digest = Sha256::new();
            id_digest.update(b"veritasm:experimental-compacted-unitig:v1\0");
            id_digest.update([input.k, unitig.topology as u8]);
            id_digest.update((unitig.sequence.len() as u64).to_le_bytes());
            id_digest.update(&unitig.sequence);
            assert_eq!(unitig.id, CompactedUnitigId(id_digest.finalize().into()));

            let mut provenance = Sha256::new();
            provenance.update(b"veritasm:experimental-compacted-unitig-provenance:v2\0");
            provenance.update(input.source_identity);
            provenance.update([input.support_unit as u8, input.k, unitig.topology as u8]);
            provenance.update((unitig.steps.len() as u64).to_le_bytes());
            for (position, step) in unitig.steps.iter().enumerate() {
                let window = &unitig.sequence[position..position + k];
                let literal = super::super::wide_kmer::encode_exact_bases(window).unwrap();
                let reverse_literal =
                    super::super::wide_kmer::encode_exact_bases(&reverse(window)).unwrap();
                let key = literal.min(reverse_literal);
                let orientation = if literal == reverse_literal {
                    EdgeOrientation::SelfReverseComplement
                } else if literal == key {
                    EdgeOrientation::Canonical
                } else {
                    EdgeOrientation::ReverseComplement
                };
                assert_eq!(step.key, key);
                assert_eq!(step.orientation, orientation);
                assert_eq!(step.support, support_by_key[&key]);
                assert!(represented.insert(key, step.support).is_none());

                provenance.update(step.key.to_be_bytes());
                provenance.update([step.orientation as u8]);
                provenance.update(step.support.to_le_bytes());
            }
            assert_eq!(
                unitig.provenance_sha256,
                <[u8; 32]>::from(provenance.finalize())
            );
        }
        assert_eq!(represented, support_by_key);
    }

    #[test]
    fn linear_path_preserves_exact_keys_support_and_provenance() {
        let input = result_from_sequences(5, &[b"AACCGGTT".to_vec()]);
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert_literal_evidence(&input, &output);
        assert_eq!(
            output.stats.canonical_edges,
            output.stats.represented_canonical_edges
        );
        assert_eq!(output.stats.input_support, output.stats.represented_support);
        assert_eq!(output.stats.input_support, 4);
        assert!(output.unitigs.iter().all(|unitig| {
            unitig.steps.len() as u64 <= output.stats.canonical_edges
                && unitig.total_support > 0
                && unitig.provenance_sha256 != [0; 32]
        }));
        assert_ne!(output.exact_edge_table_sha256, [0; 32]);
    }

    #[test]
    fn branch_fixture_preserves_all_literal_transitions_as_links() {
        let input = result_from_edges(3, &[b"AAA", b"AAC", b"AAG", b"CAA", b"GAA"]);
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert!(output.stats.degree_boundary_nodes > 0);
        assert!(output.stats.directed_boundary_transitions >= 4);
        assert!(!output.links.is_empty());
        assert!(output.links.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn authenticated_graph_rejects_omitted_topology_and_seal_mutations() {
        let graph = authenticated_branch_graph();
        let checked = graph.checked_graph().unwrap();
        assert!(!checked.links.is_empty());
        assert_eq!(checked.links.len() as u64, checked.stats.canonical_links);

        let mut omitted = graph.clone();
        omitted.graph.links.pop();
        omitted.graph.stats.canonical_links -= 1;
        assert_eq!(
            omitted.checked_graph().unwrap_err().code(),
            ErrorCode::InternalInvariant
        );

        let mutations: &[fn(&mut AuthenticatedCompactedGraph)] = &[
            |value| value.retention_root[0] ^= 1,
            |value| value.retained_table_root[0] ^= 1,
            |value| value.retention_operational_root[0] ^= 1,
            |value| value.retained_key_count += 1,
            |value| value.retained_support += 1,
            |value| value.graph_root[0] ^= 1,
            |value| value.ancestry_root[0] ^= 1,
            |value| value.operational_root[0] ^= 1,
            |value| value.limits.max_link_candidates += 1,
            |value| value.graph.exact_edge_table_sha256[0] ^= 1,
        ];
        for (index, mutate) in mutations.iter().enumerate() {
            let mut changed = graph.clone();
            mutate(&mut changed);
            assert!(
                changed.checked_graph().is_err(),
                "authenticated compacted-graph mutation {index} was accepted"
            );
        }

        let different_source = authenticated_retained_counts(b">different\nAAAACCCC\n");
        let mut changed = graph;
        changed.source_equivalence = different_source.source_equivalence();
        assert!(changed.checked_graph().is_err());
    }

    #[test]
    fn operational_limits_do_not_change_authenticated_scientific_identity() {
        let retained = authenticated_branch_retained_counts();
        let first_limits = generous_limits();
        let mut second_limits = first_limits;
        second_limits.max_accounted_bytes += 1;
        second_limits.max_link_candidates += 1;

        let first = compact_retained_counts(&retained, first_limits).unwrap();
        let second = compact_retained_counts(&retained, second_limits).unwrap();
        assert_eq!(
            first.checked_graph().unwrap(),
            second.checked_graph().unwrap()
        );
        assert_eq!(first.graph_root(), second.graph_root());
        assert_eq!(first.ancestry_root(), second.ancestry_root());
        assert_ne!(first.operational_root(), second.operational_root());
    }

    #[test]
    fn repeat_fixture_stops_at_ambiguous_topology_without_choosing_a_path() {
        let input = result_from_sequences(3, &[b"AAACAAAC".to_vec(), b"AAAGAAAG".to_vec()]);
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert!(output.stats.degree_boundary_nodes > 0);
        assert!(output.unitigs.len() > 1);
        assert_eq!(normalized_unitigs(&output), literal_oracle_unitigs(&input));
    }

    #[test]
    fn cycle_fixture_emits_closed_walk_and_exact_self_link() {
        let input = result_from_edges(3, &[b"ACA", b"CAC"]);
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert_eq!(output.stats.closed_unitigs, 1);
        let closed = output
            .unitigs
            .iter()
            .find(|unitig| unitig.topology == CompactedTopology::ClosedWalk)
            .unwrap();
        assert!(output
            .links
            .iter()
            .any(|link| link.from == closed.id && link.to == closed.id));
    }

    #[test]
    fn many_closed_orbits_emit_one_self_link_without_cross_orbit_rescans() {
        let orbit_count = 20_000_usize;
        let mut walks = Vec::with_capacity(orbit_count * 2);
        let mut mappings = Vec::with_capacity(orbit_count * 2);
        let mut unitigs = Vec::with_capacity(orbit_count);
        for unitig_index in 0..orbit_count {
            for orbit_representative in [true, false] {
                walks.push(RawWalk {
                    first: 0,
                    len: 1,
                    topology: CompactedTopology::ClosedWalk,
                });
                mappings.push(RawMapping {
                    unitig_index,
                    forward: orbit_representative,
                    reverse: !orbit_representative,
                    orbit_representative,
                });
            }
            let mut id = [0_u8; 32];
            id[..8].copy_from_slice(&u64::try_from(unitig_index).unwrap().to_le_bytes());
            unitigs.push(CompactedUnitig {
                id: CompactedUnitigId(id),
                topology: CompactedTopology::ClosedWalk,
                sequence: Vec::new(),
                steps: Vec::new(),
                total_support: 0,
                minimum_support: 0,
                lower_median_support: 0,
                maximum_support: 0,
                provenance_sha256: [0; 32],
            });
        }

        let links = derive_links(3, &[], &walks, &[], &mappings, &unitigs, orbit_count).unwrap();
        assert_eq!(links.len(), orbit_count);
        assert_eq!(
            mappings
                .iter()
                .filter(|mapping| mapping.orbit_representative)
                .count(),
            orbit_count
        );
        assert!(links.iter().zip(&unitigs).all(|(link, unitig)| {
            link.from == unitig.id
                && link.to == unitig.id
                && link.from_orientation == UnitigOrientation::Forward
                && link.to_orientation == UnitigOrientation::Forward
        }));
    }

    #[test]
    fn reverse_complement_fixed_edge_is_a_one_step_boundary_unitig() {
        let input = result_from_edges(4, &[b"AATT"]);
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert_eq!(output.stats.self_reverse_complement_edges, 1);
        assert_eq!(output.unitigs.len(), 1);
        assert_eq!(output.unitigs[0].steps.len(), 1);
        assert_eq!(
            output.unitigs[0].steps[0].orientation,
            EdgeOrientation::SelfReverseComplement
        );
        assert_eq!(output.stats.boundary_nodes, 2);
    }

    #[test]
    fn self_reverse_complement_node_stops_an_otherwise_linear_walk() {
        let input = result_from_edges(3, &[b"AAT", b"ATC"]);
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert!(output.stats.self_reverse_complement_nodes > 0);
        assert_eq!(output.unitigs.len(), 2);
        assert!(output.unitigs.iter().all(|unitig| unitig.steps.len() == 1));
    }

    #[test]
    fn wide_k_127_path_retains_full_identity() {
        let first = (0..127).map(|index| b"ACGT"[index % 4]).collect::<Vec<_>>();
        let mut second = first[1..].to_vec();
        second.push(b'A');
        let input = result_from_edges(127, &[&first, &second]);
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert_eq!(output.stats.canonical_edges, 2);
        assert_eq!(
            output
                .unitigs
                .iter()
                .map(|unitig| unitig.steps.len())
                .sum::<usize>(),
            2
        );
        assert!(output
            .unitigs
            .iter()
            .flat_map(|unitig| &unitig.steps)
            .any(|step| step.key.words()[0] != 0));
    }

    #[test]
    fn packed_word_boundaries_through_k127_match_literal_oracle() {
        for k in [31_u8, 32, 33, 63, 64, 65, 95, 96, 97, 126, 127] {
            let sequence = (0..usize::from(k) + 11)
                .map(|index| b"ACGTTGCA"[(index * 5 + index / 3) % 8])
                .collect::<Vec<_>>();
            let input = result_from_sequences(k, &[sequence]);
            let output = compact_external_counts(&input, generous_limits()).unwrap();
            assert_eq!(
                normalized_unitigs(&output),
                literal_oracle_unitigs(&input),
                "packed-word-boundary mismatch at k={k}"
            );
            assert_eq!(
                output.links,
                literal_oracle_links(&input, &output.unitigs),
                "link mismatch at packed-word boundary k={k}"
            );
        }
    }

    #[test]
    fn shared_k_domain_matches_the_independent_stable_graph_compactor() {
        let fixtures: Vec<(u8, Vec<&[u8]>)> = vec![
            (3, vec![b"CAA", b"AAC", b"AAG"]),
            (3, vec![b"ACA", b"CAC"]),
            (3, vec![b"AAT", b"ATC"]),
            (4, vec![b"AATT", b"ATTG"]),
            (5, vec![b"AACGC", b"ACGCT", b"CGCTA"]),
        ];
        for (k, edge_sequences) in fixtures {
            let input = result_from_edges(k, &edge_sequences);
            let observed = compact_external_counts(&input, generous_limits()).unwrap();
            let mut retained = input
                .edge_counts
                .iter()
                .map(|row| crate::model::KmerCount {
                    key: u128::try_from(row.key).unwrap(),
                    support: row.support,
                })
                .collect::<Vec<_>>();
            retained.sort_by_key(|record| record.key);
            let stable_graph =
                crate::graph::ExactGraph::from_sorted_retained(k, &retained, 1_000_000_000)
                    .unwrap();
            let stable = crate::compact::compact_graph(&stable_graph, 1_000_000_000).unwrap();
            let mut stable_unitigs = stable
                .unitigs
                .into_iter()
                .map(|unitig| {
                    (
                        match unitig.topology {
                            crate::model::Topology::Linear => CompactedTopology::Linear,
                            crate::model::Topology::ClosedGraphWalk => {
                                CompactedTopology::ClosedWalk
                            }
                        },
                        unitig.sequence,
                    )
                })
                .collect::<Vec<_>>();
            stable_unitigs.sort();
            assert_eq!(
                normalized_unitigs(&observed),
                stable_unitigs,
                "stable differential mismatch at k={k}"
            );
        }
    }

    #[test]
    fn rejects_mutated_routing_and_preallocates_under_resource_limits() {
        let mut input = result_from_sequences(5, &[b"AACCGGTTA".to_vec()]);
        input.edge_counts[0].bucket_id =
            (input.edge_counts[0].bucket_id + 1) % input.virtual_bucket_count;
        let error = compact_external_counts(&input, generous_limits()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InternalInvariant);

        let input = result_from_sequences(5, &[b"AACCGGTTA".to_vec()]);
        let mut limits = generous_limits();
        limits.max_accounted_bytes = 1;
        let error = compact_external_counts(&input, limits).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn scalar_configuration_errors_precede_memory_admission() {
        let input = result_from_sequences(5, &[b"AACCGGTTA".to_vec()]);
        let mut limits = generous_limits();
        limits.max_accounted_bytes = 0;

        let mut zero_minimizer = input.clone();
        zero_minimizer.minimizer_length = 0;
        let error = compact_external_counts(&zero_minimizer, limits).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigurationInvalidK);

        let mut long_minimizer = input.clone();
        long_minimizer.minimizer_length = long_minimizer.k + 1;
        let error = compact_external_counts(&long_minimizer, limits).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigurationInvalidK);

        let mut zero_buckets = input;
        zero_buckets.virtual_bucket_count = 0;
        let error = compact_external_counts(&zero_buckets, limits).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigurationInvalidLimit);
    }

    #[test]
    fn every_explicit_graph_cardinality_and_output_limit_fails_closed() {
        let fixed = result_from_edges(4, &[b"AATT"]);
        for mutate in [
            |limits: &mut CompactedGraphLimits| limits.max_oriented_handles = 0,
            |limits: &mut CompactedGraphLimits| limits.max_literal_nodes = 1,
            |limits: &mut CompactedGraphLimits| limits.max_unitigs = 0,
            |limits: &mut CompactedGraphLimits| limits.max_output_bases = 3,
        ] as [fn(&mut CompactedGraphLimits); 4]
        {
            let mut limits = generous_limits();
            mutate(&mut limits);
            let error = compact_external_counts(&fixed, limits).unwrap_err();
            assert_eq!(error.code(), ErrorCode::ResourceMemory);
        }

        let branch = result_from_edges(3, &[b"CAA", b"AAC", b"AAG"]);
        let mut limits = generous_limits();
        limits.max_link_candidates = 0;
        let error = compact_external_counts(&branch, limits).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn rejects_duplicate_unsorted_and_nonconserved_support_rows() {
        let input = result_from_sequences(5, &[b"AACCGGTTA".to_vec()]);

        let mut duplicate = input.clone();
        duplicate.edge_counts.push(duplicate.edge_counts[0]);
        duplicate.distinct_kmers += 1;
        let error = compact_external_counts(&duplicate, generous_limits()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InternalInvariant);

        let mut unsorted = input.clone();
        unsorted.edge_counts.reverse();
        let error = compact_external_counts(&unsorted, generous_limits()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InternalInvariant);

        let mut nonconserved = input;
        nonconserved.support_events += 1;
        let error = compact_external_counts(&nonconserved, generous_limits()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InternalInvariant);
    }

    #[test]
    fn fragment_instance_support_unit_is_preserved_without_reinterpretation() {
        let mut input = result_from_sequences(5, &[b"AACCGGTTA".to_vec()]);
        input.support_unit = WideRunSupportUnit::SuppliedFragmentInstance;
        for row in &mut input.edge_counts {
            row.support = 1;
        }
        input.support_events = input.edge_counts.len() as u64;
        let output = compact_external_counts(&input, generous_limits()).unwrap();
        assert_eq!(
            output.support_unit,
            WideRunSupportUnit::SuppliedFragmentInstance
        );
        assert_eq!(output.stats.input_support, input.support_events);
        assert!(output
            .unitigs
            .iter()
            .flat_map(|unitig| &unitig.steps)
            .all(|step| step.support == 1));
        assert_literal_evidence(&input, &output);
    }

    #[test]
    fn support_unit_changes_provenance_but_not_content_identity() {
        let occurrence = result_from_sequences(5, &[b"AACCGGTTA".to_vec()]);
        let mut fragment = occurrence.clone();
        fragment.support_unit = WideRunSupportUnit::SuppliedFragmentInstance;

        let occurrence_graph = compact_external_counts(&occurrence, generous_limits()).unwrap();
        let fragment_graph = compact_external_counts(&fragment, generous_limits()).unwrap();
        assert_eq!(occurrence_graph.unitigs.len(), 1);
        assert_eq!(fragment_graph.unitigs.len(), 1);
        assert_eq!(
            occurrence_graph.unitigs[0].provenance_sha256,
            [
                71, 248, 238, 110, 134, 7, 192, 55, 173, 248, 191, 28, 230, 242, 211, 173, 133,
                132, 188, 227, 157, 123, 68, 250, 75, 9, 57, 40, 102, 88, 206, 212,
            ]
        );
        assert_eq!(
            fragment_graph.unitigs[0].provenance_sha256,
            [
                35, 161, 38, 235, 30, 189, 171, 213, 98, 150, 247, 125, 89, 55, 243, 28, 25, 134,
                91, 209, 237, 190, 70, 121, 245, 167, 67, 83, 97, 206, 42, 48,
            ]
        );
        assert_ne!(
            occurrence_graph.exact_edge_table_sha256,
            fragment_graph.exact_edge_table_sha256
        );
        assert_eq!(occurrence_graph.unitigs.len(), fragment_graph.unitigs.len());
        for (occurrence_unitig, fragment_unitig) in
            occurrence_graph.unitigs.iter().zip(&fragment_graph.unitigs)
        {
            assert_eq!(occurrence_unitig.id, fragment_unitig.id);
            assert_eq!(occurrence_unitig.sequence, fragment_unitig.sequence);
            assert_eq!(occurrence_unitig.steps, fragment_unitig.steps);
            assert_ne!(
                occurrence_unitig.provenance_sha256,
                fragment_unitig.provenance_sha256
            );
        }
        assert_literal_evidence(&occurrence, &occurrence_graph);
        assert_literal_evidence(&fragment, &fragment_graph);
    }

    #[test]
    fn serial_compaction_ignores_source_observation_and_ambient_pool_order() {
        let forward = vec![b"AACCGGTTAA".to_vec(), b"AAACCCGGG".to_vec()];
        let mut reordered = forward.clone();
        reordered.reverse();
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| {
                compact_external_counts(&result_from_sequences(5, &forward), generous_limits())
                    .unwrap()
            });
        let four = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap()
            .install(|| {
                compact_external_counts(&result_from_sequences(5, &reordered), generous_limits())
                    .unwrap()
            });
        assert_eq!(one, four);
    }

    #[test]
    fn linear_time_rotation_selector_matches_naive_exhaustive_oracle() {
        for length in 1..=7 {
            let cases = 4_usize.pow(length as u32);
            for encoded in 0..cases {
                let mut value = encoded;
                let mut values = vec![0_usize; length];
                for item in values.iter_mut().rev() {
                    *item = value & 3;
                    value >>= 2;
                }
                let observed = minimum_rotation(values.len(), |index| values[index]);
                let expected = (0..values.len())
                    .min_by(|left, right| {
                        (0..values.len())
                            .map(|offset| values[(left + offset) % values.len()])
                            .cmp(
                                (0..values.len())
                                    .map(|offset| values[(right + offset) % values.len()]),
                            )
                            .then_with(|| left.cmp(right))
                    })
                    .unwrap();
                assert_eq!(
                    (0..values.len())
                        .map(|offset| values[(observed + offset) % values.len()])
                        .collect::<Vec<_>>(),
                    (0..values.len())
                        .map(|offset| values[(expected + offset) % values.len()])
                        .collect::<Vec<_>>(),
                    "rotation mismatch for encoded case {encoded} length {length}"
                );
            }
        }
    }

    #[test]
    fn large_cycle_rotation_and_matching_have_linear_comparison_bounds() {
        let length = 100_003_usize;
        let mut repetitive = vec![0_usize; length];
        repetitive[length - 1] = 1;
        let (rotation, comparisons) =
            minimum_rotation_with_comparisons(length, |index| repetitive[index]);
        assert!(rotation < length);
        assert!(
            comparisons <= 4 * length,
            "{comparisons} comparisons exceeded the linear test bound"
        );

        let values = (0..length).collect::<Vec<_>>();
        let mates = values.clone();
        let reverse = values.iter().rev().copied().collect::<Vec<_>>();
        let rotation_offset = 41_237_usize;
        let rotated_reverse = (0..length)
            .map(|position| reverse[(rotation_offset + position) % length])
            .collect::<Vec<_>>();
        assert!(cycle_equal(&reverse, &rotated_reverse));
        assert!(reverse_walk_matches(
            &values,
            &rotated_reverse,
            &mates,
            CompactedTopology::ClosedWalk
        ));
    }

    #[test]
    fn exhaustive_k3_short_sources_match_independent_literal_oracle() {
        for length in 3..=6 {
            let cases = 4_usize.pow(length as u32);
            for encoded in 0..cases {
                let mut value = encoded;
                let mut sequence = vec![b'A'; length];
                for base in sequence.iter_mut().rev() {
                    *base = b"ACGT"[value & 3];
                    value >>= 2;
                }
                let input = result_from_sequences(3, &[sequence]);
                let actual = compact_external_counts(&input, generous_limits()).unwrap();
                assert_eq!(
                    normalized_unitigs(&actual),
                    literal_oracle_unitigs(&input),
                    "literal oracle mismatch for encoded source {encoded} length {length}"
                );
                assert_eq!(actual.links, literal_oracle_links(&input, &actual.unitigs));
            }
        }
    }

    #[test]
    fn exhaustive_single_and_pair_edges_k3_k4_match_independent_oracles() {
        for k in [3_u8, 4] {
            let universe = 4_usize.pow(u32::from(k));
            let mut canonical_edges = Vec::<Vec<u8>>::new();
            for encoded in 0..universe {
                let mut value = encoded;
                let mut edge = vec![b'A'; usize::from(k)];
                for base in edge.iter_mut().rev() {
                    *base = b"ACGT"[value & 3];
                    value >>= 2;
                }
                if edge <= reverse(&edge) {
                    canonical_edges.push(edge);
                }
            }

            for left in 0..canonical_edges.len() {
                let singleton = result_from_edges(k, &[canonical_edges[left].as_slice()]);
                assert_graph_oracles(&singleton);
                for right in left + 1..canonical_edges.len() {
                    let pair = result_from_edges(
                        k,
                        &[
                            canonical_edges[left].as_slice(),
                            canonical_edges[right].as_slice(),
                        ],
                    );
                    assert_graph_oracles(&pair);
                }
            }

            let complete_slices = canonical_edges
                .iter()
                .map(Vec::as_slice)
                .collect::<Vec<_>>();
            assert_graph_oracles(&result_from_edges(k, &complete_slices));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(384))]

        #[test]
        fn generated_sources_match_literal_oracle_and_reverse_complement(
            k in 3_u8..=8,
            bases in prop::collection::vec(0_u8..4, 8..28),
        ) {
            let sequence = bases
                .into_iter()
                .map(|base| b"ACGT"[usize::from(base)])
                .collect::<Vec<_>>();
            let forward = result_from_sequences(k, std::slice::from_ref(&sequence));
            let backward_sequence = reverse(&sequence);
            let backward =
                result_from_sequences(k, std::slice::from_ref(&backward_sequence));
            let observed =
                compact_external_counts(&forward, generous_limits()).unwrap();
            let reversed =
                compact_external_counts(&backward, generous_limits()).unwrap();
            prop_assert_eq!(
                normalized_unitigs(&observed),
                literal_oracle_unitigs(&forward)
            );
            prop_assert_eq!(
                observed.links.clone(),
                literal_oracle_links(&forward, &observed.unitigs)
            );
            assert_literal_evidence(&forward, &observed);
            prop_assert_eq!(observed, reversed);
        }

        #[test]
        fn arbitrary_small_exact_edge_sets_match_literal_oracle(
            encoded_edges in prop::collection::vec(0_u8..64, 0..20),
        ) {
            let edge_storage = encoded_edges
                .into_iter()
                .map(|mut value| {
                    let mut edge = vec![b'A'; 3];
                    for base in edge.iter_mut().rev() {
                        *base = b"ACGT"[usize::from(value & 3)];
                        value >>= 2;
                    }
                    edge
                })
                .collect::<Vec<_>>();
            let edge_slices = edge_storage
                .iter()
                .map(Vec::as_slice)
                .collect::<Vec<_>>();
            let input = result_from_edges(3, &edge_slices);
            let actual =
                compact_external_counts(&input, generous_limits()).unwrap();
            prop_assert_eq!(
                normalized_unitigs(&actual),
                literal_oracle_unitigs(&input)
            );
            prop_assert_eq!(
                actual.links.clone(),
                literal_oracle_links(&input, &actual.unitigs)
            );
            assert_literal_evidence(&input, &actual);
        }
    }

    #[derive(Clone, Default)]
    struct OracleNode {
        incoming: Vec<Vec<u8>>,
        outgoing: Vec<Vec<u8>>,
        fixed_incident: bool,
    }

    fn assert_graph_oracles(input: &ExternalPartitionResult) {
        let actual = compact_external_counts(input, generous_limits()).unwrap();
        assert_eq!(normalized_unitigs(&actual), literal_oracle_unitigs(input));
        assert_eq!(actual.links, literal_oracle_links(input, &actual.unitigs));
        assert_literal_evidence(input, &actual);
    }

    fn literal_oracle_unitigs(
        input: &ExternalPartitionResult,
    ) -> Vec<(CompactedTopology, Vec<u8>)> {
        let k = usize::from(input.k);
        let mut handles = BTreeSet::<Vec<u8>>::new();
        let mut fixed_edges = BTreeSet::<Vec<u8>>::new();
        for row in &input.edge_counts {
            let forward = decode_mer(row.key, input.k).unwrap();
            let backward = reverse(&forward);
            handles.insert(forward.clone());
            if backward == forward {
                fixed_edges.insert(forward);
            } else {
                handles.insert(backward);
            }
        }
        let mut nodes = BTreeMap::<Vec<u8>, OracleNode>::new();
        for handle in &handles {
            let source = handle[..k - 1].to_vec();
            let target = handle[1..].to_vec();
            nodes
                .entry(source)
                .or_default()
                .outgoing
                .push(handle.clone());
            nodes
                .entry(target)
                .or_default()
                .incoming
                .push(handle.clone());
        }
        for node in nodes.values_mut() {
            node.incoming.sort();
            node.outgoing.sort();
            node.fixed_incident = node
                .incoming
                .iter()
                .chain(&node.outgoing)
                .any(|handle| fixed_edges.contains(handle));
        }
        let boundary = |code: &[u8], node: &OracleNode| {
            node.incoming.len() != 1
                || node.outgoing.len() != 1
                || reverse(code) == code
                || node.fixed_incident
        };
        let mut used = BTreeSet::<Vec<u8>>::new();
        let mut paths = Vec::<(CompactedTopology, Vec<Vec<u8>>)>::new();
        for (code, node) in &nodes {
            if !boundary(code, node) {
                continue;
            }
            for first in &node.outgoing {
                if used.contains(first) {
                    continue;
                }
                let mut path = Vec::new();
                let mut current = first.clone();
                loop {
                    assert!(used.insert(current.clone()));
                    path.push(current.clone());
                    let target = current[1..].to_vec();
                    let next_node = &nodes[&target];
                    if boundary(&target, next_node) {
                        break;
                    }
                    current = next_node.outgoing[0].clone();
                }
                paths.push((CompactedTopology::Linear, path));
            }
        }
        for first in &handles {
            if used.contains(first) {
                continue;
            }
            let mut path = Vec::new();
            let mut current = first.clone();
            loop {
                assert!(used.insert(current.clone()));
                path.push(current.clone());
                let target = current[1..].to_vec();
                let next = nodes[&target].outgoing[0].clone();
                if next == *first {
                    break;
                }
                current = next;
            }
            paths.push((CompactedTopology::ClosedWalk, path));
        }
        assert_eq!(used, handles);
        let mut orbits = BTreeSet::new();
        for (topology, path) in paths {
            let representative = match topology {
                CompactedTopology::Linear => {
                    let sequence = oracle_spell(&path);
                    sequence.clone().min(reverse(&sequence))
                }
                CompactedTopology::ClosedWalk => {
                    let mut choices = Vec::new();
                    let reverse_path = path
                        .iter()
                        .rev()
                        .map(|handle| reverse(handle))
                        .collect::<Vec<_>>();
                    for candidate in [&path, &reverse_path] {
                        for rotation in 0..candidate.len() {
                            let rotated = (0..candidate.len())
                                .map(|offset| {
                                    candidate[(rotation + offset) % candidate.len()].clone()
                                })
                                .collect::<Vec<_>>();
                            choices.push(oracle_spell(&rotated));
                        }
                    }
                    choices.into_iter().min().unwrap()
                }
            };
            orbits.insert((topology, representative));
        }
        orbits.into_iter().collect()
    }

    fn oracle_spell(path: &[Vec<u8>]) -> Vec<u8> {
        let mut sequence = path[0].clone();
        for handle in &path[1..] {
            sequence.push(*handle.last().unwrap());
        }
        sequence
    }

    fn literal_oracle_links(
        input: &ExternalPartitionResult,
        unitigs: &[CompactedUnitig],
    ) -> Vec<CompactedLink> {
        let k = usize::from(input.k);
        let mut handles = BTreeSet::<Vec<u8>>::new();
        let mut fixed_edges = BTreeSet::<Vec<u8>>::new();
        for row in &input.edge_counts {
            let forward = decode_mer(row.key, input.k).unwrap();
            let backward = reverse(&forward);
            handles.insert(forward.clone());
            if backward == forward {
                fixed_edges.insert(forward);
            } else {
                handles.insert(backward);
            }
        }

        let mut nodes = BTreeMap::<Vec<u8>, OracleNode>::new();
        for handle in &handles {
            let source = handle[..k - 1].to_vec();
            let target = handle[1..].to_vec();
            nodes
                .entry(source)
                .or_default()
                .outgoing
                .push(handle.clone());
            nodes
                .entry(target)
                .or_default()
                .incoming
                .push(handle.clone());
        }
        for node in nodes.values_mut() {
            node.incoming.sort();
            node.outgoing.sort();
            node.fixed_incident = node
                .incoming
                .iter()
                .chain(&node.outgoing)
                .any(|handle| fixed_edges.contains(handle));
        }

        type OrientedUnitig = (CompactedUnitigId, UnitigOrientation);
        let mut starts = BTreeMap::<Vec<u8>, Vec<OrientedUnitig>>::new();
        let mut ends = BTreeMap::<Vec<u8>, Vec<OrientedUnitig>>::new();
        for unitig in unitigs
            .iter()
            .filter(|unitig| unitig.topology == CompactedTopology::Linear)
        {
            for orientation in [
                UnitigOrientation::Forward,
                UnitigOrientation::ReverseComplement,
            ] {
                let sequence = oracle_oriented_sequence(&unitig.sequence, orientation);
                starts
                    .entry(sequence[..k].to_vec())
                    .or_default()
                    .push((unitig.id, orientation));
                ends.entry(sequence[sequence.len() - k..].to_vec())
                    .or_default()
                    .push((unitig.id, orientation));
            }
        }

        let mut links = Vec::new();
        for (code, node) in &nodes {
            let boundary = node.incoming.len() != 1
                || node.outgoing.len() != 1
                || reverse(code) == *code
                || node.fixed_incident;
            if !boundary {
                continue;
            }
            for incoming in &node.incoming {
                let from = ends
                    .get(incoming)
                    .expect("oracle incoming handle lacks an oriented unitig end");
                for outgoing in &node.outgoing {
                    let to = starts
                        .get(outgoing)
                        .expect("oracle outgoing handle lacks an oriented unitig start");
                    for &(from_id, from_orientation) in from {
                        for &(to_id, to_orientation) in to {
                            links.push(oracle_canonical_link(CompactedLink {
                                from: from_id,
                                from_orientation,
                                to: to_id,
                                to_orientation,
                                overlap_bases: input.k - 1,
                            }));
                        }
                    }
                }
            }
        }
        for unitig in unitigs
            .iter()
            .filter(|unitig| unitig.topology == CompactedTopology::ClosedWalk)
        {
            links.push(oracle_canonical_link(CompactedLink {
                from: unitig.id,
                from_orientation: UnitigOrientation::Forward,
                to: unitig.id,
                to_orientation: UnitigOrientation::Forward,
                overlap_bases: input.k - 1,
            }));
        }
        links.sort_by(oracle_link_order);
        links.dedup();
        links
    }

    fn oracle_canonical_link(link: CompactedLink) -> CompactedLink {
        let reverse = CompactedLink {
            from: link.to,
            from_orientation: oracle_reverse_orientation(link.to_orientation),
            to: link.from,
            to_orientation: oracle_reverse_orientation(link.from_orientation),
            overlap_bases: link.overlap_bases,
        };
        if oracle_link_order(&link, &reverse) == Ordering::Greater {
            reverse
        } else {
            link
        }
    }

    fn oracle_link_order(left: &CompactedLink, right: &CompactedLink) -> Ordering {
        left.from
            .0
            .cmp(&right.from.0)
            .then_with(|| (left.from_orientation as u8).cmp(&(right.from_orientation as u8)))
            .then_with(|| left.to.0.cmp(&right.to.0))
            .then_with(|| (left.to_orientation as u8).cmp(&(right.to_orientation as u8)))
            .then_with(|| left.overlap_bases.cmp(&right.overlap_bases))
    }

    fn oracle_reverse_orientation(orientation: UnitigOrientation) -> UnitigOrientation {
        match orientation {
            UnitigOrientation::Forward => UnitigOrientation::ReverseComplement,
            UnitigOrientation::ReverseComplement => UnitigOrientation::Forward,
        }
    }

    fn oracle_oriented_sequence(sequence: &[u8], orientation: UnitigOrientation) -> Vec<u8> {
        match orientation {
            UnitigOrientation::Forward => sequence.to_vec(),
            UnitigOrientation::ReverseComplement => reverse(sequence),
        }
    }
}
