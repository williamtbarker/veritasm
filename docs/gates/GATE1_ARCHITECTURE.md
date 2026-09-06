# Gate 1: adversarial architecture review

**Status: PASS**  
**Initial blocked review:** 2026-09-03  
**Resolved independent re-review:** 2026-09-03  
**Scope:** `ARCHITECTURE.md`, `docs/PRODUCT_CONTRACT.md`,
`docs/OUTPUT_SCHEMA.md`, `docs/SCIENTIFIC_LIMITATIONS.md`, and ADRs 0001--0005.  
**Target:** VeritAsm 0.1 on Rust 1.85, Linux and Apple Silicon macOS.

This is a document and implementability review. It is not evidence that any described component is
implemented, correct, performant, or portable. The original blocked findings and work plan below are
preserved as review history; the final resolution section records how the current normative contracts
closed them.

## Executive decision

The proposed core is scientifically defensible: exact full-key counting, an immutable observation
spool, a retained exact graph, topology-preserving unitigs, non-joining pair evidence, and explicit
limitations are appropriate foundations for a serious first release. The prohibition on probabilistic
graph membership and probabilistic final evidence is especially important and passes review.

The current documents are not yet a single implementable contract. There are blocking contradictions
in support-mode semantics, Bloom-sieve exactness, duplicate-record handling, removal journaling,
deterministic artifact membership, and destination replacement. External counting omits a bounded
global-order merge. Closed graph walks do not yet have mutually consistent FASTA, GFA, coordinate, and
read-audit semantics. Pair states and `pair_links.tsv` are too underspecified to prove that reported
counts are exact or that a `J` record has a defensible distance meaning.

A feasible serious 0.1 should therefore ship the exact no-sieve, single-k path first; exact
zero-mismatch read placement; pair annotations that never change sequence; a non-deleting profile plus
an explicit absolute retention threshold; and a verified no-replace bundle transaction. The Bloom
sieve, scout, tip deletion, multi-k concordance, mismatch-tolerant mapping, and GFA `J` distances should
remain unavailable or explicitly experimental until their respective contracts and tests pass.

## Gate criteria

| ID | Criterion | Status | Basis and required exit evidence |
|---|---|---|---|
| G1-01 | Neutral scientific scope and claim boundary | **PASS** | The product and limitations consistently reject organism detection, biological absence, molecular circularity, global haplotypes, forced paths, and clinical or regulatory claims. |
| G1-02 | Exact key identity and checked final support | **BLOCKED** | Full `u128` identity and checked `u64` counts are sound, but fragment/occurrence retention and profile parameters conflict, and the Bloom path conflicts with the promised complete histogram. Resolve B01 and B02. |
| G1-03 | Truly bounded ingestion and external counting | **BLOCKED** | Record/spool/graph caps are identified, but cross-partition global ordering, merge fan-in, file-descriptor use, run-manifest growth, and aggregate worker memory are not bounded. Resolve B03 and R03. |
| G1-04 | Exact graph compaction and closed-walk semantics | **BLOCKED** | Branch preservation is sound, but cyclic spelling, rotation, support-vector length, GFA overlap, and circular read placement do not form one specification. Resolve B06. |
| G1-05 | Conservative, exact read and pair evidence | **BLOCKED** | Pairs cannot alter unitig sequence, but placement states, indeterminacy propagation, link canonicalization, library orientation, endpoint eligibility, gap arithmetic, and output columns are incomplete. Resolve B07. |
| G1-06 | Every filtering/transformation operation is measurable | **BLOCKED** | Aggregate stage fields exist, but the promised removed-key journal has no output artifact, and support retention is outside the documented transform ledger. Resolve B05. |
| G1-07 | Deterministic stable outputs | **BLOCKED** | Stable reductions and total ordering are good principles, but the core artifact set, execution telemetry, Bloom telemetry, manifest membership, shortened-ID collision rule, and multi-k directory layout are not frozen. Resolve B08. |
| G1-08 | Failure-atomic, no-overwrite result transaction | **BLOCKED** | Same-parent staging is appropriate, but check-then-`rename` is not a no-replace primitive and the post-rename error boundary is undefined. Resolve B09. |
| G1-09 | Probabilistic mechanisms cannot change evidence | **BLOCKED** | ADR 0004 has the correct boundary and scout quarantine, but `ARCHITECTURE.md` implements a different first-pass update and the stable output/equivalence rules conflict. The mechanism itself remains unimplemented by its ADR. Resolve B02 and B08. |
| G1-10 | Rust 1.85 implementation and dependency closure | **BLOCKED** | The exact core is feasible in safe Rust 1.85, but concrete SHA-256, stable Bloom hash/serialization, and safe Linux/macOS no-replace choices are not accepted in the locked build. Resolve B10. |
| G1-11 | Explicit scientific limitations | **PASS** | `docs/SCIENTIFIC_LIMITATIONS.md` correctly distinguishes record evidence, graph topology, mapping-selected pair evidence, incomplete controls, and truth tiers. These statements must remain normative in the report. |
| G1-12 | No runtime network, database, external process, or unsafe project code | **PASS (design only)** | The architecture prohibits these paths. Dependency inspection and runtime tests are still release-gate work, not established by this document. |

## Blocking contradictions and omissions

### B01 — Support mode, threshold, and profile semantics conflict

**Evidence.** `docs/PRODUCT_CONTRACT.md:18-20` defines the scientific primitive using a
fragment-support rule, while `docs/PRODUCT_CONTRACT.md:47-48` and `ARCHITECTURE.md:120-122` also expose
occurrence mode. The `sensitive` and `conservative` profiles name only
`min_fragment_support` (`docs/PRODUCT_CONTRACT.md:54-57`). `unitig_evidence.tsv` has full
minimum/median/maximum fragment fields but only a minimum occurrence field
(`docs/OUTPUT_SCHEMA.md:52-55`).

**Why this blocks.** An occurrence-mode run has no defined retention parameter or complete per-unitig
support summary. Reusing `min_fragment_support` would mislabel a window count as a fragment count;
ignoring it would make profile expansion incomplete.

**Required correction.** Define one typed configuration:

- `support_mode = fragment_instance | occurrence`;
- a mode-neutral `min_support` whose serialized object includes the unit, or two mutually exclusive
  fields with schema-enforced nullability;
- exact profile expansion in each supported mode; and
- symmetric minimum/lower-median/maximum output fields for the selected mode, with the non-selected
  mode `NA` unless it was independently computed.

All filter, histogram, transform, FASTA-header, and report labels must carry the same unit. Never use
`coverage`, `depth`, `molecules`, or `abundance` for either count.

### B02 — The Bloom path contradicts the exact histogram and its own normative ADR

**Evidence.** ADR 0002 requires retention only after a complete histogram
(`docs/adr/0002-immutable-spool-and-exact-disk-counting.md:17-20`), and
`ARCHITECTURE.md:46,156-158` promises that histogram. ADR 0004 explicitly states that the two-hit sieve
cannot recover exact singleton or other non-routed cardinalities and must be disabled when a complete
pre-threshold histogram is required (`docs/adr/0004-exactness-preserving-probabilistic-acceleration.md:163-174`).

The normative ADR also requires an unconditional insert into `seen_once` after every query
(`docs/adr/0004-exactness-preserving-probabilistic-acceleration.md:118-127`). In contrast,
`ARCHITECTURE.md:142-149` inserts into `seen_once` only in the `else` branch and incorrectly conditions
the no-false-negative property on an “unsaturated” filter. A fully occupied monotone bit Bloom filter
still has no false negatives; it merely routes nearly everything.

**Why this blocks.** The documents authorize two different algorithms and two incompatible stable
evidence products. The current byte-equivalence criterion also cannot hold for a full run record if one
path contains acceleration telemetry or `NA` histogram fields and the other contains exact values.

**Required correction.** For the first stable slice, select the simpler option:

1. keep no-sieve exact external counting mandatory;
2. produce the complete exact pre-threshold histogram; and
3. leave Mechanism A unavailable outside an explicitly experimental command.

If Mechanism A is later exposed, copy ADR 0004's algorithm verbatim, gate it on
`fragment_instance && min_support >= 2` with no sub-two rescue path, and either disable it whenever the
stable schema requires full below-threshold statistics or revise the stable contract through a new
schema/ADR. Define the byte-equivalence projection precisely; Bloom estimates can never fill an exact
field. Use fixed integer dimensions or an exactly specified rounding algorithm rather than
platform-sensitive floating-point sizing at a boundary.

### B03 — Disk partitioning is not yet a bounded globally ordered algorithm

**Evidence.** `ARCHITECTURE.md:130-138` hashes keys into partitions and sorts each partition, while
`ARCHITECTURE.md:156-158` requires one globally sorted retained stream. No cross-partition merge is
specified. “Deterministic k-way merge” has no maximum fan-in, so many runs can exceed the open-file or
heap budget. The resource table (`ARCHITECTURE.md:299-309`) omits file descriptors, merge fan-in,
run-manifest memory, global sorting, and total rather than per-worker count memory.

**Why this blocks.** Concatenating hash partitions is not key order. Opening all partitions or runs is
not bounded. A nominally streaming input can therefore still fail nondeterministically with descriptor
exhaustion or construct a graph in partition order contrary to its declared representation.

**Required correction.** Specify and test one of these complete algorithms:

- range/prefix partition full keys so partition concatenation is globally ordered; or
- hash partition, then perform a deterministic bounded-fan-in external global merge.

Freeze key bytes, partition function/version/seed, run header/trailer, stable tie order, maximum merge
fan-in, maximum simultaneously open files, multi-pass merge naming, manifest streaming/checksums, and
checked `u64` reduction. An aggregate memory-token budget must cover parser batches, worker vectors,
sort buffers, merge heaps, graph allocation, audit index, and report generation. A key cap alone is not
a byte budget.

### B04 — “Duplicated record” handling contradicts record-instance support

**Evidence.** `ARCHITECTURE.md:88-95` says duplicated paired records fail, while
`docs/PRODUCT_CONTRACT.md:43-45` says duplicated input records remain distinct supplied instances.
Global identifier uniqueness is neither promised nor compatible with bounded streaming without an
external index. The architecture rejects one physical object as both mates but does not state whether
reusing one physical input as two lanes is rejected. It also does not explicitly preserve the baseline
requirement that paired mates have the same FASTX type.

**Why this blocks.** A valid pair of repeated identifiers could either be counted or rejected depending
on which document an implementer follows. A global duplicate-ID set would violate the stated resource
model. Reusing the same lane can silently multiply support.

**Required correction.** Distinguish these cases:

- repeated record identifiers/content in synchronized positions: valid distinct record instances and
  counted again;
- mate streams with different next normalized IDs, roles, formats, or cardinality: fatal pair-sync
  error;
- reordered or one-sided repeated records: fatal because the next positions do not synchronize;
- reuse/alias of the same physical source in multiple logical roles or lanes: reject by default, with
  any deliberate override explicit and recorded.

Freeze the complete header grammar and precedence, slash/CASAVA role rules, whitespace tokenization,
unsuffixed-role policy, cross-lane ID policy, paired format rule, and diagnostic redaction. Add the
two-byte-short-read gzip sniff and trailing/corrupt multi-member behavior to the normative tests.

### B05 — A complete removal journal is promised but no artifact can carry it

**Evidence.** `ARCHITECTURE.md:171-178` says journal entries include removed keys.
`docs/PRODUCT_CONTRACT.md:56-58` says every removal is listed in the transformation journal. The only
declared file is `transform_summary.tsv`, defined as one aggregate row per stage
(`docs/OUTPUT_SCHEMA.md:72-77`); `run.json` contains only a journal summary
(`docs/OUTPUT_SCHEMA.md:89-93`). Support-threshold retention occurs before the transform subsystem, so
its discarded keys are not covered by the described graph journal.

**Why this blocks.** Aggregate counts and digests cannot simultaneously satisfy a promise to list every
removed key. The current bundle cannot audit which exact observations a deleting rule removed.

**Required correction.** Either add a versioned, deterministic `transform_events.tsv` (or an equally
explicit compressed artifact) with stage, round, full canonical key, exact support unit/value, reason,
and stable sort order, or narrow the product promise to aggregate effects plus a verifiable decision-set
digest. For an evidence-first 0.1, the explicit event ledger is preferred. Represent support retention
as a measured stage, including observed distinct count, retained distinct count, and exact support mass;
do not claim a per-key below-threshold ledger when the Bloom sieve did not count those keys exactly.

### B06 — Closed graph walks lack one valid sequence and coordinate model

**Evidence.** `ARCHITECTURE.md:182-193` and `docs/OUTPUT_SCHEMA.md:19-38` emit a cyclic core once without
the closing `k-1` prefix, use that same sequence as a GFA segment with exact de Bruijn overlap links, and
then audit full reads against canonical unitigs (`ARCHITECTURE.md:195-205`). No cyclic mapping,
rotation-equivalence, modulo-coordinate, or multi-lap rule is defined. A one-edge homopolymer loop has a
one-base minimal core for any k, so a self-link with a `(k-1)M` overlap can exceed the segment length and
ordinary full-length reads cannot map linearly to the one-base FASTA record. `kmer_edges` is also
ambiguous about whether the reverse-complement mirror is counted
(`docs/OUTPUT_SCHEMA.md:50-55`).

**Why this blocks.** FASTA, GFA, unitig IDs, evidence vector length, and read placements can disagree on
the sequence represented by the same graph component. This is an exactness defect, not merely a display
choice.

**Required correction.** Freeze an edge-centric graph model and one of two representations:

1. **Recommended for 0.1:** emit a deterministic linear spelling of the closed walk containing its
   `n` retained k-mer edges and required `k-1` context (length `n+k-1`), label it
   `closed_graph_walk`, and retain the closing graph link. Either set read/pair audit fields for that
   segment to `NA` with a typed `closed_walk_audit_unsupported` reason in 0.1, or separately define
   exact traversal through the closing link for reads longer than the linear spelling; or
2. retain a minimal periodic core only after specifying topology-aware GFA, modulo coordinates,
   rotation/reverse-complement canonicalization, multi-lap exact placement, and interoperability for
   periods shorter than `k-1`.

Define whether support statistics visit each retained canonical key once, the exact relationship among
sequence length, edge count, and support-vector length for linear and closed walks, and the handling of
self-reverse-complement k-mers and palindromic `(k-1)` boundaries. Add exhaustive small-graph and
homopolymer-cycle oracles for every supported parity of k.

### B07 — Read and pair evidence cannot be implemented from the present schema

**Evidence.** `ARCHITECTURE.md:197-205` allows an unspecified mismatch policy, gives one per-read
candidate limit, and assigns mutually exclusive fragment classes. `docs/OUTPUT_SCHEMA.md:56-63` instead
defines per-unitig aggregates and says they become `NA` if “a candidate bound” prevents an exhaustive
result, without defining whether one indeterminate read invalidates every unitig. Ambiguous IUPAC bases,
quality eligibility, palindromic placements, equivalent placements on a closed walk, and placement-group
identity are unspecified.

For pairs, `ARCHITECTURE.md:211-223` gives only an informal endpoint tuple. The contract does not freeze
library orientation, endpoint-distance eligibility, insert-model minimum and quantiles, link-status
state transitions, group canonicalization, or gap formula. `pair_links.tsv` has no exact column list,
types, nullability, or total sort key (`docs/OUTPUT_SCHEMA.md:65-70`). A candidate-limited or ambiguous
pair may have no exact endpoint pair on which to place its promised rejected/indeterminate row. GFA
`J` has a scalar distance field, whereas the TSV promises an interval; conversion is undefined.

**Why this blocks.** Different implementations can report different exact counts and still claim
conformance. An indeterminate candidate search can be accidentally serialized as an exact zero. A
midpoint chosen from an interval could appear as fabricated gap evidence.

**Required correction.** Limit stable 0.1 placement to zero mismatches unless a separate exhaustive
mapper ADR proves another policy. Specify:

- read eligibility, strand and coordinate conventions, placement equivalence, exact/unmapped/
  ambiguous/indeterminate states, and how any indeterminate read propagates to aggregate fields;
- separate read-instance, fragment-instance, placement, and placement-group counts;
- an explicit paired-library orientation supplied by configuration rather than silently inferred;
- the same-unitig sample rule, exact integer quantile definition, minimum sample count, endpoint window,
  outer-span and signed-gap formulas, and every exclusion state;
- canonicalization as a total order over valid oriented endpoint representations; and
- a complete TSV schema that permits `NA` endpoints for global rejection/indeterminacy summaries, or a
  separate `pair_audit_summary.tsv` for cases that cannot be assigned to one link.

Omit GFA `J` in the first slice, or emit an unknown distance plus explicitly versioned interval tags,
until the representation has an independently parsed golden test. Never convert an interval to an
unreported point estimate.

### B08 — “Core bytes” and bundle membership are not defined consistently

**Evidence.** `docs/PRODUCT_CONTRACT.md:72-86` calls `manifest.sha256` a digest of every other committed
artifact but permits non-scientific execution logs outside a core digest. `docs/OUTPUT_SCHEMA.md:104-109`
also hashes every committed file, then permits an unlisted log namespace. ADR 0005 separates execution
telemetry from the core digest (`docs/adr/0005-transactional-deterministic-bundle.md:19-21`). At the same
time, `run.json` must contain optional acceleration telemetry (`docs/OUTPUT_SCHEMA.md:79-93`), while
`ARCHITECTURE.md:293-295,343-355` requires complete core-bundle and Bloom-on/off byte equality. Those
bytes necessarily differ if the enabled path is recorded in `run.json`. The optional scout adds
`experimental_triage.json`, which is absent from the product inventory.

The multi-k layout is also incomplete. `ARCHITECTURE.md:225-231` promises self-contained child bundles
and a possible parent concordance table, while the output contract defines only one flat directory, one
manifest, and no concordance schema. The display-ID prefix length and collision-suffix algorithm are not
frozen (`docs/OUTPUT_SCHEMA.md:21-24`).

**Why this blocks.** “Byte-identical bundle” has no testable set of paths. A manifest that omits a
committed regular file contradicts the all-files checksum promise. Operational choices can change the
supposedly deterministic core.

**Required correction.** Publish one artifact inventory with, for every path, `required/optional`,
`scientific/operational/experimental`, schema, and digest membership. Recommended rules are:

- every file inside the committed destination is listed in `manifest.sha256`;
- nondeterministic execution telemetry lives outside the committed result directory, or is omitted;
- the core determinism digest names an exact sorted subset and digest construction;
- Bloom/scout provenance is either outside that subset with equivalence tested on the named scientific
  paths, or encoded so the claim is semantic equality rather than impossible byte equality;
- JSON member and array ordering, numeric types, `NA` reason codes, warnings, and typed errors are
  frozen in machine-readable schemas; and
- use full SHA-256 IDs in stable machine artifacts, or specify the exact prefix width and full-digest
  tie rule.

Define and atomically commit a parent/child manifest hierarchy and concordance schema before enabling
multi-k. Otherwise make multi-k explicitly unavailable in 0.1; independent single-k commands remain a
scientifically justified alternative.

### B09 — The output commit is check-then-rename, not guaranteed no-replace

**Evidence.** `ARCHITECTURE.md:245-254` says the destination is never removed or replaced but treats a
platform no-replace rename as a possible future strengthening. ADR 0005 similarly promises never to
overwrite and defers selection of the no-replace API
(`docs/adr/0005-transactional-deterministic-bundle.md:14-17,30-34`). On Unix, ordinary directory rename
can replace an existing empty directory; an absence check immediately before rename does not close the
race with a non-cooperating writer. The documents also do not define what happens if a fallible parent
directory sync, lock cleanup, or telemetry operation occurs after the successful rename.

**Why this blocks.** The strongest output claim is presently false outside cooperating processes. If a
post-rename operation converts the exit status to failure, a normal result can exist even though the
state machine says failure never commits.

**Required correction.** Before implementing bundle commit, choose one contract and encode it in ADR
0005:

- use a reviewed safe Rust 1.85-compatible no-replace directory primitive on Linux and macOS and fail
  closed when unavailable; or
- explicitly scope the guarantee to destinations that existed when the run began and to cooperating
  VeritAsm writers, removing the unconditional “never replaced” language.

The first option is preferred. Define the rename as the linearization/commit point. Complete every
fallible validation, file sync, staging-directory sync, and manifest check before it. Define parent
directory sync semantics; if a parent sync is attempted after rename, its failure cannot turn the
already committed run into a failed command unless a distinct committed-but-durability-uncertain exit
contract is defined. Post-commit lock cleanup failures are likewise nonfatal warnings. Specify
stale-lock ownership/recovery without automatically deleting an uncertain live lock. Test an adversary
that creates a destination between the final check and rename, concurrent VeritAsm processes, process
termination at each stage, and preservation of a pre-existing tree including file contents and modes.

### B10 — Rust 1.85 feasibility is plausible but the required primitives are not selected

**Evidence.** `ARCHITECTURE.md:322-324` commits to Rust 1.85-compatible, reviewed dependencies and no
unsafe project code. ADR 0004 prohibits merging a probabilistic hash until an exact algorithm,
dependency, license, advisory, MSRV, and golden vectors are recorded. ADR 0005 has not selected a safe
no-replace API. SHA-256 is normative for spools, IDs, state digests, and manifests, but the current
locked dependency set does not establish the implementation choice.

**Why this blocks.** None of these primitives is inherently infeasible on Rust 1.85, but an architecture
cannot claim a verified dependency boundary while essential platform and hashing choices remain open.
Rolling `u128`, sorted `Vec` storage, checked arithmetic, bounded files, and typed serialization are all
available in safe Rust 1.85; the unresolved pieces should not force custom cryptography or a raw unsafe
syscall.

**Required correction.** Record and lock:

1. a SHA-256 implementation with Rust 1.85, license, source, feature, build-script, unsafe, and known
   vector review;
2. a concrete stable non-identity partition function with cross-platform vectors;
3. a concrete deterministic Bloom/scout hash only if ADR 0004 is implemented; and
4. a safe no-replace Linux/macOS API or the narrowed guarantee in B09.

All serialized integers are fixed-width with checked `usize` conversion. Scientific decisions and
stable serialization must not depend on host word size, locale, randomized hash state, platform math,
or filesystem enumeration order. The clean locked source must build and test with exactly Rust 1.85,
not merely a newer compiler honoring `rust-version` metadata.

## Required non-blocking clarifications

These are not independent reasons to reject the scientific primitive, but they must be frozen before
the relevant feature is called stable.

| ID | Clarification |
|---|---|
| R01 | Define the spool configuration digest as parser/QC/input-role semantics plus the complete selected-k list, rather than a per-child value that prevents all k values from consuming the same verified spool. Store both raw transport and logical decoded source digests with exact byte domains. |
| R02 | Freeze FASTA/FASTQ corner semantics: empty records, CRLF, sequence whitespace, lowercase and complete IUPAC set, FASTA behavior under a quality threshold, gzip trailing bytes, concatenated members, and aggregate decompressed-byte accounting. |
| R03 | Give every resource limit a unit, domain, default, minimum/maximum, aggregate-versus-per-source meaning, validation order, and stable typed error code. Add audit-index bytes, output bytes, file descriptors, number of partitions/runs, and queue-resident bytes. |
| R04 | Specify transformation round numbering, convergence bound, decision conflicts, reverse-complement symmetry, no-op rows, and whether the optional tip rule is actually supported. “May support” is not a stable algorithm. Prefer no tip deletion in the first release until a separate ADR fixes it. |
| R05 | Define GFA header version, exact `L` overlap (`k-1M` when representable), tag names/types, link canonicalization, self-links, and independent-parser round trips. Reconcile the GFA 1.2 promise with any supporting document that still recommends a GFA 1.0-only subset. |
| R06 | Define `complete_empty` artifacts: empty FASTA, header-only TSVs, valid header-only GFA, exact zero counts, audit status, and HTML rendering. |
| R07 | Define the error taxonomy and CLI exit mapping for input, pair sync, limit, overflow, corruption, audit strictness, schema, lock, destination, and commit failures. Diagnostics may vary; typed codes in stable data may not. |
| R08 | Reconcile baseline stdout output with the one-directory transaction. Either preserve stdout only as an explicitly non-transactional export of an already committed artifact, or record a deliberate compatibility break. Do not stream a plausible partial assembly to stdout during construction. |

## Normative invariants required before implementation review

1. **Observation snapshot:** every downstream scientific pass reads one fully verified immutable spool;
   no live source is reopened.
2. **Pair lockstep:** fragment ordinal advances only after both mate records pass format, identity, and
   role checks; a one-sided EOF or mismatch emits no fragment.
3. **Exact identity:** hashes may select storage or work order, but membership, equality, support,
   graph edges, and final comparisons use complete packed keys or complete sequence bytes.
4. **Support unit:** one run has one declared support unit; fragment mode deduplicates across both mates,
   occurrence mode does not, and no field silently changes units.
5. **Count conservation:** exact external counts equal the in-memory oracle for every key; sum and
   distinct-key relationships are checked without overflow.
6. **Bloom containment:** when enabled under ADR 0004, every key with exact fragment support at least two
   is in `seen_twice`, and the final retained `(key,count)` stream is byte-identical to no-sieve output.
7. **Graph conservation:** each retained canonical k-mer contributes one biological support value;
   oriented views do not double evidence, and compaction neither loses nor duplicates retained keys.
8. **Branch preservation:** unitig traversal stops at every unresolved branch; neither support, pair,
   map, iteration, or hash order silently chooses a continuation.
9. **Audit honesty:** a truncated candidate search is indeterminate, never unmapped, unique, zero, or
   exact; affected aggregates are `NA` or carry proved lower/upper bounds.
10. **Pair non-mutation:** pair evidence cannot add, delete, reorder, extend, or concatenate a unitig or
    retained graph edge in 0.1.
11. **Transform accountability:** every deleting decision is tied to a predeclared versioned rule,
    frozen input state, exact decision set, measured effect, and stable journal/digest.
12. **Determinism:** the named core paths are byte-identical across supported worker counts, batch
    schedules, repeated processes, x86-64 Linux, and AArch64 macOS for identical input bytes and
    scientific parameters.
13. **Atomic visibility:** before the commit linearization point the destination is absent or an older
    untouched tree; after it, the complete verified new tree is visible. No failed pre-commit run changes
    an existing tree.
14. **Bounded live state:** every resident queue, vector, map, merge heap, open-file set, spool, temporary
    run, graph, audit index, and report buffer has an enforced checked budget or the run fails before
    commit.

## Falsification suite required by this gate

| Boundary | Minimum adversarial tests |
|---|---|
| Input and pairs | One-byte gzip prefix reads; concatenated and corrupt/truncated members; gzip trailing junk; wrapped FASTA/FASTQ; huge headers/records; R1/R2 format mismatch; missing, extra, swapped, reordered, and one-sided duplicate mates; synchronized duplicate IDs accepted as distinct instances; physical-path aliases across lanes and roles. |
| DNA and count | Exhaustive small k encoding/RC/canonicalization; even-k self-RC keys; ambiguity and quality accounting; fragment mate-overlap deduplication; occurrence repeats; thresholds 1/2/N; every partition/thread/batch configuration; counter and offset overflow; skewed single partition; more runs than merge fan-in. |
| Bloom | Exhaustive small-universe containment; tiny/all-ones filters; deliberate collisions; first-pass worker reordering; support-one and occurrence bypass; corrupt serialization; spool/config mismatch; no-sieve retained-stream and named-artifact byte oracle. |
| Graph and compaction | Exhaustive small retained graphs; tips, branches, parallel orientation cases, palindromic boundaries, isolated edges, homopolymer self-loops, cycles shorter/equal/longer than k, reverse-complement collapse, exact edge/support conservation, independent GFA parse. |
| Audit and pairs | Unique, repeat, palindrome, closed-walk, IUPAC, low-quality, and candidate-limit reads; one/both mates unmapped; multimapping; wrong orientation; read-through; same-unitig span; insufficient insert samples; contradictory gaps; endpoint ties; swap/RC canonicalization; no change to FASTA/graph with pairs enabled. |
| Bundle | Failure before/after every file operation; short writes; full disk; schema failure; manifest mismatch; existing nonempty and empty destinations; symlinks; concurrent cooperative writers; adversarial destination creation between check and commit; termination leaving a stale lock; no post-commit error status; tree digest/mode unchanged after every failed pre-commit run. |
| Determinism | Compare the explicit core path set at 1/2/4/N threads, multiple batch sizes, partition counts, merge fan-ins, repeated processes, and Linux x86-64 versus macOS AArch64. Operational artifacts are tested under their separately declared contract. |

## Implementation work breakdown after corrections

Work may proceed in small experimental branches while Gate 1 is blocked, but no component should be
called contract-complete until its upstream specification is fixed.

### W0 — Freeze the contract and schemas

1. Resolve B01--B10 in the normative documents and make ADR precedence explicit.
2. Define the minimal 0.1 feature set: no Bloom/scout, no deleting tip rule, no mismatch-tolerant
   mapper, no pair-driven sequence, and either no multi-k wrapper or a fully specified parent bundle.
3. Write exact JSON Schema plus TSV/FASTA/GFA grammars, column order, enums, nullability, sort keys,
   IDs, digest domains, and artifact inventory before serializers.
4. Turn the fourteen invariants above into named test requirements traceable from code modules.

**Exit:** two independent implementers can derive the same accepted/rejected inputs and identical
artifact bytes without making a biological or filesystem policy choice.

### W1 — Configuration, fixed-width types, and error/resource model

Implement profile expansion, support-unit types, k validation, checked budgets, stable error codes,
fixed-width counters/offsets, and deterministic parameter serialization. Select and lock the reviewed
SHA-256 and transaction dependencies under Rust 1.85.

**Exit:** configuration golden tests cover every mode and reject incompatible Bloom, occurrence,
singleton-rescue, pair, and multi-k combinations before input is read.

### W2 — Input, gzip, pair synchronization, and immutable spool

Implement bounded two-byte content sniffing, multi-member decoding, the frozen FASTX state machine,
source-identity checks, ordered lane/pair parsing, raw/logical digests, and the versioned spool with a
verified trailer. Keep one parser task and bounded fragment-indexed worker queues.

**Exit:** inherited valid-input corpus passes; malformed/fuzz cases never panic, skip a record, split a
pair, over-allocate, or produce a consumable partial spool.

### W3 — DNA extraction and exact external counts

Implement `u128` rolling canonical k-mers for k=3--63, mutually exclusive window accounting, both
support modes, deterministic bounded run generation, multi-pass bounded merging, global full-key order,
checked counts, exact histogram, and graph-cap preflight. Use the no-sieve path only.

**Exit:** exhaustive/small in-memory oracle equality holds across thread, batch, partition, and fan-in
settings; descriptor and live-memory high-water marks stay inside enforced budgets.

### W4 — Exact retained graph and compaction

Implement the sorted canonical table, oriented view, degree rules, palindromic boundaries, frozen
decision interface, non-branching traversal, RC collapse, closed-walk representation selected in B06,
stable full IDs, and exact graph links.

**Exit:** exhaustive small-graph conservation, canonicalization, cycle, and GFA round-trip oracles pass;
the non-deleting path has no transform events beyond explicitly measured retention.

### W5 — Read audit and pair annotations

Implement zero-mismatch exhaustive seed-and-verify with a proved seed strategy and candidate-limit
state. Then implement the frozen read/fragment state machine, same-unitig span sample, canonical pair
groups, exact exclusions, and TSV evidence. Do not emit `J` until its unknown/scalar/interval semantics
are fixed and independently parsed.

**Exit:** all indeterminate paths remain visibly indeterminate, pair enabling leaves unitig FASTA and
exact graph links unchanged, and every pair TSV count reconciles to the run-level pair partition.

### W6 — Typed artifacts, HTML, manifest, and commit

Generate every machine artifact from shared typed records; validate by reparsing; render HTML only from
those records; compute the exact core digest and all-file manifest; then commit with the B09
no-replace/scoped primitive. The successful rename is the final fallible state transition.

**Exit:** schema/golden/determinism tests pass, hostile text cannot escape HTML context, every committed
file is manifested, and the full failure-injection matrix preserves prior output.

### W7 — Optional features, in evidence order

1. Add independent multi-k child runs only after the parent bundle and concordance schema pass.
2. Add the two-hit sieve only after ADR 0004's proof tests, exactness projection, concrete stable hash,
   and retained benchmark justify it.
3. Add the default-off scout only after whole-fragment scheduling, starvation, control quarantine, and
   exact residual-path equivalence tests pass.
4. Add a frozen-round tip rule only through a separate algorithm ADR and full event ledger.

No optional feature can be used to compensate for a failing exact path.

## Gate 1 exit checklist

Gate 1 can change to **PASS** only when all of the following are true:

- [ ] Every B01--B10 correction is reflected consistently in the product, architecture, schema, and
      applicable ADR.
- [ ] Stable and experimental feature sets are separate, and unavailable features are not advertised
      as 0.1 behavior.
- [ ] Pair/read states, support units, cycle spelling, transformation evidence, artifact membership,
      and commit semantics are machine-testable without inferred policy.
- [ ] The complete counting algorithm has global order and explicit RAM/disk/file-descriptor bounds.
- [ ] ADR 0005 either selects an atomic no-replace primitive for Linux/macOS or narrows every overwrite
      claim to the proved cooperative boundary.
- [ ] Essential dependencies and features are locked, reviewed, and build-tested with Rust 1.85.
- [ ] Every normative invariant has a named oracle, property, integration, or failure-injection test in
      the validation plan.
- [ ] No document treats a Bloom hit, scout score, graph cycle, pair link, unitig, or empty output as a
      detection, absence, circularity, genome, haplotype, or independent-validation result.

The paragraph above records the initial decision. It is superseded only for architecture
implementability by the resolution below; it remains true that this gate supplies no implementation,
performance, accuracy, or superiority evidence.

## Final resolution and independent re-review

On 2026-09-03, a fresh adversarial review read the current architecture, product/configuration/CLI and
output contracts, scientific limitations and risks, ADRs, and every JSON descriptor; all schema JSON
parsed successfully. The reviewer returned **PASS** after the following were frozen consistently:

- one exact single-k, no-sieve stable path and an isolated experimental Bloom boundary;
- support/profile expansion, global input arity, bounded FASTX byte accounting, and exact error codes;
- oriented-handle identity, palindrome boundaries, RC/cycle orbits, conservation equations, GFA 1.0
  links, content-derived IDs, and all digest preimages;
- complete read and pair state partitions, canonical-coordinate pair observations, exact row ordering,
  empty/closed-only bundles, and run-level reconciliations;
- the deterministic spool byte stream and checksum coverage, fixed `run.json` warning/limitation
  vocabulary, and schemas matching every table;
- same-filesystem safe no-replace commit with no fallback and stderr-only execution telemetry; and
- removal or explicit deferral of multi-k, consensus, insert inference, circular-junction auditing,
  low-complexity scoring, and other unavailable stable behaviors.

The exit checklist is therefore resolved for design. Implementation must still mutation-test custom
`x-*` schema invariants, add byte-golden spool vectors, exhaust graph/palindrome/cycle cases, reconcile
every read/pair state, failure-inject pre-commit operations, and prove repeated/thread-count bundle
identity. Those are Gate 2 and validation obligations, not evidence already obtained.

Gate 1 authorizes implementation of the frozen 0.1 vertical slice. It does not authorize any claim
that VeritAsm is correct, performant, portable, accurate, or better than Virustic2.
