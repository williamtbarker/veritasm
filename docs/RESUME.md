# Development roadmap

Start with [current status](STATUS.md), [the module map](MODULE_MAP.md), and
`AGENTS.md`. Use Rust 1.85 or newer with rustfmt, Clippy, Bash and Unix tools.
Run `./scripts/verify.sh` before making changes to establish a local baseline.

Keep the original development archive and diagnostic fixtures outside the
repository. Some reported failures below still depend on those maintainer-held
fixtures; their complete reproducers are not checked in. Record a minimal
reproducer before changing behavior.

## Next steps

### 1. Reproduce and classify the adjacency report

Relevant files: `src/graph.rs`, `src/compact.rs`, `src/pipeline.rs`,
`tests/graph_properties.rs`, and the maintainer-held diagnostic fixtures.
Prerequisite: understand the fixed-k topology contract and locate the smallest
input/expected-result record in the development notes. A graph edge and a
read-witnessed transition are different assertions; do not silently redefine one.

Baseline command: `cargo test --locked --test graph_properties`.
Acceptance: a retained minimal reproducer and an explicit determination of
whether it violates the current contract or belongs to the proposed witnessed
reconstruction extension. Any behavior change needs a focused regression and
an approved contract decision. The current checkpoint does not resolve this report.

### 2. Reproduce the small paired snapshot/resource failure

Relevant files: `src/experimental/multik_pipeline.rs`,
`src/experimental/multik_bundle.rs`, `src/bin/veritasm-multik.rs`, and
`tests/multik_cli.rs`. Prerequisite: task 1's scope decision and the original
small paired fixture or a clearly documented new minimal reproducer.

Baseline command: `cargo test --locked --test multik_cli`.
Acceptance: the specific failure is reproduced and the simultaneous live-memory,
compressed-size, and decoded-document limits are explained. If fixed, test exact
boundaries and deterministic output; do not conceal the problem by broadly
increasing defaults. The reported resource failure remains open.

### 3. Finish or explicitly retire dormant recompaction integration

Relevant files: `src/experimental/witnessed_recompaction.rs`,
`src/experimental/mod.rs`, `src/experimental/evidence_reconstruction.rs`, and
`src/experimental/transition_witness.rs`. Prerequisite: the earlier tasks' contract
and resource decisions. Preserve the old file even if choosing a different design.

Initial command after deliberate module wiring:
`cargo check --locked --all-targets --all-features`.
Acceptance: either a tested, documented opt-in integration with independent
evidence/conservation regressions, or an explicit documented decision to keep it
deferred. A green check while the file remains undeclared does not cover it.

## Completion milestones

The immediate tasks above address the known blockers. Completing the project
also requires the following work, corresponding to the roadmap in the
[README](../README.md#roadmap-to-a-complete-release).

### Feature completion and release scope

Document the supported fixed-k and multi-k workflows and the intended role of
each experimental module. Finish integration, error handling, schemas, examples
and end-to-end tests for every feature included in the release. Review correction,
pair evidence and external graph processing against that scope. A module must
not be advertised as supported merely because it compiles. List deferred research
separately, with its remaining work and rationale.

Completion evidence: one feature-to-command-to-test inventory; no undeclared
or unreachable code advertised as a release capability; reproducible examples
that require no maintainer-held fixtures.

### Resource behavior, portability and resilience

Measure peak process memory, temporary disk use, file-descriptor demand and
runtime over the supported workload. Check configured limits and concurrent
allocation accounting. Exercise malformed input, disk exhaustion, interrupted
work, namespace races and existing-output protection. Run the complete Linux
and macOS matrix with current stable Rust and the declared MSRV, then complete
the documented fuzzing and dependency/license checks.

Completion evidence: recorded operating limits, clean failure behavior, retained
failure-injection results, successful platform CI, and a documented resolution
for every release-blocking defect. See [SECURITY.md](../SECURITY.md),
[FUZZING.md](FUZZING.md) and [the release procedure](VERIFICATION.md).

### Independent evaluation and measured improvements

Complete the generator/evaluator qualification plan before using its metrics to
judge the assembler. Freeze representative datasets, comparator versions, commands,
resource limits and metrics. Keep held-out evaluation separate from development
fixtures. Retain failures and negative results. Profile measured bottlenecks and
repeat correctness, determinism and evaluation checks after each accepted change.

Completion evidence: a reproducible evaluation package with metric definitions,
independently checked calculations, per-case quality/runtime/memory results and
clear limitations. See [VALIDATION.md](../VALIDATION.md),
[BENCHMARK.md](../BENCHMARK.md) and [QUALIFICATION_PROTOCOL.md](QUALIFICATION_PROTOCOL.md).

### Release and maintenance

Freeze the supported CLI and artifact schemas, document migration from prior
versions, and finish installation and troubleshooting instructions. Resolve the
working-name decision before a formal product or crate release. Build source
and binary artifacts from a tagged revision, verify clean installation, and
publish checksums, release notes and the exact verification scope. Establish a
private security-reporting route and a support/dependency maintenance policy.

Completion evidence: a new user can install the release, run its examples and
reproduce the documented validation workflow without private setup knowledge.
All seven README milestones must have retained evidence or an explicit release
scope decision; a green unit-test suite alone is insufficient.
