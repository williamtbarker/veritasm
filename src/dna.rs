//! Exact two-bit DNA encoding and quality-aware canonical k-mer extraction.
//!
//! Packed keys use `A=00`, `C=01`, `G=10`, and `T=11`. The active
//! `2 * k` bits are right-aligned in a `u128`; the remaining high bits are
//! always zero. Ambiguous IUPAC DNA symbols are valid read input but break
//! every overlapping window. They are never converted to an arbitrary base.

use crate::config::SupportUnit;
use crate::error::{ErrorCode, Result, VeritasmError};
use crate::model::{Fragment, WindowStats};

/// Minimum k-mer length accepted by the stable encoding contract.
pub const MIN_PACKED_K: u8 = 3;
/// Maximum k-mer length; two bits per base occupy at most 126 bits.
pub const MAX_PACKED_K: u8 = 63;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmerScan {
    /// Canonical packed k-mers in read-window order.
    pub kmers: Vec<u128>,
    /// Mutually exclusive accounting for every possible window.
    pub stats: WindowStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentScan {
    /// Accepted observations under the requested support unit.
    ///
    /// Fragment-instance support returns sorted, distinct keys across both
    /// mates. Occurrence support retains every accepted read window.
    pub kmers: Vec<u128>,
    pub stats: WindowStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaseClass {
    Exact(u8),
    Ambiguous,
}

/// Validate the public k-mer range.
pub fn validate_k(k: u8) -> Result<()> {
    if (MIN_PACKED_K..=MAX_PACKED_K).contains(&k) {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!("k must be in {MIN_PACKED_K}..={MAX_PACKED_K}; received {k}"),
        ))
    }
}

/// Encode one exact A/C/G/T k-mer into the stable right-aligned representation.
pub fn encode_kmer(sequence: &[u8]) -> Result<u128> {
    let k = u8::try_from(sequence.len()).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "k-mer length exceeds {MAX_PACKED_K}; received {}",
                sequence.len()
            ),
        )
    })?;
    validate_k(k)?;
    encode_exact_bases(sequence)
}

/// Encode up to 63 exact bases.
///
/// This more general form also supports `(k-1)` graph nodes and the empty
/// sequence. Ambiguous symbols remain invalid because a packed key represents
/// one exact string.
pub fn encode_exact_bases(sequence: &[u8]) -> Result<u128> {
    if sequence.len() > usize::from(MAX_PACKED_K) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "packed DNA length must be at most {MAX_PACKED_K}; received {}",
                sequence.len()
            ),
        ));
    }

    let mut code = 0_u128;
    for (position, &base) in sequence.iter().enumerate() {
        match classify_base(base, position)? {
            BaseClass::Exact(bits) => code = (code << 2) | u128::from(bits),
            BaseClass::Ambiguous => {
                return Err(VeritasmError::new(
                    ErrorCode::InputNucleotide,
                    format!(
                        "ambiguous nucleotide byte 0x{base:02x} at zero-based position {position} cannot be encoded as an exact packed string"
                    ),
                ));
            }
        }
    }
    Ok(code)
}

/// Decode a right-aligned packed string of up to 63 bases.
pub fn decode_mer(mut code: u128, length: u8) -> Result<Vec<u8>> {
    validate_packed_code(code, length)?;
    let mut sequence = vec![b'A'; usize::from(length)];
    for base in sequence.iter_mut().rev() {
        *base = bits_base((code & 0b11) as u8);
        code >>= 2;
    }
    Ok(sequence)
}

/// Decode to an owned ASCII string.
pub fn decode_mer_string(code: u128, length: u8) -> Result<String> {
    // The decoder constructs only ASCII A/C/G/T bytes.
    String::from_utf8(decode_mer(code, length)?).map_err(|_| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "packed DNA decoder produced non-ASCII output",
        )
    })
}

/// Reverse-complement a right-aligned packed string of up to 63 bases.
pub fn reverse_complement_code(code: u128, length: u8) -> Result<u128> {
    validate_packed_code(code, length)?;
    Ok(reverse_complement_validated(code, length))
}

/// Select the unsigned-numeric minimum of a packed string and its reverse
/// complement.
pub fn canonical_code(code: u128, length: u8) -> Result<u128> {
    validate_packed_code(code, length)?;
    Ok(canonical_validated(code, length))
}

/// Reverse-complement an IUPAC DNA sequence and normalize it to uppercase.
pub fn reverse_complement(sequence: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(sequence.len());
    for (reverse_position, &base) in sequence.iter().rev().enumerate() {
        let original_position = sequence.len() - reverse_position - 1;
        let complement = match base.to_ascii_uppercase() {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            b'R' => b'Y',
            b'Y' => b'R',
            b'S' => b'S',
            b'W' => b'W',
            b'K' => b'M',
            b'M' => b'K',
            b'B' => b'V',
            b'D' => b'H',
            b'H' => b'D',
            b'V' => b'B',
            b'N' => b'N',
            _ => return Err(invalid_nucleotide(base, original_position)),
        };
        output.push(complement);
    }
    Ok(output)
}

/// Scan one read with a rolling forward/reverse-complement encoder.
///
/// Quality bytes are Phred+33 (`!` through `~`, Q0 through Q93). A window is
/// accepted only when it contains neither ambiguity nor a base below
/// `minimum_base_quality`. Without qualities, the quality threshold is not
/// applied.
pub fn scan_canonical_kmers(
    sequence: &[u8],
    quality: Option<&[u8]>,
    k: u8,
    minimum_base_quality: u8,
) -> Result<KmerScan> {
    validate_k(k)?;
    validate_minimum_quality(minimum_base_quality)?;
    if let Some(values) = quality {
        if sequence.len() != values.len() {
            return Err(VeritasmError::new(
                ErrorCode::InputFastqStructure,
                format!(
                    "sequence length {} differs from quality length {}",
                    sequence.len(),
                    values.len()
                ),
            ));
        }
    }

    let k_usize = usize::from(k);
    let capacity = if sequence.len() < k_usize {
        0
    } else {
        sequence.len() - k_usize + 1
    };
    let mut kmers = Vec::new();
    kmers.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve canonical k-mer scan buffer: {cause}"),
        )
    })?;
    let mut stats = WindowStats::default();
    let mask = active_mask(k);
    let reverse_shift = 2 * u32::from(k - 1);
    let mut forward = 0_u128;
    let mut reverse = 0_u128;
    let mut exact_run = 0_usize;
    let mut ambiguity_ring = vec![false; k_usize];
    let mut quality_ring = vec![false; k_usize];
    let mut ambiguity_count = 0_usize;
    let mut low_quality_count = 0_usize;

    for (position, &base) in sequence.iter().enumerate() {
        let slot = position % k_usize;
        if position >= k_usize {
            ambiguity_count -= usize::from(ambiguity_ring[slot]);
            low_quality_count -= usize::from(quality_ring[slot]);
        }

        match classify_base(base, position)? {
            BaseClass::Exact(bits) => {
                forward = ((forward << 2) | u128::from(bits)) & mask;
                reverse = (reverse >> 2) | (u128::from(3 - bits) << reverse_shift);
                exact_run = exact_run.saturating_add(1).min(k_usize);
                ambiguity_ring[slot] = false;
            }
            BaseClass::Ambiguous => {
                forward = 0;
                reverse = 0;
                exact_run = 0;
                ambiguity_ring[slot] = true;
                ambiguity_count += 1;
            }
        }

        let low_quality = if let Some(values) = quality {
            let value = values[position];
            validate_quality_byte(value, position)?;
            value - 33 < minimum_base_quality
        } else {
            false
        };
        quality_ring[slot] = low_quality;
        low_quality_count += usize::from(low_quality);

        if position + 1 < k_usize {
            continue;
        }

        checked_increment(&mut stats.possible, "possible-window count overflow")?;
        match (ambiguity_count > 0, low_quality_count > 0) {
            (false, false) => {
                if exact_run != k_usize {
                    return Err(VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "ambiguity-free window did not have a complete rolling code",
                    ));
                }
                checked_increment(&mut stats.accepted, "accepted-window count overflow")?;
                kmers.push(forward.min(reverse));
            }
            (true, false) => checked_increment(
                &mut stats.ambiguity_only,
                "ambiguity-only window count overflow",
            )?,
            (false, true) => checked_increment(
                &mut stats.quality_only,
                "quality-only window count overflow",
            )?,
            (true, true) => checked_increment(
                &mut stats.ambiguity_and_quality,
                "ambiguity-and-quality window count overflow",
            )?,
        }
    }

    validate_partition(&stats)?;
    Ok(KmerScan { kmers, stats })
}

/// Scan every read in one supplied fragment under an explicit support unit.
pub fn scan_fragment(
    fragment: &Fragment,
    k: u8,
    minimum_base_quality: u8,
    support_unit: SupportUnit,
) -> Result<FragmentScan> {
    validate_k(k)?;
    validate_minimum_quality(minimum_base_quality)?;

    // Reserve the final aggregate once.  Besides making allocation failure a
    // typed error, this removes amortized reallocation from the scan-memory
    // model used to admit batches in `pipeline`: at most this aggregate and
    // one read-local `KmerScan` data allocation coexist for each fragment.
    let k_usize = usize::from(k);
    let aggregate_capacity = fragment.reads.iter().try_fold(0_usize, |total, read| {
        let windows = read
            .sequence
            .len()
            .saturating_sub(k_usize.saturating_sub(1));
        total.checked_add(windows).ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                "fragment possible-window capacity exceeds usize",
            )
        })
    })?;
    let mut kmers = Vec::new();
    kmers
        .try_reserve_exact(aggregate_capacity)
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve fragment k-mer scan buffer: {cause}"),
            )
        })?;
    let mut stats = WindowStats::default();
    for read in &fragment.reads {
        let scan = scan_canonical_kmers(
            &read.sequence,
            read.quality.as_deref(),
            k,
            minimum_base_quality,
        )?;
        add_stats(&mut stats, &scan.stats)?;
        kmers.extend(scan.kmers);
    }

    if support_unit == SupportUnit::SuppliedFragmentInstance {
        kmers.sort_unstable();
        kmers.dedup();
    }
    validate_partition(&stats)?;
    Ok(FragmentScan { kmers, stats })
}

/// Return the active-bit mask for a validated k or packed-string length.
pub(crate) fn active_mask(length: u8) -> u128 {
    if length == 0 {
        0
    } else {
        (1_u128 << (2 * u32::from(length))) - 1
    }
}

/// Reverse-complement a code whose length and inactive bits are already known
/// to be valid.
pub(crate) fn reverse_complement_validated(mut code: u128, length: u8) -> u128 {
    let mut reverse = 0_u128;
    for _ in 0..length {
        reverse = (reverse << 2) | (3 - (code & 0b11));
        code >>= 2;
    }
    reverse
}

pub(crate) fn canonical_validated(code: u128, length: u8) -> u128 {
    code.min(reverse_complement_validated(code, length))
}

fn validate_packed_code(code: u128, length: u8) -> Result<()> {
    if length > MAX_PACKED_K {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!("packed DNA length must be in 0..={MAX_PACKED_K}; received {length}"),
        ));
    }
    if code & !active_mask(length) != 0 {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!("packed DNA code has nonzero inactive bits for length {length}"),
        ));
    }
    Ok(())
}

fn classify_base(base: u8, position: usize) -> Result<BaseClass> {
    match base.to_ascii_uppercase() {
        b'A' => Ok(BaseClass::Exact(0)),
        b'C' => Ok(BaseClass::Exact(1)),
        b'G' => Ok(BaseClass::Exact(2)),
        b'T' => Ok(BaseClass::Exact(3)),
        b'R' | b'Y' | b'S' | b'W' | b'K' | b'M' | b'B' | b'D' | b'H' | b'V' | b'N' => {
            Ok(BaseClass::Ambiguous)
        }
        _ => Err(invalid_nucleotide(base, position)),
    }
}

fn invalid_nucleotide(base: u8, position: usize) -> VeritasmError {
    VeritasmError::new(
        ErrorCode::InputNucleotide,
        format!("invalid nucleotide byte 0x{base:02x} at zero-based position {position}"),
    )
}

fn validate_minimum_quality(minimum: u8) -> Result<()> {
    if minimum <= 93 {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!("minimum base quality must be in 0..=93; received {minimum}"),
        ))
    }
}

fn validate_quality_byte(value: u8, position: usize) -> Result<()> {
    if (33..=126).contains(&value) {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::InputQuality,
            format!(
                "quality byte 0x{value:02x} at zero-based position {position} is outside Phred+33 Q0..Q93"
            ),
        ))
    }
}

fn checked_increment(value: &mut u64, context: &'static str) -> Result<()> {
    *value = value
        .checked_add(1)
        .ok_or_else(|| VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context))?;
    Ok(())
}

fn add_stats(total: &mut WindowStats, addend: &WindowStats) -> Result<()> {
    total.possible = total.possible.checked_add(addend.possible).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            "possible-window aggregate overflow",
        )
    })?;
    total.accepted = total.accepted.checked_add(addend.accepted).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            "accepted-window aggregate overflow",
        )
    })?;
    total.ambiguity_only = total
        .ambiguity_only
        .checked_add(addend.ambiguity_only)
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "ambiguity-only window aggregate overflow",
            )
        })?;
    total.quality_only = total
        .quality_only
        .checked_add(addend.quality_only)
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "quality-only window aggregate overflow",
            )
        })?;
    total.ambiguity_and_quality = total
        .ambiguity_and_quality
        .checked_add(addend.ambiguity_and_quality)
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "ambiguity-and-quality window aggregate overflow",
            )
        })?;
    Ok(())
}

fn validate_partition(stats: &WindowStats) -> Result<()> {
    let classified = stats
        .accepted
        .checked_add(stats.ambiguity_only)
        .and_then(|value| value.checked_add(stats.quality_only))
        .and_then(|value| value.checked_add(stats.ambiguity_and_quality))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "window classification total overflow",
            )
        })?;
    if classified != stats.possible {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!(
                "window classes sum to {classified}, but possible-window count is {}",
                stats.possible
            ),
        ));
    }
    Ok(())
}

const fn bits_base(bits: u8) -> u8 {
    match bits {
        0 => b'A',
        1 => b'C',
        2 => b'G',
        3 => b'T',
        _ => b'N',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MateRole, ReadRecord};
    use proptest::prelude::*;

    fn read(role: MateRole, sequence: &[u8], quality: Option<&[u8]>) -> ReadRecord {
        ReadRecord {
            role,
            normalized_id_digest: [0; 32],
            sequence: sequence.to_vec(),
            quality: quality.map(<[u8]>::to_vec),
        }
    }

    #[test]
    fn exact_round_trip_at_both_length_limits() {
        let long = b"ACGT".repeat(16);
        for sequence in [b"ACG".as_slice(), &long[..63]] {
            let code = encode_kmer(sequence).unwrap();
            assert_eq!(decode_mer(code, sequence.len() as u8).unwrap(), sequence);
        }
    }

    #[test]
    fn iupac_reverse_complement_is_an_involution_after_normalization() {
        let sequence = b"acgtryswkmbdhvn";
        let reverse = reverse_complement(sequence).unwrap();
        assert_eq!(
            reverse_complement(&reverse).unwrap(),
            sequence.to_ascii_uppercase()
        );
    }

    #[test]
    fn every_declared_iupac_ambiguity_is_a_window_break() {
        for ambiguity in b"RYSWKMBDHVNryswkmbdhvn" {
            let sequence = [b'A', b'A', *ambiguity, b'A', b'A'];
            let scan = scan_canonical_kmers(&sequence, None, 3, 0).unwrap();
            assert_eq!(scan.stats.possible, 3);
            assert_eq!(scan.stats.ambiguity_only, 3);
            assert!(scan.kmers.is_empty());
        }
    }

    #[test]
    fn maximum_length_key_leaves_two_high_bits_zero() {
        let sequence = vec![b'T'; 63];
        let code = encode_kmer(&sequence).unwrap();
        assert_eq!(code >> 126, 0);
        assert_eq!(decode_mer(code, 63).unwrap(), sequence);
    }

    #[test]
    fn scanner_partitions_ambiguity_and_quality_exclusively() {
        // k=3 windows: AAA accepted; AAN ambiguity only; ANN both;
        // NNA both; NAA both; AAA quality only.
        let scan = scan_canonical_kmers(b"AAANNAAA", Some(b"IIII!!II"), 3, 20).unwrap();
        assert_eq!(scan.stats.possible, 6);
        assert_eq!(scan.stats.accepted, 1);
        assert_eq!(scan.stats.ambiguity_only, 1);
        assert_eq!(scan.stats.quality_only, 1);
        assert_eq!(scan.stats.ambiguity_and_quality, 3);
    }

    #[test]
    fn quality_domain_boundaries_are_q0_through_q93() {
        assert_eq!(
            scan_canonical_kmers(b"AAA", Some(b"!!!"), 3, 0)
                .unwrap()
                .stats
                .accepted,
            1
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", Some(b"~~~"), 3, 93)
                .unwrap()
                .stats
                .accepted,
            1
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", Some(b"!!~"), 3, 1)
                .unwrap()
                .stats
                .quality_only,
            1
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", None, 3, 93)
                .unwrap()
                .stats
                .accepted,
            1
        );
    }

    #[test]
    fn invalid_input_uses_stable_typed_errors() {
        assert_eq!(
            scan_canonical_kmers(b"AAU", None, 3, 0).unwrap_err().code(),
            ErrorCode::InputNucleotide
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", Some(b"II\n"), 3, 0)
                .unwrap_err()
                .code(),
            ErrorCode::InputQuality
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", Some(b"II"), 3, 0)
                .unwrap_err()
                .code(),
            ErrorCode::InputFastqStructure
        );
    }

    #[test]
    fn fragment_support_deduplicates_within_and_across_mates() {
        let aaa = encode_kmer(b"AAA").unwrap();
        let fragment = Fragment {
            ordinal: 0,
            lane_ordinal: 0,
            reads: vec![
                read(MateRole::R1, b"AAAA", None),
                read(MateRole::R2, b"TTTT", None),
            ],
        };
        let fragment_scan =
            scan_fragment(&fragment, 3, 0, SupportUnit::SuppliedFragmentInstance).unwrap();
        assert_eq!(fragment_scan.kmers, vec![aaa]);

        let occurrence_scan =
            scan_fragment(&fragment, 3, 0, SupportUnit::AcceptedWindowOccurrence).unwrap();
        assert_eq!(occurrence_scan.kmers, vec![aaa; 4]);
    }

    proptest! {
        #[test]
        fn packed_reverse_complement_and_canonical_properties(
            symbols in prop::collection::vec(0_u8..4, 3..=63)
        ) {
            let sequence: Vec<u8> = symbols.iter().map(|&value| bits_base(value)).collect();
            let k = sequence.len() as u8;
            let code = encode_kmer(&sequence).unwrap();
            let reverse = reverse_complement_code(code, k).unwrap();
            prop_assert_eq!(reverse_complement_code(reverse, k).unwrap(), code);
            prop_assert_eq!(canonical_code(code, k).unwrap(), canonical_code(reverse, k).unwrap());
            prop_assert_eq!(decode_mer(code, k).unwrap(), sequence);
        }

        #[test]
        fn rolling_scan_matches_a_window_oracle(
            symbols in prop::collection::vec(0_u8..=5, 0..96),
            qualities in prop::collection::vec(0_u8..=2, 0..96),
            requested_k in 3_u8..=15,
            minimum_quality in 0_u8..=40,
        ) {
            let length = symbols.len().min(qualities.len());
            let sequence: Vec<u8> = symbols[..length]
                .iter()
                .map(|value| match value {
                    0 => b'A', 1 => b'C', 2 => b'G', 3 => b'T', 4 => b'N', _ => b'R',
                })
                .collect();
            let quality: Vec<u8> = qualities[..length]
                .iter()
                .map(|value| 33 + value * 30)
                .collect();
            let scan = scan_canonical_kmers(
                &sequence,
                Some(&quality),
                requested_k,
                minimum_quality,
            ).unwrap();

            let k = usize::from(requested_k);
            let mut oracle_keys = Vec::new();
            let mut oracle = WindowStats::default();
            if length >= k {
                for start in 0..=length-k {
                    oracle.possible += 1;
                    let ambiguous = sequence[start..start+k]
                        .iter()
                        .any(|base| !matches!(base, b'A' | b'C' | b'G' | b'T'));
                    let low_quality = quality[start..start+k]
                        .iter()
                        .any(|value| value - 33 < minimum_quality);
                    match (ambiguous, low_quality) {
                        (false, false) => {
                            oracle.accepted += 1;
                            let code = encode_kmer(&sequence[start..start+k]).unwrap();
                            oracle_keys.push(canonical_code(code, requested_k).unwrap());
                        }
                        (true, false) => oracle.ambiguity_only += 1,
                        (false, true) => oracle.quality_only += 1,
                        (true, true) => oracle.ambiguity_and_quality += 1,
                    }
                }
            }
            prop_assert_eq!(scan.stats, oracle);
            prop_assert_eq!(scan.kmers, oracle_keys);
        }
    }
}
