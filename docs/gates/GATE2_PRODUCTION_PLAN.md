# Gate 2: bounded production-development plan

- Status: **reformulated after adversarial audit; in progress**
- Original plan accepted: 2026-09-03
- Reformulation date: 2026-09-04
- Audited implementation baseline: Git commit
  `e37daabab554480e94c934e1357ccc55e5b316e9`
- Release-candidate commit and archive SHA-256: **unbound until a clean verifier passes**

The 2026-09-04 audit found a defensively engineered fixed-k unitig alpha, not a production-grade
general assembler. Input validation, exact counting, graph compaction, transactional output, and
evidence reporting are substantial. Multi-k reconstruction, pair-constrained traversal, positional
coverage, diversity profiles, disk-resident graph operation, molecular circularity evidence, public
data, executed comparator results, sustained sanitizer campaigns, and executed macOS evidence remain
absent or experimental. All seven fuzz targets and seed corpora exist, but sanitizer execution is
`NOT RUN` for 0.3.0-alpha.1. A bounded smoke exists only for historical 0.2.0-alpha.1 source commit
`cdb88007f2643776b092e8379b200dfb1404ac0c`. This plan makes those distinctions release-blocking
rather than aspirational.

## Product boundary

The intended product is a supported, evidence-preserving de novo assembler for Illumina-like short
DNA or cDNA reads, within a measured operating envelope, on Linux and Apple Silicon macOS. The
initial qualification domain is single-end and ordinary paired-end data from isolates, plasmids,
organelles, synthetic constructs, and bounded low-to-moderate-complexity mixtures.

The plan does not authorize claims covering arbitrary sequencing data, long reads, linked reads,
mate-pair libraries, whole-eukaryotic-genome assembly, global strain or haplotype reconstruction,
taxonomy, organism detection, analytical sensitivity, absence, sterility, or regulatory use.
Low-abundance reconstruction in complex backgrounds is an application-validation stratum, not an
organism-detection claim.

## Release ladder

| Level | Required evidence | Permitted description |
|---|---|---|
| Source-review candidate | Exact fixed-k unitig slice and its bounded verification record | Research prototype |
| Scientific alpha | Executable truth system plus experimental multi-scale, mapping, coverage, cleaning, and pair constraints | Experimental assembler |
| Evaluation beta | Frozen synthetic, external, semi-synthetic, and public matrices plus modern comparators | Evaluation-ready for the measured domain |
| Supported 1.0 | Scientific gates, operating envelope, platform tests, fault/fuzz evidence, stable interfaces, provenance, and independent reproduction | Supported research assembler within the published envelope |
| Intended-use qualification | A separately frozen end-to-end laboratory and informatics workflow | Only that named workflow may make its validated intended-use claims |

Advancing one row never implies the row below it.

The audited code targets the **source-review candidate** row. The integrated working-tree checkpoint
passed its dual-toolchain Linux source gates, but the commit-bound archive and independently extracted
crates have not yet passed the full verifier. A future Linux verifier pass would not promote it to scientific alpha; it would
establish only that the named fixed-k source archive compiles and behaves consistently under the
verifier's bounded tests.

## Dependency-ordered execution

1. **Seal the fixed-k alpha.** Repair local and clean-extraction verification, add missing inherited
   input regressions, reconcile every release claim with retained evidence, build the source ZIP
   twice, and require one authoritative dual-toolchain Linux verifier `PASS`. Keep macOS,
   LeakSanitizer/sustained-fuzz, public-data, and scientific-comparison rows explicitly `NOT RUN`
   until executed; retain the historical 0.2.0-alpha.1 bounded ASan/libFuzzer smoke separately as
   prior evidence, never as qualification of the current tree.
2. **Characterize the promoted exact audit before changing reconstruction.** The fixed-q15 indexed
   mapper is now stable and byte-equivalent to the retained exhaustive oracle across exhaustive,
   randomized, strand, palindrome, short-read, closed-walk, limit, and thread-count tests. Measure
   representative end-to-end time/RSS and repetitive low-memory failures; design a bounded fallback
   before claiming broad compatibility. Define positional depth, strand, end, and pair-support
   denominators; retain mapping ambiguity instead of converting it to support.
3. **Evaluate multi-k as independent evidence.** Consume one authenticated spool with a frozen k list
   and construct deterministic child graphs independently. First report cross-k support and conflicts;
   do not merge paths until an ADR defines identity, containment, conflict, and provenance rules and
   an oracle establishes that no unsupported adjacency is introduced.
4. **Admit pair constraints conservatively.** Qualify the insert model on uniquely placed,
   orientation-consistent fragments. A pair may resolve a traversal only when all alternatives and
   conflicts are reported and the chosen join is replayable. Otherwise leave the graph unresolved.
   Unitigs, contigs, and scaffolds remain distinct products.
5. **Expose diversity without claiming phasing.** Catalogue bubbles before offering separate
   diversity-preserving and consensus-summary profiles. Preserve qualifying alternatives, report
   unphaseable combinations, and prohibit global haplotype language without molecule-spanning
   evidence.
6. **Bound contaminated and high-depth operation.** Partition the retained exact graph or fail at a
   documented limit; measure parser, spool, counting, graph, mapper, and reporting RSS/I/O separately.
   A Bloom structure may reduce candidate work only when exact recount proves output equivalence.
7. **Require closure evidence for circular labels.** Distinguish graph cycles from an assembled
   sequence with read- or fragment-backed closing-adjacency support. Even closing-adjacency support is
   not molecular proof of a circular molecule; orthogonal validation is required for that claim.
   Repeat, linear terminal-repeat, and concatemer confounders belong in the promotion matrix.
8. **Qualify, then describe.** Freeze seeds, public accessions and checksums, comparator versions,
   commands, metrics, resources, exclusions, and stopping rules before scoring. Execute Linux and
   Apple Silicon, sanitizer/fault, synthetic, contaminated-background, mixture, repeat, circular,
   low-input, high-depth, and public-data matrices; retain every failure and regression.
9. **Package only evidence-matched milestones.** The full release verifier rejects a renamed source
   ZIP, and its checksum sidecar records the canonical basename. Deliver the canonical ZIP and canonical
   sidecar inside a unique commit-bearing handoff directory; a uniquely named outer review bundle may
   wrap that directory. Do not push or publish. `Supported 1.0` remains unavailable until its entire
   release-ladder row passes, regardless of code size or feature count.

## Audit-to-work ledger

| Capability | 2026-09-04 state | Next falsifiable gate |
|---|---|---|
| FASTX, gzip sniffing, lanes, strict pairs, IUPAC/QC | Implemented; Linux tests expanded | Stable and Rust 1.85 full suites, clean archive, then Apple Silicon |
| Exact fixed-k count and compact graph | Implemented for `k=3..63`; graph resident in memory | Exhaustive conservation plus resource-envelope qualification |
| Deterministic atomic evidence bundle | Implemented; bounded fixture evidence | Repeated 1/2/4/8-thread and spill-boundary matrix on both platforms |
| Construction-read audit | Fixed-q15 indexed exact mapper stable; exhaustive scanner retained as its oracle | Representative time/RSS and repetitive low-memory characterization, then a bounded fallback decision |
| Multi-k reconstruction | Not implemented | Independent child graphs and cross-k conflict ledger before merging |
| Pair-driven repeat resolution | Not implemented; pair observations annotate only | Truth-known resolved-join precision with zero unsupported joins |
| Positional coverage and diversity profiles | Not implemented | Versioned denominators, goldens, and mixture/bubble truth tests |
| Circular sequence assertion | Graph-walk topology only | Seam-spanning read/fragment evidence and confounder controls |
| Large contaminated data | Count spilling only; retained graph in memory | Partitioned exact graph or explicit measured failure envelope |
| Haplotype, segmentation, taxonomy, variants | Proposed or out of stable scope | Separate ADR and task-equivalent validation; no implied promotion |
| General accuracy/performance superiority | Not demonstrated | Frozen modern-comparator matrix with uncertainty and all regressions |

## Absolute release blockers

- Any exact-counter, graph, compaction, evidence-reconciliation, or serialization oracle mismatch.
- Any successful contig containing an adjacency that cannot be replayed from the retained graph and
  its declared read, larger-k, or pair constraints.
- Any silent incomplete result, partial committed bundle, destination replacement, unexpected panic,
  sanitizer finding, or unresolved data-corruption defect.
- Nondeterministic scientific artifacts across declared thread counts or repeated executions.
- A probabilistic value used as final sequence identity, graph membership, biological identity,
  detection, or absence evidence.

## Scientific promotion rules

Thresholds, datasets, commands, seeds, evaluators, resources, exclusions, and stopping rules are
frozen before qualification truth is viewed. Development, blinded qualification, and characterization
populations remain separate.

- Perfect read-observable single-source cases require complete observable recovery, zero base error,
  and zero false junctions.
- Repeat resolution is judged first by resolved-join precision; leaving an unsupported repeat
  unresolved is acceptable.
- No target-background junction is acceptable when none exists in truth.
- A new feature may not increase false junctions in a core cell. Per-cell genome-fraction regression
  over 0.5 percentage points or consensus-QV regression over 3 points blocks promotion unless the
  frozen profile explicitly makes and documents that tradeoff.
- An improvement claim requires a predeclared primary endpoint whose paired 95% interval excludes
  zero, noninferiority on correctness endpoints, and publication of every unfavorable result in the
  same family.
- No scalar composite score or post-hoc best-k result is a release endpoint.

## Probabilistic acceleration boundary

The existing two-hit Bloom prototype remains outside stable assembly. It may nominate an exact-recount
superset but may never decide final graph membership. Stable admission requires retained full keys and
counts identical to the no-sieve oracle under ordinary and collision-saturated tests, fail-closed
integrity behavior, honest unavailable fields, and a preregistered resource benefit. Without that
evidence it remains experimental or is removed.

## Baseline execution record

Earlier working-tree runs passed formatting, compilation, warnings-denied Clippy, tests,
documentation, release build, and Cargo packaging on Linux, but every retained pre-audit full
release-verifier run ended `Overall: FAIL`. Those failures remain evidence. The reformulated plan
therefore requires a new verifier transcript bound to the exact post-audit archive before describing
even the source-review candidate as verified. A Linux pass will not substitute for Apple Silicon,
dynamic network isolation, public-data validation, or the scientific matrix. A bounded
10-second-per-target ASan/libFuzzer smoke passed only for historical 0.2.0-alpha.1 source commit
`cdb88007f2643776b092e8379b200dfb1404ac0c`; current-candidate sanitizer execution is `NOT RUN`.
That prior smoke does not substitute for current, sustained, platform-complete sanitizer campaigns.

All development occurs in isolated local Git worktrees. No agent is authorized to push, publish a
crate, create a remote release, or weaken a failed gate into a claim.
