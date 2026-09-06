#!/usr/bin/env bash
# Two-phase, fail-closed development matrix for small generator-v4 datasets.
# This is benchmark tooling, not an assembler runtime dependency.

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

readonly schema='veritasm-development-matrix-v1'
readonly pinned_virustic2_commit='b211915fc7cce82629766b77024463c6cabcc749'
readonly megahit_version='1.2.9'
readonly megahit_archive_sha256='7e5710f62b0743471c5d6938ebc28132dcea2104ccdbeafb4f2ca8dbb47728d3'
readonly megahit_wrapper_sha256='723f8a4993c0777c4259d26b9576dcbbd14496ccd9325344505c9447f6344d98'
readonly megahit_no_hw_core_sha256='2f9cfb075bf32b4270f67b917d01f8c9f03a08a553c46bbfc58cd14df401241e'
readonly megahit_release_url='https://github.com/voutcn/megahit/releases/download/v1.2.9/MEGAHIT-1.2.9-Linux-x86_64-static.tar.gz'
readonly spades_version='4.3.0'
readonly spades_archive_sha256='e88a8c533c8614dd4b7c5788cfcd46427848a0575267f97c690a75fd2a343034'
readonly spades_wrapper_sha256='1e4b04612721e15afe6b9181ffdace2d4950af37ed8b3f6a2719febee29c63d8'
readonly spades_core_sha256='d88ba44db4404fe6812cbc113199a51f039bf2a7285b66c278338897ef71350c'
readonly spades_hammer_sha256='686f68bb642829031920aa3c7038e741008c2f9c937d3cd577eeb97241abb831'
readonly spades_release_url='https://github.com/ablab/spades/releases/download/v4.3.0/SPAdes-4.3.0-Linux.tar.gz'
readonly timeout_seconds='300'
readonly timeout_grace_seconds='5'
readonly measurement_cc='/usr/bin/cc'
readonly memory_budget_bytes='536870912'
readonly max_spool_bytes='536870912'
readonly max_temp_bytes='4294967296'
readonly max_staged_output_bytes='1073741824'

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repository_root=$(CDPATH= cd -- "$script_directory/../.." && pwd -P)
readonly script_directory repository_root
readonly matrix_source="$script_directory/matrix.tsv"
readonly measure_source="$script_directory/wait4_measure.c"
readonly adapter_script="$script_directory/extract_k_child.sh"

usage() {
    cat >&2 <<EOF
usage:
  ${0##*/} check
  ${0##*/} prepare /tmp/veritasm-development-matrix.NAME VIRUSTIC2_SOURCE_ROOT MEGAHIT_ARCHIVE SPADES_ARCHIVE
  ${0##*/} execute /tmp/veritasm-development-matrix.NAME EXPECTED_PLAN_SHA256

prepare builds frozen binaries, generates all six datasets, writes every exact
command, and stops before any assembly command. execute re-verifies the frozen
plan and inputs before running it. Results are development-only.
EOF
}

die() {
    printf 'development-matrix error: %s\n' "$*" >&2
    exit 1
}

require_tools() {
    local tool
    for tool in awk bash cc chmod cmp cp cut dirname find git jq ln lscpu mkdir \
        mktemp mv python pwd readlink sed sha256sum sort stat tar tee uname unlink; do
        command -v "$tool" >/dev/null 2>&1 || die "required host tool is unavailable: $tool"
    done
    command -v cargo >/dev/null 2>&1 || die 'Cargo is unavailable in PATH'
    command -v rustc >/dev/null 2>&1 || die 'rustc is unavailable in PATH'
    [[ -x "$measurement_cc" ]] || die 'benchmark measurement compiler is unavailable: /usr/bin/cc'
}

sha256_file() {
    sha256sum "$1" | awk '{print $1}'
}

sha256_stdin() {
    sha256sum | awk '{print $1}'
}

require_clean_repository() {
    [[ -z $(git -C "$repository_root" status --porcelain=v1 --untracked-files=all) ]] ||
        die 'VeritAsm repository must be completely clean before prepare/execute'
}

validate_run_root_name() {
    local requested=$1
    [[ "$requested" = /* ]] || die 'run root must be an absolute path'
    local parent=${requested%/*}
    local name=${requested##*/}
    [[ -n "$parent" && -n "$name" ]] || die 'run root has an invalid parent or basename'
    [[ -d "$parent" && ! -L "$parent" ]] || die 'run-root parent must be a non-symlink directory'
    parent=$(CDPATH= cd -- "$parent" && pwd -P)
    [[ "$parent" == /tmp ]] || die 'bulky development evidence must be placed directly below /tmp'
    [[ "$name" =~ ^veritasm-development-matrix\.[A-Za-z0-9._-]+$ ]] ||
        die 'run-root basename must use the veritasm-development-matrix. prefix and portable characters'
    printf '%s/%s\n' "$parent" "$name"
}

validate_matrix() {
    [[ -f "$matrix_source" && ! -L "$matrix_source" ]] || die 'matrix.tsv is missing or a symlink'
    local expected_header=$'case_id\treplicate\tmaster_seed_hex\tfragments\ttruth_length\tread_length\tinsert_length\tsubstitution_rate_ppm\tlow_quality_rate_ppm\tcompression'
    [[ $(sed -n '1p' "$matrix_source") == "$expected_header" ]] || die 'matrix.tsv header differs from v1'
    local count=0
    local case_id replicate seed fragments truth_length read_length insert_length substitution low_quality compression extra
    declare -A seen=()
    while IFS=$'\t' read -r case_id replicate seed fragments truth_length read_length \
        insert_length substitution low_quality compression extra; do
        [[ "$case_id" != case_id ]] || continue
        [[ -z "${extra:-}" ]] || die "matrix row has extra fields: $case_id"
        [[ -z "${seen[$case_id]:-}" ]] || die "matrix case is duplicated: $case_id"
        seen[$case_id]=1
        case "$case_id" in
            linear-se|linear-pe|circular-pe|mixture-pe)
                [[ "$substitution" == 0 && "$low_quality" == 0 ]] ||
                    die "$case_id must use zero error and low-quality rates"
                ;;
            error-pe)
                [[ "$substitution" == 20000 && "$low_quality" == 20000 ]] ||
                    die 'error-pe rates differ from the frozen matrix'
                ;;
            qc-censoring-control)
                [[ "$substitution" == 20000 && "$low_quality" == 0 ]] ||
                    die 'qc-censoring-control rates differ from the frozen matrix'
                ;;
            *) die "unknown matrix case: $case_id" ;;
        esac
        [[ "$replicate" == 0 && "$fragments" == 200 && "$truth_length" == 2000 && \
            "$read_length" == 100 && "$insert_length" == 250 && "$compression" == plain ]] ||
            die "matrix scalar differs from frozen small settings: $case_id"
        [[ "$seed" =~ ^[0-9a-f]{64}$ ]] || die "matrix seed is not lowercase SHA-256: $case_id"
        local derived
        derived=$(printf 'veritasm-validation-v1\0%s\0%s' "$case_id" "$replicate" | sha256_stdin)
        [[ "$seed" == "$derived" ]] || die "matrix seed derivation mismatch: $case_id"
        count=$((count + 1))
    done <"$matrix_source"
    [[ $count -eq 6 ]] || die "matrix must contain exactly six cases; observed $count"
}

canonical_tree_manifest() {
    local tree_root=$1
    local output=$2
    [[ -d "$tree_root" && ! -L "$tree_root" ]] || die "tree root is invalid: $tree_root"
    if [[ -n $(find "$tree_root" -type l -print -quit) ]]; then
        die "tree contains a symlink: $tree_root"
    fi
    if [[ -n $(find "$tree_root" ! -type d ! -type f -print -quit) ]]; then
        die "tree contains a non-regular filesystem entry: $tree_root"
    fi
    local temporary="${output}.partial.$$"
    [[ ! -e "$temporary" && ! -L "$temporary" ]] || die "tree-manifest temporary exists: $temporary"
    (
        cd "$tree_root"
        find . -type f -printf '%P\n' | sort | while IFS= read -r relative; do
            [[ "$relative" =~ ^[A-Za-z0-9._/-]+$ ]] || {
                printf 'nonportable evidence path: %q\n' "$relative" >&2
                exit 1
            }
            printf '%s\t%s\t%s\n' "$(sha256_file "$relative")" \
                "$(stat -c '%s' "$relative")" "$relative"
        done
    ) >"$temporary"
    chmod 0600 "$temporary"
    mv -- "$temporary" "$output"
}

append_command() {
    local commands_file=$1
    local id=$2
    local phase=$3
    local case_id=$4
    local repetition=$5
    local arm=$6
    local timed=$7
    local expected_exit=$8
    local working_directory=$9
    local stdout_log=${10}
    local stderr_log=${11}
    local metrics_file=${12}
    local artifact=${13}
    local dependencies_json=${14}
    shift 14
    local argv_json order_key
    argv_json=$(printf '%s\0' "$@" | jq -Rs 'split("\u0000")[:-1]')
    if [[ "$phase" == assemble ]]; then
        order_key=$(printf 'veritasm-development-matrix-run-order-v1\0%s\0%s\0%s' \
            "$case_id" "$repetition" "$arm" | sha256_stdin)
    else
        order_key='not_applicable'
    fi
    jq -cn \
        --arg id "$id" --arg phase "$phase" --arg case_id "$case_id" \
        --arg repetition "$repetition" --arg arm "$arm" --arg order_key "$order_key" \
        --arg working_directory "$working_directory" --arg stdout_log "$stdout_log" \
        --arg stderr_log "$stderr_log" --arg metrics_file "$metrics_file" \
        --arg artifact "$artifact" --argjson timed "$timed" \
        --arg expected_exit "$expected_exit" --argjson dependencies "$dependencies_json" \
        --argjson argv "$argv_json" \
        '{id:$id,phase:$phase,case_id:$case_id,repetition:$repetition,arm:$arm,
          order_key:$order_key,timed:$timed,expected_exit_code_decimal:$expected_exit,
          working_directory:$working_directory,stdout_log:$stdout_log,stderr_log:$stderr_log,
          metrics_file:$metrics_file,primary_artifact:$artifact,depends_on:$dependencies,argv:$argv}' \
        >>"$commands_file"
}

append_build_command() {
    local output=$1
    local id=$2
    local working_directory=$3
    local stdout_log=$4
    local stderr_log=$5
    local environment_json=$6
    shift 6
    local argv_json
    argv_json=$(printf '%s\0' "$@" | jq -Rs 'split("\u0000")[:-1]')
    jq -cn --arg id "$id" --arg working_directory "$working_directory" \
        --arg stdout_log "$stdout_log" --arg stderr_log "$stderr_log" \
        --argjson environment "$environment_json" --argjson argv "$argv_json" \
        '{id:$id,working_directory:$working_directory,stdout_log:$stdout_log,
          stderr_log:$stderr_log,environment:$environment,argv:$argv}' >>"$output"
}

validate_frozen_commands_file() {
    local commands=$1
    jq -e '
        length == 187 and
        ([.[].id] | unique | length) == 187 and
        ([.[] | select(.phase == "generation")] | length) == 6 and
        ([.[] | select(.phase == "assemble")] | length) == 72 and
        ([.[] | select(.phase == "adapter")] | length) == 12 and
        ([.[] | select(.phase == "evaluate")] | length) == 84 and
        ([.[] | select(.phase == "determinism")] | length) == 12 and
        ([.[] | select(.phase == "negative")] | length) == 1 and
        ([.[] | select(.timed == true)] | length) == 72 and
        all(.[] | select(.phase == "assemble"); .timed == true and
            .expected_exit_code_decimal == "0" and .metrics_file != "not_applicable") and
        all(.[] | select(.phase != "assemble"); .timed == false and
            .metrics_file == "not_applicable") and
        ([.[] | select(.expected_exit_code_decimal == "2")] | length) == 1 and
        ([.[] | select(.expected_exit_code_decimal != "0" and
                       .expected_exit_code_decimal != "2")] | length) == 0 and
        ([.[] | select(.phase == "assemble") | .order_key] | unique | length) == 72 and
        ([.[] | .stdout_log] | unique | length) == 187 and
        ([.[] | .stderr_log] | unique | length) == 187 and
        ([.[] | select(.phase != "assemble" and .order_key != "not_applicable")] | length) == 0 and
        ([.[] | select((.argv | type) != "array" or (.argv | length) == 0)] | length) == 0 and
        ([.[] | select((.depends_on | type) != "array")] | length) == 0 and
        ([.[] | .depends_on[]] - [.[].id] | length) == 0 and
        (["linear-se","linear-pe","circular-pe","mixture-pe","error-pe",
          "qc-censoring-control"] - ([.[] | select(.phase == "generation") | .case_id] | unique) | length) == 0 and
        (["veritasm-fixed-k31","veritasm-multik-diversity",
          "veritasm-multik-exact-agreement","virustic2-k31","megahit-1.2.9",
          "spades-4.3.0"] - ([.[] | select(.phase == "assemble") | .arm] | unique) | length) == 0 and
        ([.[] | select(.phase == "assemble") | .arm] | unique | length) == 6 and
        ([.[] | select(.phase == "assemble") | .arm] | group_by(.) |
          all(.[]; length == 12)) and
        (["veritasm-fixed-k31","veritasm-multik-diversity",
          "veritasm-multik-exact-agreement","virustic2-k31","megahit-1.2.9",
          "spades-4.3.0","veritasm-multik-k31-diagnostic"] -
          ([.[] | select(.phase == "evaluate") | .arm] | unique) | length) == 0 and
        ([.[] | select(.phase == "evaluate") | .arm] | unique | length) == 7 and
        ([.[] | select(.phase == "evaluate") | .arm] | group_by(.) |
          all(.[]; length == 12))
    ' < <(jq -s . "$commands") >/dev/null ||
        die 'frozen command inventory failed structural validation'
}

create_frozen_commands() {
    local run_root=$1
    local veritasm=$2
    local multik=$3
    local simulate=$4
    local evaluate=$5
    local virustic2=$6
    local megahit=$7
    local spades=$8
    local freeze="$run_root/freeze"
    local commands="$freeze/commands.jsonl"
    : >"$commands"

    local case_id replicate seed fragments truth_length read_length insert_length substitution low_quality compression extra
    while IFS=$'\t' read -r case_id replicate seed fragments truth_length read_length \
        insert_length substitution low_quality compression extra; do
        [[ "$case_id" != case_id ]] || continue
        local dataset_root="$run_root/datasets/$case_id"
        local generator_work="$run_root/generation/$case_id"
        append_command "$commands" "generate::$case_id" generation "$case_id" prepare \
            veritasm-simulate false 0 "$generator_work" \
            "$run_root/logs/generate-$case_id.stdout" "$run_root/logs/generate-$case_id.stderr" \
            not_applicable "$dataset_root/dataset.json" '[]' \
            "$simulate" --out "$dataset_root" --case "$case_id" --replicate "$replicate" \
            --fragments "$fragments" --truth-length "$truth_length" \
            --read-length "$read_length" --insert-length "$insert_length" \
            --substitution-rate-ppm "$substitution" --low-quality-rate-ppm "$low_quality"

        local -a stable_input multi_input v2_input megahit_input spades_input
        if [[ "$case_id" == linear-se ]]; then
            stable_input=(--single "$dataset_root/assembler_input/reads_SE.fastq")
            multi_input=(--single "$dataset_root/assembler_input/reads_SE.fastq")
            v2_input=(--single "$dataset_root/assembler_input/reads_SE.fastq")
            megahit_input=(-r "$dataset_root/assembler_input/reads_SE.fastq")
            spades_input=(-s "$dataset_root/assembler_input/reads_SE.fastq")
        else
            stable_input=(--read1 "$dataset_root/assembler_input/reads_R1.fastq" \
                --read2 "$dataset_root/assembler_input/reads_R2.fastq")
            multi_input=(--read1 "$dataset_root/assembler_input/reads_R1.fastq" \
                --read2 "$dataset_root/assembler_input/reads_R2.fastq")
            v2_input=(--read1 "$dataset_root/assembler_input/reads_R1.fastq" \
                --read2 "$dataset_root/assembler_input/reads_R2.fastq")
            megahit_input=(-1 "$dataset_root/assembler_input/reads_R1.fastq" \
                -2 "$dataset_root/assembler_input/reads_R2.fastq")
            spades_input=(-1 "$dataset_root/assembler_input/reads_R1.fastq" \
                -2 "$dataset_root/assembler_input/reads_R2.fastq")
        fi

        local repetition
        for repetition in r1 r2; do
            local fixed_dir="$run_root/runs/$case_id/$repetition/veritasm-fixed-k31"
            local diversity_dir="$run_root/runs/$case_id/$repetition/veritasm-multik-diversity"
            local consensus_dir="$run_root/runs/$case_id/$repetition/veritasm-multik-exact-agreement"
            local v2_dir="$run_root/runs/$case_id/$repetition/virustic2-k31"
            local megahit_dir="$run_root/runs/$case_id/$repetition/megahit-1.2.9"
            local spades_dir="$run_root/runs/$case_id/$repetition/spades-4.3.0"
            local diagnostic_dir="$run_root/runs/$case_id/$repetition/veritasm-multik-k31-diagnostic"

            local fixed_id="assemble::$case_id::$repetition::veritasm-fixed-k31"
            local diversity_id="assemble::$case_id::$repetition::veritasm-multik-diversity"
            local consensus_id="assemble::$case_id::$repetition::veritasm-multik-exact-agreement"
            local v2_id="assemble::$case_id::$repetition::virustic2-k31"
            local megahit_id="assemble::$case_id::$repetition::megahit-1.2.9"
            local spades_id="assemble::$case_id::$repetition::spades-4.3.0"
            local adapter_id="adapter::$case_id::$repetition::veritasm-multik-k31-diagnostic"

            append_command "$commands" "$fixed_id" assemble "$case_id" "$repetition" \
                veritasm-fixed-k31 true 0 "$fixed_dir" "$fixed_dir/stdout.log" \
                "$fixed_dir/stderr.log" "$fixed_dir/measure.json" "$fixed_dir/bundle/unitigs.fasta" \
                '[]' "$veritasm" assemble "${stable_input[@]}" --output-dir "$fixed_dir/bundle" \
                --k 31 --profile thresholded --support-unit supplied-fragment-instance \
                --min-support 2 --min-base-quality 20 --no-remap --threads 1 \
                --memory-budget-bytes "$memory_budget_bytes" --max-spool-bytes "$max_spool_bytes" \
                --max-temp-bytes "$max_temp_bytes" --max-staged-output-bytes "$max_staged_output_bytes"

            append_command "$commands" "$diversity_id" assemble "$case_id" "$repetition" \
                veritasm-multik-diversity true 0 "$diversity_dir" "$diversity_dir/stdout.log" \
                "$diversity_dir/stderr.log" "$diversity_dir/measure.json" \
                "$diversity_dir/bundle/contigs.fasta" '[]' "$multik" "${multi_input[@]}" \
                --output-dir "$diversity_dir/bundle" --k 21,31,51 \
                --retention inclusive-support --min-support 2 \
                --support-unit supplied-fragment-instance --output-profile diversity-preserving \
                --min-base-quality 20 --threads 1 --memory-budget-bytes "$memory_budget_bytes" \
                --max-spool-bytes "$max_spool_bytes" --max-temp-bytes "$max_temp_bytes" \
                --max-child-keys 1000000 --max-child-snapshot-bytes 8388608 \
                --max-loaded-snapshot-bytes 33554432 \
                --max-staged-output-bytes "$max_staged_output_bytes"

            append_command "$commands" "$consensus_id" assemble "$case_id" "$repetition" \
                veritasm-multik-exact-agreement true 0 "$consensus_dir" \
                "$consensus_dir/stdout.log" "$consensus_dir/stderr.log" \
                "$consensus_dir/measure.json" "$consensus_dir/bundle/contigs.fasta" '[]' \
                "$multik" "${multi_input[@]}" --output-dir "$consensus_dir/bundle" --k 21,31,51 \
                --retention inclusive-support --min-support 2 \
                --support-unit supplied-fragment-instance \
                --output-profile exact-agreement-consensus --min-base-quality 20 --threads 1 \
                --memory-budget-bytes "$memory_budget_bytes" --max-spool-bytes "$max_spool_bytes" \
                --max-temp-bytes "$max_temp_bytes" --max-child-keys 1000000 \
                --max-child-snapshot-bytes 8388608 --max-loaded-snapshot-bytes 33554432 \
                --max-staged-output-bytes "$max_staged_output_bytes"

            append_command "$commands" "$v2_id" assemble "$case_id" "$repetition" \
                virustic2-k31 true 0 "$v2_dir" "$v2_dir/stdout.log" "$v2_dir/stderr.log" \
                "$v2_dir/measure.json" "$v2_dir/assembly.fasta" '[]' "$virustic2" assemble \
                "${v2_input[@]}" --output "$v2_dir/assembly.fasta" --report "$v2_dir/report.json" \
                --kmer-size 31 --min-support 2 --min-base-quality 20 --min-contig-length 0 \
                --tip-length 0 --support-mode fragment --threads 1 --batch-size 4096 --quiet

            append_command "$commands" "$megahit_id" assemble "$case_id" "$repetition" \
                megahit-1.2.9 true 0 "$megahit_dir" "$megahit_dir/stdout.log" \
                "$megahit_dir/stderr.log" "$megahit_dir/measure.json" \
                "$megahit_dir/output/final.contigs.fa" '[]' "$megahit" \
                "${megahit_input[@]}" --out-dir "$megahit_dir/output" \
                --tmp-dir "$megahit_dir/tmp" --k-list 21,31,51 --min-count 2 \
                --num-cpu-threads 1 --memory 1073741824 --no-hw-accel --min-contig-len 0

            append_command "$commands" "$spades_id" assemble "$case_id" "$repetition" \
                spades-4.3.0 true 0 "$spades_dir" "$spades_dir/stdout.log" \
                "$spades_dir/stderr.log" "$spades_dir/measure.json" \
                "$spades_dir/output/contigs.fasta" '[]' "$spades" "${spades_input[@]}" \
                -o "$spades_dir/output" -k 21,31,51 -t 1 -m 1 --phred-offset 33

            append_command "$commands" "$adapter_id" adapter "$case_id" "$repetition" \
                veritasm-multik-k31-diagnostic false 0 "$diagnostic_dir" \
                "$diagnostic_dir/stdout.log" "$diagnostic_dir/stderr.log" not_applicable \
                "$diagnostic_dir/assembly.fasta" "[\"$diversity_id\"]" "$adapter_script" \
                "$diversity_dir/bundle/segments.fasta" "$diagnostic_dir/assembly.fasta" 31

            local arm assembly dependency evaluation_dir eval_id
            for arm in veritasm-fixed-k31 veritasm-multik-diversity \
                veritasm-multik-exact-agreement virustic2-k31 megahit-1.2.9 spades-4.3.0 \
                veritasm-multik-k31-diagnostic; do
                case "$arm" in
                    veritasm-fixed-k31)
                        assembly="$fixed_dir/bundle/unitigs.fasta"
                        dependency="$fixed_id"
                        evaluation_dir="$fixed_dir/evaluation"
                        ;;
                    veritasm-multik-diversity)
                        assembly="$diversity_dir/bundle/contigs.fasta"
                        dependency="$diversity_id"
                        evaluation_dir="$diversity_dir/evaluation"
                        ;;
                    veritasm-multik-exact-agreement)
                        assembly="$consensus_dir/bundle/contigs.fasta"
                        dependency="$consensus_id"
                        evaluation_dir="$consensus_dir/evaluation"
                        ;;
                    virustic2-k31)
                        assembly="$v2_dir/assembly.fasta"
                        dependency="$v2_id"
                        evaluation_dir="$v2_dir/evaluation"
                        ;;
                    megahit-1.2.9)
                        assembly="$megahit_dir/output/final.contigs.fa"
                        dependency="$megahit_id"
                        evaluation_dir="$megahit_dir/evaluation"
                        ;;
                    spades-4.3.0)
                        assembly="$spades_dir/output/contigs.fasta"
                        dependency="$spades_id"
                        evaluation_dir="$spades_dir/evaluation"
                        ;;
                    veritasm-multik-k31-diagnostic)
                        assembly="$diagnostic_dir/assembly.fasta"
                        dependency="$adapter_id"
                        evaluation_dir="$diagnostic_dir/evaluation"
                        ;;
                esac
                eval_id="evaluate::$case_id::$repetition::$arm"
                append_command "$commands" "$eval_id" evaluate "$case_id" "$repetition" "$arm" \
                    false 0 "${evaluation_dir%/*}" "${evaluation_dir%/*}/evaluate.stdout.log" \
                    "${evaluation_dir%/*}/evaluate.stderr.log" not_applicable \
                    "$evaluation_dir/result.json" "[\"$dependency\"]" "$evaluate" \
                    --dataset "$dataset_root/dataset.json" --assembly "$assembly" \
                    --out "$evaluation_dir" --junction-flank 15 --max-edit-rate-ppm 150000 \
                    --max-dp-cells 50000000 --max-exact-alignment-scan-bases 1000000000 \
                    --max-junction-comparisons 500000000 --junction-evidence non-correct \
                    --max-junction-index-bytes 536870912 \
                    --max-junction-evidence-bytes 134217728 \
                    --max-truth-evidence-replay-bytes 402653184
            done
        done

        local fixed_t2="$run_root/runs/$case_id/thread2/veritasm-fixed-k31"
        local v2_t2="$run_root/runs/$case_id/thread2/virustic2-k31"
        append_command "$commands" "determinism::$case_id::thread2::veritasm-fixed-k31" \
            determinism "$case_id" thread2 veritasm-fixed-k31 false 0 "$fixed_t2" \
            "$fixed_t2/stdout.log" "$fixed_t2/stderr.log" not_applicable \
            "$fixed_t2/bundle/unitigs.fasta" '[]' "$veritasm" assemble "${stable_input[@]}" \
            --output-dir "$fixed_t2/bundle" --k 31 --profile thresholded \
            --support-unit supplied-fragment-instance --min-support 2 --min-base-quality 20 \
            --no-remap --threads 2 --memory-budget-bytes "$memory_budget_bytes" \
            --max-spool-bytes "$max_spool_bytes" --max-temp-bytes "$max_temp_bytes" \
            --max-staged-output-bytes "$max_staged_output_bytes"
        append_command "$commands" "determinism::$case_id::thread2::virustic2-k31" \
            determinism "$case_id" thread2 virustic2-k31 false 0 "$v2_t2" \
            "$v2_t2/stdout.log" "$v2_t2/stderr.log" not_applicable "$v2_t2/assembly.fasta" \
            '[]' "$virustic2" assemble "${v2_input[@]}" --output "$v2_t2/assembly.fasta" \
            --report "$v2_t2/report.json" --kmer-size 31 --min-support 2 \
            --min-base-quality 20 --min-contig-length 0 --tip-length 0 \
            --support-mode fragment --threads 2 --batch-size 4096 --quiet
    done <"$run_root/freeze/matrix.tsv"

    local negative_dir="$run_root/runs/contract-negative/multik-thread2"
    append_command "$commands" 'negative::multik-thread2-rejected' negative linear-se \
        contract-negative veritasm-multik-diversity false 2 "$negative_dir" \
        "$negative_dir/stdout.log" "$negative_dir/stderr.log" not_applicable \
        "$negative_dir/should-not-exist" '[]' "$multik" --single \
        "$run_root/datasets/linear-se/assembler_input/reads_SE.fastq" \
        --output-dir "$negative_dir/should-not-exist" --k 21,31,51 \
        --retention inclusive-support --min-support 2 --support-unit supplied-fragment-instance \
        --output-profile diversity-preserving --min-base-quality 20 --threads 2
}

prepare() {
    (($# == 4)) || { usage; exit 2; }
    require_tools
    [[ $(uname -s) == Linux ]] || die 'prepare requires Linux for wait4 ru_maxrss semantics'
    validate_matrix
    require_clean_repository

    local requested_root=$1
    local virustic2_root=$2
    local megahit_archive=$3
    local spades_archive=$4
    local run_root
    run_root=$(validate_run_root_name "$requested_root")
    [[ ! -e "$run_root" && ! -L "$run_root" ]] || die "run root already exists: $run_root"
    [[ -d "$virustic2_root/.git" || -f "$virustic2_root/.git" ]] ||
        die 'Virustic2 source root is not a Git worktree'
    virustic2_root=$(CDPATH= cd -- "$virustic2_root" && pwd -P)
    [[ $(git -C "$virustic2_root" rev-parse HEAD) == "$pinned_virustic2_commit" ]] ||
        die 'Virustic2 checkout is not the pinned commit'
    [[ -z $(git -C "$virustic2_root" status --porcelain=v1 --untracked-files=all) ]] ||
        die 'Virustic2 checkout must be clean'
    [[ -f "$megahit_archive" && ! -L "$megahit_archive" ]] || die 'MEGAHIT release archive is missing or a symlink'
    [[ -f "$spades_archive" && ! -L "$spades_archive" ]] || die 'SPAdes release archive is missing or a symlink'
    megahit_archive=$(readlink -f "$megahit_archive")
    spades_archive=$(readlink -f "$spades_archive")
    [[ $(sha256_file "$megahit_archive") == "$megahit_archive_sha256" ]] || die 'MEGAHIT release archive SHA-256 mismatch'
    [[ $(sha256_file "$spades_archive") == "$spades_archive_sha256" ]] || die 'SPAdes release archive SHA-256 mismatch'

    mkdir "$run_root"
    mkdir "$run_root/build" "$run_root/datasets" "$run_root/freeze" "$run_root/logs"
    mkdir "$run_root/build/comparators"
    printf '%s\n' "$schema" >"$run_root/.veritasm-development-matrix-root"
    chmod 0400 "$run_root/.veritasm-development-matrix-root"

    local freeze="$run_root/freeze"
    local veritasm_target="$run_root/build/veritasm-target"
    local virustic2_target="$run_root/build/virustic2-target"
    local measure="$run_root/build/wait4_measure"
    tar -xzf "$megahit_archive" -C "$run_root/build/comparators"
    tar -xzf "$spades_archive" -C "$run_root/build/comparators"
    local megahit="$run_root/build/comparators/MEGAHIT-1.2.9-Linux-x86_64-static/bin/megahit"
    local megahit_core="$run_root/build/comparators/MEGAHIT-1.2.9-Linux-x86_64-static/bin/megahit_core_no_hw_accel"
    local spades="$run_root/build/comparators/SPAdes-4.3.0-Linux/bin/spades.py"
    local spades_core="$run_root/build/comparators/SPAdes-4.3.0-Linux/bin/spades-core"
    local spades_hammer="$run_root/build/comparators/SPAdes-4.3.0-Linux/bin/spades-hammer"
    [[ $(sha256_file "$megahit") == "$megahit_wrapper_sha256" ]] || die 'extracted MEGAHIT wrapper SHA-256 mismatch'
    [[ $(sha256_file "$megahit_core") == "$megahit_no_hw_core_sha256" ]] || die 'extracted MEGAHIT no-hw core SHA-256 mismatch'
    [[ $(sha256_file "$spades") == "$spades_wrapper_sha256" ]] || die 'extracted SPAdes wrapper SHA-256 mismatch'
    [[ $(sha256_file "$spades_core") == "$spades_core_sha256" ]] || die 'extracted SPAdes core SHA-256 mismatch'
    [[ $(sha256_file "$spades_hammer") == "$spades_hammer_sha256" ]] || die 'extracted SPAdes hammer SHA-256 mismatch'
    [[ $("$megahit" --version 2>&1) == 'MEGAHIT v1.2.9' ]] || die 'MEGAHIT version output mismatch'
    [[ $("$spades" --version 2>&1) == 'SPAdes genome assembler v4.3.0' ]] || die 'SPAdes version output mismatch'
    local veritasm_commit virustic2_commit source_tar_sha virustic2_tar_sha
    veritasm_commit=$(git -C "$repository_root" rev-parse HEAD)
    virustic2_commit=$(git -C "$virustic2_root" rev-parse HEAD)
    source_tar_sha=$(git -C "$repository_root" archive --format=tar HEAD | sha256_stdin)
    virustic2_tar_sha=$(git -C "$virustic2_root" archive --format=tar HEAD | sha256_stdin)

    {
        printf 'schema_version=%s\n' "$schema"
        printf 'kernel=%s\n' "$(uname -srvmo)"
        printf 'machine=%s\n' "$(uname -m)"
        printf 'path=%s\n' "$PATH"
        printf 'thread_environment=OMP_NUM_THREADS=1;OPENBLAS_NUM_THREADS=1;MKL_NUM_THREADS=1;NUMEXPR_NUM_THREADS=1;VECLIB_MAXIMUM_THREADS=1\n'
        printf 'python_hash_seed=0\n'
        printf 'cargo=%s\n' "$(cargo +stable --version)"
        printf 'rustc_begin\n'
        rustc +stable --version --verbose
        printf 'rustc_end\n'
        printf 'cc_path=%s\n' "$measurement_cc"
        printf 'cc_realpath=%s\n' "$(readlink -f "$measurement_cc")"
        printf 'cc_sha256=%s\n' "$(sha256_file "$(readlink -f "$measurement_cc")")"
        printf 'cc_version_begin\n'
        "$measurement_cc" --version
        printf 'cc_version_end\n'
        printf 'python_path=%s\n' "$(command -v python)"
        printf 'python_realpath=%s\n' "$(readlink -f "$(command -v python)")"
        printf 'python_sha256=%s\n' "$(sha256_file "$(readlink -f "$(command -v python)")")"
        printf 'python_version=%s\n' "$(python --version 2>&1)"
        printf 'cpu_begin\n'
        lscpu
        printf 'cpu_end\n'
        printf 'gnu_time=/usr/bin/time unavailable; wait4 wrapper used\n'
    } >"$freeze/environment.txt"

    local build_commands="$freeze/build-commands.jsonl"
    : >"$build_commands"
    append_build_command "$build_commands" wait4-measure "$repository_root" \
        not_captured not_captured '{}' "$measurement_cc" -std=c11 -O2 -Wall -Wextra -Werror \
        -pedantic "$measure_source" -o "$measure"
    append_build_command "$build_commands" benchmark-tools-self-test "$repository_root" \
        "$run_root/logs/benchmark-tools-self-test.stdout" \
        "$run_root/logs/benchmark-tools-self-test.stderr" \
        "$(jq -cn --arg megahit "$megahit" --arg spades "$spades" \
            '{MEGAHIT_BIN:$megahit,SPADES_BIN:$spades}')" \
        "$script_directory/self_test.sh"
    append_build_command "$build_commands" veritasm-release-bins "$repository_root" \
        "$run_root/logs/veritasm-build.stdout" "$run_root/logs/veritasm-build.stderr" \
        "$(jq -cn --arg target "$veritasm_target" \
            '{RUSTFLAGS:null,CARGO_ENCODED_RUSTFLAGS:null,CARGO_INCREMENTAL:"0",CARGO_TARGET_DIR:$target}')" \
        cargo +stable build --locked --release --bins
    append_build_command "$build_commands" virustic2-release-bin "$virustic2_root" \
        "$run_root/logs/virustic2-build.stdout" "$run_root/logs/virustic2-build.stderr" \
        "$(jq -cn --arg target "$virustic2_target" \
            '{RUSTFLAGS:null,CARGO_ENCODED_RUSTFLAGS:null,CARGO_INCREMENTAL:"0",CARGO_TARGET_DIR:$target}')" \
        cargo +stable build --locked --release --bin virustic2 \
        --manifest-path "$virustic2_root/Cargo.toml"

    "$measurement_cc" -std=c11 -O2 -Wall -Wextra -Werror -pedantic \
        "$measure_source" -o "$measure"
    MEGAHIT_BIN="$megahit" SPADES_BIN="$spades" "$script_directory/self_test.sh" \
        >"$run_root/logs/benchmark-tools-self-test.stdout" \
        2>"$run_root/logs/benchmark-tools-self-test.stderr"

    env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS CARGO_INCREMENTAL=0 \
        CARGO_TARGET_DIR="$veritasm_target" cargo +stable build --locked --release --bins \
        >"$run_root/logs/veritasm-build.stdout" 2>"$run_root/logs/veritasm-build.stderr"
    env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS CARGO_INCREMENTAL=0 \
        CARGO_TARGET_DIR="$virustic2_target" cargo +stable build --locked --release --bin virustic2 \
        --manifest-path "$virustic2_root/Cargo.toml" \
        >"$run_root/logs/virustic2-build.stdout" 2>"$run_root/logs/virustic2-build.stderr"

    local veritasm="$veritasm_target/release/veritasm"
    local multik="$veritasm_target/release/veritasm-multik"
    local simulate="$veritasm_target/release/veritasm-simulate"
    local evaluate="$veritasm_target/release/veritasm-evaluate"
    local virustic2="$virustic2_target/release/virustic2"
    for executable in "$veritasm" "$multik" "$simulate" "$evaluate" "$virustic2" "$measure" \
        "$megahit" "$megahit_core" "$spades" "$spades_core" "$spades_hammer"; do
        [[ -x "$executable" && ! -L "$executable" ]] || die "expected executable is missing: $executable"
    done

    printf 'name\tsha256\tbytes\tpath\n' >"$freeze/binaries.tsv"
    local name executable
    while IFS=$'\t' read -r name executable; do
        printf '%s\t%s\t%s\t%s\n' "$name" "$(sha256_file "$executable")" \
            "$(stat -c '%s' "$executable")" "$executable" >>"$freeze/binaries.tsv"
    done <<EOF
veritasm	$veritasm
veritasm-multik	$multik
veritasm-simulate	$simulate
veritasm-evaluate	$evaluate
virustic2	$virustic2
wait4-measure	$measure
megahit-wrapper	$megahit
megahit-core-no-hw	$megahit_core
spades-wrapper	$spades
spades-core	$spades_core
spades-hammer	$spades_hammer
EOF

    cp "$matrix_source" "$freeze/matrix.tsv"
    local datasets_jsonl="$freeze/datasets.jsonl"
    : >"$datasets_jsonl"
    local case_id replicate seed fragments truth_length read_length insert_length substitution low_quality compression extra
    while IFS=$'\t' read -r case_id replicate seed fragments truth_length read_length \
        insert_length substitution low_quality compression extra; do
        [[ "$case_id" != case_id ]] || continue
        local dataset_root="$run_root/datasets/$case_id"
        local generator_work="$run_root/generation/$case_id"
        local generation_stdout="$run_root/logs/generate-$case_id.stdout"
        local generation_stderr="$run_root/logs/generate-$case_id.stderr"
        mkdir -p "$generator_work"
        (
            cd "$generator_work"
            "$simulate" --out "$dataset_root" --case "$case_id" --replicate "$replicate" \
                --fragments "$fragments" --truth-length "$truth_length" \
                --read-length "$read_length" --insert-length "$insert_length" \
                --substitution-rate-ppm "$substitution" --low-quality-rate-ppm "$low_quality"
        ) >"$generation_stdout" 2>"$generation_stderr"
        [[ $(jq -r '.generator.seed_hex' "$dataset_root/dataset.json") == "$seed" ]] ||
            die "generated seed differs from matrix: $case_id"
        [[ $(jq -r '.case_id' "$dataset_root/dataset.json") == "$case_id" ]] ||
            die "generated case differs from matrix: $case_id"
        local expected_mode expected_reads
        if [[ "$case_id" == linear-se ]]; then
            expected_mode=single_end
            expected_reads=$fragments
        else
            expected_mode=paired_end
            expected_reads=$((fragments * 2))
        fi
        jq -e --arg case_id "$case_id" --arg replicate "$replicate" --arg seed "$seed" \
            --arg fragments "$fragments" --arg truth_length "$truth_length" \
            --arg read_length "$read_length" --arg insert_length "$insert_length" \
            --arg substitution "$substitution" --arg low_quality "$low_quality" \
            --arg compression "$compression" --arg mode "$expected_mode" \
            --arg reads "$expected_reads" '
            .case_id == $case_id and .replicate_decimal == $replicate and
            .generator.seed_hex == $seed and
            .generator.seed_source == "derived_from_case_and_replicate" and
            .generator.parameters.fragments_decimal == $fragments and
            .generator.parameters.truth_length_decimal == $truth_length and
            .generator.parameters.read_length_decimal == $read_length and
            .generator.parameters.insert_length_decimal == $insert_length and
            .generator.parameters.substitution_rate_ppm_decimal == $substitution and
            .generator.parameters.low_quality_rate_ppm_decimal == $low_quality and
            .generator.parameters.output_compression == $compression and
            .assembler_input.mode == $mode and
            .assembler_input.fragments_decimal == $fragments and
            .assembler_input.reads_decimal == $reads
        ' "$dataset_root/dataset.json" >/dev/null ||
            die "generated parameters differ from frozen matrix: $case_id"
        local manifest_root dataset_manifest_sha dataset_tree_manifest dataset_tree_sha input_json
        manifest_root=$(sha256_file "$dataset_root/manifest.sha256")
        dataset_manifest_sha=$(sha256_file "$dataset_root/dataset.json")
        dataset_tree_manifest="$freeze/dataset-$case_id.tree.tsv"
        canonical_tree_manifest "$dataset_root" "$dataset_tree_manifest"
        dataset_tree_sha=$(sha256_file "$dataset_tree_manifest")
        input_json=$(jq -c --arg root "$dataset_root" '
            [.assembler_input.read_paths[] as $path |
             .files[] | select(.path == $path) |
             {path:($root + "/" + .path),sha256:.sha256,bytes_decimal:.bytes_decimal,role:.role}]
        ' "$dataset_root/dataset.json")
        jq -cn --arg case_id "$case_id" --arg replicate "$replicate" --arg seed "$seed" \
            --arg dataset_root "$dataset_root" \
            --arg dataset_id "$(jq -r '.dataset_id' "$dataset_root/dataset.json")" \
            --arg dataset_manifest_sha "$dataset_manifest_sha" --arg content_root "$manifest_root" \
            --arg tree_manifest_sha "$dataset_tree_sha" \
            --arg truth_fasta_sha "$(jq -r '.files[] | select(.path=="evaluation_truth/truth.fasta") | .sha256' "$dataset_root/dataset.json")" \
            --argjson inputs "$input_json" \
            '{case_id:$case_id,replicate_decimal:$replicate,master_seed_hex:$seed,
              dataset_root:$dataset_root,dataset_id:$dataset_id,
              dataset_json_sha256:$dataset_manifest_sha,
              dataset_content_root_sha256:$content_root,
              canonical_tree_manifest_sha256:$tree_manifest_sha,
              truth_fasta_sha256:$truth_fasta_sha,assembler_inputs:$inputs,
              binding:"development_unbound_no_external_content_root"}' >>"$datasets_jsonl"
    done <"$freeze/matrix.tsv"

    create_frozen_commands "$run_root" "$veritasm" "$multik" "$simulate" "$evaluate" \
        "$virustic2" "$megahit" "$spades"
    validate_frozen_commands_file "$freeze/commands.jsonl"

    jq -n --slurpfile datasets "$datasets_jsonl" \
        --slurpfile commands "$freeze/commands.jsonl" \
        --slurpfile build_commands "$build_commands" \
        --arg schema "$schema" --arg veritasm_commit "$veritasm_commit" \
        --arg veritasm_source_tar_sha256 "$source_tar_sha" \
        --arg virustic2_commit "$virustic2_commit" \
        --arg virustic2_source_tar_sha256 "$virustic2_tar_sha" \
        --arg virustic2_source_root "$virustic2_root" \
        --arg megahit_version "$megahit_version" --arg megahit_url "$megahit_release_url" \
        --arg megahit_archive_path "$megahit_archive" --arg megahit_archive_sha256 "$megahit_archive_sha256" \
        --arg spades_version "$spades_version" --arg spades_url "$spades_release_url" \
        --arg spades_archive_path "$spades_archive" --arg spades_archive_sha256 "$spades_archive_sha256" \
        --arg matrix_sha256 "$(sha256_file "$freeze/matrix.tsv")" \
        --arg environment_sha256 "$(sha256_file "$freeze/environment.txt")" \
        --arg binaries_sha256 "$(sha256_file "$freeze/binaries.tsv")" \
        --arg build_commands_sha256 "$(sha256_file "$build_commands")" \
        --arg datasets_jsonl_sha256 "$(sha256_file "$datasets_jsonl")" \
        --arg commands_jsonl_sha256 "$(sha256_file "$freeze/commands.jsonl")" \
        --arg wait4_source_sha256 "$(sha256_file "$measure_source")" \
        --arg adapter_awk_sha256 "$(sha256_file "$script_directory/extract_k_child.awk")" \
        --arg adapter_shell_sha256 "$(sha256_file "$adapter_script")" \
        --arg harness_sha256 "$(sha256_file "$script_directory/run.sh")" \
        --arg timeout "$timeout_seconds" --arg grace "$timeout_grace_seconds" \
        '{schema_version:$schema,status:"frozen_before_assembly",
          claim_boundary:"development-only reconstruction comparison; no organism detection, absence, universal superiority, or production claim",
          source:{veritasm_commit:$veritasm_commit,veritasm_git_archive_sha256:$veritasm_source_tar_sha256,
                  virustic2_commit:$virustic2_commit,virustic2_source_root:$virustic2_source_root,
                  virustic2_git_archive_sha256:$virustic2_source_tar_sha256},
          comparators:{megahit:{version:$megahit_version,release_url:$megahit_url,
                               archive_path:$megahit_archive_path,archive_sha256:$megahit_archive_sha256,
                               configuration_scope:"standard defaults plus frozen k/count/thread/memory/no-hw/min-length controls"},
                       spades:{version:$spades_version,release_url:$spades_url,
                              archive_path:$spades_archive_path,archive_sha256:$spades_archive_sha256,
                              configuration_scope:"normal full pipeline with correction; frozen k/thread/memory/phred controls; no dataset-specific preset"}},
          identities:{matrix_sha256:$matrix_sha256,environment_sha256:$environment_sha256,
                      binaries_tsv_sha256:$binaries_sha256,wait4_source_sha256:$wait4_source_sha256,
                      build_commands_jsonl_sha256:$build_commands_sha256,
                      datasets_jsonl_sha256:$datasets_jsonl_sha256,
                      commands_jsonl_sha256:$commands_jsonl_sha256,
                      adapter_awk_sha256:$adapter_awk_sha256,adapter_shell_sha256:$adapter_shell_sha256,
                      harness_sha256:$harness_sha256},
          resources:{timeout_seconds_decimal:$timeout,termination_grace_seconds_decimal:$grace,
                     measurement:"CLOCK_MONOTONIC plus Linux wait4 direct-child rusage; descendant CPU is accumulated when reaped, while ru_maxrss is the Linux maximum rather than concurrent process-tree RSS; GNU /usr/bin/time unavailable"},
          build_commands:$build_commands,datasets:$datasets,commands:$commands,
          preregistration:{repetitions:"two timed serial repetitions per assembly arm",
             fixed_thread_diagnostic:"one additional t=2 fixed-k and Virustic2 run per case",
             multi_thread_contract:"one retained expected rejection for t=2",
             selection:"all outputs retained; no best arm or case selected"},
          limitations:["one derived seed per built-in case","tiny uniform-random synthetic truths",
             "multi-k diversity and exact-agreement FASTAs are distinct non-rank-equivalent presentations",
             "the extracted k31 child is diagnostic and receives no independent runtime",
             "generator and evaluator are development-unbound","startup and evidence I/O can dominate timing",
             "MEGAHIT uses iterative multi-k cleaning/mercy/local-assembly semantics and a 1,073,741,824-byte memory control",
             "SPAdes uses BayesHammer correction, graph transformations, repeat resolution, and a 1 GB memory control",
             "SPAdes applies its own contig reporting length behavior; this arm has no semantically exact min-contig-length control",
             "VeritAsm stable and multi-k report graph-unitig/segment products rather than SPAdes-style post-resolution contigs",
             "both third-party wrappers require Python and emit different evidence/output inventories",
             "no public biological datasets are included"]}' \
        >"$freeze/plan.json"
    local plan_sha
    plan_sha=$(sha256_file "$freeze/plan.json")
    printf '%s  plan.json\n' "$plan_sha" >"$freeze/plan.sha256"
    printf '%s\n' "$plan_sha" >"$freeze/PREPARED"
    chmod 0400 "$freeze"/*
    find "$run_root/datasets" -type f -exec chmod 0400 {} +
    find "$run_root/datasets" -type d -exec chmod 0500 {} +
    find "$run_root/build/comparators" -type f -perm /111 -exec chmod 0500 {} +
    find "$run_root/build/comparators" -type f ! -perm /111 -exec chmod 0400 {} +
    find "$run_root/build/comparators" -type d -exec chmod 0500 {} +
    chmod 0500 "$measure" "$veritasm" "$multik" "$simulate" "$evaluate" "$virustic2" \
        "$megahit" "$megahit_core" "$spades" "$spades_core" "$spades_hammer"
    printf 'prepared_plan_sha256=%s\n' "$plan_sha"
    printf 'assembly_cells_not_run=true\n'
}

verify_frozen_inputs() {
    local run_root=$1
    local expected_plan_sha=$2
    local freeze="$run_root/freeze"
    [[ -f "$run_root/.veritasm-development-matrix-root" && ! -L "$run_root/.veritasm-development-matrix-root" ]] ||
        die 'run-root ownership marker is missing'
    [[ $(<"$run_root/.veritasm-development-matrix-root") == "$schema" ]] ||
        die 'run-root ownership marker differs'
    [[ "$expected_plan_sha" =~ ^[0-9a-f]{64}$ ]] || die 'expected plan SHA-256 is not canonical'
    [[ -f "$freeze/plan.json" && ! -L "$freeze/plan.json" ]] || die 'frozen plan is missing'
    [[ $(sha256_file "$freeze/plan.json") == "$expected_plan_sha" ]] || die 'frozen plan SHA-256 mismatch'
    (cd "$freeze" && sha256sum -c plan.sha256 >/dev/null) || die 'plan checksum file failed'
    [[ -f "$freeze/PREPARED" && ! -L "$freeze/PREPARED" && \
        $(<"$freeze/PREPARED") == "$expected_plan_sha" ]] || die 'prepared marker differs from plan'
    [[ $(jq -r '.schema_version' "$freeze/plan.json") == "$schema" ]] || die 'plan schema differs'
    [[ $(jq -r '.status' "$freeze/plan.json") == frozen_before_assembly ]] || die 'plan was not frozen before assembly'
    [[ $(sha256_file "$matrix_source") == "$(jq -r '.identities.matrix_sha256' "$freeze/plan.json")" ]] ||
        die 'source matrix changed after prepare'
    [[ -f "$freeze/matrix.tsv" && ! -L "$freeze/matrix.tsv" && \
        $(sha256_file "$freeze/matrix.tsv") == "$(jq -r '.identities.matrix_sha256' "$freeze/plan.json")" ]] ||
        die 'frozen matrix changed after prepare'
    [[ -f "$freeze/environment.txt" && ! -L "$freeze/environment.txt" && \
        $(sha256_file "$freeze/environment.txt") == "$(jq -r '.identities.environment_sha256' "$freeze/plan.json")" ]] ||
        die 'frozen environment record changed after prepare'
    [[ $(sha256_file "$measure_source") == "$(jq -r '.identities.wait4_source_sha256' "$freeze/plan.json")" ]] ||
        die 'measurement source changed after prepare'
    [[ $(sha256_file "$script_directory/extract_k_child.awk") == "$(jq -r '.identities.adapter_awk_sha256' "$freeze/plan.json")" ]] ||
        die 'AWK adapter changed after prepare'
    [[ $(sha256_file "$adapter_script") == "$(jq -r '.identities.adapter_shell_sha256' "$freeze/plan.json")" ]] ||
        die 'adapter wrapper changed after prepare'
    [[ $(sha256_file "$script_directory/run.sh") == "$(jq -r '.identities.harness_sha256' "$freeze/plan.json")" ]] ||
        die 'matrix harness changed after prepare'
    require_clean_repository
    [[ $(git -C "$repository_root" rev-parse HEAD) == "$(jq -r '.source.veritasm_commit' "$freeze/plan.json")" ]] ||
        die 'VeritAsm commit changed after prepare'
    [[ $(git -C "$repository_root" archive --format=tar HEAD | sha256_stdin) == "$(jq -r '.source.veritasm_git_archive_sha256' "$freeze/plan.json")" ]] ||
        die 'VeritAsm source archive changed after prepare'
    local virustic2_root
    virustic2_root=$(jq -r '.source.virustic2_source_root' "$freeze/plan.json")
    [[ $(git -C "$virustic2_root" rev-parse HEAD) == "$pinned_virustic2_commit" ]] ||
        die 'pinned Virustic2 source commit changed after prepare'
    [[ -z $(git -C "$virustic2_root" status --porcelain=v1 --untracked-files=all) ]] ||
        die 'pinned Virustic2 source became dirty after prepare'
    [[ $(git -C "$virustic2_root" archive --format=tar HEAD | sha256_stdin) == "$(jq -r '.source.virustic2_git_archive_sha256' "$freeze/plan.json")" ]] ||
        die 'Virustic2 source archive changed after prepare'
    local comparator archive_path archive_sha
    for comparator in megahit spades; do
        archive_path=$(jq -r --arg comparator "$comparator" '.comparators[$comparator].archive_path' "$freeze/plan.json")
        archive_sha=$(jq -r --arg comparator "$comparator" '.comparators[$comparator].archive_sha256' "$freeze/plan.json")
        [[ -f "$archive_path" && ! -L "$archive_path" && $(sha256_file "$archive_path") == "$archive_sha" ]] ||
            die "$comparator release archive changed after prepare"
    done
    [[ $(sha256_file "$freeze/binaries.tsv") == "$(jq -r '.identities.binaries_tsv_sha256' "$freeze/plan.json")" ]] ||
        die 'frozen binary inventory changed after prepare'
    [[ -f "$freeze/build-commands.jsonl" && ! -L "$freeze/build-commands.jsonl" && \
        $(sha256_file "$freeze/build-commands.jsonl") == "$(jq -r '.identities.build_commands_jsonl_sha256' "$freeze/plan.json")" ]] ||
        die 'frozen build-command inventory changed after prepare'
    [[ -f "$freeze/datasets.jsonl" && ! -L "$freeze/datasets.jsonl" && \
        $(sha256_file "$freeze/datasets.jsonl") == "$(jq -r '.identities.datasets_jsonl_sha256' "$freeze/plan.json")" ]] ||
        die 'frozen dataset inventory changed after prepare'
    [[ -f "$freeze/commands.jsonl" && ! -L "$freeze/commands.jsonl" && \
        $(sha256_file "$freeze/commands.jsonl") == "$(jq -r '.identities.commands_jsonl_sha256' "$freeze/plan.json")" ]] ||
        die 'frozen command inventory changed after prepare'
    cmp <(jq -S -c '.build_commands' "$freeze/plan.json") \
        <(jq -S -c -s . "$freeze/build-commands.jsonl") >/dev/null ||
        die 'plan/build-command inventory disagreement'
    cmp <(jq -S -c '.datasets' "$freeze/plan.json") \
        <(jq -S -c -s . "$freeze/datasets.jsonl") >/dev/null ||
        die 'plan/dataset inventory disagreement'
    cmp <(jq -S -c '.commands' "$freeze/plan.json") \
        <(jq -S -c -s . "$freeze/commands.jsonl") >/dev/null ||
        die 'plan/command inventory disagreement'
    validate_frozen_commands_file "$freeze/commands.jsonl"

    local name digest bytes path
    while IFS=$'\t' read -r name digest bytes path; do
        [[ "$name" != name ]] || continue
        [[ -f "$path" && ! -L "$path" ]] || die "frozen executable is missing: $name"
        [[ $(sha256_file "$path") == "$digest" ]] || die "frozen executable hash changed: $name"
        [[ $(stat -c '%s' "$path") == "$bytes" ]] || die "frozen executable size changed: $name"
    done <"$freeze/binaries.tsv"

    local case_id dataset_root stored_manifest temporary_manifest
    while IFS= read -r case_id; do
        dataset_root=$(jq -r --arg case_id "$case_id" '.datasets[] | select(.case_id==$case_id) | .dataset_root' "$freeze/plan.json")
        stored_manifest="$freeze/dataset-$case_id.tree.tsv"
        [[ -f "$stored_manifest" && ! -L "$stored_manifest" && \
            $(sha256_file "$stored_manifest") == "$(jq -r --arg case_id "$case_id" '.datasets[] | select(.case_id==$case_id) | .canonical_tree_manifest_sha256' "$freeze/plan.json")" ]] ||
            die "frozen dataset tree manifest changed: $case_id"
        temporary_manifest=$(mktemp "/tmp/veritasm-development-matrix-verify-$case_id.XXXXXXXX")
        unlink -- "$temporary_manifest"
        canonical_tree_manifest "$dataset_root" "$temporary_manifest"
        if ! cmp "$stored_manifest" "$temporary_manifest"; then
            unlink -- "$temporary_manifest"
            die "dataset tree changed after prepare: $case_id"
        fi
        unlink -- "$temporary_manifest"
        [[ $(sha256_file "$dataset_root/dataset.json") == "$(jq -r --arg case_id "$case_id" '.datasets[] | select(.case_id==$case_id) | .dataset_json_sha256' "$freeze/plan.json")" ]] ||
            die "dataset manifest changed: $case_id"
        [[ $(sha256_file "$dataset_root/manifest.sha256") == "$(jq -r --arg case_id "$case_id" '.datasets[] | select(.case_id==$case_id) | .dataset_content_root_sha256' "$freeze/plan.json")" ]] ||
            die "dataset content root changed: $case_id"
    done < <(jq -r '.datasets[].case_id' "$freeze/plan.json")

}

execute_one_command() {
    local run_root=$1
    local id=$2
    local plan="$run_root/freeze/plan.json"
    local record
    record=$(jq -ce --arg id "$id" '.commands[] | select(.id==$id)' "$plan") ||
        die "frozen command not found: $id"
    local working stdout_log stderr_log metrics artifact expected timed phase arm
    working=$(jq -r '.working_directory' <<<"$record")
    stdout_log=$(jq -r '.stdout_log' <<<"$record")
    stderr_log=$(jq -r '.stderr_log' <<<"$record")
    metrics=$(jq -r '.metrics_file' <<<"$record")
    artifact=$(jq -r '.primary_artifact' <<<"$record")
    expected=$(jq -r '.expected_exit_code_decimal' <<<"$record")
    timed=$(jq -r '.timed' <<<"$record")
    phase=$(jq -r '.phase' <<<"$record")
    arm=$(jq -r '.arm' <<<"$record")
    mkdir -p "$working"
    if [[ "$phase" == assemble && "$arm" == megahit-1.2.9 ]]; then
        mkdir "$working/tmp"
    fi
    [[ ! -e "$stdout_log" && ! -L "$stdout_log" && ! -e "$stderr_log" && ! -L "$stderr_log" ]] ||
        die "command log already exists: $id"
    if [[ "$timed" == true ]]; then
        [[ "$metrics" != not_applicable && ! -e "$metrics" && ! -L "$metrics" ]] ||
            die "timed command metrics path is invalid: $id"
    else
        [[ "$metrics" == not_applicable ]] || die "untimed command unexpectedly has metrics: $id"
    fi

    local dependency
    while IFS= read -r dependency; do
        if [[ "${command_ok[$dependency]:-0}" != 1 ]]; then
            jq -cn --arg id "$id" --arg phase "$phase" --arg dependency "$dependency" \
                '{id:$id,phase:$phase,state:"skipped_dependency",dependency:$dependency}' \
                >>"$run_root/evidence/command-status.jsonl"
            command_ok[$id]=0
            return
        fi
    done < <(jq -r '.depends_on[]' <<<"$record")

    local -a argv
    mapfile -d '' -t argv < <(jq -j '.argv[] + "\u0000"' <<<"$record")
    ((${#argv[@]} > 0)) || die "empty argv in frozen command: $id"
    local status
    set +e
    if [[ "$timed" == true ]]; then
        (
            cd "$working"
            "$run_root/build/wait4_measure" --output "$metrics" \
                --timeout-seconds "$timeout_seconds" --grace-seconds "$timeout_grace_seconds" \
                -- "${argv[@]}"
        ) >"$stdout_log" 2>"$stderr_log"
        status=$?
    else
        (cd "$working" && "${argv[@]}") >"$stdout_log" 2>"$stderr_log"
        status=$?
    fi
    set -e

    local metrics_validation='not_applicable'
    if [[ "$timed" == true ]]; then
        metrics_validation='invalid_or_missing'
        if [[ -f "$metrics" && ! -L "$metrics" ]] && jq -e --arg status "$status" '
            .schema_version == "veritasm-linux-wait4-measure-v1" and
            .clock == "CLOCK_MONOTONIC" and
            .resource_api == "wait4_rusage_direct_child" and
            .max_rss_unit == "kibibytes_linux_ru_maxrss" and
            (.wall_nanoseconds_decimal | test("^[0-9]+$")) and
            (.user_microseconds_decimal | test("^[0-9]+$")) and
            (.system_microseconds_decimal | test("^[0-9]+$")) and
            (.max_rss_kib_decimal | test("^[0-9]+$")) and
            (.timed_out | type) == "boolean" and
            (if $status == "0" then
                .exit_kind == "exited" and .exit_code_decimal == "0" and
                .termination_signal_decimal == null and .timed_out == false
             else true end)
        ' "$metrics" >/dev/null; then
            metrics_validation='valid'
        fi
    fi

    local state='unexpected_exit'
    local artifact_sha='not_available'
    if [[ "$status" == "$expected" ]]; then
        if [[ "$expected" == 0 ]]; then
            if [[ "$timed" == true && "$metrics_validation" != valid ]]; then
                state='expected_exit_invalid_metrics'
                command_ok[$id]=0
            elif [[ -f "$artifact" && ! -L "$artifact" ]]; then
                state='completed_expected'
                artifact_sha=$(sha256_file "$artifact")
                command_ok[$id]=1
            else
                state='expected_exit_missing_artifact'
                command_ok[$id]=0
            fi
        elif [[ ! -e "$artifact" && ! -L "$artifact" ]]; then
            state='completed_expected_rejection'
            command_ok[$id]=1
        else
            state='expected_rejection_created_artifact'
            command_ok[$id]=0
        fi
    else
        command_ok[$id]=0
    fi
    jq -cn --arg id "$id" --arg phase "$phase" --arg state "$state" \
        --arg expected_exit "$expected" --arg actual_exit "$status" \
        --arg artifact "$artifact" --arg artifact_sha "$artifact_sha" \
        --arg metrics_validation "$metrics_validation" \
        --arg stdout_sha "$(sha256_file "$stdout_log")" \
        --arg stderr_sha "$(sha256_file "$stderr_log")" \
        --arg metrics_sha "$([[ -f "$metrics" ]] && sha256_file "$metrics" || printf not_available)" \
        '{id:$id,phase:$phase,state:$state,expected_exit_code_decimal:$expected_exit,
          actual_exit_code_decimal:$actual_exit,primary_artifact:$artifact,
          primary_artifact_sha256:$artifact_sha,stdout_sha256:$stdout_sha,
          stderr_sha256:$stderr_sha,metrics_validation:$metrics_validation,
          metrics_sha256:$metrics_sha}' \
        >>"$run_root/evidence/command-status.jsonl"
}

compare_trees() {
    local run_root=$1
    local check_id=$2
    local left=$3
    local right=$4
    local evidence="$run_root/evidence/determinism"
    mkdir -p "$evidence"
    if [[ ! -d "$left" || -L "$left" || ! -d "$right" || -L "$right" ]]; then
        printf '%s\ttree\tnot_comparable_missing_artifact\tnot_available\tnot_available\t%s\t%s\n' \
            "$check_id" "$left" "$right" >>"$run_root/evidence/determinism.tsv"
        return
    fi
    local safe_id=${check_id//:/_}
    local left_manifest="$evidence/$safe_id.left.tsv"
    local right_manifest="$evidence/$safe_id.right.tsv"
    canonical_tree_manifest "$left" "$left_manifest"
    canonical_tree_manifest "$right" "$right_manifest"
    local left_sha right_sha result
    left_sha=$(sha256_file "$left_manifest")
    right_sha=$(sha256_file "$right_manifest")
    result=different
    [[ "$left_sha" == "$right_sha" ]] && result=byte_identical
    printf '%s\ttree\t%s\t%s\t%s\t%s\t%s\n' "$check_id" "$result" \
        "$left_sha" "$right_sha" "$left" "$right" >>"$run_root/evidence/determinism.tsv"
}

compare_files() {
    local run_root=$1
    local check_id=$2
    local left=$3
    local right=$4
    if [[ ! -f "$left" || -L "$left" || ! -f "$right" || -L "$right" ]]; then
        printf '%s\tfile\tnot_comparable_missing_artifact\tnot_available\tnot_available\t%s\t%s\n' \
            "$check_id" "$left" "$right" >>"$run_root/evidence/determinism.tsv"
        return
    fi
    local left_sha right_sha result
    left_sha=$(sha256_file "$left")
    right_sha=$(sha256_file "$right")
    result=different
    [[ "$left_sha" == "$right_sha" ]] && result=byte_identical
    printf '%s\tfile\t%s\t%s\t%s\t%s\t%s\n' "$check_id" "$result" \
        "$left_sha" "$right_sha" "$left" "$right" >>"$run_root/evidence/determinism.tsv"
}

build_determinism_record() {
    local run_root=$1
    printf 'check_id\tkind\tresult\tleft_manifest_or_file_sha256\tright_manifest_or_file_sha256\tleft\tright\n' \
        >"$run_root/evidence/determinism.tsv"
    local case_id arm
    while IFS= read -r case_id; do
        compare_trees "$run_root" "$case_id::fixed::serial-repeat" \
            "$run_root/runs/$case_id/r1/veritasm-fixed-k31/bundle" \
            "$run_root/runs/$case_id/r2/veritasm-fixed-k31/bundle"
        compare_trees "$run_root" "$case_id::fixed::thread1-thread2" \
            "$run_root/runs/$case_id/r1/veritasm-fixed-k31/bundle" \
            "$run_root/runs/$case_id/thread2/veritasm-fixed-k31/bundle"
        compare_files "$run_root" "$case_id::virustic2::serial-repeat-fasta" \
            "$run_root/runs/$case_id/r1/virustic2-k31/assembly.fasta" \
            "$run_root/runs/$case_id/r2/virustic2-k31/assembly.fasta"
        compare_files "$run_root" "$case_id::virustic2::thread1-thread2-fasta" \
            "$run_root/runs/$case_id/r1/virustic2-k31/assembly.fasta" \
            "$run_root/runs/$case_id/thread2/virustic2-k31/assembly.fasta"
        compare_files "$run_root" "$case_id::megahit::serial-repeat-fasta" \
            "$run_root/runs/$case_id/r1/megahit-1.2.9/output/final.contigs.fa" \
            "$run_root/runs/$case_id/r2/megahit-1.2.9/output/final.contigs.fa"
        compare_files "$run_root" "$case_id::spades::serial-repeat-fasta" \
            "$run_root/runs/$case_id/r1/spades-4.3.0/output/contigs.fasta" \
            "$run_root/runs/$case_id/r2/spades-4.3.0/output/contigs.fasta"
        for arm in veritasm-multik-diversity veritasm-multik-exact-agreement; do
            compare_trees "$run_root" "$case_id::$arm::serial-repeat" \
                "$run_root/runs/$case_id/r1/$arm/bundle" \
                "$run_root/runs/$case_id/r2/$arm/bundle"
        done
        compare_files "$run_root" "$case_id::multik-k31-diagnostic::serial-repeat" \
            "$run_root/runs/$case_id/r1/veritasm-multik-k31-diagnostic/assembly.fasta" \
            "$run_root/runs/$case_id/r2/veritasm-multik-k31-diagnostic/assembly.fasta"
        for arm in veritasm-fixed-k31 veritasm-multik-diversity \
            veritasm-multik-exact-agreement virustic2-k31 megahit-1.2.9 spades-4.3.0 \
            veritasm-multik-k31-diagnostic; do
            compare_trees "$run_root" "$case_id::$arm::evaluation-repeat" \
                "$run_root/runs/$case_id/r1/$arm/evaluation" \
                "$run_root/runs/$case_id/r2/$arm/evaluation"
        done
    done < <(jq -r '.datasets[].case_id' "$run_root/freeze/plan.json")
}

command_state() {
    local run_root=$1
    local id=$2
    jq -rs --arg id "$id" '[.[] | select(.id==$id)][-1].state // "not_recorded"' \
        "$run_root/evidence/command-status.jsonl"
}

build_result_tables() {
    local run_root=$1
    local results="$run_root/evidence/results.tsv"
    local molecules="$run_root/evidence/per-molecule.tsv"
    printf 'case_id\trepetition\tarm\tassembly_command_state\tevaluation_state\tevaluation_status\tassembly_records\tassembly_bases\tcompatible_placement_status\tunique_coordinate_fraction\tcompatible_fraction_lower\tcompatible_fraction_upper\tmatches\tmismatches\tinserted_bases\tdeleted_bases\terror_rate\terror_rate_status\tconsensus_accuracy\tconsensus_accuracy_status\tqv\tqv_status\tunaligned_assembly_bases\tduplication_ratio\tduplication_status\tcorrect_junctions\tfalse_junctions\tindeterminate_ambiguous_flank\tindeterminate_unmapped_flank\tprimary_minor_chimera\twall_nanoseconds\tmax_rss_kib\tassembly_fasta_sha256\n' >"$results"
    printf 'case_id\trepetition\tarm\tmolecule_id\ttruth_class\ttopology\tcompatible_placement_status\tunique_coordinate_fraction\tcompatible_fraction_lower\tcompatible_fraction_upper\n' >"$molecules"

    local case_id repetition arm base command_id eval_id assembly_state eval_state result metrics fasta
    while IFS= read -r case_id; do
        for repetition in r1 r2; do
            for arm in veritasm-fixed-k31 veritasm-multik-diversity \
                veritasm-multik-exact-agreement virustic2-k31 megahit-1.2.9 spades-4.3.0 \
                veritasm-multik-k31-diagnostic; do
                base="$run_root/runs/$case_id/$repetition/$arm"
                if [[ "$arm" == veritasm-multik-k31-diagnostic ]]; then
                    command_id="adapter::$case_id::$repetition::$arm"
                    metrics=not_applicable
                    fasta="$base/assembly.fasta"
                else
                    command_id="assemble::$case_id::$repetition::$arm"
                    metrics="$base/measure.json"
                    case "$arm" in
                        veritasm-fixed-k31) fasta="$base/bundle/unitigs.fasta" ;;
                        veritasm-multik-*) fasta="$base/bundle/contigs.fasta" ;;
                        virustic2-k31) fasta="$base/assembly.fasta" ;;
                        megahit-1.2.9) fasta="$base/output/final.contigs.fa" ;;
                        spades-4.3.0) fasta="$base/output/contigs.fasta" ;;
                    esac
                fi
                eval_id="evaluate::$case_id::$repetition::$arm"
                assembly_state=$(command_state "$run_root" "$command_id")
                eval_state=$(command_state "$run_root" "$eval_id")
                result="$base/evaluation/result.json"
                local wall='not_applicable' rss='not_applicable' fasta_sha='not_available'
                if [[ -f "$metrics" && ! -L "$metrics" ]]; then
                    wall=$(jq -r '.wall_nanoseconds_decimal' "$metrics")
                    rss=$(jq -r '.max_rss_kib_decimal' "$metrics")
                fi
                if [[ -f "$fasta" && ! -L "$fasta" ]]; then
                    fasta_sha=$(sha256_file "$fasta")
                fi
                if [[ -f "$result" && ! -L "$result" ]]; then
                    jq -r --arg case_id "$case_id" --arg repetition "$repetition" \
                        --arg arm "$arm" --arg assembly_state "$assembly_state" \
                        --arg eval_state "$eval_state" --arg wall "$wall" --arg rss "$rss" \
                        --arg fasta_sha "$fasta_sha" '
                        def value_or_na: if .value_decimal == null then "NA" else .value_decimal end;
                        [$case_id,$repetition,$arm,$assembly_state,$eval_state,.status.code,
                         .assembly.records_decimal,.assembly.bases_decimal,
                         .base_metrics.recovery.compatible_placement_status,
                         (.base_metrics.recovery.unique_coordinate_truth_genome_fraction|value_or_na),
                         (.base_metrics.recovery.compatible_truth_genome_fraction_lower_bound|value_or_na),
                         (.base_metrics.recovery.compatible_truth_genome_fraction_upper_bound|value_or_na),
                         .base_metrics.matches_decimal,.base_metrics.mismatches_decimal,
                         .base_metrics.inserted_bases_decimal,.base_metrics.deleted_bases_decimal,
                         (.base_metrics.error_rate|value_or_na),.base_metrics.error_rate.status,
                         (.base_metrics.consensus_accuracy|value_or_na),.base_metrics.consensus_accuracy.status,
                         (.base_metrics.qv.value_decimal // "NA"),.base_metrics.qv.status,
                         .base_metrics.unaligned_assembly_bases_decimal,
                         (.base_metrics.aligned_duplication_ratio|value_or_na),
                         .base_metrics.aligned_duplication_ratio.status,
                         .junction_metrics.correct_decimal,.junction_metrics.false_decimal,
                         .junction_metrics.indeterminate_ambiguous_flank_decimal,
                         .junction_metrics.indeterminate_unmapped_flank_decimal,
                         .junction_metrics.primary_minor_chimera_decimal,
                         $wall,$rss,$fasta_sha] | @tsv
                    ' "$result" >>"$results"
                    jq -r --arg case_id "$case_id" --arg repetition "$repetition" --arg arm "$arm" '
                        def value_or_na: if .value_decimal == null then "NA" else .value_decimal end;
                        .per_molecule[] |
                        [$case_id,$repetition,$arm,.molecule_id,.truth_class,.topology,
                         .compatible_placement_status,
                         (.unique_coordinate_truth_genome_fraction|value_or_na),
                         (.compatible_truth_genome_fraction_lower_bound|value_or_na),
                         (.compatible_truth_genome_fraction_upper_bound|value_or_na)] | @tsv
                    ' "$result" >>"$molecules"
                else
                    printf '%s\t%s\t%s\t%s\t%s\tNOT_AVAILABLE' \
                        "$case_id" "$repetition" "$arm" "$assembly_state" "$eval_state" \
                        >>"$results"
                    local missing_field
                    for ((missing_field = 0; missing_field < 27; missing_field++)); do
                        printf '\tNA' >>"$results"
                    done
                    printf '\n' >>"$results"
                fi
            done
        done
    done < <(jq -r '.datasets[].case_id' "$run_root/freeze/plan.json")
}

write_development_report() {
    local run_root=$1
    local plan_sha unexpected failed_determinism
    plan_sha=$(sha256_file "$run_root/freeze/plan.json")
    unexpected=$(jq -s '[.[] | select(.state != "completed_expected" and .state != "completed_expected_rejection" and .state != "completed_during_prepare")] | length' \
        "$run_root/evidence/command-status.jsonl")
    failed_determinism=$(awk -F '\t' 'NR>1 && $3 != "byte_identical" {count++} END {print count+0}' \
        "$run_root/evidence/determinism.tsv")
    cat >"$run_root/evidence/DEVELOPMENT_MATRIX_RESULT.md" <<EOF
# Small generator-v4 development matrix

Status: executed development evidence; not a qualification scorecard.

- Frozen plan SHA-256: \`$plan_sha\`
- Unexpected, skipped, or artifact-missing command records: \`$unexpected\`
- Determinism comparisons not byte-identical (including missing artifacts): \`$failed_determinism\`
- Complete metric table: \`results.tsv\`
- Per-molecule recovery table: \`per-molecule.tsv\`
- Raw command states: \`command-status.jsonl\`
- Determinism record: \`determinism.tsv\`

The matrix includes every generator-v4 built-in at one predeclared small seed and
configuration. It compares fixed k=31, the two distinct multi-k presentation
profiles at k=21,31,51, pinned Virustic2 k=31, official MEGAHIT 1.2.9,
official SPAdes 4.3.0, and a diagnostic extraction of the multi-k k=31 child.
No arm or case is selected after scoring.

The diversity portfolio repeats child-scoped sequences across k and the exact-
agreement profile can deliberately omit singleton sequences. They are distinct,
non-rank-equivalent products, not interchangeable conventional assemblies. The
k31 extraction is diagnostic and has no independent runtime.

All evaluator-v4 records are \`development_unbound\`. These tiny synthetic,
uniform-random cases do not model real instruments, repeats, host background,
large data, biological detection, absence, a validated limit of detection, or
universal assembler superiority. Linux \`wait4\` reports direct-child
\`ru_maxrss\` in KiB; GNU \`/usr/bin/time\` was unavailable. Startup and
evidence-writing overhead can dominate these measurements.

MEGAHIT and SPAdes use materially different support, cleaning, correction,
repeat-resolution, memory-limit, process-tree, and output contracts. Their raw
wall/RSS and reconstruction rows are retained, but are not presented as if the
algorithms or work performed were identical.
EOF
}

write_evidence_root() {
    local run_root=$1
    local manifest="$run_root/evidence/artifact-manifest.tsv"
    local temporary="$run_root/evidence/.artifact-manifest.partial.$$"
    [[ ! -e "$manifest" && ! -e "$temporary" ]] || die 'evidence manifest already exists'
    local top
    for top in datasets freeze logs runs evidence; do
        [[ -z $(find "$run_root/$top" -type l -print -quit) ]] ||
            die "evidence subtree contains a symlink: $top"
        [[ -z $(find "$run_root/$top" ! -type d ! -type f -print -quit) ]] ||
            die "evidence subtree contains a non-regular filesystem entry: $top"
    done
    (
        cd "$run_root"
        for top in datasets freeze logs runs evidence; do
            find "$top" -type f -printf '%p\n'
        done | sort | while IFS= read -r relative; do
            case "$relative" in
                evidence/artifact-manifest.tsv|evidence/EVIDENCE_ROOT.sha256) continue ;;
            esac
            [[ "$relative" =~ ^[A-Za-z0-9._/-]+$ ]] || {
                printf 'nonportable evidence path: %q\n' "$relative" >&2
                exit 1
            }
            printf '%s\t%s\t%s\n' "$(sha256_file "$relative")" \
                "$(stat -c '%s' "$relative")" "$relative"
        done
    ) >"$temporary"
    mv "$temporary" "$manifest"
    printf '%s  artifact-manifest.tsv\n' "$(sha256_file "$manifest")" \
        >"$run_root/evidence/EVIDENCE_ROOT.sha256"
}

execute_matrix() {
    (($# == 2)) || { usage; exit 2; }
    require_tools
    [[ $(uname -s) == Linux ]] || die 'execute requires Linux for wait4 ru_maxrss semantics'
    local run_root
    run_root=$(validate_run_root_name "$1")
    local expected_plan_sha=$2
    [[ -d "$run_root" && ! -L "$run_root" ]] || die 'prepared run root does not exist'
    [[ ! -e "$run_root/EXECUTED" && ! -L "$run_root/EXECUTED" ]] || die 'run root was already executed'
    [[ ! -e "$run_root/.execute.lock" && ! -L "$run_root/.execute.lock" ]] || die 'execution lock already exists'
    mkdir "$run_root/.execute.lock"
    verify_frozen_inputs "$run_root" "$expected_plan_sha"
    mkdir "$run_root/evidence" "$run_root/runs"
    {
        printf 'schema_version=%s-execution-environment-v1\n' "$schema"
        printf 'frozen_plan_sha256=%s\n' "$expected_plan_sha"
        printf 'kernel=%s\n' "$(uname -srvmo)"
        printf 'machine=%s\n' "$(uname -m)"
        printf 'path=%s\n' "$PATH"
        printf 'thread_environment=OMP_NUM_THREADS=1;OPENBLAS_NUM_THREADS=1;MKL_NUM_THREADS=1;NUMEXPR_NUM_THREADS=1;VECLIB_MAXIMUM_THREADS=1\n'
        printf 'python_hash_seed=0\n'
        printf 'python_path=%s\n' "$(command -v python)"
        printf 'python_realpath=%s\n' "$(readlink -f "$(command -v python)")"
        printf 'python_sha256=%s\n' "$(sha256_file "$(readlink -f "$(command -v python)")")"
        printf 'python_version=%s\n' "$(python --version 2>&1)"
        printf 'cpu_begin\n'
        lscpu
        printf 'cpu_end\n'
    } >"$run_root/evidence/execution-environment.txt"
    : >"$run_root/evidence/command-status.jsonl"
    declare -A command_ok=()

    local id case_id artifact
    while IFS= read -r id; do
        case_id=${id#generate::}
        artifact="$run_root/datasets/$case_id/dataset.json"
        command_ok[$id]=1
        jq -cn --arg id "$id" --arg artifact "$artifact" \
            --arg artifact_sha "$(sha256_file "$artifact")" \
            '{id:$id,phase:"generation",state:"completed_during_prepare",
              expected_exit_code_decimal:"0",actual_exit_code_decimal:"0",
              primary_artifact:$artifact,primary_artifact_sha256:$artifact_sha}' \
            >>"$run_root/evidence/command-status.jsonl"
    done < <(jq -r '.commands[] | select(.phase=="generation") | .id' "$run_root/freeze/plan.json")

    while IFS= read -r id; do
        execute_one_command "$run_root" "$id"
    done < <(jq -r '.commands | map(select(.phase=="assemble")) | sort_by(.order_key)[] | .id' \
        "$run_root/freeze/plan.json")
    for phase in determinism negative adapter evaluate; do
        while IFS= read -r id; do
            execute_one_command "$run_root" "$id"
        done < <(jq -r --arg phase "$phase" '.commands | map(select(.phase==$phase)) | sort_by(.id)[] | .id' \
            "$run_root/freeze/plan.json")
    done

    build_determinism_record "$run_root"
    build_result_tables "$run_root"
    write_development_report "$run_root"
    write_evidence_root "$run_root"
    local evidence_root
    evidence_root=$(awk '{print $1}' "$run_root/evidence/EVIDENCE_ROOT.sha256")
    printf '%s\n' "$evidence_root" >"$run_root/EXECUTED"
    chmod 0400 "$run_root/EXECUTED" "$run_root/evidence/EVIDENCE_ROOT.sha256"
    printf 'execution_complete=true\n'
    printf 'frozen_plan_sha256=%s\n' "$expected_plan_sha"
    printf 'evidence_manifest_sha256=%s\n' "$evidence_root"
    printf 'unexpected_or_skipped_commands=%s\n' \
        "$(jq -s '[.[] | select(.state != "completed_expected" and .state != "completed_expected_rejection" and .state != "completed_during_prepare")] | length' "$run_root/evidence/command-status.jsonl")"
}

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
    return 0
fi

case "${1:-}" in
    check)
        (($# == 1)) || { usage; exit 2; }
        require_tools
        [[ $(uname -s) == Linux ]] || die 'check requires Linux'
        validate_matrix
        "$script_directory/self_test.sh"
        printf 'development matrix static checks: PASS\n'
        ;;
    prepare)
        shift
        prepare "$@"
        ;;
    execute)
        shift
        execute_matrix "$@"
        ;;
    *)
        usage
        exit 2
        ;;
esac
