# ADR 0024: Construct and reconcile the exact cDBG out of core

- Status: proposed for an isolated experimental implementation; not accepted for the stable CLI
- Date: 2026-09-05
- Scope: one retained fixed-k graph for `3 <= k <= 127`
- Depends on: ADRs 0006, 0010, 0014, 0016, 0020, and 0022
- Orthogonal to: ADR 0023, which governs whether independently built k layers may create a
  reconstructed sequence

## Decision summary

Replace the whole-resident experimental compactor with an exact external pipeline whose durable
objects are retained canonical edges, literal-node states, oriented handles, partition-local
segments, and a sparse discontinuity graph. Compute global maximal walks by deterministic external
list reconciliation. Reverse-complement quotienting and stable identifiers occur only after the
global walks are complete.

The first implementation will use sort-merge joins and deterministic pointer doubling. This is a
correctness-first algorithm with a comparatively expensive `O(log S)` reconciliation, where `S` is
the number of local segments. A faster list-ranking implementation can replace it only after exact
output equivalence and resource measurements. No speed, RSS, assembly-sensitivity, or production
claim follows from this decision.

## Audit of the present experimental path

The current `external_run` and `external_reduce` modules establish useful foundations:

- full packed keys, rather than minimizers or hashes, decide equality;
- every run binds k, support unit, source identity, bucket, generation, ordinal interval, counts,
  lengths, ordering, and a SHA-256 trailer;
- bounded-fan-in merging uses checked support arithmetic and verifies replacement output before
  reclaiming predecessors;
- run catalogs use numeric identities instead of owning caller-length paths; and
- ordinary failures own and clean their private run directory or surface a cleanup error.

They are not an external graph data plane. The reducer writes a 72-byte observation for every
accepted event, retains a redundant 32-byte minimizer beside every 32-byte key, materializes all
final `ExactSupportCount` rows in one `Vec`, and then removes every final run. Its whole-file digest
also requires a complete seal/verification traversal. The final stream is ordered by
`(virtual_bucket, full_key)`, not globally by full key. These are appropriate oracle choices, not a
scalable cDBG interface.

The current `compacted_dbg` is likewise a valuable oracle. It validates every materialized support
row, constructs literal oriented handles and endpoints, reduces exact node degrees, applies the
degree/self-reverse-complement/fixed-edge boundaries from ADR 0010, partitions all raw handles into
walks, quotients walks by reverse complement, canonicalizes cycles, spells sequences, retains the
full boundary transition product, and checks edge/support conservation.

It simultaneously owns copies of canonical edges, handles, mate indices, two endpoint rows per
handle, literal nodes, visited and owner arrays, flattened raw walks, orbit representatives,
sequences, per-edge provenance, support scratch, and link candidates. Its admission is an owned
payload estimate, not an RSS bound. It consumes an already materialized result and does not stream
or authenticate final-run ancestry. Therefore it remains the independent small-graph oracle and
must not be incrementally decorated into the external implementation.

## What is adopted from prior work

| Precedent | Pattern retained | Pattern not inherited without proof |
|---|---|---|
| BCALM2 | Prefix/suffix-minimizer partitioning, independent local compaction, explicit glue records, and a proof that duplicated boundary objects reunite without losing k-mers | Its exact bucket/glue representation, union-find implementation, output orientation behavior, and abundance/palindrome conventions |
| GGCAT | Extra boundary context lets buckets emit intermediate unitigs instead of every k-mer; local products are joined through a sparse endpoint relation | Hash or fingerprint as identity, randomized merge order, read-splitting as a substitute for VeritAsm QC/support semantics, and large-k behavior without complete-key verification |
| Cuttlefish 2 | A bounded DNA node state is sufficient to encode exact incident sides and extract unitigs; topology need not be represented as pointer-heavy graph objects | A whole-resident MPHF/DFA array, absent-key use of an MPHF, its `(k+1)`-mer threshold model, and its odd-k/current-size domain |
| Cuttlefish 3 | Unique edge ownership, partition-local contraction, an external discontinuity graph, path contraction/list ranking, expansion, and collation | Its implementation-specific parallel ranking, color sparsification, graph convention, or performance claims |

BCALM2 proves a compact/glue pattern; GGCAT shows why retaining one character of boundary context can
avoid writing uncompacted interiors; Cuttlefish 2 shows that DNA degree state is tiny; and Cuttlefish
3 is the closest architectural precedent for local contraction followed by a discontinuity graph.
None of them establishes equivalence to VeritAsm's canonical-edge support units or its conservative
fixed-point boundaries. Equivalence is an executable obligation below.

## Frozen graph semantics

This ADR does not change `compacted_dbg` semantics:

1. One retained canonical k-mer and its checked support are one backing edge.
2. A non-self-reverse-complementary edge has two literal oriented handles; a fixed edge has one.
3. A node is the exact, noncanonicalized `(k-1)`-mer at a handle endpoint.
4. Degree counts distinct exact handles, never support mass.
5. A node is a hard boundary when in-degree or out-degree differs from one, the node is
   self-reverse-complementary, or it is incident to a self-reverse-complementary edge.
6. A walk may cross only a node that is not a hard boundary.
7. Every boundary retains the complete incoming-by-outgoing transition product. Compaction never
   selects a branch.
8. A residual one-in/one-out component is a `closed_graph_walk`, not evidence of a circular molecule.

Retention and support reduction happen upstream. This pipeline consumes a sealed retained-edge
stream and never changes a support threshold.

## Exact types and ownership

Let `N` be retained canonical edges, `H <= 2N` oriented handles, `V <= 2H` literal nodes, `S <= H`
partition-local segments, `U <= N` reverse-complement unitig orbits, `B` the external-I/O block size,
and `M` the admitted module-owned payload budget.

Packed strings are right-aligned two-bit values and compare as unsigned big-endian bytes. Width is
authenticated and specialized as `W64` for `k <= 31`, `W128` for `32 <= k <= 63`, and `W256` for
`64 <= k <= 127`. A `(k-1)` value uses the same physical width as its run. Inactive high bits must be
zero. Full strings are always rechecked before an identity-dependent action.

Ownership is singular at every stage:

- the retained stream owns each canonical edge once;
- each literal node state is owned by one immutable virtual partition;
- each oriented handle is owned by the chunk containing its source node;
- each local segment owns each of its oriented handles once;
- one global directed component owns each local segment once; and
- after reverse-complement quotienting, one emitted unitig owns each canonical edge once.

The node owner is

```text
pi(v) = route(leftmost_lexicographic_minimizer(canonical(v), m_node), P)
```

for `1 <= m_node <= k-1` and frozen virtual partition count `P`. Canonicalizing the complete node
before minimizer selection makes `pi(v) == pi(reverse_complement(v))`. The full literal node remains
the identity; the minimizer and partition only route work.

## Authenticated external block stream v1

All new intermediate files use one container, `VTEBLK01`. Integers are unsigned little-endian;
packed DNA is fixed-width big-endian; records have no padding. The fixed 192-byte file header is:

| Field, in byte order | Type |
|---|---|
| magic, schema, record-kind | `[u8;8]`, `u16=1`, `u16` |
| key-width tag, k, `m_node`, support-unit | four `u8` |
| virtual partition, generation | `u32`, `u32`; `u32::MAX` means globally sorted |
| numeric run ordinal | `u64` |
| retained source root, scientific-configuration root, parent-set root | three `[u8;32]` |
| fixed record width, maximum block payload | `u16`, `u32` |
| record count, block count, payload bytes | three `u64` |
| reserved | 34 zero bytes |

Each block is a 64-byte header, fixed-width records, then a 32-byte digest. The block header is
`VTBLOCK1:[u8;8], block_ordinal:u64, record_count:u32, payload_bytes:u32,
previous_block_digest:[u8;32], reserved:[u8;8]`; these fields total exactly 64 bytes. A block cannot
exceed the file header's `u32` payload cap, and its fixed record width must divide `payload_bytes`
exactly. The block digest is SHA-256 over a versioned domain; record kind, key width, k, `m_node`,
support unit, partition, generation, run ordinal, all three roots, record width, and block cap; then
the complete block header and exact payload. The three mutable file totals and reserved file-header
bytes are not part of that per-block domain. Blocks form a hash chain. The 104-byte trailer is
`VTBLKEND:[u8;8]`, three repeated `u64` totals, final block digest, file content root, and exact
`u64` file length. The content root hashes the versioned domain, complete canonical file header,
trailer magic, repeated totals, final block digest, and exact file length, excluding the stored
content-root field itself. An empty stream has zero blocks, records, and payload, and uses an all-zero
final-block digest; no nonempty stream may use that sentinel.

The scientific-configuration root covers only semantics that can change the retained graph or its
interpretation: k, support unit, retention rule, QC/source domain, and fixed-point boundary version.
Partition count, minimizer, buffers, fan-in, thread count, and chunk limits are operational plan
fields recorded in intermediate manifests and do not enter stable unitig provenance.

The parent-set root hashes the sorted complete list of `(record kind, partition, generation,
run ordinal, content root)`. That list remains in a bounded external ancestry ledger; the root is not
a substitute for retaining the list. A descriptor-bound reader checks registered identity and length,
canonical header bytes, every record invariant and total order, every block chain link, trailer,
content root, exact EOF, and descriptor metadata before success. SHA-256 detects cache corruption and
mix-ups; it is not a MAC against an actor able to replace the data and every expected digest.

A consumer may derive unregistered temporary bytes from a verified block, but it may not seal or
register its output, reclaim a predecessor, or publish anything until all input readers authenticate
their trailers. A failed input invalidates every derived byte.

EC-1a realizes that rule by writing derived spills with an all-zero 192-byte placeholder header and
no trailer. Such a provisional file is not a `VTEBLK01` artifact. After every parent reader verifies
its trailer, exact EOF, and final descriptor identity, children are sealed and registered in
deterministic numeric-ID order. A late parent failure therefore leaves no registered descendant in a
normal result. Each invocation uses a collision-resistant mode-`0700` private directory and
mode-`0600` run files. Final-component symlinks are not followed; registered predecessor deletion
checks both opened-descriptor and literal-path identity. Cleanup is bounded by the admitted run-file
count and never recursively deletes a pathname: it validates the `0700` directory by path and
`NOFOLLOW` descriptor, validates all exact numeric `.vte` entries before deleting any, then repeats
entry validation immediately before unlinking each literal owner-matching mode-`0600` regular file.
An unknown entry, symlink, permission mismatch, count excess, or substituted directory fails closed.
All scanning and unlinking is relative to opened directory descriptors. This is a corruption and
ordinary pathname-race defense, not a security boundary against a hostile concurrent process with
the same UID: portable POSIX cannot atomically condition unlink on inode identity. The configured
work directory remains trusted. Crash residue is neither trusted nor resumed.
The successful topology owner is non-constructible and non-cloneable outside its module. Roots,
summaries, statistics, and ancestry are held in private fields and exposed by read-only accessors;
downstream code must not receive a safely mutable proof-shaped capability.

### Fixed record families

`W` is the authenticated packed width and `WM` is the packed width of `m_node`. Every reserved byte
must be zero. Explicitly redundant fields are independently recomputed.

| Kind | Exact field order | Sort order |
|---|---|---|
| `EDGE` | `canonical_key[W], support:u64` | canonical key |
| `INCIDENCE` | `node[W], handle[W], direction:u8, fixed_edge:u8` | node, direction, handle |
| `NODE_STATE` | `node[W], owner_minimizer[WM], owner_partition:u32, incoming_mask:u8, outgoing_mask:u8, flags:u8, reserved:u8` | partition, node |
| `NODE_AUDIT` (EC-1a internal) | `node[W], incoming_mask:u8, outgoing_mask:u8, flags:u8, reserved:u8` | literal node |
| `HANDLE` | `handle[W], support:u64, source_node[W], target_node[W], source_chunk:(u32,u64), target_chunk_present:u8, target_chunk:(u32,u64), source_flags:u8, target_flags:u8` | source chunk, handle |
| `LOCAL_SEGMENT` | `segment_key[W], chunk:(u32,u64), first_handle[W], last_handle[W], minimum_handle[W], first_node[W], last_node[W], step_count:u64, total_support:u64, min_support:u64, max_support:u64, flags:u8` | segment key |
| `LOCAL_STEP` | `segment_key[W], local_offset:u64, handle[W], canonical_key[W], orientation:u8, support:u64` | segment key, offset |
| `JOIN` | `tail_segment[W], head_segment[W], junction_node[W], tail_handle[W], head_handle[W], kind:u8` | tail, head |
| `RANK` | `segment_key[W], predecessor_present:u8, predecessor[W], successor_present:u8, successor[W], component_key[W], step_prefix:u64, component_steps:u64, class:u8` | segment key |
| `FINAL_STEP` | `component_key[W], offset:u64, handle[W], canonical_key[W], orientation:u8, support:u64` | component, offset |
| `BOUNDARY_END` | `node[W], direction:u8, handle[W], unitig_digest:[u8;32], unitig_orientation:u8` | node, direction, handle, unitig tuple |

The exact encodings must be generated from typed codecs, not `repr(C)`. Width formulas and maximum
block cardinalities are checked before allocation. `segment_key` is the first exact oriented handle,
which is globally unique because retained handles are unique; it is not a digest. Digests name stable
artifacts only after collision checks and never replace exact graph identity.

## Construction algorithm

### 0. Preserve a streaming retained-edge root

Add an experimental reducer result that transfers ownership of sealed final `EDGE` runs through a
guard instead of materializing a `Vec` and deleting the runs. It exposes a registered, descriptor-bound
stream and a content root. A separate oracle adapter may materialize it under a small explicit cap.
The stream must already reflect the frozen retention rule and exact occurrence or fragment-instance
support.

Until that owned transfer exists, EC-1a exposes only
`UnverifiedMaterializedEdgeAdapter` and
`build_external_cdbg_from_unverified_materialized`. The naming is a mandatory trust-boundary cue:
`ExternalPartitionResult` is public mutable materialization, so its source/configuration-like fields
are checked labels rather than authenticated predecessor provenance. The adapter is suitable for
bounded oracle comparison, not for a production provenance claim.

Bounded-fan-in merge the bucket-final streams into global full-key order and reject a duplicate full
key. In addition to layout-dependent file content roots, compute a canonical retained-edge-table root
over the versioned scientific domain, source root, scientific-configuration root, row count, support
mass, and the globally sorted exact `(canonical_key,support)` rows. This table root is independent of
partition count, block size, spill threshold, fan-in, and thread count.

### 1. Materialize oriented incidence

For every `EDGE`, recompute canonicality and support, create its canonical handle, and create its
reverse-complement handle unless the spelling is fixed. For each handle `h:u->v`, emit outgoing
`(u,h)` and incoming `(v,h)` incidence. Externally sort by exact literal node. Lockstep conservation
requires one or two handles per canonical edge and exactly two incidences per handle. Resource caps
are applied to checked actual handle/incidence counts during this scan; the `2N`/`4N` upper bounds
must not reject an input whose reverse-complement-fixed edges make the actual counts fit.

### 2. Reduce global node state

Reduce each node group to four-bit incoming and outgoing masks; incoming bits index the leading base
of an incoming handle and outgoing bits index the trailing base of an outgoing handle. For a set of unique exact k-mers,
two distinct literal handles at one node and direction cannot have the same terminal base: that node,
direction, and base reconstruct the same complete handle spelling. The reducer nevertheless checks
this rather than assuming it; a repeated direction/base incidence is a duplicate-handle integrity
failure before the bit is set. More than four sides, a handle whose prefix/suffix does not equal the
grouped node, or disagreement with its fixed-edge flag is also fatal. Recompute
self-reverse-complement status and `pi(node)`. Emit one
`NODE_STATE`, including hard-boundary flags, and independently verify reverse-complement node/mask
symmetry with an externally sorted mirror stream.

The EC-1a verifier creates direct and reverse-complement `NODE_AUDIT` projections only after a full
authenticated `NODE_STATE` scan, externally merges both into literal-node order, and compares them
lockstep. Mirror construction reverse-complements the complete node, swaps incoming/outgoing masks,
complements each base bit, and preserves independently validated fixed-edge, fixed-node, and hard-
boundary flags. Any missing row or mask/flag mismatch is fatal. The direct final projection also
recomputes the canonical node-table root from retained bytes; the final `EDGE` stream is scanned
twice to recompute support mass and its canonical table root without materialization. These roots
must equal their pre-seal values. Audit runs are authenticated descendants and then reclaimed.

This global reduction is deliberately performed before local contraction. A bucket never guesses a
degree from incomplete local observations.

### 3. Create bounded deterministic chunks

Sort outgoing handles by `(owner partition, source node, handle)`. Within one virtual partition,
scan source-node groups in exact node order. Start a new chunk before the next complete node group
would exceed the admitted handle, node, or owned-byte capacity. A source node is never split and has
at most four outgoing handles, so a configuration unable to admit one four-handle node fails before
construction.

Chunk IDs are `(virtual_partition, sequential_chunk_ordinal)`. They depend on exact data and the
recorded memory configuration, never thread completion. External joins attach source and target
`NODE_STATE` plus the target node's chunk, if it has an outgoing handle. A non-boundary one-out target
must have a target chunk.

Skew can create more chunks but cannot force an oversized bucket into memory. This is the mechanism
that makes the algorithm bounded rather than merely “partitioned.”

### 4. Compact each chunk locally

Load at most one admitted chunk. Define an allowed successor from handle `h` to `g` only when:

1. `h.target == g.source` exactly;
2. the target node is globally non-boundary;
3. `g` is that node's unique global outgoing handle; and
4. `g` belongs to this same chunk.

Allowed predecessor degree is likewise at most one. Extract all paths from local predecessor-free
handles, then all residual local cycles. A residual local cycle is cut at its smallest exact handle
and represented as a `LOCAL_SEGMENT` plus one synthetic `local_cycle_closure` join. Emit ordered
`LOCAL_STEP` rows. Every loaded handle must be emitted once, and every step overlap is rechecked.

### 5. Build the discontinuity graph

For a local tail ending at a globally non-boundary node in another chunk, obtain the unique outgoing
handle from `NODE_STATE` and external-join it to the `LOCAL_SEGMENT` beginning with that handle. Emit
one `cross_chunk` `JOIN`. Hard-boundary tails emit no continuation join. Local synthetic closures are
carried unchanged.

Reduce joins to a sparse functional graph. Every local segment has at most one predecessor and one
successor. Missing required joins, joins across a hard boundary, duplicate predecessors/successors,
nonmatching `(k-1)` overlap, or a join whose head is not the unique outgoing handle is fatal. This
graph contains `S` vertices rather than `H` handles.

### 6. Classify, rank, and cut global components

Use authenticated external sort-merge pointer doubling for exactly
`R = ceil(log2(max(1,S)))` rounds.

- Successor and predecessor pointers jump two links per round, with checked segment and step
  distances.
- A path must reach a null end within `R` rounds.
- A component whose pointers remain non-null is a cycle; min-label propagation over the same rounds
  yields the smallest exact handle in the cycle.
- A non-null pointer in a purported path, a null pointer in a purported cycle, nonconvergence, or
  rank/size disagreement is fatal.

For a cycle, the segment containing its unique minimum handle is split logically at that handle. Cut
the predecessor relation there and rank the resulting pieces as one path; no step bytes are copied
until final collation. Path offsets are exact prefix sums of `step_count`. Component step totals must
agree from both ends and with the number of collated steps.

Pointer doubling is intentionally conservative: it is deterministic and straightforward to compare
with a serial oracle, but performs `O(log S)` external join rounds. A later deterministic or
seed-recorded parallel list-ranking algorithm must be introduced by its own equivalence record.

### 7. Quotient reverse complements and spell unitigs

For every ranked directed component, map every handle to its exact mate and external-join one mate
handle to the component that owns it. This relation must be an involution with equal step count,
topology, reversed order, support vector, and literal endpoints.

Create two ordered candidate step streams per reverse-complement orbit: the component and
`mate(reverse(component))`. Linear candidates begin at their graph endpoint. Cycle candidates begin
at their unique minimum handle after rotation. Compare complete exact handle streams
lexicographically; emit the smaller, with a full stream comparison when first handles tie. A tie is
allowed only when every exact step agrees.

Stream selected `FINAL_STEP` rows to spell sequence, compute total/min/max support, and compute the
  lower median from a separate external `(component,support,offset)` sort. The unitig provenance digest
  binds the canonical retained-edge-table root, scientific-configuration root, support unit, k, topology, and
every ordered `(canonical key, orientation, support)` step. The sequence ID retains the existing
versioned k/topology/sequence digest. Equal digests with unequal exact sequence or topology are a
fatal identifier collision.

Finally external-sort selected canonical keys and compare them lockstep with `EDGE`. Every canonical
edge and support must appear exactly once. Separately compare all local steps with the complete
oriented-handle stream to prove pre-quotient handle conservation.

### 8. Reconstruct exact graph links

For every global path, emit endpoint mappings for its raw orientation and reverse-complement mate.
Join those mappings to incidence at hard-boundary nodes. For each boundary node, emit the complete
incoming-by-outgoing product, map to emitted unitig orientation, canonicalize each relation with its
reverse complement, sort, and deduplicate exact tuples. Emit one closure relation per closed walk.

Every link must independently verify the literal `(k-1)` overlap against the streamed unitig prefix
and suffix. Pair evidence never enters this stage.

## Deterministic order

Scientific output is independent of worker count, descriptor scheduling, spill threshold, fan-in,
bucket skew, and admitted chunk size:

1. virtual owners are pure functions of complete exact nodes;
2. chunks scan exact node order and are operational only;
3. local segments use exact first-handle keys;
4. global ranks derive only from exact predecessor/successor relations;
5. cycle rotation and reverse-complement choice compare exact handle streams;
6. unitigs sort by full stable digest after collision checks; and
7. links sort by the complete typed canonical tuple.

Changing an operational limit may change intermediate roots and chunk metadata. It must not change
the final edge-step streams, unitig sequences/IDs, links, or scientific evidence values. Operational
telemetry records the actual plan outside deterministic scientific artifacts where the stable schema
requires that separation.

## Resource and I/O bounds

Under the standard external-memory model, let `sort(X) = O((X/B) log_(M/B)(X/B))` block transfers and
`scan(X) = O(X/B)`. The construction uses a constant number of sorts/scans over `H` for incidence,
node state, handle attachment, and local contraction; `O(log S)` sort-merge rounds over the sparse
discontinuity graph; and a constant number of sorts over `N` for collation, medians, conservation,
and links:

```text
I/O = O(sort(H) + log(S) * sort(S) + sort(N) + scan(output_bases))
CPU = O(sort_cpu(H) + log(S) * sort_cpu(S) + sort_cpu(N) + output_bases)
```

With ordinary comparison sorting, the CPU expression is conservatively
`O(H log H + S (log S)^2 + N log N + output_bases)`; fixed-width radix sorting may change that
constant/model but is not assumed here. The pointer-doubling term is an upper-bound design cost, not
a performance claim. At every phase,
module-owned payload is at most admitted chunk payload plus `fan_in` reader buffers, one writer
buffer, heap rows, fixed codecs/hash state, and bounded catalogs. Large vectors reserve fallibly
after the complete phase high-water is admitted. File descriptors are at most `fan_in + 1` run files
plus explicitly counted manifest/ledger descriptors.

With predecessor reclamation after replacement authentication, live temporary payload is linear in
the largest coexisting old/new generation plus retained roots: `O(H + S + N + output_bases)` bytes
with width-dependent constants. Exact projected bytes and disk high-water are checked before each
output generation. This is a module-owned RAM and temporary-byte contract. It excludes allocator
metadata, thread stacks, libraries, code pages, kernel cache, filesystem metadata, and unrelated
process state, and therefore is not an RSS bound.

## Fatal states and atomicity

The implementation fails without a normal result for:

- unknown schema/encoding/record kind, inactive bits, noncanonical edges, zero support, support or
  rank overflow, or invalid k/m/width domain;
- source/config/parent-root mismatch, descriptor replacement, length change, block-chain or trailer
  corruption, truncation, trailing bytes, or noncanonical ordering;
- wrong node owner, degree above four, duplicate incidence side, broken reverse-complement masks,
  or an incorrect fixed-point flag;
- a split source-node group, oversized minimum chunk, lost/duplicated handle, bad local overlap, or
  synthetic closure on a noncycle;
- missing/duplicate/incompatible continuation joins, nonfunctional discontinuity graph,
  pointer-doubling nonconvergence, rank disagreement, or cycle-cut disagreement;
- reverse-complement noninvolution, unequal mate support/order, edge or support nonconservation,
  unitig-ID collision, or invalid boundary link;
- admitted RAM, temporary bytes, run count, file descriptor, output bases, or integer limits;
- short write/read, ENOSPC, sync/rename failure, injected interruption, predecessor reclamation
  failure, or cleanup failure.

No intermediate is resumable in the first slice. Ordinary failure removes only bounded,
individually revalidated run files and their identity-checked empty private directory; cleanup
failure reports the retained private path. A process crash leaves an incomplete
tree that a later invocation rejects rather than resumes. Stable publication remains governed by the
existing early lease and no-replace bundle transaction, so graph failure cannot clobber an existing
result.

## Independent oracles and kill tests

Promotion requires all of the following, not merely ordinary examples:

1. Differential equality with `compacted_dbg` for every shared-domain graph: exact unitig topology,
   sequence, ordered steps/support, statistics, and canonical links.
2. An independent byte-string oracle that never calls packed prefix/suffix, canonicalization,
   minimizer, node-state, compaction, or cycle helpers from the implementation.
3. Exhaustive retained edge subsets at k=3 and k=4 where feasible, plus generated k=5 subsets,
   checking all thread counts, partition counts, fan-ins, and chunk capacities.
4. Boundary fixtures for branch products, hairpins, self-reverse-complement nodes, every even-k
   self-reverse-complement edge pattern, disconnected components, and cycles split across 1, 2, and
   many chunks.
5. One-bucket and one-source-node skew, minimum admissible chunks, every record-width transition
   around k=31/32 and 63/64, and k=127 terminal-bit vectors.
6. Metamorphic reverse-complement, input permutation, support-unit label, spill threshold, block
   size, merge fan-in, chunk-size, and 1/2/4/8-thread tests. Scientific outputs must be byte-identical
   where the schema declares them deterministic.
7. Mutation-negative tests for every header field, parent root, block link, key, support, node mask,
   owner, chunk, local offset, join, rank, cycle cut, orientation, trailer, EOF, and ancestry row.
8. Kill/fault tests after every block write, flush, sync, registration, predecessor deletion, list
   round, and final collation; real ENOSPC/short-write tests where the platform permits them.
9. Lockstep conservation: `EDGE <-> selected FINAL_STEP`, handles `<-> LOCAL_STEP`, joins `<-> ranked
   components`, boundary incidences `<-> canonical links`, and all support totals with checked u64.
10. Long-cycle tests that demonstrate linear streaming canonicalization and prevent doubled-cycle or
    quadratic comparison materialization.

A probabilistic structure may schedule blocks or index a known exact set, but a Bloom, fingerprint,
or MPHF-only result must be killed by forced-collision/absent-key tests before it can influence any
row above.

## Smallest implementable next slice: EC-1

Implement only the authenticated topology seam, with no stable CLI or FASTA/GFA change:

1. Add an owned streaming-final-run variant to `external_reduce`; keep the current materialized API
   as a capped oracle adapter.
2. Implement `VTEBLK01` codecs for `EDGE`, `INCIDENCE`, and `NODE_STATE`, including descriptor-bound
   verification, ancestry registration, and predecessor cleanup.
3. Stream exact handles from retained edges, external-sort incidences, and reduce globally exact node
   states through k=127.
4. Deterministically chunk source-node groups, attach target node states, and emit locally compacted
   `LOCAL_SEGMENT`/`LOCAL_STEP` plus unresolved seam records. Do not yet glue seams into global
   unitigs.
5. Compare every node state, local allowed successor, local step, break reason, and conservation total
   with an adapter over the whole-resident oracle. Force at least one path and one cycle across chunk
   boundaries in three different chunk plans.

EC-1 is small enough to review because it stops before list ranking, reverse-complement orbit output,
median support, and link serialization. It is still architecturally decisive: it removes final-count
materialization, proves global degree before local contraction, survives arbitrary bucket skew by
node-group chunking, and creates the exact discontinuity input required by EC-2. It must remain
experimental until EC-2 completes global reconciliation and all gates above pass.

## Rejected alternatives

- Loading one minimizer bucket and calling it bounded memory: an adversarial bucket can contain the
  complete graph.
- Canonicalizing literal nodes before degree reduction: opposite literal sides would be conflated.
- Local degree inference from a bucket: missing external incidence can turn a branch into a false
  unitig.
- Hash/minimizer/fingerprint/MPHF as handle or node identity: a collision or absent-key query can
  change topology.
- Keeping the current materialized count vector while externalizing only unitig strings: retained
  graph memory remains proportional to N.
- Random merge order without a frozen seed and output-equivalence proof: it complicates replay and
  deterministic failure localization.
- Union-find alone: it identifies component membership but not ordered path rank, orientation, cycle
  rotation, or streamed sequence spelling.
- Emitting partition-local paths as final unitigs: chunk boundaries are operational, not biological
  or graph boundaries.
- Treating a graph cycle as molecular circularity or a compacted graph as an assembled genome.

## References

- Chikhi R, Limasset A, Medvedev P. “Compacting de Bruijn graphs from sequencing data quickly and in
  low memory.” *Bioinformatics* 32(Suppl 1), 2016. <https://doi.org/10.1093/bioinformatics/btw279>
- Cracco A, Tomescu AI. “Extremely fast construction and querying of compacted and colored de Bruijn
  graphs with GGCAT.” *Genome Research* 33, 2023.
  <https://doi.org/10.1101/gr.277615.122>
- Khan J, Kokot M, Deorowicz S, Patro R. “Scalable, ultra-fast, and low-memory construction of
  compacted de Bruijn graphs with Cuttlefish 2.” *Genome Biology* 23, 2022.
  <https://doi.org/10.1186/s13059-022-02743-6>
- Khan J, Dhulipala L, Pandey P, Patro R. “Fast and Scalable Parallel External-Memory Construction of
  Colored Compacted de Bruijn Graphs with Cuttlefish 3.” 2025 preprint / RECOMB 2026 proceedings.
  <https://doi.org/10.1101/2025.02.02.636161>
- Official BCALM2 source and format notes: <https://github.com/GATB/bcalm>
- Official GGCAT source: <https://github.com/algbio/GGCAT>
- Official Cuttlefish source: <https://github.com/COMBINE-lab/cuttlefish>
