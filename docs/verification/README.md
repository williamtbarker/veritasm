# Verification evidence

These are historical Linux command transcripts, including failed attempts,
captured before the macOS file-mode conversion fix. [Current status](../STATUS.md)
records later verification. The command output and source-input hashes are retained, with the path
normalization described below; they do not identify later source edits.

- `initial-check`: compilation before checkpoint edits.
- `stable-tests`: toolchain/dependency temporary-object failure; no test verdict.
- `stable-tests-retry`: full suite with the stale operational-hash regression failure.
- `determinism-regression`: the focused semantic replacement regression.
- `stable-verification`: final selected-toolchain gate.
- `msrv-verification`: dependency temporary-object failure after MSRV Clippy passed.
- `msrv-verification-retry`: dependency rebuild succeeded; 549 tests passed before a Cargo-produced test binary lacked its executable mode.
- `msrv-verification-final`: 621 tests passed before a second generated executable was found empty.
- `msrv-validation-schema`: focused pass after regenerating that failed output.
- `msrv-complete`: final complete MSRV gate, exit zero; all 625 tests and 15 doctests passed.
- `final-source-package-clean`: archive generation, integrity, and explicit cleanup report.
- `clean-source-check`: a separately copied tree and fresh target directory,
  verified against the frozen input inventory, compiled with all targets/features.
- `verified-source-inputs.txt`: exact source, test, schema, fixture, script,
  and Cargo inputs used by the final source checks. It deliberately excludes
  evolving evidence documents, the separately tested source packager, and its filename inventory.

The `.json` companions record commands, exit codes, working directories, and
durations. In these historical `.json` and `.log` records, the original absolute
workspace root has been replaced consistently with `<verification-workspace>`.
Relative checkout and build-directory relationships are preserved. This marker is
an explanatory path token, not a literal directory to use when rerunning commands.
No command option, test result, failure, timestamp, duration, or source-input hash
was changed. Original records remain available in Git history before this
normalization; these normalized files are not byte-identical raw transcripts.

Local helper scripts used to capture logs are not required to build or resume the
project. Final ZIP integrity is checked separately against the same frozen input
hashes.
