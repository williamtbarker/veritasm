# Changelog

All notable source and contract changes are recorded here. This project follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) in structure. No compatibility or semantic-
versioning promise begins until a first reviewed release is explicitly tagged.

## 0.4.0-dev.1 — Development checkpoint

- Include the experimental v0.4 modules, integration tests and development tooling.
- Make source packaging announce success only after owned temporary state is removed.
- Reconcile package metadata, source inventory, compressed fixtures, and source archive tooling.
- Add a concise project entry point, current verification status, module map, and bounded restart plan.
- Reuse local Cargo build artifacts during verification; keep expensive release/fuzz campaigns opt-in.
- Replace stale operational hash constants with a limit-perturbation regression while preserving the scientific golden and complete repeated-run byte checks.
- Apply standard Rust formatting to the integrated source and mark dormant recompaction work explicitly.
- Use infallible file-mode conversions on macOS; the maintainer's Rust 1.98.1 verifier passed.
- Retain earlier reports and failures as historical records.
- Simplify public documentation and move obsolete import instructions and duplicate review material out of the source tree.

## 0.3.0-alpha.1 — Unreleased

Status: pre-release implementation review candidate. The project name is provisional. The intended
fixed-k assembly contract v0.1 is implemented, but scientific and cross-platform validation are
not complete. This entry makes no general accuracy, performance, portability, or
production-readiness claim.

### Added

- An early exact destination lease acquired before input consumption or work-directory creation,
  plus explicit aggregate raw-transport and gzip-member limits with typed failures and schema-1.2
  effective-limit reporting.
- A neutral, database-free fixed-k unitig-assembly contract for Illumina-like single-end and strictly
  synchronized paired-end short DNA reads.
- Content-detected plain/gzip FASTA and FASTQ ingestion, ordered multiple-lane configuration, strict
  pair identity/role/cardinality rules, IUPAC handling, and Phred+33 window filtering.
- An immutable, checksummed fragment-spool design so repeated scientific passes use one validated
  observation stream.
- Full canonical `u128` k-mer identities for `3 <= k <= 63`, checked `u64` support arithmetic, exact
  supplied-fragment-instance and accepted-window-occurrence support modes, and explicit retention
  profiles.
- An isolated experimental exact compaction oracle over materialized external support rows through
  k=127, with literal oriented-handle degrees, conservative reverse-complement fixed-point
  boundaries, branch-preserving exact-overlap links, full-key step provenance, checked allocation
  admission, conservation ledgers, and independent byte-string oracle tests. It is not integrated
  with the stable CLI and makes no scalability, accuracy, or performance claim.
- Deterministic compacted de Bruijn-graph unitigs and conservative GFA 1.0 `H`/`S`/`L` output without
  branch selection or pair-created links.
- Exact zero-mismatch construction-read placement states, per-unitig placement aggregates, and
  conservative cross-unitig pair endpoint observations that never join sequence.
- A versioned evidence bundle containing FASTA, GFA, three evidence/audit TSVs, a transformation TSV,
  `run.json`, self-contained HTML, schemas, and a complete SHA-256 manifest.
- A same-filesystem staged bundle transaction with an operating-system no-replace commit primitive;
  existing destinations are not overwrite targets.
- Control-escaped, single-line typed error context and ownership-checked cooperative-lock cleanup,
  with failpoints spanning lock initialization and every bundle-render/verification milestone.
- Count writers that remain poisoned after an observation/write failure, authenticated append-only
  run manifests, checked spool/count sizes, and a bounded public manifest-only bundle verifier. The
  default verifier limits a manifest to 1 MiB, the inventory to 4,096 regular files and 1,024
  directories, path depth to 32 components, and hashed artifact bytes to 16 GiB; callers may supply
  explicit alternate bounds. Adversarial tests cover corruption, races, and inclusive resource
  boundaries. Manifest verification is an integrity check, not schema or biological validation.
- Typed configuration, input, pair, resource, integrity, destination, commit, and internal error-code
  families.
- A deterministic non-biological development fixture generator. It is not the pre-registered
  scientific benchmark simulator.
- A truth-separated validation-tooling slice with a 256-bit deterministic SHA-256 counter generator,
  SE/PE linear, circular, mixture, retained-substitution, and explicit QC-censoring-control cases;
  per-read origin, error, and non-default-quality ledgers; strict dataset/result schemas;
  ambiguity-aware exact-recovery bounds; rotation-aware fitting alignment; explicit empty-output
  semantics; exact-flank adjacency classification; raw metric rows; resource caps; and
  checksum-bearing no-replace output directories. It remains qualification tooling, not completed
  scientific evidence.
- Fail-closed evaluator parsing and cross-artifact replay for generated FASTQ, origin, substitution,
  and quality-event ledgers, plus full SHA-256 dataset identities that commit every frozen
  generator parameter including output compression. Checksum-refreshed malformed ledgers are
  rejected before any evaluation bundle can be committed.
- Research, architecture, validation, benchmark, security, contribution, and verification documents.
- A requirement-traceability ledger that maps the original baseline, research and architecture
  phases, implementation priorities, validation matrix, engineering gates, and deliverables to
  present code, tests, retained evidence, and explicit gaps.
- Release CI definitions that separate current-stable and Rust 1.85 Linux gates, assert both kernel
  and Rust target architecture on the official Apple Silicon runner, byte-compare source and Cargo
  packages, and independently build/test the extracted Cargo package.
- A bounded source-ZIP preflight and a fail-closed release-verifier finalizer whose authoritative
  summary is written only after isolated build-state cleanup and transcript closure, plus a focused
  no-compilation fault-injection self-test. The verifier also enforces the exact stable bundle
  inventory and compares all committed smoke-bundle file paths and bytes across toolchains/threads;
  focused regression tests are part of the clean-source CI job.

### Changed from the frozen implementation ancestor

- Reframed the software as a neutral short-read assembler. The frozen predecessor remains a
  comparison point, not the current product contract.
- Changed output from independently committed files to one verified result directory.
- Changed pair handling from input validation only to evidence-only endpoint co-observations while
  retaining the prohibition on sequence joins.
- Removed stable tip deletion, minimum-unitig-length filtering, and biological circularity wording;
  unresolved alternatives and graph topology remain explicit.
- Excluded raw input paths, timestamps, thread counts, host data, and temporary paths from committed
  deterministic artifacts.
- Kept the deterministic source packager compatible with macOS's Bash 3.2 and either `sha256sum` or
  `shasum -a 256`; this is source portability work, not evidence from a completed macOS build.
- Made source-package rollback, temporary-directory removal, and cooperative-lock cleanup failures
  visible and release-blocking instead of allowing an otherwise successful packaging command to hide
  them.
- Documented a unique commit-bearing handoff directory (or optional outer review bundle) that preserves
  the verifier-required canonical source-ZIP and sidecar basenames; renamed inner files are rejected.
- Corrected the experimental simulator's negative-strand reads from reverse-only to reverse-complement
  sequence, bumped its generator/dataset/read namespaces to version 3, and added independent
  strand, error-ledger, graph-orbit, schema, tamper, and paired assembly/evaluation regressions.
- Corrected release smoke comparison so generated `manifest.sha256` files are accepted without
  weakening source-package path policy, and made comparison failures identify the affected bundle.
- Kept isolated-target ownership for the verifier's entire lifetime and added final filesystem
  attestation so recreated, unregistered, or incompletely removed build state cannot be reported as
  cleanup success.
- Separated check/Clippy, test/doc, and release/package targets for both source ZIPs and independently
  extracted crates so execution and linking gates cannot inherit cross-mode build state.
- Moved member-name inspection into the ZIP central-directory preflight and added focused CR/LF,
  finite-ratio boundary, and infinite-ratio rejection tests that run before decompression.
- Corrected stale dependency/archive evidence references, reconciled the Bloom prototype's implemented
  primitives with its unintegrated status, repaired ADR research links, and disclosed that the initial
  research, architecture records, and implementation entered Git history together rather than
  claiming a demonstrable research-before-code chronology.
- Documented that blanket drop-in Virustic2 parity is unmet: CLI spelling and semantics, output and
  report schemas, graph-cleaning defaults, Rust APIs, exit codes, and Windows support require migration
  and have no compatibility wrapper or translator.
- Promoted the q=15 indexed exact linear-unitig mapper into construction-read auditing, retained the
  interval scanner as a differential test oracle, admitted persistent and per-query mapper allocation
  estimates, and carried the performing instance's descriptor and accounting into machine and HTML
  output. This is not an end-to-end speed or peak-RSS claim.
- Isolated raw cross-unitig pair-link aggregation by immutable input-lane ordinal, added complete
  zero-inclusive per-lane pair-state partitions and global/per-lane reconciliation to `run.json` and
  HTML, and bumped the bundle/run and pair-link artifact schemas to 1.1 while leaving unchanged
  pair-summary and transformation row schemas at 1.0. This provenance repair does not resolve repeats
  or change topology.
- Replaced the qualification evaluator's repeated per-adjacency truth scans and retained placement
  sets with lossless packed occurrence indexes and streamed `all`, `non-correct`, or `summary`
  evidence modes. Evaluator/result and junction schemas are version 2; exact-substring alignment,
  scan work, index capacities, output bytes, logical-row counts, and omission semantics are explicit.
- Repaired the qualification oracle under ADR 0015. Generator/dataset version 4 domain-separates
  layout, substitution, and quality streams, records every non-default quality event, independently
  replays origin geometry, and keeps the superseded substitution-linked-Q10 fixture only as an
  explicit censoring control. Evaluator/result version 3 treats the primary alignment as diagnostic,
  publishes unique and compatible-coordinate lower/upper exact-recovery bounds, and emits unavailable
  duplication/recovery ratios when exact ambiguity or an incomplete approximate-placement universe
  prevents identification. Historical generator-v3 error/mixture/recovery outputs are not admitted
  qualification evidence.
- Bound each evaluator truth or assembly digest to the same owned bounded byte snapshot that is
  parsed and scored, so later path replacement cannot mix metrics from one file with provenance from
  another. Dataset artifacts are traversed beneath one held root descriptor without following
  descendant symlinks. The public result validator also reconciles the no-index status with zero
  eligible adjacencies; binds all input identity fields; and recomputes record/base, alignment,
  coverage, adjacency, ratio, and QV invariants. QV formatting now uses a frozen integer algorithm
  rather than platform-dependent floating-point logarithms.
- Corrected fitting-alignment tie optimization so edit distance, descending matches, indels, and
  oriented start are optimized globally before molecule/strand ordering. A compact checked score
  encoding is compared with an independent exhaustive short-sequence oracle; the change prevents an
  insertion/deletion traceback tie from changing the selected truth molecule.
- Hardened source/release packaging against same-length input replacement, live-source mutation,
  governed-tree exclusions, case-fold collisions, and inherited transcript writers. Identity
  descriptors are never reused as content readers, private verifier copies are byte-bounded, and
  transcript finalization times out and fails closed. The Darwin descriptor semantics are covered by
  Linux-side portability regressions, not by an executed Apple Silicon result.
- Corrected even-k compaction at self-complemental k-mer handles. Both incident nodes are now
  conservative boundaries, emitted walks may not repeat a backing canonical key, and every unitig
  must satisfy `edge_steps == canonical_kmers`; exact neighboring alternatives remain in GFA. FASTA,
  GFA `SC`, and unitig-evidence artifact schemas advance to 1.1 for this semantic correction.

### Experimental

- A safe-Rust, insertion-only Bloom filter and two-hit candidate-sieve primitive are isolated
  research code. They are not exposed by the stable assembly CLI and are not authorized to define
  graph membership, counts, evidence, or biological interpretation.
- A standalone paired-end library-model substrate reports per-lane FR/RF/FF/RR observations,
  integer empirical span quantiles, a reconciled exclusion ledger, and explicit availability gates.
  It is disconnected from stable output and cannot infer gaps or join sequence.
- An exact four-limb wide-k representation and rolling scanner supports research k values through
  127 while remaining disconnected from stable spool, counter, graph, schema, and CLI formats.
- The exact minimizer/super-k-mer oracle now binds every occurrence and span to an independently
  recomputed source byte window and rejects noncanonical or wrong-owner mutations. A separate
  typed-support, fixed-width wide-key slice performs deterministic authenticated spill and
  bounded-fan-in exact reduction with explicit source/routing domains, temporary/open-file limits,
  predecessor-reclamation records, and corruption tests. Its 72-byte correctness record is not the
  width-specialized production data plane; parallel construction, direct cDBG
  compaction, and stable-pipeline integration remain unimplemented.
- An isolated immutable-spool bridge now replays exact QC semantics through k=127, differentially
  checks both stable support units through k=63, and authenticates support/source/config identity in
  XWR schema v2. It is not wired to the stable CLI, graph, bundle, or report.
- Multi-k orchestration, probabilistic scheduling, coverage reconstruction, molecular-circularity
  evidence, local path reconstruction, and reference-assisted analysis remain proposed work.

### Known release blockers and non-results

- An earlier, now-superseded `0.3.0-alpha.1` candidate-tree gate recorded formatting,
  all-target/all-feature check, warnings-denied Clippy, 305 all-target tests, warnings-denied rustdoc,
  release build, and Cargo package verification under Rust 1.85.0 and stable 1.98.1 on x86-64 Linux.
  Subsequent source changes mean the full MSRV/stable gate for the current tree is **NOT RUN**. The
  final commit-bound source archive and independently extracted crates also require the isolated
  verifier; the historical result is not current-tree or release-archive evidence.
- Apple Silicon macOS execution has not been run; CI configuration is not evidence that it passes.
- Historical dependency vulnerability and automated license-policy checks passed an unchanged
  third-party resolution, but fresh `cargo-audit`/`cargo-deny` runs on the current manifest and lock
  hashes were **NOT RUN** because those tools and a RustSec database were unavailable. Redistribution
  terms for native code bundled by the fuzz-only `libfuzzer-sys` dependency remain unresolved, so
  its dependency closure must not yet be redistributed. The seven fuzz targets and seed corpora are
  present, but `cargo fuzz check`, ASan/libFuzzer, and LeakSanitizer are **NOT RUN** against this
  0.3.0-alpha.1 candidate. A pinned 10-second-per-target smoke passed only for historical
  0.2.0-alpha.1 source commit `cdb88007f2643776b092e8379b200dfb1404ac0c`; it is not evidence for
  this source tree. Sustained campaigns and full failure-injection coverage remain pending.
- Static source/dependency inspection found no runtime networking implementation. Dynamic
  syscall- or namespace-isolated runtime verification could not run in the available environment and
  remains a release gate.
- A binary-hash-bound dirty-tree engineering probe on one perfect 1 Mb paired fixture produced four
  byte-identical result trees and one error-free 999,949-base unitig. The superseded evaluator printed
  99.9949% selected-target coverage and zero false eligible junctions, but ADR 0015 invalidated that
  recovery interpretation; those values are retained only as rerun targets. One timed run at eight
  threads was slower than one thread. With no comparator, one favorable dataset, and one timed run
  per setting, this is not an admitted scientific, scaling, sensitivity, or production result.
- A prior development summary reported an exact-substring tie and worse VeritAsm runtime/peak RSS on
  one tiny, easy, perfect-read fixture. Its raw 15-run commands, logs, timing records, bundles, and
  evaluator outputs are unavailable in this handoff, so neither the reported tie nor regression is
  retained execution evidence. The transcribed values remain only a conservative rerun target.
- Public scientific datasets, established-tool comparisons, and the pre-registered validation matrix
  have not been run. No general improvement over the frozen predecessor or another assembler is
  demonstrated. See [VALIDATION.md](VALIDATION.md) and [BENCHMARK.md](BENCHMARK.md).
