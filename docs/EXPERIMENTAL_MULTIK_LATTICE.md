# Experimental independent multi-k evidence lattice

Status: isolated in-memory library substrate; not used by stable `assemble` and not an assembly,
sensitivity, or performance claim.

This slice implements the first non-projecting milestone from ADR 0016. It builds every configured
k independently from the same post-QC `AcceptedSegment` values through
`experimental::partitioned_dbg`. Child k values must be sorted, strictly increasing, and unique.
No child consumes another child's keys, counts, unitigs, or derived sequence. The current substrate
therefore has exact accepted-window occurrence support only; it does not imply fragment, molecule,
coverage, abundance, or confidence support.

## Relation semantics

Rows exist only between consecutive configured child layers and are sorted by `(lower_k, higher_k,
typed evidence)`. They annotate child edges and cannot create sequence or graph membership.

| Type | Exact statement | Not established |
|---|---|---|
| `supports` | One complete observed higher-k canonical edge contains two oriented lower-k edges at adjacent offsets with an exact k-1 overlap. | A preferred traversal, an independent observation, or a safe contig join. |
| `contains` | One oriented lower-k spelling is an exact substring of one complete observed higher-k canonical edge at the recorded offset. | Biological origin, phase, or truth. |
| `conflicts` | At least two complete higher-k witnesses give different immediate bases on the same normalized side of one lower-k key; every witness row remains present. | A variant, strain, sequencing error, or globally phased haplotype. |
| `unresolved` | The recorded number of lower-edge occurrence sides cannot be covered by any higher-k window within the same accepted segment. | Absence of longer context elsewhere or evidence that the edge is erroneous. |

Sides are normalized to the lower edge's canonical orientation. An even-k
self-reverse-complemental edge has no strand-identifiable left or right side, so its side is
`FixedPoint`; complementary flank bases are also normalized to one orbit representative. This
prevents input reverse complementation from fabricating different uncertainty or conflict labels.
One fixed-point occurrence can contribute two unresolved terminal-side events because its two
literal sides collapse into that single non-orientable side class.

## Exactness and no-unsupported-adjacency rule

The builder independently enumerates every exact source slice, canonicalizes its full 256-bit key,
sorts and run-length counts it, and compares that result with each partitioned child. Minimizers and
bucket IDs never establish equality. Every child must also carry the same SHA-256 accepted-segment
source identity, and every key is decoded and re-encoded before relation admission.

`validate_no_unsupported_adjacencies` replays every `supports` row inside the complete higher-k
canonical sequence and checks both lower child memberships, both full oriented spellings, and their
literal overlap. Small-k traversability, support magnitude, a minimizer match, or a bucket match is
insufficient. `validate_against_segments` additionally repeats the slice recount and recomputes all
source-boundary `unresolved` rows. The builder runs this strongest oracle before returning.

Support replay builds the sorted full-key indexes at most once for each child in an adjacent layer
pair, validates all support rows in that pair, and then releases the pair scratch before advancing.
Pair relation ranges are found by binary partition in the globally sorted relation table, and a
support row's layer pair is checked by binary search in the sorted child list. Let `E` be the total
edge rows indexed across supported adjacent pairs, `R` the relation count, `C` the child count, and
`K <= 127` the largest spelling length. Standalone support replay is
`O(E log E + R * (log E + log C + K) + C log R)` time; because `K` is fixed by the module domain and
`C <= E` for nonempty supported pairs, this is `O(E log E + R log E)` rather than
`O(R * E log E)`. Scratch is proportional to the largest adjacent edge pair. The implementation
records index-build and visited-support counts privately; a high-row-count regression requires
exactly two index builds for one layer pair. Full invariant and source-bound relation validation use
the same binary pair ranges rather than rescanning all `R` relations for every child pair.

## Determinism and limits

Segment input order is normalized by unique `(source_ordinal, segment_ordinal)` coordinates. Children
follow strictly increasing k, all relation rows have a total order, and no worker-completion or hash
table order enters the result. Tests compare reversed segment inputs under one- and four-thread Rayon
pools and exercise reverse-complement equivalence, including self-reverse-complemental lower edges.

Every child retains its explicit `PartitionLimits`. `MultiKLimits` separately bounds child count,
source-segment count, the sum of configured child payload ceilings, per-child independent-oracle
windows, candidate relation events, returned relation records, and projected relation and
validation-scratch payload. Before constructing any child, the parent admits and allocates the
segment-order index, child result headers, exact child-window totals, and the largest child oracle
recount; the oracle-window limit therefore cannot first fail after a child graph has already been
built. Arithmetic and `u64`/`usize` conversions are checked, vector allocation is fallible, and
actual capacities are rejected if they exceed the pre-admitted payload. These are module-owned
payload admission limits, not a process-RSS guarantee.

## Deliberately absent

- stable CLI, spool, bundle, FASTA, GFA, or report integration;
- support thresholding, fragment deduplication, graph cleaning, correction, or low-count rescue;
- compaction or sequence projection across layers;
- pair constraints, scaffolds, circularity claims, or haplotype reconstruction;
- an authenticated external spill format or a production-scale memory/performance result; and
- any claim of improved reconstruction sensitivity.

Promotion requires an authenticated shared-spool parent/child manifest, exact fragment-support
reduction where requested, external construction, stable evidence schemas, adversarial fault and
determinism tests, and frozen scientific comparisons. A later path projection still requires its own
ADR and original-read or admitted physical-link replay for every emitted adjacency.
