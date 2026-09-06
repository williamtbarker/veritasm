# ADR 0003: Paired reads annotate without joining

- Status: accepted for 0.1 implementation
- Date: 2026-09-03

## Context

Paired reads can constrain repeat traversals, but false, chimeric, multimapping, role-swapped, or
insert-incompatible pairs can create convincing false junctions. The first slice needs genuine pair
use without presenting fabricated gap sequence.

## Decision

Treat a synchronized pair as one supplied fragment instance for default k-mer support. After unitig
compaction, brute-force place eligible reads with zero mismatches against linear emitted unitigs and
group pairs having one accepted placement group per mate on different unitigs into canonical oriented
endpoint observations. Keep ineligible, indeterminate, unmapped, multiply placed, same-unitig, and
endpoint-tie cases in the precedence-ordered mutually exclusive summary defined in `ARCHITECTURE.md`.
Version 0.1 records mate role and strand but makes no library-orientation compatibility decision.

Version 0.1 does not infer an insert distribution or gap and emits no GFA `J` record. Pair observations
remain in `pair_links.tsv`; exclusions remain in `pair_audit_summary.tsv`. They do not join, extend,
reorder, or alter unitig sequences.

## Consequences

- Pair data contribute an exact, mapping-rule-conditional observation table but cannot improve FASTA
  contiguity in 0.1.
- Repeat resolution claims are deferred.
- Exact placement cost and accepted-group explosion require an explicit bound and `indeterminate`
  outcome.
- A future insert estimate would be mapping-selected and require a separate contract; version 0.1
  emits none.

## Deferred acceptance for sequence joining

A future joining mode requires a new ADR, explicit gap semantics, unique compatible continuation,
minimum independent linkage, contradiction handling, simulated and real insert/chimera tests, and a
demonstrated false-junction benefit. GFA evidence alone is not authorization.

## Evidence

See the paired de Bruijn graph, exSPAnder, and BESST literature in `RESEARCH.md`.
