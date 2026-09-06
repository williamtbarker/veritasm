# ADR 0016: Build an exact external cDBG and multi-k evidence lattice

- Status: accepted as the v0.4+ production data-plane direction; implementation remains gated
- Date: 2026-09-05
- Preserves: ADRs 0003, 0004, 0006, 0010, 0012, and 0014 exactness/uncertainty boundaries

## Context

The stable fixed-k implementation is a strong small-data oracle, but its graph and compaction are
whole-resident and duplicate oriented/key/node state. Source-derived admission approaches hundreds of
bytes per retained key, while counting and spooling perform many complete traversals. The existing
partitioned/wide-k module materializes every occurrence and has incomplete corruption checks; it is
an oracle, not a scalable engine.

Literature supports exact minimizer/super-k-mer partitioning and direct compacted construction.
Literature also shows that small and large k solve complementary problems, but does not make
cross-layer path projection safe automatically. Large-k fingerprint collisions reported for GGCAT
are incompatible with this project's identity contract.

## Decision

### Exact external primitive

Create one reusable authenticated block-stream and deterministic external-reduction substrate.
Records carry a versioned domain, source/spool identity, exact key width, k, support unit, virtual
partition, ordinal interval, counts, lengths, and checksum. Full keys decide equality.

Support modes remain exact:

- occurrence mode represents every accepted source window exactly once;
- fragment mode counts a key at most once per immutable fragment ordinal across mates, segments,
  repeated windows, buffers, and workers.

Virtual partition IDs and final sort keys are independent of worker count. Physical streams are
multiplexed through a bounded descriptor pool. A verified replacement run is registered before its
predecessors are deleted. Failure preserves enough authenticated ancestry to reject or audit the
state; it never silently resumes from an unverified record.

### Exact keys and graph

Use width-specialized two-bit keys through the admitted range. A hash/minimizer/fingerprint/MPHF may
route or index, but lookup success is accepted only after full-key or sequence verification.

External oriented-side records compute degrees and fixed-point boundaries. Partition interiors are
compacted locally, and boundary stubs are stitched only after global exact-degree reconciliation.
The frozen graph representation uses packed sequence arenas, numeric IDs, endpoint side arrays, CSR
adjacency, and separate evidence arrays. The existing graph/compactor remains the differential oracle
for admitted small graphs.

### Multi-k evidence lattice

The first multi-k milestone builds independent exact child graphs from one authenticated spool. A
parent record relates child sequences/edges with typed `supports`, `contains`, `conflicts`, and
`unresolved` rows. Relations report evidence and create no bases or adjacencies.

Any later projected path must satisfy a no-unsupported-adjacency rule: every emitted adjacency is
replayable from original reads, from an explicitly admitted larger-k child edge, or from a frozen
lane-specific physical-link constraint. A small-k traversal by itself is not larger-k evidence.
Derived contigs do not increment original-read support.

## Rejected alternatives

- Decorating the current occurrence-sized partition oracle until it becomes the production engine.
- Retaining obsolete merge payload generations indefinitely.
- One universal 32-byte key type in every hot small-k path.
- Fingerprint-only identity for k>64.
- MPHF lookup without absent-key verification.
- Minimizer-space assembly as the default short-read graph.
- Variable-order storage presented as equivalent to iterative assembly.
- Colors presented as molecule-level phase.
- Merging child contigs before a typed provenance and conflict model exists.

## Promotion gates

1. Independent byte-string oracle for `(source, window, canonical key, owner)` plus mutation-negative
   tests for noncanonical keys, wrong owners, redistributed counts, and source mismatch.
2. Exact equality with stable counting in both support modes across key widths, partitions, spill
   thresholds, merge depths, thread counts, input orders, and collision-saturated routing.
3. Exact retained-edge, oriented-side, palindrome-boundary, unitig, and GFA adjacency equality with
   the stable graph on the oracle domain.
4. No lost or duplicated cross-partition edge, boundary, cycle, hairpin, or self-complemental object.
5. Actual ENOSPC/partial-write/interruption tests and verified predecessor-reclamation behavior.
6. Retained wall/CPU/RSS/temp-disk/I/O/FD evidence; no speed or scale claim from source inspection.
7. Multi-k parent output must remain deterministic and no projected sequence can enter stable FASTA
   until its separate no-unsupported-adjacency and scientific gates pass.

