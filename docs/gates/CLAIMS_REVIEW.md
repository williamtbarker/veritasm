# Gate: adversarial claims review

> **Historical gate record; superseded as a description of the current tree.** This document preserves
> the initial 2026-09-03 FAIL against the pre-rewrite public surfaces. The original findings and
> dispositions remain below for audit provenance; statements that metadata still describes Virustic2
> are not current. This notice does not convert the claims gate to PASS. Final integrated package
> verification, platform evidence, scientific validation, and a release-time claims review remain open.

**Review date:** 2026-09-03

**Historical status at review:** **FAIL — public packaging was blocked**

**Current status:** **SUPERSEDED AS CURRENT-STATE ASSESSMENT; RELEASE-TIME CLAIMS REVIEW OPEN**

**Scope reviewed:** `docs/PRODUCT_CONTRACT.md`, `ARCHITECTURE.md`, `docs/OUTPUT_SCHEMA.md`,
`docs/SCIENTIFIC_LIMITATIONS.md`, `README.md`, `CITATION.cff`, `SECURITY.md`, `CHANGELOG.md`, and
`docs/ADVENTITIOUS_AGENT_APPLICATION.md` as present on the review date

## Later remediation status — not a gate pass

| Original findings | Source-level disposition in the 0.2.0-alpha.1 candidate | Still open |
|---|---|---|
| CR-01--CR-05: inherited identity, readiness language, independent-evidence wording, circularity, and viral framing | Public metadata and examples now use the provisional neutral VeritAsm identity, unreleased research status, same-read internal-consistency language, and `closed_graph_walk` terminology | Name/namespace clearance and final archive-wide consistency review |
| CR-06, CR-08--CR-11, CR-13--CR-14: ambiguous technical, evidence, control, regulatory, pair, and probabilistic language | Product, schema, limitation, application, architecture, and report surfaces now define algorithmic terms, prohibit biological interpretation, and keep probabilistic values and pair observations out of stable sequence decisions | Final rendered-bundle inspection and any future control/scout/pair-qualification claims |
| CR-07, CR-09, CR-15: absent implementation ledger, exactness definition, and validation record | README, validation ledger, final report, source modules, tests, schemas, and ADRs now separate stable, experimental, failed, and proposed work and define `exact` narrowly | Final clean-source and extracted-crate gates, retained logs, and release-time claims review |
| CR-12: network, memory, atomicity, determinism, platform, and performance overstatement | Current language narrows each property to its implemented contract or recorded bounded evidence and publishes the measured development regression | Apple Silicon execution, dynamic network isolation, complete crash/durability coverage, and broader performance/resource evidence |
| Scientific superiority, detection, absence, or regulated-use implications | Current public surfaces prohibit these claims | Public-data/comparator validation and any separately governed application validation remain NOT RUN; none is implied by this remediation |

The initial review below remains useful as an adversarial checklist. Its unchecked boxes are historical
review marks, not a machine-readable representation of current source remediation or final gate status.

## Release decision

The architecture and limitation documents mostly establish an appropriately narrow scientific
boundary. The repository's actual public surfaces do not. `README.md`, `CITATION.cff`, `SECURITY.md`,
`CHANGELOG.md`, and the Cargo package metadata still describe the inherited Virustic2 package. They
claim a viral identity, production posture, strict pair behavior, independent evidence, circular
unitigs, and current features that conflict with the VeritAsm contract and with the completed baseline
audit.

No source archive, Cargo package, crate publication, tagged release, or public repository handoff may
be described as VeritAsm until the P0 findings below are closed and a feature-to-test evidence ledger
shows which contract items are actually implemented. Design documents describe intended behavior;
their presence is not implementation evidence.

Severity in this review means:

- **P0:** release blocker; a public artifact would be false, materially misleading, or internally
  contradictory;
- **P1:** wording or schema ambiguity likely to cause scientific overinterpretation; required before
  review-package approval; and
- **P2:** consistency or evidence-hardening change required before making the affected claim.

## Findings summary

| ID | Severity | Claim surface | Finding | Required disposition |
|---|---|---|---|---|
| CR-01 | P0 | README, citation, security, changelog, Cargo metadata | Public identity and feature set are still Virustic2 | Rewrite and verify every installed/public surface |
| CR-02 | P0 | README | “Production-minded,” “serious beta,” and “not yet a substitute” imply readiness or a regulatory progression unsupported by evidence | Remove; use explicit pre-release research scope |
| CR-03 | P0 | README, AA note | Supplied records are called independent evidence | Replace with `distinct supplied fragment instances`; label read-back as internal consistency |
| CR-04 | P0 | README, changelog, AA note | Graph closure is described or can be read as molecular circularity | Use `closed_graph_walk`; state seam reads are insufficient to prove a circular molecule |
| CR-05 | P0 | README, package metadata | Viral/organism-specific identity conflicts with the neutral contract | Remove viral identity from current product metadata |
| CR-06 | P0 | output schema | `complete`, `complete_empty`, `unique`, and `audit` can be read as biological or validation outcomes | Define or rename as algorithmic execution/mapping states |
| CR-07 | P0 | all public surfaces | No implemented-versus-designed inventory supports the planned VeritAsm feature claims | Add evidence ledger and keep unimplemented features out of present-tense copy |
| CR-08 | P1 | product contract, limitations, AA note | “Sensitive,” “quality-control,” and “read-backed evidence” can imply analytical sensitivity, GxP suitability, or independent confirmation | Use operational names and same-read provenance qualifiers |
| CR-09 | P1 | architecture, output schema | `exact` lacks one repository-wide public definition | Define its algorithmic scope and state what it does not establish |
| CR-10 | P1 | AA note | Control association, carryover, novelty, and enrichment names can be read as causal or biological calls | Use non-causal, exact-observation names and retain finite-control caveats |
| CR-11 | P1 | AA note, limitations | Regulatory citations and “regulated use requires” wording could imply a supported validation pathway | Add a no-conformance/no-endorsement statement adjacent to citations and disclaimer |
| CR-12 | P1 | README, security | Network, memory, pair safety, atomicity, and cross-platform claims exceed the retained evidence or describe only a component | Bound each claim to an executed test and documented platform |
| CR-13 | P1 | output bundle contract | Machine artifacts do not yet require evidence provenance and interpretation-scope fields | Add machine-readable scope fields and render them in HTML |
| CR-14 | P2 | architecture and related ADR wording | `biological support`, `qualified link`, and Bloom shorthand weaken otherwise careful definitions | Replace with algorithmic terms and the full proof premises |
| CR-15 | P0 | repository gate set | `ARCHITECTURE.md` refers to `VALIDATION.md`, but that file was absent at review time | Create and satisfy a validation gate before claims approval |

## P0 required changes

### CR-01 — inherited public identity is not shippable

Affected snapshot locations:

- `README.md:1-10`, `README.md:29-73`, and `README.md:111-133`;
- `CITATION.cff:3-15`;
- `SECURITY.md:3-11`;
- `CHANGELOG.md:5-15`; and
- Cargo metadata: package, library, and executable names, description, repository, homepage, and
  keywords.

These surfaces identify `Virustic2`, use its executable and repository, and describe the inherited
implementation. They cannot serve as VeritAsm release metadata. Before any public package:

1. choose and collision-check the final product and crate name;
2. replace every installed example and link with the actual built executable and repository;
3. remove `viral genomics` and virus-specific product identity from metadata;
4. describe only features present in the packaged source and exercised by retained tests; and
5. identify Virustic2 only as a pinned comparison ancestor, never as the current product.

The safe lead, after the corresponding implementation exists, is:

> VeritAsm is pre-release research software that constructs deterministic, database-free conservative
> graph segments from supported short DNA reads. Segments are maximal only under its declared degree
> and fixed-point boundary model. Results are conditional on the recorded input, quality rules, k,
> support definition, and graph transformations. It does not identify organisms or emit biological
> presence or absence decisions.

Do not use “general-purpose,” “for any single-end or paired-end data,” “extremely accurate,” “best,”
“state of the art,” or “production ready.” The initial evaluation domain in the product contract is a
bounded test domain, not evidence of broad generality.

### CR-02 — production and clinical/regulatory progression language

`README.md:3`, `README.md:9-10`, and `README.md:90-91` use “production-minded,” “serious beta,” “not yet
a substitute,” and “production-oriented.” These formulations imply current operational maturity or
that validation would naturally turn this assembler into a replacement assay. `README.md:132-133`
prohibits clinical use only “without independent validation,” which can be read as authorizing such use
after an unspecified validation exercise.

Required replacement:

> This pre-release research implementation is not for diagnosis, treatment, public-health decisions,
> manufacturing acceptance or release, sterility conclusions, or regulatory decision-making.

The word `production` may describe a measured software deployment property only after the exact
property and environment are evidenced. It must not describe the package's general posture.

### CR-03 — construction records are not independent evidence

`README.md:69-71` calls fragment-counted k-mers “independent evidence.” The same source records are used
to build and audit the graph; PCR duplicates, optical duplicates, copied inputs, index misassignment,
and shared preparation artifacts can all make record counts non-independent. The careful correction in
`docs/SCIENTIFIC_LIMITATIONS.md:23-27` must control every public surface.

Additional ambiguous locations are:

- `docs/PRODUCT_CONTRACT.md:7-9` (`read ... evidence` without immediate provenance);
- `ARCHITECTURE.md:195` (`Exact read-backed audit`, despite the good caveat at line 207);
- `docs/ADVENTITIOUS_AGENT_APPLICATION.md:13`, `:29`, `:57`, and `:97-107`; and
- related design language such as “minimum independent linkage.”

Required terminology:

- use **distinct supplied fragment instances**, never independent fragments or molecules;
- use **construction-read remapping** or **same-read internal-consistency audit**, never independent
  validation;
- reserve **orthogonal evidence** for data or methods genuinely external to graph construction; and
- use **separately processed inputs/runs** when “independent” means computational separation.

The phrase `read-backed evidence` is permitted only when the same paragraph or a mandatory adjacent
field says that the reads are the construction data and that remapping is an internal-consistency
check. Multi-k children reuse one spool and are separate algorithm runs, not independent observations.

### CR-04 — graph closure must not become molecular circularity

`README.md:83-88`, `README.md:98-108`, and `CHANGELOG.md:13` describe circular unitigs, circularity, or
canonical circular output. The baseline audit established only a closed one-in/one-out graph cycle.
Those statements are release-blocking false biological implications.

`docs/OUTPUT_SCHEMA.md:26-28`, `ARCHITECTURE.md:180-190`, and
`docs/SCIENTIFIC_LIMITATIONS.md:48-53` correctly prefer `closed_graph_walk`. That term must be used in
FASTA, GFA, JSON, HTML, examples, screenshots, and prose.

`docs/ADVENTITIOUS_AGENT_APPLICATION.md:109-111` must also change. A seam-spanning read or fragment can
support a candidate closing adjacency but can arise from repeats, ligation, amplification, or a linear
concatemer. It is not sufficient for molecular circularity. Required wording:

> A closed graph walk is an algorithmic topology result, not proof of a circular molecule. Report the
> exact closing-adjacency support and conflicting explanations, and retain
> `topology=closed_graph_walk`. Molecular circularity requires orthogonal validation outside this
> assembler.

Do not use `circular`, `circular contig`, `complete circle`, `plasmid`, or `circular genome` as a
software conclusion.

### CR-05 — organism and adventitious-agent detection

The public product identity must remain neutral. An assembler result does not prove an organism or a
contamination event. The package must never emit or promote:

- `agent_present`, `agent_absent`, `organism_detected`, `pathogen`, `sterile`, `safe`, `clean`, or a
  product-disposition state;
- “detects unknown/adventitious agents”;
- “no agent was detected” as a synonym for no retained unitig or read match; or
- taxonomic, viability, infectivity, novelty, or risk conclusions from graph output.

Adventitious-agent material belongs in an explicitly optional research-application document and must
use “may support method development or investigation within a complete workflow.” It must not become a
Cargo keyword, primary tagline, default mode name, or binary success criterion.

### CR-06 — execution and mapping statuses can leak biological meaning

`docs/OUTPUT_SCHEMA.md:56-63` and `:95-96` require stronger names or definitions:

- `unique_fragment_placements` means one accepted placement group in the emitted unitig set under the
  recorded mapper. It does not mean unique biological origin or an independent molecule. Prefer
  `single_accepted_placement_group_fragments`.
- `audit_status=complete` means candidate enumeration completed under the recorded rules and limits.
  Prefer `placement_enumeration_complete`; never call it a complete assembly validation.
- `run.status=complete` means the software transaction completed, not that a genome, sample analysis,
  or workflow is complete.
- `complete_empty` is especially unsafe. Rename it `complete_no_unitigs_under_parameters` and require
  the HTML and JSON limitation: “No sequence met the recorded assembly rules; this is not biological
  absence.”

FASTA and GFA alone are easy to detach from HTML caveats. The committed bundle must include a short
machine-readable interpretation scope, and any exported standalone sequence artifact should identify
the bundle/schema needed to interpret it.

### CR-07 — planned behavior is not an implemented feature

`docs/PRODUCT_CONTRACT.md:3` and `ARCHITECTURE.md:3-7` appropriately label the material as a candidate or
intended architecture. The public README and changelog do not make that distinction. Before packaging,
create a feature-to-evidence ledger with, for every public feature:

- implementation location and version;
- unit/integration/property/fuzz/golden test identifiers;
- supported platforms actually executed;
- dataset and input limits;
- retained result digest; and
- unresolved failures or exclusions.

Public documentation must have visibly separate **implemented and tested**, **experimental**, and
**proposed** sections. Anything only in the product contract or architecture remains future tense.

At review time, `VALIDATION.md`, referenced by `ARCHITECTURE.md:6`, was absent. That absence alone blocks
claims approval. Creating the filename is not sufficient; its gates must be executed and results
retained.

## P1 semantic changes

### CR-08 — profile names and quality-control implications

`docs/PRODUCT_CONTRACT.md:12-14` calls low-abundance reconstruction a quality-control use case. Use
“research evaluation and method-development scenario” unless a precise software-QC meaning is given.
Do not imply GxP, release testing, or analytical-method suitability.

The profile name `sensitive` at `docs/PRODUCT_CONTRACT.md:54-55` and the same terminology elsewhere can
be mistaken for validated analytical sensitivity. Prefer operational names:

| Current | Required public term | Exact meaning |
|---|---|---|
| `sensitive` | `retain_all` or `no_deleting_transform` | Retain every accepted observed k-mer; no claim about target recovery or false negatives |
| `conservative` | `thresholded` | Apply the recorded support/tip rule; no claim about biological conservatism or safety |
| `quality-aware` | `base-quality-thresholded` | Reject windows by the recorded Phred rule; not quality-weighted counting |

If CLI compatibility requires old names, help text and every report must display these operational
definitions and disclaimers.

### CR-09 — define `exact` once and qualify every use

The contract, architecture, and schema use `exact` frequently. Add this normative definition near the
first public use and incorporate it into `run.json`:

> `exact` means full encoded sequence identity and checked integer counting under the recorded parser,
> QC, support, graph, and mapping rules. It does not mean the reconstruction is the true biological
> sequence, that placement is the true origin, that the input is unbiased, or that all sequence in the
> sample was observed.

Specific changes:

- rename `ARCHITECTURE.md:195` from “Exact read-backed audit” to “Exact construction-read remapping
  audit”;
- replace `ARCHITECTURE.md:168` “biological support” with “canonical k-mer fragment-instance support”;
- define an `exact placement` in `docs/OUTPUT_SCHEMA.md:56` as a full-sequence match under the recorded
  mapper and current unitig set, not true-origin mapping; and
- qualify `exact graph links` as exact adjacencies in the retained algorithmic graph.

### CR-10 — non-causal control and novelty terminology

`docs/ADVENTITIOUS_AGENT_APPLICATION.md` correctly says controls are context rather than a subtraction
oracle, but several proposed labels still imply causes:

- replace `possible_cross_sample_carryover` with `shared_with_multiplex_neighbor`; provide carryover,
  index misassignment, shared biology, and contamination only as non-exclusive hypotheses;
- define `control_associated` as exact observation in a supplied control, not evidence that the control
  caused the sample sequence;
- replace final-field `novel` with `control_selected_token_unobserved`;
- replace final-field `enriched` with a neutral named statistic such as
  `sample_control_exact_count_ratio`, including raw counts, denominators, formula, pseudocount, and NA
  reasons; and
- keep all Bloom, syncmer, and Count-Min values in an `experimental_triage` namespace labelled
  approximate.

Finite controls, stochastic contamination, unequal depth, and multiplex leakage prevent both causal
classification and absence claims. Exact verification eliminates approximate membership errors; it
does not eliminate sampling error, preparation bias, or incomplete controls.

### CR-11 — regulatory citations describe context, not conformance

`docs/SCIENTIFIC_LIMITATIONS.md:83-90` and `docs/ADVENTITIOUS_AGENT_APPLICATION.md:24-50` correctly reject
assay claims, but “Any regulated use requires ... validation” can be read as implying that this package
has an established validation path or is otherwise suitable. Regulatory and compendial citations must
carry this adjacent sentence:

> Regulatory and standards references describe the surrounding workflow and claim boundary only.
> They do not state or imply that VeritAsm is compliant, approved, cleared, qualified, validated, or
> fit for a regulated purpose.

ICH Q5A(R2) is viral-safety guidance for its stated product scope, not a blanket adventitious-agent or
software certification. USP mycoplasma and sterility frameworks are separate. Citing FDA, EMA, EDQM,
WHO, NIST, USP, or PDA must not add their names to badges, feature lists, compatibility statements, or
marketing headings.

Do not automatically apply FDA's formal RUO IVD label. Whether it is appropriate depends on product
classification, intended use, distribution, and promotion. An RUO label cannot neutralize diagnostic,
release-testing, or product-disposition claims.

### CR-12 — operational claims need bounded evidence

The inherited README overstates several properties:

- `README.md:16` says input memory changed from whole-file to streamed batches. This does not establish
  bounded whole-run memory because all retained graph keys can remain resident.
- `README.md:20` says strict pair validation, but Gate 0 found reversed/duplicated mate roles and path
  aliases were accepted.
- `README.md:25` says bounded deterministic parallelism; only batch memory is bounded, and cross-thread
  determinism was demonstrated for a narrow fixture rather than every artifact/platform.
- `README.md:27` says atomic output, while Gate 0 demonstrated that one artifact can replace an existing
  file before a later artifact fails.
- `README.md:117-119` can be retained only as an exact description of an executed baseline gate, not as
  evidence for the new bundle architecture.
- `SECURITY.md:3` says no network requests. The future claim must name the default executable, version,
  dependency set, and verification method; design intent alone is not runtime proof.

Use component-scoped language such as `bounded parser queue`, `deterministic on retained fixture X`, or
`same-file atomic rename` until end-to-end, cross-platform, and failure-injection evidence supports the
stronger property.

### CR-13 — machine-readable interpretation scope

`docs/OUTPUT_SCHEMA.md:79-102` should require `run.json` fields that HTML renders verbatim:

- `scientific_scope = algorithmic_unitig_reconstruction`;
- `evidence_source = construction_reads`;
- `evidence_interpretation = internal_consistency_not_independent_validation`;
- `support_unit = supplied_fragment_instance` or `accepted_window_occurrence`;
- `biological_call = not_performed`;
- `taxonomy = not_performed`;
- `control_context = not_supplied | supplied_not_interpreted | exact_comparison_available`; and
- a stable disclaimer/version identifier.

`report.html` must display, without user action, the scientific disclaimer, incomplete/NA reasons, and
the meaning of technical completion. The HTML must not transform counts or statuses into traffic-light
red/green biological outcomes.

### CR-14 — qualifiers and Bloom proof shorthand

The architecture's probabilistic separation is strong, but two phrases should change before public
review:

- `ARCHITECTURE.md:145-149` should not use the blanket shorthand “Bloom filters have no false
  negatives in a valid, unsaturated implementation.” State the actual premises from ADR 0004: classic
  insertion-only filters, completed insertions, identical key/hash/dimension/seed semantics, no cleared
  or corrupt bits, ordered first-pass state updates, identical QC/deduplication, and complete passes
  over the same immutable spool. Saturation increases false positives; it is not the proof premise.
- `qualified pair link` should be `algorithmically threshold-qualified pair-link group`. Qualification
  means only that recorded placement/orientation/span/count rules passed; it is not a validated
  adjacency or sequence join.

The scout's core rule must remain: a Bloom collision may alter priority or compute only. It cannot
discard evidence, change any exact artifact, or support detection or absence. An incomplete residual
path cannot commit a normal result bundle.

## File-by-file disposition

| File | Current disposition | Required before public package |
|---|---|---|
| `docs/PRODUCT_CONTRACT.md` | Conditional pass | Replace QC/profile ambiguity; add normative `exact` and same-read-evidence definitions |
| `ARCHITECTURE.md` | Conditional pass as design only | Change Bloom shorthand, `biological support`, audit heading, and link qualification; never cite it as implementation proof |
| `docs/OUTPUT_SCHEMA.md` | Fail pending semantic fields | Clarify exact/unique/complete; rename `complete_empty`; require interpretation scope in JSON/HTML |
| `docs/SCIENTIFIC_LIMITATIONS.md` | Conditional pass | Strengthen same-read disclaimer and regulatory no-conformance language; keep topology wording normative |
| `README.md` | P0 fail | Complete rewrite for the actual package, actual CLI, implemented features, evidence status, and current no-use boundaries |
| `CITATION.cff` | P0 fail | Final neutral name/version/repository; remove viral keywords; do not publish placeholder metadata |
| `SECURITY.md` | P0 fail | Current name and reporting route; reconcile path handling; scope no-network claim; document temporary sensitive data and non-guaranteed secure deletion |
| `CHANGELOG.md` | P0 fail | Separate inherited baseline from VeritAsm changes; list only changes actually implemented and tested; remove circularity claim |
| `docs/ADVENTITIOUS_AGENT_APPLICATION.md` | Conditional pass | Fix independent-evidence, circularity, causal control labels, exact disclaimer, and regulatory implication wording |

## Required canonical disclaimer

Replace the current product disclaimer in `docs/SCIENTIFIC_LIMITATIONS.md` and
`docs/ADVENTITIOUS_AGENT_APPLICATION.md`, and place the same version in the public README and generated
HTML report:

> VeritAsm is unvalidated research software. It reconstructs algorithm-defined unitigs and reports
> internal consistency from the same supplied reads used for construction. It does not detect or
> identify an organism; establish biological presence or absence, viability, infectivity, sample
> sterility, or product safety; determine product disposition; or replace a validated or compendial
> method. Regulatory and standards references describe the surrounding workflow only and do not state
> or imply compliance, approval, clearance, qualification, validation, or fitness for a regulated
> purpose.

If the final product name differs, substitute only the name; do not weaken the remaining text without
a new claims review.

## Claims approval checklist

Claims review may move to PASS only when all of the following are retained:

- [ ] Public name, Cargo metadata, binary, README, examples, citation metadata, security policy, and
      changelog agree.
- [ ] Every present-tense feature in the README maps to an implemented behavior and an executed test.
- [ ] Implemented, experimental, failed/regressed, and proposed capabilities are visibly separated.
- [ ] No supplied record, read pair, multi-k child, or remapping result is called independent evidence.
- [ ] Support is always `fragment_instance` or `occurrence`; it is not coverage, abundance, confidence,
      or molecule count.
- [ ] All topology output uses `closed_graph_walk` and states that seam support does not prove molecular
      circularity.
- [ ] Technical `complete`, `empty`, `unique`, `exact`, and `qualified` states have non-biological
      definitions in schemas and reports.
- [ ] No no-contig/no-hit/no-syncmer result can render as agent absence, sterility, safety, or a green
      pass state.
- [ ] Control sharing and sample/control statistics remain non-causal observations with raw exact
      counts and finite-sampling warnings.
- [ ] Probabilistic structures affect compute only; exact-output equivalence and collision adversaries
      pass.
- [ ] Regulatory/standards citations carry the no-conformance statement and no badge or endorsement
      implication.
- [ ] Network, memory, atomicity, determinism, platform, and performance claims cite executed retained
      evidence and exact conditions.
- [ ] `VALIDATION.md` exists, pre-registers the relevant gates, and records results without suppressing
      failures.
- [ ] The final report states what is implemented/tested, experimental, failed/regressed, and proposed,
      and makes comparison claims only for named datasets and metrics.

Until every P0 item and applicable checklist item is closed, the only defensible handoff description
is: **architecture and research review material accompanying an inherited prototype, not a VeritAsm
release candidate.**

The preceding sentence is the original review disposition. For the present source tree, use the
unreleased-review-candidate wording and open-gate inventory in `README.md`, `VALIDATION.md`, and
`docs/FINAL_REPORT.md`; none of those documents authorizes publication or a production-readiness claim.
