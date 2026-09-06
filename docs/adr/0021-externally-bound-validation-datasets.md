# ADR 0021: externally bound validation datasets

- Status: Accepted for qualification tooling v4
- Date: 2026-09-05
- Scope: synthetic-dataset generation, evaluation, schemas, and admitted scorecards

## Context

The generator records hashes, replays read origins and injected events, and derives a full SHA-256
parameter commitment. Those checks can detect partial corruption and incoherent edits. They cannot
authenticate a dataset against a writer that coherently replaces the reads, truth, ledgers,
parameters, hashes, and manifest. An unkeyed identifier derived only from declared parameters is not
a content trust root.

The evaluator also parsed bounded TSV snapshots into nested row vectors and retained ordered sets of
event offsets. The input byte cap did not dominate those secondary allocations. Finally, some
generator feasibility checks and portable path restrictions were not shared by the evaluator, so a
forged declaration could describe a case the generator itself would have rejected.

## Decision

1. Keep the deterministic parameter identifier, but name and document it only as a parameter
   commitment. Add `dataset_content_root_sha256`, defined as SHA-256 of the exact canonical
   `manifest.sha256` bytes. The manifest lists `dataset.json` and every declared artifact, but not
   itself; the content root is not stored inside `dataset.json`, avoiding a circular commitment.
2. Evaluation accepts `--expected-dataset-content-root-sha256` supplied outside the dataset
   directory. It first requires the manifest to be the exact canonical, sorted, complete
   `path + digest` inventory, then compares the SHA-256 of those exact opened bytes with the external
   expectation before semantic artifacts are consumed. The expected digest is committed into the
   result. When absent, output is explicitly marked
   `development_unbound` and is ineligible for an admitted qualification scorecard.
3. The qualification protocol and scorecard tooling require externally bound mode. The expected
   digest must come from a preregistration record or an independently regenerated dataset, not from
   the dataset being evaluated in the same command.
4. Generator and evaluator call one shared parameter/feasibility validator, including conservative
   artifact-size bounds. A parameter tuple rejected by generation cannot be accepted as a declared
   generator-v4 dataset.
5. Ledger validation is single-pass over the owned bounded byte snapshot. It may retain only an
   explicitly admitted compact replay state; nested row vectors and per-event tree nodes are not
   permitted. Emission order is part of the format: error and quality cursors advance together by
   `(read rank, offset)` while each origin/read is replayed, so the QC-linked equality check needs no
   per-event set or bit vector.
6. Dataset-relative paths use one portable canonical ASCII grammar shared by Rust validation and
   JSON Schema. Generated roles have their exact expected paths; arbitrary control characters,
   backslashes, empty components, dot components, absolute paths, and platform-dependent aliases are
   rejected.
7. Documentation states the actual number and purpose of integrity and semantic reads. “Opened once”
   is not used unless descriptor ownership and the implementation establish it literally.

## Required falsification tests

- Coherently rewrite a generated dataset and refresh every internal checksum. Externally bound
  evaluation must reject it because the expected manifest digest is unchanged.
- Changing the expected digest, omitting it, using uppercase/noncanonical hex, or introducing
  manifest trailing bytes produces a typed and schema-valid trust state or a typed failure as
  specified.
- Every generator parameter mutation either remains generator-feasible in both paths or is rejected
  by both paths before evaluation work.
- Near-cap ledgers demonstrate that secondary parser/replay allocation is admitted independently of
  the input snapshot, with limit-minus-one/exact/plus-one tests.
- Runtime path validation and JSON Schema accept and reject the same corpus on Linux and macOS.
- Result validation recomputes and reconciles parameter identity, content-root identity, binding
  mode, and expected digest.

## Consequences

Development evaluation remains convenient but cannot be presented as independently anchored truth.
Existing generator-v4 datasets gain a new result trust state and may require regeneration if their
path grammar or feasibility declaration is noncanonical. This strengthens provenance; it does not
make the simulator biologically realistic or validate assembler sensitivity.

## Rejected alternatives

- Trust `dataset_id`: it commits declared parameters, not content.
- Read the expected digest from a file inside the same dataset directory: a coherent rewriter can
  replace both.
- Call internal semantic consistency “authentication”: SHA-256 without an external trust root does
  not establish authorship or preregistration.
- Raise the ledger byte cap: input bytes and heap amplification are different quantities.
