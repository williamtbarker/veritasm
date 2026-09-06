# Archived predecessor development benchmark

Status: historical context only; **not VeritAsm validation or benchmark evidence**.

The current pre-registered protocol and non-result ledger are in [../BENCHMARK.md](../BENCHMARK.md).
The frozen predecessor audit, source identity, and limitations are in
[BASELINE_AUDIT.md](BASELINE_AUDIT.md). This file preserves one earlier engineering measurement so it
cannot be mistaken for, or silently promoted into, the VeritAsm scorecard.

## Historical observation

On 2026-09-03, an earlier implementation comparison used one deterministic 200,000-read, 150-base,
Q40, single-end synthetic dataset (30,000,000 bases; input SHA-256
`396cac8bd9878e19f8fb902b21e7e6002078a862bb474ba8be75529d49f48e8d`). The recorded host was Linux
6.18.35 x86-64 in a container with an Intel Xeon Platinum 8573C, nine visible cores, Rust 1.98.0, and
GNU time 1.9.

| Historical executable | Workers | Wall time | Peak RSS |
|---|---:|---:|---:|
| First educational prototype | 1 | 18.65 s | 145,076 KB |
| Frozen implementation ancestor | 1 | 4.06 s | 14,860 KB |
| Frozen implementation ancestor | 4 | 3.37 s | 19,512 KB |

The ancestor's one- and four-worker FASTA files shared SHA-256
`f42b48f47e907453b45f5c5925fbf9e1bb9551ecf4aa8c15b2e43a59b36e644b`. The two historical programs
did not emit functionally equivalent assemblies: one emitted strand mirrors and the other collapsed
them. Consequently, even this narrow timing is not an apples-to-apples biological-output comparison.

A second ancestor-only smoke used 200,000 paired fragments (400,000 reads, 60,000,000 bases) in two
gzip streams. The recorded four-worker run took 7.18 s with 25,004 KB peak RSS. Its input SHA-256
values were:

```text
2bf6ff338fae5e4c0068aeee45d7b9da8f2f20d1157762cf08a1f87dff1d7296  R1
4b746ad9456d11b8c85e4d9f724ca39dda93ac281b41d407110a068b7c6252bd  R2
```

There was no paired-input result for the first prototype. That is a historical capability
difference, not evidence for VeritAsm.

## Why this cannot support a current claim

- VeritAsm was not one of the measured executables.
- The measurements have no retained current experiment manifest, raw resource log, executable
  digest, generated-read artifact, or repetition distribution in this repository.
- One synthetic workload cannot establish general accuracy, speed, or memory behavior.
- The historical tools emitted different biological outputs.
- No truth-scored genome fraction, base error, false-junction, duplication, or minor-path metric was
  evaluated.

No number in this file may be quoted as VeritAsm performance or as evidence that VeritAsm improves on
its frozen ancestor. Re-execution under [../BENCHMARK.md](../BENCHMARK.md) must retain all successes,
failures, timeouts, OOMs, invalid outputs, and regressions.
