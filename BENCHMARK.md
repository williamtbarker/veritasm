# VeritAsm benchmark protocol and status

- Status: **release-science protocol not executed; one binary-hash-bound dirty-tree engineering
  probe, one older post-hoc development summary, and two narrow external-constructor probes are
  disclosed below, but none is evidence admitted to the scientific scorecard**
- Protocol version: `veritasm-benchmark-v1`
- Date frozen: 2026-09-03
- Evidence ledger reviewed: 2026-09-04

This is the canonical benchmark plan for the neutral fixed-k assembly contract v0.1. Dataset identities and
admission state are in `docs/DATASET_CATALOG.md`; metric definitions and correctness gates are in
`VALIDATION.md`; comparator provenance requirements are in `docs/COMPETITOR_MATRIX.md`. The
development summary below is disclosed as a conservative investigation lead, but the raw command,
timing, output, and evaluator artifacts described by the prior report are unavailable. The later
GGCAT and Cuttlefish probes retain limited scratch evidence, but were not preregistered or measured as
benchmarks. None satisfies this document's execution freeze or can establish a reconstruction tie, a
resource regression, scientific performance, or superiority.

The checked-in `examples/generate_synthetic.rs` program is a deterministic development fixture
generator, not the pre-registered scientific simulator. Smoke-test output from it can exercise code
paths but cannot populate the release accuracy or performance scorecard. The historical section below
transcribes a reported model, seed, and hashes. Without the underlying artifacts, those values are not
independently auditable and do not retroactively satisfy preregistration.

## Questions, not claims

The benchmark is designed to answer these bounded questions:

1. Does VeritAsm preserve exact counter, graph, and unitig results across thread counts and external
   count spill patterns?
2. On which frozen workloads does exact disk-backed counting reduce peak resident memory, and what
   temporary-I/O and runtime cost does it add?
3. How do fixed-k topology-preserving unitigs trade genome fraction and contiguity against false
   junctions relative to the frozen comparators?
4. How do exact support thresholds behave when a real or simulated component has low abundance in a
   high-background library?
5. Does the pair audit report useful exact observations and exclusions without changing or falsely
   joining FASTA sequence?
6. Does the isolated two-hit Bloom prototype preserve exact outputs, and on which singleton-heavy
   workloads, if any, does it reduce partition bytes or time?

None has been answered for the release-science matrix. The unaudited historical summary below reports
one easy, perfect-read fixture; it cannot answer these questions. Recovery at a planted fragment count
would be an algorithm benchmark endpoint, not organism detection, analytical sensitivity, a limit of
detection, or evidence of absence.

## Execution freeze

A benchmark run is invalid until one signed-off experiment manifest records:

- admitted dataset and truth SHA-256 values;
- complete assembler/comparator identities and executable SHA-256 values;
- build flags, linked libraries, CPU instruction features, and container digest if used;
- command, environment variables, input adapter, working-directory policy, and output inventory;
- evaluator commit/binary digest, command, reference/truth digest, and normalization version;
- host identifier, OS/kernel, architecture, CPU model/governor, physical/logical cores, RAM, swap,
  storage device/filesystem/mount options, and free space;
- thread, RAM, temporary-disk, open-file, output, and wall-time limits;
- cache condition, warm-up policy, repetition order, and repetition count; and
- all exclusions and stopping rules before any truth score is viewed.

Candidate public accessions and short release hashes are not a freeze. Missing source hashes, truth,
licenses, or executable digests block the affected run.

## Primary run matrix

Every applicable primary dataset receives all runs in its row. Tool modes are separate results.

| Dataset class | Required primary runs |
|---|---|
| Linear/isolate-like at moderate or high depth | Virustic2; VeritAsm; ordinary SPAdes; SKESA; MEGAHIT default |
| Low-abundance sequence in high background | Virustic2; VeritAsm; metaSPAdes; MEGAHIT default; MEGAHIT `meta-sensitive` |
| Deliberate uneven-depth or two-component mixture | VeritAsm; ordinary SPAdes and/or metaSPAdes as frozen before scoring; MEGAHIT default; IDBA-UD diagnostic |
| Paired repeat-resolution challenge | Virustic2; VeritAsm; ordinary SPAdes; SKESA; ABySS diagnostic; Velvet historical control |
| Retained-graph and distinct-key resource stress | VeritAsm; MEGAHIT; Minia diagnostic; preregistered ABySS Bloom mode diagnostic |
| Unmodified public operational case | Virustic2 where accepted; VeritAsm; the task-equivalent Tier 1 comparators selected before truth/reference scoring |

Virustic2 is pinned at commit `b211915fc7cce82629766b77024463c6cabcc749`. SPAdes,
metaSPAdes, MEGAHIT, and SKESA remain blocked until the unresolved freezes in
`docs/COMPETITOR_MATRIX.md` are completed. No release benchmark can currently execute.

## Parameter policy

### VeritAsm

The primary run is one independent bundle at:

```text
k=31
profile=thresholded
support_unit=supplied_fragment_instance
min_support=2
min_base_quality=20
remap=true
```

Independent `k=21` and `k=51` runs and `retain_all` at `k=31` are sensitivity analyses on every
truth-known primary case. All are reported; none is chosen as “VeritAsm” after scoring. Stable 0.1 has
no library-orientation model, bundled multi-k reconciliation, tip removal, bubble collapse,
correction, or scaffolding setting.
Resource limits equal the values frozen in the experiment manifest and never adapt to observed graph
size or truth.

### Other assemblers

- Virustic2 receives the closest explicit k, quality, minimum-support, minimum-length, and support-mode
  values. Semantic differences remain named in the result record.
- Ordinary SPAdes and metaSPAdes use their frozen vendor defaults as separate modes unless one
  alternate was predeclared for the entire dataset class. Error-corrected output, contigs, scaffolds,
  and graphs remain distinct artifacts.
- MEGAHIT default is one result. `meta-sensitive` is a separate result only on its predeclared
  high-background/mixture classes. Hardware acceleration state is frozen.
- SKESA uses one exact frozen build and one predeclared ordinary command after its version/license
  discrepancy is resolved. Its optional connector GFA is a derived diagnostic, not native graph
  evidence.
- IDBA-UD, ABySS, Minia, and Velvet are diagnostic controls. They never replace a required primary
  comparator because they happened to score favorably.

A deterministic, checksum-recorded input adapter is permitted only when a comparator cannot consume
the canonical bytes. It may change wrapping/interleaving/FASTA-versus-FASTQ representation but may not
trim, correct, sample, normalize bases, discard a pair, or change fragment order unless that exact
change is the separately declared experiment.

## Resource protocol

### Scientific-comparison envelope

The initial Linux comparison envelope is predeclared as 8 assembler threads, 32 GiB resident-memory
limit, 500 GiB writable temporary-storage limit, 1,024 open files, and 12 hours wall time per process.
If the selected host cannot enforce that envelope, the freeze must be amended before any score is
viewed and the affected matrix rerun. A resource failure stays in the scorecard; it is not rerun with
more resources only for the failing tool.

macOS/Apple-Silicon runs establish build, functional, deterministic, transaction, and selected
performance portability. They are a separate hardware stratum and are never averaged with Linux.
External comparators unavailable on macOS are labelled unavailable rather than assigned a biological
or performance score.

### Timing and repetitions

- Build time and first-run dependency setup are excluded from assembly runtime but retained separately.
- Each successful performance cell has one unscored warm-up and five measured repetitions.
- Measured tool order follows a seed-derived balanced order fixed in the experiment manifest.
- Cold-cache and warm-cache experiments are separate. No cache-dropping command is run unless the host
  and privilege method are frozen and applied identically.
- Wall time, user/system CPU, exit status, peak RSS, maximum temporary bytes, output bytes, and where
  available bytes read/written are retained for every repetition.
- Report every raw value plus median and median absolute deviation. Do not suppress an outlier without
  a predeclared machine-health exclusion that applies to every tool.

Linux memory is taken from a cgroup-v2 peak when available and cross-checked with a pinned external
measurement tool; macOS uses a separately documented maximum-resident measurement. Values from
different measurement definitions are not directly pooled. Temporary use is sampled and reconciled
with file manifests; a sampler may underestimate transient allocation and must say so.

Comparator executables, evaluators, containers, and measurement utilities are development-only
benchmark dependencies. They do not become VeritAsm runtime dependencies and are never invoked by the
core executable.

## Accuracy and evidence scorecard

The primary per-dataset table contains, where defined:

- truth and target-unique genome fraction;
- aligned-base error rate, QV/lower bound, and consensus accuracy;
- correct-block/NGA statistics;
- false junctions, evaluator-classified misassemblies, and target-background chimeras;
- duplication, target purity, off-target aligned bases, ambiguous bases, and unaligned bases;
- local minor-path precision/recall only for truth-labelled mixture cases;
- graph-adjacency precision/recall only when stages/semantics are comparable;
- exact evidence-oracle disagreements for VeritAsm;
- runtime, CPU, peak RSS, maximum temporary bytes, and output bytes;
- raw output SHA-256 values and deterministic pass/fail; and
- process outcome, including timeout, OOM, resource cap, parse error, invalid output, or evaluator
  failure.

Definitions and NA rules are normative in `VALIDATION.md`. N50, longest sequence, number of sequences,
and total assembly length are displayed only beside accuracy measurements. A scaffold table is
separate from the primary gapless-contig table. No scalar composite score or universal rank is used.

## Determinism benchmark

For each VeritAsm benchmark representative, run two processes at each available thread count 1, 2, 4,
and 8. Vary batch size and exact count spill boundaries in a separate semantic-determinism stratum.
The primary byte contract passes only if every required bundle artifact and `manifest.sha256` match.
Execution telemetry is outside the bundle and may differ.

Comparators receive repeated identical commands. Their raw determinism is reported as observed, but a
tool is not declared inaccurate merely because timestamps or nonsemantic headers differ. Any
normalization is evaluator-owned, versioned, and reported separately from raw hashes.

## Exact-counter and Bloom experiments

### Exact count scaling

Use truth-known high-distinct-key streams and admitted high-background datasets at monotonically
increasing fragment counts. Freeze the prefix derivation before execution. Report accepted events,
distinct exact keys, partition skew, run/merge counts, bytes written/read, max temporary bytes, wall
time, peak RSS, retained keys, graph-cap outcome, and complete/incomplete status. Demonstrating bounded
count RAM does not establish bounded runtime, disk, or retained-graph memory.

### Two-hit sieve prototype

The no-sieve exact counter is the oracle. Eligible tests use fragment-instance support and thresholds
at least two; support-one and occurrence runs must bypass. Compare retained exact stream and all shared
scientific artifact bytes first, then performance.

ADR 0004's predeclared continuation threshold is at least 25% fewer partition bytes **or** at least 10%
lower wall time on at least one declared singleton-heavy workload, with no more than 10% wall-time
regression on the declared recurrent-key unfavorable workload. Every workload is reported. Any exact
output difference rejects the mechanism regardless of speed. These thresholds are experimental
go/no-go rules, not current performance claims.

The default-off syncmer scout is a different experiment. Its time-to-first-exactly-verified-candidate
metric cannot replace final assembly accuracy or normal completion, and no incomplete run contributes
an absence conclusion.

## Failure and no-clobber benchmark

Resource experiments deliberately cross memory, temporary-byte, run-count, retained-key, mapping-
candidate, and staged-output limits at `limit-1`, `limit`, and `limit+1`. Every run records the stable
error class and hashes a pre-existing destination before and after. A run passes only if it either
commits one complete verified bundle or returns failure without changing the destination.

Two simultaneous VeritAsm writers test the cooperative lock and yield exactly one VeritAsm commit. A
separate orchestrated race with a noncooperating destination creator must yield at most one VeritAsm
commit; if the external creator wins, VeritAsm fails closed. No VeritAsm execution may replace the
winner or expose its own partial directory. Timing is not compared in failure-injection runs.

## Result layout

The benchmark evidence package will contain, per experiment manifest:

```text
manifest.json
datasets/
tools/
runs/<dataset>/<tool>/<profile>/<replicate>/
evaluation/<dataset>/<tool>/<profile>/
tables/
failures/
checksums.sha256
```

Each run directory retains command, sanitized environment, stdout, stderr, exit status, resource
record, raw outputs, and checksums. Large public inputs may be referenced by an admitted immutable
cache rather than duplicated, but their hashes remain in every run manifest. Result rendering reads
machine records; hand-edited summary tables are not authoritative.

## Dirty-tree 1 Mb engineering probe — not scorecard evidence

On 2026-09-04, a binary-hash-bound VeritAsm development probe used dataset
`v3-linear-pe-r7-aa0465c17b74825b`: generator algorithm v3, one 1,000,000-base linear truth,
80,000 perfect inward-FR PE150 fragments, insert length 350, and zero substitutions. Assembly used
`k=31`, supplied-fragment-instance support, minimum support 2, minimum base quality 20, exact
construction-read auditing, and a 2 GiB memory budget. The dataset manifest SHA-256 was
`72025f2999caa9ad5e0a3c7a86a6849311fa5d79123737f885ae3968abe14327`.

The measured assembler executable SHA-256 was
`30188cf84fdf47dda490ede8c4dfa8d573275eee1e2996af6aff9d7aea8cfa7e`; it was built from a dirty
working tree, so its base commit does not identify the measured code. The external probe record has
SHA-256 `573a52a06afd77f79486c1a4525b0e5a1ac84dcefa93c87d1b6cc857fdcd8c82`.

| Threads | Measured runs | Wall time | Linux child peak RSS | Result |
|---:|---:|---:|---:|---|
| 1 | 1 | 29.172 s | 238,528 KiB | Success |
| 8 | 1 | 32.155 s | 240,888 KiB | Success; no parallel speedup on this run |

Two further uninstrumented executions supplied a second output replicate at each thread count; their
RSS samples were invalid and were discarded. All four 15-file result trees were byte-identical. The
evaluated one-thread assembly contained one error-free 999,949-base unitig, covered 999,949 of
1,000,000 truth bases, and had zero false among 999,920 eligible exact-flank adjacencies. It omitted
51 terminal truth bases. The evaluator used its exact-substring fast path and measured 2.808 seconds
and 130,464 KiB peak RSS.

This is one favorable perfect-read, single-molecule fixture, with one timed run per setting, no
controlled bare-metal host, and no established comparator. It does not enter the frozen scorecard,
does not characterize the final packaged binary, and supplies no general accuracy, sensitivity,
runtime, memory, scaling, superiority, or adventitious-agent detection claim.

## Development comparison reported 2026-09-03 — raw evidence unavailable

This section transcribes a post-hoc historical report, not a result from the frozen scientific matrix.
It used one deterministic, substitution-free, paired-end fixture with no sequencing errors, abundance
mixture, contamination, coverage challenge, repeat challenge, or public biological sample. The
summary is preserved only to disclose that the prior report claimed equal exact reconstruction on this
easy case and a VeritAsm runtime/RSS regression relative to Virustic2. The raw 15-run directory is not
present in this review workspace, so neither claim is independently auditable; the rows below are not
retained release evidence.

### Fixture and executable provenance

The prior report states that `examples/generate_synthetic.rs` version 1 generated scenario `basic` with seed `8675309`, 5,000
fragments, 100-base reads, a 250-base insert, and one 6,000-base linear truth sequence. The generator
uses a deterministic xorshift64 model and emits exact, substitution-free reads; it is not a biological
or sequencing-platform simulator.

| Item | Reported SHA-256 |
|---|---|
| `dataset.json` | `f521419e94cf880343ddd90aadc6ce95c7aba7ee61777a6c6d0ef90afff86ac0` |
| `reads_R1.fastq.gz` | `d3b20cb1bd9235d025a4100f40b9275e7c81d6a4daf82f40522060b85caeb82d` |
| `reads_R2.fastq.gz` | `95533473a71e7bd4bf70386716a6018ab614daf723296f6d9007ab207ee98c82` |
| `truth.fasta` | `5d895085d6673e6df1c659d67ab86c802ddaf528a27f8d0d0cf9bec0f448b56d` |
| VeritAsm executable | `433e455adb8cf9cefc9e0744d31ddcdfedb73cf790aaa505f5999dddd3b3d69b` |
| Virustic2 executable | `7be6db0f4b5d8c5780e333768703012b62da272e431ad412357c1575be3110dc` |
| Exact-truth evaluator executable | `f8296b65fbf317078dddb80b8c682e338234460762c79b154cbe94624a655531` |
| VeritAsm 29-file compilation-input inventory | `6d3b409036b85dd5944888ed4fad3f4f3314e0fed4de45eb5677bf5deae32816` |
| VeritAsm `Cargo.lock` | `129c48c47b704ee3f4e64df7f95503876156b494d222a477d1ed0b2b8975eaf9` |

The prior report states that Virustic2 was built from frozen commit
`b211915fc7cce82629766b77024463c6cabcc749`. It says both tools used `k=31`, fragment-instance support,
minimum support 2, minimum base quality 20,
batch size 4,096, and four threads. Virustic2 used minimum contig length 31 and tip length 0.
VeritAsm was run once with exact construction-read remapping enabled and once with `--no-remap`; all
other resource settings were the recorded defaults in each `run.json`. One warm-up reportedly
preceded each cell, then five measured runs were reportedly interleaved. The original shell command
text was not retained, which is a protocol deviation. The development report stated that executable
and input digests, per-run configuration, stdout, stderr, output bundles, GNU `time` records, and
evaluator JSON were retained by its harness; those raw artifacts could not be located during the
2026-09-04 package audit and therefore cannot be independently checked from this handoff.

The reported evaluator was `examples/evaluate_exact_truth.rs` version 1. Its documented implementation only tests whether emitted records
are exact substrings of this linear fixture truth and reports exact union coverage. It performs no
approximate alignment, circular-seam analysis, repeat-resolution analysis, phasing, or biological
interpretation. Therefore, the results below cannot substitute for QUAST-class misassembly/base-error
metrics or the full scorecard defined above.

### Transcribed development timing and memory

The prior report records all 15 measured processes as exiting with status 0. Wall time and maximum RSS
below are its transcribed `wall_seconds` and `max_rss_kb` values from GNU `time` 1.9; the absent raw
records make these rows non-auditable in the current package.

| Tool mode | Replicate | Wall (s) | Maximum RSS (KB) |
|---|---:|---:|---:|
| Virustic2 | 1 | 0.10 | 10,060 |
| Virustic2 | 2 | 0.11 | 10,136 |
| Virustic2 | 3 | 0.10 | 10,260 |
| Virustic2 | 4 | 0.11 | 10,184 |
| Virustic2 | 5 | 0.10 | 10,128 |
| VeritAsm, no remap | 1 | 0.43 | 32,664 |
| VeritAsm, no remap | 2 | 0.35 | 34,260 |
| VeritAsm, no remap | 3 | 0.34 | 32,080 |
| VeritAsm, no remap | 4 | 0.32 | 34,068 |
| VeritAsm, no remap | 5 | 0.32 | 35,028 |
| VeritAsm, remap | 1 | 0.91 | 33,196 |
| VeritAsm, remap | 2 | 0.92 | 34,452 |
| VeritAsm, remap | 3 | 0.91 | 34,536 |
| VeritAsm, remap | 4 | 0.92 | 34,352 |
| VeritAsm, remap | 5 | 0.93 | 34,076 |

| Tool mode | Reported median wall (s) | Reported wall MAD (s) | Reported median RSS (KB) | Reported RSS MAD (KB) |
|---|---:|---:|---:|---:|
| Virustic2 | 0.10 | 0.00 | 10,136 | 48 |
| VeritAsm, no remap | 0.34 | 0.02 | 34,068 | 960 |
| VeritAsm, remap | 0.92 | 0.01 | 34,352 | 184 |

The prior summary reports that both VeritAsm modes were slower and used more peak resident memory than
Virustic2. Even if the raw records are recovered, subsecond GNU `time` resolution, warm-cache
execution, a transient shared host, and a full filesystem make these development timings unsuitable
for estimating stable speed ratios.

### Exact-fixture reconstruction result

The prior summary reports that every tool/mode emitted one 5,996-base record and that all 15 assemblies
were exact truth substrings covering 5,996 of 6,000 truth bases (`99.9333%`), with no non-substring
record or base and exact aligned duplication `5996/5996` (`1.0`). The absent assemblies and evaluator
records prevent independent confirmation that the three modes tied. Even a confirmed result would not
test the four missing terminal bases as errors, approximate base accuracy, structural misassembly,
mixtures, repeats, or realistic read noise.

The prior summary reports different header-bearing FASTA hashes and one shared header-stripped sequence
hash:

| Tool mode | Reported FASTA SHA-256 across five runs | Reported header-stripped sequence SHA-256 |
|---|---|---|
| Virustic2 | `15b5c0c51f4c8ef0e220df14620c5df2f0c989b18565f830e084bc7c3f63d5b8` | `8a1b57815259d73466e27dae8e9760fa61933879f3f4cac65c4680fc757a0f0c` |
| VeritAsm, no remap | `2da843df63a320602a6a1204e19760a730462099ae64d99d9a9749155a36ea4e` | `8a1b57815259d73466e27dae8e9760fa61933879f3f4cac65c4680fc757a0f0c` |
| VeritAsm, remap | `23ce8e86944c1d1d90eaf58dec5d0f8b1319c3965d10a4d6623ebff53fa331b0` | `8a1b57815259d73466e27dae8e9760fa61933879f3f4cac65c4680fc757a0f0c` |

It also reports byte-identical VeritAsm evidence bundles across five replicates:

| VeritAsm mode | Reported complete-bundle digest | Reported `manifest.sha256` digest | Reported scientific-artifact digest |
|---|---|---|---|
| No remap | `dd5327b2105723b405b360b05a209adc4ae85686c2f996e3b154132314b0677e` | `f4eda5e232d39b59da7cfff1128bf615bbd75e30350c06f2250bda04ab1072d4` | `129fc2b05cb1c865164264fd0b8bb9bf03d705331cf2ee42a57d3d798de4581f` |
| Remap | `5cf7dc450e9644261c5c47130ce8019417a1b6d3846f6a02871f7dfc86b92280` | `af09fe15598a287503e052d16816fc538c9e03b41431a3458a3145aab8d6484f` | `b66e2d122bc40374fb73bbe3786d8eb3862f851b3e54d52704a97054f1e9abcb` |

Without the bundles, those digest rows do not independently establish repeat determinism. If recovered
and verified, they would cover only one four-thread configuration and still would not satisfy the
required cross-thread-count and spill-boundary matrix.

### Environment and interpretation limits

The prior report describes Linux 6.18.35 on x86-64, an Intel Xeon Platinum 8573C host, nine visible
CPUs with an eight-CPU quota, Rust/Cargo 1.98, GNU `time` 1.9, warm caches, and an overlay filesystem
with volatile `fsync` semantics. It says four assembler threads were requested and that the filesystem
reached 100% utilization. It does not document a cgroup-v2 memory cap, cold-cache stratum, isolated
storage device, fixed CPU governor, 8-thread/32-GiB scientific envelope, public dataset, established
external assembler, or balanced full release matrix.

No empirical comparison conclusion is independently auditable from the current handoff. The historical
summary reports the same exact 5,996-base sequence and `5996/6000` truth coverage, with slower and
higher-RSS VeritAsm runs, on one easy perfect-read fixture. Retain that as a conservative performance
risk and rerun target—not as evidence of a tie, regression, equivalence, or general behavior.

## Narrow external-constructor probes — NOT RUN for the scorecard

On 2026-09-04, one GGCAT executable and one Cuttlefish executable were exercised on one generated
development dataset to test integration and cross-thread behavior. These were post-hoc engineering
probes, **not** executions of the frozen benchmark protocol. The complete Rust ecosystem rationale,
reproduction record, hashes, and official sources are in
[`docs/RUST_FOUNDATION_AUDIT.md`](docs/RUST_FOUNDATION_AUDIT.md#narrow-external-constructor-probes).

The common fixture was `v3-linear-pe-r7-aa0465c17b74825b`: one 1,000,000-base linear synthetic truth,
80,000 paired fragments, PE150, insert length 350, zero substitutions, and seed
`aa0465c17b74825b2ef279173696dd4681fafb90860d5f58310e0212075a5652`. Its input identities were:

| File | Bytes | SHA-256 |
|---|---:|---|
| `dataset.json` | 3,845 | `72025f2999caa9ad5e0a3c7a86a6849311fa5d79123737f885ae3968abe14327` |
| `reads_R1.fastq` | 25,840,000 | `a88e164a1acbb6bec3a3c1aae83af3db0bdd635929c2ed7749b1a376b1c7a0f5` |
| `reads_R2.fastq` | 25,840,000 | `4798d0494d997d2340182e9b2ad642ce70c8e095d92d4fca3d794865998e5dd5` |

Only the two read files were passed to the constructors. No truth evaluator was run.

### GGCAT observation

The binary reported `ggcat 2.2.0` and had SHA-256
`4f2187b845ff4cb0b98f3e61ca45f146283ef3b70010c6dca2fa7ec66389e4e0`. It was built from
post-tag commit `65749e68cc30e1fde2377d3b8d61033b8a885eb9` (`v2.2.0-1-g65749e6`), not the tagged
commit currently selected in `docs/COMPETITOR_MATRIX.md`. The exact build command was not retained.
The observed run commands were:

```bash
GG=/tmp/ggcat-2.2.0-build-20260904/release/ggcat
DATA=/tmp/veritasm-bench-alpha-20260904/data/assembler_input
OUT=/tmp/ggcat-audit-determinism-20260904
mkdir -p "$OUT/t1" "$OUT/t8"
"$GG" build -k 31 -j 1 -s 2 -t "$OUT/t1" --gfa-v1 \
  "$DATA/reads_R1.fastq" "$DATA/reads_R2.fastq" -o "$OUT/t1.gfa"
"$GG" build -k 31 -j 8 -s 2 -t "$OUT/t8" --gfa-v1 \
  "$DATA/reads_R1.fastq" "$DATA/reads_R2.fastq" -o "$OUT/t8.gfa"
```

Each output was a 999,966-byte GFA with one 999,949-base segment. The one-thread GFA SHA-256 was
`d6628bfb70c0fddf6f8a05282067197f700cd6685315cd2f4e7590f97f68a397`; the eight-thread hash was
`90b618b0cc4adfaeb59a2a23f180a47301887fab8b2503c473a401e64a4b3dda`. The eight-thread segment was
the exact reverse complement of the one-thread segment, and their orientation-normalized sequence
SHA-256 was `152370939eb1c9d2257fe099f116b8c7baf332b55c3656fd43e78412594e7906`.

This is evidence of byte-level cross-thread GFA variation for that executable and invocation, not a
biological sequence/topology disagreement. The only retained auxiliary files are scratch statistics
logs `/tmp/ggcat-audit-determinism-20260904/t1.stats.log` and `t8.stats.log`; their hashes and limits
are recorded in the foundation audit.

### Cuttlefish observation

The binary reported `cuttlefish 3.0.1`, came from commit
`bf6163df43efe43f21f2bf5bd65db9b351662b12`, and had SHA-256
`c1f37e2937fa3aee3c069c62781219a87b803ce3dcf00bb8d5e91cdb7773641f`. It was built with the exact
command recorded in the foundation audit. The observed invocations were:

```bash
BIN=/tmp/cuttlefish3-audit-build/release/cuttlefish
DATA=/tmp/veritasm-bench-alpha-20260904/data/assembler_input
OUT=/tmp/cuttlefish3-audit-determinism-20260904
mkdir -p "$OUT/t1work" "$OUT/t8work"
"$BIN" build --read -s "$DATA/reads_R1.fastq" -s "$DATA/reads_R2.fastq" \
  -k 31 -c 2 -t 1 -w "$OUT/t1work" -o "$OUT/t1"
"$BIN" build --read -s "$DATA/reads_R1.fastq" -s "$DATA/reads_R2.fastq" \
  -k 31 -c 2 -t 8 -w "$OUT/t8work" -o "$OUT/t8"
```

Both invocations panicked at `crates/cuttlefish-rs/src/discontinuity.rs:5972:39` with
`materialized rank fits C++ weight_t: TryFromIntError(PosOverflow)` and left zero-byte final `.fa`
paths. Retained scratch stderr logs are `/tmp/cf3-t1.stderr` and `/tmp/cf3-t8.stderr`, with SHA-256
values `b9cf5009b6607f156b5d278c5f9d6a69dfd561b7bcb0bcdcf064d52729ec6dee` and
`b2ada1a5c8927887d39fdc233938ddfcccfc33b6152b59b9fbe4f3d2fe2c208f`, respectively. The exact exit
codes were not retained. This shows a panic and premature creation of an empty final path on this
fixture; no pre-existing destination was seeded, so it does not test clobbering.

### Probe limitations

- No signed experiment manifest, frozen host, container, linked-library inventory, resource envelope,
  balanced run order, warm-up, or repeated measurement was used.
- No external wall-time, CPU, peak-RSS, maximum-temporary-byte, or I/O measurement was collected.
  Internal phase timestamps are not substitutes for the benchmark resource protocol.
- Only one synthetic, error-free development fixture was used; there was no public, contaminated,
  uneven-depth, repeat, mixture, or malformed-input case.
- There was no truth evaluation, comparator-normalization freeze, graph-equivalence evaluator, macOS
  run, or comparison with VeritAsm output.
- Scratch logs and outputs are not a checked-in evidence bundle and may not survive packaging.
- The GGCAT commit differs from the currently proposed tagged comparator. Neither observation may be
  generalized to another commit, configuration, dataset, or platform.

Accordingly, the GGCAT result is a determinism-integration lead and the Cuttlefish result is a
failure-handling/integration lead. Both affected release comparator cells remain **NOT RUN**.

## Current execution ledger

| Evidence item | Status | Result |
|---|---|---|
| Dirty-tree 1 Mb VeritAsm probe | Binary-hash-bound engineering evidence only; one favorable fixture and one timed run per thread setting; not scorecard-admissible | Four byte-identical trees across one/eight threads and repeats; 999,949/1,000,000 exact truth bases; no eight-thread speedup; no comparator |
| Bounded perfect-read development comparison | Historical summary only; raw run artifacts unavailable; not scorecard-admissible | Reported equal one-record exact coverage (`5996/6000`) for Virustic2 and both VeritAsm modes; reported VeritAsm runtime/RSS regression on that host |
| Truth-known scientific datasets | Blocked: no scientific generator/model, seeds, and generated hashes were admitted before scoring; the development fixture above is ineligible | None |
| Semi-synthetic datasets | Blocked: no public background admitted | None |
| Unmodified public datasets | Blocked: source bytes and local SHA-256 absent | None |
| SPAdes/metaSPAdes comparator | Blocked: full freeze absent | None |
| MEGAHIT comparator | Blocked: full freeze absent | None |
| SKESA comparator | Blocked: source/version/license mismatch unresolved | None |
| GGCAT release comparator | NOT RUN: one post-tag exploratory probe used the wrong frozen commit and lacked resource/truth evaluation | Byte-level orientation variation observed; not scorecard evidence |
| Cuttlefish diagnostic comparator | NOT RUN: one exploratory build panicked on one development fixture and lacked resource/truth evaluation | Failure retained as an integration lead; not scorecard evidence |
| VeritAsm release correctness gates | Outside this benchmark run; see `VALIDATION.md` | Not admitted here |
| VeritAsm release accuracy benchmarks | Not executed | None |
| VeritAsm release performance benchmarks | Not executed | None |
| Bloom/syncmer prototypes | Not authorized in stable output; not executed | None |

Historical Virustic2 evidence remains described in its Gate 0 audit. The prior development comparison
has no raw evidence in this workspace. The two later constructor probes retain only local scratch
outputs/logs and intentionally remain outside the release scorecard.

## Reporting rule

The eventual benchmark report must lead with failures and scope, show every frozen cell, identify all
deviations, and separate implemented/tested, experimental, failed/regressed, and proposed work. Until
the release-science ledger changes with admissible artifacts, the defensible conclusion is:

> VeritAsm has no demonstrated reconstruction-accuracy or performance advantage over Virustic2 or
> any established assembler. An unaudited historical summary reported equal exact recovery and worse
> VeritAsm runtime and peak RSS on one bounded fixture, but the raw artifacts are unavailable and the
> comparison must be rerun before either the tie or regression is treated as evidence.
