# VeritAsm repository instructions

These instructions apply to every file in this repository. `VeritAsm` is a provisional working
name until the architecture gate records a collision check and final decision.

## Purpose

Build a neutral, evidence-first Rust assembler for Illumina-like single-end and paired-end short-read
DNA. Its first bounded domain is synthetic constructs, plasmids, microbial isolates, organellar DNA,
and small or moderately complex mixtures. Low-abundance recovery in high-background sequencing is a
research/QC application, including adventitious-agent assessment, but not the product's biological
identity.

The program reconstructs and audits sequence. It does not identify an organism, establish biological
presence, certify a manufacturing lot, or make a clinical or regulatory decision.

## Start every work session by reading

1. `docs/PRODUCT_CONTRACT.md`
2. `ARCHITECTURE.md`
3. `VALIDATION.md`
4. `docs/SCIENTIFIC_LIMITATIONS.md`
5. relevant architecture-decision records
6. current repository state and tests

If a required document does not exist, finish the appropriate evidence gate before implementing
undocumented behavior.

## Baseline obligations

Virustic2 commit `b211915fc7cce82629766b77024463c6cabcc749` is the implementation ancestor and a
frozen comparison point, not the new product contract. Preserve or explicitly migrate its working
plain/content-detected-gzip FASTA/FASTQ input, single/paired/multi-lane modes, streaming batches,
quality and ambiguity handling, deterministic reduction, safe Rust, contextual errors, and local
offline execution. Correct its pair-role, path-alias, support-saturation, total-memory, circularity,
and multi-artifact transaction defects. Never modify the pinned baseline checkout.

## Scientific rules

- Define every quantity; k-mer support, read depth, fragment support, abundance, confidence, and
  detection are not interchangeable.
- A unitig or contig is reconstructed sequence, not proof of an organism or contamination event.
- Preserve uncertainty at unresolved repeats and branches. Never fabricate a join.
- Pair evidence requires validated identifiers/roles, mapping uniqueness, orientation, insert-span
  compatibility, and an explicit algorithm.
- Low abundance is not equivalent to sequencing error. Every removal rule must be measured and
  reversible through a documented profile or parameter.
- Negative/process controls and optional reference classification are separate analysis layers; they
  never silently alter the de novo graph.
- Reference databases must be optional, versioned, checksummed, offline at runtime, and clearly
  separated from de novo output.
- Retain negative benchmark results, failures, timeouts, and regressions.
- Never claim “extremely accurate,” “general purpose,” bounded memory, organism detection, clinical
  validity, regulatory suitability, or production readiness without direct retained evidence.

## Engineering rules

- Prefer one complete, validated vertical slice to broad scaffolding.
- Keep input, encoding, graph construction, transformation, compaction, reconstruction, evidence,
  reporting, and CLI boundaries independently testable.
- Prefer modules until crate separation has a measured benefit.
- Make parallel reductions deterministic and compare complete output bundles across thread counts.
- Bound record/batch memory; place an explicit deterministic limit on in-memory graph state until
  exact disk partitioning is implemented.
- Commit an immutable result directory only after every artifact has been written and validated.
- Use typed library errors and contextual CLI errors.
- Forbid unsafe project code unless a later benchmark and dedicated ADR justify a narrow boundary.
- Add dependencies only after maintenance, license, advisory, MSRV, and necessity review.
- Stable schemas require versioning and migration notes; experimental fields must say so.
- The default executable performs no network access and requires no Python, Java, Conda, database,
  or external command.

## Editing and validation discipline

- Inspect before editing; preserve unrelated work.
- Add or update tests with every behavior change and run the narrowest tests immediately.
- Profile before optimizing and retain before/after conditions and failures.
- Use truth inaccessible to assembly commands and introduced only during evaluation.
- Pre-register datasets, parameters, metrics, and exclusion rules before scoring.
- Compare task-equivalent tools and publish installation or evaluation failures.

## Required release-candidate checks

- `cargo fmt --check`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo test --locked --all-targets --all-features`
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`
- release build and `cargo package` verification
- Rust 1.85 MSRV plus current stable
- dependency advisory and license/source review
- fuzz-corpus smoke runs and property tests
- clean-extraction verification
- complete-bundle determinism tests
- failure-injection proof that existing results survive

Do not push, publish, create a release, or mutate an external service. Stop with an unpushed source
package and exact human verification/push commands.

