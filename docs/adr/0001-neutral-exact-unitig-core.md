# ADR 0001: Neutral exact unitig core

- Status: accepted for 0.1 implementation
- Date: 2026-09-03

Chronology note: this ADR, the initial implementation, and the initial research/architecture records
first entered this repository together in commit `ade474f`. It is therefore a contemporaneous or
retrospective record of the implemented decision, not independent Git-history evidence that the ADR
was accepted before implementation began. Future changes to this decision require a superseding ADR
accepted before the corresponding implementation change.

## Context

Short-read assemblers trade contiguity against false joins, especially in mixtures, repeats, and uneven
depth. The product must remain useful without taxonomy or a reference and must not present an inferred
global path as direct evidence.

## Decision

The scientific primitive is an exact fixed-k de Bruijn graph constructed from accepted original-read
k-mers, followed by deterministic topology-preserving compaction into conservative graph segments.
ADR 0010 supersedes the original unqualified maximality wording: segments are maximal only between its
declared degree and fixed-point boundaries, including boundaries incident to self-complemental k-mer
edges. Unresolved branches terminate unitigs and remain explicit in GFA. The supported k range is 3
through 63 with exact `u128` keys. Stable 0.1 accepts one k per result. A future multi-k mode may be an
ensemble of independent child runs only after its parent/child bundle schema is accepted; it cannot
carry derived contigs forward or merge evidence.

The product is a neutral assembler. Low-abundance/high-background and adventitious-agent workflow
development are applications, not identities or organism-detection claims.

## Consequences

- A short or fragmented result can be scientifically correct when linkage is absent.
- Fixed-k sensitivity and repeat tradeoffs remain visible and motivate an isolated sweep.
- Unitigs do not imply genome completeness, circularity, taxonomic identity, or haplotypes.
- Database-free output is stable even when optional future analysis databases change.

## Rejected for 0.1

- an iterative small-k contig reused as observed evidence at larger k;
- automatic majority bubble collapse;
- coverage-flow global paths;
- taxonomy-directed retention;
- forced paired-end scaffolds.

## Evidence

See the research review of [exact de Bruijn graphs](../../RESEARCH.md#1-exact-de-bruijn-graphs),
[compacted de Bruijn graphs](../../RESEARCH.md#3-compacted-de-bruijn-graphs),
[fixed-k and multi-k alternatives](../../RESEARCH.md#6-fixed-k-iterative-and-multi-k-assembly),
[paired-end repeat resolution](../../RESEARCH.md#8-paired-end-repeat-resolution),
[bubbles and mixed samples](../../RESEARCH.md#9-bubbles-variants-and-mixed-samples), and the
[bounded recommendation](../../RESEARCH.md#recommendation), together with the cited primary
literature in those sections. Literature supports the design choice; executable evidence for this
implementation is tracked separately in [VALIDATION.md](../../VALIDATION.md).
