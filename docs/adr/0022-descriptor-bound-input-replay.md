# ADR 0022: descriptor-bound input and spool replay

- Status: Accepted for stable input integrity hardening
- Date: 2026-09-05
- Scope: decoded FASTX staging, `FastxReader`, `Spool::iter`, and parser admission

## Context

Input transport bytes are staged and hashed before parsing, and completed spools are hashed before
iteration. Two check/use windows remained:

- a decoded staging file could change after its logical digest was recorded but before or during
  FASTX parsing; and
- `Spool::iter` verified by path, then reopened the path and returned records without authenticating
  the descriptor actually being traversed or checking its trailer at iterator completion.

A same-length, structurally valid mutation could therefore make downstream temporary state from bytes
that differed from the recorded digest. The final result directory remained transactional, but its
provenance contract did not independently force these replays to match their registered bytes.

Parser admission also omitted each fixed 64 KiB reader buffer from the concurrent parser phase and
did not model old-plus-new vector storage during a growing sequence or quality reallocation.

## Decision

1. A decoded source is opened once for one parse pass. That descriptor's length and byte stream are
   bound to the registered logical decoded length and framed SHA-256. End of FASTX is successful only
   after exact length, digest, and EOF authentication.
2. `Spool::iter` opens one descriptor, verifies the registered file identity/length and complete
   spool on that descriptor, rewinds that same descriptor, and incrementally authenticates the bytes
   consumed by iteration. It validates header, descriptors, fragment order, trailer, exact EOF,
   pretrailer digest, and whole-file digest before returning terminal success.
3. Internal consumers must drive the iterator to authenticated completion. They may create only
   private, uncommitted intermediate state before terminal authentication; no bundle can commit from
   a partial or failed traversal. Public documentation does not imply that an early-stopped iterator
   authenticated unread bytes.
4. Prepared decoded staging is made read-only where the platform supports it. Permissions are defense
   in depth; descriptor-bound hashing is the integrity control.
5. Parser-phase admission subtracts every simultaneously live fixed reader/writer buffer before
   allocating record payload. Growing vectors charge their existing capacity plus a conservative
   candidate replacement capacity and source-line overlap before reserve, then validate the retained
   capacity after reserve. The contract remains a conservative owned-allocation estimate, not an RSS
   limit.
6. The multi-lane input set is not called a point-in-time filesystem snapshot. Each role is bound to
   the bytes actually staged for that role, and all such identities are committed to the result.

## Required falsification tests

- Mutate a decoded staging file after preparation and before parsing; the parse pass must fail and no
  completed spool may be returned.
- Mutate a completed spool after `iter()` construction, before the first fragment, between fragments,
  and before terminal completion. Every changed byte, trailer field, truncation, and extension must
  prevent authenticated completion and final output.
- Replace a path after the iterator owns its descriptor; parsing may use the original descriptor but
  must never adopt the replacement.
- An early-stopped iterator has no complete-authentication state.
- SE and PE parser buffer counts and sequence/quality reallocation peaks have limit-minus-one,
  exact-limit, and limit-plus-one regressions.
- Existing valid plain/gzip, synchronized pair, multi-lane, and deterministic-bundle tests remain
  byte-identical.

## Consequences

Input and audit passes perform additional hashing, and the usable record-payload share is slightly
smaller because fixed buffers are now charged. A detected mutation may have produced private spool or
count bytes before the terminal mismatch is known; those bytes are ineligible for commit and are
cleaned with the run workspace. A writer able to change and restore unread bytes between both exact
observations remains outside the stated threat model.

## Rejected alternatives

- Rely only on mode `0400` and a private directory: the process owner and storage faults are not
  cryptographically constrained by permissions.
- Verify a path and reopen it for use: pathname verification does not bind the later descriptor.
- Trust structural parsing: valid FASTX or spool structure does not prove byte identity.
- Retain the old parser cap wording: fixed buffers and reallocation peaks are real owned payload.
