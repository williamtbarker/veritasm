//! Exact two-bit DNA keys and rolling canonical scans through k=127.
//!
//! This module is an experimental substrate. The stable assembly pipeline
//! continues to use the `u128` implementation in [`crate::dna`] and accepts
//! only k=3..63. In particular, this module does not alter the stable spool,
//! exact-count run, graph, digest, or output-schema encodings.
//!
//! [`PackedKmer`] stores four most-significant-word-first `u64` words. The
//! active `2 * length` bits are right-aligned, using `A=00`, `C=01`, `G=10`,
//! and `T=11`; valid values for the maximum length leave the top two bits
//! clear. The value deliberately does not carry its length, so serialized
//! bytes must always be framed by an independently authenticated length.

use crate::error::{ErrorCode, Result, VeritasmError};
use crate::model::WindowStats;
use std::fmt;
use std::mem::size_of;

/// Minimum k-mer length accepted by the experimental wide scanner.
pub const MIN_PACKED_K: u8 = 3;
/// Maximum k-mer length; two bits per base occupy at most 254 bits.
pub const MAX_PACKED_K: u8 = 127;
/// Fixed byte width of a packed key.
pub const PACKED_KEY_BYTES: usize = 32;

/// A length-free, right-aligned, exact two-bit DNA value.
///
/// Words are stored most-significant first. Derived ordering is therefore
/// unsigned numeric order and is also lexicographic order over
/// [`Self::to_be_bytes`]. A raw value can contain bits that are inactive for a
/// particular length; length-consuming operations reject such a value.
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PackedKmer {
    words: [u64; 4],
}

impl PackedKmer {
    /// The all-zero packed value.
    pub const ZERO: Self = Self { words: [0; 4] };

    /// Construct a value from most-significant-first words.
    ///
    /// This does not validate inactive bits because no length is supplied.
    pub const fn from_words(words: [u64; 4]) -> Self {
        Self { words }
    }

    /// Return the most-significant-first words.
    pub const fn words(self) -> [u64; 4] {
        self.words
    }

    /// Zero-extend a stable narrow-path key.
    pub const fn from_u128(value: u128) -> Self {
        Self {
            words: [0, 0, (value >> 64) as u64, value as u64],
        }
    }

    /// Return the value as `u128` when its upper 128 bits are zero.
    pub const fn as_u128(self) -> Option<u128> {
        if self.words[0] == 0 && self.words[1] == 0 {
            Some(((self.words[2] as u128) << 64) | self.words[3] as u128)
        } else {
            None
        }
    }

    /// Serialize all four words as fixed-width, big-endian bytes.
    ///
    /// This representation preserves unsigned numeric ordering. It is not a
    /// stable assembler file format and must be framed with a length and a
    /// separately versioned domain before use on disk.
    pub fn to_be_bytes(self) -> [u8; PACKED_KEY_BYTES] {
        let mut bytes = [0_u8; PACKED_KEY_BYTES];
        for (index, word) in self.words.iter().enumerate() {
            let start = index * 8;
            bytes[start..start + 8].copy_from_slice(&word.to_be_bytes());
        }
        bytes
    }

    /// Deserialize fixed-width, big-endian bytes without assuming a length.
    pub fn from_be_bytes(bytes: [u8; PACKED_KEY_BYTES]) -> Self {
        let mut words = [0_u64; 4];
        for (index, word) in words.iter_mut().enumerate() {
            let start = index * 8;
            let mut encoded = [0_u8; 8];
            encoded.copy_from_slice(&bytes[start..start + 8]);
            *word = u64::from_be_bytes(encoded);
        }
        Self { words }
    }

    fn shift_left_two_with_base(self, base: u8) -> Self {
        debug_assert!(base < 4);
        Self {
            words: [
                (self.words[0] << 2) | (self.words[1] >> 62),
                (self.words[1] << 2) | (self.words[2] >> 62),
                (self.words[2] << 2) | (self.words[3] >> 62),
                (self.words[3] << 2) | u64::from(base),
            ],
        }
    }

    fn shift_right_two(self) -> Self {
        Self {
            words: [
                self.words[0] >> 2,
                (self.words[1] >> 2) | (self.words[0] << 62),
                (self.words[2] >> 2) | (self.words[1] << 62),
                (self.words[3] >> 2) | (self.words[2] << 62),
            ],
        }
    }

    fn with_pair_at(mut self, pair: u8, bit_offset: u16) -> Self {
        debug_assert!(pair < 4);
        debug_assert!(bit_offset <= 252);
        debug_assert_eq!(bit_offset % 2, 0);
        let word_from_low = usize::from(bit_offset / 64);
        let within_word = u32::from(bit_offset % 64);
        debug_assert!(within_word <= 62);
        self.words[3 - word_from_low] |= u64::from(pair) << within_word;
        self
    }

    const fn low_pair(self) -> u8 {
        (self.words[3] & 0b11) as u8
    }

    fn masked(self, mask: Self) -> Self {
        Self {
            words: [
                self.words[0] & mask.words[0],
                self.words[1] & mask.words[1],
                self.words[2] & mask.words[2],
                self.words[3] & mask.words[3],
            ],
        }
    }
}

impl fmt::Debug for PackedKmer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PackedKmer(0x")?;
        for word in self.words {
            write!(formatter, "{word:016x}")?;
        }
        write!(formatter, ")")
    }
}

impl From<u128> for PackedKmer {
    fn from(value: u128) -> Self {
        Self::from_u128(value)
    }
}

impl TryFrom<PackedKmer> for u128 {
    type Error = VeritasmError;

    fn try_from(value: PackedKmer) -> Result<Self> {
        value.as_u128().ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ConfigurationInvalidK,
                "wide packed DNA value does not fit in the stable u128 representation",
            )
        })
    }
}

/// Canonical keys and mutually exclusive window accounting for one read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmerScan {
    /// Canonical packed k-mers in read-window order.
    pub kmers: Vec<PackedKmer>,
    /// Accounting for every possible window under the stable QC semantics.
    pub stats: WindowStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaseClass {
    Exact(u8),
    Ambiguous,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MinimizerCandidate {
    start: usize,
    key: PackedKmer,
}

/// Fixed-capacity monotone deque used by the fused routing scan.
///
/// The maximum live candidate count is `k - m + 1`, which cannot exceed
/// [`MAX_PACKED_K`]. Keeping this small queue inline avoids an allocator call
/// per read and, more importantly, the old decoded-vector allocation per
/// accepted window.
struct MinimizerQueue {
    entries: [MinimizerCandidate; MAX_PACKED_K as usize],
    head: usize,
    len: usize,
    capacity: usize,
}

/// Conservatively charged fixed scratch for one fused minimizer scan.
pub(crate) const FUSED_MINIMIZER_SCAN_SCRATCH_BYTES: u64 = size_of::<MinimizerQueue>() as u64;

impl MinimizerQueue {
    fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 || capacity > usize::from(MAX_PACKED_K) {
            return Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                "fused minimizer queue capacity is outside the packed-k range",
            ));
        }
        Ok(Self {
            entries: [MinimizerCandidate::default(); MAX_PACKED_K as usize],
            head: 0,
            len: 0,
            capacity,
        })
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    fn expire_before(&mut self, minimum_start: usize) {
        while self
            .front()
            .is_some_and(|candidate| candidate.start < minimum_start)
        {
            self.head = (self.head + 1) % self.capacity;
            self.len -= 1;
        }
    }

    fn push_monotone(&mut self, candidate: MinimizerCandidate) -> Result<()> {
        while self
            .back()
            .is_some_and(|current| current.key > candidate.key)
        {
            self.len -= 1;
        }
        if self.len >= self.capacity {
            return Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                "fused minimizer queue exceeded its exact live-candidate bound",
            ));
        }
        let index = (self.head + self.len) % self.capacity;
        self.entries[index] = candidate;
        self.len += 1;
        Ok(())
    }

    fn front(&self) -> Option<MinimizerCandidate> {
        (self.len != 0).then(|| self.entries[self.head])
    }

    fn back(&self) -> Option<MinimizerCandidate> {
        (self.len != 0).then(|| self.entries[(self.head + self.len - 1) % self.capacity])
    }
}

/// Validate the experimental k-mer range.
pub fn validate_k(k: u8) -> Result<()> {
    if (MIN_PACKED_K..=MAX_PACKED_K).contains(&k) {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!("experimental wide k must be in {MIN_PACKED_K}..={MAX_PACKED_K}; received {k}"),
        ))
    }
}

/// Validate that no bits outside `length` are set.
///
/// Length zero through 127 is accepted so the same substrate can later
/// represent exact `(k-1)` graph nodes. This function does not promote those
/// nodes or wide k-mers into the stable graph.
pub fn validate_code(code: PackedKmer, length: u8) -> Result<()> {
    if length > MAX_PACKED_K {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental packed DNA length must be in 0..={MAX_PACKED_K}; received {length}"
            ),
        ));
    }
    let mask = active_mask(length);
    if code.masked(mask) != code {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!("wide packed DNA code has nonzero inactive bits for length {length}"),
        ));
    }
    Ok(())
}

/// Encode one exact A/C/G/T k-mer into the wide representation.
pub fn encode_kmer(sequence: &[u8]) -> Result<PackedKmer> {
    let k = u8::try_from(sequence.len()).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental k-mer length exceeds {MAX_PACKED_K}; received {}",
                sequence.len()
            ),
        )
    })?;
    validate_k(k)?;
    encode_exact_bases(sequence)
}

/// Encode zero through 127 exact bases.
///
/// Ambiguous symbols are invalid because a packed value represents one exact
/// spelling. This function exists for future graph-node experiments and does
/// not change the stable graph implementation.
pub fn encode_exact_bases(sequence: &[u8]) -> Result<PackedKmer> {
    if sequence.len() > usize::from(MAX_PACKED_K) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental packed DNA length must be at most {MAX_PACKED_K}; received {}",
                sequence.len()
            ),
        ));
    }

    let mut code = PackedKmer::ZERO;
    for (position, &base) in sequence.iter().enumerate() {
        match classify_base(base, position)? {
            BaseClass::Exact(bits) => code = code.shift_left_two_with_base(bits),
            BaseClass::Ambiguous => {
                return Err(VeritasmError::new(
                    ErrorCode::InputNucleotide,
                    format!(
                        "ambiguous nucleotide byte 0x{base:02x} at zero-based position {position} cannot be encoded as an exact experimental packed string"
                    ),
                ));
            }
        }
    }
    Ok(code)
}

/// Decode a right-aligned packed string of zero through 127 bases.
pub fn decode_mer(mut code: PackedKmer, length: u8) -> Result<Vec<u8>> {
    validate_code(code, length)?;
    let mut sequence = Vec::new();
    sequence
        .try_reserve_exact(usize::from(length))
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve experimental decoded DNA buffer: {cause}"),
            )
        })?;
    enforce_exact_capacity(
        sequence.capacity(),
        usize::from(length),
        "experimental decoded DNA buffer",
    )?;
    sequence.resize(usize::from(length), b'A');
    for base in sequence.iter_mut().rev() {
        *base = bits_base(code.low_pair());
        code = code.shift_right_two();
    }
    Ok(sequence)
}

/// Decode to an owned ASCII string.
pub fn decode_mer_string(code: PackedKmer, length: u8) -> Result<String> {
    // The decoder constructs only ASCII A/C/G/T bytes.
    String::from_utf8(decode_mer(code, length)?).map_err(|_| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "experimental packed DNA decoder produced non-ASCII output",
        )
    })
}

/// Reverse-complement a validated right-aligned packed string.
pub fn reverse_complement_code(code: PackedKmer, length: u8) -> Result<PackedKmer> {
    validate_code(code, length)?;
    Ok(reverse_complement_validated(code, length))
}

/// Select the unsigned-numeric minimum of a packed value and its reverse
/// complement.
pub fn canonical_code(code: PackedKmer, length: u8) -> Result<PackedKmer> {
    validate_code(code, length)?;
    Ok(code.min(reverse_complement_validated(code, length)))
}

/// Return the length-`length - 1` prefix of an exact packed string.
///
/// This is a literal graph operation: it removes the rightmost base without
/// canonicalizing the resulting node. `length` must be at least one.
pub fn prefix_code(code: PackedKmer, length: u8) -> Result<PackedKmer> {
    validate_nonempty_exact_length(code, length)?;
    Ok(code.shift_right_two())
}

/// Return the length-`length - 1` suffix of an exact packed string.
///
/// This is a literal graph operation: it removes the leftmost base without
/// canonicalizing the resulting node. `length` must be at least one.
pub fn suffix_code(code: PackedKmer, length: u8) -> Result<PackedKmer> {
    validate_nonempty_exact_length(code, length)?;
    Ok(code.masked(active_mask(length - 1)))
}

/// Return the ASCII terminal base of a nonempty exact packed string.
pub fn terminal_base(code: PackedKmer, length: u8) -> Result<u8> {
    validate_nonempty_exact_length(code, length)?;
    Ok(bits_base(code.low_pair()))
}

fn validate_nonempty_exact_length(code: PackedKmer, length: u8) -> Result<()> {
    if length == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            "experimental packed DNA graph operation requires a nonempty string",
        ));
    }
    validate_code(code, length)
}

/// Scan one read with exact rolling forward and reverse-complement keys.
///
/// The QC and ambiguity semantics intentionally match [`crate::dna`]: IUPAC
/// ambiguity breaks every overlapping window, a FASTQ window is accepted only
/// when all of its bases meet `minimum_base_quality`, and the four rejection
/// classes are mutually exclusive. Quality bytes are Phred+33 Q0..Q93.
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
    let capacity = sequence.len().saturating_sub(k_usize.saturating_sub(1));
    let mut kmers = Vec::new();
    kmers.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve experimental canonical k-mer scan buffer: {cause}"),
        )
    })?;
    enforce_exact_capacity(
        kmers.capacity(),
        capacity,
        "experimental canonical k-mer scan buffer",
    )?;
    let mut ambiguity_ring = allocate_false_ring(k_usize, "ambiguity")?;
    let mut quality_ring = allocate_false_ring(k_usize, "quality")?;
    let mut stats = WindowStats::default();
    let mask = active_mask(k);
    let reverse_offset = 2 * u16::from(k - 1);
    let mut forward = PackedKmer::ZERO;
    let mut reverse = PackedKmer::ZERO;
    let mut exact_run = 0_usize;
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
                forward = forward.shift_left_two_with_base(bits).masked(mask);
                reverse = reverse
                    .shift_right_two()
                    .with_pair_at(3 - bits, reverse_offset);
                exact_run = exact_run.saturating_add(1).min(k_usize);
                ambiguity_ring[slot] = false;
            }
            BaseClass::Ambiguous => {
                forward = PackedKmer::ZERO;
                reverse = PackedKmer::ZERO;
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
                        "experimental ambiguity-free window did not have a complete rolling code",
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

/// Scan canonical k-mers and their exact minimizer owner keys in one pass.
///
/// This crate-private streaming path is used only to route authenticated
/// external-count observations. It preserves the public scanner's QC and
/// ambiguity semantics, but emits each accepted `(canonical k-mer, canonical
/// minimizer)` pair directly instead of allocating a decoded k-mer and
/// rescanning all of its m-mers. A fixed monotone deque retains the minimum
/// canonical m-mer over the current k-window. Equal candidates are retained
/// until expiry for deterministic raw-read scanning. Only the strand-invariant
/// owner key is emitted; no claim is made that a raw-read tie position equals
/// the selector's position in the canonical full-k spelling. The independent
/// [`select_minimizer`](crate::experimental::partitioned_dbg::select_minimizer)
/// remains the validation oracle for the materialized exact table.
pub(crate) fn scan_canonical_kmers_with_minimizers<F>(
    sequence: &[u8],
    quality: Option<&[u8]>,
    k: u8,
    minimizer_length: u8,
    minimum_base_quality: u8,
    mut emit: F,
) -> Result<WindowStats>
where
    F: FnMut(PackedKmer, PackedKmer) -> Result<()>,
{
    validate_k(k)?;
    if minimizer_length == 0 || minimizer_length > k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental minimizer length must be in 1..={k}; received {minimizer_length}"
            ),
        ));
    }
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
    let minimizer_usize = usize::from(minimizer_length);
    let mut ambiguity_ring = allocate_false_ring(k_usize, "fused-minimizer ambiguity")?;
    let mut quality_ring = allocate_false_ring(k_usize, "fused-minimizer quality")?;
    let queue_capacity = k_usize
        .checked_sub(minimizer_usize)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "fused minimizer live-candidate bound overflow",
            )
        })?;
    let mut minima = MinimizerQueue::new(queue_capacity)?;
    let k_mask = active_mask(k);
    let minimizer_mask = active_mask(minimizer_length);
    let k_reverse_offset = 2 * u16::from(k - 1);
    let minimizer_reverse_offset = 2 * u16::from(minimizer_length - 1);
    let mut k_forward = PackedKmer::ZERO;
    let mut k_reverse = PackedKmer::ZERO;
    let mut minimizer_forward = PackedKmer::ZERO;
    let mut minimizer_reverse = PackedKmer::ZERO;
    let mut exact_run = 0_usize;
    let mut ambiguity_count = 0_usize;
    let mut low_quality_count = 0_usize;
    let mut stats = WindowStats::default();

    for (position, &base) in sequence.iter().enumerate() {
        let slot = position % k_usize;
        if position >= k_usize {
            ambiguity_count -= usize::from(ambiguity_ring[slot]);
            low_quality_count -= usize::from(quality_ring[slot]);
        }

        match classify_base(base, position)? {
            BaseClass::Exact(bits) => {
                k_forward = k_forward.shift_left_two_with_base(bits).masked(k_mask);
                k_reverse = k_reverse
                    .shift_right_two()
                    .with_pair_at(3 - bits, k_reverse_offset);
                minimizer_forward = minimizer_forward
                    .shift_left_two_with_base(bits)
                    .masked(minimizer_mask);
                minimizer_reverse = minimizer_reverse
                    .shift_right_two()
                    .with_pair_at(3 - bits, minimizer_reverse_offset);
                exact_run = exact_run.saturating_add(1).min(k_usize);
                ambiguity_ring[slot] = false;

                if exact_run >= minimizer_usize {
                    if position + 1 >= k_usize {
                        minima.expire_before(position + 1 - k_usize);
                    }
                    minima.push_monotone(MinimizerCandidate {
                        start: position + 1 - minimizer_usize,
                        key: minimizer_forward.min(minimizer_reverse),
                    })?;
                }
            }
            BaseClass::Ambiguous => {
                k_forward = PackedKmer::ZERO;
                k_reverse = PackedKmer::ZERO;
                minimizer_forward = PackedKmer::ZERO;
                minimizer_reverse = PackedKmer::ZERO;
                exact_run = 0;
                ambiguity_ring[slot] = true;
                ambiguity_count += 1;
                minima.clear();
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
                        "fused minimizer ambiguity-free window did not have a complete rolling code",
                    ));
                }
                let minimizer = minima.front().ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "accepted fused minimizer window had no owner candidate",
                    )
                })?;
                checked_increment(&mut stats.accepted, "accepted-window count overflow")?;
                emit(k_forward.min(k_reverse), minimizer.key)?;
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
    Ok(stats)
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

fn reverse_complement_validated(mut code: PackedKmer, length: u8) -> PackedKmer {
    let mut reverse = PackedKmer::ZERO;
    for _ in 0..length {
        reverse = reverse.shift_left_two_with_base(3 - code.low_pair());
        code = code.shift_right_two();
    }
    reverse
}

fn allocate_false_ring(length: usize, label: &str) -> Result<Vec<bool>> {
    let mut ring = Vec::new();
    ring.try_reserve_exact(length).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve experimental {label} window ring: {cause}"),
        )
    })?;
    ring.resize(length, false);
    let word_bits = usize::BITS as usize;
    let admitted_bits = length
        .div_ceil(word_bits)
        .checked_mul(word_bits)
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "experimental rolling-ring admission overflow",
            )
        })?;
    if ring.capacity() > admitted_bits {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "allocator returned experimental {label} window ring capacity {} above admitted capacity {admitted_bits}",
                ring.capacity()
            ),
        ));
    }
    Ok(ring)
}

fn enforce_exact_capacity(actual: usize, admitted: usize, label: &'static str) -> Result<()> {
    if actual <= admitted {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "allocator returned {label} capacity {actual} above admitted capacity {admitted}"
            ),
        ))
    }
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
        _ => Err(VeritasmError::new(
            ErrorCode::InputNucleotide,
            format!("invalid nucleotide byte 0x{base:02x} at zero-based position {position}"),
        )),
    }
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

fn validate_partition(stats: &WindowStats) -> Result<()> {
    let classified = stats
        .accepted
        .checked_add(stats.ambiguity_only)
        .and_then(|value| value.checked_add(stats.quality_only))
        .and_then(|value| value.checked_add(stats.ambiguity_and_quality))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "experimental window classification total overflow",
            )
        })?;
    if classified != stats.possible {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!(
                "experimental window classes sum to {classified}, but possible-window count is {}",
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
    use proptest::prelude::*;

    const BOUNDARY_LENGTHS: [u8; 4] = [63, 64, 95, 127];
    type RoutedEvents = Vec<(PackedKmer, PackedKmer)>;

    fn sequence_from_symbols(symbols: &[u8]) -> Vec<u8> {
        symbols.iter().map(|&symbol| bits_base(symbol)).collect()
    }

    fn patterned_sequence(length: u8) -> Vec<u8> {
        (0..length)
            .map(|position| bits_base((position.wrapping_mul(3).wrapping_add(1)) % 4))
            .collect()
    }

    fn canonical_string(window: &[u8]) -> Vec<u8> {
        let forward = window.to_ascii_uppercase();
        let reverse = crate::dna::reverse_complement(window).unwrap();
        forward.min(reverse)
    }

    fn naive_scan(
        sequence: &[u8],
        quality: Option<&[u8]>,
        k: u8,
        minimum_quality: u8,
    ) -> (Vec<Vec<u8>>, WindowStats) {
        let k = usize::from(k);
        let mut kmers = Vec::new();
        let mut stats = WindowStats::default();
        if sequence.len() < k {
            return (kmers, stats);
        }
        for start in 0..=sequence.len() - k {
            stats.possible += 1;
            let window = &sequence[start..start + k];
            let ambiguous = window
                .iter()
                .any(|base| !matches!(base.to_ascii_uppercase(), b'A' | b'C' | b'G' | b'T'));
            let low_quality = quality.is_some_and(|values| {
                values[start..start + k]
                    .iter()
                    .any(|value| value - 33 < minimum_quality)
            });
            match (ambiguous, low_quality) {
                (false, false) => {
                    stats.accepted += 1;
                    kmers.push(canonical_string(window));
                }
                (true, false) => stats.ambiguity_only += 1,
                (false, true) => stats.quality_only += 1,
                (true, true) => stats.ambiguity_and_quality += 1,
            }
        }
        (kmers, stats)
    }

    fn fused_and_oracle_events(
        sequence: &[u8],
        quality: Option<&[u8]>,
        k: u8,
        minimizer_length: u8,
        minimum_quality: u8,
    ) -> (RoutedEvents, RoutedEvents, WindowStats) {
        let oracle = scan_canonical_kmers(sequence, quality, k, minimum_quality).unwrap();
        let oracle_events: Vec<_> = oracle
            .kmers
            .iter()
            .map(|&key| {
                let owner = crate::experimental::partitioned_dbg::select_minimizer(
                    key,
                    k,
                    minimizer_length,
                )
                .unwrap();
                (key, owner.key)
            })
            .collect();
        let mut fused_events = Vec::new();
        let fused_stats = scan_canonical_kmers_with_minimizers(
            sequence,
            quality,
            k,
            minimizer_length,
            minimum_quality,
            |key, minimizer| {
                fused_events.push((key, minimizer));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(fused_stats, oracle.stats);
        (fused_events, oracle_events, fused_stats)
    }

    #[test]
    fn boundary_lengths_round_trip_and_preserve_canonical_identity() {
        for length in BOUNDARY_LENGTHS {
            let sequence = patterned_sequence(length);
            let code = encode_kmer(&sequence).unwrap();
            assert_eq!(decode_mer(code, length).unwrap(), sequence);
            let reverse = reverse_complement_code(code, length).unwrap();
            assert_eq!(reverse_complement_code(reverse, length).unwrap(), code);
            assert_eq!(
                canonical_code(code, length).unwrap(),
                canonical_code(reverse, length).unwrap()
            );
        }
    }

    #[test]
    fn every_position_and_base_round_trips_at_boundary_lengths() {
        for length in BOUNDARY_LENGTHS {
            let template = patterned_sequence(length);
            for position in 0..usize::from(length) {
                for base in b"ACGT" {
                    let mut sequence = template.clone();
                    sequence[position] = *base;
                    let code = encode_kmer(&sequence).unwrap();
                    assert_eq!(decode_mer(code, length).unwrap(), sequence);
                    assert_eq!(
                        decode_mer(canonical_code(code, length).unwrap(), length).unwrap(),
                        canonical_string(&sequence)
                    );
                }
            }
        }
    }

    #[test]
    fn active_bits_cross_word_boundaries_exactly() {
        let t63 = encode_kmer(&[b'T'; 63]).unwrap();
        assert_eq!(t63.words(), [0, 0, 0x3fff_ffff_ffff_ffff, u64::MAX]);

        let t64 = encode_kmer(&[b'T'; 64]).unwrap();
        assert_eq!(t64.words(), [0, 0, u64::MAX, u64::MAX]);

        let t95 = encode_kmer(&[b'T'; 95]).unwrap();
        assert_eq!(t95.words(), [0, 0x3fff_ffff_ffff_ffff, u64::MAX, u64::MAX]);

        let t127 = encode_kmer(&[b'T'; 127]).unwrap();
        assert_eq!(
            t127.words(),
            [0x3fff_ffff_ffff_ffff, u64::MAX, u64::MAX, u64::MAX]
        );
    }

    #[test]
    fn validation_rejects_first_inactive_bit_at_boundaries() {
        for length in BOUNDARY_LENGTHS {
            let active_bits = usize::from(length) * 2;
            let word_from_low = active_bits / 64;
            let bit_in_word = active_bits % 64;
            let mut words = [0_u64; 4];
            words[3 - word_from_low] = 1_u64 << bit_in_word;
            assert_eq!(
                validate_code(PackedKmer::from_words(words), length)
                    .unwrap_err()
                    .code(),
                ErrorCode::InternalInvariant
            );
        }
    }

    #[test]
    fn big_endian_bytes_round_trip_and_preserve_order() {
        let values = [
            PackedKmer::ZERO,
            PackedKmer::from_u128(1),
            PackedKmer::from_words([0, 1, 0, 0]),
            PackedKmer::from_words([1, 0, 0, 0]),
            PackedKmer::from_words([u64::MAX; 4]),
        ];
        for value in values {
            assert_eq!(PackedKmer::from_be_bytes(value.to_be_bytes()), value);
        }
        for left in values {
            for right in values {
                assert_eq!(
                    left.cmp(&right),
                    left.to_be_bytes().cmp(&right.to_be_bytes())
                );
            }
        }
    }

    #[test]
    fn packed_value_has_the_declared_size_and_alignment() {
        assert_eq!(std::mem::size_of::<PackedKmer>(), PACKED_KEY_BYTES);
        assert_eq!(std::mem::align_of::<PackedKmer>(), 8);
    }

    #[test]
    fn narrow_conversion_is_exact_and_checked() {
        let narrow = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210_u128;
        let wide = PackedKmer::from(narrow);
        assert_eq!(wide.as_u128(), Some(narrow));
        assert_eq!(u128::try_from(wide).unwrap(), narrow);

        let too_wide = PackedKmer::from_words([0, 1, 0, 0]);
        assert_eq!(
            u128::try_from(too_wide).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidK
        );
    }

    #[test]
    fn wide_identity_does_not_truncate_high_words() {
        let all_a = encode_kmer(&[b'A'; 127]).unwrap();
        let mut distinct = vec![b'A'; 127];
        distinct[0] = b'C';
        let high_difference = encode_kmer(&distinct).unwrap();
        assert_ne!(all_a, high_difference);
        assert_eq!(all_a.words()[1..], high_difference.words()[1..]);
        assert!(high_difference.as_u128().is_none());
    }

    #[test]
    fn scanner_matches_naive_qc_oracle_at_boundary_lengths() {
        for k in BOUNDARY_LENGTHS {
            let length = usize::from(k) + 7;
            let mut sequence: Vec<u8> = (0..length)
                .map(|position| bits_base(((position * 3 + 1) % 4) as u8))
                .collect();
            let mut quality = vec![b'I'; length];
            sequence[usize::from(k) / 2] = b'N';
            quality[usize::from(k) + 2] = b'!';

            let observed = scan_canonical_kmers(&sequence, Some(&quality), k, 20).unwrap();
            let (expected_strings, expected_stats) = naive_scan(&sequence, Some(&quality), k, 20);
            let observed_strings: Vec<Vec<u8>> = observed
                .kmers
                .iter()
                .map(|&code| decode_mer(code, k).unwrap())
                .collect();
            assert_eq!(observed.stats, expected_stats);
            assert_eq!(observed_strings, expected_strings);
        }
    }

    #[test]
    fn ambiguity_reset_requires_a_complete_new_wide_window() {
        for k in BOUNDARY_LENGTHS {
            let mut sequence = vec![b'A'; usize::from(k) * 2 + 1];
            sequence[usize::from(k)] = b'N';
            let scan = scan_canonical_kmers(&sequence, None, k, 0).unwrap();
            assert_eq!(scan.stats.possible, u64::from(k) + 2);
            assert_eq!(scan.stats.ambiguity_only, u64::from(k));
            assert_eq!(scan.stats.accepted, 2);
            assert_eq!(scan.kmers.len(), 2);
        }
    }

    #[test]
    fn all_iupac_ambiguities_break_wide_windows() {
        for ambiguity in b"RYSWKMBDHVNryswkmbdhvn" {
            let mut sequence = vec![b'A'; 129];
            sequence[64] = *ambiguity;
            let scan = scan_canonical_kmers(&sequence, None, 127, 0).unwrap();
            assert_eq!(scan.stats.possible, 3);
            assert_eq!(scan.stats.ambiguity_only, 3);
            assert!(scan.kmers.is_empty());
        }
    }

    #[test]
    fn fused_minimizer_scan_matches_exact_selector_at_ties_resets_and_word_boundaries() {
        let cases = [
            b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".as_slice(),
            b"ACACACACACACACACACACACACACACACACACACACAC".as_slice(),
            b"TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT".as_slice(),
            b"ACGTNACGTYACGTacgtACGTNNNNACGTACGTACGTACGT".as_slice(),
        ];
        let parameters = [
            (3, 1, 0),
            (3, 3, 20),
            (15, 7, 20),
            (31, 15, 20),
            (31, 31, 0),
            (63, 15, 20),
            (64, 32, 0),
            (95, 64, 20),
            (127, 127, 0),
        ];
        for sequence in cases {
            let quality = vec![b'I'; sequence.len()];
            for (k, minimizer_length, minimum_quality) in parameters {
                if usize::from(k) <= sequence.len() {
                    let (fused, oracle, _) = fused_and_oracle_events(
                        sequence,
                        Some(&quality),
                        k,
                        minimizer_length,
                        minimum_quality,
                    );
                    assert_eq!(fused, oracle);
                }
            }
        }

        let non_palindrome = b"AGCTTAGCTAATCGGATCCGATCGTACGATCGATCGGCTAACGT";
        let reverse = crate::dna::reverse_complement(non_palindrome).unwrap();
        let (forward_events, forward_oracle, _) =
            fused_and_oracle_events(non_palindrome, None, 31, 15, 0);
        let (mut reverse_events, reverse_oracle, _) =
            fused_and_oracle_events(&reverse, None, 31, 15, 0);
        assert_eq!(forward_events, forward_oracle);
        assert_eq!(reverse_events, reverse_oracle);
        reverse_events.reverse();
        assert_eq!(forward_events, reverse_events);

        for k in BOUNDARY_LENGTHS {
            let mut exact_sequence = patterned_sequence(k);
            exact_sequence.extend(patterned_sequence(k));
            for minimizer_length in [1, k.div_ceil(2), k] {
                let (fused, oracle, _) =
                    fused_and_oracle_events(&exact_sequence, None, k, minimizer_length, 0);
                assert_eq!(fused, oracle);
                assert!(!fused.is_empty());
            }

            let mut sequence = exact_sequence;
            sequence[usize::from(k)] = b'N';
            let mut quality = vec![b'I'; sequence.len()];
            quality[usize::from(k) / 2] = b'!';
            for minimizer_length in [1, k.div_ceil(2), k] {
                let (fused, oracle, _) =
                    fused_and_oracle_events(&sequence, Some(&quality), k, minimizer_length, 20);
                assert_eq!(fused, oracle);
            }
        }
    }

    #[test]
    fn invalid_input_and_lengths_use_typed_errors() {
        assert_eq!(
            validate_k(2).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidK
        );
        assert_eq!(
            encode_kmer(&[b'A'; 128]).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidK
        );
        assert_eq!(
            encode_kmer(b"AAU").unwrap_err().code(),
            ErrorCode::InputNucleotide
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", Some(b"II"), 3, 0)
                .unwrap_err()
                .code(),
            ErrorCode::InputFastqStructure
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", Some(b"II\n"), 3, 0)
                .unwrap_err()
                .code(),
            ErrorCode::InputQuality
        );
        assert_eq!(
            scan_canonical_kmers(b"AAA", None, 3, 94)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
    }

    proptest! {
        #[test]
        fn arbitrary_exact_strings_round_trip_and_canonicalize(
            symbols in prop::collection::vec(0_u8..4, 0..=127)
        ) {
            let sequence = sequence_from_symbols(&symbols);
            let length = sequence.len() as u8;
            let code = encode_exact_bases(&sequence).unwrap();
            let reverse = reverse_complement_code(code, length).unwrap();
            prop_assert_eq!(decode_mer(code, length).unwrap(), sequence.clone());
            prop_assert_eq!(reverse_complement_code(reverse, length).unwrap(), code);
            prop_assert_eq!(canonical_code(code, length).unwrap(), canonical_code(reverse, length).unwrap());
            prop_assert_eq!(decode_mer(canonical_code(code, length).unwrap(), length).unwrap(), canonical_string(&sequence));
        }

        #[test]
        fn rolling_scan_matches_independent_boundary_oracle(
            values in prop::collection::vec((0_u8..=5, 0_u8..=2), 0..160),
            boundary in 0_usize..4,
            minimum_quality in 0_u8..=40,
        ) {
            let k = BOUNDARY_LENGTHS[boundary];
            let sequence: Vec<u8> = values
                .iter()
                .map(|(symbol, _)| match symbol {
                    0 => b'A', 1 => b'C', 2 => b'G', 3 => b'T', 4 => b'N', _ => b'R',
                })
                .collect();
            let quality: Vec<u8> = values
                .iter()
                .map(|(_, quality)| 33 + quality * 30)
                .collect();
            let observed = scan_canonical_kmers(
                &sequence,
                Some(&quality),
                k,
                minimum_quality,
            ).unwrap();
            let (expected_strings, expected_stats) =
                naive_scan(&sequence, Some(&quality), k, minimum_quality);
            let observed_strings: Vec<Vec<u8>> = observed
                .kmers
                .iter()
                .map(|&code| decode_mer(code, k).unwrap())
                .collect();
            prop_assert_eq!(observed.stats, expected_stats);
            prop_assert_eq!(observed_strings, expected_strings);
        }

        #[test]
        fn stable_u128_path_is_identical_through_k63(
            symbols in prop::collection::vec(0_u8..4, 3..=63)
        ) {
            let sequence = sequence_from_symbols(&symbols);
            let k = sequence.len() as u8;
            let stable = crate::dna::encode_kmer(&sequence).unwrap();
            let wide = encode_kmer(&sequence).unwrap();
            prop_assert_eq!(wide.as_u128(), Some(stable));
            prop_assert_eq!(
                canonical_code(wide, k).unwrap().as_u128(),
                Some(crate::dna::canonical_code(stable, k).unwrap())
            );
            prop_assert_eq!(
                reverse_complement_code(wide, k).unwrap().as_u128(),
                Some(crate::dna::reverse_complement_code(stable, k).unwrap())
            );
        }

        #[test]
        fn stable_scanner_qc_is_identical_through_k63(
            values in prop::collection::vec((0_u8..=5, 0_u8..=2), 0..96),
            requested_k in 3_u8..=63,
            minimum_quality in 0_u8..=40,
        ) {
            let sequence: Vec<u8> = values
                .iter()
                .map(|(symbol, _)| match symbol {
                    0 => b'A', 1 => b'C', 2 => b'G', 3 => b'T', 4 => b'N', _ => b'R',
                })
                .collect();
            let quality: Vec<u8> = values
                .iter()
                .map(|(_, quality)| 33 + quality * 30)
                .collect();
            let stable = crate::dna::scan_canonical_kmers(
                &sequence,
                Some(&quality),
                requested_k,
                minimum_quality,
            ).unwrap();
            let wide = scan_canonical_kmers(
                &sequence,
                Some(&quality),
                requested_k,
                minimum_quality,
            ).unwrap();
            let wide_as_narrow: Vec<u128> = wide
                .kmers
                .iter()
                .map(|key| key.as_u128().unwrap())
                .collect();
            prop_assert_eq!(wide.stats, stable.stats);
            prop_assert_eq!(wide_as_narrow, stable.kmers);
        }

        #[test]
        fn fused_minimizer_events_match_literal_selector_for_arbitrary_qc_reads(
            values in prop::collection::vec((0_u8..=15, 0_u8..=93), 0..192),
            requested_k in 3_u8..=127,
            requested_m in 1_u8..=127,
            minimum_quality in 0_u8..=40,
        ) {
            let sequence: Vec<u8> = values
                .iter()
                .map(|(symbol, _)| match symbol {
                    0 => b'A', 1 => b'C', 2 => b'G', 3 => b'T',
                    4 => b'a', 5 => b'c', 6 => b'g', 7 => b't',
                    8 => b'N', 9 => b'R', 10 => b'Y', 11 => b'S',
                    12 => b'W', 13 => b'K', 14 => b'M', _ => b'V',
                })
                .collect();
            let quality: Vec<u8> = values
                .iter()
                .map(|(_, quality)| 33 + quality)
                .collect();
            let minimizer_length = 1 + (requested_m - 1) % requested_k;
            let (fused, oracle, _) = fused_and_oracle_events(
                &sequence,
                Some(&quality),
                requested_k,
                minimizer_length,
                minimum_quality,
            );
            prop_assert_eq!(fused, oracle);
        }
    }
}
