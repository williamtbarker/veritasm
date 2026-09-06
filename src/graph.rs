//! Exact, strand-symmetric de Bruijn graph over retained canonical k-mers.
//!
//! A graph edge is an oriented *view* of one retained canonical k-mer. The
//! support value belongs to the canonical key and is therefore never copied or
//! summed once per orientation. Graph navigation, however, uses every literal
//! oriented spelling (except that a self-reverse-complementary k-mer has one
//! fixed view). Both nodes incident to such a fixed view are conservative
//! compaction boundaries: crossing it would allow one raw walk to consume both
//! orientations of another backing canonical key. This distinction is central
//! to the conservation checks performed by [`crate::compact`].

use crate::dna::{active_mask, reverse_complement_validated};
use crate::error::{ErrorCode, Result, VeritasmError};
use crate::model::KmerCount;
use serde::Serialize;
use std::mem::size_of;

/// Lowest k supported by the stable packed representation.
pub const MIN_GRAPH_K: u8 = crate::dna::MIN_PACKED_K;
/// Highest k whose two-bit spelling fits in the active low bits of a `u128`.
pub const MAX_GRAPH_K: u8 = crate::dna::MAX_PACKED_K;
/// Schema 1.0 graph-allocation ceiling.
pub const MAX_RETAINED_CANONICAL_KEYS: usize = 500_000_000;

/// Orientation of a retained canonical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Orientation {
    Plus,
    Minus,
}

impl Orientation {
    pub const fn symbol(self) -> char {
        match self {
            Self::Plus => '+',
            Self::Minus => '-',
        }
    }

    pub const fn flip(self) -> Self {
        match self {
            Self::Plus => Self::Minus,
            Self::Minus => Self::Plus,
        }
    }
}

/// One literal oriented k-mer view used as a directed graph edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrientedHandle {
    canonical_index: usize,
    orientation: Orientation,
    spelling: u128,
}

impl OrientedHandle {
    pub const fn canonical_index(self) -> usize {
        self.canonical_index
    }

    pub const fn orientation(self) -> Orientation {
        self.orientation
    }

    pub const fn spelling(self) -> u128 {
        self.spelling
    }
}

/// Literal directed degree of a packed `(k - 1)`-mer node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeDegree {
    pub incoming: u8,
    pub outgoing: u8,
}

/// Fixed-capacity incidence list; DNA admits at most four extensions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NeighborIndices {
    values: [usize; 4],
    length: u8,
}

impl NeighborIndices {
    fn push(&mut self, index: usize) {
        let slot = usize::from(self.length);
        debug_assert!(slot < self.values.len());
        self.values[slot] = index;
        self.length += 1;
    }

    pub fn len(self) -> usize {
        usize::from(self.length)
    }

    pub fn first(self) -> Option<usize> {
        (self.length != 0).then_some(self.values[0])
    }

    pub fn iter(self) -> impl Iterator<Item = usize> {
        self.values.into_iter().take(usize::from(self.length))
    }
}

/// Deterministic graph accounting. Support is counted once per canonical key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct GraphStats {
    pub nodes: u64,
    pub canonical_kmers: u64,
    pub oriented_handles: u64,
    pub self_reverse_complement_keys: u64,
    pub boundary_nodes: u64,
    pub total_support: u64,
}

/// Immutable exact graph built from a sorted retained-key stream.
#[derive(Debug)]
pub struct ExactGraph {
    k: u8,
    node_mask: u128,
    canonical: Vec<KmerCount>,
    /// Sorted by unsigned oriented spelling and then `+ < -`.
    handles: Vec<OrientedHandle>,
    mate_indices: Vec<usize>,
    /// Sorted distinct literal `(k - 1)`-mer node codes. Incidence is generated
    /// from four possible bases plus binary search, so nodes require no hash
    /// index or individual heap allocation.
    nodes: Vec<u128>,
    stats: GraphStats,
    /// Conservative bytes attributed to the graph's owned heap allocations.
    /// This is an admission-control estimate, not a measured RSS value.
    accounted_allocation_bytes: u64,
}

impl ExactGraph {
    /// Build a graph from a strictly increasing stream of canonical keys.
    ///
    /// Keys must be right-aligned in their active `2*k` bits and supports must
    /// be positive. These are upstream retained-stream invariants, so a
    /// violation is reported as an internal failure rather than silently
    /// repaired. `phase_memory_budget_bytes` admits construction temporaries
    /// and graph-owned allocations using checked `size_of` estimates; the
    /// caller-owned borrowed slice and process RSS outside this phase are not
    /// included.
    pub fn from_sorted_retained(
        k: u8,
        retained: &[KmerCount],
        phase_memory_budget_bytes: u64,
    ) -> Result<Self> {
        validate_graph_k(k)?;
        validate_retained_count(retained.len())?;
        let copy_payload = bytes_for::<KmerCount>(retained.len(), "retained-key copy")?;
        let copy_required = conservative_allocation_bytes(copy_payload, 1)?;
        ensure_phase_budget(
            copy_required,
            phase_memory_budget_bytes,
            "retained-key copy",
        )?;
        let mut owned = try_vec_with_capacity(retained.len(), "copy retained canonical keys")?;
        owned.extend_from_slice(retained);
        Self::from_sorted_retained_owned(k, owned, phase_memory_budget_bytes)
    }

    /// Consuming constructor for the bounded-memory pipeline. This avoids
    /// retaining a second copy of a potentially large exact count table. The
    /// byte budget covers the consumed vector, graph-owned allocations, and
    /// construction temporaries, rather than total process RSS.
    pub fn from_sorted_retained_owned(
        k: u8,
        retained: Vec<KmerCount>,
        phase_memory_budget_bytes: u64,
    ) -> Result<Self> {
        validate_graph_k(k)?;
        validate_retained_count(retained.len())?;
        let key_mask = active_mask(k);
        let node_mask = active_mask(k - 1);
        let mut previous = None;
        let mut total_support = 0u64;
        let mut self_reverse_complement_keys = 0u64;

        // Validate the complete retained table and determine the exact oriented
        // handle count before allocating any graph-derived table.
        for (canonical_index, record) in retained.iter().copied().enumerate() {
            if record.key & !key_mask != 0 {
                return Err(invariant(format!(
                    "retained key at index {canonical_index} has nonzero bits outside the active 2*k range"
                )));
            }
            if record.support == 0 {
                return Err(invariant(format!(
                    "retained key at index {canonical_index} has zero support"
                )));
            }
            if previous.is_some_and(|value| record.key <= value) {
                return Err(invariant(format!(
                    "retained keys are not strictly increasing at index {canonical_index}"
                )));
            }
            previous = Some(record.key);

            let reverse = reverse_complement_validated(record.key, k);
            if record.key > reverse {
                return Err(invariant(format!(
                    "retained key at index {canonical_index} is not canonical"
                )));
            }
            total_support = total_support.checked_add(record.support).ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "retained canonical support mass exceeds u64",
                )
            })?;

            if reverse == record.key {
                self_reverse_complement_keys = self_reverse_complement_keys
                    .checked_add(1)
                    .ok_or_else(|| invariant("self-reverse-complement key count overflow"))?;
            }
        }

        let doubled = retained
            .len()
            .checked_mul(2)
            .ok_or_else(|| overflow("oriented handle count overflow"))?;
        let self_rc = usize::try_from(self_reverse_complement_keys)
            .map_err(|_| overflow("self-reverse-complement key count does not fit usize"))?;
        let oriented_count = doubled
            .checked_sub(self_rc)
            .ok_or_else(|| invariant("self-reverse-complement count exceeds doubled keys"))?;
        let node_entry_count = oriented_count
            .checked_mul(2)
            .ok_or_else(|| overflow("literal node-entry count overflow"))?;
        let allocation_plan = graph_allocation_plan(
            retained.capacity(),
            retained.len(),
            oriented_count,
            node_entry_count,
        )?;
        ensure_phase_budget(
            allocation_plan.peak_bytes,
            phase_memory_budget_bytes,
            "exact graph construction",
        )?;

        let mut handles = try_vec_with_capacity(oriented_count, "allocate oriented graph handles")?;
        for (canonical_index, record) in retained.iter().copied().enumerate() {
            let reverse = reverse_complement_validated(record.key, k);
            handles.push(make_handle(canonical_index, Orientation::Plus, record.key));
            if reverse != record.key {
                handles.push(make_handle(canonical_index, Orientation::Minus, reverse));
            }
        }

        handles.sort_unstable_by(|left, right| {
            left.spelling
                .cmp(&right.spelling)
                .then_with(|| left.orientation.cmp(&right.orientation))
        });

        let mut plus_by_key = try_filled_vec(
            retained.len(),
            None,
            "allocate plus-orientation handle lookup",
        )?;
        let mut minus_by_key = try_filled_vec(
            retained.len(),
            None,
            "allocate minus-orientation handle lookup",
        )?;
        for (index, handle) in handles.iter().copied().enumerate() {
            let slot = match handle.orientation {
                Orientation::Plus => &mut plus_by_key[handle.canonical_index],
                Orientation::Minus => &mut minus_by_key[handle.canonical_index],
            };
            if slot.replace(index).is_some() {
                return Err(invariant("duplicate oriented handle identity"));
            }
        }

        let mut mate_indices = try_filled_vec(
            handles.len(),
            0usize,
            "allocate reverse-complement handle map",
        )?;
        for canonical_index in 0..retained.len() {
            let plus = plus_by_key[canonical_index]
                .ok_or_else(|| invariant("canonical key has no plus-oriented handle"))?;
            if let Some(minus) = minus_by_key[canonical_index] {
                mate_indices[plus] = minus;
                mate_indices[minus] = plus;
            } else {
                mate_indices[plus] = plus;
            }
        }

        // These two temporary lookup vectors are no longer live when the
        // potentially larger node table is admitted and allocated.
        drop(plus_by_key);
        drop(minus_by_key);

        let mut nodes = try_vec_with_capacity(node_entry_count, "allocate literal graph nodes")?;
        for handle in &handles {
            nodes.push(handle.spelling >> 2);
            nodes.push(handle.spelling & node_mask);
        }
        nodes.sort_unstable();
        nodes.dedup();

        let stats = GraphStats {
            nodes: u64_from_usize(nodes.len(), "graph node count")?,
            canonical_kmers: u64_from_usize(retained.len(), "canonical key count")?,
            oriented_handles: u64_from_usize(handles.len(), "oriented handle count")?,
            self_reverse_complement_keys,
            boundary_nodes: 0,
            total_support,
        };

        let mut graph = Self {
            k,
            node_mask,
            canonical: retained,
            handles,
            mate_indices,
            nodes,
            stats,
            accounted_allocation_bytes: allocation_plan.resident_bytes,
        };
        graph.stats.boundary_nodes = u64_from_usize(
            graph
                .nodes
                .iter()
                .filter(|node| graph.is_boundary(**node))
                .count(),
            "boundary node count",
        )?;
        graph.validate_reverse_complement_involution()?;
        let actual_resident = graph_resident_bytes(
            graph.canonical.capacity(),
            graph.handles.capacity(),
            graph.mate_indices.capacity(),
            graph.nodes.capacity(),
        )?;
        ensure_phase_budget(
            actual_resident,
            phase_memory_budget_bytes,
            "resident exact graph",
        )?;
        graph.accounted_allocation_bytes = actual_resident;
        Ok(graph)
    }

    pub const fn k(&self) -> u8 {
        self.k
    }

    pub const fn node_mask(&self) -> u128 {
        self.node_mask
    }

    pub const fn stats(&self) -> GraphStats {
        self.stats
    }

    /// Conservative byte estimate used when admitting a following phase while
    /// this graph remains resident. It covers owned vector capacities plus a
    /// fixed allocator/accounting margin, but is not measured process RSS.
    pub(crate) const fn accounted_allocation_bytes(&self) -> u64 {
        self.accounted_allocation_bytes
    }

    pub fn retained(&self) -> &[KmerCount] {
        &self.canonical
    }

    pub fn handles(&self) -> &[OrientedHandle] {
        &self.handles
    }

    pub fn handle(&self, index: usize) -> Option<OrientedHandle> {
        self.handles.get(index).copied()
    }

    pub fn mate_index(&self, index: usize) -> Option<usize> {
        self.mate_indices.get(index).copied()
    }

    pub fn canonical_key_for_handle(&self, index: usize) -> Option<u128> {
        let handle = self.handles.get(index)?;
        self.canonical
            .get(handle.canonical_index)
            .map(|record| record.key)
    }

    pub fn handle_source(&self, index: usize) -> Option<u128> {
        self.handles.get(index).map(|handle| handle.spelling >> 2)
    }

    pub fn handle_target(&self, index: usize) -> Option<u128> {
        self.handles
            .get(index)
            .map(|handle| handle.spelling & self.node_mask)
    }

    pub fn node_codes(&self) -> impl Iterator<Item = u128> + '_ {
        self.nodes.iter().copied()
    }

    pub fn degree(&self, node: u128) -> NodeDegree {
        NodeDegree {
            incoming: u8::try_from(self.incoming(node).len())
                .expect("DNA has at most four possible incoming bases"),
            outgoing: u8::try_from(self.outgoing(node).len())
                .expect("DNA has at most four possible outgoing bases"),
        }
    }

    pub fn is_palindromic_node(&self, node: u128) -> bool {
        is_reverse_complement_palindrome(node, self.k - 1)
    }

    /// Whether compaction must stop at this literal node.
    ///
    /// Besides degree and self-complemental-node boundaries, both endpoints
    /// of a self-complemental k-mer handle are boundaries. The latter case is
    /// possible only for even `k` and prevents a walk from crossing an
    /// orientation fixed point and reusing one backing canonical key.
    pub fn is_boundary(&self, node: u128) -> bool {
        let incoming = self.incoming(node);
        let outgoing = self.outgoing(node);
        incoming.len() != 1
            || outgoing.len() != 1
            || self.is_palindromic_node(node)
            || incoming
                .iter()
                .chain(outgoing.iter())
                .any(|index| self.mate_index(index) == Some(index))
    }

    pub(crate) fn incoming(&self, node: u128) -> NeighborIndices {
        let shift = 2 * u32::from(self.k - 1);
        let mut indices = NeighborIndices::default();
        for base in 0u128..4 {
            if let Some(index) = self.find_spelling((base << shift) | node) {
                indices.push(index);
            }
        }
        indices
    }

    pub(crate) fn outgoing(&self, node: u128) -> NeighborIndices {
        let mut indices = NeighborIndices::default();
        for base in 0u128..4 {
            if let Some(index) = self.find_spelling((node << 2) | base) {
                indices.push(index);
            }
        }
        indices
    }

    pub(crate) fn support_for_canonical_index(&self, index: usize) -> Option<u64> {
        self.canonical.get(index).map(|record| record.support)
    }

    fn find_spelling(&self, spelling: u128) -> Option<usize> {
        self.handles
            .binary_search_by_key(&spelling, |handle| handle.spelling)
            .ok()
    }

    fn validate_reverse_complement_involution(&self) -> Result<()> {
        for (index, handle) in self.handles.iter().copied().enumerate() {
            let mate_index = self.mate_indices[index];
            let mate = self.handles[mate_index];
            let source = self
                .handle_source(index)
                .ok_or_else(|| invariant("handle disappeared during validation"))?;
            let target = self
                .handle_target(index)
                .ok_or_else(|| invariant("handle disappeared during validation"))?;
            let mate_source = self
                .handle_source(mate_index)
                .ok_or_else(|| invariant("mate handle disappeared during validation"))?;
            let mate_target = self
                .handle_target(mate_index)
                .ok_or_else(|| invariant("mate handle disappeared during validation"))?;
            if self.mate_indices[mate_index] != index
                || mate.canonical_index != handle.canonical_index
                || mate.spelling != reverse_complement_validated(handle.spelling, self.k)
                || mate_source != reverse_complement_validated(target, self.k - 1)
                || mate_target != reverse_complement_validated(source, self.k - 1)
            {
                return Err(invariant(format!(
                    "oriented handle {index} violates the reverse-complement involution"
                )));
            }
        }
        Ok(())
    }
}

fn make_handle(canonical_index: usize, orientation: Orientation, spelling: u128) -> OrientedHandle {
    OrientedHandle {
        canonical_index,
        orientation,
        spelling,
    }
}

#[derive(Debug, Clone, Copy)]
struct GraphAllocationPlan {
    peak_bytes: u64,
    resident_bytes: u64,
}

fn graph_allocation_plan(
    retained_capacity: usize,
    retained_len: usize,
    oriented_count: usize,
    node_entry_count: usize,
) -> Result<GraphAllocationPlan> {
    let retained = bytes_for::<KmerCount>(retained_capacity, "retained graph keys")?;
    let handles = bytes_for::<OrientedHandle>(oriented_count, "oriented graph handles")?;
    let orientation_lookup = bytes_for::<Option<usize>>(
        retained_len
            .checked_mul(2)
            .ok_or_else(|| overflow("orientation lookup count overflow"))?,
        "orientation lookup tables",
    )?;
    let mates = bytes_for::<usize>(oriented_count, "reverse-complement handle map")?;
    let nodes = bytes_for::<u128>(node_entry_count, "literal graph node entries")?;

    let lookup_peak = checked_sum_bytes(
        &[retained, handles, orientation_lookup, mates],
        "graph orientation-lookup peak",
    )?;
    let node_peak = checked_sum_bytes(&[retained, handles, mates, nodes], "graph node-table peak")?;
    let resident_payload = checked_sum_bytes(
        &[retained, handles, mates, nodes],
        "resident graph allocation",
    )?;
    Ok(GraphAllocationPlan {
        peak_bytes: conservative_allocation_bytes(lookup_peak.max(node_peak), 6)?,
        resident_bytes: conservative_allocation_bytes(resident_payload, 4)?,
    })
}

fn graph_resident_bytes(
    retained_capacity: usize,
    handle_capacity: usize,
    mate_capacity: usize,
    node_capacity: usize,
) -> Result<u64> {
    let payload = checked_sum_bytes(
        &[
            bytes_for::<KmerCount>(retained_capacity, "resident retained keys")?,
            bytes_for::<OrientedHandle>(handle_capacity, "resident oriented handles")?,
            bytes_for::<usize>(mate_capacity, "resident mate indices")?,
            bytes_for::<u128>(node_capacity, "resident literal nodes")?,
        ],
        "resident graph allocation",
    )?;
    conservative_allocation_bytes(payload, 4)
}

fn validate_retained_count(count: usize) -> Result<()> {
    if count > MAX_RETAINED_CANONICAL_KEYS {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            format!(
                "retained canonical-key count {count} exceeds schema 1.0 maximum {MAX_RETAINED_CANONICAL_KEYS}"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn bytes_for<T>(count: usize, label: &'static str) -> Result<u64> {
    let bytes = count
        .checked_mul(size_of::<T>())
        .ok_or_else(|| overflow(format!("{label} byte estimate overflow")))?;
    u64::try_from(bytes).map_err(|_| overflow(format!("{label} byte estimate does not fit u64")))
}

pub(crate) fn checked_sum_bytes(values: &[u64], label: &'static str) -> Result<u64> {
    values.iter().try_fold(0u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| overflow(format!("{label} byte estimate overflow")))
    })
}

/// Add an explicit 12.5% margin, a fixed 64 KiB phase allowance, and one
/// page-sized allowance per independently requested top-level vector. This is
/// a deterministic admission estimate for allocations owned by this phase. It
/// is deliberately not an RSS claim: allocator internals, stacks, code, and
/// caller-owned inputs are outside its scope.
pub(crate) fn conservative_allocation_bytes(payload: u64, vector_allocations: u64) -> Result<u64> {
    let margin = payload
        .checked_add(7)
        .ok_or_else(|| overflow("allocation margin rounding overflow"))?
        / 8;
    checked_sum_bytes(
        &[
            payload,
            margin,
            64 << 10,
            vector_allocations
                .checked_mul(4 << 10)
                .ok_or_else(|| overflow("allocation page allowance overflow"))?,
        ],
        "conservative allocation",
    )
}

pub(crate) fn ensure_phase_budget(required: u64, budget: u64, phase: &'static str) -> Result<()> {
    if required > budget {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "{phase} conservative allocation estimate {required} bytes exceeds phase memory budget {budget} bytes"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn try_vec_with_capacity<T>(capacity: usize, context: &'static str) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(ErrorCode::ResourceMemory, format!("{context}: {cause}"))
    })?;
    Ok(values)
}

fn try_filled_vec<T: Clone>(length: usize, value: T, context: &'static str) -> Result<Vec<T>> {
    let mut values = try_vec_with_capacity(length, context)?;
    values.resize(length, value);
    Ok(values)
}

fn overflow(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

pub(crate) fn validate_graph_k(k: u8) -> Result<()> {
    if !(MIN_GRAPH_K..=MAX_GRAPH_K).contains(&k) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!("k must be in {MIN_GRAPH_K}..={MAX_GRAPH_K}, received {k}"),
        ));
    }
    Ok(())
}

fn is_reverse_complement_palindrome(code: u128, length: u8) -> bool {
    code == reverse_complement_validated(code, length)
}

fn u64_from_usize(value: usize, label: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            format!("{label} does not fit in u64"),
        )
    })
}

fn invariant(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MEMORY_BUDGET: u64 = 64 << 20;

    fn encode(sequence: &[u8]) -> u128 {
        sequence.iter().fold(0u128, |code, base| {
            let bits = match base {
                b'A' => 0,
                b'C' => 1,
                b'G' => 2,
                b'T' => 3,
                _ => panic!("test sequence must be ACGT"),
            };
            (code << 2) | bits
        })
    }

    fn retained(k: u8, sequences: &[(&[u8], u64)]) -> Vec<KmerCount> {
        let mut records = sequences
            .iter()
            .map(|(sequence, support)| {
                assert_eq!(sequence.len(), usize::from(k));
                let forward = encode(sequence);
                KmerCount {
                    key: forward.min(reverse_complement_validated(forward, k)),
                    support: *support,
                }
            })
            .collect::<Vec<_>>();
        records.sort_unstable();
        records.dedup_by_key(|record| record.key);
        records
    }

    #[test]
    fn creates_one_or_two_views_without_duplicating_support() {
        let records = retained(4, &[(b"AATT", 7), (b"ATTG", 11)]);
        let graph = ExactGraph::from_sorted_retained(4, &records, TEST_MEMORY_BUDGET).unwrap();
        assert_eq!(graph.stats().canonical_kmers, 2);
        assert_eq!(graph.stats().self_reverse_complement_keys, 1);
        assert_eq!(graph.stats().oriented_handles, 3);
        assert_eq!(graph.stats().total_support, 18);
        assert_eq!(
            graph
                .handles()
                .iter()
                .map(|handle| handle.spelling())
                .collect::<Vec<_>>(),
            vec![encode(b"AATT"), encode(b"ATTG"), encode(b"CAAT")]
        );
    }

    #[test]
    fn degree_oracle_counts_distinct_view_identities() {
        let records = retained(3, &[(b"AAC", 9), (b"AAG", 2), (b"ACA", 5), (b"CAA", 4)]);
        let graph = ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
        for node in graph.node_codes() {
            let incoming = (0..graph.handles().len())
                .filter(|index| graph.handle_target(*index) == Some(node))
                .count();
            let outgoing = (0..graph.handles().len())
                .filter(|index| graph.handle_source(*index) == Some(node))
                .count();
            assert_eq!(usize::from(graph.degree(node).incoming), incoming);
            assert_eq!(usize::from(graph.degree(node).outgoing), outgoing);
        }
    }

    #[test]
    fn palindromic_node_is_boundary_even_with_unit_degree() {
        let records = retained(3, &[(b"ATA", 1)]);
        let graph = ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
        let palindromic = encode(b"AT");
        assert_eq!(
            graph.degree(palindromic),
            NodeDegree {
                incoming: 1,
                outgoing: 1,
            }
        );
        assert!(graph.is_palindromic_node(palindromic));
        assert!(graph.is_boundary(palindromic));
    }

    #[test]
    fn self_reverse_complement_handle_makes_both_endpoints_boundaries() {
        // AATT is self-reverse-complementary. CAAT -> AATT -> ATTG would
        // otherwise look one-in/one-out at both incident literal nodes and
        // compact the two orientations of the ATTG/CAAT backing key together.
        let records = retained(4, &[(b"AATT", 7), (b"ATTG", 11)]);
        let graph = ExactGraph::from_sorted_retained(4, &records, TEST_MEMORY_BUDGET).unwrap();
        let source = encode(b"AAT");
        let target = encode(b"ATT");

        for node in [source, target] {
            assert_eq!(
                graph.degree(node),
                NodeDegree {
                    incoming: 1,
                    outgoing: 1,
                }
            );
            assert!(!graph.is_palindromic_node(node));
            assert!(graph.is_boundary(node));
        }
    }

    #[test]
    fn rejects_noncanonical_unsorted_duplicate_and_zero_support_records() {
        let noncanonical = vec![KmerCount {
            key: encode(b"TTT"),
            support: 1,
        }];
        assert_eq!(
            ExactGraph::from_sorted_retained(3, &noncanonical, TEST_MEMORY_BUDGET)
                .unwrap_err()
                .code(),
            ErrorCode::InternalInvariant
        );

        let duplicate = vec![
            KmerCount {
                key: encode(b"AAA"),
                support: 1,
            },
            KmerCount {
                key: encode(b"AAA"),
                support: 1,
            },
        ];
        assert!(ExactGraph::from_sorted_retained(3, &duplicate, TEST_MEMORY_BUDGET).is_err());

        let unsorted = vec![
            KmerCount {
                key: encode(b"AAC"),
                support: 1,
            },
            KmerCount {
                key: encode(b"AAA"),
                support: 1,
            },
        ];
        assert!(ExactGraph::from_sorted_retained(3, &unsorted, TEST_MEMORY_BUDGET).is_err());

        let zero = vec![KmerCount {
            key: encode(b"AAA"),
            support: 0,
        }];
        assert!(ExactGraph::from_sorted_retained(3, &zero, TEST_MEMORY_BUDGET).is_err());
    }

    #[test]
    fn graph_construction_rejects_a_budget_below_its_checked_allocation_plan() {
        let records = retained(3, &[(b"AAC", 1), (b"AAG", 2), (b"CAA", 3)]);
        let error = ExactGraph::from_sorted_retained_owned(3, records.clone(), 1).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);

        let graph = ExactGraph::from_sorted_retained_owned(3, records, TEST_MEMORY_BUDGET).unwrap();
        assert!(graph.accounted_allocation_bytes() <= TEST_MEMORY_BUDGET);
    }

    #[test]
    fn ten_million_key_plan_exceeds_the_default_budget_without_allocating() {
        let retained = 10_000_000usize;
        let oriented = retained.checked_mul(2).unwrap();
        let nodes = oriented.checked_mul(2).unwrap();
        let plan = graph_allocation_plan(retained, retained, oriented, nodes).unwrap();
        assert!(plan.peak_bytes > 512 << 20);
    }

    #[test]
    fn exhaustive_k3_single_and_pair_subsets_satisfy_rc_and_degree_oracles() {
        let mut keys = Vec::new();
        for spelling in 0..(1u128 << 6) {
            let canonical = spelling.min(reverse_complement_validated(spelling, 3));
            if canonical == spelling {
                keys.push(canonical);
            }
        }
        keys.sort_unstable();
        keys.dedup();

        for left in 0..keys.len() {
            for right in left..keys.len() {
                let records = if left == right {
                    vec![KmerCount {
                        key: keys[left],
                        support: 1,
                    }]
                } else {
                    vec![
                        KmerCount {
                            key: keys[left],
                            support: 1,
                        },
                        KmerCount {
                            key: keys[right],
                            support: 2,
                        },
                    ]
                };
                let graph =
                    ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
                assert_eq!(graph.stats().canonical_kmers, records.len() as u64);
                assert_eq!(
                    graph.stats().oriented_handles,
                    2 * records.len() as u64 - graph.stats().self_reverse_complement_keys
                );
                for (index, handle) in graph.handles().iter().copied().enumerate() {
                    let mate = graph.handle(graph.mate_index(index).unwrap()).unwrap();
                    assert_eq!(
                        mate.spelling(),
                        reverse_complement_validated(handle.spelling(), 3)
                    );
                    assert_eq!(
                        graph.mate_index(graph.mate_index(index).unwrap()),
                        Some(index)
                    );
                }
            }
        }
    }
}
