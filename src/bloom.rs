//! Experimental, exactness-preserving Bloom-filter primitives.
//!
//! This module is deliberately outside the stable assembly path. A Bloom hit
//! is never a count, graph edge, sequence call, or biological observation. The
//! two-hit coordinator below can only nominate a superset for a second,
//! full-key exact recount. Its containment argument requires a complete,
//! ordered first pass and a complete second pass over the same immutable
//! fragment stream.
//!
//! These types are research prototypes, not an adventitious-agent detector or
//! an absence test. Their default constructors enforce a conservative total
//! memory ceiling. Callers that deliberately need a different ceiling must use
//! the explicit `*_with_memory_budget` constructors; every prototype-owned
//! heap reservation is fallible and checked against that declared budget.

use crate::config::SupportUnit;
use crate::dna::{active_mask, canonical_validated, validate_k};
use crate::error::{ErrorCode, Result, VeritasmError};
use sha2::{Digest, Sha256};

pub const BLOOM_HASH_ID: &str = "sha256-double-hash-v1";
pub const BLOOM_PROBE_DOMAIN: &[u8] = b"veritasm:bloom-probes:v1\0";
pub const SEEN_ONCE_DOMAIN: &[u8] = b"veritasm:two-hit:seen-once:v1";
pub const SEEN_TWICE_DOMAIN: &[u8] = b"veritasm:two-hit:seen-twice:v1";
pub const DEFAULT_BLOOM_SEED: [u8; 32] = [0; 32];
/// Default total accounting ceiling for one experimental Bloom primitive or
/// two-hit sieve (64 MiB).
///
/// This is an API safety ceiling, not an empirically justified sizing
/// recommendation. A caller may choose another ceiling explicitly.
pub const DEFAULT_BLOOM_MEMORY_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

const KEY_BYTES: u64 = std::mem::size_of::<u128>() as u64;
const PROBE_BYTES: u64 = std::mem::size_of::<u64>() as u64;

/// Fixed-size, insertion-only classic Bloom filter.
///
/// The structure intentionally implements no deletion, resizing, union, or
/// serialization. It is owned and mutated by one coordinator.
#[derive(Debug)]
pub struct BloomFilter {
    bits: Vec<u64>,
    bit_count: u64,
    probe_count: u8,
    k: u8,
    domain: Vec<u8>,
    seed: [u8; 32],
    inserted_events: u64,
    memory_budget_bytes: u64,
    accounted_resident_bytes: u64,
}

impl BloomFilter {
    /// Allocate a deterministic filter under the 64-MiB prototype ceiling.
    pub fn new(
        k: u8,
        bit_count: u64,
        probe_count: u8,
        domain: &[u8],
        seed: [u8; 32],
    ) -> Result<Self> {
        Self::new_with_memory_budget(
            k,
            bit_count,
            probe_count,
            domain,
            seed,
            DEFAULT_BLOOM_MEMORY_BUDGET_BYTES,
        )
    }

    /// Allocate a deterministic filter under an explicit accounting ceiling.
    ///
    /// The ceiling covers the returned value, both owned vector capacities,
    /// and one simultaneous `probe_positions` result. It does not account for
    /// the caller's input slice, allocator metadata, thread stacks, or copies
    /// made by the caller after this method returns.
    pub fn new_with_memory_budget(
        k: u8,
        bit_count: u64,
        probe_count: u8,
        domain: &[u8],
        seed: [u8; 32],
        memory_budget_bytes: u64,
    ) -> Result<Self> {
        validate_k(k)?;
        let plan = BloomAllocationPlan::new(bit_count, probe_count, domain.len())?;
        if seed != DEFAULT_BLOOM_SEED {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationUnsupportedCombination,
                "sha256-double-hash-v1 requires the frozen 32-byte zero seed",
            ));
        }
        ensure_memory_budget(
            plan.minimum_total_bytes,
            memory_budget_bytes,
            "Bloom filter",
        )?;

        let mut owned_domain = Vec::new();
        owned_domain
            .try_reserve_exact(domain.len())
            .map_err(|error| allocation_error("Bloom structure domain", error))?;
        owned_domain.extend_from_slice(domain);

        let word_count = plan.word_count;
        let mut bits = Vec::new();
        bits.try_reserve_exact(word_count)
            .map_err(|error| allocation_error("Bloom bit vector", error))?;
        bits.resize(word_count, 0);

        let accounted_resident_bytes =
            bloom_resident_bytes(bits.capacity(), owned_domain.capacity())?;
        let actual_total = accounted_resident_bytes
            .checked_add(plan.probe_scratch_bytes)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "Bloom actual-memory accounting overflow",
                )
            })?;
        ensure_memory_budget(actual_total, memory_budget_bytes, "Bloom filter")?;

        Ok(Self {
            bits,
            bit_count,
            probe_count,
            k,
            domain: owned_domain,
            seed,
            inserted_events: 0,
            memory_budget_bytes,
            accounted_resident_bytes,
        })
    }

    pub const fn bit_count(&self) -> u64 {
        self.bit_count
    }

    pub const fn probe_count(&self) -> u8 {
        self.probe_count
    }

    pub const fn k(&self) -> u8 {
        self.k
    }

    pub fn domain(&self) -> &[u8] {
        &self.domain
    }

    pub const fn seed(&self) -> &[u8; 32] {
        &self.seed
    }

    pub const fn inserted_events(&self) -> u64 {
        self.inserted_events
    }

    /// Declared accounting ceiling used by this instance.
    pub const fn memory_budget_bytes(&self) -> u64 {
        self.memory_budget_bytes
    }

    /// Conservative accounted bytes resident in the returned value.
    ///
    /// This includes the struct and the actual vector capacities, but excludes
    /// allocator metadata and stack memory.
    pub const fn accounted_resident_bytes(&self) -> u64 {
        self.accounted_resident_bytes
    }

    pub fn allocation_bytes(&self) -> Result<u64> {
        u64::try_from(self.bits.len())
            .ok()
            .and_then(|words| words.checked_mul(8))
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "Bloom allocation-byte accounting overflow",
                )
            })
    }

    pub fn occupied_bits(&self) -> u64 {
        self.bits
            .iter()
            .map(|word| u64::from(word.count_ones()))
            .sum()
    }

    /// Return all distinct probe positions in probe order.
    pub fn probe_positions(&self, key: u128) -> Result<Vec<u64>> {
        let (position, step) = self.probe_parameters(key)?;
        let probe_bytes = u64::from(self.probe_count)
            .checked_mul(PROBE_BYTES)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "Bloom probe-vector byte accounting overflow",
                )
            })?;
        let available = self
            .memory_budget_bytes
            .checked_sub(self.accounted_resident_bytes)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "Bloom resident accounting exceeds its declared memory budget",
                )
            })?;
        ensure_memory_budget(probe_bytes, available, "Bloom probe vector")?;

        let mut positions = Vec::new();
        positions
            .try_reserve_exact(usize::from(self.probe_count))
            .map_err(|error| allocation_error("Bloom probe vector", error))?;
        let actual_probe_bytes =
            vector_capacity_bytes::<u64>(positions.capacity(), "Bloom probe vector")?;
        ensure_memory_budget(actual_probe_bytes, available, "Bloom probe vector")?;
        for index in 0..self.probe_count {
            let probe = probe_at(position, step, index, self.bit_count)?;
            if positions.contains(&probe) {
                return Err(VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    format!("Bloom probe position {probe} repeated"),
                ));
            }
            positions.push(probe);
        }
        Ok(positions)
    }

    /// Query membership. `true` remains only a probabilistic result.
    pub fn contains(&self, key: u128) -> Result<bool> {
        let (position, step) = self.probe_parameters(key)?;
        for index in 0..self.probe_count {
            let bit = probe_at(position, step, index, self.bit_count)?;
            let (word, mask) = bit_location(bit)?;
            let value = self.bits.get(word).ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "Bloom query addressed a word outside the allocated filter",
                )
            })?;
            if value & mask == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Insert a key and complete all bit writes before incrementing telemetry.
    pub fn insert(&mut self, key: u128) -> Result<()> {
        let (position, step) = self.probe_parameters(key)?;
        let next_inserted_events = self.inserted_events.checked_add(1).ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "Bloom inserted-event count overflow",
            )
        })?;
        for index in 0..self.probe_count {
            let bit = probe_at(position, step, index, self.bit_count)?;
            let (word, mask) = bit_location(bit)?;
            let value = self.bits.get_mut(word).ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "Bloom insertion addressed a word outside the allocated filter",
                )
            })?;
            *value |= mask;
        }
        self.inserted_events = next_inserted_events;
        Ok(())
    }

    fn probe_parameters(&self, key: u128) -> Result<(u64, u64)> {
        self.validate_key(key)?;

        let domain_length = u16::try_from(self.domain.len()).map_err(|_| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "validated Bloom domain length no longer fits u16",
            )
        })?;
        let mut hasher = Sha256::new();
        hasher.update(BLOOM_PROBE_DOMAIN);
        hasher.update(domain_length.to_le_bytes());
        hasher.update(&self.domain);
        hasher.update(self.seed);
        hasher.update(key.to_be_bytes());
        let digest = hasher.finalize();

        let mut first_bytes = [0_u8; 8];
        first_bytes.copy_from_slice(&digest[..8]);
        let mut second_bytes = [0_u8; 8];
        second_bytes.copy_from_slice(&digest[8..16]);
        let h1 = u64::from_le_bytes(first_bytes);
        let h2 = u64::from_le_bytes(second_bytes);

        let position = h1 % self.bit_count;
        let mut step = 1 + h2 % (self.bit_count - 1);
        while gcd(step, self.bit_count) != 1 {
            step = if step == self.bit_count - 1 {
                1
            } else {
                step + 1
            };
        }
        Ok((position, step))
    }

    fn validate_key(&self, key: u128) -> Result<()> {
        if key & !active_mask(self.k) != 0 {
            return Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                format!("Bloom key has nonzero inactive high bits for k={}", self.k),
            ));
        }
        if canonical_validated(key, self.k) != key {
            return Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                format!("Bloom key is not canonical for k={}", self.k),
            ));
        }
        Ok(())
    }
}

/// Same-process ordered coordinator for the experimental two-hit sieve.
///
/// The coordinator starts at fragment ordinal zero, enforces contiguous input
/// order, and refuses candidate queries until the declared first-pass fragment
/// count has been observed and sealed. Keys are sorted and deduplicated inside
/// each fragment before either filter is touched. These constraints prevent
/// duplicate windows or overlapping mates from manufacturing a second
/// fragment hit, and prevent query/insert races from causing a false negative.
///
/// Containment argument: a key present in at least two supplied fragments is
/// inserted into `seen_once` by its first fragment. In an insertion-only,
/// intact filter, its second fragment cannot receive a false-negative query,
/// so that observation inserts it into `seen_twice`. The complete recount then
/// routes every occurrence because `seen_twice` is also insertion-only and
/// intact. Full-key exact counting recovers the true fragment support; Bloom
/// false positives can only route extra keys that the exact threshold removes.
/// This argument additionally depends on identical QC/deduplication and the
/// same immutable stream in both complete passes; the caller must verify those
/// spool-level premises.
///
/// `fragment_local_duplicates_do_not_create_a_second_hit` exercises local
/// deduplication; `ordinals_and_complete_sealing_are_enforced` exercises the
/// ordered-completion guard; `every_exact_two_hit_key_is_a_candidate` tests
/// containment; and
/// `exhaustive_collision_heavy_retained_stream_matches_exact_oracle` tests the
/// final byte stream against exact counting with an all-ones collision case.
#[derive(Debug)]
pub struct TwoHitSieve {
    seen_once: BloomFilter,
    seen_twice: BloomFilter,
    expected_fragments: u64,
    next_fragment_ordinal: u64,
    minimum_support: u64,
    sealed: bool,
    memory_budget_bytes: u64,
    accounted_resident_bytes: u64,
}

/// Complete experimental two-hit sieve configuration.
#[derive(Debug, Clone, Copy)]
pub struct TwoHitConfig {
    pub k: u8,
    pub seen_once_bits: u64,
    pub seen_twice_bits: u64,
    pub probe_count: u8,
    pub seed: [u8; 32],
    pub expected_fragments: u64,
    pub support_unit: SupportUnit,
    pub minimum_support: u64,
}

impl TwoHitSieve {
    /// Construct the prototype under the shared 64-MiB default ceiling.
    pub fn new(config: TwoHitConfig) -> Result<Self> {
        Self::new_with_memory_budget(config, DEFAULT_BLOOM_MEMORY_BUDGET_BYTES)
    }

    /// Construct the two filters under one explicit accounting ceiling.
    ///
    /// Admission is evaluated for both filters together before either large
    /// allocation is attempted. The ceiling also limits per-call copies made
    /// while sorting/deduplicating keys and collecting candidates. It excludes
    /// the caller-owned input slice, allocator metadata, and thread stacks.
    pub fn new_with_memory_budget(config: TwoHitConfig, memory_budget_bytes: u64) -> Result<Self> {
        if config.support_unit != SupportUnit::SuppliedFragmentInstance
            || config.minimum_support < 2
        {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationUnsupportedCombination,
                "the two-hit sieve requires supplied-fragment-instance support and min-support>=2",
            ));
        }
        validate_k(config.k)?;
        if config.seed != DEFAULT_BLOOM_SEED {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationUnsupportedCombination,
                "sha256-double-hash-v1 requires the frozen 32-byte zero seed",
            ));
        }

        let once_plan = BloomAllocationPlan::new(
            config.seen_once_bits,
            config.probe_count,
            SEEN_ONCE_DOMAIN.len(),
        )?;
        let twice_plan = BloomAllocationPlan::new(
            config.seen_twice_bits,
            config.probe_count,
            SEEN_TWICE_DOMAIN.len(),
        )?;
        let planned_resident = two_hit_planned_resident_bytes(&once_plan, &twice_plan)?;
        let planned_total = planned_resident
            .checked_add(once_plan.probe_scratch_bytes)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "two-hit Bloom planned-memory accounting overflow",
                )
            })?;
        ensure_memory_budget(planned_total, memory_budget_bytes, "two-hit Bloom sieve")?;

        // Reserve the second filter's planned resident bytes before allocating
        // the first. If the allocator grants a larger-than-requested capacity,
        // that actual capacity reduces the second filter's budget before its
        // allocation begins.
        let filter_struct_bytes =
            u64::try_from(std::mem::size_of::<BloomFilter>()).map_err(|_| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "Bloom struct-size accounting overflow",
                )
            })?;
        let outer_only_bytes = u64::try_from(std::mem::size_of::<TwoHitSieve>())
            .ok()
            .and_then(|outer| outer.checked_sub(filter_struct_bytes.checked_mul(2)?))
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "two-hit Bloom struct accounting underflow",
                )
            })?;
        let twice_planned_resident = twice_plan
            .minimum_total_bytes
            .checked_sub(twice_plan.probe_scratch_bytes)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "seen-twice Bloom plan is smaller than probe scratch",
                )
            })?;
        let once_resident_budget = memory_budget_bytes
            .checked_sub(outer_only_bytes)
            .and_then(|value| value.checked_sub(twice_planned_resident))
            .and_then(|value| value.checked_sub(once_plan.probe_scratch_bytes))
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "combined Bloom admission did not leave the planned first-filter budget",
                )
            })?;
        let once_constructor_budget = once_resident_budget
            .checked_add(once_plan.probe_scratch_bytes)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "seen-once Bloom constructor-budget overflow",
                )
            })?;
        let seen_once = BloomFilter::new_with_memory_budget(
            config.k,
            config.seen_once_bits,
            config.probe_count,
            SEEN_ONCE_DOMAIN,
            config.seed,
            once_constructor_budget,
        )?;
        let twice_constructor_budget = memory_budget_bytes
            .checked_sub(outer_only_bytes)
            .and_then(|value| value.checked_sub(seen_once.accounted_resident_bytes))
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "seen-once Bloom exceeded the admitted aggregate resident budget",
                )
            })?;
        let seen_twice = BloomFilter::new_with_memory_budget(
            config.k,
            config.seen_twice_bits,
            config.probe_count,
            SEEN_TWICE_DOMAIN,
            config.seed,
            twice_constructor_budget,
        )?;
        let accounted_resident_bytes = two_hit_resident_bytes(&seen_once, &seen_twice)?;
        let actual_total = accounted_resident_bytes
            .checked_add(once_plan.probe_scratch_bytes)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "two-hit Bloom actual-memory accounting overflow",
                )
            })?;
        ensure_memory_budget(actual_total, memory_budget_bytes, "two-hit Bloom sieve")?;

        Ok(Self {
            seen_once,
            seen_twice,
            expected_fragments: config.expected_fragments,
            next_fragment_ordinal: 0,
            minimum_support: config.minimum_support,
            sealed: false,
            memory_budget_bytes,
            accounted_resident_bytes,
        })
    }

    pub const fn expected_fragments(&self) -> u64 {
        self.expected_fragments
    }

    pub const fn observed_fragments(&self) -> u64 {
        self.next_fragment_ordinal
    }

    pub const fn minimum_support(&self) -> u64 {
        self.minimum_support
    }

    pub const fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// Declared accounting ceiling shared by both filters and call scratch.
    pub const fn memory_budget_bytes(&self) -> u64 {
        self.memory_budget_bytes
    }

    /// Conservative bytes resident in the two-filter value.
    pub const fn accounted_resident_bytes(&self) -> u64 {
        self.accounted_resident_bytes
    }

    pub const fn seen_once(&self) -> &BloomFilter {
        &self.seen_once
    }

    pub const fn seen_twice(&self) -> &BloomFilter {
        &self.seen_twice
    }

    /// Apply one stateful first-pass update in exact fragment order.
    pub fn observe_fragment(&mut self, fragment_ordinal: u64, keys: &[u128]) -> Result<()> {
        if self.sealed {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                "two-hit first pass is already sealed",
            ));
        }
        if fragment_ordinal != self.next_fragment_ordinal {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                format!(
                    "expected Bloom first-pass fragment ordinal {}, received {fragment_ordinal}",
                    self.next_fragment_ordinal
                ),
            ));
        }
        if self.next_fragment_ordinal >= self.expected_fragments {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                format!(
                    "Bloom first pass received more than {} declared fragments",
                    self.expected_fragments
                ),
            ));
        }

        let scratch_budget = self.available_scratch_bytes()?;
        let mut distinct = copy_keys_with_budget(keys, scratch_budget, "Bloom fragment key copy")?;
        distinct.sort_unstable();
        distinct.dedup();

        // Validate the entire fragment and reserve counter headroom before
        // mutating either filter. Any malformed key or telemetry overflow is
        // therefore a fail-closed fragment error, not a partially applied
        // update that a caller could accidentally retry.
        for &key in &distinct {
            self.seen_once.validate_key(key)?;
            self.seen_twice.validate_key(key)?;
        }
        let event_upper_bound = u64::try_from(distinct.len()).map_err(|_| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "fragment-distinct Bloom key count does not fit u64",
            )
        })?;
        self.seen_once
            .inserted_events
            .checked_add(event_upper_bound)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "seen-once inserted-event count would overflow",
                )
            })?;
        self.seen_twice
            .inserted_events
            .checked_add(event_upper_bound)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "seen-twice inserted-event count could overflow",
                )
            })?;
        let next_fragment_ordinal = self.next_fragment_ordinal.checked_add(1).ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "Bloom fragment-ordinal count overflow",
            )
        })?;
        for key in distinct {
            if self.seen_once.contains(key)? {
                self.seen_twice.insert(key)?;
            }
            self.seen_once.insert(key)?;
        }
        self.next_fragment_ordinal = next_fragment_ordinal;
        Ok(())
    }

    /// Seal a complete first pass. No serialized/reusable state is produced.
    pub fn finish_first_pass(&mut self) -> Result<()> {
        if self.sealed {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                "two-hit first pass was sealed more than once",
            ));
        }
        if self.next_fragment_ordinal != self.expected_fragments {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                format!(
                    "two-hit first pass observed {} of {} declared fragments",
                    self.next_fragment_ordinal, self.expected_fragments
                ),
            ));
        }
        self.sealed = true;
        Ok(())
    }

    /// Second-pass candidate predicate. A positive result only routes the full
    /// key to an exact counter; it cannot be used as retained membership.
    pub fn is_candidate(&self, key: u128) -> Result<bool> {
        self.require_sealed()?;
        self.seen_twice.contains(key)
    }

    /// Sort/deduplicate one second-pass fragment and return its candidate keys.
    pub fn candidate_keys_for_fragment(&self, keys: &[u128]) -> Result<Vec<u128>> {
        self.require_sealed()?;
        let scratch_budget = self.available_scratch_bytes()?;
        let requested_key_bytes = key_vector_bytes(keys.len(), "Bloom candidate input")?;
        let worst_case_bytes = requested_key_bytes.checked_mul(2).ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "Bloom candidate scratch accounting overflow",
            )
        })?;
        ensure_memory_budget(worst_case_bytes, scratch_budget, "Bloom candidate scratch")?;

        let mut distinct = copy_keys_with_budget(keys, scratch_budget, "Bloom candidate key copy")?;
        distinct.sort_unstable();
        distinct.dedup();

        for &key in &distinct {
            self.seen_twice.validate_key(key)?;
        }

        let distinct_bytes =
            vector_capacity_bytes::<u128>(distinct.capacity(), "Bloom candidate key copy")?;
        let candidate_budget = scratch_budget.checked_sub(distinct_bytes).ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "Bloom candidate key copy exceeded admitted scratch",
            )
        })?;
        let mut candidates = Vec::new();
        let candidate_requested = key_vector_bytes(distinct.len(), "Bloom candidate result")?;
        ensure_memory_budget(
            candidate_requested,
            candidate_budget,
            "Bloom candidate result",
        )?;
        candidates
            .try_reserve_exact(distinct.len())
            .map_err(|error| allocation_error("Bloom candidate result", error))?;
        let actual_candidate_bytes =
            vector_capacity_bytes::<u128>(candidates.capacity(), "Bloom candidate result")?;
        ensure_memory_budget(
            actual_candidate_bytes,
            candidate_budget,
            "Bloom candidate result",
        )?;
        for key in distinct {
            if self.seen_twice.contains(key)? {
                candidates.push(key);
            }
        }
        Ok(candidates)
    }

    fn available_scratch_bytes(&self) -> Result<u64> {
        self.memory_budget_bytes
            .checked_sub(self.accounted_resident_bytes)
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "two-hit Bloom resident accounting exceeds its declared memory budget",
                )
            })
    }

    fn require_sealed(&self) -> Result<()> {
        if self.sealed {
            Ok(())
        } else {
            Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                format!(
                    "two-hit candidate query requires a complete first pass; observed {} of {} fragments",
                    self.next_fragment_ordinal, self.expected_fragments
                ),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BloomAllocationPlan {
    word_count: usize,
    bit_payload_bytes: u64,
    domain_payload_bytes: u64,
    probe_scratch_bytes: u64,
    minimum_total_bytes: u64,
}

impl BloomAllocationPlan {
    fn new(bit_count: u64, probe_count: u8, domain_length: usize) -> Result<Self> {
        if bit_count < 64 || bit_count % 64 != 0 {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                format!(
                    "Bloom bit count must be at least 64 and a multiple of 64; received {bit_count}"
                ),
            ));
        }
        if !(1..=64).contains(&probe_count) {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                format!("Bloom probe count must be in 1..=64; received {probe_count}"),
            ));
        }
        if domain_length == 0 || domain_length > usize::from(u16::MAX) {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                format!(
                    "Bloom structure domain length must be in 1..={}; received {domain_length}",
                    u16::MAX
                ),
            ));
        }

        let word_count = usize::try_from(bit_count / 64).map_err(|_| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                "Bloom word count does not fit the platform address space",
            )
        })?;
        let bit_payload_bytes = u64::try_from(word_count)
            .ok()
            .and_then(|words| words.checked_mul(std::mem::size_of::<u64>() as u64))
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "Bloom bit-vector byte accounting overflow",
                )
            })?;
        let domain_payload_bytes = u64::try_from(domain_length).map_err(|_| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "Bloom domain byte accounting overflow",
            )
        })?;
        let probe_scratch_bytes =
            u64::from(probe_count)
                .checked_mul(PROBE_BYTES)
                .ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::ResourceIntegerOverflow,
                        "Bloom probe scratch accounting overflow",
                    )
                })?;
        let minimum_total_bytes = u64::try_from(std::mem::size_of::<BloomFilter>())
            .ok()
            .and_then(|value| value.checked_add(bit_payload_bytes))
            .and_then(|value| value.checked_add(domain_payload_bytes))
            .and_then(|value| value.checked_add(probe_scratch_bytes))
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::ResourceIntegerOverflow,
                    "Bloom minimum-memory accounting overflow",
                )
            })?;

        Ok(Self {
            word_count,
            bit_payload_bytes,
            domain_payload_bytes,
            probe_scratch_bytes,
            minimum_total_bytes,
        })
    }
}

fn two_hit_planned_resident_bytes(
    once: &BloomAllocationPlan,
    twice: &BloomAllocationPlan,
) -> Result<u64> {
    u64::try_from(std::mem::size_of::<TwoHitSieve>())
        .ok()
        .and_then(|value| value.checked_add(once.bit_payload_bytes))
        .and_then(|value| value.checked_add(once.domain_payload_bytes))
        .and_then(|value| value.checked_add(twice.bit_payload_bytes))
        .and_then(|value| value.checked_add(twice.domain_payload_bytes))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "two-hit Bloom planned-resident accounting overflow",
            )
        })
}

fn two_hit_resident_bytes(once: &BloomFilter, twice: &BloomFilter) -> Result<u64> {
    let filter_struct_bytes = u64::try_from(std::mem::size_of::<BloomFilter>()).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            "Bloom struct-size accounting overflow",
        )
    })?;
    let once_heap = once
        .accounted_resident_bytes
        .checked_sub(filter_struct_bytes)
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "seen-once Bloom resident accounting is smaller than its struct",
            )
        })?;
    let twice_heap = twice
        .accounted_resident_bytes
        .checked_sub(filter_struct_bytes)
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "seen-twice Bloom resident accounting is smaller than its struct",
            )
        })?;
    u64::try_from(std::mem::size_of::<TwoHitSieve>())
        .ok()
        .and_then(|value| value.checked_add(once_heap))
        .and_then(|value| value.checked_add(twice_heap))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "two-hit Bloom actual-resident accounting overflow",
            )
        })
}

fn bloom_resident_bytes(bits_capacity: usize, domain_capacity: usize) -> Result<u64> {
    u64::try_from(std::mem::size_of::<BloomFilter>())
        .ok()
        .and_then(|value| {
            vector_capacity_bytes::<u64>(bits_capacity, "Bloom bit vector")
                .ok()
                .and_then(|bytes| value.checked_add(bytes))
        })
        .and_then(|value| {
            vector_capacity_bytes::<u8>(domain_capacity, "Bloom structure domain")
                .ok()
                .and_then(|bytes| value.checked_add(bytes))
        })
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "Bloom resident-memory accounting overflow",
            )
        })
}

fn vector_capacity_bytes<T>(capacity: usize, context: &str) -> Result<u64> {
    u64::try_from(capacity)
        .ok()
        .and_then(|capacity| capacity.checked_mul(std::mem::size_of::<T>() as u64))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                format!("{context} capacity-byte accounting overflow"),
            )
        })
}

fn key_vector_bytes(length: usize, context: &str) -> Result<u64> {
    u64::try_from(length)
        .ok()
        .and_then(|count| count.checked_mul(KEY_BYTES))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                format!("{context} byte accounting overflow"),
            )
        })
}

fn copy_keys_with_budget(keys: &[u128], budget: u64, context: &str) -> Result<Vec<u128>> {
    let requested = key_vector_bytes(keys.len(), context)?;
    ensure_memory_budget(requested, budget, context)?;
    let mut copy = Vec::new();
    copy.try_reserve_exact(keys.len())
        .map_err(|error| allocation_error(context, error))?;
    let actual = vector_capacity_bytes::<u128>(copy.capacity(), context)?;
    ensure_memory_budget(actual, budget, context)?;
    copy.extend_from_slice(keys);
    Ok(copy)
}

fn ensure_memory_budget(required: u64, budget: u64, context: &str) -> Result<()> {
    if required <= budget {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "{context} requires {required} accounted bytes but the declared budget is {budget} bytes"
            ),
        ))
    }
}

fn allocation_error(context: &str, error: std::collections::TryReserveError) -> VeritasmError {
    VeritasmError::new(
        ErrorCode::ResourceMemory,
        format!("unable to reserve {context}: {error}"),
    )
}

fn probe_at(position: u64, step: u64, index: u8, bit_count: u64) -> Result<u64> {
    let offset = u128::from(index)
        .checked_mul(u128::from(step))
        .and_then(|value| value.checked_add(u128::from(position)))
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::ResourceIntegerOverflow,
                "Bloom probe arithmetic overflow",
            )
        })?;
    u64::try_from(offset % u128::from(bit_count)).map_err(|_| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "Bloom probe did not fit u64 after reduction",
        )
    })
}

fn bit_location(bit: u64) -> Result<(usize, u64)> {
    let word = usize::try_from(bit / 64).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            "Bloom word index does not fit the platform address space",
        )
    })?;
    Ok((word, 1_u64 << (bit % 64)))
}

const fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::KmerCount;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    fn exact_retained(fragments: &[Vec<u128>], minimum_support: u64) -> Vec<KmerCount> {
        let mut counts = BTreeMap::<u128, u64>::new();
        for fragment in fragments {
            let mut distinct = fragment.clone();
            distinct.sort_unstable();
            distinct.dedup();
            for key in distinct {
                *counts.entry(key).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .filter_map(|(key, support)| {
                (support >= minimum_support).then_some(KmerCount { key, support })
            })
            .collect()
    }

    fn retained_stream_bytes(counts: &[KmerCount]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(counts.len() * 24);
        for count in counts {
            bytes.extend_from_slice(&count.key.to_be_bytes());
            bytes.extend_from_slice(&count.support.to_le_bytes());
        }
        bytes
    }

    fn sieve_retained(
        fragments: &[Vec<u128>],
        minimum_support: u64,
        bit_count: u64,
        probe_count: u8,
    ) -> Vec<KmerCount> {
        let mut sieve = TwoHitSieve::new(TwoHitConfig {
            k: 3,
            seen_once_bits: bit_count,
            seen_twice_bits: bit_count,
            probe_count,
            seed: DEFAULT_BLOOM_SEED,
            expected_fragments: fragments.len() as u64,
            support_unit: SupportUnit::SuppliedFragmentInstance,
            minimum_support,
        })
        .unwrap();
        for (ordinal, fragment) in fragments.iter().enumerate() {
            sieve.observe_fragment(ordinal as u64, fragment).unwrap();
        }
        sieve.finish_first_pass().unwrap();

        let mut candidate_counts = BTreeMap::<u128, u64>::new();
        for fragment in fragments {
            for key in sieve.candidate_keys_for_fragment(fragment).unwrap() {
                *candidate_counts.entry(key).or_default() += 1;
            }
        }
        candidate_counts
            .into_iter()
            .filter_map(|(key, support)| {
                (support >= minimum_support).then_some(KmerCount { key, support })
            })
            .collect()
    }

    #[test]
    fn dimensions_and_active_bits_are_enforced() {
        assert_eq!(
            BloomFilter::new(3, 63, 2, b"x", DEFAULT_BLOOM_SEED)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        let filter = BloomFilter::new(3, 64, 2, b"x", DEFAULT_BLOOM_SEED).unwrap();
        assert_eq!(
            filter.contains(1_u128 << 20).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
        assert_eq!(
            filter.contains(63).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
        assert_eq!(
            BloomFilter::new(3, 64, 2, b"x", [1; 32])
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationUnsupportedCombination
        );
    }

    #[test]
    fn default_constructor_rejects_an_oversized_filter_before_allocation() {
        let bit_count = DEFAULT_BLOOM_MEMORY_BUDGET_BYTES.checked_mul(8).unwrap();
        let error = BloomFilter::new(3, bit_count, 2, b"x", DEFAULT_BLOOM_SEED).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
        assert!(error.context().contains("declared budget"));
    }

    #[test]
    fn explicit_filter_budget_covers_owned_state_and_probe_reservation() {
        let plan = BloomAllocationPlan::new(1_024, 10, SEEN_ONCE_DOMAIN.len()).unwrap();
        assert_eq!(
            BloomFilter::new_with_memory_budget(
                31,
                1_024,
                10,
                SEEN_ONCE_DOMAIN,
                DEFAULT_BLOOM_SEED,
                plan.minimum_total_bytes - 1,
            )
            .unwrap_err()
            .code(),
            ErrorCode::ResourceMemory
        );

        let budget = plan.minimum_total_bytes + 4_096;
        let filter = BloomFilter::new_with_memory_budget(
            31,
            1_024,
            10,
            SEEN_ONCE_DOMAIN,
            DEFAULT_BLOOM_SEED,
            budget,
        )
        .unwrap();
        assert_eq!(filter.memory_budget_bytes(), budget);
        assert!(filter.accounted_resident_bytes() <= budget);
        assert_eq!(filter.probe_positions(42).unwrap().len(), 10);
    }

    #[test]
    fn domain_admission_is_checked_before_prototype_owned_copying() {
        let domain = vec![b'x'; usize::from(u16::MAX)];
        let error =
            BloomFilter::new_with_memory_budget(3, 64, 2, &domain, DEFAULT_BLOOM_SEED, 1_024)
                .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn two_filter_admission_uses_one_combined_budget() {
        let config = TwoHitConfig {
            k: 3,
            seen_once_bits: 1_024,
            seen_twice_bits: 1_024,
            probe_count: 4,
            seed: DEFAULT_BLOOM_SEED,
            expected_fragments: 0,
            support_unit: SupportUnit::SuppliedFragmentInstance,
            minimum_support: 2,
        };
        let once = BloomAllocationPlan::new(1_024, 4, SEEN_ONCE_DOMAIN.len()).unwrap();
        let twice = BloomAllocationPlan::new(1_024, 4, SEEN_TWICE_DOMAIN.len()).unwrap();
        let combined = two_hit_planned_resident_bytes(&once, &twice)
            .unwrap()
            .checked_add(once.probe_scratch_bytes)
            .unwrap();

        // Either filter fits by itself; their coordinator does not fit under
        // a budget one byte below the combined admission plan.
        BloomFilter::new_with_memory_budget(
            3,
            1_024,
            4,
            SEEN_ONCE_DOMAIN,
            DEFAULT_BLOOM_SEED,
            combined - 1,
        )
        .unwrap();
        assert_eq!(
            TwoHitSieve::new_with_memory_budget(config, combined - 1)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );

        let sieve = TwoHitSieve::new_with_memory_budget(config, combined + 4_096).unwrap();
        assert_eq!(sieve.memory_budget_bytes(), combined + 4_096);
        assert!(sieve.accounted_resident_bytes() <= sieve.memory_budget_bytes());
    }

    #[test]
    fn fragment_and_candidate_copies_fail_with_typed_memory_errors() {
        let config = TwoHitConfig {
            k: 3,
            seen_once_bits: 64,
            seen_twice_bits: 64,
            probe_count: 2,
            seed: DEFAULT_BLOOM_SEED,
            expected_fragments: 1,
            support_unit: SupportUnit::SuppliedFragmentInstance,
            minimum_support: 2,
        };
        let once = BloomAllocationPlan::new(64, 2, SEEN_ONCE_DOMAIN.len()).unwrap();
        let twice = BloomAllocationPlan::new(64, 2, SEEN_TWICE_DOMAIN.len()).unwrap();
        let minimum = two_hit_planned_resident_bytes(&once, &twice)
            .unwrap()
            .checked_add(once.probe_scratch_bytes)
            .unwrap();
        let mut sieve = TwoHitSieve::new_with_memory_budget(config, minimum).unwrap();
        let keys = [canonical_validated(1, 3), canonical_validated(2, 3)];
        assert_eq!(
            sieve.observe_fragment(0, &keys).unwrap_err().code(),
            ErrorCode::ResourceMemory
        );
        assert_eq!(sieve.observed_fragments(), 0);
        assert_eq!(sieve.seen_once().inserted_events(), 0);
        assert_eq!(sieve.seen_twice().inserted_events(), 0);

        let empty_config = TwoHitConfig {
            expected_fragments: 0,
            ..config
        };
        let mut sealed = TwoHitSieve::new_with_memory_budget(empty_config, minimum).unwrap();
        sealed.finish_first_pass().unwrap();
        assert_eq!(
            sealed.candidate_keys_for_fragment(&[0]).unwrap_err().code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn ineligible_support_semantics_cannot_construct_a_two_hit_sieve() {
        for (support_unit, minimum_support) in [
            (SupportUnit::SuppliedFragmentInstance, 1),
            (SupportUnit::AcceptedWindowOccurrence, 2),
        ] {
            assert_eq!(
                TwoHitSieve::new(TwoHitConfig {
                    k: 3,
                    seen_once_bits: 64,
                    seen_twice_bits: 64,
                    probe_count: 2,
                    seed: DEFAULT_BLOOM_SEED,
                    expected_fragments: 0,
                    support_unit,
                    minimum_support,
                })
                .unwrap_err()
                .code(),
                ErrorCode::ConfigurationUnsupportedCombination
            );
        }
    }

    #[test]
    fn minimum_filter_with_64_probes_visits_every_bit_once() {
        let mut filter = BloomFilter::new(3, 64, 64, SEEN_ONCE_DOMAIN, DEFAULT_BLOOM_SEED).unwrap();
        let positions = filter.probe_positions(7).unwrap();
        let mut sorted = positions.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0_u64..64).collect::<Vec<_>>());
        filter.insert(7).unwrap();
        assert!(filter.contains(7).unwrap());
        assert_eq!(filter.occupied_bits(), 64);
    }

    #[test]
    fn structure_domains_change_probe_vectors() {
        let once = BloomFilter::new(31, 1_024, 10, SEEN_ONCE_DOMAIN, DEFAULT_BLOOM_SEED).unwrap();
        let twice = BloomFilter::new(31, 1_024, 10, SEEN_TWICE_DOMAIN, DEFAULT_BLOOM_SEED).unwrap();
        assert_ne!(
            once.probe_positions(42).unwrap(),
            twice.probe_positions(42).unwrap()
        );
    }

    #[test]
    fn sha256_probe_contract_has_a_cross_platform_golden_vector() {
        // The SHA-256 preimage is the frozen prefix, little-endian domain
        // length 29, exact seen-once domain, 32 zero seed bytes, and the
        // 16-byte big-endian representation of integer 42. Its digest is
        // 2952d662698d1396433830d07b4cbadc743012b81ba4bff4b66b65ca6cd953e7.
        let filter = BloomFilter::new(31, 1_024, 10, SEEN_ONCE_DOMAIN, DEFAULT_BLOOM_SEED).unwrap();
        assert_eq!(
            filter.probe_positions(42).unwrap(),
            vec![553, 758, 963, 144, 349, 554, 759, 964, 145, 350]
        );
    }

    #[test]
    fn fragment_local_duplicates_do_not_create_a_second_hit() {
        let mut sieve = TwoHitSieve::new(TwoHitConfig {
            k: 3,
            seen_once_bits: 128,
            seen_twice_bits: 128,
            probe_count: 3,
            seed: DEFAULT_BLOOM_SEED,
            expected_fragments: 2,
            support_unit: SupportUnit::SuppliedFragmentInstance,
            minimum_support: 2,
        })
        .unwrap();
        sieve.observe_fragment(0, &[7, 7, 7]).unwrap();
        // Candidate queries are prohibited before proof-relevant completion.
        assert_eq!(
            sieve.is_candidate(7).unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
        sieve.observe_fragment(1, &[7]).unwrap();
        sieve.finish_first_pass().unwrap();
        assert!(sieve.is_candidate(7).unwrap());
        assert_eq!(sieve.seen_once().inserted_events(), 2);
        assert_eq!(sieve.seen_twice().inserted_events(), 1);
    }

    #[test]
    fn ordinals_and_complete_sealing_are_enforced() {
        let mut sieve = TwoHitSieve::new(TwoHitConfig {
            k: 3,
            seen_once_bits: 64,
            seen_twice_bits: 64,
            probe_count: 2,
            seed: DEFAULT_BLOOM_SEED,
            expected_fragments: 2,
            support_unit: SupportUnit::SuppliedFragmentInstance,
            minimum_support: 2,
        })
        .unwrap();
        assert_eq!(
            sieve.observe_fragment(1, &[1]).unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
        sieve.observe_fragment(0, &[1]).unwrap();
        assert_eq!(
            sieve.finish_first_pass().unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
        sieve.observe_fragment(1, &[2]).unwrap();
        sieve.finish_first_pass().unwrap();
        assert_eq!(
            sieve.observe_fragment(2, &[3]).unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
    }

    #[test]
    fn exhaustive_collision_heavy_retained_stream_matches_exact_oracle() {
        // With m=64,h=64, the first insertion sets every bit. This is an
        // intentional all-ones/collision adversary: all later keys may be
        // promoted, but exact recount plus thresholding must repair the noise.
        let subsets: Vec<Vec<u128>> = (0_u8..8)
            .map(|mask| {
                (0_u8..3)
                    .filter(|bit| mask & (1 << bit) != 0)
                    .map(u128::from)
                    .collect()
            })
            .collect();
        for first in &subsets {
            for second in &subsets {
                for third in &subsets {
                    let fragments = vec![first.clone(), second.clone(), third.clone()];
                    for threshold in 2..=3 {
                        let sieved = sieve_retained(&fragments, threshold, 64, 64);
                        let oracle = exact_retained(&fragments, threshold);
                        assert_eq!(
                            retained_stream_bytes(&sieved),
                            retained_stream_bytes(&oracle)
                        );
                    }
                }
            }
        }
    }

    proptest! {
        #[test]
        fn every_exact_two_hit_key_is_a_candidate(
            fragments in prop::collection::vec(
                prop::collection::vec(0_u128..64, 0..16),
                0..24,
            ),
            probes in 1_u8..=64,
        ) {
            let fragments: Vec<Vec<u128>> = fragments
                .into_iter()
                .map(|fragment| {
                    fragment
                        .into_iter()
                        .map(|key| canonical_validated(key, 3))
                        .collect()
                })
                .collect();
            let mut sieve = TwoHitSieve::new(TwoHitConfig {
                k: 3,
                seen_once_bits: 64,
                seen_twice_bits: 64,
                probe_count: probes,
                seed: DEFAULT_BLOOM_SEED,
                expected_fragments: fragments.len() as u64,
                support_unit: SupportUnit::SuppliedFragmentInstance,
                minimum_support: 2,
            }).unwrap();
            for (ordinal, fragment) in fragments.iter().enumerate() {
                sieve.observe_fragment(ordinal as u64, fragment).unwrap();
            }
            sieve.finish_first_pass().unwrap();
            for count in exact_retained(&fragments, 2) {
                prop_assert!(sieve.is_candidate(count.key).unwrap());
            }
            let sieved = sieve_retained(&fragments, 2, 64, probes);
            let oracle = exact_retained(&fragments, 2);
            prop_assert_eq!(retained_stream_bytes(&sieved), retained_stream_bytes(&oracle));
        }
    }
}
