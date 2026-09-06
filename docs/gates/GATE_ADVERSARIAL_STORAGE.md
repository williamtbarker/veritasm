# Adversarial storage qualification record

Status: development evidence for the post-0.1 hardening branch; not a scientific or release claim.

## Scope

This gate exercises private spool framing, exact count-run framing and state transitions, result-bundle
rendering and checksum manifests, no-replace transaction boundaries, and inclusive resource limits.
It does not establish resistance to privileged filesystem interference, sudden-power-loss durability,
or correctness on biological data.

## Added executable checks

- a completed spool rejects sampled single-bit changes and deliberately reauthenticated invalid
  ordinals, lanes, read counts, roles, lengths, alphabets, and quality tags;
- an overflowing untrusted spool trailer length returns `integrity_spool` instead of panicking;
- exact count runs reject every possible single-byte change in a small framed vector, as well as
  reauthenticated zero support, duplicate keys, and out-of-width keys;
- `CountWriter` is permanently poisoned after an ordinal or observation error and cannot emit a
  partial result if a library caller ignores the first error;
- the append-only count-run manifest is compared with an incremental expected digest before its
  checksum is admitted as provenance;
- staged-output, aggregate-temporary, and count-temporary limits exercise one byte below, exactly at,
  and one byte above the required boundary;
- the public result-manifest verifier rejects modified, missing, unlisted, duplicated, unsorted,
  unterminated, and unsafe-path entries;
- every injected precommit failure removes cooperative lock and staging state, a stale lock is
  preserved, and a noncooperating destination creator is not replaced; and
- public-API property tests sample spool and count-run single-bit mutations independently of private
  unit-test access.

## Fuzz assets

Four additional sanitizer harnesses and bounded seed corpora are checked in:

- `spool` — raw, structurally mutated, reauthenticated, and overflowing-length spool bytes;
- `count_run` — raw and reauthenticated run corruption plus ordinal/error-state sequences;
- `bundle_manifest` — valid and malformed checksum/inventory manifests; and
- `bundle` — end-to-end typed rendering, manifest verification, and second-writer no-replace checks.

They supplement the existing `fastx`, `gzip`, and `paired` targets. Their presence is not fuzzing
evidence.

## Execution record

On Linux x86-64, the complete stable gate passed with rustc/cargo 1.98.0: `cargo test --locked
--all-targets --all-features` ran 170 tests, and `cargo check`, warnings-denied `cargo clippy`,
documentation tests, warnings-denied `cargo doc --no-deps`, and the release build all passed. The same
commands passed with the declared Rust 1.85 MSRV when an isolated `CARGO_TARGET_DIR` was used.

An earlier MSRV attempt reused the stable toolchain's `target/` directory. Its unit tests passed, but
all 15 CLI tests failed with `PermissionDenied` because the shared un-hashed
`target/debug/veritasm` artifact had mode `0644`. This was a local cross-toolchain build-directory
contamination failure, not a product test pass. The isolated MSRV rerun passed all 170 tests. Final
qualification must still repeat both toolchains from the combined tree with separate target
directories and from the clean source archive.

Sanitizer-backed `cargo-fuzz` execution: **NOT RUN**. Neither a nightly toolchain nor `cargo-fuzz` was
installed in this environment. Scheduled CI is configured for all seven targets, but configuration is
not execution evidence.
