#!/usr/bin/env bash
# Focused tests for benchmark-only measurement and FASTA adapter tools.

set -euo pipefail
IFS=$'\n\t'
export LC_ALL=C
export TZ=UTC
export PYTHONDONTWRITEBYTECODE=1
export PYTHONHASHSEED=0
export OMP_NUM_THREADS=1
export OPENBLAS_NUM_THREADS=1
export MKL_NUM_THREADS=1
export NUMEXPR_NUM_THREADS=1
export VECLIB_MAXIMUM_THREADS=1
umask 077

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
measurement_cc='/usr/bin/cc'
temporary=$(mktemp -d /tmp/veritasm-development-matrix-tools.XXXXXXXX)
cleanup() {
    local status=$?
    trap - EXIT HUP INT TERM
    set +e
    case "$temporary" in
        /tmp/veritasm-development-matrix-tools.*)
            find "$temporary" -depth -mindepth 1 -delete
            rmdir "$temporary"
            ;;
        *)
            printf 'refusing to clean unexpected test path: %s\n' "$temporary" >&2
            status=1
            ;;
    esac
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

for tool in awk jq mktemp sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'required test tool is unavailable: %s\n' "$tool" >&2
        exit 1
    }
done
[[ -x "$measurement_cc" ]] || {
    printf 'required test tool is unavailable: %s\n' "$measurement_cc" >&2
    exit 1
}

measure="$temporary/wait4_measure"
"$measurement_cc" -std=c11 -O2 -Wall -Wextra -Werror -pedantic \
    "$script_directory/wait4_measure.c" -o "$measure"

set +e
"$measure" --output "$temporary/exit7.json" --timeout-seconds 5 \
    --grace-seconds 1 -- /bin/sh -c 'exit 7'
status=$?
set -e
[[ $status -eq 7 ]]
jq -e '
    .schema_version == "veritasm-linux-wait4-measure-v1" and
    .exit_kind == "exited" and
    .exit_code_decimal == "7" and
    .termination_signal_decimal == null and
    .timed_out == false and
    (.wall_nanoseconds_decimal | test("^[0-9]+$")) and
    (.max_rss_kib_decimal | test("^[0-9]+$"))
' "$temporary/exit7.json" >/dev/null

set +e
"$measure" --output "$temporary/timeout.json" --timeout-seconds 1 \
    --grace-seconds 1 -- /bin/sh -c 'trap "" TERM; exec /bin/sleep 20'
status=$?
set -e
[[ $status -eq 124 ]]
jq -e '
    .exit_kind == "signaled" and
    .exit_code_decimal == null and
    .termination_signal_decimal == "9" and
    .timed_out == true
' "$temporary/timeout.json" >/dev/null

printf 'reserved\n' >"$temporary/existing.json"
set +e
"$measure" --output "$temporary/existing.json" --timeout-seconds 5 \
    --grace-seconds 1 -- /bin/sh -c 'printf executed >"$1"' sh \
    "$temporary/should-not-exist" 2>"$temporary/no-replace.stderr"
status=$?
set -e
[[ $status -eq 125 ]]
[[ ! -e "$temporary/should-not-exist" ]]
[[ $(<"$temporary/existing.json") == reserved ]]

segment_a=mks-$(printf 'a%.0s' {1..64})
segment_b=mks-$(printf 'b%.0s' {1..64})
root_a=$(printf 'c%.0s' {1..64})
root_b=$(printf 'd%.0s' {1..64})
printf '>%.68s k=21 topology=linear child_root=%.64s status=experimental qualification=unqualified intended_use=research_use_only\nACGT\n>%.68s k=31 topology=closed_walk child_root=%.64s status=experimental qualification=unqualified intended_use=research_use_only\nTGCA\n' \
    "$segment_a" "$root_a" "$segment_b" "$root_b" >"$temporary/segments.fasta"
"$script_directory/extract_k_child.sh" "$temporary/segments.fasta" \
    "$temporary/k31.fasta" 31 >"$temporary/adapter.stdout"
printf '>%.68s k=31 topology=closed_walk child_root=%.64s status=experimental qualification=unqualified intended_use=research_use_only\nTGCA\n' \
    "$segment_b" "$root_b" >"$temporary/k31.expected.fasta"
cmp "$temporary/k31.expected.fasta" "$temporary/k31.fasta"

printf '>not-a-segment k=31\nACGT\n' >"$temporary/malformed.fasta"
if "$script_directory/extract_k_child.sh" "$temporary/malformed.fasta" \
    "$temporary/malformed.out" 31 >"$temporary/malformed.stdout" \
    2>"$temporary/malformed.err"; then
    printf 'malformed segment input was unexpectedly accepted\n' >&2
    exit 1
fi
[[ ! -e "$temporary/malformed.out" ]]

printf '>%.68s k=31 topology=linear child_root=%.64s status=experimental qualification=unqualified intended_use=research_use_only\n' \
    "$segment_b" "$root_b" >"$temporary/header-only.fasta"
if "$script_directory/extract_k_child.sh" "$temporary/header-only.fasta" \
    "$temporary/header-only.out" 31 >"$temporary/header-only.stdout" \
    2>"$temporary/header-only.err"; then
    printf 'header-only segment input was unexpectedly accepted\n' >&2
    exit 1
fi
[[ ! -e "$temporary/header-only.out" ]]

(
    # Source-only mode exposes the command renderer without dispatching a run.
    source "$script_directory/run.sh"
    command_root="$temporary/command-model"
    mkdir -p "$command_root/freeze"
    cp "$matrix_source" "$command_root/freeze/matrix.tsv"
    create_frozen_commands "$command_root" /frozen/veritasm \
        /frozen/veritasm-multik /frozen/veritasm-simulate \
        /frozen/veritasm-evaluate /frozen/virustic2 /frozen/megahit \
        /frozen/spades.py
    validate_frozen_commands_file "$command_root/freeze/commands.jsonl"
    jq -s -e '
        def option_value($name):
            .argv as $argv | ($argv | index($name)) as $index |
            if $index == null then null else $argv[$index + 1] end;
        all(.[] | select(.phase == "assemble" and .arm == "veritasm-fixed-k31");
            option_value("--k") == "31" and
            option_value("--min-support") == "2" and
            option_value("--min-base-quality") == "20" and
            option_value("--threads") == "1") and
        all(.[] | select(.phase == "assemble" and
                         (.arm == "veritasm-multik-diversity" or
                          .arm == "veritasm-multik-exact-agreement"));
            option_value("--k") == "21,31,51" and
            option_value("--retention") == "inclusive-support" and
            option_value("--support-unit") == "supplied-fragment-instance" and
            option_value("--min-support") == "2" and
            option_value("--min-base-quality") == "20" and
            option_value("--threads") == "1") and
        all(.[] | select(.phase == "assemble" and .arm == "virustic2-k31");
            option_value("--kmer-size") == "31" and
            option_value("--min-support") == "2" and
            option_value("--min-base-quality") == "20" and
            option_value("--min-contig-length") == "0" and
            option_value("--tip-length") == "0" and
            option_value("--threads") == "1") and
        all(.[] | select(.phase == "assemble" and .arm == "megahit-1.2.9");
            option_value("--k-list") == "21,31,51" and
            option_value("--min-count") == "2" and
            option_value("--num-cpu-threads") == "1" and
            option_value("--memory") == "1073741824" and
            option_value("--min-contig-len") == "0" and
            (.argv | index("--no-hw-accel")) != null) and
        all(.[] | select(.phase == "assemble" and .arm == "spades-4.3.0");
            option_value("-k") == "21,31,51" and
            option_value("-t") == "1" and option_value("-m") == "1" and
            option_value("--phred-offset") == "33")
    ' "$command_root/freeze/commands.jsonl" >/dev/null
)

if [[ -n "${MEGAHIT_BIN:-}" ]]; then
    [[ -x "$MEGAHIT_BIN" && ! -L "$MEGAHIT_BIN" ]]
    [[ $("$MEGAHIT_BIN" --version 2>&1) == 'MEGAHIT v1.2.9' ]]
    (
        cd "$temporary"
        "$MEGAHIT_BIN" --test --k-list 21,31,51 --min-count 2 \
            --num-cpu-threads 1 --memory 1073741824 --no-hw-accel \
            --min-contig-len 0
    ) >"$temporary/megahit-test.stdout" 2>"$temporary/megahit-test.stderr"
fi

if [[ -n "${SPADES_BIN:-}" ]]; then
    [[ -x "$SPADES_BIN" && ! -L "$SPADES_BIN" ]]
    [[ $("$SPADES_BIN" --version 2>&1) == 'SPAdes genome assembler v4.3.0' ]]
    (
        cd "$temporary"
        "$SPADES_BIN" --test -k 21,31,51 -t 1 -m 1 --phred-offset 33
    ) >"$temporary/spades-test.stdout" 2>"$temporary/spades-test.stderr"
fi

printf 'benchmark tool self-tests: PASS\n'
