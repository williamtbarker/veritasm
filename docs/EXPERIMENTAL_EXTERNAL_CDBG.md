# Experimental exact external cDBG topology seam (EC-1a)

Status: research implementation, outside the stable CLI and output contract.

This module implements only the first executable slice of ADR 0024. Its current public entry point
accepts an explicitly named `UnverifiedMaterializedEdgeAdapter`, validates and caps that oracle
input, and converts its exact canonical-edge rows into two owned authenticated streams:

1. globally key-sorted `EDGE` records; and
2. globally reduced literal `NODE_STATE` records sorted by `(owner_partition, node)`.

`INCIDENCE` and internal literal-order `NODE_AUDIT` runs are deterministic authenticated
predecessors. While a parent is being consumed, spill children retain an all-zero placeholder
header and no trailer, so they cannot parse as `VTEBLK01`. Only after every parent reaches its
authenticated trailer, exact EOF, and final descriptor check are those children sealed, reopened,
verified, registered, and added to ancestry in numeric-ID order. Predecessors are reclaimed only
after that registration succeeds. The successful owner retains a complete, bounded, path-free
immediate-parent ledger and its SHA-256 root.

Every build gets a collision-resistant private subdirectory with mode `0700`; run files are mode
`0600`. Existing files are opened without following the final symlink component, and predecessor
deletion checks the registered descriptor and literal path identity first. Dropping the owner
performs a best-effort bounded cleanup; `cleanup()` reports a failure explicitly. Cleanup never uses
recursive pathname deletion. It rechecks the `0700` directory by path and `NOFOLLOW` descriptor,
validates every entry before deleting any, then revalidates each entry during a second pass. Only
exact numeric `.vte` names that are literal mode-`0600` regular files owned by the directory owner
are eligible; a substituted directory, unknown entry, symlink, permission mismatch, or count above
the admitted run-file cap stops cleanup and preserves unvalidated content. A process-crash residue
has a unique name and any provisional file in it remains structurally unpublishable; automatic
cross-process residue reclamation is intentionally not implemented.

The cleanup directory and entries remain anchored to opened directory descriptors, and each name is
re-statted immediately before `unlinkat`. These checks defend against accidental replacement and
ordinary pathname races. They are not a security boundary against a hostile process running as the
same operating-system user: portable POSIX has no conditional “unlink this name only if it still has
this inode” operation. Work directories therefore remain trusted deployment inputs.

## Frozen semantics in this slice

- One retained canonical k-mer is one exact backing edge with checked nonzero support.
- A non-self-reverse-complementary edge produces two literal oriented handles; a fixed edge produces
  one. Every handle produces exactly one incoming and one outgoing incidence.
- Literal `(k-1)`-mers, not canonicalized nodes, are reduced globally. Four-bit masks count distinct
  exact sides, not support mass. A repeated node/direction/terminal-base side is rejected before its
  bit is set; for a unique exact k-mer set, that tuple reconstructs a unique full handle.
- A node is a hard boundary when either degree is not one, the node is reverse-complement fixed, or
  it touches a reverse-complement-fixed edge.
- Node routing canonicalizes only for minimizer ownership. The literal node remains the identity.
- Widths are `W64` through k=31, `W128` for k=32..63, and `W256` for k=64..127. Inactive high bits
  are rejected on decode.

No Bloom filter, fingerprint, digest, minimizer, or partition number decides edge, handle, or node
identity.

## VTEBLK01 implementation

Typed codecs emit no native struct bytes. The implemented container has the ADR-frozen 192-byte file
header, 64-byte block header, per-block 32-byte digest, and 104-byte trailer. Blocks form a digest
chain. The file root binds the final canonical header, trailer totals, final block digest, and exact
file length. Empty files have zero blocks and use the unique all-zero final-block sentinel.

The retained-source field is the plan-independent canonical `EDGE` table root. That root in turn
binds the accepted-source identity, scientific-configuration root, k, support unit, row count,
support mass, and every exact `(key, support)` row. The partition count is an operational catalog
field because VTEBLK01 stores a selected partition (or `u32::MAX` for a global run), not a partition
count. A registered `NODE_STATE` reader must receive that catalog field and independently recomputes
the owner minimizer and partition.

After the final `EDGE` run is retained, two complete authenticated passes independently recompute
its support mass and canonical scientific table root and compare both with the pre-seal values.
After final `NODE_STATE` merge, an externally sorted direct literal-node projection is compared
lockstep with an independently sorted reverse-complement projection. Incoming and outgoing masks
are swapped and complemented, and all fixed-point and boundary flags must match exactly. The direct
projection independently recomputes the canonical node-table root from retained bytes. A separate
plan-independent node-symmetry root binds that verified result, node count, and incidence count.
Both audit projections are authenticated and then reclaimed; they are not public topology data.
The owning `ExternalCdbgTopology` is non-cloneable and non-constructible outside the module. Its
scientific roots, run summaries, statistics, and ancestry ledger are private immutable state exposed
only through read-only getters, so safe downstream code cannot rewrite proof fields after
construction.

A reader checks the registered descriptor identity and length, canonical header bytes, every block
link and digest, typed record invariants and strict order, repeated trailer totals, file root, exact
EOF, and descriptor metadata again on the same opened file. Modification and change timestamps are
included with device, inode, and length in the private descriptor registration. SHA-256 is an
integrity mechanism, not a MAC.

## Resource contract

Incidence, node-state, and node-audit sorting uses fixed-capacity, fallibly allocated buffers. Merge is
bounded-fan-in and holds at most `fan_in` reader blocks, one writer block, one heap row per reader,
and bounded numeric run catalogs. File-size projections are admitted before creation; temporary
bytes are released only after successful deletion. Run paths are derived transiently from numeric
IDs and checked path allocations; catalog and ancestry rows own no paths.

The conservative heap admission covers sort storage, merge block buffers, reader and heap objects,
three worst-case run catalogs, at most twice `max_run_files` complete ancestry links, and bounded
path/fixed state. Caller-owned input and kernel page cache are outside this owned-payload bound. The
current adapter necessarily allocates a second edge vector and admits it separately.

For `N` retained edges, `H <= 2N` oriented handles and `I = 2H` incidences. The implementation does
not reject against those worst-case upper bounds before inspecting fixed edges: `max_handles` and
`max_incidences` are enforced against checked actual counts during expansion. Sorting and merging use
`O(sort_buffer + fan_in * block_payload + max_run_files)` admitted owned state. Temporary I/O is the
sum of authenticated spill/merge generations and is enforced dynamically; this implementation does
not claim an optimal I/O constant, RSS bound, speed, or demonstrated scale.

## Executable evidence

Focused module tests currently cover:

- an independent ASCII literal-node oracle, including all 528 canonical k=3 singleton-or-pair
  constructions and all 136 canonical k=4 singleton constructions, plus complete
  edge/support/handle/incidence/node conservation;
- single-source-node skew with a one-record spill buffer and output equivalence across block,
  spill, and fan-in plans;
- k=31/32/33, 63/64/65, and 126/127 width-boundary cases;
- reverse-complement-fixed edges and nodes;
- exact-cap admission for a fixed edge with one actual handle and two incidences;
- fatal reverse-complement node/mask/flag disagreement and a plan-independent symmetry root;
- byte-layout round trips and empty-stream sentinel behavior;
- rejection of every single-byte mutation, every truncation, and an append across a complete run,
  plus targeted registered descriptor replacement;
- deterministic input permutation;
- memory, temporary-byte, run-count, edge-count, node-count, and materialization caps;
- faults after verified run creation, after a provisional child but before late parent
  authentication, and during partial predecessor cleanup, including absence of private-directory
  residue;
- structurally invalid simulated crash residue followed by a successful noncolliding build; and
- unique `0700` directories and `0600` files; preservation of an occupied prefix sibling; and
  fail-closed bounded cleanup for a replaced directory, unknown entry, non-private file, symlink,
  changed directory mode, or entry count over the admitted cap.

Run the focused checks with:

```bash
cargo +stable fmt --check
cargo +stable test --locked --lib experimental::external_cdbg::tests
cargo +stable clippy --locked --lib --tests -- -D warnings
cargo +1.85.0 test --locked --lib experimental::external_cdbg::tests
```

The last two commands are whole-crate gates even when the test filter is module-specific.

## Explicit limitations and next slice

EC-1a is not an assembler and does not emit a contig. It does not implement chunk assignment,
partition-local unitig compaction, seam/glue records, discontinuity-graph reconciliation, list
ranking, cycle canonicalization, reverse-complement unitig quotienting, sequence spelling, GFA, or
read-backed reconstruction evidence. In particular, a one-in/one-out cycle in these node states is
only graph topology and is not evidence of a circular molecule.

The entry point is
`build_external_cdbg_from_unverified_materialized(..., UnverifiedMaterializedEdgeAdapter::new(...))`.
The explicit type acknowledges that mutable proof-like fields in `ExternalPartitionResult` are
labels and consistency checks, not authenticated derivation from predecessor bytes. This is not the
promised scalable provenance path. The smallest next slice is therefore an owned streaming-final
`EDGE` result from the external reducer with the same canonical edge-table root, followed by exact
lockstep equivalence against this capped oracle adapter. Chunking and local compaction should begin
only after that transfer-of-ownership seam passes mutation, cleanup, resource, and determinism
tests.
