# ADR 0009: Truth-separated executable validation plane

- Status: accepted for the qualification-tooling vertical slice
- Date: 2026-09-03

## Context

The initial repository contained a useful perfect-read fixture and an exact linear-substring example,
but no executable origin/error ledger, circular-truth evaluator, false-junction metric, or stable
dataset/result record. Allowing an assembly command to consume truth would invalidate de novo
comparison, while treating empty assembly output as a parser error would hide an important result.

## Decision

Build two separate Rust binaries over a validation-only library module:

- `veritasm-simulate` writes opaque FASTQ under `assembler_input/` and exact sequence, origin, and
  error truth under `evaluation_truth/`, with a versioned dataset record and checksums.
- `veritasm-evaluate` alone reads truth, verifies dataset artifact identity, performs bounded
  deterministic fitting alignment plus exact-flank adjacency classification, and commits a
  checksum-bearing evaluation bundle.

The generator uses a source-frozen SHA-256 counter RNG and the validation-plan seed derivation. The
evaluator recognizes circular topology only from truth metadata and admits rotation/seam traversal;
it never infers molecular circularity from assembler output. Empty FASTA output is a completed
evaluation with explicit unavailable metrics where denominators are zero.

Truth separation is an invocation contract, not an operating-system security boundary. Validation
tools remain outside `assemble`; neither generated truth nor evaluator scores can alter graph
construction.

## Consequences

- SE, PE, circular, two-component, and substitution cases now have deterministic machine provenance.
- Base and output-adjacency results retain raw alignment/classification rows and exact denominators.
- Direct dynamic programming and exact flank scanning need explicit resource caps and are suitable
  only for small truth-known cases.
- One primary alignment and exact flanks do not solve repeat-aware or large-genome evaluation.
- This slice does not admit a dataset, execute a comparator, or establish an accuracy improvement.

## Deferred acceptance

The full pre-registered matrix, error/artifact models, cross-architecture byte verification, public
data admission, independent indexed aligners, split-alignment misassembly events, graph metrics,
minor-path scoring, experiment orchestration, and comparator scorecards remain required.
