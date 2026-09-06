# Experimental read-witnessed reconstruction

Status: implemented isolated ADR 0023 child transformation; not part of the stable CLI or a
production-readiness claim.

## Contract

The experimental evidence-reconstruction module consumes one structurally validated compacted
graph at 3 <= k <= 126 and the matching private-field transition-ledger view. An adjacency is
admitted only when the complete canonical (k + 1)-mer is present in that exact original-read
transition ledger. A higher-k edge, multi-k relation, Bloom membership result, overlap, or topology
candidate cannot substitute for the row.

The transformation:

1. independently checks every compacted sequence window against its exact step, orientation,
   support, identifier, and v2 typed-support provenance;
2. splits a unitig at every missing internal witness;
3. checks closed-walk closure separately and opens a cycle when closure or interior evidence is
   missing;
4. excludes each unwitnessed raw link, preserves every witnessed alternative, and adds a
   ledger-witnessed retained-endpoint transition when fixed-point representation omitted the raw
   candidate;
5. does not recompact after an exclusion;
6. records every raw topology candidate plus every transition whose endpoint was not retained; and
7. conserves every input canonical edge and its exact support without calling transition support
   coverage, abundance, confidence, or molecular support.

Every segment ID binds the child root, topology, exact sequence, and ordered full edge steps. The
child root binds the common source root, child retention root and compacted ancestry, typed support unit, exact
edge-table root, raw compacted graph root, exact transition root, constrained graph root, and source
equivalence state. The constrained graph root hashes pre-ID segment contents to avoid a circular
ID/root definition. The v2 raw-graph content root excludes `accounted_peak_bytes`; that value remains
structurally validated and reported as operational telemetry, so changing a nonbinding memory limit
does not alter scientific child identity.

## Source equivalence

The source-backed path accepts only constructor-produced opaque capabilities:

```text
Spool
  -> SpoolAuthenticatedRaw
  -> RetainedCountArtifact
  -> AuthenticatedCompactedGraph
  +  TransitionLedger
  -> AuthenticatedWitnessedChild
```

`compact_retained_counts` owns compaction from the checked retained table. Its graph, complete
content root, retention rule, retained-table root, source-equivalence root, and ancestry root are
private. `TransitionLedger` likewise has private rows and integrity fields and can only be minted by
replaying a `Spool`. Neither type implements `Clone` in normal builds, and a modified copy of a
reporting view cannot be passed to `constrain_authenticated_compacted_child`.

Any authenticated deterministic retention rule is admissible. The rule and threshold are bound in
the compacted ancestry. A transition whose canonical endpoints were removed is recorded as
`excluded_endpoint_not_retained`; every adjacency between retained edges still needs its exact
transition row. The source-backed constructor also reruns the complete reconstruction from both
opaque inputs and requires byte-structural equality before validation succeeds.

`constrain_unverified_compacted_child` remains available for isolated experiments over freely
materialized graphs and a checked ledger view. Its `Unverified` state is root-bound and cannot be
promoted into an `AuthenticatedWitnessedChild` by supplying caller-computed hashes.

The current capability is in-memory only. `AuthenticatedCompactedGraph::checked_graph()` rechecks
its private complete preimage but does not reread physical source bytes or rerun retention; callers
that need that stronger check must retain the upstream capabilities and invoke their explicit replay
validators. A witnessed child binds the retention decision ledger through the retention/ancestry
roots, but its own transition-decision rows only say that one or both endpoints were absent. They do
not identify the removed endpoint or duplicate the upstream per-key retention decision table.
Persistent restoration of any source-backed capability remains unimplemented.

## Independent validator

The no-unsupported-adjacencies validator re-admits all record counts, capacities, bases, and
validator scratch before allocation. It builds one fallible sorted segment-ID index, then:

- derives every q-mer directly from emitted segment bytes and finds the complete key in the ledger;
- derives every GFA-style boundary q-mer byte by byte in the declared orientations and compares the
  exact ledger row and both named support values;
- rejects higher-k-child, multi-k-relation, Bloom-membership, and topology-only evidence kinds;
- recomputes segment IDs, provenance, evidence digests, constrained root, child root, and
  conservation; and
- rejects duplicate IDs, links, decisions, unsupported transitions, malformed reverse-complement
  orientations, or exhausted limits.

This ledger-only validator proves positive adjacency membership but cannot by itself prove that a
freely supplied graph omitted no raw topology candidate. The source-backed validator closes that
gap: it consumes the opaque complete compacted graph, enumerates every unitig interior, closed-walk
closure, and raw compacted link again, reconstructs the complete decision table, and compares the
entire result. Serialized FASTA, GFA, TSV, or JSON remains one-way reporting and cannot recreate the
opaque source capability.

The implementation uses exact packed strings through q=127. Reverse complements describe one
physical transition orbit; self-reverse-complement edges remain explicit fixed points.

## Resource scope

Limits cover input edges, raw topology candidates, decision rows, output segments, potential links,
output bases, validator scratch, and accounted owned vector payload. Allocation is fallible. The
source-backed validator temporarily holds the reported child and one complete replay together, so
it requires twice the independently derived construction peak before starting that replay. The
whole compacted graph and transition ledger are caller-owned and already resident; allocator,
library, thread-stack, and operating-system overhead remain outside the payload count. This is
therefore not a bounded-RSS or production-scale claim.

Construction uses sorted-vector lookup. Support summaries use bounded sort scratch, and link
validation uses a single sorted ID index rather than a segment-by-link scan. The current module
materializes one child; ADR 0023's authenticated snapshot/bundle orchestration remains separate.

For `E` retained edges, `U` compacted unitigs, `C` raw topology candidates, `T` transition rows,
`S` output segments, and `L` output links, construction owns `O(E + U + C + T + S + L + output
bases)` payload. Exact lookups and deterministic sorting give a conservative
`O((E + C + T + S + L) log(E + C + T + S + L))` time bound; there is no segment-by-link nested
scan. Source-backed validation repeats one full reconstruction while retaining the first result,
which changes the constant factor and accounted peak but not that asymptotic bound.

## Scientific limits

A witnessed segment proves only that every local adjacency was observed in at least one accepted
window under the recorded QC rules. Different adjacent rows may come from different reads. The
result does not prove one read spans a segment, global phase, a complete source sequence, or
circular physical topology.

The module does not perform cross-k extension, voting, polishing, correction, pair-driven traversal,
source classification, reference assistance, or consensus inference. Passing its tests establishes
software invariants only, not improved sensitivity, accuracy, speed, memory use, or production
readiness.

## Focused verification

    cargo test --locked --lib evidence_reconstruction
    cargo clippy --locked --lib --tests -- -D warnings
    rustup run 1.85.0 cargo test --locked --lib evidence_reconstruction
    rustup run 1.85.0 cargo clippy --locked --lib --tests -- -D warnings

The 13 focused tests cover opaque and unverified ancestry, retain-all and inclusive-threshold
retention, missing interior and boundary witnesses, branch cross-products, complete and opened
cycles, IUPAC boundaries, endpoint exclusion, unsupported evidence kinds, every public reporting
field category, randomized digest-bit changes, exact resource boundaries, thread-pool determinism,
exhaustive DNA strings through length five at k=3, q=127, a 512-edge/2,080-link complete k=5 branch
graph, and a larger linear input. These fixtures are engineering evidence, not a scientific
benchmark.
