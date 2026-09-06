# Virustic2 baseline audit

- Audit date: 2026-09-03
- Migration review date: 2026-09-04
- Repository: <https://github.com/williamtbarker/virustic2>
- Pinned commit: `b211915fc7cce82629766b77024463c6cabcc749`
- Package: Virustic2 0.1.0, MIT, Rust 2021, MSRV 1.85
- Audit host: Linux 6.18.35 x86-64, Rust/Cargo 1.98.0

## Executive result

Virustic2 is a coherent exact-unitig assembler and a sound compatibility baseline. Its streaming FASTX parser, packed canonical k-mers, conservative graph traversal, deterministic FASTA, and narrowly stated benchmark are supported by code and tests. It is not a genome-reconstruction system: mates are reduced to a fragment-level k-mer set, graph branches remain unresolved, and no read-back evidence is calculated.

The audit also found contract defects not covered by the original test suite. Atomicity applies to each file, not the output set; mate identifiers are normalized without validating mate role; unresolved path aliases can make two requested outputs collide; and circularity is inferred from graph topology without molecular junction evidence. Virustic3 treats these as regression tests rather than inherited behavior.

## Architecture observed

| Module | Actual responsibility | Important behavior |
|---|---|---|
| `fastx` | Streaming wrapped FASTA/FASTQ parser and gzip sniffing | Uses gzip magic bytes and supports concatenated members. |
| `dna` | Two-bit DNA encoding and quality-aware rolling scan | Returns canonical k-mers for `3 <= k <= 31`; ambiguous windows are skipped. |
| `pipeline` | Lane/pair iteration, bounded Rayon batches, fragment counting | Parallel results are collected and merged in input order. |
| `graph` | Strand-symmetric node/edge support and graph cleaning | Stores four outgoing `u32` supports per `(k-1)` node. |
| `assembly` | Maximal nonbranching paths and canonical output | Collapses reverse-complement equivalents and canonicalizes cycles. |
| `output` | FASTA/JSON serialization and per-file temporary writes | Each destination is individually flushed, synced, and renamed. |
| `main` | CLI, output-path guards, error presentation | Writes FASTA before JSON, so the bundle is not transactional. |

## Confirmed compatibility obligations

- Plain or gzip-compressed FASTA and FASTQ; compression by content.
- Wrapped records and concatenated gzip members.
- Single-end, paired-end, ordered multi-lane, and standard-input operation.
- Lockstep pair count, format, and normalized identifier checks.
- Phred+33 validation and per-window base-quality filtering.
- IUPAC ambiguity handling with contextual errors for unsupported symbols.
- Fragment support (one canonical k-mer contribution per record pair) and occurrence support.
- Strand-symmetric packed graph, minimum-support pruning, and iterative weak-tip clipping.
- Deterministic unitigs across tested worker counts.
- Typed library errors, CLI context, JSON aggregate reporting, and same-directory atomic writes.
- Linux/macOS/Windows CI and Linux MSRV testing.

These are obligations inherited from the ancestor audit, not a statement that the current package is
drop-in compatible. VeritAsm substantially preserves the FASTX data-input behaviors, but several CLI,
output, default-algorithm, library, and platform surfaces deliberately or accidentally diverge.

## Non-parity and migration contract

Blanket drop-in Virustic2 compatibility is **not met**. No legacy frontend, output-schema translator,
or Rust API compatibility facade exists. Reviewers must either accept this explicit migration boundary
or require a separately tested compatibility layer; the present package must not be described as a
drop-in replacement.

| Surface | Virustic2 0.1.0 | VeritAsm `0.3.0-alpha.1` | Migration status |
|---|---|---|---|
| Package, executable, and library name | `virustic2` | `veritasm` | Breaking rename; Cargo dependencies, imports, and executable paths do not work unchanged. |
| Single-input flag | `-U`/`--single` plus `--input` alias | `-U`/`--single`; no `--input` alias | Replace legacy `--input` uses. |
| Output destination and stdout | Required `-o`/`--output FASTA`; `-` streams FASTA to stdout | Required `-o`/`--output-dir DIR`; no stdout-result or individual-artifact mode | Breaking semantic change: `-o` names a new directory, not a FASTA file. |
| JSON/report interface | Optional `--report FILE`; `-` streams JSON to stdout | Mandatory `run.json` and `report.html` inside every bundle; no `--report` | Breaking CLI and schema change; consumers require redevelopment or a translator. |
| k option | `-k`/`--kmer-size`, range 3--31 | `-k`/`--k`, range 3--63 | Short form/prior values remain usable; long-form scripts must migrate. |
| Graph/output filtering | `--min-contig-length`, `--tip-length`, and `--tip-support-ratio`; default tip clipping and output-length cutoff | No corresponding flags; stable path performs no tip/bubble deletion and no legacy length cutoff | Scientific/default-output non-parity; no flag-for-flag reproduction is available. |
| Support option | `--support-mode fragment\|occurrence` | `--support-unit supplied-fragment-instance\|accepted-window-occurrence` | Breaking flag and enum spelling; semantics are more explicit. |
| Batch option | `--batch-size` | `--batch-fragments`, with a separate internal scheduling cap | Breaking flag rename and operational-contract change. |
| Thread automation | `--threads 0` means automatic and is the default | Omission means automatic; explicit zero is invalid | Remove `--threads 0` from scripts. Positive values retain the same spelling. |
| Quiet/success output | `-q`/`--quiet`; otherwise a Virustic2 completion summary | No quiet flag or compatible completion-summary contract | Breaking operational-output change. |
| FASTA identity/header | Sequential `virustic2_000001` IDs; fields include `unique_bases`, floating mean support, and `circular=true\|false` | Digest-derived `utg-...` IDs and schema-1.1 evidence fields; `topology=closed_graph_walk` is not molecular circularity | Breaking header contract; downstream parsers must migrate. |
| Aggregate JSON content | Includes raw paths, thread/batch settings, cleaning fields, and elapsed wall time | Deterministic evidence schema uses sanitized source labels/digests and excludes thread/time from stable artifacts | No JSON-schema compatibility. |
| Existing destination | Per-file atomic writes can replace existing FASTA/report; the output set is not transactional | Existing destination is rejected; one staged bundle commits with no replacement | Intentional safety improvement, but overwrite workflows are incompatible. |
| Pair acceptance | Normalized identity/cardinality checks; role swaps and physical aliases were accepted | Declared mate roles and physical-source reuse are fatal typed errors | Intentional strictness improvement; previously accepted malformed invocations fail. |
| Error/exit contract | Free-form `error: ...`; operational failures exit 1 | `error[code]: context`; typed exit families 2--8 or 70 | Breaking process and diagnostic contract. |
| Windows | Ancestor CI included Windows, although this audit did not execute it | CI targets Linux and Apple Silicon macOS; current bundle code imports Unix filesystem APIs | Windows build parity is absent and unclaimed. This is outside the stated Linux/macOS mission but remains an ancestor migration gap. |
| Rust library API | `assembly`, `output`, `PackedGraph`, `Contig`, `AssemblyReport`, and `SupportMode` surfaces | Different modules/types including `RunOutcome`, `ScientificConfig`, `SupportUnit`, and typed errors | No source-compatible Rust API. |

Current contracts are in [`CLI_CONTRACT.md`](CLI_CONTRACT.md) and
[`OUTPUT_SCHEMA.md`](OUTPUT_SCHEMA.md); current CLI, bundle, and error implementations are in
[`src/main.rs`](../src/main.rs), [`src/bundle.rs`](../src/bundle.rs), and
[`src/error.rs`](../src/error.rs). Input-data compatibility does not imply command, artifact, platform,
or scientific-output parity.

## Confirmed defects and misleading boundaries

1. **Output-set failure can replace an existing result.** FASTA commits before report creation. A deliberately invalid report destination caused exit 1 after the prior FASTA was replaced.
2. **Path aliases can collide.** When both destinations do not yet exist, canonicalization cannot identify paths such as `x/result` and `x/sub/../result`; JSON can overwrite the requested FASTA destination.
3. **Mate role is not validated.** R1 `/2` plus R2 `/1`, two CASAVA mate-1 records, and suffixed/unsuffixed variants sharing a first token were accepted.
4. **Physical-file aliases are not rejected.** A path and its `./` alias can be supplied as both mates.
5. **Fragment support is record-instance support.** IDs and repeated lanes are not deduplicated, so repeating a lane doubles support.
6. **Circularity means a closed one-in/one-out graph cycle.** It does not establish a circular molecule or a read-supported closing junction.
7. **Input streaming does not bound total memory.** Distinct observed k-mers remain resident until post-ingestion pruning; an arbitrarily long record is materialized.
8. **Support saturation is silent.** Counts use `saturating_add(u32)` without a counter or warning.
9. **Quality-aware is thresholded, not probabilistically weighted.** This is valid but narrower than the phrase can imply.

## Verification and reproduction

The following gates passed on the audit host:

```text
./scripts/verify.sh
cargo +1.85.0 test --locked --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
cargo audit
```

Results: 22 library tests, 2 binary tests, and 7 CLI tests passed (31 total); release build, documentation, package verification, MSRV, and dependency-advisory checks passed. Static inspection found no runtime network path. Runtime syscall tracing was unavailable because ptrace was blocked.

The documented 200,000-read synthetic input reproduced exactly:

```text
input SHA-256  396cac8bd9878e19f8fb902b21e7e6002078a862bb474ba8be75529d49f48e8d
FASTA SHA-256  f42b48f47e907453b45f5c5925fbf9e1bb9551ecf4aa8c15b2e43a59b36e644b
```

On the coordinating run, one thread used 4.95 s wall/14,988 KB peak RSS. Four threads used 6.32 s/22,936 KB and was slower under shared-host contention while producing byte-identical FASTA. An independent repeat measured 4.92 s/14,876 KB and 3.90 s/19,416 KB. These measurements establish reproducibility and variability, not a stable performance claim.

## Missing baseline validation

- Property and fuzz tests.
- Mate-role, physical-alias, duplicate-lane, and multi-lane adversarial cases.
- Output-bundle transaction failure and unresolved path aliases.
- Broad malformed FASTQ/gzip cases and support saturation.
- Repeats, bubbles, uneven coverage, mixtures, contamination, and molecular circularity controls.
- Real viral data and reference-based accuracy metrics.
- Cross-platform output digests, license policy automation, and runtime network enforcement.

## Inheritance decision

Virustic3/VeritAsm retains the parser semantics, rolling DNA encoding, fragment/occurrence definitions,
stable-boundary sorting, conservative palindromic-node boundary, and typed-error separation. It
replaces pair validation, multi-artifact commit logic, path identity handling, graph/evidence schemas,
and the unqualified `circular` label. Those replacements improve specified tested behaviors but make
blanket/drop-in ancestor parity false until an accepted migration or compatibility layer covers every
surface above.
