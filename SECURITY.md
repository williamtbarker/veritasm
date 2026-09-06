# Security and data handling

VeritAsm is development-stage research software. Version `0.4.0-dev.1` is the
current checkpoint; no supported production release is available.

| Version | Status |
|---|---|
| `0.4.0-dev.1` | Development checkpoint; no support commitment |
| Earlier ancestor versions | Outside this project's support policy |

## Reporting a vulnerability

Do not attach private sequence data, raw headers, filesystem paths, credentials, or an unreduced
dataset to a public issue. Use GitHub's **Security > Advisories > Report a vulnerability**
option if it is enabled. Otherwise, request a private reporting channel through a public issue
without including vulnerability details or sensitive data. Send the minimum synthetic reproducer
needed to establish the issue once a private channel is available.

Include, when safe:

- the exact source commit or source-archive SHA-256;
- operating system, architecture, Rust version, and filesystem type;
- the command with sensitive path components replaced consistently;
- the stable error code and sanitized stderr;
- expected and observed behavior; and
- a small synthetic reproducer plus its checksum.

Please report suspected output replacement, path traversal, symlink/hard-link confusion, parser or
decompressor denial of service, integer overflow, checksum bypass, HTML injection, unintended network
or process execution, and reproducibility/integrity failures as security-relevant.

## Sensitive-data boundary

Sequence records, qualities, identifiers, k-mers, fragment relationships, temporary count runs, and
assembled output may all be sensitive. Use an access-controlled local filesystem and review data-use
permissions before running the software or sharing a bundle.

Output contract v0.1 stores sanitized lane/role labels, counts, and content digests instead of raw
input paths or raw FASTX headers. A digest can still enable confirmation attacks when the candidate
input is known or low entropy; it is an integrity identifier, not anonymization. Output unitigs are
derived sequence and may reveal source material directly.

Temporary spools and exact count runs contain sample-derived data. The design requests owner-only
temporary directories and performs best-effort cleanup, but does not promise secure deletion. Copies
may persist on journaling, snapshotting, copy-on-write, network, backup, or flash filesystems. Put the
temporary and destination parents on storage appropriate for the data classification, and dispose of
the storage under local policy.

## Input and decompression boundary

Treat all FASTA, FASTQ, gzip, private spool, schema, and result-bundle bytes as untrusted. The design
requires incremental limits for headers, records, reads, decoded input, spool, temporary files,
retained keys, mapping candidates, and staged output. Corrupt or truncated gzip members and trailing
non-gzip bytes are intended to fail closed. These are implementation requirements, not evidence that
every malformed input has been handled safely; fuzzing and adversarial limit tests remain release
gates.

Compressed input can expand far beyond its transport size. Set `--max-decoded-input-bytes`,
`--max-spool-bytes`, `--max-temp-bytes`, and `--max-staged-output-bytes` for the available controlled
storage. A configured cap is a failure boundary, not a throughput or bounded-memory guarantee.

The release verifier separately rejects a source ZIP above 64 MiB and enforces central-directory
member, per-member, total declared-uncompressed-size, and expansion-ratio limits before integrity
decompression or extraction. Those checks reduce accidental exhaustion and ordinary ZIP-bomb risk;
they do not sandbox the decompressor, authenticate an untrusted sidecar, or bound a forged local
header's CPU behavior. Obtain the checksum through a trusted channel and run verification under the
host's normal untrusted-code controls.

## Filesystem and output boundary

Fixed-k assembly contract v0.1 is designed to:

- reject reuse of one physical file in multiple logical input roles;
- check lexical paths and Unix device/inode identity for aliases;
- write beneath a same-filesystem sibling staging directory;
- validate artifacts and checksums before commit;
- use a no-replace directory rename rather than an overwriting fallback; and
- leave any pre-existing destination unchanged on a pre-commit failure.

The destination must not exist and there is no overwrite option. A successful rename provides an
atomic-visibility/no-replacement boundary. Local Linux checks and a maintainer-reported macOS
verification run passed; see [current verification scope](docs/STATUS.md). This is not a guarantee of persistence
through power loss. Lock cleanup and parent-directory synchronization after
the commit point are best effort. Lock initialization failures are guarded from the instant of
exclusive creation, and cleanup verifies the open lock file's Unix device/inode identity before
unlinking its pathname. Existing locks are preserved for manual review; PID-only stale-lock recovery
is not considered safe. The identity check and unlink are not atomic against a malicious namespace
writer. Do not place the output under a directory writable by untrusted users, and do not assume the
cooperative lock defends against every privileged or malicious process.

`manifest.sha256`, spool digests, and probabilistic-state checksums detect accidental changes under
the stated local model. They are not digital signatures or authentication: an attacker able to
replace both data and hashes can forge them. Authenticate distributed archives through a separately
trusted channel. The public bundle verifier anchors an opened directory descriptor, uses descriptor-
relative no-follow/nonblocking opens, requires exact opened descriptors to be regular files, hashes
those descriptors directly, and checks device/inode/length/mtime/ctime before and after reads. It
therefore fails closed on ordinary symbolic-link, FIFO, non-regular-entry, and concurrent-substitution
attempts during verification. It does not freeze a mutable namespace or protect pathnames after it
returns; verify in a quiescent trusted directory or on an immutable filesystem snapshot. A privileged
writer capable of changing content and restoring all observed metadata remains outside this model.

## Report and browser boundary

`report.html` is intended to contain no remote scripts, images, fonts, telemetry, or runtime fetches
and to escape untrusted fields. It is a derived view, not the authoritative evidence store. Open
reports in an environment suitable for the sensitivity of their sequence content. Report any active
content, external request, or escaping failure.

## Runtime and dependency boundary

The core executable is designed with no network client, dynamic plugin loading, shell invocation,
telemetry, runtime database download, or external-process dependency. Building and auditing can access
Cargo registries and advisory databases; that development-time access is distinct from runtime.

The root and fuzz Cargo manifests forbid `unsafe` Rust in project targets. Dependencies may contain `unsafe`, build scripts, or
platform-specific code and require a separately retained source/license/advisory review. A lockfile,
`cargo audit`, or `cargo deny` result is one input to that review and is not by itself proof of safety.

## Scientific and operational misuse

Incorrect reconstruction, false junctions, incomplete assembly, and apparently exact read-back
evidence can cause consequential interpretation errors even when no conventional software exploit is
present. VeritAsm does not perform organism detection or establish presence, absence, identity,
viability, infectivity, sterility, product safety, or fitness for release. Independently validate any
use beyond software-method research, and retain negative controls, positive controls, raw reads,
parameters, complete result bundles, and failures.
