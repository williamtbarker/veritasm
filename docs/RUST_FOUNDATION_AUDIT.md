# Rust foundation audit for exact short-read assembly

- Review date: 2026-09-04
- Product reviewed: VeritAsm `0.3.0-alpha.1`
- Product MSRV: Rust 1.85
- Scope: build-versus-buy review for exact k-mer partitioning, compacted de Bruijn graph
  construction, sequence I/O, compression, external sorting, static indexing, succinct storage, read
  mapping, and exactness-preserving probabilistic acceleration
- Status: architecture evidence and candidate triage; this document does not add a dependency or
  establish an accuracy, memory, or speed improvement

This document supplements [`DEPENDENCY_REVIEW.md`](DEPENDENCY_REVIEW.md). That file remains normative
for the exact locked product/fuzz dependency closures. For candidate-source observations and the two
dated constructor probes, this later snapshot records the newer evidence; it does not silently change
an adoption decision or the frozen comparator set.

## Decision

Keep the stable assembly identity, counting, compacted-graph, evidence, and indexed exact-mapping
cores in VeritAsm. No reviewed Rust crate or embeddable assembler jointly supplies:

- collision-free full-key identity over the required dynamic k-mer range;
- checked, non-saturating support accounting;
- hard byte, temporary-storage, and open-file bounds;
- output that is byte-identical across thread counts;
- process-local rather than global parallel configuration;
- transactional final publication; and
- an evidence ledger for every retained, filtered, ambiguous, capped, or failed operation.

Use maintained, permissively licensed crates for narrow infrastructure where their contract is a
strict subset of VeritAsm's contract. In the current stable path, that means `flate2` with its
pure-Rust backend, `tempfile`, a locally constructed Rayon pool, `sha2`, `serde`/`serde_json`, and
`rustix`. Keep SIMD minimization, radix sorting, minimal perfect hashing, succinct structures, and
standards libraries behind version-pinned experimental boundaries until representative benchmarks
and differential tests justify them.

GGCAT and Cuttlefish remain external graph-construction comparators. They are not drop-in libraries
for the VeritAsm core. Generic de Bruijn graph, general graph, native HTSlib, and generic external-sort
libraries do not currently replace enough audited code to justify their additional semantic and
resource risk.

## Review method and limits

Candidate versions, license expressions, declared Rust versions, documentation, repository state,
and relevant implementation paths were inspected from downloaded crate sources and official
repositories available on the review date. A missing `rust-version` is **not** MSRV evidence.
License metadata is not a complete source or legal audit. "Direct unsafe" below means that unsafe
Rust was observed in the candidate's own inspected source; it is not a soundness judgment and it is
not a complete transitive-closure scan. Conversely, no direct unsafe found does not prove that a
crate's dependencies or all feature combinations are unsafe-free.

Candidate performance descriptions belong to their authors unless this document explicitly records
a local probe. VeritAsm makes no speed or memory claim from upstream benchmark figures. A candidate
marked "evaluate" is authorized only for an isolated prototype with pinned sources, MSRV CI,
license/advisory review, scalar or brute-force differential oracles, malformed-input tests, and
representative Linux and Apple-Silicon measurements.

## Dated build-versus-buy matrix

### Exact k-mers, minimizers, counting, and graph construction

| Candidate snapshot (2026-09-04) | License / declared MSRV | Maintenance and unsafe observations | API or scientific risk | Decision |
|---|---|---|---|---|
| GGCAT 2.2.0 | Top-level MIT; README says Rust 1.75 or later, but inspected current crates use edition 2024 and do not declare `rust-version` | Active Rust cDBG implementation; direct unsafe, process-global configuration, global Rayon initialization, process FD-limit adjustment, and UUID temporary paths were observed | Memory is a suggested rather than hard whole-process limit. The Rust API is an unpublished path crate. Later calls may inherit singleton state. Its wide-k hash regime does not supply VeritAsm's required proof that full keys, rather than a finite hash, define identity | Use a pinned executable as a graph/unitig comparator; do not embed |
| Cuttlefish 3.0.1 | BSD-3-Clause / 1.91 | Active pure-Rust external cDBG construction; direct unsafe concurrency, libc, and environment paths were observed | Exceeds the product MSRV. The library warns that normal semantic-version compatibility is not guaranteed. Resource limits are not VeritAsm hard bounds; pair/evidence semantics and transactional bundle publication are absent | Use a pinned executable as a comparator; do not embed |
| cf1-rs 0.5.1 | BSD-3-Clause / 1.91 | Active Rust reference-cDBG/index implementation; direct unsafe packing, atomics, and libc paths | Exact k-mers are limited to `k <= 63`; the design targets reference sequences rather than error-bearing assembly reads. It exceeds the product MSRV and final-output syncing is not the default contract | Algorithm and test reference only |
| `debruijn` 0.3.4 | MIT / not declared | Older 10x Genomics packed-kmer/graph crate; direct unsafe SIMD paths | Static k-mer types, old BoomPHF integration, no bounded external construction, pair model, evidence ledger, or deterministic publication contract | Do not use in the stable core |
| `bio-seq` 0.14.8 | MIT / 1.85 | Current const-generic packed biological sequences; direct unsafe was observed | `Kmer<Dna, K>` makes dynamic CLI `k` a monomorphization/dispatch problem. It does not replace counting, graph, or evidence logic | Differential encoding oracle or isolated packing prototype |
| `kmerust` 0.3.2 | MIT / 1.75 | In-memory exact canonical counts through `k <= 32`; uses DashMap | Saturating counts, extension-based input selection, small k limit, and unordered output violate stable contracts | Reject |
| `minimizer-iter` 1.2.1 | MIT / not declared | Maintained scalar minimizer iterator over `u64` and `u128`; unchecked table accesses were observed | Its default byte table maps every non-ACGT byte to A unless the caller splits input first. Canonical mode constrains minimizer width. It may choose partitions but cannot define k-mer identity | Guarded scalar oracle only; own ambiguity-aware rolling selector |
| `simd-minimizers` 3.0.0 | MIT / not declared; edition 2024 | Current AVX2/NEON work; direct SIMD unsafe | Requires a scalar fallback, runtime/compile-target discipline, and byte-for-byte differential tests on ambiguity and tie rules | Optional profiled accelerator only |
| `packed-seq` 5.0.0 | MIT / not declared | Current AVX2/NEON packed operations with a scalar feature; direct SIMD unsafe | Native-target recommendations cannot be the sole portable Apple-Silicon/Linux representation | Optional profiled accelerator only |
| SSHash-rs 0.7.1 | BSD-3-Clause / 1.88 | Active compressed exact canonical-kmer dictionary; direct unsafe/mmap/MPHF paths | Exceeds the current MSRV; weighted dictionaries are incomplete; young Rust port and static-index semantics do not replace mutable construction/evidence state | Future frozen read/graph-index experiment |
| `hashbrown` 0.17.1 | MIT OR Apache-2.0 / 1.85 | Maintained Swiss-table map; direct unsafe internals | Hash iteration and allocation behavior cannot determine stable IDs or hard-bounded primary counts | Bounded ancillary maps only; exact count core remains sort-and-RLE |
| `dashmap` 6.2.1 | MIT / 1.65 | Maintained concurrent map | Concurrent insertion order and per-entry/shard overhead complicate deterministic reduction and exact resource admission | Do not use for canonical counts |
| `petgraph` 0.8.3 | MIT OR Apache-2.0 / 1.64 | Mature generic graph crate | General node/edge objects are unnecessarily heavy for DNA's bounded degree and evidence side arrays; insertion order cannot become a persistent ID | Small-graph oracle only |
| `genome-graph` 11.0.0 | BSD-2-Clause / 1.80.1 | Generic bigraph/compact-genome facilities | Reads BCALM-like material but is not a noisy-read cDBG constructor, checked counter, or evidence engine | Do not use for the construction core |

Quality-weighted support does not justify a floating-point or probabilistic identity path. If a later
profile accumulates quality-derived support, use a documented integer or fixed-point quantity with
checked addition and retain ordinary exact occurrence/fragment support beside it. Never saturate a
count silently.

### FASTA/FASTQ, compression, and interchange

| Candidate snapshot (2026-09-04) | License / declared MSRV | Maintenance and unsafe observations | API or compatibility risk | Decision |
|---|---|---|---|---|
| `needletail` 0.7.3 | MIT / not declared | Maintained minimal-copy FASTX parser with content-level compression detection; one direct unsafe lookup was observed | Broad default codecs can add native dependencies. It does not provide VeritAsm's mate-role, exact identifier-normalization, synchronization, or lane contract | Keep as a differential parser; do not replace strict ingestion yet |
| `seq_io` 0.3.4 | MIT / 1.61 | Maintained, explicit parser errors; no direct unsafe found | Documented FASTQ grammar accepts one sequence and one quality line per record; no pair layer | Differential parser only |
| `paraseq` 0.5.1 | MIT / not declared | Active high-throughput SE, PE, interleaved, and multi-file FASTX; direct unsafe was observed | Upstream says it is not yet as rigorously tested as `seq_io`. Its generic pair check defaults to a no-op and inspected FASTA/FASTQ readers do not override it. Default niffler features widen the codec/native surface | Benchmark target after an explicit strict-pair wrapper; not a drop-in parser |
| `flate2` 1.1.10 | MIT OR Apache-2.0 / 1.67 | Maintained; selected `rust_backend` uses `miniz_oxide` | Decoder type affects concatenated-member and read-ahead behavior; truncation/trailing-data handling remains the caller's contract | Keep and test content sniffing plus multi-member decoding |
| `niffler` 3.0.1 | MIT OR Apache-2.0 / 1.82 | Maintained content sniffing | Default feature set enables bgzip, bzip2, gzip, lzma, and zstd and can pull native dependencies that are not required by the product | Defer; direct `flate2` is narrower |
| `noodles` 0.116.0 | MIT / 1.89 | Active specification-oriented FASTA/FASTQ/SAM/BAM/CRAM/VCF family; no direct unsafe found in the inspected source | Exceeds the MSRV. Upstream describes APIs as experimental; selected compression features and subcrate versions need their own audit. It does not implement GFA | Revisit individual subcrates after an explicit MSRV decision |
| `rust-htslib` 1.0.1 | MIT / not declared | Active wrapper around native HTSlib | C/FFI build, codec and optional network surface conflict with the no-mandatory-runtime and small-core goals | Never mandatory; likely reject in favor of future noodles work |
| `gfa` 0.10.1 | MIT / not declared | Last reviewed release is old; direct unsafe UTF-8/mmap paths observed | Writer contains panic sites and favors materialized structures; it does not add enough value over the small normative GFA 1 subset | Keep VeritAsm's own fallible streaming writer |

The stable parser remains responsible for content-based gzip detection, multi-member behavior,
bounded record storage, exact malformed-input context, IUPAC validation, ambiguity-run resets, strict
mate ID/role validation, cross-lane ordering, and an error before any final output is published.

### Exact read mapping and alignment

| Candidate snapshot (2026-09-04) | License / declared MSRV | Maintenance and unsafe observations | API or evidence risk | Decision |
|---|---|---|---|---|
| `bio` 4.0.1 (`rust-bio`) | MIT / 1.87 | Active FM/FMD, q-gram, suffix, and alignment algorithms; a public unsafe FMD constructor was observed | Exceeds the MSRV and brings a broad dependency/API surface. Its algorithms do not supply VeritAsm's placement-group, cap, pair, and evidence semantics | Brute-force/differential oracle only |
| `block-aligner` 0.5.1 | MIT / not declared | SIMD adaptive-block aligner; extensive direct unsafe and compile-target-specific code | Adaptive block search is heuristic and documents accuracy tradeoffs; caller must ensure ISA compatibility. It cannot decide an evidence-exact absence | Optional polishing experiment only |
| `triple_accel` 0.4.0 | MIT / not declared | Older edit/Hamming implementation with x86 SIMD/scalar selection | No ARM NEON path and no seed index; not an evidence model | Small-sequence oracle only |
| `minimap2` crate 0.1.31 with minimap2 2.30 (`minimap2-rs` wrapper) | MIT OR Apache-2.0 / 1.86 | Native minimap2 FFI; direct unsafe/native build boundary | Exceeds MSRV and targets heuristic long-read/general mapping rather than exact construction-read auditing | Reject for the core mapper |

### Sorting and disk-backed state

| Candidate snapshot (2026-09-04) | License / declared MSRV | Maintenance and unsafe observations | Resource or determinism risk | Decision |
|---|---|---|---|---|
| standard `slice::sort_unstable` | Rust standard library / product MSRV | In-place comparison sort with a caller-supplied total order | Comparison cost may exceed radix sort on some distributions, but it keeps the dependency and scratch-memory contract small | Baseline that every alternative must beat |
| `radsort` 0.1.1 | MIT OR Apache-2.0 / 1.60 | Stable LSD radix sort; direct unsafe `MaybeUninit` implementation | Requires temporary memory; upstream notes wide keys may not beat comparison sort | Optional benchmark behind an internal sort interface |
| `rdst` 0.20.14 | MIT OR Apache-2.0 / not declared | Flexible parallel unstable radix sort for `u128` and byte arrays; direct unsafe uninitialized-vector path | Default Rayon behavior and extra scratch memory need isolation. Upstream documentation says it is sometimes slightly slower than standard sort | Optional benchmark with default features disabled where possible |
| `extsort` 0.5.0 | Apache-2.0 / not declared | Generic segment sorter; no direct unsafe found | Segment bound is an item count, not an exact byte bound; merge retains every segment file/reader, so fan-in and FD use are not hard bounded | Reject for the core |
| `ext-sort` 0.1.5 | Unlicense / not declared | Generic Serde/MessagePack external sort | Estimate-based memory limits and variable-width serialization do not meet fixed-record byte admission or authenticated-run requirements | Reject |
| `spillover` 0.2.0 | MIT / not declared; edition 2024 | Active, young generic spill/merge crate; no direct unsafe found; supports byte/item budgets, comparators, deduplication, bounded fan-in, and truncation errors | Generic memory admission still depends on a caller estimator; its format lacks VeritAsm's existing domain/version/digest invariants | Re-evaluate later; keep the owned fixed-record run format |
| `memmap2` 0.9.11 | MIT OR Apache-2.0 / 1.65 | Maintained; file mapping creates an unsafe mutation/lifetime boundary | External mutation/truncation can invalidate assumptions. Mapping does not itself authenticate bytes or make publication atomic | Avoid until immutable authenticated artifacts and a dedicated tested wrapper exist |
| `tempfile` 3.27.0 | MIT OR Apache-2.0 / 1.63 | Maintained temporary-file primitive | Drop cleanup is best-effort and does not replace checked persist/fsync/rename semantics | Keep; own explicit cleanup and publication protocol |

### Minimal perfect hashing, succinct structures, and routing hashes

| Candidate snapshot (2026-09-04) | License / declared MSRV | Maintenance and unsafe observations | Membership, portability, or API risk | Decision |
|---|---|---|---|---|
| `boomphf` 0.6.0 | MIT / not declared | No direct unsafe found; parallel construction uses the default global Rayon pool | Requires unique construction keys, has assert/panic paths, and returns an arbitrary slot for an absent query. An MPHF is not a membership test | Optional static accelerator only, with an exact side key |
| `ptr_hash` 2.1.1 | MIT / not declared | Current compact MPHF; many direct unchecked unsafe paths plus Rayon/randomized construction and unsafe mmap serialization | Construction determinism across every path needs proof; intended scale and serialized layout need versioning. An absent lookup still requires exact verification | Future static-index experiment only |
| `sucds` 0.9.1 | MIT OR Apache-2.0 / 1.62 | Maintained rank/select, integer, and Elias-Fano structures | Public documented unsafe low-level APIs exist even though safe owning wrappers are available. Static representations do not simplify mutable construction | Consider only for a frozen graph/index after profiling |
| `vers-vecs` 1.10.2 | MIT OR Apache-2.0 / not declared | Active rank/select, Elias-Fano, and wavelet structures; direct intrinsic/SIMD unsafe | Performance guidance is x86/BMI2/popcnt-oriented and non-x86 fallback may differ substantially | Benchmark on Apple Silicon before adoption |
| `rapidhash` 4.5.1 | MIT OR Apache-2.0 / 1.71 | Maintained portable V3 direct API; default path is safe, with a separate optional unsafe feature | Standard `Hasher` adapters are not a serialized stability contract. A fast hash can route or probe but can never define sequence identity | Preferred experiment for routing/Bloom probes: direct V3 API, fixed secrets, pinned vectors |
| `fastbloom` 0.17.0 | MIT OR Apache-2.0 / 1.70 | Maintained Bloom implementation | Serialized algorithm/layout changed in 0.17; generic behavior does not encode VeritAsm's exact-recount and evidence contract | Own the small contract-specific Bloom layer if benchmarks justify it |

All reviewed licenses are permissive metadata expressions, but every exact archive and selected
feature closure still requires `cargo-deny`, notice, build-script, bundled-source, advisory, and
redistribution review before adoption. `ext-sort`'s Unlicense metadata is not a technical advantage
and does not outweigh its serialization/resource mismatch.

## Recommended exact minimizer-partition core

### 1. Exact identity tiers

Keep the stable `u128` canonical representation for `3 <= k <= 63`. If validation justifies wider
k-mers, add an internally monomorphic tier such as `[u64; 4]` for `k <= 127`, chosen outside hot
loops. Do not attach a discriminant to every hot record. Encode on-disk key width explicitly as 8,
16, or 32 bytes and define one total byte order independent of host endianness.

A finite hash may select a partition, Bloom bit, or candidate slot. It may never replace the full
encoded sequence in equality, deduplication, graph identity, support aggregation, or serialized
ordering.

### 2. Ambiguity-aware rolling minimizers

For a practical first implementation, store exact canonical m-mers with `m <= 31` in `u64`. Maintain
forward and reverse-complement rolling values and reset both at every non-ACGT base. Select a window
minimizer by the documented tuple:

```text
(frozen_portable_hash, exact_canonical_mmer, documented_position_tie_rule)
```

Persist the hash algorithm identifier, version, secrets/seed, m, window definition, canonicalization,
and tie rule. Hash collisions then affect routing distribution at worst; they cannot merge biological
keys.

### 3. Deterministic virtual partitions and evidence-bearing super-kmers

Route records into substantially more virtual buckets than worker threads. Assign bucket ordinals to
workers deterministically and concatenate or merge results by bucket ordinal, never completion order.
This separates persistent identifiers and output bytes from `--threads`.

Super-kmer records may reduce repeated key bytes, but each record must retain enough immutable
provenance to reconstruct every full k-mer and its evidence unit. If the support unit is the supplied
fragment instance, externally sort `(full_kmer, fragment_ordinal)` and deduplicate that exact pair
before reducing support. Do not infer fragment support from a Bloom hit or hash equality.

### 4. Authenticated bounded runs

Use a versioned, fixed-width run format with explicit endianness and checked fields for schema,
domain, k, key width, evidence width, hash/routing parameters, record count, payload bytes, and
digest. Validate headers, exact payload length, sortedness, checked counts, trailer, and digest before
a run can enter a merge. Reject trailing bytes and truncation.

Admission must be in bytes, not estimated item counts. Bound the read buffers, write buffers, heap,
fan-in, and simultaneously open files. Merge in deterministic bounded-fan-in passes, recording every
input and output digest. The current domain-specific VeritAsm spill/run format is a stronger starting
point than the generic reviewed external-sort crates.

### 5. Exact reduction and quality evidence

Sort complete keys under the frozen total order and run-length encode with checked `u64` arithmetic.
No counter may saturate silently. Retain ordinary exact occurrence/fragment support even if a later
experimental profile adds an integer or fixed-point quality sum. Floating-point reduction order must
not enter deterministic scientific artifacts.

Start with standard `sort_unstable`; benchmark radix implementations on complete representative
records, including scratch allocation and Apple Silicon behavior, before changing it.

### 6. DNA-specific compact graph

Construct the doubled/bidirected graph from sorted exact edges. Generate oriented endpoint records,
sort them, and assign node/edge IDs from canonical sequence and orientation order rather than map or
worker insertion order. Use a sequence arena plus offsets and DNA-specific degree masks or CSR side
arrays instead of a general graph object per node or edge.

Store occurrence support, fragment support, quality evidence, pair observations, filter state, and
transformation decisions in parallel versioned side arrays. Compaction and every later graph
transformation must emit conservation totals and per-reason counts. Palindromic/self-complemental
boundaries require their existing dedicated invariants; an orientation-normalized sequence match
alone is not sufficient evidence that the graph operation was correct.

### 7. Deterministic parallelism and publication

Use a process-local Rayon `ThreadPool` or scoped threads. Never initialize or depend on a mutable
global pool. Partition work by immutable ordinal ranges; sort before joining; reduce only through
checked associative integer operations; serialize from the final stable order.

Write every final artifact into a same-filesystem staging directory, close and validate it, fsync as
required by the documented durability tier, and publish at one commit point. An external constructor
that writes a final path before successful completion cannot be used as VeritAsm's publication layer.

## Exact indexed-mapper boundary

Retain the current exact, zero-mismatch mapper as the stable evidence path and change its physical
postings representation only after equivalence tests:

- sorted unique exact q-gram keys;
- a `u64` CSR offset array of length `keys + 1`;
- one packed `u64` posting containing checked `target_id: u32` and `position: u32`; and
- an explicitly selected wider posting tier when either bound exceeds `u32`.

Build the index by the same authenticated external sort over `(qgram, target_id, position)`. Query by
exact key binary search, choose candidates with a documented rarest-seed rule, and verify every
reported placement against the full read. The current flat structure containing `u64 + usize + usize`
is structurally 24 bytes per occurrence on a 64-bit target; a packed posting is 8 bytes plus key and
offset overhead. This is a representation calculation, **not** a measured peak-RSS reduction.

An MPHF can accelerate lookup only if each selected slot retains and compares an exact side key.
Absent MPHF queries otherwise return an arbitrary slot and do not prove absence. Candidate caps,
placement caps, path caps, integer overflow, or memory admission failure must produce an explicit
`indeterminate`/error state, never an absence claim.

For a future approximate mapper, first establish a brute-force differential oracle. A defensible
initial design is deterministic pigeonhole seeds followed by an exact thresholded Myers or banded-DP
verification. Heuristic X-drop or adaptive alignment may rank candidates in an experimental polishing
layer but cannot establish evidence-exact inclusion or exclusion.

Map mates independently and preserve all placement groups. Train an insert/orientation model only on
unambiguous, uniquely placed, orientation-consistent pairs, recording the training set and robust
summary. Enumerate graph paths under explicit distance and path-count bounds. A cap yields
`indeterminate`; a join is emitted only when the retained evidence makes it unique under the declared
model.

## Bloom-filter boundary

A small owned Bloom filter is reasonable only as an exactness-preserving nomination accelerator:

- a Bloom positive may nominate a key, partition, or exact recount;
- a Bloom negative must not become a biological absence assertion unless the complete algorithmic
  protocol proves that use safe;
- stable assembly retention and support come from a complete immutable pass plus exact recount;
- persist the hash name/version, fixed secrets or seed, bit count, probe count, expected occupancy,
  realized occupancy, format version, and pass digests;
- use independent domain-separated probes and pinned cross-platform test vectors; and
- publish counts for nominated, exact-confirmed, false-positive, and rejected records.

The existing SHA-256-based probe derivation is conservative but may be costly in a hot loop. Benchmark
the direct portable `rapidhash::v3` API with fixed secrets; do not use its standard `Hasher` adapter as
a persisted algorithm. The optional unsafe rapid-hash feature is not justified absent a substantial,
reproduced end-to-end improvement and a dedicated unsafe-boundary review. Enable no Bloom path by
default until it demonstrates lower wall time or peak RSS on representative singleton-heavy data with
byte-identical exact output.

## Narrow external-constructor probes

These probes are disclosed because they falsified two integration assumptions. They are **NOT RUN**
for the release-science scorecard: there was no signed experiment freeze, no balanced repetition
order, no external wall/CPU/RSS or maximum-temporary-storage measurement, no truth evaluator, no
public dataset, no macOS run, and no retained package-level artifact bundle. Scratch paths listed here
are local evidence locations, not distributable benchmark artifacts.

### Fixture

| Field | Exact value |
|---|---|
| Dataset ID | `v3-linear-pe-r7-aa0465c17b74825b` |
| Declared role | `development_qualification_not_release_scorecard` |
| Generator | `veritasm-simulate` 3; plan `veritasm-validation-v1`; RNG `sha256-counter-v1` |
| Seed | `aa0465c17b74825b2ef279173696dd4681fafb90860d5f58310e0212075a5652` |
| Truth | one linear molecule, 1,000,000 bases |
| Reads | 80,000 paired fragments; PE150; insert length 350; inward FR with random truth strand |
| Error model | zero substitutions; constant-quality synthetic reads; not a sequencing-platform model |
| `dataset.json` | 3,845 bytes; SHA-256 `72025f2999caa9ad5e0a3c7a86a6849311fa5d79123737f885ae3968abe14327` |
| R1 | 25,840,000 bytes; SHA-256 `a88e164a1acbb6bec3a3c1aae83af3db0bdd635929c2ed7749b1a376b1c7a0f5` |
| R2 | 25,840,000 bytes; SHA-256 `4798d0494d997d2340182e9b2ad642ce70c8e095d92d4fca3d794865998e5dd5` |
| Truth FASTA | 1,012,510 bytes; SHA-256 `00fea321f75357344d5c7d62c5bf9056064104462ff8e88f8f9590fc1993c482` |
| Scratch root | `/tmp/veritasm-bench-alpha-20260904/data` |

Only R1 and R2 were passed to the constructors.

### GGCAT probe

The executable reported `ggcat 2.2.0`, but it was built from post-tag commit
`65749e68cc30e1fde2377d3b8d61033b8a885eb9` (`v2.2.0-1-g65749e6`, commit subject
`Fix fastq reading bug`), **not** the `v2.2.0` tag commit currently listed in
`COMPETITOR_MATRIX.md`. Executable SHA-256:

```text
4f2187b845ff4cb0b98f3e61ca45f146283ef3b70010c6dca2fa7ec66389e4e0
```

The exact GGCAT build command was not retained. The run environment and argument vectors were
retained; an outer shell/capture wrapper, if any, was not. The observed run commands were:

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

Both commands produced a 999,966-byte GFA containing one `S` record of 999,949 bases. The byte
digests differed:

| Threads | GFA SHA-256 | Retained auxiliary log |
|---:|---|---|
| 1 | `d6628bfb70c0fddf6f8a05282067197f700cd6685315cd2f4e7590f97f68a397` | `/tmp/ggcat-audit-determinism-20260904/t1.stats.log`; SHA-256 `4d88d572cd8150d5599baac3826d05d7de576d1202a5b6c09f52eb546566109e` |
| 8 | `90b618b0cc4adfaeb59a2a23f180a47301887fab8b2503c473a401e64a4b3dda` | `/tmp/ggcat-audit-determinism-20260904/t8.stats.log`; SHA-256 `cd409461f83d9ec26b265a816764a3ef2825f88157566cbcbf8b716c9a9e72fb` |

The eight-thread sequence was the exact reverse complement of the one-thread sequence. After that
orientation normalization, the no-newline sequence SHA-256 was
`152370939eb1c9d2257fe099f116b8c7baf332b55c3656fd43e78412594e7906`. Thus this probe falsifies
byte-identical GFA output across thread counts for this executable and command; it does **not** show a
biological sequence or topology disagreement. It also does not measure accuracy against the fixture
truth, because no evaluator was run.

### Cuttlefish probe

The executable reported `cuttlefish 3.0.1` and was built from commit
`bf6163df43efe43f21f2bf5bd65db9b351662b12`. The selected toolchain was
`rustc 1.98.1 (48a229cea 2026-09-01)` and `cargo 1.98.1 (797e8a9bc 2026-08-05)` for
`x86_64-unknown-linux-gnu`. Executable SHA-256:

```text
c1f37e2937fa3aee3c069c62781219a87b803ce3dcf00bb8d5e91cdb7773641f
```

The build command below omits the original workspace-specific Rust/Cargo
installation paths; it should be run from that dependency's checkout with the
recorded toolchain selected:

```bash
CARGO_TARGET_DIR=/tmp/cuttlefish3-audit-build \
cargo +stable build --release -p cuttlefish-rs-cli
```

The run environment and argument vectors were retained; an outer shell/capture wrapper, if any, was
not. The observed run commands were:

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

Both invocations reached final coordinate materialization and panicked at
`crates/cuttlefish-rs/src/discontinuity.rs:5972:39` with:

```text
materialized rank fits C++ weight_t: TryFromIntError(PosOverflow)
```

| Threads | Final-path state | Retained stderr |
|---:|---|---|
| 1 | `/tmp/cuttlefish3-audit-determinism-20260904/t1.fa`, 0 bytes | `/tmp/cf3-t1.stderr`; SHA-256 `b9cf5009b6607f156b5d278c5f9d6a69dfd561b7bcb0bcdcf064d52729ec6dee` |
| 8 | `/tmp/cuttlefish3-audit-determinism-20260904/t8.fa`, 0 bytes | `/tmp/cf3-t8.stderr`; SHA-256 `b2ada1a5c8927887d39fdc233938ddfcccfc33b6152b59b9fbe4f3d2fe2c208f` |

This narrowly demonstrates a panic and creation of an empty final path before successful completion
on this fixture. The exact process exit codes were not retained. An existing destination was not
seeded, so the probe does **not** establish whether Cuttlefish would clobber a pre-existing result.
The intermediate trees were not validated as scientific output.

## Foundation recommendation

Proceed with an owned exact minimizer-partition core, exact full-key external sort-and-RLE, a
DNA-specific doubled/CSR compact graph, an evidence-carrying transformation ledger, an owned exact CSR
read index, and transactional deterministic reporting. Use GGCAT and Cuttlefish as pinned independent
oracles after their exact commits, binaries, commands, formats, and resource envelopes are frozen.

The promising engineering direction is the combination of deterministic virtual minimizer
partitions, evidence-bearing super-kmer records, and authenticated conservation ledgers. Each element
has published precedent; the composition may be useful, but this audit does not call it a breakthrough
or claim performance. It earns adoption only by exact differential tests, adversarial resource tests,
and pre-registered end-to-end reconstruction benchmarks.

## Primary official sources

### Assembly and graph construction

- [GGCAT source](https://github.com/algbio/GGCAT), [GGCAT paper](https://doi.org/10.1101/gr.277615.122)
- [Cuttlefish source](https://github.com/COMBINE-lab/cuttlefish), [Cuttlefish 2 paper](https://doi.org/10.1186/s13059-022-02743-6), [Cuttlefish 3 preprint](https://doi.org/10.1101/2025.02.02.636161)
- [cf1-rs source](https://github.com/COMBINE-lab/cf1-rs)
- [`debruijn` source](https://github.com/10XGenomics/rust-debruijn)
- [`bio-seq` source](https://github.com/jeff-k/bio-seq)
- [`kmerust` source](https://github.com/suchapalaver/kmerust)
- [SSHash-rs source](https://github.com/COMBINE-lab/sshash-rs)
- [`hashbrown` source](https://github.com/rust-lang/hashbrown)
- [`dashmap` source](https://github.com/xacrimon/dashmap)
- [`petgraph` source](https://github.com/petgraph/petgraph)
- [`genome-graph` source](https://github.com/sebschmi/genome-graph)
- [BCALM2 paper](https://doi.org/10.1093/bioinformatics/btw279)
- [Bifrost paper](https://doi.org/10.1186/s13059-020-02135-8)

### Minimizers and packed sequence

- [`minimizer-iter` source](https://github.com/rust-seq/minimizer-iter)
- [`simd-minimizers` source](https://github.com/rust-seq/simd-minimizers), [SIMD minimizer paper](https://doi.org/10.4230/LIPIcs.SEA.2025.20)
- [`packed-seq` source](https://github.com/rust-seq/packed-seq)

### Sequence I/O, formats, and alignment

- [`needletail` source](https://github.com/onecodex/needletail)
- [`seq_io` source](https://github.com/markschl/seq_io)
- [`paraseq` source](https://github.com/noamteyssier/paraseq)
- [`flate2` source](https://github.com/rust-lang/flate2-rs)
- [`niffler` source](https://github.com/luizirber/niffler)
- [`noodles` source](https://github.com/zaeleus/noodles)
- [`rust-bio` source](https://github.com/rust-bio/rust-bio)
- [`rust-htslib` source](https://github.com/rust-bio/rust-htslib)
- [GFA specification](https://github.com/GFA-spec/GFA-spec), [`gfa` crate source](https://github.com/chfi/rs-gfa)
- [`block-aligner` source](https://github.com/Daniel-Liu-c0deb0t/block-aligner), [block aligner paper](https://doi.org/10.1093/bioinformatics/btad487)
- [`triple_accel` source](https://github.com/Daniel-Liu-c0deb0t/triple_accel)
- [`minimap2` wrapper source](https://github.com/jguhlin/minimap2-rs), [upstream minimap2 source](https://github.com/lh3/minimap2)

### Sorting, MPHFs, succinct structures, and hashing

- [`radsort` source](https://github.com/JakubValtar/radsort)
- [`rdst` source](https://github.com/Nessex/rdst)
- [`extsort` source](https://github.com/appaquet/extsort-rs)
- [`ext-sort` source](https://github.com/dapper91/ext-sort-rs)
- [`spillover` source](https://github.com/nrminor/spillover)
- [`boomphf` source](https://github.com/10XGenomics/rust-boomphf)
- [PtrHash source](https://github.com/RagnarGrootKoerkamp/ptrhash), [PtrHash paper](https://doi.org/10.4230/LIPIcs.SEA.2025.21)
- [`sucds` source](https://github.com/kampersanda/sucds)
- [`vers-vecs` source](https://github.com/Cydhra/vers)
- [`rapidhash` source](https://github.com/hoxxep/rapidhash)
- [`fastbloom` source](https://github.com/tomtomwombat/fastbloom)
- [`memmap2` source](https://github.com/RazrFalcon/memmap2-rs)
