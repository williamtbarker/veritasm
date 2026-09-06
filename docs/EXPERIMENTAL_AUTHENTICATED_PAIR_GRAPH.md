# Experimental authenticated pair-graph adapter

Status: isolated Rust substrate; not called by the stable `veritasm assemble`
pipeline or represented as a promoted production capability.

`src/experimental/authenticated_pair_graph.rs` closes the ancestry gap between
the read-witnessed compacted child and paired-end placement. A caller-created
`PairPathGraph` remains an unverified topology value. It cannot be supplied to
the source-backed pair mapper.

## Capability chain

The public constructor requires all three opaque source-backed inputs from one
exact reconstruction:

```text
AuthenticatedCompactedGraph + TransitionLedger
                         + AuthenticatedWitnessedChild
                                      |
                         complete child replay
                                      |
                         deterministic conversion
                                      |
                         AuthenticatedPairGraph
                                      |
                   authenticated paired spool replay
                                      |
                            PairMapperResult
```

`adapt_authenticated_witnessed_child` first calls the witnessed child's full
source validator. That operation regenerates the complete constrained child
from the exact originating compacted graph and transition ledger. It then
checks common source identity, compacted ancestry, and transition identity
again at the adapter boundary. A child and ledger from different spools, or a
child paired with a different genuine graph, fail rather than being rebound by
copying hashes.

The returned `AuthenticatedPairGraph` has private fields, no unchecked public
constructor, no mutable content view, and no `Clone` implementation. Its
internal ancestry token is a crate-visible type because the sibling pair-mapper
module must read it, but the token constructor and fields are private to the
adapter module. The raw pair-mapper entry point is crate-private. Public callers
can neither mint the token nor invoke the raw boundary.

## Deterministic exact conversion

Each witnessed segment becomes one mapper target:

- target ID: the complete lowercase hexadecimal `WitnessedSegmentId`;
- sequence: the complete A/C/G/T witnessed sequence;
- topology: linear or closed-walk without relabeling;
- support summaries: the witnessed segment's exact edge summaries; and
- placement fields: explicitly `not_evaluated` until pair mapping completes.

Targets are sorted by full target ID. Each witnessed link becomes one oriented
`GraphLink` using the complete endpoint IDs and exact orientations. Links are
sorted, and every link must report exactly `k-1` overlap. The existing pair
graph constructor independently validates target spelling, full-ID uniqueness,
each overlap, reciprocal canonicalization, and connected components. The
adapter requires output segment and canonical-link counts to equal the
witnessed child; it does not silently merge or discard topology.

Closed-walk targets are retained in the graph but remain outside the current
linear-unitig mapper domain. Reads that cannot be completely enumerated in that
domain are explicitly unavailable. The adapter does not add graph-junction
alignment, scaffold, synthesize sequence, or promote an inferred join.

## Root contract

The adapter authentication root is a domain-separated SHA-256 digest over its
algorithm ID/version and:

1. common authenticated spool-source root;
2. exact-count source-equivalence root;
3. compacted-graph ancestry root;
4. exact original-read transition root;
5. complete witnessed-child content root;
6. witnessed-child authentication root; and
7. exact pair-graph content root.

The pair-graph content root binds `k`, complete target IDs, target topology and
sequence digests, and the canonical oriented link catalog. The pair mapper
recomputes the adapter root, checks it against the current spool and graph, and
binds every individual ancestry root plus the adapter root into its version-2
producer root. The authenticated path analyzer accepts a `PairMapperResult`
only when every stored ancestry root and both placement certificates match the
same opaque adapter.

These hashes are integrity and local derivation identifiers. They are not a
signature or proof against an actor able to replace trusted executable code.

## Resource boundary

`PairGraphAdapterLimits` bounds witnessed segments, witnessed links, total
target bases, and construction accounting. Before allocating output, the
adapter admits:

- exact target-vector headers, two fixed-width 64-byte hexadecimal strings per
  target, and all target bases;
- temporary link-vector headers and two full 64-byte IDs per link;
- the configured pair-graph memory allowance;
- conservative scratch allowances for sorting, link normalization, component
  construction, and temporary index vectors inside pair-graph construction;
  and
- the adapter header plus a fixed implementation margin.

All arithmetic is checked and target, string, sequence, and link allocations
are fallible. Actual vector and string capacities are checked against the
projection. Returned graph/vector capacities are remeasured before publication.
Allocator metadata, the authenticated inputs already owned by the caller,
Rayon internals, and process RSS are outside this owned-payload statement.

## Validation evidence and limitations

Focused tests construct the complete chain from real paired FASTQ files and
exercise successful placement and authenticated path analysis. They also cover
determinism across one and four mapper workers, repeated deterministic adapter
construction, changed target sequence, changed adapter/configuration roots,
cross-source graph/ledger/spool swaps, single-end rejection at the pair-mapper
boundary, and cardinality/memory-limit failures. Compile-fail documentation
checks that an external crate cannot clone or mutate the adapter or invoke the
raw producer, mint its ancestry token, or promote a freely constructed
`PairPathGraph` through the authenticated producer.

This is implementation evidence, not evidence of improved repeat resolution,
assembly sensitivity, biological detection, or production qualification. The
placement domain is still `linear_unitig_only`; complete exact placement across
graph links remains a stated promotion blocker.
