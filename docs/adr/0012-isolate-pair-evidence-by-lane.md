# ADR 0012: Isolate raw paired-read evidence by lane and defer topology changes

- Status: accepted for stable evidence output
- Date: 2026-09-04
- Supersedes: the cross-lane aggregation portion of ADR 0003

## Context

The stable pair audit classifies synchronized pairs after exact read remapping. Cross-unitig endpoint
observations are currently keyed only by the two endpoints, so matching observations from different
lanes are silently pooled. Different lanes can have different orientation, insert distributions,
failure modes, or library preparation. Pooled raw counts cannot later support an auditable lane model
or conservative repeat-resolution decision.

The experimental library model already uses lane identity, integer empirical quantiles, explicit
exclusions, and unavailable states. It is not integrated into the pipeline, and its caller-supplied
target metadata is not yet protected by a run-wide immutable target registry. The current pair audit
also rejects every multiply placed mate, including the observations most relevant to repeat
resolution.

## Decision

Add `lane_ordinal` to every raw pair-link record and to its aggregation/sort key. Validate the lane
against the immutable input source catalogue. Reconcile both global cross-unitig support and the sum
of lane-specific records. Do not pool raw evidence across lanes in the stable artifact.

Also emit a lane-qualified pair-state summary. Pre-register every paired lane so a lane with zero
observations in a particular state remains visible. For each lane, the state sum must equal supplied
fragments from that lane and link support must equal that lane's `cross_unitig_observation` count.
The global state table must equal the checked sum of the lane tables. These reconciliations are part
of typed bundle validation, not assertions inferred from rendered text.

This schema change bumps the affected pair-link artifact and bundle contract. The row's status remains
an exact unique-placement pair observation. It does not become a join, a gap estimate, a library
model, or proof of adjacency.

Defer all topology-changing pair use from this milestone. A later promotion requires:

1. an immutable target registry shared by every observation;
2. a replayable mapping-evidence spool carrying lane, fragment, mate, ambiguity, and placements;
3. deterministic per-lane orientation and span inference from eligible same-target observations;
4. a second pass applying only finalized lane models;
5. graph-path distance, contradiction, duplicate-fragment, and independent-support rules;
6. a resolver that selects only an existing graph continuation compatible with every retained
   constraint, otherwise abstains; and
7. truth-known evidence showing zero false resolutions in the preregistered identifiable and
   unidentifiable repeat suites.

Lanes may be merged only after an explicit compatibility test whose inputs, thresholds, and outcome
are reported. Online model fitting followed by immediate decisions is prohibited because it makes
the outcome depend on input order.

## Required evidence

- two lanes with identical endpoint tuples remain two output groups;
- every paired lane has the complete ordered state set, including explicit zero rows;
- each lane's states and links reconcile independently before global reconciliation;
- replay or processing order for already assigned immutable lane ordinals, fragment order within the
  supported stream contract, and thread count cannot change sorted output or counts; changing lane
  declaration order deliberately changes `lane_ordinal` provenance;
- every link lane exists in the input catalogue and has both R1 and R2 sources;
- total link support equals the global `cross_unitig_observation` state;
- malformed or inconsistent lane metadata fails before bundle commit;
- legacy single-lane output remains semantically equivalent apart from the declared schema column.

## Consequences and exclusions

The change repairs evidence provenance but does not improve contiguity. No FASTA or GFA topology is
changed. Global haplotype reconstruction, scaffolding, and adventitious-agent detection remain outside
the claim boundary. Leaving a repeat unresolved is the correct result when fragment evidence cannot
phase it uniquely.
