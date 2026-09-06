# ADR 0027: opaque capabilities for source-backed evidence

- Status: accepted for the v0.4 experimental integration boundary
- Date: 2026-09-05
- Depends on: ADR 0022, ADR 0023, ADR 0024, ADR 0025, and ADR 0026

## Context

Several experimental records contain a source root, canonical table root, and
conservation totals.  Those fields can reveal accidental mutation only when the
consumer already trusts the record producer.  If a public caller can construct
or mutate the record, that caller can change a transition, graph link, or
placement and recompute every unkeyed digest.  The resulting object is internally
consistent but is not derived from the registered input.

An unkeyed content hash is an integrity identifier.  It is neither an
authorization token nor proof of derivation.  Calling a freely forgeable record
`authenticated` would overstate the evidence and can convert an invented
adjacency into apparently read-backed sequence.

## Decision

Every API that confers a source-backed scientific claim uses an opaque capability:

1. Its integrity-bearing fields are private and it has no public unchecked
   constructor or mutation method.
2. It is produced only by a constructor that consumes or replays an immutable,
   registered upstream capability and checks terminal authentication.
3. It exposes checked, read-only views and scalar getters.  A caller may copy
   reported rows for analysis, but a modified copy is ordinary unverified data
   and cannot be converted back into the capability by supplying new hashes.
4. A downstream capability binds the complete upstream identity, algorithm and
   schema versions, scientific configuration, complete canonical content, and
   declared transformation.  Operational paths, thread count, and scheduling
   remain outside scientific identity.
5. Independent validators either replay the registered source or consume the
   same opaque upstream chain.  Structural self-validation alone is named
   `validate_structure`, never `authenticate`.
6. Serialization is one-way evidence reporting until a bounded parser can
   reconstruct and verify the complete ancestry chain.  Deserializing JSON,
   TSV, GFA, or FASTA alone never recreates a source-backed capability.

The required experimental chain is:

```text
registered Spool
  -> authenticated raw-count capability
  -> authenticated retention capability
  -> authenticated compacted-graph capability
  -> authenticated read-transition capability
  -> witnessed-child capability
```

The paired path is independently capability-bound:

```text
authenticated compacted graph + transition ledger + witnessed child
  -> opaque authenticated pair-graph adapter
registered Spool + authenticated pair-graph adapter
  -> opaque complete-placement artifact
  -> lineage-checked calibration/replay analysis
  -> pair annotations over existing paths only
```

### Pair-graph ancestry amendment

A materialized `PairPathGraph`, even when internally consistent, is not an
authenticated graph. The source-backed pair mapper accepts only the opaque
adapter minted by `adapt_authenticated_witnessed_child`. Minting requires a
complete replay of the witnessed child against its originating
`AuthenticatedCompactedGraph` and `TransitionLedger`. Its root binds the common
source, exact-count equivalence, compacted ancestry, transition evidence,
witnessed-child content and authentication, and exact pair-graph content.

The raw pair producer and authenticated pair-path analyzer are crate-private.
The adapter module owns the private ancestry-token constructor; sibling modules
can validate and read the token but cannot construct it. A public caller-created
`PairPathGraph` can enter only `analyze_unverified_pair_paths`, whose result is
not eligible to mint a source-backed bundle record.

Public structs remain useful for format tests, external interchange, and small
oracles, but their type or constructor name must include `Unverified`,
`MaterializedAdapter`, or equally explicit wording.  They cannot enter the
source-backed reconstruction constructor.

## Consequences

- Tests must attempt forged transition rows, omitted topology candidates,
  altered count rows, cross-library calibration, and changed mapper domains.
  Recomputing all public digests must not create a source-backed capability.
- Pair tests must additionally swap genuine children, compacted graphs,
  transition ledgers, and spools across sources; mutate adapter targets and
  roots; and prove worker-count-independent roots.
- Some validators repeat expensive source replay.  That cost is accepted until
  an authenticated persistent artifact reader carries the same derivation
  guarantee.
- Opaque types reduce convenient struct-literal use.  Test-only corruptors may
  exist inside the defining module, but must not compile into the public API.
- Cryptographic signatures or MACs are not required for the local integrity model.
  Simultaneous replacement of the executable and all run-owned bytes is outside
  that checksum model.  The API still must prevent ordinary library callers
  from manufacturing evidence claims.

## Rejected alternatives

- Recompute an unkeyed SHA-256 after accepting caller-provided rows.  This proves
  only that the supplied rows match the supplied digest.
- Trust a matching `source_root` field.  The field can be copied from an
  unrelated genuine result.
- Validate totals without complete candidate replay.  Conservation does not
  reveal an omitted unwitnessed graph link.
- Treat documentation warnings as a type boundary.  Production integration is
  fail-closed in code.

## Promotion gate

No component may be described as source-authenticated in the production path
until its public API passes a compile-time construction audit and mutation tests
show that a caller cannot forge, omit, relabel, or cross-bind evidence without
registered-source replay.  This rule is about derivation integrity, not an
external identity claim.
