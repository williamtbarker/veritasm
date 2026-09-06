# VeritAsm validation plan and evidence ledger

> Historical development/release-review record. For the 0.4.0-dev.1 preservation checkpoint, see [current status](docs/STATUS.md). Earlier results below retain their original snapshot scope.

- Status: **pre-registered scientific plan, an executable generator-v4/evaluator-v4 qualification
  substrate, and bounded Linux engineering records; no Tier 1, 2, or 3 scientific result has been
  admitted**
- Plan version: `veritasm-validation-v1`
- Date frozen: 2026-09-03
- Evidence ledger reviewed: 2026-09-05
- Applies to: the 0.1 contract in `docs/PRODUCT_CONTRACT.md`, `ARCHITECTURE.md`,
  `docs/CONFIGURATION.md`, and `docs/OUTPUT_SCHEMA.md`

This document states what must be tested, the exact experiment identities, and how results will be
interpreted. A checked box, metric, or benchmark value may be added only with a retained command,
software identity, input manifest, raw output, exit state, and checksum. Described tests and test
source are not evidence that the present implementation passes them. The candidate ledger below
separates historical commands run on superseded candidates from current-tree gates, which are **NOT
RUN** after subsequent source changes. Neither becomes final clean-extraction release evidence until
the source archive, archive checksum, extracted tree, and retained transcript are identified together.
The separately retained Virustic2 Gate 0 audit is historical ancestor evidence, not VeritAsm
validation.

## Claims boundary

Validation concerns algorithmic unitig reconstruction and the accuracy of its reported evidence. It
does not validate organism identification, biological presence or absence, viability, infectivity,
sample sterility, a limit of detection, product disposition, clinical use, or a regulated workflow.
Construction-read remapping is an internal-consistency measurement because the same observations
constructed the graph. It is never counted as independent truth.

No universal accuracy, speed, memory, or superiority statement is authorized by this plan. A future
claim must name the exact dataset, version, parameters, comparator, metric, hardware where relevant,
and uncertainty or failure conditions that support it.

## Evidence tiers

Software-correctness tests are release preconditions. Scientific results are then kept in three
separate evidence tiers; they are never pooled into one accuracy number.

| Tier | Evidence population | What it can establish | What it cannot establish |
|---|---|---|---|
| 1 — truth-known computational | Deterministically generated molecules, fragments, reads, errors, repeats, mixtures, and controls | Conformance to the specified simulator truth; exact counter/graph/evidence correctness; controlled factor response | Real library preparation, instrument artifacts, matrix effects, or biological detection performance |
| 2 — semi-synthetic | A checksum-frozen, unmodified public background plus a checksum-frozen simulated spike and deterministic pair-preserving merge | Recovery and false-junction behavior in one realistic background while retaining known spike truth | A natural sample, representative prevalence, complete background truth, or a validated detection limit |
| 3 — unmodified public | Public reads used without sequence-level editing | Operational compatibility, resource behavior, and reference-consistent reconstruction where an appropriate reference exists | Complete truth when the sample is heterogeneous, contaminated, evolved from the reference, or incompletely documented |

Every scored dataset has exactly one tier and one declared role. Exploratory data remain visibly
labelled exploratory and cannot be promoted into the frozen scorecard after outcomes are inspected.
Dataset admission rules and current candidates are in `docs/DATASET_CATALOG.md`.

## Freeze, blinding, and result retention

Before any accuracy run, a machine-readable experiment manifest must freeze:

1. dataset ID, tier, intended role, source and derived-file SHA-256 values;
2. truth object and topology, inaccessible to the assembly command;
3. generator/evaluator full commit, executable digest, configuration, and random seed;
4. assembler/comparator full commit or immutable package/image digest and executable digest;
5. complete command, environment, thread/RAM/temporary-disk/time limits, and input adapters;
6. primary and secondary metrics, NA rules, exclusion rules, and stopping rules; and
7. output directory names and expected raw-artifact inventory.

Truth is introduced only to the evaluator. Parameter selection cannot read truth alignments. Vendor
defaults and every predeclared alternate profile are independent runs; the best result is not selected
post hoc. A parse failure, install failure, timeout, out-of-memory exit, resource-cap exit, invalid
output, evaluator failure, or regression is a result and remains in the same result index as a
successful run. Corrections create a new plan version; they never rewrite an executed freeze.

### Required experiment-manifest fields

The machine record has `schema_version = "veritasm-experiment-manifest-v1"` and contains these named
top-level objects before a run is admitted:

| Field | Required contents |
|---|---|
| `experiment_id` | Stable ASCII ID unique within the plan; never derived from an observed score |
| `plan` | Validation and benchmark plan versions plus source-document SHA-256 values |
| `dataset` | Dataset ID, tier, role, source URL/accession where applicable, sanitized input labels, byte counts, and SHA-256 values |
| `truth` | Truth type, topology, eligibility mask/rule, sequence and annotation SHA-256 values, or an explicit `not_available` reason |
| `generation` | Generator commit and executable SHA-256, complete argument vector, RNG name/version, 256-bit seed hex, and derived-file manifest SHA-256; `not_applicable` for unmodified public data |
| `tool` | Name/mode, source commit or immutable package/image identity, executable SHA-256, license record, build command, CPU features, and complete run argument vector |
| `adapter` | Adapter source/executable SHA-256, input/output hashes, and complete command, or explicit `not_used` |
| `evaluator` | Evaluator commit/executable SHA-256, reference/truth hashes, exact command, metric-version IDs, and raw-alignment inventory |
| `resources` | Threads, RAM, temporary/output bytes, open files, wall limit, OS/kernel, architecture, CPU, storage/filesystem, and enforcement method |
| `randomization` | Replicate ordinal, warm-up policy, measurement repetition, and frozen seed-derived run order |
| `metrics` | Primary and secondary metric IDs, exact denominators, NA rules, and summary rules |
| `exclusions` | Predeclared dataset/run/measurement exclusions; an empty array is required when none exist |
| `stopping_rules` | Timeout, memory, disk, invalid-output, and evaluator-failure rules fixed before scoring |
| `expected_artifacts` | Relative-path inventory and required/optional status for raw tool, log, resource, alignment, score, and checksum files |

Every integer capable of exceeding JavaScript's exact range is a decimal string; hashes are complete
lowercase SHA-256 hex. Arrays with no biological ordering declare their bytewise sort key. The frozen
manifest itself is hashed before any executable receives the reads. Post-run exit state, actual
artifact hashes, deviations, and failure classification are appended to a separate result record so
the pre-run freeze cannot be silently rewritten.

`schema/validation_experiment.schema.json` now freezes these top-level sections. Its nested experiment
objects remain deliberately open in experiment-schema v1, so schema validity alone is not admission. Strict emitted
dataset and evaluation-result records and TSV column descriptors are in the other
`schema/validation_*.schema.json` files.

## Checked-in development fixture generator

`examples/generate_synthetic.rs` is implemented as a small deterministic development fixture tool. It
accepts a direct `u64 --seed`, uses the source-frozen local `XorShift64` implementation, emits perfect
Q40 reads, writes a checksum-bearing `dataset.json`, and offers `basic`, `repeat`, `circular`,
`uneven`, `mixture`, and `host-contaminated` scenarios. Its manifest calls out that it is not a
biological or platform simulator.

That generator is useful for parser, graph, pair, determinism, and transaction smoke tests only. It
does **not** implement the matrix, 256-bit seed derivation, error model, truth events, or independent
evaluator specified below and is not admitted as Tier 1 scientific benchmark evidence. Merely checking
it into the tree is not an executed result.

## Executable generator/evaluator qualification slice

`veritasm-simulate` implements a separate version-4 qualification slice for `linear-se`, `linear-pe`,
`circular-pe`, `mixture-pe`, `error-pe`, and `qc-censoring-control`. It retains the 256-bit master-seed
derivation below and derives separate layout, error, and quality SHA-256 counter streams. It writes
opaque assembler inputs separately from evaluator-only truth and records every read origin,
substitution, and non-default quality event. `error-pe` includes retained Q40 errors and low-quality
correct bases; the old substitution-linked-Q10 behavior exists only as the named censoring control.
It does not yet implement the full frozen matrix or realistic instrument/library artifacts.

Evaluator algorithm version 4 and result schema version 4 require an exact canonical dataset checksum
inventory, compute its content root, and explicitly distinguish externally bound qualification-eligible
results from development-unbound results. They verify dataset hashes, accept an empty
FASTA as a measured empty assembly, prove exact linear or rotation-aware circular placements before
using bounded fitting alignment, and record diagnostic base metrics, exact-compatible recovery bounds,
and indexed exact-flank output-adjacency classifications. Per-adjacency evidence
selection is explicit; every mode retains exact aggregate metrics and a digest of the complete logical
row stream. Dynamic-programming, exact-substring scan work, exact-index memory, packed-key
comparisons, materialized evidence bytes, and truth-ledger replay memory have explicit caps. Exact long contigs are supported,
but long approximate or split alignments still require an independent scalable evaluator. Exact
definitions and runnable commands are in
[`docs/VALIDATION_TOOLING.md`](docs/VALIDATION_TOOLING.md); ADR 0009 records the truth boundary,
ADR 0013 records the version-2 junction substrate, controlling ADR 0015 records the repaired
error/quality oracle and ambiguity-aware recovery estimands, and ADR 0021 records external content
binding, shared generator feasibility, portable paths, and bounded lockstep ledger replay.

## Pre-registered scientific truth-known generator

The complete generator described here remains **planned and not yet verified as a frozen suite**.
Generator version 4 implements the seed contract and a six-case qualification slice, but its eventual
full matrix, cross-platform byte verification, binary SHA-256, generated-file catalog, and admission
review are required before any dataset is admitted. Generator version 2 is invalidated by the
minus-strand orientation defect recorded below and in `docs/FINAL_REPORT.md`; its outputs are
non-admissible historical failure artifacts. Truth molecules, exact fragment origins, error events,
and FASTX are separate; assembler commands receive only declared FASTX paths.

### Seed contract

Every listed grid cell is generated for replicate ordinals `0`, `1`, and `2`. Its 256-bit seed is the
raw SHA-256 digest

```text
SHA256("veritasm-validation-v1" || NUL || case_id || NUL || decimal_replicate)
```

where strings are UTF-8/ASCII exactly as printed in this document, `NUL` is one zero byte, and the
replicate has no sign or leading zeros. Generator v4 derives layout, error, and quality stream seeds
with `SHA256("veritasm:validation-rng-stream:v1" || NUL || master_seed || stream_name)`. Each stream uses
the source-frozen `sha256-counter-v1` algorithm: SHA-256 blocks bind the 32 seed bytes and a
little-endian `u64` counter, and `u64` output words are decoded little-endian. An explicitly supplied
quality seed replaces only the derived quality stream. Until full-matrix generated file hashes and
cross-platform verification appear in the dataset catalog, the seeds define planned identities but
no admitted files.

Generator-v4 dataset IDs use the full SHA-256 parameter commitment specified in
`docs/VALIDATION_TOOLING.md`, including every RNG/seed/source, numeric model, library, and
plain/gzip-output parameter. This binds the declared parameter tuple; it is not an authenticity
signature and does not make an unadmitted dataset qualification evidence.
Internal semantic replay also cannot prove that a coherently rewritten truth/read/ledger set came
from the declared RNG. Evaluator v4 computes
`dataset_content_root_sha256 = SHA256(exact canonical manifest.sha256 bytes)`. Admission requires
`--expected-dataset-content-root-sha256` from an independent regeneration or preregistered record;
an omitted expected root is recorded as `development_unbound` and is ineligible for a qualification
scorecard.

Braces in the matrix below mean literal enumeration in printed order and are not part of a case ID.
For example, `linear-gc20-d1-SE150-I` at replicate `0` is one complete seed identity. Numeric suffixes
are fixed strings: `005`, `010`, and `050` mean 0.5%, 1%, and 5%; `trough0`, `trough01`, and
`trough10` mean 0x, 0.3x, and 3x in the declared 30x nominal-depth case.

### Common read models

| Model ID | Planned model |
|---|---|
| `SE150-I` | Single 150-base read; perfect sequence; constant Q40 |
| `PE150-I` | Two 150-base reads; FR orientation; outer span from a normal distribution with mean 350 and standard deviation 50, rejected outside 250–600; perfect sequence; Q40 |
| `PE150-E` | Same fragment model as `PE150-I`; versioned substitution/quality model with no indels; every simulated error and quality byte recorded in truth |

For a requested haploid depth `C` on a molecule of length `L`, the ideal generator emits
`ceil(C*L/150)` SE reads or `ceil(C*L/300)` PE fragments before declared duplicate or contamination
injection. Linear start positions are sampled only where the fragment fits. Closed truth molecules
permit wraparound and retain origin coordinates modulo `L`. These are simulator depths, not inferred
coverage values.

### Frozen synthetic matrix

All factor combinations named in one row are included; unlisted cross-products are not part of the
primary suite. Sequence templates are generator-derived and checksum-frozen before assembly.

| Family and case-ID grammar | Frozen factors | Purpose |
|---|---|---|
| `linear-gc{20,50,80}-d{1,2,5,10,30,100}-{SE150-I,PE150-I}` | One 50,000-base linear truth molecule; GC fraction 0.20, 0.50, or 0.80; listed simulator depths | k/depth/GC fragmentation and high-depth count behavior |
| `uneven-gc50-trough{0,01,10}-PE150-E` | One 50,000-base linear molecule at nominal 30x; central 5,000 bases sampled at 0x, 0.3x, or 3x | Zero and low-depth gaps without inventing sequence |
| `repeat-exact-len{50,150,300,1000}-PE150-I` | One 50,000-base molecule with two copies of one exact repeat; 30x | Repeat shorter/equal/longer than reads and typical inserts |
| `repeat-near-len300-div{005,010,050}-PE150-E` | Two 300-base copies differing at 0.5%, 1%, or 5%; 30x | Collapse, branching, and false traversal |
| `topology-{closed,linear-terminal-repeat,linear-concatemer,shared-repeat}-PE150-I` | 10,000-base closed truth or a length-matched linear confounder; 30x | Closed graph walk versus molecular-topology confounders; no automatic circularity claim |
| `mixture-div{005,010,050}-ratio{90-10,99-1}-PE150-E` | Two 50,000-base components at 0.5%, 1%, or 5% divergence; total 100x; listed fragment ratios | Minor local-path recovery without claiming global haplotypes |
| `background-ratio{10,100,1000,10000}-target{2,10,50}-PE150-E` | One 20,000-base target component at 2, 10, or 50x plus a disjoint 5,000,000-base background with the listed background:target fragment ratio | Exact counting, graph-cap behavior, low-abundance retention, and target-background false junctions |
| `quality-{q40,q30,tailq10,n001,n010}-PE150-E` | One 50,000-base molecule at 30x; constant Q40, constant Q30, terminal 20 bases at Q10, or 0.1%/1% `N` substitution in observed reads | Quality and mutually exclusive ambiguity/window accounting |
| `artifact-{adapter05,duplicate20,duplicate80,chimeric001,chimeric010}-PE150-E` | One 50,000-base molecule at 30x; 5% adapter-bearing reads, 20%/80% copied fragment instances, or 0.1%/1% cross-locus chimeric pairs | Artifact tolerance, fragment-instance terminology, and pair exclusions |
| `pair-repeat-span{inside,outside,broad}-PE150-I` | 100,000-base truth with a 500-base repeat; 30x; span distributions chosen wholly inside, spanning outside, or broadly overlapping repeat limits | Exact placements, multimapping, endpoint observations, and proof that pairs do not modify FASTA |
| `low-input-fragments{0,1,2,10,100}-PE150-I` | 10,000-base truth with exactly the listed fragment count | Empty/no-unitig semantics, support thresholds, and small-sample edge cases |

The `target` and `minor` labels exist only in the truth/evaluation namespace. VeritAsm receives no
target sequence, taxonomic label, reference, or sample-class label. The high-background matrix tests
algorithmic recovery at specified planted inputs; it is not an adventitious-agent assay and its
recovery curve is not a limit of detection.

### Required generator self-tests

- deterministic byte-identical output for the same case and seed on x86-64 and AArch64;
- every emitted read maps to its recorded origin before injected errors;
- exact requested fragment counts, orientations, spans, error events, and duplicate relationships;
- no undeclared truth sequence in headers supplied to an assembler;
- circular wraparound and reverse-complement coordinates round-trip;
- pair-preserving shuffle and merge never split, duplicate, or reorder mates internally; and
- generated FASTX, truth, and manifest hashes are recomputed after clean extraction.

Current generator-v4 and evaluator-v4 unit and CLI tests exercise the frozen master-seed vector,
domain-separated stream replay, repeated same-process generation, SE/PE record counts,
truth-path/header separation, independent circular/linear and strand coordinate replay, exact mixture
assignment, retained-error versus QC-censoring behavior, quality-only seed changes, self-truth bounds,
duplicate-sequence/repeated-start ambiguity, truth-order permutation, plain/gzip origin/error/quality
replay, fail-closed parsing and semantic reconciliation of every truth ledger against truth and
FASTQ, checksum-refreshed malformed-ledger rejection, full generator-parameter dataset-ID
commitment, canonical manifest and portable-path rejection, externally bound coherent-rewrite kill
tests, development-unbound state, and explicit truth-replay memory boundaries. Full RNG draw replay
is intentionally not implemented. Cross-architecture
byte identity, the complete matrix, and clean-extraction generated-file hashes remain open gates.

## Semi-synthetic protocol

Tier 2 starts only after a public background is admitted. The background FASTQ bytes remain unchanged.
Generated spike pairs are assigned a noncolliding identifier namespace, then whole background and
spike fragments are ordered by a deterministic SHA-256 key over
`merge_version || background_sha256 || spike_sha256 || normalized_fragment_id`. R1 and R2 travel
together. The derived manifest records every source hash, output hash, selected fragment ordinal, and
merge implementation digest.

For every admitted background, the frozen series is:

- an unspiked negative computational control;
- target fragment counts corresponding to the `2x`, `10x`, and `50x` simulator-depth definitions;
- background:target fragment ratios of `10:1`, `100:1`, `1,000:1`, and `10,000:1` when the admitted
  background contains enough reads without duplication; and
- three seed-derived replicates per cell.

No read is removed because it resembles the spike, host, a control, or an adapter. Any optional future
subtraction is a separate non-primary experiment. A sequence present in a negative or blank is not
automatically discarded, and absence from a finite negative is not evidence of biological novelty.

## Metric contract

Evaluator version 4 is source-versioned and executable for bounded truth-known qualification cases,
but the final independent evaluator and experiment versions are not frozen; therefore no scientific
scorecard may begin. The final evaluator must retain raw alignments and implement the following
predeclared quantities.
`NA` plus a stable reason is used when truth or mapping ambiguity makes a quantity undefined.

| Metric | Definition and interpretation |
|---|---|
| Truth genome fraction | For exact compatible placements, report unique-coordinate coverage and compatible-coordinate lower/upper fractions. The lower bound unions coordinates common to every placement of each contig; the upper bound unions coordinates in any placement. Report per molecule and macro/micro summaries separately. If the compatible approximate-placement universe is incomplete, report NA rather than selecting a primary target |
| Target-unique genome fraction | Same fraction restricted to target bases overlapped by at least one canonical 31-mer absent from every background truth sequence; reported beside, never instead of, total target fraction. Ambiguous truth bases are ineligible |
| Aligned-base error rate and QV | Total mismatches plus inserted and deleted bases divided by aligned evaluated bases; `QV=-10 log10(error_rate)`. Zero observed errors reports a lower bound determined by evaluated bases, not infinity. The Q63 integer-log approximation and half-up conversion to micro-QV introduced in evaluator v2 and retained in v4 keep result bytes independent of platform `f64` transcendental precision. They do not claim arbitrary-precision, correctly rounded `log10` |
| Consensus accuracy | `1 - aligned-base error rate`, with evaluated-base denominator shown |
| Correct-block/NGA statistics | Length distribution of truth-consistent alignment blocks after splitting at declared false junctions, normalized to eligible truth length; contiguity is never reported alone |
| False junctions | Output adjacencies for which sufficiently long flanks have no compatible adjacency in any truth molecule. Ambiguous repeat flanks are `indeterminate`, not false or correct. The flank length and aligner rule must be frozen before execution |
| Misassemblies | Pinned evaluator's full classified events, reported by type and retained with raw alignments; not substituted for the explicit false-junction definition |
| Target-background chimera | A false junction whose two qualified flanks map to different truth classes, target and background |
| Duplication ratio | Correctness-qualified aligned assembly bases divided by compatible covered truth bases only when coordinate placement is unambiguous. Exact ambiguity or an incomplete non-exact placement universe is NA with the raw numerator and available compatible-upper denominator retained; also report duplicated intervals once a scalable ambiguity-aware evaluator exists |
| Target purity | Correctness-qualified target-aligned output bases divided by all evaluated output bases. Unaligned and ambiguous bases remain separate categories |
| Minor-path precision/recall | On mixture cases only, exact or alignment-qualified local alternate paths matched to simulator-labelled paths. Global phasing and haplotype completeness are explicitly not scored |
| Graph topology precision/recall | Truth-compatible retained adjacencies and false/indeterminate adjacencies under an evaluator independent of the GFA writer; graph stages from unlike tools are not pooled |
| Evidence agreement | Exact count, transformation, placement, and pair-summary fields compared with exhaustive truth/oracle values where defined. Biological origin is not inferred from an exact sequence match |
| Runtime/resources | Wall time, CPU time, peak RSS, maximum aggregate temporary bytes, bytes read/written where measured, output bytes, and exit status under the benchmark protocol |
| Determinism | Raw SHA-256 equality for every core bundle artifact across repeat runs and thread counts. Semantic equality alone does not pass VeritAsm's byte contract |

Primary scorecards show every per-dataset result. Medians may summarize replicates inside one frozen
grid cell, with all replicate values retained. Tier 1, 2, and 3 values never feed one aggregate rank.
N50, longest sequence, total output length, and read-back likelihood are secondary diagnostics, not
standalone accuracy evidence.

## Software-correctness test matrix

All items below are required and currently **planned unless a linked retained test record says
otherwise**.

### Unit and exhaustive-oracle tests

- DNA encode/decode for all supported bases and every boundary `3 <= k <= 63`;
- rolling forward/reverse-complement k-mers equal a naive string oracle;
- reverse complement is an involution and canonicalization is strand invariant;
- even-k palindromes, maximum-width shifts/masks, read length exactly `k`, and reads shorter than `k`;
- the disconnected wide-k experiment round-trips and matches a slice oracle at k=63, 64, 95, and
  127; matches the stable packed representation through k=63; and retains identical mutually
  exclusive quality/ambiguity accounting without implying stable assembler support above k=63;
- checked `u64` support overflow returns the typed error and never saturates;
- mutually exclusive possible/accepted/ambiguity/quality window counts sum exactly;
- fragment-instance deduplication across repeated windows and both mates versus occurrence mode;
- numeric partitions, sorted runs, fan-in merges, and exact histograms match a small in-memory oracle;
- graph in/out degree, reverse-complement mirror, neighbor, palindromic-boundary, and oriented-link
  invariants against exhaustive small DNA universes, including `mate(mate(e))=e`,
  `src(mate(e))=reverse_complement(dst(e))`, and handle count
  `2*retained_keys-self_reverse_complement_keys`;
- compaction conserves every retained oriented edge exactly once before strand collapse; no path crosses
  a branch; isolated-cycle canonicalization is invariant to start, strand, and input order; the
  `k=4` `{AATT,ATTG}` self-reverse-complement walk has three edge steps but two distinct backing keys;
- FASTA, GFA, evidence tables, and content-derived IDs agree on sequence, topology, support, and links;
- committed graph vectors cover ordinary `AAC`, palindromic-node hairpin `ACG`, self-RC `AATT`,
  self-RC walk `{AATT,ATTG}`, homopolymer cycle `AAA`, and branch `{CAA,AAC,AAG}` cases;
- complete placement enumeration never labels a prefix as unique; candidate `limit+1` is
  `indeterminate_candidate_limit`;
- the stable indexed exact mapper matches an independent brute-force placement oracle across
  target/read lengths, strands, palindromes, q values, target order, closed-target exclusion, short-
  read fallback, duplicate-ID rejection, candidate limits, one/four-thread queries, and allocation
  boundaries; the construction-read audit fixes q=15, retains exhaustive interval scanning as a test
  oracle, and serializes the performing mapper's descriptor and accounting;
- pair endpoint and swap-only canonicalization are idempotent; observations at opposite emitted
  canonical ends never merge; read-strand reverse-complement placement remains coordinate-correct;
- the isolated experimental library model classifies all four coordinate-ordered orientations,
  keeps lanes separate, uses integer nearest-rank quantiles and exact rational dominance, reconciles
  exclusions, rejects corrupt or duplicate evidence, is invariant to observation order, and marks
  insufficient, mixed, or broad-span lanes unavailable.

Property tests use a committed seed/corpus manifest as well as shrinkable random cases. A failure's
minimal input and seed become a permanent regression fixture.

### FASTA, FASTQ, gzip, and pair integration tests

- plain and content-detected gzip input independent of filename suffix, including a reader that
  returns one byte at a time during magic detection;
- FASTA and FASTQ wrapping, CRLF, permitted blank-line cases, lowercase bases, all IUPAC symbols, and
  Q0 through Q93;
- concatenated gzip members, truncated headers/deflate streams/trailers, bad CRC/length, corrupt later
  member, and trailing non-gzip bytes;
- empty source, empty sequence, header without identifier, invalid nucleotide, internal sequence
  whitespace, invalid quality byte, and sequence/quality length mismatch;
- every configured header/read/record/decoded/spool/temp/run/manifest/retained-key/output limit at
  `limit-1`, `limit`, and `limit+1`, with checked allocation arithmetic;
- paired cardinality, normalized identity, order, `/1` and `/2`, CASAVA role, conflicting dual role,
  reversed streams, missing mate, extra mate, repeated identifiers, and inferred roles;
- multiple ordered lanes, a duplicate lane, lexical aliases, symlinks, hard links, and reuse of one
  physical source in two roles;
- stdin in one role, rejection in multiple roles, and equality of stdin/file scientific results; and
- source mutation during parse/spooling and spool/run/checksum corruption after finalization; and
- public result-manifest verification enforces inclusive manifest/file/directory/depth/aggregate-hash
  ceilings at `limit-1`, `limit`, and `limit+1`, including oversized streamed manifests, deep trees,
  many-file trees, corrupt digests, and invalid limit configurations.

Malformed input is never skipped or converted into a normal empty bundle. Every failure asserts its
stable error-code family, absence of a committed new result, and preservation of any prior result.

### Fuzzing

`cargo-fuzz` targets are required for: raw FASTA, raw FASTQ, content sniffing, multi-member gzip,
paired-header normalization/synchronization, private spool decoding, exact count-run/manifest decoding,
GFA/TSV/JSON serialization boundaries, and result-manifest verification. Seed corpora include every
golden valid/malformed fixture and minimized prior crash. Release evidence records the pinned nightly,
fuzzer version, corpus digest, target, duration, executions, crashes, timeouts, and sanitizer. A smoke
run is a quality gate; it is not proof of parser safety.

### Golden output and independent parsing

Small linear, branch, repeat, palindrome, closed-walk, empty-under-parameters, SE, PE, multi-lane, and
all-NA-reason cases produce byte-frozen complete bundles. Golden reports include hostile identifiers
containing HTML metacharacters. An independent parser verifies FASTA and GFA syntax and checks that GFA
segments/links match the machine evidence. JSON Schema and TSV column/order tests reject unknown major
versions. `manifest.sha256` verification rejects an unlisted, missing, duplicated, or modified file.

Golden changes require a reviewed schema/algorithm reason plus old/new semantic diff. Regenerating
goldens merely because tests failed is prohibited.

### Determinism and parallelism

For every golden case and at least one multi-run partition/merge case, run with threads 1, 2, 4, and 8
when available, two process repetitions each, and fixed scientific parameters. Compare every committed
byte and manifest digest. Vary parser batch size, worker completion order, count-run spill boundaries,
merge-pass count, and temporary directory names without changing scientific artifacts. Also compare
x86-64 Linux and AArch64 Apple outputs. Thread counts, timing, host data, and temporary paths must not
appear in committed output.

### Transaction and failure injection

- inject failure before and after each pipeline state transition through `verified`;
- fail individual write, flush, sync, reopen, schema parse, checksum, staging-byte accounting, and
  no-replace commit operations;
- snapshot a pre-existing destination before the run and prove every filename, file byte, content
  digest, and permission mode is unchanged after failure;
- race two cooperative writers and one noncooperating destination creator; exactly one no-replace
  commit may succeed and no existing directory may be replaced;
- test dangling symlink, symlinked ancestor, hard-link/input collision, rejected cross-filesystem
  staging, permission denial, disk-full/temporary-limit, stale lock, pre-commit signal/abort, and
  unsupported no-replace behavior; and
- inspect a successful destination for the exact artifact inventory and no temporary or secret state.

Atomic visibility is not a power-loss durability claim. Parent-directory sync and cleanup behavior are
assessed under the boundary in ADR 0005.

### Probabilistic-prototype falsification

The two-hit Bloom sieve and syncmer scout are not stable 0.1 assembly features. Their tests follow ADR
0004 and cannot be used to weaken the exact path. A no-sieve v0.1 source-review package is gated on
the exact path and proof that the optional mechanisms are unreachable from stable output; it is not
blocked merely because optional promotion studies are incomplete. The following conditions become
mandatory before the named sieve or scout can be enabled or promoted:

- exhaustive small streams prove every key with fragment support at least two is in the two-hit
  candidate superset;
- retained `(full_key, exact_support)` streams and all shared downstream scientific artifacts are
  byte-identical with sieve on/off for eligible configurations;
- support one, occurrence mode, or incompatible rules bypass the sieve;
- tiny, nearly full, all-ones, and collision-crafted filters may increase work but cannot alter exact
  output;
- two fragments carrying one key on different extraction workers are promoted by the ordered
  coordinator; shard-local two-hit filters plus OR are a committed forbidden-implementation
  counterexample;
- Mechanism-A state is same-process only and cannot load a partial serialized first pass; scout-filter
  corruption, pass/config/spool mismatch, and interrupted recount fail or use the no-sieve
  oracle without partial output; and
- scout on/off complete runs have identical exact artifacts, preserve mates, service the residual
  queue, and label resource-stopped work incomplete.

Any changed retained key, count, graph edge, sequence, exact evidence field, or false uniqueness is an
automatic rejection of the probabilistic mechanism.

## Comparator validation

The required set is the frozen Virustic2 ancestor, VeritAsm, SPAdes, metaSPAdes, MEGAHIT, and SKESA.
Exact candidate versions and unresolved freeze blockers are in `docs/COMPETITOR_MATRIX.md`.
Virustic2 uses commit `b211915fc7cce82629766b77024463c6cabcc749`; the other executables are **not
frozen or admitted yet**.

Rules:

1. All tools receive the same biological reads. A deterministic format adapter is versioned and both
   sides are hashed when a tool cannot accept the canonical input.
2. Primary VeritAsm uses one predeclared `k=31`, Q20, fragment-instance support, and `min_support=2`.
   `retain_all` and independent k values 21 and 51 are separate sensitivity runs and are never selected
   after truth inspection.
3. Virustic2 receives the closest explicit fixed-k/support/QC configuration. Differences in parser,
   graph, pair, and filtering semantics are recorded rather than treated as matched.
4. Ordinary SPAdes is primary for isolate-like and paired-repeat cases. metaSPAdes is a distinct
   primary comparator for high-background and deliberate-mixture cases. Modes are never pooled.
5. MEGAHIT default is retained on every applicable primary case; `meta-sensitive` is a separate
   predeclared run on high-background/mixture cases.
6. SKESA is a primary conservative isolate comparator after its source/version/license discrepancy is
   resolved. It is not treated as equivalent on complex mixtures.
7. Contigs are the primary sequence estimand. Scaffolds and any gap-bearing outputs are scored in a
   separate table. Corrected reads, graphs, paths, logs, and parameters are retained.
8. A comparator's graph format or header field is not assigned VeritAsm semantics. Evaluator-generated
   SAM/BAM/PAF/VCF is attributed to the evaluator.
9. No best-of parameter grid appears as one tool result. All runs, including worse alternates and
   failures, remain visible.
10. Comparator source/archive/image, license files, build transcript, executable digest, CPU features,
    complete command, raw output, logs, and evaluator inputs/outputs are retained.

IDBA-UD, ABySS, Minia, and Velvet remain diagnostic controls on predeclared subsets and do not alter
the primary ranking. No one-dimensional ranking is reported.

## Platform, packaging, and engineering gates

The release-review package requires retained evidence for every row. Status words in this ledger
have deliberately narrow meanings:

- **PASS** means the exact command or bounded check named in that row completed successfully against
  the identified source snapshot. It does not imply that a later tree, broader platform, or scientific gate
  passed.
- **PARTIAL** means useful evidence exists, but at least one required part of the row is unexecuted,
  unresolved, or not yet repeated from the final clean extraction. A PARTIAL row remains open.
- **FAIL** means a required check executed and failed. No failure is converted to NOT RUN or hidden by
  reporting a narrower successful command.
- **NOT RUN** means the required check did not complete. An attempted check blocked by the environment
  is still NOT RUN, with the blocker recorded.

### Current identity and superseded bounded evidence

An earlier superseded candidate was gated on 2026-09-03 on Linux 6.18.35 x86-64, on an Intel Xeon
Platinum 8573C host, with Rust 1.85 and then-current stable Rust 1.98.0. Its recorded dependency-input
identities were `Cargo.lock` SHA-256
`ac77827391e07e0afa34428e887274e74862ac6617530e65f5a43067ebb9f8e7`, `Cargo.toml` SHA-256
`9f4c5461cff63620372b8adf40363d5d3740ac0bcd940a1afa08deb3db953b88`, `fuzz/Cargo.lock`
SHA-256 `55aa3c68ba2632873b13ca86cff54678185b3981f20dc2fa397ec4af2a52895e`, and
`fuzz/Cargo.toml` SHA-256
`3ae242327ac7718d7e581c125dca57cf26237028ea1afc38aa4a25fdef42afee`. Those are historical audit
inputs, not the current files. The current exact inputs are `Cargo.toml`
`7caff77a75243be3af55446c8b0f993ebddad150bf22863c62e7d244f17511d5`, `Cargo.lock`
`a6472637042f90c430529a0a9603c4227d9be0d5d9eab1dceab6559e8bdd9b6b`, `fuzz/Cargo.toml`
`56edb62a8116208100d07a9450ca4507e9445dcd284d8e8fa06b6ec33c5ab9cc`, and `fuzz/Cargo.lock`
`6051e8c8ea93705ed7618530b53918041b35e75b5901e3f9d9cd03bb20750e02`. Their third-party resolution
is unchanged, but fresh `cargo-audit` and `cargo-deny` runs against these current hashes were **NOT
RUN**. A superseded `0.3.0-alpha.1` candidate gate recorded 305 all-target tests per toolchain;
subsequent source changes mean the full MSRV/stable gate for the current tree is **NOT RUN**. The
source review ZIP, its checksum, and independently extracted final `.crate` remain pending until the
post-freeze external release verifier runs; no final archive hash is claimed here.

| Gate | Required execution | Current status |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | **CURRENT TREE: NOT RUN** after post-gate source changes. A superseded 0.3 candidate passed on stable and Rust 1.85; final clean-ZIP rerun pending |
| Compilation check | `cargo check --locked --all-targets --all-features` | **CURRENT TREE: NOT RUN** after post-gate source changes. A superseded 0.3 candidate passed on stable and Rust 1.85; final clean-ZIP rerun pending |
| Lints | `cargo clippy --locked --all-targets --all-features -- -D warnings` | **CURRENT TREE: NOT RUN** after post-gate source changes. A superseded 0.3 candidate passed on stable and Rust 1.85; final clean-ZIP rerun pending |
| Tests | `cargo test --locked --all-targets --all-features` | **CURRENT TREE: NOT RUN** after post-gate source changes. A superseded 0.3 candidate passed 305 tests per toolchain; final clean-ZIP rerun pending |
| Documentation | `cargo test --locked --doc` and `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps` | **CURRENT TREE: NOT RUN** after post-gate source changes. The superseded 0.3 candidate passed both commands on both toolchains and discovered 0 doc tests; final clean-ZIP rerun pending |
| Release build | `cargo build --locked --release` | **CURRENT TREE: NOT RUN** after post-gate source changes. A superseded 0.3 candidate passed on stable and Rust 1.85; final clean-ZIP rerun pending |
| Package | `cargo package --locked --allow-dirty`, then build/test the produced `.crate` in a verified temporary extraction | **CURRENT TREE: NOT RUN** after post-gate source changes. Cargo packaging and built-in verification passed for a superseded 0.3 candidate under stable 1.98.1 and Rust 1.85.0; independent extraction/build/test of the final post-freeze `.crate` remains pending |
| MSRV | full locked gates with Rust 1.85 | **CURRENT TREE: NOT RUN** after post-gate source changes. The superseded 0.3 candidate passed fmt, check, clippy, 305 tests, the doc command, rustdoc, release build, and Cargo packaging. Final clean-ZIP rerun pending. This is not an all-target-OS claim: the unselected dev-only WASI lockfile path contains `wasip2` with a declared Rust 1.87 requirement |
| Stable | full locked gates with the recorded current stable toolchain | **CURRENT TREE: NOT RUN** after post-gate source changes. The superseded 0.3 candidate passed fmt, check, clippy, 305 tests, the doc command, rustdoc, release build, and Cargo packaging under Rust 1.98.1; final clean-ZIP rerun pending |
| Linux | clean x86-64 Linux build, test, transaction, fuzz smoke, and benchmark harness smoke | **PARTIAL**: a superseded candidate passed stable/MSRV gates, but the full current-tree Linux gate is **NOT RUN**. Current-candidate `cargo fuzz check`, ASan/libFuzzer, and LeakSanitizer are also **NOT RUN**. A bounded smoke passed only for historical 0.2.0-alpha.1 source commit `cdb88007f2643776b092e8379b200dfb1404ac0c`, not this tree. The raw 15-run development-comparison directory is unavailable, so its reported rows are not execution evidence. Final clean extraction, sustained fuzzing, and the complete injected-failure matrix remain open |
| macOS | clean Apple Silicon macOS build, test, no-replace transaction, and deterministic golden comparison | **NOT RUN**. CI configuration is present but is not execution evidence |
| Dependencies | pinned `cargo audit` plus `cargo deny check` for advisories, licenses, sources, and bans; manual build-script/native/unsafe/network review | **PARTIAL (release blocker)**: the clean `cargo-audit` 0.22.2 and `cargo-deny` 0.20.2 records apply only to the historical four hashes and RustSec commit `5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5`. The third-party resolution is unchanged, but fresh scans of the current hashes were **NOT RUN**. Manual review was bounded, not a soundness proof, and fuzz/native redistribution remains blocked by the unresolved `libfuzzer-sys 0.4.13` notice mismatch documented in `docs/DEPENDENCY_REVIEW.md` |
| Fuzzing | pinned `cargo-fuzz` sanitizer smoke for every required target, with corpus and result records | **NOT RUN for 0.3.0-alpha.1.** Targets and seed corpora exist, but current-candidate `cargo fuzz check`, ASan/libFuzzer, and LeakSanitizer did not execute. Historical 0.2.0-alpha.1 commit `cdb88007f2643776b092e8379b200dfb1404ac0c` passed a 10-second-per-target ASan/libFuzzer smoke with leak detection disabled; that result is disclosed only as prior evidence and does not qualify this tree. Sustained and platform-complete campaigns remain open |
| Runtime isolation | core executable succeeds with network denied; static/runtime inspection finds no network client, subprocess, plugin, or download path | **NOT RUN** as a complete current-candidate gate: static source inspection found no network-client or subprocess path. An earlier candidate binary's linked-library inspection found only ordinary local runtime libraries, but that prior-snapshot observation is not evidence for the current candidate working tree. `strace` was denied by the environment's ptrace policy and `bwrap --unshare-net` was denied by its namespace policy, so no network-denied dynamic run completed |
| Clean extraction | source archive excludes target/temp/results, checksum verifies, then all MSRV/current-stable package gates run from extraction | **NOT RUN** for the current tree, final post-freeze archive, and independently extracted `.crate`; the superseded candidate's `cargo package` success is not a substitute |
| Output safety | late-failure, pre-existing destination, and concurrent no-replace suites | **PARTIAL**: checked-in output-safety and no-replace tests passed in the superseded 0.3 candidate suite; the current tree and final clean ZIP are **NOT RUN**, and the complete power/signal/disk-full/concurrent noncooperator matrix listed above was not executed |
| Public scientific datasets | admitted unmodified and semi-synthetic public datasets with frozen accessions, checksums, truth/reference policy, and raw results | **NOT RUN** |
| Scientific comparators | frozen SPAdes, metaSPAdes, MEGAHIT, and SKESA runs plus the required evaluators | **NOT RUN**; the freeze blockers in `docs/COMPETITOR_MATRIX.md` remain unresolved |

Tool versions for `cargo-audit`, `cargo-deny`, `cargo-fuzz`, evaluators, and archive utilities are part
of the evidence. A CI configuration file is intent, not proof of a successful Linux or macOS run.

### Historical generator-v3 qualification smoke — recovery/error interpretations invalidated

After invalidating generator-v2 results for the negative-strand error described in
`docs/FINAL_REPORT.md`, the five version-3 cases were regenerated and run on the identified
2026-09-03 candidate snapshot. ADR 0015 later established that version 3 assigned Q10 to every
substitution and that evaluator v2 selected the first compatible truth target for recovery and
duplication. Consequently the reported recovery, error-case, mixture, and duplication interpretations
below are historical debugging output and are not admissible qualification evidence.
Each used replicate 0, 300 fragments, 600-base truth, 75-base reads, a 180-base paired insert where
applicable, gzip FASTQ, `k=31`, and minimum supplied-fragment support 2. For every case, independently
committed assemblies from 1 and 4 threads were byte-identical. All 20 dataset, assembly, and
evaluation checksum manifests verified.

| Case | Assembly records/bases | Unaligned records/bases | Historical selected-target fraction | Diagnostic consensus accuracy | False junctions |
|---|---:|---:|---:|---:|---:|
| `linear-se` | 1 / 592 | 0 / 0 | 0.986666 | 1.000000 | 0 |
| `linear-pe` | 1 / 590 | 0 / 0 | 0.983333 | 1.000000 | 0 |
| `circular-pe` | 1 / 630 | 0 / 0 | 1.000000 | 1.000000 | 0 |
| `mixture-pe` | 35 / 1,934 | 0 / 0 | 0.948333 | 1.000000 | 0 |
| `error-pe` | 1 / 587 | 0 / 0 | 0.978333 | 1.000000 | 0 |

These are deterministic, tiny historical cases from the package's own simulator and evaluator, not
an admitted scientific scorecard or independent accuracy benchmark. Their byte-identity and checksum
observations remain engineering/debugging evidence; their affected scientific estimands do not. The circular closed walk is
excluded from the current exact remapper, and mixture reads that span separate graph segments cannot
be represented as one unitig placement; pair-audit unavailability in those cases remains explicit.
The results establish only that generator v3 no longer created the previously observed
reverse/complement-only artifact components under these parameters. They establish no retained-error
sensitivity, mixture recovery, or duplication behavior.

### Dirty-tree 1 Mb engineering probe — not validation scorecard evidence

A binary-hash-bound development run on generated dataset `v3-linear-pe-r7-aa0465c17b74825b`
used one 1,000,000-base linear truth and 80,000 perfect PE150 fragments. Four result trees across one
and eight threads were byte-identical. The historical evaluator-v2 output described one error-free
999,949-base unitig, a selected-target truth fraction of `0.999949`, and zero false among 999,920
eligible exact-flank adjacencies; 51 terminal truth bases were absent. The selected-target fraction is
not admitted under ADR 0015. One timed run per configuration measured 29.172
seconds/238,528 KiB at one thread and 32.155 seconds/240,888 KiB at eight threads, so this fixture
showed no parallel speedup.

The assembler binary SHA-256 was
`30188cf84fdf47dda490ede8c4dfa8d573275eee1e2996af6aff9d7aea8cfa7e`, and the external probe record
SHA-256 was `573a52a06afd77f79486c1a4525b0e5a1ac84dcefa93c87d1b6cc857fdcd8c82`.
The binary came from a dirty source tree, only one timed run per setting was made, and no comparator
ran. These values are engineering evidence tied to those exact hashes, not clean-package, scorecard,
general performance, sensitivity, superiority, or adventitious-agent detection evidence. Full
provenance and limitations are in [BENCHMARK.md](BENCHMARK.md).

### Unaudited historical development summary — not validation evidence

A prior development report described a 15-process comparison (five runs in each of three modes) using
generator seed `8675309`, one 6,000-base linear truth, 5,000 perfect Q40 paired fragments, 100-base
reads, a 250-base insert, and four threads. It recorded dataset-manifest SHA-256
`f521419e94cf880343ddd90aadc6ce95c7aba7ee61777a6c6d0ef90afff86ac0`, Virustic2 commit
`b211915fc7cce82629766b77024463c6cabcc749`, and executable hashes in
[BENCHMARK.md](BENCHMARK.md).

The raw command transcript, per-run stdout/stderr, GNU `time` records, output bundles, and evaluator
JSON described by that report are not present in this handoff. The values below are therefore a
transcription of an unaudited historical summary, not retained execution evidence:

| Program/mode | Reported median wall time | Reported median peak RSS | Reported exact truth coverage |
|---|---:|---:|---:|
| Virustic2 | 0.10 s | 10,136 KiB | 5,996/6,000 bases |
| VeritAsm, remapping disabled | 0.34 s | 34,068 KiB | 5,996/6,000 bases |
| VeritAsm, remapping enabled | 0.92 s | 34,352 KiB | 5,996/6,000 bases |

The prior summary reported equal exact-substring recovery and worse VeritAsm wall time and peak RSS on
this easy fixture. Because the underlying 15-run artifacts are unavailable, this package cannot
independently establish either the reported tie or the reported resource regression. The summary is
retained only as a conservative performance-risk lead that must be rerun under the frozen benchmark
protocol; it supports no equivalence, improvement, regression, or biological-detection claim.

## Release and improvement decisions

### Release-blocking requirements

- every exact oracle, graph invariant, parser/pair regression, golden, deterministic, manifest, and
  no-clobber test passes;
- all valid baseline input capabilities are covered or an explicit accepted migration documents the
  difference;
- stable output schemas exist as shipped machine-readable files and independently parse;
- resource-limit exits are typed and commit no normal result;
- no known correctness failure is hidden by a retry, exclusion, or changed seed; and
- all engineering gates above have retained evidence on the required platforms.

Scientific benchmark quality does not become a binary product-safety claim. A known scientific
failure may remain in a research release only if it is reproduced, documented, and inside the stated
scope; a software invariant or output-safety failure blocks release.

### “Better than Virustic2” rule

VeritAsm is not demonstrably better merely because it is larger or emits more files. The final report
must separate:

- inherited capabilities that pass equivalent regression tests;
- corrected software behavior, such as pair-role rejection or whole-bundle no-replace commit, with a
  direct failing-baseline/passing-VeritAsm test;
- new evidence fields whose values match an independent oracle;
- dataset-specific reconstruction improvements with named metrics and regressions; and
- unresolved, experimental, failed, or proposed work.

A broad superiority statement is prohibited. A narrow improvement statement is permitted only when
its complete evidence package is retained and all unfavorable results in the same pre-registered
family are shown.

## Validation references

- GAGE-B: <https://doi.org/10.1093/bioinformatics/btt273>
- QUAST: <https://doi.org/10.1093/bioinformatics/btt086>
- ALE: <https://doi.org/10.1093/bioinformatics/bts723>
- Merqury: <https://doi.org/10.1186/s13059-020-02134-9>
- CAMI I: <https://doi.org/10.1038/nmeth.4458>
- CAMI II: <https://doi.org/10.1038/s41592-022-01431-4>

These methods inform metric design. They do not validate this implementation, and exact evaluator
versions/commands must still be frozen.
