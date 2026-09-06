# ADR 0019: Lane-specific paired-end path evidence with mandatory abstention

- Status: accepted for isolated v0.4 experiments; stable traversal remains gated
- Date: 2026-09-05
- Preserves: ADRs 0003, 0008, 0015, and 0016 uncertainty and no-fabricated-join rules

## Context

Paired reads can distinguish existing graph continuations within the physical fragment span, but
multimapping, repeats, lane-specific library distributions, chimeric fragments, and incomplete path
enumeration can make the evidence non-identifying. Picking one alignment, pooling lanes, or stopping
search at a work cap and treating the surviving path as unique can manufacture a false join.

The stable v0.3 path reports pair observations between already constructed unitigs; it does not use
those observations to resolve graph paths. A traversal decision requires a stronger evidence model.

## Decision

1. Retain complete exact read-placement groups up to an explicit cap. A mapper-selected primary is
   diagnostic only and cannot establish uniqueness. The current domain is explicitly
   `linear_unitig_only`; junction-spanning placement is unavailable and quantified.
2. Infer orientation and empirical span/inner-gap intervals independently for each input lane, using
   only frozen eligibility rules and uniquely anchored observations. Report sample counts,
   exclusions, quantile rule, bounds, and failure to infer.
3. Bind the exact graph (`k`, full unitig identifiers/topologies/sequences, and canonical physical
   links), read-set identity, fragment/read identities, mapper algorithm/version/parameters, placement
   groups, and completeness certificate into deterministic content roots. Reject any mismatch or
   within-read disagreement in exact placement span. Bind the complete result—including configuration,
   worker count, accounting, models, decisions, aggregates, and summaries—into a separately versioned
   result root with a recomputation validator. Public caller-built placement records and their
   self-consistent roots remain explicitly unverified. Source-backed integration accepts only the
   opaque witnessed-child-derived pair-graph adapter and crate-sealed pair-level capability issued by
   `pair_mapper`; its opaque result separately binds the complete graph ancestry and producer root to
   the complete analysis root.
4. Use distinct calibration and replay artifacts under one library-source lineage and identical mapper
   semantics. Freeze every lane model before replay and never pool lanes. Generic reports expose exact
   calibration/replay fragment-identity reuse; the authenticated mapper capability requires distinct
   subset roots and rejects any reuse.
5. Enumerate every existing graph path compatible with the placement alternatives and declared
   length limits. Store search states in a pre-reserved parent-index arena and reconstruct only
   compatible target paths. Depth, state, examined-arc, reconstructed-path, path-count, memory, or work exhaustion makes the result
   `indeterminate`; it does not make the paths seen so far complete.
6. A fragment is a unique-path observation only when the complete search has exactly one compatible
   canonical physical path across all placement alternatives and no cross-component, orientation, or
   geometry alternative exists. Geometry-pruned alternatives are adverse rather than silently absent.
   Placements on, and paths through, non-linear compacted topology cannot support replay. Reciprocal
   orientations of the same physical path are one result; distinct paths remain alternatives. A
   zero-link observation is `trivial_within_unitig`, not path support.
7. Count each immutable supplied fragment identity at most once per aggregate path row. Retain conflicts,
   outliers, multimapping, cross-component observations, incomplete searches, and other abstentions
   in typed ledgers. Attribute a contradiction or incomplete search to every candidate path in its
   exact unordered endpoint-segment domain. If a work cap prevents complete domain attribution, mark
   every candidate aggregate path indeterminate; never drop adverse evidence merely because its
   compatible-path list is empty.
8. Never let one fragment authorize reconstruction. An aggregate experimental constraint requires a
   configurable minimum of at least two distinct supporting fragments and no contradiction or
   indeterminate observation. This still does not authorize graph mutation or sequence synthesis.
9. Pair evidence can select among graph sequence already present. It cannot invent bases, bridge
   disconnected components, or silently convert a scaffold gap into contiguous sequence. Contigs
   and scaffolds remain distinct artifacts.
10. Admit calibration/replay counts, total graph sequence/hash work, retained graph bytes, endpoint
    attribution/result bounds, and concurrent worker-search bounds against explicit checked limits.
    Report configuration, worker count, input counts, geometry pruning, examined arcs,
    reconstructed-path work, and each cap hit.

## Promotion gates

- Exhaustive bounded-graph comparison against an independent path enumerator.
- Repeat, branch, cycle, reciprocal-orientation, multimapping, outlier, mixed-lane, and chimeric-pair
  fixtures with explicit expected abstentions.
- Exact equality of roots, models, decisions, aggregates, and summaries across input order,
  batching, and thread counts; the explicitly recorded requested worker count is execution telemetry
  and therefore differs when that input differs.
- Kill tests proving every incomplete enumeration route is indeterminate and no predecessor path is
  promoted after a limit fires.
- Lane-model calibration and false-junction comparisons on frozen truth-known and public paired data.
- Stable output schemas that expose model provenance, all outcome counts, path support, contradiction,
  and limitations before pair-driven reconstruction is enabled.
- A graph-link-spanning mapper with a content-bound complete-enumeration certificate. The current
  stable linear-unitig read audit cannot supply this and therefore cannot qualify pair traversal.
- Independent validation of upstream read/fragment content-root and mapper-certificate generation.
- Mutation tests for every result-root field class and independent root recomputation.
- A 100,000-segment chain regression demonstrating bounded near-linear search/reconstruction work,
  plus exact boundary tests for graph-base, examined-arc, and path-reconstruction caps.

## Consequences

Many repeats will remain unresolved, especially when inserts do not span the repeat or placements are
not unique. That is the correct result when physical evidence cannot phase the alternatives. A
future global haplotype claim would require additional long-range evidence and separate validation;
local pair-supported paths do not establish global phase.

## References

- Bankevich et al., SPAdes, <https://doi.org/10.1089/cmb.2012.0021>
- Prjibelski et al., exSPAnder, <https://doi.org/10.1093/bioinformatics/btu266>
- Wick et al., Unicycler, <https://doi.org/10.1371/journal.pcbi.1005595>
