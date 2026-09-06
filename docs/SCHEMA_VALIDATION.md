# Stable schema validation boundary

Status: independent development conformance check for the stable 1.2 bundle. It is not a JSON Schema
implementation certification or third-party format certification.

The production bundle writer validates typed runtime invariants before commit. The independent
integration test in `tests/stable_schema.rs` does not call that serializer or its validators. It:

- parses an emitted `run.json` against the emitted, byte-matched `schema/run.schema.json`;
- implements the exact draft-2020-12 keyword subset currently used by that schema, including local
  `$ref`, `const`, `enum`, types, numeric and string bounds, object membership, arrays, `oneOf`,
  `allOf`, `if`/`then`/`else`, `prefixItems`, and boolean schemas;
- rejects any unknown schema keyword or pattern instead of silently treating it as an annotation;
- explicitly recognizes the repository's current `x-*` annotations and enforces
  `x-maximum-decimal`; semantic checks independently enforce the relevant cross-field constraints;
- independently parses FASTA, the supported GFA 1.0 subset, and every stable TSV descriptor/data
  pair, then reconciles IDs, sequences, digests, counts, sort order, topology, links, audit states,
  transforms, and the scientific-artifact digest across files; and
- proves rejection of mutated constants/names, enums, bounds, missing and extra properties,
  conditional status/message pairs, NA-state contradictions, record order, and cross-artifact IDs.

This deliberately small validator has no runtime dependency and fails closed if the shipped schema
gains a keyword or regular-expression pattern that it does not implement. That behavior prevents a
schema expansion from acquiring a false green test. It does not implement the complete JSON Schema
draft, validate the schema against the official metaschema, establish compatibility with every JSON
Schema implementation, certify general FASTA/GFA interoperability, or replace an external parser in
release qualification. The custom `x-conditional-row-sets`, `x-sort-key`, and free-text
`x-invariant(s)` annotations are not a general executable constraint language; only the named stable
artifact semantics exercised by the independent cross-artifact checker are covered.
That named set includes proving that observed raw-transport bytes and gzip-member counts do not
exceed their recorded effective limits.

Before a supported release claim, repeat these tests from the exact clean source archive and add an
identified external draft-2020-12 validator and an independent GFA parser to the retained release
record. A CI configuration or passing local integration test is evidence only for the exact source,
toolchain, and command that ran.
