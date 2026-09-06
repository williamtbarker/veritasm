# ADR 0023: Read-witnessed multi-k reconstruction without child-contig laundering

- Status: accepted for the next isolated v0.4 vertical slice; stable promotion remains gated
- Date: 2026-09-05
- Scope: exact transition evidence, evidence-constrained compaction, multi-k parent ancestry,
  reconstruction profiles, and an experimental transactional bundle
- Preserves: ADRs 0004, 0005, 0010, 0012, 0016, 0018, 0019, 0020, and 0022
- Supersedes: no stable behavior or schema

## Context and audit result

The current stable pipeline is a deterministic, transactional, fixed-k unitig assembler. Its
immutable spool is the appropriate source of truth, but its GFA boundary links are the complete
incoming-by-outgoing topology product. A valid overlap therefore does not prove that one supplied
read observed that transition.

The experimental components do not yet form a safe reconstruction pipeline:

| Component | Reusable result | Missing for reconstruction |
|---|---|---|
| `spool_external` / external reducer | source-bound exact k-mer counts in either support unit | retained per-occurrence transition provenance and a persistent child snapshot |
| `multik` | independent in-memory occurrence-only children and non-projecting relations | authenticated external ancestry; its relations cannot make a contig or become read support |
| `compacted_dbg` | exact wide-k unitigs, links, edge steps, and conservation | it materializes a child and its boundary links are topology candidates, not read-transition evidence |
| `quality_correction` | immutable raw spectra and a replay journal | scientific qualification; a corrected spelling is not an original-read witness |
| `pair_path` | a conservative prototype over linear-unitig placements | graph-path placement, junction-spanning reads, aggregate path support, completed independent review, and promotion evidence |
| stable `bundle` | no-replace commit, manifest, deterministic rendering | one-k assumptions and a closed artifact inventory |

In particular, a higher-k count or a child contig is derived evidence. Reusing either as though it
were a new read observation would multiply support and can launder a path that no read traversed.

## Decision

### 1. Make a source-read transition the atomic reconstruction witness

For a child graph at k, an adjacent pair of oriented k-mers spells exactly one `(k+1)`-mer. The
original-read witness plane therefore scans every authenticated spool read for accepted windows of
length `q=k+1`. A window is accepted only when all q bases are in the same read and pass the frozen
ambiguity and base-quality rules. It never crosses a mate, record, ambiguity, or rejected-quality
boundary.

One event records:

```text
(k, canonical_qmer, observed_orientation,
 lane_ordinal, fragment_ordinal, mate_role,
 normalized_id_digest, zero_based_read_offset)
```

`canonical_qmer` is the complete packed q-mer, never a hash. `observed_orientation` distinguishes
canonical, reverse-complement, and fixed-point observations. The source coordinate tuple is unique;
duplicate tuples are an integrity error. Events are externally sorted by complete transition key and
source coordinate. A reduced row reports checked accepted-window-occurrence support, checked distinct
supplied-fragment-instance support, and a SHA-256 digest of the sorted full event frames. This reports
two defined quantities; neither is called coverage, abundance, confidence, or molecule support.

Before q-mer sorting, the authenticated spool replay must present those coordinates in strict
lane/fragment/mate/offset order. One retained previous coordinate is sufficient to reject a duplicate
or reordering exactly, including across spill boundaries. Run authentication then preserves that
validated event multiset through every merge, and source replay remains the final evidence validator.

The physical transition is the reverse-complement orbit of the q-mer. Decoding its canonical
spelling yields the exact oriented prefix and suffix k-mers. Both endpoint canonical k-mers must be
present in the same retained child graph before the transition is eligible. A positive endpoint
count, overlap, minimizer, Bloom hit, higher-k child edge, or child-contig substring is not a
transition witness.

The q-mer/event format is admitted only for `3 <= k <= 126`, because the existing exact packed type
ends at q=127. k=127 remains valid for the disconnected count/compaction oracle but is not accepted by
this reconstruction slice.

Bloom filters or sketches may reject definite nonmembers before exact lookup. They cannot admit a
transition, suppress an event required by the exact table, define an ID, or enter final evidence.

### 2. Constrain, rather than reinterpret, each child graph

Each child is still built independently from the same descriptor-authenticated spool. Its raw exact
k-mer table is retained or filtered under its own recorded support rule; another child never adds a
key or support.

Evidence-constrained compaction consumes one child `CompactedGraphResult` and that child's exact
transition ledger:

1. Validate the child count-table, compaction, source, support-unit, and transition ancestry roots.
2. For every consecutive edge-step pair inside a compacted unitig, derive its complete q-mer and
   require an exact transition row. Split at every missing row.
3. For a closed walk, check the closing transition too. One missing transition opens and splits the
   representation; a surviving closed walk remains only a graph-topology candidate.
4. For every raw compacted link, derive its complete q-mer. Retain the link only with an exact row.
5. Preserve every read-witnessed branch in GFA and stop FASTA paths at branches or fixed points.
   Never select an alternative by traversal order or support magnitude.
6. Record every candidate transition as `admitted_original_read`,
   `excluded_endpoint_not_retained`, or `excluded_no_original_read_witness`. Exclusion is visible and
   checksum-bound; it is not silently erased.

This first implementation may undercompact after exclusions; it must not rejoin segments merely
because filtering made their degree one. Every emitted sequence of more than one k-mer is validated
by independently decoding each adjacent q-mer and finding its exact source-bound ledger row. A
single-k-mer segment needs exact child-edge support but has no transition to claim.

This local rule does **not** prove that one read spans an entire contig, that successive local
adjacencies are globally phased, or that a segment is a complete molecule or genome.

### 3. Ship a multi-k portfolio before any cross-k path projection

The first end-to-end slice creates independent children at a sorted, unique list of k values, applies
the read-witness constraint to each child, and writes one parent evidence bundle. It does not splice,
extend, polish, or vote between child sequences. Existing `supports`, `contains`, `conflicts`, and
`unresolved` relations remain annotations and cannot authorize sequence.

Two explicit output profiles are frozen:

- `diversity_preserving` emits every child-scoped evidence segment in the primary FASTA. Exact
  sequence duplicates at different k remain separate because their evidence domains differ.
- `exact_agreement_consensus` writes a primary presentation record only for a group with
  byte-identical canonical sequence and the same topology in at least two distinct k children. It
  retains every member child ID and evidence root. Singletons and groups confined to one k remain in
  the complete child outputs and receive `excluded_no_cross_k_exact_agreement`; they are not called
  cross-k agreement. The profile performs no substring removal, majority vote, abundance choice,
  bubble collapse, or base edit.

Both profiles always ship the complete child-scoped `segments.fasta`, the full witnessed GFA, and all
branch and decision rows. Any sequence difference remains an alternative in both profiles. The word
`consensus` therefore means exact presentation agreement only, not biological consensus, strain
resolution, or haplotype phase.

A later high-k extension through a lower-k path requires another accepted ADR and a complete path
oracle. It may not be smuggled into this portfolio implementation.

### 4. Pair and corrected evidence do not enter Slice A

Slice A admits only `original_read_transition` witnesses. The bundle records pair-driven
reconstruction and correction as `disabled_unqualified`, not as zero evidence.

A future pair witness may authorize only transitions on an already present graph path after all of
ADR 0019 is satisfied: complete graph-path placement alternatives (including link-spanning reads), a
frozen lane model, exhaustive bounded path enumeration, one canonical compatible path, distinct
fragment support, a contradiction ledger, and no indeterminate search. It cannot bridge components,
insert gap bases, or create an overlap. The current linear-unitig-only prototype is insufficient.

Corrected-only sequence is a typed corrected layer and is never relabelled as original-read support.
The current correction experiment cannot add a primary reconstruction edge. Raw and corrected arms
remain separate until prospective ablation satisfies ADR 0018.

### 5. Authenticate the complete parent/child ancestry

All hashes below use raw 32-byte digests, little-endian integers, one-byte enum tags, and
`u64_length_le || bytes` framing for variable byte strings. Tables are strictly sorted and include
their row count. Paths, thread count, timestamps, and run filenames are excluded.

```text
source_root = SHA256("veritasm:multik-source:v1\0" ||
  spool_sha256 || spool_pretrailer_sha256 || spool_schema ||
  fragment_count || read_count || input_mode || min_base_quality)

transition_root(k) = SHA256("veritasm:transition-ledger:v1\0" ||
  source_root || k || row_count || complete_sorted_transition_rows)

child_root(k) = SHA256("veritasm:multik-child:v1\0" ||
  source_root || retention_root || compacted_ancestry_root ||
  equivalence || k || support_unit || minimizer_length || virtual_bucket_count ||
  exact_edge_table_root ||
  raw_compacted_graph_root || transition_root(k) ||
  constrained_graph_root)

authenticated_child_root(k) = SHA256("veritasm:authenticated-witnessed-child:v1\0" ||
  source_root || compacted_ancestry_root || transition_root(k) || child_root(k))

parent_root = SHA256("veritasm:multik-parent:v2\0" ||
  source_root || profile || sorted_child_count || sorted_(k,child_root) ||
  sorted_authenticated_child_roots ||
  exact_agreement_relation_root || pair_state || correction_state)
```

The source root identifies bytes and QC semantics, not an author or biological truth. A child source
binding must be recomputed from the spool; merely asserting the same source root is insufficient.
The ordinary bundle `manifest.sha256` independently binds the exact serialized inventory and avoids
self-reference.

A child segment ID is the full lowercase digest of domain, child root, topology, exact sequence,
and ordered full edge-step/orientation records, prefixed `mks-`. An exact-agreement presentation ID
hashes domain, source root, topology, sequence, and all sorted member segment IDs, prefixed `mkc-`.
Unequal preimages with the same digest are fatal. IDs and every artifact row have documented total
orders independent of scheduling.

### 6. Use a separate experimental bundle and preserve the stable command

The stable `assemble` command and schema 1.x remain byte-for-byte unchanged. Slice A is exposed only
as the plainly experimental `veritasm-multik` binary. It uses the same no-replace transaction
primitive after typed validation of every artifact.

The experimental bundle contains at least:

| Artifact | Contract |
|---|---|
| `segments.fasta` | every child-scoped witnessed segment, ordered by `(k,length desc,sequence,id)` |
| `contigs.fasta` | the selected presentation profile; never the source of evidence |
| `assembly.gfa` | all child witnessed segments and links; no cross-k GFA links |
| `segment_evidence.tsv` | child/source roots, exact edge and transition summaries, and limitations |
| `adjacency_evidence.tsv` | one row per admitted q-mer witness with both defined support units and event digest |
| `transition_decisions.tsv` | every raw topology candidate and its admitted/excluded status |
| `profile_decisions.tsv` | every primary-FASTA membership or exact-duplicate collapse decision |
| `run.json` | effective parameters, ancestry roots, conservation, resource telemetry, states, and limitations |
| `report.html` | self-contained rendering from the same validated typed data |
| `schema/` and `manifest.sha256` | exact schemas and complete checksum inventory |

No child temporary path or derived child sequence is written into an evidence field. `assembly.gfa`
retains all witnessed branch alternatives; `contigs.fasta` is never described as the full graph.

### 7. Keep a realistic bounded-memory route

Transition events use fixed-width authenticated external runs and the pathless catalog/accounting
rules of ADR 0020. Event aggregation deduplicates fragment support by sorted fragment ordinal rather
than an unbounded hash set. The implementation processes one whole-resident compacted child at a
time, completely validates the opaque child against its sources, writes and re-verifies a bounded
one-way reporting snapshot, then releases the capabilities before the next child. The snapshot is
not promotable authenticated evidence. Current compaction therefore remains subject to an explicit
per-child retained-graph cap and is not a bounded-RSS claim.

The current alpha loads verified reporting snapshots for a final whole-resident pass under small
encoded-snapshot and explicitly accounted decoded-payload limits. Exact-agreement grouping compares
complete sequence bytes and topology; hash equality alone is never accepted. This pass is not yet the
external grouping design and is not a production-scale or bounded-RSS claim. Limits cover child
count, k/q values, input events, external runs, aggregate temporary bytes, file descriptors, retained
edges, topology candidates, segments, output bases, snapshot bytes, decoded reporting payload,
presentation payload, artifact rows, and verification scratch. Limit exhaustion is typed failure; it
never produces a normal partial bundle.

Partition-local compaction and global boundary reconciliation remain the production-scale path from
ADR 0016. Until that replaces the whole-resident child, only the explicitly measured/capped domain
may be reported.

## Smallest authorized implementation slice

The first integrated executable restricts Slice A to `3 <= k <= 63` even though the transition
format has a wider isolated domain. Slice A uses an authenticated deterministic retention rule,
original-read transitions only,
no cross-k extension, no correction, no pair-driven traversal, and the two presentation profiles
above. Retain-all and an inclusive positive support threshold are both valid only when the exact
rule, threshold, decision ledger, retained table, and source ancestry are bound and reported. It is
enough to test an exact multi-k FASTA/GFA/evidence bundle without making an unsupported assembly
improvement claim.

The intended source API is:

```rust
pub fn build_transition_ledger(
    spool: &Spool,
    options: &TransitionLedgerOptions,
) -> Result<TransitionLedger>;

pub fn compact_retained_counts(
    retained: &RetainedCountArtifact,
    limits: CompactedGraphLimits,
) -> Result<AuthenticatedCompactedGraph>;

pub fn constrain_unverified_compacted_child(
    child: &CompactedGraphResult,
    transitions: &TransitionLedgerView<'_>,
    limits: ReconstructionLimits,
) -> Result<WitnessedChild>;

pub fn constrain_authenticated_compacted_child(
    child: &AuthenticatedCompactedGraph,
    transitions: &TransitionLedger,
    limits: ReconstructionLimits,
) -> Result<AuthenticatedWitnessedChild>;

pub fn build_multik_portfolio(
    source: SourceAncestry,
    children: &[WitnessedChildSnapshot],
    profile: ReconstructionProfile,
    limits: PortfolioLimits,
) -> Result<MultiKPortfolio>;

pub fn validate_no_unsupported_adjacencies(
    portfolio: &MultiKPortfolio,
    ledgers: &[TransitionLedgerView<'_>],
) -> Result<()>;
```

Implementation file plan:

| File | Change |
|---|---|
| `src/experimental/transition_witness.rs` | event framing, RC orbit, external reduction, dual support counts, roots, and replay validator |
| `src/experimental/evidence_reconstruction.rs` | split/filter compaction, child roots/IDs, exact-agreement grouping, and independent adjacency validator |
| `src/experimental/multik_pipeline.rs` | authenticated one-child-at-a-time orchestration and cleanup ownership |
| `src/experimental/multik_bundle.rs` | typed artifacts, schemas, streaming validators, and transaction adapter |
| `src/transaction.rs` | extract the already-tested generic no-replace staging/manifest mechanism without changing stable bytes |
| `src/bin/veritasm-multik.rs` | explicitly experimental CLI; no hidden invocation from stable `assemble` |
| `docs/EXPERIMENTAL_MULTIK_RECONSTRUCTION.md` | user-visible semantics, examples, limits, and prohibited claims |
| `schema/multik_*.schema.json` | run, GFA, TSV, and manifest descriptors with fixed enum/order contracts |

`src/experimental/mod.rs`, `src/lib.rs`, `Cargo.toml`, CLI tests, schema tests, and documentation indexes
must be updated. `scripts/source-package-files.txt` must explicitly list this ADR, every new source,
schema, test, fuzz target, and user document; `scripts/package_source.sh` and clean-extraction tests
must reject a missing or unlisted file. No implementation is complete while the package allowlist
omits its evidence contract.

## Required falsification tests

1. Exhaustively compare transition canonicalization and event aggregation with a literal string
   oracle for small k; reverse complementation must preserve physical transition identity.
2. Mutate event coordinate, orientation, full q-mer, support unit, source root, k, count, or digest;
   ancestry validation must fail.
3. Demonstrate that two individually retained overlapping k-mers do not join without an accepted
   q-mer event, including a stable-compactor boundary cross-product fixture.
4. Split on a missing internal transition, open a cycle with a missing closure, preserve hairpins and
   fixed points, and retain every witnessed branch without choosing one.
5. Independently decode every q-length window in every emitted segment and every GFA link; each must
   resolve to one exact ledger row or a future typed admitted pair row. Tampering must fail.
6. Show that a higher-k edge, a cross-k `supports` row, a child contig, and a Bloom false positive
   cannot satisfy the validator when the source transition row is absent.
7. Prove `diversity_preserving` retains separate equal-sequence child records, while
   `exact_agreement_consensus` collapses only byte-identical records and retains every differing
   branch/sequence in GFA and decision tables.
8. Compare complete bundle bytes at 1, 2, and 4 threads and under permuted child request order when
   the recorded operational parameters and source-coordinate domain are identical. Across spill size
   and merge depth, compare the complete scientific rows and roots while expecting run-replacement
   telemetry to differ. A lane permutation must preserve sequence/support results when lane order is
   biologically non-semantic, but source-bound roots and coordinate evidence are expected to differ.
9. Exercise limit-minus-one, exact-limit, and plus-one cases for every event, run, graph, snapshot,
   output, and verification allocation; inject short writes, corruption, ENOSPC, interruption, and
   cleanup failure.
10. At every bundle failpoint, prove an existing destination remains byte-for-byte unchanged and no
    incomplete destination is presented as success.
11. Cover plain/content-inspected-gzip FASTA/FASTQ, SE/PE/multiple lanes, ambiguity/quality boundaries,
    empty/short reads, malformed pairs, corrupt/truncated gzip, repeats, uneven depth, and two-strain
    mixtures.
12. Add property tests and fuzz targets for event framing, transition replay, snapshot parsing, and
    the experimental bundle; validate GFA with an independent parser.

Narrow test files are `tests/multik_experimental_cli.rs`, `tests/multik_schema.rs`,
`tests/multik_determinism.rs`, and `tests/multik_atomicity.rs`. Stable fixed-k golden bundles must not
change.

## Promotion gates

- Every emitted adjacency has a validator-replayed original-read event or, in a later version, a
  separately typed fully qualified pair-path witness.
- Full child edge/support conservation and transition admitted/excluded conservation hold against
  the immutable spool oracle in both support modes.
- The external snapshot path replaces simultaneous whole-child retention and passes corruption and
  resource fault injection.
- Truth-known comparisons publish false junctions, unsupported sequence, recovery, base error,
  duplication, minor-path precision/recall, runtime, RSS, temporary disk, I/O, and failures.
- MSRV/current-stable Linux and Apple Silicon gates, fuzz smoke tests, package clean extraction, and
  complete-bundle determinism pass.

Passing Slice A tests establishes software invariants only. It does not establish higher sensitivity,
accuracy, speed, bounded RSS, production readiness, or source-classification performance.

## Rejected alternatives

- Concatenate child unitigs on overlap and call the result multi-k assembly.
- Let a higher-k child count increment lower-k read or fragment support.
- Treat every de Bruijn overlap or boundary cross-product as an observed transition.
- Pick a branch by largest support, first traversal, longest contig, or post-hoc truth score.
- Use a Bloom/filter/fingerprint/MPHF positive as final transition identity.
- Enable the current pair-path or correction prototype in primary FASTA.
- Hide unresolved branches by emitting only the selected presentation FASTA.
- Refactor the stable command and stable schema in the same change as this experimental slice.
