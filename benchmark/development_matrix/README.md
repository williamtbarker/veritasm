# Frozen small development matrix

This directory contains a Linux-only, two-phase harness for a deliberately
small, truth-known **development** comparison. The scored matrix has not run
until a retained `freeze/plan.json` is reviewed and its SHA-256 is supplied to
the separate `execute` phase. Preparing the plan generates inputs and performs
non-scored tool smoke tests; it does not execute a scored assembly cell.

This is not a qualification benchmark. It cannot support organism detection,
absence, limit-of-detection, production, clinical, or universal-superiority
claims.

## Predeclared cases

`matrix.tsv` includes every generator-v4 built-in case once, at replicate 0.
All cases use 200 supplied fragments, a 2,000-base truth, 100-base reads, a
250-base insert, and plain FASTQ. The seed is the documented SHA-256 derivation
from case and replicate and is validated before preparation.

| Case | Input | Substitution ppm | Independent Q10 ppm | Interpretation boundary |
|---|---:|---:|---:|---|
| `linear-se` | SE | 0 | 0 | Small linear mechanism case |
| `linear-pe` | PE | 0 | 0 | Small paired linear mechanism case |
| `circular-pe` | PE | 0 | 0 | Generator circular-topology mechanism case |
| `mixture-pe` | PE | 0 | 0 | Generator mixture mechanism case |
| `error-pe` | PE | 20,000 | 20,000 | Independent substitution/quality streams |
| `qc-censoring-control` | PE | 20,000 | 0 | Linked low-quality substitutions; QC mechanism control only |

These uniform-random, tiny truths do not represent instruments, repeats, host
background, real abundance distributions, or large input. There is one seed
per case, so uncertainty across truth sequences is not estimated.

## Frozen arms

Every scored assembly arm has two serial repetitions. No result is selected
after evaluation.

| Arm | Frozen settings | Primary FASTA | Important mismatch |
|---|---|---|---|
| VeritAsm stable fixed-k | k=31; thresholded; supplied-fragment-instance support >=2; Q20; remap off; 1 thread; 512 MiB modelled budget | `unitigs.fasta` | Graph unitigs; no post-resolution contig stage |
| Experimental diversity portfolio | k=21,31,51; inclusive supplied-fragment-instance support >=2; Q20; 1 thread; 512 MiB modelled budget | `contigs.fasta` | Concatenated child-scoped segments can repeat exact sequence across k |
| Experimental exact-agreement presentation | Same children; exact agreement across distinct k | `contigs.fasta` | Presentation filter can be empty; does not infer or phase consensus |
| Virustic2 `b211915…` | k=31; fragment support >=2; Q20; tip length 0; minimum contig length 0; 1 thread | `assembly.fasta` | Different graph, evidence, and resource contracts |
| MEGAHIT 1.2.9 | k=21,31,51; minimum count 2; 1 thread; 1,073,741,824-byte control; no HW acceleration; minimum contig length 0 | `final.contigs.fa` | Iterative cleaning, mercy, and local-assembly semantics; count is not VeritAsm fragment support |
| SPAdes 4.3.0 | Normal full pipeline; k=21,31,51; 1 thread; 1 GB; Phred+33 | `contigs.fasta` | BayesHammer and graph/repeat transformations; no semantically exact minimum-contig-length control |
| Multi-k k31 diagnostic | Exact extraction of authenticated k31 child segments from the diversity bundle | `assembly.fasta` | Derived diagnostic; no independent runtime and not a separately optimized arm |

The experimental diversity and exact-agreement products are evaluated as
distinct, non-rank-equivalent presentations. They are never collapsed into a
“whichever scores best” row. The adapter validates the canonical experimental
segment header before extracting k31.

VeritAsm stable and Virustic2 also receive one untimed two-thread run per case
for cross-thread comparison. An expected-exit-2 multi-k two-thread request is
retained as a contract-negative result. MEGAHIT and SPAdes receive serial-repeat
FASTA comparisons, not a cross-thread test.

## Comparator identities

The harness requires caller-supplied official release archives and rejects any
hash mismatch:

- [MEGAHIT 1.2.9 Linux x86-64 static archive](https://github.com/voutcn/megahit/releases/download/v1.2.9/MEGAHIT-1.2.9-Linux-x86_64-static.tar.gz):
  `7e5710f62b0743471c5d6938ebc28132dcea2104ccdbeafb4f2ca8dbb47728d3`
- [SPAdes 4.3.0 Linux archive](https://github.com/ablab/spades/releases/download/v4.3.0/SPAdes-4.3.0-Linux.tar.gz):
  `e88a8c533c8614dd4b7c5788cfcd46427848a0575267f97c690a75fd2a343034`

The plan records and verifies the invoked wrappers and principal cores in
addition to the complete archive identities. Preparation reruns each official
`--test` with the frozen matrix controls. Those are smoke tests, not scored
cells. Virustic2 must be a clean checkout at exactly
`b211915fc7cce82629766b77024463c6cabcc749`.

## Measurement contract

GNU `/usr/bin/time` is unavailable in the execution environment. The harness
therefore compiles the retained `wait4_measure.c` with `/usr/bin/cc` using
`-std=c11 -O2 -Wall -Wextra -Werror -pedantic`. It records monotonic wall time,
child user/system CPU, Linux `ru_maxrss` in KiB, exit disposition, and timeout
state. Source, compiler identity, and compiled-wrapper identity are frozen.

For wrapper-driven comparators, Linux child accounting can accumulate
descendant CPU after those descendants are reaped, while `ru_maxrss` is a
maximum rather than concurrent process-tree RSS. It must not be presented as a
perfectly equivalent memory contract across algorithms. The C wrapper is a
benchmark-only measurement dependency, never a VeritAsm runtime dependency.
Python is required by the two third-party wrappers only.

The harness freezes `LC_ALL=C`, `TZ=UTC`, `PYTHONHASHSEED=0`, and common
OpenMP/BLAS/vecLib thread controls at one thread. The full effective `PATH`,
tool identities, CPU description, and a second execution-time environment
record are retained; these controls still do not make dissimilar algorithms
perform identical work.

## Reproduction

Required Linux host tools are Bash, GNU coreutils/findutils, `awk`, `git`,
`jq`, `lscpu`, `tar`, `/usr/bin/cc`, Python for comparator wrappers, and the
repository's pinned Rust toolchain/Cargo. Start with the focused checks:

```console
bash benchmark/development_matrix/self_test.sh
bash benchmark/development_matrix/run.sh check
```

After the VeritAsm source is committed and completely clean, prepare a new
direct child of `/tmp`:

```console
bash benchmark/development_matrix/run.sh prepare \
  /tmp/veritasm-development-matrix.REVIEW \
  /tmp/virustic2-b211 \
  /tmp/MEGAHIT-1.2.9-Linux-x86_64-static.tar.gz \
  /tmp/SPAdes-4.3.0-Linux.tar.gz
```

`prepare` prints the immutable plan SHA-256 and `assembly_cells_not_run=true`.
Review `freeze/plan.json`, `freeze/commands.jsonl`, source/binary/input hashes,
and the build/smoke logs. Only then explicitly execute that exact plan:

```console
plan_sha=$(awk '{print $1}' \
  /tmp/veritasm-development-matrix.REVIEW/freeze/plan.sha256)
bash benchmark/development_matrix/run.sh execute \
  /tmp/veritasm-development-matrix.REVIEW "$plan_sha"
```

The harness refuses reuse, refuses a dirty or changed source tree, validates
187 unique predeclared commands, and preserves failures rather than rerunning
or replacing outputs. Bulky datasets, comparator distributions, builds, and
raw results remain under that one `/tmp` root and are not source-package
contents.

## Retained evidence

The final evidence tree contains the frozen plan and commands, input tree
manifests, exact output/log/metric hashes, raw evaluator-v4 results,
per-molecule recovery rows, command failures/skips, repeat and thread
determinism comparisons, and a canonical artifact manifest/root. Evaluator-v4
is run development-unbound because these generated inputs have no independently
supplied external content root. The compact checked-in result summary is only
written after an authorized frozen execution and must disclose every failure
and regression.
