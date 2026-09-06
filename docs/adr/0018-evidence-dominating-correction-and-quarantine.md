# ADR 0018: Evidence-dominating correction and low-count quarantine

- Status: accepted for isolated v0.4 experiments; stable integration remains gated
- Date: 2026-09-05
- Preserves: ADRs 0003, 0004, 0015, and 0016 exactness and qualification boundaries

## Context

Sequencing errors fragment a de Bruijn graph, but low-abundance sequence, heterogeneity, and errors
can occupy the same low-count region of a k-mer spectrum. A permissive threshold improves recovery
and also admits error paths; an aggressive correction can increase apparent contiguity while erasing
real minority sequence. Published correctors use spectrum, graph-mapping, or read-alignment context,
and comparative evidence does not support unconditional benefit on every dataset.

A sensitive assembler therefore needs more than a corrected FASTQ. It needs the immutable raw
observation, the alternatives considered, the evidence for any edit, and an explicit unresolved
state.

## Decision

1. Raw bases, qualities, exact raw spectra, and raw graph evidence are immutable. Corrected evidence
   is a separate layer and never increments raw support.
2. The first correction engine enumerates only a checked, bounded substitution neighborhood. Indels
   are deferred rather than silently represented as substitutions or clipping.
3. Candidate evaluation reports named integer components for each configured k: supported, trusted,
   and unsupported windows; raw support; quality penalty; edits; and conflicts. No single opaque
   floating score is the evidence record.
4. A candidate can be accepted only when it meets every frozen minimum and is the unique candidate
   that evidence-dominates the raw spelling and all other qualifying candidates. A tie, incomplete
   search, contradictory k layer, or exhausted work/memory limit yields a typed abstention.
5. Every base/read result is one of `unchanged`, `corrected`, `trimmed`, or `quarantined`, with an
   exact replay journal and aggregate conservation counts. The first slice may implement fewer
   actions, but it may not relabel an unimplemented action as a correction.
6. Low-count exact keys are quarantined rather than globally deleted. Rescue into an assembly path
   requires a later, separately versioned rule based on original-read adjacency or a uniquely
   compatible frozen physical-link constraint.
7. Corrected and raw assemblies are compared as preregistered arms. Truth cannot select a correction
   profile after results are inspected.

## Promotion gates

- Exhaustive small-read equality with an independently implemented candidate oracle.
- Exact replay of every accepted edit and byte-identical results across input order and thread count.
- Separate outcomes for high-quality errors, low-quality correct bases, correlated errors,
  low-abundance real sequence, mixtures, repeats, and ambiguity boundaries.
- No increase in unsupported sequence or false junctions beyond the preregistered tolerance, and a
  retained raw-control result for every comparison.
- Hard candidate, edit, work, memory, journal, and output limits, including fail-closed tests at each
  boundary.
- Evidence from frozen public and truth-known data; unit tests alone cannot promote correction into
  the stable assembler.

## Consequences

This rule is intentionally conservative and can leave correctable errors unresolved. That is an
observable recovery cost, not a reason to force an edit. Correction can improve sensitivity only if
the qualification frontier shows more true recovery without an unacceptable loss of minor-path
precision or junction accuracy.

## Adversarial-review amendment (2026-09-05)

The first implementation audit found that raw affected-window counts are not comparable between
terminal and internal edits, that search caps can incorrectly quarantine an already-trusted read if
they are checked before baseline scoring, and that exact k-mer spectra cannot distinguish an error
correction from a never-observed mosaic or low-abundance real path. The following rules therefore
refine this decision and are normative for the isolated experiment:

1. Baseline evidence is separately admitted and scored before editable-position, candidate, or
   two-pass search limits. Journals distinguish an unperformed projection, projected distinct
   candidates, evaluated distinct candidates, and repeated score evaluations.
2. Affected-window counts are audit fields, not Pareto objectives. Candidate ranking must contain
   every Pareto objective in the same direction; exhaustive tests cover both the position-bias and
   rank/dominance inconsistency counterexamples.
3. Correction is an explicit three-arm choice. `RawOnly` is the immutable control.
   `DiversityPreserving` is the default and quarantines every proposal touching a nonzero
   subthreshold exact raw window. Until a separately versioned independent read-adjacency or
   physical-link channel exists, it also quarantines every exact-base substitution and every IUPAC
   ambiguity resolution. `ConsensusExperimental` may emit a spectrum-only change but must report
   affected low-count evidence and remain alongside the raw arm.
4. Multiple k values derived from one read collection are correlated layers, not independent
   observations. Summed occurrence support over overlapping windows is reported as such and is not
   described as a likelihood.
5. IUPAC ambiguity resolution is distinct from substitution and is restricted to the symbol's
   encoded base set. Spectrum compatibility does not establish end-to-end observation: the exact
   `AANCGG` counterexample resolves to an unobserved `AACCGG` from spectra of `AACCTT` and `TTCCGG`.
   Diversity mode therefore abstains for every IUPAC resolution symbol. High-quality ambiguities
   remain unresolved in this first slice because the edit gate is quality based.
6. A replayable transformation is not evidence verification. The engine-bound verifier checks an
   algorithm identity, full configuration identity, exact spectrum identity, caller-supplied source
   descriptor identity, and immutable read identity; it recomputes and compares the complete
   decision before replay. The source descriptor is an integrity label, not an authentication claim.
7. Batch payload projection is enforced before any O(read-count) order or result-slot allocation.
8. Consensus journals bind every evaluated alternative, not only threshold winners. A
   domain-separated digest commits to each alternative's edits, qualification state, aggregate and
   per-layer evidence in versioned enumeration order. Complete counts distinguish exact,
   ambiguity, mixed, qualified, and subthreshold-supported alternatives. A separately configured
   cap bounds deterministic summaries ordered first by candidate identity and then by the complete
   explicit edit/evidence tuple; a truncation flag exposes any omitted summaries. These hashes are
   verifier-checked integrity commitments, not signatures. The
   versioned order is target edit count, ascending read position, then explicit `A<C<G<T`
   replacement order with the original exact base omitted; it must not depend on unspecified sort
   ordering. A hardcoded digest vector is tested on the MSRV and current stable toolchains.
9. Cheap base/quality length equality and length-cap comparison precede full content scanning and
   hashing. A returned cap journal still validates and hashes the source read so replay cannot be
   bound to malformed or unidentified bytes.
10. Engine construction admits a configured total exact-spectrum-entry count before validating and
    hashing those entries. Per-read admission separately bounds exact binary-search key comparisons,
    deterministic bounded-summary-heap comparisons, and a versioned conservative auxiliary-work
    projection. These limits are configuration-bound; projections and exact counters are included
    in the recomputed result. The bounded summary set retains the smallest `S` summaries in an
    explicit total order using `O(C log S)` comparisons and `O(S)` memory for `C` candidates.
11. Search admission conservatively projects both full candidate passes even when the second pass
    would be skipped after a one-pass abstention. A resulting cap quarantine is an explicit recovery
    limitation. Reads shorter than every configured k likewise have no correction evidence and are
    quarantined by the default unresolved policy; raw-only and explicit leave-unchanged policies may
    retain their unmodified spelling.

These constraints intentionally make the default correction arm less aggressive. No base-changing
correction—whether an exact-base substitution or an ambiguity resolution—may be promoted from this
experiment until the missing independent context channel and prospective downstream ablation
evidence exist. The separately typed terminal-trim policy is an opt-in, lossy QC transform: it may
delete only a contiguous prefix and/or suffix, can reduce recovery, and is never classified or
promoted as a correction.

## References

- Kelley et al., Quake, <https://doi.org/10.1186/gb-2010-11-11-r116>
- Li, BFC, <https://doi.org/10.1093/bioinformatics/btv290>
- Limasset et al., Bcool, <https://doi.org/10.1093/bioinformatics/btz102>
- Kallenborn et al., CARE, <https://doi.org/10.1093/bioinformatics/btaa738>
- Dlugosz et al., correction evaluation, <https://doi.org/10.1038/s41598-024-52386-9>
