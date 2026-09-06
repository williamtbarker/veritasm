# VeritAsm

**A Rust framework for deterministic short-read assembly and inspectable evidence.**

VeritAsm turns FASTA or FASTQ reads into conservative de Bruijn-graph unitigs,
preserves unresolved graph branches, and writes sequence, graph, support,
read-placement evidence, and an offline HTML report together. Its emphasis is
on explicit accounting, reproducible output, and failures that leave existing
results untouched.

**Status: `0.4.0-dev.1`, development paused.** The fixed-k command is implemented;
multi-k reconstruction remains experimental. Performance and reconstruction
quality have not been established on representative datasets. See
[verification results and known issues](docs/STATUS.md).

## Build and try it

Rust 1.85 or newer is required. Intended platforms are Linux and macOS.

```bash
cargo build --release --locked
./scripts/smoke.sh
```

The smoke script uses the checked-in toy fixture, verifies the output checksum
manifest, tests malformed-input rejection and protection of existing output,
then removes its temporary results. It needs Bash and ordinary Unix utilities.

To keep an example result for inspection:

```bash
mkdir -p results
target/release/veritasm assemble \
  --single examples/reads.fasta \
  --output-dir results/example \
  --k 5 --profile retain-all --min-base-quality 0
```

Open `results/example/report.html` in a browser. The output directory must be
new; choose another name if you have already run the example. The tiny fixture
is a software demonstration, not a reconstruction benchmark.

## Implemented core

- Plain or content-detected gzip FASTA/FASTQ, including concatenated gzip members.
- Single-end or synchronized paired-end input, with ordered multiple lanes.
- Exact canonical k-mer counting at one k per run, with explicit quality,
  ambiguity, and support rules.
- Deterministic graph compaction and conservative GFA output.
- Optional exact construction-read placement and lane-specific pair observations.
- Typed errors, resource admission checks, checksummed artifacts, and staged
  publication of a complete result directory without overwriting existing results.

| Output | Purpose |
| --- | --- |
| `unitigs.fasta` | Reconstructed graph segments |
| `assembly.gfa` | Segments and retained graph adjacencies |
| `unitig_evidence.tsv`, `pair_links.tsv` | Support and construction-read observations |
| `pair_audit_summary.tsv`, `transform_summary.tsv` | Accounting and exclusions |
| `run.json`, `schema/` | Effective configuration and machine-readable contracts |
| `report.html` | Self-contained report |
| `manifest.sha256` | Artifact integrity inventory |

Graph adjacency is not proof that a read or molecule traverses that adjacency.
Construction-read auditing uses the same reads that built the graph. Resource
budgets are component admission controls, not a hard process-memory guarantee.
The [product contract](docs/PRODUCT_CONTRACT.md) and
[limitations](docs/SCIENTIFIC_LIMITATIONS.md) define these boundaries.

## Experimental work

`veritasm-multik` retains independent k children and their evidence in a separate
experimental bundle. External graph construction, quality correction, pair-path
analysis, and validation tools are also present. Their implementation and
integration states differ; see the [module map](docs/MODULE_MAP.md).

One unfinished recompaction module is outside the build. The
[module map](docs/MODULE_MAP.md) identifies it and other integration boundaries.

## Roadmap to a complete release

The completion target is a documented, reproducible assembler release with a
defined supported workload, fully integrated supported features, measured resource
limits, and independently evaluated results. The milestones below are outstanding;
passing the current test suite does not complete them.

| Milestone | Work remaining | Completion criterion |
| --- | --- | --- |
| 1. Resolve known correctness issues | Recover the minimal adjacency case, classify it against the graph contract, and fix any confirmed violation. | Checked-in reproducer, documented contract decision, and passing regression tests. |
| 2. Establish the operating limits | Reproduce the paired snapshot failure; audit simultaneous memory, temporary storage and descriptor use. | The reported failure is resolved, supported limits are measured, and boundary failures leave existing output intact. |
| 3. Complete feature integration | Finish the intended multi-k and recompaction path; review external graph, correction and pair-evidence modules against the release scope. | Every supported feature is reachable, documented and tested end to end; unfinished research is explicitly outside that release. |
| 4. Qualify portability and failure handling | Run stable/MSRV checks on Linux and macOS, sustained fuzzing, dependency/license review, and interruption/storage failure tests. | A clean tagged candidate passes the platform matrix, with no unresolved release-blocking integrity or security defects. |
| 5. Validate the evaluation tools | Complete independent generator/evaluator checks, ambiguous-case tests and separation of development fixtures from held-out evaluation. | Published metrics have documented definitions and independently checked calculations; superseded results remain identified. |
| 6. Measure quality and performance | Run the frozen representative dataset and comparator matrix; profile measured bottlenecks and rerun affected checks after changes. | Reproducible results report correctness, runtime, peak memory, failures and limitations for the supported workload. |
| 7. Publish and maintain the release | Stabilize CLI/schema contracts, installation instructions, examples, migration notes, release artifacts and support policy. | A new user can install and reproduce the documented examples from a clean release; checksums, CI results and release notes are available. |

Start with the [three immediate tasks](docs/RESUME.md#next-steps). The
[completion plan](docs/RESUME.md#completion-milestones),
[validation plan](VALIDATION.md), and [benchmark protocol](BENCHMARK.md) expand
the milestones. Performance or reconstruction claims will follow measured
results for named workloads and comparators.

## Development and resuming

```bash
./scripts/verify.sh
```

The verifier runs formatting, strict Clippy, tests, documentation, a release
build, and the smoke example using the existing Cargo cache. It stops at the
first failure. CI defines Linux and Apple Silicon checks; configured CI is
separate from locally observed verification.

- [Current status](docs/STATUS.md): exact checks and remaining issues.
- [Development roadmap](docs/RESUME.md): open issues and next steps.
- [Architecture](ARCHITECTURE.md) and [module map](docs/MODULE_MAP.md).
- [Contributing](CONTRIBUTING.md) and [development benchmark harness](benchmark/development_matrix/README.md).
- [Historical reports](docs/FINAL_REPORT.md): earlier evidence and design work.

MIT licensed. Copyright William Barker. Citation metadata is in `CITATION.cff`.
