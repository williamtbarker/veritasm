# VeritAsm fixed-k assembly configuration contract v0.1

Status: normative architecture input. Defaults are conservative engineering starting points, not
evidence of dataset suitability or performance.

## Scientific configuration

| Field | Type/default | Valid domain | Meaning |
|---|---|---|---|
| `k` | integer, `31` | 3 through 63 | One exact graph word size per run |
| `profile` | enum, `thresholded` | `retain_all`, `thresholded`, `custom` | Serialized preset name; no analytical sensitivity or biological-safety meaning |
| `support_unit` | enum, `supplied_fragment_instance` | `supplied_fragment_instance`, `accepted_window_occurrence` | Unit of the exact counter and retention threshold |
| `min_support` | integer, `2` in `thresholded`; `1` in `retain_all` | 1 through `u64::MAX` | Inclusive exact retention threshold in `support_unit` |
| `min_base_quality` | integer, `20` | 0 through 93 | Reject a k-mer window containing a FASTQ base below this Phred+33 value; not applied to FASTA |
| `remap` | boolean, `true` | true/false | Run zero-mismatch same-construction-read internal-consistency remapping |

The profile is an explicit CLI choice. `retain_all` fixes `min_support=1`; supplying another value is
`configuration_profile_conflict`. `thresholded` defaults to 2 and accepts an explicit value of 2 or
greater. `custom` requires an explicit `min_support` of 1 or greater. `support_unit`,
`min_base_quality`, and `remap` are orthogonal explicit fields and never change a profile name
silently. The effective configuration in `run.json` records both the selected profile and every
primitive field. There is no tip, bubble, consensus, correction, scaffold,
multi-k, host-subtraction, or taxonomy parameter in the stable 0.1 command.

## Resource configuration

All byte quantities are binary bytes. Resource-size arithmetic and material `u64 -> usize`
conversions are checked before the associated allocation or I/O. CLI values may lower or raise
defaults within the stated hard domain; successful processing at a configured limit is not a
performance claim. `memory_budget_bytes` governs conservative component/phase admission estimates. It
is not a hard process-RSS cap: caller-owned unitig/graph data remains live during audit without a
central lease, and allocator metadata, runtime/library state, stack guard pages, and small fixed
objects are not all charged to it. The indexed audit mapper plans its catalog and q=15 postings before
allocation, shares the audit-persistent one-eighth allowance, checks constructed vector capacities,
and receives a caller-bounded allowance for each query. Query admission covers the simultaneous
reverse-complement, compact-placement, materialized-group, and identifier peak; actual Vec/String
capacities are checked after each fallible reservation. Rayon worker stacks are fixed at 1 MiB each
and their total reservation is separately limited to one quarter of this budget.

The stable mapper does not fall back to the former whole-target exhaustive scan when its q=15 index
cannot fit the audit-persistent share. It fails with `resource_memory`; users may explicitly disable
construction-read remapping, but doing so makes the placement and pair evidence unavailable. This is
a fail-closed resource policy, not a scalability or performance claim.

| Field | Default | Hard domain | Scope and enforcement |
|---|---:|---:|---|
| `max_header_bytes` | 1 MiB | 1..64 MiB | Per FASTA/FASTQ header before allocation grows beyond the limit |
| `max_read_bases` | 10 MiB | 1..1 GiB | Per sequence and per quality string |
| `max_record_bytes` | 24 MiB | 2..2 GiB | Decoded record span defined below, enforced incrementally before the next byte is buffered |
| `max_raw_transport_bytes` | 1 TiB | 1 MiB..`u64::MAX` | Aggregate physical bytes over every logical input role before decompression; file metadata can reject early, while the counted reader is authoritative |
| `max_decoded_input_bytes` | 1 TiB | 1 MiB..`u64::MAX` | Aggregate uncompressed bytes over every logical input role |
| `max_gzip_members` | 1,000,000 | 1..`u64::MAX` | Aggregate concatenated gzip members across every gzip source, including empty members |
| `batch_fragments` | 4,096 | 1..1,048,576 | Maximum fragments queued in one ordinal batch |
| `memory_budget_bytes` | 512 MiB | 32 MiB..64 GiB | Input to conservative phase/component admission checks for parsing, scan vectors, counting, graph/compaction, audit, pair aggregation, and reporting; graph/compaction admission subtracts live pipeline metadata; not a central token pool or RSS cap |
| `max_spool_bytes` | 1 TiB | 1 MiB..`u64::MAX` | Complete private immutable spool |
| `max_temp_bytes` | 2 TiB | 1 MiB..`u64::MAX` | Aggregate spool, raw runs, merged runs, and staged output while each exists; checked at every growth |
| `partition_prefix_bits` | 6 | 0..`min(8,2*k)` | Numeric range partitions; `P=2^p` |
| `sort_buffer_keys` | 1,048,576 | 1,024..268,435,456 | Maximum full-key events in one exact-count sort buffer, also constrained by the count-phase allocation estimate |
| `merge_fan_in` | 16 | fixed at 16 in 0.1 | Maximum input runs in one merge group |
| `max_count_open_files` | 17 | fixed at 17 in 0.1 | At most 16 merge inputs plus one payload output inside the count stage; other process descriptors are outside this number, and `EMFILE`/`ENFILE` fail closed |
| `max_runs` | 1,000,000 | 1..10,000,000 | Aggregate raw and intermediate count runs before another is created |
| `max_manifest_bytes` | 1 GiB | 1 MiB..16 GiB | Append-only temporary count manifest |
| `max_retained_kmers` | 10,000,000 | 1..500,000,000 | Exact count checked before graph allocation; never raises `min_support` automatically |
| `max_mapping_candidates` | 10,000 | 1..10,000,000 | Maximum distinct accepted zero-mismatch placement groups per eligible read; accepted group `limit+1` creates an indeterminate state |
| `max_staged_output_bytes` | 100 GiB | 1 MiB..`u64::MAX` | Entire result staging directory before commit |
| `html_max_unitig_rows` | 1,000 | 0..100,000 | Deterministic presentation subset only; TSV retains all rows and HTML states the omitted count |
| `threads` | available parallelism, capped by stack admission | 1..min(1,024, floor(`memory_budget_bytes` / 4 MiB)) | Operational only, excluded from committed artifacts and scientific choices; each worker requests a fixed 1 MiB stack |

An implementation may discover that these defaults are impractical. It must change them through an
explicit decision and measured tests, not by exceeding them internally.

## FASTX corner semantics

- Sources with zero records are `input_empty` errors. A nonempty valid source whose reads yield no
  retained k-mer produces `software_run_complete_no_unitigs_under_parameters`.
- Empty FASTA/FASTQ sequences are errors. Blank lines between FASTA records and before the first record
  are ignored. Blank lines within a FASTA record are ignored but do not make an otherwise empty record
  nonempty. Blank FASTQ sequence or quality lines are invalid rather than skipped.
- CRLF is accepted and the CR belonging to the line ending is removed. Other leading/trailing ASCII
  whitespace on sequence lines is ignored for baseline compatibility; whitespace inside sequence
  text is an invalid nucleotide. FASTQ quality bytes are not trimmed except the line ending.
- ASCII lowercase sequence is normalized to uppercase. Accepted DNA symbols are
  `A C G T R Y S W K M B D H V N` in either case. IUPAC ambiguity breaks affected k-mer windows. `U`,
  gap symbols, digits, non-ASCII, and all other bytes are invalid.
- FASTQ quality bytes are ASCII 33 through 126 and sequence/quality lengths must match after sequence
  normalization. The configured threshold partitions possible windows into `accepted`,
  `ambiguity_only`, `quality_only`, and `ambiguity_and_quality`; these four counts sum exactly.
- FASTA has no qualities. `min_base_quality` is recorded as `not_applicable_to_fasta`; it never rejects
  a FASTA window.
- Header payloads are arbitrary non-newline bytes bounded by `max_header_bytes`; they need not be
  UTF-8. Identity tokenization recognizes ASCII whitespace only and compares normalized identity
  bytes exactly. An identity that is empty before or after stripping a recognized mate suffix is an
  error. Reports retain only sanitized role labels and header/identity digests, not raw header text.
- A nonempty FASTQ `+` payload is parsed with the same first-token and mate-role grammar as the `@`
  header. Its normalized identity and any stated role must agree; later description text is ignored.
  A bare `+` is valid.
- Different lanes may mix FASTA and FASTQ. The two roles within one paired lane must use the same
  format. Quality-window accounting records a per-source applicability state and aggregates FASTA and
  FASTQ contributions without applying a quality threshold to FASTA.
- Gzip is detected only by the complete two-byte magic prefix. Concatenated members are accepted.
  A truncated member, invalid checksum, or any trailing non-gzip byte is an error.
- Raw transport bytes are counted before content-selected decoding for both files and standard input.
  The inclusive aggregate limit permits exactly `max_raw_transport_bytes` bytes and probes at most
  one byte beyond it before `input_raw_transport_limit`. Each gzip magic-delimited member is admitted
  before decompression; the inclusive aggregate member limit therefore bounds empty-member storms.

`max_header_bytes` counts header payload bytes after the `>` or `@` marker and before CR/LF; the marker
and line ending do not count. `max_read_bases` counts normalized sequence symbols only. In contrast,
`max_record_bytes` counts every decoded source byte in the record span, including the initial marker,
header payload, CR and/or LF, wrapped sequence bytes, ignored sequence-line edge whitespace, FASTQ `+`
marker and payload, and quality bytes. A FASTA span begins at `>` and ends immediately before the next
record's `>` or at EOF; blank lines after at least one sequence line remain in that span. A FASTQ span
begins at `@` and ends after the final quality line ending, or at EOF after the final quality byte;
blank lines outside a completed FASTQ record are not part of either adjacent record but still count
toward `max_decoded_input_bytes`. The parser increments both decoded-input and current-record counters
as bytes are consumed, fails before buffering a byte that would make a counter exceed its limit, and
does not allocate an unbounded physical line.

A run is globally `single_end` or `paired_end`. All lanes have that arity; mixed arity is
`configuration_unsupported_combination`. Different lanes may still mix FASTA and FASTQ as specified
above.

## Stable error codes

Diagnostics may add context, but the code and exit class do not change without a schema-major change.

| Exit | Error-code family | Examples |
|---:|---|---|
| 2 | `configuration_*` | invalid k, support unit/threshold conflict, zero/overflowing resource value, unsupported feature combination |
| 3 | `input_*` | open/read/decompression failure, empty source, malformed FASTA/FASTQ, invalid nucleotide/quality, raw/decoded byte limit, gzip-member limit |
| 4 | `pair_*` | lane-count, format, identity, role, order, or cardinality mismatch; physical-source reuse |
| 5 | `resource_*` | memory token, spool, temporary bytes, file descriptor, run count, graph key, mapping candidate strictness, output bytes |
| 6 | `integrity_*` | spool/run/manifest checksum, schema, ordinal coverage, duplicate range, incompatible temporary state |
| 7 | `destination_*` | unsafe/colliding path, existing destination, live/stale lock requiring review, no-replace unsupported |
| 8 | `commit_*` | pre-commit write/flush/sync/parse/manifest/no-replace rename failure |
| 70 | `internal_*` | violated invariant or otherwise unclassified software failure |

Candidate-limit remapping is normally a recorded indeterminate state rather than a failed run. A
future strict-audit option may promote it to `resource_mapping_candidates`, but that option is not in
stable 0.1.

The current stable codes are below. The CLI prints `error[<code>]: <context>` to stderr. Context may
grow more specific without changing the code or exit class.

| Family | Stable codes |
|---|---|
| configuration | `configuration_invalid_k`, `configuration_profile_conflict`, `configuration_invalid_support`, `configuration_invalid_limit`, `configuration_unsupported_combination` |
| input | `input_open`, `input_read`, `input_empty`, `input_format`, `input_header`, `input_fasta_structure`, `input_fastq_structure`, `input_nucleotide`, `input_quality`, `input_decompression`, `input_raw_transport_limit`, `input_decoded_limit`, `input_gzip_member_limit` |
| pair | `pair_lane_count`, `pair_format`, `pair_identity`, `pair_role`, `pair_order_or_cardinality`, `pair_physical_source_reuse` |
| resource | `resource_memory`, `resource_spool_bytes`, `resource_temporary_bytes`, `resource_open_files`, `resource_run_count`, `resource_manifest_bytes`, `resource_retained_keys`, `resource_mapping_candidates`, `resource_output_bytes`, `resource_integer_overflow` |
| integrity | `integrity_spool`, `integrity_count_run`, `integrity_manifest`, `integrity_ordinal_coverage`, `integrity_schema`, `integrity_artifact` |
| destination | `destination_unsafe_path`, `destination_input_collision`, `destination_existing`, `destination_locked`, `destination_no_replace_unsupported` |
| commit | `commit_write`, `commit_flush`, `commit_sync`, `commit_reopen`, `commit_validate`, `commit_manifest`, `commit_rename_no_replace` |
| internal | `internal_invariant`, `internal_unexpected` |

A final no-replace `EEXIST` maps to `destination_existing`; an unavailable platform/filesystem
primitive maps to `destination_no_replace_unsupported`; another rename failure maps to
`commit_rename_no_replace`. Malformed input is classified by the narrowest applicable input code and
never converted to an empty successful run.

## Deterministic JSON and `NA`

JSON objects are serialized from declared structs in documented field order; map-like content uses
sorted arrays or `BTreeMap`. Integers remain JSON integers only through the exact range supported by
the schema; full `u64` values that might lose precision in consumers are decimal strings with an
`_decimal` suffix. No NaN or infinity is serialized. A missing measurement is an object
`{"status":"not_available","reason":"<stable_reason>"}`, never zero, an empty string, or a
platform-dependent null.
