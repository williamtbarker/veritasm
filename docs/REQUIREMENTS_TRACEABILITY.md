# VeritAsm original-requirement traceability

> Historical review dated 2026-09-05. See [current status](STATUS.md) for
> subsequent verification and the current package version.

- Review date: 2026-09-05
- Original mission name: Virustic3
- Reviewed development snapshot: based on commit
  `c565568368e9`; Cargo metadata remains `0.3.0-alpha.1` until the source freeze
- Frozen ancestor: Virustic2 commit `b211915fc7cce82629766b77024463c6cabcc749`
- Disposition: unreleased v0.4 development tree; research use only and not production qualified

## Purpose and evidence rules

This document maps the original mission to decisions, source, tests, retained evidence, and open
gaps. The neutral VeritAsm name does not erase requirements inherited from the original Virustic3
request. A link to code or a checked-in test establishes implementation intent, not successful
execution. A configured CI job establishes intent, not a platform result.

Status terms are deliberately narrow:

| Status | Meaning |
|---|---|
| **Implemented** | Source for the bounded behavior exists in the stable path. |
| **Candidate-tree pass** | The identified 2026-09-03 Linux candidate-tree command passed; it is not evidence for later edits or the final ZIP. |
| **Transient-tree pass** | The exact named command passed on a dirty development tree; later edits, a clean extraction, another toolchain, and another platform remain unqualified. |
| **Experimental** | Isolated source/tests exist, but the feature is disconnected from stable assembly output or lacks promotion evidence. |
| **Partial** | Only part of the requirement is implemented or executed. |
| **Proposed** | A design or validation plan exists, but the feature is not implemented. |
| **Not run** | The required execution evidence does not exist. |
| **Blocked** | A known issue prevents promotion or redistribution. |

The current tree contains changes after every recorded checkpoint. Every engineering gate must
therefore be rerun from the final clean source ZIP before a package can be approved. The authoritative
evidence boundary is [VALIDATION.md](../VALIDATION.md), not the mere presence of a test command below.

### Transient v0.4 Linux checkpoint

On 2026-09-05, while the uncommitted development working tree was based on
`c565568368e9`, Rust `1.98.1` passed the following commands:

```console
cargo +stable fmt --all -- --check
CARGO_TARGET_DIR=/tmp/veritasm-root-gate2 cargo +stable check --locked --all-targets --all-features
CARGO_TARGET_DIR=/tmp/veritasm-root-gate2 cargo +stable clippy --locked --all-targets --all-features -- -D warnings
CARGO_TARGET_DIR=/tmp/veritasm-root-gate2 cargo +stable test --locked --all-targets --all-features --no-fail-fast
```

The test command passed 607 tests and failed none: library 537; stable main binary 4;
experimental multi-k binary 3; stable CLI 25; low-file-descriptor counting 1; graph properties 4;
stable schemas 5; storage integrity 2; validation CLI 22; validation schemas 2; and examples 2.
The evaluator, simulator, and two other example targets each discovered zero tests. Active pair-path,
CLI-integration, benchmark, packaging, and audit edits began after that run, so it is already a
superseded transient checkpoint. It is not current-head, MSRV, macOS, documentation, release-build,
package, fuzz, clean-extraction, or scientific-comparison evidence.

### Active post-checkpoint work and blockers

| Workstream | Evidence-bound status at this review point |
|---|---|
| Pair-to-portfolio integration | Active. Opaque pair-graph/mapping/path substrates exist, but no final integrated result or post-integration full gate has been reported. |
| Multi-k CLI integration tests | Active. Seven cases passed stable/MSRV; after a live lease-contention case was added, eight passed stable while the eight-case MSRV rerun remained pending. The suite must rerun after pair integration and is not counted in the 607-test checkpoint. |
| Development benchmark | Planned, not run. The frozen draft covers all six generator-v4 built-ins, stable fixed-k, both multi-k profiles, the pinned Virustic2 ancestor, and a diagnostic k=31 child. Results will be `development_unbound`; public datasets and established assemblers remain **NOT RUN**. |
| Fuzz gate | `cargo +nightly-2026-08-18 fuzz check` compiled all seven targets with rustc 1.100.0-nightly and cargo-fuzz 0.13.2 on a dirty tree. No libFuzzer/sanitizer campaign ran, so fuzz qualification remains **NOT RUN**. |
| Package/release | No v0.4 source archive exists, the tree is not frozen, and Cargo metadata/source inventory still require release reconciliation. Archive, checksum, package, and clean-extraction statuses remain **NOT RUN**. |
| Threat hardening | No new High/Critical defect was reported inside the trusted-local-parent model. Two experimental multi-k blockers remain: snapshot verification uses separate pathname opens without one descriptor identity, and snapshot write/verification can coexist with both the original and decoded report while the aggregate model admits one report cap. The hostile-local-filesystem and aggregate-memory claims remain open until fixes and adversarial tests pass. |

The interim multi-k CLI report used x86-64 Linux, stable Rust 1.98.1 and MSRV Rust 1.85.0,
with `CARGO_TARGET_DIR=/tmp/veritasm-root-coherence` for Cargo:

```console
cargo +stable test --locked --test multik_cli -- --nocapture       # 8 passed after lease-race addition
cargo +1.85.0 test --locked --test multik_cli -- --nocapture       # 7 passed
cargo +stable clippy --locked --test multik_cli -- -D warnings     # passed
rustfmt +stable --edition 2021 --check tests/multik_cli.rs          # passed
```

The eight-case stable result includes a live two-process lease race; the last MSRV, focused Clippy,
and rustfmt records predate that eighth case. All commands predate pair-to-portfolio integration and
must be repeated afterward. They are not part of the 607-test all-target checkpoint or final release
evidence.

## Decision chronology disclosure

The repository cannot demonstrate that the original instruction to complete research before major
implementation was followed chronologically. Commit `ade474f` imported `RESEARCH.md`, the initial
architecture and ADRs 0001--0005, stable implementation source, and tests together. ADRs 0006--0009
also first entered history in the commits that added their corresponding experimental substrates.
Those records are useful contemporaneous or retrospective explanations, but their dates and accepted
labels are not independent evidence of a research-before-code sequence.

Later commits do provide an auditable sequence for subsequent corrections, failed verifier attempts,
and hardening. Future scientific-contract changes should land a reviewed research amendment or ADR
before implementation. This crosswalk is itself retrospective and must not be cited as contemporaneous
acceptance evidence.

## Non-negotiable Virustic2 baseline

The ancestor behavior and reproduced defects are recorded in
[BASELINE_AUDIT.md](BASELINE_AUDIT.md). “Preserved” below means within the narrower VeritAsm contract,
not general biological equivalence.

| Original requirement | Decision/contract | Code | Test or retained evidence | Status |
|---|---|---|---|---|
| Plain FASTA and FASTQ | Parse both formats through one validated stream contract. | [`input.rs`](../src/input.rs), [`fastx.rs`](../src/fastx.rs) | [`cli.rs`](../tests/cli.rs), input unit tests | **Implemented; transient-tree pass; final rerun pending** |
| Compressed FASTA and FASTQ | Support gzip, including concatenated members. | [`input.rs`](../src/input.rs) | CLI content/gzip tests and corrupt-member unit tests | **Implemented; transient-tree pass; final rerun pending** |
| Gzip detected from content, not filename | Inspect gzip magic before selecting transport. | [`input.rs`](../src/input.rs) | `detects_gzip_fasta_despite_a_fastq_filename`, concatenated-member tests | **Implemented; transient-tree pass; final rerun pending** |
| Single-end sequencing | One or more ordered `--single` lanes. | [`config.rs`](../src/config.rs), [`pipeline.rs`](../src/pipeline.rs) | CLI single-end and validation-tool tests | **Implemented; transient-tree pass; final rerun pending** |
| Paired-end sequencing | Strict synchronized `--read1`/`--read2` lanes; stable pair observations cannot join sequence. | [`input.rs`](../src/input.rs), [`pairs.rs`](../src/pairs.rs), [`experimental/authenticated_pair_graph.rs`](../src/experimental/authenticated_pair_graph.rs) | CLI valid/adversarial-pair tests plus experimental source-backed pair-path tests | **Stable input/evidence implemented and transient-tree tested; graph-path constraints remain experimental** |
| Multiple lanes | Preserve declared lane order and pair lists by position. | [`config.rs`](../src/config.rs), [`input.rs`](../src/input.rs) | `accepts_strict_paired_plain_and_gzip_lanes` | **Implemented; transient-tree pass; final rerun pending** |
| Strict synchronization and identifier validation | Check format, cardinality, normalized ID, mate role, order, and physical source identity. | [`fastx.rs`](../src/fastx.rs), [`input.rs`](../src/input.rs) | Mismatch, reorder, missing-mate, role-swap, format, alias tests in [`cli.rs`](../tests/cli.rs) | **Implemented; transient-tree pass; final rerun pending** |
| Streaming, bounded-memory input | Stream into a bounded-record immutable spool; count externally; cap retained graph state. | [`input.rs`](../src/input.rs), [`spool.rs`](../src/spool.rs), [`count.rs`](../src/count.rs) | Input/resource unit tests, [`count_open_files.rs`](../tests/count_open_files.rs), [`storage_integrity.rs`](../tests/storage_integrity.rs) | **Implemented by explicit limits/failure; transient checkpoint included the corrected low-FD test; whole-run measured bound remains partial** |
| Base-quality filtering | Hard Phred+33 acceptance; rejected windows are counted by reason. | [`dna.rs`](../src/dna.rs), [`count.rs`](../src/count.rs) | DNA properties and CLI quality-accounting test | **Implemented; transient-tree pass; final rerun pending** |
| IUPAC ambiguity handling | Validate IUPAC; non-ACGT resets a rolling window without imputation. | [`dna.rs`](../src/dna.rs), [`fastx.rs`](../src/fastx.rs) | DNA tests and `reports_quality_and_iupac_window_exclusions_without_coercion` | **Implemented; transient-tree pass; final rerun pending** |
| Determinism across thread counts | Sort every stable boundary and exclude runtime telemetry from scientific artifacts. | [`pipeline.rs`](../src/pipeline.rs), [`bundle.rs`](../src/bundle.rs) | `complete_bundle_is_identical_across_thread_counts`; historical five-case 1/4-thread smoke | **Partial:** selected Linux tests passed in the transient checkpoint; final-tree 1/2/4/8 and cross-platform matrix not run |
| Atomic output; failure cannot destroy an existing result | Stage, verify, and no-replace commit one directory. | [`bundle.rs`](../src/bundle.rs) and [ADR 0005](adr/0005-transactional-deterministic-bundle.md) | Existing-destination, failpoint, race, and manifest tests | **Partial:** ordinary tests passed in the transient checkpoint; final tree and exhaustive crash/power/filesystem matrix not run** |
| Apple Silicon macOS and Linux | Target both; keep portable Bash and OS-specific no-replace implementation. | Cargo source, packaging scripts, CI configuration | Historical and transient local Linux gates; macOS commands in [VERIFICATION.md](VERIFICATION.md) | **Partial:** the v0.4 transient Linux checkpoint is superseded; Apple Silicon **not run** |
| Documented MSRV | Rust 1.85 in package metadata and verification contract. | [`Cargo.toml`](../Cargo.toml), [VERIFICATION.md](VERIFICATION.md) | Recorded Rust 1.85 candidate-tree gates | **Implemented; final clean ZIP rerun pending** |
| No mandatory Python, Java, Conda, or external runtime | Stable executable is Rust-native and database-free. | [`Cargo.toml`](../Cargo.toml), stable source | Static source/dependency review; ordinary local executions | **Implemented by design; dynamic network-denied runtime gate not run** |

This table records substantial data-input and bounded behavior preservation, not blanket drop-in
compatibility. The binary/library name, several long flags and defaults, stdout and JSON/report
interfaces, FASTA headers, exit codes, Rust API, and Windows support differ. No legacy wrapper or
translator exists, so the original blanket preserve/improve obligation is **unmet as a compatibility
claim** unless an accepted migration explicitly narrows it. See the
[Virustic2 non-parity and migration table](BASELINE_AUDIT.md#non-parity-and-migration-contract).

## Phase 1 — deep research

The original research taxonomy covers de Bruijn/compacted graphs, exact and quality-aware counting,
multi-k, uneven depth, pairs, mixtures, quasispecies/haplotypes, bubbles, segmentation, circularity,
high-background scaling, validation, relevant tools, formats, and Rust reuse. The later
[Production-redesign review](research/2026-09-05-production-redesign.md)
audits the prior tree, updates maintained comparator/design inputs, and controls the v0.4 direction.
These documents support decisions; they do not validate VeritAsm.

| Requested deliverable | Decision/document | Evidence | Status |
|---|---|---|---|
| Algorithm taxonomy | [RESEARCH.md algorithm taxonomy](../RESEARCH.md#algorithm-taxonomy) | Primary-method references R1--R64 and bounded qualifications | **Complete as a targeted review; not systematic** |
| Competitor capability matrix | [RESEARCH.md concise snapshot](../RESEARCH.md#concise-competitor-capability-snapshot), [expanded matrix](COMPETITOR_MATRIX.md), and [v0.4 redesign snapshot](research/2026-09-05-production-redesign.md#competitor-capability-snapshot) | Capability, role, license, qualification constraint, and freeze-blocker details | **Documented; comparator binaries/results not frozen** |
| Known failure modes | [RESEARCH.md failure modes](../RESEARCH.md#known-failure-modes) | Mechanism, required response, citation per row | **Documented** |
| Features worth implementing | [Selected v0.1 features](../RESEARCH.md#selected-v01-features) | Mapped to architecture and ADRs | **Documented; implementation status varies below** |
| Features explicitly not implemented | [Deferred](../RESEARCH.md#deferred-features) and [rejected](../RESEARCH.md#rejected-v01-features-and-behaviors) lists | Explicit stable/non-goal boundaries | **Documented** |
| Scientific and engineering risks | [RESEARCH.md concise summary](../RESEARCH.md#concise-scientific-and-engineering-risk-summary) and [expanded register](SCIENTIFIC_RISKS.md) | In-file Phase 1 summary plus 31 named risks, exposure evidence, controls, and residual risk | **Documented; controls are not proof of elimination** |
| Citations and stable links | [RESEARCH.md references](../RESEARCH.md#references) and competitor matrix | DOI, official documentation, repositories, license links | **Documented as of review date** |
| Evidence-supported recommendation | [Original recommendation](../RESEARCH.md#recommendation) plus [v0.4 recommendation](research/2026-09-05-production-redesign.md#evidence-supported-recommendation) | Preserve the fixed-k exact oracle while building an authenticated external data plane and independent read-witnessed multi-k evidence before path projection | **Documented; experimental v0.4 slice partially implements it, qualification remains open** |
| Research before implementation | Original process constraint | Git chronology described above | **Not demonstrable retrospectively** |

## Phase 2 — architecture principles

| Original principle | Decision/contract | Code/evidence | Status |
|---|---|---|---|
| Useful database-free de novo engine | Exact fixed-k unitigs and GFA need no database; the experimental portfolio also requires no reference database. | [ADR 0001](adr/0001-neutral-exact-unitig-core.md), stable pipeline, [ADR 0026](adr/0026-authenticated-retention-and-multik-portfolio.md) | **Stable fixed-k implemented; multi-k experimental** |
| Separate input, graph construction/transformation, reconstruction, evidence, reporting, CLI | One crate with responsibility-specific stable and experimental modules. | Stable modules plus `experimental::{spool_external,retention,compacted_dbg,transition_witness,evidence_reconstruction,multik_pipeline,multik_bundle,pair_mapper,pair_path}` | **Implemented as modules; v0.4 modules are experimental** |
| Prefer modules to premature microcrates | Retain one main crate plus isolated fuzz package. | [`src/lib.rs`](../src/lib.rs), Cargo manifests | **Implemented** |
| Deterministic parallel execution | Ordered ingestion, deterministic partitions/reductions/ties/serialization; the first experimental portfolio deliberately rejects thread counts other than one. | [ARCHITECTURE.md](../ARCHITECTURE.md), thread-count tests, [ADR 0026](adr/0026-authenticated-retention-and-multik-portfolio.md) | **Partial platform evidence; experimental portfolio is serial** |
| Every filter/cleaning operation measurable | Stable QC/threshold decisions and experimental authenticated retention/correction decisions have explicit ledgers and conservation checks. | [`transform.rs`](../src/transform.rs), [`experimental/retention.rs`](../src/experimental/retention.rs), [`experimental/quality_correction.rs`](../src/experimental/quality_correction.rs) | **Implemented for the named operations; correction remains experimental and no tip/bubble deleter exists** |
| Do not silently discard plausible alternate paths | Stable compaction preserves branches; the experimental witnessed child excludes unsupported topology links but retains every decision, and the diversity profile emits every child segment. | [`compact.rs`](../src/compact.rs), [`experimental/evidence_reconstruction.rs`](../src/experimental/evidence_reconstruction.rs), [ADR 0023](adr/0023-read-witnessed-multik-reconstruction.md) | **Implemented within each declared support/evidence rule; biological plausibility is not inferred** |
| Consensus versus diversity is an explicit choice | `veritasm-multik` exposes `diversity-preserving` and `exact-agreement-consensus`; the latter only presents byte-identical sequence/topology found at at least two distinct k values and leaves excluded rows in child outputs. | [`experimental/multik_bundle.rs`](../src/experimental/multik_bundle.rs), [`veritasm-multik.rs`](../src/bin/veritasm-multik.rs) | **Experimental implementation; not stable or scientifically qualified, and not a phasing/consensus inference** |
| Use pairs conservatively; never fabricate joins | Stable output reports endpoint co-observations; experimental source-backed pair placement and path analysis can constrain only already existing graph paths. | [`pairs.rs`](../src/pairs.rs), [`experimental/authenticated_pair_graph.rs`](../src/experimental/authenticated_pair_graph.rs), [`experimental/pair_mapper.rs`](../src/experimental/pair_mapper.rs), [`experimental/pair_path.rs`](../src/experimental/pair_path.rs) | **Stable evidence-only behavior plus experimental graph-path substrate; no synthetic edge, scaffold, gap, or sequence join** |
| Keep reference assistance separate | Stable pipeline has no reference input or taxonomy. | CLI contract and stable source | **Separation implemented; reference stage proposed** |
| Version/checksum an optional database | No optional external database exists. | Research/architecture requirement only | **Proposed / not applicable to stable 0.1** |
| Prefer interpretable evidence to opaque scores | Exact counts, reason states, mappings, graph topology, transition/retention decisions, pair-path states, and content roots. | [OUTPUT_SCHEMA.md](OUTPUT_SCHEMA.md), [ADR 0027](adr/0027-opaque-source-backed-evidence-capabilities.md) | **Implemented for stable evidence and experimental source-backed capability chain** |
| Avoid unsafe Rust absent evidence | First-party crate forbids unsafe code; dependencies reviewed separately. | [`src/lib.rs`](../src/lib.rs), [DEPENDENCY_REVIEW.md](DEPENDENCY_REVIEW.md) | **Implemented for first-party source; dependency review remains bounded** |

### Evaluated pipeline

| Proposed architecture element | Selected design | Implementation/test anchor | Status |
|---|---|---|---|
| 1. Streaming FASTX ingestion and QC | Validate once and write an authenticated immutable spool. | `input.rs`, `fastx.rs`, `spool.rs`; CLI/storage tests | **Implemented** |
| 2. Rolling canonical k-mers | Exact two-bit canonical `u128`, `3 <= k <= 63`. | `dna.rs`; unit/property tests | **Implemented** |
| 3. Multi-k or iterative construction | Reuse one authenticated spool but independently count, retain, compact, transition-audit, and reconstruct each configured k; never feed a child sequence into another child. | [`experimental/multik_pipeline.rs`](../src/experimental/multik_pipeline.rs), [ADR 0026](adr/0026-authenticated-retention-and-multik-portfolio.md) | **Experimental multi-k portfolio implemented; no general cross-k path reconciliation or stable promotion** |
| 4. Compact graph | Stable exact oriented compaction plus an opaque retained-count compacted-graph oracle through k=127; an external topology seam exists separately. | `graph.rs`, `compact.rs`, [`experimental/compacted_dbg.rs`](../src/experimental/compacted_dbg.rs), [`experimental/external_cdbg.rs`](../src/experimental/external_cdbg.rs) | **Stable whole-resident narrow-k implementation; experimental wide/external substrates** |
| 5. Tip, error, low-complexity processing | No automatic tip/bubble deletion; authenticated exact retention is integrated experimentally, and an isolated journaled substitution/ambiguity correction experiment exists. | [`experimental/retention.rs`](../src/experimental/retention.rs), [`experimental/quality_correction.rs`](../src/experimental/quality_correction.rs) | **Retention integrated experimentally; correction isolated/unqualified; tip and low-complexity processing absent** |
| 6. Diversity-aware bubbles | Preserve retained branches and expose a diversity-preserving portfolio presentation; exact-agreement presentation never converts a bubble into a phased haplotype. | graph output plus [`experimental/multik_bundle.rs`](../src/experimental/multik_bundle.rs) | **Experimental profile choice implemented; bubble classification and biological consensus remain absent** |
| 7. Pair insert inference/repeat resolution | Source-backed exact linear-unitig placement and conservative existing-path analysis supplement the older lane-specific library summary. | [`experimental/authenticated_pair_graph.rs`](../src/experimental/authenticated_pair_graph.rs), [`experimental/pair_mapper.rs`](../src/experimental/pair_mapper.rs), [`experimental/pair_path.rs`](../src/experimental/pair_path.rs) | **Experimental path-constraint substrate implemented; insert-qualified repeat resolution and scaffolding not implemented** |
| 8. Conservative contig/haplotype reconstruction | Stable output emits unitigs; the experimental witnessed-child transform emits transition-supported segments/links independently at each k. | `compact.rs`, [`experimental/evidence_reconstruction.rs`](../src/experimental/evidence_reconstruction.rs), [ADR 0023](adr/0023-read-witnessed-multik-reconstruction.md) | **Stable unitigs plus experimental read-witnessed segments; no local/global haplotype claim** |
| 9. Read-backed assembly audit | Stable exact remapping is supplemented experimentally by exact source-replayed `(k+1)` transitions and opaque pair placement/path capabilities. | `audit.rs`, [`experimental/transition_witness.rs`](../src/experimental/transition_witness.rs), [`experimental/pair_mapper.rs`](../src/experimental/pair_mapper.rs) | **Implemented as internal consistency, not independent truth** |
| 10. Structured and HTML reporting | Stable bundle plus an experimental transactional multi-k FASTA/GFA/TSV/JSON/schema/checksum/offline-HTML bundle. | `bundle.rs`, `report.rs`, [`experimental/multik_bundle.rs`](../src/experimental/multik_bundle.rs) | **Stable fixed-k and experimental multi-k implementations; multi-k package qualification pending** |

## Phase 3 — implementation priorities

### Priority A

| Requirement | Decision/code/test evidence | Status |
|---|---|---|
| Multi-k or research-justified alternative | `veritasm-multik` builds source-bound independent k children from one immutable spool; it neither launders child contigs into evidence nor splices across k. | **Experimental implementation and transient-tree test pass; scientific qualification/stable promotion pending** |
| Compacted graph | Exact stable narrow-k compaction plus experimental opaque retained-count compaction and read-witnessed adjacency filtering. | **Implemented stable fixed-k and experimental multi-k paths; transient-tree test pass; external production-scale compaction remains incomplete** |
| Genuine paired-end graph constraints | Opaque source-backed mapping and pair-path analysis can annotate an already existing graph path; stable pairs remain endpoint evidence only. | **Experimental substrate exists; integration is active and insert-qualified repeat-resolution evidence is not yet established** |
| FASTA and GFA | Stable `unitigs.fasta`/GFA and experimental child-scoped `segments.fasta`/GFA plus profile-only `contigs.fasta`. | **Implemented; multi-k formats remain experimental** |
| Per-contig evidence table | Stable `unitig_evidence.tsv`; experimental multi-k `segment_evidence.tsv`, adjacency/transition ledgers, and profile decisions. | **Implemented for graph segments under explicit narrowed semantics; they are not finished-genome contigs** |
| Self-contained HTML report | Stable and experimental transactional bundles each render an offline `report.html`. | **Implemented; multi-k report remains unqualified** |
| Deterministic parallel execution | Stable complete-bundle tests cover selected worker counts; the first multi-k integration intentionally accepts only one worker. | **Partial:** transient Linux test pass; full final-tree 1/2/4/8, multi-k parallelism, and cross-platform matrix not run |
| Comprehensive malformed-input behavior | Broad parser/gzip/pair/resource tests plus storage/bundle adversarial tests and fuzz targets. | **Partial:** transient 607-test checkpoint passed; final-tree sanitizer, sustained fuzz, and platform-complete campaigns are not run |

### Priority B

| Requirement | Decision/code/test evidence | Status |
|---|---|---|
| Coverage reconstruction | Exact placement aggregates are deliberately not called coverage or depth. | **Not implemented** |
| Circular-contig detection with explicit evidence | `closed_graph_walk` reports topology only; no molecular closing-junction audit. | **Not implemented beyond topology label** |
| Conservative bubble preservation | Stable graph performs no bubble collapse and keeps retained branches in GFA. | **Implemented** |
| Configurable consensus/diversity profiles | Experimental `diversity-preserving` emits every child segment; `exact-agreement-consensus` presents only byte/topology-identical groups observed at two or more distinct k values. | **Experimental implementation; the consensus label is a presentation rule, not base inference or phasing, and stable promotion is pending** |
| Disk-backed/partitioned operation for large contaminated data | Stable counting and experimental wide-k reduction are exact/external; an EC-1a partition-local/global-boundary topology seam exists, while child compaction remains whole-resident. | **Partial experimental foundation; no production-scale end-to-end external cDBG or demonstrated large-background envelope** |

### Priority C

| Requirement | Decision/code/test evidence | Status |
|---|---|---|
| Local haplotype reconstruction | Alternatives remain graph branches; no phasing model or haplotype output. | **Proposed** |
| Segmented-genome analysis | No grouping, completeness, constellation, or reassortment inference. | **Proposed** |
| Optional reference similarity/taxonomy | Deliberately outside the database-free core; no reference stage. | **Proposed** |
| Reference-relative variants | No stable SAM/BAM/PAF/VCF production or variant caller. | **Proposed** |
| Do not mislabel bubbles as global haplotypes | Stable documentation and output make no global haplotype claim. | **Implemented claim boundary** |

## Validation requirements

### Test and dataset matrix

| Original requirement | Implementation/evidence | Status |
|---|---|---|
| Unit tests | Module tests span stable code plus external reduction, authenticated retention/compaction/transitions, witnessed reconstruction, pair mapping/path analysis, multi-k integration, correction, and validation v4. | **Implemented; 537 library tests passed in the transient checkpoint. Later edits and final clean-ZIP rerun pending** |
| Integration and CLI tests | Stable CLI, multi-k binary, validation CLI, low-file, storage, and schema tests. | **Implemented; 70 non-library binary/integration/schema/property/example tests passed in the 607-test transient checkpoint. Active CLI/pair additions and final rerun pending** |
| DNA and graph property tests | DNA scanner properties plus independent exhaustive stable and experimental graph/transition oracles. | **Implemented; transient-tree pass; final rerun pending** |
| Fuzz FASTA/FASTQ/gzip/pairs | Targets and seed corpora exist. | **Present but NOT RUN for the v0.4 development tree:** historical 0.2.0-alpha.1 commit `cdb88007f2643776b092e8379b200dfb1404ac0c` passed a bounded ASan/libFuzzer smoke, which is not current-tree evidence |
| Fuzz private storage/output formats | Spool, count-run, bundle-manifest, and bundle targets exist. | **Present but NOT RUN for the v0.4 development tree:** sustained and platform-complete campaigns are absent; new experimental artifact readers are not covered by a completed sanitizer campaign |
| Golden outputs | Deterministic whole-bundle byte comparisons and graph vectors exist, but the full named golden inventory is not independently frozen. | **Partial** |
| Determinism across thread counts | Stable bundle tests passed in the transient checkpoint; a historical five-case generator-v3 smoke and dirty-tree 1 Mb probe also produced byte-identical selected-worker outputs. ADR 0015 invalidates affected historical metric interpretations, not byte comparisons. Experimental multi-k currently rejects thread counts other than one. | **Partial:** final clean-ZIP full 1/2/4/8, multi-k parallelism, and Linux/macOS equality not run |
| Truncated/corrupt gzip | CRC corruption, truncation, later-member, and trailing-junk cases. | **Implemented; transient-tree pass; final clean-ZIP rerun pending** |
| Missing/mismatched/reordered mates | CLI fatal-error tests cover identity, order, role, format, alias, and cardinality. | **Implemented; transient-tree pass; active experimental paired-CLI additions and final rerun pending** |
| Variable quality and ambiguity | Stable window accounting/parser fixtures plus experimental correction journals and generator-v4 independent error/quality streams exist. | **Software behavior passed the transient checkpoint; prospective scientific response/ablation matrix not run** |
| Repeats, circular genomes, uneven coverage | Development fixtures offer scenarios; qualification evaluator v4 exercises repeated exact placements, compatible-coordinate bounds, and circular truth, while the version-2 exact-flank junction method is retained. | **Partial; frozen scientific matrix not run** |
| Two-strain mixtures at multiple ratios | Generator v4 can emit one 90:10 two-component fixture, but the earlier generator-v3 mixture-recovery result is invalidated by ADR 0015. The planned divergence/ratio matrix, including 99:1, is unexecuted. | **Fixture implemented; scientific matrix not run** |
| Host-contaminated samples | Development-fixture scenario and preregistered high-background plan exist. | **Not admitted or scientifically run** |
| Low-input and high-depth samples | Frozen matrix defines them. | **Not run** |
| Truth-known simulations with recorded seeds | Development fixtures and generator v4 record a master seed, domain-separated layout/error/quality seeds, origins, substitutions, and non-default quality events. Independent replay and oracle kill tests are checked in. | **Experimental qualification slice implemented; full scientific generator/matrix not complete** |
| Public segmented RNA, non-segmented RNA, and DNA datasets with checksums | [DATASET_CATALOG.md](DATASET_CATALOG.md#original-three-class-public-dataset-requirement) contains no public-read entry for any required class. | **Absent and not run** |
| Compare Virustic2 and established tools | An unaudited historical Virustic2 summary is transcribed, but its raw 15-run artifacts are unavailable; SPAdes/metaSPAdes/MEGAHIT/SKESA were not run. | **Not satisfied; no retained comparator execution evidence** |

### Required metrics

| Metric | Current evidence | Status |
|---|---|---|
| Genome fraction/completeness | Evaluator/result v4 implements unique-coordinate and compatible-coordinate lower/upper exact-recovery bounds and reports NA when the approximate-placement universe is incomplete. No v4 scorecard is admitted. | **Experimental implementation only** |
| Base accuracy/consensus accuracy | Evaluator/result v4 reports bounded primary-alignment error diagnostics and consensus accuracy, with an exact-substring fast path. Primary alignment does not select recovery coordinates. | **Experimental only** |
| Misassemblies/false junctions | Evaluator/result v4 retains the version-2 exact-flank adjacency classifier; a general independent split-alignment/misassembly evaluator is not frozen. | **Partial** |
| Duplication | Evaluator/result v4 reports a ratio only for exact coordinate-unambiguous placements; exact ambiguity and incomplete non-exact placement universes are explicit NA. No ambiguity-aware scientific scorecard has run. | **Experimental/narrow** |
| Minor-path recall and precision | Defined in validation plan but not implemented in the current evaluator. | **Not implemented** |
| Runtime and peak RSS | A historical five-repetition-per-mode summary reports values, but its raw commands, timing records, bundles, and evaluator outputs are unavailable. | **Unaudited rerun target; no retained comparator measurement** |
| Output determinism | Historical whole-bundle byte equality on selected Linux cases. | **Partial platform/workload scope; final-tree rerun pending** |
| Publish failures and regressions | The generator-v2 strand defect, generator-v3 Q10/error coupling, evaluator-v2 order-biased recovery estimand, and failed release-verifier attempts are retained; the unavailable benchmark is labelled only as a historical performance concern. | **Implemented reporting practice for known failures; benchmark tie/regression unsubstantiated** |

## Engineering quality gates

| Gate | Evidence/status for the deliverable under review |
|---|---|
| `cargo fmt --check` | **Transient-tree pass** on Rust 1.98.1; subsequent edits supersede it. Final clean ZIP rerun required. |
| `cargo clippy` with warnings denied | **Transient-tree pass** for all targets/features on Rust 1.98.1; subsequent edits supersede it. MSRV and final clean ZIP reruns required. |
| `cargo test --all-targets` | **Transient-tree pass:** 607 passed, 0 failed on Rust 1.98.1, including 537 library tests. Subsequent edits supersede it; MSRV and final clean-ZIP reruns required. |
| Documentation build with warnings denied | **Current tree: NOT RUN after post-gate changes.** A superseded 0.3 candidate passed on both toolchains; final clean ZIP rerun required. |
| Release build | **Current tree: NOT RUN after post-gate changes.** A superseded 0.3 candidate passed on both toolchains; final clean ZIP rerun required. |
| Cargo package verification | **Current tree: NOT RUN after post-gate changes.** A superseded 0.3 candidate passed Cargo's built-in verification; final extracted `.crate` verification is not complete. |
| Dependency vulnerability audit | Historical root/fuzz lock graphs passed recorded `cargo-audit`; fresh scans against the current exact manifest/lock hashes were **NOT RUN**. |
| License/source policy audit | Historical root/fuzz `cargo-deny` runs passed; fresh runs against current hashes were **NOT RUN**, and fuzz-only `libfuzzer-sys` notice reconciliation is **blocked**. |
| Coverage-guided fuzzing | **NOT RUN for the v0.4 development tree.** `cargo +nightly-2026-08-18 fuzz check` compiled all seven targets with rustc 1.100.0-nightly and cargo-fuzz 0.13.2, but compilation is not a sanitizer campaign. A bounded ASan/libFuzzer smoke passed only for historical 0.2.0-alpha.1 commit `cdb88007f2643776b092e8379b200dfb1404ac0c`; current-candidate and sustained/platform-complete campaigns remain open. |
| Linux CI/execution | CI configured; the transient local x86-64 Linux fmt/check/Clippy/test checkpoint passed, but subsequent edits supersede it. **Full final tree pending**. |
| Apple Silicon macOS CI/execution | CI and exact commands configured. **Not run**. |
| MSRV and stable CI | Separate jobs/targets configured; earlier candidate tree passed both. **Final clean ZIP pending**. |
| Clean-extraction verification | Several verifier attempts failed closed and informed fixes. **No final post-audit PASS record**. |
| No unexpected runtime network access | Static review found no client/subprocess path. Dynamic network-denied observation **not run**. |
| No output clobber after failure | Ordinary existing-destination/failpoint/race tests passed in the transient checkpoint. Subsequent edits require rerun; exhaustive crash/disk/power matrix **partial**. |

## Deliverables

| Original deliverable | Location/evidence | Status |
|---|---|---|
| Complete source code | [`src/`](../src), schemas, tests, examples | **Complete for the stable fixed-k slice and an experimental read-witnessed multi-k portfolio; original production-grade mission incomplete** |
| README with compressed SE/PE examples | [README.md](../README.md#single-end-compressed-input) | **Present** |
| `RESEARCH.md` | [RESEARCH.md](../RESEARCH.md) | **Present; targeted review** |
| `ARCHITECTURE.md` and ADRs | [ARCHITECTURE.md](../ARCHITECTURE.md), [`adr/`](adr) | **Present; chronology qualification above** |
| `VALIDATION.md` | [VALIDATION.md](../VALIDATION.md) | **Present; most scientific matrix cells remain unexecuted** |
| `BENCHMARK.md` | [BENCHMARK.md](../BENCHMARK.md) | **Present; release protocol unexecuted and the sole historical comparison lacks its raw artifacts** |
| `CHANGELOG.md` | [CHANGELOG.md](../CHANGELOG.md) | **Present** |
| `SECURITY.md` | [SECURITY.md](../SECURITY.md) | **Present** |
| `CITATION.cff` | [CITATION.cff](../CITATION.cff) | **Present** |
| Automated CI and dependency maintenance | CI/fuzz workflows and Dependabot configuration | **Configured; configuration is not execution evidence** |
| Reproducible synthetic-data generator | Development generator and generator-v4/evaluator-v4 qualification binaries | **Present, bounded, truth-separated, and explicitly non-biological; full frozen matrix incomplete and no v4 scorecard admitted** |
| Small example data | [`examples/data/basic_paired`](../examples/data/basic_paired) | **Present** |
| Source ZIP excluding build artifacts | An interim byte-reproducible ZIP existed but is superseded by later edits. The verifier requires canonical inner ZIP/sidecar basenames; external uniqueness comes from a commit-bearing handoff directory or optional outer review bundle, not by renaming the ZIP. | **Final post-audit ZIP and handoff directory pending** |
| SHA-256 checksum | Interim checksum is disclosed only as failed/incomplete history. The final canonical sidecar must stay adjacent to the canonical ZIP, and the external verification report must bind the handoff directory, commit, ZIP digest, and verifier record. | **Final checksum pending** |
| Exact macOS verification commands | [VERIFICATION.md](VERIFICATION.md) | **Present; not executed** |
| Exact GitHub push commands | [VERIFICATION.md](VERIFICATION.md) | **Present; no push authorized or performed** |

## Completion assessment

VeritAsm is not complete against the original mission and is not a production-grade general
assembler. The stable result remains an evidence-rich fixed-k exact graph-segment slice. The v0.4
development tree additionally implements an experimental, serial, read-witnessed multi-k portfolio:
exact external counts feed authenticated retention, compacted children, original-read transition
ledgers, conservative witnessed reconstruction, explicit diversity/exact-agreement presentations,
and a transactional report bundle. Opaque pair-graph, exact pair-placement, and bounded existing-path
analysis substrates also exist, but their active portfolio integration is not a completed or
qualified repeat-resolution result.

The most consequential remaining algorithmic items are production-scale external compacted-graph
construction, qualified paired-end repeat resolution, coverage reconstruction, molecular circularity
evidence, and any haplotype/segment/reference stage. Quality correction exists only as a separate
unqualified ablation arm, and the exact-agreement “consensus” profile does not infer bases or phase
variants. The largest evidence gaps are final clean-extraction verification, Apple Silicon and MSRV
execution on the frozen tree, sustained fuzzing and LeakSanitizer coverage, public datasets,
established comparators, and the preregistered scientific matrix.

No evidence currently shows a general reconstruction improvement over Virustic2. An unaudited prior
summary reported tied sequence reconstruction and worse runtime/peak RSS, but the raw 15-run artifacts
are unavailable and cannot establish either finding. The supported improvements are narrower: stricter
input-validation contracts, exact checked/external counting semantics, richer evidence artifacts,
historical selected-worker bundle equality, and no-replace bundle design/test source. See
[FINAL_REPORT.md](FINAL_REPORT.md) for the permitted claim language.
