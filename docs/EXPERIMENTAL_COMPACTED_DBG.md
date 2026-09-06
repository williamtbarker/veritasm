# Experimental exact compacted graph

Status: isolated research substrate; not used by the stable CLI and not a
production-readiness, speed, sensitivity, or assembly-accuracy claim.

The unverified compacted-graph API consumes the already materialized exact
support rows of an `ExternalPartitionResult`. The source-backed API instead
accepts only an opaque `RetainedCountArtifact`, performs compaction internally,
and returns a non-Clone `AuthenticatedCompactedGraph` with private content and
ancestry seals. Both paths validate k, support
conservation, strict bucket/full-key row order, canonical full keys, positive
support, the complete minimizer owner, and the deterministic virtual bucket.
It binds the normalized full table, source identity, support unit, and support
mass into an exact edge-table digest. Run filenames and reclaimed predecessor
paths are not graph identities.

## Frozen experimental semantics

- A canonical exact k-mer is one backing graph edge. Its support is counted
  once under the input result's explicit occurrence or fragment-instance unit.
- A handle is a literal directed spelling. A non-self-complementary edge has a
  canonical handle and a reverse-complement handle. A self-reverse-complement
  edge has one fixed handle whose mate is itself.
- A node is a literal, noncanonicalized (k-1)-mer. Its incoming and outgoing
  degree count distinct exact oriented handles, not support, fingerprints,
  minimizers, or buckets.
- Compaction passes through a literal node only when its indegree and
  outdegree are both one. It additionally stops at a self-reverse-complement
  node and at either endpoint of a self-reverse-complement edge. These
  conservative fixed-point boundaries prevent one unitig orbit from consuming
  one backing canonical edge twice.
- Every degree boundary retains the full incoming-by-outgoing transition
  product as exact overlap relations. Relations are collapsed only with their
  reverse-complement equivalent; no branch is selected and no sequence is
  inserted.
- Residual one-in/one-out components are emitted as closed graph walks with
  repeated terminal (k-1) context. ClosedWalk is graph topology evidence, not
  proof that a biological molecule is circular.

Unitig identifiers are SHA-256 digests of a versioned domain, k, topology, and
exact sequence. They deliberately remain content identities rather than sample
or evidence identities. The v2 provenance digest separately binds the source
identity, typed support unit, and ordered full-key/orientation/support steps.
Unitigs sort by identifier and links by their complete typed tuple. The current
compactor is serial, so an ambient Rayon pool does not enter either order.

Closed-walk canonicalization and rotation-equivalence checks use a deterministic
linear-time minimum-rotation algorithm over logical views. They do not
materialize doubled cycles. An exhaustive small-array oracle and a 100,003-step
instrumented regression check the chosen rotation and a linear comparison
bound.

`AuthenticatedCompactedGraph::checked_graph()` recomputes the complete v2 graph
content root before lending a read-only view. That root covers every unitig,
edge step, support summary, provenance value, raw link, and scientific topology
counter. Its ancestry additionally binds the opaque source-equivalence root,
retention rule and inclusive threshold, retention root, retained-table root,
retained key/support totals, and graph root. Operational limits and measured
accounted bytes are kept in a separate operational root, so changing a
nonbinding limit does not change scientific identity. A copied or modified
`CompactedGraphResult` cannot be promoted back into the source-backed type.

Orbit construction also stores which raw walk is the deterministic representative of each
reverse-complement orbit. Closed-walk self-link derivation therefore scans the raw-walk table once
and emits only marked representatives; it does not rescan the complete mapping table to rediscover
an orbit minimum for every cycle. This phase is `O(W)` for `W` raw walks and uses a flag inside the
already admitted fixed-size mapping row. A 20,000-orbit/40,000-walk regression requires exactly one
self-link per marked orbit.

## Resource and conservation contract

Allocation-free scalar validation of k, minimizer length, and virtual-bucket
count precedes memory admission. The caller then supplies explicit limits for canonical edges, oriented handles,
literal nodes, unitigs, pre-deduplication link candidates, emitted bases, and
conservatively accounted owned bytes. Checked arithmetic and a pessimistic
two-orientation bound run before base-phase allocations. Exact topology-derived
output and link bounds run before output allocations. Vectors reserve
fallibly and reject allocator capacities above their admitted requested
capacity before use. The estimate includes payloads, flat walk tables, output sequences,
step provenance, and fixed allowances for nested vectors. It excludes the
borrowed external result, allocator internals, stacks, dependencies, and
process runtime, so it is not a hard RSS bound.

For `E` canonical edges, at most `2E` oriented handles, `N` literal nodes,
`W` raw walks, `C` pre-deduplication boundary candidates, `U` unitigs, and `L`
canonical links, owned payload is `O(E + N + W + C + U + L + output bases)`.
Sorted indices and binary node resolution give a conservative
`O((E + N + W + C + U + L) log(E + N + W + C + U + L))` time bound. The
minimum-rotation and closed-orbit matching phases are linear in their walk
lengths. `C` is explicit because a high-degree boundary contributes its full
incoming-by-outgoing product and is rejected before materialization when it
exceeds `max_link_candidates`.

Successful construction verifies:

1. every oriented handle belongs to exactly one raw walk;
2. handle and raw-walk reverse complement maps are involutions;
3. every emitted step has an exact literal overlap with its neighbor;
4. every canonical edge belongs to exactly one reverse-complement unitig
   orbit;
5. represented support equals input support in the unchanged support unit;
6. every emitted link has its declared literal (k-1) overlap; and
7. every result vector is in its documented deterministic order.

## Executable evidence

Module tests include an independently written byte-string graph/compaction
oracle, a separate directed-boundary-transition and reverse-complement-link
quotient oracle, literal sequence-window/key/orientation/support reconstruction,
typed provenance test vectors, exhaustive sources through length six at k=3,
generated reverse-complement properties, serial runs under one- and four-thread
ambient pools, and explicit
linear, repeat, branch, cycle, self-complementary-node,
self-complementary-edge, malformed-row, support-unit, and k=127 fixtures.
Selected shared-domain fixtures are also differential-tested against the
stable exact graph and compactor. These tests establish bounded software
invariants only. Opaque-boundary tests additionally remove a branch link,
change every seal category, and verify that operational limits remain outside
the scientific root.

## Limitations and promotion blockers

- The implementation materializes canonical edges, oriented handles, literal
  nodes, raw walks, unitigs, and links in memory. It is not partition-local
  compaction or a global boundary-stitching implementation.
- The unverified API consumes a completed in-memory external result. The opaque
  path validates its retained capability, but only an explicit upstream source
  replay re-reads the physical source bytes.
- Only graph-unitig and exact-overlap link records are emitted. There is no
  stable FASTA/GFA/schema integration, filtering, error correction, pair
  constraint, scaffold, phase inference, or source classification.
- Support is an exact count under the upstream unit. It is not coverage,
  abundance, confidence, molecule count, sensitivity, or biological truth.
- Fixed-point boundaries deliberately undercompact some palindromic topology.
- Exact minimizer revalidation currently decodes a short owned byte vector per
  key, and walk extraction resolves target nodes by binary search. Allocation-
  free packed validation and precomputed node indices remain performance work.
- Compaction itself is serial. Ambient Rayon-pool tests are not evidence of a
  parallel implementation or its determinism.
- No performance, peak-RSS, large-data, fault-injection, Apple Silicon, or
  comparative assembly benchmark has yet qualified this component.

Promotion requires equality with the stable exact graph and compactor
through their shared k range, external partition-local construction with
global degree reconciliation, authenticated end-to-end ancestry, retained
Linux and Apple Silicon resource evidence, stable artifact schemas, fault
injection, and pre-registered scientific comparisons.
