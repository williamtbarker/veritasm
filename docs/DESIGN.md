# Historical design note

Status: **superseded; not a VeritAsm behavior or roadmap specification**.

This path was inherited from the frozen Virustic2 source tree. Its former contents described
Virustic2's viral-specific product framing, `u64` k-mers, deleting graph transformations, per-file
outputs, and other behavior that conflicts with the current VeritAsm contract. Keeping that text in
the review package without a hard boundary made it too easy to mistake ancestor design for current
behavior.

Use these documents for the current system:

- [`../ARCHITECTURE.md`](../ARCHITECTURE.md) — architecture and invariants;
- [`PRODUCT_CONTRACT.md`](PRODUCT_CONTRACT.md) — supported behavior and explicit non-goals;
- [`CONFIGURATION.md`](CONFIGURATION.md) — exact configuration and failure semantics;
- [`OUTPUT_SCHEMA.md`](OUTPUT_SCHEMA.md) — output meanings and schemas; and
- [`BASELINE_AUDIT.md`](BASELINE_AUDIT.md) — the retained, evidence-bounded Virustic2 comparison.

The unmodified Virustic2 checkout and commit recorded in `BASELINE_AUDIT.md`, not this compatibility
notice, are the historical source of truth for ancestor behavior.
