# Output schema contract

Status: implemented normative 0.1 schema contract for the unreleased review candidate. The source and
each committed evidence bundle ship matching machine-readable schema files; release stability still
depends on the external qualification record.

## Common conventions

- UTF-8, Unix LF, final newline, decimal integers without separators.
- TSV files use one header row, no comments, tabs as delimiters, and JSON escaping only in explicitly
  JSON-valued columns.
- Missing measurements are `NA`; zero means the quantity was measured and found to be zero.
- Coordinates are zero-based, half-open unless a field explicitly says otherwise.
- Strand is `+` or `-`; unitig ends are `L` or `R`.
- Stable rows have an explicitly documented total sort key.
- Bundle and `run.json` schema version 1.2 adds effective aggregate raw-transport and gzip-member
  limits. Version 1.1 added lane-isolated paired-read evidence. FASTA,
  GFA, and unitig-evidence schema 1.1 also require `edge_steps == canonical_kmers` after the
  self-complemental-edge compaction repair; `pair_links.tsv` is artifact schema 1.1. Unchanged
  pair-summary and transformation rows remain artifact schema 1.0. Consumers must validate each
  artifact's shipped descriptor instead of assuming one row version across the bundle.
- Stable artifacts never contain timestamps, thread counts, timing, memory addresses, hash-map order,
  input absolute paths, host names, or temporary paths.

`exact` means full encoded sequence identity and checked integer counting under the recorded parser,
base-quality, support, graph, and mapping rules. It does not establish biological truth, true sequence
origin, input independence, or complete observation of a sample.

## `unitigs.fasta`

Records are sorted by descending sequence length, then canonical sequence bytes, then stable unitig ID.
Each sequence is the representative of the exact reverse-complement orbit defined in
`ARCHITECTURE.md`. Lines are wrapped at 80 bases. The ID grammar is `utg-` followed by all 64 lowercase
hexadecimal characters of the identity digest defined there; traversal or thread order is not part of
identity. A digest collision between unequal identity preimages is an internal fatal error.

The literal header is
`><unitig_id> schema_version=1.1 length_bases=<u64> k=<u8> topology=<topology> edge_steps=<u64> canonical_kmers=<u64> support_unit=<support_unit> retention_min_support=<u64> minimum_represented_key_support=<u64> lower_median_represented_key_support=<u64> maximum_represented_key_support=<u64> placement_enumeration_status=<status> sequence_sha256=<lowercase_sha256>`.
A closed one-in/one-out walk with `n=edge_steps` is emitted as a deterministic linear spelling of
length `n + k - 1`, including the `k-1` closing context. Its header says
`topology=closed_graph_walk`, never `circular=true`. Construction-read remapping fields for such a
record are `NA` with reason `closed_walk_audit_unsupported` in 0.1. With no unitigs, `unitigs.fasta` is
the zero-byte file; the common final-newline rule applies only to nonempty line-oriented artifacts.

## `assembly.gfa`

The stable file is the interoperable GFA 1.0 subset with `H`, `S`, and `L` records. It starts with the
literal fields
`H<TAB>VN:Z:1.0<TAB>PN:Z:veritasm<TAB>PV:Z:<package-version><TAB>SC:Z:1.1`. `S` lines are
`S<TAB>id<TAB>sequence<TAB>TP:Z:<topology><TAB>ES:i:<edge_steps><TAB>CK:i:<canonical_kmers><TAB>SH:H:<uppercase_sha256>`.
`ES` and `CK` are documented VeritAsm user tags. Their signed GFA integers are safe because `CK` is
bounded by `max_retained_kmers <= 500000000` and `ES` is separately bounded by 1000000000; both are
below `i32::MAX`, and exceeding either bound fails before commit. `SH:H` is the 32 sequence-digest bytes encoded as exactly 64
uppercase hexadecimal characters, while TSV/JSON digests remain lowercase. `L` lines are
`L<TAB>from<TAB>orientation<TAB>to<TAB>orientation<TAB><decimal-k-minus-one>M`. Version 0.1 emits no
`J` records; pair observations remain in TSV until distance semantics have an accepted schema and
independent-parser tests. An empty graph contains the H line only.

Records appear as the H line, all S lines in the exact FASTA/evidence row order, then all L lines.
Segments and links use the oriented-walk mapping defined in `ARCHITECTURE.md`. Links are sorted by the canonical tuple
`(segment_a, orientation_a, segment_b, orientation_b, overlap)`. Duplicate reverse-complement links
are represented once under the schema's canonicalization rule; hairpin and self-links are retained.
Before commit, the built-in streaming validator reconstructs the exact ordered H/S/L bytes from typed
bundle data and checks every oriented suffix/prefix overlap for exactly `k-1` bases. Compatibility with
an independent GFA parser remains a release-validation task and must not be inferred from that internal
check alone.

## `unitig_evidence.tsv`

The literal column order is:

`schema_version`, `unitig_id`, `length_bases`, `k`, `topology`, `edge_steps`, `canonical_kmers`,
`support_unit`, `retention_min_support`, `minimum_represented_key_support`,
`lower_median_represented_key_support`, `maximum_represented_key_support`,
`enumeration_complete_read_placements`,
`single_group_read_instances`, `multi_group_read_instances_with_group`,
`placement_enumeration_status`, `sequence_sha256`.

Required column meanings:

| Column | Meaning |
|---|---|
| `schema_version` | Row schema version |
| `unitig_id` | Stable content-derived identifier |
| `length_bases` | Sequence length |
| `k` | Graph k |
| `topology` | `linear` or `closed_graph_walk` |
| `edge_steps` | Number of oriented-handle steps in the selected representative walk; `length_bases=edge_steps+k-1` |
| `canonical_kmers` | Number of distinct retained canonical k-mers represented, each counted once for support summaries |
| `support_unit` | `supplied_fragment_instance` or `accepted_window_occurrence` |
| `retention_min_support` | Inclusive run configuration threshold in the selected support unit |
| `minimum_represented_key_support` | Minimum selected-unit exact support across represented distinct canonical keys |
| `lower_median_represented_key_support` | Deterministic lower median selected-unit support across represented distinct canonical keys |
| `maximum_represented_key_support` | Maximum selected-unit exact support across represented distinct canonical keys |
| `enumeration_complete_read_placements` | Accepted groups on this unitig from enumeration-complete read instances; not true-origin assignments |
| `single_group_read_instances` | Read instances whose sole accepted group is on this unitig |
| `multi_group_read_instances_with_group` | Read instances with multiple groups that include this unitig, deduplicated once per read and unitig |
| `placement_enumeration_status` | `placement_enumeration_complete`, `unavailable_closed_walk_audit_unsupported`, `unavailable_remap_disabled`, or `indeterminate_candidate_limit`; not an assembly-validation status |
| `sequence_sha256` | Full lowercase SHA-256 of canonical sequence |

Read-placement columns are `NA`, not zero, if auditing is disabled or a candidate bound prevents a
complete result. One candidate-limited eligible read conservatively marks every linear-unitig row
`indeterminate_candidate_limit`, because the unenumerated placement could affect any row. Rows use the
same total order as FASTA: descending sequence length, canonical sequence bytes, then full unitig ID.
No-unitig output is the literal header plus LF and no data rows.

Schema 1.1 requires `edge_steps == canonical_kmers`. Self-complemental k-mer handles are
conservative compaction boundaries, so one emitted representative cannot consume both orientations
of the same backing canonical key. Exact neighboring transitions remain represented by GFA links.

The mapping target universe is emitted linear unitig sequences only. A read crossing a GFA link or a
closed-walk seam is not placeable by this table, while an eligible read shorter than `k` can place even
though it contributed no graph k-mer. “Single group” means one group in this target universe, not a
globally unique origin or proof of graph-construction contribution. `max_mapping_candidates` limits
accepted groups; it does not bound unsuccessful comparisons or runtime.

## `pair_links.tsv`

One row per canonical oriented endpoint group supported by pairs for which both mates have exactly one
accepted placement group on different linear unitigs and neither endpoint ties. The literal TSV header
is:

`schema_version<TAB>k<TAB>lane_ordinal<TAB>segment_a<TAB>end_a<TAB>strand_a<TAB>end_distance_a<TAB>mate_role_a<TAB>segment_b<TAB>end_b<TAB>strand_b<TAB>end_distance_b<TAB>mate_role_b<TAB>supplied_fragment_instances<TAB>status`.

Ends are chosen by the nearer segment boundary using the half-open placed interval; an exact tie is
excluded and counted in `pair_audit_summary.tsv`. Canonicalization uses the typed transform and enum
ranks in `ARCHITECTURE.md`. Rows sort first by numeric `lane_ordinal`, then by the complete canonical
endpoint tuple in that same typed order. A lane ordinal must identify a synchronized paired lane in
the immutable source catalogue; endpoint-identical observations from different lanes remain separate
rows.
Each end distance is a nonnegative integer strictly less than the referenced emitted segment length.
The only 0.1 status is `exact_unique_placement_pair_observation`. “Unique” means one accepted placement
group per mate in the emitted-linear-unitig target universe. It is not a globally unique origin or a
validated adjacency. No library orientation, insert span, or gap estimate is emitted.
No-observation output is the literal header plus LF and no data rows.

All coordinates, `L`/`R` ends, and strands are anchored to the emitted canonical forward segment.
Pair grouping canonicalizes endpoint order by swap only; it does not reverse-complement canonical
coordinates. The sum of `supplied_fragment_instances` over rows must equal the
`cross_unitig_observation` row in `pair_audit_summary.tsv`.
For every lane, its row support sum must also equal that lane's `cross_unitig_observation` count in
`run.json`.

## `pair_audit_summary.tsv`

The literal header is
`schema_version<TAB>k<TAB>state<TAB>supplied_fragment_instances`. For paired input the exhaustive enum
and row order are `remap_not_requested`, `mate_ineligible`,
`mate_indeterminate_candidate_limit`, `mate_unmapped`, `mate_multiple_placement_groups`,
`same_linear_unitig`, `endpoint_tie`, and `cross_unitig_observation`. When remapping is enabled, omit
the zero `remap_not_requested` row and emit the other seven rows including zeros; their counts sum to
all paired fragments. When remapping is disabled, emit only `remap_not_requested` with all paired
fragments. For single-end input, emit only `not_paired_input` with all single-end fragments.
This artifact remains the global partition for compatibility; the zero-inclusive partition introduced
in bundle schema 1.1 remains recorded for every lane under
`run.json/pair_audit/lane_state_counts`.

## `transform_summary.tsv`

One row for every configured stage, including no-op stages. Required fields include stage order,
algorithm ID/version, support unit, parameters JSON, input/output distinct canonical keys, input/output
support mass, removed key count, removed support mass, the SHA-256 digest of the sorted removed
`(full_key,support)` decision set, pre/post state digest, and status. Version 0.1 has an observation
stage and an absolute-retention stage only; there is no topology-deleting tip/bubble stage. Aggregate
statistics plus the decision-set digest are the promised audit boundary; individual removed keys are
not committed.

The literal header is
`schema_version<TAB>stage_order<TAB>algorithm_id<TAB>algorithm_version<TAB>support_unit<TAB>parameters_json<TAB>input_distinct_canonical_keys<TAB>output_distinct_canonical_keys<TAB>input_support_mass<TAB>output_support_mass<TAB>removed_key_count<TAB>removed_support_mass<TAB>decision_set_sha256<TAB>pre_state_sha256<TAB>post_state_sha256<TAB>status`.
Rows sort by numeric `stage_order`; stable 0.1 always emits observation stage 0 and retention stage 1,
including when retention removes nothing.

The status literals are `software_stage_complete_no_change` and
`software_stage_complete_with_change`. A row is `no_change` exactly when its sorted input and output
streams are byte-identical and both removed quantities are zero; otherwise it is `with_change`.
Stage 0 is consequently always `no_change`. Stage 1 is `with_change` exactly when at least one key is
removed.

Graph-state and decision-set digests use fixed binary framing. A graph-state preimage is
`"veritasm:graph-state:v1\0" || k_u8 || support_unit_tag_u8 || record_count_u64_le || records`, where
each record is `canonical_key_u128_be || support_u64_le` in numeric key order and support-unit tag 0 is
`supplied_fragment_instance` and 1 is `accepted_window_occurrence`. A decision-set preimage is
`"veritasm:decision-set:v1\0" || k_u8 || support_unit_tag_u8 || stage_order_u8 ||
record_count_u64_le || records` using the same record encoding. These definitions include the empty
set and require no hard-coded digest literal.

Stage 0 is `exact_count_observation`, version `1`, with `parameters_json={}`. Its input and output are
both the complete exact observed stream; its pre/post digests are equal and its empty decision set has
zero removals. Stage 1 is `absolute_support_retention`, version `1`, with compact key-sorted JSON
`{"retention_min_support_decimal":"<u64>","support_unit":"<support_unit>"}`. Its pre-state is the
complete observed stream, post-state is the retained stream, and its decision set is every removed
full key and support. The empty and nonempty cases use the same framing.

## `run.json`

Required top-level sections:

- `schema_version`, `software`, and `status`;
- logical input roles with content hashes and record counts, plus aggregate raw-transport bytes,
  decoded-input bytes, inferred mate-role count, gzip-source count, and gzip-member count;
- spool schema and checksum;
- effective parameters and profile expansion;
- quality/ambiguity/window accounting;
- exact no-sieve counting telemetry;
- retained graph and compaction counts;
- read-audit and pair-audit partitions;
- transformation journal summary;
- warnings and typed limitations;
- core-artifact determinism digest.

It also contains these mandatory interpretation fields, rendered verbatim in HTML:

- `scientific_scope = algorithmic_unitig_reconstruction`;
- `evidence_source = construction_reads`;
- `evidence_interpretation = internal_consistency_not_independent_validation`;
- `support_unit = supplied_fragment_instance | accepted_window_occurrence`;
- `biological_call = not_performed`;
- `taxonomy = not_performed`;
- `control_context = not_supplied` in stable 0.1; and
- `disclaimer = {version,text}` containing the exact canonical disclaimer rendered by HTML.

Valid technical transaction statuses are `software_run_complete` and
`software_run_complete_no_unitigs_under_parameters`. They do not mean that a genome, sample analysis,
validation, or larger workflow is complete. The no-unitig state must carry: “No sequence met the
recorded assembly rules; this is not biological absence.” Interrupted, resource-exhausted, or partially
processed runs are failures and do not commit a normal result bundle.

The machine-readable `schema/run.schema.json` freezes required object shapes, enums, decimal-string
fields, NA objects, and array order constraints. Serialization follows the schema's declared property
order from typed Rust structs; all arrays declare a deterministic sort key. `run.json` includes the
mapper instance's ID/version, fixed seed length, execution status, target universe, linear-target and
posting cardinalities, and conservative mapper-owned index-byte accounting. It also includes read-
state counts by role, global placement-enumeration status, global and per-lane pair-state counts,
link-group/support totals, and all checked reconciliation totals described in `ARCHITECTURE.md`.
Every lane includes the complete conditional pair-state set, including zero-count rows; each lane sum
matches its source-catalogue fragment count, per-state lane sums match the global rows, and pair-link
support reconciles globally and per lane. With remapping disabled,
the execution status is `not_requested` and all three index-accounting decimals are zero because no
index is built.

The `input` object records `raw_transport_bytes_decimal`, `decoded_input_bytes_decimal`,
`inferred_mate_roles_decimal`, `gzip_sources_decimal`, and `gzip_members_decimal` as aggregate
decimal counts. Raw transport is the physical byte stream before content-selected decompression;
decoded input is the complete logical FASTA/FASTQ byte stream after decompression. A gzip source can
contain multiple concatenated members, so member count may exceed source count. Inferred roles count
read records whose paired role came from declared file position rather than an explicit, consistent
header marker. These fields contain no path, header text, timestamp, or source name.

`parameters.limits` records `max_raw_transport_bytes_decimal` and `max_gzip_members_decimal` in
addition to the decoded-input and existing resource limits. All three transport limits are inclusive.
Observed raw bytes and gzip members must not exceed their recorded effective limits.

`read_audit.state_counts` has these exact conditional row sets. With remapping disabled, single-end
input emits only `(S,not_requested)` and paired input emits `(R1,not_requested)` then
`(R2,not_requested)`; each role count equals that role's supplied read instances. With remapping
enabled, each applicable role emits, including zeros and in this order,
`ineligible_ambiguity_or_quality`, `indeterminate_candidate_limit`, `unmapped`,
`single_placement_group`, and `multiple_placement_groups`; roles order `S<R1<R2`, and each role's five
counts sum to its supplied read instances. `global_enumeration_status` is
`unavailable_remap_disabled` exactly when remapping is disabled, otherwise
`indeterminate_candidate_limit` exactly when any applicable role has a nonzero candidate-limit row,
and otherwise `placement_enumeration_complete`. All three `placement_totals` are `NA` with the matching
reason in the first two cases and measured decimal strings in the complete case.

Stable 0.1 emits `warnings` as the exact empty array. It emits `limitations` as this exact ordered
array:

1. `Closed graph walks are not audited by construction-read remapping.`
2. `Construction-read remapping is internal consistency, not independent validation.`
3. `Pairs are reported as observations and never create joins.`
4. `Results do not establish biological identity, presence, absence, viability, infectivity, safety, or product disposition.`

Changing that vocabulary or conditionally emitting a warning requires a schema change. Operational
warnings after the commit boundary go only to stderr and cannot enter `run.json`.

The disclaimer version is `veritasm-disclaimer-1` and its exact UTF-8 text is: “VeritAsm is
unvalidated research software. It reconstructs algorithm-defined unitigs and reports internal
consistency from the same supplied reads used for construction. It does not detect or identify an
organism; establish biological presence or absence, viability, infectivity, sample sterility, or
product safety; determine product disposition; or replace a validated or compendial method. Regulatory
and standards references describe the surrounding workflow only and do not state or imply compliance,
approval, clearance, qualification, validation, or fitness for a regulated purpose.”

## Empty and closed-only bundle matrix

| Case | FASTA | GFA | Unitig evidence | Pair links | Pair summary | Transforms and remaining files |
|---|---|---|---|---|---|---|
| No retained unitigs | zero bytes | literal H line plus LF | header plus LF | header plus LF | conditional rows required above | both transform rows, complete `run.json`, report, schemas, and manifest |
| One or more closed walks, no linear unitig | closed records | H, S, and cycle links | closed rows with audit fields `NA` | header plus LF | paired fragments classify through the read states; no link support | normal complete status, both transform rows, complete interpretation/integrity files |
| Linear unitigs present | all records | H, ordered S, ordered L | all rows | zero or more rows | conditional rows required above | normal complete status and complete remaining inventory |

No-unitig files use measured zeros, not `NA`, wherever the operation completed and zero is meaningful.
An empty linear mapping target universe is still completely searched; eligible reads are `unmapped`,
while closed rows retain the separately specified unsupported-audit status.

## `report.html`

The report is self-contained and contains no remote scripts, styles, images, fonts, tracking, or
links that execute network requests. It renders only values present in machine-readable artifacts,
states every NA reason, and shows the canonical disclaimer and technical-status interpretation without
user action. It uses no traffic-light biological outcomes. All user-controlled text is escaped for its
exact HTML/JSON context.

The run summary includes observed raw transport bytes and gzip members together with their effective
aggregate maxima. The visible scientific-parameter table includes k, profile, support unit, retention
threshold, minimum base quality, remapping request, mapper ID/version/status, fixed q,
linear-target/posting cardinalities, conservative mapper-owned index bytes, and the exact mapper
target-universe token. A
prominent note states that construction-read remapping searches emitted linear unitigs only and
excludes closed graph walks. A separate interpretation table renders the mandatory `scientific_scope`,
`evidence_source`, `evidence_interpretation`, `support_unit`, `biological_call`, `taxonomy`, and
`control_context` values verbatim; the canonical disclaimer text is rendered verbatim in the scope
disclaimer section.

## Artifact inventory and deterministic digest

Required scientific paths are `unitigs.fasta`, `assembly.gfa`, `unitig_evidence.tsv`,
`pair_links.tsv`, `pair_audit_summary.tsv`, and `transform_summary.tsv`. Required interpretation paths
are `run.json`, `report.html`, and the files below `schema/`. All are deterministic and all are listed
in `manifest.sha256`. No execution log or experimental Bloom/scout artifact is permitted inside a
stable 0.1 result.

`run.json` contains `scientific_artifacts_digest`, computed by sorting the six scientific paths above
by path bytes and hashing, for each file, `path || NUL || lowercase_file_sha256 || LF`. The manifest,
`run.json`, report, and schemas are not in that inner digest, avoiding self-reference while naming its
exact projection.

## `manifest.sha256`

GNU-style lowercase SHA-256 lines, sorted by relative path byte order. It covers every committed file
except itself. Relative paths cannot contain tabs, LF, CR, backslash, absolute components, or `..`.
Verification recomputes every listed digest and rejects every unlisted regular file. The public
default verifier uses inclusive ceilings of 1 MiB for the manifest, 4,096 regular files including the
manifest, 1,024 directories below the bundle root, 32 relative path components, and 16 GiB of listed
artifact bytes hashed. A caller may supply different explicit limits through the library API;
verifier-owned manifest lines, paths, traversal state, and inventory vectors use fallible growth even
when those limits are raised. On supported Unix targets, the verifier anchors traversal to one opened
root directory, opens every observed entry relative to its exact parent descriptor with final-component
`NOFOLLOW` and `NONBLOCK`, classifies the opened descriptor with `fstat`, and hashes that same regular
file descriptor. Device/inode/length/mtime/ctime snapshots reject ordinary changes during manifest,
artifact, and directory reads; symbolic links, FIFOs, devices, sockets, and other non-regular entries
fail closed. These checks establish checksum-manifest and inventory integrity only; they do not rerun
typed schema validation or establish biological validity. A read-only verifier cannot freeze a mutable
namespace: use a quiescent trusted directory or immutable filesystem snapshot, and do not assume that
verified pathnames remain unchanged after the call returns.
