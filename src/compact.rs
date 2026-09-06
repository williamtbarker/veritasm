//! Deterministic conservative non-branching compaction and GFA link derivation.
//!
//! In addition to ordinary degree boundaries, compaction stops at
//! self-complemental nodes and at both endpoints of self-complemental k-mer
//! handles. These fixed-point boundaries prevent one emitted walk from using
//! both orientations of one backing canonical key.

use crate::error::{ErrorCode, Result, VeritasmError};
use crate::graph::{
    bytes_for, checked_sum_bytes, conservative_allocation_bytes, ensure_phase_budget,
    try_vec_with_capacity, ExactGraph,
};
use crate::model::{AvailabilityU64, GraphLink, Topology, Unitig};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CompactionStats {
    pub raw_walks: u64,
    pub raw_linear_walks: u64,
    pub raw_closed_walks: u64,
    pub unitigs: u64,
    pub linear_unitigs: u64,
    pub closed_graph_walks: u64,
    pub graph_links: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub unitigs: Vec<Unitig>,
    pub links: Vec<GraphLink>,
    pub stats: CompactionStats,
}

#[derive(Debug, Clone)]
struct RawWalk {
    handles: Vec<usize>,
    topology: Topology,
}

impl RawWalk {
    fn source(&self, graph: &ExactGraph) -> Result<u128> {
        let first = self
            .handles
            .first()
            .copied()
            .ok_or_else(|| invariant("raw walk is empty"))?;
        graph
            .handle_source(first)
            .ok_or_else(|| invariant("raw walk has an out-of-range first handle"))
    }

    fn target(&self, graph: &ExactGraph) -> Result<u128> {
        let last = self
            .handles
            .last()
            .copied()
            .ok_or_else(|| invariant("raw walk is empty"))?;
        graph
            .handle_target(last)
            .ok_or_else(|| invariant("raw walk has an out-of-range last handle"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OrbitKey {
    topology: Topology,
    representative: Vec<usize>,
}

#[derive(Debug, Default)]
struct OrbitBuilder {
    raw_indices: Vec<usize>,
    canonical_indices: BTreeSet<usize>,
}

#[derive(Debug)]
struct EmittedOrbit {
    key: OrbitKey,
    raw_indices: Vec<usize>,
    canonical_indices: Vec<usize>,
    unitig: Unitig,
}

// These allowances cover heap metadata not represented by `size_of` for the
// standard-library containers used below. They are intentionally generous for
// the Rust 1.85 implementations tested by this project; they are deterministic
// admission constants, not statements about a particular allocator's RSS.
const NESTED_ALLOCATION_ALLOWANCE: u64 = 64;
const BTREE_ENTRY_ALLOWANCE: u64 = 192;
const UNITIG_ID_HEAP_BYTES: u64 = 68;
const SHA256_HEX_HEAP_BYTES: u64 = 64;

fn ensure_compaction_extraction_budget(graph: &ExactGraph, budget: u64) -> Result<()> {
    let handles = graph.handles().len();
    // A growing per-walk Vec can have a small minimum allocation. Six handle
    // slots per graph handle bounds 2*H amortized capacity plus 4 slots for as
    // many as H one-edge walks. The actual capacities are checked again after
    // extraction, before any tree is constructed.
    let nested_handle_slots = checked_count_mul(handles, 6, "raw-walk handle capacity")?;
    let additional_payload = checked_sum_bytes(
        &[
            bytes_for::<bool>(handles, "raw-walk visited flags")?,
            bytes_for::<RawWalk>(handles, "raw-walk records")?,
            bytes_for::<usize>(nested_handle_slots, "raw-walk handle storage")?,
            per_item_bytes(
                handles,
                NESTED_ALLOCATION_ALLOWANCE,
                "raw-walk nested allocation allowance",
            )?,
        ],
        "raw-walk extraction allocation",
    )?;
    let additional = conservative_allocation_bytes(additional_payload, 2)?;
    let required = checked_sum_bytes(
        &[graph.accounted_allocation_bytes(), additional],
        "graph plus raw-walk extraction allocation",
    )?;
    ensure_phase_budget(required, budget, "graph compaction extraction")
}

fn ensure_compaction_budget(
    graph: &ExactGraph,
    raw_walks: &[RawWalk],
    raw_walk_capacity: usize,
    budget: u64,
) -> Result<()> {
    let handles = graph.handles().len();
    let canonical = graph.retained().len();
    let walks = raw_walks.len();
    let unitigs = walks.min(canonical);
    let closed = raw_walks
        .iter()
        .filter(|walk| walk.topology == Topology::ClosedGraphWalk)
        .count();

    let raw_handle_capacity = raw_walks.iter().try_fold(0usize, |sum, walk| {
        sum.checked_add(walk.handles.capacity())
            .ok_or_else(|| overflow("raw-walk handle capacities overflow"))
    })?;
    let raw_payload = checked_sum_bytes(
        &[
            bytes_for::<RawWalk>(raw_walk_capacity, "resident raw-walk records")?,
            bytes_for::<usize>(raw_handle_capacity, "resident raw-walk handles")?,
            per_item_bytes(
                walks,
                NESTED_ALLOCATION_ALLOWANCE,
                "resident raw-walk allocation allowance",
            )?,
        ],
        "resident raw-walk allocation",
    )?;

    // Stored orbit representatives total at most H slots. Three further H
    // terms conservatively cover the forward rotation, reverse-complement,
    // reverse rotation, and final selected copy while a closed key is admitted
    // into the tree (the stored total and current walk share that bound).
    let representative_slots = checked_count_mul(handles, 4, "orbit representative slots")?;
    let orbit_tree_entry = checked_sum_bytes(
        &[
            u64::try_from(std::mem::size_of::<OrbitKey>())
                .map_err(|_| overflow("orbit-key size does not fit u64"))?,
            u64::try_from(std::mem::size_of::<OrbitBuilder>())
                .map_err(|_| overflow("orbit-builder size does not fit u64"))?,
            BTREE_ENTRY_ALLOWANCE,
        ],
        "orbit-tree entry size",
    )?;
    let orbit_raw_slots = checked_count_add(
        checked_count_mul(walks, 2, "orbit raw-index capacity")?,
        checked_count_mul(unitigs, 4, "orbit raw-index minimum capacity")?,
        "orbit raw-index capacity",
    )?;
    let canonical_tree_entry = checked_sum_bytes(
        &[
            u64::try_from(std::mem::size_of::<usize>())
                .map_err(|_| overflow("canonical-index size does not fit u64"))?,
            BTREE_ENTRY_ALLOWANCE,
        ],
        "canonical-index tree entry size",
    )?;
    let orbit_payload = checked_sum_bytes(
        &[
            bytes_for::<usize>(representative_slots, "orbit representatives")?,
            bytes_for::<usize>(orbit_raw_slots, "orbit raw indices")?,
            per_item_bytes(unitigs, orbit_tree_entry, "orbit tree")?,
            per_item_bytes(canonical, canonical_tree_entry, "canonical-index trees")?,
            per_item_bytes(
                checked_count_mul(unitigs, 3, "orbit nested allocation count")?,
                NESTED_ALLOCATION_ALLOWANCE,
                "orbit nested allocations",
            )?,
        ],
        "unitig-orbit allocation",
    )?;

    let sequence_bytes = checked_count_add(
        handles,
        checked_count_mul(
            unitigs,
            usize::from(graph.k() - 1),
            "unitig repeated-context bytes",
        )?,
        "assembled sequence bytes",
    )?;
    let emitted_payload = checked_sum_bytes(
        &[
            bytes_for::<EmittedOrbit>(unitigs, "emitted orbit records")?,
            bytes_for::<usize>(handles, "emitted orbit representatives")?,
            bytes_for::<usize>(orbit_raw_slots, "emitted raw indices")?,
            bytes_for::<usize>(canonical, "emitted canonical indices")?,
            bytes_for::<u8>(sequence_bytes, "assembled sequences")?,
            bytes_for::<u64>(canonical, "support summary workspace")?,
            per_item_bytes(unitigs, UNITIG_ID_HEAP_BYTES, "unitig identifiers")?,
            per_item_bytes(unitigs, SHA256_HEX_HEAP_BYTES, "unitig sequence digests")?,
            per_item_bytes(unitigs, BTREE_ENTRY_ALLOWANCE, "unitig identity tree")?,
            per_item_bytes(
                checked_count_mul(unitigs, 5, "emitted nested allocation count")?,
                NESTED_ALLOCATION_ALLOWANCE,
                "emitted nested allocations",
            )?,
        ],
        "emitted unitig allocation",
    )?;

    let owner_payload = checked_sum_bytes(
        &[
            bytes_for::<Option<usize>>(walks, "raw-walk ownership table")?,
            bytes_for::<Option<usize>>(canonical, "canonical-key ownership table")?,
        ],
        "compaction conservation tables",
    )?;
    let mapping_entries = checked_count_mul(walks, 2, "raw-walk orientation mappings")?;
    let mapping_payload = checked_sum_bytes(
        &[
            bytes_for::<Vec<(usize, char)>>(walks, "raw-walk mapping vectors")?,
            bytes_for::<(usize, char)>(mapping_entries, "raw-walk orientation mappings")?,
            per_item_bytes(
                walks,
                NESTED_ALLOCATION_ALLOWANCE,
                "mapping nested allocations",
            )?,
        ],
        "raw-walk mapping allocation",
    )?;

    let transition_count = graph.node_codes().try_fold(0usize, |total, node| {
        if !graph.is_boundary(node) {
            return Ok(total);
        }
        let degree = graph.degree(node);
        let transitions = usize::from(degree.incoming)
            .checked_mul(usize::from(degree.outgoing))
            .ok_or_else(|| overflow("boundary transition count overflow"))?;
        total
            .checked_add(transitions)
            .ok_or_else(|| overflow("boundary transition count overflow"))
    })?;
    // Each raw walk has at most two representations (+ and -), producing at
    // most four emitted relations for one literal incoming/outgoing pair.
    let link_bound = checked_count_add(
        checked_count_mul(transition_count, 4, "graph-link candidate bound")?,
        closed,
        "graph-link candidate bound",
    )?;
    let endpoint_map_entries = checked_count_mul(walks, 2, "walk endpoint map entries")?;
    let endpoint_index_slots = checked_count_mul(walks, 12, "walk endpoint index capacity")?;
    let link_tree_item = checked_sum_bytes(
        &[
            u64::try_from(std::mem::size_of::<GraphLink>())
                .map_err(|_| overflow("graph-link size does not fit u64"))?,
            UNITIG_ID_HEAP_BYTES
                .checked_mul(2)
                .ok_or_else(|| overflow("graph-link identifier allowance overflow"))?,
            BTREE_ENTRY_ALLOWANCE,
        ],
        "graph-link tree item size",
    )?;
    let link_payload = checked_sum_bytes(
        &[
            per_item_bytes(unitigs, BTREE_ENTRY_ALLOWANCE, "segment index tree")?,
            per_item_bytes(
                endpoint_map_entries,
                BTREE_ENTRY_ALLOWANCE,
                "walk endpoint trees",
            )?,
            bytes_for::<usize>(endpoint_index_slots, "walk endpoint indices")?,
            per_item_bytes(
                endpoint_map_entries,
                NESTED_ALLOCATION_ALLOWANCE,
                "walk endpoint nested allocations",
            )?,
            per_item_bytes(link_bound, link_tree_item, "graph-link tree")?,
            bytes_for::<GraphLink>(link_bound, "final graph-link vector")?,
            UNITIG_ID_HEAP_BYTES
                .checked_mul(2)
                .ok_or_else(|| overflow("temporary graph-link identifiers overflow"))?,
        ],
        "graph-link derivation allocation",
    )?;

    let emission_peak = checked_sum_bytes(
        &[raw_payload, orbit_payload, emitted_payload],
        "orbit emission peak",
    )?;
    let validation_peak = checked_sum_bytes(
        &[raw_payload, emitted_payload, owner_payload],
        "compaction validation peak",
    )?;
    let linking_peak = checked_sum_bytes(
        &[raw_payload, emitted_payload, mapping_payload, link_payload],
        "graph-link derivation peak",
    )?;
    let output_peak = checked_sum_bytes(
        &[
            emitted_payload,
            bytes_for::<Unitig>(unitigs, "final unitig vector")?,
            bytes_for::<GraphLink>(link_bound, "resident final graph links")?,
        ],
        "compaction output peak",
    )?;
    let additional_payload = emission_peak
        .max(validation_peak)
        .max(linking_peak)
        .max(output_peak);
    let additional = conservative_allocation_bytes(additional_payload, 16)?;
    let required = checked_sum_bytes(
        &[graph.accounted_allocation_bytes(), additional],
        "graph plus compaction allocation",
    )?;
    ensure_phase_budget(required, budget, "graph compaction")
}

fn checked_count_mul(left: usize, right: usize, label: &'static str) -> Result<usize> {
    left.checked_mul(right)
        .ok_or_else(|| overflow(format!("{label} overflow")))
}

fn checked_count_add(left: usize, right: usize, label: &'static str) -> Result<usize> {
    left.checked_add(right)
        .ok_or_else(|| overflow(format!("{label} overflow")))
}

fn per_item_bytes(count: usize, bytes: u64, label: &'static str) -> Result<u64> {
    u64::try_from(count)
        .map_err(|_| overflow(format!("{label} count does not fit u64")))?
        .checked_mul(bytes)
        .ok_or_else(|| overflow(format!("{label} byte estimate overflow")))
}

/// Compact every oriented view, collapse reverse-complement orbits, and derive
/// graph-only GFA links. No post-compaction length filter is applied.
///
/// `phase_memory_budget_bytes` bounds a conservative estimate of the graph's
/// resident owned allocations plus allocations owned by compaction. The
/// estimate is based on `size_of` and checked cardinality bounds. It includes
/// explicit allowances for nested vectors and `BTreeMap`/`BTreeSet` nodes, but
/// is not measured RSS and excludes caller-owned data, stacks, and allocator
/// internals. Standard library trees have no fallible insertion API, so their
/// complete worst-case footprint is admitted before the first insertion.
pub fn compact_graph(
    graph: &ExactGraph,
    phase_memory_budget_bytes: u64,
) -> Result<CompactionResult> {
    ensure_compaction_extraction_budget(graph, phase_memory_budget_bytes)?;
    let raw_walks = extract_raw_walks(graph)?;
    ensure_compaction_budget(
        graph,
        &raw_walks,
        raw_walks.capacity(),
        phase_memory_budget_bytes,
    )?;
    let mut orbits: BTreeMap<OrbitKey, OrbitBuilder> = BTreeMap::new();

    for (raw_index, raw) in raw_walks.iter().enumerate() {
        let representative = match raw.topology {
            Topology::Linear => canonical_linear_walk(graph, &raw.handles)?,
            Topology::ClosedGraphWalk => canonical_closed_walk(graph, &raw.handles)?,
        };
        let key = OrbitKey {
            topology: raw.topology,
            representative,
        };
        let orbit = orbits.entry(key).or_default();
        try_push(
            &mut orbit.raw_indices,
            raw_index,
            "grow raw-walk orbit membership",
        )?;
        for handle_index in &raw.handles {
            let handle = graph
                .handle(*handle_index)
                .ok_or_else(|| invariant("raw walk references an out-of-range handle"))?;
            orbit.canonical_indices.insert(handle.canonical_index());
        }
    }

    let mut emitted = try_vec_with_capacity(orbits.len(), "allocate emitted unitig orbits")?;
    for (key, orbit) in orbits {
        try_push(
            &mut emitted,
            emit_orbit(graph, key, orbit)?,
            "grow emitted unitig orbits",
        )?;
    }
    validate_orbit_conservation(graph, &raw_walks, &emitted)?;

    emitted.sort_by(|left, right| {
        right
            .unitig
            .sequence
            .len()
            .cmp(&left.unitig.sequence.len())
            .then_with(|| left.unitig.sequence.cmp(&right.unitig.sequence))
            .then_with(|| left.unitig.id.cmp(&right.unitig.id))
    });
    validate_identity_uniqueness(&emitted)?;

    let raw_representations = map_linear_raw_walks(graph, &raw_walks, &emitted)?;
    let links = derive_links(graph, &raw_walks, &emitted, &raw_representations)?;
    let raw_linear_walks = raw_walks
        .iter()
        .filter(|walk| walk.topology == Topology::Linear)
        .count();
    let raw_closed_walks = raw_walks.len().saturating_sub(raw_linear_walks);
    let linear_unitigs = emitted
        .iter()
        .filter(|orbit| orbit.unitig.topology == Topology::Linear)
        .count();
    let closed_graph_walks = emitted.len().saturating_sub(linear_unitigs);
    let stats = CompactionStats {
        raw_walks: u64_count(raw_walks.len(), "raw walk count")?,
        raw_linear_walks: u64_count(raw_linear_walks, "raw linear walk count")?,
        raw_closed_walks: u64_count(raw_closed_walks, "raw closed walk count")?,
        unitigs: u64_count(emitted.len(), "unitig count")?,
        linear_unitigs: u64_count(linear_unitigs, "linear unitig count")?,
        closed_graph_walks: u64_count(closed_graph_walks, "closed graph walk count")?,
        graph_links: u64_count(links.len(), "graph link count")?,
    };
    drop(raw_representations);
    drop(raw_walks);
    let mut unitigs = try_vec_with_capacity(emitted.len(), "allocate final unitig vector")?;
    for orbit in emitted {
        try_push(&mut unitigs, orbit.unitig, "grow final unitig vector")?;
    }
    Ok(CompactionResult {
        unitigs,
        links,
        stats,
    })
}

fn extract_raw_walks(graph: &ExactGraph) -> Result<Vec<RawWalk>> {
    let mut used = try_vec_with_capacity(graph.handles().len(), "allocate raw-walk visited flags")?;
    used.resize(graph.handles().len(), false);
    let mut walks = try_vec_with_capacity(graph.handles().len(), "allocate raw-walk record table")?;

    // Boundary-started phase. BTreeMap node order and handle order are both
    // frozen by ExactGraph.
    for node in graph.node_codes() {
        if !graph.is_boundary(node) {
            continue;
        }
        for first in graph.outgoing(node).iter() {
            if used[first] {
                continue;
            }
            let mut handles = Vec::new();
            let mut current = first;
            loop {
                if used[current] {
                    return Err(invariant(
                        "boundary-started walk attempted to enter a used oriented handle",
                    ));
                }
                used[current] = true;
                try_push(&mut handles, current, "grow boundary-started raw walk")?;
                let target = graph
                    .handle_target(current)
                    .ok_or_else(|| invariant("walk handle is out of range"))?;
                if graph.is_boundary(target) {
                    break;
                }
                let degree = graph.degree(target);
                if degree.incoming != 1 || degree.outgoing != 1 {
                    return Err(invariant(
                        "non-boundary traversal node does not have degree one-in/one-out",
                    ));
                }
                let outgoing = graph.outgoing(target);
                if outgoing.len() != 1 {
                    return Err(invariant(
                        "non-boundary traversal node lacks its unique outgoing handle",
                    ));
                }
                let next = outgoing
                    .first()
                    .ok_or_else(|| invariant("non-boundary node lost its outgoing handle"))?;
                if used[next] {
                    return Err(invariant(
                        "boundary-started walk would stop at a used handle before a boundary",
                    ));
                }
                current = next;
            }
            try_push(
                &mut walks,
                RawWalk {
                    handles,
                    topology: Topology::Linear,
                },
                "grow raw-walk record table",
            )?;
        }
    }

    // Every residual component must be an isolated directed one-in/one-out
    // cycle. Handle indices are the exact specified traversal order.
    for first in 0..graph.handles().len() {
        if used[first] {
            continue;
        }
        let first_source = graph
            .handle_source(first)
            .ok_or_else(|| invariant("residual handle is out of range"))?;
        if graph.is_boundary(first_source) {
            return Err(invariant(
                "a boundary-sourced handle remained after the boundary traversal phase",
            ));
        }

        let mut handles = Vec::new();
        let mut current = first;
        loop {
            if used[current] {
                return Err(invariant(
                    "residual traversal entered a previously used handle",
                ));
            }
            used[current] = true;
            try_push(&mut handles, current, "grow residual raw walk")?;
            let target = graph
                .handle_target(current)
                .ok_or_else(|| invariant("residual walk handle is out of range"))?;
            if graph.is_boundary(target) {
                return Err(invariant(
                    "residual traversal reached a boundary rather than closing a cycle",
                ));
            }
            let outgoing = graph.outgoing(target);
            if outgoing.len() != 1 || graph.degree(target).incoming != 1 {
                return Err(invariant("residual component is not one-in/one-out"));
            }
            let next = outgoing
                .first()
                .ok_or_else(|| invariant("residual node lost its outgoing handle"))?;
            if next == first {
                break;
            }
            if used[next] {
                return Err(invariant(
                    "residual traversal joined a different used component",
                ));
            }
            current = next;
            if handles.len() > graph.handles().len() {
                return Err(invariant("residual traversal exceeded the handle count"));
            }
        }
        try_push(
            &mut walks,
            RawWalk {
                handles,
                topology: Topology::ClosedGraphWalk,
            },
            "grow raw-walk record table",
        )?;
    }

    if used.iter().any(|value| !*value) {
        return Err(invariant("compaction did not visit every oriented handle"));
    }
    let visited = walks.iter().try_fold(0usize, |total, walk| {
        total
            .checked_add(walk.handles.len())
            .ok_or_else(|| invariant("visited handle count overflow"))
    })?;
    if visited != graph.handles().len() {
        return Err(invariant(format!(
            "raw walks contain {visited} handles but graph contains {}",
            graph.handles().len()
        )));
    }
    Ok(walks)
}

fn canonical_linear_walk(graph: &ExactGraph, handles: &[usize]) -> Result<Vec<usize>> {
    let reverse = reverse_complement_walk(graph, handles)?;
    copy_indices(
        handles.min(reverse.as_slice()),
        "allocate canonical linear walk",
    )
}

fn canonical_closed_walk(graph: &ExactGraph, handles: &[usize]) -> Result<Vec<usize>> {
    if handles.is_empty() {
        return Err(invariant("cannot canonicalize an empty closed walk"));
    }
    let forward = minimum_rotation(handles)?;
    let reverse = reverse_complement_walk(graph, handles)?;
    let reverse = minimum_rotation(&reverse)?;
    copy_indices(
        forward.as_slice().min(reverse.as_slice()),
        "allocate canonical closed walk",
    )
}

fn reverse_complement_walk(graph: &ExactGraph, handles: &[usize]) -> Result<Vec<usize>> {
    let mut reverse = try_vec_with_capacity(handles.len(), "allocate reverse-complement walk")?;
    for index in handles.iter().rev() {
        try_push(
            &mut reverse,
            graph
                .mate_index(*index)
                .ok_or_else(|| invariant("walk references an out-of-range mate handle"))?,
            "grow reverse-complement walk",
        )?;
    }
    Ok(reverse)
}

fn emit_orbit(graph: &ExactGraph, key: OrbitKey, orbit: OrbitBuilder) -> Result<EmittedOrbit> {
    if key.representative.is_empty() {
        return Err(invariant("unitig representative is empty"));
    }
    if orbit.raw_indices.is_empty() {
        return Err(invariant("unitig orbit has no raw walk"));
    }
    if key.representative.len() != orbit.canonical_indices.len() {
        return Err(invariant(
            "unitig representative repeats a backing canonical key",
        ));
    }

    let sequence = spell_walk(graph, &key.representative, key.topology)?;
    let mut canonical_indices = try_vec_with_capacity(
        orbit.canonical_indices.len(),
        "allocate unitig canonical-index list",
    )?;
    for &handle_index in &key.representative {
        let handle = graph
            .handle(handle_index)
            .ok_or_else(|| invariant("unitig representative contains an out-of-range handle"))?;
        try_push(
            &mut canonical_indices,
            handle.canonical_index(),
            "grow unitig canonical-index list",
        )?;
    }
    canonical_indices.sort_unstable();
    if canonical_indices
        .windows(2)
        .any(|window| window[0] == window[1])
        || !canonical_indices
            .iter()
            .copied()
            .eq(orbit.canonical_indices.iter().copied())
    {
        return Err(invariant(
            "unitig representative does not contain each orbit canonical key exactly once",
        ));
    }
    if canonical_indices.is_empty() {
        return Err(invariant("unitig orbit has no backing canonical key"));
    }
    let mut supports = try_vec_with_capacity(
        canonical_indices.len(),
        "allocate unitig support summary workspace",
    )?;
    for index in &canonical_indices {
        try_push(
            &mut supports,
            graph
                .support_for_canonical_index(*index)
                .ok_or_else(|| invariant("unitig references an out-of-range canonical key"))?,
            "grow unitig support summary workspace",
        )?;
    }
    supports.sort_unstable();
    let minimum_support = supports[0];
    let lower_median_support = supports[(supports.len() - 1) / 2];
    let maximum_support = supports[supports.len() - 1];
    let sequence_sha256 = sha256_lower(&sequence)?;
    let id = unitig_id(graph.k(), key.topology, &sequence)?;
    let (unavailable_reason, placement_enumeration_status) = match key.topology {
        Topology::Linear => ("remap_disabled", "unavailable_remap_disabled"),
        Topology::ClosedGraphWalk => (
            "closed_walk_audit_unsupported",
            "unavailable_closed_walk_audit_unsupported",
        ),
    };
    let unitig = Unitig {
        id,
        sequence,
        topology: key.topology,
        edge_steps: u64_count(key.representative.len(), "unitig edge-step count")?,
        canonical_kmers: u64_count(canonical_indices.len(), "unitig canonical-key count")?,
        minimum_support,
        lower_median_support,
        maximum_support,
        enumeration_complete_read_placements: AvailabilityU64::NotAvailable(unavailable_reason),
        single_group_read_instances: AvailabilityU64::NotAvailable(unavailable_reason),
        multi_group_read_instances_with_group: AvailabilityU64::NotAvailable(unavailable_reason),
        placement_enumeration_status,
        sequence_sha256,
    };
    Ok(EmittedOrbit {
        key,
        raw_indices: orbit.raw_indices,
        canonical_indices,
        unitig,
    })
}

fn spell_walk(graph: &ExactGraph, handles: &[usize], topology: Topology) -> Result<Vec<u8>> {
    let first = graph
        .handle(handles[0])
        .ok_or_else(|| invariant("representative starts with an out-of-range handle"))?;
    let mut sequence = crate::dna::decode_mer(first.spelling(), graph.k())?;
    sequence
        .try_reserve_exact(handles.len().saturating_sub(1))
        .map_err(|cause| memory_error(format!("reserve assembled unitig sequence: {cause}")))?;
    let mut previous_index = handles[0];
    for &index in &handles[1..] {
        let handle = graph
            .handle(index)
            .ok_or_else(|| invariant("representative contains an out-of-range handle"))?;
        if graph.handle_target(previous_index) != graph.handle_source(index) {
            return Err(invariant(
                "adjacent representative handles do not share their literal node",
            ));
        }
        sequence.push(base_byte(handle.spelling()));
        previous_index = index;
    }

    if topology == Topology::ClosedGraphWalk {
        if graph.handle_target(previous_index) != graph.handle_source(handles[0]) {
            return Err(invariant(
                "closed representative does not return to its start node",
            ));
        }
        let overlap = usize::from(graph.k() - 1);
        if sequence[..overlap] != sequence[sequence.len() - overlap..] {
            return Err(invariant(
                "closed representative lacks its repeated terminal k-1 context",
            ));
        }
    }
    Ok(sequence)
}

fn base_byte(spelling: u128) -> u8 {
    match spelling & 0b11 {
        0 => b'A',
        1 => b'C',
        2 => b'G',
        3 => b'T',
        _ => unreachable!("two-bit base"),
    }
}

fn validate_orbit_conservation(
    graph: &ExactGraph,
    raw_walks: &[RawWalk],
    emitted: &[EmittedOrbit],
) -> Result<()> {
    let mut raw_owner =
        try_vec_with_capacity(raw_walks.len(), "allocate raw-walk ownership table")?;
    raw_owner.resize(raw_walks.len(), None);
    let mut canonical_owner = try_vec_with_capacity(
        graph.retained().len(),
        "allocate canonical-key ownership table",
    )?;
    canonical_owner.resize(graph.retained().len(), None);
    let mut edge_steps = 0usize;

    for (orbit_index, orbit) in emitted.iter().enumerate() {
        if orbit.unitig.topology != orbit.key.topology
            || orbit.unitig.edge_steps
                != u64_count(orbit.key.representative.len(), "orbit edge-step count")?
            || orbit.unitig.canonical_kmers
                != u64_count(orbit.canonical_indices.len(), "orbit canonical-key count")?
            || orbit.unitig.edge_steps != orbit.unitig.canonical_kmers
        {
            return Err(invariant("emitted unitig metadata differs from its orbit"));
        }
        for &raw_index in &orbit.raw_indices {
            let raw = raw_walks
                .get(raw_index)
                .ok_or_else(|| invariant("orbit references an out-of-range raw walk"))?;
            if raw.topology != orbit.key.topology {
                return Err(invariant("one unitig orbit mixes raw-walk topologies"));
            }
            if raw_owner[raw_index].replace(orbit_index).is_some() {
                return Err(invariant("one raw walk belongs to multiple unitig orbits"));
            }
            edge_steps = edge_steps
                .checked_add(raw.handles.len())
                .ok_or_else(|| invariant("raw-walk edge-step sum overflow"))?;
        }
        for &canonical_index in &orbit.canonical_indices {
            let owner = canonical_owner
                .get_mut(canonical_index)
                .ok_or_else(|| invariant("orbit has an out-of-range canonical-key index"))?;
            if owner.replace(orbit_index).is_some() {
                return Err(invariant(
                    "one retained canonical key belongs to multiple unitig orbits",
                ));
            }
        }
    }

    if raw_owner.iter().any(Option::is_none) {
        return Err(invariant("one or more raw walks have no unitig orbit"));
    }
    if canonical_owner.iter().any(Option::is_none) {
        return Err(invariant(
            "one or more retained canonical keys have no unitig orbit",
        ));
    }
    if edge_steps != graph.handles().len() {
        return Err(invariant(
            "unitig raw-walk orbits do not conserve oriented handles",
        ));
    }
    Ok(())
}

fn validate_identity_uniqueness(emitted: &[EmittedOrbit]) -> Result<()> {
    let mut identities: BTreeMap<&str, (&[u8], Topology)> = BTreeMap::new();
    for orbit in emitted {
        if let Some((sequence, topology)) = identities.insert(
            &orbit.unitig.id,
            (&orbit.unitig.sequence, orbit.unitig.topology),
        ) {
            if sequence != orbit.unitig.sequence || topology != orbit.unitig.topology {
                return Err(invariant(
                    "unitig identity digest collision between unequal preimages",
                ));
            }
            return Err(invariant("duplicate emitted unitig identity"));
        }
    }
    Ok(())
}

fn map_linear_raw_walks(
    graph: &ExactGraph,
    raw_walks: &[RawWalk],
    emitted: &[EmittedOrbit],
) -> Result<Vec<Vec<(usize, char)>>> {
    let mut mapping =
        try_vec_with_capacity(raw_walks.len(), "allocate raw-walk representation table")?;
    mapping.resize_with(raw_walks.len(), Vec::new);
    for (segment_index, orbit) in emitted.iter().enumerate() {
        if orbit.key.topology != Topology::Linear {
            continue;
        }
        let reverse = reverse_complement_walk(graph, &orbit.key.representative)?;
        for &raw_index in &orbit.raw_indices {
            let raw = raw_walks
                .get(raw_index)
                .ok_or_else(|| invariant("orbit references an out-of-range raw walk"))?;
            if raw.handles == orbit.key.representative {
                try_push(
                    &mut mapping[raw_index],
                    (segment_index, '+'),
                    "grow raw-walk representation list",
                )?;
            }
            if raw.handles == reverse {
                try_push(
                    &mut mapping[raw_index],
                    (segment_index, '-'),
                    "grow raw-walk representation list",
                )?;
            }
            mapping[raw_index].sort_unstable();
            mapping[raw_index].dedup();
            if mapping[raw_index].is_empty() {
                return Err(invariant(
                    "linear raw walk matches neither orientation of its representative",
                ));
            }
        }
    }
    for (index, raw) in raw_walks.iter().enumerate() {
        if raw.topology == Topology::Linear && mapping[index].is_empty() {
            return Err(invariant("linear raw walk has no emitted orientation"));
        }
    }
    Ok(mapping)
}

fn derive_links(
    graph: &ExactGraph,
    raw_walks: &[RawWalk],
    emitted: &[EmittedOrbit],
    raw_representations: &[Vec<(usize, char)>],
) -> Result<Vec<GraphLink>> {
    let segment_indices = emitted
        .iter()
        .enumerate()
        .map(|(index, orbit)| (orbit.unitig.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut starts: BTreeMap<u128, Vec<usize>> = BTreeMap::new();
    let mut ends: BTreeMap<u128, Vec<usize>> = BTreeMap::new();
    for (raw_index, raw) in raw_walks.iter().enumerate() {
        if raw.topology != Topology::Linear {
            continue;
        }
        let start_indices = starts.entry(raw.source(graph)?).or_default();
        try_push(start_indices, raw_index, "grow raw-walk start-node index")?;
        let end_indices = ends.entry(raw.target(graph)?).or_default();
        try_push(end_indices, raw_index, "grow raw-walk end-node index")?;
    }

    let mut links = BTreeSet::new();
    for node in graph.node_codes() {
        if !graph.is_boundary(node) {
            continue;
        }
        let incoming = ends.get(&node).map_or(&[][..], Vec::as_slice);
        let outgoing = starts.get(&node).map_or(&[][..], Vec::as_slice);
        for &incoming_raw in incoming {
            for &outgoing_raw in outgoing {
                for &(from_index, from_orientation) in &raw_representations[incoming_raw] {
                    for &(to_index, to_orientation) in &raw_representations[outgoing_raw] {
                        let candidate = GraphLink {
                            from_segment: try_clone_string(
                                &emitted[from_index].unitig.id,
                                "allocate graph-link source identifier",
                            )?,
                            from_orientation,
                            to_segment: try_clone_string(
                                &emitted[to_index].unitig.id,
                                "allocate graph-link target identifier",
                            )?,
                            to_orientation,
                        };
                        validate_link_overlap(graph, emitted, &segment_indices, &candidate)?;
                        let canonical = canonical_link(candidate);
                        validate_link_overlap(graph, emitted, &segment_indices, &canonical)?;
                        links.insert(canonical);
                    }
                }
            }
        }
    }

    // A residual directed cycle has one explicit last-to-first relation. Its
    // canonical reverse-complement relation is the `-/-` self-link, so `+/+`
    // is always the typed minimum.
    for orbit in emitted {
        if orbit.key.topology == Topology::ClosedGraphWalk {
            let candidate = GraphLink {
                from_segment: try_clone_string(
                    &orbit.unitig.id,
                    "allocate closed graph-link source identifier",
                )?,
                from_orientation: '+',
                to_segment: try_clone_string(
                    &orbit.unitig.id,
                    "allocate closed graph-link target identifier",
                )?,
                to_orientation: '+',
            };
            validate_link_overlap(graph, emitted, &segment_indices, &candidate)?;
            let canonical = canonical_link(candidate);
            validate_link_overlap(graph, emitted, &segment_indices, &canonical)?;
            links.insert(canonical);
        }
    }
    let mut output = try_vec_with_capacity(links.len(), "allocate final graph-link vector")?;
    output.extend(links);
    Ok(output)
}

fn canonical_link(link: GraphLink) -> GraphLink {
    let reverse_key = (
        link.to_segment.as_str(),
        orientation_rank(flip_symbol(link.to_orientation)),
        link.from_segment.as_str(),
        orientation_rank(flip_symbol(link.from_orientation)),
    );
    if link_key(&link) <= reverse_key {
        link
    } else {
        GraphLink {
            from_segment: link.to_segment,
            from_orientation: flip_symbol(link.to_orientation),
            to_segment: link.from_segment,
            to_orientation: flip_symbol(link.from_orientation),
        }
    }
}

fn link_key(link: &GraphLink) -> (&str, u8, &str, u8) {
    (
        &link.from_segment,
        orientation_rank(link.from_orientation),
        &link.to_segment,
        orientation_rank(link.to_orientation),
    )
}

fn orientation_rank(orientation: char) -> u8 {
    match orientation {
        '+' => 0,
        '-' => 1,
        _ => 2,
    }
}

fn flip_symbol(orientation: char) -> char {
    match orientation {
        '+' => '-',
        '-' => '+',
        _ => orientation,
    }
}

fn validate_link_overlap(
    graph: &ExactGraph,
    emitted: &[EmittedOrbit],
    segment_indices: &BTreeMap<&str, usize>,
    link: &GraphLink,
) -> Result<()> {
    let from = segment_indices
        .get(link.from_segment.as_str())
        .and_then(|index| emitted.get(*index))
        .ok_or_else(|| invariant("graph link references a missing source segment"))?;
    let to = segment_indices
        .get(link.to_segment.as_str())
        .and_then(|index| emitted.get(*index))
        .ok_or_else(|| invariant("graph link references a missing target segment"))?;
    let overlap = usize::from(graph.k() - 1);
    let from_sequence = &from.unitig.sequence;
    let to_sequence = &to.unitig.sequence;
    if from_sequence.len() < overlap || to_sequence.len() < overlap {
        return Err(invariant(format!(
            "graph link {}{} -> {}{} lacks its exact k-1 overlap",
            link.from_segment, link.from_orientation, link.to_segment, link.to_orientation
        )));
    }
    for offset in 0..overlap {
        let from_base = oriented_base(
            from_sequence,
            link.from_orientation,
            from_sequence.len() - overlap + offset,
        )?;
        let to_base = oriented_base(to_sequence, link.to_orientation, offset)?;
        if from_base != to_base {
            return Err(invariant(format!(
                "graph link {}{} -> {}{} lacks its exact k-1 overlap",
                link.from_segment, link.from_orientation, link.to_segment, link.to_orientation
            )));
        }
    }
    Ok(())
}

fn oriented_base(sequence: &[u8], orientation: char, index: usize) -> Result<u8> {
    let base = match orientation {
        '+' => *sequence
            .get(index)
            .ok_or_else(|| invariant("oriented sequence index is out of range"))?,
        '-' => {
            let one_past = index
                .checked_add(1)
                .ok_or_else(|| invariant("reverse-oriented sequence index overflow"))?;
            let source_index = sequence
                .len()
                .checked_sub(one_past)
                .ok_or_else(|| invariant("reverse-oriented sequence index is out of range"))?;
            match sequence[source_index] {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => return Err(invariant("assembled sequence contains a non-ACGT byte")),
            }
        }
        _ => return Err(invariant("graph link has an invalid orientation symbol")),
    };
    if !matches!(base, b'A' | b'C' | b'G' | b'T') {
        return Err(invariant("assembled sequence contains a non-ACGT byte"));
    }
    Ok(base)
}

fn unitig_id(k: u8, topology: Topology, sequence: &[u8]) -> Result<String> {
    let sequence_length = u64_count(sequence.len(), "unitig sequence length")?;
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:unitig:v1\0");
    hasher.update([k]);
    hasher.update([topology.tag()]);
    hasher.update(sequence_length.to_le_bytes());
    hasher.update(sequence);
    let digest = lower_hex(&hasher.finalize())?;
    let mut id = String::new();
    id.try_reserve_exact(4usize.saturating_add(digest.len()))
        .map_err(|cause| memory_error(format!("allocate unitig identifier: {cause}")))?;
    id.push_str("utg-");
    id.push_str(&digest);
    Ok(id)
}

fn sha256_lower(bytes: &[u8]) -> Result<String> {
    lower_hex(&Sha256::digest(bytes))
}

fn lower_hex(bytes: &[u8]) -> Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let capacity = bytes
        .len()
        .checked_mul(2)
        .ok_or_else(|| overflow("hex output length overflow"))?;
    let mut output = String::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|cause| memory_error(format!("allocate hexadecimal digest: {cause}")))?;
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(output)
}

/// Booth's linear-time minimum rotation over exact oriented-handle ranks.
fn minimum_rotation(values: &[usize]) -> Result<Vec<usize>> {
    if values.len() < 2 {
        return copy_indices(values, "allocate minimum rotation");
    }
    let n = values.len();
    let mut left = 0usize;
    let mut right = 1usize;
    let mut offset = 0usize;
    while left < n && right < n && offset < n {
        let a = values[(left + offset) % n];
        let b = values[(right + offset) % n];
        if a == b {
            offset += 1;
            continue;
        }
        if a > b {
            left += offset + 1;
            if left <= right {
                left = right + 1;
            }
        } else {
            right += offset + 1;
            if right <= left {
                right = left + 1;
            }
        }
        offset = 0;
    }
    let start = left.min(right);
    let mut rotated = try_vec_with_capacity(values.len(), "allocate minimum rotation")?;
    rotated.extend_from_slice(&values[start..]);
    rotated.extend_from_slice(&values[..start]);
    Ok(rotated)
}

fn u64_count(value: usize, label: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            format!("{label} does not fit in u64"),
        )
    })
}

fn copy_indices(values: &[usize], context: &'static str) -> Result<Vec<usize>> {
    let mut copy = try_vec_with_capacity(values.len(), context)?;
    copy.extend_from_slice(values);
    Ok(copy)
}

fn try_push<T>(values: &mut Vec<T>, value: T, context: &'static str) -> Result<()> {
    values
        .try_reserve(1)
        .map_err(|cause| memory_error(format!("{context}: {cause}")))?;
    values.push(value);
    Ok(())
}

fn try_clone_string(value: &str, context: &'static str) -> Result<String> {
    let mut clone = String::new();
    clone
        .try_reserve_exact(value.len())
        .map_err(|cause| memory_error(format!("{context}: {cause}")))?;
    clone.push_str(value);
    Ok(clone)
}

fn memory_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn overflow(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn invariant(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dna::reverse_complement_validated;
    use crate::model::KmerCount;

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
        let mut records = BTreeMap::new();
        for (sequence, support) in sequences {
            let forward = encode(sequence);
            let canonical = forward.min(reverse_complement_validated(forward, k));
            records.insert(canonical, *support);
        }
        records
            .into_iter()
            .map(|(key, support)| KmerCount { key, support })
            .collect()
    }

    fn assemble(k: u8, sequences: &[(&[u8], u64)]) -> CompactionResult {
        let records = retained(k, sequences);
        let graph = ExactGraph::from_sorted_retained(k, &records, TEST_MEMORY_BUDGET).unwrap();
        compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap()
    }

    fn unitig_id_for_sequence<'a>(result: &'a CompactionResult, sequence: &[u8]) -> &'a str {
        result
            .unitigs
            .iter()
            .find(|unitig| unitig.sequence == sequence)
            .map(|unitig| unitig.id.as_str())
            .expect("fixture sequence must have one emitted unitig")
    }

    /// Independent test oracle for the frozen link/RC equivalence relation.
    fn oracle_canonical_link(
        from: &str,
        from_orientation: char,
        to: &str,
        to_orientation: char,
    ) -> GraphLink {
        let forward = GraphLink {
            from_segment: from.to_owned(),
            from_orientation,
            to_segment: to.to_owned(),
            to_orientation,
        };
        let reverse = GraphLink {
            from_segment: to.to_owned(),
            from_orientation: match to_orientation {
                '+' => '-',
                '-' => '+',
                _ => panic!("test link orientation must be + or -"),
            },
            to_segment: from.to_owned(),
            to_orientation: match from_orientation {
                '+' => '-',
                '-' => '+',
                _ => panic!("test link orientation must be + or -"),
            },
        };
        let rank = |orientation| if orientation == '+' { 0u8 } else { 1u8 };
        let forward_key = (
            forward.from_segment.as_str(),
            rank(forward.from_orientation),
            forward.to_segment.as_str(),
            rank(forward.to_orientation),
        );
        let reverse_key = (
            reverse.from_segment.as_str(),
            rank(reverse.from_orientation),
            reverse.to_segment.as_str(),
            rank(reverse.to_orientation),
        );
        if forward_key <= reverse_key {
            forward
        } else {
            reverse
        }
    }

    #[test]
    fn empty_graph_has_no_walks_unitigs_or_links() {
        let graph = ExactGraph::from_sorted_retained(5, &[], TEST_MEMORY_BUDGET).unwrap();
        let result = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
        assert!(result.unitigs.is_empty());
        assert!(result.links.is_empty());
        assert_eq!(result.stats, CompactionStats::default());
    }

    #[test]
    fn compaction_rejects_a_budget_without_room_beyond_the_resident_graph() {
        let records = retained(3, &[(b"CAA", 2), (b"AAC", 3), (b"AAG", 5)]);
        let graph = ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
        let error = compact_graph(&graph, graph.accounted_allocation_bytes()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn emits_one_reverse_complement_orbit_for_a_linear_path() {
        let result = assemble(5, &[(b"AACGC", 3), (b"ACGCT", 5), (b"CGCTA", 7)]);
        assert_eq!(result.unitigs.len(), 1);
        assert_eq!(result.unitigs[0].sequence, b"AACGCTA");
        assert_eq!(result.unitigs[0].edge_steps, 3);
        assert_eq!(result.unitigs[0].canonical_kmers, 3);
        assert_eq!(result.unitigs[0].minimum_support, 3);
        assert_eq!(result.unitigs[0].lower_median_support, 5);
        assert_eq!(result.unitigs[0].maximum_support, 7);
        assert_eq!(result.unitigs[0].topology, Topology::Linear);
    }

    #[test]
    fn self_rc_edge_is_a_conservative_compaction_barrier() {
        let result = assemble(4, &[(b"AATT", 13), (b"ATTG", 5)]);
        assert_eq!(result.unitigs.len(), 2);
        assert_eq!(
            result
                .unitigs
                .iter()
                .map(|unitig| unitig.sequence.as_slice())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([b"AATT".as_slice(), b"ATTG".as_slice()])
        );
        assert!(result
            .unitigs
            .iter()
            .all(|unitig| unitig.edge_steps == 1 && unitig.canonical_kmers == 1));
        assert_eq!(result.links.len(), 2);
    }

    #[test]
    fn palindromic_boundaries_retain_a_hairpin_link() {
        let result = assemble(3, &[(b"ATA", 4)]);
        assert_eq!(result.unitigs.len(), 1);
        assert_eq!(result.unitigs[0].sequence, b"ATA");
        assert_eq!(result.unitigs[0].topology, Topology::Linear);
        assert_eq!(result.links.len(), 2);
        assert!(result.links.iter().all(|link| {
            link.from_segment == result.unitigs[0].id && link.to_segment == result.unitigs[0].id
        }));
        assert_eq!(
            result
                .links
                .iter()
                .map(|link| (link.from_orientation, link.to_orientation))
                .collect::<Vec<_>>(),
            vec![('+', '-'), ('-', '+')]
        );
    }

    #[test]
    fn residual_cycle_has_repeated_context_and_canonical_self_link() {
        let result = assemble(3, &[(b"ACA", 2), (b"CAC", 9)]);
        assert_eq!(result.unitigs.len(), 1);
        assert_eq!(result.unitigs[0].sequence, b"ACAC");
        assert_eq!(result.unitigs[0].edge_steps, 2);
        assert_eq!(result.unitigs[0].canonical_kmers, 2);
        assert_eq!(result.unitigs[0].topology, Topology::ClosedGraphWalk);
        assert_eq!(result.links.len(), 1);
        assert_eq!(
            (
                result.links[0].from_orientation,
                result.links[0].to_orientation
            ),
            ('+', '+')
        );
    }

    #[test]
    fn branch_walks_conserve_views_and_keys_and_emit_all_transitions() {
        // CAA enters literal node AA, where AAC and AAG leave. The earlier
        // fixture had only the two outgoing arms; a source branch with no
        // incoming walk correctly has no end-to-start GFA transition.
        let records = retained(3, &[(b"CAA", 2), (b"AAC", 3), (b"AAG", 5)]);
        let graph = ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
        let result = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
        assert_eq!(
            result
                .unitigs
                .iter()
                .map(|unitig| unitig.canonical_kmers)
                .sum::<u64>(),
            graph.stats().canonical_kmers
        );
        assert_eq!(
            result.stats.raw_linear_walks + result.stats.raw_closed_walks,
            result.stats.raw_walks
        );

        let incoming = unitig_id_for_sequence(&result, b"CAA");
        let left_arm = unitig_id_for_sequence(&result, b"AAC");
        let right_arm = unitig_id_for_sequence(&result, b"AAG");
        let expected = BTreeSet::from([
            oracle_canonical_link(incoming, '+', left_arm, '+'),
            oracle_canonical_link(incoming, '+', right_arm, '+'),
        ]);
        let actual = result.links.iter().cloned().collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn unitig_identity_uses_the_frozen_complete_preimage() {
        let result = assemble(3, &[(b"AAC", 1)]);
        let unitig = &result.unitigs[0];
        let mut hasher = Sha256::new();
        hasher.update(b"veritasm:unitig:v1\0");
        hasher.update([3]);
        hasher.update([0]);
        hasher.update(3u64.to_le_bytes());
        hasher.update(b"AAC");
        assert_eq!(
            unitig.id,
            format!("utg-{}", lower_hex(&hasher.finalize()).unwrap())
        );
    }

    #[test]
    fn reverse_complement_input_spelling_has_identical_output() {
        let forward = assemble(5, &[(b"AACGC", 3), (b"ACGCT", 5), (b"CGCTA", 7)]);
        let reverse = assemble(5, &[(b"GCGTT", 3), (b"AGCGT", 5), (b"TAGCG", 7)]);
        assert_eq!(forward, reverse);
    }

    #[test]
    fn booth_rotation_handles_repeated_values() {
        assert_eq!(minimum_rotation(&[2, 1, 2, 1]).unwrap(), vec![1, 2, 1, 2]);
        assert_eq!(minimum_rotation(&[4, 4, 4]).unwrap(), vec![4, 4, 4]);
    }

    #[test]
    fn exhaustive_k3_single_and_pair_sets_compact_without_loss() {
        let mut keys = Vec::new();
        for spelling in 0..(1u128 << 6) {
            let canonical = spelling.min(reverse_complement_validated(spelling, 3));
            if spelling == canonical {
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
                let result = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
                assert_eq!(
                    result
                        .unitigs
                        .iter()
                        .map(|unitig| unitig.canonical_kmers)
                        .sum::<u64>(),
                    records.len() as u64
                );
                assert_eq!(
                    result.stats.raw_walks,
                    result.stats.raw_linear_walks + result.stats.raw_closed_walks
                );
            }
        }
    }
}
