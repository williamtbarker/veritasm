# ADR 0008: Experimental paired-end library-model substrate

- Status: accepted for an isolated experiment; not authorized for stable assembly output or joins
- Date: 2026-09-03
- Supersedes: none; ADR 0003 remains the stable 0.1 pair contract

## Context

Conservative repeat resolution eventually needs lane-specific evidence about read orientation and
fragment span. Pooling lanes, accepting multiply placed reads, using floating heuristic confidence,
or fitting a distribution without retaining exclusions could create false precision. ADR 0003
therefore prohibited a library model in stable 0.1.

## Decision

Add a standalone `library_model` module that consumes caller-asserted complete, exact, unique
placements for both mates. The module checks coordinates, strands, target metadata, observation
identity, and resource limits. It cannot independently prove the caller's exactness or uniqueness
assertion.

Each `(lane_ordinal, fragment_ordinal)` occurs at most once. A valid candidate follows this
first-match ledger: different target, shared non-linear target, coincident start coordinate, or
included same-linear-unitig. Invalid geometry, duplicate identity, and conflicting same-target
metadata are fatal integrity errors rather than ordinary exclusions.

Orientation is the two strands in increasing emitted-forward-unitig start order: `+/-` is FR,
`-/+` is RF, `+/+` is FF, and `-/-` is RR. Mate role does not select the left observation. Span is
`max(end_R1,end_R2) - min(start_R1,start_R2)`. This mapped outer-envelope span is not a physical
molecule length or inferred gap.

Every lane is finalized separately. Each orientation reports sorted empirical nearest-rank P10,
P25, P50, P75, and P90 order statistics plus minimum and maximum. Rank is `ceil(p*n/100)` in
one-indexed notation, so P50 is the lower empirical median for even `n`. No floating-point value
enters inference.

A lane is available only after, in order: minimum included count; one unique leading orientation;
minimum leading count; an exact leading/all-included numerator/denominator threshold; and a maximum
integer P90-P10 width. Ratios use `u128` cross multiplication; counts use checked `u64`. Observation,
lane, identifier, and conservative allocation ceilings fail closed. Lane ordering and span sorting
make the result independent of candidate arrival order.

P90-P10 width is an interpretable compactness gate, not a statistical modality test. A wide bimodal
population can fail it, while close modes can pass it. Reported extrema keep tail outliers visible.

## Consequences

- The stable CLI, pipeline, FASTA, GFA, evidence tables, and schemas are unchanged.
- The model cannot join unitigs, choose a branch, estimate a gap, or support an accuracy claim.
- Mixed or insufficient lanes remain explicit unavailable states and are never pooled.
- Inputs are mapping-selected construction-read observations, not independent evidence.
- Memory is linear in retained observation identities and included spans inside explicit ceilings.

## Promotion gate

Before reconstruction use, a later ADR must specify graph-thread compatibility, read-length and
clipping semantics, chimeric-pair handling, sampling bias, contradiction rules, minimum independent
support, and a replayable join ledger. Truth-known tests must show repeat-resolution benefit without
false-junction regression. Unavailable models must leave the graph unresolved.

## Evidence

Unit tests cover all four orientations and mate-coordinate order, outliers, wide bimodality,
insufficient and mixed evidence, independent mixed lanes, every exclusion, corrupt coordinates and
metadata, duplicates, arbitrary arrival order, integer-quantile boundaries, checked arithmetic, and
resource ceilings.
