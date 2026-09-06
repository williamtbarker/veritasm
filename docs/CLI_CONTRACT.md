# VeritAsm fixed-k assembly CLI contract v0.1

Status: normative architecture input. This contract freezes the first stable command surface; it is
not evidence that the implementation exists until the executable tests pass.

## Invocation

The executable is `veritasm`. Stable 0.1 has one subcommand:

```text
veritasm assemble [INPUT] --output-dir DIR [SCIENTIFIC OPTIONS] [RESOURCE OPTIONS]
```

`--help` and `--version` exit 0. A successful assembly exits 0, including the recorded
`software_run_complete_no_unitigs_under_parameters` state. Every failure prints exactly one leading
diagnostic line `error[<stable-code>]: <context>` to stderr and exits with the family code frozen in
`docs/CONFIGURATION.md`; it does not commit `DIR`.

## Input forms

Exactly one of these mutually exclusive forms is required:

- single-end: `-U, --single <FASTX>...`;
- paired-end: `-1, --read1 <FASTX>... -2, --read2 <FASTX>...`.

Repeated values are ordered lanes. Paired lists must have equal nonzero length and lane `i` is
`read1[i]` plus `read2[i]`. Comma-separated path lists are not expanded. A literal `-` means standard
input and may occur once across the entire invocation. Empty arguments, a repeated physical source,
mixed single/paired arity, and incomplete paired options fail under the typed configuration or pair
codes. Input-format and pairing semantics are frozen in `docs/CONFIGURATION.md` and
`ARCHITECTURE.md`.

`-o, --output-dir <DIR>` is required. It names the new committed result directory, not a FASTA file.
The destination must not exist. Stable 0.1 has no overwrite, resume, force, stdout-result,
individual-artifact output, or execution-log file mode. Operational messages go only to stderr, and
no post-commit stderr failure may change a successful exit into a failure.

## Scientific options

| Flag | Value/default | Stable meaning |
|---|---|---|
| `-k, --k <INTEGER>` | `31` | Exact graph word size, 3 through 63 |
| `--profile <PROFILE>` | `thresholded` | `retain-all`, `thresholded`, or `custom`; serialized with underscores |
| `--support-unit <UNIT>` | `supplied-fragment-instance` | `supplied-fragment-instance` or `accepted-window-occurrence`; serialized with underscores |
| `--min-support <INTEGER>` | profile-dependent | Inclusive exact threshold under the profile rules |
| `--min-base-quality <INTEGER>` | `20` | Phred+33 threshold, 0 through 93; recorded but not applied to FASTA |
| `--no-remap` | absent | If present, sets `remap=false`; otherwise `remap=true` |

Hyphenated CLI enum values map exactly to the underscore values in machine artifacts. There are no
tip, bubble, circularity, consensus, correction, scaffold, multi-k, taxonomy, reference, host,
variant, or Bloom flags in the stable command.

Profile expansion is performed before any source is opened. `retain-all` rejects any explicit
`--min-support` other than 1 and otherwise supplies 1. `thresholded` supplies 2 when the flag is absent
and rejects values below 2. `custom` requires the flag. Effective values are serialized in
`run.json`; the implementation never changes them adaptively.

## Resource and execution options

Every configurable limit in the resource table of `docs/CONFIGURATION.md` has the same kebab-case CLI
spelling, integer byte values are base-10 bytes, and values are parsed as unsigned decimal with no
sign, exponent, suffix, comma, or surrounding whitespace:

`--max-header-bytes`, `--max-read-bases`, `--max-record-bytes`,
`--max-raw-transport-bytes`, `--max-decoded-input-bytes`, `--max-gzip-members`,
`--batch-fragments`, `--memory-budget-bytes`,
`--max-spool-bytes`, `--max-temp-bytes`, `--partition-prefix-bits`,
`--sort-buffer-keys`, `--max-runs`, `--max-manifest-bytes`, `--max-retained-kmers`,
`--max-mapping-candidates`, `--max-staged-output-bytes`, and `--html-max-unitig-rows`.

`--merge-fan-in` and `--max-count-open-files` are intentionally not exposed because stable 0.1 fixes
them at 16 and 17. `-t, --threads <INTEGER>` accepts 1 through the lower of 1,024 and the configured
memory budget's worker-stack admission limit. Each Rayon worker requests a fixed 1 MiB stack, and all
requested worker stacks together may use at most one quarter of `memory_budget_bytes`. When the flag
is absent, available parallelism is capped at that same limit. Threads are operational and excluded
from the committed bundle. An explicitly supplied zero is invalid rather than an alias for automatic
selection.

The parser rejects duplicate scalar flags, unknown flags, invalid UTF-8 in option names/values, and
trailing positional arguments through the command-line framework before opening input. Numeric
domain failures use `configuration_invalid_k`, `configuration_invalid_support`, or
`configuration_invalid_limit` as narrowly applicable.

## Determinism and path handling

Input argument ordering is scientifically meaningful only as lane and record-instance order; it is
recorded through sanitized role labels and content digests, never raw paths. Equivalent thread counts
produce byte-identical committed bundles. Locale, current time, host name, temporary path, process ID,
hash-map iteration, and completion order cannot enter output bytes.

The implementation performs lexical and physical path safety checks before source consumption, then
uses the same-filesystem transaction in ADR 0005. An output/input collision, unsafe ancestor, active
lock, existing destination, or unavailable no-replace primitive fails with its frozen code. There is
no network option and no external command is invoked.

## Non-stable developer commands

Benchmarks, synthetic-data generation, schema verification, and the Bloom-sieve experiment may be
separate examples, test binaries, or feature-gated developer commands. They must identify themselves
as experimental, must not write into a stable result bundle, and are outside this CLI compatibility
promise.
