# Scientific limitations

This document is normative. User-facing reports and publications must not weaken these statements.

## What a unitig establishes

A unitig establishes that, under the recorded parser, quality rule, ambiguity rule, k, support mode,
support threshold, graph transformations, and compaction-boundary model, a conservative sequence
segment exists in the retained de Bruijn graph. It is maximal only between the declared degree and
fixed-point boundaries; self-complemental nodes and incident self-complemental k-mer edges can stop an
otherwise non-branching walk. Its evidence comes from the supplied read records. That evidence is not
independent of the data used to build the graph.

A unitig does **not** by itself establish:

- the identity or presence of an organism;
- a complete genome, segment, plasmid, or chromosome;
- molecular circularity;
- viability, infectivity, source, novelty, or biological risk;
- a phased strain or haplotype;
- sample sterility or absence when no unitig is produced.

## Observation and sampling limits

Input records are observations, not necessarily independent molecules. PCR duplicates, optical
duplicates, copied files, overlapping mates, index misassignment, reagent contamination, and
systematic sequencer errors can make record support overstate independent evidence. VeritAsm reports
fragment-instance or occurrence support and does not infer molecular counts without external UMIs and
a separately validated model.

Failure to reconstruct sequence may result from no source sequence, insufficient sampling, low base
quality, ambiguity, an unsuitable k, filtering, repeats, mixture complexity, an exhausted resource
limit, or implementation failure. It is never converted into a biological absence claim.

## Graph ambiguity

Different genomes, repeats, shared sequence, sequencing errors, and real variants can produce the same
local graph structure. A branch is not automatically an error or a biological variant. A bubble is not
a globally phased haplotype. VeritAsm 0.1 therefore preserves unresolved alternatives and reports
local topology. Its only deleting rule is the declared absolute pre-graph support threshold; removed
key counts and support mass remain in the transformation journal.

## Pair evidence

Read pairs can constrain adjacency and orientation only after a validated library model and explicit
compatibility rules. Version 0.1 deliberately has neither: it reports canonical-coordinate endpoint
co-observations and exclusions isolated by supplied input lane, makes no
adjacency/orientation/insert/gap conclusion, and does not join unitigs. Lane isolation prevents silent
pooling across library preparations; it does not establish that lanes are mutually independent. Its
uniquely placed subset is mapping-selected. Repeats, chimeric molecules, read-through, and incorrect
mate metadata can still make an endpoint observation misleading.

An isolated experimental module can summarize per-lane orientation and mapped outer-envelope spans
from caller-asserted exact unique same-linear-unitig placements. Its `available` state means only
that explicit integer count, dominance, and central-width gates passed. It is not used by the stable
pipeline, is not a validated library model, does not infer physical insert length or gaps, and cannot
authorize a graph traversal or sequence join. See ADR 0008.

## Construction-read placement scope

Version 0.1 remaps against emitted **linear unitig sequences only**, not the full GFA path space.
Consequently, a read spanning a graph link or a closed-walk seam is reported unmapped or unavailable
under this target universe even when its k-mers helped construct the graph. Conversely, an eligible
read shorter than `k` contributes no graph k-mer but may exactly match an emitted linear unitig. A
placement therefore describes target-limited sequence compatibility; it is not proof that the read
caused the unitig, that the placement is globally unique, or that an unmapped read lacks graph support.
`max_mapping_candidates` bounds accepted placement groups, not failed comparisons or mapper runtime.
The fixed-q literal index verifies complete zero-mismatch candidates, falls back to interval scanning
for reads shorter than q=15, and stores every target q-gram. It is not an approximate aligner and has
no demonstrated end-to-end speed or peak-memory advantage on representative datasets. There is no
runtime low-memory fallback for a target index that exceeds the audit-persistent one-eighth memory
share: remap-enabled assembly fails with `resource_memory`, while an explicitly remap-disabled run
can proceed without constructing the index. This makes index admission a known operational
compatibility risk until a partitioned exact index or a separately specified fallback is promoted.

## Circularity

A closed graph walk is a topology candidate, not proof of a circular molecule. Even a read spanning a
candidate closing junction may arise from repeats, ligation, amplification, or a linear concatemer.
Any future circularity field must identify the exact supporting placements and retain the label
`topology_candidate` unless independently validated.

## Multi-k results

Assemblies at different k use different vertices, support opportunities, repeat resolution, and read
placement spaces. Their support numbers are not directly interchangeable. Stable 0.1 has no sweep;
independent invocations may be compared externally. Any future optional sweep must keep each assembly
independent. Concordance would indicate compatible sequence intervals, not proof that the longest or
most frequent answer is correct.

## High-background and low-abundance data

VeritAsm is target-neutral. It cannot know which low-support sequence matters. A minimum-support rule
can remove genuine low-abundance sequence, while a sensitive rule can retain large quantities of
error and background. Exact disk partitioning constrains counting memory but does not make runtime,
temporary storage, or retained-graph memory independent of dataset complexity. The retained-key cap
can terminate an otherwise valid high-background run.

Optional probabilistic acceleration is allowed only when it cannot suppress processing required for
the exact retained key set and retained counts or become final evidence. The stable 0.1 assembly path
still counts every accepted key exactly so it can report the complete pre-threshold histogram. A
Bloom-filter hit is uncertain. A Count-Min estimate is an overestimate under its assumptions. Exact
verification is required before any scientific artifact or control comparison.

## Controls and supplied backgrounds

Test samples, extraction blanks, library blanks, process negatives, and positive controls should be
assembled independently. Coassembly can hide which sample supplied an edge. Comparison happens after
assembly and must report exact raw support in each supplied dataset. A sequence observed in a blank is
not automatically irrelevant; a sequence absent from a finite blank or background index is not
biologically novel. Host or background subtraction, if later provided, must be optional, reversible,
checksummed, and audited against the unmodified result.

## Adventitious-agent application boundary

VeritAsm is unvalidated research software. It reconstructs algorithm-defined unitigs and reports
internal consistency from the same supplied reads used for construction. It does not detect or
identify an organism; establish biological presence or absence, viability, infectivity, sample
sterility, or product safety; determine product disposition; or replace a validated or compendial
method. Regulatory and standards references describe the surrounding workflow only and do not state
or imply compliance, approval, clearance, qualification, validation, or fitness for a regulated
purpose.

See `docs/ADVENTITIOUS_AGENT_APPLICATION.md` for control design, prohibited claims, and source
evidence.

## Benchmark interpretation

Truth-known simulation can test exact recovery against its simulator assumptions but cannot reproduce
all library and biological artifacts. Semi-synthetic spikes preserve real background while retaining
an artificial target/background boundary. Unmodified public datasets improve operational realism but
usually lack complete truth. Results from these tiers are never pooled into a single accuracy number.

Comparator failures, timeouts, installation problems, parameter mismatches, and VeritAsm regressions
remain in the record. Benchmark endpoints such as recovery at a given planted fragment count are not a
validated detection limit.
