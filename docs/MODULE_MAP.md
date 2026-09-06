# Module map

The package has one Rust library and four auto-discovered/declared executables.
All declared library modules compile in the default build; there are no hidden
feature exclusions. Unit and integration tests cover these modules to differing
degrees. A compiled module is not a qualified assembly capability.

| Source | Role and integration |
| --- | --- |
| `src/main.rs`, `src/pipeline.rs` | Fixed-k CLI and orchestration |
| `input`, `fastx`, `spool` | Input validation, parsing, immutable replay |
| `dna`, `count`, `graph`, `compact`, `transform` | Fixed-k representation, counting, topology and accounting |
| `audit`, `indexed_mapper`, `pairs` | Construction-read placement and pair observations |
| `bundle`, `report` | Transactional artifacts, integrity checks, offline rendering |
| `config`, `error`, `model` | Shared contracts and typed failures |
| `bloom`, `library_model` | Compiled experimental substrates; outside fixed-k assembly policy |
| `experimental::{wide_kmer, partitioned_dbg, external_run, external_reduce, spool_external}` | Compiled experimental data handling and representation |
| `experimental::{retention, compacted_dbg, transition_witness, evidence_reconstruction}` | Compiled experimental evidence/reconstruction chain |
| `experimental::{authenticated_pair_graph, pair_mapper, pair_path}` | Compiled experimental paired-read evidence |
| `experimental::{multik, multik_pipeline, multik_bundle}`; `src/bin/veritasm-multik.rs` | Integrated experimental multi-k executable and reporting |
| `experimental::{external_cdbg, quality_correction}` | Compiled research substrates; incomplete production integration |
| `src/validation/`; `src/bin/veritasm-{simulate,evaluate}.rs` | Separate qualification tooling; not invoked by fixed-k assembly |
| `src/experimental/witnessed_recompaction.rs` | Preserved, undeclared work in progress; not compiled or tested |
| `tests/`, `examples/`, `schema/` | Integration checks, small fixtures, machine contracts |
| `fuzz/` | Separate Cargo workspace; manual fuzz workflow, outside ordinary root Cargo tests |
| `benchmark/development_matrix/` | Preserved development comparison harness; scored comparisons deferred |

See the [development roadmap](RESUME.md) for the remaining integration work.
