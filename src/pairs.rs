//! Conservative paired-read observations derived from complete read audits.
//!
//! Pair observations never alter sequences or graph links and make no claim
//! about insert orientation, gap length, or biological adjacency.

use crate::audit::{
    AuditAccumulator, AuditConfig, AuditInputMode, AuditSummary, PlacementGroup, ReadAudit,
    ReadState,
};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::model::{
    Fragment, MateRole, PairEndpoint, PairLaneStateCount, PairLink, StateCount, Topology, Unitig,
};
use crate::spool::{MemoryBoundedNext, SpoolIter};
use std::collections::{BTreeMap, BTreeSet};

const PAIR_PERSISTENT_SHARE_DIVISOR: u64 = 8;
const AUDIT_FRAGMENT_DECODE_SHARE_DIVISOR: u64 = 4;
const AUDIT_DERIVED_FRAGMENT_SHARE_DIVISOR: u64 = 2;
const TREE_ENTRY_ALLOWANCE: u64 = 192;
const HEAP_ALLOCATION_ALLOWANCE: u64 = 64;
const PAIR_FIXED_ALLOWANCE: u64 = 64 << 10;

/// First-match pair classification in stable output order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PairState {
    RemapNotRequested,
    MateIneligible,
    MateIndeterminateCandidateLimit,
    MateUnmapped,
    MateMultiplePlacementGroups,
    SameLinearUnitig,
    EndpointTie,
    CrossUnitigObservation,
    NotPairedInput,
}

impl PairState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RemapNotRequested => "remap_not_requested",
            Self::MateIneligible => "mate_ineligible",
            Self::MateIndeterminateCandidateLimit => "mate_indeterminate_candidate_limit",
            Self::MateUnmapped => "mate_unmapped",
            Self::MateMultiplePlacementGroups => "mate_multiple_placement_groups",
            Self::SameLinearUnitig => "same_linear_unitig",
            Self::EndpointTie => "endpoint_tie",
            Self::CrossUnitigObservation => "cross_unitig_observation",
            Self::NotPairedInput => "not_paired_input",
        }
    }
}

/// Exact pair observations plus exhaustive pair-state accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairAuditResult {
    pub links: Vec<PairLink>,
    pub state_counts: Vec<StateCount>,
    pub lane_state_counts: Vec<PairLaneStateCount>,
    pub link_group_count: u64,
    pub link_support: u64,
}

/// Combined result of the one-pass bounded construction-read and pair audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingAuditResult {
    pub audit: AuditSummary,
    pub pairs: PairAuditResult,
}

/// Incremental pair classifier.  It retains only endpoint-group support and
/// fixed-size state totals; per-read placements live only for the current
/// fragment.
pub struct PairAccumulator<'a> {
    input_mode: AuditInputMode,
    remap: bool,
    targets: BTreeMap<&'a str, &'a [u8]>,
    previous_fragment_ordinal: Option<u64>,
    fragment_count: u64,
    counts: [u64; 7],
    counts_by_lane: BTreeMap<u32, PairLaneAccumulator>,
    link_supports: BTreeMap<(u32, PairEndpoint, PairEndpoint), u64>,
    allocation_budget_bytes: u64,
    accounted_allocation_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct PairLaneAccumulator {
    fragment_count: u64,
    counts: [u64; 7],
}

/// Bound pair-owned borrowed indexes, accumulated link keys, and the final
/// output vector. Caller-owned unitigs, current fragments, and read-audit rows
/// are accounted by their respective phases and are not counted here.
fn pair_target_allocation_bound(unitigs: &[Unitig], enabled: bool) -> Result<u64> {
    let targets = if enabled {
        unitigs
            .iter()
            .filter(|unitig| unitig.topology == Topology::Linear)
            .count()
    } else {
        0
    };
    let entry_bytes = u64::try_from(std::mem::size_of::<(&str, &[u8])>())
        .map_err(|_| overflow("pair target entry size does not fit u64"))?
        .checked_add(TREE_ENTRY_ALLOWANCE)
        .ok_or_else(|| overflow("pair target entry size overflow"))?;
    let payload = u64::try_from(targets)
        .map_err(|_| overflow("pair target count does not fit u64"))?
        .checked_mul(entry_bytes)
        .and_then(|bytes| bytes.checked_add(PAIR_FIXED_ALLOWANCE))
        .ok_or_else(|| overflow("pair target allocation overflow"))?;
    conservative_pair_bytes(payload)
}

fn pair_endpoint_transient_bound(r1: &ReadAudit, r2: &ReadAudit) -> Result<u64> {
    if r1.state != ReadState::SinglePlacementGroup || r2.state != ReadState::SinglePlacementGroup {
        return Ok(0);
    }
    let identifier_bytes = [r1, r2].into_iter().try_fold(0u64, |total, audit| {
        let length = audit
            .placement_groups
            .as_deref()
            .and_then(|groups| groups.first())
            .map_or(0usize, |group| group.unitig_id.len());
        total
            .checked_add(
                u64::try_from(length)
                    .map_err(|_| overflow("pair endpoint identifier length does not fit u64"))?,
            )
            .ok_or_else(|| overflow("pair endpoint identifier total overflow"))
    })?;
    let endpoint_records = u64::try_from(std::mem::size_of::<PairEndpoint>())
        .map_err(|_| overflow("pair endpoint size does not fit u64"))?
        .checked_mul(2)
        .ok_or_else(|| overflow("pair endpoint record size overflow"))?;
    let payload = endpoint_records
        .checked_add(identifier_bytes)
        .and_then(|bytes| bytes.checked_add(HEAP_ALLOCATION_ALLOWANCE.checked_mul(2)?))
        .ok_or_else(|| overflow("pair endpoint transient allocation overflow"))?;
    conservative_pair_bytes(payload)
}

fn pair_link_group_allocation_bound(a: &PairEndpoint, b: &PairEndpoint) -> Result<u64> {
    let map_record = u64::try_from(std::mem::size_of::<((u32, PairEndpoint, PairEndpoint), u64)>())
        .map_err(|_| overflow("pair-link map record size does not fit u64"))?;
    let output_record = u64::try_from(std::mem::size_of::<PairLink>())
        .map_err(|_| overflow("pair-link output record size does not fit u64"))?;
    let identifiers = u64::try_from(a.segment.len())
        .map_err(|_| overflow("pair-link identifier length does not fit u64"))?
        .checked_add(
            u64::try_from(b.segment.len())
                .map_err(|_| overflow("pair-link identifier length does not fit u64"))?,
        )
        .ok_or_else(|| overflow("pair-link identifier total overflow"))?;
    let payload = map_record
        .checked_add(output_record)
        .and_then(|bytes| bytes.checked_add(identifiers))
        .and_then(|bytes| bytes.checked_add(TREE_ENTRY_ALLOWANCE))
        .and_then(|bytes| bytes.checked_add(HEAP_ALLOCATION_ALLOWANCE.checked_mul(2)?))
        .ok_or_else(|| overflow("pair-link group allocation overflow"))?;
    conservative_pair_bytes(payload)
}

fn pair_lane_allocation_bound() -> Result<u64> {
    let map_record = u64::try_from(std::mem::size_of::<(u32, PairLaneAccumulator)>())
        .map_err(|_| overflow("pair-lane map record size does not fit u64"))?;
    let output_record = u64::try_from(std::mem::size_of::<PairLaneStateCount>())
        .map_err(|_| overflow("pair-lane output record size does not fit u64"))?
        .checked_mul(7)
        .ok_or_else(|| overflow("pair-lane output allocation overflow"))?;
    let payload = map_record
        .checked_add(output_record)
        .and_then(|bytes| bytes.checked_add(TREE_ENTRY_ALLOWANCE))
        .ok_or_else(|| overflow("pair-lane allocation overflow"))?;
    conservative_pair_bytes(payload)
}

fn conservative_pair_bytes(payload: u64) -> Result<u64> {
    let margin = payload
        .checked_add(7)
        .ok_or_else(|| overflow("pair allocation margin overflow"))?
        / 8;
    payload
        .checked_add(margin)
        .ok_or_else(|| overflow("pair conservative allocation overflow"))
}

fn ensure_pair_memory_share(required: u64, budget: u64, phase: &'static str) -> Result<()> {
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

impl<'a> PairAccumulator<'a> {
    pub fn new(
        input_mode: AuditInputMode,
        remap: bool,
        unitigs: &'a [Unitig],
        memory_budget_bytes: u64,
    ) -> Result<Self> {
        if !(32_u64 << 20..=64_u64 << 30).contains(&memory_budget_bytes) {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                "pair audit received an invalid memory budget",
            ));
        }
        let allocation_budget_bytes = memory_budget_bytes / PAIR_PERSISTENT_SHARE_DIVISOR;
        let accounted_allocation_bytes = pair_target_allocation_bound(
            unitigs,
            input_mode == AuditInputMode::PairedEnd && remap,
        )?;
        ensure_pair_memory_share(
            accounted_allocation_bytes,
            allocation_budget_bytes,
            "pair target index and fixed state",
        )?;
        let targets = if input_mode == AuditInputMode::PairedEnd && remap {
            validate_linear_targets(unitigs)?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            input_mode,
            remap,
            targets,
            previous_fragment_ordinal: None,
            fragment_count: 0,
            counts: [0; 7],
            counts_by_lane: BTreeMap::new(),
            link_supports: BTreeMap::new(),
            allocation_budget_bytes,
            accounted_allocation_bytes,
        })
    }

    /// Consume one fragment and its already-computed, complete per-mate audit
    /// rows.  The rows are not retained after this call.
    pub fn observe_fragment(
        &mut self,
        fragment: &Fragment,
        read_audits: &[ReadAudit],
    ) -> Result<()> {
        if self
            .previous_fragment_ordinal
            .is_some_and(|previous| previous >= fragment.ordinal)
        {
            return Err(invariant(
                "pair-audit fragment ordinals are not strictly increasing",
            ));
        }
        validate_fragment_layout(std::slice::from_ref(fragment), self.input_mode)?;
        let audits = index_and_validate_audits(
            std::slice::from_ref(fragment),
            read_audits,
            self.input_mode,
            self.remap,
        )?;
        self.previous_fragment_ordinal = Some(fragment.ordinal);
        self.fragment_count = self
            .fragment_count
            .checked_add(1)
            .ok_or_else(|| overflow("streaming pair fragment count overflow"))?;
        if !self.counts_by_lane.contains_key(&fragment.lane_ordinal) {
            let lane_bytes = pair_lane_allocation_bound()?;
            let required = self
                .accounted_allocation_bytes
                .checked_add(lane_bytes)
                .ok_or_else(|| overflow("pair-lane allocation accounting overflow"))?;
            ensure_pair_memory_share(
                required,
                self.allocation_budget_bytes,
                "pair-lane counters and finish-time output",
            )?;
            self.accounted_allocation_bytes = required;
        }
        let lane = self
            .counts_by_lane
            .entry(fragment.lane_ordinal)
            .or_default();
        lane.fragment_count = lane
            .fragment_count
            .checked_add(1)
            .ok_or_else(|| overflow("streaming pair-lane fragment count overflow"))?;

        if self.input_mode == AuditInputMode::SingleEnd || !self.remap {
            return Ok(());
        }
        let r1 = *audits
            .get(&(fragment.ordinal, MateRole::R1))
            .ok_or_else(|| invariant("paired fragment lacks R1 audit"))?;
        let r2 = *audits
            .get(&(fragment.ordinal, MateRole::R2))
            .ok_or_else(|| invariant("paired fragment lacks R2 audit"))?;
        let transient = pair_endpoint_transient_bound(r1, r2)?;
        let transient_peak = self
            .accounted_allocation_bytes
            .checked_add(transient)
            .ok_or_else(|| overflow("pair endpoint transient allocation overflow"))?;
        ensure_pair_memory_share(
            transient_peak,
            self.allocation_budget_bytes,
            "pair endpoint classification",
        )?;
        let outcome = classify_pair(r1, r2, &self.targets)?;
        let index = paired_state_index(outcome.state)?;
        self.counts[index] = self.counts[index]
            .checked_add(1)
            .ok_or_else(|| overflow("streaming pair-state support overflow"))?;
        let lane = self
            .counts_by_lane
            .get_mut(&fragment.lane_ordinal)
            .ok_or_else(|| invariant("streaming pair-lane counter disappeared"))?;
        lane.counts[index] = lane.counts[index]
            .checked_add(1)
            .ok_or_else(|| overflow("streaming pair-lane state support overflow"))?;
        if let Some((first, second)) = outcome.link {
            let key = (fragment.lane_ordinal, first, second);
            if !self.link_supports.contains_key(&key) {
                let group_bytes = pair_link_group_allocation_bound(&key.1, &key.2)?;
                let required = self
                    .accounted_allocation_bytes
                    .checked_add(group_bytes)
                    .ok_or_else(|| overflow("pair-link allocation accounting overflow"))?;
                ensure_pair_memory_share(
                    required,
                    self.allocation_budget_bytes,
                    "pair-link groups and finish-time output",
                )?;
                self.accounted_allocation_bytes = required;
            }
            let support = self.link_supports.entry(key).or_default();
            *support = support
                .checked_add(1)
                .ok_or_else(|| overflow("streaming pair-link support overflow"))?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<PairAuditResult> {
        let lane_state_counts = build_lane_state_counts(
            self.input_mode,
            self.remap,
            &self.counts_by_lane,
            &self.counts,
            self.fragment_count,
        )?;
        if self.input_mode == AuditInputMode::SingleEnd {
            return Ok(PairAuditResult {
                links: Vec::new(),
                state_counts: vec![StateCount {
                    role: None,
                    state: PairState::NotPairedInput.as_str(),
                    count: self.fragment_count,
                }],
                lane_state_counts,
                link_group_count: 0,
                link_support: 0,
            });
        }
        if !self.remap {
            return Ok(PairAuditResult {
                links: Vec::new(),
                state_counts: vec![StateCount {
                    role: None,
                    state: PairState::RemapNotRequested.as_str(),
                    count: self.fragment_count,
                }],
                lane_state_counts,
                link_group_count: 0,
                link_support: 0,
            });
        }

        let output_states = paired_output_states();
        let state_counts = output_states
            .iter()
            .zip(self.counts)
            .map(|(state, count)| StateCount {
                role: None,
                state: state.as_str(),
                count,
            })
            .collect();
        let classified = self.counts.iter().try_fold(0u64, |sum, count| {
            sum.checked_add(*count)
                .ok_or_else(|| overflow("streaming pair-state reconciliation overflow"))
        })?;
        if classified != self.fragment_count {
            return Err(invariant(
                "streaming pair states do not reconcile to supplied fragments",
            ));
        }

        let mut links = Vec::new();
        links
            .try_reserve_exact(self.link_supports.len())
            .map_err(|_| memory_error("allocate streaming pair-link rows"))?;
        let mut link_support = 0u64;
        for ((lane_ordinal, a, b), support) in self.link_supports {
            link_support = link_support
                .checked_add(support)
                .ok_or_else(|| overflow("streaming pair-link total overflow"))?;
            links.push(PairLink {
                lane_ordinal,
                a,
                b,
                supplied_fragment_instances: support,
            });
        }
        if link_support != self.counts[6] {
            return Err(invariant(
                "streaming pair-link support does not reconcile to cross-unitig observations",
            ));
        }
        let link_group_count = u64::try_from(links.len())
            .map_err(|_| overflow("streaming pair-link group count does not fit u64"))?;
        Ok(PairAuditResult {
            links,
            state_counts,
            lane_state_counts,
            link_group_count,
            link_support,
        })
    }
}

/// Audit a replayable spool stream once without retaining fragments or all
/// per-read placements in memory.  Pair classification happens immediately
/// after both mates of the current fragment have been audited.
pub fn audit_and_summarize_stream<I>(
    fragments: I,
    unitigs: &mut [Unitig],
    config: AuditConfig,
) -> Result<StreamingAuditResult>
where
    I: IntoIterator<Item = Result<Fragment>>,
{
    let mut audit = AuditAccumulator::new(unitigs, config)?;
    let mut pairs = PairAccumulator::new(
        config.input_mode,
        config.remap,
        unitigs,
        config.memory_budget_bytes,
    )?;
    for fragment in fragments {
        let fragment = fragment?;
        let current = audit.audit_fragment(&fragment)?;
        pairs.observe_fragment(&fragment, &current)?;
    }
    let finalized = audit.finish()?;
    let pairs = pairs.finish()?;
    finalized.apply_to_unitigs(unitigs)?;
    Ok(StreamingAuditResult {
        audit: finalized.summary,
        pairs,
    })
}

/// Pipeline entry point that admits each decoded spool fragment before reading
/// its variable-length body. Decode admission deliberately excludes the scan-
/// phase k-mer vectors; audit-derived allocations have their own share. The
/// audit phase partitions its configured budget into decoded-fragment (1/4),
/// derived per-fragment (1/2), audit-persistent (1/8), and pair-persistent
/// (1/8) shares. These are conservative owned-allocation bounds, not a whole-
/// process RSS guarantee.
pub(crate) fn audit_and_summarize_spool_stream(
    mut fragments: SpoolIter,
    unitigs: &mut [Unitig],
    config: AuditConfig,
) -> Result<StreamingAuditResult> {
    let mut audit = AuditAccumulator::new(unitigs, config)?;
    let mut pairs = PairAccumulator::new(
        config.input_mode,
        config.remap,
        unitigs,
        config.memory_budget_bytes,
    )?;
    let decode_budget = config.memory_budget_bytes / AUDIT_FRAGMENT_DECODE_SHARE_DIVISOR;
    let derived_budget = config.memory_budget_bytes / AUDIT_DERIVED_FRAGMENT_SHARE_DIVISOR;
    loop {
        match fragments.next_with_decode_memory_limit(decode_budget)? {
            MemoryBoundedNext::Fragment { fragment, .. } => {
                let current = audit.audit_fragment_with_memory_limit(&fragment, derived_budget)?;
                pairs.observe_fragment(&fragment, &current)?;
            }
            MemoryBoundedNext::RequiresMemory(required) => {
                return Err(VeritasmError::new(
                    ErrorCode::ResourceMemory,
                    format!(
                        "one spooled fragment requires an estimated {required} allocator bytes, exceeding the {decode_budget}-byte audit decoding share"
                    ),
                ));
            }
            MemoryBoundedNext::End => break,
        }
    }
    let finalized = audit.finish()?;
    let pairs = pairs.finish()?;
    finalized.apply_to_unitigs(unitigs)?;
    Ok(StreamingAuditResult {
        audit: finalized.summary,
        pairs,
    })
}

/// Classify every fragment after both read audits have completed and aggregate
/// accepted cross-unitig endpoint observations.
pub fn summarize_pairs(
    input_mode: AuditInputMode,
    remap: bool,
    fragments: &[Fragment],
    read_audits: &[ReadAudit],
    unitigs: &[Unitig],
) -> Result<PairAuditResult> {
    validate_fragment_layout(fragments, input_mode)?;
    let audits = index_and_validate_audits(fragments, read_audits, input_mode, remap)?;

    if input_mode == AuditInputMode::SingleEnd {
        let count = u64::try_from(fragments.len())
            .map_err(|_| overflow("single-end fragment count does not fit u64"))?;
        return Ok(PairAuditResult {
            links: Vec::new(),
            state_counts: vec![StateCount {
                role: None,
                state: PairState::NotPairedInput.as_str(),
                count,
            }],
            lane_state_counts: lane_state_counts_from_fragments(
                input_mode, remap, fragments, &[0; 7],
            )?,
            link_group_count: 0,
            link_support: 0,
        });
    }

    if !remap {
        let count = u64::try_from(fragments.len())
            .map_err(|_| overflow("paired fragment count does not fit u64"))?;
        return Ok(PairAuditResult {
            links: Vec::new(),
            state_counts: vec![StateCount {
                role: None,
                state: PairState::RemapNotRequested.as_str(),
                count,
            }],
            lane_state_counts: lane_state_counts_from_fragments(
                input_mode, remap, fragments, &[0; 7],
            )?,
            link_group_count: 0,
            link_support: 0,
        });
    }

    let targets = validate_linear_targets(unitigs)?;
    let output_states = paired_output_states();
    let mut counts = [0u64; 7];
    let mut counts_by_lane: BTreeMap<u32, PairLaneAccumulator> = BTreeMap::new();
    let mut link_supports: BTreeMap<(u32, PairEndpoint, PairEndpoint), u64> = BTreeMap::new();

    for fragment in fragments {
        let r1 = *audits
            .get(&(fragment.ordinal, MateRole::R1))
            .ok_or_else(|| invariant("paired fragment lacks R1 audit"))?;
        let r2 = *audits
            .get(&(fragment.ordinal, MateRole::R2))
            .ok_or_else(|| invariant("paired fragment lacks R2 audit"))?;

        let outcome = classify_pair(r1, r2, &targets)?;
        let state_index = output_states
            .iter()
            .position(|state| *state == outcome.state)
            .ok_or_else(|| invariant("paired remap produced an invalid summary state"))?;
        counts[state_index] = counts[state_index]
            .checked_add(1)
            .ok_or_else(|| overflow("pair-state support overflow"))?;
        let lane = counts_by_lane.entry(fragment.lane_ordinal).or_default();
        lane.fragment_count = lane
            .fragment_count
            .checked_add(1)
            .ok_or_else(|| overflow("pair-lane fragment count overflow"))?;
        lane.counts[state_index] = lane.counts[state_index]
            .checked_add(1)
            .ok_or_else(|| overflow("pair-lane state support overflow"))?;
        if let Some((first, second)) = outcome.link {
            let support = link_supports
                .entry((fragment.lane_ordinal, first, second))
                .or_default();
            *support = support
                .checked_add(1)
                .ok_or_else(|| overflow("pair-link support overflow"))?;
        }
    }

    let state_counts: Vec<_> = output_states
        .iter()
        .zip(counts)
        .map(|(state, count)| StateCount {
            role: None,
            state: state.as_str(),
            count,
        })
        .collect();
    let classified = counts.iter().try_fold(0u64, |sum, count| {
        sum.checked_add(*count)
            .ok_or_else(|| overflow("pair-state reconciliation overflow"))
    })?;
    let supplied = u64::try_from(fragments.len())
        .map_err(|_| overflow("paired fragment count does not fit u64"))?;
    if classified != supplied {
        return Err(invariant(
            "pair states do not reconcile to supplied fragments",
        ));
    }

    let mut links = Vec::new();
    links
        .try_reserve_exact(link_supports.len())
        .map_err(|_| memory_error("allocate pair-link rows"))?;
    let mut link_support = 0u64;
    for ((lane_ordinal, a, b), support) in link_supports {
        link_support = link_support
            .checked_add(support)
            .ok_or_else(|| overflow("pair-link total support overflow"))?;
        links.push(PairLink {
            lane_ordinal,
            a,
            b,
            supplied_fragment_instances: support,
        });
    }
    if link_support != counts[6] {
        return Err(invariant(
            "pair-link support does not reconcile to cross-unitig observations",
        ));
    }
    let link_group_count = u64::try_from(links.len())
        .map_err(|_| overflow("pair-link group count does not fit u64"))?;

    Ok(PairAuditResult {
        links,
        state_counts,
        lane_state_counts: build_lane_state_counts(
            input_mode,
            remap,
            &counts_by_lane,
            &counts,
            supplied,
        )?,
        link_group_count,
        link_support,
    })
}

const fn paired_output_states() -> [PairState; 7] {
    [
        PairState::MateIneligible,
        PairState::MateIndeterminateCandidateLimit,
        PairState::MateUnmapped,
        PairState::MateMultiplePlacementGroups,
        PairState::SameLinearUnitig,
        PairState::EndpointTie,
        PairState::CrossUnitigObservation,
    ]
}

fn paired_state_index(state: PairState) -> Result<usize> {
    paired_output_states()
        .iter()
        .position(|candidate| *candidate == state)
        .ok_or_else(|| invariant("paired remap produced an invalid summary state"))
}

fn lane_state_counts_from_fragments(
    input_mode: AuditInputMode,
    remap: bool,
    fragments: &[Fragment],
    global_counts: &[u64; 7],
) -> Result<Vec<PairLaneStateCount>> {
    let mut counts_by_lane: BTreeMap<u32, PairLaneAccumulator> = BTreeMap::new();
    for fragment in fragments {
        let lane = counts_by_lane.entry(fragment.lane_ordinal).or_default();
        lane.fragment_count = lane
            .fragment_count
            .checked_add(1)
            .ok_or_else(|| overflow("pair-lane fragment count overflow"))?;
    }
    let total = u64::try_from(fragments.len())
        .map_err(|_| overflow("pair fragment count does not fit u64"))?;
    build_lane_state_counts(input_mode, remap, &counts_by_lane, global_counts, total)
}

fn build_lane_state_counts(
    input_mode: AuditInputMode,
    remap: bool,
    counts_by_lane: &BTreeMap<u32, PairLaneAccumulator>,
    global_counts: &[u64; 7],
    fragment_count: u64,
) -> Result<Vec<PairLaneStateCount>> {
    let states: &[PairState] = match (input_mode, remap) {
        (AuditInputMode::SingleEnd, _) => &[PairState::NotPairedInput],
        (AuditInputMode::PairedEnd, false) => &[PairState::RemapNotRequested],
        (AuditInputMode::PairedEnd, true) => &paired_output_states(),
    };
    let capacity = counts_by_lane
        .len()
        .checked_mul(states.len())
        .ok_or_else(|| overflow("pair-lane state row count overflow"))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| memory_error("allocate pair-lane state rows"))?;
    let mut observed_fragments = 0u64;
    let mut aggregated = [0u64; 7];
    for (&lane_ordinal, lane) in counts_by_lane {
        observed_fragments = observed_fragments
            .checked_add(lane.fragment_count)
            .ok_or_else(|| overflow("pair-lane fragment reconciliation overflow"))?;
        let lane_classified = if input_mode == AuditInputMode::PairedEnd && remap {
            lane.counts.iter().try_fold(0u64, |sum, count| {
                sum.checked_add(*count)
                    .ok_or_else(|| overflow("pair-lane state reconciliation overflow"))
            })?
        } else {
            lane.fragment_count
        };
        if lane_classified != lane.fragment_count {
            return Err(invariant(
                "pair-lane states do not reconcile to supplied lane fragments",
            ));
        }
        for (state_index, state) in states.iter().enumerate() {
            let count = if input_mode == AuditInputMode::PairedEnd && remap {
                lane.counts[state_index]
            } else {
                lane.fragment_count
            };
            output.push(PairLaneStateCount {
                lane_ordinal,
                state: state.as_str(),
                count,
            });
            if input_mode == AuditInputMode::PairedEnd && remap {
                aggregated[state_index] = aggregated[state_index]
                    .checked_add(count)
                    .ok_or_else(|| overflow("pair-lane global state reconciliation overflow"))?;
            }
        }
    }
    if observed_fragments != fragment_count {
        return Err(invariant(
            "pair-lane fragment counts do not reconcile to supplied fragments",
        ));
    }
    if input_mode == AuditInputMode::PairedEnd && remap && aggregated != *global_counts {
        return Err(invariant(
            "pair-lane state counts do not reconcile to global pair states",
        ));
    }
    Ok(output)
}

struct PairOutcome {
    state: PairState,
    link: Option<(PairEndpoint, PairEndpoint)>,
}

fn classify_pair(
    r1: &ReadAudit,
    r2: &ReadAudit,
    targets: &BTreeMap<&str, &[u8]>,
) -> Result<PairOutcome> {
    let states = [r1.state, r2.state];
    let simple = if states.contains(&ReadState::IneligibleAmbiguityOrQuality) {
        Some(PairState::MateIneligible)
    } else if states.contains(&ReadState::IndeterminateCandidateLimit) {
        Some(PairState::MateIndeterminateCandidateLimit)
    } else if states.contains(&ReadState::Unmapped) {
        Some(PairState::MateUnmapped)
    } else if states.contains(&ReadState::MultiplePlacementGroups) {
        Some(PairState::MateMultiplePlacementGroups)
    } else {
        None
    };
    if let Some(state) = simple {
        return Ok(PairOutcome { state, link: None });
    }

    if states
        != [
            ReadState::SinglePlacementGroup,
            ReadState::SinglePlacementGroup,
        ]
    {
        return Err(invariant("remapped pair contains an unexpected read state"));
    }
    let group1 = sole_group(r1)?;
    let group2 = sole_group(r2)?;
    if group1.unitig_id == group2.unitig_id {
        // Validate even observations not used for links, so corrupted audit
        // coordinates cannot silently enter summary accounting.
        validate_group(group1, targets)?;
        validate_group(group2, targets)?;
        return Ok(PairOutcome {
            state: PairState::SameLinearUnitig,
            link: None,
        });
    }

    let endpoint1 = endpoint(group1, MateRole::R1, targets)?;
    let endpoint2 = endpoint(group2, MateRole::R2, targets)?;
    let (Some(endpoint1), Some(endpoint2)) = (endpoint1, endpoint2) else {
        return Ok(PairOutcome {
            state: PairState::EndpointTie,
            link: None,
        });
    };
    // Canonicalize by complete-endpoint swap only.  In particular, do not
    // reverse-complement ends or strands, because they are emitted-sequence
    // coordinates and that would name the opposite physical endpoint.
    let (a, b) = if endpoint1 <= endpoint2 {
        (endpoint1, endpoint2)
    } else {
        (endpoint2, endpoint1)
    };
    Ok(PairOutcome {
        state: PairState::CrossUnitigObservation,
        link: Some((a, b)),
    })
}

fn validate_fragment_layout(fragments: &[Fragment], mode: AuditInputMode) -> Result<()> {
    let mut ordinals = BTreeSet::new();
    for fragment in fragments {
        if !ordinals.insert(fragment.ordinal) {
            return Err(invariant("duplicate fragment ordinal in pair audit"));
        }
        let roles: Vec<_> = fragment.reads.iter().map(|read| read.role).collect();
        let expected: &[MateRole] = match mode {
            AuditInputMode::SingleEnd => &[MateRole::S],
            AuditInputMode::PairedEnd => &[MateRole::R1, MateRole::R2],
        };
        if roles != expected {
            return Err(invariant(
                "fragment roles do not match pair-audit input mode",
            ));
        }
    }
    Ok(())
}

fn index_and_validate_audits<'a>(
    fragments: &[Fragment],
    read_audits: &'a [ReadAudit],
    input_mode: AuditInputMode,
    remap: bool,
) -> Result<BTreeMap<(u64, MateRole), &'a ReadAudit>> {
    let expected_state = if remap {
        None
    } else {
        Some(ReadState::NotRequested)
    };
    let expected_keys: BTreeSet<_> = fragments
        .iter()
        .flat_map(|fragment| {
            fragment
                .reads
                .iter()
                .map(move |read| (fragment.ordinal, read.role))
        })
        .collect();
    let mut indexed = BTreeMap::new();
    for audit in read_audits {
        let key = (audit.fragment_ordinal, audit.mate_role);
        if !expected_keys.contains(&key) || indexed.insert(key, audit).is_some() {
            return Err(invariant(
                "read-audit rows do not bijectively match supplied reads",
            ));
        }
        validate_read_audit_shape(audit)?;
        if expected_state.is_some_and(|state| audit.state != state) {
            return Err(invariant(
                "disabled remap has a non-not-requested read state",
            ));
        }
        if remap && audit.state == ReadState::NotRequested {
            return Err(invariant("enabled remap has a not-requested read state"));
        }
    }
    if indexed.len() != expected_keys.len() {
        return Err(invariant(
            "read-audit rows are missing supplied read instances",
        ));
    }
    let expected_roles: BTreeSet<_> = match input_mode {
        AuditInputMode::SingleEnd => [MateRole::S].into_iter().collect(),
        AuditInputMode::PairedEnd => [MateRole::R1, MateRole::R2].into_iter().collect(),
    };
    if indexed
        .keys()
        .any(|(_, role)| !expected_roles.contains(role))
    {
        return Err(invariant("read-audit role is incompatible with input mode"));
    }
    Ok(indexed)
}

fn validate_read_audit_shape(audit: &ReadAudit) -> Result<()> {
    let valid_shape = matches!(
        (audit.state, audit.placement_groups.as_deref()),
        (ReadState::Unmapped, Some([]))
            | (ReadState::SinglePlacementGroup, Some([_]))
            | (ReadState::MultiplePlacementGroups, Some([_, _, ..]))
            | (
                ReadState::NotRequested
                    | ReadState::IneligibleAmbiguityOrQuality
                    | ReadState::IndeterminateCandidateLimit,
                None,
            )
    );
    if !valid_shape {
        return Err(invariant("read-audit state and placement payload disagree"));
    }
    if audit
        .placement_groups
        .as_deref()
        .is_some_and(|groups| groups.windows(2).any(|pair| pair[0] >= pair[1]))
    {
        return Err(invariant(
            "read-audit placement groups are not sorted and distinct",
        ));
    }
    Ok(())
}

fn validate_linear_targets(unitigs: &[Unitig]) -> Result<BTreeMap<&str, &[u8]>> {
    let mut targets = BTreeMap::new();
    for unitig in unitigs {
        if unitig.topology != Topology::Linear {
            continue;
        }
        if unitig.sequence.is_empty()
            || unitig
                .sequence
                .iter()
                .any(|base| !matches!(base, b'A' | b'C' | b'G' | b'T'))
        {
            return Err(invariant("pair target is not nonempty uppercase ACGT"));
        }
        if targets
            .insert(unitig.id.as_str(), unitig.sequence.as_slice())
            .is_some()
        {
            return Err(invariant(
                "duplicate linear unitig identifier in pair audit",
            ));
        }
    }
    Ok(targets)
}

fn sole_group(audit: &ReadAudit) -> Result<&PlacementGroup> {
    match audit.placement_groups.as_deref() {
        Some([group]) => Ok(group),
        _ => Err(invariant(
            "single-placement read does not have exactly one group",
        )),
    }
}

fn validate_group<'a>(
    group: &PlacementGroup,
    targets: &'a BTreeMap<&str, &[u8]>,
) -> Result<&'a [u8]> {
    let sequence = targets
        .get(group.unitig_id.as_str())
        .copied()
        .ok_or_else(|| invariant("pair placement names a non-linear unitig"))?;
    let length =
        u64::try_from(sequence.len()).map_err(|_| overflow("unitig length does not fit u64"))?;
    if group.start >= group.end || group.end > length || !matches!(group.strand, '+' | '-') {
        return Err(invariant(
            "pair placement has invalid coordinates or strand",
        ));
    }
    Ok(sequence)
}

fn endpoint(
    group: &PlacementGroup,
    role: MateRole,
    targets: &BTreeMap<&str, &[u8]>,
) -> Result<Option<PairEndpoint>> {
    if !matches!(role, MateRole::R1 | MateRole::R2) {
        return Err(invariant("pair endpoint has a non-paired mate role"));
    }
    let sequence = validate_group(group, targets)?;
    let length =
        u64::try_from(sequence.len()).map_err(|_| overflow("unitig length does not fit u64"))?;
    let left = group.start;
    let right = length
        .checked_sub(group.end)
        .ok_or_else(|| invariant("pair placement end exceeds unitig length"))?;
    if left == right {
        return Ok(None);
    }
    let (end, end_distance) = if left < right {
        ('L', left)
    } else {
        ('R', right)
    };
    Ok(Some(PairEndpoint {
        segment: try_clone_string(&group.unitig_id, "allocate pair endpoint identifier")?,
        end,
        strand: group.strand,
        end_distance,
        mate_role: role,
    }))
}

fn invariant(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
    use crate::model::{AvailabilityU64, ReadRecord};
    use crate::spool::create_spool;
    use std::fs;
    use tempfile::tempdir;

    fn unitig(id_suffix: char, length: usize) -> Unitig {
        Unitig {
            id: format!("utg-{}", id_suffix.to_string().repeat(64)),
            sequence: vec![b'A'; length],
            topology: Topology::Linear,
            edge_steps: 1,
            canonical_kmers: 1,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: AvailabilityU64::Value(0),
            single_group_read_instances: AvailabilityU64::Value(0),
            multi_group_read_instances_with_group: AvailabilityU64::Value(0),
            placement_enumeration_status: "placement_enumeration_complete",
            sequence_sha256: "0".repeat(64),
        }
    }

    fn paired_fragment(ordinal: u64) -> Fragment {
        paired_fragment_in_lane(ordinal, 0)
    }

    fn paired_fragment_in_lane(ordinal: u64, lane_ordinal: u32) -> Fragment {
        Fragment {
            ordinal,
            lane_ordinal,
            reads: vec![
                ReadRecord {
                    role: MateRole::R1,
                    normalized_id_digest: [0; 32],
                    sequence: b"AAA".to_vec(),
                    quality: None,
                },
                ReadRecord {
                    role: MateRole::R2,
                    normalized_id_digest: [0; 32],
                    sequence: b"AAA".to_vec(),
                    quality: None,
                },
            ],
        }
    }

    fn audit(
        ordinal: u64,
        role: MateRole,
        state: ReadState,
        group: Option<PlacementGroup>,
    ) -> ReadAudit {
        ReadAudit {
            fragment_ordinal: ordinal,
            mate_role: role,
            state,
            placement_groups: match state {
                ReadState::Unmapped => Some(Vec::new()),
                ReadState::SinglePlacementGroup => Some(vec![group.unwrap()]),
                ReadState::MultiplePlacementGroups => {
                    let first = group.unwrap();
                    let second = PlacementGroup {
                        unitig_id: first.unitig_id.clone(),
                        start: first.start + 1,
                        end: first.end + 1,
                        strand: first.strand,
                    };
                    Some(vec![first, second])
                }
                _ => None,
            },
        }
    }

    fn group(unitig: &Unitig, start: u64, end: u64, strand: char) -> PlacementGroup {
        PlacementGroup {
            unitig_id: unitig.id.clone(),
            start,
            end,
            strand,
        }
    }

    #[test]
    fn first_match_precedence_covers_every_paired_state() {
        let a = unitig('a', 10);
        let b = unitig('b', 10);
        let unitigs = vec![a.clone(), b.clone()];
        let scenarios = [
            (
                ReadState::IneligibleAmbiguityOrQuality,
                ReadState::IndeterminateCandidateLimit,
                PairState::MateIneligible,
            ),
            (
                ReadState::IndeterminateCandidateLimit,
                ReadState::Unmapped,
                PairState::MateIndeterminateCandidateLimit,
            ),
            (
                ReadState::Unmapped,
                ReadState::MultiplePlacementGroups,
                PairState::MateUnmapped,
            ),
            (
                ReadState::MultiplePlacementGroups,
                ReadState::SinglePlacementGroup,
                PairState::MateMultiplePlacementGroups,
            ),
            (
                ReadState::SinglePlacementGroup,
                ReadState::SinglePlacementGroup,
                PairState::SameLinearUnitig,
            ),
        ];
        for (ordinal, (s1, s2, expected)) in scenarios.into_iter().enumerate() {
            let ordinal = ordinal as u64;
            let fragment = vec![paired_fragment(ordinal)];
            let same = group(&a, 0, 3, '+');
            let audits = vec![
                audit(ordinal, MateRole::R1, s1, Some(same.clone())),
                audit(ordinal, MateRole::R2, s2, Some(same.clone())),
            ];
            let result = summarize_pairs(
                AuditInputMode::PairedEnd,
                true,
                &fragment,
                &audits,
                &unitigs,
            )
            .unwrap();
            assert_eq!(
                result
                    .state_counts
                    .iter()
                    .find(|row| row.state == expected.as_str())
                    .unwrap()
                    .count,
                1
            );
        }
    }

    #[test]
    fn endpoint_tie_and_cross_unitig_are_separate() {
        let a = unitig('a', 10);
        let b = unitig('b', 10);
        let fragments = vec![paired_fragment(0), paired_fragment(1)];
        let audits = vec![
            audit(
                0,
                MateRole::R1,
                ReadState::SinglePlacementGroup,
                Some(group(&a, 4, 6, '+')),
            ),
            audit(
                0,
                MateRole::R2,
                ReadState::SinglePlacementGroup,
                Some(group(&b, 0, 2, '-')),
            ),
            audit(
                1,
                MateRole::R1,
                ReadState::SinglePlacementGroup,
                Some(group(&a, 0, 2, '+')),
            ),
            audit(
                1,
                MateRole::R2,
                ReadState::SinglePlacementGroup,
                Some(group(&b, 8, 10, '-')),
            ),
        ];
        let result = summarize_pairs(
            AuditInputMode::PairedEnd,
            true,
            &fragments,
            &audits,
            &[a, b],
        )
        .unwrap();
        assert_eq!(result.state_counts[5].count, 1);
        assert_eq!(result.state_counts[6].count, 1);
        assert_eq!(result.link_support, 1);
    }

    #[test]
    fn pair_link_is_rejected_before_an_unbudgeted_group_is_retained() {
        let a = unitig('a', 10);
        let b = unitig('b', 10);
        let unitigs = vec![a.clone(), b.clone()];
        let mut accumulator =
            PairAccumulator::new(AuditInputMode::PairedEnd, true, &unitigs, 512 << 20).unwrap();
        let first = PairEndpoint {
            segment: a.id.clone(),
            end: 'L',
            strand: '+',
            end_distance: 0,
            mate_role: MateRole::R1,
        };
        let second = PairEndpoint {
            segment: b.id.clone(),
            end: 'R',
            strand: '-',
            end_distance: 0,
            mate_role: MateRole::R2,
        };
        let group_bytes = pair_link_group_allocation_bound(&first, &second).unwrap();
        let lane_bytes = pair_lane_allocation_bound().unwrap();
        accumulator.allocation_budget_bytes = accumulator
            .accounted_allocation_bytes
            .checked_add(lane_bytes)
            .unwrap()
            .checked_add(group_bytes)
            .unwrap()
            - 1;

        let fragment = paired_fragment(0);
        let audits = vec![
            audit(
                0,
                MateRole::R1,
                ReadState::SinglePlacementGroup,
                Some(group(&a, 0, 3, '+')),
            ),
            audit(
                0,
                MateRole::R2,
                ReadState::SinglePlacementGroup,
                Some(group(&b, 7, 10, '-')),
            ),
        ];
        let error = accumulator
            .observe_fragment(&fragment, &audits)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
        assert!(accumulator.link_supports.is_empty());
    }

    #[test]
    fn pair_target_tree_is_admitted_before_insertion() {
        let targets = (0..20_000)
            .map(|index| {
                let mut target = unitig('a', 1);
                target.id = format!("utg-{index:064x}");
                target
            })
            .collect::<Vec<_>>();
        let error = match PairAccumulator::new(AuditInputMode::PairedEnd, true, &targets, 32 << 20)
        {
            Ok(_) => panic!("oversized pair target tree was unexpectedly admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn canonicalization_is_swap_only_and_preserves_roles_and_physical_ends() {
        let a = unitig('a', 20);
        let b = unitig('b', 20);
        let fragments = vec![paired_fragment(0), paired_fragment(1)];
        let audits = vec![
            audit(
                0,
                MateRole::R1,
                ReadState::SinglePlacementGroup,
                Some(group(&b, 17, 20, '-')),
            ),
            audit(
                0,
                MateRole::R2,
                ReadState::SinglePlacementGroup,
                Some(group(&a, 0, 3, '+')),
            ),
            audit(
                1,
                MateRole::R1,
                ReadState::SinglePlacementGroup,
                Some(group(&b, 17, 20, '-')),
            ),
            audit(
                1,
                MateRole::R2,
                ReadState::SinglePlacementGroup,
                Some(group(&a, 0, 3, '+')),
            ),
        ];
        let before: Vec<_> = [&a, &b]
            .into_iter()
            .map(|unitig| unitig.sequence.clone())
            .collect();
        let unitigs = vec![a, b];
        let result = summarize_pairs(
            AuditInputMode::PairedEnd,
            true,
            &fragments,
            &audits,
            &unitigs,
        )
        .unwrap();
        assert_eq!(result.links.len(), 1);
        assert_eq!(result.links[0].lane_ordinal, 0);
        assert_eq!(result.links[0].supplied_fragment_instances, 2);
        assert_eq!(result.links[0].a.mate_role, MateRole::R2);
        assert_eq!(result.links[0].a.end, 'L');
        assert_eq!(result.links[0].a.strand, '+');
        assert_eq!(result.links[0].b.mate_role, MateRole::R1);
        assert_eq!(result.links[0].b.end, 'R');
        assert_eq!(result.links[0].b.strand, '-');
        let after: Vec<_> = unitigs
            .iter()
            .map(|unitig| unitig.sequence.clone())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn identical_endpoint_groups_remain_isolated_by_lane() {
        let a = unitig('a', 20);
        let b = unitig('b', 20);
        let fragments = vec![paired_fragment_in_lane(0, 1), paired_fragment_in_lane(1, 0)];
        let audits = vec![
            audit(
                0,
                MateRole::R1,
                ReadState::SinglePlacementGroup,
                Some(group(&a, 0, 3, '+')),
            ),
            audit(
                0,
                MateRole::R2,
                ReadState::SinglePlacementGroup,
                Some(group(&b, 17, 20, '-')),
            ),
            audit(
                1,
                MateRole::R1,
                ReadState::SinglePlacementGroup,
                Some(group(&a, 0, 3, '+')),
            ),
            audit(
                1,
                MateRole::R2,
                ReadState::SinglePlacementGroup,
                Some(group(&b, 17, 20, '-')),
            ),
        ];
        let result = summarize_pairs(
            AuditInputMode::PairedEnd,
            true,
            &fragments,
            &audits,
            &[a.clone(), b.clone()],
        )
        .unwrap();
        assert_eq!(result.links.len(), 2);
        assert_eq!(result.links[0].lane_ordinal, 0);
        assert_eq!(result.links[1].lane_ordinal, 1);
        assert!(result
            .links
            .iter()
            .all(|link| link.supplied_fragment_instances == 1));
        assert_eq!(result.lane_state_counts.len(), 14);
        for lane in 0..=1 {
            let rows = result
                .lane_state_counts
                .iter()
                .filter(|row| row.lane_ordinal == lane)
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 7);
            assert_eq!(rows[6].state, "cross_unitig_observation");
            assert_eq!(rows[6].count, 1);
            assert!(rows[..6].iter().all(|row| row.count == 0));
        }

        let mut reversed_fragments = fragments;
        reversed_fragments.reverse();
        let mut reversed_audits = audits;
        reversed_audits.reverse();
        let reordered = summarize_pairs(
            AuditInputMode::PairedEnd,
            true,
            &reversed_fragments,
            &reversed_audits,
            &[a, b],
        )
        .unwrap();
        assert_eq!(reordered, result);
    }

    #[test]
    fn conditional_summary_rows_and_reconciliations_are_exact() {
        let fragments = vec![paired_fragment_in_lane(0, 0), paired_fragment_in_lane(1, 1)];
        let audits = fragments
            .iter()
            .flat_map(|fragment| {
                [
                    audit(
                        fragment.ordinal,
                        MateRole::R1,
                        ReadState::NotRequested,
                        None,
                    ),
                    audit(
                        fragment.ordinal,
                        MateRole::R2,
                        ReadState::NotRequested,
                        None,
                    ),
                ]
            })
            .collect::<Vec<_>>();
        let disabled =
            summarize_pairs(AuditInputMode::PairedEnd, false, &fragments, &audits, &[]).unwrap();
        assert_eq!(disabled.state_counts.len(), 1);
        assert_eq!(disabled.state_counts[0].count, 2);
        assert_eq!(disabled.lane_state_counts.len(), 2);
        assert_eq!(disabled.lane_state_counts[0].lane_ordinal, 0);
        assert_eq!(disabled.lane_state_counts[1].lane_ordinal, 1);
        assert!(disabled
            .lane_state_counts
            .iter()
            .all(|row| { row.state == PairState::RemapNotRequested.as_str() && row.count == 1 }));

        let singles = vec![Fragment {
            ordinal: 0,
            lane_ordinal: 0,
            reads: vec![ReadRecord {
                role: MateRole::S,
                normalized_id_digest: [0; 32],
                sequence: b"AAA".to_vec(),
                quality: None,
            }],
        }];
        let single_audits = vec![audit(0, MateRole::S, ReadState::Unmapped, None)];
        let single = summarize_pairs(
            AuditInputMode::SingleEnd,
            true,
            &singles,
            &single_audits,
            &[],
        )
        .unwrap();
        assert_eq!(single.state_counts[0].state, "not_paired_input");
        assert_eq!(single.state_counts[0].count, 1);
    }

    #[test]
    fn streaming_path_matches_slice_oracle_without_retaining_read_rows() {
        let a = Unitig {
            sequence: b"AAACCC".to_vec(),
            ..unitig('a', 6)
        };
        let b = Unitig {
            sequence: b"GGGTTA".to_vec(),
            ..unitig('b', 6)
        };
        let mut fragment = paired_fragment(0);
        fragment.reads[0].sequence = b"AAA".to_vec();
        fragment.reads[1].sequence = b"TAA".to_vec();
        let fragments = vec![fragment];
        let config = AuditConfig {
            input_mode: AuditInputMode::PairedEnd,
            remap: true,
            min_base_quality: 20,
            max_mapping_candidates: 10,
            memory_budget_bytes: 512 << 20,
        };

        let mut slice_unitigs = vec![a.clone(), b.clone()];
        let full = crate::audit::audit_fragments(&fragments, &mut slice_unitigs, config).unwrap();
        let pair_oracle = summarize_pairs(
            AuditInputMode::PairedEnd,
            true,
            &fragments,
            &full.read_audits,
            &slice_unitigs,
        )
        .unwrap();

        let mut streamed_unitigs = vec![a, b];
        let streamed = audit_and_summarize_stream(
            fragments.clone().into_iter().map(Ok),
            &mut streamed_unitigs,
            config,
        )
        .unwrap();
        assert_eq!(streamed.audit.state_counts, full.state_counts);
        assert_eq!(
            streamed.audit.global_enumeration_status,
            full.global_enumeration_status
        );
        assert_eq!(streamed.audit.placement_totals, full.placement_totals);
        assert_eq!(streamed.pairs, pair_oracle);
        assert_eq!(streamed_unitigs, slice_unitigs);
        assert_eq!(streamed.pairs.link_support, 1);
    }

    #[test]
    fn spool_audit_admits_decode_without_charging_scan_vectors() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("long-read.fa");
        let mut fasta = Vec::with_capacity(300_016);
        fasta.extend_from_slice(b">long\n");
        fasta.extend(std::iter::repeat_n(b'A', 300_000));
        fasta.push(b'\n');
        fs::write(&input_path, fasta).unwrap();
        let scientific = ScientificConfig::resolve(
            3,
            Profile::RetainAll,
            SupportUnit::SuppliedFragmentInstance,
            None,
            20,
            false,
        )
        .unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific,
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let config = AuditConfig {
            input_mode: AuditInputMode::SingleEnd,
            remap: false,
            min_base_quality: 20,
            max_mapping_candidates: 10,
            memory_budget_bytes: 32 << 20,
        };
        let decode_share = config.memory_budget_bytes / AUDIT_FRAGMENT_DECODE_SHARE_DIVISOR;

        let mut decoded = spool.iter().unwrap();
        let decode_required = match decoded.next_with_decode_memory_limit(0).unwrap() {
            MemoryBoundedNext::RequiresMemory(required) => required,
            other => panic!("expected deferred decode, got {other:?}"),
        };
        assert!(decode_required <= decode_share);
        let mut scanned = spool.iter().unwrap();
        let scan_required = match scanned.next_with_memory_limit(0).unwrap() {
            MemoryBoundedNext::RequiresMemory(required) => required,
            other => panic!("expected deferred scan, got {other:?}"),
        };
        assert!(scan_required > decode_share);

        let mut unitigs = Vec::new();
        let result =
            audit_and_summarize_spool_stream(spool.iter().unwrap(), &mut unitigs, config).unwrap();
        assert_eq!(result.audit.state_counts.len(), 1);
        assert_eq!(result.audit.state_counts[0].state, "not_requested");
        assert_eq!(result.audit.state_counts[0].count, 1);
        assert_eq!(result.pairs.state_counts[0].state, "not_paired_input");
        assert_eq!(result.pairs.state_counts[0].count, 1);
    }
}
