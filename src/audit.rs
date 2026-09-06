//! Exact, indexed zero-mismatch remapping of construction reads to emitted
//! linear unitigs.
//!
//! Every indexed candidate is checked against the complete read. The retained
//! exhaustive matcher is the differential oracle for the indexed path. Audit
//! results are internal consistency against construction reads, not independent
//! validation or an assertion about biological origin.
//!
//! The target universe is each emitted **linear unitig string**, independently.
//! A read spanning a GFA link is therefore reported unmapped unless that full
//! sequence also occurs inside one target string. Conversely, any eligible
//! read length is mapped, including reads shorter than construction `k`; these
//! are read-placement observations and must not be interpreted as k-mer
//! support or depth.

use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::indexed_mapper::{
    IndexedExactMapper, IndexedMapperConfig, ALGORITHM_ID as INDEXED_MAPPER_ID,
    ALGORITHM_VERSION as INDEXED_MAPPER_VERSION, DEFAULT_SEED_LENGTH,
    TARGET_UNIVERSE as INDEXED_TARGET_UNIVERSE,
};
use crate::model::{AvailabilityU64, Fragment, MateRole, StateCount, Topology, Unitig};
use std::collections::{BTreeMap, BTreeSet};

const AUDIT_PERSISTENT_SHARE_DIVISOR: u64 = 8;
const AUDIT_CURRENT_FRAGMENT_SHARE_DIVISOR: u64 = 2;
const AUDIT_SLICE_OUTPUT_SHARE_DIVISOR: u64 = 4;
const TREE_ENTRY_ALLOWANCE: u64 = 192;
const HEAP_ALLOCATION_ALLOWANCE: u64 = 64;
const PHASE_FIXED_ALLOWANCE: u64 = 64 << 10;

/// The globally fixed read layout for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditInputMode {
    SingleEnd,
    PairedEnd,
}

/// Parameters that affect exact construction-read remapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditConfig {
    pub input_mode: AuditInputMode,
    pub remap: bool,
    pub min_base_quality: u8,
    pub max_mapping_candidates: u64,
    /// Phase-wide allocation ceiling used to fail before retaining an
    /// unbounded placement list for one read.
    pub memory_budget_bytes: u64,
}

/// One exact placement on an emitted linear unitig.
///
/// Coordinates are zero-based, half-open, and anchored to the emitted forward
/// sequence.  The derived ordering is the stable mapper ordering because IDs
/// are ASCII, starts and ends are numeric, and `+` sorts before `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlacementGroup {
    pub unitig_id: String,
    pub start: u64,
    pub end: u64,
    pub strand: char,
}

/// Exhaustive state of one supplied read instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadState {
    NotRequested,
    IneligibleAmbiguityOrQuality,
    IndeterminateCandidateLimit,
    Unmapped,
    SinglePlacementGroup,
    MultiplePlacementGroups,
}

impl ReadState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::IneligibleAmbiguityOrQuality => "ineligible_ambiguity_or_quality",
            Self::IndeterminateCandidateLimit => "indeterminate_candidate_limit",
            Self::Unmapped => "unmapped",
            Self::SinglePlacementGroup => "single_placement_group",
            Self::MultiplePlacementGroups => "multiple_placement_groups",
        }
    }
}

/// Audit record for one supplied read instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadAudit {
    pub fragment_ordinal: u64,
    pub mate_role: MateRole,
    pub state: ReadState,
    /// `Some` only for enumeration-complete states.  `Unmapped` has an empty
    /// vector; the two mapped states have one or more groups.
    pub placement_groups: Option<Vec<PlacementGroup>>,
}

/// Run-wide placement totals, following the same missingness rule as unitigs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementTotals {
    pub enumeration_complete_read_placements: AvailabilityU64,
    pub single_group_read_instances: AvailabilityU64,
    pub multi_group_read_instances_with_group: AvailabilityU64,
}

/// Provenance derived from the mapper selected for this audit run.
///
/// An execution status of `not_requested` means no target index was built and
/// the three index cardinalities are zero. The algorithm and seed still name
/// the deterministic mapper configured for an enabled audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapperDescriptor {
    pub algorithm_id: &'static str,
    pub algorithm_version: &'static str,
    pub target_universe: &'static str,
    pub seed_length: u8,
    pub execution_status: &'static str,
    pub linear_targets: u64,
    pub index_postings: u64,
    pub accounted_index_bytes: u64,
}

/// Complete output of the exact construction-read audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditResult {
    pub mapper: MapperDescriptor,
    pub global_enumeration_status: &'static str,
    pub read_audits: Vec<ReadAudit>,
    pub state_counts: Vec<StateCount>,
    pub placement_totals: PlacementTotals,
}

/// Run-wide result produced by the bounded streaming auditor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditSummary {
    pub mapper: MapperDescriptor,
    pub global_enumeration_status: &'static str,
    pub state_counts: Vec<StateCount>,
    pub placement_totals: PlacementTotals,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UnitigAuditUpdate {
    id: String,
    topology: Topology,
    enumeration_complete_read_placements: AvailabilityU64,
    single_group_read_instances: AvailabilityU64,
    multi_group_read_instances_with_group: AvailabilityU64,
    placement_enumeration_status: &'static str,
}

/// Finalized streaming audit, including updates that can be applied after the
/// accumulator's immutable borrow of the unitig targets has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedAudit {
    pub summary: AuditSummary,
    unitig_updates: Vec<UnitigAuditUpdate>,
}

impl FinalizedAudit {
    /// Apply only audit fields. Unitig identity and topology are checked;
    /// sequence, support, and graph metadata are never written.
    pub fn apply_to_unitigs(&self, unitigs: &mut [Unitig]) -> Result<()> {
        if unitigs.len() != self.unitig_updates.len() {
            return Err(invariant(
                "unitig set changed between audit and finalization",
            ));
        }
        for (unitig, update) in unitigs.iter_mut().zip(&self.unitig_updates) {
            if unitig.id != update.id || unitig.topology != update.topology {
                return Err(invariant(
                    "unitig identity changed between audit and finalization",
                ));
            }
            unitig.enumeration_complete_read_placements =
                update.enumeration_complete_read_placements.clone();
            unitig.single_group_read_instances = update.single_group_read_instances.clone();
            unitig.multi_group_read_instances_with_group =
                update.multi_group_read_instances_with_group.clone();
            unitig.placement_enumeration_status = update.placement_enumeration_status;
        }
        Ok(())
    }
}

/// Bounded streaming construction-read auditor.
///
/// Only the current fragment's placement groups are returned to the caller.
/// Global storage is proportional to the emitted unitig set, not to the read
/// count.  Fragment ordinals must arrive in strictly increasing spool order.
pub struct AuditAccumulator<'a> {
    config: AuditConfig,
    unitigs: &'a [Unitig],
    mapper: Option<IndexedExactMapper<'a>>,
    mapper_descriptor: MapperDescriptor,
    target_by_id: BTreeMap<&'a str, usize>,
    previous_fragment_ordinal: Option<u64>,
    state_tallies: BTreeMap<(MateRole, ReadState), u64>,
    supplied_by_role: BTreeMap<MateRole, u64>,
    placements: Vec<u64>,
    singles: Vec<u64>,
    multi_memberships: Vec<u64>,
    total_placements: u64,
    total_singles: u64,
    total_multi_memberships: u64,
    any_limit: bool,
}

impl<'a> AuditAccumulator<'a> {
    pub fn new(unitigs: &'a [Unitig], config: AuditConfig) -> Result<Self> {
        validate_config(config)?;
        let persistent_budget = config.memory_budget_bytes / AUDIT_PERSISTENT_SHARE_DIVISOR;
        let audit_required = audit_persistent_allocation_bound(unitigs)?;
        let (mapper, mapper_descriptor, persistent_required) = if config.remap {
            let plan = IndexedExactMapper::plan(unitigs, DEFAULT_SEED_LENGTH)?;
            let persistent_required = audit_required
                .checked_add(plan.resident_bound_bytes())
                .ok_or_else(|| overflow("audit and mapper persistent allocation overflow"))?;
            ensure_memory_share(
                persistent_required,
                persistent_budget,
                "audit state and indexed target index",
            )?;
            let index_budget = indexed_mapper_memory_budget(unitigs, config.memory_budget_bytes)?;
            let mapper = IndexedExactMapper::build_planned(
                unitigs,
                IndexedMapperConfig {
                    seed_length: DEFAULT_SEED_LENGTH,
                    index_memory_budget_bytes: index_budget,
                    placement_memory_budget_bytes: config.memory_budget_bytes
                        / AUDIT_CURRENT_FRAGMENT_SHARE_DIVISOR,
                },
                plan,
            )?;
            let descriptor = mapper_descriptor(&mapper, MAPPER_EXECUTED)?;
            (Some(mapper), descriptor, persistent_required)
        } else {
            (
                None,
                MapperDescriptor {
                    algorithm_id: INDEXED_MAPPER_ID,
                    algorithm_version: INDEXED_MAPPER_VERSION,
                    target_universe: INDEXED_TARGET_UNIVERSE,
                    seed_length: u8::try_from(DEFAULT_SEED_LENGTH)
                        .map_err(|_| invariant("stable mapper seed length exceeds u8"))?,
                    execution_status: MAPPER_NOT_REQUESTED,
                    linear_targets: 0,
                    index_postings: 0,
                    accounted_index_bytes: 0,
                },
                audit_required,
            )
        };
        ensure_memory_share(
            persistent_required,
            persistent_budget,
            "audit state and indexed target index",
        )?;
        let target_indices = validate_and_order_targets(unitigs)?;
        let target_by_id = target_indices
            .iter()
            .map(|&index| (unitigs[index].id.as_str(), index))
            .collect();
        Ok(Self {
            config,
            unitigs,
            mapper,
            mapper_descriptor,
            target_by_id,
            previous_fragment_ordinal: None,
            state_tallies: BTreeMap::new(),
            supplied_by_role: BTreeMap::new(),
            placements: zeroed_counters(unitigs.len(), "allocate placement counters")?,
            singles: zeroed_counters(unitigs.len(), "allocate single-read counters")?,
            multi_memberships: zeroed_counters(
                unitigs.len(),
                "allocate multi-membership counters",
            )?,
            total_placements: 0,
            total_singles: 0,
            total_multi_memberships: 0,
            any_limit: false,
        })
    }

    /// Audit one complete fragment and return only that fragment's read rows.
    /// For paired input, both rows are produced before this method returns.
    pub fn audit_fragment(&mut self, fragment: &Fragment) -> Result<Vec<ReadAudit>> {
        self.audit_fragment_with_memory_limit(
            fragment,
            self.config.memory_budget_bytes / AUDIT_CURRENT_FRAGMENT_SHARE_DIVISOR,
        )
    }

    /// Audit one caller-owned fragment while bounding every derived allocation.
    /// The caller may subtract its own decoded-fragment accounting before
    /// passing this allowance, as the spool-aware pipeline does.
    pub(crate) fn audit_fragment_with_memory_limit(
        &mut self,
        fragment: &Fragment,
        derived_memory_bytes: u64,
    ) -> Result<Vec<ReadAudit>> {
        if self
            .previous_fragment_ordinal
            .is_some_and(|previous| previous >= fragment.ordinal)
        {
            return Err(invariant("fragment ordinals are not strictly increasing"));
        }
        validate_fragment_roles(fragment, self.config.input_mode)?;
        self.previous_fragment_ordinal = Some(fragment.ordinal);

        let row_bytes = bytes_for_count::<ReadAudit>(
            fragment.reads.len(),
            "current read-audit row allocation overflow",
        )?;
        let fixed_bytes = row_bytes
            .checked_add(PHASE_FIXED_ALLOWANCE)
            .ok_or_else(|| overflow("current read-audit fixed allocation overflow"))?;
        let read_workspace = derived_memory_bytes
            .checked_sub(fixed_bytes)
            .ok_or_else(|| {
                memory_error("current read-audit rows exceed their derived-memory allowance")
            })?;
        let read_count = u64::try_from(fragment.reads.len())
            .map_err(|_| overflow("fragment read count does not fit u64"))?;
        let per_read_budget = read_workspace
            .checked_div(read_count)
            .ok_or_else(|| invariant("validated fragment contains no reads"))?;

        let mut current = Vec::new();
        current
            .try_reserve_exact(fragment.reads.len())
            .map_err(|_| memory_error("allocate current read-audit rows"))?;
        for read in &fragment.reads {
            let read_length = u64::try_from(read.sequence.len())
                .map_err(|_| overflow("audit read length does not fit u64"))?;
            let mapper_budget = if self.config.remap {
                let normalized = read_length
                    .checked_add(HEAP_ALLOCATION_ALLOWANCE)
                    .ok_or_else(|| overflow("audit read workspace allocation overflow"))?;
                per_read_budget.checked_sub(normalized).ok_or_else(|| {
                    memory_error("normalized read leaves no indexed-mapper workspace allowance")
                })?
            } else {
                per_read_budget
            };
            let (state, placement_groups) = if !self.config.remap {
                (ReadState::NotRequested, None)
            } else if let Some(normalized) = eligible_normalized_read(
                &read.sequence,
                read.quality.as_deref(),
                self.config.min_base_quality,
            )? {
                let mapping = self
                    .mapper
                    .as_ref()
                    .ok_or_else(|| invariant("enabled audit has no indexed mapper"))?
                    .map_with_memory_limit(
                        &normalized,
                        self.config.max_mapping_candidates,
                        mapper_budget,
                    )?;
                (mapping.state, mapping.placement_groups)
            } else {
                (ReadState::IneligibleAmbiguityOrQuality, None)
            };
            let audit = ReadAudit {
                fragment_ordinal: fragment.ordinal,
                mate_role: read.role,
                state,
                placement_groups,
            };
            self.observe_read(&audit)?;
            current.push(audit);
        }
        Ok(current)
    }

    fn observe_read(&mut self, audit: &ReadAudit) -> Result<()> {
        let supplied = self.supplied_by_role.entry(audit.mate_role).or_default();
        *supplied = checked_increment(*supplied, "supplied read count overflow")?;
        let state = self
            .state_tallies
            .entry((audit.mate_role, audit.state))
            .or_default();
        *state = checked_increment(*state, "read-state count overflow")?;

        match audit.state {
            ReadState::IndeterminateCandidateLimit => self.any_limit = true,
            ReadState::SinglePlacementGroup | ReadState::MultiplePlacementGroups => {
                let groups = audit
                    .placement_groups
                    .as_deref()
                    .ok_or_else(|| invariant("mapped read lacks placement groups"))?;
                let multiple = audit.state == ReadState::MultiplePlacementGroups;
                if (!multiple && groups.len() != 1) || (multiple && groups.len() < 2) {
                    return Err(invariant(if multiple {
                        "multiple-placement state has too few groups"
                    } else {
                        "single-placement state has the wrong group count"
                    }));
                }
                if groups.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(invariant(
                        "mapped read placement groups are not strictly ordered",
                    ));
                }
                // Indexed mapper groups are strictly ordered by target ID,
                // start, and strand.  Groups for one target are therefore
                // contiguous, so adjacent deduplication avoids a second,
                // input-scaled allocation while the placement list is live.
                let mut previous_member = None;
                for group in groups {
                    let index = *self
                        .target_by_id
                        .get(group.unitig_id.as_str())
                        .ok_or_else(|| invariant("placement names a non-linear target"))?;
                    self.placements[index] = checked_increment(
                        self.placements[index],
                        "per-unitig complete placement overflow",
                    )?;
                    self.total_placements = checked_increment(
                        self.total_placements,
                        "run-wide complete placement overflow",
                    )?;
                    if multiple && previous_member != Some(index) {
                        self.multi_memberships[index] = checked_increment(
                            self.multi_memberships[index],
                            "per-unitig multi-membership overflow",
                        )?;
                        self.total_multi_memberships = checked_increment(
                            self.total_multi_memberships,
                            "run-wide multi-membership overflow",
                        )?;
                    }
                    previous_member = Some(index);
                }
                if !multiple {
                    let index = previous_member
                        .ok_or_else(|| invariant("single placement has no target"))?;
                    self.singles[index] =
                        checked_increment(self.singles[index], "per-unitig single-read overflow")?;
                    self.total_singles =
                        checked_increment(self.total_singles, "run-wide single-read overflow")?;
                }
            }
            ReadState::Unmapped => {
                if audit.placement_groups.as_deref() != Some(&[]) {
                    return Err(invariant("unmapped read lacks an empty complete set"));
                }
            }
            ReadState::NotRequested | ReadState::IneligibleAmbiguityOrQuality => {}
        }
        Ok(())
    }

    pub fn finish(self) -> Result<FinalizedAudit> {
        let state_counts = build_tallied_state_counts(
            &self.state_tallies,
            &self.supplied_by_role,
            self.config.input_mode,
            self.config.remap,
        )?;
        let summed_placements = checked_sum(
            &self.placements,
            "streaming per-unitig placement reconciliation overflow",
        )?;
        let summed_singles = checked_sum(
            &self.singles,
            "streaming per-unitig single reconciliation overflow",
        )?;
        let summed_multi = checked_sum(
            &self.multi_memberships,
            "streaming per-unitig multi reconciliation overflow",
        )?;
        if (summed_placements, summed_singles, summed_multi)
            != (
                self.total_placements,
                self.total_singles,
                self.total_multi_memberships,
            )
        {
            return Err(invariant(
                "streaming audit aggregates failed reconciliation",
            ));
        }

        let (global_status, placement_totals) = if !self.config.remap {
            (REMAP_DISABLED, unavailable_totals("remap_disabled"))
        } else if self.any_limit {
            (LIMIT, unavailable_totals("candidate_limit"))
        } else {
            (
                COMPLETE,
                PlacementTotals {
                    enumeration_complete_read_placements: AvailabilityU64::Value(
                        self.total_placements,
                    ),
                    single_group_read_instances: AvailabilityU64::Value(self.total_singles),
                    multi_group_read_instances_with_group: AvailabilityU64::Value(
                        self.total_multi_memberships,
                    ),
                },
            )
        };

        let mut unitig_updates = Vec::new();
        unitig_updates
            .try_reserve_exact(self.unitigs.len())
            .map_err(|_| memory_error("allocate finalized unitig audit updates"))?;
        for (index, unitig) in self.unitigs.iter().enumerate() {
            let (placements, singles, multi, status) =
                if unitig.topology == Topology::ClosedGraphWalk {
                    (
                        AvailabilityU64::NotAvailable("closed_walk_audit_unsupported"),
                        AvailabilityU64::NotAvailable("closed_walk_audit_unsupported"),
                        AvailabilityU64::NotAvailable("closed_walk_audit_unsupported"),
                        CLOSED_UNSUPPORTED,
                    )
                } else if !self.config.remap {
                    (
                        AvailabilityU64::NotAvailable("remap_disabled"),
                        AvailabilityU64::NotAvailable("remap_disabled"),
                        AvailabilityU64::NotAvailable("remap_disabled"),
                        REMAP_DISABLED,
                    )
                } else if self.any_limit {
                    (
                        AvailabilityU64::NotAvailable("candidate_limit"),
                        AvailabilityU64::NotAvailable("candidate_limit"),
                        AvailabilityU64::NotAvailable("candidate_limit"),
                        LIMIT,
                    )
                } else {
                    (
                        AvailabilityU64::Value(self.placements[index]),
                        AvailabilityU64::Value(self.singles[index]),
                        AvailabilityU64::Value(self.multi_memberships[index]),
                        COMPLETE,
                    )
                };
            unitig_updates.push(UnitigAuditUpdate {
                id: try_clone_string(&unitig.id, "allocate finalized unitig identifier")?,
                topology: unitig.topology,
                enumeration_complete_read_placements: placements,
                single_group_read_instances: singles,
                multi_group_read_instances_with_group: multi,
                placement_enumeration_status: status,
            });
        }

        Ok(FinalizedAudit {
            summary: AuditSummary {
                mapper: self.mapper_descriptor,
                global_enumeration_status: global_status,
                state_counts,
                placement_totals,
            },
            unitig_updates,
        })
    }
}

/// Maximum mapper-owned persistent allocation compatible with the audit
/// phase's deterministic memory-share contract for these emitted unitigs.
///
/// Bundle validation uses the same calculation to reject descriptors that
/// could not have been produced by an admitted stable audit instance.
pub(crate) fn indexed_mapper_memory_budget(
    unitigs: &[Unitig],
    memory_budget_bytes: u64,
) -> Result<u64> {
    let persistent_budget = memory_budget_bytes / AUDIT_PERSISTENT_SHARE_DIVISOR;
    persistent_budget
        .checked_sub(audit_persistent_allocation_bound(unitigs)?)
        .ok_or_else(|| memory_error("audit state leaves no indexed-mapper allowance"))
}

const MAPPER_EXECUTED: &str = "executed";
const MAPPER_NOT_REQUESTED: &str = "not_requested";
const COMPLETE: &str = "placement_enumeration_complete";
const LIMIT: &str = "indeterminate_candidate_limit";
const REMAP_DISABLED: &str = "unavailable_remap_disabled";
const CLOSED_UNSUPPORTED: &str = "unavailable_closed_walk_audit_unsupported";

fn mapper_descriptor(
    mapper: &IndexedExactMapper<'_>,
    execution_status: &'static str,
) -> Result<MapperDescriptor> {
    Ok(MapperDescriptor {
        algorithm_id: mapper.algorithm_id(),
        algorithm_version: mapper.algorithm_version(),
        target_universe: mapper.target_universe(),
        seed_length: u8::try_from(mapper.seed_length())
            .map_err(|_| invariant("indexed-mapper seed length exceeds u8"))?,
        execution_status,
        linear_targets: u64::try_from(mapper.target_count())
            .map_err(|_| overflow("indexed-mapper target count does not fit u64"))?,
        index_postings: u64::try_from(mapper.posting_count())
            .map_err(|_| overflow("indexed-mapper posting count does not fit u64"))?,
        accounted_index_bytes: mapper.accounted_resident_bytes(),
    })
}

/// Audit every read exactly, then populate the read-instance aggregates on
/// `unitigs`.
///
/// A candidate-limit event on any eligible read invalidates all run-wide and
/// linear-unitig placement aggregates.  The event does not stop auditing later
/// reads, so read and pair state accounting stays exhaustive.
pub fn audit_fragments(
    fragments: &[Fragment],
    unitigs: &mut [Unitig],
    config: AuditConfig,
) -> Result<AuditResult> {
    let mut accumulator = AuditAccumulator::new(unitigs, config)?;
    let mut read_audits = Vec::new();
    let read_capacity = fragments
        .len()
        .checked_mul(match config.input_mode {
            AuditInputMode::SingleEnd => 1,
            AuditInputMode::PairedEnd => 2,
        })
        .ok_or_else(|| overflow("read-audit capacity overflow"))?;
    let output_budget = config.memory_budget_bytes / AUDIT_SLICE_OUTPUT_SHARE_DIVISOR;
    let header_bytes =
        bytes_for_count::<ReadAudit>(read_capacity, "slice read-audit row allocation overflow")?;
    let mut output_bytes = conservative_bytes(
        header_bytes
            .checked_add(PHASE_FIXED_ALLOWANCE)
            .ok_or_else(|| overflow("slice audit fixed allocation overflow"))?,
    )?;
    ensure_memory_share(output_bytes, output_budget, "retained slice audit rows")?;
    read_audits
        .try_reserve_exact(read_capacity)
        .map_err(|_| memory_error("allocate slice audit result"))?;
    for fragment in fragments {
        let current = accumulator.audit_fragment(fragment)?;
        output_bytes = output_bytes
            .checked_add(read_audit_payload_bytes(&current)?)
            .ok_or_else(|| overflow("slice audit result allocation overflow"))?;
        ensure_memory_share(
            output_bytes,
            output_budget,
            "retained slice audit placement groups",
        )?;
        read_audits.extend(current);
    }
    let finalized = accumulator.finish()?;
    finalized.apply_to_unitigs(unitigs)?;
    let summary = finalized.summary;
    Ok(AuditResult {
        mapper: summary.mapper,
        global_enumeration_status: summary.global_enumeration_status,
        read_audits,
        state_counts: summary.state_counts,
        placement_totals: summary.placement_totals,
    })
}

fn validate_config(config: AuditConfig) -> Result<()> {
    if config.min_base_quality > 93
        || !(1..=10_000_000).contains(&config.max_mapping_candidates)
        || !(32_u64 << 20..=64_u64 << 30).contains(&config.memory_budget_bytes)
    {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "audit quality and candidate limits are outside the stable domain",
        ));
    }
    Ok(())
}

/// Conservative bound for allocations owned by a streaming audit accumulator
/// and its finish-time output. Unitig sequences and identifiers already owned
/// by the caller are excluded; borrowed tree indexes, counter vectors, cloned
/// update identifiers, and fixed tree/allocator allowances are included.
fn audit_persistent_allocation_bound(unitigs: &[Unitig]) -> Result<u64> {
    let unitig_count = unitigs.len();
    let linear_count = unitigs
        .iter()
        .filter(|unitig| unitig.topology == Topology::Linear)
        .count();
    let identifier_bytes = unitigs.iter().try_fold(0u64, |total, unitig| {
        let length = u64::try_from(unitig.id.len())
            .map_err(|_| overflow("unitig identifier length does not fit u64"))?;
        total
            .checked_add(length)
            .ok_or_else(|| overflow("unitig identifier byte total overflow"))
    })?;
    let target_indices =
        bytes_for_count::<usize>(unitig_count, "audit target-index allocation overflow")?;
    let validation_entry = u64::try_from(std::mem::size_of::<&[u8]>())
        .map_err(|_| overflow("audit validation key size does not fit u64"))?
        .checked_add(TREE_ENTRY_ALLOWANCE)
        .ok_or_else(|| overflow("audit validation entry size overflow"))?;
    let target_entry = u64::try_from(std::mem::size_of::<(&str, usize)>())
        .map_err(|_| overflow("audit target entry size does not fit u64"))?
        .checked_add(TREE_ENTRY_ALLOWANCE)
        .ok_or_else(|| overflow("audit target entry size overflow"))?;
    let validation_tree = per_item_bytes(
        unitig_count,
        validation_entry,
        "audit validation tree allocation overflow",
    )?;
    let target_tree = per_item_bytes(
        linear_count,
        target_entry,
        "audit target tree allocation overflow",
    )?;
    let counters = bytes_for_count::<u64>(
        unitig_count
            .checked_mul(3)
            .ok_or_else(|| overflow("audit counter count overflow"))?,
        "audit counter allocation overflow",
    )?;
    let persistent = checked_byte_sum(
        &[target_indices, target_tree, counters, PHASE_FIXED_ALLOWANCE],
        "audit persistent allocation overflow",
    )?;
    let update_records = bytes_for_count::<UnitigAuditUpdate>(
        unitig_count,
        "unitig audit-update allocation overflow",
    )?;
    let update_heaps = per_item_bytes(
        unitig_count,
        HEAP_ALLOCATION_ALLOWANCE,
        "unitig audit-update heap allowance overflow",
    )?;
    let finish_peak = checked_byte_sum(
        &[persistent, update_records, identifier_bytes, update_heaps],
        "audit finish allocation overflow",
    )?;
    let validation_peak = checked_byte_sum(
        &[target_indices, validation_tree, PHASE_FIXED_ALLOWANCE],
        "audit target validation allocation overflow",
    )?;
    conservative_bytes(finish_peak.max(validation_peak))
}

fn bytes_for_count<T>(count: usize, overflow_context: &'static str) -> Result<u64> {
    let bytes = count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(|| overflow(overflow_context))?;
    u64::try_from(bytes).map_err(|_| overflow(overflow_context))
}

fn per_item_bytes(count: usize, bytes: u64, overflow_context: &'static str) -> Result<u64> {
    u64::try_from(count)
        .map_err(|_| overflow(overflow_context))?
        .checked_mul(bytes)
        .ok_or_else(|| overflow(overflow_context))
}

fn checked_byte_sum(values: &[u64], overflow_context: &'static str) -> Result<u64> {
    values.iter().try_fold(0u64, |total, value| {
        total
            .checked_add(*value)
            .ok_or_else(|| overflow(overflow_context))
    })
}

fn conservative_bytes(payload: u64) -> Result<u64> {
    let margin = payload
        .checked_add(7)
        .ok_or_else(|| overflow("audit allocation margin overflow"))?
        / 8;
    payload
        .checked_add(margin)
        .ok_or_else(|| overflow("audit conservative allocation overflow"))
}

fn read_audit_payload_bytes(audits: &[ReadAudit]) -> Result<u64> {
    let mut bytes = 0u64;
    for audit in audits {
        let Some(groups) = &audit.placement_groups else {
            continue;
        };
        bytes = bytes
            .checked_add(bytes_for_count::<PlacementGroup>(
                groups.capacity(),
                "slice placement-group allocation overflow",
            )?)
            .and_then(|value| value.checked_add(HEAP_ALLOCATION_ALLOWANCE))
            .ok_or_else(|| overflow("slice placement payload allocation overflow"))?;
        for group in groups {
            bytes = bytes
                .checked_add(
                    u64::try_from(group.unitig_id.len())
                        .map_err(|_| overflow("placement unitig identifier does not fit u64"))?,
                )
                .and_then(|value| value.checked_add(HEAP_ALLOCATION_ALLOWANCE))
                .ok_or_else(|| overflow("slice placement identifier allocation overflow"))?;
        }
    }
    Ok(bytes)
}

fn ensure_memory_share(required: u64, budget: u64, phase: &'static str) -> Result<()> {
    if required > budget {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "{phase} conservative allocation estimate {required} bytes exceeds its {budget}-byte memory-budget share"
            ),
        ));
    }
    Ok(())
}

fn validate_fragment_roles(fragment: &Fragment, mode: AuditInputMode) -> Result<()> {
    let expected: &[MateRole] = match mode {
        AuditInputMode::SingleEnd => &[MateRole::S],
        AuditInputMode::PairedEnd => &[MateRole::R1, MateRole::R2],
    };
    if fragment.reads.len() != expected.len()
        || fragment
            .reads
            .iter()
            .zip(expected)
            .any(|(read, role)| read.role != *role)
    {
        return Err(invariant("fragment roles do not match the run input mode"));
    }
    Ok(())
}

fn validate_and_order_targets(unitigs: &[Unitig]) -> Result<Vec<usize>> {
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    targets
        .try_reserve_exact(unitigs.len())
        .map_err(|_| memory_error("allocate linear target index"))?;
    for (index, unitig) in unitigs.iter().enumerate() {
        if !seen.insert(unitig.id.as_bytes()) {
            return Err(invariant(
                "duplicate unitig identifier in mapping target set",
            ));
        }
        if unitig.sequence.is_empty()
            || unitig
                .sequence
                .iter()
                .any(|base| !matches!(base, b'A' | b'C' | b'G' | b'T'))
        {
            return Err(invariant(
                "unitig mapping target is not nonempty uppercase ACGT",
            ));
        }
        if unitig.topology == Topology::Linear {
            targets.push(index);
        }
    }
    targets.sort_unstable_by(|&left, &right| unitigs[left].id.cmp(&unitigs[right].id));
    Ok(targets)
}

fn eligible_normalized_read(
    sequence: &[u8],
    quality: Option<&[u8]>,
    min_base_quality: u8,
) -> Result<Option<Vec<u8>>> {
    if sequence.is_empty() {
        return Err(invariant("validated spool contains an empty read"));
    }
    if let Some(quality) = quality {
        if quality.len() != sequence.len() {
            return Err(invariant(
                "validated spool has unequal sequence and quality lengths",
            ));
        }
        for &value in quality {
            if !(33..=126).contains(&value) {
                return Err(invariant(
                    "validated spool contains an invalid Phred+33 byte",
                ));
            }
            if value - 33 < min_base_quality {
                return Ok(None);
            }
        }
    }

    let mut normalized = Vec::new();
    normalized
        .try_reserve_exact(sequence.len())
        .map_err(|_| memory_error("allocate normalized read"))?;
    for &base in sequence {
        let base = base.to_ascii_uppercase();
        if !matches!(base, b'A' | b'C' | b'G' | b'T') {
            return Ok(None);
        }
        normalized.push(base);
    }
    Ok(Some(normalized))
}

#[cfg(test)]
fn map_eligible_read(
    read: &[u8],
    target_indices: &[usize],
    unitigs: &[Unitig],
    max_candidates: u64,
    max_placement_bytes: u64,
) -> Result<(ReadState, Option<Vec<PlacementGroup>>)> {
    let reverse = reverse_complement(read)?;
    let mut groups = Vec::new();
    let mut accepted = 0u64;
    let mut placement_bytes = 0_u64;

    for &target_index in target_indices {
        let target = &unitigs[target_index];
        if read.len() > target.sequence.len() {
            continue;
        }
        let final_start = target.sequence.len() - read.len();
        for start in 0..=final_start {
            let end = start
                .checked_add(read.len())
                .ok_or_else(|| overflow("placement coordinate overflow"))?;
            let interval = &target.sequence[start..end];
            for (strand, query) in [('+', read), ('-', reverse.as_slice())] {
                if interval != query {
                    continue;
                }
                accepted = accepted
                    .checked_add(1)
                    .ok_or_else(|| overflow("placement-group counter overflow"))?;
                if accepted > max_candidates {
                    // The accepted prefix is intentionally dropped: it is not
                    // an exhaustive result and must not contribute evidence.
                    return Ok((ReadState::IndeterminateCandidateLimit, None));
                }
                let group_bytes = u64::try_from(std::mem::size_of::<PlacementGroup>())
                    .map_err(|_| overflow("placement-group size does not fit u64"))?
                    .checked_mul(4)
                    .ok_or_else(|| overflow("placement-group vector capacity overflow"))?
                    .checked_add(
                        u64::try_from(target.id.len())
                            .map_err(|_| overflow("unitig identifier length does not fit u64"))?,
                    )
                    .and_then(|value| {
                        value.checked_add(
                            u64::try_from(std::mem::size_of::<usize>())
                                .ok()?
                                .checked_add(TREE_ENTRY_ALLOWANCE)?,
                        )
                    })
                    .and_then(|value| value.checked_add(HEAP_ALLOCATION_ALLOWANCE.checked_mul(2)?))
                    .ok_or_else(|| overflow("placement-group memory estimate overflow"))?;
                placement_bytes = placement_bytes
                    .checked_add(group_bytes)
                    .ok_or_else(|| overflow("placement-list memory estimate overflow"))?;
                if placement_bytes > max_placement_bytes {
                    return Err(memory_error(
                        "one read's exact placement groups exceed their memory-budget share",
                    ));
                }
                groups
                    .try_reserve(1)
                    .map_err(|_| memory_error("allocate placement group"))?;
                groups.push(PlacementGroup {
                    unitig_id: try_clone_string(
                        &target.id,
                        "allocate placement-group unitig identifier",
                    )?,
                    start: u64::try_from(start)
                        .map_err(|_| overflow("placement start does not fit u64"))?,
                    end: u64::try_from(end)
                        .map_err(|_| overflow("placement end does not fit u64"))?,
                    strand,
                });
            }
        }
    }

    debug_assert!(groups.windows(2).all(|pair| pair[0] < pair[1]));
    Ok(match groups.len() {
        0 => (ReadState::Unmapped, Some(groups)),
        1 => (ReadState::SinglePlacementGroup, Some(groups)),
        _ => (ReadState::MultiplePlacementGroups, Some(groups)),
    })
}

#[cfg(test)]
fn reverse_complement(read: &[u8]) -> Result<Vec<u8>> {
    let mut reverse = Vec::new();
    reverse
        .try_reserve_exact(read.len())
        .map_err(|_| memory_error("allocate reverse-complement read"))?;
    for base in read.iter().rev() {
        reverse.push(match base {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            _ => unreachable!("eligibility checked before reverse complementation"),
        });
    }
    Ok(reverse)
}

fn build_tallied_state_counts(
    tallies: &BTreeMap<(MateRole, ReadState), u64>,
    supplied_by_role: &BTreeMap<MateRole, u64>,
    mode: AuditInputMode,
    remap: bool,
) -> Result<Vec<StateCount>> {
    let roles: &[MateRole] = match mode {
        AuditInputMode::SingleEnd => &[MateRole::S],
        AuditInputMode::PairedEnd => &[MateRole::R1, MateRole::R2],
    };
    let states: &[ReadState] = if remap {
        &[
            ReadState::IneligibleAmbiguityOrQuality,
            ReadState::IndeterminateCandidateLimit,
            ReadState::Unmapped,
            ReadState::SinglePlacementGroup,
            ReadState::MultiplePlacementGroups,
        ]
    } else {
        &[ReadState::NotRequested]
    };
    let mut rows = Vec::with_capacity(roles.len() * states.len());
    for &role in roles {
        let mut classified = 0u64;
        for &state in states {
            let count = tallies.get(&(role, state)).copied().unwrap_or(0);
            classified = classified
                .checked_add(count)
                .ok_or_else(|| overflow("streaming read-state reconciliation overflow"))?;
            rows.push(StateCount {
                role: Some(role),
                state: state.as_str(),
                count,
            });
        }
        if classified != supplied_by_role.get(&role).copied().unwrap_or(0) {
            return Err(invariant(
                "streaming read-state rows do not reconcile to supplied reads",
            ));
        }
    }
    Ok(rows)
}

fn unavailable_totals(reason: &'static str) -> PlacementTotals {
    PlacementTotals {
        enumeration_complete_read_placements: AvailabilityU64::NotAvailable(reason),
        single_group_read_instances: AvailabilityU64::NotAvailable(reason),
        multi_group_read_instances_with_group: AvailabilityU64::NotAvailable(reason),
    }
}

fn checked_increment(value: u64, context: &'static str) -> Result<u64> {
    value.checked_add(1).ok_or_else(|| overflow(context))
}

fn checked_sum(values: &[u64], context: &'static str) -> Result<u64> {
    values.iter().try_fold(0u64, |sum, value| {
        sum.checked_add(*value).ok_or_else(|| overflow(context))
    })
}

fn zeroed_counters(length: usize, context: &'static str) -> Result<Vec<u64>> {
    let mut counters = Vec::new();
    counters
        .try_reserve_exact(length)
        .map_err(|_| memory_error(context))?;
    counters.resize(length, 0);
    Ok(counters)
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
    use crate::compact::compact_graph;
    use crate::dna::reverse_complement_validated;
    use crate::graph::ExactGraph;
    use crate::model::{KmerCount, ReadRecord, Topology};

    const TEST_MEMORY_BUDGET: u64 = 64 << 20;

    fn unitig(id_suffix: char, sequence: &[u8], topology: Topology) -> Unitig {
        Unitig {
            id: format!("utg-{}", id_suffix.to_string().repeat(64)),
            sequence: sequence.to_vec(),
            topology,
            edge_steps: 1,
            canonical_kmers: 1,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: AvailabilityU64::Value(999),
            single_group_read_instances: AvailabilityU64::Value(999),
            multi_group_read_instances_with_group: AvailabilityU64::Value(999),
            placement_enumeration_status: "stale",
            sequence_sha256: "0".repeat(64),
        }
    }

    fn single_fragment(ordinal: u64, sequence: &[u8], quality: Option<&[u8]>) -> Fragment {
        Fragment {
            ordinal,
            lane_ordinal: 0,
            reads: vec![ReadRecord {
                role: MateRole::S,
                normalized_id_digest: [0; 32],
                sequence: sequence.to_vec(),
                quality: quality.map(<[u8]>::to_vec),
            }],
        }
    }

    fn config(limit: u64) -> AuditConfig {
        AuditConfig {
            input_mode: AuditInputMode::SingleEnd,
            remap: true,
            min_base_quality: 20,
            max_mapping_candidates: limit,
            memory_budget_bytes: 512 << 20,
        }
    }

    fn paired_fragment(ordinal: u64, r1: &[u8], r2: &[u8]) -> Fragment {
        Fragment {
            ordinal,
            lane_ordinal: 0,
            reads: vec![
                ReadRecord {
                    role: MateRole::R1,
                    normalized_id_digest: [0; 32],
                    sequence: r1.to_vec(),
                    quality: None,
                },
                ReadRecord {
                    role: MateRole::R2,
                    normalized_id_digest: [0; 32],
                    sequence: r2.to_vec(),
                    quality: None,
                },
            ],
        }
    }

    #[test]
    fn palindromic_read_has_two_strand_groups_at_each_start() {
        let fragments = vec![single_fragment(0, b"AT", None)];
        let mut unitigs = vec![unitig('a', b"ATAT", Topology::Linear)];
        let result = audit_fragments(&fragments, &mut unitigs, config(10)).unwrap();
        let groups = result.read_audits[0].placement_groups.as_ref().unwrap();
        assert_eq!(
            result.read_audits[0].state,
            ReadState::MultiplePlacementGroups
        );
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].start, 0);
        assert_eq!(groups[0].strand, '+');
        assert_eq!(groups[1].start, 0);
        assert_eq!(groups[1].strand, '-');
        assert_eq!(groups[2].start, 2);
        assert_eq!(groups[3].start, 2);
    }

    #[test]
    fn target_start_and_strand_order_is_frozen() {
        let fragments = vec![single_fragment(0, b"ACG", None)];
        let mut unitigs = vec![
            unitig('b', b"CGTACG", Topology::Linear),
            unitig('a', b"ACGCGT", Topology::Linear),
        ];
        let result = audit_fragments(&fragments, &mut unitigs, config(20)).unwrap();
        let groups = result.read_audits[0].placement_groups.as_ref().unwrap();
        let observed: Vec<_> = groups
            .iter()
            .map(|group| (group.unitig_id.as_bytes()[4], group.start, group.strand))
            .collect();
        assert_eq!(
            observed,
            vec![
                (b'a', 0, '+'),
                (b'a', 3, '-'),
                (b'b', 0, '-'),
                (b'b', 3, '+'),
            ]
        );
    }

    #[test]
    fn sorted_target_blocks_are_deduplicated_without_a_membership_index() {
        let fragments = vec![single_fragment(0, b"AAA", None)];
        // Keep input order opposite mapper order and give each target multiple
        // placements. Each target must still receive exactly one membership.
        let mut unitigs = vec![
            unitig('b', b"AAAA", Topology::Linear),
            unitig('a', b"AAAAA", Topology::Linear),
        ];
        let result = audit_fragments(&fragments, &mut unitigs, config(20)).unwrap();
        let groups = result.read_audits[0].placement_groups.as_ref().unwrap();

        assert_eq!(groups.len(), 5);
        assert!(groups.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            result
                .placement_totals
                .multi_group_read_instances_with_group,
            AvailabilityU64::Value(2)
        );
        assert!(unitigs.iter().all(|unitig| {
            unitig.multi_group_read_instances_with_group == AvailabilityU64::Value(1)
        }));
    }

    #[test]
    fn limit_plus_one_discards_candidate_prefix_and_marks_every_linear_row() {
        let fragments = vec![
            single_fragment(0, b"ACG", None),
            single_fragment(1, b"TTA", None),
        ];
        let mut unitigs = vec![
            unitig('a', b"ACGACG", Topology::Linear),
            unitig('b', b"TTACCC", Topology::Linear),
            unitig('c', b"ACGT", Topology::ClosedGraphWalk),
        ];
        let result = audit_fragments(&fragments, &mut unitigs, config(1)).unwrap();
        assert_eq!(
            result.read_audits[0].state,
            ReadState::IndeterminateCandidateLimit
        );
        assert!(result.read_audits[0].placement_groups.is_none());
        assert_eq!(result.read_audits[1].state, ReadState::SinglePlacementGroup);
        assert_eq!(result.global_enumeration_status, LIMIT);
        for unitig in &unitigs[..2] {
            assert_eq!(
                unitig.enumeration_complete_read_placements,
                AvailabilityU64::NotAvailable("candidate_limit")
            );
        }
        assert_eq!(
            unitigs[2].enumeration_complete_read_placements,
            AvailabilityU64::NotAvailable("closed_walk_audit_unsupported")
        );
    }

    #[test]
    fn candidate_limit_on_r1_does_not_short_circuit_r2() {
        let fragments = vec![paired_fragment(0, b"ACG", b"TTA")];
        let mut unitigs = vec![
            unitig('a', b"ACGACG", Topology::Linear),
            unitig('b', b"TTACCC", Topology::Linear),
        ];
        let result = audit_fragments(
            &fragments,
            &mut unitigs,
            AuditConfig {
                input_mode: AuditInputMode::PairedEnd,
                remap: true,
                min_base_quality: 20,
                max_mapping_candidates: 1,
                memory_budget_bytes: 512 << 20,
            },
        )
        .unwrap();
        assert_eq!(
            result.read_audits[0].state,
            ReadState::IndeterminateCandidateLimit
        );
        assert_eq!(result.read_audits[1].state, ReadState::SinglePlacementGroup);
        assert_eq!(
            result.read_audits[1]
                .placement_groups
                .as_ref()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn empty_linear_target_universe_is_complete_and_eligible_reads_are_unmapped() {
        let fragments = vec![single_fragment(0, b"ACG", None)];
        let mut unitigs = vec![unitig('c', b"ACGT", Topology::ClosedGraphWalk)];
        let result = audit_fragments(&fragments, &mut unitigs, config(1)).unwrap();
        assert_eq!(result.read_audits[0].state, ReadState::Unmapped);
        assert_eq!(result.global_enumeration_status, COMPLETE);
        assert_eq!(
            result.placement_totals.enumeration_complete_read_placements,
            AvailabilityU64::Value(0)
        );
    }

    #[test]
    fn stable_audit_uses_q15_index_and_carries_instance_provenance() {
        let fragment = single_fragment(0, b"AACCGGTTAACCGGTT", None);
        let mut unitigs = vec![
            unitig('b', b"GGGAACCGGTTAACCGGTTCCC", Topology::Linear),
            unitig('a', b"AACCGGTTAACCGGTTAACCGGTT", Topology::Linear),
            unitig('c', b"AACCGGTTAACCGGTT", Topology::ClosedGraphWalk),
        ];
        let target_indices = validate_and_order_targets(&unitigs).unwrap();
        let oracle = map_eligible_read(
            &fragment.reads[0].sequence,
            &target_indices,
            &unitigs,
            20,
            TEST_MEMORY_BUDGET,
        )
        .unwrap();

        let result = audit_fragments(&[fragment], &mut unitigs, config(20)).unwrap();
        assert_eq!(
            (
                result.read_audits[0].state,
                result.read_audits[0].placement_groups.clone()
            ),
            oracle
        );
        assert_eq!(result.mapper.algorithm_id, INDEXED_MAPPER_ID);
        assert_eq!(result.mapper.algorithm_version, INDEXED_MAPPER_VERSION);
        assert_eq!(result.mapper.target_universe, INDEXED_TARGET_UNIVERSE);
        assert_eq!(result.mapper.seed_length, 15);
        assert_eq!(result.mapper.execution_status, MAPPER_EXECUTED);
        assert_eq!(result.mapper.linear_targets, 2);
        assert_eq!(result.mapper.index_postings, 18);
        assert!(result.mapper.accounted_index_bytes > 0);
    }

    #[test]
    fn disabled_audit_builds_no_index_and_reports_that_fact() {
        let fragments = vec![single_fragment(0, b"ACG", None)];
        let mut unitigs = vec![unitig('a', b"ACGT", Topology::Linear)];
        let mut disabled = config(10);
        disabled.remap = false;
        let result = audit_fragments(&fragments, &mut unitigs, disabled).unwrap();
        assert_eq!(result.mapper.algorithm_id, INDEXED_MAPPER_ID);
        assert_eq!(result.mapper.seed_length, 15);
        assert_eq!(result.mapper.execution_status, MAPPER_NOT_REQUESTED);
        assert_eq!(result.mapper.linear_targets, 0);
        assert_eq!(result.mapper.index_postings, 0);
        assert_eq!(result.mapper.accounted_index_bytes, 0);
    }

    #[test]
    fn aggregates_count_groups_single_reads_and_deduplicated_multi_memberships() {
        let fragments = vec![
            single_fragment(0, b"AAA", None),
            single_fragment(1, b"CCC", None),
        ];
        let mut unitigs = vec![
            unitig('a', b"AAAA", Topology::Linear),
            unitig('b', b"CCCT", Topology::Linear),
        ];
        let result = audit_fragments(&fragments, &mut unitigs, config(20)).unwrap();
        // AAA has two '+' groups on unitig a; one read/unitig membership is
        // counted even though the read has two groups there.
        assert_eq!(
            result.read_audits[0]
                .placement_groups
                .as_ref()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(result.read_audits[1].state, ReadState::SinglePlacementGroup);
        assert_eq!(
            result.placement_totals.enumeration_complete_read_placements,
            AvailabilityU64::Value(3)
        );
        assert_eq!(
            result.placement_totals.single_group_read_instances,
            AvailabilityU64::Value(1)
        );
        assert_eq!(
            result
                .placement_totals
                .multi_group_read_instances_with_group,
            AvailabilityU64::Value(1)
        );
        assert_eq!(
            unitigs[0].multi_group_read_instances_with_group,
            AvailabilityU64::Value(1)
        );
    }

    #[test]
    fn quality_and_ambiguity_are_ineligible_and_later_mate_equivalent_is_still_audited() {
        let fragments = vec![
            single_fragment(0, b"ACN", None),
            single_fragment(1, b"ACG", Some(b"I!I")),
        ];
        let mut unitigs = vec![unitig('a', b"ACGT", Topology::Linear)];
        let result = audit_fragments(&fragments, &mut unitigs, config(10)).unwrap();
        assert!(result
            .read_audits
            .iter()
            .all(|audit| audit.state == ReadState::IneligibleAmbiguityOrQuality));
    }

    #[test]
    fn junction_spanning_reads_are_not_claimed_as_segment_placements() {
        let encode = |sequence: &[u8]| {
            sequence.iter().fold(0u128, |code, base| {
                (code << 2)
                    | match base {
                        b'A' => 0,
                        b'C' => 1,
                        b'G' => 2,
                        b'T' => 3,
                        _ => panic!("test sequence must contain only ACGT"),
                    }
            })
        };
        let mut records = [
            (b"CAA".as_slice(), 2),
            (b"AAC".as_slice(), 1),
            (b"AAG".as_slice(), 1),
        ]
        .into_iter()
        .map(|(sequence, support)| {
            let forward = encode(sequence);
            KmerCount {
                key: forward.min(reverse_complement_validated(forward, 3)),
                support,
            }
        })
        .collect::<Vec<_>>();
        records.sort_unstable();
        let graph = ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
        let mut compacted = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
        assert_eq!(compacted.links.len(), 2);

        let fragments = vec![
            single_fragment(0, b"CAAC", None),
            single_fragment(1, b"CAAG", None),
        ];
        let result = audit_fragments(&fragments, &mut compacted.unitigs, config(20)).unwrap();
        assert!(result
            .read_audits
            .iter()
            .all(|audit| audit.state == ReadState::Unmapped));
        assert_eq!(
            result.placement_totals.enumeration_complete_read_placements,
            AvailabilityU64::Value(0)
        );
        assert!(compacted.unitigs.iter().all(|unitig| {
            unitig.enumeration_complete_read_placements == AvailabilityU64::Value(0)
        }));
        // The exact mapper's target universe is emitted linear segments, not
        // graph paths. This fixture must not be read as absence of graph-path
        // evidence for the two explicit junctions above.
    }

    #[test]
    fn shorter_than_k_placement_is_not_mislabeled_as_kmer_support() {
        let forward = 16u128; // CAA in two-bit encoding.
        let records = vec![KmerCount {
            key: forward.min(reverse_complement_validated(forward, 3)),
            support: 1,
        }];
        let graph = ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
        assert_eq!(graph.stats().canonical_kmers, 1);
        assert_eq!(graph.stats().total_support, 1);
        let mut compacted = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
        let fragments = vec![
            single_fragment(0, b"CAA", None),
            single_fragment(1, b"C", None),
        ];
        let result = audit_fragments(&fragments, &mut compacted.unitigs, config(20)).unwrap();
        assert!(result
            .read_audits
            .iter()
            .all(|audit| audit.state == ReadState::SinglePlacementGroup));
        assert_eq!(
            result.placement_totals.single_group_read_instances,
            AvailabilityU64::Value(2)
        );
        assert_eq!(
            compacted.unitigs[0].single_group_read_instances,
            AvailabilityU64::Value(2)
        );
        // Placement totals describe supplied reads on segment strings. The
        // one-base read cannot contribute a k=3 construction window, so these
        // two placements are intentionally not a support-depth claim.
    }

    #[test]
    fn placement_memory_limit_fails_before_retaining_the_first_group() {
        let targets = vec![unitig('a', b"A", Topology::Linear)];
        let error = map_eligible_read(b"A", &[0], &targets, 10, 0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn target_and_finish_indexes_are_admitted_before_tree_construction() {
        let mut target = unitig('a', b"A", Topology::Linear);
        target.id = "x".repeat(5 << 20);
        let mut bounded = config(10);
        bounded.memory_budget_bytes = 32 << 20;
        let error = match AuditAccumulator::new(&[target], bounded) {
            Ok(_) => panic!("oversized audit index was unexpectedly admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn indexed_target_storage_shares_the_audit_persistent_admission() {
        let sequence = vec![b'A'; 200_000];
        let mut target = unitig('a', &sequence, Topology::Linear);
        target.id = "bounded-target".to_owned();
        let mut bounded = config(10);
        bounded.memory_budget_bytes = 32 << 20;
        let error = match AuditAccumulator::new(std::slice::from_ref(&target), bounded) {
            Ok(_) => panic!("oversized audit index was unexpectedly admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::ResourceMemory);

        bounded.remap = false;
        let disabled = AuditAccumulator::new(std::slice::from_ref(&target), bounded).unwrap();
        assert_eq!(
            disabled.mapper_descriptor.execution_status,
            MAPPER_NOT_REQUESTED
        );
        assert!(disabled.mapper.is_none());
    }

    #[test]
    fn derived_read_workspace_is_checked_before_normalization() {
        let targets = vec![unitig('a', b"ACGT", Topology::Linear)];
        let mut accumulator = AuditAccumulator::new(&targets, config(10)).unwrap();
        let fragment = single_fragment(0, b"ACG", None);
        let fixed_only =
            PHASE_FIXED_ALLOWANCE + u64::try_from(std::mem::size_of::<ReadAudit>()).unwrap();
        let error = accumulator
            .audit_fragment_with_memory_limit(&fragment, fixed_only)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }
}
