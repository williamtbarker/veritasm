//! Deterministic indexed exact matching against emitted linear unitigs.
//!
//! Every candidate obtained from the literal seed index is verified against
//! the complete read, and short reads fall back to an exhaustive scan. The
//! index therefore changes search work, not the set or interpretation of
//! reported placement groups.

use crate::audit::{PlacementGroup, ReadState};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::model::{Topology, Unitig};

/// Algorithm identity for the indexed exact mapper.
pub const ALGORITHM_ID: &str = "literal_rarest_seed_zero_mismatch";
/// Algorithm version for reproducible experiments with this module.
pub const ALGORITHM_VERSION: &str = "1";
/// The target universe preserved from the stable construction-read audit.
pub const TARGET_UNIVERSE: &str = "all_emitted_linear_unitigs_only";

/// Default literal seed length. A read shorter than this is scanned exactly.
pub const DEFAULT_SEED_LENGTH: usize = 15;
/// Default dedicated memory allowance for the immutable target index.
pub const DEFAULT_INDEX_MEMORY_BUDGET_BYTES: u64 = 512 << 20;
/// Default per-query allowance for temporary and returned placements.
pub const DEFAULT_PLACEMENT_MEMORY_BUDGET_BYTES: u64 = 64 << 20;

const FIXED_ALLOCATION_ALLOWANCE: u64 = 64 << 10;
const HEAP_ALLOCATION_ALLOWANCE: u64 = 64;
const MAX_SEED_LENGTH: usize = 31;
const MAX_MAPPING_CANDIDATES: u64 = 10_000_000;

/// Deterministic allocation plan for one immutable target index.
///
/// The bound covers mapper-owned target-catalog and posting-vector storage
/// plus the module's fixed allowance and conservative margin. Borrowed unitig
/// identifiers and sequences, caller-owned metadata, stacks, and allocator
/// internals are outside this estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedMapperPlan {
    seed_length: usize,
    catalog_slots: usize,
    linear_targets: usize,
    posting_count: usize,
    resident_bound_bytes: u64,
}

impl IndexedMapperPlan {
    pub const fn seed_length(self) -> usize {
        self.seed_length
    }

    pub const fn linear_targets(self) -> usize {
        self.linear_targets
    }

    pub const fn posting_count(self) -> usize {
        self.posting_count
    }

    pub const fn resident_bound_bytes(self) -> u64 {
        self.resident_bound_bytes
    }
}

/// Explicit resource parameters for an indexed exact-mapper instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedMapperConfig {
    /// Length of the literal two-bit q-grams indexed from every linear target.
    pub seed_length: usize,
    /// Dedicated conservative ceiling for target records and seed postings.
    pub index_memory_budget_bytes: u64,
    /// Per-read conservative ceiling for mapper-owned placement data.
    pub placement_memory_budget_bytes: u64,
}

impl Default for IndexedMapperConfig {
    fn default() -> Self {
        Self {
            seed_length: DEFAULT_SEED_LENGTH,
            index_memory_budget_bytes: DEFAULT_INDEX_MEMORY_BUDGET_BYTES,
            placement_memory_budget_bytes: DEFAULT_PLACEMENT_MEMORY_BUDGET_BYTES,
        }
    }
}

/// Deterministic work counters for one mapping request.
///
/// These counters describe algorithmic work, not biological evidence. Seed
/// hits never consume the scientific placement-group limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MappingWork {
    /// Query q-grams whose posting-list lengths were inspected.
    pub seed_lookups: u64,
    /// Entries in the selected posting ranges before candidate filtering.
    pub selected_seed_hits: u64,
    /// Selected-seed postings examined, including at most one merge-lookahead
    /// candidate per orientation when a placement cap stops the search.
    pub postings_examined: u64,
    /// In-bounds indexed starts compared against the complete oriented read;
    /// this stops exactly at the cap-triggering verified group.
    pub indexed_full_verifications: u64,
    /// Target intervals compared by the exact short-read fallback.
    pub fallback_full_verifications: u64,
    /// Fully verified groups seen, including the limit-breaking group.
    pub verified_placement_groups_seen: u64,
}

/// Exact mapper result for one already-normalized, eligible read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedMapping {
    /// Uses the stable audit states for direct semantic comparison.
    pub state: ReadState,
    /// Complete sorted groups, an empty set for `Unmapped`, or `None` after
    /// candidate-limit indeterminacy.
    pub placement_groups: Option<Vec<PlacementGroup>>,
    /// Deterministic implementation work counters.
    pub work: MappingWork,
}

#[derive(Debug, Clone, Copy)]
struct TargetRef<'a> {
    id: &'a str,
    sequence: &'a [u8],
    topology: Topology,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SeedPosting {
    key: u64,
    target_rank: usize,
    target_position: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CompactPlacement {
    target_rank: usize,
    start: usize,
    strand: char,
}

#[derive(Debug, Clone, Copy)]
struct QueryMemory {
    reverse_capacity: usize,
    limit_bytes: u64,
}

struct OrientationCursor<'mapper, 'target> {
    postings: std::slice::Iter<'mapper, SeedPosting>,
    targets: &'mapper [TargetRef<'target>],
    query_length: usize,
    seed_offset: usize,
    strand: char,
}

impl OrientationCursor<'_, '_> {
    fn next_candidate(&mut self, work: &mut MappingWork) -> Result<Option<CompactPlacement>> {
        for posting in self.postings.by_ref() {
            work.postings_examined = checked_increment(
                work.postings_examined,
                "indexed-mapper examined-posting count overflow",
            )?;
            let Some(start) = posting.target_position.checked_sub(self.seed_offset) else {
                continue;
            };
            let target = self
                .targets
                .get(posting.target_rank)
                .ok_or_else(|| invariant("indexed-mapper posting target is out of bounds"))?;
            let Some(end) = start.checked_add(self.query_length) else {
                return Err(overflow("indexed-mapper placement coordinate overflow"));
            };
            if end > target.sequence.len() {
                continue;
            }
            return Ok(Some(CompactPlacement {
                target_rank: posting.target_rank,
                start,
                strand: self.strand,
            }));
        }
        Ok(None)
    }
}

/// Immutable literal-seed index over all and only emitted linear unitigs.
///
/// Unitig sequences and identifiers are borrowed. Closed graph walks remain
/// outside the mapping universe. Targets are internally ordered by full ID
/// bytes, so input ordering cannot affect placement ordering.
pub struct IndexedExactMapper<'a> {
    config: IndexedMapperConfig,
    targets: Vec<TargetRef<'a>>,
    postings: Vec<SeedPosting>,
    accounted_resident_bytes: u64,
}

impl<'a> IndexedExactMapper<'a> {
    /// Stable algorithm identifier for this constructed instance.
    pub const fn algorithm_id(&self) -> &'static str {
        ALGORITHM_ID
    }

    /// Stable algorithm version for this constructed instance.
    pub const fn algorithm_version(&self) -> &'static str {
        ALGORITHM_VERSION
    }

    /// Target-universe contract enforced by this constructed instance.
    pub const fn target_universe(&self) -> &'static str {
        TARGET_UNIVERSE
    }

    /// Validate the complete target set and determine the exact index
    /// cardinalities before allocating mapper-owned vectors.
    pub fn plan(unitigs: &[Unitig], seed_length: usize) -> Result<IndexedMapperPlan> {
        validate_seed_length(seed_length)?;
        let mut linear_targets = 0usize;
        let mut posting_count = 0usize;
        for unitig in unitigs {
            validate_target(unitig)?;
            if unitig.topology == Topology::Linear {
                linear_targets = linear_targets
                    .checked_add(1)
                    .ok_or_else(|| overflow("indexed-mapper target count overflow"))?;
                posting_count = posting_count
                    .checked_add(
                        unitig
                            .sequence
                            .len()
                            .saturating_sub(seed_length.saturating_sub(1)),
                    )
                    .ok_or_else(|| overflow("indexed-mapper posting count overflow"))?;
            }
        }
        Ok(IndexedMapperPlan {
            seed_length,
            // Construction deliberately validates duplicate IDs before
            // removing closed targets, so its catalog temporarily contains
            // one borrowed record per supplied unitig.
            catalog_slots: unitigs.len(),
            linear_targets,
            posting_count,
            resident_bound_bytes: index_memory_required(unitigs.len(), posting_count)?,
        })
    }

    /// Validate targets and construct an exact literal q-gram index.
    pub fn new(unitigs: &'a [Unitig], config: IndexedMapperConfig) -> Result<Self> {
        validate_config(config)?;
        let plan = Self::plan(unitigs, config.seed_length)?;
        Self::build_planned(unitigs, config, plan)
    }

    /// Build a mapper from a previously admitted plan. The complete plan is
    /// recomputed so a stale or foreign plan cannot weaken resource checks.
    pub fn build_planned(
        unitigs: &'a [Unitig],
        config: IndexedMapperConfig,
        plan: IndexedMapperPlan,
    ) -> Result<Self> {
        validate_config(config)?;
        let expected = Self::plan(unitigs, config.seed_length)?;
        if plan != expected {
            return Err(invariant(
                "indexed-mapper plan does not describe the supplied targets and configuration",
            ));
        }
        ensure_memory_budget(
            plan.resident_bound_bytes,
            config.index_memory_budget_bytes,
            "indexed-mapper target index",
        )?;

        let mut targets = Vec::new();
        targets
            .try_reserve_exact(plan.catalog_slots)
            .map_err(|_| memory_error("allocate indexed-mapper target catalog"))?;
        for unitig in unitigs {
            targets.push(TargetRef {
                id: unitig.id.as_str(),
                sequence: unitig.sequence.as_slice(),
                topology: unitig.topology,
            });
        }
        targets.sort_unstable_by(|left, right| left.id.cmp(right.id));
        if targets.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(invariant(
                "duplicate unitig identifier in indexed mapping target set",
            ));
        }
        targets.retain(|target| target.topology == Topology::Linear);
        if targets.len() != plan.linear_targets {
            return Err(invariant(
                "indexed-mapper target count changed after planning",
            ));
        }

        let mut postings = Vec::new();
        postings
            .try_reserve_exact(plan.posting_count)
            .map_err(|_| memory_error("allocate indexed-mapper seed postings"))?;
        for (target_rank, target) in targets.iter().enumerate() {
            append_postings(
                target.sequence,
                target_rank,
                config.seed_length,
                &mut postings,
            )?;
        }
        if postings.len() != plan.posting_count {
            return Err(invariant(
                "indexed-mapper posting count changed during construction",
            ));
        }
        postings.sort_unstable();

        let accounted_resident_bytes =
            index_memory_required(targets.capacity(), postings.capacity())?;
        ensure_memory_budget(
            accounted_resident_bytes,
            config.index_memory_budget_bytes,
            "indexed-mapper actual target index capacity",
        )?;

        Ok(Self {
            config,
            targets,
            postings,
            accounted_resident_bytes,
        })
    }

    /// Number of linear targets in this index.
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Number of literal target q-gram postings in this index.
    pub fn posting_count(&self) -> usize {
        self.postings.len()
    }

    /// Configured literal seed length.
    pub fn seed_length(&self) -> usize {
        self.config.seed_length
    }

    /// Conservative mapper-owned resident allocation estimate derived from
    /// the vector capacities of this constructed instance.
    pub const fn accounted_resident_bytes(&self) -> u64 {
        self.accounted_resident_bytes
    }

    /// Enumerate every exact placement group or explicitly report that the
    /// verified-group limit prevented complete enumeration.
    ///
    /// `read` must be nonempty uppercase A/C/G/T after the same eligibility
    /// check used by the audit layer. The group limit counts only complete,
    /// fully verified `(unitig, interval, strand)` groups. Exactly `limit`
    /// groups is complete; the next verified group returns indeterminate and
    /// discards the prefix. Seed hits and failed full-read comparisons do not
    /// count toward the limit.
    pub fn map(&self, read: &[u8], max_mapping_candidates: u64) -> Result<IndexedMapping> {
        self.map_with_memory_limit(
            read,
            max_mapping_candidates,
            self.config.placement_memory_budget_bytes,
        )
    }

    /// Map with a caller-supplied ceiling for this query's derived data.
    /// The effective ceiling is the smaller of this value and the mapper's
    /// configured per-query ceiling.
    pub fn map_with_memory_limit(
        &self,
        read: &[u8],
        max_mapping_candidates: u64,
        available_memory_bytes: u64,
    ) -> Result<IndexedMapping> {
        validate_read(read)?;
        if !(1..=MAX_MAPPING_CANDIDATES).contains(&max_mapping_candidates) {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                "indexed-mapper candidate limit is outside the stable domain",
            ));
        }

        let placement_memory_budget_bytes =
            available_memory_bytes.min(self.config.placement_memory_budget_bytes);
        self.admit_placement_memory(read.len(), 0, None, placement_memory_budget_bytes)?;
        let reverse = reverse_complement(read)?;
        let mut work = MappingWork::default();
        let mut placements = Vec::new();
        let query_memory = QueryMemory {
            reverse_capacity: reverse.capacity(),
            limit_bytes: placement_memory_budget_bytes,
        };
        self.admit_actual_placement_memory(
            query_memory.reverse_capacity,
            placements.capacity(),
            0,
            0,
            0,
            query_memory.limit_bytes,
        )?;

        if read.len() < self.config.seed_length {
            if self.scan_read(
                read,
                &reverse,
                query_memory,
                max_mapping_candidates,
                &mut placements,
                &mut work,
            )? {
                return Ok(indeterminate(work));
            }
        } else if self.index_read(
            read,
            &reverse,
            query_memory,
            max_mapping_candidates,
            &mut placements,
            &mut work,
        )? {
            return Ok(indeterminate(work));
        }

        if placements.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(invariant(
                "indexed mapper produced non-canonical or duplicate placement order",
            ));
        }
        let groups =
            self.materialize_groups(read.len(), query_memory, &placements, placements.capacity())?;
        debug_assert!(groups.windows(2).all(|pair| pair[0] < pair[1]));
        let state = match groups.len() {
            0 => ReadState::Unmapped,
            1 => ReadState::SinglePlacementGroup,
            _ => ReadState::MultiplePlacementGroups,
        };
        Ok(IndexedMapping {
            state,
            placement_groups: Some(groups),
            work,
        })
    }

    fn index_read(
        &self,
        read: &[u8],
        reverse: &[u8],
        query_memory: QueryMemory,
        max_mapping_candidates: u64,
        placements: &mut Vec<CompactPlacement>,
        work: &mut MappingWork,
    ) -> Result<bool> {
        let (plus_offset, plus_start, plus_end) = self.rarest_seed(read, work)?;
        let (minus_offset, minus_start, minus_end) = self.rarest_seed(reverse, work)?;
        work.selected_seed_hits = checked_add_usize(
            work.selected_seed_hits,
            plus_end - plus_start,
            "indexed-mapper selected-seed hit count overflow",
        )?;
        work.selected_seed_hits = checked_add_usize(
            work.selected_seed_hits,
            minus_end - minus_start,
            "indexed-mapper selected-seed hit count overflow",
        )?;

        let mut plus = OrientationCursor {
            postings: self.postings[plus_start..plus_end].iter(),
            targets: &self.targets,
            query_length: read.len(),
            seed_offset: plus_offset,
            strand: '+',
        };
        let mut minus = OrientationCursor {
            postings: self.postings[minus_start..minus_end].iter(),
            targets: &self.targets,
            query_length: reverse.len(),
            seed_offset: minus_offset,
            strand: '-',
        };
        let mut plus_next = plus.next_candidate(work)?;
        let mut minus_next = minus.next_candidate(work)?;

        while plus_next.is_some() || minus_next.is_some() {
            let take_plus = match (plus_next, minus_next) {
                (Some(left), Some(right)) => left < right,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => unreachable!("loop condition requires a placement"),
            };
            let placement = if take_plus {
                plus_next.expect("take_plus requires a plus placement")
            } else {
                minus_next.expect("minus branch requires a minus placement")
            };
            let target = self
                .targets
                .get(placement.target_rank)
                .ok_or_else(|| invariant("indexed-mapper candidate target is out of bounds"))?;
            let end = placement
                .start
                .checked_add(read.len())
                .ok_or_else(|| overflow("indexed-mapper placement coordinate overflow"))?;
            let query = if placement.strand == '+' {
                read
            } else {
                reverse
            };
            work.indexed_full_verifications = checked_increment(
                work.indexed_full_verifications,
                "indexed-mapper full-verification count overflow",
            )?;
            if target.sequence[placement.start..end] == *query
                && self.record_group(
                    placement,
                    read.len(),
                    query_memory,
                    max_mapping_candidates,
                    placements,
                    work,
                )?
            {
                return Ok(true);
            }
            if take_plus {
                plus_next = plus.next_candidate(work)?;
            } else {
                minus_next = minus.next_candidate(work)?;
            }
        }
        Ok(false)
    }

    fn scan_read(
        &self,
        read: &[u8],
        reverse: &[u8],
        query_memory: QueryMemory,
        max_mapping_candidates: u64,
        placements: &mut Vec<CompactPlacement>,
        work: &mut MappingWork,
    ) -> Result<bool> {
        for (target_rank, target) in self.targets.iter().enumerate() {
            if read.len() > target.sequence.len() {
                continue;
            }
            let final_start = target.sequence.len() - read.len();
            for start in 0..=final_start {
                let end = start
                    .checked_add(read.len())
                    .ok_or_else(|| overflow("indexed-mapper placement coordinate overflow"))?;
                for (strand, query) in [('+', read), ('-', reverse)] {
                    work.fallback_full_verifications = checked_increment(
                        work.fallback_full_verifications,
                        "indexed-mapper fallback-verification count overflow",
                    )?;
                    if target.sequence[start..end] != *query {
                        continue;
                    }
                    if self.record_group(
                        CompactPlacement {
                            target_rank,
                            start,
                            strand,
                        },
                        read.len(),
                        query_memory,
                        max_mapping_candidates,
                        placements,
                        work,
                    )? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn rarest_seed(&self, query: &[u8], work: &mut MappingWork) -> Result<(usize, usize, usize)> {
        let q = self.config.seed_length;
        let mask = (1u64 << (2 * q)) - 1;
        let mut key = 0u64;
        for &base in &query[..q] {
            key = (key << 2) | encode_base(base);
        }

        let mut best = (0usize, 0usize, self.postings.len());
        let mut best_count = usize::MAX;
        for offset in 0..=query.len() - q {
            if offset != 0 {
                key = ((key << 2) | encode_base(query[offset + q - 1])) & mask;
            }
            work.seed_lookups = checked_increment(
                work.seed_lookups,
                "indexed-mapper seed-lookup count overflow",
            )?;
            let (range_start, range_end) = posting_range(&self.postings, key);
            let count = range_end - range_start;
            if count < best_count {
                best = (offset, range_start, range_end);
                best_count = count;
                if count == 0 {
                    break;
                }
            }
        }
        Ok(best)
    }

    fn record_group(
        &self,
        placement: CompactPlacement,
        read_length: usize,
        query_memory: QueryMemory,
        max_mapping_candidates: u64,
        placements: &mut Vec<CompactPlacement>,
        work: &mut MappingWork,
    ) -> Result<bool> {
        work.verified_placement_groups_seen = checked_increment(
            work.verified_placement_groups_seen,
            "indexed-mapper placement-group counter overflow",
        )?;
        if work.verified_placement_groups_seen > max_mapping_candidates {
            return Ok(true);
        }

        let next_count = placements
            .len()
            .checked_add(1)
            .ok_or_else(|| overflow("indexed-mapper placement count overflow"))?;
        self.admit_placement_memory(read_length, next_count, None, query_memory.limit_bytes)?;
        placements
            .try_reserve(1)
            .map_err(|_| memory_error("allocate indexed-mapper compact placement"))?;
        self.admit_actual_placement_memory(
            query_memory.reverse_capacity,
            placements.capacity(),
            0,
            0,
            0,
            query_memory.limit_bytes,
        )?;
        placements.push(placement);
        Ok(false)
    }

    fn materialize_groups(
        &self,
        read_length: usize,
        query_memory: QueryMemory,
        placements: &[CompactPlacement],
        compact_capacity: usize,
    ) -> Result<Vec<PlacementGroup>> {
        let identifier_bytes = placements.iter().try_fold(0u64, |total, placement| {
            let target = self
                .targets
                .get(placement.target_rank)
                .ok_or_else(|| invariant("indexed placement target is out of bounds"))?;
            total
                .checked_add(
                    u64::try_from(target.id.len())
                        .map_err(|_| overflow("placement identifier length does not fit u64"))?,
                )
                .ok_or_else(|| overflow("placement identifier byte total overflow"))
        })?;
        self.admit_placement_memory(
            read_length,
            placements.len(),
            Some(identifier_bytes),
            query_memory.limit_bytes,
        )?;

        let mut groups = Vec::new();
        groups
            .try_reserve_exact(placements.len())
            .map_err(|_| memory_error("allocate indexed-mapper placement groups"))?;
        self.admit_actual_placement_memory(
            query_memory.reverse_capacity,
            compact_capacity,
            groups.capacity(),
            0,
            0,
            query_memory.limit_bytes,
        )?;
        let mut identifier_capacity_bytes = 0u64;
        let mut identifier_allocations = 0usize;
        for placement in placements {
            let target = self
                .targets
                .get(placement.target_rank)
                .ok_or_else(|| invariant("indexed placement target is out of bounds"))?;
            let end = placement
                .start
                .checked_add(read_length)
                .ok_or_else(|| overflow("indexed placement coordinate overflow"))?;
            let unitig_id =
                try_clone_string(target.id, "allocate indexed-mapper placement identifier")?;
            identifier_capacity_bytes = identifier_capacity_bytes
                .checked_add(
                    u64::try_from(unitig_id.capacity())
                        .map_err(|_| overflow("placement identifier capacity does not fit u64"))?,
                )
                .ok_or_else(|| overflow("placement identifier capacity total overflow"))?;
            identifier_allocations = identifier_allocations
                .checked_add(1)
                .ok_or_else(|| overflow("placement identifier allocation count overflow"))?;
            self.admit_actual_placement_memory(
                query_memory.reverse_capacity,
                compact_capacity,
                groups.capacity(),
                identifier_capacity_bytes,
                identifier_allocations,
                query_memory.limit_bytes,
            )?;
            groups.push(PlacementGroup {
                unitig_id,
                start: u64::try_from(placement.start)
                    .map_err(|_| overflow("placement start does not fit u64"))?,
                end: u64::try_from(end).map_err(|_| overflow("placement end does not fit u64"))?,
                strand: placement.strand,
            });
        }
        Ok(groups)
    }

    fn admit_placement_memory(
        &self,
        read_length: usize,
        count: usize,
        identifier_bytes: Option<u64>,
        placement_memory_budget_bytes: u64,
    ) -> Result<()> {
        ensure_memory_budget(
            placement_memory_required(read_length, count, identifier_bytes)?,
            placement_memory_budget_bytes,
            "indexed-mapper per-read placements",
        )
    }

    fn admit_actual_placement_memory(
        &self,
        reverse_capacity: usize,
        compact_capacity: usize,
        group_capacity: usize,
        identifier_capacity_bytes: u64,
        identifier_allocations: usize,
        placement_memory_budget_bytes: u64,
    ) -> Result<()> {
        ensure_memory_budget(
            placement_capacity_memory_required(
                reverse_capacity,
                compact_capacity,
                group_capacity,
                identifier_capacity_bytes,
                identifier_allocations,
            )?,
            placement_memory_budget_bytes,
            "indexed-mapper actual per-read capacities",
        )
    }
}

fn indeterminate(work: MappingWork) -> IndexedMapping {
    IndexedMapping {
        state: ReadState::IndeterminateCandidateLimit,
        placement_groups: None,
        work,
    }
}

fn validate_config(config: IndexedMapperConfig) -> Result<()> {
    validate_seed_length(config.seed_length)?;
    if config.index_memory_budget_bytes == 0 || config.placement_memory_budget_bytes == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "indexed-mapper seed length and memory budgets are outside their domain",
        ));
    }
    Ok(())
}

fn validate_seed_length(seed_length: usize) -> Result<()> {
    if !(1..=MAX_SEED_LENGTH).contains(&seed_length) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "indexed-mapper seed length is outside its domain",
        ));
    }
    Ok(())
}

fn validate_target(unitig: &Unitig) -> Result<()> {
    if unitig.sequence.is_empty()
        || unitig
            .sequence
            .iter()
            .any(|base| !matches!(base, b'A' | b'C' | b'G' | b'T'))
    {
        return Err(invariant(
            "indexed mapping target is not nonempty uppercase ACGT",
        ));
    }
    Ok(())
}

fn validate_read(read: &[u8]) -> Result<()> {
    if read.is_empty()
        || read
            .iter()
            .any(|base| !matches!(base, b'A' | b'C' | b'G' | b'T'))
    {
        return Err(invariant(
            "indexed mapper requires a nonempty eligible uppercase ACGT read",
        ));
    }
    Ok(())
}

fn index_memory_required(target_count: usize, posting_count: usize) -> Result<u64> {
    let targets = bytes_for_count::<TargetRef<'_>>(
        target_count,
        "indexed-mapper target catalog memory overflow",
    )?;
    let postings = bytes_for_count::<SeedPosting>(
        posting_count,
        "indexed-mapper seed-posting memory overflow",
    )?;
    let estimated = checked_byte_sum(
        &[targets, postings, FIXED_ALLOCATION_ALLOWANCE],
        "indexed-mapper index memory estimate overflow",
    )?;
    conservative_bytes(estimated)
}

fn placement_memory_required(
    read_length: usize,
    count: usize,
    identifier_bytes: Option<u64>,
) -> Result<u64> {
    let reverse = u64::try_from(read_length)
        .map_err(|_| overflow("indexed-mapper read length does not fit u64"))?
        .checked_add(HEAP_ALLOCATION_ALLOWANCE)
        .ok_or_else(|| overflow("indexed-mapper reverse-complement memory overflow"))?;
    let compact = bytes_for_count::<CompactPlacement>(
        count,
        "indexed-mapper compact-placement memory overflow",
    )?
    .checked_mul(4)
    .ok_or_else(|| overflow("indexed-mapper compact-placement capacity overflow"))?;
    let groups = if identifier_bytes.is_some() {
        bytes_for_count::<PlacementGroup>(count, "indexed-mapper placement-group memory overflow")?
            .checked_mul(2)
            .ok_or_else(|| overflow("indexed-mapper placement-group capacity overflow"))?
    } else {
        0
    };
    let heaps = if identifier_bytes.is_some() {
        per_item_bytes(
            count,
            HEAP_ALLOCATION_ALLOWANCE,
            "indexed-mapper placement heap allowance overflow",
        )?
    } else {
        0
    };
    let estimated = checked_byte_sum(
        &[
            reverse,
            compact,
            groups,
            identifier_bytes.unwrap_or(0),
            heaps,
            FIXED_ALLOCATION_ALLOWANCE,
        ],
        "indexed-mapper placement memory estimate overflow",
    )?;
    conservative_bytes(estimated)
}

fn placement_capacity_memory_required(
    reverse_capacity: usize,
    compact_capacity: usize,
    group_capacity: usize,
    identifier_capacity_bytes: u64,
    identifier_allocations: usize,
) -> Result<u64> {
    let reverse = u64::try_from(reverse_capacity)
        .map_err(|_| overflow("indexed-mapper reverse capacity does not fit u64"))?
        .checked_add(HEAP_ALLOCATION_ALLOWANCE)
        .ok_or_else(|| overflow("indexed-mapper reverse-capacity memory overflow"))?;
    let compact = bytes_for_count::<CompactPlacement>(
        compact_capacity,
        "indexed-mapper actual compact-placement capacity overflow",
    )?;
    let groups = bytes_for_count::<PlacementGroup>(
        group_capacity,
        "indexed-mapper actual placement-group capacity overflow",
    )?;
    let heaps = per_item_bytes(
        identifier_allocations,
        HEAP_ALLOCATION_ALLOWANCE,
        "indexed-mapper actual placement heap allowance overflow",
    )?;
    let estimated = checked_byte_sum(
        &[
            reverse,
            compact,
            groups,
            identifier_capacity_bytes,
            heaps,
            FIXED_ALLOCATION_ALLOWANCE,
        ],
        "indexed-mapper actual placement capacity estimate overflow",
    )?;
    conservative_bytes(estimated)
}

fn append_postings(
    sequence: &[u8],
    target_rank: usize,
    seed_length: usize,
    postings: &mut Vec<SeedPosting>,
) -> Result<()> {
    if sequence.len() < seed_length {
        return Ok(());
    }
    let mask = (1u64 << (2 * seed_length)) - 1;
    let mut key = 0u64;
    for &base in &sequence[..seed_length] {
        key = (key << 2) | encode_base(base);
    }
    postings.push(SeedPosting {
        key,
        target_rank,
        target_position: 0,
    });
    for position in 1..=sequence.len() - seed_length {
        key = ((key << 2) | encode_base(sequence[position + seed_length - 1])) & mask;
        postings.push(SeedPosting {
            key,
            target_rank,
            target_position: position,
        });
    }
    Ok(())
}

fn posting_range(postings: &[SeedPosting], key: u64) -> (usize, usize) {
    let start = postings.partition_point(|posting| posting.key < key);
    let end = postings.partition_point(|posting| posting.key <= key);
    (start, end)
}

fn encode_base(base: u8) -> u64 {
    match base {
        b'A' => 0,
        b'C' => 1,
        b'G' => 2,
        b'T' => 3,
        _ => unreachable!("validated DNA contains only ACGT"),
    }
}

fn reverse_complement(read: &[u8]) -> Result<Vec<u8>> {
    let mut reverse = Vec::new();
    reverse
        .try_reserve_exact(read.len())
        .map_err(|_| memory_error("allocate indexed-mapper reverse complement"))?;
    for &base in read.iter().rev() {
        reverse.push(match base {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            _ => unreachable!("validated DNA contains only ACGT"),
        });
    }
    Ok(reverse)
}

fn checked_increment(value: u64, context: &'static str) -> Result<u64> {
    value.checked_add(1).ok_or_else(|| overflow(context))
}

fn checked_add_usize(value: u64, addend: usize, context: &'static str) -> Result<u64> {
    value
        .checked_add(u64::try_from(addend).map_err(|_| overflow(context))?)
        .ok_or_else(|| overflow(context))
}

fn bytes_for_count<T>(count: usize, context: &'static str) -> Result<u64> {
    let bytes = count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(|| overflow(context))?;
    u64::try_from(bytes).map_err(|_| overflow(context))
}

fn per_item_bytes(count: usize, bytes: u64, context: &'static str) -> Result<u64> {
    u64::try_from(count)
        .map_err(|_| overflow(context))?
        .checked_mul(bytes)
        .ok_or_else(|| overflow(context))
}

fn checked_byte_sum(values: &[u64], context: &'static str) -> Result<u64> {
    values.iter().try_fold(0u64, |total, value| {
        total.checked_add(*value).ok_or_else(|| overflow(context))
    })
}

fn conservative_bytes(payload: u64) -> Result<u64> {
    let margin = payload
        .checked_add(7)
        .ok_or_else(|| overflow("indexed-mapper allocation margin overflow"))?
        / 8;
    payload
        .checked_add(margin)
        .ok_or_else(|| overflow("indexed-mapper conservative allocation overflow"))
}

fn ensure_memory_budget(required: u64, budget: u64, phase: &'static str) -> Result<()> {
    if required > budget {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "{phase} conservative allocation estimate {required} bytes exceeds its {budget}-byte budget"
            ),
        ));
    }
    Ok(())
}

fn try_clone_string(value: &str, context: &'static str) -> Result<String> {
    let mut clone = String::new();
    clone
        .try_reserve_exact(value.len())
        .map_err(|_| memory_error(context))?;
    clone.push_str(value);
    Ok(clone)
}

fn memory_error(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn invariant(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{audit_fragments, AuditConfig, AuditInputMode};
    use crate::model::{AvailabilityU64, Fragment, MateRole, ReadRecord};
    use proptest::collection::vec;
    use proptest::prelude::*;
    use rayon::prelude::*;

    const TEST_MEMORY_BUDGET: u64 = 128 << 20;

    fn config(seed_length: usize) -> IndexedMapperConfig {
        IndexedMapperConfig {
            seed_length,
            index_memory_budget_bytes: TEST_MEMORY_BUDGET,
            placement_memory_budget_bytes: TEST_MEMORY_BUDGET,
        }
    }

    fn unitig(id: &str, sequence: &[u8], topology: Topology) -> Unitig {
        Unitig {
            id: id.to_owned(),
            sequence: sequence.to_vec(),
            topology,
            edge_steps: 1,
            canonical_kmers: 1,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: AvailabilityU64::Value(0),
            single_group_read_instances: AvailabilityU64::Value(0),
            multi_group_read_instances_with_group: AvailabilityU64::Value(0),
            placement_enumeration_status: "test",
            sequence_sha256: "0".repeat(64),
        }
    }

    fn reverse_oracle(read: &[u8]) -> Vec<u8> {
        read.iter()
            .rev()
            .map(|base| match base {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => panic!("oracle input must be ACGT"),
            })
            .collect()
    }

    fn brute_force_oracle(
        unitigs: &[Unitig],
        read: &[u8],
        limit: u64,
    ) -> (ReadState, Option<Vec<PlacementGroup>>) {
        let reverse = reverse_oracle(read);
        let mut targets: Vec<_> = unitigs
            .iter()
            .filter(|unitig| unitig.topology == Topology::Linear)
            .collect();
        targets.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        let mut groups = Vec::new();
        for target in targets {
            if read.len() > target.sequence.len() {
                continue;
            }
            for start in 0..=target.sequence.len() - read.len() {
                let end = start + read.len();
                for (strand, query) in [('+', read), ('-', reverse.as_slice())] {
                    if target.sequence[start..end] == *query {
                        groups.push(PlacementGroup {
                            unitig_id: target.id.clone(),
                            start: start as u64,
                            end: end as u64,
                            strand,
                        });
                        if groups.len() as u64 > limit {
                            return (ReadState::IndeterminateCandidateLimit, None);
                        }
                    }
                }
            }
        }
        let state = match groups.len() {
            0 => ReadState::Unmapped,
            1 => ReadState::SinglePlacementGroup,
            _ => ReadState::MultiplePlacementGroups,
        };
        (state, Some(groups))
    }

    fn assert_oracle_equivalent(unitigs: &[Unitig], read: &[u8], seed_length: usize, limit: u64) {
        let mapper = IndexedExactMapper::new(unitigs, config(seed_length)).unwrap();
        let indexed = mapper.map(read, limit).unwrap();
        let oracle = brute_force_oracle(unitigs, read, limit);
        assert_eq!((indexed.state, indexed.placement_groups), oracle);
    }

    fn dna_strings(max_length: usize) -> Vec<Vec<u8>> {
        fn extend(prefix: &mut Vec<u8>, remaining: usize, output: &mut Vec<Vec<u8>>) {
            if remaining == 0 {
                output.push(prefix.clone());
                return;
            }
            for base in b"ACGT" {
                prefix.push(*base);
                extend(prefix, remaining - 1, output);
                prefix.pop();
            }
        }

        let mut output = Vec::new();
        for length in 1..=max_length {
            extend(&mut Vec::new(), length, &mut output);
        }
        output
    }

    #[test]
    fn exhaustive_small_dna_universe_matches_independent_oracle() {
        let sequences = dna_strings(4);
        for target_sequence in &sequences {
            let targets = [unitig("target", target_sequence, Topology::Linear)];
            for read in &sequences {
                for seed_length in 1..=3 {
                    assert_oracle_equivalent(&targets, read, seed_length, 100);
                }
            }
        }
    }

    #[test]
    fn palindromic_groups_and_canonical_order_match_stable_audit() {
        let targets = vec![
            unitig("z", b"CGTACG", Topology::Linear),
            unitig("a", b"ATAT", Topology::Linear),
        ];
        let mapper = IndexedExactMapper::new(&targets, config(2)).unwrap();
        let indexed = mapper.map(b"AT", 20).unwrap();
        let groups = indexed.placement_groups.unwrap();
        assert_eq!(
            groups
                .iter()
                .map(|group| (group.unitig_id.as_str(), group.start, group.strand))
                .collect::<Vec<_>>(),
            vec![("a", 0, '+'), ("a", 0, '-'), ("a", 2, '+'), ("a", 2, '-'),]
        );

        let fragments = [Fragment {
            ordinal: 0,
            lane_ordinal: 0,
            reads: vec![ReadRecord {
                role: MateRole::S,
                normalized_id_digest: [0; 32],
                sequence: b"AT".to_vec(),
                quality: None,
            }],
        }];
        let mut audit_targets = targets;
        let stable = audit_fragments(
            &fragments,
            &mut audit_targets,
            AuditConfig {
                input_mode: AuditInputMode::SingleEnd,
                remap: true,
                min_base_quality: 0,
                max_mapping_candidates: 20,
                memory_budget_bytes: 128 << 20,
            },
        )
        .unwrap();
        assert_eq!(
            stable.read_audits[0].placement_groups.as_ref(),
            Some(&groups)
        );
    }

    #[test]
    fn seed_hits_and_failed_full_verifications_do_not_consume_the_group_limit() {
        let targets = [unitig("target", b"AAAACCCCGGGGTTTTACGT", Topology::Linear)];
        let mapper = IndexedExactMapper::new(&targets, config(1)).unwrap();
        let mapping = mapper.map(b"ACGT", 2).unwrap();
        assert_eq!(mapping.state, ReadState::MultiplePlacementGroups);
        assert_eq!(mapping.work.verified_placement_groups_seen, 2);
        assert!(mapping.work.selected_seed_hits > 2);
        assert!(
            mapping.work.indexed_full_verifications > mapping.work.verified_placement_groups_seen
        );
    }

    #[test]
    fn exact_limit_is_complete_and_limit_plus_one_discards_prefix() {
        let targets = [unitig("target", b"ACGACG", Topology::Linear)];
        let mapper = IndexedExactMapper::new(&targets, config(2)).unwrap();
        let complete = mapper.map(b"ACG", 2).unwrap();
        assert_eq!(complete.state, ReadState::MultiplePlacementGroups);
        assert_eq!(complete.placement_groups.unwrap().len(), 2);

        let limited = mapper.map(b"ACG", 1).unwrap();
        assert_eq!(limited.state, ReadState::IndeterminateCandidateLimit);
        assert!(limited.placement_groups.is_none());
        assert_eq!(limited.work.verified_placement_groups_seen, 2);
    }

    #[test]
    fn strand_combined_limit_and_short_read_fallback_are_exact() {
        let targets = [unitig("target", b"AT", Topology::Linear)];
        let mapper = IndexedExactMapper::new(&targets, config(3)).unwrap();
        let limited = mapper.map(b"AT", 1).unwrap();
        assert_eq!(limited.state, ReadState::IndeterminateCandidateLimit);
        assert!(limited.placement_groups.is_none());
        assert_eq!(limited.work.fallback_full_verifications, 2);
    }

    #[test]
    fn seed_boundary_lengths_and_both_orientation_modes_match_oracle() {
        let targets = [
            unitig("forward", b"AACCGGTTA", Topology::Linear),
            unitig("reverse", b"TAACCGGTT", Topology::Linear),
        ];
        for read in [b"ACCG".as_slice(), b"AACCG", b"AACCGG"] {
            assert_oracle_equivalent(&targets, read, 5, 20);
        }
        assert_oracle_equivalent(&targets, b"AACCG", 4, 20);
    }

    #[test]
    fn maximum_seed_width_preserves_full_literal_identity() {
        let sequence = b"ACGTACGTACGTACGTACGTACGTACGTACGTACGT";
        let targets = [unitig("target", sequence, Topology::Linear)];
        for read in [&sequence[0..30], &sequence[1..32], &sequence[2..34]] {
            assert_oracle_equivalent(&targets, read, 31, 100);
        }

        let mut invalid = config(32);
        invalid.index_memory_budget_bytes = TEST_MEMORY_BUDGET;
        let error = match IndexedExactMapper::new(&targets, invalid) {
            Ok(_) => panic!("q=32 was unexpectedly accepted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::ConfigurationInvalidLimit);
    }

    #[test]
    fn identical_sequences_under_distinct_ids_are_distinct_groups() {
        let targets = [
            unitig("z", b"AACCGG", Topology::Linear),
            unitig("a", b"AACCGG", Topology::Linear),
        ];
        let mapping = IndexedExactMapper::new(&targets, config(3))
            .unwrap()
            .map(b"AACCGG", 10)
            .unwrap();
        let groups = mapping.placement_groups.unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].unitig_id, "a");
        assert_eq!(groups[1].unitig_id, "z");
    }

    #[test]
    fn reordered_targets_closed_walks_and_segment_boundaries_do_not_change_results() {
        let first = vec![
            unitig("b", b"AAC", Topology::Linear),
            unitig("closed", b"CAAC", Topology::ClosedGraphWalk),
            unitig("a", b"CAA", Topology::Linear),
        ];
        let mut second = first.clone();
        second.reverse();
        let left = IndexedExactMapper::new(&first, config(2))
            .unwrap()
            .map(b"CAAC", 10)
            .unwrap();
        let right = IndexedExactMapper::new(&second, config(2))
            .unwrap()
            .map(b"CAAC", 10)
            .unwrap();
        assert_eq!(left.state, ReadState::Unmapped);
        assert_eq!(left.placement_groups, right.placement_groups);
    }

    #[test]
    fn empty_linear_universe_is_a_complete_empty_enumeration() {
        let targets = [unitig("closed", b"ACGT", Topology::ClosedGraphWalk)];
        let mapper = IndexedExactMapper::new(&targets, config(2)).unwrap();
        assert_eq!(mapper.target_count(), 0);
        assert_eq!(mapper.posting_count(), 0);
        let mapping = mapper.map(b"ACG", 1).unwrap();
        assert_eq!(mapping.state, ReadState::Unmapped);
        assert_eq!(mapping.placement_groups, Some(Vec::new()));
    }

    #[test]
    fn admitted_plan_and_constructed_instance_account_for_the_same_index() {
        let targets = [
            unitig("linear", b"AACCGGTTAACCGGTTAACC", Topology::Linear),
            unitig("closed", b"TTTTTTTTTTTTTTTTTTTT", Topology::ClosedGraphWalk),
        ];
        let plan = IndexedExactMapper::plan(&targets, 15).unwrap();
        assert_eq!(plan.seed_length(), 15);
        assert_eq!(plan.linear_targets(), 1);
        assert_eq!(plan.posting_count(), 6);

        let mapper = IndexedExactMapper::build_planned(&targets, config(15), plan).unwrap();
        assert_eq!(mapper.algorithm_id(), ALGORITHM_ID);
        assert_eq!(mapper.algorithm_version(), ALGORITHM_VERSION);
        assert_eq!(mapper.target_universe(), TARGET_UNIVERSE);
        assert_eq!(mapper.target_count(), plan.linear_targets());
        assert_eq!(mapper.posting_count(), plan.posting_count());
        assert!(mapper.accounted_resident_bytes() >= plan.resident_bound_bytes());

        let mut stale = plan;
        stale.posting_count += 1;
        let error = match IndexedExactMapper::build_planned(&targets, config(15), stale) {
            Ok(_) => panic!("stale mapper plan was unexpectedly accepted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::InternalInvariant);
    }

    #[test]
    fn invalid_inputs_and_memory_limits_fail_explicitly() {
        let duplicate = vec![
            unitig("same", b"ACG", Topology::Linear),
            unitig("same", b"TGC", Topology::ClosedGraphWalk),
        ];
        let duplicate_error = match IndexedExactMapper::new(&duplicate, config(2)) {
            Ok(_) => panic!("duplicate target IDs were unexpectedly accepted"),
            Err(error) => error,
        };
        assert_eq!(duplicate_error.code(), ErrorCode::InternalInvariant);

        let targets = [unitig("target", b"ACGT", Topology::Linear)];
        let mut tiny = config(2);
        tiny.index_memory_budget_bytes = 1;
        let memory_error = match IndexedExactMapper::new(&targets, tiny) {
            Ok(_) => panic!("undersized index budget was unexpectedly accepted"),
            Err(error) => error,
        };
        assert_eq!(memory_error.code(), ErrorCode::ResourceMemory);

        let mapper = IndexedExactMapper::new(&targets, config(2)).unwrap();
        assert_eq!(
            mapper.map(b"ACN", 1).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
        assert_eq!(
            mapper.map(b"AC", 0).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );

        let mut output_limited = config(2);
        output_limited.placement_memory_budget_bytes = 1;
        let output_mapper = IndexedExactMapper::new(&targets, output_limited).unwrap();
        assert_eq!(
            output_mapper.map(b"AC", 10).unwrap_err().code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn estimated_index_and_materialization_memory_boundaries_are_inclusive() {
        let long_id = "x".repeat(4_096);
        let targets = [unitig(&long_id, &[b'A'; 128], Topology::Linear)];
        let index_required = index_memory_required(1, 128).unwrap();
        let exact_index = IndexedMapperConfig {
            seed_length: 1,
            index_memory_budget_bytes: index_required,
            placement_memory_budget_bytes: TEST_MEMORY_BUDGET,
        };
        assert!(IndexedExactMapper::new(&targets, exact_index).is_ok());
        assert_eq!(
            match IndexedExactMapper::new(
                &targets,
                IndexedMapperConfig {
                    index_memory_budget_bytes: index_required - 1,
                    ..exact_index
                },
            ) {
                Ok(_) => panic!("estimate minus one was unexpectedly admitted"),
                Err(error) => error,
            }
            .code(),
            ErrorCode::ResourceMemory
        );

        let identifier_bytes = 128 * u64::try_from(long_id.len()).unwrap();
        let placement_required = placement_memory_required(1, 128, Some(identifier_bytes)).unwrap();
        let exact_output = IndexedMapperConfig {
            placement_memory_budget_bytes: placement_required,
            ..exact_index
        };
        let exact_output_mapper = IndexedExactMapper::new(&targets, exact_output).unwrap();
        let complete = exact_output_mapper
            .map_with_memory_limit(b"A", 200, placement_required)
            .unwrap();
        assert_eq!(complete.placement_groups.unwrap().len(), 128);
        assert_eq!(
            exact_output_mapper
                .map_with_memory_limit(b"A", 200, placement_required - 1)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
        let below_output = IndexedExactMapper::new(
            &targets,
            IndexedMapperConfig {
                placement_memory_budget_bytes: placement_required - 1,
                ..exact_output
            },
        )
        .unwrap();
        assert_eq!(
            below_output.map(b"A", 200).unwrap_err().code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn actual_allocator_capacities_are_checked_at_each_coexisting_peak() {
        let targets = [unitig("target", b"A", Topology::Linear)];
        let mapper = IndexedExactMapper::new(&targets, config(1)).unwrap();

        let compact_peak = placement_memory_required(1, 1, None).unwrap();
        assert_eq!(
            compact_peak,
            placement_capacity_memory_required(1, 4, 0, 0, 0).unwrap()
        );
        mapper
            .admit_actual_placement_memory(1, 4, 0, 0, 0, compact_peak)
            .unwrap();
        for capacities in [(2, 4, 0), (1, 5, 0)] {
            assert_eq!(
                mapper
                    .admit_actual_placement_memory(
                        capacities.0,
                        capacities.1,
                        capacities.2,
                        0,
                        0,
                        compact_peak,
                    )
                    .unwrap_err()
                    .code(),
                ErrorCode::ResourceMemory
            );
        }

        let materialized_peak = placement_memory_required(1, 1, Some(1)).unwrap();
        assert_eq!(
            materialized_peak,
            placement_capacity_memory_required(1, 4, 2, 1, 1).unwrap()
        );
        mapper
            .admit_actual_placement_memory(1, 4, 2, 1, 1, materialized_peak)
            .unwrap();
        for (group_capacity, identifier_capacity) in [(3, 1), (2, 2)] {
            assert_eq!(
                mapper
                    .admit_actual_placement_memory(
                        1,
                        4,
                        group_capacity,
                        identifier_capacity,
                        1,
                        materialized_peak,
                    )
                    .unwrap_err()
                    .code(),
                ErrorCode::ResourceMemory
            );
        }
    }

    #[test]
    fn generated_cases_match_the_stable_audit_across_limits_and_fallback() {
        let mut targets = vec![
            unitig("z", b"AAAACGTATAT", Topology::Linear),
            unitig("closed", b"ACGTACGT", Topology::ClosedGraphWalk),
            unitig("a", b"CCCGTACCC", Topology::Linear),
        ];
        targets.reverse();
        let reads = dna_strings(3);
        let fragments: Vec<_> = reads
            .iter()
            .enumerate()
            .map(|(ordinal, read)| Fragment {
                ordinal: ordinal as u64,
                lane_ordinal: 0,
                reads: vec![ReadRecord {
                    role: MateRole::S,
                    normalized_id_digest: [0; 32],
                    sequence: read.clone(),
                    quality: None,
                }],
            })
            .collect();

        for limit in [1, 2, 5, 100] {
            let mapper = IndexedExactMapper::new(&targets, config(3)).unwrap();
            let indexed: Vec<_> = reads
                .iter()
                .map(|read| {
                    let mapping = mapper.map(read, limit).unwrap();
                    (mapping.state, mapping.placement_groups)
                })
                .collect();
            let mut audit_targets = targets.clone();
            let stable = audit_fragments(
                &fragments,
                &mut audit_targets,
                AuditConfig {
                    input_mode: AuditInputMode::SingleEnd,
                    remap: true,
                    min_base_quality: 0,
                    max_mapping_candidates: limit,
                    memory_budget_bytes: 128 << 20,
                },
            )
            .unwrap();
            let stable: Vec<_> = stable
                .read_audits
                .into_iter()
                .map(|audit| (audit.state, audit.placement_groups))
                .collect();
            assert_eq!(indexed, stable);
        }
    }

    #[test]
    fn unique_seed_fixture_reduces_full_sequence_comparisons() {
        let mut sequence = Vec::with_capacity(100_000);
        let mut state = 0x4d595df4d0f33173u64;
        for _ in 0..100_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            sequence.push(b"ACGT"[((state >> 62) & 3) as usize]);
        }
        let targets = [unitig("target", &sequence, Topology::Linear)];
        let mapper = IndexedExactMapper::new(&targets, config(15)).unwrap();
        let read = &sequence[41_337..41_437];
        let mapping = mapper.map(read, 100).unwrap();
        assert_eq!(mapping.state, ReadState::SinglePlacementGroup);
        let exhaustive_intervals = 2 * (sequence.len() - read.len() + 1);
        let indexed_verifications =
            usize::try_from(mapping.work.indexed_full_verifications).unwrap();
        assert!(indexed_verifications < exhaustive_intervals / 1_000);
    }

    #[test]
    fn shared_index_is_deterministic_across_parallel_thread_counts() {
        let targets = [
            unitig("b", b"AACCGGTTACGTACGT", Topology::Linear),
            unitig("a", b"ATATATCCCGGGAAA", Topology::Linear),
        ];
        let mapper = IndexedExactMapper::new(&targets, config(3)).unwrap();
        let reads = dna_strings(4);
        let map_with_threads = |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| {
                    reads
                        .par_iter()
                        .map(|read| mapper.map(read, 100).unwrap())
                        .collect::<Vec<_>>()
                })
        };
        assert_eq!(map_with_threads(1), map_with_threads(4));
    }

    fn dna_vec(min: usize, max: usize) -> impl Strategy<Value = Vec<u8>> {
        vec(
            prop_oneof![Just(b'A'), Just(b'C'), Just(b'G'), Just(b'T')],
            min..=max,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn randomized_index_matches_independent_oracle(
            target_sequences in vec(dna_vec(1, 80), 0..6),
            read in dna_vec(1, 30),
            seed_length in 1usize..9,
            limit in 1u64..20,
            reverse_targets in any::<bool>(),
        ) {
            let mut targets: Vec<_> = target_sequences
                .iter()
                .enumerate()
                .map(|(index, sequence)| {
                    unitig(&format!("target-{index:03}"), sequence, Topology::Linear)
                })
                .collect();
            if reverse_targets {
                targets.reverse();
            }
            let mapper = IndexedExactMapper::new(&targets, config(seed_length)).unwrap();
            let indexed = mapper.map(&read, limit).unwrap();
            let oracle = brute_force_oracle(&targets, &read, limit);
            prop_assert_eq!((indexed.state, indexed.placement_groups), oracle);
        }
    }
}
