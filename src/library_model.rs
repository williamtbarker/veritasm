//! Experimental, evidence-only paired-end library-model inference.
//!
//! This module consumes observations that an upstream mapper has already
//! established as one complete exact placement group per mate.  It validates
//! coordinates and target metadata, then summarizes each lane independently.
//! It does not prove placement uniqueness, infer an adjacency or gap, alter a
//! graph, or join sequence.  It is intentionally disconnected from the stable
//! assembly pipeline.

use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::model::Topology;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Stable identifier for the experimental inference algorithm.
pub const ALGORITHM_ID: &str = "same_linear_unitig_library_model";
/// Version of the orientation, span, quantile, and availability semantics.
pub const ALGORITHM_VERSION: &str = "experimental-1";

const RESOURCE_FIXED_ALLOWANCE: u64 = 4 << 10;
const RESOURCE_PER_OBSERVATION_ALLOWANCE: u64 = 192;
const RESOURCE_PER_LANE_ALLOWANCE: u64 = 1 << 10;

/// Explicit thresholds and resource limits for library-model inference.
///
/// A lane is available only when it has at least `min_included_pairs`, its
/// unique leading orientation has at least `min_dominant_orientation_pairs`,
/// the leading count divided by all included counts is at least
/// `dominance_numerator / dominance_denominator`, and the leading
/// orientation's empirical P90-P10 width is at most
/// `max_p90_minus_p10_span_width` bases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LibraryModelConfig {
    pub min_included_pairs: u64,
    pub min_dominant_orientation_pairs: u64,
    pub dominance_numerator: u64,
    pub dominance_denominator: u64,
    pub max_p90_minus_p10_span_width: u64,
    pub max_observations: u64,
    pub max_lanes: u64,
    pub max_unitig_id_bytes: u64,
    pub memory_budget_bytes: u64,
}

/// One mate's exact, complete, unique-placement assertion from an upstream
/// evidence producer.
///
/// Coordinates are zero-based, half-open coordinates on the emitted forward
/// unitig sequence.  This type cannot itself prove uniqueness or exact
/// sequence identity; those remain producer proof obligations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighConfidenceMatePlacement {
    pub unitig_id: String,
    pub target_length: u64,
    pub target_topology: Topology,
    pub start: u64,
    pub end: u64,
    pub strand: char,
}

/// One candidate paired-fragment observation.
///
/// `(lane_ordinal, fragment_ordinal)` is its exact identity and must occur at
/// most once.  Observations may arrive in any order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairPlacementObservation {
    pub lane_ordinal: u32,
    pub fragment_ordinal: u64,
    pub r1: HighConfidenceMatePlacement,
    pub r2: HighConfidenceMatePlacement,
}

/// Strand directions in increasing emitted-unitig coordinate order.
///
/// `F` means `+` and `R` means `-`.  Mate role does not determine which
/// letter comes first: the placement with the smaller start coordinate is the
/// left observation.  Equal starts are excluded rather than tie-broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum LibraryOrientation {
    #[serde(rename = "FR")]
    Fr,
    #[serde(rename = "RF")]
    Rf,
    #[serde(rename = "FF")]
    Ff,
    #[serde(rename = "RR")]
    Rr,
}

impl LibraryOrientation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fr => "FR",
            Self::Rf => "RF",
            Self::Ff => "FF",
            Self::Rr => "RR",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Fr => 0,
            Self::Rf => 1,
            Self::Ff => 2,
            Self::Rr => 3,
        }
    }

    const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Fr),
            1 => Some(Self::Rf),
            2 => Some(Self::Ff),
            3 => Some(Self::Rr),
            _ => None,
        }
    }
}

/// Exact per-orientation included-pair counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct OrientationCounts {
    pub fr: u64,
    pub rf: u64,
    pub ff: u64,
    pub rr: u64,
}

impl OrientationCounts {
    const fn as_array(self) -> [u64; 4] {
        [self.fr, self.rf, self.ff, self.rr]
    }
}

/// First-match disposition ledger for supplied observations.
///
/// Invalid coordinates, strands, duplicate identities, or inconsistent
/// metadata are fatal integrity errors and therefore are not exclusion rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ObservationLedger {
    pub included_same_linear_unitig: u64,
    pub excluded_different_unitig: u64,
    pub excluded_non_linear_target: u64,
    pub excluded_coincident_start: u64,
}

impl ObservationLedger {
    /// Total candidate observations reconciled by this ledger.
    pub fn total(self) -> Result<u64> {
        self.included_same_linear_unitig
            .checked_add(self.excluded_different_unitig)
            .and_then(|value| value.checked_add(self.excluded_non_linear_target))
            .and_then(|value| value.checked_add(self.excluded_coincident_start))
            .ok_or_else(|| overflow("library-model disposition total overflow"))
    }
}

/// Fixed empirical nearest-rank order statistics for one orientation.
///
/// P10/P25/P50/P75/P90 use one-indexed rank `ceil(p * n / 100)`.  Thus P50 is
/// the lower empirical median for an even number of observations.  Minimum
/// and maximum are retained so tail outliers remain visible even when they do
/// not change the central interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SpanQuantiles {
    pub observed_pairs: u64,
    pub minimum: u64,
    pub p10: u64,
    pub p25: u64,
    pub median: u64,
    pub p75: u64,
    pub p90: u64,
    pub maximum: u64,
}

impl SpanQuantiles {
    /// Width of the reported central 80% empirical interval.
    pub fn p90_minus_p10(self) -> Result<u64> {
        self.p90
            .checked_sub(self.p10)
            .ok_or_else(|| invalid_result("library-model P90 is below P10"))
    }
}

/// Per-orientation span distributions.  `None` means no included observation
/// had that orientation; it never means zero span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrientationSpanSummaries {
    pub fr: Option<SpanQuantiles>,
    pub rf: Option<SpanQuantiles>,
    pub ff: Option<SpanQuantiles>,
    pub rr: Option<SpanQuantiles>,
}

/// Why a lane may or may not supply a usable experimental model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryModelAvailability {
    Available,
    InsufficientIncludedPairs,
    NoUniqueLeadingOrientation,
    InsufficientLeadingOrientationPairs,
    DominanceBelowThreshold,
    CentralSpanIntervalTooWide,
}

impl LibraryModelAvailability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::InsufficientIncludedPairs => "insufficient_included_pairs",
            Self::NoUniqueLeadingOrientation => "no_unique_leading_orientation",
            Self::InsufficientLeadingOrientationPairs => "insufficient_leading_orientation_pairs",
            Self::DominanceBelowThreshold => "dominance_below_threshold",
            Self::CentralSpanIntervalTooWide => "central_span_interval_too_wide",
        }
    }
}

/// Complete evidence and decision for one lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaneLibraryModel {
    pub lane_ordinal: u32,
    pub ledger: ObservationLedger,
    pub orientation_counts: OrientationCounts,
    pub orientation_span_summaries: OrientationSpanSummaries,
    pub leading_orientation: Option<LibraryOrientation>,
    pub leading_orientation_pairs: u64,
    pub dominance_threshold_met: bool,
    pub availability: LibraryModelAvailability,
}

/// Deterministic experimental result, ordered by numeric lane ordinal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryModelResult {
    pub algorithm_id: &'static str,
    pub algorithm_version: &'static str,
    pub config: LibraryModelConfig,
    pub observations: u64,
    pub ledger: ObservationLedger,
    pub lanes: Vec<LaneLibraryModel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservationDisposition {
    Included {
        orientation: LibraryOrientation,
        span: u64,
    },
    DifferentUnitig,
    NonLinearTarget,
    CoincidentStart,
}

#[derive(Debug, Default)]
struct LaneAccumulator {
    ledger: ObservationLedger,
    spans: [Vec<u64>; 4],
}

/// Bounded streaming accumulator for experimental per-lane models.
///
/// It retains one `(lane, fragment)` identity for duplicate detection and one
/// span for every included observation.  It never retains target sequence or
/// uses the observations for reconstruction.
#[derive(Debug)]
pub struct LibraryModelAccumulator {
    config: LibraryModelConfig,
    observations: u64,
    ledger: ObservationLedger,
    lanes: BTreeMap<u32, LaneAccumulator>,
    seen_observations: BTreeSet<(u32, u64)>,
}

impl LibraryModelAccumulator {
    pub fn new(config: LibraryModelConfig) -> Result<Self> {
        validate_config(config)?;
        ensure_resource_budget(0, 0, config.memory_budget_bytes)?;
        Ok(Self {
            config,
            observations: 0,
            ledger: ObservationLedger::default(),
            lanes: BTreeMap::new(),
            seen_observations: BTreeSet::new(),
        })
    }

    /// Validate and add one observation.  Input order cannot affect a
    /// completed result.
    pub fn observe(&mut self, observation: &PairPlacementObservation) -> Result<()> {
        validate_placement(observation, "R1", &observation.r1, self.config)?;
        validate_placement(observation, "R2", &observation.r2, self.config)?;
        validate_shared_target_metadata(observation)?;

        let identity = (observation.lane_ordinal, observation.fragment_ordinal);
        if self.seen_observations.contains(&identity) {
            return Err(invalid_observation(
                observation,
                "duplicate lane and fragment identity",
            ));
        }

        let next_observations = self
            .observations
            .checked_add(1)
            .ok_or_else(|| overflow("library-model observation count overflow"))?;
        if next_observations > self.config.max_observations {
            return Err(resource_error(format!(
                "library-model observation count {next_observations} exceeds configured maximum {}",
                self.config.max_observations
            )));
        }
        let new_lane = !self.lanes.contains_key(&observation.lane_ordinal);
        let current_lanes = u64::try_from(self.lanes.len())
            .map_err(|_| overflow("library-model lane count does not fit u64"))?;
        let next_lanes = current_lanes
            .checked_add(u64::from(new_lane))
            .ok_or_else(|| overflow("library-model lane count overflow"))?;
        if next_lanes > self.config.max_lanes {
            return Err(resource_error(format!(
                "library-model lane count {next_lanes} exceeds configured maximum {}",
                self.config.max_lanes
            )));
        }
        ensure_resource_budget(
            next_observations,
            next_lanes,
            self.config.memory_budget_bytes,
        )?;

        let disposition = classify_observation(observation)?;
        let lane = self.lanes.entry(observation.lane_ordinal).or_default();
        if let ObservationDisposition::Included { orientation, .. } = disposition {
            lane.spans[orientation.index()]
                .try_reserve(1)
                .map_err(|_| resource_error("cannot allocate library-model span"))?;
        }

        self.seen_observations.insert(identity);
        self.observations = next_observations;
        apply_disposition(&mut self.ledger, disposition)?;
        apply_disposition(&mut lane.ledger, disposition)?;
        if let ObservationDisposition::Included { orientation, span } = disposition {
            lane.spans[orientation.index()].push(span);
        }
        Ok(())
    }

    /// Finalize all lanes in numeric order and verify every reconciliation.
    pub fn finish(self) -> Result<LibraryModelResult> {
        if self.ledger.total()? != self.observations {
            return Err(invalid_result(
                "global library-model ledger does not reconcile",
            ));
        }
        let mut lanes = Vec::new();
        lanes
            .try_reserve_exact(self.lanes.len())
            .map_err(|_| resource_error("cannot allocate finalized library-model lanes"))?;
        for (lane_ordinal, lane) in self.lanes {
            lanes.push(finalize_lane(lane_ordinal, lane, self.config)?);
        }
        let lane_observations = lanes.iter().try_fold(0u64, |sum, lane| {
            sum.checked_add(lane.ledger.total()?)
                .ok_or_else(|| overflow("finalized library-model lane total overflow"))
        })?;
        if lane_observations != self.observations {
            return Err(invalid_result(
                "per-lane library-model ledgers do not reconcile",
            ));
        }
        Ok(LibraryModelResult {
            algorithm_id: ALGORITHM_ID,
            algorithm_version: ALGORITHM_VERSION,
            config: self.config,
            observations: self.observations,
            ledger: self.ledger,
            lanes,
        })
    }
}

/// Infer experimental lane models from an arbitrary observation order.
pub fn infer_library_models<I>(
    observations: I,
    config: LibraryModelConfig,
) -> Result<LibraryModelResult>
where
    I: IntoIterator<Item = PairPlacementObservation>,
{
    let mut accumulator = LibraryModelAccumulator::new(config)?;
    for observation in observations {
        accumulator.observe(&observation)?;
    }
    accumulator.finish()
}

fn validate_config(config: LibraryModelConfig) -> Result<()> {
    if config.min_included_pairs == 0
        || config.min_dominant_orientation_pairs == 0
        || config.max_observations == 0
        || config.max_lanes == 0
        || config.max_unitig_id_bytes == 0
        || config.memory_budget_bytes < RESOURCE_FIXED_ALLOWANCE
    {
        return Err(invalid_config(
            "library-model count, identity, lane, and memory limits must be nonzero and internally admissible",
        ));
    }
    if config.min_included_pairs > config.max_observations
        || config.min_dominant_orientation_pairs > config.max_observations
    {
        return Err(invalid_config(
            "library-model availability counts exceed maximum observations",
        ));
    }
    if config.dominance_denominator == 0
        || config.dominance_numerator > config.dominance_denominator
        || u128::from(config.dominance_numerator) * 2 <= u128::from(config.dominance_denominator)
    {
        return Err(invalid_config(
            "library-model dominance fraction must be greater than one half and at most one",
        ));
    }
    Ok(())
}

fn ensure_resource_budget(observations: u64, lanes: u64, budget: u64) -> Result<()> {
    let required = observations
        .checked_mul(RESOURCE_PER_OBSERVATION_ALLOWANCE)
        .and_then(|value| {
            lanes
                .checked_mul(RESOURCE_PER_LANE_ALLOWANCE)
                .and_then(|lane_bytes| value.checked_add(lane_bytes))
        })
        .and_then(|value| value.checked_add(RESOURCE_FIXED_ALLOWANCE))
        .ok_or_else(|| overflow("library-model resource estimate overflow"))?;
    if required > budget {
        return Err(resource_error(format!(
            "library-model conservative allocation estimate {required} bytes exceeds its {budget}-byte budget"
        )));
    }
    Ok(())
}

fn validate_placement(
    observation: &PairPlacementObservation,
    role: &'static str,
    placement: &HighConfidenceMatePlacement,
    config: LibraryModelConfig,
) -> Result<()> {
    let id_bytes = u64::try_from(placement.unitig_id.len())
        .map_err(|_| overflow("library-model unitig identifier length does not fit u64"))?;
    if id_bytes > config.max_unitig_id_bytes {
        return Err(resource_error(format!(
            "library-model {role} unitig identifier has {id_bytes} bytes, exceeding configured maximum {}",
            config.max_unitig_id_bytes
        )));
    }
    if placement.unitig_id.is_empty() {
        return Err(invalid_observation(
            observation,
            "placement has an empty unitig identifier",
        ));
    }
    if placement.start >= placement.end || placement.end > placement.target_length {
        return Err(invalid_observation(
            observation,
            "placement has invalid zero-based half-open coordinates",
        ));
    }
    if !matches!(placement.strand, '+' | '-') {
        return Err(invalid_observation(
            observation,
            "placement strand is neither plus nor minus",
        ));
    }
    Ok(())
}

fn validate_shared_target_metadata(observation: &PairPlacementObservation) -> Result<()> {
    if observation.r1.unitig_id == observation.r2.unitig_id
        && (observation.r1.target_length != observation.r2.target_length
            || observation.r1.target_topology != observation.r2.target_topology)
    {
        return Err(invalid_observation(
            observation,
            "mates naming one unitig disagree on target metadata",
        ));
    }
    Ok(())
}

fn classify_observation(observation: &PairPlacementObservation) -> Result<ObservationDisposition> {
    if observation.r1.unitig_id != observation.r2.unitig_id {
        return Ok(ObservationDisposition::DifferentUnitig);
    }
    if observation.r1.target_topology != Topology::Linear {
        return Ok(ObservationDisposition::NonLinearTarget);
    }
    if observation.r1.start == observation.r2.start {
        return Ok(ObservationDisposition::CoincidentStart);
    }

    let (left, right) = if observation.r1.start < observation.r2.start {
        (&observation.r1, &observation.r2)
    } else {
        (&observation.r2, &observation.r1)
    };
    let orientation = orientation(left.strand, right.strand)?;
    let left_bound = observation.r1.start.min(observation.r2.start);
    let right_bound = observation.r1.end.max(observation.r2.end);
    let span = right_bound
        .checked_sub(left_bound)
        .ok_or_else(|| invalid_result("library-model span underflow"))?;
    if span == 0 {
        return Err(invalid_result("library-model included a zero span"));
    }
    Ok(ObservationDisposition::Included { orientation, span })
}

fn orientation(left: char, right: char) -> Result<LibraryOrientation> {
    match (left, right) {
        ('+', '-') => Ok(LibraryOrientation::Fr),
        ('-', '+') => Ok(LibraryOrientation::Rf),
        ('+', '+') => Ok(LibraryOrientation::Ff),
        ('-', '-') => Ok(LibraryOrientation::Rr),
        _ => Err(invalid_result(
            "validated library-model strand was not plus or minus",
        )),
    }
}

fn apply_disposition(
    ledger: &mut ObservationLedger,
    disposition: ObservationDisposition,
) -> Result<()> {
    let counter = match disposition {
        ObservationDisposition::Included { .. } => &mut ledger.included_same_linear_unitig,
        ObservationDisposition::DifferentUnitig => &mut ledger.excluded_different_unitig,
        ObservationDisposition::NonLinearTarget => &mut ledger.excluded_non_linear_target,
        ObservationDisposition::CoincidentStart => &mut ledger.excluded_coincident_start,
    };
    checked_increment(counter, "library-model disposition count overflow")
}

fn checked_increment(value: &mut u64, context: &'static str) -> Result<()> {
    *value = value.checked_add(1).ok_or_else(|| overflow(context))?;
    Ok(())
}

fn finalize_lane(
    lane_ordinal: u32,
    mut lane: LaneAccumulator,
    config: LibraryModelConfig,
) -> Result<LaneLibraryModel> {
    if lane.ledger.total()? == 0 {
        return Err(invalid_result("library-model retained an empty lane"));
    }
    for spans in &mut lane.spans {
        spans.sort_unstable();
    }
    let orientation_counts = OrientationCounts {
        fr: length_u64(&lane.spans[0])?,
        rf: length_u64(&lane.spans[1])?,
        ff: length_u64(&lane.spans[2])?,
        rr: length_u64(&lane.spans[3])?,
    };
    let included_from_orientations =
        orientation_counts
            .as_array()
            .into_iter()
            .try_fold(0u64, |sum, count| {
                sum.checked_add(count)
                    .ok_or_else(|| overflow("library-model orientation total overflow"))
            })?;
    if included_from_orientations != lane.ledger.included_same_linear_unitig {
        return Err(invalid_result(
            "library-model orientations do not reconcile to included pairs",
        ));
    }

    let orientation_span_summaries = OrientationSpanSummaries {
        fr: summarize_spans(&lane.spans[0])?,
        rf: summarize_spans(&lane.spans[1])?,
        ff: summarize_spans(&lane.spans[2])?,
        rr: summarize_spans(&lane.spans[3])?,
    };
    let counts = orientation_counts.as_array();
    let maximum = counts.into_iter().max().unwrap_or(0);
    let maximum_count = counts
        .into_iter()
        .filter(|count| *count == maximum && maximum != 0)
        .count();
    let leading_orientation = if maximum_count == 1 {
        counts
            .into_iter()
            .position(|count| count == maximum)
            .and_then(LibraryOrientation::from_index)
    } else {
        None
    };
    let dominance_threshold_met = leading_orientation.is_some()
        && ratio_at_least(
            maximum,
            lane.ledger.included_same_linear_unitig,
            config.dominance_numerator,
            config.dominance_denominator,
        );
    let leading_summary = leading_orientation.and_then(|orientation| match orientation {
        LibraryOrientation::Fr => orientation_span_summaries.fr,
        LibraryOrientation::Rf => orientation_span_summaries.rf,
        LibraryOrientation::Ff => orientation_span_summaries.ff,
        LibraryOrientation::Rr => orientation_span_summaries.rr,
    });
    let availability = if lane.ledger.included_same_linear_unitig < config.min_included_pairs {
        LibraryModelAvailability::InsufficientIncludedPairs
    } else if leading_orientation.is_none() {
        LibraryModelAvailability::NoUniqueLeadingOrientation
    } else if maximum < config.min_dominant_orientation_pairs {
        LibraryModelAvailability::InsufficientLeadingOrientationPairs
    } else if !dominance_threshold_met {
        LibraryModelAvailability::DominanceBelowThreshold
    } else if leading_summary
        .ok_or_else(|| invalid_result("leading orientation lacks a span summary"))?
        .p90_minus_p10()?
        > config.max_p90_minus_p10_span_width
    {
        LibraryModelAvailability::CentralSpanIntervalTooWide
    } else {
        LibraryModelAvailability::Available
    };

    Ok(LaneLibraryModel {
        lane_ordinal,
        ledger: lane.ledger,
        orientation_counts,
        orientation_span_summaries,
        leading_orientation,
        leading_orientation_pairs: maximum,
        dominance_threshold_met,
        availability,
    })
}

fn summarize_spans(sorted_spans: &[u64]) -> Result<Option<SpanQuantiles>> {
    if sorted_spans.is_empty() {
        return Ok(None);
    }
    let observed_pairs = length_u64(sorted_spans)?;
    let value = |percentile| -> Result<u64> {
        let index = empirical_nearest_rank_index(observed_pairs, percentile)?;
        let index = usize::try_from(index)
            .map_err(|_| overflow("library-model quantile index does not fit usize"))?;
        sorted_spans
            .get(index)
            .copied()
            .ok_or_else(|| invalid_result("library-model quantile index is out of bounds"))
    };
    Ok(Some(SpanQuantiles {
        observed_pairs,
        minimum: sorted_spans[0],
        p10: value(10)?,
        p25: value(25)?,
        median: value(50)?,
        p75: value(75)?,
        p90: value(90)?,
        maximum: sorted_spans[sorted_spans.len() - 1],
    }))
}

fn empirical_nearest_rank_index(observations: u64, percentile: u8) -> Result<u64> {
    if observations == 0 || !(1..=100).contains(&percentile) {
        return Err(invalid_result(
            "empirical quantile requires observations and a percentile from one through one hundred",
        ));
    }
    let rank = (u128::from(observations) * u128::from(percentile)).div_ceil(100);
    u64::try_from(rank - 1).map_err(|_| overflow("library-model quantile rank does not fit u64"))
}

fn ratio_at_least(part: u64, whole: u64, numerator: u64, denominator: u64) -> bool {
    whole != 0
        && u128::from(part) * u128::from(denominator) >= u128::from(whole) * u128::from(numerator)
}

fn length_u64<T>(slice: &[T]) -> Result<u64> {
    u64::try_from(slice.len()).map_err(|_| overflow("library-model vector length does not fit u64"))
}

fn invalid_config(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ConfigurationInvalidLimit, context)
}

fn invalid_observation(
    observation: &PairPlacementObservation,
    context: &'static str,
) -> VeritasmError {
    VeritasmError::new(
        ErrorCode::IntegrityArtifact,
        format!(
            "lane {} fragment {}: {context}",
            observation.lane_ordinal, observation.fragment_ordinal
        ),
    )
}

fn invalid_result(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

fn resource_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> LibraryModelConfig {
        LibraryModelConfig {
            min_included_pairs: 10,
            min_dominant_orientation_pairs: 9,
            dominance_numerator: 9,
            dominance_denominator: 10,
            max_p90_minus_p10_span_width: 100,
            max_observations: 10_000,
            max_lanes: 16,
            max_unitig_id_bytes: 1024,
            memory_budget_bytes: 8 << 20,
        }
    }

    fn mate(start: u64, end: u64, strand: char) -> HighConfidenceMatePlacement {
        HighConfidenceMatePlacement {
            unitig_id: "utg-a".to_owned(),
            target_length: 10_000,
            target_topology: Topology::Linear,
            start,
            end,
            strand,
        }
    }

    fn observation(
        lane: u32,
        ordinal: u64,
        span: u64,
        left: char,
        right: char,
    ) -> PairPlacementObservation {
        PairPlacementObservation {
            lane_ordinal: lane,
            fragment_ordinal: ordinal,
            r1: mate(100, 200, left),
            r2: mate(100 + span - 100, 100 + span, right),
        }
    }

    #[test]
    fn classifies_all_orientations_in_coordinate_order() {
        let cases = [
            ('+', '-', LibraryOrientation::Fr),
            ('-', '+', LibraryOrientation::Rf),
            ('+', '+', LibraryOrientation::Ff),
            ('-', '-', LibraryOrientation::Rr),
        ];
        for (left, right, expected) in cases {
            let direct = observation(0, 0, 300, left, right);
            assert!(matches!(
                classify_observation(&direct).unwrap(),
                ObservationDisposition::Included { orientation, span: 300 }
                    if orientation == expected
            ));

            let mut reversed_roles = direct.clone();
            std::mem::swap(&mut reversed_roles.r1, &mut reversed_roles.r2);
            assert!(matches!(
                classify_observation(&reversed_roles).unwrap(),
                ObservationDisposition::Included { orientation, span: 300 }
                    if orientation == expected
            ));
        }
    }

    #[test]
    fn outliers_remain_visible_without_moving_central_quantiles() {
        let mut observations = Vec::new();
        observations.push(observation(1, 0, 150, '+', '-'));
        for ordinal in 1..99 {
            observations.push(observation(1, ordinal, 300, '+', '-'));
        }
        observations.push(observation(1, 99, 2_000, '+', '-'));

        let result = infer_library_models(observations, config()).unwrap();
        let lane = &result.lanes[0];
        let spans = lane.orientation_span_summaries.fr.unwrap();
        assert_eq!(spans.minimum, 150);
        assert_eq!(spans.p10, 300);
        assert_eq!(spans.median, 300);
        assert_eq!(spans.p90, 300);
        assert_eq!(spans.maximum, 2_000);
        assert_eq!(lane.availability, LibraryModelAvailability::Available);
    }

    #[test]
    fn broad_bimodal_span_population_fails_compact_interval_gate() {
        let observations = (0..100).map(|ordinal| {
            let span = if ordinal < 50 { 300 } else { 900 };
            observation(2, ordinal, span, '+', '-')
        });
        let result = infer_library_models(observations, config()).unwrap();
        let lane = &result.lanes[0];
        let spans = lane.orientation_span_summaries.fr.unwrap();
        assert_eq!(spans.p10, 300);
        assert_eq!(spans.p90, 900);
        assert_eq!(spans.p90_minus_p10().unwrap(), 600);
        assert_eq!(
            lane.availability,
            LibraryModelAvailability::CentralSpanIntervalTooWide
        );
    }

    #[test]
    fn insufficient_and_mixed_orientation_states_are_explicit() {
        let insufficient = (0..9).map(|ordinal| observation(3, ordinal, 300, '+', '-'));
        let lane = &infer_library_models(insufficient, config()).unwrap().lanes[0];
        assert_eq!(
            lane.availability,
            LibraryModelAvailability::InsufficientIncludedPairs
        );
        assert_eq!(lane.leading_orientation, Some(LibraryOrientation::Fr));

        let mixed = (0..10).map(|ordinal| {
            if ordinal < 6 {
                observation(4, ordinal, 300, '+', '-')
            } else {
                observation(4, ordinal, 300, '-', '+')
            }
        });
        let mut mixed_config = config();
        mixed_config.min_dominant_orientation_pairs = 1;
        let lane = &infer_library_models(mixed, mixed_config).unwrap().lanes[0];
        assert_eq!(lane.orientation_counts.fr, 6);
        assert_eq!(lane.orientation_counts.rf, 4);
        assert_eq!(
            lane.availability,
            LibraryModelAvailability::DominanceBelowThreshold
        );

        let tied = (0..10).map(|ordinal| {
            if ordinal < 5 {
                observation(5, ordinal, 300, '+', '-')
            } else {
                observation(5, ordinal, 300, '-', '+')
            }
        });
        let lane = &infer_library_models(tied, mixed_config).unwrap().lanes[0];
        assert_eq!(lane.leading_orientation, None);
        assert_eq!(
            lane.availability,
            LibraryModelAvailability::NoUniqueLeadingOrientation
        );
    }

    #[test]
    fn lanes_are_never_pooled() {
        let mut observations = Vec::new();
        for ordinal in 0..10 {
            observations.push(observation(9, ordinal, 300, '+', '-'));
            observations.push(observation(2, ordinal, 400, '-', '+'));
        }
        let result = infer_library_models(observations, config()).unwrap();
        assert_eq!(result.lanes.len(), 2);
        assert_eq!(result.lanes[0].lane_ordinal, 2);
        assert_eq!(
            result.lanes[0].leading_orientation,
            Some(LibraryOrientation::Rf)
        );
        assert_eq!(result.lanes[1].lane_ordinal, 9);
        assert_eq!(
            result.lanes[1].leading_orientation,
            Some(LibraryOrientation::Fr)
        );
        assert!(result
            .lanes
            .iter()
            .all(|lane| lane.availability == LibraryModelAvailability::Available));
    }

    #[test]
    fn exclusions_follow_one_reconciling_precedence_order() {
        let included = observation(0, 0, 300, '+', '-');
        let mut different = observation(0, 1, 300, '+', '-');
        different.r2.unitig_id = "utg-b".to_owned();
        different.r2.target_topology = Topology::ClosedGraphWalk;
        let mut non_linear = observation(0, 2, 300, '+', '-');
        non_linear.r1.target_topology = Topology::ClosedGraphWalk;
        non_linear.r2.target_topology = Topology::ClosedGraphWalk;
        let mut coincident = observation(0, 3, 300, '+', '-');
        coincident.r2.start = coincident.r1.start;

        let result =
            infer_library_models([included, different, non_linear, coincident], config()).unwrap();
        assert_eq!(result.observations, 4);
        assert_eq!(result.ledger.included_same_linear_unitig, 1);
        assert_eq!(result.ledger.excluded_different_unitig, 1);
        assert_eq!(result.ledger.excluded_non_linear_target, 1);
        assert_eq!(result.ledger.excluded_coincident_start, 1);
        assert_eq!(result.ledger.total().unwrap(), 4);
        assert_eq!(result.lanes[0].ledger, result.ledger);
    }

    #[test]
    fn invalid_coordinates_strands_and_metadata_are_fatal_without_mutation() {
        let mut accumulator = LibraryModelAccumulator::new(config()).unwrap();
        let mut invalid = observation(0, 0, 300, '+', '-');
        invalid.r1.end = invalid.r1.start;
        assert_eq!(
            accumulator.observe(&invalid).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        invalid = observation(0, 1, 300, '+', '-');
        invalid.r2.end = invalid.r2.target_length + 1;
        assert_eq!(
            accumulator.observe(&invalid).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        invalid = observation(0, 2, 300, '+', 'x');
        assert_eq!(
            accumulator.observe(&invalid).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        invalid = observation(0, 3, 300, '+', '-');
        invalid.r2.target_length += 1;
        assert_eq!(
            accumulator.observe(&invalid).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        accumulator
            .observe(&observation(0, 4, 300, '+', '-'))
            .unwrap();
        let result = accumulator.finish().unwrap();
        assert_eq!(result.observations, 1);
        assert_eq!(result.ledger.included_same_linear_unitig, 1);
    }

    #[test]
    fn arbitrary_input_order_has_identical_output_and_duplicates_fail() {
        let ordered: Vec<_> = (0..20)
            .map(|ordinal| observation(ordinal as u32 % 2, ordinal, 250 + ordinal, '+', '-'))
            .collect();
        let mut reversed = ordered.clone();
        reversed.reverse();
        let expected = infer_library_models(ordered.clone(), config()).unwrap();
        let actual = infer_library_models(reversed, config()).unwrap();
        assert_eq!(actual, expected);

        let duplicate = vec![ordered[0].clone(), ordered[0].clone()];
        assert_eq!(
            infer_library_models(duplicate, config())
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn integer_nearest_rank_quantiles_have_fixed_even_sample_semantics() {
        let spans = summarize_spans(&[100, 200, 300, 400]).unwrap().unwrap();
        assert_eq!(spans.p10, 100);
        assert_eq!(spans.p25, 100);
        assert_eq!(spans.median, 200);
        assert_eq!(spans.p75, 300);
        assert_eq!(spans.p90, 400);
        assert_eq!(
            empirical_nearest_rank_index(u64::MAX, 90).unwrap(),
            16_602_069_666_338_596_453
        );
    }

    #[test]
    fn dominance_cross_multiplication_cannot_overflow() {
        assert!(ratio_at_least(u64::MAX, u64::MAX, u64::MAX, u64::MAX));
        assert!(!ratio_at_least(u64::MAX / 2, u64::MAX, u64::MAX, u64::MAX));
        let mut count = u64::MAX;
        assert_eq!(
            checked_increment(&mut count, "test overflow")
                .unwrap_err()
                .code(),
            ErrorCode::ResourceIntegerOverflow
        );
    }

    #[test]
    fn observation_lane_identifier_and_memory_limits_fail_closed() {
        let mut limited = config();
        limited.max_observations = 1;
        limited.min_included_pairs = 1;
        limited.min_dominant_orientation_pairs = 1;
        let error = infer_library_models(
            [
                observation(0, 0, 300, '+', '-'),
                observation(0, 1, 300, '+', '-'),
            ],
            limited,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);

        limited = config();
        limited.max_lanes = 1;
        let error = infer_library_models(
            [
                observation(0, 0, 300, '+', '-'),
                observation(1, 0, 300, '+', '-'),
            ],
            limited,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);

        limited = config();
        limited.max_unitig_id_bytes = 4;
        let error = infer_library_models([observation(0, 0, 300, '+', '-')], limited).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);

        limited = config();
        limited.memory_budget_bytes = RESOURCE_FIXED_ALLOWANCE
            + RESOURCE_PER_LANE_ALLOWANCE
            + RESOURCE_PER_OBSERVATION_ALLOWANCE
            - 1;
        let error = infer_library_models([observation(0, 0, 300, '+', '-')], limited).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn invalid_configuration_is_rejected() {
        let mut invalid = config();
        invalid.dominance_denominator = 0;
        assert_eq!(
            LibraryModelAccumulator::new(invalid).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        invalid = config();
        invalid.dominance_numerator = 1;
        invalid.dominance_denominator = 2;
        assert_eq!(
            LibraryModelAccumulator::new(invalid).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
    }
}
