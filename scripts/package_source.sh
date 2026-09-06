#!/usr/bin/env bash
# Build and validate the deterministic VeritAsm 0.4.0-dev.1 source review archive.

set -euo pipefail
IFS=$'\n\t'
export LC_ALL=C
export TZ=UTC
umask 077

# Info-ZIP reads ZIPOPT (and some builds also honor ZIP) from the caller's
# environment. Archive policy is defined below, so ambient options must not
# add metadata, recursion, encryption, or another output mode.
unset ZIP ZIPOPT

readonly package_name='veritasm'
readonly package_version='0.4.0-dev.1'
readonly archive_root="${package_name}-${package_version}"
readonly archive_name="${archive_root}-source.zip"
readonly checksum_name="${archive_name}.sha256"
readonly normalized_timestamp='198001010000'

if (($# != 0)); then
    printf 'usage: %s\n' "${0##*/}" >&2
    exit 2
fi

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repository_root=$(CDPATH= cd -- "$script_directory/.." && pwd -P)
archive_output="$repository_root/$archive_name"
checksum_output="$repository_root/$checksum_name"
lock_directory="$repository_root/.veritasm-source-package.lock"
source_allowlist="$repository_root/scripts/source-package-files.txt"

for required_tool in awk chmod cmp cp diff dirname find grep ln mkdir mktemp rmdir sort touch tr unlink unzip zip; do
    if ! command -v "$required_tool" >/dev/null 2>&1; then
        printf 'error: required packaging tool is unavailable: %s\n' "$required_tool" >&2
        exit 1
    fi
done
if ! command -v sha256sum >/dev/null 2>&1 \
    && ! command -v shasum >/dev/null 2>&1; then
    printf 'error: neither sha256sum nor shasum is available\n' >&2
    exit 1
fi

sha256_file() {
    local file=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{ print $1 }'
    else
        shasum -a 256 "$file" | awk '{ print $1 }'
    fi
}

if [[ ! -f "$repository_root/Cargo.toml" \
    || -L "$repository_root/Cargo.toml" ]]; then
    printf 'error: required source file is missing or not regular: Cargo.toml\n' >&2
    exit 1
fi
manifest_version=$(awk '
    /^\[package\]$/ { in_package = 1; next }
    /^\[/ { in_package = 0 }
    in_package && /^version[[:space:]]*=/ {
        value = $0
        sub(/^[^=]*=[[:space:]]*"/, "", value)
        sub(/"[[:space:]]*$/, "", value)
        print value
        exit
    }
' "$repository_root/Cargo.toml")
if [[ "$manifest_version" != "$package_version" ]]; then
    printf 'error: Cargo.toml package version is %q, expected %q\n' \
        "$manifest_version" "$package_version" >&2
    exit 1
fi

if [[ -e "$archive_output" || -L "$archive_output" ]]; then
    printf 'error: refusing to overwrite existing output: %s\n' "$archive_output" >&2
    exit 1
fi
if [[ -e "$checksum_output" || -L "$checksum_output" ]]; then
    printf 'error: refusing to overwrite existing output: %s\n' "$checksum_output" >&2
    exit 1
fi
lock_owned=0
temporary_directory=''
readonly ownership_sentinel_name='.veritasm-source-package.owner'
lock_ownership_sentinel="$lock_directory/$ownership_sentinel_name"
temporary_ownership_sentinel=''
ownership_fd_open=0
archive_published=0
checksum_published=0
archive_fd_open=0
checksum_fd_open=0
first_validation_readers_open=0
final_validation_readers_open=0
package_complete=0

ignore_termination_signals() {
    trap '' HUP INT TERM
}

arm_termination_signals() {
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
}

cleanup() {
    local status=$?
    local cleanup_failed=0
    trap - EXIT HUP INT TERM
    set +e

    if ((package_complete == 0)); then
        # Publication flags are deliberately not consulted here. A signal can
        # arrive after ln(1) creates a public link but before the next shell
        # assignment. The held descriptors are the authoritative ownership
        # anchors throughout that interval.
        if [[ -e "$checksum_output" || -L "$checksum_output" ]]; then
            if ((checksum_fd_open != 1)) \
                || [[ ! -f "$checksum_output" || -L "$checksum_output" ]] \
                || [[ ! "$checksum_output" -ef /dev/fd/7 ]]; then
                printf 'error: refusing to roll back replaced source checksum: %s\n' \
                    "$checksum_output" >&2
                cleanup_failed=1
            elif ! unlink -- "$checksum_output"; then
                printf 'error: could not roll back published checksum: %s\n' \
                    "$checksum_output" >&2
                cleanup_failed=1
            fi
        fi
        if [[ -e "$archive_output" || -L "$archive_output" ]]; then
            if ((archive_fd_open != 1)) \
                || [[ ! -f "$archive_output" || -L "$archive_output" ]] \
                || [[ ! "$archive_output" -ef /dev/fd/8 ]]; then
                printf 'error: refusing to roll back replaced source archive: %s\n' \
                    "$archive_output" >&2
                cleanup_failed=1
            elif ! unlink -- "$archive_output"; then
                printf 'error: could not roll back published archive: %s\n' \
                    "$archive_output" >&2
                cleanup_failed=1
            fi
        fi
    fi

    if [[ -n "$temporary_directory" \
        && ( -e "$temporary_directory" || -L "$temporary_directory" ) ]]; then
        case "$temporary_directory" in
            "$repository_root"/.veritasm-package.*)
                if ((ownership_fd_open != 1)) \
                    || [[ ! -d "$temporary_directory" || -L "$temporary_directory" ]] \
                    || [[ ! -f "$temporary_ownership_sentinel" \
                        || -L "$temporary_ownership_sentinel" ]] \
                    || [[ ! "$temporary_ownership_sentinel" -ef /dev/fd/9 ]]; then
                    printf 'error: refusing to clean replaced source-package temporary directory: %s\n' \
                        "$temporary_directory" >&2
                    cleanup_failed=1
                else
                    if ! find "$temporary_directory" -depth -mindepth 1 -delete; then
                        printf 'error: could not clean source-package temporary contents: %s\n' \
                            "$temporary_directory" >&2
                        cleanup_failed=1
                    fi
                    if [[ -d "$temporary_directory" ]] \
                        && ! rmdir -- "$temporary_directory"; then
                        printf 'error: could not remove source-package temporary directory: %s\n' \
                            "$temporary_directory" >&2
                        cleanup_failed=1
                    fi
                fi
                ;;
            *)
                printf 'error: refusing to clean unexpected temporary path: %s\n' \
                    "$temporary_directory" >&2
                cleanup_failed=1
                ;;
        esac
    fi

    if ((lock_owned == 1)) \
        && [[ -e "$lock_directory" || -L "$lock_directory" ]]; then
        if ((ownership_fd_open != 1)) \
            || [[ ! -d "$lock_directory" || -L "$lock_directory" ]] \
            || [[ ! -f "$lock_ownership_sentinel" \
                || -L "$lock_ownership_sentinel" ]] \
            || [[ ! "$lock_ownership_sentinel" -ef /dev/fd/9 ]]; then
            printf 'error: refusing to clean replaced source-package lock directory: %s\n' \
                "$lock_directory" >&2
            cleanup_failed=1
        else
            if ! unlink -- "$lock_ownership_sentinel"; then
                printf 'error: could not remove source-package lock ownership sentinel: %s\n' \
                    "$lock_ownership_sentinel" >&2
                cleanup_failed=1
            fi
            if [[ -d "$lock_directory" ]] && ! rmdir -- "$lock_directory"; then
                printf 'error: could not remove source-package lock directory: %s\n' \
                    "$lock_directory" >&2
                cleanup_failed=1
            fi
        fi
    fi
    if ((checksum_fd_open == 1)); then
        exec 7>&-
    fi
    if ((archive_fd_open == 1)); then
        exec 8>&-
    fi
    if ((first_validation_readers_open == 1)); then
        exec 10>&-
        exec 11>&-
        exec 12>&-
        exec 13>&-
    fi
    if ((final_validation_readers_open == 1)); then
        exec 14>&-
        exec 15>&-
        exec 16>&-
        exec 17>&-
    fi
    if ((ownership_fd_open == 1)); then
        exec 9>&-
    fi
    if ((status == 0 && cleanup_failed != 0)); then
        status=1
    fi
    if ((status == 0 && package_complete == 1)); then
        if [[ -e "$temporary_directory" || -L "$temporary_directory" \
            || -e "$lock_directory" || -L "$lock_directory" ]]; then
            printf 'error: source-package cleanup left temporary state behind\n' >&2
            status=1
        else
            printf 'created %s\n' "$archive_output"
            printf 'created %s\n' "$checksum_output"
            printf 'sha256 %s\n' "$archive_digest"
            printf 'source-package cleanup: PASS\n'
        fi
    fi
    exit "$status"
}
trap cleanup EXIT

# mkdir(1) cannot create a populated directory atomically. Ignore catchable
# termination signals only across the tiny acquisition section so no signal
# can land after the public lock appears but before its held ownership anchor
# exists. A pre-existing foreign lock still makes mkdir fail and is untouched.
ignore_termination_signals
if ! mkdir -- "$lock_directory" 2>/dev/null; then
    arm_termination_signals
    printf 'error: source packaging is locked: %s\n' "$lock_directory" >&2
    exit 1
fi
lock_owned=1
if ! exec 9>"$lock_ownership_sentinel"; then
    arm_termination_signals
    printf 'error: could not establish source-package lock ownership\n' >&2
    exit 1
fi
ownership_fd_open=1
arm_termination_signals

# Apply the same rule to the private workspace: assign its path and install
# the hard-linked sentinel before a catchable termination can run cleanup.
ignore_termination_signals
if ! temporary_directory=$(mktemp -d \
    "$repository_root/.veritasm-package.XXXXXXXXXX"); then
    arm_termination_signals
    printf 'error: could not create source-package temporary directory\n' >&2
    exit 1
fi
temporary_ownership_sentinel="$temporary_directory/$ownership_sentinel_name"
if ! ln -- "$lock_ownership_sentinel" "$temporary_ownership_sentinel"; then
    arm_termination_signals
    printf 'error: could not establish source-package temporary ownership\n' >&2
    exit 1
fi
arm_termination_signals
staging_parent="$temporary_directory/stage"
staging_root="$staging_parent/$archive_root"
candidate_list="$temporary_directory/candidates.nul"
source_inventory="$temporary_directory/source-inventory.txt"
actual_directory_inventory="$temporary_directory/source-directory-actual.txt"
expected_directory_inventory="$temporary_directory/source-directory-expected.txt"
archive_inventory="$temporary_directory/archive-inventory.txt"
actual_inventory="$temporary_directory/archive-actual.txt"
temporary_archive="$temporary_directory/$archive_name"
temporary_checksum="$temporary_directory/$checksum_name"
root_candidate_list="$temporary_directory/root-candidates.nul"
post_candidate_list="$temporary_directory/post-candidates.nul"
post_directory_inventory="$temporary_directory/source-directory-post.txt"
live_source_manifest_before="$temporary_directory/live-source-before.txt"
staged_source_manifest="$temporary_directory/staged-source.txt"
live_source_manifest_after="$temporary_directory/live-source-after.txt"
live_source_manifest_published="$temporary_directory/live-source-published.txt"

mkdir -p -- "$staging_root"
: >"$candidate_list"
: >"$source_inventory"
: >"$actual_directory_inventory"

readonly -a required_root_files=(
    '.gitattributes'
    '.gitignore'
    'AGENTS.md'
    'ARCHITECTURE.md'
    'BENCHMARK.md'
    'CHANGELOG.md'
    'CITATION.cff'
    'CONTRIBUTING.md'
    'Cargo.lock'
    'Cargo.toml'
    'LICENSE'
    'README.md'
    'RESEARCH.md'
    'SECURITY.md'
    'VALIDATION.md'
    'deny.toml'
    'rustfmt.toml'
)
readonly -a required_source_directories=(
    '.github'
    'benchmark'
    'docs'
    'examples'
    'fuzz'
    'proptest-regressions'
    'schema'
    'scripts'
    'src'
    'tests'
)

is_excluded_relative_path() {
    local relative=$1
    local component basename lowercase_relative
    # macOS ships Bash 3.2, which does not support Bash 4's ${value,,}
    # lowercase expansion. Keep the source-packaging path executable there.
    lowercase_relative=$(printf '%s' "$relative" | tr '[:upper:]' '[:lower:]')
    basename=${lowercase_relative##*/}

    case "$basename" in
        *.zip | *.sha256 | *.pem | *.key | *.p12 | *.pfx | *.token | \
            .env | .env.* | credentials | credentials.* | secrets | secrets.* | \
            id_rsa | id_ed25519)
            return 0
            ;;
    esac

    local remainder=$lowercase_relative
    while :; do
        component=${remainder%%/*}
        case "$component" in
            .git | target | temp | tmp | result | results | credentials | secrets)
                return 0
                ;;
        esac
        [[ "$remainder" == */* ]] || break
        remainder=${remainder#*/}
    done
    return 1
}

validate_portable_relative_path() {
    local relative=$1
    if [[ -z "$relative" \
        || "$relative" == /* \
        || "$relative" == */ \
        || "$relative" == *//* \
        || "$relative" == '.' \
        || "$relative" == '..' \
        || "$relative" == ./* \
        || "$relative" == */./* \
        || "$relative" == */. \
        || "$relative" == ../* \
        || "$relative" == */../* \
        || "$relative" == */.. \
        || "$relative" == *[!A-Za-z0-9._/-]* ]]; then
        return 1
    fi
    return 0
}

validate_relative_path() {
    local relative=$1
    if ! validate_portable_relative_path "$relative"; then
        printf 'error: unsafe source path: %q\n' "$relative" >&2
        return 1
    fi
}

validate_no_casefold_collisions() {
    local inventory=$1
    local label=$2
    if ! awk -v label="$label" '
        {
            folded = tolower($0)
            if (folded in first) {
                printf "error: %s contains case-fold-colliding paths: %s and %s\n", \
                    label, first[folded], $0 > "/dev/stderr"
                exit 1
            }
            first[folded] = $0
        }
    ' "$inventory"; then
        return 1
    fi
    return 0
}

is_required_root_file() {
    local candidate=$1
    local required
    for required in "${required_root_files[@]}"; do
        if [[ "$candidate" == "$required" ]]; then
            return 0
        fi
    done
    return 1
}

is_required_source_directory() {
    local candidate=$1
    local required
    for required in "${required_source_directories[@]}"; do
        if [[ "$candidate" == "$required" ]]; then
            return 0
        fi
    done
    return 1
}

# Root entries need a separate, explicit policy. The recursive source
# allowlist covers only required_source_directories; without this check, a new
# top-level source, configuration, or documentation file could be silently
# omitted. Generated paths are admitted only by exact name and ownership, not
# by broad suffix or prefix patterns.
validate_repository_root_inventory() {
    local entry relative

    if ! find "$repository_root" -mindepth 1 -maxdepth 1 -print0 \
        >"$root_candidate_list"; then
        printf 'error: could not enumerate repository-root entries\n' >&2
        return 1
    fi

    while IFS= read -r -d '' entry; do
        relative=${entry#"$repository_root/"}
        validate_relative_path "$relative"

        if is_required_root_file "$relative"; then
            if [[ ! -f "$entry" || -L "$entry" ]]; then
                printf 'error: required source file is missing or not regular: %s\n' \
                    "$relative" >&2
                return 1
            fi
            continue
        fi
        if is_required_source_directory "$relative"; then
            if [[ ! -d "$entry" || -L "$entry" ]]; then
                printf 'error: required source directory is missing or not a directory: %s\n' \
                    "$relative" >&2
                return 1
            fi
            continue
        fi

        case "$relative" in
            .git)
                # A normal checkout uses a directory; a linked Git worktree
                # uses a regular gitdir pointer file. Neither enters the ZIP.
                if [[ -L "$entry" || ( ! -d "$entry" && ! -f "$entry" ) ]]; then
                    printf 'error: repository metadata path has an unsafe type: %s\n' \
                        "$relative" >&2
                    return 1
                fi
                ;;
            target)
                if [[ ! -d "$entry" || -L "$entry" ]]; then
                    printf 'error: generated build path is not a regular directory: %s\n' \
                        "$relative" >&2
                    return 1
                fi
                ;;
            "$archive_name")
                if ((archive_published != 1)) \
                    || [[ ! -f "$entry" || -L "$entry" ]] \
                    || [[ ! -e "$temporary_archive" ]] \
                    || [[ ! "$entry" -ef "$temporary_archive" ]]; then
                    printf 'error: source archive exists but is not owned by this packaging run: %s\n' \
                        "$relative" >&2
                    return 1
                fi
                ;;
            "$checksum_name")
                if ((checksum_published != 1)) \
                    || [[ ! -f "$entry" || -L "$entry" ]] \
                    || [[ ! -e "$temporary_checksum" ]] \
                    || [[ ! "$entry" -ef "$temporary_checksum" ]]; then
                    printf 'error: source checksum exists but is not owned by this packaging run: %s\n' \
                        "$relative" >&2
                    return 1
                fi
                ;;
            .veritasm-source-package.lock)
                if ((lock_owned != 1)) \
                    || [[ "$entry" != "$lock_directory" ]] \
                    || [[ ! -d "$entry" || -L "$entry" ]] \
                    || ((ownership_fd_open != 1)) \
                    || [[ ! -f "$lock_ownership_sentinel" \
                        || -L "$lock_ownership_sentinel" ]] \
                    || [[ ! "$lock_ownership_sentinel" -ef /dev/fd/9 ]]; then
                    printf 'error: source-package lock is not owned by this packaging run: %s\n' \
                        "$relative" >&2
                    return 1
                fi
                ;;
            .veritasm-package.*)
                if [[ "$entry" != "$temporary_directory" \
                    || ! -d "$entry" || -L "$entry" \
                    || ! -f "$temporary_ownership_sentinel" \
                    || -L "$temporary_ownership_sentinel" ]] \
                    || ((ownership_fd_open != 1)) \
                    || [[ ! "$temporary_ownership_sentinel" -ef /dev/fd/9 ]]; then
                    printf 'error: source-package temporary path is not owned by this packaging run: %s\n' \
                        "$relative" >&2
                    return 1
                fi
                ;;
            *)
                printf 'error: unexpected repository-root entry would be omitted: %s\n' \
                    "$relative" >&2
                return 1
                ;;
        esac
    done <"$root_candidate_list"
}

validate_repository_root_inventory

write_live_directory_inventory() {
    local candidate_output=$1
    local inventory_output=$2
    local relative source_path

    : >"$candidate_output"
    : >"$inventory_output"
    for relative in "${required_source_directories[@]}"; do
        source_path="$repository_root/$relative"
        if [[ ! -d "$source_path" || -L "$source_path" ]]; then
            printf 'error: required source directory is missing or not a directory: %s\n' \
                "$relative" >&2
            return 1
        fi
        find "$source_path" \
            \( -type d \( -name .git -o -name target -o -name temp -o -name tmp \
                -o -name result -o -name results -o -name credentials -o -name secrets \) \
                -prune -print0 \) \
            -o -print0 >>"$candidate_output" || return 1
    done

    while IFS= read -r -d '' source_path; do
        relative=${source_path#"$repository_root/"}
        validate_relative_path "$relative" || return 1
        if is_excluded_relative_path "$relative"; then
            printf 'error: governed source tree contains an excluded path: %s\n' \
                "$relative" >&2
            return 1
        fi
        if [[ -d "$source_path" && ! -L "$source_path" ]]; then
            continue
        fi
        if [[ -L "$source_path" || ! -f "$source_path" ]]; then
            printf 'error: included source tree contains a non-regular entry: %s\n' \
                "$relative" >&2
            return 1
        fi
        printf '%s\n' "$relative" >>"$inventory_output" || return 1
    done <"$candidate_output"
    sort -u -o "$inventory_output" "$inventory_output"
}

write_source_manifest() {
    local tree_root=$1
    local output=$2
    local relative source_path digest

    : >"$output"
    while IFS= read -r relative; do
        source_path="$tree_root/$relative"
        if [[ ! -f "$source_path" || -L "$source_path" ]]; then
            printf 'error: source changed type while packaging: %s\n' "$relative" >&2
            return 1
        fi
        digest=$(sha256_file "$source_path") || return 1
        if [[ ! "$digest" =~ ^[0-9a-f]{64}$ ]]; then
            printf 'error: SHA-256 tool returned an invalid source digest for %s\n' \
                "$relative" >&2
            return 1
        fi
        printf '%s  %s\n' "$digest" "$relative" >>"$output" || return 1
    done <"$source_inventory"
}

for relative in "${required_root_files[@]}"; do
    source_path="$repository_root/$relative"
    if [[ ! -f "$source_path" || -L "$source_path" ]]; then
        printf 'error: required source file is missing or not regular: %s\n' "$relative" >&2
        exit 1
    fi
    validate_relative_path "$relative"
    printf '%s\n' "$relative" >>"$source_inventory"
done

write_live_directory_inventory "$candidate_list" "$actual_directory_inventory"

if [[ ! -f "$source_allowlist" || -L "$source_allowlist" ]]; then
    printf 'error: source-package allowlist is missing or not regular: %s\n' \
        "$source_allowlist" >&2
    exit 1
fi
sort -u "$source_allowlist" >"$expected_directory_inventory"
if ! cmp -s "$source_allowlist" "$expected_directory_inventory"; then
    printf 'error: source-package allowlist must be nonempty, sorted, and unique\n' >&2
    diff -u "$expected_directory_inventory" "$source_allowlist" >&2 || true
    exit 1
fi
if [[ ! -s "$expected_directory_inventory" ]]; then
    printf 'error: source-package allowlist is empty\n' >&2
    exit 1
fi
validate_no_casefold_collisions \
    "$expected_directory_inventory" 'source-package allowlist'

while IFS= read -r relative; do
    validate_relative_path "$relative"
    if is_excluded_relative_path "$relative"; then
        printf 'error: source-package allowlist contains an excluded path: %s\n' \
            "$relative" >&2
        exit 1
    fi
    source_path="$repository_root/$relative"
    if [[ ! -f "$source_path" || -L "$source_path" ]]; then
        printf 'error: allowlisted source is missing or not regular: %s\n' "$relative" >&2
        exit 1
    fi
done <"$expected_directory_inventory"

sort -u -o "$actual_directory_inventory" "$actual_directory_inventory"
if ! cmp -s "$expected_directory_inventory" "$actual_directory_inventory"; then
    printf 'error: source directory inventory differs from scripts/source-package-files.txt\n' >&2
    diff -u "$expected_directory_inventory" "$actual_directory_inventory" >&2 || true
    exit 1
fi

while IFS= read -r relative; do
    printf '%s\n' "$relative" >>"$source_inventory"
done <"$expected_directory_inventory"

sort -u -o "$source_inventory" "$source_inventory"
if [[ ! -s "$source_inventory" ]]; then
    printf 'error: source inventory is empty\n' >&2
    exit 1
fi
validate_no_casefold_collisions "$source_inventory" 'source inventory'

for required_prefix in '.github/workflows/' 'benchmark/' 'docs/' 'examples/' 'fuzz/' \
    'proptest-regressions/' 'schema/' 'scripts/' 'src/' 'tests/'; do
    if ! grep -q "^${required_prefix}" "$source_inventory"; then
        printf 'error: source inventory lacks required content prefix: %s\n' \
            "$required_prefix" >&2
        exit 1
    fi
done

write_source_manifest "$repository_root" "$live_source_manifest_before"

while IFS= read -r relative; do
    source_path="$repository_root/$relative"
    destination_path="$staging_root/$relative"
    mkdir -p -- "$(dirname -- "$destination_path")"
    cp -- "$source_path" "$destination_path"
done <"$source_inventory"

write_source_manifest "$staging_root" "$staged_source_manifest"
if ! cmp -s "$live_source_manifest_before" "$staged_source_manifest"; then
    printf 'error: staged source bytes differ from the initial live-source snapshot\n' >&2
    diff -u "$live_source_manifest_before" "$staged_source_manifest" >&2 || true
    exit 1
fi

find "$staging_root" -type d -exec chmod 0755 {} +
find "$staging_root" -type f -exec chmod 0644 {} +
while IFS= read -r executable_path; do
    chmod 0755 "$executable_path"
done < <(
    find "$staging_root/scripts" "$staging_root/benchmark" \
        -type f -name '*.sh' -print | sort
)
find "$staging_root" -exec touch -t "$normalized_timestamp" {} +

awk -v root="$archive_root" '{ print root "/" $0 }' \
    "$source_inventory" >"$archive_inventory"
(
    cd -- "$staging_parent"
    zip -q -X -9 "$temporary_archive" -@ <"$archive_inventory"
)

unzip -tq "$temporary_archive" >/dev/null
unzip -Z1 "$temporary_archive" >"$actual_inventory"
if ! cmp -s "$archive_inventory" "$actual_inventory"; then
    printf 'error: ZIP inventory differs from the declared source inventory\n' >&2
    diff -u "$archive_inventory" "$actual_inventory" >&2 || true
    exit 1
fi

while IFS= read -r archived_path; do
    case "$archived_path" in
        "$archive_root"/*)
            relative=${archived_path#"$archive_root/"}
            ;;
        *)
            printf 'error: ZIP entry escapes the required top-level directory: %s\n' \
                "$archived_path" >&2
            exit 1
            ;;
    esac
    validate_relative_path "$relative"
    if is_excluded_relative_path "$relative"; then
        printf 'error: excluded path entered ZIP inventory: %s\n' "$relative" >&2
        exit 1
    fi
    if ! unzip -p "$temporary_archive" "$archived_path" \
        | cmp -s - "$staging_parent/$archived_path"; then
        printf 'error: ZIP content differs from staged source: %s\n' "$archived_path" >&2
        exit 1
    fi
done <"$actual_inventory"

# Re-enumerate and re-hash the producing tree immediately before publication.
# This fails closed if an editor, generator, or concurrent process changes a
# source path or byte while the archive is being staged and validated.
validate_repository_root_inventory
write_live_directory_inventory "$post_candidate_list" "$post_directory_inventory"
if ! cmp -s "$expected_directory_inventory" "$post_directory_inventory"; then
    printf 'error: source directory inventory changed while packaging\n' >&2
    diff -u "$expected_directory_inventory" "$post_directory_inventory" >&2 || true
    exit 1
fi
write_source_manifest "$repository_root" "$live_source_manifest_after"
if ! cmp -s "$live_source_manifest_before" "$live_source_manifest_after"; then
    printf 'error: live source bytes changed while packaging\n' >&2
    diff -u "$live_source_manifest_before" "$live_source_manifest_after" >&2 || true
    exit 1
fi

archive_digest=$(sha256_file "$temporary_archive")
if [[ ! "$archive_digest" =~ ^[0-9a-f]{64}$ ]]; then
    printf 'error: SHA-256 tool returned an invalid digest\n' >&2
    exit 1
fi
printf '%s  %s\n' "$archive_digest" "$archive_name" >"$temporary_checksum"
if [[ "$(sha256_file "$temporary_archive")" != "$archive_digest" ]]; then
    printf 'error: temporary archive digest changed after sidecar creation\n' >&2
    exit 1
fi
chmod 0644 "$temporary_archive" "$temporary_checksum"
touch -t "$normalized_timestamp" "$temporary_archive" "$temporary_checksum"
exec 8<"$temporary_archive"
archive_fd_open=1
exec 7<"$temporary_checksum"
checksum_fd_open=1
# Darwin /dev/fd opens duplicate the referenced descriptor and therefore share
# its open-file offset. Keep descriptors 7 and 8 untouched as identity anchors,
# and give every content operation in both validation passes its own reader.
exec 10<"$temporary_archive"
exec 11<"$temporary_archive"
exec 12<"$temporary_archive"
exec 13<"$temporary_checksum"
first_validation_readers_open=1
exec 14<"$temporary_archive"
exec 15<"$temporary_archive"
exec 16<"$temporary_archive"
exec 17<"$temporary_checksum"
final_validation_readers_open=1

# Validate bytes through the private descriptors that anchor this invocation's
# outputs, not through public names that another same-UID process can replace.
# Public-name identity is checked on both sides of the content checks so a
# replacement during validation also fails closed.
validate_published_package_outputs() {
    local validation_phase=$1
    local archive_hash_reader=$2
    local archive_integrity_reader=$3
    local archive_inventory_reader=$4
    local checksum_reader=$5
    local anchored_archive_digest

    if ((archive_fd_open != 1)) \
        || [[ ! -f "$archive_output" || -L "$archive_output" ]] \
        || [[ ! "$archive_output" -ef /dev/fd/8 ]]; then
        printf 'error: published source archive was replaced before %s\n' \
            "$validation_phase" >&2
        return 1
    fi
    if ((checksum_fd_open != 1)) \
        || [[ ! -f "$checksum_output" || -L "$checksum_output" ]] \
        || [[ ! "$checksum_output" -ef /dev/fd/7 ]]; then
        printf 'error: published source checksum was replaced before %s\n' \
            "$validation_phase" >&2
        return 1
    fi

    if ! anchored_archive_digest=$(sha256_file "$archive_hash_reader"); then
        printf 'error: could not hash anchored source archive during %s\n' \
            "$validation_phase" >&2
        return 1
    fi
    if [[ "$anchored_archive_digest" != "$archive_digest" ]]; then
        printf 'error: anchored source archive digest differs during %s\n' \
            "$validation_phase" >&2
        return 1
    fi
    if ! unzip -tq "$archive_integrity_reader" >/dev/null; then
        printf 'error: anchored source archive failed integrity during %s\n' \
            "$validation_phase" >&2
        return 1
    fi
    if ! unzip -Z1 "$archive_inventory_reader" | cmp -s - "$archive_inventory"; then
        printf 'error: anchored source archive inventory differs during %s\n' \
            "$validation_phase" >&2
        return 1
    fi
    if ! printf '%s  %s\n' "$archive_digest" "$archive_name" \
        | cmp -s - "$checksum_reader"; then
        printf 'error: anchored source checksum content differs during %s\n' \
            "$validation_phase" >&2
        return 1
    fi

    if [[ ! -f "$archive_output" || -L "$archive_output" \
        || ! "$archive_output" -ef /dev/fd/8 ]]; then
        printf 'error: published source archive was replaced during %s\n' \
            "$validation_phase" >&2
        return 1
    fi
    if [[ ! -f "$checksum_output" || -L "$checksum_output" \
        || ! "$checksum_output" -ef /dev/fd/7 ]]; then
        printf 'error: published source checksum was replaced during %s\n' \
            "$validation_phase" >&2
        return 1
    fi
}

# Recheck after acquiring the cooperative lock and immediately before the
# no-overwrite hard-link publication points.
if [[ -e "$archive_output" || -L "$archive_output" \
    || -e "$checksum_output" || -L "$checksum_output" ]]; then
    printf 'error: refusing to overwrite an output created during packaging\n' >&2
    exit 1
fi
if ! ln -- "$temporary_archive" "$archive_output"; then
    printf 'error: could not publish source archive without replacement\n' >&2
    exit 1
fi
archive_published=1
if [[ ! -f "$archive_output" || -L "$archive_output" \
    || ! "$archive_output" -ef /dev/fd/8 ]]; then
    printf 'error: published source archive was replaced before ownership validation\n' >&2
    exit 1
fi
if ! ln -- "$temporary_checksum" "$checksum_output"; then
    printf 'error: could not publish source checksum without replacement\n' >&2
    exit 1
fi
checksum_published=1
if [[ ! -f "$checksum_output" || -L "$checksum_output" \
    || ! "$checksum_output" -ef /dev/fd/7 ]]; then
    printf 'error: published source checksum was replaced before ownership validation\n' >&2
    exit 1
fi

if ! validate_published_package_outputs 'post-publication validation' \
    /dev/fd/10 /dev/fd/11 /dev/fd/12 /dev/fd/13; then
    exit 1
fi
exec 10>&-
exec 11>&-
exec 12>&-
exec 13>&-
first_validation_readers_open=0

# Catch top-level entries introduced while the archive was being staged or
# validated. The two published files pass only because they are hard links to
# the exact validated temporary outputs owned by this invocation.
validate_repository_root_inventory
write_live_directory_inventory "$post_candidate_list" "$post_directory_inventory"
if ! cmp -s "$expected_directory_inventory" "$post_directory_inventory"; then
    printf 'error: source directory inventory changed before packaging completed\n' >&2
    diff -u "$expected_directory_inventory" "$post_directory_inventory" >&2 || true
    exit 1
fi
write_source_manifest "$repository_root" "$live_source_manifest_published"
if ! cmp -s "$live_source_manifest_before" "$live_source_manifest_published"; then
    printf 'error: live source bytes changed before packaging completed\n' >&2
    diff -u "$live_source_manifest_before" "$live_source_manifest_published" >&2 || true
    exit 1
fi

if ! validate_published_package_outputs 'final completion validation' \
    /dev/fd/14 /dev/fd/15 /dev/fd/16 /dev/fd/17; then
    exit 1
fi
exec 14>&-
exec 15>&-
exec 16>&-
exec 17>&-
final_validation_readers_open=0
package_complete=1
# Finalize explicitly before announcing success. The EXIT trap remains the
# failure/signal fallback, and cleanup disarms it before closing descriptors.
cleanup
