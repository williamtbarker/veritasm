# Development status

Version `0.4.0-dev.1`; updated 2026-09-06. Development is paused. The fixed-k
command and experimental multi-k implementation are included; one unfinished
recompaction module remains outside the build.

## Verification

| Platform and toolchain | Result | Evidence scope |
| --- | --- | --- |
| Linux x86_64, Rust 1.98.1 | PASS: formatting, strict Clippy, 625 tests, 15 doctests, rustdoc, release build and smoke checks | [Retained transcript](verification/stable-verification.log), before the macOS conversion fix |
| Linux x86_64, Rust 1.85.0 | PASS: the same checks | [Retained transcript](verification/msrv-complete.log), before the macOS conversion fix |
| macOS, Rust 1.98.1 | PASS: complete `scripts/verify.sh` run after the conversion fix | Maintainer-reported local run on 2026-09-06; successful final output supplied, full transcript not retained here |

The macOS fix replaces two fallible file-mode conversions with `u32::from` in
`src/experimental/external_cdbg.rs`. The earlier Linux transcripts describe the
source before that fix. The configured CI matrix will check the submitted tree
on Linux and macOS with stable Rust and the Rust 1.85 MSRV; no successful GitHub
Actions run is asserted here. macOS architecture was not recorded in the supplied
output.

Run `./scripts/verify.sh` with Rust 1.85 or newer, rustfmt and Clippy. It stops at
the first failure and reuses Cargo's build cache. With multiple Rust installations,
select a toolchain explicitly, for example:

```bash
rustup run 1.98.1 bash ./scripts/verify.sh
```

The [verification records](verification/README.md) retain earlier failures,
the corrected determinism regression, source-input hashes and packaging checks.
Those hashes identify the earlier tested snapshot, not subsequent source edits.

## Known issues

- A reported minimal case raises a question about inferred graph adjacency versus
  direct read-transition support. It still needs a reproducer and classification
  against the fixed-k contract.
- A small paired experimental portfolio reportedly exhausted snapshot/resource
  limits. That case and the broader aggregate-memory envelope remain unresolved.
- `src/experimental/witnessed_recompaction.rs` is undeclared and untested.
- Fresh dependency advisory scans, sustained sanitizer fuzzing, representative
  public-data evaluation and comparative benchmarks remain outstanding.

The passing checks establish software behavior on the stated fixtures and
platforms. Reconstruction quality and comparative performance remain unqualified.
Root Cargo checks do not include the separate fuzz workspace.

See the [module map](MODULE_MAP.md) and [development roadmap](RESUME.md).
Historical reports retain their original snapshot scope and negative results.
