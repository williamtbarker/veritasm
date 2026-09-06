# Experimental exact transition-witness ledger

`experimental::transition_witness` is an isolated ADR 0023 substrate. It scans one
descriptor-authenticated `VTSPOOL1` replay at graph k and records every accepted read window of
length `q = k + 1`, for `3 <= k <= 126`. It is not connected to the stable CLI and does not make an
assembly, sensitivity, coverage, abundance, molecule-support, or diagnostic claim.

Each event contains the complete canonical packed q-mer, its observed orientation, lane ordinal,
supplied-fragment ordinal, mate role, normalized identifier digest, and zero-based read offset.
Scanning restarts for every read. IUPAC ambiguity and any base below the spool's frozen quality
threshold reject the whole overlapping window. An event can therefore never cross a read, mate,
record, ambiguity, or quality boundary.

The reduced table reports two quantities with different denominators:

- `accepted_window_occurrences`: every accepted source-read window;
- `distinct_supplied_fragment_instances`: one contribution per supplied fragment for a transition,
  deduplicating repeats and mate overlap within that fragment.

Neither quantity estimates physical molecule count. A complete q-mer, rather than a hash, decides
transition identity. SHA-256 binds complete event frames and sorted rows; it is corruption evidence,
not a substitute for exact comparison and not a MAC. No Bloom filter participates in positive
membership.

## External-run contract

Events spill to fixed-width, create-new private runs. Each build owns a unique 0700 run directory;
each run is created at mode 0600. Cleanup rechecks the directory's literal type, owner, mode, and
recorded device/inode identity, then makes two bounded passes over only the fixed-width numeric run
names. Every entry must remain a private regular file with matching path/descriptor identity before
it is unlinked; an unknown, excess, or replacement entry is preserved and fails cleanup closed.
Headers bind the common source root, k, numeric generation/run identity, exact event interval,
record count, and payload length. Every run
has a domain-separated SHA-256 trailer and exact EOF. Merge readers authenticate the same descriptor
they consume. A merged output is sealed and independently re-read before predecessor runs are
deleted; replacement records retain predecessor digests and reclaimed byte counts, never paths.
All registered private run files and the run directory are removed on success or error.

The source replay validates one strict coordinate stream before q-mer sorting, retaining only the
previous coordinate. This rejects duplicate coordinates exactly even when they carry different
q-mers or straddle spill runs. The terminally authenticated spool descriptor is closed before an
external merge opens its writer and bounded reader fan-in.

Limits cover accepted events, reduced rows, per-fragment decode and window bounds, event-sort
storage, retained result/catalog payloads, temporary run bytes, cumulative run files, merge fan-in,
and open files. The memory preflight is deliberately conservative. `max_temp_bytes` covers this
component's run files; the already-live spool remains owned and accounted by its caller. Operational
path strings and small error messages are not retained per event or run.

For `N` accepted window events and `R` reduced complete-key rows, comparison work is bounded by the
fixed-width external sort and merge, conservatively `O(N log N)`, followed by an `O(N)` reduction.
Resident payload is bounded by the configured sort chunk, merge fan-in, run/replacement catalogs,
decode buffer, and `O(R)` returned rows; temporary bytes and cumulative run count have separate
checked limits. Exact lookup in a validated ledger is `O(log R)`.

## Roots and validation

`TransitionSourceDescriptor` is a path-free fixed-width preimage containing raw spool digests,
spool schema, fragment/read counts, input mode, and the base-quality rule. Every k child derives the
same `source_root` from it. `transition_root` binds that source root, k, row count, each complete
canonical q-mer, both support values, and the digest of its sorted full event frames.

Every `TransitionLedger` integrity field is private, and the type has no `Clone` implementation in
normal builds. `TransitionLedger::view()` validates ordering, widths, support invariants, QC
conservation, totals, and the transition root before graph code receives a borrowed view. Copying
the reported rows cannot reconstruct a source-backed ledger. The stronger
`validate_transition_ledger_replay` rebuilds the ledger from the authenticated spool and compares
all fields. Bundle validation should use replay, not structural validation alone.

Multi-k orchestration must call the one-k builder independently for sorted unique k values. This
repeats authenticated spool replay by design. It does not treat one child, a child contig, a
minimizer, or a higher-k count as evidence for another child.

## Current limitations

- Reduced rows are materialized under `max_rows`; this is not a whole-pipeline bounded-RSS claim.
- Sorting is global fixed-width external merge sorting, without minimizer partitioning yet.
- The event table is reduced after validation; source coordinates remain checksum-bound rather than
  retained as a public random-access table. Exact replay is required to audit them later.
- The spool and work directory must be private to the process. SHA-256 reveals inconsistent bytes
  but does not establish continuity if all data and registered digests are replaced together.
- Transition witnesses prove local read-observed adjacency only. They do not prove global phasing,
  full-contig read span, a complete genome, circularity, or biological origin.
- Event-framing/replay fuzz targets and injected short-write, ENOSPC, interruption, and cleanup-failure
  tests remain required before this substrate is qualified for promotion.

## Focused verification

    cargo test --locked --lib transition_witness
    cargo clippy --locked --lib --tests -- -D warnings
    rustup run 1.85.0 cargo test --locked --lib transition_witness
    rustup run 1.85.0 cargo clippy --locked --lib --tests -- -D warnings

The 18 focused tests include exhaustive literal q-mer/orientation comparison, source-coordinate
uniqueness across spills, complete event/header mutation checks, replay rejection after
self-consistent row/root changes, exact event/run/temporary/open-file boundaries, deterministic
spill geometry, private file modes, unique-directory identity-safe cleanup, bounded registered-entry
cleanup, and empty-input behavior.
