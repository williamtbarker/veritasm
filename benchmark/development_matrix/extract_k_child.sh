#!/usr/bin/env bash
# Fail-closed, no-replace wrapper around the frozen multi-k FASTA adapter.

set -euo pipefail
IFS=$'\n\t'
export LC_ALL=C
umask 077

if (($# != 3)); then
    printf 'usage: %s SEGMENTS_FASTA OUTPUT_FASTA K\n' "${0##*/}" >&2
    exit 64
fi

readonly input=$1
readonly output=$2
readonly expected_k=$3
script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)

if [[ ! -f "$input" || -L "$input" ]]; then
    printf 'extract_k_child: input must be a non-symlink regular file\n' >&2
    exit 66
fi
if [[ -e "$output" || -L "$output" ]]; then
    printf 'extract_k_child: refusing to replace output\n' >&2
    exit 73
fi
output_parent=$(dirname -- "$output")
if [[ ! -d "$output_parent" || -L "$output_parent" ]]; then
    printf 'extract_k_child: output parent must be a non-symlink directory\n' >&2
    exit 73
fi
output_parent=$(CDPATH= cd -- "$output_parent" && pwd -P)
output_name=${output##*/}
if [[ -z "$output_name" || "$output_name" == . || "$output_name" == .. || "$output_name" == */* ]]; then
    printf 'extract_k_child: invalid output basename\n' >&2
    exit 64
fi

temporary=$(mktemp "$output_parent/.${output_name}.partial.XXXXXXXX")
published=0
cleanup() {
    local status=$?
    trap - EXIT HUP INT TERM
    set +e
    if ((published == 0)) && [[ -f "$temporary" && ! -L "$temporary" ]]; then
        unlink -- "$temporary"
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

awk -v expected_k="$expected_k" -f "$script_directory/extract_k_child.awk" \
    "$input" >"$temporary"
chmod 0600 "$temporary"
if ! ln -- "$temporary" "$output"; then
    printf 'extract_k_child: could not publish new output without replacement\n' >&2
    exit 73
fi
published=1
unlink -- "$temporary"
printf 'adapter_output_sha256=%s\n' "$(sha256sum "$output" | awk '{print $1}')"
