# ADR 0005: Transactional deterministic evidence bundle

- Status: accepted for 0.1 implementation with `rustix` no-replace commit
- Date: 2026-09-03

## Context

Virustic2 commits requested files independently and can replace an existing FASTA before a later report
failure. Stable scientific comparisons also fail when thread counts, timestamps, or traversal order
change bytes.

## Decision

One run produces one directory. Acquire a cooperative exclusive sibling lock, write every artifact to
a same-filesystem sibling staging directory, close and verify every file and schema, create a manifest,
and sync the staged files/directory where supported. Commit with `rustix` 1.1.4
`renameat_with(..., RenameFlags::NOREPLACE)`. Version 0.1 supports Linux and Apple targets for this
operation, never falls back to ordinary replacing rename, and never overwrites an existing destination.

Core artifacts exclude timestamps, thread count, timings, host names, temporary/absolute paths, and
unordered iteration. Canonical representations and total ordering are specified per format. Stable
0.1 execution telemetry, if emitted, goes only to stderr; there is no execution-log file option.

## Consequences

- A failed VeritAsm run cannot destroy a pre-existing result when writers cooperate with the lock.
- Directory-rename and sync guarantees remain filesystem/platform dependent and are documented.
- Users choose a new destination for every successful run.
- Byte equality across thread counts is a release-blocking test, not an informal expectation.

## Commit and failure boundary

The successful no-replace rename is the linearization point and final operation that can affect exit
success. An unsupported kernel/filesystem fails closed before commit. Parent-directory sync and lock
cleanup after commit are best-effort warnings; their failure does not turn a visible committed bundle
into a failed command. This supplies atomic visibility and no replacement but does not claim survival
across sudden power loss. An existing lock is never removed automatically because ownership may still
be live. The lock guard is established immediately after exclusive creation, holds the created file
descriptor through the transaction, and requests cleanup only if the lock pathname still has that
descriptor's device/inode identity. Failure to establish identity or remove the owned lock is
best-effort cleanup and leaves a record for manual review. PID-only automatic stale-lock reclamation is
rejected because process liveness and PID reuse cannot be established portably from that record.
The final identity check and pathname unlink are not an atomic defense against a malicious writer in
the output parent; that directory remains inside the trusted local filesystem boundary.

`rustix` is a dependency implementation containing audited platform code; VeritAsm project source
remains safe Rust. The package is pinned, built at Rust 1.85, license/advisory reviewed, and tested on
Linux and Apple Silicon macOS before the release gate can pass.
