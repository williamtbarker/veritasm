# VeritAsm v0.4 development review report

> Historical development/release-review record. For the 0.4.0-dev.1 preservation checkpoint, see [current status](STATUS.md). Earlier results below retain their original snapshot scope.

- Review date: 2026-09-05
- Development identity: uncommitted development tree based on
  `c565568368e9`; Cargo metadata remains `0.3.0-alpha.1` until source freeze
- Candidate status: **unreleased, experimental research-software development tree; not production qualified**
- Report role: **development status ledger to be frozen into a later review archive; the external
  verification report must bind the commit-bearing handoff directory, canonical inner ZIP, SHA-256,
  transcript, and final gate results**
- Baseline: Virustic2 commit `b211915fc7cce82629766b77024463c6cabcc749`
- Scope: stable database-free fixed-k unitig reconstruction plus an experimental read-witnessed
  multi-k portfolio from short DNA reads

## Bottom line

VeritAsm contains an evidence-first stable fixed-k vertical slice and a materially larger experimental
v0.4 integration slice, but not the production-grade general assembler described in the original
mission. The new slice builds independent k children from one authenticated spool, retains exact
support decisions and original-read transition witnesses, and reports either every child segment or
only exact sequence/topology agreements across distinct k values. Its “consensus” mode is a
presentation rule: it neither infers bases nor phases variants. Source-backed pair-path and correction
substrates exist, but neither currently establishes qualified repeat resolution or improved
reconstruction.

An unaudited historical summary reported that VeritAsm and
Virustic2 produced the same single exact 5,996-base reconstruction from a 6,000-base truth and that
VeritAsm used more time and peak RSS. Its raw 15-run artifacts are unavailable, so this package cannot
establish either the reported tie or regression. No result currently supports a general
reconstruction-accuracy, completeness, sensitivity, resource-efficiency, or scientific-superiority
claim.

What is demonstrably richer is the software evidence interface: the stable path commits a checksummed FASTA,
GFA, per-unitig evidence table, transformation ledger, read/pair audit, machine record, and offline
HTML report as one no-replace bundle. The experimental portfolio adds authenticated retention and
transition ledgers, child-scoped sequence/graph/evidence rows, profile decisions, and its own
transactional manifest/report. A transient Linux checkpoint passed 607 tests, but later edits already
supersede that dirty-tree result. These are narrow implementation, engineering, and observability
advances—not evidence of better biological truth.

VeritAsm does not identify or detect organisms. It has no taxonomy database, classifier, validated
control model, analytical sensitivity study, or biological absence model. It must not be used to
claim presence or absence, viability, infectivity, sterility, product safety, or product disposition.
Use in adventitious-agent research is limited to an investigational sequence-reconstruction component
whose outputs require appropriate controls and independent confirmation. See
[the application boundary](ADVENTITIOUS_AGENT_APPLICATION.md) and
[scientific limitations](SCIENTIFIC_LIMITATIONS.md).

## 1. What is implemented and tested

### Implemented stable vertical slice

- Streaming plain or content-detected gzip FASTA/FASTQ ingestion, including concatenated gzip
  members, with bounded physical-line and record sizes.
- Single-end input and strictly synchronized paired-end input across ordered multiple lanes. Pair
  identifiers, declared mate roles, cardinality, order, formats, and physical-source aliases are
  validated.
- Explicit base-quality rejection and IUPAC ambiguity-window exclusion with mutually exclusive
  accounting.
- Exact rolling canonical k-mer extraction for one selected `k`, `3 <= k <= 63`.
- Checked `u64` occurrence or supplied-fragment-instance support, including deduplication across both
  mates of one supplied fragment.
- Exact partitioned, sorted-run counting with bounded fan-in merge and configured memory, temporary
  storage, retained-key, and open-file admission checks.
- An exact oriented de Bruijn graph and deterministic compaction into conservative graph segments
  that are maximal only under the declared degree and fixed-point boundary model; unresolved branches
  and adjacencies across self-complemental-edge boundaries remain explicit in GFA. Assembly contract
  v0.1 performs no tip or bubble deletion beyond the declared absolute support threshold.
- Conservative pair evidence from exact placements. Pair rows retain the immutable input-lane
  ordinal, never pool matching endpoint tuples across lanes, and describe endpoint co-observations
  only; they never create a graph edge, scaffold, gap estimate, or sequence join.
- Optional fixed-q15 indexed exact zero-mismatch remapping of construction reads to emitted linear
  unitigs, with complete-candidate verification, an exhaustive differential oracle, instance-derived
  mapper provenance, and explicit ineligible, unmapped, multi-placement, and candidate-limit states.
- Deterministic FASTA, GFA 1.0 `H`/`S`/`L`, per-unitig TSV, pair TSVs, transformation TSV, `run.json`,
  self-contained HTML, bundled schemas, and SHA-256 manifest.
- A public manifest-only integrity verifier that rejects malformed, duplicated, missing, modified,
  unlisted, unsafe, or symbolic-link entries under explicit resource limits. Its default bounds are a
  1 MiB manifest, 4,096 regular files including the manifest, 1,024 directories, 32 relative path
  components, and 16 GiB of hashed artifact bytes. This verifies checksums and inventory, not artifact
  schemas or biological correctness.
- Same-filesystem staged output followed by a no-replace commit. An existing destination is not an
  overwrite target.
- A deterministic development-fixture generator and a narrow exact-substring truth evaluator.
- A separate experimental validation-tooling slice with 256-bit deterministic generation, read
  origin/substitution ledgers, truth-separated inputs, circular-aware fitting alignment, explicit
  empty-assembly semantics, exact-flank adjacency metrics, descriptor-anchored artifact reads,
  integer-defined QV, and executable cross-field result validation. This is qualification
  infrastructure, not an admitted benchmark result.
- First-party Rust targets forbid unsafe code (`#![forbid(unsafe_code)]`); dependencies are separately
  reviewed rather than covered by that lint. The package records a Rust 1.85 MSRV, locked
  dependencies, Linux/macOS CI definitions, dependency-update configuration, and same-tool
  deterministic source-package tooling.

The exact semantics and limitations are normative in the
[architecture](../ARCHITECTURE.md), [configuration contract](CONFIGURATION.md), and
[output schema](OUTPUT_SCHEMA.md).

### Implemented experimental v0.4 slice

These capabilities exist in the development source and were included in the transient test
checkpoint below. They remain outside the stable `veritasm assemble` contract and are not
scientifically or operationally qualified:

- exact wide-key external spill/reduction from the immutable spool, followed by an opaque
  source-bound retention transformation with exact retained/discarded key and support conservation;
- whole-resident exact compacted-graph children, exact original-read `(k+1)` transition ledgers, and
  witnessed reconstruction that excludes unsupported topology candidates without converting a child
  contig into read evidence;
- opaque evidence capabilities from registered spool through raw counts, retention, compaction,
  transition witness, and reconstructed child, preventing public callers from recreating
  source-backed claims by editing rows and recomputing unkeyed hashes;
- the serial `veritasm-multik` executable, which independently reconstructs a strictly increasing
  k-list from one spool and transactionally emits child-scoped FASTA, GFA, evidence/decision tables,
  JSON, an offline HTML report, schemas, and a checksum manifest;
- explicit experimental `diversity-preserving` and `exact-agreement-consensus` output profiles. The
  latter admits only byte-identical sequence and topology observed in at least two distinct k
  children; every excluded singleton remains in child outputs, and no base vote, path splice, or
  haplotype inference occurs;
- source-backed pair-graph adaptation, exact linear-unitig mate-placement production, and bounded
  lane-specific analysis of already existing graph paths. These substrates cannot add an edge,
  synthesize a gap, scaffold sequence, or phase a strain. Portfolio integration and its CLI tests are
  active work at this report checkpoint, not a completed Priority-A result;
- an isolated journaled quality-correction experiment with `RawOnly`, diversity-preserving, and
  explicitly consensus-experimental behavior. It is disabled in the portfolio and has no prospective
  accuracy ablation;
- an experimental external-cDBG topology seam for partition-local records and global boundary
  reconciliation. It is not the complete external compaction/stitching engine and establishes no
  production-scale envelope; and
- generator-v4/evaluator-v4 qualification tooling with domain-separated layout/error/quality streams,
  explicit historical QC-censoring control, truth-separated inputs, semantic ledger replay,
  external dataset-content-root binding, and ambiguity-aware exact-recovery bounds. No v4 dataset or
  scorecard is admitted.

The source contracts and limitations are in
[the multi-k portfolio contract](EXPERIMENTAL_AUTHENTICATED_MULTIK.md),
[the retention contract](EXPERIMENTAL_RETENTION.md),
[the transition-witness contract](EXPERIMENTAL_TRANSITION_WITNESS.md),
[the authenticated pair-graph contract](EXPERIMENTAL_AUTHENTICATED_PAIR_GRAPH.md), and ADRs
[0023](adr/0023-read-witnessed-multik-reconstruction.md)--[0027](adr/0027-opaque-source-backed-evidence-capabilities.md).

### Transient v0.4 Linux engineering checkpoint

On 2026-09-05, the uncommitted development tree based on `c565568368e9` passed these commands
with Rust `1.98.1`:

```text
cargo +stable fmt --all -- --check
CARGO_TARGET_DIR=/tmp/veritasm-root-gate2 cargo +stable check --locked --all-targets --all-features
CARGO_TARGET_DIR=/tmp/veritasm-root-gate2 cargo +stable clippy --locked --all-targets --all-features -- -D warnings
CARGO_TARGET_DIR=/tmp/veritasm-root-gate2 cargo +stable test --locked --all-targets --all-features --no-fail-fast
```

The test command passed 607 tests and failed none: library 537; stable main binary 4;
experimental multi-k binary 3; stable CLI 25; low-file-descriptor counting 1; graph properties 4;
stable schemas 5; storage integrity 2; validation CLI 22; validation schemas 2; and examples 2.
The evaluator, simulator, and two other example targets each discovered zero tests.

Active pair-path/portfolio integration, CLI-test, benchmark, packaging, and audit edits began after
this checkpoint, so the result is already superseded and is not a pass for the present working tree.
It is not MSRV, macOS, rustdoc, release-build, package, fuzz, clean-extraction, or scientific-
comparison evidence. The frozen source must rerun every required gate.

### Post-checkpoint work in progress

| Workstream | Current status |
|---|---|
| Pair-to-multi-k integration | Active; no frozen integrated output or final post-integration gate yet. |
| Multi-k CLI integration | Seven cases passed stable and Rust 1.85; after a live lease-contention case was added, eight passed stable. The eight-case MSRV/full rerun still awaits pair integration. |
| Development benchmark | Frozen draft matrix prepared; execution is **NOT RUN**. It will remain `development_unbound`, not scientific qualification. |
| Fuzz gate | Dirty-tree `cargo +nightly-2026-08-18 fuzz check` compiled all seven targets; no current sanitizer execution ran, so fuzz qualification remains **NOT RUN**. |
| Release package | No v0.4 archive, checksum, package verification, or clean-extraction record exists; metadata/inventory reconciliation is active. |

The interim CLI record used x86-64 Linux, stable Rust 1.98.1 and MSRV Rust 1.85.0 with
`CARGO_TARGET_DIR=/tmp/veritasm-root-coherence` for Cargo:

```text
cargo +stable test --locked --test multik_cli -- --nocapture       # 8 passed after lease-race addition
cargo +1.85.0 test --locked --test multik_cli -- --nocapture       # 7 passed
cargo +stable clippy --locked --test multik_cli -- -D warnings     # passed
rustfmt +stable --edition 2021 --check tests/multik_cli.rs          # passed
```

The eight-case stable result includes a live two-process lease race. The last MSRV, focused Clippy,
and rustfmt records predate that eighth case, and all commands predate pair integration. They do not
amend the 607-test checkpoint, qualify the combined tree, or replace the final full-gate rerun.

### Historical superseded candidate-tree engineering gates on Linux

An earlier `0.3.0-alpha.1` candidate tree recorded passes with then-current stable Rust 1.98.1 and MSRV
Rust 1.85.0 for the following source-level gates. These are historical candidate-tree checks; they do
not qualify the subsequently changed current tree, final clean ZIP, or extracted crate:

```text
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo test --locked --doc
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
cargo build --locked --release
```

Each historical all-target run passed 305 tests. Other binaries and examples with no tests do not add
evidence; the documentation test target contained no doctests.

`cargo package --locked --allow-dirty` and Cargo's built-in package verification also passed under
both toolchains on that superseded candidate tree. Exact package inventories and sizes remain in the
retained command logs. An interim post-isolation source ZIP was subsequently built twice with
byte-identical output and passed ZIP/checksum/inventory preflight, but Rust was unavailable in that
execution environment, so the archive was not compiled or tested from extraction. Subsequent source
changes supersede both that gate and the interim archive. The full MSRV/stable gate for the current
tree is **NOT RUN**. A final post-audit ZIP and independent MSRV/stable verification of both its source
tree and produced `.crate` remain pending. [VALIDATION.md](../VALIDATION.md) tracks that distinction.

The historical candidate exercised content-selected gzip, malformed and corrupt input, synchronized
multi-lane pairs, reordered/missing/swapped mates, physical aliases and source mutation, IUPAC and
quality accounting, exact-count oracles, graph and compaction invariants, candidate-limited mapping,
deterministic bundles across worker counts, manifest reconciliation, and failure without modification
of an existing destination. Relevant historical executable evidence is in [the CLI tests](../tests/cli.rs)
and [the independent graph oracle](../tests/graph_properties.rs). A corrected
[low-descriptor fan-in regression](../tests/count_open_files.rs) was not established by the superseded
305-test candidate but passed as a one-test integration target in the transient 607-test checkpoint;
later edits and the final archive still require rerun.

The root and fuzz dependency graphs have historical `cargo-audit` 0.22.2 and frozen `cargo-deny`
0.20.2 pass records against the older four manifest/lockfile hashes and RustSec snapshot recorded in
[DEPENDENCY_REVIEW.md](DEPENDENCY_REVIEW.md). The current exact hashes are separately listed there;
their third-party resolution is unchanged, but fresh `cargo-audit` and `cargo-deny` runs against
those current hashes were **NOT RUN**. This does not clear the separate fuzz-redistribution issue in
section 3, and the bounded manual source review is not a dependency-soundness proof.

### Dirty-tree 1 Mb engineering probe

One binary-hash-bound development probe used generated dataset
`v3-linear-pe-r7-aa0465c17b74825b`: a 1,000,000-base linear truth with 80,000 perfect PE150
fragments, 350-base inserts, `k=31`, minimum fragment-instance support 2, and a 2 GiB assembly memory
budget. Four output trees—two at one thread and two at eight threads—were byte-identical. The
one-thread evaluated result contained one error-free 999,949-base linear unitig, covered 999,949 of
1,000,000 truth bases, and had zero false among 999,920 eligible exact-flank adjacencies. It omitted
51 terminal truth bases.

One instrumented execution per configuration measured 29.172 seconds and 238,528 KiB peak RSS at
one thread, versus 32.155 seconds and 240,888 KiB at eight threads. The eight-thread run was slower;
this fixture provides no positive parallel-scaling result. The evaluator took 2.808 seconds and
130,464 KiB peak RSS. The measured assembler executable SHA-256 was
`30188cf84fdf47dda490ede8c4dfa8d573275eee1e2996af6aff9d7aea8cfa7e`; the external probe record has
SHA-256 `573a52a06afd77f79486c1a4525b0e5a1ac84dcefa93c87d1b6cc857fdcd8c82`.

The measured executable came from a dirty tree, there was one timed run per setting, and no
established comparator ran. This is a narrow engineering observation tied to those binary and input
hashes—not release-scorecard evidence, a packaged-binary result, a stable performance estimate, a
sensitivity result, or evidence of assembler superiority or adventitious-agent detection.

## 2. What is experimental

Every v0.4 capability listed under “Implemented experimental v0.4 slice” is experimental. Its
placement in section 1 records that bounded source and tests existed at one checkpoint; it does not
promote the capability into the stable assembler, qualify its scientific behavior, or close the
release blockers in sections 3 and 4.

### Exactness-preserving two-hit Bloom prototype

[`src/bloom.rs`](../src/bloom.rs) implements a deterministic, insertion-only two-filter prototype.
For supplied-fragment support with a retention threshold of at least two, it can nominate a superset
of recurrent keys for a second complete, full-key exact recount. A Bloom false positive may add exact
work; a Bloom result is never emitted as a count, graph edge, sequence call, classifier result, or
negative result. Default prototype allocation is capped at 64 MiB and explicit larger budgets require
an opt-in constructor.

Unit and exhaustive/collision test source covers its containment contract and allocation checks. Those
tests passed in the transient 607-test v0.4 checkpoint, but subsequent edits and the final clean
archive have not rerun them. The prototype is not reachable from the stable assembly CLI, has not been
admitted to a scientific or performance benchmark, and cannot support a claim that it saves time,
memory, or disk.
Its acceptance and rejection conditions are in
[ADR 0004](adr/0004-exactness-preserving-probabilistic-acceleration.md).

### Four-limb wide-k scanner

[`src/experimental/wide_kmer.rs`](../src/experimental/wide_kmer.rs) implements exact two-bit packed
DNA values and rolling canonical scans through k=127. Checked-in boundary, property, and narrow-path
differential tests cover word crossings and equivalence with the stable `u128` scanner through k=63.
The module does not change the stable k=3..63 CLI, spool, count-run, graph, digest, or output-schema
encodings. It is a representation and scanner substrate, not a wide-k assembler. Its boundary is
recorded in [ADR 0006](adr/0006-experimental-wide-packed-kmers.md).

### Exact minimizer-partition correctness foundation

[`src/experimental/partitioned_dbg.rs`](../src/experimental/partitioned_dbg.rs) implements a serial,
in-memory oracle for strand-invariant exact minimizer ownership, super-k-mer window conservation,
virtual bucket routing, and full-key edge counting through k=127. Full 256-bit minimizers and k-mers
remain in every count row, so a bucket collision cannot merge graph identity. Differential,
reverse-complement, cross-word-boundary, collision, resource, and Rayon-pool-independence tests are
checked in. It does not provide an authenticated spill format, parallel bucket workers, direct cDBG
compaction, fragment-support reduction, or stable-pipeline integration, and establishes no assembly
speed or accuracy result. See
[ADR 0014](adr/0014-experimental-exact-minimizer-partition-foundation.md).

### Paired-end library model

[`src/library_model.rs`](../src/library_model.rs) consumes caller-asserted complete exact unique mate
placements and reports lane-specific FR/RF/FF/RR counts, integer empirical span quantiles, exclusions,
and threshold-based availability. It does not map reads, prove that an upstream placement is unique,
infer a gap or adjacency, modify the graph, or join sequence. It is disconnected from stable output and
described in [the experimental library-model contract](EXPERIMENTAL_LIBRARY_MODEL.md) and
[ADR 0008](adr/0008-experimental-paired-library-model.md).

### Development-only truth evaluator and fixtures

[`examples/evaluate_exact_truth.rs`](../examples/evaluate_exact_truth.rs) checks exact substring
recovery on perfect linear development fixtures. It is useful for regression detection but does not
measure substitutions, indels, repeat-aware alignment, false junction classes, strain mixtures, or
real-read behavior. The checked and generated synthetic inputs are development fixtures, not
scientifically admitted simulations.

The newer `veritasm-simulate` and `veritasm-evaluate` binaries extend qualification to six generated
case classes, independently seeded base-error and quality processes, an explicit historical
QC-censoring control, circular rotations, compatible-coordinate exact-recovery bounds, and
output-adjacency evidence. They remain experimental:
the generator does not model a platform, the direct evaluator is limited to small truth-known cases,
the complete pre-registered matrix has not been generated, and no resulting dataset or scorecard has
been admitted. Exact behavior is in
[`docs/VALIDATION_TOOLING.md`](VALIDATION_TOOLING.md).

The historical generator-v3 smoke used five cases with 300 fragments, 600-base truth, 75-base reads,
gzip input, `k=31`, and support 2. Complete 1-thread and 4-thread bundles matched byte for byte and 20
checksum manifests verified. ADR 0015 later invalidated its error-case, mixture-recovery, selected-
target recovery, and duplication interpretations: substitutions were deterministically Q10 and the
evaluator selected the first compatible truth target. Its retained table in
[VALIDATION.md](../VALIDATION.md) is debugging history, not sensitivity, recovery, comparator, or
general accuracy evidence.

### Closed graph walks

Stable output can label a topology-preserving `closed_graph_walk`. That label is graph evidence only,
not evidence that the originating molecule was circular. Molecular circularity detection remains
unimplemented.

## 3. What failed or regressed

### Unaudited historical comparison — raw artifacts unavailable

A prior development report described 5,000 perfect paired fragments (100-base reads, 250-base insert),
a 6,000-base truth, seed `8675309`, four threads, one warm-up, and five interleaved repetitions per
mode on one Linux host. It reported the values below:

| Mode | Reported median wall time | Reported median peak RSS | Reported exact recovery |
|---|---:|---:|---:|
| Virustic2 | 0.10 s | 10,136 KiB | 5,996/6,000 bases |
| VeritAsm, remapping disabled | 0.34 s | 34,068 KiB | 5,996/6,000 bases |
| VeritAsm, remapping enabled | 0.92 s | 34,352 KiB | 5,996/6,000 bases |

The raw 15-run commands, stdout/stderr, timing records, bundles, and evaluator outputs are unavailable
in this handoff. [BENCHMARK.md](../BENCHMARK.md) therefore retains the transcribed values only as an
unaudited performance-risk lead. This package cannot independently establish the reported sequence
tie, runtime/RSS regression, equivalence, or any general accuracy/resource trend; the comparison must
be rerun under the frozen protocol before any such claim is admitted.

### Indexed-audit low-memory compatibility risk

The promoted q=15 exact mapper stores every q-gram posting for every emitted linear unitig and has no
runtime exhaustive or partitioned fallback when that index exceeds the audit-persistent one-eighth
memory share. A checked unit regression deliberately shows a 200,000-base repetitive target refused
under a 32 MiB total budget while remap-disabled construction remains admissible. This is correct
fail-closed accounting, but it can make a remap-enabled run fail where the former slow exhaustive
auditor did not require target-length-proportional persistent storage. No representative end-to-end
benchmark establishes whether the trade is favorable; it remains a disclosed operational regression
risk, and disabling remapping forfeits placement and pair evidence.

### Experimental snapshot pathname-substitution risk

The production-threat review found a medium-severity hardening gap in the experimental multi-k
snapshot loader at the inspected checkpoint: `ChildSnapshotRef::load_verified` performed its digest,
parse, and canonical-byte comparison through separate pathname opens without binding all operations to
one descriptor identity. Under the documented trusted-parent model this is not a remote-input defect,
but a same-UID writer with access to the private work tree could substitute coherent A/B files between
checks. Until a descriptor-bound implementation and adversarial regression pass, the experimental
snapshot cannot support a hostile-local-filesystem integrity claim. The broader stable and
experimental transaction contracts also assume a trusted output parent; they are not claimed as a
security boundary against a same-UID adversary.

The same review found an aggregate-memory-accounting blocker: during child snapshot creation and
verification, the original report can coexist with a fully decoded verification copy while the
portfolio projection admits only one report cap. The component caps still fail closed at their local
boundaries, but the aggregate coexistence proof is incomplete. Until the writer avoids that duplicate
live representation or accounts for both copies, and exact limit-boundary tests pass, the portfolio
must not be described as having a closed aggregate-memory envelope or production-bounded memory.

### Validation and release gaps

- **Release-verifier attempts failed closed during hardening.** One pre-final archive's smoke
  comparison reused source-package path policy and therefore rejected the required generated
  `manifest.sha256`; the four retained smoke bundles themselves matched by committed path and byte.
  That same attempt exposed a cleanup-reporting gap when build-state paths were visible after a
  reported cleanup success. The verifier now has separate generated-output path policy, persistent
  ownership, direct post-cleanup filesystem attestation, exact stable bundle inventory, and focused
  injected-failure tests. A later archive passed MSRV and all stable source/check/test/doc gates but
  encountered a stable linker `undefined symbol: main` only on the extracted crate's final release
  build; rebuilding that exact extracted crate in a fresh target succeeded and produced a runnable
  binary. A subsequent archive passed every stable gate, but its Cargo 1.85 source check/test target
  left the top-level debug binary at mode `0600`, so all 16 CLI tests failed with
  `PermissionDenied`; the exact extraction then passed those tests in a fresh test-only target with
  an executable mode `0755` binary. The verifier now gives source and crate check/Clippy, test/doc,
  and release/package phases distinct fresh targets rather than retrying a failed gate. None of
  these failed attempts is release evidence; the post-change clean archive still requires the
  external verifier record.

- **Generator-v2 qualification smoke: failed and invalidated.** Minus-strand reads traversed truth
  coordinates backward without complementing their bases. Across the five bounded smoke cases, all
  33 rejected contigs (3,735 bases) were exact reverse- or complement-only artifacts rather than
  graph mosaics. The assembler correctly represented those artificial input components. Generator
  version 3 fixes the orientation, changes dataset/read namespaces, and adds independent sequence,
  error-ledger, graph-orbit, schema, tamper, and paired assemble/evaluate regressions. Historical
  post-fix stable/MSRV source gates passed on the identified 2026-09-03 snapshot, and all five
  regenerated smoke cases had zero unaligned bases and zero false junctions. ADR 0015 subsequently
  invalidated the affected generator-v3 error, mixture-recovery, selected-target recovery, and
  duplication interpretations; generator v4/evaluator v4 are the versioned correction. Evaluator v4
  additionally requires an external canonical dataset content root for qualification eligibility. The original
  v2 run and affected v3 estimands remain disclosed validation failures, not evidence; the final
  edited archive still needs rerun.

- **Apple Silicon macOS: NOT RUN.** CI and exact verification commands exist, but configuration is
  intent rather than evidence from a completed macOS run.
- **Public biological datasets: NOT RUN.** No checksum-frozen segmented RNA, non-segmented RNA, or DNA
  public dataset has been admitted and evaluated.
- **Established assemblers: NOT RUN.** SPAdes/metaSPAdes, MEGAHIT, SKESA, and other proposed
  comparators have not been frozen and executed. The only ancestor-comparison record is the
  unaudited historical Virustic2 summary above; its raw artifacts are unavailable.
- **Final source ZIP and independently extracted Cargo crate: NOT RUN.** Cargo packaging passed for a
  superseded 0.3 candidate under both required Rust toolchains. An interim source ZIP was produced
  twice with identical SHA-256 `fd5abdab943d36706d8c29ad01f47ba57efd7a72d583efce9d8da3451b198779`
  and passed archive preflight, but Rust was unavailable for clean-extraction compilation and later
  audit edits superseded those bytes. It is not an approved package. The final archive/checksum and
  successful external clean-extraction verifier record still do not exist. The verifier requires the canonical
  basename `veritasm-0.3.0-alpha.1-source.zip`, and its sidecar records that canonical name. Final
  human delivery places the unrenamed pair inside a commit-bearing handoff directory (optionally
  wrapped by a uniquely named outer review bundle); the external verification report binds that
  directory, full commit, ZIP digest, and transcript.
- **Dynamic runtime network isolation: NOT RUN.** Static source inspection found no product network
  client path. A linked-library inspection of an earlier candidate binary found only ordinary local
  runtime libraries; that prior-snapshot result was not rerun on the current candidate working tree.
  `ptrace` tracing and network-namespace isolation were blocked by the execution environment, so the
  stronger syscall-observation gate remains unsatisfied.
- **Coverage-guided fuzz qualification: NOT RUN for the v0.4 development tree.** With rustc
  1.100.0-nightly (`nightly-2026-08-18`) and cargo-fuzz 0.13.2, the exact dirty-tree command
  `cargo +nightly-2026-08-18 fuzz check` compiled all seven targets and exited zero. No current
  ASan/libFuzzer or LeakSanitizer execution ran; harness compilation is not a fuzz campaign. A pinned
  10-second-per-target ASan/libFuzzer smoke with leak detection disabled passed
  only for historical 0.2.0-alpha.1 source commit
  `cdb88007f2643776b092e8379b200dfb1404ac0c`. That bounded prior result does not qualify this tree,
  is not a sustained or platform-complete campaign, and supports no general parser-robustness claim.
- **Fuzz dependency redistribution: blocked.** `libfuzzer-sys 0.4.13` metadata requires
  `(MIT OR Apache-2.0) AND NCSA`, while bundled source notices and the crate's included license texts
  do not provide a reconciled NCSA/LLVM-exception notice set. Fuzz binaries or fuzz dependency sources
  must not be redistributed until that discrepancy receives maintainer or qualified legal review.
- **Crash durability: not fully established.** Existing-destination and ordinary failure paths are
  tested, but sudden power loss, kill-at-every-syscall, filesystem exhaustion at every commit step,
  and every supported filesystem have not been exhaustively tested.

No negative-result, organism-detection, analytical-sensitivity, clinical, regulatory, or product-
disposition gate was attempted; such claims are outside this package's contract rather than failed
assembler endpoints.

## 4. What remains proposed

The following promotion steps and algorithms remain proposed. The corresponding substrate may exist,
but none is a stable, scientifically qualified feature merely because experimental source exists:

- promote the independent read-witnessed multi-k portfolio only after frozen scientific ablations;
  cross-k path splicing, voting, or best-k selection remains deliberately unimplemented;
- qualify any journaled read correction against a retained raw arm; quality-weighted likelihood,
  insertion/deletion correction, and pair-aware correction remain unimplemented;
- promote the diversity/exact-agreement presentation profiles only with stable schemas and tests;
  biological consensus inference and cross-variant phasing remain unimplemented;
- conservative bubble classification with read-backed alternate-path evidence beyond exact local
  transitions;
- complete the active pair-path portfolio integration, then validate insert/orientation-calibrated
  repeat resolution. Evidence-qualified scaffolding would require a separate decision and must never
  fabricate sequence;
- complete partition-local unitig construction and global stitching for graph-wide disk-backed
  operation, then measure a reproducible envelope on very large or heavily contaminated inputs;
- coverage reconstruction and independently audited read-to-graph support;
- molecular circularity evidence across closing junctions;
- local haplotype reconstruction with explicit phase limits; global haplotypes are not promised;
- segmented-genome grouping or analysis;
- optional, separately versioned reference similarity, taxonomy, and reference-relative variation;
- a benchmark-qualified, cache-efficient exact-residual syncmer/Bloom/Count-Min scheduling scout; and
- the pre-registered truth-known mixture, repeat, uneven-depth, host-background, low-input, high-depth,
  circular, public-data, and comparator validation matrix.

The evidence required before any of these can move from proposal to supported feature is defined in
[VALIDATION.md](../VALIDATION.md) and [BENCHMARK.md](../BENCHMARK.md). The research basis and explicit
non-goals are in [RESEARCH.md](../RESEARCH.md).

## 5. Is VeritAsm demonstrably better than Virustic2?

**Not as a general assembler and not in demonstrated biological reconstruction accuracy.** The sole
ancestor-comparison summary reports an exact sequence tie and worse VeritAsm resource use, but its raw
15-run artifacts are unavailable and neither result is independently established. Real data,
error-rich simulations, repeat challenges, mixtures, high-background samples, and established current
assemblers have not been compared.

Blanket/drop-in Virustic2 parity is also unmet. VeritAsm preserves substantial FASTX data-input
behavior but changes the executable/library name, long flags, stdout/JSON/report interfaces, output
schema and defaults, exit codes, Rust API, and Windows support without a compatibility wrapper. See
[the migration table](BASELINE_AUDIT.md#non-parity-and-migration-contract).

VeritAsm is demonstrably richer only in several bounded software dimensions:

| Dimension | Finding | Permitted conclusion |
|---|---|---|
| Sequence reconstruction in the unaudited historical summary | Reported tie: one exact 5,996-base substring from a 6,000-base truth; raw outputs/evaluator records unavailable | No retained accuracy comparison; rerun required |
| Runtime and peak RSS in the unaudited historical summary | Reported worse VeritAsm values; raw timing records unavailable | Performance-risk lead only, not established regression evidence |
| Evidence communication | Stable VeritAsm emits and internally reconciles graph, unitig, transform, mapping, pair, run, report, schema, and checksum artifacts; the experimental portfolio adds source-bound retention, transition, reconstruction, cross-k-profile, and ancestry roots | The interface is richer and more auditable under its documented semantics; content roots prove local integrity/derivation constraints, not biological truth |
| Pair/input integrity | VeritAsm rejects mate-role, ordering, cardinality, format, and physical-alias cases that the baseline audit identified as missing or defective | Stricter validation contract plus historical and transient-tree test evidence; final-tree rerun pending |
| Output transaction | VeritAsm implements no replacement of an existing destination and commits its evidence set as one bundle; ordinary tests passed in the transient v0.4 checkpoint, while the baseline audit reproduced FASTA replacement before a later report failure | Improved no-replace design plus bounded test evidence; final-tree rerun pending and no universal crash durability |
| Counting arithmetic and resource handling | VeritAsm uses checked `u64` counts and exact spill/merge paths; the transient checkpoint passed the corrected low-descriptor fan-in regression, while Virustic2 used resident `u32` saturation | Improved arithmetic failure semantics for tested paths; no general memory, scale, or speed advantage established |
| Multi-k and read-transition evidence | The experimental command constructs independent children and removes unsupported topology adjacencies using exact original-read transition replay; the transient checkpoint included its unit/binary tests | New falsifiable evidence interface only; no retained Virustic2/comparator reconstruction result and no accuracy/sensitivity conclusion |
| Determinism | Historical Linux CLI/qualification records and transient-tree tests cover complete-bundle equality under selected execution plans; the first multi-k command is intentionally serial | Stronger artifact-level design and bounded evidence; final tree, full worker matrix, and macOS reruns pending |
| Adventitious-agent use | No detector, taxonomy, control model, or assay validation exists | No advantage or detection claim is permitted |

The detailed ancestor findings are in [BASELINE_AUDIT.md](BASELINE_AUDIT.md). This comparison must not
be generalized beyond the exact tested cases.

## 6. Evidence supporting each improvement claim

| Improvement claim | Direct evidence | Boundary of evidence |
|---|---|---|
| Richer stable evidence output | The implemented artifact inventory and reconciliation rules in [OUTPUT_SCHEMA.md](OUTPUT_SCHEMA.md); complete-bundle and manifest tests in [the CLI tests](../tests/cli.rs), included in the transient 607-test pass; Virustic2 artifact behavior in [BASELINE_AUDIT.md](BASELINE_AUDIT.md) | Supports implemented structure and bounded internal-consistency evidence, not the post-edit/final tree, biological correctness, or independent validation |
| Stricter paired-input validation | CLI cases for valid multi-lane plain/gzip pairs and failures for reordered, missing, role-swapped, format-mismatched, and aliased mates; corresponding baseline defects in [BASELINE_AUDIT.md](BASELINE_AUDIT.md) | The transient checkpoint passed the cases; later edits require rerun, and the cases do not cover every sequencer header dialect |
| Safer destination behavior | `an_existing_destination_is_never_modified` and malformed-input no-partial-output cases in [the CLI tests](../tests/cli.rs); baseline replacement failure in [BASELINE_AUDIT.md](BASELINE_AUDIT.md); transaction design in [ADR 0005](adr/0005-transactional-deterministic-bundle.md) | The transient checkpoint exercised ordinary failure paths; later edits require rerun, and this is not a power-loss guarantee on every filesystem |
| Checked exact counting and low-FD merge | Exact in-memory oracle, overflow/resource, corruption, partition, multi-pass fan-in, and layout-independence tests in [`src/count.rs`](../src/count.rs), plus the corrected [low-descriptor integration regression](../tests/count_open_files.rs) | The low-FD regression was the one-test passing integration target in the transient checkpoint. No lower peak memory or faster execution is established |
| Conservative compacted graph | Exhaustive small-universe/generated subgraph comparisons in [the independent graph oracle](../tests/graph_properties.rs), stable unit tests, and experimental full-key compaction/external-topology oracles | Establishes encoded graph invariants for tested/oracle domains; not genome reconstruction accuracy or production-scale external compaction |
| Authenticated retention and transition-constrained reconstruction | Opaque-capability construction/mutation tests in [`retention.rs`](../src/experimental/retention.rs), [`transition_witness.rs`](../src/experimental/transition_witness.rs), and [`evidence_reconstruction.rs`](../src/experimental/evidence_reconstruction.rs), all included in the transient 537-library-test pass | Establishes tested source-binding, conservation, and no-unsupported-adjacency invariants at that checkpoint; not biological truth, global phase, or a final-tree pass |
| Explicit multi-k diversity/exact-agreement choice | [`multik_pipeline.rs`](../src/experimental/multik_pipeline.rs), [`multik_bundle.rs`](../src/experimental/multik_bundle.rs), and three passing binary tests in the transient checkpoint | Implements an experimental presentation choice without cross-k splicing; no scientific result shows either profile improves recovery |
| Deterministic complete artifacts | Stable complete-bundle tests in the transient checkpoint and the historical generator-v3 1/4-thread byte comparison in [VALIDATION.md](../VALIDATION.md) | Historical metric interpretations are invalidated under ADR 0015, but byte comparisons remain engineering evidence. Selected Linux cases only; final-tree and macOS identity are not run. Multi-k currently has no parallel execution |
| More explicit quality and ambiguity evidence | Stable DNA/CLI accounting plus the experimental journaled correction module and generator-v4 independent error/quality streams, included in the transient library/validation tests | Shows deterministic rules and replay for tested inputs, not that a threshold or correction improves scientific accuracy |
| Conservative pair reporting and path substrate | Stable placement/endpoint/lane tests plus experimental authenticated adapter, mapper, and existing-path analysis tests in the transient library pass | Stable rows remain co-observations; experimental paths cannot add links or sequence. Portfolio integration is active, and no repeat-resolution benefit is established |
| Repaired qualification substrate | Generator-v4/evaluator-v4 source and CLI tests covering independent RNG streams, ledger replay, ambiguity-aware exact recovery, and externally supplied dataset-content roots | Tests implementation behavior only; no v4 scorecard, public dataset, or comparator result has been admitted |
| Offline, no-mandatory-runtime design | Static product-source scan, safe-Rust source, dependency review, and ordinary local release execution | Dynamic syscall/network isolation was blocked; therefore this is design/static evidence, not a completed runtime-isolation proof |

No other improvement claim is authorized. In particular, the present record supports no statement
that VeritAsm is more accurate, more sensitive, faster, lower-memory, better on contaminated data,
capable of resolving strains or haplotypes, or suitable for detecting adventitious agents.

## Review disposition

The v0.4 tree is appropriate for continued adversarial review and method development, but it is not
yet a source freeze. A verified source-review candidate is not attained until active pair/CLI/audit
work is frozen and the external release verifier passes the exact final archive under every required
toolchain. It is not ready for
publication as a validated assembler release, a crate publication, or an adventitious-agent workflow
claim. Human approval should require resolution or explicit acceptance of the open gates in
[VALIDATION.md](../VALIDATION.md), preservation of the unaudited historical performance warning and
its missing-evidence disclosure, and no expansion of the claims boundary without new retained evidence.
