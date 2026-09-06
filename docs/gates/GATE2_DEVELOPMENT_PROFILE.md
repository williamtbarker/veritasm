# Gate 2 development profile: frozen 0.1 core

- Status: **diagnostic only; not release science**
- Date: 2026-09-03
- Source baseline: `ade474f861255bc64ccfad08427295e1c4188e08`
- Executable SHA-256: `b4aa1f11f3c7987f11f04d575ff3458707f84d825968f6255a4048a51e30e014`
- Host: Linux 6.18.35, x86-64, Intel Xeon Platinum 8573C, nine logical CPUs exposed
- Measurement: GNU `/usr/bin/time`, one observation per cell, no warm-up or cache control

This run exists only to find engineering work. It used the development generator rather than an
admitted simulator, has one observation per cell, and cannot support a performance or reconstruction
claim.

## Input

The generator used scenario `mixture`, seed `9102026`, 20,000 paired fragment instances, 100-base
reads, and a fixed 250-base insert. Its reads are exact, substitution-free, deterministic Q40
observations and are not a sequencer model.

| File | SHA-256 |
|---|---|
| R1 gzip FASTQ | `1c8eea6810d5859e6df9274e1c9836a0dd9c1ffd843725939f6442a69d6e2515` |
| R2 gzip FASTQ | `ed81714a3642ff484fe9ab74b56bff77368982da55b4da310b67ed46968a0a10` |
| Truth FASTA, not supplied to assembly | `56580fa8e66ff86abc70d05a7fb6b9eb13e990646999417447a2866147463d85` |

## Observations

The fixed command used `k=31`, the thresholded fragment-instance profile, and the default limits.

| Mode | Threads | Wall seconds | Peak RSS KiB | Bundle manifest-file SHA-256 |
|---|---:|---:|---:|---|
| no remap | 1 | 1.32 | 47,684 | `7e461655567bef73497563f65c4652a17bd0cf4284b9960b013dce3aea49fabb` |
| no remap | 2 | 1.40 | 47,812 | same |
| no remap | 4 | 1.45 | 47,952 | same |
| no remap | 8 | 1.88 | 48,612 | same |
| no remap, earlier diagnostic | 4 | 1.20 | 47,712 | not compared with the later thread series |
| exhaustive remap, earlier diagnostic | 4 | 1.58 | 48,948 | not compared with the later thread series |

The four no-remap bundles were byte-identical at the committed manifest boundary. More threads were
slower on this small workload. This is consistent with the current implementation parallelizing only
batched k-mer extraction while serializing most other phases; one short diagnostic does not establish
a general scaling curve.

## Toolchain check anomaly

An in-place Rust 1.85 all-target test run compiled the library and binary tests, then encountered
`Permission denied` because one just-linked CLI test artifact had mode `0644`. A complete rerun with a
new, previously nonexistent `CARGO_TARGET_DIR` produced an executable artifact and passed all 157
tests. The failed attempt remains disclosed. It currently indicates a shared-target/build-environment
artifact anomaly rather than a reproducible source defect; final qualification therefore uses fresh
targets and clean extractions and never reuses a target directory across toolchains.

## Consequences

1. Do not claim useful parallel scaling from the current pipeline.
2. Replace exhaustive remapping with an indexed mapper before making it the large-input default.
   ADR 0011 subsequently completed that semantic promotion with fixed q=15 and retained the
   exhaustive scanner as a test oracle; this historical profile is not performance evidence for the
   promoted mapper.
3. Measure phase CPU, RSS, and cumulative/live temporary I/O before optimizing storage passes.
4. Build and test every final archive from fresh extractions and fresh target directories under both
   the MSRV and current stable toolchains.
