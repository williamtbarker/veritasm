//! Exact, deterministic minimizer partitioning for canonical de Bruijn edges.
//!
//! This is an isolated correctness substrate, not the stable assembler data
//! plane. It consumes segments that have already passed parsing, ambiguity,
//! and quality filtering. Every supplied byte must be A/C/G/T (case
//! insensitive); ambiguity is rejected instead of being assigned to a bucket.
//!
//! Each k-mer occurrence is represented by its complete canonical
//! [`PackedKmer`]. A complete canonical m-mer chooses its minimizer owner, and
//! the exact numeric residue of that owner chooses a virtual bucket. Different
//! minimizers may deliberately share a bucket, but equality and counting use
//! the complete k-mer only. No hash or finite fingerprint defines identity.

use super::wide_kmer::{
    canonical_code, decode_mer, encode_exact_bases, scan_canonical_kmers, validate_code,
    validate_k, PackedKmer,
};
use crate::error::{ErrorCode, Result, VeritasmError};
use sha2::{Digest, Sha256};
use std::mem::size_of;

const ACCEPTED_SOURCE_DOMAIN: &[u8] = b"veritasm:experimental-accepted-segments:v1\0";
const ACCEPTED_SEGMENT_DOMAIN: &[u8] = b"veritasm:experimental-accepted-segment:v1\0";

/// One exact post-QC sequence segment and its immutable provenance.
///
/// Callers must split reads at every ambiguity or rejected-quality boundary
/// before constructing this value. Empty and shorter-than-k segments are
/// retained in the coverage ledger but contribute no graph edge.
#[derive(Debug, Clone, Copy)]
pub struct AcceptedSegment<'a> {
    pub source_ordinal: u64,
    pub segment_ordinal: u64,
    pub bases: &'a [u8],
}

/// Explicit resource limits for one in-memory partitioning experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionLimits {
    pub max_input_bases: u64,
    pub max_windows: u64,
    pub max_super_kmers: u64,
    pub max_distinct_kmers: u64,
    /// Maximum projected owned payload, excluding allocator metadata.
    ///
    /// This is an admission bound for the vectors owned or transiently
    /// materialized by this module. It is not a whole-process RSS guarantee.
    pub max_accounted_bytes: u64,
}

/// Frozen parameters for exact virtual partitioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionConfig {
    pub k: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub limits: PartitionLimits,
}

/// The exact minimizer selected for a canonical k-mer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimizerOwner {
    /// Complete canonical m-mer; never a hash or fingerprint.
    pub key: PackedKmer,
    /// Leftmost zero-based start in the canonical spelling of the k-mer.
    pub leftmost_position: u8,
}

/// One exact canonical de Bruijn-edge count.
///
/// Rows are ordered by `(bucket_id, key)`. The complete owner is retained so
/// intentional virtual-bucket collisions remain visible and auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactEdgeCount {
    pub bucket_id: u32,
    pub minimizer: PackedKmer,
    pub key: PackedKmer,
    pub occurrences: u64,
}

/// One source-bound occurrence proof in increasing source/window order.
///
/// This intentionally duplicates occurrence-level information in the
/// in-memory oracle. It lets validation bind every aggregate edge count and
/// every super-k-mer span to one independently decoded source byte slice.
/// The external engine uses authenticated run records instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceWindowProof {
    pub source_ordinal: u64,
    pub segment_ordinal: u64,
    pub window_ordinal: u64,
    pub bucket_id: u32,
    pub minimizer: PackedKmer,
    pub key: PackedKmer,
}

/// A maximal run of consecutive source windows with one exact minimizer.
///
/// The represented source substring starts at `first_window` and has length
/// `k + window_count - 1`. Coordinates, rather than copied bases, keep this
/// proof substrate bounded; a future spill format must retain authenticated
/// source identity before these records can cross a process boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperKmerSpan {
    pub source_ordinal: u64,
    pub segment_ordinal: u64,
    pub first_window: u64,
    pub window_count: u64,
    pub bucket_id: u32,
    pub minimizer: PackedKmer,
}

/// Coverage metadata that proves super-k-mer spans partition each segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentPartition {
    pub source_ordinal: u64,
    pub segment_ordinal: u64,
    pub input_bases: u64,
    pub window_count: u64,
    /// SHA-256 of normalized exact bases and immutable segment coordinates.
    pub source_sha256: [u8; 32],
    pub first_window_proof: u64,
    pub first_super_kmer: u64,
    pub super_kmer_count: u64,
}

/// One nonempty virtual-bucket range in [`PartitionedDbg::edge_counts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketRange {
    pub bucket_id: u32,
    pub first_edge_count: u64,
    pub distinct_edge_count: u64,
    pub occurrence_count: u64,
}

/// Deterministic in-memory result of the experimental data plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionedDbg {
    pub k: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub input_bases: u64,
    pub accepted_windows: u64,
    /// SHA-256 over the complete sorted accepted-segment stream.
    pub source_identity: [u8; 32],
    /// Conservative requested vector payload at admission time.
    pub projected_payload_bytes: u64,
    pub segments: Vec<SegmentPartition>,
    pub window_proofs: Vec<SourceWindowProof>,
    pub super_kmers: Vec<SuperKmerSpan>,
    pub buckets: Vec<BucketRange>,
    pub edge_counts: Vec<ExactEdgeCount>,
}

/// Select the exact minimizer of a k-mer under the frozen experimental order.
///
/// The supplied k-mer is canonicalized first. Every length-m substring of
/// that canonical spelling is itself canonicalized; the numerically smallest
/// complete packed m-mer wins, with the leftmost position breaking equal-key
/// ties. Consequently a k-mer and its reverse complement always have the same
/// owner.
pub fn select_minimizer(kmer: PackedKmer, k: u8, minimizer_length: u8) -> Result<MinimizerOwner> {
    validate_parameters(k, minimizer_length, 1)?;
    validate_code(kmer, k)?;
    let canonical = canonical_code(kmer, k)?;
    let sequence = decode_mer(canonical, k)?;
    select_minimizer_from_canonical_bases(&sequence, minimizer_length)
}

/// Route a complete exact minimizer to a virtual bucket.
///
/// Routing is the unsigned 256-bit value modulo `virtual_bucket_count`. This
/// is exact portable arithmetic, not a hash. A residue collision merely puts
/// multiple complete minimizers in the same bucket.
pub fn route_minimizer(
    minimizer: PackedKmer,
    minimizer_length: u8,
    virtual_bucket_count: u32,
) -> Result<u32> {
    if minimizer_length == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            "experimental minimizer length must be at least one",
        ));
    }
    validate_code(minimizer, minimizer_length)?;
    if virtual_bucket_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "experimental virtual bucket count must be at least one",
        ));
    }

    let modulus = u128::from(virtual_bucket_count);
    let mut remainder = 0_u128;
    for word in minimizer.words() {
        remainder = ((remainder << 64) | u128::from(word)) % modulus;
    }
    u32::try_from(remainder).map_err(|_| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "experimental virtual-bucket residue did not fit in u32",
        )
    })
}

/// Compute the immutable identity used by the experimental external runs.
///
/// Segment order supplied by the caller is irrelevant. Bases are normalized
/// to uppercase because [`AcceptedSegment`] has case-insensitive DNA
/// semantics. Duplicate source coordinates and non-ACGT bytes are rejected.
pub fn accepted_segments_source_identity(segments: &[AcceptedSegment<'_>]) -> Result<[u8; 32]> {
    let order = sorted_segment_order(segments)?;
    let mut digest = Sha256::new();
    digest.update(ACCEPTED_SOURCE_DOMAIN);
    digest.update(
        u64::try_from(order.len())
            .map_err(|_| overflow("experimental segment count does not fit in u64"))?
            .to_le_bytes(),
    );
    for index in order {
        let segment = segments[index];
        validate_accepted_bases(segment)?;
        digest.update(segment_source_sha256(segment)?);
    }
    Ok(digest.finalize().into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndependentWindow {
    pub key: PackedKmer,
    pub owner: MinimizerOwner,
    pub bucket_id: u32,
}

/// Deliberately slow byte-slice oracle independent of the rolling scanner.
///
/// Canonical spelling and every minimizer candidate are selected by literal
/// uppercase byte comparison before the selected strings are packed. This is
/// used to falsify rolling-key, owner, and source-window bookkeeping defects;
/// it is not intended to be the optimized production selector.
pub(crate) fn independent_window_oracle(
    window: &[u8],
    k: u8,
    minimizer_length: u8,
    virtual_bucket_count: u32,
) -> Result<IndependentWindow> {
    validate_parameters(k, minimizer_length, virtual_bucket_count)?;
    if window.len() != usize::from(k) {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!(
                "independent experimental window length {} differs from k {k}",
                window.len()
            ),
        ));
    }
    let canonical = canonical_bytes(window)?;
    let key = encode_exact_bases(&canonical)?;
    let m = usize::from(minimizer_length);
    let mut best: Option<(Vec<u8>, u8)> = None;
    for (position, candidate) in canonical.windows(m).enumerate() {
        let candidate = canonical_bytes(candidate)?;
        let position = u8::try_from(position)
            .map_err(|_| overflow("independent minimizer position does not fit in u8"))?;
        if best
            .as_ref()
            .is_none_or(|(current, _)| candidate < *current)
        {
            best = Some((candidate, position));
        }
    }
    let (owner_bases, leftmost_position) = best.ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "independent minimizer oracle produced no candidate",
        )
    })?;
    let owner = MinimizerOwner {
        key: encode_exact_bases(&owner_bases)?,
        leftmost_position,
    };
    let bucket_id = route_minimizer(owner.key, minimizer_length, virtual_bucket_count)?;
    Ok(IndependentWindow {
        key,
        owner,
        bucket_id,
    })
}

/// Partition and exactly count post-QC A/C/G/T segments.
///
/// Input order is not semantically significant: segments are processed by
/// `(source_ordinal, segment_ordinal)`. Duplicate provenance coordinates are
/// rejected. The current implementation is serial and entirely in memory;
/// it is an oracle and format-design substrate for a future bounded spill and
/// merge implementation.
pub fn build_partitioned_dbg(
    config: PartitionConfig,
    segments: &[AcceptedSegment<'_>],
) -> Result<PartitionedDbg> {
    validate_parameters(
        config.k,
        config.minimizer_length,
        config.virtual_bucket_count,
    )?;

    let mut input_bases = 0_u64;
    let mut accepted_windows = 0_u64;
    let mut maximum_segment_windows = 0_u64;
    let k_usize = usize::from(config.k);

    for &segment in segments {
        validate_accepted_bases(segment)?;
        let base_count = u64::try_from(segment.bases.len())
            .map_err(|_| overflow("experimental segment length does not fit in u64"))?;
        input_bases = checked_add(
            input_bases,
            base_count,
            "experimental input-base count overflow",
        )?;
        let windows = if segment.bases.len() < k_usize {
            0
        } else {
            u64::try_from(segment.bases.len() - k_usize + 1)
                .map_err(|_| overflow("experimental segment window count does not fit in u64"))?
        };
        accepted_windows = checked_add(
            accepted_windows,
            windows,
            "experimental accepted-window count overflow",
        )?;
        maximum_segment_windows = maximum_segment_windows.max(windows);
    }

    enforce_limit(
        input_bases,
        config.limits.max_input_bases,
        ErrorCode::ResourceMemory,
        "experimental input bases",
    )?;
    enforce_limit(
        accepted_windows,
        config.limits.max_windows,
        ErrorCode::ResourceRetainedKeys,
        "experimental accepted windows",
    )?;

    let super_capacity = accepted_windows.min(config.limits.max_super_kmers);
    let possible_buckets = accepted_windows
        .min(config.limits.max_distinct_kmers)
        .min(u64::from(config.virtual_bucket_count));
    let projected_payload_bytes = projected_payload_bytes(
        segments.len(),
        accepted_windows,
        maximum_segment_windows,
        super_capacity,
        possible_buckets,
        config.k,
    )?;
    enforce_limit(
        projected_payload_bytes,
        config.limits.max_accounted_bytes,
        ErrorCode::ResourceMemory,
        "experimental projected owned payload bytes",
    )?;

    let source_identity = accepted_segments_source_identity(segments)?;
    let order = sorted_segment_order(segments)?;

    let window_capacity = as_usize(accepted_windows, "experimental window capacity")?;
    let super_capacity = as_usize(super_capacity, "experimental super-k-mer capacity")?;
    let bucket_capacity = as_usize(possible_buckets, "experimental bucket capacity")?;
    let mut edge_counts = Vec::new();
    edge_counts
        .try_reserve_exact(window_capacity)
        .map_err(|cause| {
            resource_memory(format!(
                "cannot reserve experimental exact-edge observations: {cause}"
            ))
        })?;
    let mut window_proofs = Vec::new();
    window_proofs
        .try_reserve_exact(window_capacity)
        .map_err(|cause| {
            resource_memory(format!(
                "cannot reserve experimental source-window proofs: {cause}"
            ))
        })?;
    let mut super_kmers = Vec::new();
    super_kmers
        .try_reserve_exact(super_capacity)
        .map_err(|cause| {
            resource_memory(format!(
                "cannot reserve experimental super-k-mer spans: {cause}"
            ))
        })?;
    let mut segment_partitions = Vec::new();
    segment_partitions
        .try_reserve_exact(segments.len())
        .map_err(|cause| {
            resource_memory(format!(
                "cannot reserve experimental segment coverage ledger: {cause}"
            ))
        })?;

    for &index in &order {
        let segment = segments[index];
        let input_length = u64::try_from(segment.bases.len())
            .map_err(|_| overflow("experimental segment length does not fit in u64"))?;
        let scan = scan_canonical_kmers(segment.bases, None, config.k, 0)?;
        let first_window_proof = u64::try_from(window_proofs.len())
            .map_err(|_| overflow("experimental window-proof index does not fit in u64"))?;
        let first_super_kmer = u64::try_from(super_kmers.len())
            .map_err(|_| overflow("experimental super-k-mer index does not fit in u64"))?;

        for (position, key) in scan.kmers.into_iter().enumerate() {
            let end = position
                .checked_add(k_usize)
                .ok_or_else(|| overflow("experimental source-window end overflow"))?;
            let window_bases = segment.bases.get(position..end).ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "rolling experimental scan produced a window outside its source segment",
                )
            })?;
            let independent = independent_window_oracle(
                window_bases,
                config.k,
                config.minimizer_length,
                config.virtual_bucket_count,
            )?;
            if key != independent.key {
                return invariant(
                    "rolling experimental key disagrees with independent source-window oracle",
                );
            }
            let owner = independent.owner;
            let bucket_id = independent.bucket_id;
            let window = u64::try_from(position)
                .map_err(|_| overflow("experimental window position does not fit in u64"))?;
            window_proofs.push(SourceWindowProof {
                source_ordinal: segment.source_ordinal,
                segment_ordinal: segment.segment_ordinal,
                window_ordinal: window,
                bucket_id,
                minimizer: owner.key,
                key,
            });
            edge_counts.push(ExactEdgeCount {
                bucket_id,
                minimizer: owner.key,
                key,
                occurrences: 1,
            });

            let extends_last = super_kmers.last().is_some_and(|last: &SuperKmerSpan| {
                last.source_ordinal == segment.source_ordinal
                    && last.segment_ordinal == segment.segment_ordinal
                    && last.minimizer == owner.key
                    && last.bucket_id == bucket_id
                    && last.first_window.checked_add(last.window_count) == Some(window)
            });
            if extends_last {
                let last = super_kmers.last_mut().ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "experimental super-k-mer extension lost its preceding span",
                    )
                })?;
                last.window_count = checked_add(
                    last.window_count,
                    1,
                    "experimental super-k-mer window count overflow",
                )?;
            } else {
                let current = u64::try_from(super_kmers.len())
                    .map_err(|_| overflow("experimental super-k-mer count does not fit in u64"))?;
                if current >= config.limits.max_super_kmers {
                    return Err(VeritasmError::new(
                        ErrorCode::ResourceRetainedKeys,
                        format!(
                            "experimental super-k-mer count would exceed configured limit {}",
                            config.limits.max_super_kmers
                        ),
                    ));
                }
                super_kmers.push(SuperKmerSpan {
                    source_ordinal: segment.source_ordinal,
                    segment_ordinal: segment.segment_ordinal,
                    first_window: window,
                    window_count: 1,
                    bucket_id,
                    minimizer: owner.key,
                });
            }
        }

        let super_kmer_count = u64::try_from(super_kmers.len())
            .map_err(|_| overflow("experimental super-k-mer count does not fit in u64"))?
            .checked_sub(first_super_kmer)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "experimental super-k-mer segment range moved backwards",
                )
            })?;
        segment_partitions.push(SegmentPartition {
            source_ordinal: segment.source_ordinal,
            segment_ordinal: segment.segment_ordinal,
            input_bases: input_length,
            window_count: scan.stats.accepted,
            source_sha256: segment_source_sha256(segment)?,
            first_window_proof,
            first_super_kmer,
            super_kmer_count,
        });
    }

    let materialized_windows = u64::try_from(edge_counts.len())
        .map_err(|_| overflow("experimental materialized-window count does not fit in u64"))?;
    if materialized_windows != accepted_windows {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!(
                "experimental partition materialized {materialized_windows} windows, expected {accepted_windows}"
            ),
        ));
    }

    edge_counts.sort_unstable_by(|left, right| {
        (left.bucket_id, left.key).cmp(&(right.bucket_id, right.key))
    });
    coalesce_exact_counts(&mut edge_counts)?;
    let distinct_count = u64::try_from(edge_counts.len())
        .map_err(|_| overflow("experimental distinct-edge count does not fit in u64"))?;
    enforce_limit(
        distinct_count,
        config.limits.max_distinct_kmers,
        ErrorCode::ResourceRetainedKeys,
        "experimental distinct canonical k-mers",
    )?;

    let mut buckets = Vec::new();
    buckets
        .try_reserve_exact(bucket_capacity)
        .map_err(|cause| {
            resource_memory(format!(
                "cannot reserve experimental virtual-bucket ranges: {cause}"
            ))
        })?;
    build_bucket_ranges(&edge_counts, &mut buckets)?;

    let result = PartitionedDbg {
        k: config.k,
        minimizer_length: config.minimizer_length,
        virtual_bucket_count: config.virtual_bucket_count,
        input_bases,
        accepted_windows,
        source_identity,
        projected_payload_bytes,
        segments: segment_partitions,
        window_proofs,
        super_kmers,
        buckets,
        edge_counts,
    };
    result.validate_invariants()?;
    result.validate_against_segments(segments)?;
    Ok(result)
}

impl PartitionedDbg {
    /// Recheck conservation, ordering, exact ownership, and bucket routing.
    pub fn validate_invariants(&self) -> Result<()> {
        validate_parameters(self.k, self.minimizer_length, self.virtual_bucket_count)?;

        let mut expected_first_super = 0_u64;
        let mut expected_first_proof = 0_u64;
        let mut previous_segment = None;
        let mut covered_windows = 0_u64;
        let mut covered_bases = 0_u64;
        for segment in &self.segments {
            let coordinate = (segment.source_ordinal, segment.segment_ordinal);
            if previous_segment.is_some_and(|previous| previous >= coordinate) {
                return invariant("experimental segment ledger is not strictly ordered");
            }
            previous_segment = Some(coordinate);
            if segment.first_super_kmer != expected_first_super {
                return invariant("experimental segment ledger has a noncontiguous span range");
            }
            if segment.first_window_proof != expected_first_proof {
                return invariant(
                    "experimental segment ledger has a noncontiguous source-window proof range",
                );
            }
            let end = segment
                .first_super_kmer
                .checked_add(segment.super_kmer_count)
                .ok_or_else(|| overflow("experimental segment span range overflow"))?;
            let start_index = as_usize(
                segment.first_super_kmer,
                "experimental segment first-super index",
            )?;
            let end_index = as_usize(end, "experimental segment end-super index")?;
            let spans = self
                .super_kmers
                .get(start_index..end_index)
                .ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "experimental segment ledger points outside super-k-mer storage",
                    )
                })?;
            let proof_end = segment
                .first_window_proof
                .checked_add(segment.window_count)
                .ok_or_else(|| overflow("experimental segment proof range overflow"))?;
            let proof_start_index = as_usize(
                segment.first_window_proof,
                "experimental segment first-proof index",
            )?;
            let proof_end_index = as_usize(proof_end, "experimental segment end-proof index")?;
            let proofs = self
                .window_proofs
                .get(proof_start_index..proof_end_index)
                .ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "experimental segment ledger points outside source-window proofs",
                    )
                })?;
            let mut next_window = 0_u64;
            let mut previous_minimizer = None;
            for span in spans {
                if span.source_ordinal != segment.source_ordinal
                    || span.segment_ordinal != segment.segment_ordinal
                    || span.first_window != next_window
                    || span.window_count == 0
                {
                    return invariant(
                        "experimental super-k-mer spans do not exactly partition their segment",
                    );
                }
                if previous_minimizer == Some(span.minimizer) {
                    return invariant(
                        "adjacent experimental super-k-mer spans with one minimizer are not maximal",
                    );
                }
                previous_minimizer = Some(span.minimizer);
                let routed = route_minimizer(
                    span.minimizer,
                    self.minimizer_length,
                    self.virtual_bucket_count,
                )?;
                if routed != span.bucket_id {
                    return invariant("experimental super-k-mer has an invalid bucket route");
                }
                next_window = checked_add(
                    next_window,
                    span.window_count,
                    "experimental super-k-mer coverage overflow",
                )?;
            }
            if next_window != segment.window_count {
                return invariant(
                    "experimental super-k-mer spans lose or duplicate source windows",
                );
            }
            let expected_windows = segment
                .input_bases
                .checked_sub(u64::from(self.k))
                .and_then(|difference| difference.checked_add(1))
                .unwrap_or(0);
            if segment.window_count != expected_windows {
                return invariant(
                    "experimental segment window count disagrees with its input length",
                );
            }
            let _span_advances = validate_proof_span_ownership(
                proofs,
                spans,
                segment.source_ordinal,
                segment.segment_ordinal,
                self.k,
                self.minimizer_length,
                self.virtual_bucket_count,
            )?;
            covered_windows = checked_add(
                covered_windows,
                segment.window_count,
                "experimental segment coverage total overflow",
            )?;
            covered_bases = checked_add(
                covered_bases,
                segment.input_bases,
                "experimental segment input-base total overflow",
            )?;
            expected_first_super = end;
            expected_first_proof = proof_end;
        }
        if as_usize(expected_first_super, "experimental total super-k-mer count")?
            != self.super_kmers.len()
        {
            return invariant("experimental super-k-mer storage has unowned trailing spans");
        }
        if as_usize(
            expected_first_proof,
            "experimental total source-window proof count",
        )? != self.window_proofs.len()
        {
            return invariant("experimental source-window proof storage has unowned trailing rows");
        }
        if covered_windows != self.accepted_windows {
            return invariant("experimental segment coverage does not equal accepted windows");
        }
        if covered_bases != self.input_bases {
            return invariant("experimental segment ledger does not conserve input bases");
        }

        let mut occurrence_total = 0_u64;
        let mut previous_count: Option<(u32, PackedKmer)> = None;
        for count in &self.edge_counts {
            if count.occurrences == 0 {
                return invariant("experimental exact edge has zero occurrences");
            }
            let order_key = (count.bucket_id, count.key);
            if previous_count.is_some_and(|previous| previous >= order_key) {
                return invariant("experimental exact-edge rows are not strictly ordered");
            }
            previous_count = Some(order_key);
            validate_canonical_key(count.key, self.k)?;
            let owner = select_minimizer(count.key, self.k, self.minimizer_length)?;
            if owner.key != count.minimizer {
                return invariant("experimental exact edge has the wrong complete minimizer");
            }
            if route_minimizer(
                count.minimizer,
                self.minimizer_length,
                self.virtual_bucket_count,
            )? != count.bucket_id
            {
                return invariant("experimental exact edge has the wrong virtual bucket");
            }
            occurrence_total = checked_add(
                occurrence_total,
                count.occurrences,
                "experimental exact-edge occurrence total overflow",
            )?;
        }
        if occurrence_total != self.accepted_windows {
            return invariant("experimental exact-edge counts do not conserve accepted windows");
        }

        let mut proof_counts = Vec::new();
        proof_counts
            .try_reserve_exact(self.window_proofs.len())
            .map_err(|cause| {
                resource_memory(format!(
                    "cannot reserve experimental proof-count validation buffer: {cause}"
                ))
            })?;
        proof_counts.extend(self.window_proofs.iter().map(|proof| ExactEdgeCount {
            bucket_id: proof.bucket_id,
            minimizer: proof.minimizer,
            key: proof.key,
            occurrences: 1,
        }));
        proof_counts.sort_unstable_by(|left, right| {
            (left.bucket_id, left.key).cmp(&(right.bucket_id, right.key))
        });
        coalesce_exact_counts(&mut proof_counts)?;
        if proof_counts != self.edge_counts {
            return invariant(
                "experimental exact-edge aggregates disagree with source-window proofs",
            );
        }

        let mut expected_first_count = 0_u64;
        let mut previous_bucket = None;
        let mut bucket_occurrences = 0_u64;
        for bucket in &self.buckets {
            if bucket.distinct_edge_count == 0
                || previous_bucket.is_some_and(|previous| previous >= bucket.bucket_id)
                || bucket.first_edge_count != expected_first_count
            {
                return invariant("experimental virtual-bucket ranges are not canonical");
            }
            previous_bucket = Some(bucket.bucket_id);
            let end = bucket
                .first_edge_count
                .checked_add(bucket.distinct_edge_count)
                .ok_or_else(|| overflow("experimental virtual-bucket range overflow"))?;
            let start_index = as_usize(
                bucket.first_edge_count,
                "experimental bucket first-edge index",
            )?;
            let end_index = as_usize(end, "experimental bucket end-edge index")?;
            let counts = self
                .edge_counts
                .get(start_index..end_index)
                .ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "experimental virtual-bucket range points outside exact-edge storage",
                    )
                })?;
            let mut observed = 0_u64;
            for count in counts {
                if count.bucket_id != bucket.bucket_id {
                    return invariant("experimental virtual-bucket range contains another bucket");
                }
                observed = checked_add(
                    observed,
                    count.occurrences,
                    "experimental bucket occurrence subtotal overflow",
                )?;
            }
            if observed != bucket.occurrence_count {
                return invariant("experimental virtual-bucket occurrence subtotal is wrong");
            }
            bucket_occurrences = checked_add(
                bucket_occurrences,
                observed,
                "experimental bucket occurrence total overflow",
            )?;
            expected_first_count = end;
        }
        if as_usize(
            expected_first_count,
            "experimental total bucketed exact-edge count",
        )? != self.edge_counts.len()
        {
            return invariant("experimental exact-edge rows are not fully bucketed");
        }
        if bucket_occurrences != self.accepted_windows {
            return invariant("experimental buckets do not conserve accepted windows");
        }
        Ok(())
    }

    /// Recompute every occurrence from immutable source bytes with the
    /// independent slice oracle.
    ///
    /// [`Self::validate_invariants`] proves self-consistency and rejects a
    /// noncanonical stored key. This stronger check additionally detects a
    /// coordinated mutation of proofs/spans/counts by binding them back to
    /// the caller's complete accepted-segment stream.
    pub fn validate_against_segments(&self, segments: &[AcceptedSegment<'_>]) -> Result<()> {
        self.validate_invariants()?;
        let actual_identity = accepted_segments_source_identity(segments)?;
        if actual_identity != self.source_identity {
            return Err(VeritasmError::new(
                ErrorCode::IntegritySpool,
                "experimental accepted-segment source identity mismatch",
            ));
        }
        let order = sorted_segment_order(segments)?;
        if order.len() != self.segments.len() {
            return invariant(
                "experimental source segment count disagrees with the partition ledger",
            );
        }
        let k = usize::from(self.k);
        for (&source_index, ledger) in order.iter().zip(&self.segments) {
            let source = segments[source_index];
            let input_bases = u64::try_from(source.bases.len())
                .map_err(|_| overflow("experimental segment length does not fit in u64"))?;
            if source.source_ordinal != ledger.source_ordinal
                || source.segment_ordinal != ledger.segment_ordinal
                || input_bases != ledger.input_bases
                || segment_source_sha256(source)? != ledger.source_sha256
            {
                return invariant(
                    "experimental partition ledger disagrees with its source segment",
                );
            }
            let proof_start = as_usize(
                ledger.first_window_proof,
                "experimental source-validation first-proof index",
            )?;
            let proof_end = ledger
                .first_window_proof
                .checked_add(ledger.window_count)
                .ok_or_else(|| overflow("experimental source-validation proof range overflow"))?;
            let proof_end = as_usize(proof_end, "experimental source-validation end-proof index")?;
            let proofs = self
                .window_proofs
                .get(proof_start..proof_end)
                .ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "experimental source-validation proof range is out of bounds",
                    )
                })?;
            for (position, proof) in proofs.iter().enumerate() {
                let end = position
                    .checked_add(k)
                    .ok_or_else(|| overflow("experimental source-validation window overflow"))?;
                let source_window = source.bases.get(position..end).ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "experimental source-validation window is out of bounds",
                    )
                })?;
                let expected = independent_window_oracle(
                    source_window,
                    self.k,
                    self.minimizer_length,
                    self.virtual_bucket_count,
                )?;
                if proof.key != expected.key
                    || proof.minimizer != expected.owner.key
                    || proof.bucket_id != expected.bucket_id
                {
                    return invariant(
                        "experimental source-window proof disagrees with independent byte-slice oracle",
                    );
                }
            }
        }
        Ok(())
    }
}

fn validate_proof_span_ownership(
    proofs: &[SourceWindowProof],
    spans: &[SuperKmerSpan],
    source_ordinal: u64,
    segment_ordinal: u64,
    k: u8,
    minimizer_length: u8,
    virtual_bucket_count: u32,
) -> Result<usize> {
    let mut span_index = 0_usize;
    let mut span_advances = 0_usize;
    for (window, proof) in proofs.iter().enumerate() {
        let window = u64::try_from(window)
            .map_err(|_| overflow("experimental proof window index does not fit in u64"))?;
        if proof.source_ordinal != source_ordinal
            || proof.segment_ordinal != segment_ordinal
            || proof.window_ordinal != window
        {
            return invariant("experimental source-window proof has the wrong source coordinate");
        }
        validate_canonical_key(proof.key, k)?;
        let owner = select_minimizer(proof.key, k, minimizer_length)?;
        if owner.key != proof.minimizer {
            return invariant("experimental source-window proof has the wrong complete minimizer");
        }
        if route_minimizer(proof.minimizer, minimizer_length, virtual_bucket_count)?
            != proof.bucket_id
        {
            return invariant("experimental source-window proof has the wrong virtual bucket");
        }

        loop {
            let span = spans.get(span_index).ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "experimental source-window proof is not owned by a span",
                )
            })?;
            let span_end = span
                .first_window
                .checked_add(span.window_count)
                .ok_or_else(|| overflow("experimental proof-owner span range overflow"))?;
            if proof.window_ordinal < span_end {
                if proof.window_ordinal < span.first_window {
                    return invariant("experimental source-window proof is not owned by a span");
                }
                if span.minimizer != proof.minimizer || span.bucket_id != proof.bucket_id {
                    return invariant(
                        "experimental super-k-mer span owner disagrees with its source windows",
                    );
                }
                break;
            }
            span_index = span_index
                .checked_add(1)
                .ok_or_else(|| overflow("experimental proof-owner span cursor overflow"))?;
            span_advances = span_advances
                .checked_add(1)
                .ok_or_else(|| overflow("experimental proof-owner span advance count overflow"))?;
        }
    }
    Ok(span_advances)
}

fn select_minimizer_from_canonical_bases(
    sequence: &[u8],
    minimizer_length: u8,
) -> Result<MinimizerOwner> {
    let m = usize::from(minimizer_length);
    if m == 0 || m > sequence.len() {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental minimizer length must be in 1..={}; received {minimizer_length}",
                sequence.len()
            ),
        ));
    }
    let mask = active_mask(minimizer_length);
    let reverse_offset = 2 * u16::from(minimizer_length - 1);
    let mut forward = PackedKmer::ZERO;
    let mut reverse = PackedKmer::ZERO;
    let mut best: Option<MinimizerOwner> = None;

    for (position, &base) in sequence.iter().enumerate() {
        let bits = exact_base_bits(base).ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "decoded experimental canonical k-mer contained a non-ACGT base",
            )
        })?;
        forward = packed_shift_left_two(forward, bits).masked(mask);
        reverse = packed_shift_right_two(reverse).with_pair(3 - bits, reverse_offset);
        if position + 1 < m {
            continue;
        }
        let key = forward.min(reverse);
        let start = u8::try_from(position + 1 - m)
            .map_err(|_| overflow("experimental minimizer position does not fit in u8"))?;
        if best.is_none_or(|current| key < current.key) {
            best = Some(MinimizerOwner {
                key,
                leftmost_position: start,
            });
        }
    }
    best.ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "experimental minimizer selection produced no candidate",
        )
    })
}

fn validate_parameters(k: u8, minimizer_length: u8, bucket_count: u32) -> Result<()> {
    validate_k(k)?;
    if minimizer_length == 0 || minimizer_length > k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental minimizer length must be in 1..={k}; received {minimizer_length}"
            ),
        ));
    }
    if bucket_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "experimental virtual bucket count must be at least one",
        ));
    }
    Ok(())
}

fn validate_canonical_key(key: PackedKmer, k: u8) -> Result<()> {
    validate_code(key, k)?;
    if canonical_code(key, k)? != key {
        return invariant("experimental edge key is not in canonical orientation");
    }
    Ok(())
}

fn sorted_segment_order(segments: &[AcceptedSegment<'_>]) -> Result<Vec<usize>> {
    let mut order = Vec::new();
    order.try_reserve_exact(segments.len()).map_err(|cause| {
        resource_memory(format!(
            "cannot reserve experimental segment-order index: {cause}"
        ))
    })?;
    order.extend(0..segments.len());
    order.sort_unstable_by_key(|&index| {
        (
            segments[index].source_ordinal,
            segments[index].segment_ordinal,
        )
    });
    let mut previous_coordinate = None;
    for &index in &order {
        let segment = segments[index];
        let coordinate = (segment.source_ordinal, segment.segment_ordinal);
        if previous_coordinate == Some(coordinate) {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                format!(
                    "duplicate experimental accepted-segment coordinate {}/{}",
                    coordinate.0, coordinate.1
                ),
            ));
        }
        previous_coordinate = Some(coordinate);
    }
    Ok(order)
}

fn segment_source_sha256(segment: AcceptedSegment<'_>) -> Result<[u8; 32]> {
    validate_accepted_bases(segment)?;
    let mut digest = Sha256::new();
    digest.update(ACCEPTED_SEGMENT_DOMAIN);
    digest.update(segment.source_ordinal.to_le_bytes());
    digest.update(segment.segment_ordinal.to_le_bytes());
    digest.update(
        u64::try_from(segment.bases.len())
            .map_err(|_| overflow("experimental segment length does not fit in u64"))?
            .to_le_bytes(),
    );
    for &base in segment.bases {
        digest.update([base.to_ascii_uppercase()]);
    }
    Ok(digest.finalize().into())
}

fn canonical_bytes(sequence: &[u8]) -> Result<Vec<u8>> {
    let mut forward = Vec::new();
    forward.try_reserve_exact(sequence.len()).map_err(|cause| {
        resource_memory(format!(
            "cannot reserve independent forward DNA slice: {cause}"
        ))
    })?;
    for (position, &base) in sequence.iter().enumerate() {
        if exact_base_bits(base).is_none() {
            return Err(VeritasmError::new(
                ErrorCode::InputNucleotide,
                format!(
                    "independent experimental DNA slice contains non-ACGT byte 0x{base:02x} at zero-based position {position}"
                ),
            ));
        }
        forward.push(base.to_ascii_uppercase());
    }
    let mut reverse = Vec::new();
    reverse.try_reserve_exact(sequence.len()).map_err(|cause| {
        resource_memory(format!(
            "cannot reserve independent reverse-complement DNA slice: {cause}"
        ))
    })?;
    for &base in forward.iter().rev() {
        reverse.push(match base {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            _ => {
                return invariant(
                    "independent canonical DNA oracle received a non-exact normalized base",
                )
            }
        });
    }
    Ok(if reverse < forward { reverse } else { forward })
}

fn validate_accepted_bases(segment: AcceptedSegment<'_>) -> Result<()> {
    for (position, &base) in segment.bases.iter().enumerate() {
        if exact_base_bits(base).is_none() {
            return Err(VeritasmError::new(
                ErrorCode::InputNucleotide,
                format!(
                    "experimental accepted segment {}/{} contains non-ACGT byte 0x{base:02x} at zero-based position {position}; split at every ambiguity or quality boundary before partitioning",
                    segment.source_ordinal, segment.segment_ordinal
                ),
            ));
        }
    }
    Ok(())
}

fn coalesce_exact_counts(counts: &mut Vec<ExactEdgeCount>) -> Result<()> {
    let mut write = 0_usize;
    for read in 0..counts.len() {
        let row = counts[read];
        if write > 0
            && counts[write - 1].bucket_id == row.bucket_id
            && counts[write - 1].key == row.key
        {
            if counts[write - 1].minimizer != row.minimizer {
                return invariant(
                    "one experimental canonical edge was assigned multiple minimizers",
                );
            }
            counts[write - 1].occurrences = checked_add(
                counts[write - 1].occurrences,
                row.occurrences,
                "experimental exact-edge count overflow",
            )?;
        } else {
            if write != read {
                counts[write] = row;
            }
            write += 1;
        }
    }
    counts.truncate(write);
    Ok(())
}

fn build_bucket_ranges(counts: &[ExactEdgeCount], buckets: &mut Vec<BucketRange>) -> Result<()> {
    for (index, count) in counts.iter().enumerate() {
        let index = u64::try_from(index)
            .map_err(|_| overflow("experimental exact-edge index does not fit in u64"))?;
        match buckets.last_mut() {
            Some(bucket) if bucket.bucket_id == count.bucket_id => {
                bucket.distinct_edge_count = checked_add(
                    bucket.distinct_edge_count,
                    1,
                    "experimental bucket distinct-edge count overflow",
                )?;
                bucket.occurrence_count = checked_add(
                    bucket.occurrence_count,
                    count.occurrences,
                    "experimental bucket occurrence count overflow",
                )?;
            }
            _ => buckets.push(BucketRange {
                bucket_id: count.bucket_id,
                first_edge_count: index,
                distinct_edge_count: 1,
                occurrence_count: count.occurrences,
            }),
        }
    }
    Ok(())
}

fn projected_payload_bytes(
    segment_count: usize,
    accepted_windows: u64,
    maximum_segment_windows: u64,
    super_capacity: u64,
    bucket_capacity: u64,
    k: u8,
) -> Result<u64> {
    let segment_count = u64::try_from(segment_count)
        .map_err(|_| overflow("experimental segment count does not fit in u64"))?;
    let mut total = 0_u64;
    total = checked_payload_add::<usize>(total, segment_count, "segment order")?;
    total = checked_payload_add::<SegmentPartition>(total, segment_count, "segment ledger")?;
    total = checked_payload_add::<ExactEdgeCount>(total, accepted_windows, "edge observations")?;
    total =
        checked_payload_add::<SourceWindowProof>(total, accepted_windows, "source-window proofs")?;
    total =
        checked_payload_add::<ExactEdgeCount>(total, accepted_windows, "proof-count validation")?;
    total = checked_payload_add::<SuperKmerSpan>(total, super_capacity, "super-k-mer spans")?;
    total = checked_payload_add::<BucketRange>(total, bucket_capacity, "bucket ranges")?;
    total = checked_payload_add::<PackedKmer>(
        total,
        maximum_segment_windows,
        "rolling-scan materialization",
    )?;
    // Rolling validity state plus the deliberately allocating literal
    // forward/reverse source-window and minimizer oracle workspaces.
    let per_window_workspaces = u64::from(k)
        .checked_mul(6)
        .ok_or_else(|| overflow("experimental rolling workspace byte count overflow"))?;
    total = checked_add(
        total,
        per_window_workspaces,
        "experimental rolling workspace byte count overflow",
    )?;
    // Seven persistent/build vectors plus the returned scan vector, its two
    // rolling validity rings, and one decoded canonical k-mer.
    let vector_headers = u64::try_from(size_of::<Vec<usize>>() * 11)
        .map_err(|_| overflow("experimental vector-header bytes do not fit in u64"))?;
    checked_add(
        total,
        vector_headers,
        "experimental projected-payload total overflow",
    )
}

fn checked_payload_add<T>(total: u64, count: u64, label: &'static str) -> Result<u64> {
    let width = u64::try_from(size_of::<T>())
        .map_err(|_| overflow("experimental record width does not fit in u64"))?;
    let bytes = count
        .checked_mul(width)
        .ok_or_else(|| overflow("experimental projected-payload multiplication overflow"))?;
    total.checked_add(bytes).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            format!("experimental projected-payload addition overflow for {label}"),
        )
    })
}

fn enforce_limit(observed: u64, limit: u64, code: ErrorCode, label: &'static str) -> Result<()> {
    if observed <= limit {
        Ok(())
    } else {
        Err(VeritasmError::new(
            code,
            format!("{label} {observed} exceeds configured limit {limit}"),
        ))
    }
}

fn as_usize(value: u64, label: &'static str) -> Result<usize> {
    usize::try_from(value).map_err(|_| {
        resource_memory(format!(
            "{label} {value} does not fit this platform's usize"
        ))
    })
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn overflow(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn resource_memory(context: String) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn invariant<T>(context: &'static str) -> Result<T> {
    Err(VeritasmError::new(ErrorCode::InternalInvariant, context))
}

fn exact_base_bits(base: u8) -> Option<u8> {
    match base.to_ascii_uppercase() {
        b'A' => Some(0),
        b'C' => Some(1),
        b'G' => Some(2),
        b'T' => Some(3),
        _ => None,
    }
}

fn active_mask(length: u8) -> PackedKmer {
    let active_bits = usize::from(length) * 2;
    let full_words = active_bits / 64;
    let partial_bits = active_bits % 64;
    let mut words = [0_u64; 4];
    for index in 0..full_words {
        words[3 - index] = u64::MAX;
    }
    if partial_bits != 0 {
        words[3 - full_words] = (1_u64 << partial_bits) - 1;
    }
    PackedKmer::from_words(words)
}

fn packed_shift_left_two(value: PackedKmer, base: u8) -> PackedKmer {
    let words = value.words();
    PackedKmer::from_words([
        (words[0] << 2) | (words[1] >> 62),
        (words[1] << 2) | (words[2] >> 62),
        (words[2] << 2) | (words[3] >> 62),
        (words[3] << 2) | u64::from(base),
    ])
}

fn packed_shift_right_two(value: PackedKmer) -> PackedKmer {
    let words = value.words();
    PackedKmer::from_words([
        words[0] >> 2,
        (words[1] >> 2) | (words[0] << 62),
        (words[2] >> 2) | (words[1] << 62),
        (words[3] >> 2) | (words[2] << 62),
    ])
}

trait PackedOperations {
    fn masked(self, mask: Self) -> Self;
    fn with_pair(self, pair: u8, bit_offset: u16) -> Self;
}

impl PackedOperations for PackedKmer {
    fn masked(self, mask: Self) -> Self {
        let value = self.words();
        let mask = mask.words();
        PackedKmer::from_words([
            value[0] & mask[0],
            value[1] & mask[1],
            value[2] & mask[2],
            value[3] & mask[3],
        ])
    }

    fn with_pair(self, pair: u8, bit_offset: u16) -> Self {
        debug_assert!(pair < 4);
        debug_assert!(bit_offset <= 252);
        let mut words = self.words();
        let word_from_low = usize::from(bit_offset / 64);
        let within_word = u32::from(bit_offset % 64);
        words[3 - word_from_low] |= u64::from(pair) << within_word;
        PackedKmer::from_words(words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn limits() -> PartitionLimits {
        PartitionLimits {
            max_input_bases: 10_000,
            max_windows: 10_000,
            max_super_kmers: 10_000,
            max_distinct_kmers: 10_000,
            max_accounted_bytes: 10_000_000,
        }
    }

    fn config(k: u8, minimizer_length: u8, virtual_bucket_count: u32) -> PartitionConfig {
        PartitionConfig {
            k,
            minimizer_length,
            virtual_bucket_count,
            limits: limits(),
        }
    }

    fn bases(symbols: &[u8]) -> Vec<u8> {
        symbols
            .iter()
            .map(|symbol| match symbol % 4 {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                _ => b'T',
            })
            .collect()
    }

    fn key_counts(result: &PartitionedDbg) -> Vec<(PackedKmer, u64)> {
        let mut counts: Vec<_> = result
            .edge_counts
            .iter()
            .map(|count| (count.key, count.occurrences))
            .collect();
        counts.sort_unstable_by_key(|row| row.0);
        counts
    }

    fn direct_counts(sequence: &[u8], k: u8) -> Vec<(PackedKmer, u64)> {
        let mut keys = scan_canonical_kmers(sequence, None, k, 0).unwrap().kmers;
        keys.sort_unstable();
        let mut counts: Vec<(PackedKmer, u64)> = Vec::new();
        for key in keys {
            if counts.last().is_some_and(|last| last.0 == key) {
                counts.last_mut().unwrap().1 += 1;
            } else {
                counts.push((key, 1));
            }
        }
        counts
    }

    fn naive_minimizer(kmer: PackedKmer, k: u8, m: u8) -> MinimizerOwner {
        let canonical = canonical_code(kmer, k).unwrap();
        let sequence = decode_mer(canonical, k).unwrap();
        let mut best: Option<MinimizerOwner> = None;
        for (position, window) in sequence.windows(usize::from(m)).enumerate() {
            let exact = super::super::wide_kmer::encode_exact_bases(window).unwrap();
            let key = canonical_code(exact, m).unwrap();
            if best.is_none_or(|current| key < current.key) {
                best = Some(MinimizerOwner {
                    key,
                    leftmost_position: u8::try_from(position).unwrap(),
                });
            }
        }
        best.unwrap()
    }

    fn assert_super_kmers_match_windows(result: &PartitionedDbg, segment: AcceptedSegment<'_>) {
        let ledger = result
            .segments
            .iter()
            .find(|row| {
                row.source_ordinal == segment.source_ordinal
                    && row.segment_ordinal == segment.segment_ordinal
            })
            .unwrap();
        let start = usize::try_from(ledger.first_super_kmer).unwrap();
        let end = usize::try_from(ledger.first_super_kmer + ledger.super_kmer_count).unwrap();
        let spans = &result.super_kmers[start..end];
        let keys = scan_canonical_kmers(segment.bases, None, result.k, 0)
            .unwrap()
            .kmers;
        for (position, key) in keys.into_iter().enumerate() {
            let position = u64::try_from(position).unwrap();
            let span = spans
                .iter()
                .find(|span| {
                    position >= span.first_window
                        && position < span.first_window + span.window_count
                })
                .unwrap();
            let owner = select_minimizer(key, result.k, result.minimizer_length).unwrap();
            assert_eq!(span.minimizer, owner.key);
            assert_eq!(
                span.bucket_id,
                route_minimizer(
                    owner.key,
                    result.minimizer_length,
                    result.virtual_bucket_count
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn minimizer_owner_is_strand_invariant_and_ties_go_left() {
        let homopolymer = super::super::wide_kmer::encode_kmer(b"AAAAAAA").unwrap();
        let owner = select_minimizer(homopolymer, 7, 3).unwrap();
        assert_eq!(owner.leftmost_position, 0);

        let key = super::super::wide_kmer::encode_kmer(b"TCGCGAC").unwrap();
        let reverse = super::super::wide_kmer::reverse_complement_code(key, 7).unwrap();
        assert_eq!(
            select_minimizer(key, 7, 3).unwrap(),
            select_minimizer(reverse, 7, 3).unwrap()
        );
    }

    #[test]
    fn minimizer_rolling_state_matches_slice_oracle_across_word_boundaries() {
        let sequence: Vec<u8> = (0..127)
            .map(|position| match (position * 7 + position / 5 + 1) % 4 {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                _ => b'T',
            })
            .collect();
        for (k, minimizers) in [
            (31, &[1, 15, 31][..]),
            (32, &[1, 31, 32][..]),
            (63, &[31, 32, 63][..]),
            (64, &[31, 32, 63, 64][..]),
            (95, &[32, 64, 95][..]),
            (127, &[31, 64, 96, 127][..]),
        ] {
            let key = super::super::wide_kmer::encode_kmer(&sequence[..usize::from(k)]).unwrap();
            for &m in minimizers {
                assert_eq!(
                    select_minimizer(key, k, m).unwrap(),
                    naive_minimizer(key, k, m),
                    "k={k}, m={m}"
                );
            }
        }
    }

    #[test]
    fn bucket_collisions_do_not_merge_exact_minimizers_or_edges() {
        let segments = [AcceptedSegment {
            source_ordinal: 4,
            segment_ordinal: 2,
            bases: b"ACGTTGCAAGTCCTAGGCTA",
        }];
        let result = build_partitioned_dbg(config(7, 3, 1), &segments).unwrap();
        assert_eq!(result.buckets.len(), 1);
        assert!(result
            .edge_counts
            .windows(2)
            .any(|pair| pair[0].minimizer != pair[1].minimizer));
        assert_eq!(result.buckets[0].occurrence_count, result.accepted_windows);
        result.validate_invariants().unwrap();
    }

    #[test]
    fn invariant_validation_rejects_noncanonical_complete_keys() {
        let segment = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACGT",
        }];
        let mut result = build_partitioned_dbg(config(5, 3, 1), &segment).unwrap();
        let canonical = result.edge_counts[0].key;
        let reverse = super::super::wide_kmer::reverse_complement_code(canonical, 5).unwrap();
        assert_ne!(canonical, reverse);
        result.edge_counts[0].key = reverse;
        assert_eq!(
            result.validate_invariants().unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn span_ownership_is_bound_to_each_source_window_proof() {
        let segment = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACGT",
        }];
        let mut result = build_partitioned_dbg(config(5, 3, 1), &segment).unwrap();
        let wrong = encode_exact_bases(b"CCC").unwrap();
        assert_ne!(result.super_kmers[0].minimizer, wrong);
        result.super_kmers[0].minimizer = wrong;
        result.super_kmers[0].bucket_id = 0;
        assert_eq!(
            result.validate_invariants().unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn coordinated_proof_mutation_is_rejected_by_byte_slice_oracle() {
        let segment = [AcceptedSegment {
            source_ordinal: 7,
            segment_ordinal: 9,
            bases: b"AACGT",
        }];
        let mut result = build_partitioned_dbg(config(5, 3, 1), &segment).unwrap();
        let replacement = independent_window_oracle(b"AAACC", 5, 3, 1).unwrap();
        result.window_proofs[0].key = replacement.key;
        result.window_proofs[0].minimizer = replacement.owner.key;
        result.window_proofs[0].bucket_id = replacement.bucket_id;
        result.super_kmers[0].minimizer = replacement.owner.key;
        result.super_kmers[0].bucket_id = replacement.bucket_id;
        result.edge_counts[0] = ExactEdgeCount {
            bucket_id: replacement.bucket_id,
            minimizer: replacement.owner.key,
            key: replacement.key,
            occurrences: 1,
        };
        result.buckets[0].bucket_id = replacement.bucket_id;
        result.validate_invariants().unwrap();
        assert_eq!(
            result
                .validate_against_segments(&segment)
                .unwrap_err()
                .code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn segment_and_super_kmer_ledgers_cover_every_window_once() {
        let segments = [
            AcceptedSegment {
                source_ordinal: 2,
                segment_ordinal: 0,
                bases: b"ACGT",
            },
            AcceptedSegment {
                source_ordinal: 1,
                segment_ordinal: 1,
                bases: b"AACCGGTTAACCGG",
            },
            AcceptedSegment {
                source_ordinal: 1,
                segment_ordinal: 0,
                bases: b"TTGCAACGT",
            },
        ];
        let result = build_partitioned_dbg(config(5, 3, 7), &segments).unwrap();
        assert_eq!(
            result
                .segments
                .iter()
                .map(|segment| (segment.source_ordinal, segment.segment_ordinal))
                .collect::<Vec<_>>(),
            vec![(1, 0), (1, 1), (2, 0)]
        );
        assert_eq!(result.accepted_windows, 5 + 10);
        for segment in segments {
            assert_super_kmers_match_windows(&result, segment);
        }
        result.validate_invariants().unwrap();
    }

    #[test]
    fn proof_to_span_validation_advances_monotonically_with_many_owner_changes() {
        let mut state = 0x6a09_e667_f3bc_c909_u64;
        let sequence = (0..4_096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                b"ACGT"[usize::try_from(state & 3).unwrap()]
            })
            .collect::<Vec<_>>();
        let segment = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: &sequence,
        }];
        let result = build_partitioned_dbg(config(15, 15, 17), &segment).unwrap();
        let ledger = result.segments[0];
        let span_start = usize::try_from(ledger.first_super_kmer).unwrap();
        let span_end = usize::try_from(ledger.first_super_kmer + ledger.super_kmer_count).unwrap();
        let proof_start = usize::try_from(ledger.first_window_proof).unwrap();
        let proof_end = usize::try_from(ledger.first_window_proof + ledger.window_count).unwrap();
        let spans = &result.super_kmers[span_start..span_end];
        let proofs = &result.window_proofs[proof_start..proof_end];
        assert!(spans.len() > 3_000, "fixture did not change owners often");

        let advances = validate_proof_span_ownership(
            proofs,
            spans,
            ledger.source_ordinal,
            ledger.segment_ordinal,
            result.k,
            result.minimizer_length,
            result.virtual_bucket_count,
        )
        .unwrap();
        assert_eq!(advances + 1, spans.len());
        assert!(advances < proofs.len());
    }

    #[test]
    fn rejects_unsplit_ambiguity_and_duplicate_coordinates() {
        let ambiguous = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACNCAA",
        }];
        assert_eq!(
            build_partitioned_dbg(config(5, 3, 4), &ambiguous)
                .unwrap_err()
                .code(),
            ErrorCode::InputNucleotide
        );

        let duplicate = [
            AcceptedSegment {
                source_ordinal: 0,
                segment_ordinal: 0,
                bases: b"AACCGG",
            },
            AcceptedSegment {
                source_ordinal: 0,
                segment_ordinal: 0,
                bases: b"TTGGCC",
            },
        ];
        assert_eq!(
            build_partitioned_dbg(config(5, 3, 4), &duplicate)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
    }

    #[test]
    fn resource_limits_fail_before_unbounded_growth() {
        let segment = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACCGGTTAACCGGTT",
        }];
        let mut limited = config(5, 3, 4);
        limited.limits.max_windows = 2;
        assert_eq!(
            build_partitioned_dbg(limited, &segment).unwrap_err().code(),
            ErrorCode::ResourceRetainedKeys
        );

        limited = config(5, 3, 4);
        limited.limits.max_accounted_bytes = 1;
        assert_eq!(
            build_partitioned_dbg(limited, &segment).unwrap_err().code(),
            ErrorCode::ResourceMemory
        );

        limited = config(5, 3, 4);
        limited.limits.max_super_kmers = 0;
        assert_eq!(
            build_partitioned_dbg(limited, &segment).unwrap_err().code(),
            ErrorCode::ResourceRetainedKeys
        );
    }

    #[test]
    fn input_order_and_rayon_pool_size_do_not_change_output() {
        let first = AcceptedSegment {
            source_ordinal: 9,
            segment_ordinal: 0,
            bases: b"ACGTACGTTGCATGCA",
        };
        let second = AcceptedSegment {
            source_ordinal: 3,
            segment_ordinal: 1,
            bases: b"TTTACCGGATCGATCG",
        };
        let one_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let four_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let one =
            one_pool.install(|| build_partitioned_dbg(config(7, 3, 11), &[first, second]).unwrap());
        let four = four_pool
            .install(|| build_partitioned_dbg(config(7, 3, 11), &[second, first]).unwrap());
        assert_eq!(one, four);
    }

    proptest! {
        #[test]
        fn exact_partition_matches_direct_rolling_count_oracle(
            symbols in prop::collection::vec(0_u8..4, 0..96),
            k_seed in 3_u8..20,
            minimizer_seed in 1_u8..20,
            buckets in 1_u32..17,
        ) {
            let sequence = bases(&symbols);
            let k = k_seed.min(19);
            let minimizer_length = 1 + ((minimizer_seed - 1) % k);
            let segment = [AcceptedSegment {
                source_ordinal: 0,
                segment_ordinal: 0,
                bases: &sequence,
            }];
            let result = build_partitioned_dbg(
                config(k, minimizer_length, buckets),
                &segment,
            ).unwrap();
            prop_assert_eq!(key_counts(&result), direct_counts(&sequence, k));
            assert_super_kmers_match_windows(&result, segment[0]);
            result.validate_invariants().unwrap();
        }

        #[test]
        fn reverse_complement_preserves_exact_partitioned_edge_multiset(
            symbols in prop::collection::vec(0_u8..4, 3..80),
            k_seed in 3_u8..18,
            minimizer_seed in 1_u8..18,
            buckets in 1_u32..13,
        ) {
            let sequence = bases(&symbols);
            let k = k_seed.min(sequence.len() as u8).max(3);
            let minimizer_length = 1 + ((minimizer_seed - 1) % k);
            let reverse = crate::dna::reverse_complement(&sequence).unwrap();
            let forward_segment = [AcceptedSegment {
                source_ordinal: 0,
                segment_ordinal: 0,
                bases: &sequence,
            }];
            let reverse_segment = [AcceptedSegment {
                source_ordinal: 0,
                segment_ordinal: 0,
                bases: &reverse,
            }];
            let parameters = config(k, minimizer_length, buckets);
            let forward = build_partitioned_dbg(parameters, &forward_segment).unwrap();
            let reverse = build_partitioned_dbg(parameters, &reverse_segment).unwrap();
            prop_assert_eq!(forward.edge_counts, reverse.edge_counts);
            prop_assert_eq!(forward.buckets, reverse.buckets);
        }
    }
}
