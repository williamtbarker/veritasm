# Experimental authenticated multi-k portfolio

`veritasm-multik` is an **experimental, unqualified, research-use-only**
integration executable. It is not the stable assembler, is not production
qualified, and makes no accuracy, sensitivity, sample-content, clinical, or
performance claim. Paired input is synchronized and spooled, but pair-path
constraints are explicitly `disabled_unqualified`; quality correction is also
`disabled_unqualified`. This slice therefore does not yet satisfy the planned
Priority-A paired-constraint gate.

## What it does

The command acquires a no-replace destination lease before input is opened,
then creates one immutable authenticated spool. It processes a strictly
increasing, unique list of `k` values in `3..=63`, one child at a time:

1. replay the spool into a source-bound exact canonical k-mer table;
2. apply `retain_all` or an inclusive, positive exact-support threshold;
3. compact the opaque retained-count capability;
4. independently replay exact `(k+1)`-mer transitions from original reads;
5. split or exclude graph transitions that lack an exact transition row;
6. validate the witnessed child against both opaque sources; and
7. write and re-read a bounded private reporting snapshot before releasing the
   child's graph and evidence capabilities.

It never creates a cross-k edge, joins child sequences, votes on a base, or
chooses a preferred child. A serialized snapshot or output file is a one-way
report and cannot be promoted back into a source-authenticated capability.

## Profiles

- `diversity-preserving` writes every child-scoped segment to the primary
  `contigs.fasta`. Exact duplicates at different k remain separate.
- `exact-agreement-consensus` writes a primary record only when byte-identical
  sequence and topology occur in at least two **distinct** k children. A
  singleton or a group confined to one k is excluded from the primary FASTA
  with `excluded_no_cross_k_exact_agreement`. It remains in `segments.fasta`,
  `assembly.gfa`, and the evidence tables. “Consensus” here is only a
  presentation rule; it does not phase or infer a sequence.

## Examples

Single-end gzip input (gzip is detected from content):

```console
veritasm-multik \
  -U reads.fastq.gz \
  -o multik-result \
  -k 21,31,51 \
  --retention retain-all \
  --threads 1
```

Paired lanes can be ingested and strictly synchronized, but no pair constraint
enters reconstruction in this alpha:

```console
veritasm-multik \
  -1 lane1_R1.fastq.gz lane2_R1.fastq.gz \
  -2 lane1_R2.fastq.gz lane2_R2.fastq.gz \
  -o multik-paired-result \
  -k 21,31 \
  --retention inclusive-support \
  --min-support 2 \
  --threads 1
```

Execution is intentionally serial. Omitting `--threads` means one; every value
other than one is a typed configuration error rather than an ignored hint.

## Output transaction

The destination must not exist. The executable renders these files in a
private sibling staging directory:

- `segments.fasta` — every independent child segment;
- `contigs.fasta` — profile-only presentation;
- `assembly.gfa` — child-local segments and witnessed links only;
- `segment_evidence.tsv`;
- `adjacency_evidence.tsv`;
- `transition_decisions.tsv`;
- `profile_decisions.tsv`;
- `run.json`;
- `report.html` — self-contained HTML;
- `schema/multik_run.schema.json`; and
- `manifest.sha256` — sorted SHA-256 inventory.

Every renderer is byte-limited while writing. The staged tree and manifest are
verified before a filesystem no-replace rename. A failure removes only owned
work/staging data and cannot replace a pre-existing result. The final manifest
authenticates serialized bytes; the separate `parent_root` binds scientific
child ancestry, while `operational_root` binds the recorded execution/resource
contract without changing scientific identity.

## Resource contract

`--memory-budget-bytes` is an aggregate ceiling over explicitly modelled owned
payload phases. Component limits are partitioned so overlapping raw/retention,
retention/compaction, graph/transition/reconstruction/report, and final
reporting states cannot each spend the full ceiling independently. This is
reported as `modelled_owned_payload_not_process_rss`: allocator metadata,
shared-library mappings, stacks outside the model, and OS page accounting mean
it is not an RSS promise.

`--max-temp-bytes` is enforced across the simultaneously live spool,
accumulated child snapshots, component run files, and final private staging.
The spool bridge already includes the spool in its own accounting; the parent
subtracts prior snapshots. The transition builder counts its own runs, so the
parent first reserves the spool and snapshots. Final staging receives only the
remaining aggregate allowance.

The final report pass is deliberately whole-resident within small encoded and
owned-payload caps. It is not described as streaming or production-scale.

## Recovery limitations

- A read shorter than a child k contributes no edge to that child. It is not
  projected from another k and can therefore reduce recovery.
- Inclusive retention can remove true low-support sequence. Every discarded
  key/support mass is reported; no threshold is qualified as optimal.
- Exact local transition rows do not establish global phase, a complete
  molecule, or a globally correct reconstruction.
- Closed-walk output is graph topology with explicit transition evidence, not
  a molecular circularity claim.
- Pair-path constraints and correction remain disabled in this slice.
- All children are whole-resident within explicit caps; the disk-partitioned
  production-scale route remains proposed.

## Focused verification

```console
cargo fmt --check
cargo test --lib experimental::multik_bundle::tests
cargo test --lib experimental::multik_pipeline::tests
cargo test --bin veritasm-multik
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --document-private-items
```

The focused tests cover configuration-before-I/O, serial thread enforcement,
aggregate limit boundaries, snapshot mutation rejection, unsupported internal
adjacency replay, exact-agreement singleton exclusion, bounded writes,
transaction failures, no-clobber races, manifest verification, malformed
snapshot bytes, and byte determinism. These are software-integrity tests, not
reconstruction-accuracy validation.
