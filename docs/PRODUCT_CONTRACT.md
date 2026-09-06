# VeritAsm fixed-k assembly contract v0.1

Status: implemented normative contract for the unreleased fixed-k review candidate.

Package `0.3.0-alpha.1` implements this still-narrow contract. It promotes the fixed-q15 indexed
exact mapper only inside the stable construction-read audit; the wide-k, partitioned-graph,
library-model, and validation modules remain disconnected experimental or qualification substrates.

## Purpose

VeritAsm is a database-free, evidence-first assembler for Illumina-like single-end and paired-end
short DNA reads. It reconstructs exact de Bruijn-graph unitigs and reports the read, fragment, graph,
and pair evidence that supports them. The first bounded evaluation domain is synthetic constructs,
plasmids, microbial isolates, organellar DNA, and small or moderately complex mixtures.

Low-abundance reconstruction in a high-background library is an important research-evaluation and
method-development scenario. Adventitious-agent workflow development is one such application.
VeritAsm is not itself an organism detector, taxonomic classifier, sterility test, validated assay, or
product-disposition tool.

In this repository, **exact** means full encoded sequence identity and checked integer counting under
the recorded parser, base-quality, support, graph, and mapping rules. It does not mean that a
reconstruction is the true biological sequence, that a placement is the true origin, that inputs are
unbiased, or that all sequence in a sample was observed. Construction-read remapping is an internal
consistency check using the same supplied reads, not independent validation.

## 0.1 scientific primitive

One assembly run at one k constructs an exact canonical k-mer multiset from every accepted input
fragment, retains keys under an explicit support rule whose unit is part of the configuration, builds
the corresponding exact compact de Bruijn graph, and emits conservative graph segments. Those
segments are maximal only between the declared degree, self-complemental-node, and incident
self-complemental-edge boundaries; this fixed-point rule deliberately stops some otherwise
non-branching walks to avoid consuming one canonical observation twice. It does not choose one path
through an unresolved branch.

The stable 0.1 command accepts exactly one k. Users can run several independent result directories to
study the k tradeoff. A bundled multi-k sweep and concordance schema remain proposed; no derived contig
is reused as read evidence.

## Supported inputs

- plain or content-detected gzip FASTA and FASTQ;
- single-end reads, with every lane single-end in one run;
- strictly synchronized paired-end reads, with every lane paired-end in one run;
- ordered multiple lanes supplied as repeated lane pairs or repeated single-end files; mixing
  single-end and paired-end lanes in one run is `configuration_unsupported_combination`;
- standard input in exactly one logical input role;
- IUPAC nucleotide symbols, with every window containing ambiguity excluded and counted;
- FASTQ Phred+33 quality values from Q0 through Q93;
- `3 <= k <= 63`, including even k, using exact two-bit `u128` identities.

Input is parsed once into an immutable, versioned, checksummed fragment spool. All scientific passes
consume that spool so a run cannot mix observations from input files that changed between passes.

## Support semantics

The default support unit is a **fragment instance**: a canonical k-mer contributes at most once per
single-end record or synchronized read pair. This is not a unique molecule count because PCR
duplicates and duplicated input records remain distinct instances.

An explicit occurrence mode counts every accepted k-mer window. Neither quantity is called coverage,
depth, abundance, or confidence. Counters are checked `u64`; overflow is a typed fatal error.

## Profiles

Profiles are immutable parameter presets recorded in `run.json`. The configuration is typed as
`support_unit = supplied_fragment_instance | accepted_window_occurrence` plus a mode-neutral
`min_support`. Every serialized threshold and result repeats the unit.

- `retain_all`: retains every observed accepted k-mer (`min_support = 1` in the selected unit) and performs no
  deleting topology transformation. The name makes no analytical-sensitivity claim.
- `thresholded`: uses the declared minimum support in the selected unit. Every removed key and support
  mass is counted, and a digest of the sorted decision set is recorded in the transformation journal.
- `custom`: every effective parameter is serialized; no hidden adaptive threshold is permitted.

Profiles do not imply consensus or biological truth. Bubbles, repeats, and other unresolved branches
remain graph structure.

## Pair evidence

Read pairs are one supplied fragment instance for support counting. After unitig construction,
zero-mismatch remapping against emitted linear unitigs may produce oriented unitig-end observations
when identifier, mate role, placement enumeration, and endpoint rules are satisfied. The table retains
input-lane ordinal, R1/R2 role, strand, selected end, and exact end distance. Matching observations
from different lanes are never pooled. The evidence makes no library-orientation, insert-span, gap,
or adjacency conclusion. Pair observations never concatenate unitigs, invent sequence, or turn a
graph branch into a haplotype in 0.1. Every non-observation follows one explicit global and per-lane
summary state.

## Outputs

A successful run commits one result directory containing:

- `unitigs.fasta` — deterministic canonical unitig sequences;
- `assembly.gfa` — a conservative GFA 1.0 subset containing segments and exact retained-graph adjacencies;
- `unitig_evidence.tsv` — defined per-unitig graph and same-construction-read remapping fields;
- `pair_links.tsv` — lane-isolated exact unique-placement oriented pair-link observations;
- `pair_audit_summary.tsv` — mutually exclusive pair eligibility, placement, and exclusion counts;
- `transform_summary.tsv` — every configured graph transformation and its measured effect;
- `run.json` — schema version, inputs, effective parameters, counts, warnings, and status;
- `report.html` — self-contained rendering of the same machine-readable evidence;
- `schema/` — exact JSON and TSV schema descriptions;
- `manifest.sha256` — checksums of every other committed artifact.

No committed artifact contains wall-clock time, thread count, temporary paths, unordered-map order,
host names, or timestamps. Stable 0.1 writes execution telemetry only to stderr.

## Resource and failure contract

Raw transport bytes, concatenated gzip members, record length, header length, quality length,
scan-batch allocation estimates, spool bytes, temporary bytes, partition count, and retained graph
state have explicit controls. A detected limit excess
returns a typed error; VeritAsm never deliberately truncates a record, silently samples reads, or
emits a normal successful bundle from an incomplete run. These controls are not a hard process-RSS
cap: allocator bookkeeping, runtime/library state, stack guard pages, and the calling thread's stack
remain outside some estimates. Worker stacks use a fixed 1 MiB request and a separately admitted
aggregate ceiling, as specified in `ARCHITECTURE.md` and `docs/CONFIGURATION.md`.

Before any source is opened or work directory is created, an exact cooperative destination lease is
acquired and held through publication. All output is written and validated in a same-filesystem
sibling staging directory. Commit occurs with the platform no-replace primitive only
when the destination does not exist and all artifacts and checksums are complete. Any pre-existing
destination remains byte-for-byte unchanged after failure. Contract v0.1 has no overwrite flag and no
streaming-to-stdout assembly mode; exporting a committed artifact is a separate ordinary file-copy
operation.

Static first-party source inspection finds no network client, download, plugin, or subprocess path,
and ordinary execution requires no Python, Java, Conda, database, or separate runtime. Dynamic
network-denied observation remains an open qualification gate, so this design fact is not presented
as proof over every compiled dependency and platform path.

## Explicit non-goals for 0.1

- organism identity, taxonomy, pathogenicity, infectivity, or viability;
- positive/negative calls, limits of detection, sample sterility, or product disposition;
- a finished chromosome/genome claim from graph length or topology;
- global haplotype or strain reconstruction;
- reference-guided correction, polishing, host subtraction, or variant calling;
- long-read, linked-read, or Hi-C assembly;
- a bundled multi-k sweep or cross-k reconciliation;
- deleting tip or bubble transformations;
- forced scaffolds or sequence joins from paired reads;
- probabilistic k-mer identity, probabilistic graph membership, or Bloom-only evidence;
- claims of extreme accuracy, general superiority, production readiness, or bounded operation beyond
  measured and published conditions.

## Acceptance boundary

The 0.1 implementation is acceptable only when its exact results match small exhaustive oracles,
all inherited valid-input behavior has regression coverage, malformed and adversarial inputs fail
without output clobbering, complete core bundles are byte-identical across supported thread counts,
and pre-registered benchmarks publish both successes and failures. Performance or reconstruction
improvement over Virustic2 must be stated only for the exact datasets and metrics that demonstrate it.
