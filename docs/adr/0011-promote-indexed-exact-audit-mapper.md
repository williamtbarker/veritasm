# ADR 0011: Promote the indexed exact mapper with end-to-end provenance

- Status: accepted for the stable construction-read audit
- Date: 2026-09-04
- Supersedes: the stable exhaustive-mapper choice in ADR 0007; its mapping semantics remain normative

## Context

The stable audit compares every eligible read, in both orientations, with every possible interval of
every emitted linear unitig. That implementation is a useful oracle, but its work grows with the
product of read count and assembled target length. A one-megabase engineering probe confirmed that
this path is not operationally useful at realistic read counts.

The repository already contains a literal rarest-q-gram index. Every candidate is verified against
the complete read, the target universe and exact zero-mismatch semantics match the exhaustive
implementation, and exhaustive plus randomized differential tests exist. A deterministic probe
showed materially less query work on its fixture. That is sufficient to justify integration, not an
accuracy or end-to-end speed claim.

The current bundle renderer hardcodes the exhaustive mapper even though `AuditSummary` carries mapper
identity. Integrating the index without repairing that path would publish false provenance.

## Decision

Promote `literal_rarest_seed_zero_mismatch`, with a fixed stable q of 15, to the construction-read
audit path. Retain the exhaustive implementation as a test oracle and for direct differential tests.

The mapping contract remains unchanged:

- targets are all and only emitted linear unitig strings, independently;
- reads must pass the declared ambiguity and base-quality eligibility policy;
- placements are exact, zero-mismatch, zero-based half-open intervals;
- both `+` and `-` placement groups remain distinct, including palindromic reads;
- output ordering is complete unitig identifier, start, end, then strand;
- exactly `limit` verified groups is complete and group `limit + 1` makes the result indeterminate;
- an indeterminate prefix contributes no placement evidence.

Introduce one typed mapper descriptor containing algorithm ID, algorithm version, target universe,
and seed length. Construct it with the mapper and carry it through the audit result, pipeline,
`BundleData`, bundle validation, `run.json`, schema, and report. No output layer may reconstruct mapper
identity from constants unrelated to the instance that performed the work.

Index allocation is part of the audit phase's admitted memory. The mapper must expose its conservative
persistent allocation bound. Each query additionally receives the caller's derived-memory allowance;
the effective per-read placement allowance is the smaller of that value and the mapper's configured
ceiling. Memory exhaustion is a fatal typed resource error, never unmapped or candidate-limit state.

The index remains serially queried in spool order for this milestone. Deterministic parallel mapping
requires ordinal-tagged results, a global resource lease, and byte-equivalence evidence before
promotion.

## Required evidence

- exhaustive and randomized equality to the independent interval-scanning oracle;
- equality at candidate-limit boundaries and under reordered targets;
- exact short-read fallback, palindromic read, absent seed, repeated seed, closed-target exclusion,
  duplicate-ID, and memory-boundary tests;
- a pipeline regression proving `run.json` names the actual indexed mapper and q;
- unchanged scientific artifacts across supported thread counts and repeated runs;
- end-to-end wall time and peak RSS measured with index construction included before making a speed
  claim.

## Consequences and exclusions

The promoted mapper accelerates only exact segment-local read-back auditing. It does not align through
errors, graph links, or closed walks; infer biological origin; validate an assembly independently; or
resolve a repeat. Its index stores every target q-gram and is not the final production graph mapper.
A later minimizer/syncmer and banded-alignment mapper needs a new completeness argument, ambiguity
model, resource contract, and differential validation.
