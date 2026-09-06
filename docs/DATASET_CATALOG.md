# Dataset catalog and admission ledger

- Status: **catalogue of planned and candidate data; no public dataset is admitted**
- Catalog version: `veritasm-datasets-v1`
- Review date: 2026-09-04

An accession, paper, URL, or reported checksum is a lead, not a locally verified dataset. VeritAsm
benchmarks may use only entries whose exact bytes and metadata have passed the admission procedure
below. This file intentionally records missing evidence rather than filling it from memory.

## Status vocabulary

| Status | Meaning | May enter frozen scorecard? |
|---|---|---:|
| `candidate` | Stable locator or literature lead exists, but one or more required bytes, hashes, metadata, truth, or rights fields are missing | No |
| `downloaded-unverified` | Bytes exist locally but the authoritative transport checksum or expected inventory has not been verified | No |
| `admitted` | Source bytes, authoritative checksum when supplied, local SHA-256, metadata, rights, role, truth/limitations, and derivation manifest are frozen | Yes |
| `derived-admitted` | Every parent is admitted and a deterministic, checksum-frozen derivation produced the scored bytes | Yes |
| `exploratory` | Useful for development or diagnosis but excluded before scoring | No |
| `rejected` | Proven unsuitable, irreproducible, legally unusable, or non-equivalent; reason retained | No |

Changing an identifier's status never overwrites the previous ledger entry. Superseded bytes keep
their hashes and result associations.

## Admission procedure

For each source object:

1. Record provider, accession/version or DOI, landing page, retrieval timestamp, protocol, and exact
   remote filename/object identifier.
2. Preserve provider metadata including sample, organism/construct label as supplied, platform,
   layout, read length, library preparation, run count, file sizes, source checksums, and rights or
   data-use terms. A repository label is not assumed biologically correct.
3. Download without transforming bytes. Verify every provider checksum that binds to the exact object.
4. Compute local SHA-256 and size. Retain a transport log and directory inventory.
5. Decode with a separately pinned validator; record FASTA/FASTQ format, gzip member count, read and
   pair counts, normalized-ID results, base/quality ranges, and logical decoded-read digest. Do not
   repair malformed pairs silently.
6. Freeze the reference/truth object by version and SHA-256. State whether it is exact simulator truth,
   a strain reference, a community definition, a spike truth, or merely contextual. Reference
   disagreement is not automatically an assembly error.
7. Assign exactly one validation tier and role, document known confounders, and predeclare metrics.
8. Write a machine-readable admission manifest and checksum it before assembly. Independent review of
   the manifest changes status to `admitted`.

If a provider supplies MD5 but not SHA-256, both are retained: MD5 checks transport identity; local
SHA-256 identifies benchmark bytes. Compressed-file and decoded-read hashes are distinct named
domains. URL redirects, regenerated archives, and changed SRA/ENA representations are not assumed
byte-equivalent.

## Current admission summary

| Collection | Status | Blocking evidence |
|---|---|---|
| Generated truth-known suite | `candidate` | A six-case deterministic generator v4 and evaluator/result v4 are implemented with external content-root binding, but the full matrix, cross-platform byte verification, executable digests, preregistered content roots, and admission review remain absent |
| Semi-synthetic suite | `candidate` | No background admitted; merge implementation and derived hashes absent |
| Public read datasets below | `exploratory`/`candidate` | One neutral paired-end operational run is byte-verified below but has not passed independent admission review; other source checksums/truth mappings remain unresolved |
| Public reference objects below | `exploratory`/`candidate` | One versioned MG1655 reference object is byte-verified below but is not automatically exact truth for the public read run |

**Admitted dataset count: 0.** No current entry authorizes an accuracy, recovery, runtime, or memory
claim.

## Planned truth-known suite

The complete factor grid and seed derivation are normative in `VALIDATION.md`. Each grid cell has
three deterministic replicate IDs. The following are dataset families, not generated files.

| Planned family | Tier | Truth | Current status |
|---|---:|---|---|
| Linear GC/depth, SE and PE | 1 | Exact generated linear molecule, origins, and error events | `candidate` |
| Uneven-depth/dropout | 1 | Exact generated molecule and per-interval sampling probabilities | `candidate` |
| Exact and near-repeat | 1 | Exact molecule, repeat copies, and differences | `candidate` |
| Closed topology and linear confounders | 1 | Exact molecular topology and fragment origins | `candidate` |
| Two-component divergence/ratio | 1 | Exact component paths and fragment source | `candidate` |
| Low-abundance/high-background | 1 | Exact target/background truth and fragment source | `candidate` |
| Quality, ambiguity, adapter, duplicate, and chimera artifacts | 1 | Exact injected event ledger | `candidate` |
| Pair-repeat and low-input boundaries | 1 | Exact pair origin/span/orientation and molecule truth | `candidate` |

The truth-only words `target`, `background`, and `component` are not placed in assembler-visible
headers. Generated recovery is not a biological detection study.

## Candidate and exploratory reference objects

Accession versions are specified so a later retrieval cannot silently follow a moving unversioned
record. A locally verified object remains `exploratory` until its admission manifest and intended
truth relationship receive independent review.

| Catalog ID | Stable locator | Proposed role | Local SHA-256 | Status and limitation |
|---|---|---|---|---|
| `ref-puc19-l09137.2` | NCBI GenBank `L09137.2` | Small synthetic/plasmid-like circular reference and spike candidate | Missing | `candidate`; exact FASTA representation/topology and rights metadata must be frozen |
| `ref-phix174-nc_001422.1` | NCBI RefSeq `NC_001422.1` | Small circular control/reference candidate | Missing | `candidate`; use does not make a sequencer-control observation independent sample truth |
| `ref-lambda-nc_001416.1` | NCBI RefSeq `NC_001416.1` | Linear repeat/junction and spike candidate | Missing | `candidate`; reference relationship to any read run must be established |
| `ref-ecoli-u00096.3` | GenBank `U00096.3` | E. coli K-12 MG1655 reference candidate | Missing | `candidate`; sample strain and structural differences must be verified for each run |
| `ref-ecoli-nc_000913.3` | NCBI assembly `GCF_000005845.2`, object `GCF_000005845.2_ASM584v2_genomic.fna.gz`; contains RefSeq `NC_000913.3` | Versioned MG1655 contextual reference for `reads-err11767108` | Compressed `a96d3cfa58c88d477013c768f90a11d2386d42a70d566abfb6d9013dcbd24255`; decoded FASTA `53bb6a51b6e92139ced1e38f74b7938781027c52200922ff03718c2237d23bb4` | `exploratory`; provider MD5 `c13d459b5caa702ff7e1f26fe44b8ad7` verified; one 4,641,652-base record; sample/reference differences remain possible |
| `ref-yeast-gcf_000146045.2` | NCBI Assembly `GCF_000146045.2` | Yeast background/reference candidate | Missing | `candidate`; exact assembly level and sequence set must be frozen |
| `ref-cho-gcf_003668045.3` | NCBI Assembly `GCF_003668045.3` | CHO high-background contextual reference candidate | Missing | `candidate`; cell-line divergence and host/reference incompleteness limit truth use |
| `refs-zymo-zenodo-3935737` | Zenodo DOI <https://doi.org/10.5281/zenodo.3935737> | Defined-community reference bundle candidate | Missing | `candidate`; a reported MD5 `be18fd9195379082096061b8249489b3` has not yet been bound here to a verified exact filename, so it is not admission evidence |

The NCBI accessions can be resolved through the NCBI Datasets or Entrez records, but the exact retrieval
command, archive inventory, and output hashes must be retained. No runtime download is performed by
VeritAsm.

## Candidate and exploratory public read datasets

Published sizes or MD5 values in this table are discovery metadata only until independently matched to
the exact retrieved files. `Missing` is deliberate.

| Catalog ID | Locator and proposed role | Reported source evidence | Required truth/context | Local SHA-256 | Status/blocker |
|---|---|---|---|---|---|
| `reads-err11767108` | ENA run `ERR11767108`; neutral paired-end operational and compression/parser stress case | ENA reports *E. coli* K-12 MG1655, WGS/genomic, paired, Illumina NovaSeq 6000, BioProject `PRJEB56300`, BioSample `SAMEA111397405`; exact `_1`/`_2` byte sizes 36,831,122/41,694,872 and MD5 `6647011752558801272adcff57896c7f`/`bdbe43d18b7102d836d909ef8dfe9e64` | `ref-ecoli-nc_000913.3` is contextual, not assumed sample truth; predeclared use is operational robustness plus separately caveated reference agreement | Compressed R1 `ede87d6477a15440039a25820de1a6e3f35043a49140c198e582ab5725a0f568`; R2 `89eae9c65e5bf33d5a94d4eb8b62ad27449dda495cab2a48216f797382e881a5`; decoded R1 `eb92cb40c596cdcabd39f6f65c431b69f0afd43a432925c0b84d9432a7a1f9cf`; decoded R2 `91d72502c139b9f83b12e5700febc7419025448a92b61424155b59d77bc2b527` | `exploratory`; downloaded 2026-09-05T04:26Z and provider MD5s verified; 1,265,740 synchronized 2x151 pairs, one gzip member per file; admission review, rights field, assembler/comparator plan hash, and scorecard remain absent |
| `reads-err022075` | ENA run `ERR022075`; paired short-read isolate-like operational candidate | Candidate filenames reported as `_1.fastq.gz`, 1,875,702,420 bytes, MD5 `4766e60e72da987e4c54e47fee5653a4`; `_2.fastq.gz`, 1,982,742,623 bytes, MD5 `78608288625e084d68b4b5e8c1725933` | Exact sample/strain metadata and appropriate versioned reference unresolved | Missing | `candidate`; remote bytes and MD5 have not been verified |
| `reads-srr519926` | SRA run `SRR519926`; E. coli MiSeq paired operational/quality-stress candidate | Described in prior review as approximately 400,000 2x250 pairs and about 43x, with lower R2 quality; no source checksum recorded | Exact metadata, file representation, source checksum, and matching reference | Missing | `candidate`; descriptive values are not verified catalog facts |
| `reads-srr452441` | SRA run `SRR452441`; yeast paired operational candidate | Prior review identified 101-base paired reads; no exact file/checksum recorded | Exact sample strain, library metadata, and relationship to `GCF_000146045.2` | Missing | `candidate` |
| `reads-atcc-msa1003` | BioProject `PRJNA510527`, candidate experiment `SRX5169925`; defined microbial mixture | Project/experiment locators only | Exact run accession(s), expected component/reference versions and abundances, source checksums | Missing | `candidate`; experiment locator is not a FASTQ identity |
| `reads-zymo-d6300` | BioProject `PRJNA648136`, candidate experiment `SRX8824472`; defined microbial mixture/high-background case | Project/experiment locators only | Exact run accession(s), lot/community truth, source checksums, and reference bundle | Missing | `candidate` |
| `reads-porter-blanks` | BioProject `PRJNA735051`; candidate runs `SRR14737466` through `SRR14737471`; blank/control application set | Accession range from the associated study; no exact files/checksums recorded | Exact sample-role mapping, preparation metadata, source checksums, and permissible cross-sample comparison design | Missing | `candidate`; blank observations are not biological absence truth |
| `reads-cho-high-background` | High-CHO-background multilaboratory study lead, Chin et al., <https://doi.org/10.1038/s41541-025-01351-2> | Literature locator only | Public repository accession, sample/control mapping, read files, source hashes, spike truth, and terms unresolved | Missing | `candidate`; do not cite as a dataset until an accession is resolved |

## Original three-class public-dataset requirement

The original mission required small public read datasets representing a segmented RNA virus, a
non-segmented RNA virus, and a DNA virus, with recorded accessions and checksums. The neutral VeritAsm
repositioning changes the product and claims boundary; it does not by itself waive that validation
obligation. No current public-read entry belongs to any of the three required classes:

| Required public-read class | Current catalog entry | Status |
|---|---|---|
| Segmented RNA virus | None | **Absent; accession, exact read objects, checksums, metadata, reference/truth policy, and admission review are all missing** |
| Non-segmented RNA virus | None | **Absent; accession, exact read objects, checksums, metadata, reference/truth policy, and admission review are all missing** |
| DNA virus | None | **Absent; candidate PhiX/lambda reference sequences are not public read datasets and do not satisfy this row** |

This is a validation gap, not a deliberate fulfillment by omission. Such data may be used only as a
separate application stratum after the ordinary admission, control, and claim rules pass. Assembly
recovery still cannot be converted into an organism-presence or absence call.

## Semi-synthetic derivations to create after admission

Each admitted E. coli, yeast, defined-community, or CHO-like background is eligible for a Tier 2
derivation only after an exact target reference and generator are admitted. Planned cells are the
unspiked control and spike depths 2x, 10x, and 50x at background:target fragment ratios 10:1, 100:1,
1,000:1, and 10,000:1 where enough original background fragments exist.

The derivation manifest must contain:

- parent compressed and logical-read SHA-256 values;
- exact target truth and generated-spike hashes;
- plan version, case ID, seed digest, generator/RNG identity, and spike fragment-origin ledger;
- pair-preserving merge algorithm/version and ordered output fragment IDs;
- derived R1/R2 byte sizes, provider-independent local SHA-256 values, record/pair counts, and logical
  read digest; and
- an explicit statement that no background read was trimmed, subtracted, sampled with replacement,
  corrected, or relabelled beyond collision-free output identifiers.

If a requested ratio cannot be formed from the original background without reuse, that cell is
`not_constructed_insufficient_background`; reads are not duplicated to make it pass.

## Reference and truth caveats

- A public isolate can differ from its named reference through biological variation, passage,
  contamination, library artifacts, or metadata error. QUAST-like reference disagreement is not
  automatically assembler error.
- Defined-community certificates describe intended components, not necessarily every molecule in the
  sequenced library. Undeclared sequence is not automatically a false assembly.
- A negative or blank has incomplete sampling and may contain background sequence. It is an
  application control, not absence truth.
- pUC19, PhiX, lambda, microbial, yeast, or CHO references can supply computational alignment context;
  they do not establish viability, source, taxonomy of an unknown, or regulatory suitability.
- Different FASTQ exports of one archive record may preserve logical reads but differ byte-for-byte.
  Both compressed-byte and decoded logical-read identities are retained.

## Rejection and exclusion log

No dataset is currently rejected. A future rejection entry must preserve the ID, exact bytes/hash if
obtained, date, reason, and whether any exploratory result was already viewed. Reasons include pair
corruption, missing rights, irrecoverable metadata ambiguity, unavailable truth for a truth-required
role, checksum mismatch, or a task/data-type mismatch. Difficulty, poor VeritAsm performance, or a
comparator win is never a valid exclusion reason.

## Sources and discovery links

- NCBI Datasets: <https://www.ncbi.nlm.nih.gov/datasets/>
- NCBI Sequence Read Archive: <https://www.ncbi.nlm.nih.gov/sra>
- European Nucleotide Archive: <https://www.ebi.ac.uk/ena/browser/home>
- Zymo reference bundle candidate: <https://doi.org/10.5281/zenodo.3935737>
- CHO high-background study lead: <https://doi.org/10.1038/s41541-025-01351-2>

These links establish discovery routes only. Admission depends on exact retrieved objects and local
manifests, not the continued content of a landing page.
