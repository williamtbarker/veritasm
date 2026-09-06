# Experimental authenticated exact-count retention

Status: **experimental, unqualified, research use only**. This module is not
connected to the stable command, does not establish an assembly improvement,
and makes no sensitivity, accuracy, organism-detection, absence, clinical,
compendial, production-readiness, or performance claim.

The implementation is `src/experimental/retention.rs`. It realizes only the
exact-retention portion of [ADR 0026](adr/0026-authenticated-retention-and-multik-portfolio.md).

## Contract

Structural validation of an arbitrary `ExternalPartitionResult` is private to
the module and produces only an internal, explicitly unverified seal. A hash of
self-consistent caller fields is not source provenance. There is no public API
that promotes a free-standing `ExternalPartitionResult` or
`SpoolExternalResult` into a source-authenticated capability.

The upstream `SpoolExternalResult` itself is now field-private, non-`Clone`,
and source-produced. Its `validated()` operation checks the complete exact
table and returns only a borrowed immutable view. Retention consumes that view;
it never accepts a caller-constructed or caller-mutated bridge result.

`authenticate_spool_external_counts` instead requires an opaque `&Spool` and
the exact `&SpoolExternalOptions`. It calls `build_spool_external_counts`
itself, causing the physical spool verification and complete scientific
replays, then reconciles the result descriptor, registered lower-case spool
digests, read/fragment cardinality, exhaustive window accounting, support unit,
replay counts, source root, child binding, and operational high-water
relationships. Only that path can mint an opaque `SpoolAuthenticatedRaw` and
its field-private, constructor-free `SourceEquivalence` capability.

The bridge currently makes three logical verifier calls: one explicit call and
one while opening each authenticated iterator. Each verifier call separately
hashes the whole-file and framed pretrailer digest preimages while decoding the
complete structured record stream in one bounded forward descriptor traversal.
The bridge therefore performs three integrity physical read passes plus two
terminal scientific replays, or **five full-file-equivalent physical read
passes**. It records
`integrity_verification_calls = 3`,
`integrity_physical_read_passes = 3`, `scientific_replay_passes = 2`, and
`total_physical_spool_read_passes = 5`. This deliberately exposes the current
algorithmic I/O cost; it is not a measured throughput claim.

`retain_spool_authenticated_counts` accepts that capability plus one explicit
rule. `retain_spool_external_counts` is the convenience call that performs the
spool replay and retention together:

- `RetainAll` retains every exact row;
- `InclusiveSupport { minimum_support }` retains exactly rows whose full-key
  support is greater than or equal to the nonzero threshold.

The two rules are different scientific declarations. `RetainAll` and inclusive
threshold one materialize identical rows but deliberately have different
decision, retained-table, and retention roots. Threshold zero is rejected.

Every raw key contributes one `(full key, exact support, retained byte)` frame
to the decision ledger in globally increasing full-key order. The retained
adapter preserves the upstream strict `(bucket, full key)` route order required
by the compacted graph consumer. Full exact keys and exact support values alone
decide inclusion. Minimizers and virtual buckets are recomputed only to prove
canonical routing. SHA-256 roots authenticate results but do not define
membership. No Bloom filter, fingerprint, hash collision, graph topology, or
reference match can retain or discard a key.

`SpoolAuthenticatedRaw` and `RetainedCountArtifact` have no public constructor,
do not implement `Clone`, and expose no mutable table reference.
`retained_counts()` first validates its source-equivalence capability and all
self-contained transformation invariants, then returns a borrowed
`&ExternalPartitionResult`. `validate_against_authenticated_raw` replays every
decision and compares every retained row, all conservation totals, scientific
roots, resource accounting, and operational roots against an existing
spool-backed capability. `validate_against_spool` performs a fresh physical
spool replay before making the same comparison.

The borrowed `ExternalPartitionResult` is explicitly a **materialized adapter**.
Its `source_identity` is the retention root; its support and distinct-key totals
describe retained rows only. Its run catalogs, replacement ledger, run count,
open-file high-water, and temporary-byte fields are empty or zero. It does not
claim that retained rows survived a distinct external run. Raw external-run and
spool telemetry remains in the caller-owned `SpoolAuthenticatedRaw` and must be
reported separately if operational provenance is presented. The one-call
convenience wrapper does not copy that telemetry into the retained artifact;
callers needing it must use the two-stage API and its checked upstream-result
view.

## Validation before projection

Before allocating a decision index or output table, the implementation checks:

1. wide `k`, minimizer length, and nonzero virtual-bucket count;
2. raw source identity against the expected binding;
3. declared distinct-key count against vector length and the raw-key limit;
4. nonzero support for every row;
5. inactive-bit validity and canonical form of every full key;
6. exact minimizer and virtual-bucket recomputation;
7. strict `(bucket, full key)` row order, which also rejects duplicate keys;
8. checked support summation against the declared support mass; and
9. complete raw-table and authentication roots.

Noncanonical input is rejected, never silently normalized. A permutation of an
otherwise valid table is rejected rather than normalized because route order is
part of the authenticated external-count contract. Decision-ledger order is a
separate deterministic global full-key ordering.

The transformation checks both conservation equations:

```text
raw keys    = retained keys    + discarded keys
raw support = retained support + discarded support
```

Empty input is valid. Its raw, authentication, decision, retained-table,
retention, and operational roots are ordinary SHA-256 results over the exact
empty preimages described below; no all-zero digest is used.

## Resource and complexity model

Let `E` be the raw distinct-key count and `R` the retained count. Raw and
retained scans are linear. Globally sorting exact-key indices for the complete
decision ledger costs `O(E log E)` comparisons. Materialization is `O(E)`.
There is no data-dependent retry loop or quadratic key lookup.

All variable-sized transformation vectors are admitted before allocation and
created using `try_reserve_exact`. Arithmetic and platform-width conversions
are checked. The decision-index vector is destroyed before allocation of the
retained table, so the dynamic heap projection is:

```text
max(
    E * size_of::<usize>(),
    R * size_of::<ExactSupportCount>(),
    E == 0 ? 0 : k
)
```

The last term is the fallibly allocated decoder used while independently
recomputing minimizers. `max_accounted_bytes` covers this projection.
`max_raw_keys` bounds validation and sorting work; `max_retained_keys` bounds
output cardinality. Exact allocator capacities and both projected and accounted
peak bytes are recorded. The operational root binds all three configured limits
and those measurements.

The accounting excludes the caller-owned raw table, the artifact after
ownership transfer, allocator metadata, fixed stack/hash state, and bounded
diagnostic strings. It is configured-payload accounting, not measured process
RSS. The transformation performs no filesystem or network operation.

## Version 1 root framing

Every root uses SHA-256 and begins with the listed NUL-terminated ASCII domain,
then little-endian algorithm version `1`. Packed DNA keys and minimizers are
fixed-width 32-byte big-endian values. Integer scalar fields are little-endian;
tags are one byte. There are no variable-width or delimiter-dependent fields.

| Root | Domain | Remaining ordered preimage |
|---|---|---|
| Raw table | `veritasm:retention-raw-table:v1\0` | common source root; raw binding; `k`; minimizer length; bucket count; support-unit tag; raw key count; raw support; for every strict route-ordered row: bucket, minimizer, full key, support |
| Raw authentication | `veritasm:retention-raw-authentication:v1\0` | common source root; raw binding; raw-table root; graph/scientific fields; raw key count; raw support |
| Decision ledger | `veritasm:retention-decision-ledger:v1\0` | common retention preamble; every globally full-key-sorted row: full key, support, retained byte |
| Retained table | `veritasm:retention-retained-table:v1\0` | common retention preamble; every retained row in strict route order: full key, support |
| Retention | `veritasm:retention-artifact:v1\0` | common retention preamble; decision-ledger root; retained-table root |
| Core operational | `veritasm:retention-operational:v1\0` | retention root; raw-key limit; retained-key limit; accounted-byte limit; decision-index capacity; retained-table capacity; projected peak; accounted peak |
| Source equivalence | `veritasm:retention-source-equivalence:v1\0` | complete fixed-width spool descriptor; common source root; raw binding; raw-table root; raw-authentication root |
| Source operational | `veritasm:retention-source-operational:v1\0` | source-equivalence root; the three limits used while authenticating raw counts |
| Public artifact operational | `veritasm:retention-artifact-operational:v1\0` | source-equivalence root; source-operational root; core-operational root |

The common retention preamble is: algorithm version; common source root; raw
binding; raw-table root; raw-authentication root; graph/scientific fields;
rule tag (`0` retain-all, `1` inclusive); threshold (`0` for retain-all);
raw/retained/discarded key counts; and raw/retained/discarded support masses.

Resource limits never alter a scientific root. They alter only acceptance and
the operational root. The algorithm version is bound into every root. Changing
the minimizer length or bucket count changes raw/configuration roots even if the
same full keys happen to be retained.

## Deliberate boundary

This slice does not yet make downstream public result types unforgeable.
Production integration therefore requires a later opaque retained-to-compacted
wrapper that consumes `&RetainedCountArtifact`, validates its borrowed adapter,
constructs the compacted child internally, and returns another opaque checked
artifact. Authenticated reconstruction must accept only that wrapper, not a
caller-mutated `ExternalPartitionResult` or `CompactedGraphResult`.

This module also does not perform correction, rescue, approximate counting,
Bloom filtering, graph cleaning, contig reconstruction, cross-k selection,
taxonomy, or adventitious-agent detection. Scientific qualification and
end-to-end production claims remain future work.
