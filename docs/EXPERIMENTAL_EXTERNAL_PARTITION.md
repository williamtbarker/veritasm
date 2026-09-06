# Experimental authenticated wide-key partition slice

- Status: implemented correctness vertical slice; not a stable assembler path
- Scope: `experimental::partitioned_dbg`, `experimental::external_run`, and
  `experimental::external_reduce`; authenticated spool replay is documented
  separately in `EXPERIMENTAL_SPOOL_EXTERNAL.md`
- Support unit: the accepted-segment wrapper is occurrence-only; XWR v2 and
  the spool bridge explicitly carry occurrence or fragment-instance support
- Stable CLI/schema effect: none

## What this slice establishes

The in-memory partition oracle now retains one `SourceWindowProof` per accepted
window. Construction compares the rolling scanner with a separate literal
byte-slice implementation that:

1. normalizes exact A/C/G/T bytes;
2. constructs and lexicographically compares the full reverse complement;
3. selects the canonical source window;
4. independently canonicalizes every minimizer candidate; and
5. applies the leftmost owner tie rule.

`validate_invariants` rejects noncanonical stored keys and reconciles every
span and aggregate count with the occurrence proof ledger.
`validate_against_segments` is the stronger source-bound operation: it
recomputes the complete proof stream from caller-supplied immutable segments
and checks the global and per-segment source SHA-256 identities. This closes
the known counterexamples in which a reverse-complement key or a self-consistent
but false span owner previously survived aggregate-only validation.

Within each segment, proof-to-span ownership validation advances one span cursor while proofs advance
in window order. It performs `O(W + S)` owner checks for `W` source-window proofs and `S`
super-k-mer spans, uses no proof/span lookup allocation, and never restarts the span scan for a proof.
Canonical-key and complete-minimizer verification still performs its separately bounded work for
each proof. A many-owner-change regression exposes the span-advance counter and requires it to stay
below the proof count while reaching every span exactly once.

The external slice assigns deterministic event ordinals in sorted
`(source_ordinal, segment_ordinal, window_ordinal)` order. It spills complete
32-byte keys and complete 32-byte minimizers into virtual-bucket runs, then
performs a streaming bounded-fan-in exact reduction. A bucket or minimizer is
never used as key identity. The final count rows remain ordered by
`(virtual_bucket, complete_key)`, and their checked occurrence sum must equal
the independently planned source-window total.

## Run format v2

The run is a private experimental format, not a stable artifact.

- Magic: `VTXRUN01`; schema: little-endian `u16 = 2`.
- Encoding tag 1 means little-endian integers and fixed 32-byte big-endian
  packed keys.
- Header fields bind run kind, `k`, minimizer length, typed support unit
  (`0=supplied_fragment_instance`, `1=accepted_window_occurrence`),
  virtual-bucket count and ID, generation, deterministic run ordinal, exact
  32-byte source identity, event-ordinal interval, record count, support mass,
  and payload length. Reserved bytes must be zero.
- Both observation and reduced payload records are 72 bytes:
  `complete_key_be[32]`, `complete_minimizer_be[32]`, and `value_u64_le`.
  `value` is an event ordinal in an observation run and exact checked support in
  a reduced run.
- The trailer is `VTXEND01`, followed by SHA-256 of
  `"veritasm:experimental-wide-run:v1\0" || header || payload`, followed by the
  declared total file length.

A reader opens with `O_NOFOLLOW|O_NONBLOCK`, rejects non-regular paths and any
mode other than `0600`, then checks exact file length before allocation,
schema, encodings, reserved bytes, source, support and routing domain, record
ordering, active key bits, canonical key spelling, minimizer ownership,
virtual-bucket route, checked support totals, trailer, digest, and EOF. Each
catalog row privately retains the sealed descriptor's device, inode, owner,
mode, and length. Registered reads and reclamation require both the current
literal path and a no-follow descriptor to match that identity. These physical
fields are operational only: they are excluded from scientific equality and
digests. The SHA-256 is cache-integrity evidence, not a MAC against an actor
able to replace the complete run.

## Resource and failure contract

The caller sets independent limits for segment count, input bases, accepted
windows, distinct keys, sort-buffer bytes, phase-owned memory, I/O-buffer
bytes, temporary bytes, created runs, merge fan-in, and open files. The merge
requires at least two inputs per pass and reserves one descriptor for its
output, so observed simultaneous descriptors cannot exceed
`merge_fan_in + 1 <= max_open_files`. All large vectors use fallible
reservation. XWR readers and writers use crate-owned fixed-capacity buffers
whose allocation is performed with `try_reserve_exact`; an impossible
allocation returns `resource.memory` instead of entering the standard
`BufReader`/`BufWriter` infallible allocation path. The actual vector capacity
must not exceed the admitted byte count. Configured buffers are capped at 64
MiB because larger buffers have no measured benefit here and allocator
overcommit would weaken clean failure. Writer admission precedes file
creation. XWR paths use create-new semantics, are explicitly set to owner-only
mode `0600`, and are independently checked through their opened descriptors on
the supported Unix platforms. All byte/count/range arithmetic is checked.

An output merge run is sealed and independently reopened before an ancestry
row is registered. Only after registration are verified predecessors removed.
Temporary-byte accounting includes old and new generations while they coexist
and records a high-water value. Catalog rows retain only a fixed-width numeric
run ID; paths are derived at the filesystem boundary with their actual length
and allocation capacity checked. Scan, merge, and final-materialization
admission uses the Rust widths of every simultaneously live catalog row,
replacement row, predecessor digest, reader slot, heap row, output row, and
fixed verification buffer. Every build creates a random, private
`experimental-wide-runs-*` directory with mode `0700`; a stale legacy name or
another concurrent build is never reused. The guard records the directory
device, inode, owner, and mode from both the literal path and a no-follow
descriptor. Error cleanup first verifies that identity, bounds and validates
every entry, then revalidates every exact XWR entry before unlinking it. It
never calls recursive deletion and preserves a substituted path or unknown
entry while surfacing cleanup failure. A successful build authenticates and
removes every final run before identity-checking and removing the empty
directory, so returned `final_runs` are evidence summaries rather than
anonymous temporary-file ownership.

POSIX pathname removal still has an unavoidable check/unlink interval for a
malicious process running as the same UID. The unique `0700` directory excludes
other users and addresses accidental collisions, stale names, symlink swaps,
and ordinary rename/substitution. Defending against an actively hostile
same-UID process requires a directory-FD-relative lifecycle that this
experiment does not yet implement.

## Executable proof obligations

Focused tests cover:

- reverse-complement/noncanonical key rejection;
- wrong span ownership and coordinated proof/count mutation against source;
- exact equality with the independent in-memory oracle;
- forced multi-pass reduction with a three-descriptor ceiling;
- intentional all-keys-in-one-bucket collisions without key merging;
- input-order independence and exact wide keys at `k=127`;
- source-identity mismatch and memory/temp/open-file admissions;
- limit-minus-one, exact-limit, and limit-plus-one scan, merge, and final
  materialization boundaries, including a long work-directory path;
- explicit numeric endianness and fixed record widths; and
- fixed-buffer round trips at capacities from one byte upward, typed impossible
  allocation before file creation, and owner-only run-file permissions; and
- unique `0700` build directories, create-new collisions, stale-name
  preservation, symlink and inode substitution rejection, and surfaced cleanup
  refusal without deleting replacement or unknown entries; and
- header, payload, trailer, schema, encoding, truncation, and trailing-byte
  corruption.

These are software-correctness tests. They do not establish lower RSS, higher
throughput, assembly sensitivity, reconstruction accuracy, or benefit from a
wide k-mer.

This 72-byte record is a correctness format, not ADR 0016's final
width-specialized high-performance representation. The minimizer is derivable
from the complete key and is retained here so corruption and routing can be
checked directly. A width-specialized record retaining both fields would avoid
`(32 - key_width(k)) + (32 - key_width(m))` bytes per record. With an `m <= 31`
eight-byte minimizer, that is 48 bytes (66.7%) at `k <= 31`, 40 bytes (55.6%)
at `k <= 63`, and 24 bytes (33.3%) at `k <= 127`, before considering whether a
verified block can omit redundant per-record owners entirely. Those are layout
calculations, not measured I/O, memory, or speed improvements.

## Promotion blockers

The accepted-segment wrapper deliberately does not implement fragment-instance
deduplication or quality/ambiguity-aware spool streaming; those exist only in
the isolated spool bridge. The wider slice still does not implement authenticated
independently verified blocks within a large run, durable crash-resume ancestry, real ENOSPC or
interruption injection, parallel bucket scheduling, endpoint construction,
width-specialized compact records, direct compaction, boundary stitching,
stable output schemas, or CLI wiring.
The byte-admission calculations cover named module-owned payloads and checked
allocator capacities; caller-owned slices, stacks, allocator bookkeeping,
dependency internals, and operating-system cache remain outside the contract.
It is not a whole-process RSS guarantee.

Promotion requires exact equality with the stable counter in both support
modes, fault-injected predecessor reclamation, Linux and Apple-Silicon
determinism, retained RSS/I/O/temp-disk/descriptor measurements, and exact
graph/compaction equality on the stable oracle domain as required by ADR 0016.
