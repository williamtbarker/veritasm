# VeritAsm scientific risk register

- Review date: 2026-09-04
- Applies to: v0.1 design, implementation, validation, reporting, and review packaging
- Evidence basis: [`../RESEARCH.md`](../RESEARCH.md)
- Product boundary: neutral reconstruction of Illumina-like short-read DNA

## Purpose

Short-read data can be insufficient to distinguish sequencing error, real variation, repeats,
multiple similar genomes, plasmids, organellar sequence, and other sample components. VeritAsm must
return that uncertainty rather than hide it behind a single plausible sequence.

Severity below describes consequence if a risk occurs; it is not an estimated probability. A control
is not evidence that the risk is eliminated. Completion requires the named tests and retained
results.

VeritAsm is research software. It does not identify an organism, establish biological presence or
absence, certify a process or manufacturing lot, or make a clinical or regulatory decision.

## Interpretation contract

| Statement | Minimum evidence |
|---|---|
| “exact count” | Equivalence to the exact reference counter; no overflow, dropped partition, approximate membership, or unreported recovery |
| “exact retained count with two-hit sieve” | Eligible ADR-0004 configuration, complete ordered first pass and exact recount, full-key equality, and retained-stream equivalence to no-sieve counting |
| “complete pre-threshold histogram” | No-sieve exact counting or a separately validated exact method; Bloom candidates and estimates are insufficient |
| “bounded counter memory” | Measured peak RSS under fixed counter-memory settings and adversarial partition skew |
| “bounded whole-run memory” | Separate proven bounds for parsing, counting, retained graph, compaction, reconstruction, and reporting |
| “fragment support” | Declared record-instance definition and deduplication across both synchronized mates |
| “read-backed” | Mapping policy, accepted/ambiguous/conflicting counts, quality filters, and evidence locations |
| “paired-supported join” | Valid mate identity/role, unique anchors, orientation, insert compatibility, support from distinct supplied fragment instances, and contradictions; this does not prove independent laboratory molecules |
| “resolved repeat” | Exactly one continuation compatible with the declared evidence; otherwise the branch remains unresolved |
| “candidate circular topology” | Closed path plus terminal/junction evidence and reported alternative explanations |
| “complete circular molecule” | A separately validated finishing standard; graph closure alone is insufficient |
| “improved accuracy/performance” | Preregistered task-equivalent comparison with retained failures and uncertainty |
| “low-abundance recovery” | Truth-known abundance series with recall, precision, false paths, and resource conditions |
| “scout priority or enrichment” | Experimental scheduling telemetry only; never novelty, abundance, confidence, presence, absence, or final evidence |
| “biological identity or presence” | Outside the de novo assembler's scope; contig support alone is insufficient |

## Selected v0.1 resource model

Exact disk-backed counting and retained graph storage are deliberately separate:

1. the no-sieve path assigns every accepted canonical observation to deterministic exact-count
   partitions and remains the reference and fallback;
2. an eligible ADR-0004 two-hit sieve may eventually route a recurrent-key superset through a complete
   second spool pass to those exact partitions, but that optional path is not integrated into stable
   v0.1;
3. every retained full key and its final support must be exact under the chosen fragment-support
   definition, irrespective of the eligible counting path;
4. a complete exact pre-threshold distinct-key histogram is available only from the no-sieve path or
   another separately validated exact method; a sieve path cannot replace missing values with Bloom
   estimates;
5. the retention rule is explicit and is never raised automatically to fit memory;
6. retained k-mers enter an exact in-memory graph;
7. the retained graph has a deterministic hard cap;
8. exceeding disk, counter, or graph limits fails the run before any result bundle is committed.

This design may bound counter memory. It does not justify an end-to-end bounded-memory claim until
every phase has measured limits.

## Scientific and engineering risk register

| ID | Severity | Risk and mechanism | Evidence that would expose it | Required v0.1 control | Residual risk |
|---|---|---|---|---|---|
| SR-01 | High | **Sequencing error is preserved as sequence diversity.** Recurrent or high-quality errors form tips and bubbles similar to real variants. | False paths rise as quality falls or lack fragment/strand/position consistency. | Record quality, ambiguity, occurrence, and fragment evidence; validate variable-quality and systematic-error datasets. | Some platform errors are recurrent and confidently scored. |
| SR-02 | High | **Real low-abundance sequence is removed as error.** Absolute support thresholds confuse abundance with correctness. | Truth-derived minor paths disappear near the configured threshold. | Count exactly before filtering; expose every exact histogram available from the selected count path, use reason-coded `NA` rather than approximate replacement, record all retention parameters, and test abundance and divergence series. | Below the information limit, error and true rare sequence may be indistinguishable. |
| SR-03 | High | **A repeat is traversed incorrectly.** More than one graph continuation fits the observed short sequence. | A junction lacks unique spanning evidence or conflicts with an equally compatible path. | Stop contigs at unresolved branches; retain alternatives in GFA; never force a genome-length answer. | Repeats longer than available linkage can remain fundamentally unresolved. |
| SR-04 | High | **A pair constraint fabricates a join.** Chimeric pairs, role errors, multimapping, or a bad insert model mimic linkage. | Orientation/insert residuals are inconsistent or the join fails truth-based comparison. | Stable 0.1 validates pairs and reports exact endpoint co-observations only; it infers no insert model and creates no join. | Library artifacts can still make an endpoint observation misleading. |
| SR-05 | High | **A model-inferred path is presented as observed.** Coverage, flow, or multi-k reconciliation connects unlinked branches. | No read/pair spans a decision, or several global paths explain the same local evidence. | Type and label every reconstruction class; v0.1 does not perform coverage-flow deconvolution. | Short reads may never identify one global arrangement. |
| SR-06 | High | **Consensus hides a mixture.** The higher-support branch replaces a credible alternative. | An alternate path is absent from output despite retained graph support. | Stable 0.1 has no consensus profile or bubble collapse; every retained branch remains graph structure. | A future consensus would be a summary rather than a complete sample model and requires a new ADR. |
| SR-07 | High | **A graph cycle is falsely called a complete circular molecule.** Repeats or linear structures create the same topology. | No junction-spanning evidence, abnormal junction support, low-complexity overlap, or competing paths. | Stable 0.1 says only `closed_graph_walk`; circular-junction auditing and biological circularity fields are explicitly deferred. | Short reads may not distinguish a circle from repeated or concatemeric DNA. |
| SR-08 | High | **A circular candidate is assigned the wrong biological class.** Topology or coverage is treated as plasmid, organelle, chromosome, or virus identity. | The sequence shares repeats across replicons or lacks independent classification evidence. | No biological class assignment in the database-free v0.1 assembler. | Downstream users may still overinterpret exported contigs. |
| SR-09 | High | **Similar components collapse.** Related genomes or constructs behave as long approximate repeats. | Consensus sequence combines truth from multiple components; component recall falls with decreasing divergence. | Preserve ambiguity; evaluate mixtures across divergence and abundance; prohibit strain/organism claims. | Some mixtures are non-identifiable with the available reads. |
| SR-10 | High | **Graph cleaning is not auditable.** Plausible sequence disappears without a reproducible reason. | Reports cannot account for removed nodes, edges, support, or transformation order. | Version every transformation; record parameters and before/after statistics; retain alternative-path summaries. | Aggregate provenance may not retain every rejected read unless designed to do so. |
| SR-11 | High | **Read-back auditing becomes circular evidence.** Construction reads also score their own assembly. | High internal likelihood coexists with a truth-inconsistent repeat join. | Label read-back as internal consistency; report evidence components; include independent truth where available. | Without external truth, a coherent wrong assembly may remain plausible. |
| SR-12 | High | **A contiguity metric rewards a wrong result.** N50 or total length improves while base or junction accuracy regresses. | Metric rankings disagree or longer contigs contain more misassemblies/duplication. | Predeclare and retain genome fraction, base error, false junction, duplication, minor-path precision/recall, runtime, RSS, and disk metrics. | Metric importance remains application-dependent. |
| SR-13 | Medium | **One k value biases reconstruction.** Small k collapses repeats; large k fragments sparse regions. | Results change materially across neighboring k values. | Construct declared k values independently and expose per-k evidence. | No k creates missing linkage or resolves repeats longer than the data support. |
| SR-14 | High | **Multi-k reconciliation invents support.** A candidate from one graph is treated as an independent observation in another. | Output support increases when derived contigs are added without new reads. | Build every v0.1 graph from original reads; derived candidates never increment fragment support. | Reconciliation rules can still prefer the wrong candidate and require truth testing. |
| SR-15 | Medium | **Quality values are miscalibrated.** Nominal Phred scores do not match actual run-specific error. | Empirical error by Q score is inconsistent; quality-weighted and unweighted results diverge. | Preserve threshold behavior; defer probabilistic weighting/correction; validate by quality stratum. | Systematic errors may not be reflected in per-base qualities. |
| SR-16 | Medium | **Ambiguity handling hides gaps or invents bases.** IUPAC observations break paths, while imputation creates unsupported sequence. | Contig boundaries follow ambiguous windows or output lacks rejection provenance. | No ambiguity imputation; report affected windows and bases with input context. | Sequence across an ambiguity remains unreconstructed. |
| SR-17 | High | **Disk-backed counts are not exact.** Observations are lost, duplicated, mispartitioned, or combined under the wrong support mode. | Disk and in-memory counters differ by key or count; paired overlap is double-counted in fragment mode. | Property/golden tests for one-bucket assignment, partition completeness, canonicalization, fragment deduplication, and exact equivalence. | Undetected filesystem or implementation faults remain possible without end-to-end manifests. |
| SR-18 | High | **Counter overflow or saturation is silent.** High depth changes a count without an error. | Counts plateau at an integer maximum or differ by ingestion order. | Checked wide counters, explicit overflow errors, and boundary tests; never saturating arithmetic without telemetry. | Extremely deep inputs may require a documented maximum. |
| SR-19 | High | **Disk exhaustion leaves a partial count that looks reusable.** A later run consumes incomplete partitions or output. | Missing partitions, mismatched manifests, or surviving partial artifacts after failure. | Run-unique temporary directory, expected-partition manifest, checksums, completion marker, fsync/error handling, and atomic finalization/cleanup. | Multi-filesystem crash semantics require documented limits and recovery tests. |
| SR-20 | Medium | **Partition skew defeats the memory bound.** One minimizer/signature bucket becomes much larger than expected. | Peak RSS exceeds the configured counter limit on low-complexity/adversarial input. | Bound subpartition size, recursively repartition or external-sort, and stress homopolymers/skewed signatures. | More partitions and passes can cause severe I/O amplification. |
| SR-21 | High | **The retained graph exceeds its hard cap.** Exact counting succeeds, but too many k-mers pass the explicit rule. | Graph construction reaches the declared state limit. | Fail deterministically before output commit; report counts, limit, and explicit remedies; never raise support silently. | High-background singleton retention may remain infeasible in v0.1. |
| SR-22 | Medium | **Temporary count data expose sequence or consume uncontrolled storage.** Disk partitions retain sample-derived k-mers after completion/failure. | Temporary files persist or use an unexpected filesystem. | User-visible temporary path, restrictive permissions, size estimate, cleanup policy, and failure tests. | Secure deletion is filesystem-dependent and cannot be promised generally. |
| SR-23 | Medium | **Parallelism changes count or graph choices.** Partition scheduling, hash order, or ties reach serialized output. | Count databases or complete bundles differ across threads/repeated runs. | Stable partition/hash policy, deterministic reductions/ties, canonical sorting, and byte-identical bundle tests. | Cross-platform dependencies may need additional normalization. |
| SR-24 | High | **Malformed or desynchronized paired input changes support.** Reordered, missing, duplicated, role-swapped, or aliased mates are accepted. | Known-invalid pairs assemble rather than fail. | Validate normalized identity, mate role, format, order, cardinality, lane correspondence, and physical file identity. | Header conventions vary and require versioned accepted rules. |
| SR-25 | High | **A failed run replaces valid prior results.** Counts, FASTA, GFA, evidence, or HTML commit independently. | Existing output changes after any later artifact fails. | Validate and atomically commit one immutable result bundle with a shared run ID. | Filesystem guarantees differ; achieved semantics must be tested and documented. |
| SR-26 | High | **Validation is tuned or cherry-picked.** Datasets, k values, thresholds, or competitor settings are selected after results are observed. | Only favorable samples are retained or exclusion rules change post hoc. | Freeze manifests, seeds, checksums, commands, versions, metrics, and exclusions before scoring; publish failures/timeouts. | A finite benchmark cannot represent every library or genome. |
| SR-27 | High | **High-background application language becomes a detection claim.** Failure to reconstruct sequence is interpreted as agent absence or process acceptability. | Reports use detected/not-detected, pass/fail, sensitivity, or lot-release language without a validated workflow. | Use reconstruction/evidence language only; prominent research-use limitation in CLI, reports, README, and citation metadata. | Users may apply results outside the stated scope. |
| SR-28 | High | **Reference assistance silently changes de novo results.** A database biases retention, polishing, or identity while provenance omits it. | Results change with database version or network state. | Database-free core; any later reference stage is separate, optional, offline, versioned, checksummed, and labelled. | Every finite reference database omits some biological diversity. |
| SR-29 | High | **The two-hit sieve drops a threshold-eligible key.** Concurrent query/insert races, inconsistent mate deduplication, an incomplete pass, corruption, or mismatched hash/configuration state invalidates the no-false-negative proof. | A retained full-key stream or downstream core artifact differs from the no-sieve oracle. | Enforce ADR-0004 eligibility; ordered stateful updates; immutable spool identity; insertion/integrity checks; complete second spool pass and exact candidate recount; exhaustive, collision, failure, and cross-thread equivalence tests; automatic no-sieve fallback only before scientific output. | Design acceptance is not implementation evidence; a single missing eligible key rejects the optimization. |
| SR-30 | High | **Scout triage becomes evidence or starves residual work.** Approximate syncmer/Bloom/Count-Min signals are interpreted as novelty or prevent low-priority fragments from reaching exact processing. | Scout-on changes a core artifact, a fragment remains unprocessed, or an incomplete run is presented as successful/negative. | Default off; quarantine telemetry; schedule mates together; guarantee eventual exact residual service; require scout-on/off core byte equivalence; label interrupted residual work incomplete. | Priority can still be biased by controls, collisions, low complexity, adapters, and systematic errors. |
| SR-31 | Medium | **Sieve telemetry masquerades as a complete count distribution.** Distinct singleton counts or below-threshold ledgers are inferred from Bloom candidates. | A stable exact field changes between sieve and no-sieve paths or contains an approximate value without an approximation label. | Disable the sieve when stable schemas require those exact fields unless an independent exact method supplies them; otherwise use reason-coded `NA` only in a schema that permits it. | Disabling the sieve can forfeit any workload-specific I/O benefit. |

## Exact disk-counter and optional-sieve invariants

The disk-backed counter is not complete until tests establish all of the following:

1. on the no-sieve path every accepted canonical occurrence maps to exactly one logical partition;
   on an eligible two-hit path every routed candidate occurrence maps to exactly one logical
   partition;
2. partition order and thread scheduling do not change final keys or counts;
3. occurrence mode counts every accepted window exactly once;
4. fragment mode counts a canonical k-mer at most once per supplied single read or synchronized pair,
   including overlapping mates;
5. multi-lane semantics match ordered baseline ingestion;
6. quality and ambiguity rejections match the in-memory reference scanner;
7. no approximate structure changes retained membership or integer counts;
8. counter overflow returns a typed error before output commit;
9. all expected partitions, schema versions, parameters, and input identities are recorded in a
   completion manifest;
10. resumed or reused temporary state is rejected unless a future, separately tested resume protocol
    authenticates every artifact;
11. final retained full-key/count streams are byte-deterministic after canonical ordering; a complete
    pre-threshold histogram is required only when produced by the no-sieve path or another exact
    method, and is otherwise unavailable rather than estimated;
12. disk exhaustion, permission failure, truncation, corruption, and abrupt worker failure leave no
    valid-looking completed database;
13. an eligible two-hit run matches the no-sieve retained stream and every shared core scientific
    artifact exactly; an ineligible configuration bypasses the sieve;
14. an optional scout changes scheduling only, eventually processes every fragment exactly, and
    cannot contribute to a scientific decision or exact field.

## Accepted probabilistic-acceleration decision

> **STATUS: ADR-0004 ACCEPTED AS A CONSTRAINED DESIGN. THE IN-MEMORY BLOOM FILTER AND
> TWO-HIT COORDINATOR PRIMITIVES AND THEIR UNIT/ORACLE TEST SOURCES EXIST; A HISTORICAL CANDIDATE
> RECORD INCLUDES THOSE TESTS, BUT THE FINAL CLEAN ARCHIVE RERUN IS PENDING. THEY ARE NOT INTEGRATED
> INTO THE STABLE COUNTING PIPELINE OR VALIDATED END TO END.**

ADR-0004 authorizes two mechanisms with different boundaries:

1. **Two-hit sieve:** conditionally authorized only when
   `support_unit=supplied_fragment_instance && min_support>=2`, with no sub-two rescue rule. Canonical keys are deduplicated across
   both mates; stateful insertion-only Bloom updates occur in fragment order; a complete second pass
   over the same immutable spool sends the recurrent-key superset to the exact full-key counter.
   False positives may add work but cannot become retained evidence. Support one, occurrence mode,
   incompatible retention, invalid state, or unmet proof premises use the no-sieve path.
2. **Scout:** default-off and experimental. Versioned syncmer, background-Bloom, and Count-Min values
   may schedule whole fragments only. Every fragment must remain in the exact residual path, mates
   remain together, and an incomplete residual pass cannot commit a normal result or support an
   absence claim.

Only the fixed-size `BloomFilter` and ordered `TwoHitSieve` research primitives described for the
first mechanism exist in source. Their checked-in unit, oracle, collision, and property tests were
included in the historical 2026-09-03 candidate-tree test record, but that record predates the current
tree; the final clean archive has not rerun them. The completed-spool passes, exact-counter routing,
stable CLI and schema integration, sieve-on/off complete-bundle equivalence run, failure matrix, and
retained resource-benefit benchmark do not exist. The scout remains design-only.

Neither mechanism authorizes probabilistic graph membership, approximate final counts, evidence-based
filtering, or biological novelty/detection language. The two-hit sieve cannot by itself supply exact
distinct-singleton cardinality, a complete pre-threshold histogram, or a complete below-threshold
removal ledger. The scout cannot change FASTA, GFA, retained counts, transformations, or read/pair
evidence. Stable-pipeline integration, end-to-end exactness, or benefit claims remain prohibited
until the ADR's oracle, failure, determinism, schema, and preregistered workload gates have retained
results. The implemented-primitive claim is limited to isolated source plus the identified historical
test record until the final archive is rerun.

## Stable no-sieve release-blocking scientific conditions

A stable v0.1 package uses the exact no-sieve path. It must not claim completion if:

- disk and in-memory exact counters disagree;
- fragment support is inflated by overlapping mates or repeated occurrences in one record pair;
- any counter or support value can saturate silently;
- the counter-memory configuration or graph cap is bypassed nondeterministically;
- graph-cap failure changes the user's existing result;
- a filter or graph transformation lacks complete parameter and before/after provenance;
- a pair-supported join lacks role, uniqueness, orientation, insert, and conflict evidence;
- a model-derived path is presented as observed linkage;
- a graph cycle is labelled a complete circular molecule or plasmid;
- a contig is labelled as an organism, agent, strain, or contamination event;
- thread-count bundle determinism is not tested;
- truth-known failures, regressions, timeouts, or competitor installation failures are omitted;
- any “better,” accuracy, low-abundance, bounded-memory, or performance claim lacks direct retained
  evidence.

The absence of promotion studies for a disabled, unreachable sieve or unimplemented scout does not
by itself block a no-sieve source-review alpha. Stable packaging must, however, demonstrate that those
optional mechanisms are absent from or unreachable through the stable CLI and cannot alter stable
artifacts.

## Optional sieve/scout promotion blockers

These conditions block only enabling or promoting the named optional mechanism. They do not convert
an otherwise conforming, no-sieve v0.1 source-review package into a failure:

- an enabled two-hit sieve changes a retained key/count stream or shared core artifact relative to
  the no-sieve oracle, runs outside its eligibility boundary, or exposes an unavailable
  below-threshold statistic as exact;
- the complete spool/recount path, integrity/failure matrix, thread equivalence, and stable-schema
  treatment have not passed against the exact no-sieve oracle;
- the current final archive has not rerun the primitive unit/oracle/collision tests;
- no preregistered workload shows a retained resource benefit; or
- scout-on changes a core scientific artifact, fails to service every residual fragment, presents
  approximate priority as evidence, or lacks proof that interrupted residual work cannot appear
  complete.

## Minimum stable evidence per output unitig and bundle

The stable 0.1 unitig row exposes exactly the content-derived ID, sequence checksum, length, k,
`linear|closed_graph_walk` topology label, oriented edge-step count, distinct backing canonical-key
count, support unit, retention threshold, minimum/lower-median/maximum distinct-key support, three
construction-read placement aggregates, and the placement-enumeration status or required `NA`.

The bundle additionally exposes exact input/source/spool digests and counts; the four-way quality and
ambiguity window partition; complete exact support histogram; retained-graph counts; the two-stage
transformation journal; complete per-role read-state and applicable pair-state partitions; exact
canonical-coordinate pair endpoint observations; GFA segments and retained-graph links; software,
schema, and effective parameters; the fixed limitations and disclaimer; and artifact checksums.
Thread count, timings, paths, host data, and a synthetic run ID are deliberately excluded from stable
bytes.

Graph-component IDs, support means/distributions, pair insert inference or conflicts, multi-k
reconciliation, circular-junction evidence, and low-complexity annotations are not stable 0.1 fields.
They remain possible future features requiring an ADR, schema, and validation. Unresolved alternatives
remain GFA topology; 0.1 does not claim to enumerate or interpret every biological branch or bubble.
The HTML report may summarize machine-readable evidence but must not omit contrary evidence or weaken
uncertainty labels.

## Evidence map

Complete citations and qualifications are in [`../RESEARCH.md`](../RESEARCH.md). Stable primary links
for the highest-consequence risks are:

| Risk group | Evidence |
|---|---|
| Exact disk counting | [DSK](https://doi.org/10.1093/bioinformatics/btt020), [KMC2](https://doi.org/10.1093/bioinformatics/btv022), and [KMC3](https://doi.org/10.1093/bioinformatics/btx304) |
| Exact graph compaction | [BCALM2](https://doi.org/10.1093/bioinformatics/btw279), [Bifrost](https://doi.org/10.1186/s13059-020-02135-8), and [Cuttlefish 2](https://doi.org/10.1186/s13059-022-02743-6) |
| Probabilistic acceleration and graph risk | [Bloom](https://doi.org/10.1145/362686.362692), [Count-Min sketch](https://doi.org/10.1016/j.jalgor.2003.12.001), [syncmers](https://doi.org/10.7717/peerj.10805), and [Pell et al.](https://doi.org/10.1073/pnas.1121464109) |
| k-size and uneven-depth tradeoff | [IDBA-UD](https://doi.org/10.1093/bioinformatics/bts174), [SPAdes](https://doi.org/10.1089/cmb.2012.0021), and [MEGAHIT](https://doi.org/10.1093/bioinformatics/btv033) |
| Error versus low abundance | [Velvet](https://doi.org/10.1101/gr.074492.107), [Quake](https://doi.org/10.1186/gb-2010-11-11-r116), and [CAMI II](https://doi.org/10.1038/s41592-022-01431-4) |
| Pair/repeat constraints | [Paired de Bruijn Graphs](https://doi.org/10.1089/cmb.2011.0151), [exSPAnder](https://doi.org/10.1093/bioinformatics/btu266), and [BESST](https://doi.org/10.1186/1471-2105-15-281) |
| Circular/plasmid ambiguity | [Recycler](https://doi.org/10.1093/bioinformatics/btw651), [SCAPP](https://doi.org/10.1186/s40168-021-01068-z), and the [independent plasmid benchmark](https://doi.org/10.1099/mgen.0.000128) |
| Validation limits | [GAGE-B](https://doi.org/10.1093/bioinformatics/btt273), [ALE](https://doi.org/10.1093/bioinformatics/bts723), [QUAST](https://doi.org/10.1093/bioinformatics/btt086), and [Merqury](https://doi.org/10.1186/s13059-020-02134-9) |

## Residual uncertainty statement

Even after every control passes, multiple genomes or mixtures can explain the same short reads.
VeritAsm must return unresolved structure when evidence is insufficient. A transparent incomplete
assembly is scientifically preferable to a more contiguous sequence created by an unsupported
choice.
