# Product pivot record

- Date: 2026-09-03
- Status: Accepted direction; neutral name and exact release contract pending architecture gate
- Previous working project: Virustic3, viral reconstruction
- New working project: VeritAsm, evidence-first short-read DNA assembly

## Reason

The user requested a neutral general-purpose assembler whose evidence model is useful for
low-abundance, high-background sequencing and adventitious-agent research/QC. Virus-specific
taxonomy, quasispecies, segment, and pathogen framing are no longer requirements. The pivot reduces
scientific overreach and produces a tool applicable to a wider set of ordinary sequencing problems.

## Reused evidence

- The complete Virustic2 source and behavior audit.
- FASTX, gzip, DNA encoding, deterministic reduction, graph-compaction, paired-evidence, output
  transaction, GFA, and general assembly literature.
- The rule that read-backed support and graph ambiguity must remain interpretable.

## Superseded evidence

- Viral competitor rankings and virus-specific validation datasets.
- Quasispecies, segmented-virus, host-depletion, viral circularity, and taxonomic feature priorities.
- Any claim that the successor is specifically a viral genome reconstruction package.

The prior research draft remains outside this repository at `../virustic3` for audit history. It is
not a source of current requirements unless a neutral ADR cites a reusable result explicitly.

## New claim boundary

The program may claim only to assemble and report evidence from supported short-read DNA. It may be
evaluated as a component in adventitious-agent workflows. It must not claim organism detection,
absence, regulatory validation, lot-release suitability, or clinical performance.

