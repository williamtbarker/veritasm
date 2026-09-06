# Gate 0: Virustic2 baseline

**Status:** PASS for research and architecture work; not an accuracy, production, or release gate.  
**Audit date:** 2026-09-03  
**Baseline repository:** <https://github.com/williamtbarker/virustic2>  
**Pinned commit:** `b211915fc7cce82629766b77024463c6cabcc749`  
**Package:** `virustic2 0.1.0`, MIT, Rust 2021, declared MSRV 1.85  
**Audit host:** Linux 6.18.35 x86-64; `rustc 1.98.0 (88d9e12ae 2026-08-18)`;
`cargo 1.98.0 (797e8a9bc 2026-08-05)`

This gate establishes what Virustic2 actually does, which behavior is a compatibility obligation,
and which observed behavior is a defect that Virustic3 must not inherit. The complete narrative audit
is in `docs/BASELINE_AUDIT.md`; frozen dispositions are in `docs/DECISION_LOG.md`.

## Scope and method

All 26 tracked files at the pinned commit were inventoried and inspected, including every Rust source
and test, both manifests, the lockfile, the synthetic generator and example, the verification script,
CI and Dependabot configuration, benchmark/design documents, and release metadata. Documentation was
checked against executable behavior rather than treated as evidence by itself. The baseline checkout
was not edited and remained clean.

The observed pipeline is:

1. stream wrapped plain or gzip FASTA/FASTQ records;
2. scan canonical two-bit k-mers with an ambiguity and Phred+33 threshold;
3. optionally deduplicate canonical k-mers within each input fragment;
4. insert both orientations into a packed de Bruijn graph;
5. prune by support and iteratively clip relative-support dead-end tips;
6. emit canonical maximal nonbranching paths and an aggregate JSON report.

It is an exact single-k unitig assembler. Pair distance, insert orientation, and read-to-contig
evidence do not participate in reconstruction.

## Executed gates

Commands were run from the pinned Virustic2 checkout.

```bash
./scripts/verify.sh
cargo +1.85.0 test --locked --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
cargo audit

audit_target="$(mktemp -d)"
CARGO_TARGET_DIR="$audit_target" \
  cargo test --locked --all-targets --all-features --release
```

| Check | Observed result |
|---|---|
| Format, check, and Clippy with warnings denied | Passed |
| Debug tests | 22 library + 2 binary + 7 CLI = 31 passed |
| Documentation | Passed with warnings denied; zero doctests exist |
| Release build | Passed |
| Cargo package and clean extracted-package verification | Passed; 26 packaged files |
| Rust 1.85 all-target test build | Passed |
| Isolated release-profile all-target tests | 31 passed |
| RustSec advisory scan | Passed; no known advisory in the 55 locked package entries |
| Dependency source review | All locked third-party packages came from crates.io with checksums; no git dependencies |
| License metadata review | All dependencies offer a permissive license path compatible with MIT distribution; no automated policy is present |
| Project-source unsafe scan | No `unsafe` found; the crate does not yet enforce `forbid(unsafe_code)` |
| Runtime network inspection | No network path found statically; syscall tracing could not run because ptrace was denied |

Linux and the declared MSRV were directly tested. The repository CI names Ubuntu, macOS, and Windows,
but this audit did not independently execute macOS, Apple Silicon, or Windows binaries.

## Benchmark reproduction

### Published single-end workload

```bash
cargo build --release --locked --example generate_synthetic
target/release/examples/generate_synthetic \
  200000 /tmp/virustic2-benchmark.fastq

/usr/bin/time -f 'elapsed=%e maxrss_kb=%M' \
  target/release/virustic2 assemble \
  --single /tmp/virustic2-benchmark.fastq \
  --output /tmp/virustic2-t1.fasta \
  --kmer-size 31 --min-support 2 --min-contig-length 200 \
  --tip-length 0 --support-mode occurrence --threads 1 --quiet

/usr/bin/time -f 'elapsed=%e maxrss_kb=%M' \
  target/release/virustic2 assemble \
  --single /tmp/virustic2-benchmark.fastq \
  --output /tmp/virustic2-t4.fasta \
  --kmer-size 31 --min-support 2 --min-contig-length 200 \
  --tip-length 0 --support-mode occurrence --threads 4 --quiet

sha256sum /tmp/virustic2-benchmark.fastq \
  /tmp/virustic2-t1.fasta /tmp/virustic2-t4.fasta
```

```text
396cac8bd9878e19f8fb902b21e7e6002078a862bb474ba8be75529d49f48e8d  input
f42b48f47e907453b45f5c5925fbf9e1bb9551ecf4aa8c15b2e43a59b36e644b  threads=1 FASTA
f42b48f47e907453b45f5c5925fbf9e1bb9551ecf4aa8c15b2e43a59b36e644b  threads=4 FASTA
```

The independent audit run measured 4.92 s and 14,876 KB peak RSS with one thread, and 3.90 s and
19,416 KB with four threads. Both output digests exactly reproduce `docs/BENCHMARK.md` in the baseline
repository. A separate run under shared-host contention made four threads slower, establishing that
these timings are snapshots rather than stable performance guarantees.

### Published paired compressed workload

```bash
target/release/examples/generate_synthetic 200000 \
  /tmp/virustic2-R1.fastq.gz /tmp/virustic2-R2.fastq.gz

/usr/bin/time -f 'elapsed=%e maxrss_kb=%M' \
  target/release/virustic2 assemble \
  --read1 /tmp/virustic2-R1.fastq.gz \
  --read2 /tmp/virustic2-R2.fastq.gz \
  --output /tmp/virustic2-paired.fasta --threads 4 --quiet

sha256sum /tmp/virustic2-R1.fastq.gz \
  /tmp/virustic2-R2.fastq.gz /tmp/virustic2-paired.fasta
```

```text
2bf6ff338fae5e4c0068aeee45d7b9da8f2f20d1157762cf08a1f87dff1d7296  R1
4b746ad9456d11b8c85e4d9f724ca39dda93ac281b41d407110a068b7c6252bd  R2
83db64425c795ccc06a59877768681ee215a750f2da210cfd24476d09c8dc667  FASTA
```

The audit run measured 11.48 s and 23,956 KB peak RSS and emitted one 49,999-base unitig. The two
published input hashes reproduced exactly. Virustic v1 was not present for an independent comparison,
so this gate does not independently affirm the published v1 speed or memory ratios. Neither workload
measures assembly accuracy.

The generator uses the 350-base paired-insert start bound even for single-end data; consequently the
last 201 bases of its 50 kb genome cannot be sampled in single-end mode. It is suitable for throughput
reproduction, not truth-known completeness validation.

## Confirmed compatibility surface

- Plain and content-detected gzip FASTA/FASTQ, including wrapped records and concatenated gzip members.
- Single-end, paired-end, ordered multi-lane, and one-use standard-input operation.
- Pair lockstep cardinality and format checks plus normalized first-token comparison.
- Phred+33 validation, Q0--Q93 hard-threshold window filtering, and IUPAC ambiguity splitting.
- Canonical rolling two-bit k-mers for `3 <= k <= 31`.
- Fragment and occurrence support modes, with fragment precisely meaning at most one contribution per
  canonical k-mer per supplied single read or record pair.
- Deterministic canonical FASTA across the tested thread counts.
- Typed library errors, CLI context, aggregate cleaning counters, and versioned JSON.
- Same-directory temporary-file write, flush, file sync, and rename for each individual destination.

These are behavior obligations, not a requirement to retain Virustic2's internal types or CLI schema
unchanged.

## Defects and adversarial observations

| ID | Observation | Evidence | Virustic3 disposition |
|---|---|---|---|
| V2-D01 | A failed two-output run can replace an existing FASTA. | FASTA is committed before JSON (`src/main.rs:164-170`). An invalid report destination returned exit 1 after the existing FASTA digest changed. | Replace with an output-bundle transaction and regression test. |
| V2-D02 | Two unresolved path aliases can target one destination. | `result` and `sub/../result` were accepted; exit 0 left JSON at the FASTA path (`src/main.rs:193-229`). | Resolve destination identity before committing and reject collisions. |
| V2-D03 | Pair role is not validated. | R1 `x/2` + R2 `x/1`, and two CASAVA mate-1 headers, both exited 0 (`src/pipeline.rs:317,485-491`). | Parse and validate identifier identity separately from mate role. |
| V2-D04 | Lexical aliases of one physical file can be used as both mates. | `examples/reads.fasta` + `./examples/reads.fasta` exited 0 (`src/pipeline.rs:469-473`). | Compare resolved file identity where supported and reject/warn conservatively. |
| V2-D05 | “Fragment support” is record-instance support, not molecule support. | IDs and lane paths are not globally deduplicated; supplying one lane twice doubled observed support from 4 to 8. | Preserve the counting mode but use exact terminology and detect duplicate paths. |
| V2-D06 | `circular=true` means only a closed graph cycle. | `src/assembly.rs:86-101,141-179`; no reads are remapped to prove a molecular closing junction. | Replace the boolean claim with a circular-candidate state and explicit junction evidence. |
| V2-D07 | Streaming input does not bound whole-run memory. | All distinct k-mers remain resident until pruning; one whole record and a batch of record/k-mer vectors are materialized. | Add measured memory controls, partitioning/disk-backed operation, or early failure with a remedy. |
| V2-D08 | Edge support silently saturates. | `src/graph.rs:75-77` uses `u32::saturating_add` without telemetry. | Widen counters or report every saturation event as a data-quality failure. |
| V2-D09 | “Quality-aware” is a hard filter, not quality weighting. | `src/dna.rs:187-210`; accepted k-mers all add one support unit. | Keep threshold behavior for compatibility; do not describe it as weighted evidence. |
| V2-D10 | Gzip sniffing assumes the first buffer exposure contains two bytes. | `src/fastx.rs:118-129` calls `fill_buf().starts_with([0x1f,0x8b])`. | Use a two-byte lookahead robust to legal short reads. |

Additional scientific boundaries are not defects: Virustic2 has no multi-k assembly, compacted unitig
graph, read correction, pair-distance constraint, bubble phasing, strain reconstruction, positional
coverage, GFA, or read-backed junction audit. Those capabilities require separate evidence and must
not be inferred from successful unitig emission.

## Retained and replaced mechanisms

| Retain as a tested behavior or principle | Replace or strengthen before Virustic3 compatibility exit |
|---|---|
| Content-based gzip handling, wrapped FASTX, and loud malformed-input errors | Two-byte sniff implementation and malformed/compressed fuzz coverage |
| Rolling canonical encoding and IUPAC window splitting | Single-k-only reconstruction and graph-wide in-memory scaling |
| Fragment and occurrence counting definitions | “Independent molecule” language, duplicate-lane silence, and saturating `u32` counts |
| Deterministic ordered parallel reduction and sorting at stable boundaries | Tiny one-dataset determinism evidence with cross-platform golden tests |
| Conservative boundary at reverse-complement-palindromic nodes | Unqualified graph-cycle-to-circular-molecule inference |
| Exact unitig extraction as a useful database-free fallback | Unitig-only output as a claim of reconstructed consensus or haplotype |
| Typed domain errors and contextual CLI presentation | Record errors lacking lane/mate/path context |
| Aggregate QC and graph-cleaning counters | Aggregate-only evidence with no per-contig/read/junction ledger |
| Same-directory temporary-file write primitive | Sequential multi-artifact commit, unresolved aliases, and absent directory sync |
| Pure-Rust gzip and no application network path | Lack of an enforceable runtime-network and dependency-license gate |

## Gate 0 exit criteria

| Criterion | Status |
|---|---|
| Exact immutable Virustic2 source point recorded | Met |
| Every tracked source, test, document, workflow, benchmark, and manifest audited | Met |
| Advertised local quality gates independently executed | Met |
| Declared MSRV independently compiled and tested | Met |
| Published synthetic inputs and deterministic FASTA reproduced by digest | Met |
| Actual compatibility surface separated from documentation claims | Met |
| Defects assigned explicit retain/replace dispositions | Met |
| Accuracy and performance claims constrained to available evidence | Met |
| Baseline checkout preserved without source edits | Met |

Gate 0 permits research and architecture work. Major Virustic3 implementation remains contingent on
the research and architecture gates. Virustic3 does not satisfy baseline compatibility merely by
compiling: each retained behavior and each V2-D regression must have an executable test, and all
requested output artifacts must survive any failed run unchanged.
