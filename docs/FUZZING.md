# Coverage-guided fuzzing contract

Status: harness and CI configuration. This file is not an execution record, sanitizer result, or
proof that malformed input is safe.

VeritAsm keeps `cargo-fuzz` outside the runtime dependency graph. The separate `fuzz/Cargo.toml`
package pins the product version and owns the native libFuzzer dependency. The unresolved
`libfuzzer-sys` notice discrepancy in `docs/DEPENDENCY_REVIEW.md` still blocks redistribution of fuzz
binaries or fuzz dependency sources; it does not prevent a local qualification run after review.

## Target contracts

| Target | Boundary | A producer success must establish |
|---|---|---|
| `fastx` | Complete bounded FASTA/FASTQ parse, DNA scan, reverse complement | Every returned record satisfies the parser contract; malformed input remains a typed error |
| `gzip` | Complete bounded content sniffing, concatenated members, corruption and trailing bytes | Every returned decoded record satisfies the parser contract |
| `paired` | Pair synchronization and spool production | A successful spool independently verifies, fully iterates without error, and reproduces its fragment/read counts |
| `spool` | Public post-seal spool mutation boundary | Any byte or length change is rejected by both verification and scientific replay; an unchanged producer output fully replays and reproduces its fragment/read counts |
| `count_run` | Registered count-run mutation and writer poisoning | Changed bytes cannot finalize against the producer-registered identity; invalid ordinal state poisons the writer; unchanged output retains exact totals |
| `bundle_manifest` | Manifest grammar, path safety, membership and hashes | A generated canonical manifest is accepted; duplicate paths, unsafe paths, missing final LF, and invalid digests are rejected |
| `bundle` | End-to-end render/transaction boundary | Success verifies; a second writer cannot replace it; any returned assembly error leaves no normal committed destination |

Typed parse, input, pair, resource, integrity, destination, and commit failures are expected outcomes
for adversarial bytes. A panic, abort, sanitizer finding, producer-success invariant failure, or
ordinary result directory left by an error is a fuzz failure. The harnesses do not reinterpret typed
errors as biological outcomes.

The public fuzz crate cannot rewrite a registered digest or cardinality and thereby mint a new
source-backed `Spool`; that is an intentional capability boundary. Reauthenticated structural
mutations are exercised by crate-internal spool tests, where test-only corruptors cannot enter the
public API. The `spool` fuzz target therefore tests only the externally reachable boundary: immutable
producer output or detectable post-seal physical mutation.

Seed corpora live under `fuzz/corpus/<target>/`. They include valid producer paths as well as corrupt,
truncated, reordered, duplicate, unsafe-path, and limit-oriented inputs. A qualification record must
hash the complete starting corpus before execution; newly discovered corpus entries and minimized
crashes are retained separately rather than silently replacing the checked-in seeds.

## Pinned smoke commands

Install the reviewed tool and compile every target with the pinned nightly:

```bash
cargo install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-08-18 fuzz check
```

Run one bounded AddressSanitizer/libFuzzer plus LeakSanitizer smoke, substituting each target and its
declared maximum input length from `.github/workflows/fuzz.yml`:

```bash
cargo +nightly-2026-08-18 fuzz run spool fuzz/corpus/spool -- \
  -max_total_time=60 -max_len=16384 -timeout=5 -detect_leaks=1 -print_final_stats=1
```

The scheduled workflow executes all seven targets independently and retains whatever setup, compile,
and corpus evidence a matrix job produced. When a job reaches the bounded fuzz step, it uploads the
complete bounded-run log, resulting corpus, and any crash artifacts even when that fuzz run fails. A
failure before the bounded run can leave no fuzzer log; it remains a failed or incomplete job rather
than evidence of a passing fuzz run. A passing 60-second smoke is only a release gate at that exact
identity. It is not a sustained campaign, platform-complete fuzzing, a soundness proof, or evidence
about assembly accuracy.

## Evidence admission

For each target, retain the source commit, `Cargo.toml` and lockfile hashes, pinned nightly identity,
`cargo-fuzz` version, sanitizer configuration, complete command, starting corpus manifest and digest,
wall limit, final libFuzzer statistics, exit state, resulting corpus manifest, and every crash or
timeout artifact. Record a failed compile, sanitizer initialization failure, timeout, or infrastructure
failure as a failed/not-completed run; do not convert it into a pass because another target succeeded.

## 2026-09-05 development attempt — not completed

One dirty, concurrently changing development tree was used to exercise the strengthened `paired`
producer-success oracle with `cargo-fuzz 0.13.2`, rustc
`1.100.0-nightly (8fa1c96cf 2026-08-17)`, AddressSanitizer/libFuzzer, and
`-detect_leaks=1`. The harness file SHA-256 at the attempt was
`220b44b1bdc1c20bc82fab80c0e43a6a3634e34d3fc13924a5d63c5d20b5f65f`; the starting three-file
paired corpus manifest digest was
`015a4420dd2c12e5259e024a8815f126dcf58a49c70696583708cbdd29de54b0`. The command used a private copy
of that corpus and otherwise the arguments shown above, with target `paired`, `-max_total_time=10`,
`-max_len=65536`, and `-timeout=5`.

The target body executed 31,723 inputs in approximately 11 seconds without a target assertion or
reported memory error. LeakSanitizer then failed internally while trying to inspect a `/proc/.../task`
entry and reported that the environment's ptrace policy prevented leak checking. It emitted an empty
infrastructure crash artifact (SHA-256
`da39a3ee5e6b4b0d3255bfef95601890afd80709`). The complete raw log was not retained. Therefore this
attempt is **NOT COMPLETED**, is not a sanitizer pass, and is not release evidence. Sanitizers were not
disabled or weakened to manufacture a green result. `cargo +nightly-2026-08-18 fuzz check` did
separately compile all seven targets on the same development tree; that compile observation is also
not clean-source or release evidence.
