# ADR 0017: Acquire an early run lease and bound transport work

- Status: accepted for v0.4 operational hardening
- Date: 2026-09-05

## Context

The result writer uses an owner-restricted sibling lock and a same-filesystem no-replace directory
rename. Those are strong publication boundaries. The lock is currently acquired only after the
entire assembly, so a locked destination can consume stdin and duplicate hours of work before
failing. Decoded-byte limits also do not bound raw transport size or a storm of empty gzip members.
Schema checks rerender stable JSON through the same serializer rather than validating it against the
shipped schema.

## Decision

1. Normalize and reject an existing destination, then acquire a `RunLease` before opening or reading
   any source. Retain that exact file object through staging and no-replace commit.
2. The bundle writer consumes the existing lease rather than acquiring a second late lock. Public
   library helpers may retain a convenience path that acquires their own lease immediately. ADR 0027
   later closes that option for stable evidence-bearing bundles: construction and writing are
   crate-internal, while manifest verification remains public.
3. Failure before commit releases only the exact owned lock file. Replaced or unverifiable lock paths
   remain for manual review. Stale-lock deletion is never inferred from a PID alone.
4. Add explicit aggregate raw-transport and gzip-member limits. File metadata may fail early, but the
   counted reader remains authoritative for stdin, mutation, and compressed input.
5. Record effective limits and observed raw bytes/member counts in the evidence bundle. Exceeding a
   limit is a typed failure before output commit.
6. Introduce admitted buffered I/O without changing scientific artifact bytes, hashes, ordering, or
   no-replace semantics.
7. Validate stable output fixtures against the embedded JSON Schema using an independent development
   implementation and negative mutations. Runtime typed invariants remain defense in depth.
8. Fuzz producer-success states as contracts: a successful spool must verify and fully iterate; a
   successful bundle must verify; an error must not leave a normal committed destination.

## Required tests

- Preexisting lock causes failure before stdin consumption and before a work directory is created.
- Two complete pipelines targeting one destination cannot both consume work under a cooperative lock,
  and exactly one can commit.
- Raw limit minus one, exact limit, and limit plus one for files, stdin, plain, and gzip input.
- Empty gzip-member storm, concatenated valid members, corrupt/truncated members, large metadata, and
  non-gzip trailing bytes.
- Buffered and oracle renderers emit identical bytes for every artifact; write/flush/sync failures
  retain their typed classification.
- Stable JSON accepts against the shipped schema; mutated name, enum, bound, missing/extra field, NA
  state, order, or cross-artifact ID fails the relevant independent checker.

## Non-goals

This ADR does not promise power-loss durability, automatic resume, automatic stale-lock cleanup, or a
whole-process CPU/time limit. Those require separate execution provenance and operational policy.
