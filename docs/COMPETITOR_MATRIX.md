# Comparator matrix and benchmark-freeze policy

- Review date: 2026-09-03
- Status: decision document; comparator installations and benchmark results are not yet verified
- Product: VeritAsm, a neutral evidence-first assembler for Illumina-like short reads
- Frozen ancestor: Virustic2 commit `b211915fc7cce82629766b77024463c6cabcc749`

## Decision

VeritAsm will be compared with tools that solve the same de novo short-read reconstruction task,
not with a post hoc collection of tools that happened to score well. The required comparator set is
Virustic2, SPAdes, MEGAHIT, and SKESA. IDBA-UD, ABySS, Minia, and GGCAT are diagnostic controls for
uneven depth, paired/scaffolded assembly, low-memory graph construction, and graph/unitig construction.
Velvet is a historical fixed-k control. Other wrappers, target-enriched methods, long-read assemblers,
reference-guided methods, and strain/haplotype reconstruction systems are not pooled into the primary
leaderboard.

This selection does not imply that any comparator is a ground truth or that VeritAsm is better. A
comparison becomes evidence only after the exact executable, input, command, resources, outputs,
failures, and evaluation procedure have been retained.

## Tier definitions

| Tier | Purpose | Inclusion rule |
|---|---|---|
| 0 | Ancestor compatibility and regression | The exact implementation from which behavior is migrated |
| 1 | Required task-equivalent comparison | General short-read de novo assembler with a defensible isolate or high-background mode |
| 2 | Diagnostic comparison | Tests one important design alternative but is less directly equivalent or has an operational caveat |
| 3 | Historical or boundary control | Scientifically informative, but stale, specialized, or operationally non-equivalent |
| Excluded | No pooled score | Uses truth-like priors, targets a different data type, or silently changes the evidence population |

## Comparator freeze matrix

“Candidate freeze” means the version identified by this review. It is not a completed freeze until
the full commit, archive or image digest, executable digest, build record, and smoke test are in the
validation manifest. Short Git hashes below are locator hints, not sufficient provenance.

| Tier | Comparator and candidate freeze | Algorithmic role and task fit | Principal outputs to retain | License and maintenance evidence | Important limits before use |
|---|---|---|---|---|---|
| 0 | **Virustic2**, commit `b211915fc7cce82629766b77024463c6cabcc749` | Exact, fixed-k, database-free unitig baseline; exercises migrated FASTX, support, determinism, and atomic-output behavior | FASTA, JSON, stderr/stdout, full command | MIT; immutable audited source snapshot | Not a modern multi-k assembler; its development benchmark is not an accuracy benchmark |
| 1 | **SPAdes 4.3.0**, tag resolved to `a7f6e55b69f6b8eb87dfd7b014e22fc324a8a01b` on the review date | Multi-k assembly with graph transformation and paired-read repeat resolution. Run ordinary SPAdes on preregistered isolate-like cases and metaSPAdes on preregistered high-background/mixture cases; these are distinct comparators | `contigs.fasta`, `scaffolds.fasta`, `before_rr.fasta` if produced, `assembly_graph_with_scaffolds.gfa`, `assembly_graph.fastg`, path files, corrected reads if produced, logs and parameters | The v4.3.0 top-level file grants GPL version 2; an unresolved issue opened against 4.0.0 reports GPL-3+-licensed and Apache-2.0-licensed files. The exact v4.3.0 artifact still requires license reconciliation [C1-C4], [C31], [C32] | Mode changes the algorithm. SPAdes correction, pre-repeat-resolution sequence, contigs, and scaffolds must not be collapsed into one result. Freeze a source-archive SHA-256 before execution; a remotely observed tag/commit is not an artifact digest |
| 1 | **MEGAHIT 1.2.9**, tag resolved to `d729cca1e201ca16749b67f750b0bc5465c9a990` on the review date | Iterative succinct de Bruijn graphs and local low-depth rescue; direct fit for large high-background or metagenomic-like inputs | Final contigs FASTA, logs, options, intermediate graphs, and FASTG conversion when enabled | Reviewed tagged source notices include GPL-3.0-or-later; exact-artifact scan remains controlling. The latest upstream release found was 2019 [C5-C7], [C33] | Call it release-stale, not abandoned. Run default and `meta-sensitive` only as separately preregistered profiles. CPU-feature-specific builds can change performance. Intermediate FASTG identifiers may not map one-to-one to final contigs. Paired-file acceptance is documented, but mate-constraint semantics require source tracing |
| 1 | **SKESA source version 2.5.1**; exact commit/build unresolved | Conservative short-read isolate assembler and useful false-join/consensus control; less matched to complex mixtures than SPAdes/metaSPAdes or MEGAHIT | Primary FASTA, logs, command, and separately generated connector GFA if used | NCBI core is public-domain work, but bundled `Integer`/`LargeInt` files declare AGPL-3.0; current source/version evidence is inconsistent with the GitHub release page [C8-C12] | GitHub release metadata exposed 2.4.0 while source and Bioconda exposed 2.5.1. Freeze an exact source commit or package build and archive every applicable license. The separate GFA connector is not the assembler's native graph |
| 2 | **IDBA-UD 1.1.3** | Iterative small-to-large k assembly designed for highly uneven depth; tests derived-contig carry-forward against stable fixed-k output and the proposed future independent-child wrapper | `contig.fa`, `scaffold.fa`, intermediate contigs/graphs, log | GPL-2.0-or-later; legacy upstream [C13-C15] | Legacy build/toolchain risk. Input commonly requires deterministic FASTQ-to-FASTA/interleaving adaptation. Package maintenance is not evidence of upstream maintenance |
| 2 | **ABySS 2.3.10**, tag resolved to `5dc06d676b4c2bd51a5f7e38f79eed273bb6b9fa` on the review date | Mature paired short-read assembler; exercises paired/scaffold graph and Bloom-filter scale alternatives | unitig/contig/scaffold FASTA, DOT/path files, SAM/BAM where generated, `abyss-pe versions`, logs | Top-level grant is GPL version 3 and directs readers to per-file copyright/license records; the exact archive controls [C16-C19], [C34] | Distributed/HPC features and scaffolding can make the run non-equivalent. Predeclare one local-machine recipe and score contigs separately from scaffolds |
| 2 | **Minia 3.2.6** | Low-memory compact de Bruijn graph/contig control | Contig FASTA, command, logs, resource measurements | AGPL-3.0-or-later; packaged release evidence [C20-C22] | Standalone Minia is not a paired scaffolder. Its repository recommends a GATB multi-k pipeline that introduces a Python 2 dependency; that pipeline is outside the default comparison |
| 2 | **GGCAT 2.2.0**, tag resolved to `f40e0b418aba4a0d93a3e480e351d03d595a0571` on the review date | Rust-native compacted-de-Bruijn-graph and maximal-unitig diagnostic; tests graph topology, unitig spelling, GFA export, and resource behavior rather than full assembler repeat resolution | Raw unitig/graph output, GFA v1 output, links when enabled, command, logs, parameters, temporary-storage and resource measurements | MIT at the tagged top level; full dependency/source archive audit remains required [C28-C30] | Not a task-equivalent full assembler and no documented mate-constraint contract. Its default minimum multiplicity and graph/evidence semantics differ from VeritAsm, so configure explicitly and compare graph/unitig invariants separately from assembler contigs |
| 3 | **Velvet 1.2.10** | Historical fixed-k de Bruijn graph and paired-read control | `contigs.fa`, `stats.txt`, `LastGraph`, command and logs | Representative tagged source notices state GPL-2.0-or-later; exact-artifact scan remains controlling [C23-C25], [C35] | Upstream is legacy and requires preselected k. `LastGraph` is a Velvet-specific representation, not GFA. Retain package revision separately from upstream version |
| 3 | **MaSuRCA 4.1.4**, tag resolved to `43e485a2485fd2ec358ceb84a354f81fb9b0de45` on the review date | Optional current general assembler boundary; useful only if its short-read recipe can be made task-equivalent | final assembly FASTA, config, logs, intermediates | GPL-3.0; upstream release/repository [C26] | Linux-heavy workflow and broader hybrid/genome scope. Use the attached release archive rather than GitHub's auto-generated source archives as upstream instructs; freeze its SHA-256. Do not require it on macOS or rank an installation failure below a biological result |

## Capability matrix

Cells describe documented design intent, not independently reproduced capability. “Graph” means an
exported assembly representation; it does not mean that graph records have equivalent semantics.

| Comparator | SE short reads | PE input accepted | Documented mate constraints in reconstruction | Multiple k values | Uneven/high-background design | Error correction | Graph-like output | Primary comparison unit |
|---|---:|---:|---|---:|---:|---:|---:|---|
| VeritAsm 0.3.0-alpha.1 (contract v0.1) | Yes | Yes, strictly synchronized | No reconstruction constraint; exact unique cross-unitig endpoint observations are evidence only | No; one k per bundle | Exact disk count plus an explicit retained-graph cap, not adaptive low-depth rescue | No; immutable reads and quality-window rejection | Deterministic documented GFA 1.0 subset | Conservative graph segments maximal only under the declared degree/fixed-point boundary model, plus evidence bundle |
| Virustic2 | Yes | Yes | No; pairs are counted together without insert-aware traversal | No | No | No; quality-window rejection only | No | Unitig FASTA |
| SPAdes | Yes | Yes | Yes, including graph-path resolution | Yes | metaSPAdes is a separate mode | Yes by default in normal pipelines | GFA/FASTG plus paths | Contigs; scaffolds separate |
| MEGAHIT | Yes | Yes | Unresolved from the cited official documentation; accepting R1/R2 files is not by itself evidence of a mate-constrained path decision [C6], [C36] | Yes, iterative | Yes | Graph cleaning and low-depth rescue, not a standalone corrected-read contract | Intermediate FASTG conversion | Final contig FASTA |
| SKESA | Yes | Yes | Yes; the method paper describes insert estimation and use of paired reads | Iterative implementation details are tool-defined | Primarily isolates | Conservative built-in processing | Separate connector GFA | Contig FASTA |
| IDBA-UD | Yes | Yes | Yes | Yes, iterative carry-forward | Yes, highly uneven depth | Integrated graph processing | Tool-specific intermediates | Contigs; scaffolds separate |
| ABySS | Yes | Yes | Yes, including scaffolding | One selected k per primary run | Bloom-filter options address scale, not low-abundance truth | Pipeline-dependent | DOT/path and some alignment outputs | Contigs; scaffolds separate |
| Minia | Yes | No equivalent dedicated paired-library contract in the standalone control | No equivalent paired scaffolding in the selected standalone control | One selected k per standalone run | Memory-focused | Integrated graph simplification | No common interchange graph | Contig FASTA |
| GGCAT | Yes | Sequence files accepted; mate roles are not part of the cited graph contract | No documented mate-constraint contract | One selected k per build | Disk/memory-focused graph construction | Minimum-multiplicity retention, not an assembler correction contract | GFA v1/v2 or unitig/link output | Graph and maximal unitigs only |
| Velvet | Yes | Yes | Yes | One selected k | No | Integrated graph simplification | `LastGraph` | Contigs; scaffolds separate if used |

## RNA-virus and haplotype boundary matrix

These specialized tools answer different questions and are not pooled with the primary neutral
assembler leaderboard. A consensus, local haplotype, haplotype-specific contig, global strain path,
and abundance estimate are different estimands. The rows identify future diagnostic comparisons,
not completed freezes or endorsements.

| Tool or mode | Intended output/model | Appropriate diagnostic use | License and operational boundary |
|---|---|---|---|
| rnaviralSPAdes (`SPAdes 4.3.0 --rnaviral`) | Assembly of viral RNA-Seq data; not by itself a global quasispecies guarantee | Preregistered segmented and non-segmented RNA-virus cDNA datasets, scored separately from ordinary/metaSPAdes | Same unresolved exact-artifact SPAdes license review as the primary row; mode is documented in the current official manual [C2], [C37] |
| metaviralSPAdes (`SPAdes 4.3.0 --metaviral`) | Extraction of putative extrachromosomal viral contigs from metagenomic assembly graphs | Separate viral-metagenome reconstruction profile; never treated as a neutral parameter replicate | Same SPAdes artifact/license blocker; specialized simplification and verification recommendations change the task [C37], [C38] |
| IVA | Iterative de novo assembly of paired Illumina RNA-virus reads at highly variable depth | Legacy RNA-virus consensus/assembly control, especially amplification-skewed reads without long repeats | GPLv3; upstream states that support is unavailable, and its external-runtime/toolchain requirements make it an operationally legacy comparator [C39], [C40] |
| VICUNA 1.1 (2012) | One consensus representation for ultra-deep, genetically heterogeneous viral populations | Consensus-versus-diversity boundary; target-like reference filtering/guided merging, when used, must be a separate reference-assisted run | The official license is single-user, academic/non-commercial, non-transferable, and prohibits redistribution; do not bundle, mirror, or make it a default automated comparator. Any eligible academic user must acquire it under the official terms and independently freeze the executable [C41], [C42], [C57] |
| ShoRAH | Error-aware local haplotypes/variants in windows; global reconstruction is a separate workflow concern | Local-haplotype precision/recall and a control against calling every graph bubble a phased strain | GPL family in the published implementation; exact current source/dependency freeze is required [C43], [C44] |
| SAVAGE / HaploConduct | Iterative overlap-graph haplotype-specific contigs from deep paired Illumina data | De novo quasispecies/haplotig comparison when truth contains physically phased strains | GPLv3; documented workflow uses Python 2 and assumes very high aggregate coverage, so installation and task equivalence are release blockers [C45], [C46] |
| Haploflow | Coverage-flow decomposition of a unitig graph into strain-resolved paths | Tests model-derived strain paths against VeritAsm's conservative branch preservation | GPLv3; paper version 0.2 has a stable Zenodo snapshot, but remains a separate executable [C47] |
| VG-Flow | Flow-variation-graph full-length haplotypes plus abundance estimates from preassembled contigs | Global-path and abundance-model comparison after preserving the input contig set | MIT code, but solver/runtime licensing and the exact artifact must be frozen independently [C48] |
| viaDBG / ViQUF | Paired-de-Bruijn-graph or unitig-flow reconstruction of quasispecies; ViQUF also estimates frequencies | Paired/flow-derived global paths under truth-known phasing | Public source exists, but no reusable software license was verified for the reviewed repositories; do not redistribute or integrate before resolution [C49-C52] |
| PenguiN | Overlap/Bayesian strain-resolved viral DNA/RNA and 16S assembly in complex metagenomes | Current high-background overlap-assembly boundary | GPL-licensed upstream repository; exact release, dependency closure, and executable digest remain to be frozen [C53], [C54] |

The 2014 and 2020 independent evaluations illustrate why no method is a truth oracle: phasing beyond
read linkage and rare-haplotype recovery were difficult in the former, while the latter reported
large performance variation with viral diversity [C55], [C56]. Any future haplotype scorecard must
separate local from global truth, contigs from full-length strains, and sequence recovery from
abundance estimation.

Every specialized tool is a separately acquired executable, not a Rust dependency. GPL-family code
must not be copied or linked into the MIT package without a dedicated distribution review, and a
license to execute a comparator does not imply permission to redistribute it. VICUNA's official
single-user restriction is stricter still and makes a shared benchmark image or bundled download
inappropriate without separate permission. These are engineering license gates, not legal advice.

## Benchmark profiles

The benchmark manifest must assign each dataset to profiles before truth is inspected.

| Profile | Required runs | Reason |
|---|---|---|
| Isolate-like, moderate and high depth | Virustic2; VeritAsm; ordinary SPAdes; SKESA; MEGAHIT default | Tests consensus, repeats, pair use, and overassembly without granting a metagenomic prior |
| Low-abundance sequence in high background | Virustic2; VeritAsm; metaSPAdes; MEGAHIT default; MEGAHIT `meta-sensitive` | Tests recovery and target-background false joins under a task-relevant background |
| Deliberate uneven-depth synthetic mixture | VeritAsm; SPAdes/metaSPAdes as preregistered; MEGAHIT; IDBA-UD | Tests stable fixed-k output against local/global cleaning and iterative or multi-k behavior |
| Paired repeat-resolution challenge | VeritAsm; ordinary SPAdes; SKESA; ABySS; Velvet control | Tests whether additional contiguity is supported or fabricated |
| Retained-graph memory stress | VeritAsm; MEGAHIT; Minia; GGCAT; ABySS Bloom mode if preregistered | Separates counting/graph memory mechanisms from reconstruction quality |
| RNA-virus cDNA, segmented and non-segmented | Virustic2; VeritAsm; rnaviralSPAdes; IVA where installation succeeds | Tests generic nucleotide assembly against RNA-virus-specific handling; segment assemblies are scored independently and no particle-level segment constellation is inferred |
| Truth-phased two-strain/quasispecies mixture | VeritAsm branch/unitig outputs; one preregistered specialized tool per estimand (for example SAVAGE for haplotigs or ViQUF for global paths) | Measures minor-path and false-path behavior without pooling conservative unitigs, local haplotypes, global strains, and abundance estimates |

Profiles must use the same biological reads. When a comparator cannot accept the canonical input,
the adapter must be deterministic, versioned, tested, and recorded with both input and output
SHA-256 digests. Converted input is a disclosed compatibility transformation, not a new dataset.

## Output comparability rules

1. **Contigs and scaffolds are different estimands.** Score gapless contigs in the primary sequence
   comparison. Score scaffolds in a separate table, retaining every `N` gap and join. Never reward an
   unsupported gap-bearing join as reconstructed bases.
2. **Raw reproducibility and biological equivalence are separate.** Retain byte-for-byte hashes of
   every output. A second normalized-sequence view may canonicalize case, wrapping, order, reverse
   complement, and circular rotation under a versioned algorithm; it must not replace the raw hash.
3. **Headers are not evidence.** Tool-generated coverage, length, or path tokens in FASTA identifiers
   must be parsed only under a documented tool/version schema. Unknown fields remain opaque.
4. **Graphs are not interchangeable merely because they are GFA or FASTG.** Retain the declared
   specification, record types, coordinate conventions, tags, path semantics, and whether the graph
   is before or after scaffolding. Do not compare graph-node counts across unlike stages.
5. **SPAdes mode is part of comparator identity.** Ordinary SPAdes, metaSPAdes, metaviralSPAdes,
   rnaviralSPAdes, coronaSPAdes, wastewaterSPAdes, and plasmidSPAdes are not parameter replicates.
   Ordinary and metaSPAdes belong only to the primary preregistered dataset classes above;
   rnaviralSPAdes and metaviralSPAdes, if run, remain specialized comparisons. Reference-requiring
   modes cannot enter the de novo leaderboard.
6. **MEGAHIT intermediate graphs are not final assemblies.** FASTG generated from intermediate
   graph files is retained for inspection; it is not assumed to encode exactly the final FASTA.
7. **SKESA connector output is derived.** If `GFA_connector` is run, preserve its separate command
   and version and do not describe its GFA as the native assembly graph.
8. **Graph constructors are a separate diagnostic.** Compare VeritAsm conservative boundary-model
   segments and retained topology with GGCAT or another pinned graph constructor under matched `k`, retained-key, strand,
   and multiplicity semantics. Do not pool a graph/unitig score with post-resolution assembler contigs.
9. **Alignment and variant formats are evaluation products.** SAM/BAM, PAF, and VCF generated by an
   evaluator must be attributed to the aligner/caller and must not be presented as assembler-native
   evidence.
10. **Do not select the best parameter after seeing truth.** Vendor defaults and any sensitivity
   profile must be preregistered and reported as separate runs. A parameter sweep is an experiment,
   not a single comparator.
11. **Retain failures.** Install failure, timeout, out-of-memory termination, corrupt output, and
    unsupported input are reportable outcomes with the same visibility as successful scores.

The primary scorecard must report at least target genome fraction, base accuracy, misassemblies,
target-background false junctions, duplication, target and off-target assembled bases, minor-path
precision/recall when truth defines paths, runtime, peak resident memory, and determinism. N50 or
longest contig alone is never an accuracy score.

## Freeze and execution record

Every comparator run must preserve:

1. upstream version/tag and full resolved commit;
2. source-archive SHA-256, package artifact digest, or immutable OCI image digest;
3. all license and notice files from the exact artifact;
4. compiler, linker, build flags, linked-library versions, and enabled CPU instruction features;
5. operating system, architecture, container digest where used, and scheduler/resource limits;
6. executable SHA-256 and a smoke-test transcript;
7. complete command, environment, working-directory policy, thread limit, RAM limit, and temporary
   storage limit;
8. raw input SHA-256 plus a logical-read digest after the shared validated decoder;
9. stdout, stderr, parameter files, corrected reads, graph files, path files, and intermediates
   needed to interpret the final sequence;
10. raw output checksums, evaluator versions/commands, and every failure or exclusion reason.

For MEGAHIT, record whether BMI2/POPCNT acceleration or `--no-hw-accel` was used. For packaged
Velvet and IDBA-UD, record both upstream version and Debian/Bioconda build revision. For ABySS,
retain `abyss-pe versions`. A source tag and a package with the same displayed version are not assumed
to be the same executable. For GGCAT, record output mode, minimum multiplicity, minimizer/hash options,
memory limit, compression, and whether links or GFA were requested.

## Explicit exclusions from the primary leaderboard

| Tool class or example | Decision | Reason |
|---|---|---|
| Shovill | Exclude; optional operational context only | Isolate-oriented wrapper; its default depth cap/downsampling changes the evidence population and it delegates assembly to other engines [C27] |
| SAUTE and target-enriched assembly | Exclude from neutral comparison | Uses target/reference priors and answers a different question |
| IVA, VICUNA, rnaviralSPAdes, Haploflow, SAVAGE, VG-Flow, ShoRAH, and V-pipe | Separate RNA-virus consensus or haplotype experiment | Specialized consensus, strain, or local-haplotype inference is not equivalent to conservative neutral de novo reconstruction; inferred paths may exceed physical phasing evidence, and VICUNA has restrictive single-user terms |
| viralFlye and other long-read assemblers | Exclude | Different sequencing data and error model |
| Reference-guided reconstruction/polishing | Exclude from de novo score; separate optional analysis | Reference choice can supply truth-like sequence and obscure novel or divergent structure |
| plasmidSPAdes/Recycler/SCAPP | Separate circular-element experiment | Specialized topology/classification assumptions; not neutral general assembly |

## Unverified gaps and stop conditions

- SPAdes, MEGAHIT, ABySS, GGCAT, and MaSuRCA tags were resolved to the full commits shown above on
  the review date, but source/binary archive SHA-256 values, executable digests, and smoke tests have
  not been recorded. A remote tag lookup is not a completed freeze; do not run a release benchmark
  until the exact acquired artifacts are retained.
- SKESA's source-reported 2.5.1, official release-page version, package revision, and applicable
  license set have not been reconciled. This is a comparator-freeze blocker.
- No selected comparator has yet been built and smoke-tested on both the benchmark Linux host and
  Apple Silicon macOS. A Linux-only comparator may be retained if declared, but cannot support a
  cross-platform performance conclusion.
- The exact relationship between each tool's final FASTA and graph/path artifacts has not been
  round-trip tested. Graph-level comparisons remain exploratory until it is.
- GGCAT's canonicalization, minimum-multiplicity, graph-link, and maximal-unitig semantics have not
  been normalized against VeritAsm. It remains a diagnostic oracle candidate, not a ground truth.
- No RNA-virus or haplotype-specific candidate in the boundary matrix has completed the executable,
  dependency, license, input-adapter, and smoke-test freeze. rnaviralSPAdes is a distinct mode, not
  evidence that ordinary SPAdes has been tested for the RNA profiles.
- No scoring contract yet links independently assembled segments into one segmented genome, and no
  short-read-only comparator may infer a particle-level segment constellation without an explicit
  linkage model and truth.
- The planned datasets, abundance series, random seeds, accessions, checksums, evaluators, timeout,
  RAM cap, and exclusion rules must be preregistered in `VALIDATION.md` before results are scored.
- No retained head-to-head result currently establishes that VeritAsm is more complete, accurate,
  memory-efficient, faster, or more deterministic than any comparator.

## Sources

- **[C1]** Bankevich A et al. “SPAdes: A New Genome Assembly Algorithm and Its Applications to
  Single-Cell Sequencing.” *Journal of Computational Biology* (2012).
  <https://doi.org/10.1089/cmb.2012.0021>
- **[C2]** SPAdes team. SPAdes documentation, version 4.3.0 at review time.
  <https://ablab.github.io/spades/>
- **[C3]** SPAdes source and releases. <https://github.com/ablab/spades/releases>
- **[C4]** Nurk S et al. “metaSPAdes: a new versatile metagenomic assembler.” *Genome Research*
  (2017). <https://doi.org/10.1101/gr.213959.116>
- **[C5]** Li D et al. “MEGAHIT: an ultra-fast single-node solution for large and complex
  metagenomics assembly via succinct de Bruijn graph.” *Bioinformatics* (2015).
  <https://doi.org/10.1093/bioinformatics/btv033>
- **[C6]** MEGAHIT source. <https://github.com/voutcn/megahit>
- **[C7]** MEGAHIT 1.2.9 release. <https://github.com/voutcn/megahit/releases/tag/v1.2.9>
- **[C8]** Souvorov A, Agarwala R, Lipman DJ. “SKESA: strategic k-mer extension for scrupulous
  assemblies.” *Genome Biology* (2018). <https://doi.org/10.1186/s13059-018-1540-z>
- **[C9]** SKESA source. <https://github.com/ncbi/SKESA>
- **[C10]** SKESA releases. <https://github.com/ncbi/SKESA/releases>
- **[C11]** SKESA Bioconda recipe/package history. <https://bioconda.github.io/recipes/skesa/README.html>
- **[C12]** NCBI SKESA 2.4.0 GFA connector source.
  <https://github.com/ncbi/SKESA/blob/2.4.0/gfa_connector.cpp>
- **[C13]** Peng Y et al. “IDBA-UD: a de novo assembler for single-cell and metagenomic sequencing
  data with highly uneven depth.” *Bioinformatics* (2012).
  <https://doi.org/10.1093/bioinformatics/bts174>
- **[C14]** IDBA source. <https://github.com/loneknightpy/idba>
- **[C15]** IDBA 1.1.3 source release. <https://github.com/loneknightpy/idba/releases/tag/1.1.3>
- **[C16]** Simpson JT et al. “ABySS: a parallel assembler for short read sequence data.” *Genome
  Research* (2009). <https://doi.org/10.1101/gr.089532.108>
- **[C17]** Jackman SD et al. “ABySS 2.0: resource-efficient assembly of large genomes using a Bloom
  filter.” *Genome Research* (2017). <https://doi.org/10.1101/gr.214346.116>
- **[C18]** ABySS source. <https://github.com/BirolLab/abyss>
- **[C19]** ABySS 2.3.10 release. <https://github.com/BirolLab/abyss/releases/tag/2.3.10>
- **[C20]** Chikhi R, Rizk G. “Space-efficient and exact de Bruijn graph representation based on a
  Bloom filter.” *Algorithms for Molecular Biology* (2013).
  <https://doi.org/10.1186/1748-7188-8-22>
- **[C21]** Minia source. <https://github.com/GATB/minia>
- **[C22]** Minia Bioconda recipe/package history.
  <https://bioconda.github.io/recipes/minia/README.html>
- **[C23]** Zerbino DR, Birney E. “Velvet: Algorithms for de novo short read assembly using de
  Bruijn graphs.” *Genome Research* (2008). <https://doi.org/10.1101/gr.074492.107>
- **[C24]** Velvet source. <https://github.com/dzerbino/velvet>
- **[C25]** Velvet 1.2.10 release. <https://github.com/dzerbino/velvet/releases/tag/v1.2.10>
- **[C26]** MaSuRCA source and releases. <https://github.com/alekseyzimin/masurca/releases>
- **[C27]** Shovill source and documented depth cap. <https://github.com/tseemann/shovill>
- **[C28]** Cracco A, Tomescu AI. “Extremely-fast construction and querying of compacted and colored
  de Bruijn graphs with GGCAT.” *Genome Research* (2023).
  <https://doi.org/10.1101/gr.277615.122>
- **[C29]** GGCAT 2.2.0 release. <https://github.com/algbio/ggcat/releases/tag/v2.2.0>
- **[C30]** GGCAT 2.2.0 top-level MIT license.
  <https://github.com/algbio/ggcat/blob/v2.2.0/LICENSE>
- **[C31]** SPAdes 4.3.0 top-level license.
  <https://github.com/ablab/spades/blob/v4.3.0/LICENSE>
- **[C32]** SPAdes upstream issue 1330, “Incompatible licenses.”
  <https://github.com/ablab/spades/issues/1330>
- **[C33]** MEGAHIT 1.2.9 tagged driver source and license notice.
  <https://github.com/voutcn/megahit/blob/v1.2.9/src/megahit>
- **[C34]** ABySS 2.3.10 top-level license and per-file-license direction.
  <https://github.com/BirolLab/abyss/blob/2.3.10/LICENSE>
- **[C35]** Velvet 1.2.10 tagged source license notice.
  <https://github.com/dzerbino/velvet/blob/v1.2.10/src/graphReConstruction.c>
- **[C36]** MEGAHIT upstream issue 154 requesting documentation of paired-end information use.
  <https://github.com/voutcn/megahit/issues/154>
- **[C37]** SPAdes team. “Command line options,” including `--metaviral`, `--rnaviral`, `--corona`,
  and `--sewage`; current official documentation reviewed with the 4.3.0 candidate release.
  <https://ablab.github.io/spades/running.html>
- **[C38]** Antipov D et al. “MetaviralSPAdes: assembly of viruses from metagenomic data.”
  *Bioinformatics* (2020). <https://doi.org/10.1093/bioinformatics/btaa490>
- **[C39]** Hunt M et al. “IVA: accurate de novo assembly of RNA virus genomes.”
  *Bioinformatics* (2015). <https://doi.org/10.1093/bioinformatics/btv120>
- **[C40]** IVA source, usage, support notice, and GPLv3 license.
  <https://github.com/sanger-pathogens/iva>
- **[C41]** Yang X et al. “De novo assembly of highly diverse viral populations.” *BMC Genomics*
  (2012). <https://doi.org/10.1186/1471-2164-13-475>
- **[C42]** Broad Institute. “VICUNA,” current tool description reviewed 2026-09-03.
  <https://www.broadinstitute.org/viral-genomics/vicuna>
- **[C43]** Zagordi O et al. “ShoRAH: estimating the genetic diversity of a mixed sample from
  next-generation sequencing data.” *BMC Bioinformatics* (2011).
  <https://doi.org/10.1186/1471-2105-12-119>
- **[C44]** ShoRAH source and GPL-3.0 license. <https://github.com/cbg-ethz/shorah>
- **[C45]** Baaijens JA et al. “De novo assembly of viral quasispecies using overlap graphs.”
  *Genome Research* (2017). <https://doi.org/10.1101/gr.215038.116>
- **[C46]** HaploConduct/SAVAGE source, installation requirements, and GPL-3.0 license.
  <https://github.com/HaploConduct/HaploConduct>
- **[C47]** Fritz A et al. “Haploflow: strain-resolved de novo assembly of viral genomes.” *Genome
  Biology* (2021); archived paper-version source. <https://doi.org/10.1186/s13059-021-02426-8>
  <https://doi.org/10.5281/zenodo.4106497>
- **[C48]** Baaijens JA et al. “Strain-aware assembly of genomes from mixed samples using flow
  variation graphs.” In *Research in Computational Molecular Biology* (2020); author software page.
  <https://doi.org/10.1007/978-3-030-45257-5_14>
  <https://jbaaijens.github.io/software/>
- **[C49]** Freire B et al. “Inference of viral quasispecies with a paired de Bruijn graph.”
  *Bioinformatics* (2021). <https://doi.org/10.1093/bioinformatics/btaa782>
- **[C50]** viaDBG source. <https://github.com/borjaf696/viaDBG>
- **[C51]** Freire B et al. “ViQUF: de novo viral quasispecies reconstruction using unitig-based
  flow networks.” *IEEE/ACM Transactions on Computational Biology and Bioinformatics* (2023).
  <https://doi.org/10.1109/TCBB.2022.3190282>
- **[C52]** ViQUF source. <https://github.com/borjaf696/ViQUF>
- **[C53]** Jochheim A et al. “Strain-resolved de-novo metagenomic assembly of viral genomes and
  microbial 16S rRNAs.” *Microbiome* (2024).
  <https://doi.org/10.1186/s40168-024-01904-y>
- **[C54]** Plass/PenguiN source and repository license. <https://github.com/soedinglab/plass>
- **[C55]** Schirmer M, Sloan WT, Quince C. “Benchmarking of viral haplotype reconstruction
  programmes.” *Briefings in Bioinformatics* (2014). <https://doi.org/10.1093/bib/bbs081>
- **[C56]** Eliseev A et al. “Evaluation of haplotype callers for next-generation sequencing of
  viruses.” *Infection, Genetics and Evolution* (2020).
  <https://doi.org/10.1016/j.meegid.2020.104277>
- **[C57]** Broad Institute. “Viral Genomics Software License: VICUNA,” including single-user,
  academic/non-commercial, non-transferable, and no-redistribution terms; reviewed 2026-09-03.
  <https://www.broadinstitute.org/viral-genomics/viral-genomics-software-license-vicuna>

License identifiers in this document summarize reviewed upstream metadata; they are not legal advice.
The exact frozen artifact, including bundled third-party code, remains the controlling evidence.
