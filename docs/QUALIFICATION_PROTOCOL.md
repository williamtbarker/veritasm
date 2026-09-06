# Qualification protocol for a supported research release

- Protocol ID: `veritasm-supported-release-v1`
- Frozen: 2026-09-04
- Status: protocol only; no release has passed it
- Product boundary: database-free de novo reconstruction from Illumina-like single-end and
  ordinary paired-end short DNA/cDNA reads

This protocol defines what would be required to describe a VeritAsm release as a **supported research
assembler within a measured operating envelope**. It does not qualify an organism-detection assay,
an adventitious-agent test, a clinical device, a sterility method, or a manufacturing disposition
workflow. Passing software and assembly gates cannot establish those claims.

The normative metric definitions are in [`../VALIDATION.md`](../VALIDATION.md), dataset admission is
in [`DATASET_CATALOG.md`](DATASET_CATALOG.md), comparator policy is in
[`COMPETITOR_MATRIX.md`](COMPETITOR_MATRIX.md), and execution controls are in
[`../BENCHMARK.md`](../BENCHMARK.md). This document is the promotion checklist tying them together.

## Release strata

Development, qualification, and regression data are disjoint by immutable object digest. A dataset
first inspected during algorithm selection can never later become qualification evidence for that
algorithm version.

| Stratum | Permitted use | Prohibited use |
|---|---|---|
| Development | Debugging, parameter design, oracle construction, profiling, and kill-test creation | Release accuracy claims or threshold selection after viewing qualification truth |
| Qualification | One preregistered execution of the frozen candidate and comparators | Algorithm edits, case removal, or parameter changes after any score is viewed |
| Regression | Prevent recurrence of disclosed failures after qualification | Replacing failed qualification cells or widening the supported domain |

Synthetic replicates use case IDs and seeds derived before generation as
`SHA-256(protocol_id || case_definition || replicate_ordinal)`. The manifest records the full digest,
generator commit and binary digest, RNG contract, generated-object digests, and truth objects. Seeds
are never selected by outcome.

Every admitted generated dataset must also have an externally retained
`dataset_content_root_sha256`, defined as SHA-256 of the exact canonical `manifest.sha256` bytes.
Qualification runs must pass that preregistered or independently regenerated value through
`--expected-dataset-content-root-sha256`. A result whose `inputs.dataset_binding.mode` is
`development_unbound`, whose admission state is not `eligible_externally_bound`, or whose expected
root does not equal its observed root is rejected before scorecard aggregation. Reading the expected
digest from the dataset under evaluation in the same workflow is not independent admission.

## Proposed initial operating envelope

These values are hypotheses to test, not current support claims:

- Phred+33 Illumina-like FASTA/FASTQ, plain or content-detected gzip;
- one or more homogeneous single-end lanes, or one or more strictly synchronized ordinary paired-end
  lanes whose roles and per-lane geometry are valid;
- read lengths 75--300 bases;
- haploid viral, bacterial, archaeal, plasmid, organellar, synthetic-construct, and small fungal
  truth objects up to a preregistered maximum total truth length;
- single isolates plus bounded two-component and low-complexity mixtures;
- Linux x86-64 and Apple Silicon macOS configurations named by exact toolchain and hardware records.

Long reads, linked reads, mate-pair libraries, Hi-C, whole diploid or polyploid eukaryotic genomes,
global strain/haplotype claims, taxonomy, and biological presence/absence calls remain outside this
envelope. A failed or absent reconstruction never establishes that sequence was absent from a sample.

## Frozen qualification matrix

Before generating or downloading inputs, a signed-off experiment manifest must instantiate every
cell below, its replicate count, factor levels, parameters, resource limits, stopping rules, and
admission status. No failed cell may be deleted because it is inconvenient or unfavorable.

| Panel | Required factors | Primary purpose |
|---|---|---|
| Exact linear synthetic | SE/PE; lengths; GC; 5x--1000x; at least three seed-derived replicates | Exact reconstruction, depth response, deterministic scaling |
| Uneven/dropout synthetic | smooth gradients, sharp dropouts, zero-depth intervals | Fragmentation and unsupported-join behavior |
| Repeat synthetic | exact/near repeats bracketing fragment span; orientation; copy count | Conservative repeat termination and qualified pair resolution |
| Two-component synthetic | divergence grid and abundance ratios through at least 99:1 | Minor-path precision/recall without global phasing claims |
| Error/artifact synthetic | substitutions, indels, quality gradients, IUPAC, adapters, duplicates, chimeras | Cleaning tradeoffs and evidence accounting |
| Closed-topology controls | true closed molecules plus linear concatemer/terminal-repeat confounders | Graph-closure reporting without molecular-circularity claims |
| High-background semi-synthetic | admitted unmodified background plus exact pair-preserving spikes | Low-abundance reconstruction and false-junction behavior in named backgrounds |
| Public isolate panel | checksum-frozen FDA-ARGOS or comparable truth-qualified isolates | Operational performance outside generator assumptions |
| Defined community panel | checksum-frozen MBARC-26, Zymo, or ATCC material with truth caveats | Mixture/background robustness |
| Original application panel | segmented RNA-virus, non-segmented RNA-virus, and DNA-virus read sets | Preserve the original validation obligation as a separate application stratum |

Collection names are discovery leads until exact objects are admitted. Repository labels and
references are not automatically biological truth. Blank and negative controls are not absence truth.

## Comparators and parameter firewall

Every applicable primary cell runs the frozen Virustic2 ancestor, VeritAsm, SPAdes, SKESA, and
MEGAHIT or metaSPAdes/MEGAHIT where the dataset class calls for metagenomic behavior. Diagnostic graph
constructors such as GGCAT do not replace end-to-end assemblers. Comparator installation failures,
timeouts, crashes, invalid output, and nondeterminism remain result rows.

VeritAsm parameters are frozen for a dataset class before qualification truth is scored. Sensitivity
analyses are separately named complete matrix runs, never a best-of selection per sample. Comparator
defaults and any alternate profiles are similarly frozen. Input adapters may only perform declared,
checksum-recorded representation changes; they may not trim, correct, normalize, filter, resample, or
repair pairs.

## Scientific acceptance gates

Threshold values are filled in and approved before qualification. `TBD` blocks promotion; it is not a
wildcard.

| Gate | Required statistic | Promotion rule |
|---|---|---|
| Base correctness | aligned errors, consensus QV/lower bound, unaligned bases | Noninferiority margin `TBD`, satisfied on every mandatory class with a preregistered interval method |
| Truth recovery | total and truth-unique genome fraction | Noninferiority margin `TBD`; declared low-input/dropout failures remain visible |
| Structural correctness | false junctions, target/background chimeras, evaluator misassemblies | Zero false pair-created joins in unidentifiable repeat controls; broader margin `TBD` |
| Duplication | aligned duplication ratio and duplicated truth intervals | Margin `TBD`; never trade unlimited duplication for genome fraction |
| Diversity | local labelled minor-path precision/recall | Thresholds `TBD` by abundance/divergence cell; no global haplotype claim |
| Abstention | unresolved branches/repeats and excluded evidence | Every unresolved case represented without fabricated FASTA adjacency |
| Evidence fidelity | independent small oracle versus emitted counts, links, mappings, and transformations | Zero discrepancies |
| Ancestor compatibility | inherited valid inputs plus explicit migration table | Zero unexplained regressions; every intentional difference documented |

There is no scalar leaderboard. A release fails if it meets an average threshold by hiding a fatal
subgroup, false junction, omitted result, evaluator failure, or resource failure.

## Engineering acceptance gates

All gates apply to the exact candidate commit and deterministic source archive:

1. Formatting, all-target/all-feature check, warnings-denied Clippy, tests, warnings-denied rustdoc,
   release build, and `cargo package` pass under Rust 1.85 and the frozen current stable release.
2. Every fuzz target compiles; a sustained sanitizer campaign of preregistered duration passes on
   Linux, and every crash corpus is retained. Unsupported sanitizer/platform combinations are named,
   never silently omitted.
3. Complete scientific bundles are byte-identical across 1, 2, 4, and 8 threads and repeated
   processes. Any normalized comparator comparison is reported separately from raw bytes.
4. Input mutation, malformed FASTX, corrupt/truncated/multi-member gzip, mate mismatch/reorder,
   integer/resource boundaries, disk-full/write/flush/sync failures, interruption points, writer
   races, and destination-appearance races fail without modifying a pre-existing result.
5. Peak RSS, temporary bytes, open files, wall time, and output bytes stay within every preregistered
   cell's envelope. An explicit resource refusal is a failure row, not a successful truncated run.
6. Dependency advisories, licenses, source provenance, and the root and fuzz lock graphs pass the
   frozen policy. Runtime network denial is observed dynamically; no external command or optional
   database is used by the core assembler.
7. Native Apple Silicon macOS and Linux execute the platform matrix. Cross-compilation or CI
   configuration alone is not platform evidence.
8. Two independently built source ZIPs and crates are byte-identical. The clean-extraction verifier
   passes both toolchains from the archived bytes, with no build artifact, VCS metadata, credential,
   absolute development path, or undeclared file.

## Performance reporting

Performance is reported only for named hosts, datasets, commands, limits, and repetitions. Each cell
retains raw wall/user/system time, peak RSS, temporary and output bytes, process outcome, and artifact
digests. Report all values plus median and median absolute deviation. “Faster”, “lower memory”, or
“more sensitive” is permitted only beside the exact cells and metric definition that demonstrate it.

Graph-construction speed is not end-to-end assembly speed. Construction-read remapping is not
independent validation. Recovery at a planted depth is not an analytical limit of detection.

## Promotion record

A promotion record must include:

- candidate commit, source ZIP, crate, binaries, schemas, and evaluator SHA-256 values;
- compiler, dependency, host, OS, container, comparator, and dataset manifests;
- every command, result, raw artifact digest, failure, exclusion, and protocol deviation;
- a requirement-to-test-to-result traceability table;
- an independent reviewer sign-off that did not develop the candidate algorithm; and
- a claim ledger listing the exact statements admitted by the evidence and the statements still
  prohibited.

Until every mandatory row passes, the correct label is **pre-release research software**. A later
software qualification still would not validate an adventitious-agent or other biological-detection
workflow; that requires separately governed wet-lab sampling, controls, reference/classification,
decision rules, analytical performance, and intended-use evidence.
