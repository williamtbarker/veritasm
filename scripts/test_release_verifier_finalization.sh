#!/usr/bin/env bash
# Exercise the verifier's fail-closed final-summary path without compiling Rust.

set -euo pipefail
IFS=$'\n\t'
export LC_ALL=C
umask 077

if (($# != 2)); then
    printf 'usage: %s SOURCE_ZIP SHA256_SIDECAR\n' "${0##*/}" >&2
    exit 2
fi

test_case=${VERITASM_VERIFY_FINALIZATION_TEST_CASE:-}
if [[ -z "$test_case" ]]; then
    VERITASM_VERIFY_FINALIZATION_TEST_CASE='cargo-home-file' "$0" "$1" "$2"
    VERITASM_VERIFY_FINALIZATION_TEST_CASE='unregistered-target-file' "$0" "$1" "$2"
    VERITASM_VERIFY_FINALIZATION_TEST_CASE='supplied-archive-replacement' "$0" "$1" "$2"
    VERITASM_VERIFY_FINALIZATION_TEST_CASE='transcript-writer-leak' "$0" "$1" "$2"
    printf 'PASS: all release-verifier sealing and cleanup finalization cases failed closed\n'
    exit 0
fi
case "$test_case" in
    cargo-home-file | unregistered-target-file | supplied-archive-replacement | \
        transcript-writer-leak) ;;
    *)
        printf 'invalid VERITASM_VERIFY_FINALIZATION_TEST_CASE: %s\n' "$test_case" >&2
        exit 2
        ;;
esac

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
verifier="$script_directory/verify_release_candidate.sh"
test_parent=$(mktemp -d "${TMPDIR:-/tmp}/veritasm-verifier-finalization-test.XXXXXXXX")
barrier="$test_parent/barrier"
verification_root=''
verifier_pid=''
injected_path=''
leaked_writer_pid=''
leaked_writer_pid_file=''

cleanup() {
    local status=$?
    trap - EXIT HUP INT TERM
    set +e
    exec 5>&- 6>&-
    if [[ -n "$verifier_pid" ]] && kill -0 "$verifier_pid" 2>/dev/null; then
        kill -TERM "$verifier_pid" 2>/dev/null
        wait "$verifier_pid" 2>/dev/null
    fi
    if [[ -z "$leaked_writer_pid" && -n "$leaked_writer_pid_file" \
        && -f "$leaked_writer_pid_file" && ! -L "$leaked_writer_pid_file" ]]; then
        IFS= read -r leaked_writer_pid <"$leaked_writer_pid_file" || true
    fi
    case "$leaked_writer_pid" in
        '' | *[!0-9]*) ;;
        *)
            if kill -0 "$leaked_writer_pid" 2>/dev/null; then
                kill -TERM "$leaked_writer_pid" 2>/dev/null || true
            fi
            ;;
    esac
    if [[ -n "$injected_path" && -f "$injected_path" && ! -L "$injected_path" ]]; then
        if ! unlink "$injected_path"; then
            status=1
        fi
    fi
    case "$test_parent" in
        "${TMPDIR:-/tmp}"/veritasm-verifier-finalization-test.*)
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

for required_tool in chmod cmp cp dd find grep kill mkdir mkfifo mktemp rmdir sed sleep touch \
    unlink unzip; do
    command -v "$required_tool" >/dev/null 2>&1 \
        || { printf 'missing self-test tool: %s\n' "$required_tool" >&2; exit 1; }
done
if PATH=/usr/bin:/bin command -v rustup >/dev/null 2>&1; then
    printf 'self-test requires /usr/bin:/bin not to contain rustup\n' >&2
    exit 1
fi
if [[ "$test_case" == 'transcript-writer-leak' ]]; then
    leak_shim_root="$test_parent/leak-shim"
    leak_marker="$test_parent/leak-created.marker"
    leaked_writer_pid_file="$test_parent/leaked-writer.pid"
    system_sleep=$(PATH=/usr/bin:/bin command -v sleep)
    system_unzip=$(PATH=/usr/bin:/bin command -v unzip)
    mkdir -- "$leak_shim_root"
    printf '%s\n' \
        '#!/usr/bin/env bash' \
        'set -euo pipefail' \
        'if [[ ! -e "$LEAK_MARKER" ]]; then' \
        '    "$SYSTEM_SLEEP" 300 &' \
        '    printf "%s\\n" "$!" >"$LEAKED_WRITER_PID_FILE"' \
        '    : >"$LEAK_MARKER"' \
        'fi' \
        'exec "$SYSTEM_UNZIP" "$@"' \
        >"$leak_shim_root/unzip"
    chmod 0755 "$leak_shim_root/unzip"
fi
if [[ "$test_case" == 'supplied-archive-replacement' ]]; then
    input_root="$test_parent/input"
    shim_root="$test_parent/shim"
    input_archive="$input_root/${1##*/}"
    input_sidecar="$input_root/${1##*/}.sha256"
    reference_archive="$test_parent/reference-source.zip"
    replacement_marker="$test_parent/archive-replaced.marker"
    verifier_output="$test_parent/verifier.out"
    system_dd=$(PATH=/usr/bin:/bin command -v dd)
    mkdir -- "$input_root" "$shim_root"
    cp -- "$1" "$input_archive"
    cp -- "$1" "$reference_archive"
    cp -- "$2" "$input_sidecar"
    printf '%s\n' \
        '#!/usr/bin/env bash' \
        'set -euo pipefail' \
        'if [[ ! -e "$REPLACEMENT_MARKER" ]]; then' \
        '    unlink -- "$SUPPLIED_ARCHIVE"' \
        '    printf "foreign supplied archive replacement\\n" >"$SUPPLIED_ARCHIVE"' \
        '    : >"$REPLACEMENT_MARKER"' \
        'fi' \
        'exec "$SYSTEM_DD" "$@"' \
        >"$shim_root/dd"
    chmod 0755 "$shim_root/dd"

    set +e
    BASH_ENV=/dev/null ENV=/dev/null CARGO_HOME='' PATH="$shim_root:/usr/bin:/bin" \
        REPLACEMENT_MARKER="$replacement_marker" \
        SUPPLIED_ARCHIVE="$input_archive" \
        SYSTEM_DD="$system_dd" \
        VERITASM_VERIFY_WORK_PARENT="$test_parent" \
        "$verifier" "$input_archive" "$input_sidecar" \
        >"$verifier_output" 2>&1
    verifier_status=$?
    set -e

    [[ "$verifier_status" == 1 ]] \
        || { printf 'expected verifier exit 1 after supplied input replacement, observed %s\n' \
            "$verifier_status" >&2; exit 1; }
    grep -Fq 'supplied source ZIP changed identity after bounded sealing copy:' \
        "$verifier_output"
    grep -Fxq 'foreign supplied archive replacement' "$input_archive"
    verification_root=$(find "$test_parent" -mindepth 1 -maxdepth 1 -type d \
        -name 'veritasm-verify-*' -print | sed -n '1p')
    [[ -n "$verification_root" ]] \
        || { printf 'replacement test did not find the retained verification root\n' >&2; exit 1; }
    cmp -s "$reference_archive" \
        "$verification_root/sealed-input/${input_archive##*/}"
    printf 'PASS: supplied input replacement after anchoring failed closed\n'
    exit 0
fi
mkdir -- "$barrier"
mkfifo -m 0600 "$barrier/ready" "$barrier/release"
# Holding each FIFO open read/write prevents the test harness from blocking in
# open(2) when the verifier exits before reaching the finalization barrier.
exec 5<>"$barrier/ready" 6<>"$barrier/release"

if [[ "$test_case" == 'transcript-writer-leak' ]]; then
    BASH_ENV=/dev/null ENV=/dev/null CARGO_HOME='' \
        PATH="$leak_shim_root:/usr/bin:/bin" \
        LEAK_MARKER="$leak_marker" \
        LEAKED_WRITER_PID_FILE="$leaked_writer_pid_file" \
        SYSTEM_SLEEP="$system_sleep" \
        SYSTEM_UNZIP="$system_unzip" \
        VERITASM_VERIFY_WORK_PARENT="$test_parent" \
        VERITASM_VERIFY_TEST_FINALIZATION_BARRIER="$barrier" \
        "$verifier" "$1" "$2" >"$test_parent/verifier.out" 2>&1 &
    verifier_pid=$!
else
    BASH_ENV=/dev/null ENV=/dev/null CARGO_HOME='' PATH=/usr/bin:/bin \
        VERITASM_VERIFY_WORK_PARENT="$test_parent" \
        VERITASM_VERIFY_TEST_FINALIZATION_BARRIER="$barrier" \
        "$verifier" "$1" "$2" >"$test_parent/verifier.out" 2>&1 &
    verifier_pid=$!
fi
barrier_wait_seconds=0
while :; do
    if IFS= read -r -t 1 -u 5 barrier_state; then
        break
    fi
    if ! kill -0 "$verifier_pid" 2>/dev/null; then
        set +e
        wait "$verifier_pid"
        verifier_status=$?
        set -e
        verifier_pid=''
        printf 'verifier exited %s before reaching the finalization barrier\n' \
            "$verifier_status" >&2
        exit 1
    fi
    barrier_wait_seconds=$((barrier_wait_seconds + 1))
    if ((barrier_wait_seconds >= 60)); then
        printf 'verifier remained live without reaching the finalization barrier for 60 seconds\n' \
            >&2
        exit 1
    fi
done
[[ "$barrier_state" == 'ready' ]] \
    || { printf 'self-test received an invalid barrier state\n' >&2; exit 1; }
verification_root=$(sed -n 's/^Verification root: //p' "$test_parent/verifier.out" \
    | sed -n '1p')
[[ -n "$verification_root" ]] \
    || { printf 'self-test did not observe the verification root\n' >&2; exit 1; }
case "$verification_root" in
    "$test_parent"/veritasm-verify-0.4.0-dev.1.*) ;;
    *)
        printf 'self-test observed an unexpected verification root: %s\n' \
            "$verification_root" >&2
        exit 1
        ;;
esac

case "$test_case" in
    cargo-home-file)
        rmdir -- "$verification_root/cargo-home"
        injected_path="$verification_root/cargo-home"
        ;;
    unregistered-target-file)
        injected_path="$verification_root/target-unregistered-injected"
        ;;
esac
if [[ -n "$injected_path" ]]; then
    touch "$injected_path"
fi
printf 'release\n' >&6

completion_wait_seconds=0
while jobs -pr | grep -Fxq "$verifier_pid" \
    || jobs -ps | grep -Fxq "$verifier_pid"; do
    sleep 1
    completion_wait_seconds=$((completion_wait_seconds + 1))
    if ((completion_wait_seconds >= 20)); then
        printf 'verifier did not complete finalization within 20 seconds\n' >&2
        exit 1
    fi
done
set +e
wait "$verifier_pid"
verifier_status=$?
set -e
verifier_pid=''

[[ "$verifier_status" == 1 ]] \
    || { printf 'expected verifier exit 1, observed %s\n' "$verifier_status" >&2; exit 1; }
summary="$verification_root/verification-summary.txt"
[[ -f "$summary" && ! -L "$summary" ]] \
    || { printf 'verifier did not retain a regular final summary\n' >&2; exit 1; }
grep -qx 'Overall: FAIL' "$summary"
if [[ "$test_case" == 'transcript-writer-leak' ]]; then
    grep -qx 'Isolated target/Cargo-home cleanup: PASS' "$summary"
    grep -qx 'Transcript finalization: FAIL' "$summary"
    grep -Fq 'verification transcript writer did not reach EOF within 5 seconds' \
        "$test_parent/verifier.out"
    [[ -f "$verification_root/verification.log" \
        && ! -L "$verification_root/verification.log" ]]
    grep -Fq '[PASS] source ZIP integrity' "$verification_root/verification.log"
    IFS= read -r leaked_writer_pid <"$leaked_writer_pid_file"
else
    grep -qx 'Isolated target/Cargo-home cleanup: FAIL' "$summary"
    grep -qx 'Transcript finalization: PASS' "$summary"
fi
if [[ -e "$verification_root/transcript.pipe" \
    || -L "$verification_root/transcript.pipe" ]]; then
    printf 'verifier retained its transcript FIFO after finalization\n' >&2
    exit 1
fi
if grep -qx 'Overall: PASS' "$summary"; then
    printf 'verifier retained a false PASS after injected cleanup failure\n' >&2
    exit 1
fi

if [[ "$test_case" == 'transcript-writer-leak' ]]; then
    printf 'PASS: inherited transcript writer was bounded and produced an authoritative Overall: FAIL summary\n'
else
    printf 'PASS: %s cleanup failure produced exit 1 and an authoritative Overall: FAIL summary\n' \
        "$test_case"
fi
