# Deterministically extract one child k from veritasm-multik segments.fasta.
# Usage: awk -v expected_k=31 -f extract_k_child.awk segments.fasta

BEGIN {
    if (expected_k !~ /^[0-9][0-9]*$/ || expected_k + 0 < 3 || expected_k + 0 > 63) {
        print "extract_k_child: expected_k must be a canonical integer in 3..63" > "/dev/stderr"
        exit 64
    }
    seen_header = 0
    keep = 0
    failed = 0
}

/^>/ {
    if (seen_header && record_bases == 0) {
        print "extract_k_child: segment header has no sequence" > "/dev/stderr"
        failed = 1
    }
    seen_header = 1
    keep = 0
    record_bases = 0
    k_count = 0
    field_count = split(substr($0, 2), fields, /[[:space:]]+/)
    if (field_count != 7 || length(fields[1]) != 68 ||
        fields[1] !~ /^mks-[0-9a-f][0-9a-f]*$/ ||
        fields[3] !~ /^topology=(linear|closed_walk)$/ ||
        length(fields[4]) != 75 || fields[4] !~ /^child_root=[0-9a-f][0-9a-f]*$/ ||
        fields[5] != "status=experimental" ||
        fields[6] != "qualification=unqualified" ||
        fields[7] != "intended_use=research_use_only") {
        print "extract_k_child: input is not a canonical multi-k segment header" > "/dev/stderr"
        failed = 1
        next
    }
    for (field_index = 2; field_index <= field_count; field_index++) {
        if (fields[field_index] ~ /^k=[0-9][0-9]*$/) {
            k_count++
            split(fields[field_index], pair, /=/)
            observed_k = pair[2]
        }
    }
    if (k_count != 1) {
        print "extract_k_child: segment header does not contain exactly one canonical k field" > "/dev/stderr"
        failed = 1
        next
    }
    keep = (observed_k == expected_k)
    if (keep) {
        print $0
    }
    next
}

{
    if (!seen_header) {
        if ($0 == "") {
            next
        }
        print "extract_k_child: sequence appears before the first header" > "/dev/stderr"
        failed = 1
        next
    }
    if ($0 !~ /^[ACGT]+$/) {
        print "extract_k_child: sequence line is not uppercase nonempty A/C/G/T" > "/dev/stderr"
        failed = 1
        next
    }
    record_bases += length($0)
    if (keep) {
        print $0
    }
}

END {
    if (seen_header && record_bases == 0) {
        print "extract_k_child: final segment header has no sequence" > "/dev/stderr"
        failed = 1
    }
    if (failed) {
        exit 65
    }
}
