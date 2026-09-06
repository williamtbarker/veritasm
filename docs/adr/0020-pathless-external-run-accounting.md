# ADR 0020: pathless external-run accounting and owned cleanup

- Status: Accepted for the experimental v0.4 data plane
- Date: 2026-09-05
- Scope: `experimental::external_run`, `external_reduce`, `spool_external`, and multi-k admission

## Context

The first authenticated wide-key reducer bounded temporary bytes and file descriptors, but its
memory proof assigned a fixed allowance to each run-catalog entry. A catalog entry owned a
`PathBuf`, readers and writers cloned paths, replacement rows owned nested vectors, and several
vectors coexisted during reduction and final materialization. A fixed per-run constant therefore
could not dominate allocator payload for arbitrarily long valid work-directory paths. The initial
scan proof also omitted the writer's digest-verification buffer, and the multi-k orchestrator built
children before enforcing some parent limits.

These are admission-model defects even when ordinary executions stay below the configured budget.
Tests that merely observe low RSS cannot repair an incomplete ownership proof.

## Decision

1. Run identity in the reducer is a fixed-width numeric value. Catalog metadata does not own a
   filesystem path. One run directory is borrowed by the reducer, and paths are derived only at the
   filesystem call boundary.
2. Memory admission is expressed in terms of the actual fixed Rust payload types and explicit
   buffer capacities. It covers every simultaneously live catalog generation, reader slot, merge
   heap row, predecessor digest, replacement row, scan buffer, output row, and verification buffer.
   No unexplained per-entry allowance is accepted.
3. Allocations are reserved only after their complete phase high-water has been admitted. Actual
   capacities are checked after reservation where the allocator may return more than requested.
4. Final materialization includes all still-live reducer state in its admission or drops that state
   before reserving the exact-result vector.
5. Temporary run ownership uses a guard. Failure attempts cleanup, and a cleanup failure is surfaced
   with the retained private path rather than being silently ignored. Successful materialization
   either transfers an explicit owned run set or authenticates and removes the runs before returning;
   there is no anonymous successful residue.
6. The spool bridge validates static option and verification-memory requirements before traversing
   the spool, enforces known spool totals before scientific replay, and enforces replay limits
   incrementally. Operational evidence distinguishes integrity-verification traversals from
   scientific replay traversals.
7. Multi-k orchestration precomputes and admits child headers, segment order, child window counts,
   and oracle-recount work before constructing any child graph.

The budget remains an owned-payload contract. It explicitly excludes caller-owned input slices,
thread stacks already admitted by their caller, allocator bookkeeping, code pages, dependency
internals not owned by this module, and operating-system cache. Documentation and result fields must
not relabel it as an RSS bound.

## Required falsification tests

- A long but valid work-directory path cannot bypass memory admission.
- Limit-minus-one, exact-limit, and limit-plus-one cases cover scan sealing, merge fan-in, replacement
  retention, and final materialization.
- Minimal I/O buffers still charge the fixed verification workspace.
- Multi-generation reduction accounts for the input catalog, current bucket, next generation,
  final-run summaries, readers, heap, and replacement ancestry at their real overlap.
- Spool options that cannot admit one integrity pass fail before reading the spool.
- Oversized spool totals and per-replay window totals stop before unnecessary complete passes.
- Every injected post-directory-creation failure either removes owned runs or returns a cleanup error
  that identifies the retained private directory.
- MSRV Rust 1.85 and current stable produce the same scientific rows.

## Consequences

This refactor may reject configurations that the incomplete model previously accepted. That is a
correct fail-closed change, not a performance regression. The experimental `WideRunMeta` surface may
change because it has not been released or promoted. No performance, bounded-RSS, production, or
sensitivity claim follows from completing the accounting proof.

## Rejected alternatives

- Increasing the fixed catalog constant: path allocation is input-dependent, so no finite constant
  proves the claim.
- Measuring RSS in a regression test: RSS is platform- and allocator-dependent and does not establish
  pre-allocation admission.
- Treating cleanup as best effort: silent residue contradicts the owned temporary-state contract and
  obscures capacity failures.
