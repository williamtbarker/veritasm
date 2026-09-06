# Production-redesign evidence ledger

- Review date: 2026-09-04
- Status: design input; not product evidence
- Scope: Illumina-like single-end and paired-end de novo assembly
- Intended use: research only; not organism detection, absence testing, sterility assurance, or a
  clinical or regulatory decision system

## Question and decision standard

The review asks which algorithmic foundation can plausibly improve VeritAsm's accuracy, detail,
runtime, and scaling without weakening exact evidence, deterministic output, malformed-input
behavior, or transactional output. A paper or another tool can justify an experiment. It cannot
establish that VeritAsm implements the method correctly or benefits from it. Those claims require
retained VeritAsm tests and benchmarks.

Evidence is interpreted in this order:

1. mathematical or executable counterexamples to the present implementation;
2. primary algorithm papers and official source/documentation snapshots;
3. independent, truth-known comparisons;
4. author benchmarks, identified as such;
5. engineering probes, which may locate bottlenecks but do not establish biological quality.

## Reproduced facts about the starting point

| Observation | Direct evidence | Consequence |
|---|---|---|
| The imported `0.2.0-alpha.1` source compiles on its declared Rust 1.85 MSRV and current stable. | Independent clean targets passed formatting, check, warnings-denied Clippy, 268 all-target tests, rustdoc with warnings denied, release build, and `cargo package`. | The prior reported compiler failure was not reproduced from the recovered archive. Every new review archive must nevertheless repeat clean-extraction verification. |
| The default construction-read audit is asymptotically unsuitable for realistic assemblies. | Code inspection shows an exhaustive comparison of every eligible read, both orientations, against every possible interval of every linear unitig. A 1 Mb/80,000-fragment engineering probe did not finish promptly and was stopped; a no-remap run completed. | Promote an exact index only after differential equivalence and truthful mapper provenance. Do not describe the stopped probe as a stable timing result. |
| A literal rarest-q-gram prototype can reduce exact verification work without changing exact placements. | The in-repository deterministic probe checked equality to brute force on its fixture and reported roughly 93-fold fewer query-time seconds before index-build amortization; its existing property tests cover exact equality. | This is engineering evidence for integration, not mapping sensitivity or assembly superiority. Persistent index memory must enter phase admission. |
| The current evaluator cannot retain all junction rows for a 1 Mb linear unitig. | Evaluation failed explicitly because 999,920 rows exceeded the fixed 250,000-row cap. | Separate exact aggregate scoring from optional row retention and preserve explicit truncation/missingness. Never silently omit rows. |
| Virustic2 and the imported alpha can emit a reverse-complement fold through an even-k self-reverse-complement edge. | With one read `TCGCGAC`, `k=6`, and support one, each emits `GTCGCGAC`: three oriented steps backed by two canonical keys. | Treat self-reverse-complement edge incidence as a compaction boundary or otherwise prevent unreported reuse of one canonical edge in an emitted linear path. Add an exhaustive invariant and regression before performance work. |

These observations concern the exact commands and fixtures retained for this review. They are not
claims about unrelated workloads, platforms, or releases.

## Construction and graph foundation

### Selected direction

The long-term construction data plane should be an exact minimizer/super-k-mer partitioned direct
compacted de Bruijn graph builder, with width-specialized exact packed keys and collision resolution
against the complete key. GGCAT and BCALM2 provide the strongest direct precedents for partitioned
local compaction, while Cuttlefish provides a strong finite-state direct-construction precedent.

- GGCAT is implemented in Rust and MIT-licensed. Its paper reports large construction speedups, but
  those are author benchmarks of graph construction rather than end-to-end assembly results. Its
  large-k fingerprint behavior must not be copied without full-key collision resolution.
- BCALM2 independently supports minimizer partitioning and local compaction, under an MIT license.
- Cuttlefish 2 and 3 support direct compacted-graph construction; current Cuttlefish 3 documentation
  is relevant design evidence, not VeritAsm performance evidence.
- BOSS-style succinct graphs remain attractive as frozen query representations. They are less
  convenient as the first mutable cleaning and evidence engine.

The imported exact counter, graph, compactor, and exhaustive mapper remain small-data oracles while
the data plane is replaced. Approximate membership must not enter final graph identity.

Primary sources:

- GGCAT: <https://doi.org/10.1101/gr.277615.122> and
  <https://github.com/algbio/GGCAT>
- BCALM2: <https://doi.org/10.1093/bioinformatics/btw279>
- Cuttlefish 2: <https://doi.org/10.1186/s13059-022-02743-6>
- Cuttlefish 3 preprint: <https://doi.org/10.1101/2025.02.02.636161>
- BOSS: <https://doi.org/10.1007/978-3-642-34109-0_18>

## Multiple k values and uneven coverage

Fixed k cannot simultaneously maximize low-depth connectivity and repeat resolution. IDBA-UD,
SPAdes, and MEGAHIT establish iterative or multi-sized approaches; MEGAHIT's mercy k-mers and
IDBA-UD's local depth reasoning are particularly relevant to uneven inputs. Variable-order graph
research and DVOUG show that order can be selected locally, but do not establish that a variable-order
mutable core is the fastest or safest end-to-end choice here.

The first multi-k milestone will therefore construct independent exact child layers from one
authenticated read store and report concordance/conflict. It will not merge child paths until an ADR
defines projection, identity, conflict, provenance, and a no-unsupported-adjacency oracle. Later
iterative correction and local-order traversal are separate experiments.

Primary sources:

- IDBA-UD: <https://doi.org/10.1093/bioinformatics/bts174>
- SPAdes: <https://doi.org/10.1089/cmb.2012.0021>
- MEGAHIT: <https://doi.org/10.1093/bioinformatics/btv033>
- Variable-order de Bruijn graphs: <https://arxiv.org/abs/1411.2718>
- DVOUG: <https://doi.org/10.1016/j.crmeth.2025.101243>
- Variable-order de Bruijn graph/HiFi contig model (not paired-end evidence):
  <https://doi.org/10.4230/LIPIcs.WABI.2026.4>

## Error correction and low-abundance evidence

Quake, BFC, BayesHammer, and Lighter establish distinct quality-aware, search-based, clustering, and
probabilistic correction families. No method can distinguish a true singleton from an error when the
read and context contain no discriminating information.

A defensible experiment is a solid exact backbone plus a separate exact low-count quarantine. Query
the quarantine only in bounded neighborhoods such as graph ends, pair-bounded gaps, or preserved
bubbles, then require complete-read verification and multiple named evidence channels. This combines
known ideas and is not claimed as an invention. Raw reads and every correction must remain auditable.

Primary sources:

- Quake: <https://doi.org/10.1186/gb-2010-11-11-r116>
- BFC: <https://doi.org/10.1093/bioinformatics/btv290>
- BayesHammer: <https://doi.org/10.1186/1471-2164-14-S1-S7>
- Lighter: <https://doi.org/10.1186/s13059-014-0509-9>

## Safe sequence, bubbles, and uncertainty

Maximal unitigs are not universally safe strings. Incomplete coverage and a bidirected
representation can produce unsafe or fragmented output. Omnitigs give a formal safe-string result
under a stated graph model. Recent information-preserving and variable-order work further reinforces
that one forced linear answer can destroy legitimate alternatives.

SAMA is especially close prior art for uncertainty-aware assembly: it estimates a probability of
misassembly while accounting for missing graph data and emits sequences under a modelled correctness
criterion. Its presence rules out broad novelty claims about missing-edge-aware or
uncertainty-certified assembly. Any VeritAsm probability would need model calibration and held-out
validation; an interpretable evidence ledger and abstention are the nearer-term choice.

Primary sources:

- Unsafe unitigs and bidirected artifacts: <https://doi.org/10.1101/gr.276601.122>
- Omnitigs: <https://doi.org/10.1089/cmb.2016.0141>
- SAMA: <https://doi.org/10.1186/s13015-025-00280-y>
- Supregraph preprint: <https://arxiv.org/abs/2604.21951>

## Paired-end constraints

Paired de Bruijn graphs, exSPAnder, and SKESA demonstrate that paired data can constrain repeat
resolution when reads anchor uniquely and the library geometry is usable. They also show why raw link
counts are insufficient.

The production evidence flow must be two-pass and lane-specific:

1. map once and retain bounded, immutable mapping evidence with lane, fragment, mate, target, and
   ambiguity;
2. infer each lane's orientation and empirical span distribution only from eligible same-target
   pairs;
3. replay evidence against finalized lane models;
4. resolve only an existing graph continuation that is uniquely compatible, sufficiently supported,
   and not materially contradicted;
5. otherwise stop the linear sequence and preserve alternatives in GFA.

Pooling raw links across lanes is prohibited. Identical repeats longer than every informative
fragment span remain unresolvable. A graph bubble is not a globally phased haplotype.

Primary sources:

- Paired de Bruijn graphs: <https://doi.org/10.1089/cmb.2011.0151>
- exSPAnder: <https://doi.org/10.1093/bioinformatics/btu266>
- SKESA: <https://doi.org/10.1186/s13059-018-1540-z>

## Probabilistic structures

A custom Bloom filter is justified only when a measured workload and exactness proof show a benefit.
Permitted uses are candidate nomination, scheduling, and approximate spectrum telemetry. Every
retained key must be exact-recounted against the authenticated read store. A Bloom hit is never
identity, abundance, confidence, presence, or absence evidence. The existing two-hit prototype stays
off the stable path until an end-to-end equivalence and resource gate passes.

Primary sources:

- Bloom filters: <https://doi.org/10.1145/362686.362692>
- Probabilistic de Bruijn graphs: <https://doi.org/10.1073/pnas.1121464109>
- ntCard spectrum estimation: <https://doi.org/10.1093/bioinformatics/btw832>

## Build-versus-buy constraints

| Candidate | Suitable role | Constraint |
|---|---|---|
| GGCAT library | Construction experiment and oracle | Global singleton/thread-pool behavior, advisory memory limit, temporary-directory side effects, dependency size, and large-k collision semantics require an adapter and dedicated audit before embedding. |
| `flate2` Rust backend | Stable gzip decoding | Already used; preserve content sniffing, concatenated members, corruption checks, and no external runtime. |
| `needletail` or `seq_io` | Parser comparison | Reuse only if every present strict FASTX, pairing, limit, and error-context test remains equivalent. |
| `noodles` | Optional SAM/BAM/VCF I/O | Useful later; should not burden the database-free de novo core before those outputs exist. |
| SPAdes/MEGAHIT/SKESA code | Algorithm reference and comparator | GPL/AGPL or mixed licensing makes direct reuse incompatible with a permissive clean-room core unless project licensing changes. Reimplement only published ideas with provenance. |

## Frozen near-term decisions

1. Correct conservative compaction before optimizing it.
2. Promote the already differential-tested literal index as an exact zero-mismatch audit accelerator,
   with the exhaustive implementation retained as an oracle.
3. Carry a typed mapper descriptor from construction through `run.json`; hardcoded provenance is a
   release blocker.
4. Make raw pair evidence lane-specific now. A lane model and topology change require separate
   promotion evidence.
5. Separate exact evaluator aggregates from optional detailed-row retention so large truth cases can
   be scored without silent omission.
6. Do not integrate GGCAT, correction, multi-k merging, or a pair-driven join in the same change as
   these correctness repairs.

## Claim and gap matrix

| Proposed statement | Present evidence | Status / missing evidence |
|---|---|---|
| “The recovered alpha compiles on Linux with MSRV and stable.” | Two independent clean target directories and complete command transcripts. | Supported only for the recovered source and current Linux environment; repeat after edits and on clean extraction. |
| “The stable indexed mapper returns the same exact placements as exhaustive scanning within its stated target universe.” | Exhaustive/randomized differential tests, independent brute-force oracle, pipeline integration tests, limit tests, schemas, and serialized mapper provenance. | Supported as a bounded software-semantic claim; not an approximate-mapping, speed, memory, or biological-sensitivity claim. |
| “The indexed pipeline is faster.” | Standalone engineering probe shows less query work. | Needs end-to-end wall time, peak RSS, index-build amortization, and repetitive controls. |
| “Pair evidence is lane aware.” | Stable artifact schema plus cross-lane non-pooling and reconciliation tests. | Supported as a serialization/accounting claim; lane provenance is not library independence or repeat resolution. |
| “VeritAsm is more accurate than Virustic2.” | A reproduced even-k counterexample exists in both current tools. | Requires a corrected VeritAsm result plus frozen broader truth comparisons and no regressions. |
| “VeritAsm is faster or more sensitive than established assemblers.” | None. | Requires preregistered, task-equivalent SPAdes/SKESA/MEGAHIT/GGCAT comparisons with quality qualification and confidence intervals. |
| “VeritAsm detects adventitious agents.” | None; de novo reconstruction is not detection validation. | Prohibited without a separately qualified end-to-end laboratory and informatics workflow, denominators, controls, LOD, false-positive analysis, and intended-use review. |
| “Production grade.” | Defensive input/storage tests and a Linux compiler gate exist. | Blocked by incomplete scientific validation, missing Apple Silicon execution, limited reconstruction, and no independent qualification. |

## Recommendation

Build a verified `0.3` research alpha around a conservative exact fixed-k layer, scalable exact
read-back audit, lane-isolated pair evidence, and truthful transactional reporting. Preserve the old
implementations as executable oracles. In parallel, prototype the replacement minimizer-partitioned
direct cDBG data plane behind an experimental boundary. Only promote multi-k reconciliation,
correction, pair-conditioned topology changes, and low-abundance rescue one at a time after their
truth-known kill tests pass.

No available evidence supports “fastest,” “most sensitive,” “general-purpose production assembler,”
or adventitious-agent detection claims. The credible route is to publish exact supported scope,
failure modes, preregistered comparisons, and every regression alongside any measured improvement.
