# Experimental paired-end library model

Status: isolated Rust library substrate; not used by `veritasm assemble`.

`src/library_model.rs` deterministically summarizes explicit high-confidence mate placements by
lane. The producer must establish one complete exact placement group per mate. This module validates
geometry and metadata but does not remap reads or prove that upstream assertion.

Each lane reports a reconciled inclusion/exclusion ledger; exact FR, RF, FF, and RR counts in
left-to-right coordinate order; integer nearest-rank P10, P25, lower-median, P75, and P90 spans plus
extrema for every orientation; the unique leading orientation; its exact rational dominance result;
and one explicit availability state. The outer-envelope span is not a gap estimate. The P90-P10 gate
is a transparent compactness check, not a modality test or confidence score.

Candidates may arrive in any order. Duplicate `(lane, fragment)` identities and corrupt evidence are
fatal. Different unitigs, closed graph walks, and coincident starts are measured exclusions. Lanes
are never pooled.

The module is not called by the pipeline, serialized into stable output, or exposed by the CLI. An
`available` result means only that configured internal evidence gates passed. It does not establish
the true library preparation, a biological molecule, adjacency, or phasing. Mapping selection and
PCR duplicates can bias it.

The accumulator retains every observation identity and one span per included observation, with
explicit observation, lane, identifier-length, and conservative memory limits. That estimate is
admission control, not a process-RSS guarantee. See
[ADR 0008](adr/0008-experimental-paired-library-model.md) for exact semantics and promotion gates.
