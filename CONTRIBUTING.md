# Contributing

Read [current status](docs/STATUS.md), [the development roadmap](docs/RESUME.md),
and `AGENTS.md` before changing behavior. Keep each change focused on one issue.

## Local checks

Install Rust 1.85 or newer with rustfmt and Clippy, then run:

```bash
./scripts/verify.sh
```

Use the existing focused tests while editing. Run the complete verifier before
proposing the final change. Keep `Cargo.lock` committed. Source and runtime
behavior must remain compatible with the declared MSRV; record any unavailable
platform checks explicitly.

## Change discipline

- Preserve existing input, error, output, and determinism contracts.
- Add a meaningful regression for a behavior change; do not ignore failing tests.
- Keep the experimental and fixed-k paths distinguishable. An undeclared Rust
  file is preserved work, not a tested capability.
- Do not change assembly policy, add heuristics, or claim improved reconstruction
  without a separate design decision and appropriate evidence.
- Preserve negative results and distinguish historical checks from current ones.
- Do not commit build caches, local results, credentials, or recovery archives.

## Source packages

`scripts/source-package-files.txt` is a sorted inventory of files under the
source directories. Update it whenever files are added, moved, or removed.
`bash scripts/package_source.sh` checks the inventory and produces a deterministic
source ZIP plus checksum. It refuses to overwrite an existing ZIP. Keep an
earlier ZIP outside the repository before generating another.

The broader release procedure is documented in
[VERIFICATION.md](docs/VERIFICATION.md); fuzzing instructions are in
[FUZZING.md](docs/FUZZING.md). Source packaging checks archive integrity and does
not establish reconstruction quality.
