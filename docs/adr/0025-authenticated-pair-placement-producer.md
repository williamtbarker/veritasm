# ADR 0025: Produce authenticated exact pair placements from the immutable spool

- Status: accepted for an isolated experimental implementation; not accepted for the stable CLI
- Date: 2026-09-05
- Scope: paired `VTSPOOL1` input and an opaque `AuthenticatedPairGraph`
- Depends on: ADRs 0007, 0012, 0019, and 0022

## Decision

Add an experimental `pair_mapper` producer between authenticated spool replay and
`pair_path::PlacementEvidenceInput`. The producer exhausts and authenticates one opened spool
descriptor, verifies that supplied unitigs are the exact graph catalog, uses `IndexedExactMapper`
to enumerate zero-mismatch full-read placements on all and only linear unitigs, and returns two
disjoint, deterministic placement inputs for model calibration and replay. Its source-backed result
is opaque: fields are private, it is not clonable or publicly constructible, and downstream
integration must accept its sealed pair-level `AuthenticatedPlacementEvidencePair` capability
through read-only getters. This prevents calibration/replay subsets from different producer runs
being combined. The generic public
`pair_path` placement structs remain explicitly unverified caller evidence.

This producer does not traverse graph links, change graph topology, scaffold, spell sequence, or
claim a globally complete graph placement. It is a provenance and exact-enumeration boundary for
the existing `LinearUnitigOnly` domain.

## Trust boundary and identities

Before mapping, recompute and bind the common `transition_source_root(spool)` used by the
transition/count/reconstruction plane. This `common_source_root` is not replaced by a new,
incompatible source identity. Also compute a richer `pair_mapping_source_root` over a versioned
domain and the registered spool schema, whole-spool and pretrailer SHA-256 values, fragment/read
counts, input mode, full scientific configuration, and every ordered source descriptor. Each descriptor binds lane ordinal, mate role,
FASTX format, raw-transport digest, logical-decoded digest, record count, and base count. Hex is
strictly decoded to 32 bytes before hashing. Caller-supplied expected graph and both source roots
must match.

The producer then calls `Spool::iter`, which verifies the spool before opening replay and
authenticates the same descriptor, exact registered length, trailer, digests, metadata, and EOF as
it is consumed. No result is returned unless terminal authentication succeeds.

For each read, compute

```text
R = SHA256(domain || common_source_root || pair_mapping_source_root ||
           lane || fragment_ordinal || mate_role ||
           normalized_id_digest || sequence_length || sequence ||
           quality_presence || quality_length || quality)
```

and for each paired fragment compute

```text
F = SHA256(domain || common_source_root || pair_mapping_source_root ||
           lane || fragment_ordinal ||
           R1_role || R1 || R2_role || R2)
```

The authenticated fragment must contain exactly two reads in `R1,R2` order with identical
normalized identifier digests. Single-end and malformed roles are rejected. Thus identical read
sequence in two lanes or fragment instances remains distinguishable.

The mapper provenance binds its algorithm ID/version, target universe, literal seed length,
zero-mismatch/full-read-verification rule, candidate cap, eligibility rule, and the typed-unavailable
policy. The producer result root binds graph root, both source roots, mapper identity, split rule,
both subset read-set roots, both compatible placement-input roots, and conservation telemetry.
SHA-256 authenticates provenance and accidental substitution against expected roots; it is not a
MAC against an actor able to replace both bytes and trusted expected digests.

## Exact placement and unavailable semantics

The producer applies the spool's recorded minimum base quality. FASTA reads have no quality gate.
FASTQ reads are eligible only when every Phred+33 byte is valid and meets that threshold. Sequence
is uppercased and is eligible only when every base is A, C, G, or T.

For an eligible read, `IndexedExactMapper` uses a literal seed index but verifies the complete
oriented read before accepting a match. Its stable order is `(unitig ID, start, strand)`. Each
physical exact mapper group becomes one pair-path group containing one placement; the producer
rechecks segment identity, linear topology, interval length, coordinates, and strand. A
candidate-cap result never publishes the mapper's discarded prefix.

Every read is classified exactly once:

| Producer class | Groups | Compatibility field | Meaning |
|---|---:|---:|---|
| exact single group | all groups | 0 | complete exact enumeration in the linear-unitig domain |
| exact multiple groups | all groups | 0 | complete exact enumeration in the linear-unitig domain |
| ineligible ambiguity/quality | empty | +1 | no certified placement set |
| candidate cap | empty | +1 | enumeration intentionally incomplete |
| unmapped / possible graph junction | empty | +1 | no linear-unitig placement; graph-link placement was not evaluated |

`PairPlacementEvidence::junction_spanning_reads_unavailable` is the existing conservative
compatibility field and counts all three unavailable classes. Typed telemetry preserves the actual
reason; the producer does not relabel those reasons as observed junction crossings. A fragment with
either unavailable mate is excluded by `pair_path`. The certificate's
`complete_within_declared_domain=true` means every supplied fragment and read is accounted exactly
once and every published group is a complete enumeration for an eligible, completed mapper query;
all other reads are explicitly unavailable. It does **not** assert complete placement across graph
links or phasing/scaffolding evidence.

## Graph binding

The public source-backed entry point accepts an opaque `AuthenticatedPairGraph`,
not a caller-created `PairPathGraph` and unitig slice. The adapter first replays
an `AuthenticatedWitnessedChild` against its originating authenticated
compacted graph and transition ledger. It deterministically materializes the
exact graph catalog and mapper targets. The producer compares that private
catalog and targets one-for-one after sorting by full ID: count, ID, topology,
length, and SHA-256 of uppercase ACGT sequence must agree. Only catalog entries
marked linear enter the mapper. The expected graph root is mandatory; the
producer root binds the source-equivalence, compacted-ancestry, transition,
witnessed-child, exact pair-graph, and adapter-authentication roots. Target order
and worker completion order cannot alter results.

## Deterministic calibration/replay separation

For each fragment, compute

```text
S = SHA256(domain || common_source_root || pair_mapping_source_root ||
           graph_root || mapper_identity || split_salt || F)
bucket = big_endian_u64(S[0..8]) mod denominator
```

Assign the fragment to calibration exactly when `bucket < numerator`; otherwise assign it to replay.
Require `0 < numerator < denominator`. Numerator, denominator, salt, and split algorithm version are
bound into mapper parameters, subset roots, and producer root. The two outputs are separately sorted
by `(lane, fragment ordinal, fragment digest)`. Conservation requires
`calibration_fragments + replay_fragments = authenticated_spool_fragments`, no identity overlap,
and exactly two classified reads per fragment. Empty subsets are valid evidence of a small or skewed
input, not silently rebalanced data.

## Bounded work and memory

The configuration sets hard ceilings for fragments, reads, input bases, graph sequence bases,
mapper candidates per read,
mapper work counters, total groups/placements, mapper index bytes, query bytes per worker, decoded
batch bytes, batch fragments, result bytes, and total accounted bytes. It also sets an admitted
worker count. Before index construction, a mapper plan must fit the index and total budgets.

Replay uses bounded decoded batches. Before replay opens, the exact configured batch-slot capacity
and the worst-case calibration and replay outer-vector capacities are admitted, fallibly reserved,
and rechecked from actual allocator capacities. These vectors never grow during replay. The spool
decoder reports a conservative allocation bound before allocating a body. A batch is sealed before either its fragment or decoded-byte limit would
be crossed. Mapping may run in a private fixed-size Rayon pool, but results retain the input batch
ordinal and are committed only in ordinal order. At most one decoded batch, one mapped-batch result,
the immutable mapper index, and the growing final outputs coexist. Vector capacities, read
normalization/reverse-complement query allowances, fixed worker-stack allowances, and a documented
fixed overhead are included in admission. Allocator metadata, caller-owned graph/unitig/spool
objects, operating-system buffers inside `SpoolIter`, Rayon internals, and thread stacks beyond the
fixed allowance remain outside an RSS claim.

Any configured ceiling, checked-arithmetic failure, allocation failure, authentication failure,
root mismatch, malformed role/identifier, invalid mapper result, or conservation mismatch aborts
the whole production call. No partial certificate or placement input is returned. Candidate-cap
indeterminacy is the sole per-read work-limit outcome represented as typed unavailable evidence.

## Independent validation obligations

The isolated implementation must establish:

1. a literal exhaustive forward/reverse-complement oracle produces the same exact placements;
2. paired roles, lane and fragment identity, normalized identifier synchronization, and two-read
   conservation are enforced;
3. source-byte mutation and expected source/graph root mismatch fail before publication;
4. every scientific and allocation/work cap fails closed without a partial result;
5. input-unitig order, batch size, and worker counts produce byte-identical roots and records;
6. calibration and replay are disjoint, complete over fragments, and stable under execution-plan
   changes; and
7. current stable and the documented MSRV pass focused tests and strict Clippy.

## Rejected alternatives

- Seed hits, minimizer fingerprints, or unitig IDs alone are not placement evidence.
- An unavailable or unmapped mate is not converted into a nearby endpoint or graph link.
- Calibration and replay are not split by arrival order or thread number.
- Reads are not silently dropped to satisfy output or memory limits.
- The producer does not mutate the graph, infer insert size, select a path, scaffold, or claim
  adventitious-agent detection.

## Smallest implementation slice

Implement both source roots, config/read/fragment/subset/result roots, graph-catalog verification, bounded
paired spool replay, exact mapper conversion, typed telemetry, deterministic hash split, and
compatibility validation against `pair_path`. Stop before any graph-link-spanning mapper, stable CLI
wiring, insert calibration, path scoring, or scaffold consumer. This slice closes the caller-trust
gap for exact linear-unitig placements without expanding the scientific claim.
