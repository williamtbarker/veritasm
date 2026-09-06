# ADR 0002: Immutable spool and exact disk-backed counting

- Status: accepted for 0.1 implementation
- Date: 2026-09-03

## Context

Streaming FASTX alone does not bound memory when all distinct k-mers remain resident. Re-reading live
inputs for multiple passes can mix changed files. High-background and error-rich data can contain many
singleton k-mers, while raising the retention threshold automatically could erase low-abundance truth.

## Decision

Parse and validate each source exactly once into a versioned, checksummed immutable fragment spool.
Every k and downstream audit pass reads that spool.

Count full canonical packed keys exactly using numeric high-prefix range partitions, bounded sorted
runs, deterministic fan-in-16 multi-pass merging, and numeric-order partition concatenation. Final
counters are checked `u64` and globally key-sorted. Apply an explicit retention rule only after the
complete histogram exists. Load retained keys into an exact in-memory graph only after enforcing a
deterministic key cap. Never raise the threshold automatically. Exact limits and error states are in
`docs/CONFIGURATION.md`.

## Consequences

- Counter RAM can be bounded independently of the number of observed distinct keys.
- Temporary disk and extra passes are required.
- The retained graph remains memory-bounded by failure, not by being disk-resident.
- The spool and temporary counts contain sample-derived sequence and require restrictive permissions.
- The same validated observation stream feeds every k and audit.

## Failure behavior

Oversize input, overflow, missing ranges, disk exhaustion, checksum failure, corrupt spool/count runs,
or graph-cap excess is fatal before bundle commit. Temporary state is not resumed in 0.1.

## Alternatives rejected or deferred

- one in-memory hash map for all observed k-mers;
- probabilistic graph membership;
- silent sampling or adaptive support increase;
- a fully disk-resident compact graph, deferred until warranted by benchmarks.

## Evidence

See the DSK, KMC2, KMC3, BCALM2, Bifrost, and Cuttlefish 2 sources in `RESEARCH.md`. Implementation
equivalence to a small exact in-memory oracle is still required; literature is not proof of this code.
