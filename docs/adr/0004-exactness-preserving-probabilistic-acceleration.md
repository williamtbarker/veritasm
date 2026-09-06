# ADR-0004: Exactness-preserving probabilistic acceleration

- Status: Accepted for an isolated research prototype; not authorized in stable 0.1 assembly output
- Date: 2026-09-03
- Owners: VeritAsm maintainers
- Related research: [probabilistic filters and graphs](../../RESEARCH.md#5-probabilistic-filters-and-graphs),
  [low abundance in high background](../../RESEARCH.md#12-low-abundance-in-high-background),
  [Rust implementation reuse](../../RESEARCH.md#16-rust-implementation-reuse), and the
  [accepted probabilistic-acceleration decision](../../RESEARCH.md#accepted-probabilistic-acceleration-decision)
- Related application boundary: [adventitious-agent application](../ADVENTITIOUS_AGENT_APPLICATION.md)
- Related validation: [probabilistic-prototype falsification](../../VALIDATION.md#probabilistic-prototype-falsification)

Chronology note: this ADR and the initial `src/bloom.rs` prototype first entered this repository
together in commit `ade474f`. The record captures the decision boundary applied to that prototype,
but the repository history does not independently establish that this review preceded the code. No
stable-path integration is authorized. A future implementation change must be preceded by an
accepted amendment or superseding ADR and then satisfy the gates below.

## Context

High-background short-read inputs can contain many distinct error- or background-derived k-mers. An
exact external counter can process them with bounded RAM, but writing every accepted observation to
temporary partitions can dominate disk traffic. A probabilistic structure may reduce or reorder
that work, but only if its uncertainty is prevented from entering graph membership, support counts,
control comparisons, or biological interpretation.

This record separates two mechanisms with different correctness contracts:

1. **Mechanism A, the two-hit sieve**, is a narrowly constrained optimization for exact
   fragment-support counting when the retention threshold is at least two. Its Bloom filters may
   admit extra keys, but must never omit a key that can pass the exact threshold.
2. **Mechanism B, the scout**, is a default-off experimental scheduler. Syncmers, background Bloom
   filters, and Count-Min sketches may change which whole fragments receive compute first. They
   cannot change which fragments eventually reach the exact pipeline.

Neither mechanism is an error corrector, a graph representation, a classifier, a detection method,
or evidence that a sequence is absent from a sample, control, reference set, or biological system.

## Decision drivers

- Preserve exact retained k-mer membership and integer support.
- Make every probabilistic error consume compute rather than biological evidence.
- Preserve fragment-level deduplication across synchronized mates.
- Keep results deterministic across worker counts and platforms.
- Bound and report memory, temporary storage, and extra input passes.
- Retain a simple proof obligation that can be tested against an exact oracle.
- Keep approximate sample/control/background signals out of final evidence fields.
- Avoid complex dynamic filters when deletion and online resizing are not required.

## Options considered

### Option 1: No probabilistic acceleration

Every accepted, fragment-deduplicated canonical k-mer is written to exact external-count
partitions.

- **Benefits:** simplest correctness argument; complete exact pre-threshold histogram; one counting
  pass after parsing.
- **Costs:** potentially high temporary I/O for singleton-heavy data.
- **Decision:** remains the mandatory fallback, the support-one path, and the reference oracle.

### Option 2: Two-hit Bloom superset followed by exact recount

Two standard monotone Bloom filters discover a superset of keys observed in at least two supplied
fragment instances. A complete second pass sends that superset to the exact external counter.

- **Benefits:** can avoid writing most singleton observations; false positives only add exact work.
- **Costs:** requires an immutable input spool and a complete second pass; does not recover exact
  cardinality or support distribution for keys outside the candidate superset.
- **Decision:** selected for an isolated prototype under the gates below. It cannot enter stable 0.1
  assembly output while that contract requires a complete exact pre-threshold histogram and deletion
  ledger.

### Option 3: Counting Bloom, quotient, or cuckoo filter as the final counter

- **Benefits:** compact approximate counts or dynamic membership; quotient/cuckoo structures can
  support deletion or enumeration.
- **Costs:** collisions make counts or membership approximate; cuckoo insertion can fail and
  relocation complicates determinism; deletion is unnecessary; capacity behavior adds proof and
  recovery states.
- **Decision:** rejected. A Count-Min sketch is permitted only in the scout, never as a final count.

### Option 4: Probabilistic graph membership

- **Benefits:** lower graph memory in some workloads.
- **Costs:** a false-positive edge can change branches, unitigs, joins, and every downstream evidence
  field.
- **Decision:** rejected for the exact VeritAsm graph.

### Option 5: Syncmer/Bloom/CMS candidate selection without an exact residual path

- **Benefits:** can concentrate compute on unusual-looking fragments.
- **Costs:** syncmer subsampling and Bloom collisions can miss promotion; incomplete controls and
  references cannot prove novelty or absence.
- **Decision:** rejected. The scout may schedule only, and every fragment remains in the exact
  residual path.

## Decision A: two-hit Bloom sieve

### Eligibility

Mechanism A may run only when all of the following are true:

- support mode is `fragment_instance`;
- `support_unit = supplied_fragment_instance` and mode-neutral `min_support >= 2`;
- the only rule admitting a k-mer to the graph requires that exact threshold or a stricter one;
- no singleton rescue, mercy-k-mer rule, probabilistic correction, or derived-sequence rule can
  retain a key with fragment support below two;
- both passes consume the same complete, checksummed immutable record spool and configuration; and
- the no-sieve exact counter remains available.

It is bypassed for support one, occurrence-support mode, an incompatible retention rule, or any
configuration for which the proof below does not apply. Bypass is a correctness-preserving fallback,
not a warning that permits approximate output. An unavailable, corrupt, or digest-mismatched spool is
a fatal input-state error unless a new immutable snapshot can be made before either counting path
starts; it is not a reason to re-read a potentially changed source mid-run.

### Algorithm

For a selected k, define one evidence event for key `x` as one accepted canonical k-mer after local
deduplication across both reads of one synchronized fragment. Let `F(x)` be the exact number of
supplied fragment instances containing `x`.

The first complete pass maintains two fixed-size, insertion-only Bloom filters:

- `seen_once`, approximating the keys observed previously; and
- `seen_twice`, containing a probabilistic superset of keys believed to have a second hit.

For each fragment, canonical keys are sorted and deduplicated before this update. In input-fragment
order, for every deduplicated key `x`:

1. query `seen_once`;
2. if it answers present, insert `x` into `seen_twice`;
3. insert `x` into `seen_once` unconditionally.

Parallel workers may extract and deduplicate keys, but one coordinator must apply these stateful
updates in fragment order. Concurrent query-then-insert updates are prohibited: two true occurrences
could otherwise both query before either insertion and violate the proof.

The second complete pass again deduplicates per fragment. It routes `x` to the ordinary exact
external counter if `seen_twice` answers present. The external counter stores full canonical keys,
uses checked integer addition, calculates exact `F(x)` for every routed key, and applies the declared
threshold. Bloom estimates never become counts.

Separate filters are required for every k and key-encoding version. Filters from different k values,
support modes, QC configurations, or input-spool digests cannot be reused.

### Proof obligation

For every key `x` with `F(x) >= 2`, its first fragment occurrence inserts it into `seen_once`.
Because a standard insertion-only Bloom filter has no false-negative membership result under a
correct, complete implementation, the second distinct fragment occurrence observes `x` as present
and inserts it into `seen_twice`. The second Bloom filter likewise has no false negatives, so every
occurrence of `x` is routed during the complete recount. The exact counter therefore recovers the
same `F(x)` as no-sieve counting and retains exactly the same keys for every threshold at least two.

A false positive in `seen_once` can promote a singleton into `seen_twice`; a false positive in
`seen_twice` can route another low-support key. Both cases add exact counting work. The exact
threshold removes them and they cannot add a graph edge.

This result depends on all of these premises:

- filters are insertion-only and every requested insertion completes;
- query and insertion use identical key bytes, hash version, dimensions, and seeds;
- no bit is cleared, lost, or read from corrupt state;
- fragment deduplication and QC semantics are identical in both passes;
- the first pass and recount are complete and use the same immutable spool;
- stateful first-pass updates have the ordered coordinator described above; and
- exact counter overflow is an error rather than saturation.

An implementation must restate this argument next to the code and name the tests enforcing every
premise. “Bloom filters usually have no false negatives” is not sufficient evidence.

### Exactness boundary

The sieve preserves the exact retained key set and retained-key counts. It does **not** determine the
exact number of distinct singleton or other non-routed keys. Exact totals available independently of
key identity, such as parsed records, bases, possible windows, accepted windows, and QC rejection
counts, remain reportable.

If a stable schema promises an exact pre-threshold distinct-key count, complete support histogram,
or removal ledger for every below-threshold key, Mechanism A must be disabled unless an independent
exact method supplies those fields. Such values must otherwise be `NA` with a reason code in a
non-equivalence experiment; they must never be filled with Bloom estimates. The byte-equivalence
claim applies only to stable exact fields whose semantics both paths compute.

## Decision B: default-off syncmer scout

### Purpose and isolation

The scout is an experimental scheduling plane over the immutable record spool. It may prioritize
whole fragments, exact-count partitions, or exact follow-up jobs. The evidence plane still consumes
every fragment and computes all final keys, counts, graph structure, control comparisons, and
sequence evidence exactly.

The initial prototype evaluates deterministic closed syncmers at multiple, separately tagged scales,
for example `(k,s) = (21,11), (31,15), (51,19)` when read length permits. Under distinct uniformly
ordered s-mers, their approximate selection densities are respectively `2/11`, `2/17`, and `2/33`.
Observed density, ties, low-complexity oversampling, and skipped short-read scales must be reported.
A density-matched minimizer selector remains a required experimental comparator; the example
parameters are not defaults until that comparison is retained.

For the prototype rule, an accepted k-mer is canonicalized first, every contained s-mer receives the
versioned scout hash, and the k-mer is selected when an endpoint s-mer ties for the minimum hash.
Selecting either tied endpoint, rather than relying on traversal direction or hash-map order, makes
the rule strand-invariant and deterministic. Windows rejected by the exact path's IUPAC or quality
policy cannot enter the scout. Any later rotated, open, or downsampled syncmer rule requires its own
selector version and equivalence tests.

The scout may use:

- immutable Bloom filters over selected keys from an explicitly supplied, versioned, checksummed
  background reference;
- per-sample or pooled-control Count-Min sketches for approximate selected-key fragment prevalence;
  and
- optional per-control Bloom filters for scheduling context.

A counting Bloom filter was considered for prevalence, but its collision-count interpretation is
less direct than the Count-Min sketch's one-sided additive-error contract. Quotient and cuckoo
filters add insertion-failure, relocation, and load-factor behavior without a need for deletion.
They are not selected for either mechanism.

A selected key is deduplicated across both mates before a fragment-level sketch update. If either
mate receives a priority, both mates retain one fragment ordinal and move together. Pair identity and
role validation occurs before the scout.

Priority is a documented lexicographic tuple of observable scout components, not a biological
confidence score. Ties end with input fragment ordinal. A finite credit-based schedule must service
the ordinary residual queue as well as promoted queues, so a collision can add work or delay work but
cannot prevent eventual exact processing.

### Evidence quarantine

- Scout membership, counts, novelty, enrichment, and scores are explicitly approximate.
- No scout value can set a support threshold, delete a read or key, create a graph edge, select a
  graph traversal, qualify a pair link, or populate an exact evidence field.
- Final background membership requires lookup of the full selected key in an exact companion set or
  another declared exact method. Final control counts come from the original control spools.
- “Absent from the supplied exact feature set” does not mean biologically novel or absent from a
  laboratory background.
- A completed scout-on run must have the same core exact scientific artifacts as a scout-off run.
  Scout parameters and results live in a separately versioned experimental namespace, so the overall
  provenance manifest may differ.
- A resource-limited scout run that does not finish the exact residual path has status `incomplete`,
  identifies the unprocessed fragment ranges or partitions, and cannot emit a normal absence or
  successful-completion statement.

## Memory and false-positive sizing

For a standard Bloom filter with `m` bits, `n` distinct inserted keys, and `h` probes, the usual
sizing approximation is

```text
p ~= (1 - exp(-h*n/m))^h
m_min = ceil(-n * ln(p) / (ln(2)^2))
m = round_up_to_64_bits(m_min)
h = round((m/n) * ln(2))
```

At target `p = 10^-3`, this is approximately 14.38 bits per distinct key and 10 probes: ten million
keys require about 17.1 MiB and one hundred million about 171.4 MiB per filter. Mechanism A needs two
filters per k. Multi-scale background indexes need one independently sized filter per scale. The
classical expression is a planning approximation, not a calibrated biological probability; finite
and correlated inputs require empirical false-positive measurement. Word rounding adds at most 63
bits to the theoretical allocation.

Mechanism A sizes `seen_once` for expected distinct accepted keys and `seen_twice` for expected
recurrent keys plus false promotions. If the estimates are wrong, increasing occupancy can approach
a filter that answers present for nearly everything. That degrades I/O savings but not exactness.
Occupancy, allocated bits, probe count, inserted-event count, estimated distinct count, and empirical
held-out false-positive rate are operational diagnostics, never exact evidence.

For a classic Count-Min sketch with total event count `N`, width `w`, and depth `d`, the conventional
parameters are

```text
w = ceil(e / epsilon)
d = ceil(ln(1 / delta))
```

Under the algorithm's hash assumptions, a query overestimates by at most `epsilon*N` with probability
at least `1-delta`. With `epsilon = 10^-5`, `delta = 10^-4`, and `u32` counters, `w = 271829` and
`d = 10` consume about 10.4 MiB per scale and input aggregate. The scout must report dimensions,
total updates, saturation, and the fact that collision error is one-sided only before counter
saturation. It may use the estimate to promote work, never to suppress exact processing or report
final support.

Every structure is subject to a user-visible aggregate memory limit. Allocation overflow, dimension
overflow, or an impossible budget triggers bypass for Mechanism A or disables/fails the explicitly
requested scout before any scientific output is committed.

## Hash and serialization contract

Probabilistic state must not use Rust's `DefaultHasher`, process-random seeds, pointer values, host
endianness, worker-local iteration order, or an unversioned dependency default.

The first prototype uses the same dependency-reviewed SHA-256 implementation as bundle integrity; it
does not invent a hash or use a dependency's unstable default. The algorithm identifier is
`sha256-double-hash-v1` and the seed is 32 zero bytes in version 1. For a full 16-byte big-endian
canonical-key encoding, compute
`SHA256("veritasm:bloom-probes:v1\0" || structure_domain_length_u16_le || structure_domain ||
seed_32 || key_u128_be)`. Interpret digest bytes 0..8 and 8..16 as `h1` and `h2`, respectively, in
little-endian order. Mechanism A uses the exact ASCII structure domains
`veritasm:two-hit:seen-once:v1` and `veritasm:two-hit:seen-twice:v1` with no trailing NUL; the encoded
length provides framing. A packed key uses the active `2*k` bits right-aligned in the `u128`, with all
unused high bits zero, before fixed 16-byte big-endian encoding.

Bloom bit count `m` is a `u64` with `m>=64` and `m%64=0`; `m/64` must convert to `usize`, its byte
allocation must fit checked arithmetic, and the allocation must be acquired from the declared memory
budget. The initial position is `h1 mod m`; the initial step is
`1 + (h2 mod (m-1))`, then advances cyclically through `1..m-1` until `gcd(step,m)=1`. Probe `i` is
`(position + i*step) mod m`, evaluated with checked `u128` intermediate arithmetic. Probe count is in
`1..=64`; repeated positions are an invariant failure. Structure domains distinguish `seen_once`,
`seen_twice`, each syncmer scale, every background filter, and each Count-Min row. Exact equality
always compares the full packed key, never its digest. Cross-platform golden vectors and NIST SHA-256
vectors are release-blocking tests.

The first Mechanism-A prototype is same-process and in-memory only. It cannot load or reuse a serialized
`seen_once` or `seen_twice`; this prevents a checksummed but incomplete first pass from masquerading as
complete state. A later serialization design requires a separately accepted atomic sealed-completion
trailer containing processed-fragment count equal to the verified spool trailer. If a scout filter or
sketch is serialized, its binary representation contains:

- fixed magic bytes and schema version;
- mechanism and structure kind;
- canonical-key encoding version, k, and where applicable s and selection rule;
- support/QC configuration digest;
- bit count or width/depth, probe count, counter width, hash identifier, and seed;
- immutable input, control, or reference digest and declared identity;
- payload length, inserted-event count, occupancy or saturation diagnostics; and
- a checksum trailer containing SHA-256 of the header and payload, excluding the trailer itself.

Integer fields and `u64` bit words have specified little-endian encoding; unused tail bits are zero.
Readers validate all dimensions and lengths before allocation. The checksum detects accidental
corruption under the local cache threat model; it is not authentication against a writer that can
replace both payload and checksum. Unknown versions, checksum failures,
truncation, source/config mismatch, or impossible values make scout state unusable. An explicitly
requested scout fails or restarts without scout state. A same-process Mechanism-A construction failure
may fall back to no-sieve exact counting from the immutable spool only before any scientific output is
committed; it never attempts partial probabilistic recovery.

A minimal fixed-size Bloom filter is implemented locally as a bounds-checked `Vec<u64>` in safe Rust
because the reviewed candidates do not freeze this exact hash, bit order, serialization, and Rust 1.85
contract. Bit index `j` addresses word `j/64` and mask `1u64 << (j%64)`. No deletion, resizing,
concurrent mutation, or unsafe code exists. This does not justify a custom hash primitive, quotient
filter, cuckoo filter, scalable filter, or final probabilistic count.

The prototype API has a 64-MiB default accounting ceiling and an explicit caller-supplied-budget
constructor. A two-hit coordinator admits both filters against one budget before either large
allocation, accounts actual vector capacities, and bounds its fragment-key and candidate copies.
Domain, probe-vector, key-copy, and candidate-vector reservations use fallible allocation; arithmetic
overflow and budget exhaustion are typed resource errors. The accounting covers prototype-owned
structs, vector capacities, and one method's scratch, but not allocator metadata, caller-owned input,
thread stacks, or concurrent calls made through borrowed diagnostic filter references. The primitive
therefore remains experimental even after its local collision/oracle and boundary tests pass. It is
not an adventitious-agent detector, an absence filter, or evidence of operational benefit.

## Determinism and failure behavior

- Parallel extraction returns fragment-indexed key vectors; all stateful Mechanism-A updates occur in
  input order.
- Mechanism A never constructs shard-local filters and never unions `seen_once` or `seen_twice`:
  ordered query-then-insert updates occur only in the single coordinator. A scout-only Bloom union may
  use bitwise OR when all shards represent the same declared set semantics, and any parallel sketch
  reduction uses checked integer addition in a fixed order. Output is never serialized from hash-map
  iteration order.
- A full filter is a performance failure, not a scientific failure: Mechanism A may route everything
  to exact counting.
- No Bloom or sketch counter silently wraps. Exact count overflow is a typed fatal error.
- First-pass, second-pass, spool-checksum, exact-counter, temp-space, or output failures commit no
  result bundle and do not modify an existing destination.
- Temporary probabilistic files are untrusted caches. Their loss can cost compute but not evidence.
- No runtime reference download is permitted. Scout background indexes are explicit optional inputs
  with version and checksum.

## Invariants

1. With an eligible threshold and a complete run, every key retained by no-sieve exact counting is
   retained with the same integer count by Mechanism A.
2. Mechanism A never supplies graph membership or support directly; the exact recount does.
3. Support one, occurrence mode, singleton rescue, and incompatible configurations bypass the sieve.
4. Bloom false positives can increase exact work but cannot add a retained graph edge after exact
   thresholding.
5. The scout changes scheduling only, and mates cannot be split.
6. Every fragment remains reachable through the exact residual path.
7. Probabilistic sample, control, and background values never enter final exact evidence.
8. Thread count cannot change filter payloads, scout priority ties, exact results, or retained-stream
   bytes for identical inputs and parameters.
9. An incomplete residual pass cannot produce a completeness, detection, or absence claim.

## Validation and falsification

### Mechanism A correctness gates

- **Exhaustive small-universe oracle:** enumerate short key streams and fragment boundaries; compare
  no-sieve and sieve exact retained streams for thresholds 2 and above.
- **Property test:** for every generated stream, the exact set `{x | F(x) >= 2}` is a subset of the
  `seen_twice` membership set before recount.
- **Byte-equivalence oracle:** sorted retained `(full_key, exact_support)` streams are byte-identical
  with and without the sieve. All downstream core scientific artifacts must consequently match.
- **Fragment semantics:** cover repeats within one read, mate overlap, the same key in both mates,
  duplicate record instances, multiple lanes, reordered workers, and lane boundaries.
- **Collision adversary:** use tiny and nearly full filters, deliberately colliding hash vectors, and
  an all-ones `seen_twice`; exact output must still match while I/O may regress.
- **Coordinator adversary:** split two fragments carrying the same key across extraction workers; the
  ordered coordinator must promote it, and a deliberately shard-local/OR oracle must demonstrate the
  forbidden false negative.
- **Eligibility:** support one, occurrence support, singleton rescue, and mismatched configuration
  must demonstrably take the no-sieve path.
- **Failure injection:** prove that Mechanism-A state cannot be loaded from a partial serialized cache;
  change the spool between passes, exhaust temporary space, and fail exact-counter writes; no partial
  or prior output may be changed. Scout serialization separately covers truncation and corruption.
- **Determinism:** filter bytes, retained streams, and scientific artifacts match across worker and
  batch counts and on x86-64 and AArch64.

Any missing threshold-eligible key, changed exact support, or changed downstream sequence is an
automatic rejection, not an acceptable empirical error rate.

### Mechanism B falsification gates

- Scout-on and scout-off runs must produce byte-identical core exact FASTA, GFA, counts, graph
  transforms, and read/pair evidence after complete processing.
- Inserted Bloom keys have zero observed false negatives; held-out exact nonmembers measure empirical
  false positives with a predeclared confidence bound.
- Count-Min estimates never fall below exact counts before declared saturation; collision-crafted
  streams may change only promotion.
- One mate with promoted tokens must co-route its validated mate; missing, reordered, reversed, and
  conflicting roles fail before scheduling.
- A true low-abundance component whose every selected token is a Bloom false positive must remain in
  the residual path and appear in exact output when the no-scout run retains it.
- Density-matched syncmers, minimizers, and an all-k-mer oracle are compared under substitutions,
  indels, uneven quality, short reads, low complexity, adapters, index hopping, PCR duplicates, and
  incomplete or contaminated controls/background references.
- Truth-known mixtures vary absolute target fragments and background from 10-fold through
  10,000-fold. Report priority enrichment and time to first **exactly verified** candidate separately
  from final assembly accuracy.
- Forced resource interruption yields an `incomplete` status and an exact residual-work ledger, with
  no absence statement.

## Acceptance, rejection, and deletion criteria

Mechanism A may become a stable optimization only after all exactness gates pass and a retained,
predeclared benchmark shows material temporary-I/O or wall-time benefit on at least one
singleton-heavy workload without an undisclosed regression on recurrent-key workloads. Its initial
engineering target is at least 25% lower partition bytes or 10% lower wall time, with no more than
10% wall-time regression on the declared unfavorable workload. These are go/no-go experiment
thresholds, not performance claims.

Mechanism A is disabled or deleted if:

- any exact retained stream or downstream scientific artifact differs from the no-sieve oracle;
- exact below-threshold distinct statistics become a required stable product field and cannot be
  supplied independently;
- the extra pass fails to produce a retained workload-specific benefit;
- serialization/hash behavior is not stable on supported targets; or
- operational complexity makes the no-sieve failure path less reliable.

Mechanism B remains default-off and experimental. Retain it only if a predeclared high-background
suite shows at least a twofold improvement in time to an exactly verified candidate on at least one
declared workload, no more than 15% total wall-time overhead on its declared unfavorable workload,
bounded sketch memory, and exact-output equivalence. Report every dataset, including those with no
gain. Delete the scout if it changes exact evidence, starves residual work, produces unstable
priority order, or fails to improve the declared scheduling objective.

## Consequences

### Positive

- Mechanism A has a narrow, inspectable correctness proof: false positives become exact extra work.
- The exact graph and evidence model stay independent of probabilistic membership.
- The scout can explore time-to-result improvements without converting ranking into detection.
- Fixed serialization and hash contracts make caches and adversarial tests reproducible.

### Negative

- Mechanism A adds a complete input-spool pass and cannot provide exact singleton cardinality by
  itself.
- Bloom sizing errors can remove all I/O benefit while consuming memory and CPU.
- The scout adds substantial validation, schema, and control-management surface without improving
  final assembly correctness.
- Multi-scale filters can exceed the memory saved by reduced exact work.

### New risks

- A concurrency race could invalidate the two-hit proof.
- A schema may accidentally label approximate diagnostics as exact evidence.
- Users may interpret scheduling “novelty” as biological novelty or detection.
- Incomplete or contaminated controls and references can systematically distort priority.
- Systematic sequencing errors, adapters, low-complexity sequence, and duplicate record instances can
  dominate promoted work.

## Reversal plan

Both mechanisms are separable modules in front of the unchanged exact counter. Mechanism A falls
back to the no-sieve stream; Mechanism B falls back to input-order scheduling. Removing either must
require no graph, sequence, evidence-schema, or migration change beyond retiring its explicitly
versioned operational/experimental metadata.

## Evidence anchors

- Bloom BH. “Space/time trade-offs in hash coding with allowable errors.” *Communications of the
  ACM*. 1970. <https://doi.org/10.1145/362686.362692>
- Christensen K, Roginsky A, Jimeno M. “A new analysis of the false-positive rate of a Bloom filter.”
  *Information Processing Letters*. 2010. <https://doi.org/10.1016/j.ipl.2010.07.024>
- Cormode G, Muthukrishnan S. “An improved data stream summary: the Count-Min sketch and its
  applications.” *Journal of Algorithms*. 2005.
  <https://doi.org/10.1016/j.jalgor.2003.12.001>
- Edgar R. “Syncmers are more sensitive than minimizers for selecting conserved k-mers in biological
  sequences.” *PeerJ*. 2021. <https://doi.org/10.7717/peerj.10805>
- Bradley P et al. “Ultrafast search of all deposited bacterial and viral genomic data.” *Nature
  Biotechnology*. 2019. <https://doi.org/10.1038/s41587-018-0010-1>
- Pell J et al. “Scaling metagenome sequence assembly with probabilistic de Bruijn graphs.”
  *Proceedings of the National Academy of Sciences*. 2012.
  <https://doi.org/10.1073/pnas.1121464109>
