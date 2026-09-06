# ADR 0006: Experimental exact packed k-mers through k=127

- Status: accepted for an isolated research substrate; not accepted for the stable assembly pipeline
- Date: 2026-09-03

## Context

The stable 0.1 pipeline stores at most 63 two-bit bases in a `u128`. Longer exact k-mers can improve
repeat discrimination in some short-read datasets, but changing the Rust key type alone would be an
invalid migration. The frozen spool, exact-count runs, graph/decision digests, Bloom hash preimages,
memory admission formulae, output schemas, and golden bundles all assume the current 16-byte key or
the `k <= 63` contract.

Before choosing a new on-disk schema or graph implementation, the project needs a small exact value
and rolling scanner whose representation boundary can be falsified independently.

## Decision

Add `veritasm::experimental::wide_kmer` without connecting it to configuration, the CLI, spooling,
counting, graph construction, compaction, reporting, or Bloom acceleration.

The substrate supports exact lengths zero through 127 and k-mer scans for `3 <= k <= 127`. A packed
value is four most-significant-word-first `u64` words, has exactly 32 bytes of key storage, and uses
the existing A=00, C=01, G=10, T=11 right-aligned encoding. Its derived ordering is unsigned numeric
order. Explicit big-endian conversion is defined for experiments; native struct memory is never a
wire format, and the 32 bytes have no stable on-disk meaning without a separately versioned and
authenticated length.

Rolling extraction maintains full-width forward and reverse-complement states. Every IUPAC ambiguity
resets exact rolling state, and quality/accounting semantics match the stable scanner. Full-key
identity is never replaced by a hash. The implementation uses safe Rust and adds no dependency.

## Evidence required in this slice

- exact encode/decode and reverse-complement/canonical invariants at k=63, 64, 95, and 127;
- per-position/per-base boundary vectors and explicit active-bit masks across 64-bit words;
- rolling results and all mutually exclusive QC classes equal an independent slice/string oracle;
- random exact strings through length 127 satisfy round-trip and strand-invariance properties;
- every random exact key through k=63 is bit-identical to the stable `u128` implementation; and
- the stable full test suite and output behavior remain unchanged.

These are software-correctness results only. They do not show that a longer k improves an assembly.

## Promotion blockers

Stable use requires a later ADR and coordinated versioned migration. At minimum it must address:

1. a new exact-count run magic/domain and 32-byte key record framing, with corruption tests;
2. graph, compaction, partition-prefix, transformation-digest, and memory-admission changes;
3. spool/configuration, schemas, unitig identity, GFA overlap, and golden-bundle compatibility;
4. whether Bloom v1 remains narrow or receives a new hash domain and golden vectors;
5. deterministic Linux and Apple Silicon evidence across thread and spill layouts; and
6. frozen repeat datasets demonstrating a reconstruction benefit rather than merely accepting k=127.

ADR 0001 remains authoritative for the stable 0.1 k=3..63 `u128` engine. This ADR does not supersede
its stable format or product decision.

## Alternatives considered

- **Two `u128` limbs:** exact but gives 16-byte alignment and commonly increases surrounding record
  padding; four `u64` words retain a 32-byte key with 8-byte alignment.
- **A big-integer dependency:** unnecessary for fixed 254-bit shifts and comparisons, and would add
  maintenance, license, MSRV, and unsafe-dependency review surface.
- **Immediate whole-pipeline replacement:** rejected because a silent 16-to-32-byte substitution
  would violate existing format domains and make unchanged schema/version labels false.
- **Hash identity:** rejected because collisions cannot define an exact graph key.

## Consequences and limitations

The project now has an exact tested representation for wide-k research while 0.1 artifacts remain
byte-compatible. The scanner currently materializes every accepted key just like the narrow scanner;
it has not been profiled, externally counted, placed in a graph, or scientifically benchmarked. Its
duplicated QC helper logic is intentionally isolated and guarded by parity/oracle tests until a later
generic scanner refactor can prove unchanged stable output.
