#!/usr/bin/env bash
# Check the documented toy example and two output-safety contracts.
set -euo pipefail
repository_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
binary=${VERITASM_BIN:-${CARGO_TARGET_DIR:-$repository_root/target}/release/veritasm}
if [[ ! -x "$binary" ]]; then
    echo "Build first with cargo build --release --locked, or set VERITASM_BIN." >&2
    exit 1
fi
work=$(mktemp -d "${TMPDIR:-/tmp}/veritasm-smoke.XXXXXXXX")
cleanup() {
    local status=$?
    trap - EXIT
    rm -rf -- "$work"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

verify_manifest() {
    (
        cd "$1"
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum --check manifest.sha256 >/dev/null
        else
            shasum -a 256 --check manifest.sha256 >/dev/null
        fi
    )
}

"$binary" assemble --single "$repository_root/examples/reads.fasta" \
    --output-dir "$work/result" --k 5 --profile retain-all --min-base-quality 0 \
    >"$work/stdout" 2>"$work/stderr"
test ! -s "$work/stdout"
test -s "$work/result/unitigs.fasta"
test -s "$work/result/assembly.gfa"
test -s "$work/result/report.html"
verify_manifest "$work/result"
cp "$work/result/manifest.sha256" "$work/before.sha256"

set +e
"$binary" assemble --single "$repository_root/examples/reads.fasta" \
    --output-dir "$work/result" --k 5 >"$work/repeat.out" 2>"$work/repeat.err"
status=$?
set -e
if [[ "$status" -ne 7 ]] || ! grep -Fq 'error[destination_existing]:' "$work/repeat.err"; then
    cat "$work/repeat.err" >&2
    echo "Existing-output smoke failed (exit $status)." >&2
    exit 1
fi
cmp "$work/before.sha256" "$work/result/manifest.sha256"
verify_manifest "$work/result"

printf '@broken\nACGT\n+\nI\n' >"$work/broken.fastq"
set +e
"$binary" assemble --single "$work/broken.fastq" \
    --output-dir "$work/invalid" --k 3 >"$work/invalid.out" 2>"$work/invalid.err"
status=$?
set -e
if [[ "$status" -ne 3 ]] || [[ -e "$work/invalid" ]]; then
    cat "$work/invalid.err" >&2
    echo "Malformed-input smoke failed (exit $status)." >&2
    exit 1
fi
echo "Smoke PASS: example artifacts, checksums, existing-output protection, malformed-input rejection."
