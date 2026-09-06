# Executable validation tooling

Status: executable qualification substrate with generator algorithm version 4 and evaluator/result
schema version 4. It is not an admitted scientific scorecard or an independent large-genome
evaluator.

## Boundary

`veritasm-simulate` and `veritasm-evaluate` are separate binaries built from the repository. Neither
is called by the assembly pipeline. The simulator puts assembler-visible reads below
`assembler_input/` and exact sequences, read origins, and injected error events below
`evaluation_truth/`. An assembler command receives only the read paths from
`assembler_input/input_manifest.json`. This is a workflow separation, not an operating-system access
control boundary.

Generated or evaluated recovery does not establish organism identity, biological presence or
absence, assay sensitivity, sterility, or product disposition.

## Generate a dataset

The six version-4 cases exercise single-end input, paired-end input, circular truth, a 90:10
two-component mixture, substitution-bearing paired reads, and an explicit QC-censoring control:

```bash
mkdir -p validation-work
cargo run --release --locked --bin veritasm-simulate -- \
  --out validation-work/circular-r0 \
  --case circular-pe \
  --replicate 0 \
  --fragments 2000 \
  --truth-length 10000 \
  --read-length 150 \
  --insert-length 350 \
  --gzip
```

Accepted case IDs are `linear-se`, `linear-pe`, `circular-pe`, `mixture-pe`, `error-pe`, and
`qc-censoring-control`. `error-pe` defaults independently to 20,000 substitutions and 20,000 Q10
assignments per million observed bases. `--substitution-rate-ppm` and
`--low-quality-rate-ppm` override those rates separately. Correct bases can therefore be Q10 and
substituted bases can remain Q40. `--quality-seed-hex` changes only the quality stream. The
`qc-censoring-control` case preserves the superseded fixture intentionally: every substitution is
Q10 and every other base is Q40. It rejects an independent low-quality rate and is not retained-error
qualification evidence. This deliberately simple model has no indels, adapters, GC bias, quality
context, or instrument-error model.

Unless `--seed-hex` supplies all 256 bits, the seed is

```text
SHA256("veritasm-validation-v1" || NUL || case_id || NUL || decimal_replicate)
```

Generator v4 domain-separates layout, error, and quality seeds from that unchanged master seed using
`SHA256("veritasm:validation-rng-stream:v1" || NUL || master_seed || stream_name)`. Each stream uses the
local `sha256-counter-v1` generator, which hashes its seed and a little-endian `u64` counter into
32-byte blocks and decodes each `u64` word little-endian. Bounded sampling uses rejection rather than
modulo-biased range selection. The manifest records every stream seed and whether the quality seed
was derived or supplied explicitly.

The dataset ID is `v4-` followed by the full lowercase SHA-256 commitment over the literal domain
`veritasm:validation-dataset-parameters:v4` plus one NUL byte and an ordered sequence of every
generator identity, RNG/seed, case, replicate, numeric model, library, and output-compression
parameter recorded by generator v4. Every field is encoded as
`name_length_u64_le || name_UTF8 || value_length_u64_le || value_UTF8`; the order in
`dataset_id_from_parameters` is frozen by a test vector. It is an unambiguous parameter commitment,
not a signature or proof that a third party produced the files.

Each generated directory contains:

| Path | Visibility | Meaning |
|---|---|---|
| `assembler_input/input_manifest.json` | assembler | Exact read paths and hashes; contains no truth path or molecule ID |
| `assembler_input/reads_SE.fastq[.gz]` | assembler | Single-end reads for `linear-se` |
| `assembler_input/reads_R1.fastq[.gz]`, `reads_R2.fastq[.gz]` | assembler | Strictly synchronized paired reads |
| `evaluation_truth/truth.fasta` | evaluator only | Exact molecule sequences |
| `evaluation_truth/origins.tsv` | evaluator only | Per-read molecule, strand, start, step, span, wrap state, and DNA/quality hashes |
| `evaluation_truth/errors.tsv` | evaluator only | One row per injected substitution with read, observed offset, truth coordinate, and bases |
| `evaluation_truth/quality_events.tsv` | evaluator only | Every non-default Q10 assignment, independently generated except in the explicit censoring control |
| `dataset.json` | orchestration/evaluator | Versioned dataset identity, generator configuration, topology, roles, sizes, and SHA-256 values |
| `manifest.sha256` | integrity | Sorted checksum inventory of every preceding artifact and `dataset.json` |

`manifest.sha256` is canonical: it contains exactly one lowercase digest, two spaces, the portable
ASCII relative path, and LF for `dataset.json` and every record in `dataset.json.files`, sorted by
unsigned path bytes. It does not list itself. `dataset_content_root_sha256` is SHA-256 of these exact
manifest bytes. This content root is deliberately absent from `dataset.json` so the commitment is
not circular.

Read identifiers are opaque ordinals and never carry molecule, component, topology, start, or error
information. Mixture labels occur only in truth. Paired fragments retain inward FR orientation while
their orientation relative to truth is randomized. Circular coordinates are modulo molecule length;
linear origins cannot wrap.

For a `+` origin, emitted pre-error bases are the forward-FASTA bases visited with step `+1`. For a
`-` origin, coordinates move with step `-1` and each visited forward-FASTA base is complemented, so
the emitted read is a reverse complement rather than a reversal. In `errors.tsv`,
`truth_coordinate_zero_based` always names the forward-FASTA coordinate, while `truth_base` is the
pre-error base in emitted-read orientation: the FASTA base on `+`, and its complement on `-`.
The origin row separately authenticates decoded numeric Phred values. The sparse quality-event ledger
plus the declared Q40 default therefore replays every emitted quality. Generation and verification
do not share their only coordinate/orientation implementation: the generator uses
`truth_read`/`coordinate_at`, while an independent replay routine uses separate coordinate and
complement arithmetic. Tests cover linear/circular, forward/reverse, wrapping, single-end, and
paired-end cases. Generator version 2 failed to complement negative-strand bases. Generator version
3 coupled every substitution to Q10, so its `error-pe` output is now interpreted only as the behavior
represented by `qc-censoring-control`. Neither historical version supplies admissible sensitivity
evidence.

## Assemble without truth

For paired input, give the assembler only the two paths declared under `assembler_input.read_paths`:

```bash
mkdir -p validation-work/results
cargo run --release --locked --bin veritasm -- assemble \
  --read1 validation-work/circular-r0/assembler_input/reads_R1.fastq.gz \
  --read2 validation-work/circular-r0/assembler_input/reads_R2.fastq.gz \
  --output-dir validation-work/results/circular-r0-k31 \
  --k 31 \
  --profile thresholded \
  --support-unit supplied-fragment-instance \
  --min-support 2 \
  --min-base-quality 20
```

Do not supply `dataset.json`, `evaluation_truth/`, an origin ledger, or a truth-derived parameter to
the assembler. A benchmark runner must freeze the assembler parameters before truth is inspected.

## Evaluate

```bash
cargo run --release --locked --bin veritasm-evaluate -- \
  --dataset validation-work/circular-r0/dataset.json \
  --assembly validation-work/results/circular-r0-k31/unitigs.fasta \
  --out validation-work/results/circular-r0-k31-evaluation \
  --junction-flank 15 \
  --max-edit-rate-ppm 150000 \
  --max-dp-cells 50000000 \
  --max-exact-alignment-scan-bases 1000000000 \
  --max-junction-comparisons 500000000 \
  --junction-evidence non-correct \
  --max-junction-index-bytes 536870912 \
  --max-junction-evidence-bytes 134217728 \
  --max-truth-evidence-replay-bytes 402653184
```

That command is a development evaluation. Its result records `development_unbound` and
`ineligible_development_unbound`. An admitted qualification invocation must additionally supply the
content root from a preregistration record or independently regenerated dataset, outside the
dataset being evaluated:

```bash
DATASET_CONTENT_ROOT_SHA256='<64-lowercase-hex value from the external admission record>'
cargo run --release --locked --bin veritasm-evaluate -- \
  --dataset validation-work/circular-r0/dataset.json \
  --assembly validation-work/results/circular-r0-k31/unitigs.fasta \
  --out validation-work/results/circular-r0-k31-qualified-evaluation \
  --expected-dataset-content-root-sha256 "$DATASET_CONTENT_ROOT_SHA256"
```

The evaluator does not read this expected value from the dataset directory. Deriving it from that
same directory immediately before evaluation checks internal consistency but does not create an
external trust root.

The output is a new, no-replace, checksum-bearing directory containing `result.json`,
`alignments.tsv`, `junctions.tsv`, and `manifest.sha256`. Before semantic artifacts are consumed, the
evaluator opens the canonical dataset checksum manifest beneath one held directory descriptor,
reconstructs its exact sorted and complete inventory from the owned `dataset.json` snapshot, hashes
the exact manifest bytes, and compares that root with the external expectation when one was supplied.
Dataset artifacts are then size- and SHA-256-verified before evaluation output staging begins. Each
relative component is opened with no-follow semantics as a regular file. Integrity hashing and later
owned semantic snapshots are separate reads where both are required; no opened-once claim is made.
Declared and fixed byte ceilings apply to every read;
truth and assembly FASTA parsing and identity use the same owned byte snapshot rather than reopening
the path between parsing and hashing. The evaluator also verifies that the
assembler-input manifest and reads remain in the assembler namespace, that all declared truth
artifacts remain evaluation-only, and that the two manifests agree on identity, mode, counts,
ordered read roles, paths, and hashes. Built-in case IDs are checked against the required mode, truth
classes, and linear or circular topology. Generator version, plan, RNG, seed derivation, repeated
lengths, output compression, and case-specific library metadata are also cross-checked rather than
trusted as labels. The evaluator parses the exact hashed FASTQ and TSV snapshots under their frozen
schemas. It requires exact headers, column counts, LF/final-newline encoding, canonical decimals,
event domains, read and ledger cardinalities, global row ordering, synchronized mate identities,
inward-FR fragment geometry, topology-aware coordinates, sequence/quality digests, exact
truth-to-pre-error replay, and exact ledger reconstruction of every observed FASTQ record. Merely
rewriting a ledger and refreshing its `dataset.json` byte count and checksum cannot yield
`evaluation_complete` when these facts disagree.

This internal reconciliation has a deliberate trust-root limit: a party that coherently rewrites
truth, reads, every ledger, and their metadata can construct a different internally consistent
dataset. The evaluator does not independently replay every RNG draw and the SHA-256 parameter
commitment is not keyed. Evaluator v4 therefore marks evaluation without an external expected root
as development-only. When supplied, the exact expected root, observed root, binding mode, and
qualification-admission state are all recorded and committed into the evaluation ID. External
binding proves equality to that admitted byte inventory; it does not prove simulator biology or
authorship by itself.
A zero-byte assembly FASTA is valid and
produces `evaluation_complete_empty_assembly`; undefined error rate, duplication, and QV values are
`null` with a reason. Exact compatible recovery lower and upper bounds remain the measured value zero
over the nonzero truth denominator. This is not an absence statement.

### Base metrics and recovery bounds

Each contig is first searched exactly against each truth molecule in each strand with a deterministic
linear-time substring matcher. Exact circular matches may begin anywhere in the first revolution and
traverse the origin. If no exact match exists, the evaluator uses the bounded unit-cost fitting
alignment. The diagnostic primary alignment minimizes edit distance, maximizes
matches, minimizes indels, then breaks ties by molecule manifest order, `+` before `-`, and oriented
start. It remains in `alignments.tsv` for replay and base-error diagnostics only.
`equally_best_molecule_strands` counts tied molecule/strand candidates; it does not enumerate all
tied starts within one candidate. Neither the selected row nor truth-record order determines
recovery or duplication.

An alignment is accepted only when

```text
1,000,000 * (mismatches + inserted bases + deleted bases)
  <= max_edit_rate_ppm * alignment columns
```

For every accepted zero-edit contig, evaluator v4 enumerates every exact compatible coordinate
placement. Strand representations covering the same molecule coordinates are one coordinate
placement. Unique-coordinate coverage is the union of contigs with exactly one compatible coordinate
set. The compatible lower bound is the union, over contigs, of coordinates common to every compatible
placement of that contig. The compatible upper bound is the union of coordinates in any compatible
placement. Duplicate truth sequences and repeated starts therefore widen an interval instead of
assigning recovery to the first truth record. Per-molecule rows are sorted by molecule ID.

Compatible-coordinate evaluation does not materialize one coordinate vector per exact placement.
Each placement is represented on the stack as one or two sorted half-open intervals. For each
truth/strand scan, overlapping upper-bound intervals are merged monotonically before the mask is
touched. Linear lower-bound intersections use constant-size interval algebra. Circular lower-bound
intersections use the equivalent complement-of-union construction and at most one fallibly allocated
truth-length bit mask for the current contig. If `Q` contig bases build search prefix tables, `T`
oriented truth bases are searched, `P` exact placements are visited, and `B` truth-mask coordinates
are initialized, cleared, or merged, the recovery pass performs `O(Q + T + P + B)` work rather than
`O(P * contig_length)` work. A private
instrumentation counter and an exhaustive brute-coordinate oracle freeze this equivalence in tests;
the published metric values, identifiers, and schemas are unchanged.

If any accepted alignment contains edits, all compatible-recovery ratios are unavailable because
this bounded evaluator does not enumerate the complete approximate-placement universe. Duplication is
also unavailable when exact placements are ambiguous. It is measured only when every accepted contig
is exact and coordinate-unambiguous, using diagnostic matching assembly bases divided by compatible
upper-bound covered bases.

Error rate uses all diagnostic primary-alignment columns as its denominator. With zero observed errors,
QV is reported as the finite lower bound `10*log10(evaluated_columns+1)`, never infinity. The Q63
integer-log approximation and half-up conversion to micro-QV introduced in evaluator v2 and retained
in v4 do not use platform-dependent floating-point transcendental functions. This is a deterministic
metric definition, not an arbitrary-precision, correctly rounded implementation of real `log10`.

### Junction metrics

Every output base adjacency with the requested exact flank on both sides is evaluated. A combined
left-plus-right flank found contiguously in any compatible linear or circular truth is correct. When
the combined context is absent but each flank has exactly one truth placement, the adjacency is
false. A missing flank is `indeterminate_unmapped_flank`; a multiply placed flank is
`indeterminate_ambiguous_flank`. Short terminal contexts are counted but not evaluated.

Version 2 builds lossless sorted two-bit occurrence indexes for the requested flank and combined
context. An exact equal range supplies the complete placement count without allocating every repeat
placement; a coordinate is retained when and only when the count is one. `junctions.tsv` therefore
contains all classification-sufficient facts rather than unbounded placement lists.

If an output has eligible adjacencies but every linear truth molecule is shorter than the requested
flank, the empty indexes are still a completed execution and are reported as
`built_empty_truth_window_universe`; lookups are complete with zero key comparisons. This is distinct
from `not_required_no_eligible_adjacencies`, where no index is constructed.

`--junction-evidence all` writes every eligible row, `non-correct` writes every false or indeterminate
row, and `summary` writes no per-adjacency rows. The default is `non-correct`. All modes evaluate every
adjacency and report identical metrics. `result.json` records logical, emitted, omitted, and
omitted-correct row counts plus a SHA-256 digest of the complete canonical logical row stream. Modes
never change automatically after a limit is encountered.

This exact-flank definition intentionally prefers indeterminate results over forced assignments in
repeats. Adjacent false rows can describe overlapping context around one larger sequence join; this
version reports evaluated output adjacencies, not clustered misassembly events.

## Resource limits and applicability

- The simulator rejects more than 100,000 fragments, a truth molecule longer than 2,000,000 bases,
  a read longer than 10,000 bases, more than 25,000,000 emitted read bases, or a conservative
  uncompressed dataset-size estimate above 256 MiB. The estimate independently budgets worst-case
  error-ledger and quality-event-ledger rows per emitted base whenever their configured models can
  emit them; it is intentionally conservative rather than a prediction of realized output size.
- Simulator count and byte arithmetic is checked before the staging directory is created. Major
  truth, assignment, origin, read, and error-ledger allocations use fallible reservations and return
  a typed resource-limit error when reservation fails.
- Before any whole-file read, the evaluator rejects a dataset manifest above 4 MiB, truth FASTA
  above 64 MiB, or assembly FASTA above 64 MiB. Any checksummed dataset artifact above 256 MiB is
  also rejected; the assembler-input manifest has a tighter 1 MiB whole-file limit. Artifact SHA-256
  verification is streamed rather than loaded wholesale. The canonical dataset checksum manifest
  has a separate 1 MiB bound. The original fixed byte ceilings are
  repeated in the machine-readable evaluator record in `result.json`.
- Semantic read validation rejects a decoded generated FASTQ above 128 MiB per declared read file;
  gzip magic must agree with the committed compression parameter.
- `--max-truth-evidence-replay-bytes` independently admits the conservative peak for declared read,
  origin, error, and quality snapshots; decoded FASTQ overlap; retained read sequences, numeric
  qualities, map/origin state; truth bases; and the current replay buffer. TSV rows are cursor-parsed
  from bounded owned snapshots without nested row vectors. Error and quality cursors advance in
  `(read emission rank, observed offset)` order while each read is replayed, so no per-event tree or
  bit set is retained. The configured and projected byte counts are recorded in `result.json`.
- A dataset may declare at most 1,024 artifact files and at most 512 MiB of artifact bytes in total,
  so checksum verification has a fixed aggregate input bound.
- Truth FASTA parsing stops above 256 records and assembly FASTA parsing stops above 10,000 records.
  FASTA identifiers are limited to 128 visible ASCII bytes. At most 10,000 alignment rows are
  retained. Junction rows stream and repeat placement sets are represented by exact counts.
- Junction flank length is explicitly limited to 1 through 31 bases, so the combined context fits
  losslessly in one `u128`. `--max-junction-index-bytes` admits the two exact occurrence indexes
  against a conservative two-times-capacity projection before allocation. After reservation, the
  exact occurrence-vector capacities plus a fixed accounting reserve are checked against the same
  limit. The projection and post-reservation accounted capacity are recorded in `result.json`.
  When the assembly has no eligible full-flank adjacency, no truth index is built; the execution
  status records that fact and both byte counts and key comparisons are zero.
- `--max-dp-cells` bounds the traceback matrix independently for every contig/truth/orientation.
- A separate fixed ceiling of 250,000,000 cells bounds aggregate alignment work across every contig,
  truth molecule, and both orientations after exact-substring targets have been removed from DP
  work. The preflight uses the same checked dimensions as the fallback aligner.
- The exact-substring prefix table has a fixed 64 MiB ceiling. Above it, the evaluator uses the
  ordinary preflight and will usually reject a long approximate alignment under the DP limits.
- `--max-exact-alignment-scan-bases` bounds the aggregate oriented truth bases inspected by both the
  exact primary-alignment planning pass and compatible-coordinate enumeration. The configured and
  observed combined counts are recorded in `result.json`.
- Recovery retains three fallibly allocated bit masks over the complete truth-base universe. The
  circular lower-bound algorithm can additionally retain one fallibly allocated bit mask whose
  logical length is one truth molecule, and releases it before the next contig. These masks are
  bounded indirectly by the fixed 64 MiB truth-FASTA input ceiling; allocator metadata and capacity
  rounding are not a separately published hard-RSS account. The 200,000-base homopolymer regression
  with a 100,000-base contig visits 100,001 placements but performs exactly 200,000 mask-coordinate
  operations, and the exhaustive short-sequence oracle verifies exact metric equality with the
  former materialized-coordinate definition.
- `--max-junction-comparisons` bounds actual packed-index binary-search key comparisons, not a
  quadratic truth-scan estimate.
- Alignment TSV projection retains its 128 MiB cap. `--max-junction-evidence-bytes` independently
  limits actual header-plus-row bytes written inside staging.
- Limit excess is a failure, not an empty or partially successful evaluation.
- Exact long contigs are handled by the exact-substring fast path. Edited or structurally different
  long contigs still require a separately frozen indexed/banded aligner; evaluator v4 does not
  pretend that exact matching solves general large-genome validation.
- Compatible-coordinate bounds repair exact-placement ambiguity but do not solve approximate
  repeat-aware, split-alignment, NGA, or public-data evaluation.
- The six cases are a vertical slice, not the complete factor grid in `VALIDATION.md`. No generated
  dataset becomes admitted simply because the tools can produce it.

The version-4 result schema retains version 3's compatible-coordinate bounds and explicit unavailable
states, and adds externally bound versus development-unbound provenance plus replay-memory admission.
Junction metric definitions remain unchanged from version 2.

Machine schemas are shipped as `schema/validation_*.schema.json`. The experiment schema freezes the
required top-level pre-run record but intentionally leaves domain-specific nested contents open in
this first slice; it is insufficient by itself for dataset admission. Dataset and result schemas are
strict for the emitted dataset-v2 and result-v4 records. The result schema also declares cross-field `x-invariants`;
`validate_evaluation_result` checks the fixed evaluator/configuration descriptors, recomputes the
evaluation ID over all recorded inputs, content-root/binding state, and configuration, enforces resource ceilings,
record-to-base/alignment/coverage domains, exact assembly-adjacency totals,
assembly/junction/evidence reconciliation, mode-specific omission counts, per-molecule coverage
limits, derived ratios and QV/status pairs, and the no-index execution precondition before commit.
TSV descriptor schemas freeze column order and missing-value semantics.
