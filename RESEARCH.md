# VeritAsm algorithm research and production-redesign recommendation

- Review date: 2026-09-04
- Product scope: neutral, evidence-first de novo assembly of Illumina-like short-read DNA
- Initial domain: synthetic constructs, plasmids, microbial isolates, organellar DNA, and small or
  moderately complex mixtures
- Frozen implementation ancestor: Virustic2 commit
  `b211915fc7cce82629766b77024463c6cabcc749`
- Intended use: research software; not a clinical, regulatory, organism-detection, or lot-release
  system

## Review status and evidence standard

This is a targeted decision review, not a systematic review or meta-analysis. It prioritizes primary
algorithm papers, independent truth-known benchmarks, and current official documentation. The
literature supports mechanisms and testable engineering choices. It does not establish VeritAsm
accuracy, performance, scalability, or superiority.

The 2026-09-05 follow-up research and code review is
[`docs/research/2026-09-05-production-redesign.md`](docs/research/2026-09-05-production-redesign.md).
It is the controlling design input for v0.4 and records qualification-invalidating counterexamples,
the exact external cDBG/multi-k evidence-lattice decision, current comparator freezes, and explicit
production gates. The earlier 2026-09-04 deep-research addendum and claim/gap ledger is
[`docs/research/2026-09-04-production-redesign.md`](docs/research/2026-09-04-production-redesign.md).
It adds current GGCAT/Cuttlefish construction evidence, DVOUG and other variable-order work, SAMA's
missing-edge uncertainty model, recent information-preserving graph work, reproduced local
performance and evaluator blockers, and the even-k self-complemental-edge counterexample. Where the
older v0.1 recommendation conflicts with the addendum or accepted ADRs 0010 onward, the later dated
decision controls.

Evidence is weighted in this order:

1. independent truth-known or controlled-mixture benchmarks;
2. primary method papers with disclosed algorithms, datasets, and metrics;
3. community benchmarking challenges and validation-method papers;
4. current official software documentation for present capabilities.

Author-led benchmarks are retained but identified as such. Results from human, plant, single-cell,
metagenomic, or virus-specific datasets are not assumed to transfer unchanged to the bounded
VeritAsm domain. No implementation is selected by benchmark rank alone.

## Phase 1 deliverable index and chronology

The original research phase requested eight explicit outputs. They are located as follows:

| Requested output | Authoritative location |
|---|---|
| Algorithm taxonomy | [Algorithm taxonomy](#algorithm-taxonomy) in this review |
| Competitor capability matrix | [Concise competitor capability snapshot](#concise-competitor-capability-snapshot) below; expanded freeze/licensing matrix in [docs/COMPETITOR_MATRIX.md](docs/COMPETITOR_MATRIX.md) |
| Known failure modes | [Known failure modes](#known-failure-modes) in this review |
| Features worth implementing | [Selected v0.1 features](#selected-v01-features) |
| Features explicitly not selected | [Deferred features](#deferred-features) and [rejected v0.1 behaviors](#rejected-v01-features-and-behaviors) |
| Scientific and engineering risks | [Concise scientific and engineering risk summary](#concise-scientific-and-engineering-risk-summary) below; expanded register in [docs/SCIENTIFIC_RISKS.md](docs/SCIENTIFIC_RISKS.md) |
| Citations and stable links | [References](#references), plus the source and license links in the competitor matrix |
| Evidence-supported recommendation | [Recommendation](#recommendation) |

Chronology disclosure: this research review, the initial architecture/ADRs, and the initial stable
implementation were imported together in commit `ade474f`; repository history therefore does not
prove that Phase 1 was completed before major implementation began. The documents preserve the
decision rationale used during that build, but should be read as contemporaneous or retrospective
design evidence. Later work must place a reviewed research or ADR change before implementation when
it materially changes the scientific contract. The current requirement-to-evidence crosswalk is in
[docs/REQUIREMENTS_TRACEABILITY.md](docs/REQUIREMENTS_TRACEABILITY.md).

## Product-neutral evidence vocabulary

| Term | Required meaning |
|---|---|
| k-mer occurrence | One accepted k-mer window observed in one read. |
| fragment support | Number of input record instances containing an event, counted at most once per supplied single read or synchronized read pair under the declared support policy. It does not prove independent laboratory molecules. |
| count | An exact integer unless the field is explicitly labelled approximate. |
| depth | Number of aligned read bases or fragments covering a stated sequence position under a stated mapping policy. |
| graph coverage | A fully named graph statistic, such as mean edge-occurrence count; never an unqualified synonym for depth or abundance. |
| read-backed | Supported by retained read/read-pair alignments under a reported mapping, quality, and ambiguity policy. |
| paired-supported join | A join supported by correct mate roles, mapping uniqueness, orientation, insert compatibility, support from distinct supplied fragment instances, and reported conflicts. This is not proof of independent laboratory molecules. |
| contig | Reconstructed sequence under a stated path-selection rule; not proof of an organism, replicon, or biological presence. |
| candidate circular topology | A closed graph or sequence path with separately reported junction evidence; not a certified complete circular molecule. |
| inferred path | A path selected by coverage, flow, optimization, or another model without full physical linkage. |
| exact disk-backed count | A count produced through partitions or external sorting whose final key set and integer counts equal a validated exact in-memory implementation. |
| retained graph cap | A deterministic upper bound on graph state allowed in memory. Exceeding it is an explicit failure, not silent pruning. |

## Algorithm taxonomy

### 1. Exact de Bruijn graphs

A de Bruijn graph represents read-derived k-mers and their adjacency, replacing all-pairs read
overlap with local word connectivity [R1]. An exact graph means membership and retained counts are
not changed by a probabilistic false-positive data structure.

An explicit packed node/edge graph is straightforward to audit and supports exact counters, but
memory grows with the number of distinct retained k-mers. Sequencing error and high-background DNA
can therefore dominate memory even when the FASTX reader itself streams.

### 2. Overlap-layout-consensus and string graphs

De Bruijn graphs are not the only defensible assembly model. Overlap-layout-consensus methods retain
read-to-read overlap information, while a string graph removes transitive overlap relationships and
represents what remains inferable from the reads [R38]. SGA demonstrated an FM-index-based string-graph
pipeline for short-read assembly [R39]. These methods can retain read-level context that a fixed-k
graph fragments, but exact overlap discovery and read-level graph state are unattractive for the
high-background, short-read inputs targeted by the initial bounded slice.

Stable v0.1 therefore selects a de Bruijn graph for its exact, streamable k-mer evidence model and
bounded external counting path. This is an engineering scope decision, not evidence that de Bruijn
graphs are universally more accurate. A future overlap/string-graph path would require its own exact
overlap definition, containment policy, memory bound, deterministic reduction, and task-equivalent
benchmark.

### 3. Compacted de Bruijn graphs

In the conventional compacted-dBG model, compaction replaces each maximal non-branching path with a
unitig. It is a topology-preserving representation step and must remain separate from graph cleaning,
which removes evidence [R3]. Bidirected fixed points require an explicit boundary model rather than
blindly applying that ordinary-path definition.

Three established implementation families are relevant:

- BCALM2 partitions k-mers by minimizer and compacts exact disk buckets in parallel [R3].
- Bifrost constructs an exact compacted graph without retaining the entire uncompacted graph and
  supports dynamic updates and colors [R4].
- Cuttlefish 2 uses a finite-state representation to construct compacted graphs directly from raw
  reads or assembled sequences [R5].

Their published performance results are author-led and are not performance evidence for a new Rust
implementation. Circular components, reverse-complement symmetry, and palindromic nodes require
separate representation tests rather than being inferred from ordinary linear-unitig examples.

Topology-preserving compaction is not a biological-safety proof. Maximal unitigs can be absent from the
underlying genome even for error-free reads when coverage is incomplete, and a naively bidirected
representation can split palindromic unitigs [R40]. Omnitigs provide a formal safe-string alternative
under a stated genome-graph model [R41]. Stable v0.1 emits deterministic **conservative graph
segments maximal under its declared degree and fixed-point boundary model**; it does not call them
safe strings. The stable compaction rule makes self-complemental nodes and endpoints incident to
self-complemental k-mer edges compaction boundaries, and validation must measure false sequence as
well as contiguity. Evaluating
omnitigs or another safe-and-complete formulation is deferred until its assumptions, circular model,
resource cost, and output semantics are reconciled with incomplete and mixed short-read data.

### 4. Exact disk-backed counting and partitioning

Disk-backed counting is selected for v0.1 because high-background input can contain far more
distinct k-mers than a small target genome.

- DSK partitions occurrences into disk files and processes each partition with a fixed
  user-controlled amount of memory [R6].
- KMC2/KMC3 use signature-based bins, super-k-mers or `(k,x)`-mers, external sorting, and parallel
  processing to reduce RAM and I/O [R7], [R8].
- BCALM2 extends minimizer partitioning from counts to exact graph compaction [R3].

VeritAsm v0.1 selects exact disk-backed **counting**, not a fully disk-resident retained graph. The
counter must preserve the configured occurrence or fragment-support semantics exactly. After the
explicit support rule is applied, retained k-mers enter an exact in-memory graph with a deterministic
hard cap. If that cap is exceeded, the run fails before committing output and reports a remedy.

This design bounds the counter's RAM but does not justify saying that whole-run memory is independent
of graph complexity.

### 5. Probabilistic filters and graphs

Bloom-filter graphs can use substantially less memory, but false-positive membership can introduce
extra graph edges [R9]. Lighter demonstrates probabilistic sampling and Bloom filters for read-error
correction rather than exact counting [R20].

[`ADR-0004`](docs/adr/0004-exactness-preserving-probabilistic-acceleration.md) accepts two distinct,
constrained designs. The repository now contains a safe-Rust, fixed-size `BloomFilter` and an ordered
`TwoHitSieve` prototype with module-level oracle, collision, eligibility, and property tests. Those
primitives are deliberately outside the stable assembly/counting path: there is no stable CLI switch,
no completed-spool integration, no serialized filter, and no retained end-to-end performance or
output-equivalence result. The syncmer/Count-Min/control scout remains a design only. Thus ADR
acceptance and primitive tests are not evidence that either mechanism is operationally integrated,
faster, or more memory-efficient. The two-hit-sieve superset lemma below is a project-specific
correctness argument derived here; Bloom's paper establishes the underlying probabilistic membership
structure [R35], not this system-specific lemma:

1. **Two-hit sieve:** for fragment-instance support with a retention threshold of at least two, two
   insertion-only Bloom filters may identify a superset of recurrent full canonical keys. A complete
   second spool pass routes that superset to the ordinary exact external counter. Under the ADR's
   ordered-update, integrity, and completeness premises, Bloom false positives add exact work but do
   not change retained membership or integer counts. Support one, occurrence mode, singleton
   rescue, and incompatible retention rules bypass the sieve.
2. **Default-off scout:** closed syncmers, background Bloom filters, and Count-Min sketches may be
   evaluated only as an experimental scheduler [R36], [R37]. Every fragment must remain reachable by
   the exact residual path, and approximate priority values cannot alter retention, graph topology,
   sequence, pair evidence, or a biological interpretation.

The two-hit exactness boundary is deliberately narrow. It preserves the exact retained key set and
retained-key counts; by itself it does not provide exact distinct-singleton cardinality, a complete
pre-threshold support histogram, or a per-key removal ledger below the routing threshold. A schema
requiring those fields must disable the sieve or obtain them by an independent exact method. Neither
accepted mechanism permits probabilistic graph membership.

### 6. Fixed-k, iterative, and multi-k assembly

The choice of `k` creates a structural tradeoff:

- smaller `k` connects sparse data but collapses more repeats and similar sequences;
- larger `k` resolves shorter repeats but fragments low-depth regions when required k-mers are
  absent [R10], [R11].

The principal strategies are:

1. **Fixed k:** construct and clean one graph, as in Velvet [R2]. It is simple and auditable but
   cannot adapt across repeat and depth regimes.
2. **Iterative carry-forward:** IDBA-UD moves from small to large `k` and treats reconstructed
   contigs as input in later iterations [R10]. This can fill gaps, but an early incorrect path can
   become derived sequence not present in an original read.
3. **Multisized graph:** SPAdes combines information across sizes within an assembly-graph framework
   and updates pair-distance evidence through graph transformations [R11].
4. **Iterative succinct graph:** MEGAHIT builds successive succinct graphs and uses “mercy k-mers”
   plus local coverage rules to rescue some low-depth sequence [R13].

The independent CAMI benchmark found that assemblers using a range of k-mers recovered more genome
fraction than single-k assemblers in its tested metagenomes, while closely related genomes remained
difficult [R14]. That supports evaluating multiple `k` values, not assuming that every multi-k
algorithm is safer or more accurate.

Stable v0.1 accepts exactly one `k` per result bundle and builds that exact graph only from original
reads. Independent commands may evaluate different `k` values, but they produce separate authenticated
spools and bundles and do not constitute a multi-k assembly. A future parent/child multi-k wrapper may
run independent fixed-k children, but it needs a versioned concordance schema before it can deduplicate,
compare, or select candidates. No child may consume another child's unitigs or counts as read evidence.
That future design requires direct comparison with fixed-k and established iterative methods before an
improvement claim.

### 7. Sequencing-error, quality, ambiguity, and low-complexity handling

Relevant alternatives include:

1. **Immutable reads plus hard filtering:** reject windows with explicit quality or ambiguity
   failures and retain every rejection count.
2. **Quality-aware spectral correction:** Quake uses base qualities, nucleotide-specific miscall
   rates, and trusted k-mers [R17].
3. **Bayesian clustering:** BayesHammer clusters k-mers in Hamming graphs for extremely uneven
   single-cell coverage [R18].
4. **Non-greedy spectral correction:** BFC searches a broader correction space and was designed to
   suppress systematic Illumina error [R19].
5. **Probabilistic or multistage correction:** Lighter samples k-mers in Bloom filters [R20];
   Musket combines conservative, aggressive, and voting stages [R21].

Error correction can reduce graph size and improve some assemblies, but published improvements are
data- and assembler-dependent. Low abundance is not equivalent to sequencing error; a correction
model whose trusted spectrum assumes one dominant coverage regime must be tested separately on
uneven-depth mixtures rather than transferred from its original domain [R10], [R17].

The v0.1 default therefore keeps reads immutable, applies explicit base-quality and ambiguity
filters, and performs measurable graph transformations. Any future correction mode must be
experimental and emit a per-read or aggregate change ledger sufficient to reproduce the corrected
input.

Quality-aware error correction is distinct from quality-weighted counting. A fractional or
probabilistic k-mer weight would require a calibrated error model, aggregation and rounding rules, and
a new interpretation of graph support. Stable v0.1 does not do this: a FASTQ window is accepted only
when every base meets the declared Phred threshold, and every accepted occurrence or fragment event
then contributes one exact integer under the selected support policy. Quality-weighted counting and
read correction remain separately deferred experiments.

For IUPAC input, the parser validates the declared alphabet, but any non-A/C/G/T base resets the rolling
window. Stable v0.1 neither expands an ambiguous symbol into several k-mers nor substitutes a base.
Possible, accepted, quality-rejected, ambiguity-rejected, and jointly rejected windows are reported
under one versioned accounting rule. Low-complexity, homopolymer, adapter-like, and repetitive sequence
can create uninformative graph structure, but stable v0.1 applies no entropy, DUST-like, homopolymer, or
other low-complexity deletion and emits no unversioned complexity score. Any later rule must name its
algorithm and parameters, journal every aggregate removal, and demonstrate that it does not erase
truth at low abundance.

### 8. Paired-end repeat resolution

Paired reads help only when a molecule links sequence unique enough to map and its orientation and
insert behavior are consistent.

1. A paired de Bruijn graph incorporates mate relationships into graph structure [R22].
2. exSPAnder maps pairs to long assembly-graph edges, estimates insert intervals, and extends only
   when one candidate is a strong winner [R23].
3. BESST builds a post-assembly scaffold graph and shows why link count alone is insufficient in the
   presence of repeats, heterogeneity, and erroneous pairs [R24].
4. IDBA-UD performs local assembly of unaligned mates anchored by a confident contig [R10].

The paired-de-Bruijn-graph paper's main demonstration used simulated perfect reads [R22]. Repeat
resolution remains impossible when no molecule spans the ambiguous region or every anchor
multimaps.

For stable v0.1, both mates are independently remapped by complete exact matching to emitted linear
unitigs. A cross-unitig observation is emitted only when both mates each have one complete placement
group; its endpoint, strand, end-distance, mate role, and exact supplied-fragment-instance support are
reported. Multiply placed, ineligible, indeterminate, unmapped, same-unitig, and endpoint-tie pairs
remain explicit summary states. Stable v0.1 infers no library orientation, insert distribution, span,
gap, adjacency, branch traversal, or GFA jump. A later joining mode requires a separate decision and
truth-known false-junction validation.

### 9. Bubbles, variants, and mixed samples

Errors, real alleles/strains, and near repeats can all form bubbles. Relevant strategies are:

1. Velvet smooths sufficiently similar bubbles and implicitly favors the higher-coverage path [R2].
2. IDBA-UD and SPAdes use gradual or local-relative graph cleaning [R10], [R11].
3. Cortex uses colors to retain sample/population provenance in a de Bruijn graph [R25].
4. Haploflow decomposes a unitig graph using coverage flow [R26].

Coverage-flow paths are model-derived when variants lack molecule-spanning linkage. Viral strain
deconvolution is not VeritAsm's product scope; Haploflow is included because it demonstrates the
difference between preserving a branch and inferring a longer biological path.

Version 0.1 preserves unresolved alternatives in GFA and ends unitigs at branches. `retain_all` applies
no deleting transformation; `thresholded` applies only the declared absolute exact support threshold
during k-mer retention. Stable v0.1 has no deleting tip or bubble rule, and no profile is called
consensus or converts a local bubble into a phased path. Any future collapse mode requires a new
decision, explicit rejected-path evidence, and truth-known validation.

### 10. RNA-virus consensus, quasispecies, and local or global haplotypes

Sequencing an RNA virus normally supplies nucleotide strings derived through reverse transcription
and often amplification; the assembler does not observe the original RNA molecules. Amplification
bias can make depth extremely uneven, while within-sample variation can make several related
sequences compatible with the same short reads [R53], [R54]. These are distinct reconstruction
targets:

1. a **consensus** is one representative base/path choice and need not correspond to any complete
   molecule in a mixture;
2. a **local haplotype** phases variants only within a declared window or linked-read span;
3. a **haplotig** is a partial sequence intended to retain one locally supported haplotype;
4. a **global haplotype** is a full-length phased path; and
5. an **abundance estimate** is a separate model-derived quantity, not k-mer support or read depth.

Virus-focused methods span different points in this taxonomy. SPAdes 4.3.0 exposes separate
`--rnaviral`, `--metaviral`, `--corona`, and reference-requiring `--sewage` modes; mode choice changes
the task and assumptions [R12]. IVA targets paired Illumina RNA-virus reads at highly variable depth
[R53]. VICUNA constructs a consensus representation of a genetically heterogeneous population
[R54]. ShoRAH estimates local haplotypes in windows under an error model [R55]. SAVAGE uses iterative
overlap graphs to reconstruct haplotype-specific contigs without a high-quality reference [R56].
Haploflow, VG-Flow, viaDBG, ViQUF, and PenguiN use coverage/flow,
variation-graph, paired-de-Bruijn-graph, unitig-flow, or overlap-extension models to propose longer
strain paths [R26], [R57], [R58], [R59], [R60]. These outputs and assumptions are not interchangeable.
Independent evaluations have found method- and population-dependent performance, especially as
diversity increases [R61], [R62].

Short reads cannot phase two variants when no molecule-derived read or mate relationship connects
them through an unambiguous chain. A graph containing both alleles therefore supports local
alternatives, not every combinatorial full-length path. Stable v0.1 accepts the A/C/G/T/IUPAC strings
in cDNA-derived FASTA/FASTQ but has no RNA-molecule, strand, reverse-transcription, amplification,
splicing, consensus, haplotype, or abundance model. It preserves branches and reports unitigs; it
must not relabel bubbles as quasispecies or global haplotypes.

### 11. Segmented viral genomes

A segmented genome consists of multiple distinct nucleic-acid molecules, and related viruses can
exchange whole segments by reassortment [R63], [R64]. Sequence coverage can support an assembly for
each segment, but ordinary short-read libraries generally do not identify which segment copies were
packaged in the same particle or belonged to the same replicating genome constellation. Co-occurrence
in one library is not physical linkage.

Stable v0.1 treats every accepted nucleotide fragment uniformly. It may emit unitigs derived from
several segments, but it has no segment identifier, expected-segment model, terminal-motif model,
completeness rule, particle linkage, or reassortment analysis. It must not combine independently
assembled segments into one genome or infer a reassortant. Segment-aware grouping, completeness, and
reassortment are deferred to a separate, reference-aware or experimentally linked analysis with
truth-known segmented controls.

### 12. Low abundance in high background

The main algorithm families are:

1. global abundance filtering;
2. local-relative thresholds, as in IDBA-UD [R10];
3. low-depth rescue and succinct graphs, as in MEGAHIT [R13];
4. metagenomic graph transformations, as in metaSPAdes [R27];
5. exact disk partitioning before graph retention [R3], [R6], [R7].

A single global cutoff is the least defensible default: high-depth error can outnumber true sequence
from a low-abundance component [R10]. Related genomes remain difficult in both CAMI rounds, and
parameter settings materially affect reproducibility [R14], [R15].

The no-sieve reference path should count every accepted observation exactly on disk and expose the
complete exact count histogram. An eligible ADR-0004 two-hit path may route only a recurrent-key
superset to exact counting, but must preserve every threshold-eligible key and its exact support; it
cannot claim a complete below-threshold histogram. Both paths apply only an explicit retention rule
and hard-cap the graph created from retained k-mers. They must not silently increase the threshold to
fit memory. Optional reference depletion belongs to a separate, checksummed analysis layer and is not
part of de novo graph construction.

### 13. Plasmids and candidate circular elements

At least four established strategies exist:

1. plasmidSPAdes uses topology and coverage differences to separate plasmid graph components [R28];
2. Recycler extracts likely cycles using graph, coverage, and paired-read evidence [R29];
3. SCAPP adds plasmid-gene and classifier evidence to cycle peeling [R30];
4. database-assisted tools group and type plasmid-derived contigs after assembly.

An independent benchmark found that small plasmids were more recoverable than large repeat-bearing
plasmids, and none of the tested programs fully and unambiguously reconstructed distinct plasmids
across its dataset [R31]. Coverage can overlap between a plasmid and chromosome, multiple plasmids
can share repeats, and a graph cycle can be a repeat artifact.

VeritAsm v0.1 reports only a canonical `closed_graph_walk` topology for a residual one-in/one-out graph
component and preserves its GFA closure. Construction-read and pair audit fields for that closed walk
are `NA` with an explicit unsupported-audit reason. It does not claim terminal-overlap validation,
junction-spanning support, molecular circularity, plasmid identity, or completeness. Those evidence
fields require a later remapper and truth-known linear-terminal-repeat, concatemer, shared-repeat, and
multiple-circle controls.

### 14. Assembly validation

No single metric establishes correctness:

- The external GAGE-B benchmark compared small bacterial assemblies using contiguity and accuracy and found
  organism- and tool-dependent results rather than one universal winner [R16].
- QUAST defines reference-based genome fraction, duplication, base-error, and misassembly metrics
  and warns that sample/reference differences can resemble assembly errors [R33].
- ALE combines base quality, read agreement, mate orientation/insert, coverage, and k-mer frequency
  in a reference-free likelihood [R32].
- Merqury uses read k-mers for reference-free consensus QV and completeness [R34].

Read-back evidence reuses the construction data and is therefore internal consistency, not
independent truth. Contiguity metrics such as N50 can improve while false joins worsen.

Stable v0.1's read audit performs zero-mismatch, full-read placement enumeration against emitted
**linear** unitigs. It reports complete placement groups and exact read-instance aggregates, or `NA`
when the configured candidate limit prevents complete enumeration. It does not apportion multimapping
reads, infer abundance, reconstruct per-base coverage, or correct GC, PCR, insert, duplicate, and
sampling biases. Per-base or fragment-depth reconstruction requires a separate mapping model,
multimapping policy, coverage schema, and truth-known calibration; until then, “coverage” must not be
used for these placement aggregates.

### 15. Standard sequence, graph, alignment, and variant formats

FASTA and FASTQ are accepted sequence/read containers rather than complete evidence schemas. FASTQ has
historically had incompatible quality conventions, so v0.1 explicitly fixes its accepted grammar and
Phred interpretation instead of relying on a filename extension [R44]. Gzip is a content-detected
transport wrapper, not a sequence format.

GFA describes sequences and graph relationships, but identical record letters do not imply equivalent
graph stages or biological semantics [R45]. Stable v0.1 emits a deterministic documented GFA 1.0
subset with embedded segment sequences and oriented `k-1` overlaps; “complete” means that every
retained stable-v0.1 segment and graph link is represented, not that every GFA record type is emitted.
SAM/BAM and VCF are governed by the maintained GA4GH/samtools specifications [R46]. PAF is a compact
pairwise-mapping format whose core fields need not include a base-level alignment [R47]. Stable v0.1
does not emit SAM/BAM, PAF, or VCF: alignment exports are deferred until a mapper and multimapping
contract exist, and VCF remains reference-relative rather than a direct encoding of graph bubbles.

### 16. Rust implementation reuse

Rust-native reuse must reduce audited risk rather than merely reduce local line count. GGCAT is the
most directly relevant graph candidate identified in this review: it is an MIT-licensed Rust
implementation that builds compacted
and optionally colored de Bruijn graphs from raw sequence data, exposes a Rust API, and can emit GFA
[R42], [R43]. It should be evaluated both as a graph/unitig oracle and as a reuse prototype. Adoption is
not selected because its multiplicity, canonicalization, fragment-support, pair, deterministic-byte,
temporary-state, and evidence-ledger contracts have not been shown equivalent to VeritAsm's.

For FASTX, `needletail` 0.7.3 parses FASTA/FASTQ streams and auto-detects supported compression and
record format [R48], but it has not passed the inherited wrapped-record, concatenated-gzip, corruption,
pair-identifier, diagnostic-context, and allocation tests. `seq_io` 0.3.4 is high-performance but its
documented FASTQ reader accepts only single-line records, so it cannot be the sole compatibility parser
[R49]. `niffler` offers content-sniffed compression but overlaps the selected `flate2` path [R50]. The
older `debruijn` 0.3.4 crate remains useful as a small prototype/oracle [R51]. `noodles` covers many
alignment and variant formats, but the reviewed current workspace exceeds the product MSRV and those
formats are outside the stable de novo core [R52]. Exact release archives, features, licenses, unsafe
code, MSRV, malformed-input behavior, determinism, and Apple/Linux behavior remain gates, not inferred
capabilities.

## Concise competitor capability snapshot

This in-file matrix satisfies the Phase 1 requirement at decision-review resolution. Cells summarize
the reviewed method or documentation; they are not locally reproduced benchmark results. Exact
candidate versions, licenses, artifact-freeze blockers, output-retention rules, and specialized viral
and haplotype comparators are expanded in
[the comparator matrix](docs/COMPETITOR_MATRIX.md).

| Tool or class | Graph / k strategy | Paired-read use | Uneven-depth or diversity behavior | Relevant comparison role | Current evidence state |
|---|---|---|---|---|---|
| Virustic2 | Exact fixed-k de Bruijn graph; maximal unitigs | Mates share fragment-support counting; no insert-aware traversal | Absolute support filtering; no phasing | Frozen behavioral ancestor | Source and bounded audit frozen; blanket CLI/output parity is not implemented |
| VeritAsm v0.1 | Exact independently constructed fixed-k graph; conservative segments maximal under the declared degree/fixed-point boundaries, plus GFA | Strict synchronization plus endpoint co-observations; no pair-created join | Retained branches remain graph structure; no consensus or haplotype model | Subject implementation | Stable fixed-k source exists; scientific comparison matrix is unexecuted |
| SPAdes / metaSPAdes | Multisized de Bruijn graph with graph transformations [R11] | Graph-path and repeat-resolution evidence | Ordinary and metagenomic modes are distinct; mode-specific cleaning | Primary isolate-like and high-background comparators | Candidate version identified; exact artifact/license freeze and execution remain open |
| MEGAHIT | Iterative succinct de Bruijn graphs with local low-depth rescue [R13] | Paired-file acceptance is documented; exact traversal semantics require source-level freeze | Designed for large metagenomic-like inputs | Primary high-background/resource comparator | Candidate release identified; executable and benchmark run not frozen |
| SKESA | Conservative short-read isolate assembler | Method describes insert estimation and paired-read use | Primarily an isolate comparator, not a complex-mixture model | Primary false-join/consensus control | Source-version and bundled-license discrepancy blocks freeze |
| IDBA-UD | Iterative small-to-large k with derived-contig carry-forward [R10] | Paired/scaffold stages | Explicitly targets highly uneven depth | Diagnostic multi-k/uneven-depth control | Legacy operational freeze and input adaptation remain open |
| GGCAT | Rust-native compacted/colored de Bruijn graph and maximal unitigs [R42], [R43] | No reviewed mate-constraint contract | Graph construction rather than strain reconstruction | Graph/unitig oracle and reuse diagnostic | MIT candidate identified; semantics and executable still require freezing |
| Specialized haplotype tools | Window, overlap, paired-dBG, or flow models [R55]--[R60] | Tool-specific linkage models | Local haplotypes, haplotigs, global paths, and abundance are different estimands | Future truth-phased diagnostic only | Not pooled with general assemblers; exact task/version/license freezes are incomplete |

## Concise scientific and engineering risk summary

This summary is part of the Phase 1 review. The full 31-item register, minimum-evidence vocabulary,
controls, and residual risks are maintained in
[docs/SCIENTIFIC_RISKS.md](docs/SCIENTIFIC_RISKS.md).

| Risk group | Failure mechanism | Bounded decision / required evidence |
|---|---|---|
| Error versus true diversity | Recurrent sequencing error can look like a supported bubble, while an absolute threshold can remove a real low-abundance path. | Count exactly, preserve declared alternatives, expose filtering, and test quality/divergence/abundance series; short reads may remain non-identifying. |
| Repeats and paired evidence | Several traversals can fit the same k-mers; chimeric, multimapping, or insert-incompatible pairs can fabricate a join. | Stop at unresolved branches in v0.1; any future join needs role, uniqueness, orientation, insert, distinct supplied-fragment-instance support, conflicts, and truth testing. |
| Mixtures and haplotypes | Local bubbles do not phase distant variants, and coverage/flow can select an unsupported global combination. | Keep branch, local-haplotype, haplotig, and global-path claims distinct; require molecule-spanning or validated model evidence before phasing. |
| Segmentation and circularity | Co-occurring segments need not belong to one particle; a graph cycle may be a repeat or concatemer. | Do not infer a segment constellation; call only `closed_graph_walk` topology until closing-adjacency and orthogonal molecular evidence are available. |
| High-background scaling | Error- and background-derived distinct keys can exhaust RAM or disk after streaming input. | Use exact partitioned counting, explicit caps, authenticated temporary state, deterministic failure, and phase-specific RSS/I/O measurements. |
| Probabilistic acceleration | Bloom false-positive state or an incomplete first/recount pass can alter retained membership or masquerade as exact evidence. | Keep no-sieve exact counting mandatory; optional sieve/scout promotion requires exact output equivalence, integrity/failure tests, honest unavailable fields, and retained benefit. |
| Determinism and transactions | Thread/hash order can alter bytes; separately committed files can leave or replace a partial result. | Canonically order stable boundaries and atomically commit one verified no-replace bundle; rerun across threads, platforms, failures, and filesystems. |
| Validation and claims | Read-back reuses construction evidence, a single metric can reward misassembly, and post-hoc selection can hide regressions. | Freeze datasets, tools, parameters, evaluators, exclusions, and metrics; retain all failures and separate internal consistency from independent truth. |

## Central alternatives and v0.1 decisions

| Decision | Options evaluated | Selected v0.1 choice | Unresolved evidence requirement |
|---|---|---|---|
| Graph representation | Explicit packed exact dBG; overlap/string graph; Bifrost-, Cuttlefish-, GGCAT-, or BCALM-style direct/disk compaction; succinct dBG | Exact packed dBG with deterministic segment compaction, degree/fixed-point boundaries, and a hard retained-state cap; no unqualified maximality or safe-string claim | Property tests for topology, mirror symmetry, oriented-view and backing-key conservation, cap behavior, palindromes, self-complemental edges, and closed-walk components; comparison with a graph/unitig oracle |
| Count storage | In-memory hash; DSK fixed-memory partitions; KMC signature/sort bins; minimizer buckets | Exact deterministic disk-backed partitions and exact final counts | Byte/count equivalence to exact in-memory counting across threads, lanes, SE/PE, quality, ambiguity, skew, and failures |
| Probabilistic acceleration | No sieve; two-hit Bloom superset plus exact recount; approximate final counters; probabilistic graph; scheduling-only scout | No-sieve exact oracle plus conditionally authorized two-hit sieve; default-off experimental scout; no approximate final count or graph membership | Eligibility and proof-premise tests, no-sieve byte equivalence for shared exact fields, honest NA/bypass behavior for unavailable below-threshold statistics, and retained workload-specific benefit |
| k strategy | Fixed k; independent multi-k children; IDBA-style iterative carry-forward; SPAdes multisized graph; MEGAHIT iterative succinct graph | Exactly one independently constructed fixed-k graph per stable 0.1 result bundle; multi-k wrapper deferred | Parent/child manifest and concordance schema plus truth-known comparison with fixed-k and established iterative assemblers |
| Error, quality, ambiguity, and complexity | Hard quality filtering; quality-weighted counting; ambiguity expansion/substitution/window reset; Quake; BayesHammer; BFC; Lighter/Musket; low-complexity deletion | Immutable reads; hard per-base quality acceptance; non-ACGT resets windows; exact integer events; no quality weighting, ambiguity expansion, correction, or low-complexity deletion | Variable-quality, systematic-error, ambiguity, homopolymer, adapter, repeat, and low-abundance truth sets |
| Bubble policy | Majority smoothing; relative cleaning; color preservation; coverage flow | Preserve in 0.1; no bubble-collapse or consensus profile | Minor-path precision/recall and false-consensus junctions are required before any later collapse mode |
| RNA and within-sample diversity | Generic nucleotide unitigs; one consensus; local haplotypes; haplotigs; global haplotypes plus abundance | Accept cDNA-derived nucleotide reads under the ordinary DNA-string rules; preserve local alternatives; make no RNA, consensus, phasing, quasispecies, or abundance claim | Explicit reverse-transcription/amplification model and separate local/global phasing benchmarks against specialized tools |
| Segmented genomes | Independent segment assembly; reference/motif-assisted grouping; segment-constellation or reassortment inference | Emit only ordinary unitigs; no segment grouping, completeness, particle linkage, or reassortment inference | Truth-known segmented mixtures with segment-copy, coinfection, and reassortant controls plus an explicit linkage model |
| Pair use | Post-hoc endpoints; paired dBG; exSPAnder path extension; IDBA local assembly | Auditable exact unique-placement cross-unitig endpoint observations; no orientation/insert/span/gap qualification, sequence joining, or branch resolution in 0.1 | Insert recovery, chimeric pairs, multimapping, inside/outside-span repeats, and false-junction rate before any qualification or join |
| High-background scaling | Global cutoff; local thresholds; mercy k-mers; metagenomic transforms; exact partitions | Exact disk count, explicit retention, hard graph cap, atomic failure | Peak RSS and disk curves, free-space failure injection, and retained low-abundance truth |
| Circular elements | Closed graph walks; coverage separation; cycle peeling; annotation-assisted peeling; database grouping | `closed_graph_walk` topology only; closed-walk read/pair evidence is explicitly unavailable in 0.1 | True circle, linear terminal-repeat, concatemer, shared-repeat, and multiple-plasmid controls before molecular or junction claims |
| Validation and coverage | Reference truth; read likelihood; read-k-mer audit; exact placement enumeration; graph invariants; modeled per-base depth | Report applicable components separately; exact linear-unitig placement aggregates are not called coverage and multimappers are not apportioned | Preregistered datasets, metrics, parameters, exclusions, failures, competitor versions, and a separate mapping/coverage model before depth reconstruction |

## Known failure modes

| Failure | Mechanism | Required response | Evidence |
|---|---|---|---|
| Small-k collapse | Repeats or similar components share k-mers | Preserve branch; compare larger k | [R10], [R11] |
| Large-k fragmentation | Low-depth regions lack required k-mers | Compare smaller k; do not fill without original evidence | [R10], [R11] |
| Iterative error propagation | A derived small-k contig is reused as if observed | Keep derived provenance; not selected for v0.1 | [R10], [R13] |
| Low abundance removed as error | Global cutoff confuses mixture abundance with error | Exact count; explicit threshold; local evidence and sensitivity analysis | [R10], [R14], [R15] |
| Error retained as variation | Systematic or high-quality errors recur | Report quality, fragment support, strand/position patterns, and uncertainty | [R17], [R19] |
| Unsafe graph segment | Incomplete coverage can make even a conventional maximal unitig absent from the generating genome with error-free reads | State the exact boundary model, call the output a conservative graph segment rather than an unqualified maximal or safe string, measure false sequence, and evaluate safe-string alternatives | [R40], [R41] |
| Palindromic underassembly | Naive bidirected compaction can split a palindromic unitig | Make self-reverse-complement nodes boundaries and test doubled/bidirected oracles | [R40] |
| Bubble overcollapse | Variant and error bubbles share topology | Preserve default alternatives and record any collapse | [R2], [R25] |
| Haplotype overphasing | Unlinked local variants admit several global combinations | Report local alternatives; do not emit a global strain path without molecule-spanning evidence and a validated model | [R55], [R56], [R61], [R62] |
| False segment constellation | Segments co-occur in a library but are not linked to one particle or strain | Assemble independently; do not group into a complete segmented genome or infer reassortment | [R63], [R64] |
| Unsupported repeat traversal | More than one continuation fits sequence | Stop contig | [R22], [R23], [R24] |
| False pair join | Chimeric, role-swapped, multimapping, or insert-incompatible pairs | Reject malformed pairs; retain ambiguous/conflicting links | [R22], [R24] |
| Graph-memory exhaustion | Too many retained k-mers survive the explicit rule | Fail at the hard cap without changing output | [R3], [R4], [R5], [R6], [R7], [R8] |
| Disk exhaustion/corruption | Exact partitions are incomplete or mixed between runs | Manifest, checksum, free-space/error checks, isolated temp directory, atomic cleanup | [R6], [R7], [R8] |
| False circularity | Repeats or linear structures create a graph cycle | Report only candidate topology and contrary evidence | [R28], [R29], [R30], [R31] |
| Plasmid/chromosome merging | Shared repeats and similar coverage join replicons | Do not assign replicon identity | [R29], [R31] |
| Related-component collapse | Similar strains/genomes behave as long approximate repeats | Preserve ambiguity; prohibit strain/genome assignment | [R14], [R15], [R27] |
| Read-back self-confirmation | Construction reads also score the result | Label as internal consistency and add truth-based evaluation | [R32], [R34] |
| Metric substitution | N50 hides base errors, duplication, or false junctions | Report a metric vector, never one opaque winner score | [R16], [R33] |

## Selected v0.1 features

The following are design selections, not implementation-completion claims:

1. database-free, offline de novo assembly of Illumina-like short-read DNA;
2. baseline-compatible plain and content-detected-gzip FASTA/FASTQ, SE/PE, and ordered lanes;
3. strict mate identity, role, order, cardinality, and physical-input validation;
4. exact canonical occurrence and fragment-support modes with overflow detection;
5. exact deterministic disk-backed k-mer counting with a versioned manifest;
6. a mandatory no-sieve exact path and an isolated ADR-0004 `BloomFilter`/`TwoHitSieve` research
   prototype; stable-pipeline enablement remains subject to eligibility, spool integration,
   equivalence, failure, and benefit gates;
7. exact in-memory retained graph with a user-visible deterministic hard cap;
8. exactly one fixed-k graph per stable result bundle, derived only from original reads;
9. deterministic unitig compaction with conservative ambiguity boundaries;
10. operational `retain_all`, `thresholded`, and fully serialized custom parameter profiles, with no
    deleting tip/bubble/low-complexity rule or consensus implication;
11. unqualified exact unique-placement cross-unitig endpoint observations that never change graph or
    unitig sequence in 0.1;
12. deterministic FASTA, the complete retained graph in a documented GFA 1.0 subset, per-unitig
    evidence table, structured report, and HTML view;
13. `closed_graph_walk` topology fields with read/pair audit explicitly unavailable and no molecular
    circularity conclusion;
14. complete output-bundle transactional behavior;
15. read-back auditing labelled as internal consistency;
16. preregistered synthetic and public small-genome validation with retained failures.

## Deferred features

- fully disk-resident retained graph construction and traversal;
- stable enablement of the two-hit sieve until every ADR-0004 exactness, failure, determinism, schema,
  and workload-benefit gate is satisfied;
- the syncmer/Bloom/Count-Min scout beyond default-off experimental evaluation;
- stable multi-k parent/child bundles, cross-k concordance, deduplication, or selection;
- overlap/string-graph assembly and omnitig/safe-string reconstruction;
- iterative derived-contig carry-forward;
- probabilistic quality weighting or read correction;
- low-complexity scoring or deletion;
- bounded superbubble classification beyond conservative preservation;
- any bubble-collapse or consensus profile;
- local assembly of anchored mates;
- gap-bearing scaffolds;
- pair-driven branch traversal or unitig joining;
- coverage-flow mixture deconvolution;
- per-base or fragment-depth reconstruction and multimapping allocation;
- RNA-specific strand, reverse-transcription, amplification, splice, and consensus models;
- local viral haplotype, haplotig, global strain-path, quasispecies, and abundance reconstruction;
- segment-aware grouping, segment-completeness, particle linkage, and reassortment analysis;
- optional reference depletion, similarity, classification, or polishing;
- plasmid reconstruction/typing and copy-number estimation;
- long-read, hybrid, linked-read, transcript/isoform, or polyploid-specific assembly.

## Rejected v0.1 features and behaviors

- probabilistic graph membership in the exact default;
- Bloom-, Count-Min-, or scout-derived final counts, graph membership, cleaning, or path choices;
- suppressing any fragment from the exact residual path because of scout priority;
- silent adaptive support thresholds;
- silent read correction;
- silently discarding lower-support bubble paths;
- forced traversal to produce a genome-length answer;
- calling an unresolved bubble a phased local or global haplotype;
- grouping independently assembled segments into one virion/genome constellation without linkage;
- treating derived contigs as independent observations;
- fabricating sequence between pair-linked contigs;
- labelling a graph component as one organism or strain;
- labelling a cycle as a complete circular molecule or plasmid;
- taxonomic, contaminant, presence/absence, regulatory, or clinical conclusions;
- mandatory external commands, Python, Java, Conda, or reference databases;
- runtime network access.

## Accepted probabilistic-acceleration decision

> **STATUS: ADR-0004 ACCEPTED; LOCAL BLOOM/TWO-HIT PRIMITIVES IMPLEMENTED AND MODULE-TESTED;
> STABLE PIPELINE INTEGRATION AND THE SYNCMER/CONTROL SCOUT NOT IMPLEMENTED OR VALIDATED.**

The decision retains no probabilistic acceleration as the mandatory reference and fallback. The
safe-Rust prototype implements fixed-size insertion-only Bloom membership and an ordered two-hit
coordinator, but the exact counter and CLI do not consume it. It rejects counting Bloom, quotient,
cuckoo, or Count-Min structures as final counters; probabilistic graph membership; and any candidate
selector that lacks a complete exact residual path.

The two-hit sieve is conditionally authorized only when all of these boundaries hold:

- support is counted by fragment instance, with canonical keys deduplicated across both mates;
- `support_unit = supplied_fragment_instance` and mode-neutral `min_support >= 2`, with no singleton
  rescue or other sub-two admission rule;
- an immutable checksummed spool is consumed completely in both passes;
- stateful first-pass Bloom updates occur in fragment order, and filters are insertion-only,
  deterministically specified, integrity-checked, and complete every requested insertion;
- the second pass recounts candidate full keys in the exact external counter with checked integers;
- the no-sieve exact path remains available for ineligible configurations and recovery; and
- exact retained streams and all shared downstream scientific artifacts match the no-sieve oracle.

False positives may increase I/O and CPU; they cannot be treated as evidence. The sieve does not
recover exact statistics for distinct non-routed keys. Exact pre-threshold distinct-key counts,
complete support histograms, and complete below-threshold removal ledgers therefore require the
no-sieve path or another exact method; otherwise a compatible experimental schema must report `NA`
with a reason rather than an estimate.

The scout remains default-off and experimental. It may reorder whole validated fragments using
versioned syncmer/Bloom/Count-Min diagnostics, but every fragment must eventually enter the exact
pipeline. Scout-on and scout-off completed runs must have byte-identical core scientific artifacts;
an interrupted residual path is incomplete, not a successful or negative result. Scout priority,
enrichment, and background-filter membership are operational diagnostics, not novelty, abundance,
presence, absence, or confidence.

## Explicitly prohibited claims

VeritAsm v0.1 must not claim:

- better accuracy, speed, memory, or completeness than Virustic2 or an established assembler without
  a retained task-equivalent comparison;
- that a future independent-child multi-k wrapper improves assembly;
- whole-run memory independent of graph complexity;
- support for arbitrarily large or heavily contaminated inputs;
- that disk-backed counting makes graph traversal disk-backed;
- that module-tested Bloom/two-hit primitives mean stable-pipeline integration, end-to-end
  validation, or benefit on any workload; the syncmer/control scout remains unimplemented;
- that two-hit sieving supplies exact singleton cardinality, a complete pre-threshold histogram, or
  complete below-threshold provenance without a separate exact method;
- that a scout priority, Bloom membership result, or Count-Min estimate is biological novelty,
  abundance, confidence, presence, or absence;
- that a retained low-support path is biological;
- that a removed low-support path is sequencing error;
- that pair evidence resolves a repeat without truth-known false-junction validation;
- that a contig is a genome, strain, haplotype, synthetic construct, plasmid, organelle, virus, or
  other biological entity;
- that graph closure establishes molecular circularity or completeness;
- that graph support estimates organism abundance, concentration, or plasmid copy number;
- that failure to reconstruct an agent establishes its absence;
- adventitious-agent sensitivity, specificity, limit of detection, or suitability for lot release;
- that read-back agreement is independent validation;
- deterministic output across operating systems until cross-platform bundle digests pass;
- absence of runtime network access until static and runtime enforcement checks pass;
- production readiness, regulatory validation, clinical validity, or diagnostic suitability.

## Recommendation

Proceed with an exact, compacted, **single-k-per-bundle** vertical slice whose primary innovation is
inspectable evidence rather than forced contiguity. Emit conservative segments maximal only under the
declared degree and fixed-point boundary model, without implying conventional maximal-unitig,
safe-string, or molecular correctness. Implement disk-backed exact counting now
because the high-background application can otherwise exhaust memory before the retained graph is
known. Keep that graph hard-capped in memory and fail atomically rather than silently changing the
support threshold.

Keep no-sieve exact counting as the oracle and fallback. Treat the two-hit sieve only as an eligible
exactness-preserving optimization after its falsification gates pass, and keep the scout default-off
and isolated from every scientific decision. Neither mechanism supports a benefit claim before
retained workload-specific results exist.

Treat the multi-k parent/child wrapper and reconciliation, omnitig/safe-string reconstruction, local
bubble collapse, coverage reconstruction, and pair-constrained traversal as separately deferred,
independently testable transformations. Every output sequence must identify which implemented
transformations affected it and which alternatives remain. VeritAsm becomes demonstrably better than
its ancestor only when retained benchmarks show more correct sequence, fewer false junctions,
improved low-abundance recovery, more useful evidence, or successful handling of a declared input that
the ancestor cannot handle.

## References

- [R1] Pevzner PA, Tang H, Waterman MS. “An Eulerian path approach to DNA fragment assembly.”
  *Proceedings of the National Academy of Sciences*. 2001.
- [R2] Zerbino DR, Birney E. “Velvet: Algorithms for de novo short read assembly using de Bruijn
  graphs.” *Genome Research*. 2008.
- [R3] Chikhi R, Limasset A, Medvedev P. “Compacting de Bruijn graphs from sequencing data quickly
  and in low memory.” *Bioinformatics*. 2016.
- [R4] Holley G, Melsted P. “Bifrost: highly parallel construction and indexing of colored and
  compacted de Bruijn graphs.” *Genome Biology*. 2020.
- [R5] Khan J, Kokot M, Deorowicz S, Patro R. “Scalable, ultra-fast, and low-memory construction of
  compacted de Bruijn graphs with Cuttlefish 2.” *Genome Biology*. 2022.
- [R6] Rizk G, Lavenier D, Chikhi R. “DSK: k-mer counting with very low memory usage.”
  *Bioinformatics*. 2013.
- [R7] Deorowicz S, Kokot M, Grabowski S, Debudaj-Grabysz A. “KMC 2: fast and resource-frugal k-mer
  counting.” *Bioinformatics*. 2015.
- [R8] Kokot M, Długosz M, Deorowicz S. “KMC 3: counting and manipulating k-mer statistics.”
  *Bioinformatics*. 2017.
- [R9] Pell J et al. “Scaling metagenome sequence assembly with probabilistic de Bruijn graphs.”
  *Proceedings of the National Academy of Sciences*. 2012.
- [R10] Peng Y, Leung HCM, Yiu SM, Chin FYL. “IDBA-UD: a de novo assembler for single-cell and
  metagenomic sequencing data with highly uneven depth.” *Bioinformatics*. 2012.
- [R11] Bankevich A et al. “SPAdes: A New Genome Assembly Algorithm and Its Applications to
  Single-Cell Sequencing.” *Journal of Computational Biology*. 2012.
- [R12] SPAdes team. “SPAdes Assembly Toolkit command-line options and running modes.” Reviewed with
  release 4.3.0; accessed 2026-09-03.
- [R13] Li D et al. “MEGAHIT: an ultra-fast single-node solution for large and complex metagenomics
  assembly via succinct de Bruijn graph.” *Bioinformatics*. 2015.
- [R14] Sczyrba A et al. “Critical Assessment of Metagenome Interpretation—a benchmark of
  metagenomics software.” *Nature Methods*. 2017.
- [R15] Meyer F et al. “Critical Assessment of Metagenome Interpretation: the second round of
  challenges.” *Nature Methods*. 2022.
- [R16] Magoc T et al. “GAGE-B: an evaluation of genome assemblers for bacterial organisms.”
  *Bioinformatics*. 2013.
- [R17] Kelley DR, Schatz MC, Salzberg SL. “Quake: quality-aware detection and correction of
  sequencing errors.” *Genome Biology*. 2010.
- [R18] Nikolenko SI, Korobeynikov AI, Alekseyev MA. “BayesHammer: Bayesian clustering for error
  correction in single-cell sequencing.” *BMC Genomics*. 2013.
- [R19] Li H. “BFC: correcting Illumina sequencing errors.” *Bioinformatics*. 2015.
- [R20] Song L, Florea L, Langmead B. “Lighter: fast and memory-efficient sequencing error
  correction without counting.” *Genome Biology*. 2014.
- [R21] Liu Y, Schröder J, Schmidt B. “Musket: a multistage k-mer spectrum-based error corrector for
  Illumina sequence data.” *Bioinformatics*. 2013.
- [R22] Medvedev P, Pham S, Chaisson M, Tesler G, Pevzner PA. “Paired de Bruijn Graphs: A Novel
  Approach for Incorporating Mate Pair Information into Genome Assemblers.” *Journal of
  Computational Biology*. 2011.
- [R23] Prjibelski AD et al. “ExSPAnder: a universal repeat resolver for DNA fragment assembly.”
  *Bioinformatics*. 2014.
- [R24] Sahlin K et al. “BESST—efficient scaffolding of large fragmented assemblies.” *BMC
  Bioinformatics*. 2014.
- [R25] Iqbal Z, Caccamo M, Turner I, Flicek P, McVean G. “De novo assembly and genotyping of
  variants using colored de Bruijn graphs.” *Nature Genetics*. 2012.
- [R26] Fritz A et al. “Haploflow: strain-resolved de novo assembly of viral genomes.” *Genome
  Biology*. 2021.
- [R27] Nurk S, Meleshko D, Korobeynikov A, Pevzner PA. “metaSPAdes: a new versatile metagenomic
  assembler.” *Genome Research*. 2017.
- [R28] Antipov D et al. “plasmidSPAdes: assembling plasmids from whole genome sequencing data.”
  *Bioinformatics*. 2016.
- [R29] Rozov R et al. “Recycler: an algorithm for detecting plasmids from de novo assembly graphs.”
  *Bioinformatics*. 2017.
- [R30] Pellow D et al. “SCAPP: an algorithm for improved plasmid assembly in metagenomes.”
  *Microbiome*. 2021.
- [R31] Arredondo-Alonso S, Willems RJL, van Schaik W, Schürch AC. “On the (im)possibility of
  reconstructing plasmids from whole-genome short-read sequencing data.” *Microbial Genomics*.
  2017.
- [R32] Clark SC et al. “ALE: a generic assembly likelihood evaluation framework for assessing the
  accuracy of genome and metagenome assemblies.” *Bioinformatics*. 2013.
- [R33] Gurevich A, Saveliev V, Vyahhi N, Tesler G. “QUAST: quality assessment tool for genome
  assemblies.” *Bioinformatics*. 2013.
- [R34] Rhie A et al. “Merqury: reference-free quality, completeness, and phasing assessment for
  genome assemblies.” *Genome Biology*. 2020.
- [R35] Bloom BH. “Space/time trade-offs in hash coding with allowable errors.” *Communications of
  the ACM*. 1970.
- [R36] Cormode G, Muthukrishnan S. “An improved data stream summary: the Count-Min sketch and its
  applications.” *Journal of Algorithms*. 2005.
- [R37] Edgar R. “Syncmers are more sensitive than minimizers for selecting conserved k-mers in
  biological sequences.” *PeerJ*. 2021.
- [R38] Myers EW. “The fragment assembly string graph.” *Bioinformatics*. 2005.
- [R39] Simpson JT, Durbin R. “Efficient de novo assembly of large genomes using compressed data
  structures.” *Genome Research*. 2012.
- [R40] Rahman A, Medvedev P. “Assembler artifacts include misassembly because of unsafe unitigs and
  underassembly because of bidirected graphs.” *Genome Research*. 2022.
- [R41] Tomescu AI, Medvedev P. “Safe and Complete Contig Assembly Through Omnitigs.” *Journal of
  Computational Biology*. 2017.
- [R42] Cracco A, Tomescu AI. “Extremely-fast construction and querying of compacted and colored de
  Bruijn graphs with GGCAT.” *Genome Research*. 2023.
- [R43] GGCAT maintainers. “GGCAT v2.2.0 source, usage, Rust API, and license.” Release snapshot
  reviewed 2026-09-03.
- [R44] Cock PJA et al. “The Sanger FASTQ file format for sequences with quality scores, and the
  Solexa/Illumina FASTQ variants.” *Nucleic Acids Research*. 2010.
- [R45] GFA Format Specification Working Group. “Graphical Fragment Assembly format, GFA1.”
  Source snapshot `9774d44132884d9a019c0f2682cb109be23c2db4` reviewed 2026-09-03.
- [R46] GA4GH/samtools maintainers. “SAM/BAM and VCF specifications.” Source snapshot
  `510c107ba89aa13113818c9e4fa3071283cb7f7b` reviewed 2026-09-03.
- [R47] Li H. “PAF: a Pairwise mApping Format.” Source snapshot
  `2cd690de1e6af3e0438b7df0a99dd3e3b27ad6f9` reviewed 2026-09-03.
- [R48] `needletail` maintainers. “needletail 0.7.3 documentation.”
- [R49] `seq_io` maintainers. “seq_io 0.3.4 documentation.”
- [R50] `niffler` maintainers. “niffler 3.0.1 documentation and package metadata.”
- [R51] `debruijn` maintainers. “debruijn 0.3.4 documentation.”
- [R52] `noodles` maintainers. “noodles 0.116.0 documentation and package metadata.”
- [R53] Hunt M et al. “IVA: accurate de novo assembly of RNA virus genomes.”
  *Bioinformatics*. 2015.
- [R54] Yang X et al. “De novo assembly of highly diverse viral populations.” *BMC Genomics*.
  2012.
- [R55] Zagordi O et al. “ShoRAH: estimating the genetic diversity of a mixed sample from
  next-generation sequencing data.” *BMC Bioinformatics*. 2011.
- [R56] Baaijens JA et al. “De novo assembly of viral quasispecies using overlap graphs.” *Genome
  Research*. 2017.
- [R57] Baaijens JA et al. “Strain-aware assembly of genomes from mixed samples using flow variation
  graphs.” In *Research in Computational Molecular Biology*. 2020.
- [R58] Freire B et al. “Inference of viral quasispecies with a paired de Bruijn graph.”
  *Bioinformatics*. 2021.
- [R59] Freire B et al. “ViQUF: de novo viral quasispecies reconstruction using unitig-based flow
  networks.” *IEEE/ACM Transactions on Computational Biology and Bioinformatics*. 2023.
- [R60] Jochheim A et al. “Strain-resolved de-novo metagenomic assembly of viral genomes and
  microbial 16S rRNAs.” *Microbiome*. 2024.
- [R61] Schirmer M, Sloan WT, Quince C. “Benchmarking of viral haplotype reconstruction programmes:
  an overview of the capacities and limitations of currently available programmes.” *Briefings in
  Bioinformatics*. 2014.
- [R62] Eliseev A et al. “Evaluation of haplotype callers for next-generation sequencing of
  viruses.” *Infection, Genetics and Evolution*. 2020.
- [R63] Smits SL et al. “Recovering full-length viral genomes from metagenomes.” *Frontiers in
  Microbiology*. 2015.
- [R64] Varsani A et al. “Notes on recombination and reassortment in multipartite/segmented
  viruses.” *Current Opinion in Virology*. 2018.

[R1]: https://doi.org/10.1073/pnas.171285098
[R2]: https://doi.org/10.1101/gr.074492.107
[R3]: https://doi.org/10.1093/bioinformatics/btw279
[R4]: https://doi.org/10.1186/s13059-020-02135-8
[R5]: https://doi.org/10.1186/s13059-022-02743-6
[R6]: https://doi.org/10.1093/bioinformatics/btt020
[R7]: https://doi.org/10.1093/bioinformatics/btv022
[R8]: https://doi.org/10.1093/bioinformatics/btx304
[R9]: https://doi.org/10.1073/pnas.1121464109
[R10]: https://doi.org/10.1093/bioinformatics/bts174
[R11]: https://doi.org/10.1089/cmb.2012.0021
[R12]: https://ablab.github.io/spades/running.html
[R13]: https://doi.org/10.1093/bioinformatics/btv033
[R14]: https://doi.org/10.1038/nmeth.4458
[R15]: https://doi.org/10.1038/s41592-022-01431-4
[R16]: https://doi.org/10.1093/bioinformatics/btt273
[R17]: https://doi.org/10.1186/gb-2010-11-11-r116
[R18]: https://doi.org/10.1186/1471-2164-14-S1-S7
[R19]: https://doi.org/10.1093/bioinformatics/btv290
[R20]: https://doi.org/10.1186/s13059-014-0509-9
[R21]: https://doi.org/10.1093/bioinformatics/bts690
[R22]: https://doi.org/10.1089/cmb.2011.0151
[R23]: https://doi.org/10.1093/bioinformatics/btu266
[R24]: https://doi.org/10.1186/1471-2105-15-281
[R25]: https://doi.org/10.1038/ng.1028
[R26]: https://doi.org/10.1186/s13059-021-02426-8
[R27]: https://doi.org/10.1101/gr.213959.116
[R28]: https://doi.org/10.1093/bioinformatics/btw493
[R29]: https://doi.org/10.1093/bioinformatics/btw651
[R30]: https://doi.org/10.1186/s40168-021-01068-z
[R31]: https://doi.org/10.1099/mgen.0.000128
[R32]: https://doi.org/10.1093/bioinformatics/bts723
[R33]: https://doi.org/10.1093/bioinformatics/btt086
[R34]: https://doi.org/10.1186/s13059-020-02134-9
[R35]: https://doi.org/10.1145/362686.362692
[R36]: https://doi.org/10.1016/j.jalgor.2003.12.001
[R37]: https://doi.org/10.7717/peerj.10805
[R38]: https://doi.org/10.1093/bioinformatics/bti1114
[R39]: https://doi.org/10.1101/gr.126953.111
[R40]: https://doi.org/10.1101/gr.276601.122
[R41]: https://doi.org/10.1089/cmb.2016.0141
[R42]: https://doi.org/10.1101/gr.277615.122
[R43]: https://github.com/algbio/ggcat/tree/v2.2.0
[R44]: https://doi.org/10.1093/nar/gkp1137
[R45]: https://github.com/GFA-spec/GFA-spec/blob/9774d44132884d9a019c0f2682cb109be23c2db4/GFA1.md
[R46]: https://github.com/samtools/hts-specs/tree/510c107ba89aa13113818c9e4fa3071283cb7f7b
[R47]: https://github.com/lh3/miniasm/blob/2cd690de1e6af3e0438b7df0a99dd3e3b27ad6f9/PAF.md
[R48]: https://docs.rs/needletail/0.7.3/needletail/
[R49]: https://docs.rs/seq_io/0.3.4/seq_io/
[R50]: https://crates.io/crates/niffler/3.0.1
[R51]: https://docs.rs/debruijn/0.3.4/debruijn/
[R52]: https://crates.io/crates/noodles/0.116.0
[R53]: https://doi.org/10.1093/bioinformatics/btv120
[R54]: https://doi.org/10.1186/1471-2164-13-475
[R55]: https://doi.org/10.1186/1471-2105-12-119
[R56]: https://doi.org/10.1101/gr.215038.116
[R57]: https://doi.org/10.1007/978-3-030-45257-5_14
[R58]: https://doi.org/10.1093/bioinformatics/btaa782
[R59]: https://doi.org/10.1109/TCBB.2022.3190282
[R60]: https://doi.org/10.1186/s40168-024-01904-y
[R61]: https://doi.org/10.1093/bib/bbs081
[R62]: https://doi.org/10.1016/j.meegid.2020.104277
[R63]: https://doi.org/10.3389/fmicb.2015.01069
[R64]: https://doi.org/10.1016/j.coviro.2018.08.013
