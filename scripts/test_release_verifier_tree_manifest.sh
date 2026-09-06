#!/usr/bin/env bash
# Focused release-verifier regression tests that do not compile Rust.

set -euo pipefail
IFS=$'\n\t'
export LC_ALL=C
umask 077

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
verifier="$script_directory/verify_release_candidate.sh"
test_parent=$(mktemp -d "${TMPDIR:-/tmp}/veritasm-tree-manifest-test.XXXXXXXX")

cleanup() {
    local status=$?
    trap - EXIT HUP INT TERM
    set +e
    case "$test_parent" in
        "${TMPDIR:-/tmp}"/veritasm-tree-manifest-test.*)
            if ! find "$test_parent" -depth -mindepth 1 -delete; then
                status=1
            fi
            if [[ -d "$test_parent" ]] && ! rmdir -- "$test_parent"; then
                status=1
            fi
            ;;
        *)
            printf 'refusing to clean unexpected self-test path: %s\n' "$test_parent" >&2
            status=1
            ;;
    esac
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

for required_tool in awk cat chmod cmp cp diff dirname find grep ln mkdir mkfifo mktemp mv rmdir sed sort tr uniq unlink unzip wc zip; do
    command -v "$required_tool" >/dev/null 2>&1 \
        || { printf 'missing tree-manifest self-test tool: %s\n' "$required_tool" >&2; exit 1; }
done
if ! command -v sha256sum >/dev/null 2>&1 \
    && ! command -v shasum >/dev/null 2>&1; then
    printf 'tree-manifest self-test requires sha256sum or shasum\n' >&2
    exit 1
fi
system_unzip=$(command -v unzip)
system_cmp=$(command -v cmp)
system_ln=$(command -v ln)
system_mkdir=$(command -v mkdir)
system_zip=$(command -v zip)
if command -v sha256sum >/dev/null 2>&1; then
    system_sha256_tool=$(command -v sha256sum)
    system_sha256_kind='sha256sum'
else
    system_sha256_tool=$(command -v shasum)
    system_sha256_kind='shasum'
fi

# Load the exact verifier functions under test without executing its release
# workflow. Function terminators are deliberately unindented in the verifier.
function_definitions=$(
    sed -n '/^sha256_file() {$/,/^}$/p' "$verifier"
    sed -n '/^preflight_source_zip_central_directory() {$/,/^}$/p' "$verifier"
    sed -n '/^validate_portable_relative_path() {$/,/^}$/p' "$verifier"
    sed -n '/^validate_relative_path() {$/,/^}$/p' "$verifier"
    sed -n '/^tree_manifest() {$/,/^}$/p' "$verifier"
    sed -n '/^verify_bundle_manifest() {$/,/^}$/p' "$verifier"
)
eval "$function_definitions"
for required_function in sha256_file preflight_source_zip_central_directory \
    validate_portable_relative_path validate_relative_path tree_manifest \
    verify_bundle_manifest; do
    type "$required_function" >/dev/null 2>&1 \
        || { printf 'could not load verifier function: %s\n' "$required_function" >&2; exit 1; }
done

readonly package_name='veritasm'
readonly package_version='0.4.0-dev.1'
readonly archive_root="${package_name}-${package_version}"
readonly source_archive_name="${archive_root}-source.zip"
readonly source_checksum_name="${source_archive_name}.sha256"
readonly max_source_zip_archive_bytes=67108864
readonly max_source_zip_members=4096
readonly max_source_zip_member_bytes=67108864
readonly max_source_zip_uncompressed_bytes=268435456
readonly max_source_zip_expansion_ratio=200

# Replace only the two central-directory listing modes used by the preflight.
# Any accidental decompression/integrity invocation through this function is a
# test failure. Synthetic listings make exact ratio boundaries reproducible;
# real Info-ZIP archives below exercise CR/LF filename rendering end to end.
preflight_fixture_mode=''
preflight_fixture_listing=''
preflight_fixture_inventory=''
preflight_forbidden_operation=0
unzip() {
    case "${1:-}:${2:-}:$#" in
        -Z:-l:3 | -Z1:*:2) ;;
        *)
            preflight_forbidden_operation=1
            printf 'preflight attempted a non-central-directory unzip operation: %s\n' \
                "$*" >&2
            return 97
            ;;
    esac
    if [[ "$preflight_fixture_mode" == 'synthetic' ]]; then
        case "$1" in
            -Z) cat "$preflight_fixture_listing" ;;
            -Z1) cat "$preflight_fixture_inventory" ;;
        esac
        return
    fi
    command unzip "$@"
}

write_preflight_fixture() {
    local uncompressed=$1
    local compressed=$2
    preflight_fixture_listing="$test_parent/preflight-fixture.listing"
    preflight_fixture_inventory="$test_parent/preflight-fixture.inventory"
    printf '%s\n' \
        'Archive:  synthetic.zip' \
        'Zip file size: 512 bytes, number of entries: 1' \
        "-rw-r--r--  3.0 unx $uncompressed t- $compressed defX 80-Jan-01 00:00 $archive_root/fixture.txt" \
        "1 file, $uncompressed bytes uncompressed, $compressed bytes compressed" \
        >"$preflight_fixture_listing"
    printf '%s/%s\n' "$archive_root" 'fixture.txt' \
        >"$preflight_fixture_inventory"
}

synthetic_archive="$test_parent/synthetic.zip"
: >"$synthetic_archive"
preflight_fixture_mode='synthetic'

# The inclusive 200:1 boundary is accepted, while the first integer ratio
# above it fails before any member payload operation.
write_preflight_fixture 200 1
preflight_source_zip_central_directory "$synthetic_archive" 512 \
    "$test_parent/ratio-boundary.listing" "$test_parent/ratio-boundary.inventory"
[[ "$source_zip_compressed_bytes:$source_zip_uncompressed_bytes" == '1:200' ]]

write_preflight_fixture 201 1
if preflight_source_zip_central_directory "$synthetic_archive" 512 \
    "$test_parent/ratio-over.listing" "$test_parent/ratio-over.inventory" \
    2>"$test_parent/ratio-over.stderr"; then
    printf 'source ZIP preflight accepted expansion above 200:1\n' >&2
    exit 1
fi
grep -q 'declared expansion 201/1 bytes is greater than 200:1' \
    "$test_parent/ratio-over.stderr"

# A nonempty declaration over zero aggregate compressed bytes is infinite,
# not a finite ratio with a synthetic one-byte denominator. Empty 0/0 input
# for a declared empty member remains well-defined and accepted.
write_preflight_fixture 1 0
if preflight_source_zip_central_directory "$synthetic_archive" 512 \
    "$test_parent/ratio-infinite.listing" "$test_parent/ratio-infinite.inventory" \
    2>"$test_parent/ratio-infinite.stderr"; then
    printf 'source ZIP preflight accepted an infinite declared expansion ratio\n' >&2
    exit 1
fi
grep -q 'zero compressed bytes (infinite expansion ratio)' \
    "$test_parent/ratio-infinite.stderr"

write_preflight_fixture 0 0
preflight_source_zip_central_directory "$synthetic_archive" 512 \
    "$test_parent/ratio-empty.listing" "$test_parent/ratio-empty.inventory"
[[ "$source_zip_compressed_bytes:$source_zip_uncompressed_bytes" == '0:0' ]]

preflight_fixture_mode=''
control_source="$test_parent/control-source"
mkdir -p -- "$control_source/$archive_root"
printf 'safe\n' >"$control_source/$archive_root/safe.txt"
(
    cd -- "$control_source"
    zip -q -X "$test_parent/safe.zip" "$archive_root/safe.txt"
)
safe_archive_bytes=$(wc -c <"$test_parent/safe.zip" | tr -d ' ')
preflight_source_zip_central_directory "$test_parent/safe.zip" "$safe_archive_bytes" \
    "$test_parent/safe.listing" "$test_parent/safe.inventory"

for control_case in lf cr; do
    case "$control_case" in
        lf) control_relative="$archive_root/line"$'\n''break.txt' ;;
        cr) control_relative="$archive_root/carriage"$'\r''return.txt' ;;
    esac
    printf 'control\n' >"$control_source/$control_relative"
    (
        cd -- "$control_source"
        zip -q -X "$test_parent/$control_case.zip" "$control_relative"
    )
    control_archive_bytes=$(wc -c <"$test_parent/$control_case.zip" | tr -d ' ')
    if preflight_source_zip_central_directory \
        "$test_parent/$control_case.zip" "$control_archive_bytes" \
        "$test_parent/$control_case.listing" "$test_parent/$control_case.inventory" \
        2>"$test_parent/$control_case.stderr"; then
        printf 'source ZIP preflight accepted a %s-bearing member name\n' \
            "$control_case" >&2
        exit 1
    fi
    grep -q 'newline-bearing member name' "$test_parent/$control_case.stderr"
done
[[ "$preflight_forbidden_operation" == 0 ]]

for bundle in msrv-se stable-se msrv-pe stable-pe; do
    mkdir -p "$test_parent/$bundle/schema"
    printf 'H\tVN:Z:1.1\n' >"$test_parent/$bundle/assembly.gfa"
    printf 'fixture checksum manifest\n' >"$test_parent/$bundle/manifest.sha256"
    printf '{"type":"object"}\n' >"$test_parent/$bundle/schema/manifest.json"
done

for bundle in msrv-se stable-se msrv-pe stable-pe; do
    tree_manifest "$test_parent/$bundle" \
        "$test_parent/$bundle-tree.txt" generated-output
    [[ "$(wc -l <"$test_parent/$bundle-tree.txt" | tr -d ' ')" == '3' ]]
    grep -q '  manifest\.sha256$' "$test_parent/$bundle-tree.txt"
done

cmp -s "$test_parent/msrv-se-tree.txt" "$test_parent/stable-se-tree.txt"
cmp -s "$test_parent/msrv-pe-tree.txt" "$test_parent/stable-pe-tree.txt"

# The stricter source profile must still reject packaged checksum artifacts,
# and a failed manifest must not leave a partial evidence file behind.
if tree_manifest "$test_parent/msrv-se" "$test_parent/rejected-tree.txt" source \
    2>"$test_parent/rejected-tree.stderr"; then
    printf 'source profile unexpectedly accepted manifest.sha256\n' >&2
    exit 1
fi
grep -q '^source tree contains an invalid relative path: manifest\.sha256$' \
    "$test_parent/rejected-tree.stderr"
if [[ -e "$test_parent/rejected-tree.txt" || -L "$test_parent/rejected-tree.txt" ]]; then
    printf 'failed tree manifest left a partial output file\n' >&2
    exit 1
fi

printf 'changed\n' >>"$test_parent/stable-pe/assembly.gfa"
tree_manifest "$test_parent/stable-pe" \
    "$test_parent/stable-pe-changed-tree.txt" generated-output
if cmp -s "$test_parent/msrv-pe-tree.txt" "$test_parent/stable-pe-changed-tree.txt"; then
    printf 'tree manifest did not expose a generated-output difference\n' >&2
    exit 1
fi

write_bundle_manifest() {
    local bundle=$1
    local relative digest
    (
        cd -- "$bundle"
        find . -type f -print | sed 's#^\./##' \
            | grep -v '^manifest\.sha256$' | sort
    ) | while IFS= read -r relative; do
        digest=$(sha256_file "$bundle/$relative")
        printf '%s  %s\n' "$digest" "$relative"
    done >"$bundle/manifest.sha256"
}

# Internal checksum consistency is necessary but not sufficient for a release
# smoke bundle: stable assembly-contract v0.1 has an exact artifact inventory.
contract_bundle="$test_parent/contract-bundle"
verification_root="$test_parent/verifier-state"
mkdir -- "$contract_bundle" "$verification_root"
for relative in \
    'assembly.gfa' \
    'pair_audit_summary.tsv' \
    'pair_links.tsv' \
    'report.html' \
    'run.json' \
    'schema/assembly_gfa.schema.json' \
    'schema/manifest.json' \
    'schema/pair_audit_summary.schema.json' \
    'schema/pair_links.schema.json' \
    'schema/run.schema.json' \
    'schema/transform_summary.schema.json' \
    'schema/unitig_evidence.schema.json' \
    'transform_summary.tsv' \
    'unitig_evidence.tsv' \
    'unitigs.fasta'; do
    mkdir -p -- "$(dirname -- "$contract_bundle/$relative")"
    printf 'fixture for %s\n' "$relative" >"$contract_bundle/$relative"
done
write_bundle_manifest "$contract_bundle"
verify_bundle_manifest "$contract_bundle"

printf 'internally checksummed but outside the stable contract\n' \
    >"$contract_bundle/unexpected.txt"
write_bundle_manifest "$contract_bundle"
if verify_bundle_manifest "$contract_bundle" \
    2>"$test_parent/unexpected-artifact.stderr"; then
    printf 'stable bundle contract accepted an unexpected checksummed artifact\n' >&2
    exit 1
fi
grep -q '^bundle inventory differs from stable assembly-contract v0\.1:$' \
    "$test_parent/unexpected-artifact.stderr"

# The source-directory allowlist is intentionally not a repository-root
# inventory. Exercise the packager end to end to prove that every root entry
# is typed and accounted for, including entries whose suffix would otherwise
# match the recursive exclusion policy.
source_packager="$script_directory/package_source.sh"

make_source_package_fixture() {
    local project=$1
    local relative
    mkdir -p -- \
        "$project/.git" \
        "$project/.github/workflows" \
        "$project/benchmark/development_matrix" \
        "$project/docs" \
        "$project/examples" \
        "$project/fuzz" \
        "$project/proptest-regressions/experimental" \
        "$project/schema" \
        "$project/scripts" \
        "$project/src" \
        "$project/target" \
        "$project/tests"

    for relative in \
        '.gitignore' \
        'AGENTS.md' \
        'ARCHITECTURE.md' \
        'BENCHMARK.md' \
        'CHANGELOG.md' \
        'CITATION.cff' \
        'CONTRIBUTING.md' \
        'Cargo.lock' \
        'LICENSE' \
        'README.md' \
        'RESEARCH.md' \
        'SECURITY.md' \
        'VALIDATION.md' \
        'deny.toml' \
        'rustfmt.toml'; do
        printf 'fixture for %s\n' "$relative" >"$project/$relative"
    done
    printf '%s\n' \
        '[package]' \
        'name = "veritasm"' \
        "version = \"$package_version\"" \
        >"$project/Cargo.toml"

    printf 'fixture\n' >"$project/.git/HEAD"
    printf 'ignored build output\n' >"$project/target/ignored.txt"
    printf 'fixture\n' >"$project/.github/workflows/fixture.yml"
    printf '#!/usr/bin/env bash\nprintf "fixture\\n"\n' \
        >"$project/benchmark/development_matrix/fixture.sh"
    chmod 0755 "$project/benchmark/development_matrix/fixture.sh"
    printf 'fixture\n' >"$project/benchmark/development_matrix/fixture.c"
    printf 'fixture\n' >"$project/docs/fixture.txt"
    printf 'fixture\n' >"$project/examples/fixture.txt"
    printf 'fixture\n' >"$project/fuzz/fixture.txt"
    printf 'fixture\n' >"$project/proptest-regressions/experimental/fixture.txt"
    printf 'fixture\n' >"$project/schema/fixture.txt"
    printf 'fixture\n' >"$project/src/fixture.rs"
    printf 'fixture\n' >"$project/tests/fixture.rs"
    cp -- "$source_packager" "$project/scripts/package_source.sh"
    chmod 0755 "$project/scripts/package_source.sh"
    printf '%s\n' \
        '.github/workflows/fixture.yml' \
        'benchmark/development_matrix/fixture.c' \
        'benchmark/development_matrix/fixture.sh' \
        'docs/fixture.txt' \
        'examples/fixture.txt' \
        'fuzz/fixture.txt' \
        'proptest-regressions/experimental/fixture.txt' \
        'schema/fixture.txt' \
        'scripts/package_source.sh' \
        'scripts/source-package-files.txt' \
        'src/fixture.rs' \
        'tests/fixture.rs' \
        >"$project/scripts/source-package-files.txt"
}

assert_failed_package_cleanup() {
    local project=$1
    local expected_private_state=${2:-}
    local observed_private_state
    if [[ -e "$project/$source_archive_name" \
        || -L "$project/$source_archive_name" \
        || -e "$project/$source_checksum_name" \
        || -L "$project/$source_checksum_name" ]]; then
        printf 'failed source packaging published output in %s\n' \
            "${project##*/}" >&2
        exit 1
    fi
    if [[ -e "$project/.veritasm-source-package.lock" \
        || -L "$project/.veritasm-source-package.lock" ]]; then
        printf 'failed source packaging left its lock in %s\n' \
            "${project##*/}" >&2
        exit 1
    fi
    observed_private_state=$(
        while IFS= read -r -d '' private_path; do
            printf '%s\n' "${private_path##*/}"
        done < <(find "$project" -mindepth 1 -maxdepth 1 \
            -name '.veritasm-package.*' -print0) | sort
    )
    if [[ "$observed_private_state" != "$expected_private_state" ]]; then
        printf 'failed source packaging left unexpected private state in %s: %s\n' \
            "${project##*/}" "$observed_private_state" >&2
        exit 1
    fi
}

assert_root_rejection() {
    local project=$1
    local expected=$2
    local expected_private_state=${3:-}
    local case_name=${project##*/}
    local stdout_log="$test_parent/$case_name.stdout"
    local stderr_log="$test_parent/$case_name.stderr"
    if (
        cd -- "$project"
        ./scripts/package_source.sh
    ) >"$stdout_log" 2>"$stderr_log"; then
        printf 'source packager accepted an unexpected root entry in %s\n' \
            "${project##*/}" >&2
        exit 1
    fi
    grep -Fq "$expected" "$stderr_log"
    assert_failed_package_cleanup "$project" "$expected_private_state"
}

package_control="$test_parent/package-control"
make_source_package_fixture "$package_control"
(
    cd -- "$package_control"
    ./scripts/package_source.sh
) >"$test_parent/package-control.stdout" 2>"$test_parent/package-control.stderr"
[[ -f "$package_control/$source_archive_name" ]]
[[ -f "$package_control/$source_checksum_name" ]]
unzip -Z1 "$package_control/$source_archive_name" \
    >"$package_control/archive.inventory"
grep -q "^$archive_root/docs/fixture\.txt$" \
    "$package_control/archive.inventory"
benchmark_shell_mode=$(
    "$system_unzip" -Z -l "$package_control/$source_archive_name" \
        "$archive_root/benchmark/development_matrix/fixture.sh" \
        | awk '$1 ~ /^-/ { print $1; exit }'
)
benchmark_source_mode=$(
    "$system_unzip" -Z -l "$package_control/$source_archive_name" \
        "$archive_root/benchmark/development_matrix/fixture.c" \
        | awk '$1 ~ /^-/ { print $1; exit }'
)
if [[ "$benchmark_shell_mode" != '-rwxr-xr-x' ]]; then
    printf 'source package did not preserve normalized benchmark shell mode: %s\n' \
        "$benchmark_shell_mode" >&2
    exit 1
fi
if [[ "$benchmark_source_mode" != '-rw-r--r--' ]]; then
    printf 'source package gave a non-shell benchmark source an unexpected mode: %s\n' \
        "$benchmark_source_mode" >&2
    exit 1
fi
if grep -Eq '/(\.git|target)(/|$)' "$package_control/archive.inventory"; then
    printf 'source package included repository metadata or build output\n' >&2
    exit 1
fi

# Darwin opens /dev/fd/N by duplicating N, so a second reader of the same
# descriptor inherits its consumed offset. Record all descriptor-backed content
# reads and require one independently opened descriptor per operation while the
# identity anchors remain untouched.
descriptor_project="$test_parent/package-descriptor-readers"
descriptor_shim="$test_parent/package-descriptor-readers-bin"
descriptor_log="$test_parent/package-descriptor-readers.log"
make_source_package_fixture "$descriptor_project"
mkdir -- "$descriptor_shim"
: >"$descriptor_log"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'for argument in "$@"; do' \
    '    case "$argument" in' \
    '        /dev/fd/*) printf "%s\n" "$argument" >>"$DESCRIPTOR_LOG" ;;' \
    '    esac' \
    'done' \
    'exec "$REAL_CMP" "$@"' \
    >"$descriptor_shim/cmp"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'for argument in "$@"; do' \
    '    case "$argument" in' \
    '        /dev/fd/*) printf "%s\n" "$argument" >>"$DESCRIPTOR_LOG" ;;' \
    '    esac' \
    'done' \
    'if [[ "$REAL_SHA256_KIND" == "sha256sum" ]]; then' \
    '    exec "$REAL_SHA256_TOOL" "$@"' \
    'fi' \
    'exec "$REAL_SHA256_TOOL" -a 256 "$@"' \
    >"$descriptor_shim/sha256sum"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'for argument in "$@"; do' \
    '    case "$argument" in' \
    '        /dev/fd/*) printf "%s\n" "$argument" >>"$DESCRIPTOR_LOG" ;;' \
    '    esac' \
    'done' \
    'exec "$REAL_UNZIP" "$@"' \
    >"$descriptor_shim/unzip"
chmod 0755 \
    "$descriptor_shim/cmp" \
    "$descriptor_shim/sha256sum" \
    "$descriptor_shim/unzip"
(
    cd -- "$descriptor_project"
    PATH="$descriptor_shim:$PATH" \
        DESCRIPTOR_LOG="$descriptor_log" \
        REAL_CMP="$system_cmp" \
        REAL_SHA256_KIND="$system_sha256_kind" \
        REAL_SHA256_TOOL="$system_sha256_tool" \
        REAL_UNZIP="$system_unzip" \
        ./scripts/package_source.sh
) >"$test_parent/package-descriptor-readers.stdout" \
    2>"$test_parent/package-descriptor-readers.stderr"
descriptor_read_count=$(awk 'END { print NR }' "$descriptor_log")
descriptor_unique_count=$(sort -u "$descriptor_log" | awk 'END { print NR }')
if [[ "$descriptor_read_count" != 8 || "$descriptor_unique_count" != 8 ]]; then
    printf 'source packager reused a descriptor-backed content reader: %s reads, %s unique\n' \
        "$descriptor_read_count" "$descriptor_unique_count" >&2
    exit 1
fi
if grep -Eq '^/dev/fd/(7|8|9)$' "$descriptor_log"; then
    printf 'source packager consumed an identity or ownership descriptor\n' >&2
    exit 1
fi

worktree_control="$test_parent/package-worktree-control"
make_source_package_fixture "$worktree_control"
unlink -- "$worktree_control/.git/HEAD"
rmdir -- "$worktree_control/.git"
printf 'gitdir: /nonexistent/fixture\n' >"$worktree_control/.git"
(
    cd -- "$worktree_control"
    ./scripts/package_source.sh
) >"$test_parent/package-worktree-control.stdout" \
    2>"$test_parent/package-worktree-control.stderr"
[[ -f "$worktree_control/$source_archive_name" ]]
[[ -f "$worktree_control/$source_checksum_name" ]]

# Exclusion policy is a release gate, not an implicit filter. A source-looking
# regular file and a pruned directory must both stop packaging and remain in
# place for review.
excluded_regular_project="$test_parent/package-excluded-regular"
make_source_package_fixture "$excluded_regular_project"
printf 'compiled-looking excluded source\n' \
    >"$excluded_regular_project/src/secrets.rs"
if (
    cd -- "$excluded_regular_project"
    ./scripts/package_source.sh
) >"$test_parent/package-excluded-regular.stdout" \
    2>"$test_parent/package-excluded-regular.stderr"; then
    printf 'source packager silently omitted an excluded regular file\n' >&2
    exit 1
fi
grep -Fq 'error: governed source tree contains an excluded path: src/secrets.rs' \
    "$test_parent/package-excluded-regular.stderr"
[[ -f "$excluded_regular_project/src/secrets.rs" ]]
assert_failed_package_cleanup "$excluded_regular_project"

excluded_directory_project="$test_parent/package-excluded-directory"
make_source_package_fixture "$excluded_directory_project"
mkdir -- "$excluded_directory_project/docs/tmp"
if (
    cd -- "$excluded_directory_project"
    ./scripts/package_source.sh
) >"$test_parent/package-excluded-directory.stdout" \
    2>"$test_parent/package-excluded-directory.stderr"; then
    printf 'source packager silently pruned an excluded directory\n' >&2
    exit 1
fi
grep -Fq 'error: governed source tree contains an excluded path: docs/tmp' \
    "$test_parent/package-excluded-directory.stderr"
[[ -d "$excluded_directory_project/docs/tmp" ]]
assert_failed_package_cleanup "$excluded_directory_project"

uppercase_excluded_project="$test_parent/package-uppercase-excluded-directory"
make_source_package_fixture "$uppercase_excluded_project"
mkdir -- "$uppercase_excluded_project/docs/TMP"
if (
    cd -- "$uppercase_excluded_project"
    ./scripts/package_source.sh
) >"$test_parent/package-uppercase-excluded-directory.stdout" \
    2>"$test_parent/package-uppercase-excluded-directory.stderr"; then
    printf 'source packager accepted a case-variant excluded directory\n' >&2
    exit 1
fi
grep -Fq 'error: governed source tree contains an excluded path: docs/TMP' \
    "$test_parent/package-uppercase-excluded-directory.stderr"
[[ -d "$uppercase_excluded_project/docs/TMP" ]]
assert_failed_package_cleanup "$uppercase_excluded_project"

space_path_project="$test_parent/package-space-path"
make_source_package_fixture "$space_path_project"
printf 'non-portable source path\n' >"$space_path_project/docs/design notes.md"
printf '%s\n' 'docs/design notes.md' \
    >>"$space_path_project/scripts/source-package-files.txt"
sort -u -o "$space_path_project/scripts/source-package-files.txt" \
    "$space_path_project/scripts/source-package-files.txt"
if (
    cd -- "$space_path_project"
    ./scripts/package_source.sh
) >"$test_parent/package-space-path.stdout" \
    2>"$test_parent/package-space-path.stderr"; then
    printf 'source packager accepted a space-bearing source path\n' >&2
    exit 1
fi
grep -Fq 'error: unsafe source path:' "$test_parent/package-space-path.stderr"
[[ -f "$space_path_project/docs/design notes.md" ]]
assert_failed_package_cleanup "$space_path_project"

unicode_path_project="$test_parent/package-unicode-path"
make_source_package_fixture "$unicode_path_project"
unicode_relative=$(printf 'docs/nonascii-\303\251.md')
printf 'non-portable source path\n' >"$unicode_path_project/$unicode_relative"
printf '%s\n' "$unicode_relative" \
    >>"$unicode_path_project/scripts/source-package-files.txt"
sort -u -o "$unicode_path_project/scripts/source-package-files.txt" \
    "$unicode_path_project/scripts/source-package-files.txt"
if (
    cd -- "$unicode_path_project"
    ./scripts/package_source.sh
) >"$test_parent/package-unicode-path.stdout" \
    2>"$test_parent/package-unicode-path.stderr"; then
    printf 'source packager accepted a non-ASCII source path\n' >&2
    exit 1
fi
grep -Fq 'error: unsafe source path:' "$test_parent/package-unicode-path.stderr"
[[ -f "$unicode_path_project/$unicode_relative" ]]
assert_failed_package_cleanup "$unicode_path_project"

casefold_project="$test_parent/package-casefold-collision"
make_source_package_fixture "$casefold_project"
printf '%s\n' 'docs/Fixture.txt' \
    >>"$casefold_project/scripts/source-package-files.txt"
sort -u -o "$casefold_project/scripts/source-package-files.txt" \
    "$casefold_project/scripts/source-package-files.txt"
if (
    cd -- "$casefold_project"
    ./scripts/package_source.sh
) >"$test_parent/package-casefold-collision.stdout" \
    2>"$test_parent/package-casefold-collision.stderr"; then
    printf 'source packager accepted case-fold-colliding source paths\n' >&2
    exit 1
fi
grep -Fq 'error: source-package allowlist contains case-fold-colliding paths:' \
    "$test_parent/package-casefold-collision.stderr"
assert_failed_package_cleanup "$casefold_project"

# Mutate an allowlisted source file after ZIP creation. The archive itself is
# internally consistent, so only the packager's final live-source attestation
# can detect that it no longer represents one stable producing snapshot.
source_mutation_project="$test_parent/package-source-mutation"
source_mutation_shim="$test_parent/package-source-mutation-bin"
make_source_package_fixture "$source_mutation_project"
mkdir -- "$source_mutation_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    '"$REAL_ZIP" "$@"' \
    'status=$?' \
    'if ((status == 0)); then' \
    '    printf "mutated during packaging\\n" >>"$INJECT_SOURCE"' \
    'fi' \
    'exit "$status"' \
    >"$source_mutation_shim/zip"
chmod 0755 "$source_mutation_shim/zip"
if (
    cd -- "$source_mutation_project"
    PATH="$source_mutation_shim:$PATH" \
        REAL_ZIP="$system_zip" \
        INJECT_SOURCE="$source_mutation_project/docs/fixture.txt" \
        ./scripts/package_source.sh
) >"$test_parent/package-source-mutation.stdout" \
    2>"$test_parent/package-source-mutation.stderr"; then
    printf 'source packager accepted a live source mutation during packaging\n' >&2
    exit 1
fi
grep -Fq 'error: live source bytes changed while packaging' \
    "$test_parent/package-source-mutation.stderr"
grep -Fq 'mutated during packaging' "$source_mutation_project/docs/fixture.txt"
assert_failed_package_cleanup "$source_mutation_project"

source_inventory_project="$test_parent/package-source-inventory-mutation"
source_inventory_shim="$test_parent/package-source-inventory-mutation-bin"
make_source_package_fixture "$source_inventory_project"
mkdir -- "$source_inventory_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    '"$REAL_ZIP" "$@"' \
    'status=$?' \
    'if ((status == 0)); then' \
    '    printf "introduced during packaging\\n" >"$INJECT_SOURCE"' \
    'fi' \
    'exit "$status"' \
    >"$source_inventory_shim/zip"
chmod 0755 "$source_inventory_shim/zip"
if (
    cd -- "$source_inventory_project"
    PATH="$source_inventory_shim:$PATH" \
        REAL_ZIP="$system_zip" \
        INJECT_SOURCE="$source_inventory_project/docs/introduced.txt" \
        ./scripts/package_source.sh
) >"$test_parent/package-source-inventory-mutation.stdout" \
    2>"$test_parent/package-source-inventory-mutation.stderr"; then
    printf 'source packager accepted a nested source path introduced during packaging\n' >&2
    exit 1
fi
grep -Fq 'error: source directory inventory changed while packaging' \
    "$test_parent/package-source-inventory-mutation.stderr"
[[ -f "$source_inventory_project/docs/introduced.txt" ]]
assert_failed_package_cleanup "$source_inventory_project"

postpublication_mutation_project="$test_parent/package-postpublication-source-mutation"
postpublication_mutation_shim="$test_parent/package-postpublication-source-mutation-bin"
make_source_package_fixture "$postpublication_mutation_project"
mkdir -- "$postpublication_mutation_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'if [[ "${1:-}" == "-tq" && -f "$INJECT_ARCHIVE" ]]; then' \
    '    printf "mutated after publication\\n" >>"$INJECT_SOURCE"' \
    'fi' \
    'exec "$REAL_UNZIP" "$@"' \
    >"$postpublication_mutation_shim/unzip"
chmod 0755 "$postpublication_mutation_shim/unzip"
if (
    cd -- "$postpublication_mutation_project"
    PATH="$postpublication_mutation_shim:$PATH" \
        REAL_UNZIP="$system_unzip" \
        INJECT_ARCHIVE="$postpublication_mutation_project/$source_archive_name" \
        INJECT_SOURCE="$postpublication_mutation_project/docs/fixture.txt" \
        ./scripts/package_source.sh
) >"$test_parent/package-postpublication-source-mutation.stdout" \
    2>"$test_parent/package-postpublication-source-mutation.stderr"; then
    printf 'source packager accepted a source mutation after publication\n' >&2
    exit 1
fi
grep -Fq 'error: live source bytes changed before packaging completed' \
    "$test_parent/package-postpublication-source-mutation.stderr"
grep -Fq 'mutated after publication' \
    "$postpublication_mutation_project/docs/fixture.txt"
assert_failed_package_cleanup "$postpublication_mutation_project"

package_regular="$test_parent/package-unlisted-regular"
make_source_package_fixture "$package_regular"
printf 'not allowlisted\n' >"$package_regular/UNLISTED.md"
assert_root_rejection "$package_regular" \
    'error: unexpected repository-root entry would be omitted: UNLISTED.md'

package_cargo_symlink="$test_parent/package-cargo-symlink"
make_source_package_fixture "$package_cargo_symlink"
unlink -- "$package_cargo_symlink/Cargo.toml"
ln -s README.md "$package_cargo_symlink/Cargo.toml"
assert_root_rejection "$package_cargo_symlink" \
    'error: required source file is missing or not regular: Cargo.toml'

package_cargo_fifo="$test_parent/package-cargo-fifo"
make_source_package_fixture "$package_cargo_fifo"
unlink -- "$package_cargo_fifo/Cargo.toml"
mkfifo "$package_cargo_fifo/Cargo.toml"
assert_root_rejection "$package_cargo_fifo" \
    'error: required source file is missing or not regular: Cargo.toml'

package_archive="$test_parent/package-unrelated-archive"
make_source_package_fixture "$package_archive"
printf 'not a generated source package\n' >"$package_archive/unrelated.zip"
assert_root_rejection "$package_archive" \
    'error: unexpected repository-root entry would be omitted: unrelated.zip'

package_symlink="$test_parent/package-unlisted-symlink"
make_source_package_fixture "$package_symlink"
ln -s README.md "$package_symlink/unlisted-link"
assert_root_rejection "$package_symlink" \
    'error: unexpected repository-root entry would be omitted: unlisted-link'

package_target_symlink="$test_parent/package-target-symlink"
make_source_package_fixture "$package_target_symlink"
unlink -- "$package_target_symlink/target/ignored.txt"
rmdir -- "$package_target_symlink/target"
ln -s src "$package_target_symlink/target"
assert_root_rejection "$package_target_symlink" \
    'error: generated build path is not a regular directory: target'

package_fifo="$test_parent/package-unlisted-fifo"
make_source_package_fixture "$package_fifo"
mkfifo "$package_fifo/unlisted.pipe"
assert_root_rejection "$package_fifo" \
    'error: unexpected repository-root entry would be omitted: unlisted.pipe'

package_directory="$test_parent/package-unlisted-directory"
make_source_package_fixture "$package_directory"
mkdir -- "$package_directory/unlisted-directory"
assert_root_rejection "$package_directory" \
    'error: unexpected repository-root entry would be omitted: unlisted-directory'

package_stale_temp="$test_parent/package-stale-temp"
make_source_package_fixture "$package_stale_temp"
mkdir -- "$package_stale_temp/.veritasm-package.stale"
assert_root_rejection "$package_stale_temp" \
    'error: source-package temporary path is not owned by this packaging run: .veritasm-package.stale' \
    '.veritasm-package.stale'

# Inject an unlisted root file only when the published archive reaches its
# integrity check. This proves the final root scan runs and that its failure
# rolls both hard-linked outputs back without deleting the foreign entry.
late_project="$test_parent/package-late-entry"
late_shim="$test_parent/package-late-entry-bin"
make_source_package_fixture "$late_project"
mkdir -- "$late_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'if [[ "${1:-}" == "-tq" && -f "$INJECT_ARCHIVE" ]]; then' \
    '    printf "introduced during packaging\\n" >"$INJECT_PROJECT/LATE.md"' \
    'fi' \
    'exec "$REAL_UNZIP" "$@"' \
    >"$late_shim/unzip"
chmod 0755 "$late_shim/unzip"
if (
    cd -- "$late_project"
    PATH="$late_shim:$PATH" \
        INJECT_ARCHIVE="$late_project/$source_archive_name" \
        INJECT_PROJECT="$late_project" \
        REAL_UNZIP="$system_unzip" \
        ./scripts/package_source.sh
) >"$test_parent/package-late-entry.stdout" \
    2>"$test_parent/package-late-entry.stderr"; then
    printf 'source packager accepted a root entry introduced after publication\n' >&2
    exit 1
fi
grep -Fq 'error: unexpected repository-root entry would be omitted: LATE.md' \
    "$test_parent/package-late-entry.stderr"
[[ -f "$late_project/LATE.md" ]]
assert_failed_package_cleanup "$late_project"

# Signal the parent from mkdir(1) after the public lock directory exists but
# before the command returns. The acquisition critical section ignores that
# catchable signal until the held ownership sentinel is established.
acquisition_project="$test_parent/package-lock-acquisition-signal"
acquisition_shim="$test_parent/package-lock-acquisition-signal-bin"
make_source_package_fixture "$acquisition_project"
mkdir -- "$acquisition_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'destination=${!#}' \
    '"$REAL_MKDIR" "$@"' \
    'if [[ "$destination" == "$SIGNAL_LOCK" ]]; then' \
    '    kill -TERM "$PPID"' \
    'fi' \
    >"$acquisition_shim/mkdir"
chmod 0755 "$acquisition_shim/mkdir"
(
    cd -- "$acquisition_project"
    PATH="$acquisition_shim:$PATH" \
        REAL_MKDIR="$system_mkdir" \
        SIGNAL_LOCK="$acquisition_project/.veritasm-source-package.lock" \
        ./scripts/package_source.sh
) >"$test_parent/package-lock-acquisition-signal.stdout" \
    2>"$test_parent/package-lock-acquisition-signal.stderr"
[[ -f "$acquisition_project/$source_archive_name" ]]
[[ -f "$acquisition_project/$source_checksum_name" ]]
[[ ! -e "$acquisition_project/.veritasm-source-package.lock" ]]
[[ -z "$(find "$acquisition_project" -mindepth 1 -maxdepth 1 \
    -name '.veritasm-package.*' -print -quit)" ]]

# Conversely, mkdir failure means the public lock was never acquired. Its
# contents are foreign and must survive the failed packaging attempt.
foreign_lock_project="$test_parent/package-foreign-lock"
make_source_package_fixture "$foreign_lock_project"
mkdir -- "$foreign_lock_project/.veritasm-source-package.lock"
printf 'foreign lock content\n' \
    >"$foreign_lock_project/.veritasm-source-package.lock/foreign.txt"
if (
    cd -- "$foreign_lock_project"
    ./scripts/package_source.sh
) >"$test_parent/package-foreign-lock.stdout" \
    2>"$test_parent/package-foreign-lock.stderr"; then
    printf 'source packager accepted a pre-existing foreign lock\n' >&2
    exit 1
fi
grep -Fq 'error: source packaging is locked:' \
    "$test_parent/package-foreign-lock.stderr"
grep -Fxq 'foreign lock content' \
    "$foreign_lock_project/.veritasm-source-package.lock/foreign.txt"
[[ ! -e "$foreign_lock_project/$source_archive_name" ]]
[[ ! -e "$foreign_lock_project/$source_checksum_name" ]]
[[ -z "$(find "$foreign_lock_project" -mindepth 1 -maxdepth 1 \
    -name '.veritasm-package.*' -print -quit)" ]]

# Send SIGTERM after ln(1) has created the archive hard link but before the
# parent shell can execute its publication-flag assignment. Cleanup must use
# the held archive descriptor and remove the otherwise orphaned partial file.
signal_project="$test_parent/package-publication-signal"
signal_shim="$test_parent/package-publication-signal-bin"
make_source_package_fixture "$signal_project"
mkdir -- "$signal_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'destination=${!#}' \
    '"$REAL_LN" "$@"' \
    'if [[ "$destination" == "$SIGNAL_ARCHIVE" ]]; then' \
    '    kill -TERM "$PPID"' \
    'fi' \
    >"$signal_shim/ln"
chmod 0755 "$signal_shim/ln"
if (
    cd -- "$signal_project"
    PATH="$signal_shim:$PATH" \
        REAL_LN="$system_ln" \
        SIGNAL_ARCHIVE="$signal_project/$source_archive_name" \
        ./scripts/package_source.sh
) >"$test_parent/package-publication-signal.stdout" \
    2>"$test_parent/package-publication-signal.stderr"; then
    printf 'source packager ignored SIGTERM during archive publication\n' >&2
    exit 1
fi
assert_failed_package_cleanup "$signal_project"

# Replace the public archive name inside the successful hard-link operation.
# Ownership is anchored to the already-open private archive, so neither the
# validation failure nor rollback may unlink the foreign replacement.
publication_project="$test_parent/package-publication-replacement"
publication_shim="$test_parent/package-publication-replacement-bin"
make_source_package_fixture "$publication_project"
mkdir -- "$publication_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'destination=${!#}' \
    '"$REAL_LN" "$@"' \
    'if [[ "$destination" == "$INJECT_ARCHIVE" ]]; then' \
    '    unlink -- "$destination"' \
    '    printf "foreign public archive\\n" >"$destination"' \
    'fi' \
    >"$publication_shim/ln"
chmod 0755 "$publication_shim/ln"
if (
    cd -- "$publication_project"
    PATH="$publication_shim:$PATH" \
        INJECT_ARCHIVE="$publication_project/$source_archive_name" \
        REAL_LN="$system_ln" \
        ./scripts/package_source.sh
) >"$test_parent/package-publication-replacement.stdout" \
    2>"$test_parent/package-publication-replacement.stderr"; then
    printf 'source packager accepted a replaced public archive\n' >&2
    exit 1
fi
grep -Fq 'error: published source archive was replaced before ownership validation' \
    "$test_parent/package-publication-replacement.stderr"
grep -Fq 'error: refusing to roll back replaced source archive:' \
    "$test_parent/package-publication-replacement.stderr"
grep -Fxq 'foreign public archive' \
    "$publication_project/$source_archive_name"
[[ ! -e "$publication_project/$source_checksum_name" ]]
[[ ! -e "$publication_project/.veritasm-source-package.lock" ]]
[[ -z "$(find "$publication_project" -mindepth 1 -maxdepth 1 \
    -name '.veritasm-package.*' -print -quit)" ]]

# Replace both private directories after publication. Cleanup must roll back
# outputs using their recorded identities while preserving the replacement
# namespaces and their foreign contents.
replacement_project="$test_parent/package-private-replacement"
replacement_shim="$test_parent/package-private-replacement-bin"
make_source_package_fixture "$replacement_project"
mkdir -- "$replacement_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'if [[ "${1:-}" == "-tq" && -f "$INJECT_ARCHIVE" ]]; then' \
    '    owned_temp=$(find "$INJECT_PROJECT" -mindepth 1 -maxdepth 1 -type d -name ".veritasm-package.*" -print -quit)' \
    '    find "$owned_temp" -depth -mindepth 1 -delete' \
    '    rmdir -- "$owned_temp"' \
    '    mkdir -- "$owned_temp"' \
    '    printf "foreign temporary content\\n" >"$owned_temp/foreign.txt"' \
    '    find "$INJECT_PROJECT/.veritasm-source-package.lock" -depth -mindepth 1 -delete' \
    '    rmdir -- "$INJECT_PROJECT/.veritasm-source-package.lock"' \
    '    mkdir -- "$INJECT_PROJECT/.veritasm-source-package.lock"' \
    '    printf "foreign lock content\\n" >"$INJECT_PROJECT/.veritasm-source-package.lock/foreign.txt"' \
    'fi' \
    'exec "$REAL_UNZIP" "$@"' \
    >"$replacement_shim/unzip"
chmod 0755 "$replacement_shim/unzip"
if (
    cd -- "$replacement_project"
    PATH="$replacement_shim:$PATH" \
        INJECT_ARCHIVE="$replacement_project/$source_archive_name" \
        INJECT_PROJECT="$replacement_project" \
        REAL_UNZIP="$system_unzip" \
        ./scripts/package_source.sh
) >"$test_parent/package-private-replacement.stdout" \
    2>"$test_parent/package-private-replacement.stderr"; then
    printf 'source packager accepted replaced private directories\n' >&2
    exit 1
fi
grep -Fq 'error: refusing to clean replaced source-package temporary directory:' \
    "$test_parent/package-private-replacement.stderr"
grep -Fq 'error: refusing to clean replaced source-package lock directory:' \
    "$test_parent/package-private-replacement.stderr"
[[ ! -e "$replacement_project/$source_archive_name" ]]
[[ ! -e "$replacement_project/$source_checksum_name" ]]
[[ -f "$replacement_project/.veritasm-source-package.lock/foreign.txt" ]]
replacement_temp=$(find "$replacement_project" -mindepth 1 -maxdepth 1 \
    -type d -name '.veritasm-package.*' -print -quit)
[[ -n "$replacement_temp" && -f "$replacement_temp/foreign.txt" ]]

# Corrupt the published sidecar during anchored archive validation. The
# descriptor-backed content check must reject the same-inode modification.
sidecar_project="$test_parent/package-sidecar-corruption"
sidecar_shim="$test_parent/package-sidecar-corruption-bin"
make_source_package_fixture "$sidecar_project"
mkdir -- "$sidecar_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'if [[ "${1:-}" == /dev/fd/* && -f "$INJECT_SIDECAR" ]]; then' \
    '    printf "corrupted sidecar\\n" >"$INJECT_SIDECAR"' \
    'fi' \
    'if [[ "$REAL_SHA256_KIND" == "sha256sum" ]]; then' \
    '    exec "$REAL_SHA256_TOOL" "$@"' \
    'fi' \
    'exec "$REAL_SHA256_TOOL" -a 256 "$@"' \
    >"$sidecar_shim/sha256sum"
chmod 0755 "$sidecar_shim/sha256sum"
if (
    cd -- "$sidecar_project"
    PATH="$sidecar_shim:$PATH" \
        INJECT_ARCHIVE="$sidecar_project/$source_archive_name" \
        INJECT_SIDECAR="$sidecar_project/$source_checksum_name" \
        REAL_SHA256_KIND="$system_sha256_kind" \
        REAL_SHA256_TOOL="$system_sha256_tool" \
        ./scripts/package_source.sh
) >"$test_parent/package-sidecar-corruption.stdout" \
    2>"$test_parent/package-sidecar-corruption.stderr"; then
    printf 'source packager accepted a corrupted published checksum sidecar\n' >&2
    exit 1
fi
grep -Fq 'error: anchored source checksum content differs during post-publication validation' \
    "$test_parent/package-sidecar-corruption.stderr"
assert_failed_package_cleanup "$sidecar_project"

# Replace the public archive while the final live-source manifest is being
# hashed. This is after the first output validation and was the last window in
# which a foreign public name could otherwise survive through successful
# completion. The anchored output must remain private cleanup authority.
final_rehash_project="$test_parent/package-final-rehash-replacement"
final_rehash_shim="$test_parent/package-final-rehash-replacement-bin"
final_rehash_marker="$test_parent/package-final-rehash-replacement.marker"
make_source_package_fixture "$final_rehash_project"
mkdir -- "$final_rehash_shim"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'if [[ "${1:-}" == "$INJECT_SOURCE"' \
    '    && -f "$INJECT_ARCHIVE" && -f "$INJECT_SIDECAR"' \
    '    && ! -e "$INJECT_MARKER" ]]; then' \
    '    unlink -- "$INJECT_ARCHIVE"' \
    '    printf "foreign public archive\\n" >"$INJECT_ARCHIVE"' \
    '    : >"$INJECT_MARKER"' \
    'fi' \
    'if [[ "$REAL_SHA256_KIND" == "sha256sum" ]]; then' \
    '    exec "$REAL_SHA256_TOOL" "$@"' \
    'fi' \
    'exec "$REAL_SHA256_TOOL" -a 256 "$@"' \
    >"$final_rehash_shim/sha256sum"
chmod 0755 "$final_rehash_shim/sha256sum"
if (
    cd -- "$final_rehash_project"
    PATH="$final_rehash_shim:$PATH" \
        INJECT_ARCHIVE="$final_rehash_project/$source_archive_name" \
        INJECT_MARKER="$final_rehash_marker" \
        INJECT_SIDECAR="$final_rehash_project/$source_checksum_name" \
        INJECT_SOURCE="$final_rehash_project/docs/fixture.txt" \
        REAL_SHA256_KIND="$system_sha256_kind" \
        REAL_SHA256_TOOL="$system_sha256_tool" \
        ./scripts/package_source.sh
) >"$test_parent/package-final-rehash-replacement.stdout" \
    2>"$test_parent/package-final-rehash-replacement.stderr"; then
    printf 'source packager accepted a replacement during its final source rehash\n' >&2
    exit 1
fi
grep -Fq 'error: published source archive was replaced before final completion validation' \
    "$test_parent/package-final-rehash-replacement.stderr"
grep -Fq 'error: refusing to roll back replaced source archive:' \
    "$test_parent/package-final-rehash-replacement.stderr"
grep -Fxq 'foreign public archive' \
    "$final_rehash_project/$source_archive_name"
[[ -f "$final_rehash_marker" ]]
[[ ! -e "$final_rehash_project/$source_checksum_name" ]]
[[ ! -e "$final_rehash_project/.veritasm-source-package.lock" ]]
[[ -z "$(find "$final_rehash_project" -mindepth 1 -maxdepth 1 \
    -name '.veritasm-package.*' -print -quit)" ]]

printf 'PASS: ZIP preflight, generated-tree, bundle-inventory, and source-root checks passed\n'
