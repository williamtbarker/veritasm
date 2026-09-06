# ADR 0015: Repair qualification truth before optimizing sensitivity

- Status: accepted; controlling for v0.4 qualification work
- Date: 2026-09-05
- Supersedes: the affected metric interpretations in ADR 0009; it does not remove truth separation

## Context

Independent audit produced two executable counterexamples.

First, the `error-pe` generator assigns Q10 to every substituted base and Q40 to every correct base.
The default Q20 window rule therefore rejects every window containing a simulated error and declares
every erroneous read ineligible for exact read audit. That fixture tests ideal QC censoring, not
assembly with retained errors.

Second, the evaluator selects the first equally best truth target. When two truth molecules have the
same sequence, evaluating the complete truth FASTA against itself reports 0.5 overall genome
fraction, zero minor recovery, and duplication 2. Molecule order, rather than identifiable sequence
evidence, decides the score.

Any sensitivity optimization against these outputs risks fitting validation artifacts.

## Decision

1. No current error-case, mixture recovery, or duplication result is admitted as qualification
   evidence until its estimand is repaired and the counterexamples pass.
2. Retain the current error fixture only under an explicit `qc_censoring_control` identity.
3. Generate base errors and quality values as separate versioned stochastic processes. Include
   retained high-quality errors and low-quality correct bases. Record seed streams and every emitted
   error/quality event.
4. Retain exact single-primary alignment only as a diagnostic row. It cannot determine recovery or
   duplication when multiple truth coordinates are equally compatible.
5. Publish unique-coordinate recovery and compatible-coverage lower/upper bounds. Publish NA for a
   class-specific or copy-specific metric that the observed sequence cannot identify.
6. Make truth-record order irrelevant to all aggregate and class metrics.
7. Add an independently implemented origin replay oracle; generation and verification cannot share
   the same coordinate/orientation function as their only check.

## Required kill tests

- Every generated truth FASTA evaluated against itself reaches the defined complete-recovery bounds.
- Duplicate truth sequences under different IDs and repeated starts within one sequence do not create
  arbitrary missing recovery or duplication.
- Permuting truth order leaves every aggregate/class metric byte identical.
- A retained-error fixture proves that accepted construction windows actually include injected
  erroneous bases at the profile under test.
- Identical sequence/error events with shuffled qualities change only the quality-dependent outcomes
  predicted by the declared model.
- Generation and independent replay agree across linear/circular, strand, wrap, SE, and PE cases.

## Consequences

Historical validation bundles remain useful debugging artifacts but cannot support a sensitivity
claim. Repairing the evaluator may change schema and golden outputs. That is an intentional
versioned scientific correction, not a compatibility regression.

