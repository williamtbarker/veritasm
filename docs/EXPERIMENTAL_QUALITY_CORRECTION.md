# Experimental exact-spectrum quality correction

Status: isolated library experiment; unqualified and not used by the stable CLI, spool, graph,
assembler, or output bundle. Its presence is not evidence of improved sensitivity, accuracy, or
performance.

`experimental::quality_correction` tests a bounded substitution and ambiguity-resolution policy.
The caller supplies immutable read bases, numeric Phred Q0..Q93 values, sorted exact canonical
k-mer occurrence spectra built from uncorrected evidence, and a source descriptor containing
content identities for the source snapshot, source layout, and QC policy. Those identities are
integrity labels supplied by the caller; they are not signatures and do not prove that the caller's
description is true. Fragment-support spectra and approximate membership structures are rejected by
the current typed API, and all-zero unbound descriptor identities are rejected. Corrected sequence
can never increment raw support.

## Explicit correction modes

- `RawOnly` returns the immutable raw spelling and does not project or enumerate candidates.
- `DiversityPreserving` is the default. It quarantines a proposed correction if an affected raw
  exact k-mer has nonzero support below the trusted threshold. Because this slice has no independent
  read-adjacency or physical-link evidence channel, it also quarantines every proposed base-changing
  correction—both exact-base substitutions and IUPAC resolutions—with
  `IndependentContextUnavailable`. Exact spectra alone cannot show that a k-mer-compatible
  resolution was observed end to end.
- `ConsensusExperimental` permits a uniquely dominating spectrum-only correction. Counts of
  affected nonzero subthreshold raw windows and their summed support remain visible in accepted
  evidence and accounting. This mode can erase a real minority allele or create a k-mer-compatible
  sequence that was never observed end to end. It must therefore remain a separate experimental arm
  alongside the raw result.

No mode mutates the input or the borrowed spectra. There is currently no mode that claims either an
exact substitution or an ambiguity resolution is diversity-safe.

`TrimLowQualityTerminals` is separate from base correction. When explicitly selected as the
unresolved policy, it is an opt-in, lossy QC transform that can delete only a contiguous prefix
and/or suffix. It cannot introduce a base or join formerly nonadjacent retained bases, but it can
reduce recovery. A trim is typed and journaled as `Trimmed`, never `Corrected`, and is not promoted
as correction evidence.

## Frozen decision rule

For one non-raw-only read, the engine:

1. checks base/quality length equality and the configured length limit before the full symbol/quality
   scan and content hash; cap journals still validate and hash their source read;
2. admits and scores the baseline before any search-only cap, so a fully trusted raw read cannot be
   quarantined merely because it has many low-quality positions;
3. admits the exact candidate, window-evaluation, exact-spectrum-comparison,
   alternative-summary-comparison, auxiliary-search-work, and requested vector-payload projections
   before search allocation;
4. enumerates every sequence containing one through the configured maximum of three edits at
   editable positions; every choice follows explicit `A<C<G<T` order, exact bases omit the original
   base from that order, and R/Y/S/W/K/M/B/D/H/V/N retain only their standard IUPAC members;
5. requires configured trusted-window and summed-window-occurrence-support gains, support from the
   configured number of k layers, no aggregate per-layer regression, and—by default—trusted exact
   support for every affected candidate window; and
6. accepts only a candidate that strictly Pareto-dominates every other qualified candidate in the
   declared aggregate and per-layer components.

Configured k layers are correlated views of the same reads, not independent observations.
`summed_occurrence_support` is a checked sum of occurrence counts looked up for overlapping
candidate windows; it is not a likelihood and can count the same underlying read evidence
repeatedly. Affected-window counts are retained for audit but are not Pareto objectives: using them
would favor central edits, which intersect more windows than terminal edits. Lexical sequence order
is used only to retain a temporary candidate; it cannot resolve an evidence tie.

High-quality ambiguity symbols are not editable in this slice because editability remains gated by
`max_edit_phred`. This is a declared sensitivity limitation, not evidence that the symbol is exact.

## Journal, verification, and conservation

Every decision carries read, raw-spectrum, source-descriptor, full-configuration, and algorithm
identities; frozen thresholds; baseline and accepted evidence when computed; typed edits; a typed
disposition; separate projected/evaluated/score-evaluation counts; projected and actual exact
spectrum comparisons; projected summary comparisons and exact heap comparisons; projected
auxiliary search-work units; and mutually exclusive base accounting. `BaseEditKind` distinguishes
an exact-base substitution from resolution within an IUPAC set. Every evaluated
alternative—including below-threshold IUPAC resolutions—contributes its full per-layer evidence and
qualification state to a domain-separated SHA-256 set digest. The journal also retains the smallest
alternative summaries in a versioned total order: candidate identity, edits, edit count,
qualification state, supporting-layer count, trusted windows, unsupported windows, summed support,
and the four low-count evidence fields. `BaseEditKind` orders substitution before ambiguity
resolution. The summaries are emitted in that order and include explicit edit bases, qualification
state, aggregate evidence, and both replaced-baseline and candidate-spelling subthreshold counts.
`max_candidate_summaries_per_read` bounds that vector; complete evaluated, edit-kind,
qualification, and low-count totals plus a truncation flag make omitted summaries visible. The
engine-bound verifier recomputes these fields exactly. The digest is an integrity commitment under
the versioned enumerator, not an external signature or statistical independence claim.

The versioned enumeration order is target edit count, ascending read position, then explicit
`A<C<G<T` replacement order. It does not depend on library sort behavior. A hardcoded
candidate-digest vector is checked under both the MSRV and current stable toolchains.

`CorrectionJournal::replay_transformation` validates the supplied read and edit structure and then
reconstructs the recorded output. It deliberately does not verify spectrum evidence or prove who
created the journal. `QualityCorrectionEngine::verify_decision_and_replay` is the evidence-checking
API: it checks all engine identities, deterministically recomputes the complete decision and
accounting, requires exact equality, and only then replays it. A quarantined read has no emitted
sequence; this is distinct from an empty sequence.

Batch results are sorted by unique read ordinal. The complete batch payload projection is enforced
before allocating the ordinal-order vector, result slots, or private Rayon pool. Scheduling does not
change support or output order. Aggregate invariants require input bases to equal emitted plus
trimmed plus quarantined bases, reads to have exactly one disposition, evaluated candidates not to
exceed projected candidates, and quarantine subclasses not to exceed quarantined reads.

## Resource envelope

Engine construction first admits the total number of borrowed exact-spectrum entries before
validating and hashing every entry. Per-read limits cover read length, editable positions, exact
candidate count, full-window score evaluations, exact binary-search key comparisons, bounded-heap
summary comparisons, auxiliary search-work units, journal edits, bounded alternative summaries,
and a requested vector-payload model. All four new limits are part of the configuration identity;
their projections and exact counters, where applicable, are part of the recomputed decision.
Baseline-only output and evidence have a separate admission.

Candidate search uses two passes: one retains a deterministically ranked candidate over the
declared evidence components and records a bounded audit sample plus a complete digest; the second
proves strict dominance without retaining the candidate set. The audit sample uses a deterministic
bounded max-heap and final in-place heap sort, requiring `O(C log S)` comparisons and `O(S)` retained
summaries for `C` candidates and configured retained limit `S`. The comparison projection is a
checked upper bound; actual comparisons are counted exactly. The exact-spectrum binary search is
custom and likewise counts each key comparison exactly. Auxiliary work uses a versioned,
conservative weighted admission formula per candidate: three units per read base, five per
configured edit, seven per spectrum, and eight fixed units. These are declared budget weights, not
measured CPU instructions. Window evaluations, spectrum key comparisons, and heap comparisons are
budgeted separately rather than hidden in that formula.

The two-pass projection is deliberately conservative: it admits both full passes before allocating
search state, even when the first pass would find no qualified candidate and the second pass would
not run. Such a read can therefore receive a typed cap quarantine despite a hypothetical one-pass
abstention fitting the same cap. This fails closed and is a named recovery limitation, not a runtime
or accuracy claim.

The batch projection includes canonical-order and result-slot vectors, retained output bases,
baseline/accepted evidence and edit journals, plus one maximum per-read scratch allowance for each
active worker. These are module-owned requested vector payloads, not a whole-process RSS bound:
allocator metadata and rounding, Rayon stacks, code pages, caller-owned reads/spectra, and dependency
internals are excluded. Allocation remains fallible and can return `resource_memory`.

## Explicitly deferred

- an independent read-adjacency or physical-link channel capable of authorizing any base-changing
  correction in diversity mode, including ambiguity resolution;
- insertions, deletions, homopolymer realignment, adapter handling, pair-aware correction, and base
  quality recalibration;
- fragment-support and leave-one-fragment-out spectra, positional likelihoods, and online or
  external-memory spectrum access; checked combinatorial or work projections that overflow `u64`
  fail closed with a typed run error rather than producing a cap journal;
- recovery of reads shorter than every configured k. Such a read has no scorable window and the
  default unresolved policy quarantines it; `RawOnly` or explicit `LeaveUnchanged` can retain the
  raw spelling, but neither supplies correction evidence;
- graph cleaning, abundance/strain inference, haplotype phasing, or organism detection;
- stable serialization/schema commitments or stable assembler integration; and
- any accuracy, sensitivity, adventitious-agent, production, or clinical claim.

The tests include both position-bias counterexamples, baseline-before-cap behavior, spectrum-entry
and comparison-cap boundaries, exact lookup-count bounds, deterministic bounded-heap equivalence,
auxiliary-work admission, conservative two-pass and below-all-k recovery limits, low-count
minority modes, exact-base and ambiguity-resolution spectrum-compatible unobserved mosaics,
default-mode abstention for every IUPAC symbol, bounded/truncated alternative evidence, supported
subthreshold ambiguity alternatives, tamper detection, panic-free transformation replay, full
engine-bound verification, batch pre-admission, cap boundaries, deterministic worker ordering, and
an independent literal-string exhaustive oracle over all 4,096 truth/observed pairs in the complete
three-base domain. This is stronger unit evidence, not biological qualification.
Promotion still requires prospective truth-known ablations across error profiles and mixture ratios,
false-correction and false-junction reporting, journal fuzzing, external-memory integration, retained
raw controls, and evidence that downstream assembly gains outweigh minority-path loss. Until then,
the correct description is “experimental and unqualified.”
