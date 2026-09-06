# Source-package verification and human handoff

> Historical development/release-review record. For the 0.4.0-dev.1 preservation checkpoint, see [current status](STATUS.md). Earlier results below retain their original snapshot scope.

Status: commands frozen for the 0.3.0-alpha.1 review package; execution evidence is pending until logs and
checksums from the final archive are retained. These commands do not publish a crate, create a remote
repository, or push anything unless a human deliberately runs the final push block.

Required canonical files:

- `veritasm-0.3.0-alpha.1-source.zip`
- `veritasm-0.3.0-alpha.1-source.zip.sha256`

Deliver those unrenamed files inside a unique directory such as
`veritasm-0.3.0-alpha.1-review-<12-char-commit>/`, together with an external verification report that
binds the full commit, canonical ZIP SHA-256, transcript, and final result. The full verifier rejects a
renamed ZIP basename, and the checksum sidecar records the canonical name. A uniquely named outer
review bundle may wrap the handoff directory, but the inner ZIP and sidecar names must remain unchanged.

These are repository-owned review artifacts produced from `scripts/package_source.sh`. They are not
GitHub's automatically generated **Download ZIP** / **Source code (zip)** archive, are not
byte-identical to that archive, and must not be paired with a checksum copied from another download.
The deterministic-package and clean-extraction claims below cover only the canonical ZIP and sidecar
inside that identified handoff directory. A GitHub-generated
archive can be evaluated as an ordinary checkout snapshot only after its commit identity is
authenticated separately; GitHub chooses its top-level directory name and does not provide the
project-owned inventory/checksum contract.

An adjacent verification report may record the final archive hash and commands run after creation.
It is deliberately outside the ZIP: embedding the final ZIP's own digest or post-creation result
inside that ZIP would be self-referential. The source documents therefore remain conservative about
clean-extraction status until read together with that sidecar report.

## Commit-bearing handoff directory

First generate and verify the canonical pair with the commands below. After the canonical verifier
passes, place the unrenamed pair in a uniquely named handoff directory from the same clean commit:

```bash
set -euo pipefail
FULL_COMMIT="$(git rev-parse HEAD)"
SHORT_COMMIT="$(git rev-parse --short=12 HEAD)"
HANDOFF_DIR="../veritasm-0.3.0-alpha.1-review-${SHORT_COMMIT}"
test ! -e "$HANDOFF_DIR"
mkdir "$HANDOFF_DIR"
cp -p veritasm-0.3.0-alpha.1-source.zip "$HANDOFF_DIR/"
cp -p veritasm-0.3.0-alpha.1-source.zip.sha256 "$HANDOFF_DIR/"
(cd "$HANDOFF_DIR" && sha256sum -c veritasm-0.3.0-alpha.1-source.zip.sha256)
ZIP_DIGEST="$(awk 'NR == 1 { print $1 }' "$HANDOFF_DIR/veritasm-0.3.0-alpha.1-source.zip.sha256")"
printf 'handoff_directory=%s\ncommit=%s\nsha256=%s\n' \
  "$HANDOFF_DIR" "$FULL_COMMIT" "$ZIP_DIGEST"
```

The external verification report records those final three values and links them to the retained
verifier transcript. To verify a delivered directory on Linux, keep the canonical pair together:

```bash
set -euo pipefail
HANDOFF_DIR='/absolute/path/to/veritasm-0.3.0-alpha.1-review-REPLACE12'
ARCHIVE="$HANDOFF_DIR/veritasm-0.3.0-alpha.1-source.zip"
CHECKSUM="$HANDOFF_DIR/veritasm-0.3.0-alpha.1-source.zip.sha256"
(cd "$HANDOFF_DIR" && sha256sum -c veritasm-0.3.0-alpha.1-source.zip.sha256)
VERIFY_CHECKOUT="$(mktemp -d "${TMPDIR:-/tmp}/veritasm-review.XXXXXXXX")"
unzip -q "$ARCHIVE" -d "$VERIFY_CHECKOUT"
"$VERIFY_CHECKOUT/veritasm-0.3.0-alpha.1/scripts/verify_release_candidate.sh" \
  "$ARCHIVE" "$CHECKSUM"
```

Do not rename the inner pair. If an outer archive is used for transport, extract it first and verify
the canonical inner pair from its commit-bearing directory.

The archive must contain one top-level directory named `veritasm-0.3.0-alpha.1/` and must exclude `.git/`,
`target/`, temporary files, generated result directories, private reads, benchmark result caches, and
credentials.

## Deterministic Linux source-package creation

Run the repository-owned packager from a Linux checkout after all reviewed changes are complete. It
requires Bash, Info-ZIP `zip`/`unzip`, and GNU `sha256sum`; it validates the archive inventory,
contents, ZIP integrity, and checksum before publishing either review file. Existing review files are
never replaced.

The commit recorded in a handoff is evidence only if the package was made from that clean commit.
The following sequence rejects index, tracked-worktree, and non-ignored untracked changes before the
build; proves the checkout did not change during the build; and compares every archived path and byte
with the captured Git tree. Keep this sequence in one shell and record `PACKAGE_COMMIT` in the
external verification report.

The cooperative package lock serializes packager invocations, not arbitrary editors or generators.
Keep the checkout quiescent for the command's duration. The packager hashes the exact live-source
inventory before staging, verifies the staged copy against it, then re-enumerates and re-hashes the
live tree immediately before publication and again before declaring completion. A detected path,
type, or byte change rolls back any owned partial publication. This fail-closed check detects
concurrent changes; it does not make unrelated filesystem writers transactional.

```bash
set -euo pipefail
PACKAGE_COMMIT="$(git rev-parse HEAD)"
git diff --quiet
git diff --cached --quiet
test -z "$(git ls-files --others --exclude-standard)"
test ! -e veritasm-0.3.0-alpha.1-source.zip
test ! -e veritasm-0.3.0-alpha.1-source.zip.sha256
./scripts/package_source.sh
sha256sum -c veritasm-0.3.0-alpha.1-source.zip.sha256
unzip -t veritasm-0.3.0-alpha.1-source.zip
test "$PACKAGE_COMMIT" = "$(git rev-parse HEAD)"
git diff --quiet
git diff --cached --quiet
test -z "$(git ls-files --others --exclude-standard)"

COMMIT_INVENTORY="$(mktemp "${TMPDIR:-/tmp}/veritasm-commit-inventory.XXXXXXXX")"
ZIP_INVENTORY="$(mktemp "${TMPDIR:-/tmp}/veritasm-zip-inventory.XXXXXXXX")"
trap 'rm -f -- "$COMMIT_INVENTORY" "$ZIP_INVENTORY"' EXIT
git ls-tree -r --name-only "$PACKAGE_COMMIT" | LC_ALL=C sort >"$COMMIT_INVENTORY"
unzip -Z1 veritasm-0.3.0-alpha.1-source.zip \
  | sed 's#^veritasm-0.3.0-alpha.1/##' \
  | LC_ALL=C sort >"$ZIP_INVENTORY"
cmp "$COMMIT_INVENTORY" "$ZIP_INVENTORY"
while IFS= read -r SOURCE_PATH; do
  cmp -s \
    <(git show "$PACKAGE_COMMIT:$SOURCE_PATH") \
    <(unzip -p veritasm-0.3.0-alpha.1-source.zip \
      "veritasm-0.3.0-alpha.1/$SOURCE_PATH")
done <"$COMMIT_INVENTORY"
printf 'package_commit=%s\n' "$PACKAGE_COMMIT"
```

## Paste-safe local release-candidate verification

The preferred local correctness check is the repository-owned verifier below. Run it from the
checkout that created the source review files; do not paste the verifier's implementation into an
interactive shell. This example assumes the checkout root is the current directory; when invoking it
elsewhere, pass absolute paths for the script, ZIP, and sidecar:

```bash
./scripts/verify_release_candidate.sh "$PWD/veritasm-0.3.0-alpha.1-source.zip" "$PWD/veritasm-0.3.0-alpha.1-source.zip.sha256"
```

The verifier performs these checks without reading a checkout `target/` or reusing a prior
executable. The packaged verifier and source allowlist must be byte-identical to those in the
producing checkout, so the ZIP cannot define a different inventory policy for itself:

1. rejects a source ZIP larger than 64 MiB, then parses a single strict lowercase SHA-256 sidecar
   record and recomputes the ZIP digest;
2. before decompression, reads the central directory and rejects more than 4,096 members, any member
   declaring more than 64 MiB, more than 256 MiB total declared uncompressed data, a declared
   expansion ratio above 200:1, inconsistent counts/sizes, and newline-bearing names; it then tests
   ZIP integrity, rejects unsafe/duplicate entries, and compares the exact file inventory with the
   packaged allowlist plus required root files;
3. extracts the ZIP twice into newly created directories and proves both regular-file inventories
   and contents byte-identical;
4. uses a different fresh `CARGO_TARGET_DIR` for check/Clippy, test/doc, and release/package phases
   on Rust 1.85 and current-stable source trees, and for check, test/doc, and release phases on each
   independently extracted Cargo crate;
5. runs `fmt`, all-target/all-feature `check`, warnings-denied `clippy`, all-target tests, doctests,
   warnings-denied rustdoc, a release build, and `cargo package` on each installed required toolchain;
6. copies and hashes each generated `.crate`, validates its paths, extracts it independently, and
   runs check, tests, doctests, warnings-denied rustdoc, and a release build with the producing
   toolchain;
7. creates a second stable `.crate` from the other fresh source extraction in a new target and requires
   its bytes to match the first stable `.crate`; and
8. runs a plain single-end assembly and a content-detected-gzip paired-end assembly with each isolated
   release executable, then checks exact bundle inventory and every manifest digest.

The thirteen build roots are explicitly distinct and cannot be overridden by caller
`CARGO_TARGET_DIR`:

| Phase | Isolated target beneath the fresh verification root |
|---|---|
| Rust 1.85 source ZIP check/Clippy | `target-source-check-msrv` |
| Rust 1.85 source ZIP test/doc | `target-source-test-msrv` |
| Rust 1.85 source ZIP release/package | `target-source-release-msrv` |
| Rust 1.85 extracted Cargo crate check | `target-crate-check-msrv` |
| Rust 1.85 extracted Cargo crate test/doc | `target-crate-test-msrv` |
| Rust 1.85 extracted Cargo crate release build | `target-crate-release-msrv` |
| Installed stable source ZIP check/Clippy | `target-source-check-stable` |
| Installed stable source ZIP test/doc | `target-source-test-stable` |
| Installed stable source ZIP release/package | `target-source-release-stable` |
| Installed stable extracted Cargo crate check | `target-crate-check-stable` |
| Installed stable extracted Cargo crate test/doc | `target-crate-test-stable` |
| Installed stable extracted Cargo crate release build | `target-crate-release-stable` |
| Installed stable repeat package from the other source extraction | `target-source-package-stable-repeat` |

Never collapse them into one shared target. During development, a stable-then-MSRV verification attempt
that reused one target left `target/debug/veritasm` with mode `0644`, causing 15 CLI
`PermissionDenied` failures. A later Cargo 1.85 source check/test target reuse similarly left the
top-level binary at mode `0600`, causing all 16 CLI tests to fail before a fresh test-only target
passed. Those were contaminated build-artifact failures, not evidence of a Rust source defect;
phase-isolated fresh targets are the corrective control.

Manual diagnostic reruns must preserve the same separation. After replacing `VERIFY_ROOT` with the
retained verifier directory, this block performs one compilation check in each source and crate
extraction with four explicit, initially absent check targets. It is a target-isolation example, not a
substitute for the verifier's complete gate sequence:

```bash
set -euo pipefail
VERIFY_ROOT='/absolute/path/to/veritasm-verify-0.3.0-alpha.1.XXXXXXXX'
MSRV_SOURCE="$VERIFY_ROOT/extraction-msrv/veritasm-0.3.0-alpha.1"
MSRV_CRATE="$VERIFY_ROOT/crate-extract-msrv/veritasm-0.3.0-alpha.1"
STABLE_SOURCE="$VERIFY_ROOT/extraction-stable/veritasm-0.3.0-alpha.1"
STABLE_CRATE="$VERIFY_ROOT/crate-extract-stable/veritasm-0.3.0-alpha.1"

MSRV_SOURCE_TARGET="$VERIFY_ROOT/manual-target-source-msrv"
MSRV_CRATE_TARGET="$VERIFY_ROOT/manual-target-crate-msrv"
STABLE_SOURCE_TARGET="$VERIFY_ROOT/manual-target-source-stable"
STABLE_CRATE_TARGET="$VERIFY_ROOT/manual-target-crate-stable"
test ! -e "$MSRV_SOURCE_TARGET"
test ! -e "$MSRV_CRATE_TARGET"
test ! -e "$STABLE_SOURCE_TARGET"
test ! -e "$STABLE_CRATE_TARGET"

(cd "$MSRV_SOURCE" && CARGO_TARGET_DIR="$MSRV_SOURCE_TARGET" \
  cargo +1.85.0 check --locked --all-targets --all-features)
(cd "$MSRV_CRATE" && CARGO_TARGET_DIR="$MSRV_CRATE_TARGET" \
  cargo +1.85.0 check --locked --all-targets --all-features)
(cd "$STABLE_SOURCE" && CARGO_TARGET_DIR="$STABLE_SOURCE_TARGET" \
  cargo +stable check --locked --all-targets --all-features)
(cd "$STABLE_CRATE" && CARGO_TARGET_DIR="$STABLE_CRATE_TARGET" \
  cargo +stable check --locked --all-targets --all-features)
```

The script does not install Rust or mutate Rustup. Install the two toolchains and components first if
needed:

```bash
rustup toolchain install 1.85.0 --profile minimal --component rustfmt,clippy
rustup toolchain install stable --profile minimal --component rustfmt,clippy
```

The exact MSRV toolchain must resolve to `rustc 1.85.0`; a differently named or redirected compiler
cannot satisfy that gate. The installed `stable` channel must resolve to a release at least as new as
the MSRV, and its exact observed version is recorded. Running `rustup toolchain install stable` above
updates that channel; the script itself does not contact Rustup to decide whether a newer stable has
since been published.

Exit status `0` means all locally executable checks above passed. Status `1` means at least one check
failed. Status `2` means no executed check failed but one or both required Rustup toolchains were
unavailable, or the host was not recognized as Linux or Apple Silicon macOS; the result is
`INCOMPLETE`, not a pass. The verifier prints and retains a transcript, summary, both source
extractions, smoke bundles, generated crate archives, and crate extractions under a new
`veritasm-verify-0.3.0-alpha.1.*` directory. It removes only isolated `target-*` directories that it created
beneath that exact verification root, and removes completed targets between build phases to reduce
peak disk use. Set `VERITASM_VERIFY_WORK_PARENT` to an existing local directory to choose where that
root is created. The work parent must permit immediate execution of newly linked native programs;
prefer the default operating-system temporary directory over synchronized, watched, or otherwise
mediated workspaces. A process-launch denial in a custom work parent is a failed run, not a test waiver;
repeat the complete unchanged verifier in a confirmed executable local temporary directory and retain
both records. Its fresh Cargo home excludes caller configuration and extracted dependency sources.
When a caller registry cache and index exist, the verifier symlinks those two cache directories to
avoid redundant downloads; they remain shared, mutable cache state and are not hermetic evidence.
Locked checksums still apply, dependency sources are freshly extracted, and all build targets remain
isolated. Use a copied immutable cache plus enforced network denial when cache immutability itself is
part of the review claim.

The authoritative summary is written atomically only after isolated target/Cargo-home cleanup and
transcript finalization. A cleanup, transcript-writer, or FIFO-removal failure forces exit status `1`
and `Overall: FAIL`; it cannot leave a retained `Overall: PASS` summary. The preflight limits bound
declared archive expansion, not malicious decompressor CPU behavior or a forged local header. Treat
the verifier as a fail-closed consistency/build check, not as a general-purpose hostile-archive
sandbox.

After creating a review ZIP, the focused shell self-tests below run no Rust compilation. The first
checks ZIP preflight boundaries, source-root/inventory policy, live-source mutation detection,
generated bundle manifests, failure cleanup, and stable assembly-contract v0.1 rejection of an
unexpected artifact even when its checksum is internally consistent. The second hides Rustup from the
verifier and injects both a Cargo-home replacement and an unregistered `target-*` residue. Each
finalization injection must produce exit status `1`, `Overall: FAIL`, explicit cleanup failure, and no
retained transcript FIFO:

```bash
./scripts/test_release_verifier_tree_manifest.sh
./scripts/test_release_verifier_finalization.sh \
  "$PWD/veritasm-0.3.0-alpha.1-source.zip" \
  "$PWD/veritasm-0.3.0-alpha.1-source.zip.sha256"
```

The summary always marks dependency auditing, sanitizer fuzzing, network-denied execution,
cross-platform deterministic equality, and scientific validation as `NOT RUN`; those require their
separate gates. It reports only the local kernel and architecture. In particular, execution on Linux
cannot establish Apple Silicon macOS compatibility.

## Exact Apple Silicon macOS verification commands

Run this block from the producing checkout while the two review files are still byte-identical to the
copies intended for handoff. It installs the declared MSRV and updates the local stable Rust channel,
but it does not install Rust itself. The Cargo build steps may contact the configured registry if the
locked dependencies are not cached; that is build-time access, not VeritAsm runtime behavior.

Repository CI selects GitHub's documented public `macos-15` Arm64 image and then asserts both
`uname -m = arm64` and the Rust host triple. The label is tracked in GitHub's
[runner-image table](https://github.com/actions/runner-images#available-images); the runtime checks
fail closed if its architecture semantics change.

Keep separate `CARGO_TARGET_DIR` values for MSRV, current stable, and the independently extracted
Cargo package. Cargo build artifacts are not an interchange format between compiler toolchains or
source roots; sharing one target tree can produce stale or non-executable test binaries rather than
an independent clean-build result.

```bash
set -euo pipefail

ARCHIVE="$PWD/veritasm-0.3.0-alpha.1-source.zip"
CHECKSUM="$PWD/veritasm-0.3.0-alpha.1-source.zip.sha256"
VERIFIER="$PWD/scripts/verify_release_candidate.sh"
test -f "$ARCHIVE"
test -f "$CHECKSUM"
test -x "$VERIFIER"
uname -m | grep -qx 'arm64'
sw_vers
xcode-select -p
rustup --version

shasum -a 256 -c "$CHECKSUM"
unzip -t "$ARCHIVE"

rustup toolchain install 1.85.0 --profile minimal --component rustfmt,clippy
rustup toolchain install stable --profile minimal --component rustfmt,clippy
"$VERIFIER" "$ARCHIVE" "$CHECKSUM"
```

A reviewer who received the commit-bearing handoff directory can bootstrap the same verifier without
extracting the source ZIP first. The canonical checksum sidecar and full commit identity must come from
a trusted handoff channel. This executes code from the package and therefore verifies reproducibility
and packaging consistency; it is not a sandbox for hostile code.

```bash
set -euo pipefail

HANDOFF_ROOT=$(pwd -P)
ARCHIVE="$HANDOFF_ROOT/veritasm-0.3.0-alpha.1-source.zip"
CHECKSUM="$HANDOFF_ROOT/veritasm-0.3.0-alpha.1-source.zip.sha256"
(cd "$HANDOFF_ROOT" && shasum -a 256 -c veritasm-0.3.0-alpha.1-source.zip.sha256)
unzip -t "$ARCHIVE"

BOOTSTRAP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/veritasm-verifier-bootstrap.XXXXXXXX")
BOOTSTRAP_PROJECT="$BOOTSTRAP_ROOT/veritasm-0.3.0-alpha.1"
mkdir -p "$BOOTSTRAP_PROJECT/scripts"
unzip -p "$ARCHIVE" veritasm-0.3.0-alpha.1/scripts/verify_release_candidate.sh \
  >"$BOOTSTRAP_PROJECT/scripts/verify_release_candidate.sh"
unzip -p "$ARCHIVE" veritasm-0.3.0-alpha.1/scripts/source-package-files.txt \
  >"$BOOTSTRAP_PROJECT/scripts/source-package-files.txt"
chmod 0755 "$BOOTSTRAP_PROJECT/scripts/verify_release_candidate.sh"
"$BOOTSTRAP_PROJECT/scripts/verify_release_candidate.sh" "$ARCHIVE" "$CHECKSUM"
```

The verifier's summary must say `EXECUTED on Apple Silicon macOS host`; that line is not produced by a
Linux run. It already performs both ordinary smoke assemblies and full manifest verification. Run a
separate network-denied executable smoke test as an additional runtime-isolation gate. Apple's
`sandbox-exec` is deprecated but remains useful as a review-only check when present; lack of that
utility is a recorded unavailable check, not a reason to claim runtime isolation.

```bash
set -euo pipefail
ARCHIVE="$PWD/veritasm-0.3.0-alpha.1-source.zip"
VERIFY_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/veritasm-offline-smoke.XXXXXXXX")
ditto -x -k "$ARCHIVE" "$VERIFY_ROOT"
PROJECT_ROOT="$VERIFY_ROOT/veritasm-0.3.0-alpha.1"
test -f "$PROJECT_ROOT/Cargo.toml"
cd "$PROJECT_ROOT"
CARGO_TARGET_DIR="$VERIFY_ROOT/target" cargo +stable build --locked --release
RUNTIME_OUTPUT="$VERIFY_ROOT/offline-smoke"
test ! -e "$RUNTIME_OUTPUT"
/usr/bin/sandbox-exec \
  -p '(version 1)(allow default)(deny network*)' \
  "$VERIFY_ROOT/target/release/veritasm" assemble \
  --single examples/reads.fasta \
  --output-dir "$RUNTIME_OUTPUT" \
  --k 5 \
  --profile retain-all \
  --min-base-quality 0
test -f "$RUNTIME_OUTPUT/manifest.sha256"
(cd "$RUNTIME_OUTPUT" && shasum -a 256 -c manifest.sha256)
```

The smoke test checks only one small local invocation and manifest. It does not prove absence of every
network path, bounded memory, parser safety, scientific accuracy, or platform-wide atomicity.

For the dependency advisory and policy checks, use the frozen verifier versions below under current
stable Rust. Audit both the root lockfile and the separate fuzz lockfile: Cargo packaging the root
crate does not substitute for reviewing the fuzz dependency closure. These commands update
development tool/advisory caches and therefore require network access unless those caches are
already provisioned.

```bash
set -euo pipefail
ARCHIVE="$PWD/veritasm-0.3.0-alpha.1-source.zip"
VERIFY_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/veritasm-dependency-audit.XXXXXXXX")
ditto -x -k "$ARCHIVE" "$VERIFY_ROOT"
PROJECT_ROOT="$VERIFY_ROOT/veritasm-0.3.0-alpha.1"
test -f "$PROJECT_ROOT/Cargo.toml"
cd "$PROJECT_ROOT"
cargo +stable install --locked --version 0.22.2 cargo-audit
cargo +stable install --locked --version 0.20.2 cargo-deny
cargo +stable fetch --locked --manifest-path Cargo.toml
cargo +stable fetch --locked --manifest-path fuzz/Cargo.toml
cargo +stable audit --deny warnings --file Cargo.lock
cargo +stable deny --manifest-path Cargo.toml fetch db
cargo +stable deny --manifest-path Cargo.toml --all-features --frozen \
  check advisories bans licenses sources
cargo +stable audit --deny warnings --no-fetch --file fuzz/Cargo.lock
cargo +stable deny --manifest-path fuzz/Cargo.toml --all-features --frozen \
  check advisories bans licenses sources
cargo_data_home="${CARGO_HOME:-${HOME}/.cargo}"
git -C "$cargo_data_home/advisory-db" rev-parse HEAD
for deny_db in "$cargo_data_home"/advisory-dbs/*; do
  test -d "$deny_db/.git"
  git -C "$deny_db" rev-parse HEAD
done
```

Retain stdout, stderr, exit status, the two Rust version lines, macOS version, architecture, source ZIP
SHA-256, packaged-crate SHA-256, `zip -v` implementation/version output, and the final result
manifest. A passing CI configuration or an uncaptured local run is not retained release evidence.
The packager normalizes its inputs and removes ambient Info-ZIP options; its twice-built comparison
establishes byte identity for the recorded tool/environment. It does not claim that unrelated ZIP
implementations produce identical containers.

## Linux verification parity

On x86-64 Linux, install or update the same Rustup toolchains and run
`scripts/verify_release_candidate.sh` against the canonical ZIP and sidecar inside the commit-bearing
handoff directory. It selects `sha256sum`,
extracts with `unzip`, and records Linux as the local platform while explicitly marking Apple Silicon
macOS `NOT RUN`. The verifier does not perform the separate runtime-isolation gate. Use an enforced
network-denial mechanism for that check; ordinary CI execution is not runtime-isolation evidence. If
ptrace, network namespaces, or an equivalent sandbox are unavailable, record the dynamic check as
`NOT RUN` rather than inferring a pass from source inspection. Record kernel, distribution,
architecture, libc, filesystem, and no-replace transaction results. Linux and macOS performance
values are separate hardware strata and must not be pooled.

## Exact human GitHub initialization and push commands

Do not run this section until a human has approved the verified archive, selected a public project
name, completed the name/legal review, and manually created an empty GitHub repository with no
generated README, license, or `.gitignore`. The exact owner and repository are deliberately not
guessed. Replace the first two values, then run the block from the clean verified source directory.

```bash
set -euo pipefail

PROJECT_ROOT='/absolute/path/to/veritasm-0.3.0-alpha.1'
GITHUB_OWNER='REPLACE_WITH_APPROVED_OWNER'
GITHUB_REPOSITORY='REPLACE_WITH_APPROVED_REPOSITORY'

test -f "$PROJECT_ROOT/Cargo.toml"
test "$GITHUB_OWNER" != 'REPLACE_WITH_APPROVED_OWNER'
test "$GITHUB_REPOSITORY" != 'REPLACE_WITH_APPROVED_REPOSITORY'
cd "$PROJECT_ROOT"
test -n "$(git config --get user.name)"
test -n "$(git config --get user.email)"

git init -b main
git add --all
git ls-files --error-unmatch examples/data/basic_paired/reads_R1.fastq.gz
git ls-files --error-unmatch examples/data/basic_paired/reads_R2.fastq.gz
git status --short
git commit -m 'Prepare 0.3.0-alpha.1 review candidate'

GITHUB_REMOTE="git@github.com:${GITHUB_OWNER}/${GITHUB_REPOSITORY}.git"
git remote add origin "$GITHUB_REMOTE"
git remote -v
git push --dry-run origin main
git push --set-upstream origin main
```

Review `git status`, the staged file list, and `git remote -v` before the two push commands. Do not
create or push a tag, GitHub release, crate publication, container, benchmark result, or package
registry artifact as part of this handoff. Those are separate human-approved actions.

## Required evidence ledger

The final report must mark each row `PASS`, `FAIL`, or `NOT RUN` and link it to retained logs or machine
artifacts. `PASS` is not inferred from source inspection.

| Gate | Required evidence |
|---|---|
| Source checksum and extraction | Preflight size/expansion budgets, SHA-256 check, ZIP integrity, exact inventory, absence of build/private artifacts |
| Rust 1.85 | fmt, Clippy with warnings denied, all-target tests, rustdoc warnings denied, release build |
| Current stable | Same gates plus `cargo package` |
| Packaged crate | Extracted `.crate` build/test under Rust 1.85, its SHA-256, and byte equality across two fresh source extractions and stable-Cargo targets |
| Dependencies | Frozen `cargo-audit`/`cargo-deny` output plus manual source/license/build-script/native/unsafe review |
| Runtime isolation | Network-denied smoke plus static dependency/runtime inspection; no external process or database |
| Output safety | Existing-destination, injected pre-commit failure, concurrent writer, and no-replace tests |
| Determinism | Complete bundle-byte equality across declared thread counts, repeats, and platforms |
| Scientific validation | Pre-registered dataset/evaluator/comparator manifests and every success, failure, and regression |
