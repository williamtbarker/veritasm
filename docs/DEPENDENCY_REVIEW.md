# Format and dependency review

- Review date: 2026-09-03
- Product MSRV: Rust 1.85
- Status: current root and fuzz closures enumerated mechanically on Linux and given a bounded local
  source review. A 2026-09-05 dirty-tree snapshot passed fresh offline `cargo-audit` and
  `cargo-deny` scans against the exact inputs recorded below; the manifest/version freeze and final
  source-package rerun remain required. The unresolved fuzz-native notice discrepancy and
  unexecuted macOS/dynamic-network gates remain explicit
- License of VeritAsm source: MIT

## Decision

Keep the current small, permissively licensed dependency set for the vertical slice. Add a crate only
when it replaces a measured amount of risk or complexity and passes an exact-behavior, MSRV, license,
advisory, determinism, runtime-I/O, and platform gate. The current custom FASTX path remains in place
until a replacement proves all inherited behavior, especially wrapped records, content-detected and
concatenated gzip, strict paired synchronization, malformed-input context, and streaming bounds.

Rust 1.85 is a hard product compatibility floor, not a suggestion. A crate whose selected release
requires 1.86 or later is ineligible without a superseding architecture decision. Build and audit
tools may use current stable in a separate CI job; their compiler requirement does not change the
runtime crate's MSRV.

For interchange, v0.1 emits deterministic FASTA, a conservative common GFA 1.0 subset, a
versioned JSON report, a tabular evidence file, checksums/manifests, and a self-contained HTML view.
SAM/BAM and PAF are future read-audit/alignment artifacts, not de novo assembly formats. VCF is
reference-relative and remains out of scope until a separate reference-assisted layer exists.

## Review method and evidence limits

The locked table below was generated from `Cargo.lock` and `cargo metadata --locked` in this source
tree. Its `rust-version` values are package declarations, not proof: the complete project must still
compile and test on Rust 1.85. Candidate-crate values came from registry manifests and upstream
documentation visible on the review date. They must be rechecked against the exact downloaded crate
before addition because an unbounded Cargo requirement can resolve to a later release.

License expressions are metadata, not a source audit. The release gate must inspect the exact crate
archive, bundled code, build scripts, generated/native components, notice files, and resolved feature
tree. No conclusion here is legal advice.

### 2026-09-05 dirty-tree automated gate

This is retained pre-freeze evidence, not a final source-package gate. The tools were rebuilt with
network access disabled from locally cached crates using their own locked dependency graphs. Both
new binary hashes matched the previously retained binaries byte for byte. The scan used the locally
available RustSec snapshot and did not refresh it from the network.

| Evidence item | Exact value |
|---|---|
| Compiler | `rustc 1.98.1 (48a229cea 2026-09-01)`; host `x86_64-unknown-linux-gnu`; LLVM `22.1.8` |
| `cargo-audit` | Version `0.22.2`; crate archive SHA-256 `700c2b240f7fd330c24b675fe429f73a5b676531fcc6300400b2b67f155ba12a`; embedded upstream commit `281452c35cf0870969042374110f099a411bc185`; binary SHA-256 `3ee9372326928f5cf1f8679cc7733efadb6a590e9fff1c034537780ae3e36d97` |
| `cargo-deny` | Version `0.20.2`; crate archive SHA-256 `e528dfcbe739af7ce37a77d3d6df1b29dd6887b1c701d888820c0f16b864f737`; embedded upstream commit `bca0dde53651ee946720e4540b5ce2610bec8f06`; binary SHA-256 `f6204753430c201db66214a8cdb8cd630467923506ca87e8411b9634854f9ec7` |
| Root inputs | `Cargo.toml` SHA-256 `b6fc3e2603c05489d5ffc83833cbf0ca9c84381c5bcdb06795564583b1e50538`; `Cargo.lock` SHA-256 `a6472637042f90c430529a0a9603c4227d9be0d5d9eab1dceab6559e8bdd9b6b` |
| Fuzz inputs | `fuzz/Cargo.toml` SHA-256 `56edb62a8116208100d07a9450ca4507e9445dcd284d8e8fa06b6ec33c5ab9cc`; `fuzz/Cargo.lock` SHA-256 `6051e8c8ea93705ed7618530b53918041b35e75b5901e3f9d9cd03bb20750e02` |
| Policy | `deny.toml` SHA-256 `42e18932d50645fa16a7df264b4d9f906a5a9551bce942fb73bfa98a6e979244`; NCSA is a version-exact exception only for `libfuzzer-sys 0.4.13`, rather than a graph-wide allowance |
| RustSec snapshot | Git commit `5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5`, authored `2026-09-02T09:07:35Z`; clean worktree and successful `git fsck --strict`; 1,239 advisories loaded by `cargo-audit` |
| Advisory result | `cargo-audit 0.22.2 --no-fetch --deny warnings`: root 97 dependencies and fuzz 69 dependencies; zero vulnerabilities and zero warnings |
| License/source/bans result | `cargo-deny 0.20.2 --frozen`: root and fuzz passed advisories, bans, licenses, and sources; explicit Linux x86-64 and Apple Silicon target-filtered runs also passed. The root-only run emits one non-fatal unmatched-exception warning because the version-exact fuzz-only NCSA exception is deliberately absent from that graph |
| Cached archive integrity | Every cached registry archive named by the lockfiles matched its lockfile checksum: root 96 of 96; fuzz 67 of 67; zero missing and zero mismatches |

These results do not make a dependency-soundness claim, do not resolve the `libfuzzer-sys` bundled
license/notice discrepancy below, and do not represent a current advisory scan after 2026-09-02.
The source ZIP may contain the project's fuzz harness source and lockfile, but it must not contain a
fuzz executable, compiled native artifact, vendored `libfuzzer-sys` source, or other fuzz-dependency
source while that discrepancy remains unresolved.
Any change to one of the four manifest/lock inputs or `deny.toml` supersedes this snapshot and requires
a rerun. Final clean-extraction verification must retain the scan output, exit status, source archive
hash, advisory database commit, tool versions, and tool binary hashes.

## Current locked direct dependencies

These are observations from `cargo metadata --locked --format-version 1 --no-deps` and the exact
package entries in `Cargo.lock`, not a request to change `Cargo.toml`. “Locked version” is the
current resolution; most manifest requirements are compatible caret ranges rather than exact `=`
pins.

| Crate | Locked version | Declared MSRV | License | Decision and bounded use |
|---|---:|---:|---|---|
| `clap` | 4.6.6 | 1.85 | MIT OR Apache-2.0 | Retain; this version consumes the full MSRV budget, so lockfile/MSRV CI is mandatory |
| `flate2` | 1.1.10 | 1.67 | MIT OR Apache-2.0 | Retain with the pure-Rust backend; explicitly use and test multi-member decoding after content sniffing |
| `rayon` | 1.12.0 | 1.80 | MIT OR Apache-2.0 | Retain for bounded parallel work; never serialize parallel hash/insertion order, and reduce through sorted stable keys |
| `rustix` | 1.1.4 | 1.63 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Retain for the `fs` API; the direct request disables defaults, but unified normal-graph features are `alloc/default/fs/std/termios` through `tempfile` and `clap`/`terminal_size`; the lockfile resolves 1.1.4 while the manifest requirement `^1.1.4` is not an exact pin |
| `serde` | 1.0.229 | 1.56 | MIT OR Apache-2.0 | Retain for versioned structured records; field order and optional/default rules require golden tests |
| `serde_json` | 1.0.151 | 1.71 | MIT OR Apache-2.0 | Retain for machine evidence; use a schema version and deterministic collection ordering |
| `sha2` | 0.11.0 | 1.85 | MIT OR Apache-2.0 | Retain for spool, sequence, state, manifest, and experimental Bloom-probe SHA-256 contracts; verify fixed vectors |
| `tempfile` | 3.27.0 | 1.63 | MIT OR Apache-2.0 | Retain for same-filesystem staging; one temporary file does not establish an atomic multi-artifact transaction |
| `thiserror` | 2.0.20 | 1.71 | MIT OR Apache-2.0 | Retain for typed library errors and source chains |

Stable registry records: [`clap`](https://crates.io/crates/clap/4.6.6),
[`flate2`](https://crates.io/crates/flate2/1.1.10),
[`rayon`](https://crates.io/crates/rayon/1.12.0),
[`rustix`](https://crates.io/crates/rustix/1.1.4),
[`serde`](https://crates.io/crates/serde/1.0.229),
[`serde_json`](https://crates.io/crates/serde_json/1.0.151),
[`sha2`](https://crates.io/crates/sha2/0.11.0),
[`tempfile`](https://crates.io/crates/tempfile/3.27.0), and
[`thiserror`](https://crates.io/crates/thiserror/2.0.20).

### Current locked direct development dependencies

| Crate | Locked version | Declared MSRV | License | Decision and bounded use |
|---|---:|---:|---|---|
| `assert_cmd` | 2.2.2 | 1.85 | MIT OR Apache-2.0 | Retain for subprocess/CLI assertions; do not infer shell portability from Linux-only tests |
| `predicates` | 3.1.4 | 1.74 | MIT OR Apache-2.0 | Retain with `assert_cmd` for explicit stdout, stderr, and path predicates |
| `proptest` | 1.11.0 | 1.85 | MIT OR Apache-2.0 | Retain for DNA, graph, and Bloom/two-hit invariants; archive failing seeds/regressions |

Stable registry records: [`assert_cmd`](https://crates.io/crates/assert_cmd/2.2.2),
[`predicates`](https://crates.io/crates/predicates/3.1.4), and
[`proptest`](https://crates.io/crates/proptest/1.11.0).

### Locked all-target closure and local audit evidence

This snapshot treats every target-conditioned node retained in each lockfile as part of the
inventory. It then separates dependencies reachable through normal/build edges from crates added
only through VeritAsm's development edges and from crates added only by the isolated fuzz package.
Consequently, Windows, UEFI, and WASI-only entries are recorded even though they are not selected
by an ordinary Linux or macOS build. Build scripts and procedural macros are build-time code, not
runtime-loaded libraries.

| Historical 2026-09-03 evidence item | Exact reviewed value |
|---|---|
| Root inputs | `Cargo.lock` SHA-256 `ac77827391e07e0afa34428e887274e74862ac6617530e65f5a43067ebb9f8e7`; `Cargo.toml` SHA-256 `9f4c5461cff63620372b8adf40363d5d3740ac0bcd940a1afa08deb3db953b88` |
| Fuzz inputs | `fuzz/Cargo.lock` SHA-256 `55aa3c68ba2632873b13ca86cff54678185b3981f20dc2fa397ec4af2a52895e`; `fuzz/Cargo.toml` SHA-256 `3ae242327ac7718d7e581c125dca57cf26237028ea1afc38aa4a25fdef42afee` |
| Resolver/toolchain | Offline `cargo metadata --locked --format-version 1`; Cargo 1.98.0 (`797e8a9bc`, 2026-08-05), rustc 1.98.0 (`88d9e12ae`, 2026-08-18), host `x86_64-unknown-linux-gnu`; no repository `rust-toolchain` file pins this environment |
| Root graph size | 97 packages: one local VeritAsm package, 61 registry packages in the normal/build closure, and 35 registry packages added only by development dependencies |
| Fuzz graph size | 69 packages: two local packages (`veritasm` and `veritasm-fuzz`) and 67 registry packages; the 67 are the same 61 normal/build registry packages plus six fuzz-only packages |
| License/source policy | `cargo-deny` 0.20.2, `--frozen`, passed licenses, bans, and sources for both manifests; the root run warned only that the NCSA allowance was not encountered, while the fuzz run used it |
| Advisory snapshot | `cargo-audit` 0.22.2 refreshed the database for the root scan, then the fuzz scan used `--no-fetch`; neither lock reported a vulnerability or warning (97 and 69 packages respectively) against 1,239 loaded advisories. The local RustSec git database was at `5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5`, committed 2026-09-02T11:13:32+02:00; the git HEAD is the reproducibility datum |

On 2026-09-03 the exact four files named above were rehashed immediately after the recorded
`cargo-audit` 0.22.2 and `cargo-deny` 0.20.2 runs. After the root manifest's release-only Cargo
exclusions changed, both advisory scans were rerun with `--no-fetch` and both frozen policy checks
were rerun against the exact hashes above. Both advisory scans used the same RustSec database commit.
The root policy run emitted the expected non-fatal warning that its graph did not encounter the NCSA
allowance; the fuzz graph did encounter it. Later source/documentation changes do not alter this
record unless one of these four digests changes. All four digests changed when only the local
VeritAsm package version advanced from `0.2.0-alpha.1` to `0.3.0-alpha.1`; this makes the scan record
historical even though the third-party resolution did not change.

The current review-candidate inputs are:

| Current 2026-09-04 input | SHA-256 |
|---|---|
| `Cargo.toml` | `7caff77a75243be3af55446c8b0f993ebddad150bf22863c62e7d244f17511d5` |
| `Cargo.lock` | `a6472637042f90c430529a0a9603c4227d9be0d5d9eab1dceab6559e8bdd9b6b` |
| `fuzz/Cargo.toml` | `56edb62a8116208100d07a9450ca4507e9445dcd284d8e8fa06b6ec33c5ab9cc` |
| `fuzz/Cargo.lock` | `6051e8c8ea93705ed7618530b53918041b35e75b5901e3f9d9cd03bb20750e02` |

The diff from the historically audited inputs contains only those four local package-version
references. Registry package names, versions, sources, checksums, and declared features are
unchanged. Locked offline target-filtered metadata passed on 2026-09-04 for
`x86_64-unknown-linux-gnu` and `aarch64-apple-darwin`, and Rust 1.85/current stable selected identical
Linux package tuples. A manual metadata-license expression check found no new or missing expression;
that is not a per-file legal audit. `cargo-audit` 0.22.2, `cargo-deny` 0.20.2, and a current RustSec
database were unavailable in the final local environment, so the current four hashes have **not**
passed a fresh advisory or automated license-policy scan. That release gate remains open.

The complete package classification is:

| Scope | Direct packages | Transitive or scope-only packages |
|---|---|---|
| Normal/build | `clap 4.6.6`, `flate2 1.1.10`, `rayon 1.12.0`, `rustix 1.1.4`, `serde 1.0.229`, `serde_json 1.0.151`, `sha2 0.11.0`, `tempfile 3.27.0`, `thiserror 2.0.20` | `adler2 2.0.1`, `anstream 1.0.0`, `anstyle 1.0.14`, `anstyle-parse 1.0.0`, `anstyle-query 1.1.5`, `anstyle-wincon 3.0.11`, `bitflags 2.13.1`, `block-buffer 0.12.1`, `cfg-if 1.0.4`, `clap_builder 4.6.6`, `clap_derive 4.6.4`, `clap_lex 1.1.0`, `colorchoice 1.0.5`, `const-oid 0.10.2`, `cpufeatures 0.3.1`, `crc32fast 1.5.1`, `crossbeam-deque 0.8.7`, `crossbeam-epoch 0.9.20`, `crossbeam-utils 0.8.22`, `crypto-common 0.2.2`, `digest 0.11.3`, `either 1.18.0`, `errno 0.3.14`, `fastrand 2.5.0`, `getrandom 0.4.3`, `heck 0.5.0`, `hybrid-array 0.4.14`, `is_terminal_polyfill 1.70.2`, `itoa 1.0.18`, `libc 0.2.189`, `linux-raw-sys 0.12.1`, `memchr 2.8.3`, `miniz_oxide 0.9.1`, `once_cell 1.21.4`, `once_cell_polyfill 1.70.2`, `proc-macro2 1.0.107`, `quote 1.0.47`, `r-efi 6.0.0`, `rayon-core 1.13.0`, `serde_core 1.0.229`, `serde_derive 1.0.229`, `simd-adler32 0.3.10`, `strsim 0.11.1`, `syn 3.0.4`, `terminal_size 0.4.4`, `thiserror-impl 2.0.20`, `typenum 1.20.1`, `unicode-ident 1.0.24`, `utf8parse 0.2.2`, `windows-link 0.2.1`, `windows-sys 0.61.2`, `zmij 1.0.23` |
| Development only | `assert_cmd 2.2.2`, `predicates 3.1.4`, `proptest 1.11.0` | `aho-corasick 1.1.5`, `autocfg 1.5.1`, `bit-set 0.8.0`, `bit-vec 0.8.0`, `bstr 1.13.1`, `difflib 0.4.0`, `float-cmp 0.10.0`, `fnv 1.0.7`, `getrandom 0.3.4`, `normalize-line-endings 0.3.0`, `num-traits 0.2.19`, `ppv-lite86 0.2.21`, `predicates-core 1.0.10`, `predicates-tree 1.0.13`, `quick-error 1.2.3`, `r-efi 5.3.0`, `rand 0.9.5`, `rand_chacha 0.9.0`, `rand_core 0.9.5`, `rand_xorshift 0.4.0`, `regex 1.13.1`, `regex-automata 0.4.18`, `regex-syntax 0.8.11`, `rusty-fork 0.3.1`, `syn 2.0.119`, `termtree 0.5.1`, `unarray 0.1.4`, `wait-timeout 0.2.1`, `wasip2 1.0.4+wasi-0.2.12`, `wit-bindgen 0.57.1`, `zerocopy 0.8.56`, `zerocopy-derive 0.8.56` |
| Fuzz manifest | Direct: local `veritasm` with exact requirement `=0.3.0-alpha.1`, `flate2 1.1.10`, `sha2 0.11.0`, `tempfile 3.27.0`, and `libfuzzer-sys 0.4.13` | The VeritAsm normal/build closure above, plus fuzz-only `arbitrary 1.4.2`, `cc 1.4.4`, `find-msvc-tools 0.1.11`, `jobserver 0.1.35`, `libfuzzer-sys 0.4.13`, and `shlex 2.0.1`; no root development dependency is in the fuzz lock |

All registry packages in that table declare `MIT OR Apache-2.0` (including the equivalent
reverse ordering) except for the following exact metadata expressions:

| Scope | Other declared license expressions |
|---|---|
| Normal/build | `adler2`: `0BSD OR MIT OR Apache-2.0`; `unicode-ident`: `(MIT OR Apache-2.0) AND Unicode-3.0`; `linux-raw-sys` and `rustix`: `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; `simd-adler32`, `strsim`, and `zmij`: `MIT`; `r-efi 6.0.0`: `MIT OR Apache-2.0 OR LGPL-2.1-or-later`; `miniz_oxide`: `MIT OR Zlib OR Apache-2.0`; `memchr`: `Unlicense OR MIT` |
| Development only | `normalize-line-endings`: `Apache-2.0`; `fnv`: `Apache-2.0 / MIT`; `wasip2` and `wit-bindgen`: `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; `zerocopy` and `zerocopy-derive`: `BSD-2-Clause OR Apache-2.0 OR MIT`; `difflib`, `float-cmp`, and `termtree`: `MIT`; `r-efi 5.3.0`: `MIT OR Apache-2.0 OR LGPL-2.1-or-later`; `quick-error`, `rusty-fork`, and `wait-timeout`: `MIT/Apache-2.0`; `aho-corasick`: `Unlicense OR MIT` |
| Fuzz only | `libfuzzer-sys`: `(MIT OR Apache-2.0) AND NCSA`; the other five fuzz-only packages declare `MIT OR Apache-2.0` |

The metadata MSRV inventory has one explicit over-budget entry: dev-only, WASI-targeted
`wasip2 1.0.4+wasi-0.2.12` declares Rust 1.87. It is reached through
`proptest -> rand -> rand_core -> getrandom 0.3.4` only for
`cfg(all(target_arch = "wasm32", target_os = "wasi", target_env = "p2"))`; it does not enter a
Linux or macOS product build, but it makes any claim that every root-lock package supports 1.85
false. Normal/build packages without a declared `rust-version` are `adler2`, `miniz_oxide`,
`simd-adler32`, and `utf8parse`. Dev-only packages without one are `bit-set`, `bit-vec`,
`difflib`, `float-cmp`, `fnv`, `normalize-line-endings`, `quick-error`, `rusty-fork`,
`unarray`, `wait-timeout`, and `zerocopy-derive`; fuzz-only `libfuzzer-sys` also omits it.
Absence of a declaration is not compatibility evidence.

The selected direct-feature evidence is:

| Package/scope | Resolved features |
|---|---|
| `clap` | `color, default, derive, error-context, help, std, suggestions, usage, wrap_help` |
| `flate2` | `any_impl, miniz_oxide, rust_backend`; no C-zlib backend is selected |
| `rustix` | `alloc, default, fs, std, termios`; VeritAsm directly asks only for `fs`, but `tempfile` and `clap -> terminal_size` widen the unified normal graph |
| `serde` / `serde_json` | `default, derive, serde_derive, std` / `default, std` |
| `sha2` / `tempfile` / `thiserror` | `alloc, default, oid` / `default, getrandom` / `default, std` |
| Development | `predicates`: `color, default, diff, float-cmp, normalize-line-endings, regex`; `proptest`: `bit-set, default, fork, regex-syntax, rusty-fork, std, tempfile, timeout`; `assert_cmd` resolves no named feature |
| Fuzz | `libfuzzer-sys`: `default, link_libfuzzer`; the repeated `flate2` and `tempfile` features match the normal graph |

#### Build, macro, native, and unsafe boundaries

| Scope | Custom build scripts | Procedural macros | Native/link finding |
|---|---|---|---|
| Normal/build | `crc32fast`, `crossbeam-deque`, `crossbeam-epoch`, `crossbeam-utils`, `getrandom 0.4.3`, `libc`, `proc-macro2`, `quote`, `rayon-core`, `rustix`, `serde`, `serde_core`, `serde_json`, `thiserror`, `zmij` | `clap_derive`, `serde_derive`, `thiserror-impl` | `rayon-core` is the only package with Cargo `links = "rayon-core"`; its build script explicitly says it links nothing and uses the key only to prevent two versions. Reviewed normal build scripts do compiler/cfg probes and small `OUT_DIR` generation; none compiles bundled C/C++ or emits a selected native-library link. This is not a claim that the Rust crates contain no OS FFI |
| Development only | `assert_cmd`, `getrandom 0.3.4`, `num-traits`, `wit-bindgen`, `zerocopy` | `zerocopy-derive` | On wasm only, `wit-bindgen` copies and links the prebuilt `src/rt/libwit_bindgen_cabi.a`; the crate also ships WebAssembly object files and generated C source. Those artifacts were identified, not reproducibly rebuilt or compared in this review |
| Fuzz only | `libfuzzer-sys` | None | With resolved `link_libfuzzer`, its build script invokes `cc`, compiles the bundled libFuzzer C++ as C++17, and links a static archive. `CUSTOM_LIBFUZZER_PATH` can instead select an external static library and C++ runtime, making the fuzz build environment-sensitive |

The VeritAsm library root has `#![forbid(unsafe_code)]`, and both the root and fuzz manifests set
`[lints.rust] unsafe_code = "forbid"` for their package targets. This policy applies to project
targets; it does not forbid unsafe code inside dependencies. The reviewed project `.rs` files contain
no unsafe construct; the word “unsafe” occurs only in an error-message string. Registry crates do
contain unsafe code. A
conservative lexical scan counted `unsafe {`, `unsafe fn`, `unsafe impl`, `unsafe trait`, and
`unsafe extern` shapes across all cached `.rs` files, including docs, tests, examples, macro
tokens, generated bindings, inactive target code, and disabled features. Thus the counts below are
upper-bound review leads, not compiled unsafe-block counts or proof that any block is sound:

| Scope | Crates with lexical unsafe shapes (count) |
|---|---|
| Normal/build | `anstream 1.0.0 (3)`, `anstyle 1.0.14 (1)`, `anstyle-parse 1.0.0 (3)`, `anstyle-query 1.1.5 (1)`, `anstyle-wincon 3.0.11 (2)`, `bitflags 2.13.1 (2)`, `block-buffer 0.12.1 (21)`, `clap_builder 4.6.6 (5)`, `clap_lex 1.1.0 (6)`, `const-oid 0.10.2 (1)`, `cpufeatures 0.3.1 (11)`, `crc32fast 1.5.1 (15)`, `crossbeam-deque 0.8.7 (42)`, `crossbeam-epoch 0.9.20 (192)`, `crossbeam-utils 0.8.22 (81)`, `either 1.18.0 (2)`, `errno 0.3.14 (12)`, `flate2 1.1.10 (36)`, `getrandom 0.4.3 (113)`, `hybrid-array 0.4.14 (38)`, `itoa 1.0.18 (13)`, `libc 0.2.189 (672)`, `linux-raw-sys 0.12.1 (6,930)`, `memchr 2.8.3 (333)`, `once_cell 1.21.4 (52)`, `proc-macro2 1.0.107 (6)`, `r-efi 6.0.0 (335)`, `rayon 1.12.0 (67)`, `rayon-core 1.13.0 (87)`, `rustix 1.1.4 (1,599)`, `serde 1.0.229 (2)`, `serde_core 1.0.229 (2)`, `serde_json 1.0.151 (18)`, `sha2 0.11.0 (53)`, `simd-adler32 0.3.10 (36)`, `syn 3.0.4 (45)`, `tempfile 3.27.0 (3)`, `terminal_size 0.4.4 (6)`, `unicode-ident 1.0.24 (2)`, `utf8parse 0.2.2 (1)`, `windows-sys 0.61.2 (12,530)`, `zmij 1.0.23 (70)` |
| Development only | `aho-corasick 1.1.5 (227)`, `bit-set 0.8.0 (2)`, `bit-vec 0.8.0 (8)`, `bstr 1.13.1 (41)`, `getrandom 0.3.4 (97)`, `num-traits 0.2.19 (1)`, `ppv-lite86 0.2.21 (168)`, `predicates 3.1.4 (10)`, `proptest 1.11.0 (7)`, `r-efi 5.3.0 (15)`, `rand 0.9.5 (21)`, `regex 1.13.1 (1)`, `regex-automata 0.4.18 (57)`, `syn 2.0.119 (42)`, `unarray 0.1.4 (14)`, `wait-timeout 0.2.1 (6)`, `wasip2 1.0.4+wasi-0.2.12 (742)`, `wit-bindgen 0.57.1 (253)`, `zerocopy 0.8.56 (506)`, `zerocopy-derive 0.8.56 (297)` |
| Fuzz only | `arbitrary 1.4.2 (4)`, `cc 1.4.4 (18)`, `find-msvc-tools 0.1.11 (55)`, `jobserver 0.1.35 (42)`, `libfuzzer-sys 0.4.13 (9)`, `shlex 2.0.1 (6)` |

In addition to the VeritAsm project-target policy above, dependency crate roots declaring
`forbid(unsafe_code)` are normal/build `adler2`, `bitflags`, `clap`,
`clap_builder`, `clap_derive`, `crypto-common`, `digest`, `fastrand`, `heck`,
`miniz_oxide`, `strsim`, and `typenum`, plus dev-only `rand_chacha`, `rand_xorshift`, and
`regex-syntax`. A forbid declaration may coexist with lexical unsafe tokens in documentation,
disabled feature macros, tests, or examples; it is not a statement about every file shipped in the
archive.

There is one unresolved fuzz-license discrepancy. `libfuzzer-sys 0.4.13` declares
`(MIT OR Apache-2.0) AND NCSA`, and its README says the bundled `libfuzzer/` directory is NCSA.
However, all 55 reviewed bundled C/C++/header/definition files carry
`SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception`; the crate archive has
`LICENSE-APACHE` and `LICENSE-MIT` but no NCSA or LLVM-exception text. This review makes no legal
conclusion about which statement controls. Do not redistribute the fuzz/native closure until the
upstream snapshot, governing license, exception text, and notice obligations are reconciled.

The VeritAsm source ZIP does not vendor Cargo registry archives or their extracted sources: it
contains project source/manifests/lockfiles, and there is no project `vendor/` tree. That limits
what is redistributed in the source ZIP, but does not clear the downloaded crates used to build or
fuzz. In particular, any distribution of a fuzz binary or fuzz dependency sources remains blocked
on the libFuzzer finding above.

## Candidate dependency decisions

“Eligible” authorizes an implementation experiment, not an automatic dependency addition.
“Defer” means the current evidence or MSRV does not justify adoption.

### FASTX and compression

| Candidate snapshot | Declared MSRV | License | Decision | Evidence required before adoption |
|---|---:|---|---|---|
| `needletail` 0.7.3 | Not declared in reviewed manifest | MIT | Evaluate, but do not replace the parser yet | Rust 1.85 build; plain/content-gzip FASTA and FASTQ; wrapped records; concatenated members; corrupt/truncated gzip; CRLF; empty input; record-size cap; exact mate IDs/roles; byte-position context; streaming allocation profile |
| `seq_io` 0.3.4 | Not relied upon | MIT | Reject as the sole compatibility parser | Its documented FASTQ model is not sufficient evidence for wrapped FASTQ compatibility; an adapter would need the full inherited parser suite |
| `niffler` 3.0.1 | 1.82 | MIT OR Apache-2.0 | Defer | It would overlap `flate2`; show a concrete multi-codec requirement and preserve content detection, concatenated-member, error-context, and no-runtime-network behavior |
| `flate2` 1.1.10 | 1.67 | MIT OR Apache-2.0 | Adopted | Test `MultiGzDecoder` semantics directly; gzip is identified from magic bytes, never only a suffix |

Sources: [`needletail`](https://crates.io/crates/needletail/0.7.3) and its
[`parse_fastx_reader` contract](https://docs.rs/needletail/0.7.3/needletail/parser/fn.parse_fastx_reader.html),
[`seq_io`](https://crates.io/crates/seq_io/0.3.4) and its
[documented parser scope](https://docs.rs/seq_io/0.3.4/seq_io/),
[`niffler`](https://crates.io/crates/niffler/3.0.1), and
[`flate2::read::MultiGzDecoder`](https://docs.rs/flate2/1.1.10/flate2/read/struct.MultiGzDecoder.html).

### DNA, k-mers, and graph structures

| Candidate snapshot | Declared MSRV | License | Decision | Rationale and guardrail |
|---|---:|---|---|---|
| `bio-seq` 0.14.8 | 1.85 | MIT | Eligible for packed-sequence prototype | Compare A/C/G/T and IUPAC encoding, reverse complements, odd lengths, boundaries, and memory against the tested local representation; compile-time k/type constraints must not leak into the CLI contract |
| `nthash` 0.5.1 | 1.37 | MIT OR Apache-2.0 | Optional accelerator only | A rolling hash may choose a bucket, never define canonical k-mer identity or final ordering; collision behavior must be exact-verified |
| `debruijn` 0.3.4 | Not established by this review | MIT | Reference/prototype only | Last reviewed release is old and its graph/evidence semantics do not establish VeritAsm's invariants |
| GGCAT 2.2.0 source/Rust API | Upstream documentation states Rust 1.75 or later; not independently built here | MIT at the tagged top level | Evaluate as an external graph/unitig oracle and isolated reuse prototype; do not add yet | It constructs compacted/colored de Bruijn graphs from raw sequence data and emits GFA, but its default minimum multiplicity, canonicalization, support, pair, graph-link, temporary-state, and deterministic-byte semantics are not VeritAsm contracts. Freeze and audit the complete source/dependency tree before reuse |
| `petgraph` 0.8.3 | 1.64 | MIT OR Apache-2.0 | Eligible as a small-graph test oracle | Do not use its generic representation for the production k-mer graph without memory evidence; node indices and insertion order are not stable output IDs |
| `fixedbitset` 0.5.7 | 1.56 | MIT OR Apache-2.0 | Eligible | Useful for deterministic visited-state/indexed sets after stable node numbering; property-test size and index boundaries |
| `sux` 0.14.0 | 1.85 | Apache-2.0 OR LGPL-3.0 | Priority-B experiment only | Select and document the Apache-2.0 branch of the OR expression; verify architecture-specific code and Rust 1.85 before succinct/disk-scale claims |

Sources: [`bio-seq`](https://crates.io/crates/bio-seq/0.14.8),
[`nthash`](https://crates.io/crates/nthash/0.5.1),
[`debruijn`](https://crates.io/crates/debruijn/0.3.4),
[`GGCAT` 2.2.0 source](https://github.com/algbio/ggcat/tree/v2.2.0),
[`GGCAT` paper](https://doi.org/10.1101/gr.277615.122),
[`GGCAT` 2.2.0 license](https://github.com/algbio/ggcat/blob/v2.2.0/LICENSE),
[`petgraph`](https://crates.io/crates/petgraph/0.8.3),
[`fixedbitset`](https://crates.io/crates/fixedbitset/0.5.7), and
[`sux`](https://crates.io/crates/sux/0.14.0).

### Scientific semantics that a dependency may not change

- **Quality:** stable v0.1 uses a hard declared per-base Phred acceptance rule. An accepted occurrence
  or fragment event contributes one exact integer. No candidate crate may silently introduce
  fractional, expected, rounded, or probabilistically quality-weighted k-mer support.
- **IUPAC ambiguity:** the parser validates the declared IUPAC alphabet, while every non-A/C/G/T base
  resets the rolling k-mer window. Expansion, substitution, random resolution, and treating an
  ambiguous symbol as a fifth packed base are outside stable v0.1.
- **Low complexity:** stable v0.1 has no entropy, DUST-like, homopolymer, adapter-like, or other
  low-complexity deletion and emits no unversioned complexity score. A library's default masking or
  simplification must be disabled or treated as a new measured transformation requiring an ADR.
- **Coverage:** stable read-back fields are exact zero-mismatch placement groups and read-instance
  aggregates over emitted linear unitigs. They are not per-base depth, do not apportion multimappers,
  and must not be renamed “coverage.” A mapper, scoring model, multimapping policy, and calibrated
  schema are required before coverage reconstruction.

### Maps, parallelism, and bounded pipelines

| Candidate snapshot | Declared MSRV | License | Decision | Determinism consequence |
|---|---:|---|---|---|
| `hashbrown` 0.17.1 | 1.85 | MIT OR Apache-2.0 | Evaluate only against `std` | Hash iteration must never define IDs or output order; added dependency needs measured speed/memory benefit |
| `indexmap` 2.14.0 | 1.85 | MIT OR Apache-2.0 | Defer for graph identity | Insertion order is deterministic only if insertion schedule is deterministic; parallel insertion does not become reproducible by using an ordered map |
| `dashmap` 6.2.1 | 1.65 | MIT | Defer | Concurrent mutation complicates exact deterministic merge and memory accounting; prefer thread-local counts plus sorted checked reduction |
| `rayon` 1.12.0 | 1.80 | MIT OR Apache-2.0 | Adopted | Partition from stable indices, make operations associative with checked overflow, sort before serialization, and test full bundles across thread counts |
| `crossbeam-channel` 0.5.16 | 1.60 | MIT OR Apache-2.0 | Eligible if a bounded stage needs it | Capacity must be explicit and stress-tested; receiver arrival order cannot define biological or serialized order |

Sources: [`hashbrown`](https://crates.io/crates/hashbrown/0.17.1),
[`indexmap`](https://crates.io/crates/indexmap/2.14.0),
[`dashmap`](https://crates.io/crates/dashmap/6.2.1), and
[`crossbeam-channel`](https://crates.io/crates/crossbeam-channel/0.5.16).

### Disk-backed state and memory mapping

| Candidate snapshot | Declared MSRV | License | Decision | Safety/correctness gate |
|---|---:|---|---|---|
| `memmap2` 0.9.11 | 1.65 | MIT OR Apache-2.0 | Experimental partition reader only | Mapping a file creates an unsafe lifetime/mutation boundary inside the dependency. Require immutable staged files, length/checksum validation, truncated-file tests, and a safe local wrapper |
| `redb` 4.1.0 | 1.89 | MIT OR Apache-2.0 | Reject for v0.1 | Exceeds Rust 1.85 and a general embedded database is not yet justified for exact partition counting |
| `atomic-write-file` 0.3.1 | 1.85 | BSD-3-Clause | Eligible only for single-file study | It does not make a directory-sized output bundle atomic. VeritAsm still needs same-filesystem staging, complete validation, one commit point, collision checks, and failure injection |

Sources: [`memmap2`](https://crates.io/crates/memmap2/0.9.11),
[`redb`](https://crates.io/crates/redb/4.1.0), and
[`atomic-write-file`](https://crates.io/crates/atomic-write-file/0.3.1).

### Evidence serialization, checksums, and HTML

| Candidate snapshot | Declared MSRV | License | Decision | Guardrail |
|---|---:|---|---|---|
| `csv` 1.4.0 | 1.73 | Unlicense OR MIT | Eligible for TSV | Fix delimiter, quoting, line ending, null representation, numeric formatting, and column/schema version; golden-test bytes |
| `sha2` 0.11.0 | 1.85 | MIT OR Apache-2.0 | Adopted for integrity digests and the experimental Bloom hash contract | Record algorithm as SHA-256, hash raw bytes, close/fsync as required by the transaction design, and test against known vectors |
| `rustix` 1.1.4; direct request `default-features = false, features = ["fs"]`, unified normal graph `alloc/default/fs/std/termios` | 1.63 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Adopted for Linux/Apple no-replace directory commit | VeritAsm calls only safe `renameat_with(..., RenameFlags::NOREPLACE)`; fail closed on unsupported kernel/filesystem; do not fall back to replacing rename. `tempfile` and `clap`/`terminal_size` enable the additional unified features. `Cargo.lock` fixes the reviewed resolution, but the manifest's `^1.1.4` requirement permits later compatible releases if the lock is updated; MSRV and source review must be repeated then |
| `html-escape` 0.2.14 | 1.58 | MIT OR Apache-2.0 | Eligible for text-node/attribute escaping | HTML escaping is not JavaScript or CSS-context escaping. Prefer a static derived report without active embedded user strings; JSON remains a separate authoritative artifact |
| `askama` 0.16.0 | 1.88 | MIT OR Apache-2.0 | Reject for v0.1 | Exceeds the product MSRV; templating convenience does not justify raising Rust |
| `noodles` 0.116.0 workspace | 1.89 | MIT | Defer | Current workspace exceeds the MSRV and SAM/BAM/VCF are not yet necessary in the de novo core; audit an older exact pin only when read-audit scope is approved |
| `bio` 4.0.1 (`rust-bio`) | 1.87 | MIT | Reject as a broad convenience dependency | Exceeds the MSRV and imports much more surface than the current vertical slice needs |

Sources: [`csv`](https://crates.io/crates/csv/1.4.0),
[`sha2`](https://crates.io/crates/sha2/0.11.0),
[`rustix`](https://docs.rs/rustix/1.1.4/rustix/fs/fn.renameat_with.html),
[`html-escape`](https://crates.io/crates/html-escape/0.2.14),
[`askama`](https://crates.io/crates/askama/0.16.0),
[`noodles`](https://crates.io/crates/noodles/0.116.0), and
[`bio`](https://crates.io/crates/bio/4.0.1).

### Property tests, benchmarks, fuzzing, and supply-chain tools

| Candidate snapshot | Declared MSRV | License | Decision |
|---|---:|---|---|
| `assert_cmd` 2.2.2 | 1.85 | MIT OR Apache-2.0 | Adopted dev-dependency for CLI behavior and failure assertions |
| `predicates` 3.1.4 | 1.74 | MIT OR Apache-2.0 | Adopted dev-dependency used with `assert_cmd` |
| `proptest` 1.11.0 | 1.85 | MIT OR Apache-2.0 | Adopted dev-dependency for DNA encoding, canonicalization, graph, compaction, and Bloom/two-hit invariants |
| `criterion` 0.8.2 | 1.86 | Apache-2.0 OR MIT | Do not select on Rust 1.85; if compiled under the all-target MSRV gate, pin 0.7.0 (declared MSRV 1.80) and record the pin |
| `cargo-fuzz` 0.13.2 | No product MSRV conclusion | MIT OR Apache-2.0 | Use as a separately pinned nightly tool, never a runtime dependency; archive corpus and crash minimizations |
| `cargo-deny` 0.20.2 | 1.88 | MIT OR Apache-2.0 | Run on current-stable CI; pin tool version and policy; does not raise product MSRV |
| `cargo-audit` 0.22.2 | 1.88 | MIT OR Apache-2.0 | Run on current-stable CI with advisory database provenance; pin tool version; does not raise product MSRV |

Sources: [`assert_cmd`](https://crates.io/crates/assert_cmd/2.2.2),
[`predicates`](https://crates.io/crates/predicates/3.1.4),
[`proptest`](https://crates.io/crates/proptest/1.11.0),
[`criterion` 0.8.2](https://crates.io/crates/criterion/0.8.2),
[`criterion` 0.7.0](https://crates.io/crates/criterion/0.7.0),
[`cargo-fuzz`](https://crates.io/crates/cargo-fuzz/0.13.2),
[`cargo-deny`](https://crates.io/crates/cargo-deny/0.20.2), and
[`cargo-audit`](https://crates.io/crates/cargo-audit/0.22.2).

## File-format decisions

| Format | Normative source | VeritAsm decision | Evidence and interoperability caveat |
|---|---|---|---|
| FASTA | NCBI FASTA guidance [F1] | Required sequence output and accepted input. Emit uppercase IUPAC, stable unique IDs, deterministic record order and wrapping, and no semantic field that exists only in a free-text header | FASTA has widely varying dialects. Publish the exact local grammar; hash raw bytes and separately compute any normalized biological digest |
| FASTQ | NCBI SRA FASTQ guidance and the format/variant paper [F2], [F9] | Accepted input, including the baseline contract's wrapped records. Document Phred+33, exact identifier/mate rules, permitted IUPAC symbols, quality-length equality, and line/record limits | Common four-line descriptions do not cover every accepted wrapped form. Gzip is a transport wrapper detected from content, not a separate sequence format |
| GFA 1 | GFA1 specification [F3] | Emit a conservative GFA 1.0 common subset with embedded segment sequences, oriented links, declared overlaps, stable IDs, and paths only when path semantics are justified. Record `VN:Z:1.0` and document every custom tag | GFA record syntax does not make two tools' graph stage, coverage, path, or tag semantics equivalent. Round-trip with independent parsers/viewers before claiming interoperability. A graph cycle is only candidate topology |
| FASTG | FASTG specification archive [F4] | Do not use as VeritAsm's normative graph output; retain comparator FASTG unchanged | Implementations use differing conventions and some exports describe intermediate rather than final graphs |
| SAM/BAM | GA4GH/samtools SAM specification [F5] | Deferred read-audit output. Emit only after an explicit mapper, alignment scoring, multimapping policy, read-group provenance, sorting status, and reference/contig dictionary are defined | BAM is BGZF-framed binary SAM, not ordinary gzip. Alignment support is model-dependent and does not independently validate the construction reads |
| PAF | Heng Li/miniasm PAF specification [F6] | Optional future diagnostic for read-to-contig or contig-to-truth alignment; evaluator-owned unless VeritAsm implements the mapper | Core PAF does not necessarily carry a base-level CIGAR. Mapping quality and optional tags must not be treated as universal confidence probabilities |
| VCF | GA4GH/samtools VCF specification [F7] | Not a de novo v0.1 output. Reserve for a separate reference-relative variant layer with reference checksum, contig dictionary, normalization, ploidy/model, filters, and caller version | A graph bubble is not automatically a VCF allele, and unphased local alternatives are not global haplotypes |
| JSON | RFC 8259 [F8] | Authoritative structured evidence report with an explicit schema version, integer count types, defined nullability, stable key/array policy, and deterministic serialization tests | JSON member order is not generally semantic. VeritAsm may make byte ordering part of its own reproducibility contract, but consumers must use the schema |
| TSV | VeritAsm versioned schema; quoting rules may use the `csv` crate | Human/tool-friendly per-contig and per-junction evidence table, derived from the same typed records as JSON | File extension alone does not define quoting, nulls, arrays, or numeric precision; freeze all of them with the schema |
| HTML | VeritAsm derived view | Self-contained, offline, deterministic view of the machine report; no CDN, remote fonts, telemetry, runtime fetch, or truth-only benchmark data | HTML is not the authoritative evidence store. Escape by syntactic context, include generator/schema versions and artifact digests, and test hostile identifiers |

### Format-specific evidence rules

- Sequence IDs must join FASTA, GFA, JSON, TSV, and HTML through one stable identifier. Display names
  are not primary keys.
- Coordinate systems, interval closure, orientation, reverse-complement handling, circular rotation,
  and k-dependent overlaps must be stated once in the schema and tested at every export boundary.
- Every count field names its unit: occurrence, supplied read, supplied record pair, unique alignment,
  or compatible pair. “Coverage,” “depth,” “support,” and “abundance” are never interchangeable.
- GFA must retain unresolved alternatives even when the FASTA profile stops or selects a consensus.
  Removed edges and transformations belong in the evidence report; GFA alone need not encode the
  complete audit ledger.
- Output comparison uses raw artifact SHA-256 first. Any case/wrap/order/reverse-complement/circular
  normalization is a separately versioned evaluation and never overwrites raw evidence.
- HTML and JSON must be produced in the same staged result transaction as FASTA/GFA/TSV. Writing
  files one by one with atomic rename does not protect an existing bundle from partial replacement.

## Dependency acceptance gate

Before adding or updating any crate, record:

1. the exact version and checksum in `Cargo.lock`, source registry, resolved feature set, and why the
   standard library/current code is insufficient;
2. package `rust-version` and a successful
   `cargo +1.85 test --locked --all-targets --all-features`;
3. complete license expression, source/notice audit, build script, native code, and selected branch
   of every `OR` license;
4. RustSec results plus maintainer/release/advisory signals, without equating recent release with
   correctness;
5. default and enabled-feature runtime I/O, including proof that the executable introduces no
   network path;
6. unsafe code and platform-specific code in the dependency closure, with a narrow testable boundary;
7. allocation and streaming behavior on adversarial record sizes, graph skew, and corrupt inputs;
8. deterministic output across 1, 2, and N threads and repeated processes;
9. Apple Silicon macOS and Linux build/test evidence; and
10. clean-extraction and `cargo package` verification.

An MSRV-compatible manifest is necessary but insufficient. A crate may conditionally compile a
newer-language path, invoke native tools, or pull a transitive release with a higher requirement.

## License policy

- Prefer MIT, Apache-2.0, BSD-2/3-Clause, ISC, and similarly permissive dependencies compatible with
  the MIT project distribution.
- For locked crates whose metadata offers MIT as an `OR` branch, this review selects MIT for
  project distribution while retaining every packaged license and notice. `unicode-ident` still
  requires Unicode-3.0 as an `AND` term; `normalize-line-endings` is Apache-2.0-only. The
  `libfuzzer-sys` metadata/source discrepancy recorded above prevents a fuzz-closure distribution
  decision. These are distribution choices, not a completed per-file audit.
- For `A OR B`, explicitly record the selected permissive branch in the license policy and retain
  notices required by that branch. Do not silently treat `OR` as `AND` or vice versa.
- Do not vendor GPL, LGPL-only, or AGPL code into the Rust package without a dedicated legal and
  distribution review. Separate comparator executables retain their own licenses and are never
  linked into or redistributed inside VeritAsm by default.
- Public-domain statements do not erase licenses on bundled third-party files. SKESA is the concrete
  comparator warning: review every file in the frozen archive.
- Cargo metadata cannot detect all copied/generated code or data licenses. The source archive is the
  controlling object.

## Explicit unverified gaps

- Candidate-crate releases not present in `Cargo.lock` have not been downloaded, built, source-audited,
  or tested in this repository. Their rows are screening decisions only.
- The current closure is now enumerated, but the unsafe scan is lexical rather than a semantic
  inspection of every block and the license pass is metadata/tool-assisted rather than a complete
  per-file legal audit. The recorded clean `cargo-deny`/`cargo-audit` results are tied to the
  historical hashes, tool versions, and RustSec commit above. The third-party closure is unchanged,
  but fresh scans of the current four hashes were **NOT RUN** and remain a release gate.
- The root lock's dev-only WASI path contains `wasip2 1.0.4+wasi-0.2.12` with declared Rust 1.87.
  Linux/macOS product builds do not select it, but the project must either define that target as
  outside the 1.85 gate or resolve the dev graph before claiming whole-lock MSRV compatibility.
- Fuzz/native redistribution is blocked pending resolution of `libfuzzer-sys`'s NCSA versus
  Apache-2.0-WITH-LLVM-exception evidence. The target-specific prebuilt `wit-bindgen` archive also
  lacks a reproducible source-to-binary comparison in this review.
- `needletail` has no relied-upon declared MSRV in this review and has not passed the inherited FASTX
  compatibility corpus. It is not an approved parser replacement.
- `bio-seq`, `nthash`, `sux`, `memmap2`, `atomic-write-file`, `csv`, and `html-escape` remain
  screening decisions and are not present as direct dependencies. `sha2`, `assert_cmd`, `predicates`,
  and `proptest` are present in the current locked direct dependency inventory; their use does not
  establish completion of every proposed integrity, CLI, property, or fuzz gate.
- GGCAT 2.2.0 has not been built, source/dependency-audited, or compared with VeritAsm's retained-key,
  graph-link, palindromic-boundary, maximal-unitig, determinism, and evidence semantics. Its Rust API
  and tagged MIT file make it eligible for evaluation, not approved for integration.
- A deterministic GFA 1.0-subset writer and internal validator exist in `src/bundle.rs`, with CLI and
  module tests for current stable records. Independent parser/viewer round trips, Bandage
  compatibility, and broader interoperability across closed-walk and palindromic cases remain
  unverified; implementation is not interoperability evidence.
- No SAM/BAM, PAF, or VCF dependency is selected. An older `noodles` pin has not been audited for
  Rust 1.85, advisories, or required format coverage.
- Atomic whole-bundle commit semantics are not supplied by `tempfile` or `atomic-write-file` alone.
  Cross-filesystem paths, destination aliases, interrupted rename, permission failure, disk-full,
  and pre-existing-result preservation require dedicated implementation and failure injection.
- Static inspection of the current first-party product path found no runtime-network implementation.
  Dynamic network-denied execution was **NOT RUN**; every new feature/dependency needs both static and
  runtime enforcement evidence before a release claim.

## Format sources

- **[F1]** NCBI. “FASTA format.” <https://www.ncbi.nlm.nih.gov/genbank/fastaformat/>
- **[F2]** NCBI Sequence Read Archive. “File format guide: FASTQ.”
  <https://www.ncbi.nlm.nih.gov/sra/docs/submitformats/#fastq-files>
- **[F3]** GFA specification maintainers. “Graphical Fragment Assembly format, GFA1.” Reviewed
  source snapshot `9774d44132884d9a019c0f2682cb109be23c2db4`.
  <https://github.com/GFA-spec/GFA-spec/blob/9774d44132884d9a019c0f2682cb109be23c2db4/GFA1.md>
- **[F4]** FASTG specification archive. <https://fastg.sourceforge.net/FASTG_Spec_v1.00.pdf>
- **[F5]** GA4GH/samtools. “Sequence Alignment/Map format specification.”
  <https://samtools.github.io/hts-specs/SAMv1.pdf>
- **[F6]** Li H. “PAF: a Pairwise mApping Format.” Reviewed source snapshot
  `2cd690de1e6af3e0438b7df0a99dd3e3b27ad6f9`.
  <https://github.com/lh3/miniasm/blob/2cd690de1e6af3e0438b7df0a99dd3e3b27ad6f9/PAF.md>
- **[F7]** GA4GH/samtools. “Variant Call Format specification.”
  <https://samtools.github.io/hts-specs/VCFv4.5.pdf>
- **[F8]** Bray T. “The JavaScript Object Notation (JSON) Data Interchange Format.” RFC 8259
  (2017). <https://www.rfc-editor.org/rfc/rfc8259>
- **[F9]** Cock PJA et al. “The Sanger FASTQ file format for sequences with quality scores, and the
  Solexa/Illumina FASTQ variants.” *Nucleic Acids Research* (2010).
  <https://doi.org/10.1093/nar/gkp1137>
