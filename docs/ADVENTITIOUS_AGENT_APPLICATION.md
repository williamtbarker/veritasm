# Adventitious-agent sequencing application boundary

**Status:** research application note; not a validated assay specification  
**Scope:** short-read de novo assembly and evidence reporting as one component of a larger
high-throughput sequencing workflow

## Decision

VeritAsm remains a neutral, database-free assembler. It may support research, method development,
and investigations involving possible adventitious-agent nucleic-acid signals, but it is not an
adventitious-agent assay and does not emit an organism-presence or sample-absence decision.

This distinction is substantive. De novo assembly can represent several supplied read records as a
longer graph path, expose graph alternatives, and make remote similarity easier to recognize. It can
also fail to assemble real low-abundance reads, join repeats incorrectly if evidence rules are weak,
or reconstruct sequence introduced by reagents, another library, or the laboratory environment. A
contig therefore represents an algorithmic reconstruction from supplied records. It does not by
itself prove an organism, an intact particle, viability, infectivity, a contamination source,
biological relevance, or product risk.

No contig is not evidence that no adventitious agent is present. Unassembled and filtered evidence
must remain accounted for, and a complete workflow needs read-level analysis in addition to assembly.

## Current implementation boundary

This note intentionally describes requirements beyond the stable 0.1 command. The following table
prevents a proposed application behavior from being mistaken for implemented capability.

| Area | Current 0.1 source status (not a release claim) | Status in this application note |
|---|---|---|
| De novo reconstruction | Source implements a one-sample, database-free, single-k path to conservative graph segments, maximal only under the declared degree/fixed-point boundary model, plus GFA and structured evidence | The vertical slice remains subject to all release validation gates; it is not an agent call |
| Construction-read audit | Exact zero-mismatch remapping of eligible reads to emitted linear unitigs; a candidate-limit event makes the applicable aggregates indeterminate/`NA` | Internal consistency only, not independent validation |
| Paired evidence | Source implements unique cross-unitig endpoint observations and mutually exclusive exclusion states; no library-orientation, insert, gap, traversal, or join inference | Evidence primitive only, not scaffolding or a presence signal |
| Closed graph walks | Graph topology and closure can be emitted; construction-read and pair audit fields are `NA` with `closed_walk_audit_unsupported` | No molecular circularity or seam-support claim |
| Controls and sample roles | No test/control manifest or control comparison; `run.json` reports `control_context = "not_supplied"` | Proposed, not implemented |
| Host/reference/taxonomy | No host subtraction, reference similarity, or taxonomic classifier; `run.json` reports `taxonomy = "not_performed"` | Deferred and separate from de novo assembly |
| Probabilistic code | Safe-Rust `BloomFilter` and ordered `TwoHitSieve` research primitives have module-level tests but are outside the stable counter and CLI | Two-hit end-to-end integration remains experimental; syncmer/Count-Min/control triage is design only |
| RNA and segmented genomes | cDNA-derived nucleotide strings can be parsed, but there is no RNA-molecule, strand, amplification, segment, constellation, or reassortment model | Deferred; ordinary unitigs must not be relabelled as phased strains or complete segmented genomes |

## Exact product disclaimer

The following text is the required scientific disclaimer for any documentation or report that
describes this application:

> VeritAsm is unvalidated research software. It reconstructs algorithm-defined unitigs and reports
> internal consistency from the same supplied reads used for construction. It does not detect or
> identify an organism; establish biological presence or absence, viability, infectivity, sample
> sterility, or product safety; determine product disposition; or replace a validated or compendial
> method. Regulatory and standards references describe the surrounding workflow only and do not state
> or imply compliance, approval, clearance, qualification, validation, or fitness for a regulated
> purpose.

The disclaimer does not make the software a formally labelled research-use-only in vitro diagnostic,
and this document does not assert that applying such a classification or label would be appropriate.
If a product falls within FDA's RUO/IUO framework, the formal label is **“For Research Use Only. Not
for use in diagnostic procedures.”** Labelling, intended use, distribution, and promotional conduct
must all agree; a disclaimer cannot cure diagnostic or lot-release claims. Legal and regulatory
classification is outside this software specification.

## Boundary of the complete workflow

ICH Q5A(R2) treats non-targeted next-generation sequencing as a method whose performance depends on
the complete workflow: sample treatment or enrichment, nucleic-acid extraction, library preparation,
sequencing, bioinformatics, and the sequence database used for analysis. It permits an NGS method to
be considered for particular viral-safety uses only when it is suitable for the intended purpose and
appropriately qualified or validated. It also calls for investigation of positive nucleic-acid
results to determine whether they are associated with infectious virus. These properties cannot be
inherited from a standalone executable.

The proposed application boundary is confined to the following logical portion; only items 1, 2,
and 4 exist in the stable 0.1 vertical slice, and item 1 does not yet accept application-level
test/control roles:

1. accept already generated short-read FASTA/FASTQ plus explicit sample metadata;
2. reconstruct and retain graph/contig evidence, aggregate fragment-instance placement evidence, and
   explicit uncertainty states;
3. optionally prioritize computational work using a clearly labelled experimental scan (design
   only; not a stable CLI capability);
4. export deterministic evidence for separate downstream analysis.

The software does not perform or validate sampling, wet-laboratory processing, library preparation,
sequencer operation, taxonomy, infectivity assessment, risk interpretation, or product disposition.
Optional reference similarity or taxonomy must remain a separate, offline analysis layer with a
versioned and checksummed database. Database-free assembly must remain useful on its own.

ICH Q5A(R2) addresses viral safety for specified biotechnology products. In broader scientific usage,
“adventitious agent” can include bacteria, fungi, viruses, mycoplasma, and other unintentionally
introduced agents, as summarized by Morris et al. (scientific source 11 below). Those agents are
governed by different intended-use and compendial frameworks. The ability to reconstruct a microbial
sequence does not establish that the software replaces sterility, mycoplasma, or other compendial
testing. Agents without an assay-visible nucleic-acid target are outside a nucleotide assembler's
observable domain; this is a scope limit, not an exclusion result.

## Required evidence-preservation behavior

An adventitious-agent application profile, if implemented, must add metadata and evidence views. It
must not silently alter the database-free graph or convert assembly into a binary detector.

### Reads and transformations

- Contigs must not be the only retained analyzable result. The result bundle must export or account
  for unassembled reads, singleton reads, rejected mates, and every filtered base or window.
- Every quality, ambiguity, low-complexity, support, tip, bubble, repeat, and graph-pruning rule must
  report how much evidence it affected and why.
- A retain-all or no-deleting-transformation profile must make any deleting cleanup an explicit
  choice. Low abundance is not synonymous with sequencing error, and no low-support path may
  disappear silently.
- Exact read/fragment provenance must preserve input sample, lane, read group, mate role, and record
  identity. Paired mates remain one scheduling and provenance unit.
- Host subtraction, if later supported, must be optional, separate, reversible, and fully reported.
  Its reference and parameters must be checksummed. The report must warn that subtraction can remove
  endogenous viral elements, integrated or proviral sequence, and genuine target sequence homologous
  to the host.
- Large host- or process-background inputs require measured memory limits, an exact disk-backed or
  partitioned path, or a deterministic early failure with a remedy. Streaming reads alone is not a
  bounded-memory claim.

### Per-contig and per-junction evidence

Where the underlying algorithm can measure them, reports must define and distinguish:

- raw supporting read records and supplied fragment instances;
- single and multiple accepted placement-group support under the recorded mapper;
- construction-read placement breadth and a depth distribution rather than mean depth alone;
- base-quality and strand balance;
- for a future separately validated library model, correctly oriented and insert-compatible
  paired-fragment support; stable 0.1 reports neither qualification;
- distinct supplied fragment instances spanning each reported junction;
- unresolved branches, bubbles, repeats, and alternate traversals;
- k values used and stability across k values;
- low-complexity flags;
- contributing lanes and read groups; and
- exact support observed in each test and control input.

“Fragment support” means support by supplied record instances unless molecular identifiers and a
separately validated model establish a stronger unit. A closed graph walk is an algorithmic topology
result, not proof of a circular molecule. Stable 0.1 retains `topology=closed_graph_walk` and its GFA
closure from the exact retained graph, but its construction-read and pair-audit fields are `NA` with
reason `closed_walk_audit_unsupported`; it does not report seam-spanning support. A future circular
remapper would have to enumerate the closing seam separately and disclose conflicting explanations.
Even a seam-spanning record can arise from repeats, ligation, amplification, or a linear concatemer;
molecular circularity requires orthogonal validation outside this assembler.

## Control-aware analysis

Controls provide context for interpretation; they do not create a subtraction oracle. Reagent and
laboratory contaminants disproportionately affect low-biomass data, can occur stochastically, and may
be absent from a particular blank. The same sequence may occur at low support in one input and high
support in a multiplex neighbour. Non-exclusive explanations include shared source sequence,
laboratory or reagent background, index misassignment, and cross-sample carryover; the assembler does
not choose among them. Accordingly, neither “observed in a blank” nor “unobserved in one blank” is a
definitive classification rule.

### Manifest and isolation requirements

The application profile should accept explicit roles including:

- test sample;
- process negative;
- extraction blank;
- library or no-template blank;
- positive control; and
- internal control.

It must then:

1. validate that every input has exactly one declared role and batch/run context;
2. construct each sample and control graph separately unless a distinct coassembly experiment is
   explicitly requested and labelled;
3. compare exact sequence support across the separately processed inputs without using a joint
   graph to erase sample provenance;
4. retain raw and clearly defined normalized values for every sample-control comparison;
5. flag, rather than delete, sequences supported in a negative control;
6. label a sequence shared with a high-abundance multiplex neighbour as
   `shared_with_multiplex_neighbor`; carryover, index misassignment, shared source sequence, and
   laboratory background remain non-exclusive hypotheses rather than software conclusions;
7. record expected positive/internal-control recovery only when an explicit checksummed reference is
   provided; and
8. emit `control_context_unavailable` when relevant controls are missing. A strict research protocol
   may fail the run, but the default must not manufacture a negative conclusion.

Suitable evidence statuses include `assembled_sequence_candidate`, `observed_in_supplied_control`,
`observed_in_multiple_inputs`, `shared_with_multiplex_neighbor`, `insufficient_sequence_evidence`, and
`external_follow_up_needed_if_acted_upon`. These are observational workflow labels, not causal or
biological classifications. Prohibited statuses include `agent_present`, `agent_absent`, `sterile`,
`safe`, and `lot_accepted`.

If an explicitly supplied control reference is not recovered under the recorded software rules, the
assembler can report that unmet software expectation. It cannot determine why recovery failed,
validate or invalidate the laboratory run, or prescribe an acceptance decision.

## Experimental probabilistic control-token triage

A sample-versus-negative-control/background scan using deterministically selected syncmers and Bloom
filters is scientifically acceptable only as an **experimental compute-prioritization layer**. It is
not part of the exact de novo evidence model and is not a detection test.

No such control-token scan is currently wired to the command, spool, exact counter, result schema, or
sample-role model. The repository's local `BloomFilter` and `TwoHitSieve` implement only the narrower
ADR-0004 recurrent-key prototype: they have module-level tests, do not consume controls or syncmers,
and do not establish end-to-end equivalence or benefit. Everything below is therefore a design and
validation boundary for a future scout, not current product behavior.

### Permitted purpose

The scan may rank fragments, graph partitions, or exact follow-up jobs using an approximate priority
score derived from selected-token observations in the sample and controls supplied for that run. It
may change scheduling, cache use, or which work is attempted first under an explicit resource budget.
If a budget prevents all work from completing, the report must identify the unprocessed residual
evidence; it must not describe the processed subset as a complete negative analysis.

### Mandatory algorithmic constraints

- Syncmer selection must be deterministic and record the k-mer size, s-mer size, selection position
  or rule, canonicalization, hash algorithm/version, seed, and ambiguity handling.
- Every Bloom structure must record its bit count, inserted-item count, number of hash probes,
  construction parameters, input/control identities, input checksums, and estimated false-positive
  rate under the stated model.
- Bloom membership and syncmer-derived control-token comparison are triage signals only. They must not
  be serialized as exact support, exact absence, organism evidence, or a calibrated probability.
- A Bloom collision may change rank or computational effort only. It must never cause a read,
  fragment, k-mer, graph edge, unitig, contig, or alternate path to be discarded.
- No graph-cleaning threshold may consume a Bloom-membership result.
- Every sequence, support count, sample-control statistic, or observation in a supplied control
  exposed as final evidence must be recomputed and verified against exact original-sequence or
  exact-count data.
- An exact count-comparison view must expose the sample count, every control count, their denominators,
  the normalization formula, any pseudocount, and the exact candidate sequence or token definition.
  The descriptive `sample_control_exact_count_ratio` must remain separate from the approximate triage
  score.
- Approximate candidates that fail exact verification must remain auditable as triage false positives
  or collisions and must contribute no final evidence.
- Evidence not selected by the probabilistic stage must remain recoverable in an exact residual path.
  A finite-budget run must report what was deferred and why.
- Paired reads must be prioritized and verified as a fragment so triage cannot separate mates or
  inflate evidence.
- Thread count and hash-table iteration order must not alter syncmer selection, priority ties, exact
  results, or the result-bundle digest.
- The scan must be off by default or visibly labelled experimental. Its output schema and parameters
  require an experimental-version field.

Bloom filters conventionally trade space for false-positive membership results. In this application,
a collision can make a selected syncmer that is exactly unobserved in the supplied background feature
set appear to have been observed there and therefore lower its priority. Exact verification after
candidate generation does not recover evidence that was irreversibly discarded beforehand; this is
why the no-discard and exact-residual rules are mandatory. Likewise, syncmer subsampling cannot
establish sequence absence, and syncmer counts are not abundance estimates without a separately
validated model.

### Interpretation constraints

`control_selected_token_unobserved` means only that an exact selected token was not observed in the
supplied control/background inputs under the recorded parameters. It does not mean novel to science,
taxonomically novel, biologically foreign, uncontaminated, or sample-specific outside that run.
`sample_control_exact_count_ratio` is a defined descriptive statistic, not biological enrichment or a
probability of an adventitious agent. Controls have finite sampling depth and cannot prove a sequence
absent from the laboratory background.

The experimental scan must not:

- emit a detection or absence decision;
- remove sequence observed in a supplied control from the de novo graph;
- convert a Bloom false-positive-rate estimate into a biological confidence score;
- infer contamination source, index hopping, viability, or biological relevance; or
- be used to establish an assay limit of detection.

## Validation tiers

Software evidence, sequencing-data evidence, and end-to-end method evidence answer different
questions and must never be pooled into one “validated” claim.

| Tier | Required evaluation | Permitted conclusion | Not established |
|---|---|---|---|
| 1. Software/in silico | Truth-known reads; malformed input; DNA and graph properties; repeats; low-abundance mixtures; exact probabilistic-scan oracle; Bloom-collision adversaries; determinism | Correctness and performance for the tested software conditions | Matrix-specific assay performance |
| 2. Sequencing data/informatics | Predeclared public spike-in, blank, and high-background datasets; separate comparison tools; fixed database versions; retained failures | Performance on the named data and pipeline configuration | Complete-workflow detection limit or laboratory precision |
| 3. End-to-end/matrix | Representative materials from sample treatment through extraction, library preparation, sequencing, controls, bioinformatics, and follow-up; intended-use protocol | Intended-use claims bounded to the validated method and matrices | Transfer to untested matrices, instruments, or workflows |

### Tier 1 acceptance tests for probabilistic triage

- Exact output with triage enabled must equal the exact result obtained without triage whenever the
  same work is allowed to complete.
- Deliberately colliding Bloom inputs must change at most priority or measured compute, never retained
  evidence or final exact results.
- Control tokens absent due to finite control sampling must not yield an absence claim.
- A resource-limited run must enumerate deferred partitions/fragments and use a distinct incomplete
  status.
- One- and multi-thread runs must have byte-identical scientific output.
- Tests must cover a real low-abundance sequence whose selected tokens are all Bloom false positives;
  the exact residual path must still retain and assemble it when full processing completes.

### General validation design

- Pre-register datasets, random seeds, parameters, metrics, exclusion rules, and pass/fail criteria.
- Vary target read fraction, absolute read count, genome length, GC, repeats, topology, sequencing
  errors, read length, insert distribution, close relatives, and host/vector homology.
- Evaluate several host/background levels with replicates.
- Measure both read-level candidate recovery and contig-level recovery. Report cases in which true
  reads are present but do not assemble.
- Include process/extraction/library blanks and other libraries from the same multiplex context.
- Report genome fraction, base accuracy, false junctions, duplication, exact candidate precision and
  recall, wall time, peak RSS, and deterministic bundle identity.
- Publish failures, regressions, timeouts, unsupported data, and comparator installation failures.
- Do not infer a universal detection limit from simulations or one spiked matrix. Any empirical
  sensitivity statement must name the material, matrix, workflow, read count, replicates, decision
  rule, software version, and parameters.

An end-to-end analytical method is normally expected to address intended-purpose specificity,
detection limit, repeatability/intermediate precision, and robustness. Tier 1 and Tier 2 evidence do
not substitute for this work.

## Prohibited and unsupported claims

Without direct, retained, scope-matched evidence, documentation, CLI text, reports, presentations,
and publications must not claim that VeritAsm:

- detects all, any, unknown, or viable adventitious agents;
- is “extremely accurate,” regulatory-grade, FDA compliant, validated, qualified, production-ready,
  or suitable for lot release;
- replaces in vivo, in vitro, PCR, sterility, mycoplasma, or other compendial tests;
- establishes a universal detection limit, sensitivity, specificity, negative predictive value, or
  false-negative rate;
- establishes absence because no contig, read hit, or control-unobserved selected token was reported;
- proves organism identity, intact particles, viability, infectivity, contamination source,
  biological relevance, or product risk;
- diagnoses disease, supports patient management, or determines product disposition;
- quantitates biological abundance from reads, k-mers, syncmers, graph support, or contigs without a
  calibrated model;
- proves index hopping, reagent contamination, or cross-sample carryover;
- guarantees that host/background subtraction cannot remove target evidence;
- reconstructs a complete circular genome from graph topology alone;
- reconstructs global strains or haplotypes merely by preserving graph bubbles; or
- treats probabilistic membership, a triage score, or a control-token comparison as biological
  evidence.

Performance language must be conditional and reproducible. For example: “On dataset X, at target
fraction Y and read count Z, version V with parameters P recovered N% of the truth sequence with M
false junctions.” A result on one dataset does not imply performance on another matrix or workflow.

## Regulatory and standards source ledger

The sources below establish the surrounding workflow and the limits of a software-only contribution.
They do not state or imply that VeritAsm is compliant, approved, cleared, qualified, validated, or fit
for a regulated purpose. No regulator, standards body, pharmacopeia, or industry organization listed
below has evaluated or endorsed this software.

1. **FDA, ICH Q5A(R2), _Viral Safety Evaluation of Biotechnology Products Derived from Cell Lines
   of Human or Animal Origin_, final guidance, January 2024.** Establishes intended-purpose
   suitability, complete-workflow considerations, qualification versus validation, reference
   materials, matrix verification, and follow-up of positive nucleic-acid findings.  
   <https://www.fda.gov/regulatory-information/search-fda-guidance-documents/q5ar2-viral-safety-evaluation-biotechnology-products-derived-cell-lines-human-or-animal-origin>  
   <https://www.fda.gov/media/163115/download>

2. **EMA, ICH Q5A(R2), effective 14 June 2024.** European publication of the harmonized viral-safety
   guideline.  
   <https://www.ema.europa.eu/en/ich-q5ar2-guideline-viral-safety-evaluation-biotechnology-products-derived-cell-lines-human-or-animal-origin-scientific-guideline>

3. **FDA, ICH Q2(R2), _Validation of Analytical Procedures_, March 2024.** Supports
   intended-purpose validation and evaluation of specificity, detection limit, precision, and
   robustness.  
   <https://www.fda.gov/regulatory-information/search-fda-guidance-documents/q2r2-validation-analytical-procedures>  
   <https://www.fda.gov/media/161201/download>

4. **European Pharmacopoeia General Chapter 2.6.41, _High-throughput sequencing for the detection
   of viral extraneous agents_.** EDQM describes coverage of the complete workflow, routine controls,
   validation, and product-specific validation. Published in Issue 12.2 and effective 1 April 2026.  
   <https://www.edqm.eu/en/-/epc-adopts-cutting-edge-hts-chapter-to-enhance-viral-contaminant-detection-in-biological-products>

5. **WHO adventitious-virus reference-panel work.** The 2020 paper was a proposal, while the March
   2024 ECBS material and the committee report in WHO Technical Report Series 1059 document the later
   panel work. The October 2025 WHO catalogue lists the established adventitious-virus HTS reference
   reagents. These sources provide reference-material precedent; they are not VeritAsm validation.  
   <https://www.who.int/publications/m/item/WHOBS2020-2394>  
   <https://www.who.int/groups/expert-committee-on-biological-standardization/ecbs-2024---documents>  
   <https://iris.who.int/handle/10665/378317>  
   <https://cdn.who.int/media/docs/default-source/biologicals/blood-products/catalogue/who-catalogue-oct-2025.pdf?sfvrsn=2653bd65_7>

6. **NIST, _Standards for Metagenomics_.** Identifies biases contributed by sample collection,
   extraction, library preparation, sequencing, and bioinformatics.  
   <https://www.nist.gov/programs-projects/standards-metagenomics>

7. **NIST-FDA workshop report, _Standards for Next Generation Sequencing Detection of Viral
   Adventitious Agents_.** Distinguishes materials that challenge portions of a workflow from
   whole-virus materials that evaluate more of the complete process.  
   <https://www.nist.gov/publications/report-2019-nist-fda-workshop-standards-next-generation-sequencing-detection-viral>  
   <https://doi.org/10.1016/j.biologicals.2020.02.003>

8. **USP-NF chapters `<1050>`, `<63>`, and `<77>`.** Supply viral-safety context and separate
   compendial frameworks for mycoplasma testing; they do not confer replacement status on an
   assembler.  
   <https://doi.org/10.31003/USPNF_M99774_01_01>  
   <https://doi.org/10.31003/USPNF_M3687_01_01>  
   <https://doi.org/10.31003/USPNF_M17296_02_01>

9. **PDA Technical Report 71 and Advanced Virus Detection Technologies Working Group.** Useful
   industry consensus and study context, but not regulation.  
   <https://www.pda.org/bookstore/product-detail/2831-tr-71-virus-detection>  
   <https://www.pda.org/home/science-regulation/Advanced-Virus-Detection-Technologies>

10. **FDA, _Distribution of In Vitro Diagnostic Products Labeled for Research Use Only or
    Investigational Use Only_.** Supports alignment among the label, intended use, distribution, and
    promotional conduct.  
    <https://www.fda.gov/regulatory-information/search-fda-guidance-documents/distribution-in-vitro-diagnostic-products-labeled-research-use-only-or-investigational-use-only>  
    <https://www.fda.gov/media/87374/download>

## Scientific and algorithmic source ledger

1. Lambert C, et al. _Considerations for Optimization of High-Throughput Sequencing Bioinformatics
   Pipelines for Virus Detection._ **Viruses.** 2018;10(10):528. Assembly can improve divergent
   homology detection, but depends on abundance, coverage, genome properties, library/platform, and
   assembler; the paper also addresses host filtering, database contamination, evidence review, and
   follow-up.  
   <https://doi.org/10.3390/v10100528>  
   <https://pmc.ncbi.nlm.nih.gov/articles/PMC6213042/>

2. Khan AS, et al. _A Multicenter Study to Evaluate the Performance of High-Throughput Sequencing
   for Virus Detection._ **mSphere.** 2017. Reported comparable recovery of the study's model
   viruses across three independently implemented laboratory/bioinformatics workflows under the
   specified conditions. It does not establish interchangeability for other matrices, agents,
   concentrations, or workflows.  
   <https://doi.org/10.1128/mSphere.00307-17>  
   <https://pmc.ncbi.nlm.nih.gov/articles/PMC5597969/>

3. Chin P-J, et al. _Evaluation of high-throughput sequencing for replacing the conventional
   adventitious virus detection assays used for biologics._ **npj Vaccines.** 11, 28 (2026),
   published 23 December 2025; version of record 27 January 2026. The spiked CHO-K1 harvest study
   reported inter-assay and intra-laboratory sensitivity differences, supporting matrix- and
   workflow-specific claims rather than universal sensitivity.  
   <https://doi.org/10.1038/s41541-025-01351-2>  
   <https://pmc.ncbi.nlm.nih.gov/articles/PMC12847995/>

4. Salter SJ, et al. _Reagent and laboratory contamination can critically impact sequence-based
   microbiome analyses._ **BMC Biology.** 2014;12:87. Supports concurrent controls and reagent-lot
   provenance, especially for low-biomass inputs.  
   <https://doi.org/10.1186/s12915-014-0087-z>  
   <https://pmc.ncbi.nlm.nih.gov/articles/PMC4228153/>

5. Asplund M, et al. _Contaminating viral sequences in high-throughput sequencing viromics: a link
   between sequence and reagent._ **Clinical Microbiology and Infection.** 2019. Shows widespread
   laboratory-component associations and stochastic signals that can be absent from an individual
   no-template control.  
   <https://doi.org/10.1016/j.cmi.2019.04.028>

6. Costello M, et al. _Characterization and remediation of sample index swaps by non-redundant dual
   indexing on massively parallel sequencing platforms._ **BMC Genomics.** 2018;19:332. Supports
   multiplex-neighbour context while showing why the bioinformatics result alone cannot prove the
   mechanism of sample leakage.  
   <https://doi.org/10.1186/s12864-018-4703-0>  
   <https://pmc.ncbi.nlm.nih.gov/articles/PMC5941783/>

7. Goodacre N, et al. _A Reference Viral Database (RVDB) To Enhance Bioinformatics Analysis of
   High-Throughput Sequencing for Novel Virus Detection._ **mSphere.** 2018. Supports curated,
   versioned reference data and separation of similarity analysis from de novo reconstruction.  
   <https://doi.org/10.1128/mSphereDirect.00069-18>

8. Porter AF, et al. _Metagenomic Identification of Viral Sequences in Laboratory Reagents._
   **Viruses.** 2021;13(11):2122. Documents viral sequences in blank reagent libraries and uses
   read remapping/coverage views to investigate assembled candidates; public blank-library data are
   available under BioProject `PRJNA735051`. The requirement for orthogonal follow-up is grounded in
   the complete-workflow sources above and Lambert et al., not inferred from assembly alone.  
   <https://doi.org/10.3390/v13112122>  
   <https://pmc.ncbi.nlm.nih.gov/articles/PMC8625350/>

9. Bloom BH. _Space/Time Trade-offs in Hash Coding with Allowable Errors._ **Communications of the
   ACM.** 1970;13(7):422-426. Defines the probabilistic membership structure and its false-positive
   trade-off.  
   <https://doi.org/10.1145/362686.362692>

10. Edgar RC. _Syncmers are more sensitive than minimizers for selecting conserved k-mers in
    biological sequences._ **PeerJ.** 2021;9:e10805. Provides the primary basis for deterministic
    syncmer subsampling; it does not make selected tokens a biological detection test.  
    <https://doi.org/10.7717/peerj.10805>

11. Morris C, Lee YS, Yoon S. _Adventitious agent detection methods in bio-pharmaceutical
    applications with a focus on viruses, bacteria, and mycoplasma._ **Current Opinion in
    Biotechnology.** 2021;71:105-114. Defines the broader scientific scope as including bacteria,
    fungi, viruses, mycoplasma, and other unintentionally introduced agents; the exact intended-use
    framework still controls.  
    <https://doi.org/10.1016/j.copbio.2021.06.027>

## Final recommendation

Implement adventitious-agent support, if pursued, as a control-aware evidence view over the neutral
assembler. Keep the probabilistic scan experimental and subordinate to exact reconstruction. Its
only acceptable benefit is reduced or better-prioritized computation with transparent accounting;
it cannot erase evidence or support presence/absence. Product success in this application means
more faithful sequence reconstruction, clearer uncertainty, or safer handling of realistic controls
and background—not an unvalidated biological conclusion.
