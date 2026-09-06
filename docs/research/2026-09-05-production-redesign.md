# Production-redesign decision record

- Review date: 2026-09-05
- Audited source: commit `48cadd9142371bf85f06d38182f0dd7cf81b3a8b`
- Status: controlling design input for the v0.4 development line
- Scope: deterministic de novo reconstruction from Illumina-like single-end and ordinary
  paired-end reads
- Intended use: research software; not an organism-detection, absence, clinical, compendial, or
  product-disposition system

## Executive decision

The audited source is a defensively engineered fixed-k exact-unitig oracle and review alpha. It is
not a production assembler. The correct response is not to discard its exact graph, input, pair,
and transactional contracts. Those become executable reference implementations while the data
plane, qualification plane, and operational envelope are rebuilt independently.

No retrieved paper or executed result supports describing VeritAsm as fastest, most sensitive, or
better than every other assembler. Those phrases are research hypotheses. A supported claim must
name an input domain, comparator set, endpoint, resource ceiling, frozen data, uncertainty interval,
and exact software objects.

Development is ordered as follows:

1. repair qualification defects that can create favorable but false sensitivity scores;
2. close fail-fast, resource, schema, fuzz-oracle, and I/O-contract gaps;
3. build an authenticated external-data primitive and exact partitioned compacted graph beside the
   oracle;
4. expose independent multi-k evidence before any path projection;
5. admit correction, low-count rescue, or pair-driven traversal one at a time behind kill tests;
6. benchmark the precision--recovery frontier against frozen modern comparators; and
7. optimize only measured bottlenecks while preserving byte-level scientific determinism.

## Independent audit findings that change the plan

| Rank | Finding | Direct consequence |
|---|---|---|
| High | The `error-pe` simulator assigns every injected substitution Q10 and every unchanged base Q40; the default Q20 construction rule therefore receives the error positions as an oracle and removes every error-overlapping window. | Existing error-case results cannot qualify robustness to retained sequencing errors. Keep this only as an explicitly named QC-censoring control and add independent error and quality models. |
| High | The evaluator assigns an equally compatible contig to the first truth record. A truth FASTA containing two identical molecules scores 0.5 genome fraction, 0 minor recovery, and duplication 2 despite being byte-for-byte complete. | Current per-molecule recovery and duplication fields are invalid as ambiguity-aware estimands. Replace them with unique-coordinate recovery plus lower/upper compatibility bounds or explicit NA. |
| High | The stable graph plus compaction admission is source-estimated at roughly 524 bytes per ordinary retained canonical key in the extraction phase. | Rebuild the main graph data plane around external endpoint records, partition-local compaction, boundary reconciliation, packed sequence arenas, and CSR side arrays. Do not claim an RSS bound from configured payload admission. |
| High | The normal pipeline performs approximately twelve spool-equivalent traversals; count merging retains obsolete generations and repeatedly verifies/rehashes full streams. | Introduce authenticated buffered block streams, verified predecessor reclamation, and per-stage I/O telemetry before scaling claims. Preserve old digest preimages unless an explicit schema version changes them. |
| High | The low-file-descriptor integration test uses fragment support on one long homopolymer and therefore never forces a merge. | Repair the test to force and assert multi-pass merging under the descriptor limit. |
| Medium | Existing destination locks are acquired only after spooling, counting, graphing, and audit. | Acquire a run lease before consuming input and retain it through no-replace publication. Test zero stdin consumption on a locked destination. |
| Medium | File input has no raw-transport or gzip-member ceiling. | Add explicit aggregate/per-source transport and member limits; reject empty-member storms and oversized transport before unbounded repeated work. |
| Medium | Stable `run.json` is rerendered by the same serializer but not independently checked against the shipped JSON Schema. | Add an independent schema-conformance lane and mutation-negative tests; keep heavy validation dependencies out of the runtime if possible. |
| Medium | Experimental partition validation accepts a noncanonical key and a changed minimizer owner when routing/totals still agree. | Bind partition proof records to immutable source windows and independently recompute `(source, window, canonical key, owner)`; aggregate conservation alone is insufficient. |
| Medium | Bundle and spool renderers issue many tiny unbuffered reads and writes. | Add explicitly admitted buffering and prove artifact byte equality. |

The audits found no stable fixed-k lost-edge, self-complemental reuse, false-unique exact-mapping, or
ordinary output-clobber counterexample in the inspected commit. That is bounded evidence, not an
absence-of-defects claim.

## Algorithm taxonomy and selection

| Family | Strength | Principal risk | Decision |
|---|---|---|---|
| Explicit exact de Bruijn graph | Simple identity and count oracle; easy exhaustive testing | Whole-resident state scales poorly | Preserve as small-data oracle only |
| Minimizer/super-k-mer partitioned cDBG | Exact external construction and partition-local parallelism; BCALM2 and GGCAT precedents | Bucket skew, boundary duplication, and nondeterministic stitching | Selected production data-plane direction with full keys and authenticated records |
| Direct finite-state cDBG | Avoids materializing all uncompacted adjacency; Cuttlefish precedent | Different graph conventions and abundance semantics can silently change output | Comparator and design oracle; evaluate custom implementation before embedding |
| Succinct BOSS/SBWT graph | Small frozen topology with rank/select navigation | Mutable cleaning, counts, colors, construction state, and access locality add cost | Candidate frozen query layer, not first mutable core |
| Sequential/iterative multi-k | Small k reconnects sparse regions; large k resolves shorter repeats | Carried-forward sequence can launder unsupported adjacency | First ship independent exact layers; projection requires a separate evidence rule |
| Variable-order graph | Shares multiple orders in one index | Threshold semantics differ by order; not a free iterative assembler | Research alternative only |
| String/overlap graph | Retains longer read context | Expensive exact overlaps and read-level state for high-background short reads | Deferred alternative, not the first short-read core |
| Quality-aware spectral correction | Can reduce error graph and preserve more usable read context | Miscorrection can erase genuine minority sequence | Experimental, reversible, journaled, and separately scored |
| Low-count/mercy rescue | Can restore true low-depth sequence | Can retain errors and create false junctions | Exact candidate tier only; acceptance requires independent context and ablation evidence |
| Paired-end graph constraints | Can choose among existing continuations within fragment span | Multimapping, bad models, and overlong repeats create false joins | Lane-specific two-pass model; abstain unless one existing path is uniquely compatible |
| Omnitigs/safe-string methods | Formal safety under stated graph model | Incomplete coverage and mixed/circular models may violate assumptions | Evaluate after the exact cDBG, not a release shortcut |
| Bloom/MPHF/fingerprint accelerators | Can reduce candidate work or index space | False positives or absent-key lookups can corrupt identity | Never final identity; verify full key/sequence before retention or traversal |

## Competitor capability snapshot

Versions are a qualification freeze, not a ranking. Exact binary/source/container hashes remain to be
recorded before execution.

| Tool | Frozen role | Version | Important constraint |
|---|---|---:|---|
| SPAdes | ordinary PE/SE genome; metaSPAdes PE mixture; rnaSPAdes RNA | 4.3.0 | GPL-2.0; current metaSPAdes accepts one short-read library and requires PE; Python is an external runtime |
| MEGAHIT | mixed-community PE/SE and resource comparator | 1.2.9 | GPL-3.0; old stable release; mercy behavior is a tradeoff, not a sensitivity guarantee |
| SKESA | conservative microbial genome comparator | 2.5.1 in tag `skesa.2.4.0_saute.1.3.0_2` | Version/tag mismatch requires `--version` and binary digest; SAUTE is not an untargeted substitute |
| ABySS | independent PE genome/Bloom comparator | 2.3.10 | GPL-3.0; build-time maximum k and complete configuration must be frozen |
| GGCAT | Rust cDBG construction comparator | 2.2.0 | MIT; exact full representation through k=64 in the paper, but larger-k fingerprints can create joins/splits |
| Cuttlefish | current external-memory Rust cDBG comparator | 3.0.1 | BSD-3-Clause; README still identifies 3.0.0; validated odd k 3..63; graph construction is not end-to-end assembly |
| BCALM2 | proof-backed partition/compact/glue oracle | 2.2.3 | Freeze optimized build; normalize graph conventions before comparison |
| Trinity | independent transcriptome comparator | 2.15.2 | BSD-3-Clause plus bundled terms; transcriptome endpoints differ from genome endpoints |

Construction comparisons and assembly comparisons are separate. A faster cDBG builder does not
establish a faster assembler, and a longer assembly does not establish a more correct one.

## Selected data structures

### Exact keys

- Width-specialize exact two-bit keys: `u64` for k<=31, `u128` for k<=63, and four `u64` words for
  k<=127.
- On-disk records include an authenticated key-width/k domain. Full keys decide identity. Hashes,
  minimizers, fingerprints, and MPHFs route or index known keys only.
- Canonical orientation and reverse-complement fixed points are explicit types and tests.

### External construction

- Immutable virtual partition IDs make scientific content independent of worker count.
- Rolling minimizer selection uses a deque/rolling implementation differentially tested against the
  existing exact slice oracle.
- A bounded descriptor pool multiplexes many virtual buckets over a small number of physical block
  streams.
- Authenticated block headers contain schema, source/spool digest, k, support unit, partition,
  ordinal interval, record count, payload length, and checksum.
- Merge replacement is registered and verified before predecessor deletion; a compact ancestry
  ledger retains provenance without retaining all obsolete payload.
- Fragment support deduplicates `(full key, fragment ordinal)` across mates and repeated windows;
  occurrence support reduces each accepted window exactly once.

### Direct compacted graph

- External side/endpoint records compute oriented degrees and hard fixed-point boundaries.
- Partition interiors compact locally. Boundary stubs are reconciled globally only when exact sides,
  overlaps, degree, and ownership agree.
- Frozen graph state uses a two-bit sequence arena, numeric segment IDs, endpoint side arrays, CSR
  adjacency, and separate count/evidence arrays.
- Stable serialization canonicalizes strand, cyclic rotation, IDs, and row order after topology is
  complete; worker completion order never reaches artifacts.

### Multi-k evidence lattice

The first multi-k product is a parent record over independent exact child graphs from one
authenticated spool. Cross-k relations are typed as `supports`, `contains`, `conflicts`, or
`unresolved`; they do not create sequence.

A later projection may extend a high-k path only when all bases and adjacencies are replayable from
original reads or admitted lane-specific pair constraints. Small-k traversability alone is not
support. Derived child contigs never become independent observations. Conflicts and abstentions are
first-class output.

### Correction and rescue

The evidence does not support treating correction as universally beneficial. CARE reports that
full-read/alignment context can reduce false-positive corrections relative to several spectrum
methods, while a 2024 cross-corrector evaluation found downstream improvement only in selected
cases and highlighted newer Illumina quality distributions. That combination argues for a raw
control arm, correction journals, and end-to-end ablation rather than unconditional preprocessing.

- Original observations remain immutable.
- Candidate corrections use a bounded non-greedy search over a trusted exact spectrum with an
  interpretable cost from recorded base qualities.
- Each base becomes unchanged, corrected, trimmed, or unresolved; no forced correction is required.
- Corrected and raw graphs are distinct evidence layers. Every accepted correction has a replayable
  journal; downstream scores report raw and corrected support separately.
- Low-count exact keys remain quarantined. Rescue is limited to named contexts such as a read-backed
  graph end or one uniquely pair-bounded path, then completely verified against original reads.

### Pair constraints

- Map read instances to graph paths and retain ambiguity, not only one placement.
- Infer orientation and robust span distribution independently per lane from eligible unique anchors.
- Replay pair evidence after models are frozen.
- A pair can select only one already present graph continuation when every admissible alternative is
  enumerated, exactly one is compatible under the declared interval, distinct-fragment support meets
  the frozen rule, and contradiction stays below a frozen rule.
- Otherwise stop the contig and preserve alternatives in GFA. Contigs and scaffolds are distinct.

## Features explicitly not selected

- a claim of globally phased haplotypes from local bubbles;
- target-specific scanning or a hidden reference database in the de novo core;
- taxonomy, biological presence/absence, sterility, or adventitious-agent calls;
- fingerprint-only large-k identity, MPHF-only membership, or Bloom-defined topology;
- automatic deletion of low-count keys without an evidence ledger;
- pooling insert models or link counts across lanes;
- forced circularity from terminal overlap or a graph cycle;
- concatenating segmented or disconnected molecules;
- post-hoc best-k or best-profile selection on qualification truth;
- N50 as the primary correctness endpoint;
- GPU, unsafe Rust, distributed execution, or general microcrate fragmentation before a profile and
  retained benefit justify the added boundary.

## Qualification design

The evaluator must publish three distinct quantities when truth is repeated or shared:

1. uniquely assignable coordinate recovery;
2. compatible recovery lower and upper bounds; and
3. unavailable molecule/copy-specific metrics where the reads or assembly cannot identify the
   requested attribution.

Self-evaluation of every truth FASTA must reach its mathematically expected bounds, and truth-record
permutation must not change class metrics. Error and quality generation are independent configurable
processes; qualification includes retained high-quality errors, low-quality correct bases,
miscalibrated qualities, correlated errors, and a QC-censoring control.

Genome, metagenome, and transcriptome lanes have different endpoints. Initial external anchors are
E. coli K-12 MG1655 ERR008613, Rhodobacter sphaeroides 2.4.1 SRR522244/SRR522246 with all seven
replicons, native-SE RNA SRR4941227 as a robustness case, and published CAMI data only as a larger
mixed-community stratum. Accession alone is not a frozen dataset: record URL, archive MD5, local
SHA-256, size, records, read-name audit, reference versions, and deterministic subset membership.

Primary endpoints are genome/reference breadth stratified by component and abundance, base error,
false junctions/misassemblies, unsupported sequence, duplication, and minor-path precision/recall
where truth makes that estimand identifiable. Runtime, peak RSS, temporary-disk high-water, I/O,
descriptor high-water, and deterministic artifacts are co-primary engineering outputs. Publish all
failures, timeouts, zero outputs, and regressions.

## Risks and claim gates

| Risk | Required control |
|---|---|
| Low-abundance truth is indistinguishable from error | Report uncertainty; use negatives and false-sequence endpoints; never infer absence |
| Multi-k projection launders inferred sequence | Original-read adjacency replay and a no-unsupported-adjacency oracle |
| Pair evidence creates a plausible false join | Unique path compatibility, lane model, contradiction ledger, and precision-first repeat tests |
| Partition boundary loses/duplicates topology | Exact occurrence/key conservation plus independent source-window owner oracle |
| Repeats make truth attribution unidentifiable | Bounds/NA rather than arbitrary primary assignment |
| Memory is shifted from RAM to disk | Measure RSS, logical/physical I/O, disk high-water, and failure at every limit |
| Probabilistic accelerator changes scientific output | Full-key recount and byte-identical output against no-sieve oracle |
| Parallelism changes IDs/order/floating results | Integer summaries, virtual partitions, canonical output, repeated 1/2/4/8-thread tests |
| Production wording outruns evidence | Release ladder and commit-bound claim ledger; unsupported rows remain NOT RUN |

## Evidence-supported recommendation

Build VeritAsm v0.4 as a qualification-correct, operationally bounded scientific alpha, then replace
the graph data plane behind the fixed-k oracle. The most credible route to unusually high
sensitivity is not a permissive global threshold. It is an evidence lattice: exact multi-k graphs,
raw and corrected read layers, an explicit low-count quarantine, and conservative lane-specific
physical linkage, with abstention whenever those channels do not identify one path.

That design is inventive in its combination and evidence contract, but no novelty or performance
claim is made. It wins only if frozen comparisons show more true recovery without more false
junctions or unacceptable resource regressions.

## Primary and official sources

- Bankevich et al., SPAdes, <https://doi.org/10.1089/cmb.2012.0021>
- Peng et al., IDBA-UD, <https://doi.org/10.1093/bioinformatics/bts174>
- Li et al., MEGAHIT, <https://doi.org/10.1093/bioinformatics/btv033>
- Chikhi et al., BCALM2, <https://doi.org/10.1093/bioinformatics/btw279>
- Cracco and Tomescu, GGCAT, <https://doi.org/10.1101/gr.277615.122>
- Khan et al., Cuttlefish 2, <https://doi.org/10.1186/s13059-022-02743-6>
- Khan et al., Cuttlefish 3, <https://doi.org/10.1101/2025.02.02.636161>
- Holley and Melsted, Bifrost, <https://doi.org/10.1186/s13059-020-02135-8>
- Bowe et al., BOSS, <https://doi.org/10.1007/978-3-642-33122-0_18>
- Kelley et al., Quake, <https://doi.org/10.1186/gb-2010-11-11-r116>
- Nikolenko et al., BayesHammer, <https://doi.org/10.1186/1471-2164-14-S1-S7>
- Li, BFC, <https://doi.org/10.1093/bioinformatics/btv290>
- Kallenborn et al., CARE, <https://doi.org/10.1093/bioinformatics/btaa738>
- Dlugosz et al., Illumina correction evaluation, <https://doi.org/10.1038/s41598-024-52386-9>
- Nurk et al., metaSPAdes, <https://doi.org/10.1186/s40168-017-0271-2>
- Safonova et al., dipSPAdes, <https://doi.org/10.1089/cmb.2014.0153>
- Baaijens et al., SAVAGE, <https://doi.org/10.1093/bioinformatics/btx192>
- Prjibelski et al., exSPAnder, <https://doi.org/10.1093/bioinformatics/btu266>
- Wick et al., Unicycler, <https://doi.org/10.1371/journal.pcbi.1005595>
- Hunt et al., REAPR, <https://doi.org/10.1186/gb-2013-14-5-r47>
- Tomescu and Medvedev, omnitigs, <https://doi.org/10.1089/cmb.2016.0141>
- Mikheenko et al., WebQUAST, <https://doi.org/10.1093/nar/gkad406>
- Magoc et al., GAGE-B, <https://doi.org/10.1093/bioinformatics/btt273>
- Meyer et al., CAMI II, <https://doi.org/10.1038/s41592-022-01431-4>
- Official Cuttlefish repository, <https://github.com/COMBINE-lab/cuttlefish>
- Official GGCAT repository, <https://github.com/algbio/ggcat>
- Official SPAdes documentation, <https://ablab.github.io/spades/>
