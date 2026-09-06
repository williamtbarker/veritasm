# Experimental immutable-spool external-count bridge

- Status: implemented correctness slice; not in the stable CLI or assembler
- Source: a completed, immutable, re-verified `VTSPOOL1` spool
- k range: 3..=127 in this experiment; stable assembly remains 3..=63
- Support units: accepted-window occurrence and supplied-fragment instance

## Contract

`experimental::spool_external` performs two scientific replay passes over the
spool. Each iterator also performs its own full integrity verification before
opening the replay stream; those integrity traversals are not counted as
scientific passes. One verifier call now performs one bounded forward read of
one no-follow-opened regular-file descriptor. During that same read it hashes
the whole-file preimage, hashes the domain-and-length-framed pretrailer
preimage, decodes the complete structured stream, checks the fixed trailer
boundary, probes exact EOF, and compares descriptor metadata before and after
the traversal. The explicit verifier plus the two iterator-open verifiers are
therefore three logical calls and **three integrity physical read passes**.
Adding the two terminal scientific replays makes **five full-file-equivalent
physical read passes**. The result reports
`integrity_verification_calls = 3`,
`integrity_physical_read_passes = 3`, `scientific_replay_passes = 2`, and
`total_physical_spool_read_passes = 5`. No pass retains the complete read set
or a complete set of post-QC segments. This is an algorithmic I/O-count result,
not a measured throughput claim.

For each read, the wide rolling scanner applies the same rules as the stable
scanner: IUPAC ambiguity breaks every overlapping window; FASTQ quality is
Phred+33 Q0..Q93; a window is accepted only when it has neither ambiguity nor
a base below the spool's authenticated minimum-quality setting. The result
retains the mutually exclusive possible, accepted, ambiguity-only,
quality-only, and ambiguity-and-quality counts.

The count replay rolls the canonical k-mer and canonical m-mer states together.
A fixed-capacity monotone deque selects the exact minimum m-mer for each
k-window in amortized constant time without decoding and allocating a new
k-base vector per accepted event. Equal minima remain ordered until expiry, so
the leftmost-tie rule is preserved. This owner is only a routing value: complete
k-mers still decide equality. The original allocating selector remains an
independent oracle and is used to recompute every distinct retained route before
the checked result is exposed.

Occurrence mode emits every accepted read window. Fragment mode temporarily
retains at most one explicitly admitted fragment's full 32-byte keys and exact
32-byte owner keys, including both synchronized mates, then sorts and
deduplicates by the complete k-mer. Duplicate keys acquiring different owners
fail closed. Thus a key contributes at most once for one immutable fragment
even when it occurs in both mates, repeats within a mate, or occurs in exact
regions separated by rejected windows. Repeated normalized identifiers in
separate input records remain separate fragment instances because the spool
ordinal, not identifier text, defines the instance.

## Authenticated identity

The bridge also publishes the path-free common source descriptor and
`source_root` defined by ADR 0023.  They bind the exact spool and its input
mode, fragment/read counts, schema, and minimum-quality policy, and are shared
with transition ledgers at different k values.  This common root is deliberately
distinct from the child-specific external-run identity below; an orchestrator
must validate both rather than treating either digest as the other.

Every XWR v2 run header contains a typed support-unit tag. Its 32-byte source
identity additionally hashes a versioned domain plus:

- complete spool SHA-256 and pretrailer SHA-256;
- authenticated fragment/read counts and input mode;
- embedded stable k, profile, support unit, threshold, minimum quality, and
  remap setting; and
- requested external k, minimizer length, virtual-bucket count, and support
  unit.

Changing source bytes or any listed scientific field changes the run domain.
SHA-256 detects corruption and incompatible cache substitution; it is not a
MAC against an actor able to replace all source and metadata consistently.

`SpoolExternalResult` is a source-produced, field-private type with no public
constructor and no `Clone` implementation. Its only table-bearing public path
is `validated()`, which rechecks the complete source descriptor, lower-case
registered digests, read/fragment relationship, exhaustive QC ledger, support
unit, all exact rows and routes, final-run and replacement conservation, and
the five-read-pass operational contract before returning an immutable
`SpoolExternalView`. `external_counts()` on that view is the source-backed
borrowed table. A free-standing public `ExternalPartitionResult` remains only
an explicitly unverified materialized adapter for isolated graph oracles; it
cannot be promoted through this bridge.

The verifier computes the pretrailer boundary from the registered descriptor
length (`length - 64`) before decoding. A bounded reader prevents an
attacker-controlled fragment length from consuming trailer bytes. A separate
logical byte counter distinguishes decoder consumption from buffered read-ahead.
The trailer's declared boundary, counts, embedded digest, exact fixed width,
and EOF must then agree with the registered immutable fields. On Unix, final
path opening uses `O_NOFOLLOW`; device/inode, length, modification time, and
change time are compared to the registered descriptor snapshot before and
after the read. As elsewhere in this package, SHA-256 is corruption and cache
substitution evidence, not authentication against an actor able to rewrite all
bytes and all trusted metadata/capabilities consistently.

## Resource behavior

The bridge admits one decoded fragment and a declared maximum number of
fragment windows before scanning. Its memory reservation charges the decoded
fragment cap; the larger of the legacy two-key-vector allowance and one
complete `(k-mer, owner)` vector; exact rolling-ring storage; the fused
scanner's fixed inline queue scratch; the cloned work-directory and result
digest strings; and the spool reader buffer alongside the external reducer's
scan phase. Static option, source-total, path-sensitive external-phase, and
integrity-reader admission occurs before the first spool traversal. The
possible-window ceiling is enforced after each decoded fragment rather than
after a complete pass. The external temporary allowance is reduced by the
already-live spool size, and reported temporary and open-file high-water values
include the spool; the integrity verifier's actual single-spool-descriptor use
is included conservatively alongside an external run descriptor even for an
empty replay.

The checked result view performs `O(E)` validation for `E` exact rows and uses
constant additional heap space. Callers should acquire one view and reuse its
read-only getters instead of repeatedly requesting validation.

This is still an experimental allocation model, not a whole-process RSS
guarantee. The owned-payload contract excludes caller-owned state, stacks,
allocator bookkeeping, dependency internals, and operating-system cache. A
limit error or a passing focused test is fail-closed software evidence, not a
measured RSS or performance result.

## Executable evidence

Focused tests establish:

- event-for-event equality between fused routing and the original exact
  selector across k=3..127 boundaries, m=1, m=k, reverse complements,
  homopolymer/repeated ties, IUPAC resets, lowercase bases, and quality resets;
- exact differential equality at k<=63 with the stable scanner/counter in both
  support modes;
- literal full-window enumeration equality above the u128 boundary at k=65;
- nonzero accounting in all three rejection classes and exact partition of
  possible windows;
- deduplication across repeated mate keys while duplicate identifiers in
  separate records remain distinct fragments;
- rejection of corrupted and truncated spools before a run directory exists;
- rejection of a bit flip at every byte offset, every truncation boundary, an
  appended byte, direct path replacement, and a final-component symlink to the
  registered inode;
- typed fragment-memory, total-window, total-temporary, and run-count limit
  failures, including impossible verification memory rejected before a corrupt
  spool is read; and
- identical scientific counts across spill-buffer sizes and Rayon pools of one
  and four threads;
- test-only completed-traversal counters that observe exactly three verifier
  reads and two scientific replays for the complete bridge;
- an opaque checked view that reports exactly three verifier calls, three
  integrity reads, two scientific replays, and five total physical reads and
  rejects source-descriptor, exact-row, and pass-ledger tampering; and
- no retained `experimental-wide-runs-*` directory after success or the
  exercised corrupt-input and resource-limit failures.

These tests establish software behavior only. They do not establish assembly
accuracy, sensitivity, throughput, lower RSS, or production readiness.

## Development performance observation

An unqualified 10 kb / 2,000-fragment PE100 synthetic diagnostic showed a
roughly tenfold reduction in the isolated minimizer-selection kernel. Four
interleaved release executions of the complete k=31 path did **not** demonstrate
a material end-to-end wall-time improvement: filesystem I/O dominated the run
and the before/after wall-time distributions overlapped. Peak RSS was also
effectively unchanged. The complete output trees were byte-identical. These are
local development observations, not a portable benchmark or a package
performance claim; the optimization is retained for its exact equivalence and
clear removal of per-event allocation, not as evidence that the assembler is
faster end to end.

## Deliberate exclusions

The bridge does not alter the stable CLI, count-run format, graph, FASTA/GFA,
bundle, report, or schemas. It does not yet stream its results into compacted
graph construction, quality correction, paired traversal, or multi-k
reconciliation.
