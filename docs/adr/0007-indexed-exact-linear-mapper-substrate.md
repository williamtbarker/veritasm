# ADR 0007: Add a verified indexed exact-mapper substrate without changing stable audit output

- Status: accepted as an experimental standalone substrate
- Date: 2026-09-03
- Current-state note: ADR 0011 subsequently promoted the fixed-q15 implementation into the stable
  construction-read audit and retained exhaustive scanning only as a differential oracle. The
  pre-promotion statements below are historical decision context, not current pipeline status.

## Context

The stable 0.1 construction-read audit tests every possible read interval on every emitted linear
unitig. That implementation is a useful oracle, but its read-by-target scanning work is unsuitable
for a larger assembly. Replacing it inside the pipeline without first proving semantic equivalence
would risk false uniqueness, changed pair evidence, unaccounted index memory, and drift between the
mapper metadata hardcoded in the bundle schema and the algorithm actually used.

The mapping contract is the complete set

`(unitig_id, zero_based_half_open_start, zero_based_half_open_end, strand)`

for exact zero-mismatch matches against all and only emitted linear unitig strings. A palindromic read
has distinct `+` and `-` groups at the same interval. Groups are ordered by full unitig-ID bytes,
numeric start, then `+` before `-`. The configured candidate limit counts fully verified placement
groups, not seeds, index postings, or failed comparisons. The verified group at `limit + 1` makes the
result indeterminate and invalidates the retained prefix.

## Decision

Add `indexed_mapper` as a public but explicitly experimental module. It builds a sorted literal
q-gram posting index over each linear target independently. The default q is 15 and the configurable
implementation domain is 1 through 31, allowing every seed to retain full two-bit identity in a
`u64` without hashing.

For each read orientation, the mapper chooses the query q-gram having the shortest exact posting
range, breaking ties by the lowest query offset. A posting implies one candidate start. Every
in-bounds candidate is compared with the complete oriented read before it can become a placement
group. Because every exact placement contains every q-gram of the query at its corresponding offset,
the selected literal seed cannot omit an exact placement. Choosing one seed offset per orientation
also prevents a group from being rediscovered through multiple query seeds. Reads shorter than q use
the exhaustive matcher.

The two orientation streams are merged in canonical target/start/strand order before the candidate
limit is applied. This preserves the stable distinction between the two strands of a self-reverse-
complementary read. Index construction and per-read output have separate conservative memory
admission checks. A memory failure is fatal `resource_memory`; it is never converted to unmapped,
unique, or candidate-limit indeterminate.

Work counters describe the search implementation rather than scientific evidence. Establishing the
canonical two-orientation merge may examine one lookahead candidate from each orientation; after the
cap-triggering group, no additional full-read verification is performed. The placement cap does not
bound posting-list inspection or runtime.

The stable `audit` pipeline continues to use `exhaustive_zero_mismatch/1`. This ADR does not authorize
changing `run.json`, pair evidence, schemas, FASTA/GFA output, or the audit phase's current memory
shares. Pipeline integration requires a separate measured change that jointly accounts for the
index, pair accumulator, current fragment, placement buffers, and finish-time records, and must
centralize mapper metadata currently duplicated in bundle rendering.

## Evidence required and retained

- exhaustive equality to an independent brute-force oracle over all nonempty A/C/G/T strings through
  length four, multiple seed lengths, both strands, and all relative read/target lengths;
- randomized property equality over multi-target inputs, reordered targets, varied limits, and seed
  lengths;
- direct compatibility checks against the stable audit mapper;
- explicit tests for palindromes, exact-limit versus `limit + 1`, false seed hits, short-read fallback,
  closed-walk exclusion, segment boundaries, duplicate IDs, and memory failure; and
- deterministic work counters separating seed hits, full verifications, and verified groups, plus
  equality when one immutable index is queried from one and four worker threads.

`cargo run --release --example profile_indexed_mapper` runs a deterministic engineering probe that
compares complete indexed and independently brute-forced placement sets before reporting index-build,
query-time, and verifier-work measurements. It is intentionally not a biological benchmark or a
release gate.

## Consequences and limitations

- Exact absent and unique-read searches can require far fewer full-read comparisons than the stable
  scan, but index construction consumes memory proportional to total linear-target q-grams.
- Repetitive seeds can still produce near-exhaustive candidate work. Candidate-limit behavior bounds
  retained groups, not seed hits or runtime.
- The mapper remains zero-mismatch and segment-local. It does not perform correction, graph-path or
  seam traversal, alignment, biological-origin inference, or independent validation.
- Work-counter reductions are algorithmic measurements, not runtime, memory, accuracy, or production
  claims. Wall-clock and peak-RSS comparisons must be run on preregistered workloads before promotion.
- Any future mismatch-capable design needs a new completeness proof (for example, disjoint `e + 1`
  q-grams plus full edit verification) and must not reuse this exact-match proof by analogy.
