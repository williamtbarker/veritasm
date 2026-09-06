#!/usr/bin/env bash
# Verify a VeritAsm source review ZIP without using checkout build artifacts.

set -u
set -o pipefail
IFS=$'\n\t'
export LC_ALL=C
export TZ=UTC
umask 077

# Ambient compiler/wrapper settings can select an unintended target or inject
# flags that were not part of this verification. Environment-level Cargo
# network policy remains effective, but caller Cargo config files are excluded.
caller_cargo_home=${CARGO_HOME:-}
unset CARGO_BUILD_TARGET CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_DIR
unset CARGO_BUILD_RUSTC CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER
unset CARGO_BUILD_RUSTDOC CARGO_BUILD_RUSTDOCFLAGS CARGO_BUILD_RUSTFLAGS
unset RUSTC RUSTC_BOOTSTRAP RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER
unset RUSTDOC RUSTDOCFLAGS RUSTFLAGS
unset RUSTUP_TOOLCHAIN
unset TAR_OPTIONS UNZIP UNZIPOPT ZIPINFO ZIPINFOOPT

readonly package_name='veritasm'
readonly package_version='0.4.0-dev.1'
readonly archive_root="${package_name}-${package_version}"
readonly default_archive_name="${archive_root}-source.zip"
readonly msrv_toolchain='1.85.0'
readonly stable_toolchain='stable'
readonly max_source_zip_archive_bytes=67108864
readonly max_source_zip_members=4096
readonly max_source_zip_member_bytes=67108864
readonly max_source_zip_uncompressed_bytes=268435456
readonly max_source_zip_expansion_ratio=200
readonly max_checksum_sidecar_bytes=256
readonly bounded_copy_block_bytes=65536
readonly transcript_finalize_timeout_seconds=5

usage() {
    printf 'usage: %s [SOURCE_ZIP [SHA256_SIDECAR]]\n' "${0##*/}"
    printf '\n'
    printf 'Defaults to %s and %s.sha256 in the repository root.\n' \
        "$default_archive_name" "$default_archive_name"
    printf 'Installed Rustup toolchains are used; this script never installs a toolchain.\n'
}

if (($# > 2)); then
    usage >&2
    exit 2
fi
if (($# >= 1)) && [[ "$1" == '-h' || "$1" == '--help' ]]; then
    usage
    exit 0
fi

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P) || exit 1
repository_root=$(CDPATH= cd -- "$script_directory/.." && pwd -P) || exit 1
archive_argument=${1:-"$repository_root/$default_archive_name"}
checksum_argument=${2:-"${archive_argument}.sha256"}

fatal() {
    printf 'FATAL: %s\n' "$*" >&2
    exit 1
}

note() {
    printf '%s\n' "$*"
}

resolve_regular_file() {
    local supplied=$1
    local directory base
    if [[ ! -f "$supplied" || -L "$supplied" ]]; then
        return 1
    fi
    directory=$(CDPATH= cd -- "$(dirname -- "$supplied")" && pwd -P) || return 1
    base=$(basename -- "$supplied") || return 1
    printf '%s/%s\n' "$directory" "$base"
}

archive_path=$(resolve_regular_file "$archive_argument") \
    || fatal "source ZIP is missing, symlinked, or not a regular file: $archive_argument"
checksum_path=$(resolve_regular_file "$checksum_argument") \
    || fatal "checksum sidecar is missing, symlinked, or not a regular file: $checksum_argument"
supplied_archive_path=$archive_path
supplied_checksum_path=$checksum_path
readonly supplied_archive_path supplied_checksum_path

# Keep the exact input objects open from the initial path validation through
# the private bounded copy. The descriptors are each consumed only once: on
# Darwin, reopening /dev/fd/N is dup-like and therefore shares its file offset.
if ! exec 5<"$supplied_archive_path"; then
    fatal "could not anchor supplied source ZIP: $supplied_archive_path"
fi
if ! exec 6<"$supplied_checksum_path"; then
    fatal "could not anchor supplied checksum sidecar: $supplied_checksum_path"
fi

verify_supplied_input_identity() {
    local phase=$1
    if [[ ! -f "$supplied_archive_path" || -L "$supplied_archive_path" \
        || ! -f /dev/fd/5 \
        || ! "$supplied_archive_path" -ef /dev/fd/5 ]]; then
        printf 'supplied source ZIP changed identity %s: %s\n' \
            "$phase" "$supplied_archive_path" >&2
        return 1
    fi
    if [[ ! -f "$supplied_checksum_path" || -L "$supplied_checksum_path" \
        || ! -f /dev/fd/6 \
        || ! "$supplied_checksum_path" -ef /dev/fd/6 ]]; then
        printf 'supplied checksum sidecar changed identity %s: %s\n' \
            "$phase" "$supplied_checksum_path" >&2
        return 1
    fi
    return 0
}

verify_supplied_input_identity 'during initial anchoring' \
    || fatal 'supplied source ZIP or checksum sidecar changed during initial anchoring'
if [[ "$archive_path" == *[[:cntrl:]]* || "$checksum_path" == *[[:cntrl:]]* ]]; then
    fatal 'source ZIP and checksum paths must not contain control characters'
fi
archive_basename=$(basename -- "$archive_path") || fatal 'could not determine archive basename'
if [[ "$archive_basename" != "$default_archive_name" ]]; then
    fatal "source ZIP must be named $default_archive_name (received $archive_basename)"
fi

for required_tool in awk basename cmp cp dd diff dirname env find grep ln mkdir mkfifo mktemp mv \
    rmdir sed sleep sort tar tee tr uniq unlink unzip wc; do
    if ! command -v "$required_tool" >/dev/null 2>&1; then
        fatal "required verification tool is unavailable: $required_tool"
    fi
done
if ! command -v sha256sum >/dev/null 2>&1 \
    && ! command -v shasum >/dev/null 2>&1; then
    fatal 'neither sha256sum nor shasum is available'
fi

host_kernel=$(uname -s 2>/dev/null || printf 'unknown')
host_arch=$(uname -m 2>/dev/null || printf 'unknown')
platform_incomplete=0
expected_rust_host=''
case "$host_kernel:$host_arch" in
    Darwin:arm64)
        expected_rust_host='aarch64-apple-darwin'
        local_platform_status='EXECUTED on Apple Silicon macOS host'
        other_platform_status='Linux NOT RUN on this host'
        ;;
    Linux:x86_64)
        expected_rust_host='x86_64-unknown-linux-gnu'
        local_platform_status='EXECUTED on Linux x86_64 host'
        other_platform_status='Apple Silicon macOS NOT RUN on this host'
        ;;
    Linux:aarch64 | Linux:arm64)
        expected_rust_host='aarch64-unknown-linux-gnu'
        local_platform_status="EXECUTED on Linux $host_arch host"
        other_platform_status='Apple Silicon macOS NOT RUN on this host'
        ;;
    *)
        local_platform_status="EXECUTED on unsupported/unclassified host $host_kernel $host_arch"
        other_platform_status='Linux and Apple Silicon macOS support claims NOT ESTABLISHED'
        platform_incomplete=1
        ;;
esac
ambient_cargo_override=$(env | awk -F '=' '
    /^CARGO_ALIAS_/ || /^CARGO_PROFILE_/ ||
    (/^CARGO_TARGET_/ && /_(LINKER|RUNNER|RUSTFLAGS)=/) {
        print $1
        exit
    }
')
if [[ -n "$ambient_cargo_override" ]]; then
    fatal "ambient Cargo override must be unset before verification: $ambient_cargo_override"
fi

work_parent_argument=${VERITASM_VERIFY_WORK_PARENT:-${TMPDIR:-/tmp}}
if [[ ! -d "$work_parent_argument" ]]; then
    fatal "verification work parent is not a directory: $work_parent_argument"
fi
work_parent=$(CDPATH= cd -- "$work_parent_argument" && pwd -P) \
    || fatal "could not resolve verification work parent: $work_parent_argument"
if [[ "$work_parent" == *[[:cntrl:]]* ]]; then
    fatal 'verification work parent must not contain control characters'
fi
verification_root=$(mktemp -d "$work_parent/veritasm-verify-${package_version}.XXXXXXXX") \
    || fatal "could not create a fresh verification directory under $work_parent"
readonly verification_root
readonly transcript="$verification_root/verification.log"
readonly transcript_fifo="$verification_root/transcript.pipe"
readonly summary="$verification_root/verification-summary.txt"
readonly crate_store="$verification_root/generated-crates"
readonly isolated_cargo_home="$verification_root/cargo-home"
mkdir -- "$crate_store" "$isolated_cargo_home" \
    || fatal 'could not create verification support directories'

# Cargo configuration and extracted dependency sources are isolated from the
# caller. When available, registry index/package-archive caches are symlinked
# for reuse; they are shared mutable cache state, not hermetic evidence. Cargo
# still freshly extracts locked, checksummed packages into this CARGO_HOME.
if [[ -z "$caller_cargo_home" ]] && command -v rustup >/dev/null 2>&1; then
    rustup_parent=$(CDPATH= cd -- "$(dirname -- "$(command -v rustup)")/.." \
        && pwd -P) || rustup_parent=''
    if [[ -d "$rustup_parent/registry" ]]; then
        caller_cargo_home=$rustup_parent
    fi
fi
if [[ -n "$caller_cargo_home" && -d "$caller_cargo_home" ]]; then
    caller_cargo_home=$(CDPATH= cd -- "$caller_cargo_home" && pwd -P) \
        || fatal 'could not resolve caller CARGO_HOME for cache reuse'
fi
if [[ -n "$caller_cargo_home" && -d "$caller_cargo_home/registry/cache" \
    && -d "$caller_cargo_home/registry/index" ]]; then
    mkdir -- "$isolated_cargo_home/registry" \
        || fatal 'could not create isolated Cargo registry directory'
    ln -s "$caller_cargo_home/registry/cache" "$isolated_cargo_home/registry/cache" \
        || fatal 'could not link the caller package-archive cache'
    ln -s "$caller_cargo_home/registry/index" "$isolated_cargo_home/registry/index" \
        || fatal 'could not link the caller registry-index cache'
    reused_registry_cache=1
else
    reused_registry_cache=0
fi
export CARGO_HOME="$isolated_cargo_home"

: >"$transcript" || fatal "could not create verification transcript: $transcript"
mkfifo -m 0600 "$transcript_fifo" \
    || fatal "could not create verification transcript pipe: $transcript_fifo"
exec 3>&1 4>&2
tee -a "$transcript" <"$transcript_fifo" &
transcript_tee_pid=$!
readonly transcript_tee_pid
exec >"$transcript_fifo" 2>&1

# Apple ships Bash 3.2, where an empty array expanded under `set -u` is
# considered unbound. Keep a permanent sentinel and an explicit count so that
# the verifier's cleanup path is safe before the first target is registered.
# Registered paths remain recorded after intermediate removal. This lets the
# finalizer detect a path that is unexpectedly recreated after its first
# cleanup rather than forgetting that path and reporting a false PASS.
owned_targets=('__no_target_registered__')
owned_target_count=0
normal_completion=0

register_target() {
    local target=$1
    case "$target" in
        "$verification_root"/target-*) ;;
        *) fatal "refusing to register a target outside the verification root: $target" ;;
    esac
    if [[ -e "$target" || -L "$target" ]]; then
        fatal "isolated Cargo target already exists: $target"
    fi
    owned_targets[$owned_target_count]=$target
    owned_target_count=$((owned_target_count + 1))
}

remove_owned_target() {
    local target=$1
    case "$target" in
        "$verification_root"/target-*) ;;
        *)
            printf 'WARNING: refusing to clean unexpected target path: %s\n' "$target" >&2
            return 1
            ;;
    esac
    if [[ -e "$target" || -L "$target" ]]; then
        if [[ -L "$target" || ! -d "$target" ]]; then
            printf 'WARNING: refusing to clean non-directory target path: %s\n' \
                "$target" >&2
            return 1
        fi
        find "$target" -depth -mindepth 1 -delete || return 1
        rmdir -- "$target" || return 1
    fi
    if [[ -e "$target" || -L "$target" ]]; then
        printf 'WARNING: isolated target remains after cleanup: %s\n' "$target" >&2
        return 1
    fi
    return 0
}

cleanup_registered_targets_now() {
    local index target cleanup_failed=0
    for ((index = 0; index < owned_target_count; index += 1)); do
        target=${owned_targets[$index]}
        if ! remove_owned_target "$target"; then
            cleanup_failed=1
        fi
    done
    ((cleanup_failed == 0))
}

cleanup_isolated_cargo_home() {
    local cleanup_failed=0
    case "$isolated_cargo_home" in
        "$verification_root"/cargo-home) ;;
        *) cleanup_failed=1 ;;
    esac
    if [[ -d "$isolated_cargo_home" && ! -L "$isolated_cargo_home" ]]; then
        if ! find "$isolated_cargo_home" -depth -mindepth 1 -delete \
            || ! rmdir -- "$isolated_cargo_home"; then
            cleanup_failed=1
        fi
    elif [[ -e "$isolated_cargo_home" || -L "$isolated_cargo_home" ]]; then
        cleanup_failed=1
    fi
    if [[ -e "$isolated_cargo_home" || -L "$isolated_cargo_home" ]]; then
        cleanup_failed=1
    fi
    ((cleanup_failed == 0))
}

attest_isolated_build_state_absent() {
    local index target remaining cleanup_failed=0
    for ((index = 0; index < owned_target_count; index += 1)); do
        target=${owned_targets[$index]}
        if [[ -e "$target" || -L "$target" ]]; then
            printf 'WARNING: registered isolated target survived or was recreated: %s\n' \
                "$target" >&2
            cleanup_failed=1
        fi
    done
    if [[ -e "$isolated_cargo_home" || -L "$isolated_cargo_home" ]]; then
        printf 'WARNING: isolated Cargo home survived or was recreated: %s\n' \
            "$isolated_cargo_home" >&2
        cleanup_failed=1
    fi
    # Registered paths are not the only relevant postcondition. Detect any
    # unregistered target-* entry directly beneath the private verification
    # root; a regular file or symlink must also fail closed.
    remaining=''
    for target in "$verification_root"/target-*; do
        if [[ -e "$target" || -L "$target" ]]; then
            remaining=$target
            break
        fi
    done
    if [[ -n "$remaining" ]]; then
        printf 'WARNING: unexpected isolated target entry remains: %s\n' \
            "$remaining" >&2
        cleanup_failed=1
    fi
    ((cleanup_failed == 0))
}

transcript_writer_is_active() {
    # Bash's running/stopped job filters exclude a completed-but-unwaited job
    # without requiring Bash 4's `wait -n`, GNU timeout(1), or
    # platform-specific ps(1) output.
    jobs -pr | grep -Fxq "$transcript_tee_pid" \
        || jobs -ps | grep -Fxq "$transcript_tee_pid"
}

finalize_transcript() {
    local transcript_failed=0 tee_status=0 fifo_unlink_failed=0
    local wait_seconds=0 grace_seconds=0 writer_timed_out=0
    # Close every writer to the named transcript pipe, wait for `tee`, and
    # restore the caller's original streams. A transcript or FIFO failure is a
    # release-gate failure, not a warning after a retained PASS summary.
    exec 1>&- 2>&-

    # A descendant accidentally left running by a build script or test can
    # inherit stdout/stderr and therefore retain a write end of the FIFO after
    # its direct parent exits. Never let that mistake hang final verification
    # indefinitely. A timed-out writer makes the transcript gate fail closed.
    while transcript_writer_is_active; do
        if ((wait_seconds >= transcript_finalize_timeout_seconds)); then
            writer_timed_out=1
            transcript_failed=1
            break
        fi
        if ! sleep 1; then
            writer_timed_out=1
            transcript_failed=1
            break
        fi
        wait_seconds=$((wait_seconds + 1))
    done
    if ((writer_timed_out != 0)); then
        kill -TERM "$transcript_tee_pid" 2>/dev/null || true
        while transcript_writer_is_active && ((grace_seconds < 2)); do
            sleep 1 || true
            grace_seconds=$((grace_seconds + 1))
        done
        if transcript_writer_is_active; then
            kill -KILL "$transcript_tee_pid" 2>/dev/null || true
        fi
    fi
    wait "$transcript_tee_pid"
    tee_status=$?
    if ((tee_status != 0)); then
        transcript_failed=1
    fi
    if ! unlink "$transcript_fifo"; then
        transcript_failed=1
        fifo_unlink_failed=1
    fi
    if [[ -e "$transcript_fifo" || -L "$transcript_fifo" ]]; then
        transcript_failed=1
        fifo_unlink_failed=1
    fi
    exec 1>&3 2>&4
    if ((writer_timed_out != 0)); then
        printf 'WARNING: verification transcript writer did not reach EOF within %d seconds; an inherited FIFO writer may remain\n' \
            "$transcript_finalize_timeout_seconds" >&2
    fi
    if ((tee_status != 0)); then
        printf 'WARNING: verification transcript writer failed with exit %d\n' \
            "$tee_status" >&2
    fi
    if ((fifo_unlink_failed != 0)); then
        printf 'WARNING: verification transcript pipe could not be removed: %s\n' \
            "$transcript_fifo" >&2
    fi
    ((transcript_failed == 0))
}

write_final_summary() {
    local summary_temporary
    if [[ -e "$summary" || -L "$summary" ]]; then
        printf 'refusing to replace an existing final verification summary: %s\n' \
            "$summary" >&2
        return 1
    fi
    summary_temporary=$(mktemp "$verification_root/.verification-summary.XXXXXXXX") \
        || return 1
    if [[ ! -f "$summary_temporary" || -L "$summary_temporary" ]]; then
        if [[ -f "$summary_temporary" && ! -L "$summary_temporary" ]]; then
            unlink "$summary_temporary" 2>/dev/null || true
        fi
        return 1
    fi
    if ! {
        printf 'VeritAsm local release-candidate verification\n'
        printf 'Overall: %s\n' "$final_status"
        printf 'Source ZIP: PASS\n'
        printf 'Source ZIP SHA-256: %s\n' "$actual_digest"
        printf 'Rust %s (MSRV; observed %s; host %s): %s\n' \
            "$msrv_toolchain" "$msrv_observed_release" "$msrv_observed_host" "$msrv_status"
        printf 'Rust %s channel as installed (observed %s; host %s): %s\n' \
            "$stable_toolchain" "$stable_observed_release" "$stable_observed_host" "$stable_status"
        printf 'Same-host smoke bundle equality: %s\n' "$smoke_equality_status"
        printf 'Fresh-source stable Cargo crate equality: %s\n' \
            "$crate_reproducibility_status"
        printf 'Local platform: %s\n' "$local_platform_status"
        printf 'Other required platform: %s\n' "$other_platform_status"
        printf 'Cross-platform deterministic equality: NOT RUN by this single-host verifier\n'
        printf 'Dependency audit: NOT RUN by this verifier\n'
        printf 'Sanitizer-backed fuzzing: NOT RUN by this verifier\n'
        printf 'Scientific validation/comparators: NOT RUN by this verifier\n'
        printf 'Isolated target/Cargo-home cleanup: %s\n' "$cleanup_status"
        printf 'Transcript finalization: %s\n' "$transcript_status"
        printf 'Verification root: %s\n' "$verification_root"
        printf 'Transcript: %s\n' "$transcript"
    } >"$summary_temporary"; then
        unlink "$summary_temporary" 2>/dev/null || true
        return 1
    fi
    if [[ ! -f "$summary_temporary" || -L "$summary_temporary" ]]; then
        unlink "$summary_temporary" 2>/dev/null || true
        return 1
    fi
    if ! ln -- "$summary_temporary" "$summary"; then
        unlink "$summary_temporary" 2>/dev/null || true
        return 1
    fi
    if [[ ! -f "$summary" || -L "$summary" \
        || ! "$summary_temporary" -ef "$summary" ]] \
        || ! cmp -s "$summary_temporary" "$summary"; then
        if [[ -f "$summary" && ! -L "$summary" \
            && "$summary_temporary" -ef "$summary" ]]; then
            unlink "$summary" 2>/dev/null || true
        fi
        unlink "$summary_temporary" 2>/dev/null || true
        return 1
    fi
    # The hard-link publication above is the no-replace atomic commit. Failure
    # to remove this now-redundant private name does not invalidate the exact
    # bytes at the authoritative summary path.
    if ! unlink "$summary_temporary"; then
        printf 'WARNING: redundant private summary link could not be removed: %s\n' \
            "$summary_temporary" >&2
    fi
    return 0
}

cleanup_targets() {
    local original_status=$?
    local cleanup_failed=0 transcript_failed=0 summary_failed=0
    trap - EXIT HUP INT TERM
    set +u
    if ! cleanup_registered_targets_now; then
        cleanup_failed=1
    fi
    if ! cleanup_isolated_cargo_home; then
        cleanup_failed=1
    fi
    if ! finalize_transcript; then
        transcript_failed=1
        if ((original_status < 128)); then
            original_status=1
        fi
    fi
    if ! attest_isolated_build_state_absent; then
        cleanup_failed=1
    fi
    if ((cleanup_failed != 0)); then
        printf 'WARNING: isolated Cargo build/cache state could not be cleaned and attested absent\n' >&2
        if ((original_status < 128)); then
            original_status=1
        fi
    fi

    if ((normal_completion == 1)); then
        if ((cleanup_failed == 0)); then
            cleanup_status='PASS'
        else
            cleanup_status='FAIL'
        fi
        if ((transcript_failed == 0)); then
            transcript_status='PASS'
        else
            transcript_status='FAIL'
        fi
        case "$original_status" in
            0) final_status='PASS' ;;
            2) final_status='INCOMPLETE' ;;
            129 | 130 | 143) final_status='INTERRUPTED' ;;
            *) final_status='FAIL' ;;
        esac
        if ! write_final_summary; then
            summary_failed=1
            printf 'FAIL: could not atomically write final verification summary: %s\n' \
                "$summary" >&2
            if ((original_status < 128)); then
                original_status=1
            fi
        fi
        printf '\n===== Verification summary =====\n'
        if ((summary_failed == 0)); then
            while IFS= read -r summary_line; do
                printf '%s\n' "$summary_line"
            done <"$summary"
        fi
        printf 'Retained evidence excludes isolated Cargo targets/cache state only when cleanup is PASS.\n'
    fi
    exec 3>&- 4>&-
    exit "$original_status"
}

interrupted() {
    local signal_status=$1
    printf '\nINTERRUPTED: verification artifacts retained at %s\n' \
        "$verification_root" >&2
    exit "$signal_status"
}

trap cleanup_targets EXIT
trap 'interrupted 129' HUP
trap 'interrupted 130' INT
trap 'interrupted 143' TERM

sha256_file() {
    local file=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{ print $1 }'
    else
        shasum -a 256 "$file" | awk '{ print $1 }'
    fi
}

preflight_source_zip_central_directory() {
    local archive=$1
    local archive_bytes=$2
    local listing=$3
    local inventory=$4
    local metrics
    local member_name inventory_records

    if ! unzip -Z -l "$archive" >"$listing"; then
        printf 'source ZIP preflight could not read the central directory\n' >&2
        return 1
    fi
    metrics=$(awk \
        -v observed_archive_bytes="$archive_bytes" \
        -v max_members="$max_source_zip_members" \
        -v max_compressed_bytes="$max_source_zip_archive_bytes" \
        -v max_member_bytes="$max_source_zip_member_bytes" \
        -v max_uncompressed_bytes="$max_source_zip_uncompressed_bytes" \
        -v max_ratio="$max_source_zip_expansion_ratio" '
        BEGIN {
            header_seen = 0
            header_archive_bytes = -1
            header_members = -1
            parsed_members = 0
            total_compressed = 0
            total_uncompressed = 0
            invalid = 0
        }
        !header_seen && $1 == "Zip" && $2 == "file" && $3 == "size:" &&
            $5 == "bytes," && $6 == "number" && $7 == "of" && $8 == "entries:" {
            if ($4 !~ /^[0-9]+$/ || $9 !~ /^[0-9]+$/) {
                invalid = 1
                next
            }
            header_archive_bytes = $4 + 0
            header_members = $9 + 0
            header_seen = 1
            next
        }
        $1 ~ /^[-dlcbps]/ {
            if ($4 !~ /^[0-9]+$/ || $6 !~ /^[0-9]+$/) {
                printf "source ZIP preflight found a malformed member-size row near member %d\n", parsed_members + 1 > "/dev/stderr"
                invalid = 1
                next
            }
            parsed_members++
            total_uncompressed += $4
            total_compressed += $6
            if (($6 + 0) > max_compressed_bytes) {
                printf "source ZIP preflight budget exceeded: member %d declares %s compressed bytes (limit %d)\n", parsed_members, $6, max_compressed_bytes > "/dev/stderr"
                invalid = 1
            }
            if (($4 + 0) > max_member_bytes) {
                printf "source ZIP preflight budget exceeded: member %d declares %s uncompressed bytes (limit %d)\n", parsed_members, $4, max_member_bytes > "/dev/stderr"
                invalid = 1
            }
        }
        END {
            if (!header_seen || invalid) {
                if (!header_seen || invalid && parsed_members == 0) {
                    print "source ZIP preflight found an unreadable central-directory size table" > "/dev/stderr"
                }
                exit 1
            }
            if (header_archive_bytes != observed_archive_bytes) {
                printf "source ZIP preflight size mismatch: central directory reports %.0f bytes, file has %.0f\n", header_archive_bytes, observed_archive_bytes > "/dev/stderr"
                exit 1
            }
            if (header_members == 0 || parsed_members != header_members) {
                printf "source ZIP preflight member-count mismatch: header %.0f, size table %.0f\n", header_members, parsed_members > "/dev/stderr"
                exit 1
            }
            if (header_members > max_members) {
                printf "source ZIP preflight budget exceeded: %.0f members (limit %d)\n", header_members, max_members > "/dev/stderr"
                exit 1
            }
            if (total_compressed > max_compressed_bytes) {
                printf "source ZIP preflight budget exceeded: %.0f declared compressed bytes (limit %d)\n", total_compressed, max_compressed_bytes > "/dev/stderr"
                exit 1
            }
            if (total_compressed > observed_archive_bytes) {
                printf "source ZIP preflight size mismatch: %.0f declared compressed bytes exceed %.0f archive bytes\n", total_compressed, observed_archive_bytes > "/dev/stderr"
                exit 1
            }
            if (total_uncompressed > max_uncompressed_bytes) {
                printf "source ZIP preflight budget exceeded: %.0f declared uncompressed bytes (limit %d)\n", total_uncompressed, max_uncompressed_bytes > "/dev/stderr"
                exit 1
            }
            if (total_uncompressed > 0) {
                if (total_compressed == 0) {
                    printf "source ZIP preflight budget exceeded: nonempty declared output has zero compressed bytes (infinite expansion ratio)\n" > "/dev/stderr"
                    exit 1
                }
                if (total_uncompressed > max_ratio * total_compressed) {
                    printf "source ZIP preflight budget exceeded: declared expansion %.0f/%.0f bytes is greater than %d:1\n", total_uncompressed, total_compressed, max_ratio > "/dev/stderr"
                    exit 1
                }
            }
            printf "%.0f\t%.0f\t%.0f\n", header_members, total_compressed, total_uncompressed
        }
    ' "$listing") || return 1

    IFS=$'\t' read -r source_zip_member_count source_zip_compressed_bytes \
        source_zip_uncompressed_bytes <<<"$metrics" || return 1
    case "$source_zip_member_count:$source_zip_compressed_bytes:$source_zip_uncompressed_bytes" in
        *[!0-9:]* | :* | *: | *::* )
            printf 'source ZIP preflight produced invalid size metrics: %s\n' "$metrics" >&2
            return 1
            ;;
    esac

    # Inventory inspection is part of the central-directory preflight, not a
    # post-integrity check. Under LC_ALL=C, Info-ZIP renders CR/LF filename
    # bytes as ^M/^J. A raw LF instead creates too many newline-delimited
    # records, while a raw CR is checked explicitly. Literal caret spellings
    # are outside the portable path policy too, so rejecting these encodings is
    # intentionally fail-closed.
    if ! unzip -Z1 "$archive" >"$inventory"; then
        printf 'source ZIP preflight could not read member names from the central directory\n' >&2
        return 1
    fi
    [[ -s "$inventory" ]] || {
        printf 'source ZIP preflight found an empty member-name inventory\n' >&2
        return 1
    }
    inventory_records=$(wc -l <"$inventory" | tr -d ' ') || return 1
    case "$inventory_records" in
        '' | *[!0-9]*)
            printf 'source ZIP preflight produced an invalid member-name record count\n' >&2
            return 1
            ;;
    esac
    if [[ "$inventory_records" != "$source_zip_member_count" ]]; then
        printf 'source ZIP preflight found %s newline-delimited member names for %s central-directory members; newline-bearing names are forbidden\n' \
            "$inventory_records" "$source_zip_member_count" >&2
        return 1
    fi
    while IFS= read -r member_name || [[ -n "$member_name" ]]; do
        case "$member_name" in
            *'^J'* | *'^M'* | *$'\r'*)
                printf 'source ZIP preflight found a newline-bearing member name: %s\n' \
                    "$member_name" >&2
                return 1
                ;;
        esac
    done <"$inventory"
    return 0
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
    local remainder component base lowercase_base lowercase_relative
    validate_portable_relative_path "$relative" || return 1

    lowercase_relative=$(printf '%s' "$relative" | tr '[:upper:]' '[:lower:]') \
        || return 1
    remainder=$lowercase_relative
    while :; do
        component=${remainder%%/*}
        case "$component" in
            .git | target | temp | tmp | result | results | credentials | secrets)
                return 1
                ;;
        esac
        [[ "$remainder" == */* ]] || break
        remainder=${remainder#*/}
    done

    base=${lowercase_relative##*/}
    lowercase_base=$(printf '%s' "$base" | tr '[:upper:]' '[:lower:]') || return 1
    case "$lowercase_base" in
        *.zip | *.sha256 | *.pem | *.key | *.p12 | *.pfx | *.token | \
            .env | .env.* | credentials | credentials.* | secrets | secrets.* | \
            id_rsa | id_ed25519)
            return 1
            ;;
    esac
    return 0
}

validate_archive_entry() {
    local entry=$1
    local relative
    case "$entry" in
        "$archive_root"/*) relative=${entry#"$archive_root/"} ;;
        *) return 1 ;;
    esac
    validate_relative_path "$relative"
}

step() {
    local description=$1
    shift
    printf '\n[RUN] %s\n' "$description"
    "$@"
    local status=$?
    if ((status != 0)); then
        printf '[FAIL] %s (exit %d)\n' "$description" "$status" >&2
        return "$status"
    fi
    printf '[PASS] %s\n' "$description"
}

toolchain_available() {
    local toolchain=$1
    command -v rustup >/dev/null 2>&1 \
        && rustup run "$toolchain" cargo --version >/dev/null 2>&1 \
        && rustup run "$toolchain" rustc --version >/dev/null 2>&1
}

rustc_release_for() {
    local toolchain=$1
    rustup run "$toolchain" rustc --version --verbose \
        | awk '$1 == "release:" { print $2; found = 1 } END { if (!found) exit 1 }'
}

rustc_host_for() {
    local toolchain=$1
    rustup run "$toolchain" rustc --version --verbose \
        | awk '$1 == "host:" { print $2; found = 1 } END { if (!found) exit 1 }'
}

manifest_package_field() {
    local manifest=$1
    local field=$2
    awk -v requested="$field" '
        /^\[package\]$/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && $0 ~ ("^" requested "[[:space:]]*=") {
            value = $0
            sub(/^[^=]*=[[:space:]]*"/, "", value)
            sub(/"[[:space:]]*$/, "", value)
            print value
            found = 1
            exit
        }
        END { if (!found) exit 1 }
    ' "$manifest"
}

stable_release_supported() {
    local release=$1
    printf '%s\n' "$release" | awk -F '.' '
        NF == 3 && $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ &&
        ($1 > 1 || ($1 == 1 && $2 >= 85)) { valid = 1 }
        END { if (!valid) exit 1 }
    ' >/dev/null
}

cargo_for() {
    local toolchain=$1
    local target=$2
    shift 2
    CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$target" \
        rustup run "$toolchain" cargo "$@"
}

cargo_doc_for() {
    local toolchain=$1
    local target=$2
    shift 2
    CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$target" RUSTDOCFLAGS='-D warnings' \
        rustup run "$toolchain" cargo "$@"
}

tree_manifest() {
    local tree_root=$1
    local output=$2
    local path_profile=${3:-source}
    local path_validator relative digest unexpected temporary
    case "$path_profile" in
        source) path_validator=validate_relative_path ;;
        generated-output) path_validator=validate_portable_relative_path ;;
        *)
            printf 'unknown tree-manifest path profile: %s\n' "$path_profile" >&2
            return 1
            ;;
    esac
    unexpected=$(find "$tree_root" ! -type d ! -type f -print | sed -n '1p') || return 1
    if [[ -n "$unexpected" ]]; then
        printf 'source tree contains a non-regular entry: %s\n' "$unexpected" >&2
        return 1
    fi
    temporary=$(mktemp "${output}.temporary.XXXXXXXX") || return 1
    if (
        cd -- "$tree_root" || exit 1
        find . -type f -print | sed 's#^\./##' | sort
    ) | while IFS= read -r relative; do
        if ! "$path_validator" "$relative"; then
            printf '%s tree contains an invalid relative path: %s\n' \
                "$path_profile" "$relative" >&2
            exit 1
        fi
        digest=$(sha256_file "$tree_root/$relative") || exit 1
        printf '%s  %s\n' "$digest" "$relative"
    done >"$temporary"; then
        if ! mv -- "$temporary" "$output"; then
            unlink "$temporary" 2>/dev/null || true
            return 1
        fi
    else
        unlink "$temporary" 2>/dev/null || true
        return 1
    fi
}

reject_ancestor_cargo_config() {
    local project=$1
    local directory
    directory=$(CDPATH= cd -- "$project" && pwd -P) || return 1
    while :; do
        if [[ -e "$directory/.cargo/config" || -L "$directory/.cargo/config" \
            || -e "$directory/.cargo/config.toml" || -L "$directory/.cargo/config.toml" ]]; then
            printf 'ambient Cargo config would affect verification: %s/.cargo\n' \
                "$directory" >&2
            return 1
        fi
        [[ "$directory" != '/' ]] || break
        directory=$(dirname -- "$directory") || return 1
    done
    return 0
}

verify_bundle_manifest() {
    local bundle=$1
    local parsed="$verification_root/manifest-parsed.$$.txt"
    local declared_inventory="$verification_root/manifest-declared.$$.txt"
    local expected_inventory="$verification_root/manifest-expected.$$.txt"
    local actual_inventory="$verification_root/manifest-actual.$$.txt"
    local normative_inventory="$verification_root/manifest-normative.$$.txt"
    local unexpected="$verification_root/manifest-unexpected.$$.txt"
    local expected relative actual

    if [[ ! -f "$bundle/manifest.sha256" || -L "$bundle/manifest.sha256" ]]; then
        printf 'missing regular manifest: %s/manifest.sha256\n' "$bundle" >&2
        return 1
    fi
    (
        cd -- "$bundle" || exit 1
        find . ! -type d ! -type f -print
    ) >"$unexpected" || return 1
    if [[ -s "$unexpected" ]]; then
        printf 'bundle contains a non-regular entry:\n' >&2
        sed -n '1,20p' "$unexpected" >&2
        return 1
    fi

    if ! awk '
        BEGIN { valid = 1 }
        {
            if (length($1) != 64 || $1 !~ /^[0-9a-f]+$/ || NF != 2 ||
                $0 != $1 "  " $2) {
                valid = 0
                exit
            }
            print $1 "\t" $2
        }
        END {
            if (NR == 0 || valid == 0) exit 1
        }
    ' "$bundle/manifest.sha256" >"$parsed"; then
        printf 'manifest has an invalid line or is empty: %s\n' \
            "$bundle/manifest.sha256" >&2
        return 1
    fi

    awk -F '\t' '{ print $2 }' "$parsed" >"$declared_inventory" || return 1
    if [[ $(sort "$declared_inventory" | uniq -d | wc -l | tr -d ' ') != 0 ]]; then
        printf 'manifest contains duplicate paths: %s\n' "$bundle/manifest.sha256" >&2
        return 1
    fi
    sort "$declared_inventory" >"$expected_inventory" || return 1
    if ! cmp -s "$declared_inventory" "$expected_inventory"; then
        printf 'manifest paths are not sorted by relative-path bytes: %s\n' \
            "$bundle/manifest.sha256" >&2
        return 1
    fi
    (
        cd -- "$bundle" || exit 1
        find . -type f -print | sed 's#^\./##' | grep -v '^manifest\.sha256$' | sort
    ) >"$actual_inventory" || return 1
    if ! cmp -s "$expected_inventory" "$actual_inventory"; then
        printf 'bundle inventory differs from manifest:\n' >&2
        diff -u "$expected_inventory" "$actual_inventory" >&2 || true
        return 1
    fi

    # Stable assembly-contract v0.1 commits this exact artifact inventory.
    # The manifest verifier above proves internal consistency; this additional
    # comparison prevents two toolchains from agreeing on the same accidental
    # extra or missing artifact.
    printf '%s\n' \
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
        'unitigs.fasta' >"$normative_inventory" || return 1
    if ! cmp -s "$normative_inventory" "$actual_inventory"; then
        printf 'bundle inventory differs from stable assembly-contract v0.1:\n' >&2
        diff -u "$normative_inventory" "$actual_inventory" >&2 || true
        return 1
    fi

    while IFS=$'\t' read -r expected relative; do
        validate_relative_path "$relative" || {
            printf 'unsafe manifest path: %s\n' "$relative" >&2
            return 1
        }
        [[ "$relative" != 'manifest.sha256' ]] || {
            printf 'manifest must not list itself\n' >&2
            return 1
        }
        [[ -f "$bundle/$relative" && ! -L "$bundle/$relative" ]] || {
            printf 'manifest target is missing or not regular: %s\n' "$relative" >&2
            return 1
        }
        actual=$(sha256_file "$bundle/$relative") || return 1
        if [[ "$actual" != "$expected" ]]; then
            printf 'manifest digest mismatch for %s: expected %s, observed %s\n' \
                "$relative" "$expected" "$actual" >&2
            return 1
        fi
    done <"$parsed"
    return 0
}

verify_crate_archive() {
    local label=$1
    local toolchain=$2
    local crate_path=$3
    local crate_list="$verification_root/crate-inventory-$label.txt"
    local crate_extract="$verification_root/crate-extract-$label"
    local crate_project="$crate_extract/$archive_root"
    local crate_check_target="$verification_root/target-crate-check-$label"
    local crate_test_target="$verification_root/target-crate-test-$label"
    local crate_release_target="$verification_root/target-crate-release-$label"
    local entry stripped relative unexpected

    mkdir -- "$crate_extract" || return 1
    tar -tzf "$crate_path" >"$crate_list" || return 1
    [[ -s "$crate_list" ]] || return 1
    if [[ $(sort "$crate_list" | uniq -d | wc -l | tr -d ' ') != 0 ]]; then
        printf 'Cargo crate contains duplicate archive paths: %s\n' "$crate_path" >&2
        return 1
    fi
    while IFS= read -r entry || [[ -n "$entry" ]]; do
        stripped=${entry%/}
        case "$stripped" in
            "$archive_root"/*) relative=${stripped#"$archive_root/"} ;;
            "$archive_root") continue ;;
            *)
                printf 'Cargo crate entry escapes required root: %s\n' "$entry" >&2
                return 1
                ;;
        esac
        validate_relative_path "$relative" || {
            printf 'unsafe Cargo crate entry: %s\n' "$entry" >&2
            return 1
        }
    done <"$crate_list"
    tar -xzf "$crate_path" -C "$crate_extract" || return 1
    [[ -f "$crate_project/Cargo.toml" && -f "$crate_project/Cargo.lock" ]] || return 1
    unexpected=$(find "$crate_extract" ! -type d ! -type f -print | sed -n '1p') || return 1
    if [[ -n "$unexpected" ]]; then
        printf 'Cargo crate extraction contains a non-regular entry: %s\n' \
            "$unexpected" >&2
        return 1
    fi

    register_target "$crate_check_target"
    (
        cd -- "$crate_project" || exit 1
        step "$label extracted .crate check" \
            cargo_for "$toolchain" "$crate_check_target" \
            check --locked --all-targets --all-features || exit 1
    ) || return 1
    remove_owned_target "$crate_check_target" || return 1

    # Tests, doctests, and documentation start from a new target. In
    # particular, this prevents Cargo 1.85 check artifacts with non-executable
    # top-level binary modes from contaminating CLI test process launches.
    register_target "$crate_test_target"
    (
        cd -- "$crate_project" || exit 1
        step "$label extracted .crate tests" \
            cargo_for "$toolchain" "$crate_test_target" \
            test --locked --all-targets --all-features || exit 1
        step "$label extracted .crate doctests" \
            cargo_for "$toolchain" "$crate_test_target" test --locked --doc || exit 1
        step "$label extracted .crate documentation" \
            cargo_doc_for "$toolchain" "$crate_test_target" doc --locked --no-deps || exit 1
    ) || return 1
    remove_owned_target "$crate_test_target" || return 1

    # The release-link check starts from an empty target and therefore cannot
    # inherit debug/test/doc target state from the preceding gates.
    register_target "$crate_release_target"
    (
        cd -- "$crate_project" || exit 1
        step "$label extracted .crate release build" \
            cargo_for "$toolchain" "$crate_release_target" \
            build --locked --release || exit 1
    ) || return 1
    remove_owned_target "$crate_release_target" || return 1
    return 0
}

run_smoke_assemblies() {
    local label=$1
    local target=$2
    local project=$3
    local binary="$target/release/veritasm"
    local se_output="$verification_root/smoke-$label-se"
    local pe_output="$verification_root/smoke-$label-pe-gzip"
    local smoke_input_root="$verification_root/smoke-input-$label"
    local read1="$smoke_input_root/lane_R1.fastx.bin"
    local read2="$smoke_input_root/lane_R2.fastx.bin"
    local smoke_threads

    case "$label" in
        msrv) smoke_threads=1 ;;
        stable) smoke_threads=4 ;;
        *) return 1 ;;
    esac

    [[ -x "$binary" ]] || {
        printf 'release executable is absent from isolated target: %s\n' "$binary" >&2
        return 1
    }
    [[ ! -e "$se_output" && ! -L "$se_output" ]] || return 1
    [[ ! -e "$pe_output" && ! -L "$pe_output" ]] || return 1
    mkdir -- "$smoke_input_root" || return 1
    cp -- "$project/examples/data/basic_paired/reads_R1.fastq.gz" "$read1" || return 1
    cp -- "$project/examples/data/basic_paired/reads_R2.fastq.gz" "$read2" || return 1

    step "$label single-end FASTA smoke assembly" \
        "$binary" assemble \
        --single "$project/examples/reads.fasta" \
        --output-dir "$se_output" \
        --k 5 \
        --profile retain-all \
        --min-base-quality 0 \
        --threads "$smoke_threads" || return 1
    step "$label single-end bundle manifest and inventory" \
        verify_bundle_manifest "$se_output" || return 1

    step "$label content-detected-gzip paired-end smoke assembly" \
        "$binary" assemble \
        --read1 "$read1" \
        --read2 "$read2" \
        --output-dir "$pe_output" \
        --k 31 \
        --profile retain-all \
        --min-base-quality 0 \
        --threads "$smoke_threads" || return 1
    step "$label paired-end bundle manifest and inventory" \
        verify_bundle_manifest "$pe_output" || return 1
    return 0
}

run_toolchain_verification() {
    local label=$1
    local toolchain=$2
    local project=$3
    local source_tree_manifest=$4
    local source_check_target="$verification_root/target-source-check-$label"
    local source_test_target="$verification_root/target-source-test-$label"
    local source_release_target="$verification_root/target-source-release-$label"
    local crate_source crate_copy crate_digest post_tree_manifest actual_release actual_host

    note ''
    note "===== $label toolchain: $toolchain ====="
    actual_release=$(rustc_release_for "$toolchain") || {
        printf 'could not determine rustc release for toolchain %s\n' "$toolchain" >&2
        return 1
    }
    actual_host=$(rustc_host_for "$toolchain") || {
        printf 'could not determine rustc host for toolchain %s\n' "$toolchain" >&2
        return 1
    }
    case "$label" in
        msrv)
            msrv_observed_release=$actual_release
            msrv_observed_host=$actual_host
            ;;
        stable)
            stable_observed_release=$actual_release
            stable_observed_host=$actual_host
            ;;
        *) return 1 ;;
    esac
    if [[ -z "$expected_rust_host" || "$actual_host" != "$expected_rust_host" ]]; then
        printf 'toolchain %s resolves to host %s, expected %s for %s %s\n' \
            "$toolchain" "$actual_host" "${expected_rust_host:-unsupported-host}" \
            "$host_kernel" "$host_arch" >&2
        return 1
    fi
    case "$label" in
        msrv)
            if [[ "$actual_release" != '1.85.0' ]]; then
                printf 'MSRV toolchain %s resolves to rustc %s, not 1.85.0\n' \
                    "$toolchain" "$actual_release" >&2
                return 1
            fi
            ;;
        stable)
            if ! stable_release_supported "$actual_release"; then
                printf 'stable toolchain resolves to unsupported or non-release rustc %s\n' \
                    "$actual_release" >&2
                return 1
            fi
            ;;
    esac
    rustup run "$toolchain" cargo --version || return 1
    rustup run "$toolchain" rustc --version --verbose || return 1

    register_target "$source_check_target"
    (
        cd -- "$project" || exit 1
        step "$label formatting" \
            cargo_for "$toolchain" "$source_check_target" fmt --all -- --check || exit 1
        step "$label syntax and all-target compilation check" \
            cargo_for "$toolchain" "$source_check_target" \
            check --locked --all-targets --all-features || exit 1
        step "$label Clippy with warnings denied" \
            cargo_for "$toolchain" "$source_check_target" \
            clippy --locked --all-targets --all-features -- -D warnings || exit 1
    ) || return 1
    remove_owned_target "$source_check_target" || return 1

    register_target "$source_test_target"
    (
        cd -- "$project" || exit 1
        step "$label all-target tests" \
            cargo_for "$toolchain" "$source_test_target" \
            test --locked --all-targets --all-features || exit 1
        step "$label doctests" \
            cargo_for "$toolchain" "$source_test_target" test --locked --doc || exit 1
        step "$label documentation with warnings denied" \
            cargo_doc_for "$toolchain" "$source_test_target" doc --locked --no-deps || exit 1
    ) || return 1
    remove_owned_target "$source_test_target" || return 1

    register_target "$source_release_target"
    (
        cd -- "$project" || exit 1
        step "$label release build" \
            cargo_for "$toolchain" "$source_release_target" build --locked --release || exit 1
        step "$label Cargo package and Cargo's package verification" \
            cargo_for "$toolchain" "$source_release_target" package --locked --allow-dirty || exit 1
    ) || return 1

    crate_source="$source_release_target/package/${package_name}-${package_version}.crate"
    [[ -f "$crate_source" && ! -L "$crate_source" ]] || {
        printf 'Cargo did not produce the expected crate: %s\n' "$crate_source" >&2
        return 1
    }
    crate_copy="$crate_store/${package_name}-${package_version}-$label.crate"
    [[ ! -e "$crate_copy" && ! -L "$crate_copy" ]] || return 1
    cp -- "$crate_source" "$crate_copy" || return 1
    crate_digest=$(sha256_file "$crate_copy") || return 1
    printf '%s  %s\n' "$crate_digest" "${crate_copy##*/}" \
        >"$crate_copy.sha256" || return 1
    note "Generated crate SHA-256 ($label): $crate_digest"

    run_smoke_assemblies "$label" "$source_release_target" "$project" || return 1
    # The crate was copied and both source-built smoke binaries have run. Drop
    # this large target before compiling the independently extracted crate so
    # verification remains practical on storage-constrained machines.
    remove_owned_target "$source_release_target" || return 1
    verify_crate_archive "$label" "$toolchain" "$crate_copy" || return 1

    post_tree_manifest="$verification_root/source-tree-$label-after.txt"
    tree_manifest "$project" "$post_tree_manifest" || return 1
    if ! cmp -s "$source_tree_manifest" "$post_tree_manifest"; then
        printf 'source extraction changed while running %s gates:\n' "$label" >&2
        diff -u "$source_tree_manifest" "$post_tree_manifest" >&2 || true
        return 1
    fi
    step "$label extracted source remained byte-identical" \
        cmp -s "$source_tree_manifest" "$post_tree_manifest" || return 1
    return 0
}

build_repeat_stable_crate() {
    local project=$1
    local source_tree_manifest=$2
    local repeat_target="$verification_root/target-source-package-stable-repeat"
    local crate_source="$repeat_target/package/${package_name}-${package_version}.crate"
    local crate_copy="$crate_store/${package_name}-${package_version}-stable-repeat.crate"
    local crate_digest post_tree_manifest

    register_target "$repeat_target"
    (
        cd -- "$project" || exit 1
        step 'stable repeat Cargo package from the other fresh source extraction' \
            cargo_for "$stable_toolchain" "$repeat_target" \
            package --locked --allow-dirty --no-verify || exit 1
    ) || return 1
    [[ -f "$crate_source" && ! -L "$crate_source" ]] || {
        printf 'Cargo did not produce the expected repeat crate: %s\n' "$crate_source" >&2
        return 1
    }
    [[ ! -e "$crate_copy" && ! -L "$crate_copy" ]] || return 1
    cp -- "$crate_source" "$crate_copy" || return 1
    crate_digest=$(sha256_file "$crate_copy") || return 1
    printf '%s  %s\n' "$crate_digest" "${crate_copy##*/}" \
        >"$crate_copy.sha256" || return 1
    note "Generated repeat stable crate SHA-256: $crate_digest"
    remove_owned_target "$repeat_target" || return 1

    post_tree_manifest="$verification_root/source-tree-stable-repeat-after.txt"
    tree_manifest "$project" "$post_tree_manifest" || return 1
    if ! cmp -s "$source_tree_manifest" "$post_tree_manifest"; then
        printf 'source extraction changed while building the repeat stable crate:\n' >&2
        diff -u "$source_tree_manifest" "$post_tree_manifest" >&2 || true
        return 1
    fi
    return 0
}

sealed_input_root="$verification_root/sealed-input"
mkdir -- "$sealed_input_root" || fatal 'could not create sealed-input directory'
verify_supplied_input_identity 'before bounded sealing copy' \
    || fatal 'supplied source ZIP or checksum sidecar changed before sealing'
# Copy at most one 64-KiB block beyond the archive budget and one byte beyond
# the sidecar budget. The size checks below reject those sentinel overages, so
# an oversized or concurrently growing input cannot fill the work filesystem.
if ! dd bs="$bounded_copy_block_bytes" \
    count="$((max_source_zip_archive_bytes / bounded_copy_block_bytes + 1))" <&5 \
    >"$sealed_input_root/$archive_basename"; then
    fatal 'could not copy anchored source ZIP into the verification root'
fi
if ! dd bs="$((max_checksum_sidecar_bytes + 1))" count=1 <&6 \
    >"$sealed_input_root/${archive_basename}.sha256"; then
    fatal 'could not copy anchored checksum sidecar into the verification root'
fi
verify_supplied_input_identity 'after bounded sealing copy' \
    || fatal 'supplied source ZIP or checksum sidecar changed while sealing'
# On Darwin, the dd reads above advance these exact open file descriptions.
# Close them now; no later operation may attempt to consume them again.
if ! exec 5<&- 6<&-; then
    fatal 'could not close supplied-input anchor descriptors after sealing'
fi
archive_path="$sealed_input_root/$archive_basename"
checksum_path="$sealed_input_root/${archive_basename}.sha256"
[[ -f "$archive_path" && ! -L "$archive_path" \
    && -f "$checksum_path" && ! -L "$checksum_path" ]] \
    || fatal 'sealed input copies are not regular files'

note "Verification root: $verification_root"
note "Supplied source ZIP: $supplied_archive_path"
note "Sealed source ZIP copy: $archive_path"
note "Supplied checksum sidecar: $supplied_checksum_path"
note 'No checkout target directory or pre-existing executable will be used.'
if ((reused_registry_cache == 1)); then
    note 'Cargo configuration/source extraction is isolated; registry index/package archives are shared cache-backed state.'
else
    note 'Cargo configuration, registry state, and dependency source extraction use a fresh CARGO_HOME.'
fi

archive_bytes=$(wc -c <"$archive_path" | tr -d ' ') \
    || fatal 'could not determine source ZIP size'
checksum_bytes=$(wc -c <"$checksum_path" | tr -d ' ') \
    || fatal 'could not determine sealed checksum sidecar size'
case "$archive_bytes:$checksum_bytes" in
    *[!0-9:]* | :* | *: | *::* )
        fatal 'sealed input size inspection did not produce decimal byte counts'
        ;;
esac
if ((archive_bytes > max_source_zip_archive_bytes)); then
    fatal "source ZIP preflight budget exceeded: $archive_bytes archive bytes (limit $max_source_zip_archive_bytes)"
fi
if ((checksum_bytes == 0 || checksum_bytes > max_checksum_sidecar_bytes)); then
    fatal "checksum sidecar preflight budget exceeded after sealing: $checksum_bytes bytes (required 1..$max_checksum_sidecar_bytes)"
fi
expected_digest=$(awk -v expected_name="$archive_basename" '
    BEGIN { valid = 1 }
    {
        if (NR != 1 || length($1) != 64 || $1 !~ /^[0-9a-f]+$/ ||
            NF != 2 || $2 != expected_name || $0 != $1 "  " $2) {
            valid = 0
            exit
        }
        print $1
    }
    END {
        if (NR != 1 || valid == 0) exit 1
    }
' "$checksum_path") || fatal 'checksum sidecar is not one strict lowercase SHA-256 record'
actual_digest=$(sha256_file "$archive_path") || fatal 'could not hash source ZIP'
if [[ "$actual_digest" != "$expected_digest" ]]; then
    fatal "source ZIP SHA-256 mismatch: expected $expected_digest, observed $actual_digest"
fi
note "[PASS] source ZIP SHA-256: $actual_digest"

central_directory_listing="$verification_root/archive-central-directory.txt"
actual_archive_inventory="$verification_root/archive-inventory.txt"
if ! preflight_source_zip_central_directory \
    "$archive_path" "$archive_bytes" "$central_directory_listing" \
    "$actual_archive_inventory"; then
    fatal 'source ZIP central-directory preflight failed before decompression'
fi
note "[PASS] source ZIP preflight: $source_zip_member_count members, $archive_bytes archive bytes, $source_zip_compressed_bytes declared compressed bytes, $source_zip_uncompressed_bytes declared uncompressed bytes"

sorted_archive_inventory="$verification_root/archive-inventory-sorted.txt"
expected_archive_inventory="$verification_root/archive-inventory-expected.txt"
typed_archive_inventory="$verification_root/archive-inventory-regular-members.txt"
internal_allowlist="$verification_root/source-package-files.txt"
archive_inventory_records=$(wc -l <"$actual_archive_inventory" | tr -d ' ') \
    || fatal 'could not count source ZIP inventory records'
if [[ "$archive_inventory_records" != "$source_zip_member_count" ]]; then
    fatal "source ZIP inventory has $archive_inventory_records newline-delimited records for $source_zip_member_count central-directory members; embedded newlines are forbidden"
fi
if [[ $(sort "$actual_archive_inventory" | uniq -d | wc -l | tr -d ' ') != 0 ]]; then
    fatal 'source ZIP contains duplicate paths'
fi
if ! awk '
    {
        folded = tolower($0)
        if (seen[folded]++) exit 1
    }
' "$actual_archive_inventory"; then
    fatal 'source ZIP contains case-fold-colliding paths'
fi
if ! awk '
    $1 ~ /^[-dlcbps]/ {
        if (substr($1, 1, 1) != "-") invalid = 1
        print $NF
        count++
    }
    END {
        if (count == 0 || invalid) exit 1
    }
' "$central_directory_listing" >"$typed_archive_inventory"; then
    fatal 'source ZIP contains a non-regular member or unreadable member type'
fi
if ! cmp -s "$actual_archive_inventory" "$typed_archive_inventory"; then
    fatal 'typed source ZIP member inventory differs from central-directory inventory'
fi
while IFS= read -r archived_path || [[ -n "$archived_path" ]]; do
    validate_archive_entry "$archived_path" \
        || fatal "source ZIP contains an unsafe or unexpected entry: $archived_path"
done <"$actual_archive_inventory"
if ! unzip -tq "$archive_path" >/dev/null; then
    fatal 'source ZIP integrity check failed'
fi
note '[PASS] source ZIP integrity'

if ! unzip -p "$archive_path" \
    "$archive_root/scripts/source-package-files.txt" >"$internal_allowlist"; then
    fatal 'could not read source-package allowlist from ZIP'
fi
[[ -s "$internal_allowlist" ]] || fatal 'source-package allowlist in ZIP is empty'
if [[ ! -f "$repository_root/scripts/source-package-files.txt" \
    || -L "$repository_root/scripts/source-package-files.txt" ]]; then
    fatal 'producing checkout lacks a regular source-package allowlist'
fi
if ! cmp -s "$internal_allowlist" "$repository_root/scripts/source-package-files.txt"; then
    fatal 'ZIP source-package allowlist differs from the producing checkout'
fi
if ! unzip -p "$archive_path" \
    "$archive_root/scripts/verify_release_candidate.sh" \
    | cmp -s - "$repository_root/scripts/verify_release_candidate.sh"; then
    fatal 'ZIP release verifier differs from the producing checkout'
fi
sort -u "$internal_allowlist" >"$verification_root/source-package-files.sorted.txt"
if ! cmp -s "$internal_allowlist" \
    "$verification_root/source-package-files.sorted.txt"; then
    fatal 'source-package allowlist in ZIP is not sorted and unique'
fi
while IFS= read -r relative || [[ -n "$relative" ]]; do
    validate_relative_path "$relative" \
        || fatal "unsafe path in source-package allowlist: $relative"
    case "$relative" in
        .github/* | docs/* | examples/* | fuzz/* | proptest-regressions/* | schema/* | \
            scripts/* | src/* | tests/*) ;;
        *) fatal "path outside allowed source directories: $relative" ;;
    esac
done <"$internal_allowlist"

: >"$expected_archive_inventory"
for required_root_file in \
    .gitignore AGENTS.md ARCHITECTURE.md BENCHMARK.md CHANGELOG.md CITATION.cff \
    CONTRIBUTING.md Cargo.lock Cargo.toml LICENSE README.md RESEARCH.md SECURITY.md \
    VALIDATION.md deny.toml rustfmt.toml; do
    printf '%s/%s\n' "$archive_root" "$required_root_file" \
        >>"$expected_archive_inventory"
done
while IFS= read -r relative || [[ -n "$relative" ]]; do
    printf '%s/%s\n' "$archive_root" "$relative" >>"$expected_archive_inventory"
done <"$internal_allowlist"
sort -u -o "$expected_archive_inventory" "$expected_archive_inventory"
sort "$actual_archive_inventory" >"$sorted_archive_inventory"
if ! cmp -s "$expected_archive_inventory" "$sorted_archive_inventory"; then
    printf 'source ZIP inventory differs from its declared exact inventory:\n' >&2
    diff -u "$expected_archive_inventory" "$sorted_archive_inventory" >&2 || true
    fatal 'source ZIP inventory verification failed'
fi
note "[PASS] exact source ZIP inventory ($(wc -l <"$expected_archive_inventory" | tr -d ' ') files)"

extract_one="$verification_root/extraction-msrv"
extract_two="$verification_root/extraction-stable"
mkdir -- "$extract_one" "$extract_two" || fatal 'could not create extraction directories'
unzip -q "$archive_path" -d "$extract_one" || fatal 'first source ZIP extraction failed'
unzip -q "$archive_path" -d "$extract_two" || fatal 'second source ZIP extraction failed'
project_one="$extract_one/$archive_root"
project_two="$extract_two/$archive_root"
[[ -f "$project_one/Cargo.toml" && -f "$project_two/Cargo.toml" ]] \
    || fatal 'an extracted source root lacks Cargo.toml'
for extracted_project in "$project_one" "$project_two"; do
    extracted_name=$(manifest_package_field "$extracted_project/Cargo.toml" name) \
        || fatal "could not parse package name from $extracted_project/Cargo.toml"
    extracted_version=$(manifest_package_field "$extracted_project/Cargo.toml" version) \
        || fatal "could not parse package version from $extracted_project/Cargo.toml"
    [[ "$extracted_name" == "$package_name" ]] \
        || fatal "extracted package name is $extracted_name, expected $package_name"
    [[ "$extracted_version" == "$package_version" ]] \
        || fatal "extracted package version is $extracted_version, expected $package_version"
done
note "[PASS] extracted package identity is $package_name $package_version"
[[ -x "$project_one/scripts/verify_release_candidate.sh" \
    && -x "$project_two/scripts/verify_release_candidate.sh" \
    && -x "$project_one/scripts/package_source.sh" \
    && -x "$project_two/scripts/package_source.sh" ]] \
    || fatal 'packaged verification/packaging scripts lost executable mode'
reject_ancestor_cargo_config "$project_one" \
    || fatal 'first extraction has an ambient Cargo configuration ancestor'
reject_ancestor_cargo_config "$project_two" \
    || fatal 'second extraction has an ambient Cargo configuration ancestor'
unexpected_one=$(find "$extract_one" ! -type d ! -type f -print | sed -n '1p') \
    || fatal 'could not inspect first extraction'
unexpected_two=$(find "$extract_two" ! -type d ! -type f -print | sed -n '1p') \
    || fatal 'could not inspect second extraction'
[[ -z "$unexpected_one" && -z "$unexpected_two" ]] \
    || fatal 'source extraction contains a symlink or another non-regular entry'

tree_one="$verification_root/source-tree-msrv-before.txt"
tree_two="$verification_root/source-tree-stable-before.txt"
tree_manifest "$project_one" "$tree_one" || fatal 'could not hash first source extraction'
tree_manifest "$project_two" "$tree_two" || fatal 'could not hash second source extraction'
if ! cmp -s "$tree_one" "$tree_two"; then
    printf 'fresh source extractions differ:\n' >&2
    diff -u "$tree_one" "$tree_two" >&2 || true
    fatal 'fresh extraction equality failed'
fi
note '[PASS] two fresh source extractions are byte-identical'

msrv_status='NOT RUN (Rustup toolchain unavailable)'
stable_status='NOT RUN (Rustup toolchain unavailable)'
msrv_observed_release='unavailable'
stable_observed_release='unavailable'
msrv_observed_host='unavailable'
stable_observed_host='unavailable'
failure_count=0
unavailable_count=0
crate_reproducibility_status='NOT RUN (stable toolchain gate must pass)'

if ((platform_incomplete != 0)); then
    msrv_status='NOT RUN (unsupported or unclassified host)'
    note "[NOT RUN] Rust MSRV gates require a recognized native host: $host_kernel $host_arch"
elif toolchain_available "$msrv_toolchain"; then
    if run_toolchain_verification msrv "$msrv_toolchain" "$project_one" "$tree_one"; then
        msrv_status='PASS'
    else
        msrv_status='FAIL'
        failure_count=$((failure_count + 1))
    fi
else
    unavailable_count=$((unavailable_count + 1))
    note "[NOT RUN] Rust MSRV toolchain is unavailable: $msrv_toolchain"
fi
if ! cleanup_registered_targets_now; then
    msrv_status='FAIL (isolated target cleanup)'
    failure_count=$((failure_count + 1))
fi

if ((platform_incomplete != 0)); then
    stable_status='NOT RUN (unsupported or unclassified host)'
    note "[NOT RUN] current-stable gates require a recognized native host: $host_kernel $host_arch"
elif toolchain_available "$stable_toolchain"; then
    if run_toolchain_verification stable "$stable_toolchain" "$project_two" "$tree_two"; then
        stable_status='PASS'
    else
        stable_status='FAIL'
        failure_count=$((failure_count + 1))
    fi
else
    unavailable_count=$((unavailable_count + 1))
    note "[NOT RUN] current-stable Rustup toolchain is unavailable: $stable_toolchain"
fi
if ! cleanup_registered_targets_now; then
    stable_status='FAIL (isolated target cleanup)'
    failure_count=$((failure_count + 1))
fi

if [[ "$stable_status" == 'PASS' ]]; then
    stable_primary_crate="$crate_store/${package_name}-${package_version}-stable.crate"
    stable_repeat_crate="$crate_store/${package_name}-${package_version}-stable-repeat.crate"
    if build_repeat_stable_crate "$project_one" "$tree_one"; then
        if cmp -s "$stable_primary_crate" "$stable_repeat_crate"; then
            crate_reproducibility_status='PASS (same installed stable Cargo; distinct fresh source extractions and targets)'
            note '[PASS] stable Cargo crate bytes match across distinct fresh source extractions and targets'
        else
            crate_reproducibility_status='FAIL (crate bytes differ)'
            failure_count=$((failure_count + 1))
            note '[FAIL] stable Cargo crate bytes differ across distinct fresh source extractions and targets'
        fi
    else
        crate_reproducibility_status='FAIL (repeat package build or source attestation)'
        failure_count=$((failure_count + 1))
        note '[FAIL] repeat stable Cargo package did not complete from the other fresh source extraction'
    fi
fi
if ! cleanup_registered_targets_now; then
    crate_reproducibility_status='FAIL (isolated target cleanup)'
    failure_count=$((failure_count + 1))
fi

smoke_equality_status='NOT RUN (both toolchain gates must pass)'
if [[ "$msrv_status" == 'PASS' && "$stable_status" == 'PASS' ]]; then
    msrv_se_manifest="$verification_root/smoke-msrv-se-tree.txt"
    stable_se_manifest="$verification_root/smoke-stable-se-tree.txt"
    msrv_pe_manifest="$verification_root/smoke-msrv-pe-tree.txt"
    stable_pe_manifest="$verification_root/smoke-stable-pe-tree.txt"
    smoke_manifest_failed=0
    if ! tree_manifest "$verification_root/smoke-msrv-se" \
        "$msrv_se_manifest" generated-output; then
        note '[FAIL] could not hash the MSRV single-end smoke bundle'
        smoke_manifest_failed=1
    fi
    if ! tree_manifest "$verification_root/smoke-stable-se" \
        "$stable_se_manifest" generated-output; then
        note '[FAIL] could not hash the stable single-end smoke bundle'
        smoke_manifest_failed=1
    fi
    if ! tree_manifest "$verification_root/smoke-msrv-pe-gzip" \
        "$msrv_pe_manifest" generated-output; then
        note '[FAIL] could not hash the MSRV paired-end smoke bundle'
        smoke_manifest_failed=1
    fi
    if ! tree_manifest "$verification_root/smoke-stable-pe-gzip" \
        "$stable_pe_manifest" generated-output; then
        note '[FAIL] could not hash the stable paired-end smoke bundle'
        smoke_manifest_failed=1
    fi
    smoke_comparison_failed=$smoke_manifest_failed
    if ((smoke_manifest_failed == 0)); then
        if ! cmp -s "$msrv_se_manifest" "$stable_se_manifest"; then
            note '[FAIL] single-end committed file paths or bytes differ across toolchains/threads'
            diff -u "$msrv_se_manifest" "$stable_se_manifest" >&2 || true
            smoke_comparison_failed=1
        fi
        if ! cmp -s "$msrv_pe_manifest" "$stable_pe_manifest"; then
            note '[FAIL] paired-end committed file paths or bytes differ across toolchains/threads'
            diff -u "$msrv_pe_manifest" "$stable_pe_manifest" >&2 || true
            smoke_comparison_failed=1
        fi
    fi
    if ((smoke_comparison_failed == 0)); then
        smoke_equality_status='PASS (committed paths/bytes; Rust 1.85/1 thread equals stable/4 threads)'
        note '[PASS] all committed smoke-bundle file paths and bytes match across toolchains and 1 versus 4 threads'
    else
        smoke_equality_status='FAIL'
        failure_count=$((failure_count + 1))
        note '[FAIL] smoke-bundle path/byte comparison did not pass'
    fi
fi

final_status='PASS'
final_exit=0
if ((failure_count != 0)); then
    final_status='FAIL'
    final_exit=1
elif ((unavailable_count != 0 || platform_incomplete != 0)); then
    final_status='INCOMPLETE'
    final_exit=2
fi

test_finalization_barrier=${VERITASM_VERIFY_TEST_FINALIZATION_BARRIER:-}
if [[ -n "$test_finalization_barrier" ]]; then
    if [[ "$test_finalization_barrier" == *[[:cntrl:]]* \
        || ! -d "$test_finalization_barrier" \
        || -L "$test_finalization_barrier" \
        || ! -p "$test_finalization_barrier/ready" \
        || -L "$test_finalization_barrier/ready" \
        || ! -p "$test_finalization_barrier/release" \
        || -L "$test_finalization_barrier/release" ]]; then
        fatal 'invalid VERITASM_VERIFY_TEST_FINALIZATION_BARRIER'
    fi
    printf 'ready\n' >"$test_finalization_barrier/ready" \
        || fatal 'could not signal the test finalization barrier'
    IFS= read -r test_finalization_release <"$test_finalization_barrier/release" \
        || fatal 'could not read the test finalization barrier release'
    [[ "$test_finalization_release" == 'release' ]] \
        || fatal 'invalid test finalization barrier release'
fi

normal_completion=1
note 'Finalizing transcript and isolated build-state cleanup before writing the authoritative summary.'
exit "$final_exit"
