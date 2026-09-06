# Experimental paired-end graph-path evidence

Status: isolated Rust substrate; not used by `veritasm assemble`, stable FASTA, or stable GFA.

`src/experimental/pair_path.rs` tests whether a supplied fragment constrains the already-existing
compacted graph to one canonical physical path. It emits typed evidence only. It never adds an edge,
joins unitigs, fills a gap, concatenates sequence, or changes graph topology.

## Placement-domain boundary

The result always declares `placement_domain = linear_unitig_only`. `ExactPlacementGroup` means one
upstream mapper equivalence group and its `placements` are all exact placements in that group. Group
ordinal and placement uniqueness are validated; replay examines every retained placement rather than
a selected primary. Every exact placement for one read must have the same positive aligned span;
inconsistent mapper records are rejected before a placement-input root can be returned.

Each generic calibration and replay input is a separate public interchange record. Its certificate binds the full
graph content root, read-set root, exact mapper algorithm/version/parameter bytes, declared placement
domain, fragment count, and the producer's assertion that enumeration is complete *within that
domain*. The public placement-root API applies the same certificate checks as analysis and refuses a
mismatched or incomplete certificate. The computed placement-input root additionally binds every fragment-instance identity, both
read digests, group membership, placement coordinates/strands, and the junction-unavailable count.
Pair, group, placement, unitig, and link input order do not affect these roots. The module cannot
independently prove that a caller-built public record computed its read or fragment digest honestly.
Such a record and its self-consistent digest are explicitly **unverified**, not authenticated.

The placement provenance boundary is the non-clonable, immutable
`AuthenticatedPlacementEvidencePair`, whose fields and constructor are crate-private. Only the
source-verifying `pair_mapper` can issue this pair-level capability after it freezes both subsets and
its producer-result root. The capability requires one graph declaration, one authenticated library
source lineage, identical mapper semantics, distinct subset read roots, and zero fragment-identity
reuse. It cannot be assembled from two producer runs through the safe public API. Source-backed
production additionally requires the opaque `AuthenticatedPairGraph` described in
[the adapter contract](EXPERIMENTAL_AUTHENTICATED_PAIR_GRAPH.md). The generic
`analyze_unverified_pair_paths` remains useful for fixtures and external experiments but returns an
unverified report; only the adapter wrapper can reach the crate-private authenticated analyzer.

The current stable read audit cannot place a read whose alignment crosses a GFA link. The caller must
therefore supply `junction_spanning_reads_unavailable` for each fragment. Such a fragment is excluded
from model fitting and path support, and the lane ledger, decision reason, and global unavailable-read
count retain the limitation. A complete graph-path mapper, including exact junction-spanning
placements, is a promotion blocker.

## Frozen lane model

Models are inferred from the calibration artifact independently by lane and frozen before the
separate replay artifact is evaluated. The result reports calibration count, replay count, and exact
fragment-identity overlap so calibration reuse cannot be hidden. An eligible anchor has exactly
one group containing exactly one placement for each mate, no unavailable junction-spanning read, the
same computed graph component, the same linear unitig, and distinct starts. Other observations enter
one first-match exclusion counter.

Orientation is FR, RF, FF, or RR in increasing emitted-forward-unitig coordinate order. Outer span is
the outer mapped envelope. Signed inner gap is right start minus left end, so overlapping mates have a
negative value. The model retains minimum, empirical nearest-rank P10, lower median, P90, and maximum.
Only the unique dominant orientation's P10--P90 outer-span and inner-gap intervals can become
available, after explicit count, exact rational dominance, and interval-width gates. Tail observations
remain visible in extrema; this empirical interval is not a confidence interval or a molecule-length
estimate.

## Exhaustive bounded replay

`PairPathGraph::from_unverified_compaction` consumes stable exact `Unitig` and `GraphLink` records with an explicit
`k` in the upstream exact-graph domain `3..=127`. It validates ACGT sequence and every `k-1` overlap,
canonicalizes reciprocal links, assigns link indices in canonical sorted order, and computes connected
components without using caller labels. The graph root covers `k`, every full unitig identifier,
topology and sequence digest, and the canonical physical link catalog. A checked
graph record-count admission is enforced before scanning caller-owned sequences, and a checked
`maximum_graph_sequence_bases` limit is enforced before base validation and hashing. The graph exposes
the bound segment and link catalogs; the result records the graph root, `k`, exact sequence bases
hashed, complete configuration/work limits, worker count, and retained graph-byte accounting.

For a path, the initial spelled length is the source unitig length. Traversing a link appends exactly
`next_unitig_length - (k - 1)` bases. The next unitig's start in the spelled walk is therefore the
previous spelled length minus `k-1`; mate coordinates and strands are transformed for the oriented
handle before outer span and signed inner gap are compared with the frozen lane interval.

The enumerator explores every walk that can still fit the interval's upper geometry bound. Positive
per-edge contribution makes this finite even for cycles. A depth cap truncates only when a further
walk can still fit that bound. Depth, created/explored state, examined-arc work,
reconstructed-path work, target-path, compatible-path, or placement-combination exhaustion returns
typed `indeterminate`; paths observed before exhaustion are never called complete. Search states form
a fallibly pre-reserved parent-index arena. Prefix vectors are not cloned on every extension. Only a
compatible target reconstructs its bounded path, and the ledger charges the parent walk, reversal,
reciprocal construction, canonical comparison, and conservative sorted-table comparison bound before
doing that work.

Reciprocal handle/link walks canonicalize to one physical path. A supplied fragment supports a graph
constraint observation only when every bounded search completes and the union across all exact
placement alternatives contains exactly one compatible canonical path. Any cross-component,
orientation-conflicting, geometry-conflicting, or geometry-pruned alternative forces whole-fragment
abstention even when another alternative fits. A placement on, or traversal through, a
`closed_graph_walk` is typed `non_linear_topology` and cannot become support. Zero paths and multiple
paths abstain; incomplete searches are indeterminate. Each
`(lane_ordinal, fragment_ordinal)` produces one decision, while detailed counters and all completed
compatible alternatives remain available for audit.

A compatible observation with no traversed link is classified `trivial_within_unitig`, not as path
support. No individual fragment authorizes reconstruction. Aggregate rows count distinct immutable
fragment identities as supporting, contradicting, or indeterminate for each canonical non-empty-link
path. At least two distinct supporting fragments (configurable upward) and no contradiction or
indeterminacy are required for `available_for_experimental_constraint`. Contradictory and incomplete
decisions are conservatively attributed to every candidate path sharing their unordered
endpoint-segment domain. If a placement-combination cap prevents complete domain attribution, every
emitted aggregate path is indeterminate. Adverse evidence therefore cannot disappear merely because
no compatible path was retained. Even an available signal does not authorize sequence synthesis,
topology mutation, or a scaffold join.

The freely constructible `PairPathResult::result_root_sha256` is a domain-separated, explicit
little-endian/length-prefixed
digest over the algorithm/version, graph and input roots, complete configuration, requested worker
count, graph accounting, frozen models, decisions and attribution domains, aggregate rows, and
summaries. `validate_pair_path_result` recomputes it and checks typed accounting, decision, summary,
and independently regenerated aggregate invariants. This unkeyed integrity root detects mutation; it
is not a digital signature and does not authenticate the upstream producer. An authenticated analysis
is instead returned as an opaque `AuthenticatedPairPathResult`; its separate root binds the sealed
pair-mapper producer root to the complete inner result root, and its validator recomputes both.

## Resource and claim boundary

Graph construction, calibration/replay pair/group/placement counts, mapper-field bytes, placement
combinations, depth, created and explored states, examined arcs, reconstructed-path work, target
paths, compatible paths, worker count,
total graph sequence/hash work, per-worker search memory, retained-result memory, and total analysis
memory have checked limits. Endpoint-domain attribution is included in result-memory admission. The
analysis admission includes the already retained graph bytes plus configured result and concurrent
worker bounds. Ledgers count geometry pruning and every work-cap termination. The graph reports owned
vector/string capacity bytes. Admission estimates include
conservative parent-index state and bounded path-output allowances but are not a process-RSS
guarantee and cannot make allocator,
Rayon, or operating-system overhead disappear.

Tests cover exact overlap distance, a unique path, equal-length branch ambiguity, convergent
multimapping, a pruned cycle, depth indeterminacy, empirical tail outliers, lane isolation, input-order
and worker-count determinism, reciprocal link normalization, junction-spanning unavailability,
provenance tampering, calibration reuse, aggregate minimum support, a long component chain, malformed
evidence, geometry-pruned alternatives, closed-walk exclusion, aggregate contradiction attribution,
complete-root tampering, exact-placement span disagreement, graph hash-work boundaries,
cross-library/cross-mapper rejection, opaque-capability/root validation, exact work-cap boundaries,
and a 100,000-segment linear-work regression. These are implementation tests, not evidence of improved
assembly, repeat resolution, sensitivity, or production readiness. See
[ADR 0019](adr/0019-lane-specific-pair-path-evidence.md).
