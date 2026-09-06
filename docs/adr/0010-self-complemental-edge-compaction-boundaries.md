# ADR 0010: Stop compaction at self-complemental k-mer edges

- Status: accepted correctness repair for the next release candidate
- Date: 2026-09-04
- Supersedes: the exact paragraphs listed below

## Superseded text

- ADR 0001, `Decision`, first paragraph: the phrase “topology-preserving compaction into maximal
  non-branching unitigs” is narrowed to maximal walks between the degree and fixed-point boundaries
  defined in this ADR.
- `ARCHITECTURE.md`, Section 4, paragraph beginning “For every literal `(k - 1)`-mer node”: its
  compaction-boundary list gains incident self-complemental k-mer handles.
- `ARCHITECTURE.md`, Section 5, paragraph beginning “Compaction first works”: its use of “boundary”
  means the four-condition boundary in this ADR.
- `ARCHITECTURE.md`, Section 5, paragraph beginning “`edge_steps` is raw”: the statement that
  `edge_steps` and `canonical_kmers` can differ, and the example allowing both handles of one backing
  key in one walk, are withdrawn. They must be equal for every emitted unitig.

## Context

Let `rc` be reverse complementation over A/C/G/T. In the edge-centric order-`k` de Bruijn graph, a
literal k-mer `x` is an arc from its `(k - 1)`-base prefix to its `(k - 1)`-base suffix. A canonical
k-mer represents the reverse-complement orbit `{x, rc(x)}`. Most orbits have two directed handles,
but an even-length self-complemental k-mer has `x = rc(x)` and therefore only one fixed handle.

The previous doubled-graph traversal treated that fixed handle like an ordinary one-in/one-out arc.
At a self-complemental handle `p`, reverse-complement symmetry can make the apparent unbranched walk

```text
e, p, mate(e)
```

where `e` and `mate(e)` are the two orientations of one backing canonical k-mer. Collapsing the raw
walk's reverse-complement orbit then emits one string that uses the evidence for that canonical
k-mer twice. For example, the single accepted read `TCGCGAC` at `k=6` contributes canonical keys
`TCGCGA` (self-complemental) and `CGCGAC` (`rc=GTCGCG`), but the old compactor emitted
`GTCGCGAC`: three k-mer steps backed by only two canonical keys. The analogous minimal `k=4` case is
`AATTG`, which was incorrectly emitted as `CAATTG`.

This is a fixed-point problem in the representation, not extra sequence evidence. Self-complemental
DNA strings can have only even length because no A/C/G/T base complements itself. Consequently:

- odd `k` has no self-complemental k-mer arcs, but can have self-complemental `(k - 1)`-mer nodes;
- even `k` can have self-complemental k-mer arcs, but has no self-complemental `(k - 1)`-mer nodes.

The existing node-boundary rule handled only the first case. The second needs its dual edge rule.
Published bidirected-graph treatments likewise give self-complemental biarcs special traversal and
balance semantics; they cannot be consumed once independently in each direction. See Schmidt et al.,
*Matchtigs*, Genome Biology 2023, <https://doi.org/10.1186/s13059-023-02968-z>, especially the
self-complemental-biarc cases, and Orenstein et al., Bioinformatics 2013,
<https://doi.org/10.1093/bioinformatics/btt230>, Section 3.3 on even-`k` palindromic edges.

## Decision

VeritAsm continues to accept exact packed `k` values from 3 through 63, including even values.
Compaction classifies a literal `(k - 1)`-mer node as a boundary when any of these conditions holds:

1. its distinct oriented-handle indegree is not one;
2. its distinct oriented-handle outdegree is not one;
3. the node is self-complemental; or
4. the node is incident to a handle whose mate is itself.

Rule 4 marks both endpoints of every self-complemental k-mer handle. Such a handle is therefore a
one-edge linear segment and no emitted FASTA sequence crosses its orientation-fixed point. Ordinary
boundary transition derivation still emits all exact `(k - 1)` overlaps in GFA, so the graph
adjacencies are preserved rather than silently deleted.

Compaction must additionally fail closed unless every emitted unitig satisfies both equivalent
representation checks:

- its representative walk contains no canonical-key index more than once; and
- `edge_steps == canonical_kmers`.

The bundle validator repeats the count equality at the artifact boundary. A future implementation
may safely compact through some self-complemental biarcs only after replacing the present raw-walk
partition with a formally specified bidirected-side algorithm and proving that each backing biarc is
consumed once. Detecting a repeated canonical key while a walk is already being extracted is not an
accepted splitting algorithm: it chooses a break too late, obscures endpoint semantics, and can
violate reverse-complement and ownership invariants.

## Consequences

- The impossible `AATTG -> CAATTG` and `TCGCGAC -> GTCGCGAC` concatenations are removed.
- Even `k` remains supported without duplicating a self-complemental observation or inventing a
  second fixed handle.
- FASTA can be shorter around even-`k` self-complemental k-mers. This is deliberate conservative
  underassembly; the exact neighboring possibilities remain in GFA.
- Odd-`k` graphs have no self-complemental k-mer handles, so this new rule does not change their
  compaction boundary set.
- Stable output bytes and unitig identifiers can change for affected even-`k` inputs. The correction
  requires a changelog entry and fresh golden/package evidence before release.

## Rejected alternatives

### Reject every even k

This is a sound emergency stopgap because odd-length DNA k-mers cannot be self-complemental. It was
rejected as the primary repair because it removes an advertised and implemented input domain when a
small fail-closed boundary rule preserves exact counts, keys, and GFA adjacency evidence.

### Split only when a canonical key repeats in a raw walk

This catches the demonstrated symptom but is not a graph definition. It makes the split depend on
walk start order, leaves a remainder beginning at a non-boundary node, and complicates complete
oriented-handle and GFA-link conservation. Repetition detection is retained only as a fatal invariant
check after the fixed-edge boundary rule.

### Duplicate the self-complemental handle

Creating two indistinguishable handles would fabricate an extra observation and invalidate support
and ownership accounting. Full bidirected-side semantics, not blind duplication, are required for a
less conservative future treatment.

## Required evidence

- direct `k=4` and `k=6` regressions for the two minimal counterexamples;
- an independent small-even-`k` oracle covering every self-complemental key and bounded neighboring
  subgraphs;
- property checks for no repeated canonical key, `edge_steps == canonical_kmers`, complete retained
  key ownership, exact GFA overlaps, reverse-complement equivalence, and deterministic output;
- unchanged odd-`k` golden/property behavior; and
- the full Rust 1.85 release-candidate test suite before packaging.
