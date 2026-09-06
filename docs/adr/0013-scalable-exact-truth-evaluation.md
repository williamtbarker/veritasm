# ADR 0013: Scalable exact truth evaluation

- Status: Accepted
- Date: 2026-09-04
- Scope: `veritasm-evaluate` only

## Context

The version-1 truth evaluator retains one heap-heavy row and three exact-placement sets for every
eligible assembly adjacency. It also finds each placement by cloning both truth orientations and
scanning every truth start for every query. Fixed caps of 250,000 retained rows, 1,000,000 retained
placements, and 128 MiB of projected raw-table data prevent evaluation of an otherwise ordinary
one-megabase exact assembly. Raising those caps would exchange an explicit failure for unbounded
memory and impractical work.

The fitting aligner has an independent problem: a one-megabase query against one-megabase truth
would require about one trillion dynamic-programming cells per orientation even when the assembly
is an exact substring of truth.

## Decision

### Exact fixed-window occurrence index

For a configured flank length `L`, the evaluator builds immutable sorted occurrence indexes for
literal windows of lengths `L` and `2L` over every truth molecule in both orientations. Keys are
lossless two-bit DNA values, never probabilistic hashes. Linear molecules contribute every valid
window start; circular molecules contribute exactly one modular window for every oriented truth
start. Occurrence records sort by key, molecule-manifest order, `+` before `-`, and oriented start.

An equal-range lookup returns the exact placement count without allocating or enumerating a set.
When the count is one, the occurrence itself supplies the unique placement needed by the existing
false-junction and primary/minor-chimera rules. This is the complete sufficient information used by
the classifier. It deliberately does not serialize every placement of an ambiguous repeat.

The first implementation accepts `1 <= L <= 31`, so both `L` and `2L` fit losslessly in `u128`.
This explicit validation limit is preferable to hashing longer contexts or silently changing
semantics. Index memory is admitted conservatively before allocation, occurrence-vector reservation
is fallible, and the exact vector capacities are checked again after reservation. Both the
conservative projection and the post-reservation accounted capacity are recorded.

### Streaming and evidence-row modes

Junction metrics are accumulated over every eligible adjacency in all modes. Rows are written
directly to a staging artifact and are never retained as a whole:

- `all` writes one compact classification-sufficient row per eligible adjacency.
- `non-correct` writes every false or indeterminate row and aggregates correct rows.
- `summary` writes no per-adjacency rows and retains exact aggregate metrics only.

The selected mode is explicit in configuration and the result. No mode may change automatically
after a resource limit is reached. Each run hashes the same canonical logical row stream before
mode selection, so modes can be checked for identical scientific classification. The result records
logical, emitted, and omitted-correct row counts and the logical-stream digest. A configured output
byte limit causes an atomic failure; it never produces a prefix presented as a complete artifact.

The version-2 junction row contains the exact compatible-, left-, and right-placement counts and a
placement only when that count is one. These fields preserve every fact consumed by the classifier,
while avoiding intrinsically unbounded repeat-placement lists. Truth, assembly, and artifact
checksums retain replay identity.

### Exact-substring alignment fast path

Before allocating a fitting-alignment matrix, the evaluator searches every truth molecule and both
orientations for exact contig placements. If at least one exists, it emits a zero-edit alignment for
the deterministic first target under the existing molecule/strand/start tie order and records the
number of tied molecule/strand targets. Circular truth is searched through one additional query
minus one revolution while accepting starts only in the first truth revolution.

The search meters every oriented truth base inspected across all contigs against an explicit
configuration limit and records the observed count. Its prefix table has a separate fixed byte cap.
These bounds prevent the fast path itself from becoming an unreported product-of-inputs workload.

If no exact placement exists, the existing bounded fitting aligner remains the fallback. This fast
path makes long exact assemblies evaluable; it does not claim to solve scalable approximate or
split alignment.

### Compatible-recovery interval algebra

The compatible-recovery metric definition is unchanged, but its exact-placement representation is
not. One placement becomes at most two sorted half-open forward-coordinate intervals. Upper-bound
coverage merges monotonically ordered placement intervals before writing the persistent mask.
Linear lower-bound coverage intersects one interval per placement. Circular lower-bound coverage
starts with one fallibly allocated truth-length bit mask and clears the monotone union of each
placement's complement; by De Morgan's law, the remaining coordinates are exactly the intersection
of every compatible circular placement.

For `Q` contig bases used to build search prefix tables, `T` searched oriented truth bases, `P`
placements, and `B` mask-coordinate operations, the pass is `O(Q + T + P + B)` and never allocates or
sorts a contig-length coordinate collection per placement. The existing exact-scan limit continues
to meter discovery. A private work counter makes placement visits and mask operations testable
without changing result schemas. Exhaustive linear and circular short-sequence tests compare every
mask and record count against the former brute-coordinate definition; a long homopolymer kill test
freezes the non-product work bound.

### Determinism and atomicity

The initial implementation is serial. Truth and assembly FASTA bytes are read, bounded, hashed, and
parsed from one owned snapshot per path. One held descriptor anchors the dataset root; relative
components are traversed with no-follow directory opens, and checksummed artifacts are opened once as
non-symlink regular files and verified against that same handle. Assembly records retain FASTA
order, boundaries increase
numerically, truth targets retain manifest order, and occurrence ordering is fully specified.
Checked integer arithmetic is used for work, byte, and metric counters. Output is written only
inside the existing staging directory and is committed only after artifact rendering, cross-field
summary validation, and checksum-manifest construction succeed. Summary validation recomputes the
evaluation identity over every recorded input and configuration field, validates record/base,
alignment, coverage, and adjacency domains, and recomputes derived ratios and integer-defined QV
fields instead of trusting serialized values or platform `f64` logarithms. It also checks fixed
algorithm/configuration descriptors. Versioned schemas are shipped in the source package and
exercised by conformance tests; they are not copied into each evaluator result.

## Rejected alternatives

- Raising the retained-row or retained-placement caps: memory and repeated-truth work remain
  proportional to the product of assembly and truth size.
- A Bloom filter: false positives cannot supply exact placement cardinality or unique coordinates,
  and therefore cannot classify junctions by itself.
- Silent row truncation or automatic downgrade to summary: this obscures missing evidence.
- Treating an exact-substring shortcut as a general large-genome aligner: assemblies with edits or
  structural differences still require a separately designed indexed/banded validation path.

## Consequences

Version-2 result and junction schemas are required because the raw evidence representation and
resource record change. Aggregate junction metric definitions remain unchanged. Repeat-rich inputs
become bounded by index size rather than by the number of serialized placements. Approximate
one-megabase alignments remain explicitly unsupported until an independent scalable aligner is
frozen and validated.

The compatible-recovery refinement requires no schema or identity version because it changes only
the internal exact computation and is regression-checked for bit-for-bit metric equality. Persistent
recovery storage remains three bit masks over the truth-base universe; circular lower-bound
evaluation adds at most one temporary truth-molecule-length bit mask for the current contig. These
allocations are fallible and indirectly bounded by the fixed truth-FASTA ceiling, but their allocator
overhead is not a hard process-RSS guarantee.
