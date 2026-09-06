# ADR 0014: Isolate an exact minimizer-partition correctness foundation

- Status: accepted for an experimental in-memory substrate; not accepted for the stable pipeline
- Date: 2026-09-04
- Scope: `veritasm::experimental::partitioned_dbg` only

## Context

The stable engine performs exact disk-backed prefix counting and then materializes a retained
de Bruijn graph. The production-redesign review recommends eventually replacing that data plane with
an exact minimizer/super-k-mer partitioned implementation. GGCAT and BCALM2 are useful precedents,
but neither a performance result nor the word “minimizer” establishes that a proposed partition is
safe for VeritAsm's canonical bidirected edge identity, ambiguity rules, deterministic artifacts, or
resource contract.

The project already has an isolated four-word exact DNA representation through k=127. Before
designing a spill format or parallel compactor, it needs a small implementation that makes ownership,
collisions, window conservation, and output order executable invariants. This slice is that
correctness substrate. It is not a performance claim and is not used by the CLI.

## Canonical-edge ownership is safe under the chosen mapping

Let `C(x) = min(x, reverse_complement(x))` under the unsigned packed-key order. For an observed
k-mer `x`, this module first computes the complete exact key `K = C(x)`. It decodes that one canonical
spelling and considers every length-m substring. Each substring is independently canonicalized. The
owner is the minimum tuple

```text
(complete canonical m-mer, zero-based position)
```

where the position tie is leftmost. Because `C(x) = C(reverse_complement(x))`, owner selection is a
pure deterministic function of the complete canonical graph edge. Both orientations of an observed
edge therefore have the same owner. Self-reverse-complemental k-mers are not a special case: their
single canonical value is the function input. Full k-mer identity is retained separately, so an
m-mer owner never substitutes for graph-edge identity.

This initial oracle deliberately uses exact lexicographic minimizers. It does not yet implement the
portable-hash-first ordering proposed in `docs/RUST_FOUNDATION_AUDIT.md`. Exact lexicographic
selection is easier to falsify and makes the ownership proof independent of a newly invented hash.
It may be distributionally skewed; promotion requires measuring that skew and freezing any
alternative portable routing order in a later ADR.

## Virtual-bucket mapping and collisions

The virtual bucket is

```text
unsigned_256_bit_value(complete_minimizer) mod virtual_bucket_count
```

computed word by word with checked-width portable arithmetic. This is not a hash and has no secret or
host-endian state. Multiple exact minimizers can intentionally have the same residue. Every edge-count
row retains the complete 32-byte minimizer and complete 32-byte canonical k-mer. Sorting,
deduplication, support addition, invariant checking, and graph identity compare the complete k-mer;
no bucket number, minimizer, hash, or fingerprint is accepted as equality evidence. A routing
collision can affect bucket load only.

The result order is frozen as follows:

1. source segments sort by `(source_ordinal, segment_ordinal)` and duplicate coordinates fail;
2. super-k-mer spans retain that source order and increasing window coordinates;
3. exact edge counts sort by `(bucket_id, complete canonical k-mer)`; and
4. nonempty bucket ranges sort by numeric bucket ID.

The implementation is serial. Calling it under different process-local Rayon pools has no effect;
tests nevertheless compare one- and four-thread-pool invocations to guard against accidental global
pool dependence. A later parallel implementation must assign immutable virtual-bucket ordinals and
merge by the order above, never worker completion order.

## Super-k-mer and count conservation

Input is explicitly post-QC. Every segment byte must be A/C/G/T, case-insensitively. Callers must split
at every IUPAC ambiguity and every rejected-quality boundary. The module rejects an unsplit symbol;
it does not reinterpret ambiguity as A and has no quality parameter whose absence could be mistaken
for a quality decision.

Within a segment, a `SuperKmerSpan` is a maximal consecutive run of windows with the same complete
minimizer. It stores immutable source coordinates, first window, window count, complete minimizer,
and bucket. A segment ledger stores the expected number of windows and its exact range of spans.
Validation requires spans to start at zero, touch without gaps or overlap, and end at the expected
window count. Thus every accepted occurrence is represented once. Independently, the sum of exact
edge counts and the sum of bucket occurrence counts must both equal the same accepted-window total.

The span refers back to source bases instead of copying a super-k-mer string. This is suitable only
inside the current in-process experiment. A disk record cannot rely on an unauthenticated ordinal; a
future format must include or bind to a checksummed immutable source/run identity.

## Resource admission and arithmetic

Configuration independently limits input bases, accepted windows, super-k-mer spans, distinct exact
keys, and projected owned payload bytes. Before edge materialization, the code computes window totals
and a checked upper bound for:

- the segment-order and segment-ledger vectors;
- one complete exact edge-observation record per accepted window;
- the configured number of super-k-mer records;
- the maximum possible nonempty bucket ranges;
- the largest transient rolling-scan key vector; and
- the rolling validity rings and one decoded-k-mer minimizer workspace.

Every multiplication, sum, counter, range, and `u64`/`usize` conversion is checked. Vectors use
fallible exact reservation. Exact observations are sorted and run-length encoded in place so a
second full count vector is not needed. `max_accounted_bytes` is intentionally named as a projected
owned-payload bound: it excludes allocator metadata and dependency/process state and is not a whole-
process RSS guarantee. Promotion to a production data plane requires phase-specific RSS evidence and
a byte-admitted spill/merge implementation.

## Evidence in this slice

Focused MSRV tests establish:

- exact edge-count equality with the independent rolling wide-k scanner over generated sequences;
- reverse-complement equality of the complete partitioned edge multiset;
- rolling minimizer equality with an independent slice encoder at k/m values crossing 64-bit word
  boundaries through k=127;
- leftmost tie behavior and strand-invariant ownership;
- exact per-segment super-k-mer coverage with no lost or duplicated window;
- intentional same-bucket collisions that retain distinct complete minimizers and edges;
- identical results for reordered input and invocations inside one- and four-thread Rayon pools; and
- explicit ambiguity, duplicate-coordinate, count-limit, span-limit, and byte-admission failures.

These are software-correctness results on an in-memory oracle. They do not establish assembly
accuracy, speed, sensitivity, scalability, or better reconstruction than the stable engine or any
external assembler.

## Explicitly unimplemented and promotion blockers

This ADR does not authorize stable-pipeline integration. The following remain separate design and
validation work:

1. an ambiguity/quality-aware streaming segment producer wired to the immutable spool;
2. amortized rolling or deque minimizer selection profiled against this exact oracle;
3. a versioned checksummed super-k-mer/run format with explicit endian and key-width domains;
4. hard byte bounds for read/write buffers, compression, open files, disk, merge fan-in, and cleanup;
5. exact external sort and reduction for occurrence and `(k-mer, fragment ordinal)` support;
6. process-local parallel bucket scheduling with deterministic merge evidence;
7. doubled/bidirected endpoint materialization, palindrome boundaries, local compaction, and global
   maximality proofs;
8. graph transformations, pair evidence, read-backed auditing, and conservation ledgers;
9. skew/adversarial-input measurements and a justified portable routing order; and
10. Linux and Apple Silicon peak-RSS, throughput, determinism, fault-injection, and scientific
    reconstruction comparisons.

## Alternatives rejected for this slice

- **Finite fingerprint as edge identity:** a collision would silently merge graph edges.
- **Bucket or minimizer as deduplication identity:** different full k-mers legitimately share both.
- **Raw-orientation minimizers:** ownership could change with read strand unless additional orientation
  state were proved and frozen.
- **Treating ambiguity as A:** creates unsupported exact windows and contradicts the stable scanner.
- **Hash-first minimizer order without evidence:** likely useful for load balance, but it introduces a
  new algorithm/version and collision tie contract before the exact oracle exists.
- **Immediate stable replacement:** no authenticated spill format, compactor, end-to-end resource
  envelope, or reconstruction benchmark exists yet.

## References

- Chikhi et al. “Compacting de Bruijn graphs from sequencing data quickly and in low memory.”
  *Bioinformatics* (BCALM2), <https://doi.org/10.1093/bioinformatics/btw279>.
- Cracco and Tomescu. “Extremely Fast Construction and Querying of Compact De Bruijn Graphs with
  GGCAT.” *Genome Research*, <https://doi.org/10.1101/gr.277615.122>.
- VeritAsm build-versus-buy and data-plane review: `docs/RUST_FOUNDATION_AUDIT.md`.
- Production-redesign evidence ledger: `docs/research/2026-09-04-production-redesign.md`.
