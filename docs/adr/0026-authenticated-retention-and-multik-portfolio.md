# ADR 0026: authenticated retention and a non-splicing multi-k portfolio

- Status: accepted for an experimental vertical slice; not a stable-product decision
- Date: 2026-09-05
- Depends on: ADR 0022, ADR 0023, ADR 0024, and ADR 0025

## Context

The current experimental spool bridge produces exact counts for every accepted read window.  The
compacted-graph oracle consumes every row it receives.  Treating that table as though a configured
support threshold had already been applied would be a scientific-integrity defect: the spool
binding contains the requested profile, but the materialized table still contains low-support
keys.

The first useful multi-k executable also needs a deliberately modest contract.  Independently
assembling several exact child graphs is supportable now.  Selecting or splicing their paths into
one supposedly superior sequence is not.  A higher-k edge, a lower-k overlap, a Bloom hit, or a
derived child sequence is not an original-read adjacency witness.

## Decision

### Exact retention is its own authenticated transformation

Introduce a typed retained-count artifact between raw external reduction and graph construction.
It contains:

- the common authenticated spool/source root;
- the complete raw-count binding and canonical raw table root;
- the support unit and an explicit retention rule (`retain_all` or inclusive exact support);
- raw and retained key counts and support masses;
- discarded key count and support mass;
- a domain-separated decision-ledger root over every globally key-sorted
  `(key, support, retained)` row; and
- a retained `ExternalPartitionResult` whose source identity is the retention root and whose
  totals describe only retained rows.

The implementation validates and canonicalizes the raw table before projection, pre-admits the
complete output vector, uses checked arithmetic, preserves deterministic route order, and verifies
the following conservation equations:

```text
raw keys    = retained keys    + discarded keys
raw support = retained support + discarded support
```

The retained table root binds the raw table root, source root, k, support unit, rule, threshold,
counts, support masses, and every retained full key/support pair.  No hash, minimizer, Bloom
filter, or fingerprint decides retention.  A low-support key is reported as excluded; it is not
silently forgotten.

Graph and reconstruction APIs must accept this typed artifact (or an equivalently checked borrowed
view), not a caller-mutated copy of the raw external result.  Authenticated reconstruction proves
that the compacted child came from the retained table and that its transition ledger came from the
same original spool.

### First executable is a portfolio, not a cross-k assembler

The experimental command builds a single immutable spool and, for every strictly increasing
configured k:

1. replays the spool into exact raw counts;
2. applies the authenticated retention rule;
3. constructs and validates a compacted graph child;
4. independently builds exact `(k+1)` original-read transition evidence;
5. splits or disconnects every topology adjacency lacking that evidence; and
6. emits every complete child in child-scoped FASTA, GFA, evidence-table, and machine-readable rows.

Children share a source root but retain independent k, retention, graph, transition, and child
roots.  Their sequences are never concatenated, extended, voted, selected, or interpreted as
independent observations.  The top-level portfolio reports all children in configured k order and
labels differences as unresolved cross-k evidence.  Its exact-agreement primary FASTA requires the
same byte sequence and topology in at least two distinct k children; singleton groups remain in all
complete child outputs and are explicitly excluded from that presentation. It does not choose a
preferred sequence.

For paired input, an optional exact linear-unitig mapper may produce lane-specific pair-path
annotations under ADR 0025.  These annotations can mark one already existing path as an
experimental constraint only after complete-within-domain enumeration and distinct-fragment
aggregation.  They do not add an edge, join unitigs, scaffold sequence, or phase variants.

Quality correction is disabled in the default portfolio.  `RawOnly` is the only generally enabled
mode until prospective ablations qualify another mode.  `ConsensusExperimental` must be a separate
output arm and must never replace the raw child.

### Transaction and output contract

The command acquires an exclusive no-replace destination lease before opening input.  All work and
rendering occur in a private sibling directory.  It authenticates every iterator to terminal EOF,
validates every child independently, writes a sorted file manifest with SHA-256 and byte length,
then atomically publishes the directory without replacing an existing path.  Any failure removes
only run-owned staging data; failure to clean up is itself reported.

Scientific records are independent of spill size, fan-in, work path, and scheduling. The first
integrated alpha executes serially and rejects every requested thread count other than one; it does
not silently accept an unused worker hint. Operational telemetry has a separate root and remains
outside scientific roots. The report distinguishes modelled owned-payload accounting from process
RSS and makes no RSS-bound claim.

The command and every output page carry the labels `experimental`, `unqualified`, and
`research use only`.  It makes no sensitivity, accuracy, organism-detection, absence, clinical,
compendial, production-readiness, or performance claim.

## Rejected alternatives

- Filtering a cloned `ExternalPartitionResult` while retaining its old source identity: this would
  conceal a scientific transformation.
- Using the support threshold embedded in an upstream configuration digest as evidence that rows
  were actually filtered: configuration is not execution.
- Choosing the longest child, the largest N50, or the child closest to qualification truth: this is
  post-hoc selection and can create optimistic results.
- Cross-k overlap extension without original-read transition replay: graph compatibility is not a
  source observation.
- Bloom-defined retention or topology: false positives cannot be allowed to become sequence.
- Pair-supported synthetic joins in the first slice: the present mapper cannot enumerate reads
  spanning graph links.

## Promotion requirements

Promotion out of the laboratory command requires all of the following:

1. independent literal retention and no-unsupported-adjacency oracles;
2. exhaustive small-domain, mutation-negative, limit-boundary, and deterministic tests;
3. fuzz targets for the new artifact readers and renderers;
4. stable and MSRV format, Clippy, test, documentation, release, and package gates;
5. Linux and real Apple Silicon execution evidence;
6. truth-known raw-versus-retention-versus-k ablations with all failures published;
7. frozen modern comparator binaries/configurations and measured RSS/temp/I/O;
8. no-regression evidence for stable FASTX, gzip, pair synchronization, and atomic-output behavior;
   and
9. a claim ledger in which every asserted improvement names its dataset, endpoint, uncertainty,
   comparator, resource ceiling, and exact artifact digests.

Until those gates pass, the portfolio is a serious falsification target and integration substrate,
not a production-grade assembler release.
