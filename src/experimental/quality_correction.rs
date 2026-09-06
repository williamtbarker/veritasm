//! Exact-spectrum quality correction and IUPAC-resolution experiment.
//!
//! This module is isolated from the stable assembly pipeline. It borrows an
//! immutable raw-read spectrum, never updates support from corrected sequence,
//! and accepts a correction only when one threshold-qualified candidate
//! strictly Pareto-dominates every other qualified candidate. Probabilistic
//! membership structures are deliberately not accepted as evidence.

use super::wide_kmer::{canonical_code, encode_exact_bases, validate_code, validate_k, PackedKmer};
use crate::error::{ErrorCode, Result, VeritasmError};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::mem::size_of;
use std::sync::OnceLock;

const READ_IDENTITY_DOMAIN: &[u8] = b"veritasm.experimental.quality-correction-read-identity.v1\0";
const SPECTRUM_DOMAIN: &[u8] = b"veritasm.experimental.raw-spectrum.v1\0";
const CONFIG_DOMAIN: &[u8] = b"veritasm.experimental.quality-correction-config.v4\0";
const SOURCE_DOMAIN: &[u8] = b"veritasm.experimental.quality-correction-source.v1\0";
const ALGORITHM_DOMAIN: &[u8] = b"veritasm.experimental.quality-correction.algorithm.v4\0";
const CANDIDATE_IDENTITY_DOMAIN: &[u8] = b"veritasm.experimental.quality-correction-candidate.v1\0";
const EVALUATED_CANDIDATE_SET_DOMAIN: &[u8] =
    b"veritasm.experimental.quality-correction-evaluated-set.v1\0";
const DNA: [u8; 4] = *b"ACGT";

/// Content identities and support semantics supplied by the raw-evidence
/// producer. These hashes are integrity labels, not signatures or proof that
/// the caller described the source truthfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrectionSourceDescriptor {
    pub source_snapshot_sha256: [u8; 32],
    pub source_layout_sha256: [u8; 32],
    pub qc_policy_sha256: [u8; 32],
    pub support_unit: CorrectionSupportUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionSupportUnit {
    ExactKmerOccurrence,
}

/// Whether this experimental stage may propose sequence changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionMode {
    /// Retain the raw spelling without entering candidate search.
    RawOnly,
    /// Abstain on every spectrum-only base-changing correction because this
    /// slice has no independent context channel. A proposal that also
    /// intersects nonzero subthreshold raw evidence receives the more
    /// specific low-count-conflict disposition. This is the default.
    DiversityPreserving,
    /// Permit spectrum-only consensus correction while reporting every
    /// affected nonzero subthreshold raw window. This mode is experimental and
    /// must remain a separate arm from the immutable raw result.
    ConsensusExperimental,
}

/// One complete canonical k-mer and its exact occurrence count in the
/// immutable, uncorrected evidence source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpectrumEntry {
    pub key: PackedKmer,
    pub raw_occurrences: u64,
}

/// Sorted, unique exact support at one k.
#[derive(Debug, Clone, Copy)]
pub struct TrustedSpectrum<'a> {
    pub k: u8,
    pub min_trusted_occurrences: u64,
    pub entries: &'a [SpectrumEntry],
}

/// Immutable decoded read input. `phred` contains numeric Q0..Q93 values, not
/// FASTQ's ASCII Phred+33 bytes.
#[derive(Debug, Clone, Copy)]
pub struct CorrectionRead<'a> {
    pub read_ordinal: u64,
    pub bases: &'a [u8],
    pub phred: &'a [u8],
}

/// Evidence thresholds recorded in every journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrectionThresholds {
    pub max_edit_phred: u8,
    pub min_trusted_window_gain: u64,
    pub min_summed_occurrence_support_gain: u64,
    pub min_supporting_k_layers: u8,
    pub require_all_affected_windows_trusted: bool,
}

/// Explicit fallback after an exhaustive bounded search finds no admissible
/// unique correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedPolicy {
    /// Return the original sequence with a typed unresolved reason.
    LeaveUnchanged,
    /// Withhold the read from downstream sequence output.
    Quarantine,
    /// Opt-in lossy QC: remove only a contiguous prefix and/or suffix whose
    /// qualities are at or below the frozen threshold or whose symbols are
    /// ambiguous. Internal bases are untouched and no new adjacency is made.
    TrimLowQualityTerminals {
        max_terminal_phred: u8,
        min_retained_bases: u32,
    },
}

/// Hard admissions for one read and one deterministic batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrectionLimits {
    pub max_read_bases: u64,
    pub max_spectra: u16,
    /// Total borrowed exact-spectrum entries admitted before validation and
    /// hashing. This bounds work, not caller-owned allocation bytes.
    pub max_spectrum_entries_total: u64,
    /// Checked upper bound on exact spectrum-key comparisons for one read.
    pub max_spectrum_comparisons_per_read: u64,
    pub max_editable_positions: u32,
    pub max_substitutions: u8,
    pub max_candidates_per_read: u64,
    pub max_window_evaluations_per_read: u64,
    /// Checked upper bound on bounded-summary-heap comparisons for one read.
    pub max_summary_comparisons_per_read: u64,
    /// Checked weighted admission bound for auxiliary two-pass search work.
    pub max_search_work_units_per_read: u64,
    pub max_journal_entries_per_read: u64,
    pub max_candidate_summaries_per_read: u16,
    pub max_accounted_bytes_per_read: u64,
    pub max_batch_reads: u64,
    pub max_batch_bases: u64,
    pub max_batch_accounted_bytes: u64,
    pub max_worker_threads: u16,
}

/// Frozen experimental configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrectionConfig {
    pub mode: CorrectionMode,
    pub thresholds: CorrectionThresholds,
    pub unresolved_policy: UnresolvedPolicy,
    pub limits: CorrectionLimits,
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        Self {
            mode: CorrectionMode::DiversityPreserving,
            thresholds: CorrectionThresholds {
                max_edit_phred: 20,
                min_trusted_window_gain: 1,
                min_summed_occurrence_support_gain: 1,
                min_supporting_k_layers: 1,
                require_all_affected_windows_trusted: true,
            },
            unresolved_policy: UnresolvedPolicy::Quarantine,
            limits: CorrectionLimits {
                max_read_bases: 1_000,
                max_spectra: 16,
                max_spectrum_entries_total: 1_000_000_000,
                max_spectrum_comparisons_per_read: 1_000_000_000,
                max_editable_positions: 32,
                max_substitutions: 2,
                max_candidates_per_read: 100_000,
                max_window_evaluations_per_read: 100_000_000,
                max_summary_comparisons_per_read: 100_000_000,
                max_search_work_units_per_read: 1_000_000_000,
                max_journal_entries_per_read: 16,
                max_candidate_summaries_per_read: 16,
                max_accounted_bytes_per_read: 8 * 1024 * 1024,
                max_batch_reads: 1_000_000,
                max_batch_bases: 1_000_000_000,
                max_batch_accounted_bytes: 4 * 1024 * 1024 * 1024,
                max_worker_threads: 256,
            },
        }
    }
}

/// Exact per-k candidate evidence. `unsupported_windows` includes absent and
/// ambiguous windows; no approximate lookup can make a window trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerEvidence {
    pub k: u8,
    pub possible_windows: u64,
    pub exact_windows: u64,
    pub trusted_windows: u64,
    pub unsupported_windows: u64,
    pub summed_occurrence_support: u64,
    pub affected_windows: u64,
    pub affected_trusted_windows: u64,
    /// Nonzero subthreshold exact windows in the raw spelling that the edits
    /// would replace.
    pub affected_low_count_windows: u64,
    pub affected_low_count_support_sum: u64,
    /// Nonzero subthreshold exact windows in this candidate spelling. These
    /// expose supported but below-threshold IUPAC alternatives even when the
    /// ambiguous raw spelling has no exact k-mer key.
    pub affected_candidate_low_count_windows: u64,
    pub affected_candidate_low_count_support_sum: u64,
}

/// Checked, interpretable score components. They are intentionally not
/// collapsed into an opaque weighted scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEvidence {
    pub edit_count: u8,
    pub quality_penalty: u64,
    pub supporting_k_layers: u8,
    pub trusted_windows: u64,
    pub unsupported_windows: u64,
    pub summed_occurrence_support: u64,
    pub affected_low_count_windows: u64,
    pub affected_low_count_support_sum: u64,
    pub affected_candidate_low_count_windows: u64,
    pub affected_candidate_low_count_support_sum: u64,
    pub layers: Vec<LayerEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BaseEditKind {
    Substitution,
    AmbiguityResolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BaseSubstitution {
    pub position: u32,
    pub before: u8,
    pub after: u8,
    pub phred: u8,
    pub kind: BaseEditKind,
}

/// Bounded, fixed-size evidence for one evaluated alternative.
/// `changes` is position ordered and contains `edit_count` populated entries.
/// The versioned total order is the declaration order below: candidate
/// identity, edits, edit count, qualification state, and then every aggregate
/// evidence field. [`BaseEditKind`] orders substitution before ambiguity
/// resolution. The bounded heap and its final output both use this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CandidateAlternativeSummary {
    pub candidate_identity: [u8; 32],
    pub changes: [Option<BaseSubstitution>; 3],
    pub edit_count: u8,
    pub threshold_qualified: bool,
    pub supporting_k_layers: u8,
    pub trusted_windows: u64,
    pub unsupported_windows: u64,
    pub summed_occurrence_support: u64,
    pub affected_low_count_windows: u64,
    pub affected_low_count_support_sum: u64,
    pub affected_candidate_low_count_windows: u64,
    pub affected_candidate_low_count_support_sum: u64,
}

/// Complete counts and a bounded deterministic sample of every evaluated
/// alternative, including candidates below the frozen thresholds. The digest
/// covers every evaluated candidate in the versioned enumeration order,
/// including full per-layer evidence and qualification state; it is verified
/// by engine-bound recomputation, not by transformation replay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateSearchEvidence {
    pub evaluated_candidates: u64,
    pub evaluated_exact_substitution_candidates: u64,
    pub evaluated_ambiguity_resolution_candidates: u64,
    pub evaluated_mixed_edit_candidates: u64,
    pub threshold_qualified_candidates: u64,
    pub evaluated_candidates_with_low_count_evidence: u64,
    pub affected_low_count_windows: u64,
    pub affected_low_count_support_sum: u64,
    pub affected_candidate_low_count_windows: u64,
    pub affected_candidate_low_count_support_sum: u64,
    pub evaluated_candidate_set_digest: [u8; 32],
    /// The lexicographically smallest identities, sorted by
    /// `candidate_identity` and bounded by the configured summary cap.
    pub retained_alternatives: Vec<CandidateAlternativeSummary>,
    pub retained_alternatives_truncated: bool,
    /// Exact comparisons made by the deterministic bounded max-heap,
    /// including its final in-place heap sort.
    pub retained_alternative_comparisons: u64,
}

impl CandidateSearchEvidence {
    fn validate(&self, summary_limit: u16) -> Result<()> {
        let classified = self
            .evaluated_exact_substitution_candidates
            .checked_add(self.evaluated_ambiguity_resolution_candidates)
            .and_then(|value| value.checked_sub(self.evaluated_mixed_edit_candidates))
            .ok_or_else(|| overflow("evaluated candidate class count overflow"))?;
        let retained = as_u64(
            self.retained_alternatives.len(),
            "retained candidate-alternative count",
        )?;
        let expected_retained = self.evaluated_candidates.min(u64::from(summary_limit));
        if classified != self.evaluated_candidates
            || self.threshold_qualified_candidates > self.evaluated_candidates
            || self.evaluated_candidates_with_low_count_evidence > self.evaluated_candidates
            || retained != expected_retained
            || self.retained_alternatives_truncated
                != (self.evaluated_candidates > u64::from(summary_limit))
            || self
                .retained_alternatives
                .windows(2)
                .any(|pair| pair[0] > pair[1])
        {
            return Err(invariant_error(
                "bounded candidate-search evidence is inconsistent",
            ));
        }
        for alternative in &self.retained_alternatives {
            let populated = alternative
                .changes
                .iter()
                .take_while(|change| change.is_some())
                .count();
            if populated != usize::from(alternative.edit_count)
                || alternative.changes[populated..].iter().any(Option::is_some)
            {
                return Err(invariant_error(
                    "candidate alternative has inconsistent edits",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnchangedReason {
    RawOnly,
    AlreadyTrusted,
    NoEditableBases,
    EvidenceBelowThresholds,
    TerminalTrimCriteriaNotMet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapKind {
    ReadBases,
    EditablePositions,
    Candidates,
    WindowEvaluations,
    SpectrumComparisons,
    SummaryComparisons,
    SearchWorkUnits,
    AccountedBytes,
    JournalEntries,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason {
    UnresolvedEvidence,
    ConflictingCandidates,
    DiversityLowCountConflict {
        affected_windows: u64,
        affected_support_sum: u64,
    },
    IndependentContextUnavailable,
    CapExceeded {
        kind: CapKind,
        projected: u64,
        limit: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimReason {
    UnresolvedLowQualityOrAmbiguousTermini,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadDisposition {
    Unchanged(UnchangedReason),
    Corrected,
    Trimmed {
        retained: RetainedRange,
        reason: TrimReason,
    },
    Quarantined(QuarantineReason),
}

/// Complete replay record for one read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionJournal {
    pub read_ordinal: u64,
    pub input_identity: [u8; 32],
    pub spectrum_identity: [u8; 32],
    pub source_descriptor_identity: [u8; 32],
    pub config_identity: [u8; 32],
    pub algorithm_identity: [u8; 32],
    pub thresholds: CorrectionThresholds,
    pub disposition: ReadDisposition,
    pub substitutions: Vec<BaseSubstitution>,
    pub baseline_evidence: Option<CandidateEvidence>,
    pub accepted_evidence: Option<CandidateEvidence>,
    pub candidate_search_evidence: CandidateSearchEvidence,
    pub candidate_projection_performed: bool,
    pub projected_candidates: u64,
    pub evaluated_candidates: u64,
    pub candidate_score_evaluations: u64,
    pub window_evaluations: u64,
    /// Admitted worst-case exact-key comparison projection.
    pub projected_spectrum_comparisons: u64,
    /// Exact spectrum-key comparisons actually executed.
    pub spectrum_comparisons: u64,
    /// Admitted worst-case bounded-summary-heap comparison projection.
    pub projected_summary_comparisons: u64,
    /// Versioned conservative auxiliary-work projection. Window, exact-key,
    /// and heap comparisons are budgeted separately.
    pub projected_search_work_units: u64,
}

/// Mutually exclusive base and decision accounting for one read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CorrectionAccounting {
    pub reads: u64,
    pub input_bases: u64,
    pub emitted_bases: u64,
    pub trimmed_bases: u64,
    pub quarantined_bases: u64,
    pub unchanged_reads: u64,
    pub corrected_reads: u64,
    pub trimmed_reads: u64,
    pub quarantined_reads: u64,
    pub substitutions: u64,
    pub evidence_conflicts: u64,
    pub cap_quarantines: u64,
    pub diversity_low_count_quarantines: u64,
    pub independent_context_quarantines: u64,
    pub reads_with_candidate_projection: u64,
    pub projected_candidates: u64,
    pub evaluated_candidates: u64,
    pub candidate_score_evaluations: u64,
    pub window_evaluations: u64,
    pub projected_spectrum_comparisons: u64,
    pub spectrum_comparisons: u64,
    pub projected_summary_comparisons: u64,
    pub summary_comparisons: u64,
    pub projected_search_work_units: u64,
    pub consensus_low_count_windows: u64,
    pub consensus_low_count_support_sum: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionDecision {
    pub read_ordinal: u64,
    /// `None` means quarantined, not an empty biological sequence.
    pub emitted_bases: Option<Vec<u8>>,
    pub journal: CorrectionJournal,
    pub accounting: CorrectionAccounting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionBatch {
    /// Canonical order by unique read ordinal, independent of caller order and
    /// worker scheduling.
    pub decisions: Vec<CorrectionDecision>,
    pub accounting: CorrectionAccounting,
}

#[derive(Debug, Clone, Copy)]
struct PreparedSpectrum<'a> {
    spectrum: TrustedSpectrum<'a>,
}

/// Validated, immutable correction engine. There is no API that inserts
/// corrected keys into the spectra.
#[derive(Debug)]
pub struct QualityCorrectionEngine<'a> {
    config: CorrectionConfig,
    spectra: Vec<PreparedSpectrum<'a>>,
    spectrum_identity: [u8; 32],
    source_descriptor_identity: [u8; 32],
    config_identity: [u8; 32],
    algorithm_identity: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
struct EditablePosition {
    position: usize,
    before: u8,
    phred: u8,
    choices: [u8; 4],
    choice_count: u8,
    kind: BaseEditKind,
}

#[derive(Debug, Clone)]
struct RankedCandidate {
    bases: Vec<u8>,
    changes: Vec<BaseSubstitution>,
    evidence: CandidateEvidence,
}

#[derive(Debug, Clone, Copy, Default)]
struct CorrectionWork {
    window_evaluations: u64,
    projected_spectrum_comparisons: u64,
    spectrum_comparisons: u64,
    projected_summary_comparisons: u64,
    projected_search_work_units: u64,
}

struct CandidateSearchRecorder {
    evidence: CandidateSearchEvidence,
    digest: Sha256,
    retained: BoundedAlternativeHeap,
}

struct BoundedAlternativeHeap {
    values: Vec<CandidateAlternativeSummary>,
    limit: usize,
    comparisons: u64,
}

impl BoundedAlternativeHeap {
    fn new(limit: usize) -> Result<Self> {
        let mut values = Vec::new();
        values.try_reserve_exact(limit).map_err(|cause| {
            memory_error(format!(
                "cannot reserve bounded candidate summaries: {cause}"
            ))
        })?;
        Ok(Self {
            values,
            limit,
            comparisons: 0,
        })
    }

    fn compare(
        &mut self,
        left: CandidateAlternativeSummary,
        right: CandidateAlternativeSummary,
    ) -> Result<Ordering> {
        self.comparisons = checked_add(
            self.comparisons,
            1,
            "candidate-summary comparison count overflow",
        )?;
        Ok(left.cmp(&right))
    }

    fn consider(&mut self, summary: CandidateAlternativeSummary) -> Result<()> {
        if self.values.len() < self.limit {
            self.values.push(summary);
            return self.sift_up(self.values.len() - 1);
        }
        let root = self
            .values
            .first()
            .copied()
            .ok_or_else(|| invariant_error("bounded candidate heap has a zero limit"))?;
        if self.compare(summary, root)? != Ordering::Less {
            return Ok(());
        }
        self.values[0] = summary;
        let end = self.values.len();
        self.sift_down(0, end)
    }

    fn sift_up(&mut self, mut child: usize) -> Result<()> {
        while child > 0 {
            let parent = (child - 1) / 2;
            if self.compare(self.values[parent], self.values[child])? != Ordering::Less {
                break;
            }
            self.values.swap(parent, child);
            child = parent;
        }
        Ok(())
    }

    fn sift_down(&mut self, mut root: usize, end: usize) -> Result<()> {
        loop {
            let Some(left) = root.checked_mul(2).and_then(|value| value.checked_add(1)) else {
                return Err(overflow("candidate-summary heap index overflow"));
            };
            if left >= end {
                return Ok(());
            }
            let right = left + 1;
            let mut larger = left;
            if right < end && self.compare(self.values[left], self.values[right])? == Ordering::Less
            {
                larger = right;
            }
            if self.compare(self.values[root], self.values[larger])? != Ordering::Less {
                return Ok(());
            }
            self.values.swap(root, larger);
            root = larger;
        }
    }

    fn into_sorted(mut self) -> Result<(Vec<CandidateAlternativeSummary>, u64)> {
        let mut end = self.values.len();
        while end > 1 {
            self.values.swap(0, end - 1);
            end -= 1;
            self.sift_down(0, end)?;
        }
        Ok((self.values, self.comparisons))
    }
}

impl CandidateSearchRecorder {
    fn new(summary_limit: usize) -> Result<Self> {
        let mut digest = Sha256::new();
        digest.update(EVALUATED_CANDIDATE_SET_DOMAIN);
        Ok(Self {
            evidence: CandidateSearchEvidence::default(),
            digest,
            retained: BoundedAlternativeHeap::new(summary_limit)?,
        })
    }

    fn record(
        &mut self,
        candidate: &[u8],
        changes: &[BaseSubstitution],
        candidate_evidence: &CandidateEvidence,
        threshold_qualified: bool,
    ) -> Result<()> {
        if changes.is_empty()
            || changes.len() > 3
            || usize::from(candidate_evidence.edit_count) != changes.len()
        {
            return Err(invariant_error(
                "evaluated candidate has an invalid edit count",
            ));
        }
        self.evidence.evaluated_candidates = checked_add(
            self.evidence.evaluated_candidates,
            1,
            "evaluated-candidate evidence count overflow",
        )?;
        if threshold_qualified {
            self.evidence.threshold_qualified_candidates = checked_add(
                self.evidence.threshold_qualified_candidates,
                1,
                "threshold-qualified candidate count overflow",
            )?;
        }
        let has_substitution = changes
            .iter()
            .any(|change| change.kind == BaseEditKind::Substitution);
        let has_ambiguity = changes
            .iter()
            .any(|change| change.kind == BaseEditKind::AmbiguityResolution);
        if has_substitution {
            self.evidence.evaluated_exact_substitution_candidates = checked_add(
                self.evidence.evaluated_exact_substitution_candidates,
                1,
                "evaluated exact-substitution candidate count overflow",
            )?;
        }
        if has_ambiguity {
            self.evidence.evaluated_ambiguity_resolution_candidates = checked_add(
                self.evidence.evaluated_ambiguity_resolution_candidates,
                1,
                "evaluated ambiguity-resolution candidate count overflow",
            )?;
        }
        if has_substitution && has_ambiguity {
            self.evidence.evaluated_mixed_edit_candidates = checked_add(
                self.evidence.evaluated_mixed_edit_candidates,
                1,
                "evaluated mixed-edit candidate count overflow",
            )?;
        }
        if candidate_evidence.affected_low_count_windows > 0
            || candidate_evidence.affected_candidate_low_count_windows > 0
        {
            self.evidence.evaluated_candidates_with_low_count_evidence = checked_add(
                self.evidence.evaluated_candidates_with_low_count_evidence,
                1,
                "evaluated low-count candidate count overflow",
            )?;
        }
        self.evidence.affected_low_count_windows = checked_add(
            self.evidence.affected_low_count_windows,
            candidate_evidence.affected_low_count_windows,
            "evaluated-candidate affected low-count window overflow",
        )?;
        self.evidence.affected_low_count_support_sum = checked_add(
            self.evidence.affected_low_count_support_sum,
            candidate_evidence.affected_low_count_support_sum,
            "evaluated-candidate affected low-count support overflow",
        )?;
        self.evidence.affected_candidate_low_count_windows = checked_add(
            self.evidence.affected_candidate_low_count_windows,
            candidate_evidence.affected_candidate_low_count_windows,
            "evaluated candidate-spelling affected low-count window overflow",
        )?;
        self.evidence.affected_candidate_low_count_support_sum = checked_add(
            self.evidence.affected_candidate_low_count_support_sum,
            candidate_evidence.affected_candidate_low_count_support_sum,
            "evaluated candidate-spelling affected low-count support overflow",
        )?;

        let identity = candidate_identity(candidate)?;
        update_evaluated_candidate_digest(
            &mut self.digest,
            identity,
            changes,
            candidate_evidence,
            threshold_qualified,
        )?;

        let mut recorded_changes = [None; 3];
        for (slot, &change) in recorded_changes.iter_mut().zip(changes) {
            *slot = Some(change);
        }
        let summary = CandidateAlternativeSummary {
            candidate_identity: identity,
            changes: recorded_changes,
            edit_count: candidate_evidence.edit_count,
            threshold_qualified,
            supporting_k_layers: candidate_evidence.supporting_k_layers,
            trusted_windows: candidate_evidence.trusted_windows,
            unsupported_windows: candidate_evidence.unsupported_windows,
            summed_occurrence_support: candidate_evidence.summed_occurrence_support,
            affected_low_count_windows: candidate_evidence.affected_low_count_windows,
            affected_low_count_support_sum: candidate_evidence.affected_low_count_support_sum,
            affected_candidate_low_count_windows: candidate_evidence
                .affected_candidate_low_count_windows,
            affected_candidate_low_count_support_sum: candidate_evidence
                .affected_candidate_low_count_support_sum,
        };
        self.retained.consider(summary)?;
        self.evidence.retained_alternatives_truncated = self.evidence.evaluated_candidates
            > as_u64(self.retained.limit, "candidate-summary heap limit")?;
        Ok(())
    }

    fn finish(mut self) -> Result<CandidateSearchEvidence> {
        self.digest.update([0xff]);
        self.digest
            .update(self.evidence.evaluated_candidates.to_be_bytes());
        self.evidence.evaluated_candidate_set_digest = self.digest.finalize().into();
        let (retained, comparisons) = self.retained.into_sorted()?;
        self.evidence.retained_alternatives = retained;
        self.evidence.retained_alternative_comparisons = comparisons;
        Ok(self.evidence)
    }
}

impl<'a> QualityCorrectionEngine<'a> {
    pub fn new(
        config: CorrectionConfig,
        source: CorrectionSourceDescriptor,
        spectra: &[TrustedSpectrum<'a>],
    ) -> Result<Self> {
        validate_config(config, spectra.len())?;
        validate_source_descriptor(source)?;
        let total_spectrum_entries = spectra.iter().try_fold(0_u64, |total, spectrum| {
            checked_add(
                total,
                as_u64(spectrum.entries.len(), "raw spectrum entry count")?,
                "raw spectrum entry-count sum overflow",
            )
        })?;
        enforce_global_limit(
            total_spectrum_entries,
            config.limits.max_spectrum_entries_total,
            ErrorCode::ResourceRetainedKeys,
            "experimental correction raw-spectrum entries",
        )?;
        let mut prepared = Vec::new();
        prepared.try_reserve_exact(spectra.len()).map_err(|cause| {
            memory_error(format!(
                "cannot reserve experimental spectrum descriptors: {cause}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(SPECTRUM_DOMAIN);
        digest.update(
            u64::try_from(spectra.len())
                .map_err(|_| overflow("spectrum count does not fit u64"))?
                .to_be_bytes(),
        );
        let mut previous_k = None;
        for spectrum in spectra {
            validate_spectrum(*spectrum, previous_k)?;
            previous_k = Some(spectrum.k);
            digest.update([spectrum.k]);
            digest.update(spectrum.min_trusted_occurrences.to_be_bytes());
            digest.update(
                u64::try_from(spectrum.entries.len())
                    .map_err(|_| overflow("spectrum entry count does not fit u64"))?
                    .to_be_bytes(),
            );
            for entry in spectrum.entries {
                digest.update(entry.key.to_be_bytes());
                digest.update(entry.raw_occurrences.to_be_bytes());
            }
            prepared.push(PreparedSpectrum {
                spectrum: *spectrum,
            });
        }
        Ok(Self {
            config,
            spectra: prepared,
            spectrum_identity: digest.finalize().into(),
            source_descriptor_identity: source_descriptor_identity(source),
            config_identity: config_identity(config),
            algorithm_identity: Sha256::digest(ALGORITHM_DOMAIN).into(),
        })
    }

    pub const fn config(&self) -> CorrectionConfig {
        self.config
    }

    pub const fn spectrum_identity(&self) -> [u8; 32] {
        self.spectrum_identity
    }

    pub const fn source_descriptor_identity(&self) -> [u8; 32] {
        self.source_descriptor_identity
    }

    pub const fn config_identity(&self) -> [u8; 32] {
        self.config_identity
    }

    /// Exhaustively evaluate the configured bounded substitution space.
    pub fn correct_read(&self, read: CorrectionRead<'_>) -> Result<CorrectionDecision> {
        let input_bases = validate_read_lengths(read)?;
        // Decide cheap, attacker-controlled size admission before nucleotide
        // validation and hashing. A cap journal still needs a validated source
        // identity, so those operations are performed only after the outcome
        // is known.
        let read_bases_exceeded = input_bases > self.config.limits.max_read_bases;
        validate_read_content(read)?;
        let input_identity = read_identity(read)?;
        let mut work = CorrectionWork::default();
        if read_bases_exceeded {
            return self.cap_decision(
                read,
                input_identity,
                CapKind::ReadBases,
                input_bases,
                self.config.limits.max_read_bases,
                false,
                0,
                work,
            );
        }

        if self.config.mode == CorrectionMode::RawOnly {
            let projected_bytes = projected_raw_output_bytes(read.bases.len())?;
            if projected_bytes > self.config.limits.max_accounted_bytes_per_read {
                return self.cap_decision(
                    read,
                    input_identity,
                    CapKind::AccountedBytes,
                    projected_bytes,
                    self.config.limits.max_accounted_bytes_per_read,
                    false,
                    0,
                    work,
                );
            }
            return self.unchanged_decision(
                read,
                input_identity,
                UnchangedReason::RawOnly,
                None,
                false,
                0,
                0,
                0,
                work,
            );
        }

        // Baseline scoring is admitted independently of candidate search. A
        // trusted raw read must not be quarantined merely because its quality
        // values imply a large search neighborhood that is never needed.
        let windows_per_score = self.windows_per_score(read.bases.len())?;
        if windows_per_score > self.config.limits.max_window_evaluations_per_read {
            return self.cap_decision(
                read,
                input_identity,
                CapKind::WindowEvaluations,
                windows_per_score,
                self.config.limits.max_window_evaluations_per_read,
                false,
                0,
                work,
            );
        }
        let baseline_bytes = projected_baseline_bytes(read.bases.len(), self.spectra.len())?;
        if baseline_bytes > self.config.limits.max_accounted_bytes_per_read {
            return self.cap_decision(
                read,
                input_identity,
                CapKind::AccountedBytes,
                baseline_bytes,
                self.config.limits.max_accounted_bytes_per_read,
                false,
                0,
                work,
            );
        }
        work.projected_spectrum_comparisons =
            self.baseline_spectrum_comparison_bound(read.bases.len())?;
        if work.projected_spectrum_comparisons
            > self.config.limits.max_spectrum_comparisons_per_read
        {
            return self.cap_decision(
                read,
                input_identity,
                CapKind::SpectrumComparisons,
                work.projected_spectrum_comparisons,
                self.config.limits.max_spectrum_comparisons_per_read,
                false,
                0,
                work,
            );
        }
        let baseline = self.score(read.bases, read.bases, &[], &mut work)?;
        validate_work(work)?;
        if is_fully_trusted(&baseline) {
            return self.unchanged_decision(
                read,
                input_identity,
                UnchangedReason::AlreadyTrusted,
                Some(baseline),
                false,
                0,
                0,
                0,
                work,
            );
        }

        let editable_count = count_editable_positions(read, self.config.thresholds.max_edit_phred)?;
        if editable_count > u64::from(self.config.limits.max_editable_positions) {
            return self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::EditablePositions,
                editable_count,
                u64::from(self.config.limits.max_editable_positions),
                baseline,
                false,
                0,
                0,
                0,
                work,
            );
        }
        let projected_candidates = projected_candidate_count(
            read,
            self.config.thresholds.max_edit_phred,
            self.config.limits.max_substitutions,
        )?;
        if projected_candidates > self.config.limits.max_candidates_per_read {
            return self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::Candidates,
                projected_candidates,
                self.config.limits.max_candidates_per_read,
                baseline,
                true,
                projected_candidates,
                0,
                0,
                work,
            );
        }
        let score_passes = projected_candidates
            .checked_mul(2)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| overflow("candidate scoring pass count overflow"))?;
        let projected_window_evaluations = windows_per_score
            .checked_mul(score_passes)
            .ok_or_else(|| overflow("candidate window-evaluation projection overflow"))?;
        if projected_window_evaluations > self.config.limits.max_window_evaluations_per_read {
            return self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::WindowEvaluations,
                projected_window_evaluations,
                self.config.limits.max_window_evaluations_per_read,
                baseline,
                true,
                projected_candidates,
                0,
                0,
                work,
            );
        }
        let retained_candidate_summary_limit = projected_candidates.min(u64::from(
            self.config.limits.max_candidate_summaries_per_read,
        ));
        work.projected_spectrum_comparisons = projected_spectrum_comparisons(
            work.projected_spectrum_comparisons,
            projected_candidates,
        )?;
        if work.projected_spectrum_comparisons
            > self.config.limits.max_spectrum_comparisons_per_read
        {
            return self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::SpectrumComparisons,
                work.projected_spectrum_comparisons,
                self.config.limits.max_spectrum_comparisons_per_read,
                baseline,
                true,
                projected_candidates,
                0,
                0,
                work,
            );
        }
        work.projected_summary_comparisons =
            projected_summary_comparisons(projected_candidates, retained_candidate_summary_limit)?;
        if work.projected_summary_comparisons > self.config.limits.max_summary_comparisons_per_read
        {
            return self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::SummaryComparisons,
                work.projected_summary_comparisons,
                self.config.limits.max_summary_comparisons_per_read,
                baseline,
                true,
                projected_candidates,
                0,
                0,
                work,
            );
        }
        work.projected_search_work_units = projected_search_work_units(
            read.bases.len(),
            self.spectra.len(),
            self.config.limits.max_substitutions,
            projected_candidates,
        )?;
        if work.projected_search_work_units > self.config.limits.max_search_work_units_per_read {
            return self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::SearchWorkUnits,
                work.projected_search_work_units,
                self.config.limits.max_search_work_units_per_read,
                baseline,
                true,
                projected_candidates,
                0,
                0,
                work,
            );
        }
        let projected_bytes = projected_read_scratch_bytes(
            read.bases.len(),
            usize::try_from(editable_count)
                .map_err(|_| memory_error("editable-position count does not fit usize".into()))?,
            self.spectra.len(),
            self.config.limits.max_substitutions,
            retained_candidate_summary_limit,
        )?;
        if projected_bytes > self.config.limits.max_accounted_bytes_per_read {
            return self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::AccountedBytes,
                projected_bytes,
                self.config.limits.max_accounted_bytes_per_read,
                baseline,
                true,
                projected_candidates,
                0,
                0,
                work,
            );
        }

        let editable = editable_positions(read, self.config.thresholds.max_edit_phred)?;
        if projected_candidates == 0 {
            return self.resolve_unqualified(
                read,
                input_identity,
                UnchangedReason::NoEditableBases,
                baseline,
                true,
                projected_candidates,
                0,
                0,
                work,
            );
        }

        let mut working = fallible_copy(read.bases, "correction working sequence")?;
        let mut changes = Vec::new();
        changes
            .try_reserve_exact(usize::from(self.config.limits.max_substitutions))
            .map_err(|cause| memory_error(format!("cannot reserve correction changes: {cause}")))?;
        let mut search_recorder = CandidateSearchRecorder::new(
            usize::try_from(retained_candidate_summary_limit)
                .map_err(|_| memory_error("candidate-summary limit does not fit usize".into()))?,
        )?;
        let mut best: Option<RankedCandidate> = None;
        let mut low_count_conflict: Option<(u64, u64)> = None;
        let mut context_conflict = false;
        let mut first_pass_evaluations = 0_u64;
        enumerate_candidates(
            &mut working,
            &editable,
            self.config.limits.max_substitutions,
            &mut changes,
            &mut |candidate, candidate_changes| {
                first_pass_evaluations = checked_add(
                    first_pass_evaluations,
                    1,
                    "candidate score-evaluation count overflow",
                )?;
                let evidence = with_supporting_layers(
                    &baseline,
                    self.score(read.bases, candidate, candidate_changes, &mut work)?,
                )?;
                let threshold_qualified = qualifies(&baseline, &evidence, self.config.thresholds)?;
                search_recorder.record(
                    candidate,
                    candidate_changes,
                    &evidence,
                    threshold_qualified,
                )?;
                if !threshold_qualified {
                    return Ok(());
                }
                if self.config.mode == CorrectionMode::DiversityPreserving
                    && evidence.affected_low_count_windows > 0
                {
                    let observed = (
                        evidence.affected_low_count_windows,
                        evidence.affected_low_count_support_sum,
                    );
                    if low_count_conflict.is_none_or(|current| observed > current) {
                        low_count_conflict = Some(observed);
                    }
                    return Ok(());
                }
                if self.config.mode == CorrectionMode::DiversityPreserving {
                    // Exact k-mer occurrence spectra cannot establish that an
                    // end-to-end read supporting this substitution or IUPAC
                    // resolution exists. Both edit kinds therefore abstain in
                    // the default mode until an independent context channel is
                    // versioned into this engine.
                    context_conflict = true;
                    return Ok(());
                }
                retain_ranked_candidate(&mut best, candidate, candidate_changes, evidence)?;
                Ok(())
            },
        )?;
        verify_enumeration_count(first_pass_evaluations, projected_candidates)?;
        let candidate_search_evidence = search_recorder.finish()?;
        candidate_search_evidence.validate(self.config.limits.max_candidate_summaries_per_read)?;
        if candidate_search_evidence.retained_alternative_comparisons
            > work.projected_summary_comparisons
        {
            return Err(invariant_error(
                "candidate-summary comparisons exceeded their checked projection",
            ));
        }
        if candidate_search_evidence.evaluated_candidates != first_pass_evaluations {
            return Err(invariant_error(
                "candidate-search evidence does not cover every evaluated candidate",
            ));
        }
        if let Some((affected_windows, affected_support_sum)) = low_count_conflict {
            let mut decision = self.quarantine_decision(
                read,
                input_identity,
                QuarantineReason::DiversityLowCountConflict {
                    affected_windows,
                    affected_support_sum,
                },
                Some(baseline),
                None,
                true,
                projected_candidates,
                projected_candidates,
                first_pass_evaluations,
                work,
            )?;
            attach_candidate_search_evidence(&mut decision, candidate_search_evidence)?;
            return Ok(decision);
        }
        if context_conflict {
            let mut decision = self.quarantine_decision(
                read,
                input_identity,
                QuarantineReason::IndependentContextUnavailable,
                Some(baseline),
                None,
                true,
                projected_candidates,
                projected_candidates,
                first_pass_evaluations,
                work,
            )?;
            attach_candidate_search_evidence(&mut decision, candidate_search_evidence)?;
            return Ok(decision);
        }
        let Some(best) = best else {
            let mut decision = self.resolve_unqualified(
                read,
                input_identity,
                UnchangedReason::EvidenceBelowThresholds,
                baseline,
                true,
                projected_candidates,
                projected_candidates,
                first_pass_evaluations,
                work,
            )?;
            attach_candidate_search_evidence(&mut decision, candidate_search_evidence)?;
            return Ok(decision);
        };

        let mut dominance_conflict = false;
        let mut second_pass_evaluations = 0_u64;
        working.copy_from_slice(read.bases);
        changes.clear();
        enumerate_candidates(
            &mut working,
            &editable,
            self.config.limits.max_substitutions,
            &mut changes,
            &mut |candidate, candidate_changes| {
                second_pass_evaluations = checked_add(
                    second_pass_evaluations,
                    1,
                    "candidate score-evaluation count overflow",
                )?;
                let evidence = with_supporting_layers(
                    &baseline,
                    self.score(read.bases, candidate, candidate_changes, &mut work)?,
                )?;
                if qualifies(&baseline, &evidence, self.config.thresholds)?
                    && candidate != best.bases.as_slice()
                    && !strictly_dominates(&best.evidence, &evidence)
                {
                    dominance_conflict = true;
                }
                Ok(())
            },
        )?;
        verify_enumeration_count(second_pass_evaluations, projected_candidates)?;
        let evaluations = checked_add(
            first_pass_evaluations,
            second_pass_evaluations,
            "candidate score-evaluation count overflow",
        )?;
        if dominance_conflict {
            let mut decision = self.quarantine_decision(
                read,
                input_identity,
                QuarantineReason::ConflictingCandidates,
                Some(baseline),
                None,
                true,
                projected_candidates,
                projected_candidates,
                evaluations,
                work,
            )?;
            attach_candidate_search_evidence(&mut decision, candidate_search_evidence)?;
            return Ok(decision);
        }
        let journal_entries = as_u64(best.changes.len(), "correction journal entry count")?;
        if journal_entries > self.config.limits.max_journal_entries_per_read {
            let mut decision = self.cap_decision_with_baseline(
                read,
                input_identity,
                CapKind::JournalEntries,
                journal_entries,
                self.config.limits.max_journal_entries_per_read,
                baseline,
                true,
                projected_candidates,
                projected_candidates,
                evaluations,
                work,
            )?;
            attach_candidate_search_evidence(&mut decision, candidate_search_evidence)?;
            return Ok(decision);
        }
        let mut decision = self.corrected_decision(
            read,
            input_identity,
            baseline,
            best,
            true,
            projected_candidates,
            projected_candidates,
            evaluations,
            work,
        )?;
        attach_candidate_search_evidence(&mut decision, candidate_search_evidence)?;
        validate_work(work)?;
        Ok(decision)
    }

    /// Recompute and verify the complete decision before replaying its
    /// transformation. This binds the decision to this engine's algorithm,
    /// full configuration, raw spectrum, caller-supplied source descriptor,
    /// and immutable read identity.
    pub fn verify_decision_and_replay(
        &self,
        read: CorrectionRead<'_>,
        decision: &CorrectionDecision,
    ) -> Result<Option<Vec<u8>>> {
        if decision.journal.algorithm_identity != self.algorithm_identity
            || decision.journal.config_identity != self.config_identity
            || decision.journal.spectrum_identity != self.spectrum_identity
            || decision.journal.source_descriptor_identity != self.source_descriptor_identity
        {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityArtifact,
                "quality-correction decision identities do not match this engine",
            ));
        }
        let expected = self.correct_read(read)?;
        if &expected != decision {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityArtifact,
                "quality-correction decision differs from deterministic recomputation",
            ));
        }
        decision.journal.replay_transformation(read)
    }

    /// Correct independent reads under a private Rayon pool. Results are
    /// sorted by ordinal and are byte-identical across worker counts.
    pub fn correct_batch(
        &self,
        reads: &[CorrectionRead<'_>],
        worker_threads: u16,
    ) -> Result<CorrectionBatch> {
        validate_workers(worker_threads, self.config.limits.max_worker_threads)?;
        let read_count = as_u64(reads.len(), "batch read count")?;
        enforce_global_limit(
            read_count,
            self.config.limits.max_batch_reads,
            ErrorCode::ResourceRetainedKeys,
            "experimental correction batch reads",
        )?;
        let mut input_bases = 0_u64;
        for read in reads {
            input_bases = checked_add(
                input_bases,
                as_u64(read.bases.len(), "batch read length")?,
                "batch input-base count overflow",
            )?;
        }
        enforce_global_limit(
            input_bases,
            self.config.limits.max_batch_bases,
            ErrorCode::ResourceRetainedKeys,
            "experimental correction batch bases",
        )?;
        let projected =
            projected_batch_bytes(reads, self.spectra.len(), self.config, worker_threads)?;
        enforce_global_limit(
            projected,
            self.config.limits.max_batch_accounted_bytes,
            ErrorCode::ResourceMemory,
            "experimental correction batch accounted bytes",
        )?;
        // No O(read-count) allocation occurs before the complete requested
        // payload projection has been admitted.
        let mut order = Vec::new();
        order.try_reserve_exact(reads.len()).map_err(|cause| {
            memory_error(format!("cannot reserve correction read order: {cause}"))
        })?;
        order.extend(0..reads.len());
        order.sort_unstable_by_key(|&index| reads[index].read_ordinal);
        for pair in order.windows(2) {
            if reads[pair[0]].read_ordinal == reads[pair[1]].read_ordinal {
                return Err(VeritasmError::new(
                    ErrorCode::IntegrityOrdinalCoverage,
                    format!(
                        "duplicate correction read ordinal {}",
                        reads[pair[0]].read_ordinal
                    ),
                ));
            }
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(usize::from(worker_threads))
            .build()
            .map_err(|cause| {
                memory_error(format!("cannot build correction worker pool: {cause}"))
            })?;
        let mut slots = Vec::new();
        slots.try_reserve_exact(reads.len()).map_err(|cause| {
            memory_error(format!("cannot reserve correction result slots: {cause}"))
        })?;
        slots.resize_with(reads.len(), OnceLock::new);
        pool.install(|| {
            slots
                .par_iter()
                .zip(order.par_iter())
                .for_each(|(slot, &index)| {
                    let _already_initialized = slot.set(self.correct_read(reads[index]));
                });
        });
        let mut decisions = Vec::new();
        decisions.try_reserve_exact(reads.len()).map_err(|cause| {
            memory_error(format!(
                "cannot reserve ordered correction results: {cause}"
            ))
        })?;
        for slot in slots {
            let result = slot.into_inner().ok_or_else(|| {
                invariant_error("correction worker left an uninitialized result slot")
            })?;
            decisions.push(result?);
        }
        let mut accounting = CorrectionAccounting::default();
        for decision in &decisions {
            accounting.checked_accumulate(decision.accounting)?;
        }
        accounting.validate()?;
        Ok(CorrectionBatch {
            decisions,
            accounting,
        })
    }

    fn windows_per_score(&self, read_length: usize) -> Result<u64> {
        let mut total = 0_u64;
        for layer in &self.spectra {
            let windows = read_length
                .checked_sub(usize::from(layer.spectrum.k))
                .and_then(|value| value.checked_add(1))
                .unwrap_or(0);
            total = checked_add(
                total,
                as_u64(windows, "per-score window count")?,
                "per-score window count overflow",
            )?;
        }
        Ok(total)
    }

    fn baseline_spectrum_comparison_bound(&self, read_length: usize) -> Result<u64> {
        let mut total = 0_u64;
        for layer in &self.spectra {
            let windows = read_length
                .checked_sub(usize::from(layer.spectrum.k))
                .and_then(|value| value.checked_add(1))
                .unwrap_or(0);
            let comparisons = binary_search_comparison_bound(layer.spectrum.entries.len())?;
            total = checked_add(
                total,
                checked_mul(
                    as_u64(windows, "spectrum-comparison window count")?,
                    comparisons,
                    "baseline spectrum-comparison projection overflow",
                )?,
                "baseline spectrum-comparison projection overflow",
            )?;
        }
        Ok(total)
    }

    fn score(
        &self,
        baseline_bases: &[u8],
        bases: &[u8],
        changes: &[BaseSubstitution],
        work: &mut CorrectionWork,
    ) -> Result<CandidateEvidence> {
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(self.spectra.len())
            .map_err(|cause| {
                memory_error(format!("cannot reserve candidate layer evidence: {cause}"))
            })?;
        let mut trusted_windows = 0_u64;
        let mut unsupported_windows = 0_u64;
        let mut summed_occurrence_support = 0_u64;
        let mut affected_low_count_windows = 0_u64;
        let mut affected_low_count_support_sum = 0_u64;
        let mut affected_candidate_low_count_windows = 0_u64;
        let mut affected_candidate_low_count_support_sum = 0_u64;
        for prepared in &self.spectra {
            let layer = score_layer(prepared.spectrum, baseline_bases, bases, changes, work)?;
            trusted_windows = checked_add(
                trusted_windows,
                layer.trusted_windows,
                "candidate trusted-window total overflow",
            )?;
            unsupported_windows = checked_add(
                unsupported_windows,
                layer.unsupported_windows,
                "candidate unsupported-window total overflow",
            )?;
            summed_occurrence_support = checked_add(
                summed_occurrence_support,
                layer.summed_occurrence_support,
                "candidate summed-occurrence-support total overflow",
            )?;
            affected_low_count_windows = checked_add(
                affected_low_count_windows,
                layer.affected_low_count_windows,
                "candidate affected-low-count total overflow",
            )?;
            affected_low_count_support_sum = checked_add(
                affected_low_count_support_sum,
                layer.affected_low_count_support_sum,
                "candidate affected-low-count support overflow",
            )?;
            affected_candidate_low_count_windows = checked_add(
                affected_candidate_low_count_windows,
                layer.affected_candidate_low_count_windows,
                "candidate-spelling affected-low-count total overflow",
            )?;
            affected_candidate_low_count_support_sum = checked_add(
                affected_candidate_low_count_support_sum,
                layer.affected_candidate_low_count_support_sum,
                "candidate-spelling affected-low-count support overflow",
            )?;
            layers.push(layer);
        }
        let mut quality_penalty = 0_u64;
        for change in changes {
            quality_penalty = checked_add(
                quality_penalty,
                u64::from(change.phred),
                "candidate quality-penalty overflow",
            )?;
        }
        Ok(CandidateEvidence {
            edit_count: u8::try_from(changes.len())
                .map_err(|_| overflow("candidate edit count does not fit u8"))?,
            quality_penalty,
            supporting_k_layers: 0,
            trusted_windows,
            unsupported_windows,
            summed_occurrence_support,
            affected_low_count_windows,
            affected_low_count_support_sum,
            affected_candidate_low_count_windows,
            affected_candidate_low_count_support_sum,
            layers,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn cap_decision(
        &self,
        read: CorrectionRead<'_>,
        input_identity: [u8; 32],
        kind: CapKind,
        projected: u64,
        limit: u64,
        candidate_projection_performed: bool,
        projected_candidates: u64,
        work: CorrectionWork,
    ) -> Result<CorrectionDecision> {
        self.quarantine_decision(
            read,
            input_identity,
            QuarantineReason::CapExceeded {
                kind,
                projected,
                limit,
            },
            None,
            None,
            candidate_projection_performed,
            projected_candidates,
            0,
            0,
            work,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn cap_decision_with_baseline(
        &self,
        read: CorrectionRead<'_>,
        input_identity: [u8; 32],
        kind: CapKind,
        projected: u64,
        limit: u64,
        baseline: CandidateEvidence,
        candidate_projection_performed: bool,
        projected_candidates: u64,
        evaluated_candidates: u64,
        candidate_score_evaluations: u64,
        work: CorrectionWork,
    ) -> Result<CorrectionDecision> {
        self.quarantine_decision(
            read,
            input_identity,
            QuarantineReason::CapExceeded {
                kind,
                projected,
                limit,
            },
            Some(baseline),
            None,
            candidate_projection_performed,
            projected_candidates,
            evaluated_candidates,
            candidate_score_evaluations,
            work,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn quarantine_decision(
        &self,
        read: CorrectionRead<'_>,
        input_identity: [u8; 32],
        reason: QuarantineReason,
        baseline_evidence: Option<CandidateEvidence>,
        accepted_evidence: Option<CandidateEvidence>,
        candidate_projection_performed: bool,
        projected_candidates: u64,
        evaluated_candidates: u64,
        candidate_score_evaluations: u64,
        work: CorrectionWork,
    ) -> Result<CorrectionDecision> {
        let input_bases = as_u64(read.bases.len(), "quarantined read length")?;
        let accounting = CorrectionAccounting {
            reads: 1,
            input_bases,
            quarantined_bases: input_bases,
            quarantined_reads: 1,
            evidence_conflicts: u64::from(matches!(
                reason,
                QuarantineReason::ConflictingCandidates
            )),
            cap_quarantines: u64::from(matches!(reason, QuarantineReason::CapExceeded { .. })),
            diversity_low_count_quarantines: u64::from(matches!(
                reason,
                QuarantineReason::DiversityLowCountConflict { .. }
            )),
            independent_context_quarantines: u64::from(matches!(
                reason,
                QuarantineReason::IndependentContextUnavailable
            )),
            reads_with_candidate_projection: u64::from(candidate_projection_performed),
            projected_candidates,
            evaluated_candidates,
            candidate_score_evaluations,
            window_evaluations: work.window_evaluations,
            projected_spectrum_comparisons: work.projected_spectrum_comparisons,
            spectrum_comparisons: work.spectrum_comparisons,
            projected_summary_comparisons: work.projected_summary_comparisons,
            projected_search_work_units: work.projected_search_work_units,
            ..CorrectionAccounting::default()
        };
        accounting.validate()?;
        Ok(CorrectionDecision {
            read_ordinal: read.read_ordinal,
            emitted_bases: None,
            journal: CorrectionJournal {
                read_ordinal: read.read_ordinal,
                input_identity,
                spectrum_identity: self.spectrum_identity,
                source_descriptor_identity: self.source_descriptor_identity,
                config_identity: self.config_identity,
                algorithm_identity: self.algorithm_identity,
                thresholds: self.config.thresholds,
                disposition: ReadDisposition::Quarantined(reason),
                substitutions: Vec::new(),
                baseline_evidence,
                accepted_evidence,
                candidate_search_evidence: CandidateSearchEvidence::default(),
                candidate_projection_performed,
                projected_candidates,
                evaluated_candidates,
                candidate_score_evaluations,
                window_evaluations: work.window_evaluations,
                projected_spectrum_comparisons: work.projected_spectrum_comparisons,
                spectrum_comparisons: work.spectrum_comparisons,
                projected_summary_comparisons: work.projected_summary_comparisons,
                projected_search_work_units: work.projected_search_work_units,
            },
            accounting,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn unchanged_decision(
        &self,
        read: CorrectionRead<'_>,
        input_identity: [u8; 32],
        reason: UnchangedReason,
        baseline_evidence: Option<CandidateEvidence>,
        candidate_projection_performed: bool,
        projected_candidates: u64,
        evaluated_candidates: u64,
        candidate_score_evaluations: u64,
        work: CorrectionWork,
    ) -> Result<CorrectionDecision> {
        let bases = fallible_copy(read.bases, "unchanged read output")?;
        let input_bases = as_u64(bases.len(), "unchanged read length")?;
        let accounting = CorrectionAccounting {
            reads: 1,
            input_bases,
            emitted_bases: input_bases,
            unchanged_reads: 1,
            reads_with_candidate_projection: u64::from(candidate_projection_performed),
            projected_candidates,
            evaluated_candidates,
            candidate_score_evaluations,
            window_evaluations: work.window_evaluations,
            projected_spectrum_comparisons: work.projected_spectrum_comparisons,
            spectrum_comparisons: work.spectrum_comparisons,
            projected_summary_comparisons: work.projected_summary_comparisons,
            projected_search_work_units: work.projected_search_work_units,
            ..CorrectionAccounting::default()
        };
        accounting.validate()?;
        Ok(CorrectionDecision {
            read_ordinal: read.read_ordinal,
            emitted_bases: Some(bases),
            journal: CorrectionJournal {
                read_ordinal: read.read_ordinal,
                input_identity,
                spectrum_identity: self.spectrum_identity,
                source_descriptor_identity: self.source_descriptor_identity,
                config_identity: self.config_identity,
                algorithm_identity: self.algorithm_identity,
                thresholds: self.config.thresholds,
                disposition: ReadDisposition::Unchanged(reason),
                substitutions: Vec::new(),
                baseline_evidence,
                accepted_evidence: None,
                candidate_search_evidence: CandidateSearchEvidence::default(),
                candidate_projection_performed,
                projected_candidates,
                evaluated_candidates,
                candidate_score_evaluations,
                window_evaluations: work.window_evaluations,
                projected_spectrum_comparisons: work.projected_spectrum_comparisons,
                spectrum_comparisons: work.spectrum_comparisons,
                projected_summary_comparisons: work.projected_summary_comparisons,
                projected_search_work_units: work.projected_search_work_units,
            },
            accounting,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn corrected_decision(
        &self,
        read: CorrectionRead<'_>,
        input_identity: [u8; 32],
        baseline_evidence: CandidateEvidence,
        best: RankedCandidate,
        candidate_projection_performed: bool,
        projected_candidates: u64,
        evaluated_candidates: u64,
        candidate_score_evaluations: u64,
        work: CorrectionWork,
    ) -> Result<CorrectionDecision> {
        let input_bases = as_u64(read.bases.len(), "corrected read length")?;
        let substitutions = as_u64(best.changes.len(), "accepted substitution count")?;
        let accounting = CorrectionAccounting {
            reads: 1,
            input_bases,
            emitted_bases: input_bases,
            corrected_reads: 1,
            substitutions,
            reads_with_candidate_projection: u64::from(candidate_projection_performed),
            projected_candidates,
            evaluated_candidates,
            consensus_low_count_windows: if self.config.mode
                == CorrectionMode::ConsensusExperimental
            {
                best.evidence.affected_low_count_windows
            } else {
                0
            },
            consensus_low_count_support_sum: if self.config.mode
                == CorrectionMode::ConsensusExperimental
            {
                best.evidence.affected_low_count_support_sum
            } else {
                0
            },
            candidate_score_evaluations,
            window_evaluations: work.window_evaluations,
            projected_spectrum_comparisons: work.projected_spectrum_comparisons,
            spectrum_comparisons: work.spectrum_comparisons,
            projected_summary_comparisons: work.projected_summary_comparisons,
            projected_search_work_units: work.projected_search_work_units,
            ..CorrectionAccounting::default()
        };
        accounting.validate()?;
        Ok(CorrectionDecision {
            read_ordinal: read.read_ordinal,
            emitted_bases: Some(best.bases),
            journal: CorrectionJournal {
                read_ordinal: read.read_ordinal,
                input_identity,
                spectrum_identity: self.spectrum_identity,
                source_descriptor_identity: self.source_descriptor_identity,
                config_identity: self.config_identity,
                algorithm_identity: self.algorithm_identity,
                thresholds: self.config.thresholds,
                disposition: ReadDisposition::Corrected,
                substitutions: best.changes,
                baseline_evidence: Some(baseline_evidence),
                accepted_evidence: Some(best.evidence),
                candidate_search_evidence: CandidateSearchEvidence::default(),
                candidate_projection_performed,
                projected_candidates,
                evaluated_candidates,
                candidate_score_evaluations,
                window_evaluations: work.window_evaluations,
                projected_spectrum_comparisons: work.projected_spectrum_comparisons,
                spectrum_comparisons: work.spectrum_comparisons,
                projected_summary_comparisons: work.projected_summary_comparisons,
                projected_search_work_units: work.projected_search_work_units,
            },
            accounting,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_unqualified(
        &self,
        read: CorrectionRead<'_>,
        input_identity: [u8; 32],
        unchanged_reason: UnchangedReason,
        baseline: CandidateEvidence,
        candidate_projection_performed: bool,
        projected_candidates: u64,
        evaluated_candidates: u64,
        candidate_score_evaluations: u64,
        work: CorrectionWork,
    ) -> Result<CorrectionDecision> {
        match self.config.unresolved_policy {
            UnresolvedPolicy::LeaveUnchanged => self.unchanged_decision(
                read,
                input_identity,
                unchanged_reason,
                Some(baseline),
                candidate_projection_performed,
                projected_candidates,
                evaluated_candidates,
                candidate_score_evaluations,
                work,
            ),
            UnresolvedPolicy::Quarantine => self.quarantine_decision(
                read,
                input_identity,
                QuarantineReason::UnresolvedEvidence,
                Some(baseline),
                None,
                candidate_projection_performed,
                projected_candidates,
                evaluated_candidates,
                candidate_score_evaluations,
                work,
            ),
            UnresolvedPolicy::TrimLowQualityTerminals {
                max_terminal_phred,
                min_retained_bases,
            } => {
                if let Some(retained) = terminal_trim_range(
                    read,
                    max_terminal_phred,
                    usize::try_from(min_retained_bases).map_err(|_| {
                        memory_error("minimum retained bases does not fit usize".into())
                    })?,
                )? {
                    self.trimmed_decision(
                        read,
                        input_identity,
                        baseline,
                        retained,
                        candidate_projection_performed,
                        projected_candidates,
                        evaluated_candidates,
                        candidate_score_evaluations,
                        work,
                    )
                } else {
                    self.unchanged_decision(
                        read,
                        input_identity,
                        UnchangedReason::TerminalTrimCriteriaNotMet,
                        Some(baseline),
                        candidate_projection_performed,
                        projected_candidates,
                        evaluated_candidates,
                        candidate_score_evaluations,
                        work,
                    )
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn trimmed_decision(
        &self,
        read: CorrectionRead<'_>,
        input_identity: [u8; 32],
        baseline_evidence: CandidateEvidence,
        retained: RetainedRange,
        candidate_projection_performed: bool,
        projected_candidates: u64,
        evaluated_candidates: u64,
        candidate_score_evaluations: u64,
        work: CorrectionWork,
    ) -> Result<CorrectionDecision> {
        let start = usize::try_from(retained.start)
            .map_err(|_| memory_error("trim start does not fit usize".into()))?;
        let end = usize::try_from(retained.end)
            .map_err(|_| memory_error("trim end does not fit usize".into()))?;
        let emitted = fallible_copy(&read.bases[start..end], "trimmed read output")?;
        let input_bases = as_u64(read.bases.len(), "trimmed input length")?;
        let emitted_bases = as_u64(emitted.len(), "trimmed output length")?;
        let trimmed_bases = input_bases
            .checked_sub(emitted_bases)
            .ok_or_else(|| invariant_error("trimmed output exceeds input length"))?;
        let accounting = CorrectionAccounting {
            reads: 1,
            input_bases,
            emitted_bases,
            trimmed_bases,
            trimmed_reads: 1,
            reads_with_candidate_projection: u64::from(candidate_projection_performed),
            projected_candidates,
            evaluated_candidates,
            candidate_score_evaluations,
            window_evaluations: work.window_evaluations,
            projected_spectrum_comparisons: work.projected_spectrum_comparisons,
            spectrum_comparisons: work.spectrum_comparisons,
            projected_summary_comparisons: work.projected_summary_comparisons,
            projected_search_work_units: work.projected_search_work_units,
            ..CorrectionAccounting::default()
        };
        accounting.validate()?;
        Ok(CorrectionDecision {
            read_ordinal: read.read_ordinal,
            emitted_bases: Some(emitted),
            journal: CorrectionJournal {
                read_ordinal: read.read_ordinal,
                input_identity,
                spectrum_identity: self.spectrum_identity,
                source_descriptor_identity: self.source_descriptor_identity,
                config_identity: self.config_identity,
                algorithm_identity: self.algorithm_identity,
                thresholds: self.config.thresholds,
                disposition: ReadDisposition::Trimmed {
                    retained,
                    reason: TrimReason::UnresolvedLowQualityOrAmbiguousTermini,
                },
                substitutions: Vec::new(),
                baseline_evidence: Some(baseline_evidence),
                accepted_evidence: None,
                candidate_search_evidence: CandidateSearchEvidence::default(),
                candidate_projection_performed,
                projected_candidates,
                evaluated_candidates,
                candidate_score_evaluations,
                window_evaluations: work.window_evaluations,
                projected_spectrum_comparisons: work.projected_spectrum_comparisons,
                spectrum_comparisons: work.spectrum_comparisons,
                projected_summary_comparisons: work.projected_summary_comparisons,
                projected_search_work_units: work.projected_search_work_units,
            },
            accounting,
        })
    }
}

impl CorrectionJournal {
    /// Apply only the recorded source transformation after structural checks.
    ///
    /// This does not verify spectrum evidence or prove that the decision was
    /// produced by the configured engine. Use
    /// [`QualityCorrectionEngine::verify_decision_and_replay`] for that.
    pub fn replay_transformation(&self, read: CorrectionRead<'_>) -> Result<Option<Vec<u8>>> {
        validate_read(read)?;
        if read.read_ordinal != self.read_ordinal || read_identity(read)? != self.input_identity {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityArtifact,
                "quality-correction journal does not match the supplied immutable read",
            ));
        }
        match self.disposition {
            ReadDisposition::Quarantined(_) => {
                if !self.substitutions.is_empty() {
                    return Err(invariant_error(
                        "quarantined correction journal contains substitutions",
                    ));
                }
                Ok(None)
            }
            ReadDisposition::Unchanged(_) => {
                if !self.substitutions.is_empty() {
                    return Err(invariant_error(
                        "unchanged correction journal contains substitutions",
                    ));
                }
                Ok(Some(fallible_copy(read.bases, "journal replay output")?))
            }
            ReadDisposition::Trimmed { retained, .. } => {
                if !self.substitutions.is_empty() {
                    return Err(invariant_error(
                        "trimmed correction journal contains substitutions",
                    ));
                }
                let start = usize::try_from(retained.start)
                    .map_err(|_| invariant_error("journal trim start does not fit usize"))?;
                let end = usize::try_from(retained.end)
                    .map_err(|_| invariant_error("journal trim end does not fit usize"))?;
                let range = read.bases.get(start..end).ok_or_else(|| {
                    invariant_error("journal retained range lies outside immutable read")
                })?;
                Ok(Some(fallible_copy(range, "journal trim replay output")?))
            }
            ReadDisposition::Corrected => {
                if self.accepted_evidence.is_none() || self.substitutions.is_empty() {
                    return Err(invariant_error(
                        "corrected journal lacks accepted evidence or substitutions",
                    ));
                }
                let mut output = fallible_copy(read.bases, "journal correction replay output")?;
                let mut previous = None;
                for change in &self.substitutions {
                    let position = usize::try_from(change.position).map_err(|_| {
                        invariant_error("journal substitution position does not fit usize")
                    })?;
                    if previous.is_some_and(|value| position <= value) {
                        return Err(invariant_error(
                            "journal substitutions are not strictly position ordered",
                        ));
                    }
                    previous = Some(position);
                    let observed = output.get_mut(position).ok_or_else(|| {
                        invariant_error("journal substitution lies outside immutable read")
                    })?;
                    let source_phred = read.phred.get(position).copied().ok_or_else(|| {
                        invariant_error("journal substitution quality lies outside immutable read")
                    })?;
                    if *observed != change.before || source_phred != change.phred {
                        return Err(VeritasmError::new(
                            ErrorCode::IntegrityArtifact,
                            "journal substitution precondition differs from immutable read",
                        ));
                    }
                    let (choices, choice_count, expected_kind) = edit_choices(change.before)?;
                    if change.kind != expected_kind
                        || !choices[..usize::from(choice_count)].contains(&change.after)
                    {
                        return Err(invariant_error(
                            "journal substitution has an invalid replacement",
                        ));
                    }
                    *observed = change.after;
                }
                Ok(Some(output))
            }
        }
    }
}

impl CorrectionAccounting {
    fn checked_accumulate(&mut self, other: Self) -> Result<()> {
        macro_rules! add_field {
            ($field:ident) => {
                self.$field = checked_add(
                    self.$field,
                    other.$field,
                    concat!("correction accounting overflow: ", stringify!($field)),
                )?;
            };
        }
        add_field!(reads);
        add_field!(input_bases);
        add_field!(emitted_bases);
        add_field!(trimmed_bases);
        add_field!(quarantined_bases);
        add_field!(unchanged_reads);
        add_field!(corrected_reads);
        add_field!(trimmed_reads);
        add_field!(quarantined_reads);
        add_field!(substitutions);
        add_field!(evidence_conflicts);
        add_field!(cap_quarantines);
        add_field!(diversity_low_count_quarantines);
        add_field!(independent_context_quarantines);
        add_field!(reads_with_candidate_projection);
        add_field!(projected_candidates);
        add_field!(evaluated_candidates);
        add_field!(candidate_score_evaluations);
        add_field!(window_evaluations);
        add_field!(projected_spectrum_comparisons);
        add_field!(spectrum_comparisons);
        add_field!(projected_summary_comparisons);
        add_field!(summary_comparisons);
        add_field!(projected_search_work_units);
        add_field!(consensus_low_count_windows);
        add_field!(consensus_low_count_support_sum);
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let classified_reads = self
            .unchanged_reads
            .checked_add(self.corrected_reads)
            .and_then(|value| value.checked_add(self.trimmed_reads))
            .and_then(|value| value.checked_add(self.quarantined_reads))
            .ok_or_else(|| overflow("correction classified-read sum overflow"))?;
        let classified_bases = self
            .emitted_bases
            .checked_add(self.trimmed_bases)
            .and_then(|value| value.checked_add(self.quarantined_bases))
            .ok_or_else(|| overflow("correction classified-base sum overflow"))?;
        if classified_reads != self.reads || classified_bases != self.input_bases {
            return Err(invariant_error(
                "correction accounting does not conserve reads or bases",
            ));
        }
        let classified_quarantines = self
            .evidence_conflicts
            .checked_add(self.cap_quarantines)
            .and_then(|value| value.checked_add(self.diversity_low_count_quarantines))
            .and_then(|value| value.checked_add(self.independent_context_quarantines))
            .ok_or_else(|| overflow("correction quarantine subclass sum overflow"))?;
        if classified_quarantines > self.quarantined_reads
            || self.reads_with_candidate_projection > self.reads
            || self.evaluated_candidates > self.projected_candidates
            || self.spectrum_comparisons > self.projected_spectrum_comparisons
            || self.summary_comparisons > self.projected_summary_comparisons
            || (self.consensus_low_count_windows > 0 && self.corrected_reads == 0)
        {
            return Err(invariant_error(
                "correction accounting subclass or search counts are inconsistent",
            ));
        }
        Ok(())
    }
}

fn validate_work(work: CorrectionWork) -> Result<()> {
    if work.spectrum_comparisons > work.projected_spectrum_comparisons {
        return Err(invariant_error(
            "spectrum comparisons exceeded their checked projection",
        ));
    }
    Ok(())
}

fn attach_candidate_search_evidence(
    decision: &mut CorrectionDecision,
    evidence: CandidateSearchEvidence,
) -> Result<()> {
    if evidence.retained_alternative_comparisons > decision.journal.projected_summary_comparisons {
        return Err(invariant_error(
            "candidate-summary comparisons exceeded their checked projection",
        ));
    }
    decision.accounting.summary_comparisons = evidence.retained_alternative_comparisons;
    decision.journal.candidate_search_evidence = evidence;
    decision.accounting.validate()
}

fn validate_config(config: CorrectionConfig, spectrum_count: usize) -> Result<()> {
    if config.thresholds.max_edit_phred > 93
        || config.thresholds.min_trusted_window_gain == 0
        || config.thresholds.min_summed_occurrence_support_gain == 0
        || config.thresholds.min_supporting_k_layers == 0
    {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "correction thresholds require Q0..Q93 and nonzero evidence gains/layer count",
        ));
    }
    if config.limits.max_substitutions == 0 || config.limits.max_substitutions > 3 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "experimental correction max substitutions must be in 1..=3",
        ));
    }
    let spectrum_count_u64 = as_u64(spectrum_count, "correction spectrum count")?;
    if spectrum_count == 0
        || spectrum_count_u64 > u64::from(config.limits.max_spectra)
        || u64::from(config.thresholds.min_supporting_k_layers) > spectrum_count_u64
        || config.limits.max_read_bases == 0
        || config.limits.max_spectrum_entries_total == 0
        || config.limits.max_spectrum_comparisons_per_read == 0
        || config.limits.max_editable_positions == 0
        || config.limits.max_candidates_per_read == 0
        || config.limits.max_window_evaluations_per_read == 0
        || config.limits.max_summary_comparisons_per_read == 0
        || config.limits.max_search_work_units_per_read == 0
        || config.limits.max_journal_entries_per_read == 0
        || config.limits.max_candidate_summaries_per_read == 0
        || config.limits.max_accounted_bytes_per_read == 0
        || config.limits.max_batch_reads == 0
        || config.limits.max_batch_bases == 0
        || config.limits.max_batch_accounted_bytes == 0
        || config.limits.max_worker_threads == 0
        || config.limits.max_read_bases > u64::from(u32::MAX)
    {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "correction spectra and all work/memory limits must be nonzero and internally compatible",
        ));
    }
    if let UnresolvedPolicy::TrimLowQualityTerminals {
        max_terminal_phred,
        min_retained_bases,
    } = config.unresolved_policy
    {
        if max_terminal_phred > 93 || min_retained_bases == 0 {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                "terminal trimming requires Q0..Q93 and nonzero retained bases",
            ));
        }
    }
    Ok(())
}

fn validate_source_descriptor(source: CorrectionSourceDescriptor) -> Result<()> {
    if source.source_snapshot_sha256 == [0; 32]
        || source.source_layout_sha256 == [0; 32]
        || source.qc_policy_sha256 == [0; 32]
    {
        return Err(VeritasmError::new(
            ErrorCode::IntegrityArtifact,
            "quality-correction source descriptor contains an unbound all-zero identity",
        ));
    }
    Ok(())
}

fn validate_spectrum(spectrum: TrustedSpectrum<'_>, previous_k: Option<u8>) -> Result<()> {
    validate_k(spectrum.k)?;
    if previous_k.is_some_and(|value| spectrum.k <= value) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            "correction spectrum k values must be strictly increasing and unique",
        ));
    }
    if spectrum.min_trusted_occurrences == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidSupport,
            format!(
                "correction spectrum k={} has zero trusted support",
                spectrum.k
            ),
        ));
    }
    let mut previous_key = None;
    for entry in spectrum.entries {
        validate_code(entry.key, spectrum.k)?;
        if canonical_code(entry.key, spectrum.k)? != entry.key {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityCountRun,
                format!(
                    "correction spectrum k={} contains a noncanonical key",
                    spectrum.k
                ),
            ));
        }
        if entry.raw_occurrences == 0 {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidSupport,
                format!(
                    "correction spectrum k={} contains zero raw support",
                    spectrum.k
                ),
            ));
        }
        if previous_key.is_some_and(|key| entry.key <= key) {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityCountRun,
                format!(
                    "correction spectrum k={} keys are not strictly increasing",
                    spectrum.k
                ),
            ));
        }
        previous_key = Some(entry.key);
    }
    Ok(())
}

fn validate_read_lengths(read: CorrectionRead<'_>) -> Result<u64> {
    if read.bases.len() != read.phred.len() {
        return Err(VeritasmError::new(
            ErrorCode::InputFastqStructure,
            format!(
                "correction read {} has {} bases but {} numeric Phred values",
                read.read_ordinal,
                read.bases.len(),
                read.phred.len()
            ),
        ));
    }
    as_u64(read.bases.len(), "correction read length")
}

fn validate_read_content(read: CorrectionRead<'_>) -> Result<()> {
    for (position, (&base, &quality)) in read.bases.iter().zip(read.phred).enumerate() {
        validate_base(base, position)?;
        if quality > 93 {
            return Err(VeritasmError::new(
                ErrorCode::InputQuality,
                format!("numeric Phred value {quality} at position {position} is outside Q0..Q93"),
            ));
        }
    }
    Ok(())
}

fn validate_read(read: CorrectionRead<'_>) -> Result<()> {
    validate_read_lengths(read)?;
    validate_read_content(read)
}

fn validate_workers(worker_threads: u16, maximum: u16) -> Result<()> {
    if worker_threads == 0 || worker_threads > maximum {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!("correction workers must be in 1..={maximum}; received {worker_threads}"),
        ));
    }
    Ok(())
}

fn score_layer(
    spectrum: TrustedSpectrum<'_>,
    baseline_bases: &[u8],
    bases: &[u8],
    changes: &[BaseSubstitution],
    work: &mut CorrectionWork,
) -> Result<LayerEvidence> {
    if baseline_bases.len() != bases.len() {
        return Err(invariant_error(
            "baseline and candidate sequence lengths differ",
        ));
    }
    let k = usize::from(spectrum.k);
    let possible = bases
        .len()
        .checked_sub(k)
        .and_then(|value| value.checked_add(1))
        .unwrap_or(0);
    let mut evidence = LayerEvidence {
        k: spectrum.k,
        possible_windows: as_u64(possible, "layer possible-window count")?,
        exact_windows: 0,
        trusted_windows: 0,
        unsupported_windows: 0,
        summed_occurrence_support: 0,
        affected_windows: 0,
        affected_trusted_windows: 0,
        affected_low_count_windows: 0,
        affected_low_count_support_sum: 0,
        affected_candidate_low_count_windows: 0,
        affected_candidate_low_count_support_sum: 0,
    };
    for start in 0..possible {
        work.window_evaluations = checked_add(
            work.window_evaluations,
            1,
            "correction window-evaluation count overflow",
        )?;
        let end = start
            .checked_add(k)
            .ok_or_else(|| overflow("correction window end overflow"))?;
        let affected = changes.iter().any(|change| {
            usize::try_from(change.position)
                .is_ok_and(|position| start <= position && position < end)
        });
        if affected {
            evidence.affected_windows = checked_add(
                evidence.affected_windows,
                1,
                "affected-window count overflow",
            )?;
            let baseline_window = &baseline_bases[start..end];
            if baseline_window.iter().all(|&base| is_exact_base(base)) {
                let baseline_key =
                    canonical_code(encode_exact_bases(baseline_window)?, spectrum.k)?;
                let baseline_support = spectrum_support(spectrum.entries, baseline_key, work)?;
                if baseline_support > 0 && baseline_support < spectrum.min_trusted_occurrences {
                    evidence.affected_low_count_windows = checked_add(
                        evidence.affected_low_count_windows,
                        1,
                        "affected low-count window overflow",
                    )?;
                    evidence.affected_low_count_support_sum = checked_add(
                        evidence.affected_low_count_support_sum,
                        baseline_support,
                        "affected low-count support overflow",
                    )?;
                }
            }
        }
        let window = &bases[start..end];
        if !window.iter().all(|&base| is_exact_base(base)) {
            evidence.unsupported_windows = checked_add(
                evidence.unsupported_windows,
                1,
                "unsupported-window count overflow",
            )?;
            continue;
        }
        evidence.exact_windows =
            checked_add(evidence.exact_windows, 1, "exact-window count overflow")?;
        let key = canonical_code(encode_exact_bases(window)?, spectrum.k)?;
        let support = spectrum_support(spectrum.entries, key, work)?;
        evidence.summed_occurrence_support = checked_add(
            evidence.summed_occurrence_support,
            support,
            "layer summed-occurrence-support sum overflow",
        )?;
        if affected && support > 0 && support < spectrum.min_trusted_occurrences {
            evidence.affected_candidate_low_count_windows = checked_add(
                evidence.affected_candidate_low_count_windows,
                1,
                "affected candidate-spelling low-count window overflow",
            )?;
            evidence.affected_candidate_low_count_support_sum = checked_add(
                evidence.affected_candidate_low_count_support_sum,
                support,
                "affected candidate-spelling low-count support overflow",
            )?;
        }
        if support >= spectrum.min_trusted_occurrences {
            evidence.trusted_windows =
                checked_add(evidence.trusted_windows, 1, "trusted-window count overflow")?;
            if affected {
                evidence.affected_trusted_windows = checked_add(
                    evidence.affected_trusted_windows,
                    1,
                    "affected trusted-window count overflow",
                )?;
            }
        } else {
            evidence.unsupported_windows = checked_add(
                evidence.unsupported_windows,
                1,
                "unsupported-window count overflow",
            )?;
        }
    }
    if evidence
        .trusted_windows
        .checked_add(evidence.unsupported_windows)
        != Some(evidence.possible_windows)
        || evidence.affected_trusted_windows > evidence.affected_windows
    {
        return Err(invariant_error(
            "candidate layer evidence does not partition possible windows",
        ));
    }
    Ok(evidence)
}

fn spectrum_support(
    entries: &[SpectrumEntry],
    key: PackedKmer,
    work: &mut CorrectionWork,
) -> Result<u64> {
    let mut left = 0_usize;
    let mut right = entries.len();
    while left < right {
        work.spectrum_comparisons = checked_add(
            work.spectrum_comparisons,
            1,
            "spectrum comparison count overflow",
        )?;
        let middle = left + (right - left) / 2;
        match entries[middle].key.cmp(&key) {
            Ordering::Less => left = middle + 1,
            Ordering::Greater => right = middle,
            Ordering::Equal => return Ok(entries[middle].raw_occurrences),
        }
    }
    Ok(0)
}

fn qualifies(
    baseline: &CandidateEvidence,
    candidate: &CandidateEvidence,
    thresholds: CorrectionThresholds,
) -> Result<bool> {
    if baseline.layers.len() != candidate.layers.len() || candidate.edit_count == 0 {
        return Err(invariant_error(
            "candidate and baseline evidence layers are incompatible",
        ));
    }
    let trusted_gain = candidate
        .trusted_windows
        .saturating_sub(baseline.trusted_windows);
    let summed_support_gain = candidate
        .summed_occurrence_support
        .saturating_sub(baseline.summed_occurrence_support);
    if candidate.trusted_windows < baseline.trusted_windows
        || candidate.unsupported_windows > baseline.unsupported_windows
        || candidate.summed_occurrence_support < baseline.summed_occurrence_support
        || trusted_gain < thresholds.min_trusted_window_gain
        || summed_support_gain < thresholds.min_summed_occurrence_support_gain
    {
        return Ok(false);
    }
    let mut supporting = 0_u8;
    for (before, after) in baseline.layers.iter().zip(&candidate.layers) {
        if before.k != after.k
            || after.trusted_windows < before.trusted_windows
            || after.unsupported_windows > before.unsupported_windows
            || after.summed_occurrence_support < before.summed_occurrence_support
        {
            return Ok(false);
        }
        if thresholds.require_all_affected_windows_trusted
            && after.affected_windows != after.affected_trusted_windows
        {
            return Ok(false);
        }
        if after.trusted_windows > before.trusted_windows
            || after.summed_occurrence_support > before.summed_occurrence_support
        {
            supporting = supporting
                .checked_add(1)
                .ok_or_else(|| overflow("supporting k-layer count overflow"))?;
        }
    }
    Ok(supporting >= thresholds.min_supporting_k_layers)
}

fn with_supporting_layers(
    baseline: &CandidateEvidence,
    mut candidate: CandidateEvidence,
) -> Result<CandidateEvidence> {
    let mut supporting = 0_u8;
    for (before, after) in baseline.layers.iter().zip(&candidate.layers) {
        if after.trusted_windows > before.trusted_windows
            || after.summed_occurrence_support > before.summed_occurrence_support
        {
            supporting = supporting
                .checked_add(1)
                .ok_or_else(|| overflow("supporting k-layer count overflow"))?;
        }
    }
    candidate.supporting_k_layers = supporting;
    Ok(candidate)
}

fn strictly_dominates(left: &CandidateEvidence, right: &CandidateEvidence) -> bool {
    if left.layers.len() != right.layers.len()
        || left.supporting_k_layers < right.supporting_k_layers
        || left.trusted_windows < right.trusted_windows
        || left.unsupported_windows > right.unsupported_windows
        || left.summed_occurrence_support < right.summed_occurrence_support
        || left.quality_penalty > right.quality_penalty
        || left.edit_count > right.edit_count
    {
        return false;
    }
    let mut strict = left.supporting_k_layers > right.supporting_k_layers
        || left.trusted_windows > right.trusted_windows
        || left.unsupported_windows < right.unsupported_windows
        || left.summed_occurrence_support > right.summed_occurrence_support
        || left.quality_penalty < right.quality_penalty
        || left.edit_count < right.edit_count;
    for (a, b) in left.layers.iter().zip(&right.layers) {
        if a.k != b.k
            || a.trusted_windows < b.trusted_windows
            || a.unsupported_windows > b.unsupported_windows
            || a.summed_occurrence_support < b.summed_occurrence_support
        {
            return false;
        }
        strict |= a.trusted_windows > b.trusted_windows
            || a.unsupported_windows < b.unsupported_windows
            || a.summed_occurrence_support > b.summed_occurrence_support;
    }
    strict
}

fn rank_cmp(evidence: &CandidateEvidence, bases: &[u8], current: &RankedCandidate) -> Ordering {
    evidence
        .supporting_k_layers
        .cmp(&current.evidence.supporting_k_layers)
        .then_with(|| {
            evidence
                .trusted_windows
                .cmp(&current.evidence.trusted_windows)
        })
        .then_with(|| {
            current
                .evidence
                .unsupported_windows
                .cmp(&evidence.unsupported_windows)
        })
        .then_with(|| {
            evidence
                .summed_occurrence_support
                .cmp(&current.evidence.summed_occurrence_support)
        })
        .then_with(|| {
            current
                .evidence
                .quality_penalty
                .cmp(&evidence.quality_penalty)
        })
        .then_with(|| current.evidence.edit_count.cmp(&evidence.edit_count))
        .then_with(|| {
            for (left, right) in evidence.layers.iter().zip(&current.evidence.layers) {
                let order = left
                    .trusted_windows
                    .cmp(&right.trusted_windows)
                    .then_with(|| right.unsupported_windows.cmp(&left.unsupported_windows))
                    .then_with(|| {
                        left.summed_occurrence_support
                            .cmp(&right.summed_occurrence_support)
                    });
                if order != Ordering::Equal {
                    return order;
                }
            }
            Ordering::Equal
        })
        .then_with(|| current.bases.as_slice().cmp(bases))
}

fn is_fully_trusted(evidence: &CandidateEvidence) -> bool {
    evidence.trusted_windows > 0
        && evidence.unsupported_windows == 0
        && evidence
            .layers
            .iter()
            .all(|layer| layer.trusted_windows == layer.possible_windows)
}

fn editable_positions(
    read: CorrectionRead<'_>,
    maximum_phred: u8,
) -> Result<Vec<EditablePosition>> {
    let count = count_editable_positions(read, maximum_phred)?;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(
            usize::try_from(count)
                .map_err(|_| memory_error("editable-position count does not fit usize".into()))?,
        )
        .map_err(|cause| memory_error(format!("cannot reserve editable positions: {cause}")))?;
    for (position, (&base, &phred)) in read.bases.iter().zip(read.phred).enumerate() {
        if phred > maximum_phred {
            continue;
        }
        let (choices, choice_count, kind) = edit_choices(base)?;
        positions.push(EditablePosition {
            position,
            before: base,
            phred,
            choices,
            choice_count,
            kind,
        });
    }
    Ok(positions)
}

fn edit_choices(base: u8) -> Result<([u8; 4], u8, BaseEditKind)> {
    let upper = base.to_ascii_uppercase();
    let result = match upper {
        // Keep exact-base alternatives in a fully specified A<C<G<T order.
        // The evaluated-candidate digest is intentionally order-dependent, so
        // an unstable sort of equal keys would not define a portable journal
        // encoding across standard-library implementations.
        b'A' => ([b'C', b'G', b'T', 0], 3, BaseEditKind::Substitution),
        b'C' => ([b'A', b'G', b'T', 0], 3, BaseEditKind::Substitution),
        b'G' => ([b'A', b'C', b'T', 0], 3, BaseEditKind::Substitution),
        b'T' => ([b'A', b'C', b'G', 0], 3, BaseEditKind::Substitution),
        b'R' => ([b'A', b'G', 0, 0], 2, BaseEditKind::AmbiguityResolution),
        b'Y' => ([b'C', b'T', 0, 0], 2, BaseEditKind::AmbiguityResolution),
        b'S' => ([b'C', b'G', 0, 0], 2, BaseEditKind::AmbiguityResolution),
        b'W' => ([b'A', b'T', 0, 0], 2, BaseEditKind::AmbiguityResolution),
        b'K' => ([b'G', b'T', 0, 0], 2, BaseEditKind::AmbiguityResolution),
        b'M' => ([b'A', b'C', 0, 0], 2, BaseEditKind::AmbiguityResolution),
        b'B' => ([b'C', b'G', b'T', 0], 3, BaseEditKind::AmbiguityResolution),
        b'D' => ([b'A', b'G', b'T', 0], 3, BaseEditKind::AmbiguityResolution),
        b'H' => ([b'A', b'C', b'T', 0], 3, BaseEditKind::AmbiguityResolution),
        b'V' => ([b'A', b'C', b'G', 0], 3, BaseEditKind::AmbiguityResolution),
        b'N' => (DNA, 4, BaseEditKind::AmbiguityResolution),
        _ => {
            return Err(VeritasmError::new(
                ErrorCode::InputNucleotide,
                format!("invalid correction nucleotide 0x{base:02x}"),
            ));
        }
    };
    Ok(result)
}

fn count_editable_positions(read: CorrectionRead<'_>, maximum_phred: u8) -> Result<u64> {
    let count = read
        .bases
        .iter()
        .zip(read.phred)
        .filter(|&(_, &phred)| phred <= maximum_phred)
        .count();
    as_u64(count, "editable-position count")
}

fn projected_candidate_count(
    read: CorrectionRead<'_>,
    maximum_phred: u8,
    max_substitutions: u8,
) -> Result<u64> {
    let mut coefficients = [0_u64; 4];
    coefficients[0] = 1;
    let maximum = usize::from(max_substitutions);
    for (&base, &phred) in read.bases.iter().zip(read.phred) {
        if phred > maximum_phred {
            continue;
        }
        let (_, choice_count, _) = edit_choices(base)?;
        let choices = u64::from(choice_count);
        for edits in (1..=maximum).rev() {
            let added = coefficients[edits - 1]
                .checked_mul(choices)
                .ok_or_else(|| overflow("candidate-count product overflow"))?;
            coefficients[edits] =
                checked_add(coefficients[edits], added, "candidate-count sum overflow")?;
        }
    }
    coefficients[1..=maximum]
        .iter()
        .try_fold(0_u64, |sum, &value| {
            checked_add(sum, value, "candidate-count total overflow")
        })
}

fn binary_search_comparison_bound(entries: usize) -> Result<u64> {
    let mut remaining = as_u64(entries, "spectrum entry count for comparison bound")?;
    let mut comparisons = 0_u64;
    while remaining > 0 {
        comparisons = checked_add(comparisons, 1, "binary-search comparison bound overflow")?;
        remaining /= 2;
    }
    Ok(comparisons)
}

fn projected_spectrum_comparisons(baseline_bound: u64, projected_candidates: u64) -> Result<u64> {
    // Baseline scoring performs at most one lookup per window. Each of two
    // candidate passes performs at most two lookups per window: one for the
    // candidate and one for the replaced baseline spelling.
    let candidate_factor = checked_mul(
        projected_candidates,
        4,
        "candidate spectrum-comparison factor overflow",
    )?;
    checked_mul(
        baseline_bound,
        checked_add(
            candidate_factor,
            1,
            "total spectrum-comparison factor overflow",
        )?,
        "total spectrum-comparison projection overflow",
    )
}

fn ceil_log2(value: u64) -> Result<u64> {
    if value <= 1 {
        return Ok(0);
    }
    let mut ceiling = 0_u64;
    let mut power = 1_u64;
    while power < value {
        power = power
            .checked_mul(2)
            .ok_or_else(|| overflow("summary heap depth projection overflow"))?;
        ceiling = checked_add(ceiling, 1, "summary heap depth projection overflow")?;
    }
    Ok(ceiling)
}

fn projected_summary_comparisons(
    projected_candidates: u64,
    retained_summaries: u64,
) -> Result<u64> {
    let depth = ceil_log2(retained_summaries)?;
    let per_candidate = checked_add(
        checked_mul(depth, 2, "summary comparison projection overflow")?,
        1,
        "summary comparison projection overflow",
    )?;
    let selection = checked_mul(
        projected_candidates,
        per_candidate,
        "summary selection comparison projection overflow",
    )?;
    let final_sort = checked_mul(
        retained_summaries,
        checked_mul(depth, 2, "summary sort comparison projection overflow")?,
        "summary sort comparison projection overflow",
    )?;
    checked_add(
        selection,
        final_sort,
        "summary comparison projection overflow",
    )
}

fn projected_search_work_units(
    read_bases: usize,
    spectra: usize,
    max_substitutions: u8,
    projected_candidates: u64,
) -> Result<u64> {
    // One unit is one byte or fixed-size record visited by auxiliary search
    // work not already counted as a window, spectrum, or heap comparison.
    // Per candidate this conservatively covers candidate identity hashing,
    // best-candidate copying, second-pass equality, edit generation and
    // serialization, and all threshold/rank/digest/dominance layer visits.
    let bases = checked_mul(
        as_u64(read_bases, "search-work read length")?,
        3,
        "search-work base visits overflow",
    )?;
    let edits = checked_mul(
        u64::from(max_substitutions),
        5,
        "search-work edit visits overflow",
    )?;
    let layers = checked_mul(
        as_u64(spectra, "search-work spectrum count")?,
        7,
        "search-work layer visits overflow",
    )?;
    let per_candidate = bases
        .checked_add(edits)
        .and_then(|value| value.checked_add(layers))
        .and_then(|value| value.checked_add(8))
        .ok_or_else(|| overflow("per-candidate search-work projection overflow"))?;
    checked_mul(
        projected_candidates,
        per_candidate,
        "candidate search-work projection overflow",
    )
}

fn enumerate_candidates<F>(
    working: &mut [u8],
    editable: &[EditablePosition],
    max_substitutions: u8,
    changes: &mut Vec<BaseSubstitution>,
    visitor: &mut F,
) -> Result<()>
where
    F: FnMut(&[u8], &[BaseSubstitution]) -> Result<()>,
{
    for target_edits in 1..=max_substitutions {
        enumerate_depth(working, editable, 0, target_edits, changes, visitor)?;
    }
    Ok(())
}

fn enumerate_depth<F>(
    working: &mut [u8],
    editable: &[EditablePosition],
    start: usize,
    remaining: u8,
    changes: &mut Vec<BaseSubstitution>,
    visitor: &mut F,
) -> Result<()>
where
    F: FnMut(&[u8], &[BaseSubstitution]) -> Result<()>,
{
    if remaining == 0 {
        return visitor(working, changes);
    }
    let remaining_usize = usize::from(remaining);
    if editable.len().saturating_sub(start) < remaining_usize {
        return Ok(());
    }
    let last = editable.len() - remaining_usize;
    for index in start..=last {
        let site = editable[index];
        for &after in &site.choices[..usize::from(site.choice_count)] {
            if after == site.before.to_ascii_uppercase() {
                continue;
            }
            working[site.position] = after;
            changes.push(BaseSubstitution {
                position: u32::try_from(site.position)
                    .map_err(|_| memory_error("substitution position does not fit u32".into()))?,
                before: site.before,
                after,
                phred: site.phred,
                kind: site.kind,
            });
            enumerate_depth(
                working,
                editable,
                index + 1,
                remaining - 1,
                changes,
                visitor,
            )?;
            changes.pop();
            working[site.position] = site.before;
        }
    }
    Ok(())
}

fn terminal_trim_range(
    read: CorrectionRead<'_>,
    maximum_phred: u8,
    minimum_retained: usize,
) -> Result<Option<RetainedRange>> {
    let removable =
        |index: usize| read.phred[index] <= maximum_phred || !is_exact_base(read.bases[index]);
    let mut start = 0_usize;
    while start < read.bases.len() && removable(start) {
        start += 1;
    }
    let mut end = read.bases.len();
    while end > start && removable(end - 1) {
        end -= 1;
    }
    if start == 0 && end == read.bases.len() {
        return Ok(None);
    }
    if end.saturating_sub(start) < minimum_retained
        || !read.bases[start..end]
            .iter()
            .all(|&base| is_exact_base(base))
    {
        return Ok(None);
    }
    Ok(Some(RetainedRange {
        start: u32::try_from(start)
            .map_err(|_| memory_error("trim start does not fit u32".into()))?,
        end: u32::try_from(end).map_err(|_| memory_error("trim end does not fit u32".into()))?,
    }))
}

fn projected_read_scratch_bytes(
    read_bases: usize,
    editable_positions: usize,
    spectra: usize,
    max_substitutions: u8,
    retained_candidate_summaries: u64,
) -> Result<u64> {
    let bases = as_u64(read_bases, "projected read bases")?;
    let three_base_buffers = checked_mul(bases, 3, "projected read base buffers overflow")?;
    let editables = checked_mul(
        as_u64(editable_positions, "projected editable positions")?,
        as_u64(size_of::<EditablePosition>(), "editable-position type size")?,
        "projected editable-position bytes overflow",
    )?;
    let layer_vectors = checked_mul(
        checked_mul(
            as_u64(spectra, "projected spectrum count")?,
            as_u64(size_of::<LayerEvidence>(), "layer-evidence type size")?,
            "projected layer evidence bytes overflow",
        )?,
        3,
        "projected score-vector bytes overflow",
    )?;
    let change_vectors = checked_mul(
        checked_mul(
            u64::from(max_substitutions),
            as_u64(size_of::<BaseSubstitution>(), "substitution type size")?,
            "projected substitution bytes overflow",
        )?,
        3,
        "projected substitution bytes overflow",
    )?;
    let candidate_summaries = checked_mul(
        retained_candidate_summaries,
        as_u64(
            size_of::<CandidateAlternativeSummary>(),
            "candidate-alternative-summary type size",
        )?,
        "projected candidate-summary bytes overflow",
    )?;
    three_base_buffers
        .checked_add(editables)
        .and_then(|value| value.checked_add(layer_vectors))
        .and_then(|value| value.checked_add(change_vectors))
        .and_then(|value| value.checked_add(candidate_summaries))
        .and_then(|value| value.checked_add(2048))
        .ok_or_else(|| overflow("projected per-read accounted bytes overflow"))
}

fn projected_raw_output_bytes(read_bases: usize) -> Result<u64> {
    checked_add(
        as_u64(read_bases, "projected raw output bases")?,
        1024,
        "projected raw output bytes overflow",
    )
}

fn projected_baseline_bytes(read_bases: usize, spectra: usize) -> Result<u64> {
    let layers = checked_mul(
        as_u64(spectra, "projected baseline spectrum count")?,
        as_u64(size_of::<LayerEvidence>(), "layer-evidence type size")?,
        "projected baseline layer bytes overflow",
    )?;
    checked_add(
        projected_raw_output_bytes(read_bases)?,
        checked_add(layers, 1024, "projected baseline bookkeeping overflow")?,
        "projected baseline bytes overflow",
    )
}

fn projected_batch_bytes(
    reads: &[CorrectionRead<'_>],
    spectra: usize,
    config: CorrectionConfig,
    worker_threads: u16,
) -> Result<u64> {
    let mut total = checked_mul(
        as_u64(reads.len(), "projected batch read count")?,
        as_u64(
            size_of::<CorrectionDecision>(),
            "correction-decision type size",
        )?,
        "projected batch decision headers overflow",
    )?;
    total = checked_add(
        total,
        checked_mul(
            as_u64(reads.len(), "projected batch order count")?,
            as_u64(size_of::<usize>(), "usize type size")?,
            "projected batch order bytes overflow",
        )?,
        "projected batch bytes overflow",
    )?;
    total = checked_add(
        total,
        checked_mul(
            as_u64(reads.len(), "projected batch slot count")?,
            as_u64(
                size_of::<OnceLock<Result<CorrectionDecision>>>(),
                "correction result-slot type size",
            )?,
            "projected correction result-slot bytes overflow",
        )?,
        "projected batch bytes overflow",
    )?;
    let per_read_layers = checked_mul(
        as_u64(spectra, "projected batch spectrum count")?,
        as_u64(size_of::<LayerEvidence>(), "layer evidence type size")?,
        "projected batch layer bytes overflow",
    )?;
    let per_read_changes = checked_mul(
        u64::from(config.limits.max_substitutions),
        as_u64(size_of::<BaseSubstitution>(), "substitution type size")?,
        "projected batch substitution bytes overflow",
    )?;
    let per_read_candidate_summaries = checked_mul(
        u64::from(config.limits.max_candidate_summaries_per_read)
            .min(config.limits.max_candidates_per_read),
        as_u64(
            size_of::<CandidateAlternativeSummary>(),
            "candidate-alternative-summary type size",
        )?,
        "projected batch candidate-summary bytes overflow",
    )?;
    let mut maximum_scratch = 0_u64;
    for read in reads {
        let read_bases = validate_read_lengths(*read)?;
        let read_bases_exceeded = read_bases > config.limits.max_read_bases;
        validate_read_content(*read)?;
        let scratch = if read_bases_exceeded {
            0
        } else if config.mode == CorrectionMode::RawOnly {
            projected_raw_output_bytes(read.bases.len())?
        } else {
            let editable = count_editable_positions(*read, config.thresholds.max_edit_phred)?;
            projected_read_scratch_bytes(
                read.bases.len(),
                usize::try_from(editable)
                    .map_err(|_| memory_error("batch editable count does not fit usize".into()))?,
                spectra,
                config.limits.max_substitutions,
                u64::from(config.limits.max_candidate_summaries_per_read)
                    .min(config.limits.max_candidates_per_read),
            )?
        };
        maximum_scratch = maximum_scratch.max(scratch);
        total = checked_add(
            total,
            as_u64(read.bases.len(), "projected batch emitted bases")?,
            "projected batch bytes overflow",
        )?;
        if config.mode != CorrectionMode::RawOnly {
            total = checked_add(
                total,
                checked_mul(
                    per_read_layers,
                    2,
                    "projected batch journal layer bytes overflow",
                )?,
                "projected batch bytes overflow",
            )?;
            total = checked_add(total, per_read_changes, "projected batch bytes overflow")?;
            total = checked_add(
                total,
                per_read_candidate_summaries,
                "projected batch bytes overflow",
            )?;
        }
    }
    let concurrent_workers = u64::from(worker_threads).min(as_u64(reads.len(), "batch reads")?);
    total = checked_add(
        total,
        checked_mul(
            concurrent_workers,
            maximum_scratch,
            "projected concurrent correction scratch overflow",
        )?,
        "projected batch bytes overflow",
    )?;
    Ok(total)
}

fn read_identity(read: CorrectionRead<'_>) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(READ_IDENTITY_DOMAIN);
    digest.update(read.read_ordinal.to_be_bytes());
    digest.update(as_u64(read.bases.len(), "journal read length")?.to_be_bytes());
    digest.update(read.bases);
    digest.update(as_u64(read.phred.len(), "journal quality length")?.to_be_bytes());
    digest.update(read.phred);
    Ok(digest.finalize().into())
}

fn candidate_identity(candidate: &[u8]) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(CANDIDATE_IDENTITY_DOMAIN);
    digest.update(as_u64(candidate.len(), "candidate identity length")?.to_be_bytes());
    digest.update(candidate);
    Ok(digest.finalize().into())
}

fn update_evaluated_candidate_digest(
    digest: &mut Sha256,
    identity: [u8; 32],
    changes: &[BaseSubstitution],
    evidence: &CandidateEvidence,
    threshold_qualified: bool,
) -> Result<()> {
    digest.update([0x01]);
    digest.update(identity);
    digest.update(as_u64(changes.len(), "evaluated candidate edit count")?.to_be_bytes());
    for change in changes {
        digest.update(change.position.to_be_bytes());
        digest.update([change.before, change.after, change.phred]);
        digest.update([match change.kind {
            BaseEditKind::Substitution => 0,
            BaseEditKind::AmbiguityResolution => 1,
        }]);
    }
    digest.update([
        evidence.edit_count,
        evidence.supporting_k_layers,
        u8::from(threshold_qualified),
    ]);
    digest.update(evidence.quality_penalty.to_be_bytes());
    digest.update(evidence.trusted_windows.to_be_bytes());
    digest.update(evidence.unsupported_windows.to_be_bytes());
    digest.update(evidence.summed_occurrence_support.to_be_bytes());
    digest.update(evidence.affected_low_count_windows.to_be_bytes());
    digest.update(evidence.affected_low_count_support_sum.to_be_bytes());
    digest.update(evidence.affected_candidate_low_count_windows.to_be_bytes());
    digest.update(
        evidence
            .affected_candidate_low_count_support_sum
            .to_be_bytes(),
    );
    digest.update(as_u64(evidence.layers.len(), "evaluated candidate layer count")?.to_be_bytes());
    for layer in &evidence.layers {
        digest.update([layer.k]);
        digest.update(layer.possible_windows.to_be_bytes());
        digest.update(layer.exact_windows.to_be_bytes());
        digest.update(layer.trusted_windows.to_be_bytes());
        digest.update(layer.unsupported_windows.to_be_bytes());
        digest.update(layer.summed_occurrence_support.to_be_bytes());
        digest.update(layer.affected_windows.to_be_bytes());
        digest.update(layer.affected_trusted_windows.to_be_bytes());
        digest.update(layer.affected_low_count_windows.to_be_bytes());
        digest.update(layer.affected_low_count_support_sum.to_be_bytes());
        digest.update(layer.affected_candidate_low_count_windows.to_be_bytes());
        digest.update(layer.affected_candidate_low_count_support_sum.to_be_bytes());
    }
    Ok(())
}

fn source_descriptor_identity(source: CorrectionSourceDescriptor) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SOURCE_DOMAIN);
    digest.update(source.source_snapshot_sha256);
    digest.update(source.source_layout_sha256);
    digest.update(source.qc_policy_sha256);
    digest.update([match source.support_unit {
        CorrectionSupportUnit::ExactKmerOccurrence => 0,
    }]);
    digest.finalize().into()
}

fn config_identity(config: CorrectionConfig) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(CONFIG_DOMAIN);
    digest.update([match config.mode {
        CorrectionMode::RawOnly => 0,
        CorrectionMode::DiversityPreserving => 1,
        CorrectionMode::ConsensusExperimental => 2,
    }]);
    digest.update([config.thresholds.max_edit_phred]);
    digest.update(config.thresholds.min_trusted_window_gain.to_be_bytes());
    digest.update(
        config
            .thresholds
            .min_summed_occurrence_support_gain
            .to_be_bytes(),
    );
    digest.update([config.thresholds.min_supporting_k_layers]);
    digest.update([u8::from(
        config.thresholds.require_all_affected_windows_trusted,
    )]);
    match config.unresolved_policy {
        UnresolvedPolicy::LeaveUnchanged => digest.update([0]),
        UnresolvedPolicy::Quarantine => digest.update([1]),
        UnresolvedPolicy::TrimLowQualityTerminals {
            max_terminal_phred,
            min_retained_bases,
        } => {
            digest.update([2, max_terminal_phred]);
            digest.update(min_retained_bases.to_be_bytes());
        }
    }
    let limits = config.limits;
    digest.update(limits.max_read_bases.to_be_bytes());
    digest.update(limits.max_spectra.to_be_bytes());
    digest.update(limits.max_spectrum_entries_total.to_be_bytes());
    digest.update(limits.max_spectrum_comparisons_per_read.to_be_bytes());
    digest.update(limits.max_editable_positions.to_be_bytes());
    digest.update([limits.max_substitutions]);
    digest.update(limits.max_candidates_per_read.to_be_bytes());
    digest.update(limits.max_window_evaluations_per_read.to_be_bytes());
    digest.update(limits.max_summary_comparisons_per_read.to_be_bytes());
    digest.update(limits.max_search_work_units_per_read.to_be_bytes());
    digest.update(limits.max_journal_entries_per_read.to_be_bytes());
    digest.update(limits.max_candidate_summaries_per_read.to_be_bytes());
    digest.update(limits.max_accounted_bytes_per_read.to_be_bytes());
    digest.update(limits.max_batch_reads.to_be_bytes());
    digest.update(limits.max_batch_bases.to_be_bytes());
    digest.update(limits.max_batch_accounted_bytes.to_be_bytes());
    digest.update(limits.max_worker_threads.to_be_bytes());
    digest.finalize().into()
}

fn retain_ranked_candidate(
    retained: &mut Option<RankedCandidate>,
    candidate: &[u8],
    candidate_changes: &[BaseSubstitution],
    evidence: CandidateEvidence,
) -> Result<()> {
    if retained
        .as_ref()
        .is_some_and(|current| rank_cmp(&evidence, candidate, current) != Ordering::Greater)
    {
        return Ok(());
    }
    let bases = fallible_copy(candidate, "ranked correction candidate")?;
    let mut changes = Vec::new();
    changes
        .try_reserve_exact(candidate_changes.len())
        .map_err(|cause| memory_error(format!("cannot reserve ranked substitutions: {cause}")))?;
    changes.extend_from_slice(candidate_changes);
    *retained = Some(RankedCandidate {
        bases,
        changes,
        evidence,
    });
    Ok(())
}

fn verify_enumeration_count(observed: u64, expected: u64) -> Result<()> {
    if observed != expected {
        return Err(invariant_error(
            "candidate enumeration differs from its checked projection",
        ));
    }
    Ok(())
}

fn validate_base(base: u8, position: usize) -> Result<()> {
    if matches!(
        base.to_ascii_uppercase(),
        b'A' | b'C'
            | b'G'
            | b'T'
            | b'R'
            | b'Y'
            | b'S'
            | b'W'
            | b'K'
            | b'M'
            | b'B'
            | b'D'
            | b'H'
            | b'V'
            | b'N'
    ) {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::InputNucleotide,
            format!("invalid correction nucleotide 0x{base:02x} at position {position}"),
        ))
    }
}

fn is_exact_base(base: u8) -> bool {
    matches!(base.to_ascii_uppercase(), b'A' | b'C' | b'G' | b'T')
}

fn fallible_copy(bytes: &[u8], label: &'static str) -> Result<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|cause| memory_error(format!("cannot reserve {label}: {cause}")))?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn enforce_global_limit(
    observed: u64,
    limit: u64,
    code: ErrorCode,
    label: &'static str,
) -> Result<()> {
    if observed > limit {
        Err(VeritasmError::new(
            code,
            format!("{label} {observed} exceeds configured limit {limit}"),
        ))
    } else {
        Ok(())
    }
}

fn as_u64(value: usize, label: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| memory_error(format!("{label} does not fit u64")))
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn checked_mul(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| overflow(context))
}

fn overflow(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn memory_error(context: String) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn invariant_error(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    fn canonical(window: &[u8]) -> PackedKmer {
        let k = u8::try_from(window.len()).unwrap();
        canonical_code(encode_exact_bases(window).unwrap(), k).unwrap()
    }

    fn spectrum_entries(k: u8, sequences: &[(&[u8], u64)]) -> Vec<SpectrumEntry> {
        let mut counts = BTreeMap::new();
        for &(sequence, abundance) in sequences {
            for window in sequence.windows(usize::from(k)) {
                let key = canonical(window);
                *counts.entry(key).or_insert(0_u64) += abundance;
            }
        }
        counts
            .into_iter()
            .map(|(key, raw_occurrences)| SpectrumEntry {
                key,
                raw_occurrences,
            })
            .collect()
    }

    fn source_descriptor() -> CorrectionSourceDescriptor {
        CorrectionSourceDescriptor {
            source_snapshot_sha256: [1; 32],
            source_layout_sha256: [2; 32],
            qc_policy_sha256: [3; 32],
            support_unit: CorrectionSupportUnit::ExactKmerOccurrence,
        }
    }

    fn engine<'a>(
        config: CorrectionConfig,
        k3: &'a [SpectrumEntry],
        k5: &'a [SpectrumEntry],
    ) -> QualityCorrectionEngine<'a> {
        QualityCorrectionEngine::new(
            config,
            source_descriptor(),
            &[
                TrustedSpectrum {
                    k: 3,
                    min_trusted_occurrences: 2,
                    entries: k3,
                },
                TrustedSpectrum {
                    k: 5,
                    min_trusted_occurrences: 2,
                    entries: k5,
                },
            ],
        )
        .unwrap()
    }

    fn single_edit_config() -> CorrectionConfig {
        CorrectionConfig {
            mode: CorrectionMode::ConsensusExperimental,
            unresolved_policy: UnresolvedPolicy::Quarantine,
            limits: CorrectionLimits {
                max_substitutions: 1,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        }
    }

    fn single_layer_engine(
        config: CorrectionConfig,
        entries: &[SpectrumEntry],
    ) -> QualityCorrectionEngine<'_> {
        QualityCorrectionEngine::new(
            config,
            source_descriptor(),
            &[TrustedSpectrum {
                k: 3,
                min_trusted_occurrences: 2,
                entries,
            }],
        )
        .unwrap()
    }

    fn exact_spectrum_entries(keys: &[&[u8]]) -> Vec<SpectrumEntry> {
        let mut entries = keys
            .iter()
            .map(|key| SpectrumEntry {
                key: canonical(key),
                raw_occurrences: 2,
            })
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|entry| entry.key);
        entries.dedup_by_key(|entry| entry.key);
        entries
    }

    #[test]
    fn trusted_baseline_precedes_search_only_caps() {
        let sequence = vec![b'A'; 40];
        let quality = vec![0_u8; 40];
        let entries = spectrum_entries(3, &[(&sequence, 100)]);
        let decision = single_layer_engine(CorrectionConfig::default(), &entries)
            .correct_read(CorrectionRead {
                read_ordinal: 0,
                bases: &sequence,
                phred: &quality,
            })
            .unwrap();
        assert_eq!(
            decision.journal.disposition,
            ReadDisposition::Unchanged(UnchangedReason::AlreadyTrusted)
        );
        assert!(!decision.journal.candidate_projection_performed);
        assert_eq!(decision.journal.projected_candidates, 0);
        assert_eq!(decision.journal.evaluated_candidates, 0);
    }

    #[test]
    fn affected_window_geometry_cannot_break_an_evidence_tie() {
        for (baseline, keys) in [
            (
                b"AAACA".as_slice(),
                [
                    b"AAC".as_slice(),
                    b"ACA".as_slice(),
                    b"CAA".as_slice(),
                    b"ACC".as_slice(),
                    b"CCA".as_slice(),
                    b"AAC".as_slice(),
                ],
            ),
            (
                b"CAACA".as_slice(),
                [
                    b"AAA".as_slice(),
                    b"AAC".as_slice(),
                    b"ACA".as_slice(),
                    b"ACC".as_slice(),
                    b"CAC".as_slice(),
                    b"CCA".as_slice(),
                ],
            ),
        ] {
            let entries = exact_spectrum_entries(&keys);
            let decision = single_layer_engine(single_edit_config(), &entries)
                .correct_read(CorrectionRead {
                    read_ordinal: 0,
                    bases: baseline,
                    phred: &[0, 40, 0, 40, 40],
                })
                .unwrap();
            assert!(matches!(
                decision.journal.disposition,
                ReadDisposition::Quarantined(QuarantineReason::ConflictingCandidates)
            ));
        }
    }

    #[test]
    fn correction_modes_make_low_count_loss_explicit() {
        let major = b"AACCGGTTA";
        let minor = b"AATCGGTTA";
        let entries = spectrum_entries(3, &[(major, 100), (minor, 1)]);
        let read = CorrectionRead {
            read_ordinal: 0,
            bases: minor,
            phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
        };

        let diversity_config = CorrectionConfig {
            mode: CorrectionMode::DiversityPreserving,
            ..single_edit_config()
        };
        let diversity = single_layer_engine(diversity_config, &entries)
            .correct_read(read)
            .unwrap();
        assert!(matches!(
            diversity.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::DiversityLowCountConflict {
                affected_windows,
                affected_support_sum
            }) if affected_windows > 0 && affected_support_sum > 0
        ));
        assert_eq!(diversity.accounting.diversity_low_count_quarantines, 1);

        let consensus_config = CorrectionConfig {
            mode: CorrectionMode::ConsensusExperimental,
            ..single_edit_config()
        };
        let consensus = single_layer_engine(consensus_config, &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(consensus.emitted_bases.as_deref(), Some(major.as_slice()));
        assert!(consensus.accounting.consensus_low_count_windows > 0);
        assert!(consensus.accounting.consensus_low_count_support_sum > 0);
        assert!(
            consensus
                .journal
                .candidate_search_evidence
                .evaluated_candidates_with_low_count_evidence
                > 0
        );
        assert!(
            consensus
                .journal
                .candidate_search_evidence
                .affected_low_count_windows
                > 0
        );

        let raw_config = CorrectionConfig {
            mode: CorrectionMode::RawOnly,
            ..single_edit_config()
        };
        let raw = single_layer_engine(raw_config, &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(raw.emitted_bases.as_deref(), Some(minor.as_slice()));
        assert_eq!(
            raw.journal.disposition,
            ReadDisposition::Unchanged(UnchangedReason::RawOnly)
        );
        assert!(raw.journal.baseline_evidence.is_none());
        assert!(!raw.journal.candidate_projection_performed);
    }

    #[test]
    fn diversity_mode_requires_independent_context_for_exact_substitution() {
        let truth = b"AACCGGTTA";
        let entries = spectrum_entries(3, &[(truth, 20)]);
        let config = CorrectionConfig {
            limits: CorrectionLimits {
                max_substitutions: 1,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let decision = single_layer_engine(config, &entries)
            .correct_read(CorrectionRead {
                read_ordinal: 0,
                bases: b"AATCGGTTA",
                phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
            })
            .unwrap();
        assert_eq!(
            decision.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::IndependentContextUnavailable)
        );
        assert_eq!(decision.accounting.independent_context_quarantines, 1);
    }

    #[test]
    fn spectrum_only_mosaic_is_confined_to_consensus_experiment() {
        let left = b"AACCTT";
        let right = b"TTCCGG";
        let entries = spectrum_entries(3, &[(left, 20), (right, 20)]);
        let read = CorrectionRead {
            read_ordinal: 0,
            bases: b"AATCGG",
            phred: &[40, 40, 0, 40, 40, 40],
        };
        let consensus = single_layer_engine(single_edit_config(), &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(
            consensus.emitted_bases.as_deref(),
            Some(b"AACCGG".as_slice())
        );

        let diversity_config = CorrectionConfig {
            mode: CorrectionMode::DiversityPreserving,
            ..single_edit_config()
        };
        let diversity = single_layer_engine(diversity_config, &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(
            diversity.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::IndependentContextUnavailable)
        );
    }

    #[test]
    fn ambiguous_mosaic_is_raw_or_quarantined_unless_consensus_is_explicit() {
        // Neither source sequence contains AACCGG end to end. Their exact
        // k=3 spectra nevertheless make resolving N to C look complete.
        let left = b"AACCTT";
        let right = b"TTCCGG";
        let entries = spectrum_entries(3, &[(left, 20), (right, 20)]);
        let read = CorrectionRead {
            read_ordinal: 17,
            bases: b"AANCGG",
            phred: &[40, 40, 0, 40, 40, 40],
        };

        let diversity_config = CorrectionConfig {
            mode: CorrectionMode::DiversityPreserving,
            ..single_edit_config()
        };
        let diversity = single_layer_engine(diversity_config, &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(
            diversity.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::IndependentContextUnavailable)
        );
        assert!(diversity.emitted_bases.is_none());
        assert!(
            diversity
                .journal
                .candidate_search_evidence
                .evaluated_ambiguity_resolution_candidates
                > 0
        );

        let raw_config = CorrectionConfig {
            mode: CorrectionMode::RawOnly,
            ..single_edit_config()
        };
        let raw = single_layer_engine(raw_config, &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(raw.emitted_bases.as_deref(), Some(b"AANCGG".as_slice()));
        assert_eq!(
            raw.journal.disposition,
            ReadDisposition::Unchanged(UnchangedReason::RawOnly)
        );

        let consensus = single_layer_engine(single_edit_config(), &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(
            consensus.emitted_bases.as_deref(),
            Some(b"AACCGG".as_slice())
        );
        let audit = &consensus.journal.candidate_search_evidence;
        assert!(audit.evaluated_ambiguity_resolution_candidates > 0);
        assert_ne!(audit.evaluated_candidate_set_digest, [0; 32]);
        assert!(audit.retained_alternatives.iter().any(|alternative| {
            alternative.changes[0].is_some_and(|change| {
                change.before == b'N'
                    && change.after == b'C'
                    && change.kind == BaseEditKind::AmbiguityResolution
            })
        }));
    }

    #[test]
    fn diversity_abstains_for_every_iupac_resolution_symbol() {
        for symbol in b"RYSWKMBDHVN" {
            let (choices, _, _) = edit_choices(*symbol).unwrap();
            let mut truth = b"GGATCCA".to_vec();
            truth[2] = choices[0];
            let entries = spectrum_entries(3, &[(&truth, 20)]);
            let mut ambiguous = truth.clone();
            ambiguous[2] = *symbol;
            let decision = single_layer_engine(
                CorrectionConfig {
                    limits: CorrectionLimits {
                        max_substitutions: 1,
                        ..CorrectionConfig::default().limits
                    },
                    ..CorrectionConfig::default()
                },
                &entries,
            )
            .correct_read(CorrectionRead {
                read_ordinal: u64::from(*symbol),
                bases: &ambiguous,
                phred: &[40, 40, 0, 40, 40, 40, 40],
            })
            .unwrap();
            assert_eq!(
                decision.journal.disposition,
                ReadDisposition::Quarantined(QuarantineReason::IndependentContextUnavailable),
                "symbol {}",
                char::from(*symbol)
            );
            assert!(decision.emitted_bases.is_none());
            assert!(
                decision
                    .journal
                    .candidate_search_evidence
                    .evaluated_ambiguity_resolution_candidates
                    > 0,
                "symbol {}",
                char::from(*symbol)
            );
        }
    }

    #[test]
    fn consensus_journals_supported_subthreshold_ambiguity_alternatives() {
        let major = b"AACCGGTTA";
        let minor = b"AATCGGTTA";
        let entries = spectrum_entries(3, &[(major, 100), (minor, 1)]);
        let decision = single_layer_engine(single_edit_config(), &entries)
            .correct_read(CorrectionRead {
                read_ordinal: 19,
                bases: b"AANCGGTTA",
                phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
            })
            .unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(major.as_slice()));
        let audit = &decision.journal.candidate_search_evidence;
        assert_eq!(audit.evaluated_candidates, 4);
        assert_eq!(audit.evaluated_ambiguity_resolution_candidates, 4);
        assert_eq!(audit.retained_alternatives.len(), 4);
        let minor_alternative = audit
            .retained_alternatives
            .iter()
            .find(|alternative| {
                alternative.changes[0].is_some_and(|change| {
                    change.before == b'N'
                        && change.after == b'T'
                        && change.kind == BaseEditKind::AmbiguityResolution
                })
            })
            .expect("the supported T alternative must be retained under the configured cap");
        assert!(!minor_alternative.threshold_qualified);
        assert!(minor_alternative.affected_candidate_low_count_windows > 0);
        assert!(minor_alternative.affected_candidate_low_count_support_sum > 0);
        assert!(audit.affected_candidate_low_count_windows > 0);
        assert!(audit.affected_candidate_low_count_support_sum > 0);
    }

    #[test]
    fn iupac_resolution_never_leaves_the_encoded_symbol_set() {
        let truth = b"AACCGGTTA";
        let entries = spectrum_entries(3, &[(truth, 20)]);
        let decision = single_layer_engine(single_edit_config(), &entries)
            .correct_read(CorrectionRead {
                read_ordinal: 0,
                bases: b"AARCGGTTA",
                phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
            })
            .unwrap();
        assert!(decision.emitted_bases.is_none());
        assert_eq!(decision.journal.projected_candidates, 2);
    }

    #[test]
    fn every_iupac_symbol_has_the_exact_declared_resolution_set() {
        for (symbol, expected) in [
            (b'R', b"AG".as_slice()),
            (b'Y', b"CT".as_slice()),
            (b'S', b"CG".as_slice()),
            (b'W', b"AT".as_slice()),
            (b'K', b"GT".as_slice()),
            (b'M', b"AC".as_slice()),
            (b'B', b"CGT".as_slice()),
            (b'D', b"AGT".as_slice()),
            (b'H', b"ACT".as_slice()),
            (b'V', b"ACG".as_slice()),
            (b'N', b"ACGT".as_slice()),
        ] {
            let (choices, count, kind) = edit_choices(symbol).unwrap();
            assert_eq!(&choices[..usize::from(count)], expected);
            assert_eq!(kind, BaseEditKind::AmbiguityResolution);
            let (lower_choices, lower_count, lower_kind) =
                edit_choices(symbol.to_ascii_lowercase()).unwrap();
            assert_eq!(&lower_choices[..usize::from(lower_count)], expected);
            assert_eq!(lower_kind, BaseEditKind::AmbiguityResolution);
        }
    }

    #[test]
    fn every_exact_base_has_portable_acgt_order_without_the_original() {
        for (base, expected) in [
            (b'A', b"CGT".as_slice()),
            (b'C', b"AGT".as_slice()),
            (b'G', b"ACT".as_slice()),
            (b'T', b"ACG".as_slice()),
        ] {
            for spelling in [base, base.to_ascii_lowercase()] {
                let (choices, count, kind) = edit_choices(spelling).unwrap();
                assert_eq!(&choices[..usize::from(count)], expected);
                assert_eq!(kind, BaseEditKind::Substitution);
            }
        }
    }

    #[test]
    fn versioned_candidate_digest_has_a_portable_golden_vector() {
        // `AAA` at its middle base enumerates C, G, then T. This digest binds
        // that explicit replacement order as well as the complete evidence
        // serialization for all three evaluated alternatives.
        let mut entries = [
            SpectrumEntry {
                key: canonical(b"ACA"),
                raw_occurrences: 4,
            },
            SpectrumEntry {
                key: canonical(b"AGA"),
                raw_occurrences: 3,
            },
            SpectrumEntry {
                key: canonical(b"ATA"),
                raw_occurrences: 2,
            },
        ];
        entries.sort_unstable_by_key(|entry| entry.key);
        let decision = single_layer_engine(single_edit_config(), &entries)
            .correct_read(CorrectionRead {
                read_ordinal: 501,
                bases: b"AAA",
                phred: &[40, 0, 40],
            })
            .unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(b"ACA".as_slice()));
        assert_eq!(
            decision
                .journal
                .candidate_search_evidence
                .evaluated_candidate_set_digest,
            [
                0xec, 0x10, 0x4a, 0x5a, 0xb8, 0x22, 0x74, 0x04, 0x62, 0xce, 0xcf, 0x4c, 0xce, 0xac,
                0x62, 0x3d, 0x2f, 0x24, 0xf5, 0x39, 0x85, 0x4f, 0xe0, 0xa8, 0x8b, 0x0f, 0x8a, 0x4c,
                0xd9, 0x08, 0x41, 0x6f,
            ]
        );
    }

    #[test]
    fn uniquely_dominating_substitution_is_replayable() {
        let truth = b"AACCGGTTA";
        let k3 = spectrum_entries(3, &[(truth, 20)]);
        let k5 = spectrum_entries(5, &[(truth, 20)]);
        let engine = engine(single_edit_config(), &k3, &k5);
        let read = CorrectionRead {
            read_ordinal: 7,
            bases: b"AATCGGTTA",
            phred: &[40, 40, 3, 40, 40, 40, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(truth.as_slice()));
        assert_eq!(decision.journal.disposition, ReadDisposition::Corrected);
        assert_eq!(decision.journal.substitutions.len(), 1);
        assert_eq!(decision.journal.substitutions[0].position, 2);
        assert_eq!(
            decision.journal.replay_transformation(read).unwrap(),
            decision.emitted_bases
        );
        decision.accounting.validate().unwrap();
    }

    #[test]
    fn two_correlated_substitutions_require_complete_supported_context() {
        let truth = b"AACCGGTTAA";
        let k3 = spectrum_entries(3, &[(truth, 30)]);
        let k5 = spectrum_entries(5, &[(truth, 30)]);
        let config = CorrectionConfig {
            mode: CorrectionMode::ConsensusExperimental,
            limits: CorrectionLimits {
                max_substitutions: 2,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let engine = engine(config, &k3, &k5);
        let read = CorrectionRead {
            read_ordinal: 9,
            bases: b"AATTGGTTAA",
            phred: &[40, 40, 2, 2, 40, 40, 40, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(truth.as_slice()));
        assert_eq!(decision.journal.substitutions.len(), 2);
    }

    #[test]
    fn trusted_minority_path_is_not_rewritten_to_major_path() {
        let major = b"AACCGGTTA";
        let minor = b"AATCGGTTA";
        let k3 = spectrum_entries(3, &[(major, 100), (minor, 5)]);
        let k5 = spectrum_entries(5, &[(major, 100), (minor, 5)]);
        let engine = engine(single_edit_config(), &k3, &k5);
        let read = CorrectionRead {
            read_ordinal: 1,
            bases: minor,
            phred: &[40, 40, 3, 40, 40, 40, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(minor.as_slice()));
        assert_eq!(
            decision.journal.disposition,
            ReadDisposition::Unchanged(UnchangedReason::AlreadyTrusted)
        );
    }

    #[test]
    fn low_quality_correct_base_remains_when_context_is_trusted() {
        let truth = b"AACCGGTTA";
        let alternative = b"AATCGGTTA";
        let k3 = spectrum_entries(3, &[(truth, 8), (alternative, 100)]);
        let k5 = spectrum_entries(5, &[(truth, 8), (alternative, 100)]);
        let engine = engine(single_edit_config(), &k3, &k5);
        let read = CorrectionRead {
            read_ordinal: 2,
            bases: truth,
            phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(truth.as_slice()));
        assert!(matches!(
            decision.journal.disposition,
            ReadDisposition::Unchanged(UnchangedReason::AlreadyTrusted)
        ));
    }

    #[test]
    fn supported_ambiguity_resolution_is_journaled() {
        let truth = b"AACCGGTTA";
        let k3 = spectrum_entries(3, &[(truth, 20)]);
        let k5 = spectrum_entries(5, &[(truth, 20)]);
        let engine = engine(single_edit_config(), &k3, &k5);
        let read = CorrectionRead {
            read_ordinal: 3,
            bases: b"AANCGGTTA",
            phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(truth.as_slice()));
        assert_eq!(decision.journal.substitutions[0].before, b'N');
        assert_eq!(
            decision.journal.substitutions[0].kind,
            BaseEditKind::AmbiguityResolution
        );
    }

    #[test]
    fn equal_exact_evidence_quarantines_instead_of_tie_breaking_sequence() {
        let left = b"AAACCC";
        let right = b"AAGCCC";
        let k3 = spectrum_entries(3, &[(left, 10), (right, 10)]);
        let k5 = spectrum_entries(5, &[(left, 10), (right, 10)]);
        let engine = engine(single_edit_config(), &k3, &k5);
        let read = CorrectionRead {
            read_ordinal: 4,
            bases: b"AANCCC",
            phred: &[40, 40, 0, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert!(matches!(
            decision.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::ConflictingCandidates)
        ));
        assert!(decision.emitted_bases.is_none());
        assert_eq!(decision.accounting.evidence_conflicts, 1);
    }

    #[test]
    fn qualified_alternative_summaries_are_sorted_bounded_and_explicit() {
        let left = b"AAACCC";
        let right = b"AAGCCC";
        let entries = spectrum_entries(3, &[(left, 10), (right, 10)]);
        let config = CorrectionConfig {
            limits: CorrectionLimits {
                max_substitutions: 1,
                max_candidate_summaries_per_read: 1,
                ..single_edit_config().limits
            },
            ..single_edit_config()
        };
        let decision = single_layer_engine(config, &entries)
            .correct_read(CorrectionRead {
                read_ordinal: 18,
                bases: b"AANCCC",
                phred: &[40, 40, 0, 40, 40, 40],
            })
            .unwrap();
        let audit = &decision.journal.candidate_search_evidence;
        assert_eq!(audit.evaluated_candidates, 4);
        assert!(audit.threshold_qualified_candidates > 1);
        assert_eq!(
            audit.evaluated_ambiguity_resolution_candidates,
            audit.evaluated_candidates
        );
        assert_eq!(audit.evaluated_exact_substitution_candidates, 0);
        assert_eq!(audit.retained_alternatives.len(), 1);
        assert!(audit.retained_alternatives_truncated);
        assert_ne!(audit.evaluated_candidate_set_digest, [0; 32]);
        let expected_smallest_identity = [
            b"AAACCC".as_slice(),
            b"AACCCC".as_slice(),
            b"AAGCCC".as_slice(),
            b"AATCCC".as_slice(),
        ]
        .into_iter()
        .map(|candidate| candidate_identity(candidate).unwrap())
        .min()
        .unwrap();
        assert_eq!(
            audit.retained_alternatives[0].candidate_identity,
            expected_smallest_identity
        );
        assert_eq!(
            audit.retained_alternatives[0].changes[0].unwrap().kind,
            BaseEditKind::AmbiguityResolution
        );
    }

    fn synthetic_summary(seed: u8) -> CandidateAlternativeSummary {
        let mut identity = [0_u8; 32];
        identity[0] = seed % 11;
        identity[1] = seed;
        CandidateAlternativeSummary {
            candidate_identity: identity,
            changes: [
                Some(BaseSubstitution {
                    position: u32::from(seed % 7),
                    before: b'N',
                    after: DNA[usize::from(seed % 4)],
                    phred: seed % 42,
                    kind: BaseEditKind::AmbiguityResolution,
                }),
                None,
                None,
            ],
            edit_count: 1,
            threshold_qualified: seed & 1 == 0,
            supporting_k_layers: seed % 3,
            trusted_windows: u64::from(seed),
            unsupported_windows: u64::from(63 - seed),
            summed_occurrence_support: u64::from(seed) * 17,
            affected_low_count_windows: u64::from(seed % 5),
            affected_low_count_support_sum: u64::from(seed % 13),
            affected_candidate_low_count_windows: u64::from(seed % 6),
            affected_candidate_low_count_support_sum: u64::from(seed % 19),
        }
    }

    #[test]
    fn bounded_summary_heap_matches_full_total_order_for_permutations_and_limits() {
        let source = (0_u8..64).map(synthetic_summary).collect::<Vec<_>>();
        let mut permutations = vec![source.clone()];
        let mut reversed = source.clone();
        reversed.reverse();
        permutations.push(reversed);
        let mut rotated = source.clone();
        rotated.rotate_left(23);
        permutations.push(rotated);

        for limit in [1_usize, 2, 3, 7, 16, 31, 64] {
            let mut expected = source.clone();
            expected.sort();
            expected.truncate(limit);
            for permutation in &permutations {
                let mut heap = BoundedAlternativeHeap::new(limit).unwrap();
                for &summary in permutation {
                    heap.consider(summary).unwrap();
                }
                let (observed, comparisons) = heap.into_sorted().unwrap();
                assert_eq!(observed, expected, "summary limit {limit}");
                let projected = projected_summary_comparisons(64, limit as u64).unwrap();
                assert!(comparisons <= projected, "summary limit {limit}");
            }
        }
    }

    #[test]
    fn exact_spectrum_lookup_matches_linear_oracle_and_checked_bound() {
        for entry_count in 0_usize..=64 {
            let entries = (0..entry_count)
                .map(|index| SpectrumEntry {
                    key: PackedKmer::from_u128((index as u128) * 2),
                    raw_occurrences: index as u64 + 1,
                })
                .collect::<Vec<_>>();
            let bound = binary_search_comparison_bound(entry_count).unwrap();
            for query in 0_u128..=(entry_count as u128 * 2 + 1) {
                let mut work = CorrectionWork {
                    projected_spectrum_comparisons: bound,
                    ..CorrectionWork::default()
                };
                let observed =
                    spectrum_support(&entries, PackedKmer::from_u128(query), &mut work).unwrap();
                let expected = entries
                    .iter()
                    .find(|entry| entry.key == PackedKmer::from_u128(query))
                    .map_or(0, |entry| entry.raw_occurrences);
                assert_eq!(observed, expected, "entries={entry_count}, query={query}");
                assert!(work.spectrum_comparisons <= bound);
                validate_work(work).unwrap();
            }
        }
    }

    #[test]
    fn spectrum_entry_admission_is_inclusive_and_authenticated() {
        let entries = exact_spectrum_entries(&[b"AAA", b"AAC"]);
        assert_eq!(entries.len(), 2);
        let layers = [TrustedSpectrum {
            k: 3,
            min_trusted_occurrences: 2,
            entries: &entries,
        }];
        let accepted = CorrectionConfig {
            limits: CorrectionLimits {
                max_spectrum_entries_total: 2,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        QualityCorrectionEngine::new(accepted, source_descriptor(), &layers).unwrap();

        let rejected = CorrectionConfig {
            limits: CorrectionLimits {
                max_spectrum_entries_total: 1,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let error =
            QualityCorrectionEngine::new(rejected, source_descriptor(), &layers).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceRetainedKeys);
        assert_ne!(config_identity(accepted), config_identity(rejected));
    }

    #[test]
    fn comparison_and_auxiliary_work_caps_are_inclusive_and_journaled() {
        let truth = b"AACCGGTTA";
        let entries = spectrum_entries(3, &[(truth, 20)]);
        let read = CorrectionRead {
            read_ordinal: 51,
            bases: b"AANCGGTTA",
            phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
        };
        let probe = single_layer_engine(single_edit_config(), &entries);
        let baseline_bound = probe
            .baseline_spectrum_comparison_bound(read.bases.len())
            .unwrap();
        let candidates = projected_candidate_count(read, 20, 1).unwrap();
        assert_eq!(candidates, 4);
        let spectrum_projection =
            projected_spectrum_comparisons(baseline_bound, candidates).unwrap();
        let summary_projection = projected_summary_comparisons(candidates, candidates).unwrap();
        let work_projection =
            projected_search_work_units(read.bases.len(), 1, 1, candidates).unwrap();

        let baseline_rejected = CorrectionConfig {
            mode: CorrectionMode::ConsensusExperimental,
            limits: CorrectionLimits {
                max_substitutions: 1,
                max_spectrum_comparisons_per_read: baseline_bound - 1,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let decision = single_layer_engine(baseline_rejected, &entries)
            .correct_read(read)
            .unwrap();
        assert!(matches!(
            decision.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::CapExceeded {
                kind: CapKind::SpectrumComparisons,
                projected,
                limit,
            }) if projected == baseline_bound && limit == baseline_bound - 1
        ));
        assert!(decision.journal.baseline_evidence.is_none());
        assert_eq!(decision.journal.spectrum_comparisons, 0);

        for (kind, spectrum_limit, summary_limit, work_limit, projected) in [
            (
                CapKind::SpectrumComparisons,
                spectrum_projection - 1,
                summary_projection,
                work_projection,
                spectrum_projection,
            ),
            (
                CapKind::SummaryComparisons,
                spectrum_projection,
                summary_projection - 1,
                work_projection,
                summary_projection,
            ),
            (
                CapKind::SearchWorkUnits,
                spectrum_projection,
                summary_projection,
                work_projection - 1,
                work_projection,
            ),
        ] {
            let config = CorrectionConfig {
                mode: CorrectionMode::ConsensusExperimental,
                limits: CorrectionLimits {
                    max_substitutions: 1,
                    max_spectrum_comparisons_per_read: spectrum_limit,
                    max_summary_comparisons_per_read: summary_limit,
                    max_search_work_units_per_read: work_limit,
                    ..CorrectionConfig::default().limits
                },
                ..CorrectionConfig::default()
            };
            let capped = single_layer_engine(config, &entries)
                .correct_read(read)
                .unwrap();
            assert!(matches!(
                capped.journal.disposition,
                ReadDisposition::Quarantined(QuarantineReason::CapExceeded {
                    kind: observed,
                    projected: observed_projection,
                    limit,
                }) if observed == kind && observed_projection == projected && limit == projected - 1
            ));
            assert!(capped.journal.baseline_evidence.is_some());
            assert_eq!(capped.journal.evaluated_candidates, 0);
        }

        let admitted = CorrectionConfig {
            mode: CorrectionMode::ConsensusExperimental,
            limits: CorrectionLimits {
                max_substitutions: 1,
                max_spectrum_comparisons_per_read: spectrum_projection,
                max_summary_comparisons_per_read: summary_projection,
                max_search_work_units_per_read: work_projection,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let decision = single_layer_engine(admitted, &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(
            decision.journal.projected_spectrum_comparisons,
            spectrum_projection
        );
        assert!(decision.journal.spectrum_comparisons <= spectrum_projection);
        assert_eq!(
            decision.journal.projected_summary_comparisons,
            summary_projection
        );
        assert_eq!(
            decision.accounting.summary_comparisons,
            decision
                .journal
                .candidate_search_evidence
                .retained_alternative_comparisons
        );
        assert!(decision.accounting.summary_comparisons <= summary_projection);
        assert_eq!(
            decision.journal.projected_search_work_units,
            work_projection
        );
        decision.accounting.validate().unwrap();
    }

    #[test]
    fn every_new_resource_limit_changes_configuration_identity_and_rejects_zero() {
        let entries = spectrum_entries(3, &[(b"AACCG".as_slice(), 2)]);
        let layers = [TrustedSpectrum {
            k: 3,
            min_trusted_occurrences: 2,
            entries: &entries,
        }];
        let baseline = CorrectionConfig::default();
        for index in 0..4 {
            let mut changed_limits = baseline.limits;
            match index {
                0 => changed_limits.max_spectrum_entries_total += 1,
                1 => changed_limits.max_spectrum_comparisons_per_read += 1,
                2 => changed_limits.max_summary_comparisons_per_read += 1,
                3 => changed_limits.max_search_work_units_per_read += 1,
                _ => unreachable!(),
            }
            let changed = CorrectionConfig {
                limits: changed_limits,
                ..baseline
            };
            assert_ne!(config_identity(baseline), config_identity(changed));

            let mut zero_limits = baseline.limits;
            match index {
                0 => zero_limits.max_spectrum_entries_total = 0,
                1 => zero_limits.max_spectrum_comparisons_per_read = 0,
                2 => zero_limits.max_summary_comparisons_per_read = 0,
                3 => zero_limits.max_search_work_units_per_read = 0,
                _ => unreachable!(),
            }
            let zero = CorrectionConfig {
                limits: zero_limits,
                ..baseline
            };
            let error =
                QualityCorrectionEngine::new(zero, source_descriptor(), &layers).unwrap_err();
            assert_eq!(error.code(), ErrorCode::ConfigurationInvalidLimit);
        }
    }

    #[test]
    fn conservative_two_pass_projection_can_quarantine_before_a_one_pass_abstention() {
        let entries = exact_spectrum_entries(&[b"AAA"]);
        let read = CorrectionRead {
            read_ordinal: 52,
            bases: b"AANCCC",
            phred: &[40, 40, 0, 40, 40, 40],
        };
        let candidates = projected_candidate_count(read, 20, 1).unwrap();
        let baseline_windows = 4_u64;
        let one_pass_windows = baseline_windows * (1 + candidates);
        let two_pass_windows = baseline_windows * (1 + 2 * candidates);
        let config = CorrectionConfig {
            mode: CorrectionMode::ConsensusExperimental,
            limits: CorrectionLimits {
                max_substitutions: 1,
                max_window_evaluations_per_read: one_pass_windows,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let decision = single_layer_engine(config, &entries)
            .correct_read(read)
            .unwrap();
        assert!(matches!(
            decision.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::CapExceeded {
                kind: CapKind::WindowEvaluations,
                projected,
                limit,
            }) if projected == two_pass_windows && limit == one_pass_windows
        ));
        assert!(decision.journal.baseline_evidence.is_some());
        assert_eq!(decision.journal.evaluated_candidates, 0);
    }

    #[test]
    fn default_quarantines_reads_shorter_than_every_configured_k() {
        let entries = exact_spectrum_entries(&[b"AAA"]);
        let read = CorrectionRead {
            read_ordinal: 53,
            bases: b"AA",
            phred: &[40, 40],
        };
        let decision = single_layer_engine(CorrectionConfig::default(), &entries)
            .correct_read(read)
            .unwrap();
        assert_eq!(
            decision.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::UnresolvedEvidence)
        );
        let baseline = decision.journal.baseline_evidence.as_ref().unwrap();
        assert_eq!(baseline.trusted_windows, 0);
        assert_eq!(baseline.unsupported_windows, 0);
        assert_eq!(decision.journal.window_evaluations, 0);
        assert!(decision.emitted_bases.is_none());
    }

    #[test]
    fn candidate_cap_quarantines_before_scoring() {
        let truth = b"AACCGGTTA";
        let k3 = spectrum_entries(3, &[(truth, 20)]);
        let k5 = spectrum_entries(5, &[(truth, 20)]);
        let config = CorrectionConfig {
            limits: CorrectionLimits {
                max_candidates_per_read: 2,
                max_substitutions: 1,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let engine = engine(config, &k3, &k5);
        let read = CorrectionRead {
            read_ordinal: 5,
            bases: b"AANCGGTTA",
            phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert!(matches!(
            decision.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::CapExceeded {
                kind: CapKind::Candidates,
                projected: 4,
                limit: 2
            })
        ));
        assert_eq!(decision.journal.window_evaluations, 12);
        assert!(decision.journal.baseline_evidence.is_some());
        assert!(decision.journal.candidate_projection_performed);
        assert_eq!(decision.journal.evaluated_candidates, 0);
    }

    #[test]
    fn modeled_per_read_memory_limit_is_inclusive_at_the_boundary() {
        let truth = b"AACCGGTTA";
        let k3 = spectrum_entries(3, &[(truth, 20)]);
        let k5 = spectrum_entries(5, &[(truth, 20)]);
        let read = CorrectionRead {
            read_ordinal: 5,
            bases: b"AANCGGTTA",
            phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
        };
        let projected = projected_read_scratch_bytes(read.bases.len(), 1, 2, 1, 4).unwrap();
        let at_boundary = CorrectionConfig {
            limits: CorrectionLimits {
                max_substitutions: 1,
                max_accounted_bytes_per_read: projected,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let accepted = engine(at_boundary, &k3, &k5).correct_read(read).unwrap();
        assert!(!matches!(
            accepted.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::CapExceeded {
                kind: CapKind::AccountedBytes,
                ..
            })
        ));

        let below_boundary = CorrectionConfig {
            limits: CorrectionLimits {
                max_substitutions: 1,
                max_accounted_bytes_per_read: projected - 1,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let capped = engine(below_boundary, &k3, &k5).correct_read(read).unwrap();
        assert!(matches!(
            capped.journal.disposition,
            ReadDisposition::Quarantined(QuarantineReason::CapExceeded {
                kind: CapKind::AccountedBytes,
                projected: observed,
                limit
            }) if observed == projected && limit == projected - 1
        ));
    }

    #[test]
    fn explicit_terminal_trim_is_losslessly_replayable() {
        let truth = b"ACCG";
        let k3 = spectrum_entries(3, &[(truth, 10)]);
        let config = CorrectionConfig {
            unresolved_policy: UnresolvedPolicy::TrimLowQualityTerminals {
                max_terminal_phred: 30,
                min_retained_bases: 4,
            },
            limits: CorrectionLimits {
                max_substitutions: 1,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let engine = QualityCorrectionEngine::new(
            config,
            source_descriptor(),
            &[TrustedSpectrum {
                k: 3,
                min_trusted_occurrences: 2,
                entries: &k3,
            }],
        )
        .unwrap();
        let read = CorrectionRead {
            read_ordinal: 6,
            bases: b"NACCGN",
            phred: &[30, 40, 40, 40, 40, 30],
        };
        let decision = engine.correct_read(read).unwrap();
        assert_eq!(decision.emitted_bases.as_deref(), Some(truth.as_slice()));
        assert!(matches!(
            decision.journal.disposition,
            ReadDisposition::Trimmed { .. }
        ));
        assert_eq!(
            decision.journal.replay_transformation(read).unwrap(),
            decision.emitted_bases
        );
        assert_eq!(decision.accounting.trimmed_bases, 2);
    }

    #[test]
    fn noncanonical_spectrum_key_is_rejected() {
        let noncanonical = encode_exact_bases(b"TTT").unwrap();
        assert_ne!(canonical_code(noncanonical, 3).unwrap(), noncanonical);
        let error = QualityCorrectionEngine::new(
            CorrectionConfig::default(),
            source_descriptor(),
            &[TrustedSpectrum {
                k: 3,
                min_trusted_occurrences: 1,
                entries: &[SpectrumEntry {
                    key: noncanonical,
                    raw_occurrences: 1,
                }],
            }],
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
    }

    #[test]
    fn unbound_source_descriptor_is_rejected() {
        let entries = spectrum_entries(3, &[(b"AACCG".as_slice(), 2)]);
        let source = CorrectionSourceDescriptor {
            source_snapshot_sha256: [0; 32],
            ..source_descriptor()
        };
        let error = QualityCorrectionEngine::new(
            CorrectionConfig::default(),
            source,
            &[TrustedSpectrum {
                k: 3,
                min_trusted_occurrences: 2,
                entries: &entries,
            }],
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityArtifact);
    }

    #[test]
    fn zero_journal_limit_is_rejected_at_engine_construction() {
        let entries = spectrum_entries(3, &[(b"AACCG".as_slice(), 2)]);
        let config = CorrectionConfig {
            limits: CorrectionLimits {
                max_journal_entries_per_read: 0,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let error = QualityCorrectionEngine::new(
            config,
            source_descriptor(),
            &[TrustedSpectrum {
                k: 3,
                min_trusted_occurrences: 2,
                entries: &entries,
            }],
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigurationInvalidLimit);
    }

    #[test]
    fn zero_candidate_summary_limit_is_rejected_at_engine_construction() {
        let entries = spectrum_entries(3, &[(b"AACCG".as_slice(), 2)]);
        let config = CorrectionConfig {
            limits: CorrectionLimits {
                max_candidate_summaries_per_read: 0,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        let error = QualityCorrectionEngine::new(
            config,
            source_descriptor(),
            &[TrustedSpectrum {
                k: 3,
                min_trusted_occurrences: 2,
                entries: &entries,
            }],
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigurationInvalidLimit);
    }

    #[test]
    fn batch_is_order_and_worker_count_deterministic() {
        let truth = b"AACCGGTTA";
        let k3 = spectrum_entries(3, &[(truth, 20)]);
        let k5 = spectrum_entries(5, &[(truth, 20)]);
        let engine = engine(single_edit_config(), &k3, &k5);
        let quality = [40, 40, 2, 40, 40, 40, 40, 40, 40];
        let first = CorrectionRead {
            read_ordinal: 1,
            bases: b"AATCGGTTA",
            phred: &quality,
        };
        let second = CorrectionRead {
            read_ordinal: 2,
            bases: truth,
            phred: &quality,
        };
        let one = engine.correct_batch(&[second, first], 1).unwrap();
        let four = engine.correct_batch(&[first, second], 4).unwrap();
        assert_eq!(one, four);
        assert_eq!(one.decisions[0].read_ordinal, 1);
        one.accounting.validate().unwrap();
    }

    #[test]
    fn batch_projection_is_enforced_before_ordinal_order_allocation() {
        let truth = b"AACCGGTTA";
        let entries = spectrum_entries(3, &[(truth, 20)]);
        let config = CorrectionConfig {
            limits: CorrectionLimits {
                max_batch_accounted_bytes: 1,
                ..single_edit_config().limits
            },
            ..single_edit_config()
        };
        let engine = single_layer_engine(config, &entries);
        let quality = [40; 9];
        let duplicate = CorrectionRead {
            read_ordinal: 7,
            bases: truth,
            phred: &quality,
        };
        let error = engine
            .correct_batch(&[duplicate, duplicate], 1)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn transformation_replay_is_panic_free_and_engine_verification_is_complete() {
        let truth = b"AACCGGTTA";
        let entries = spectrum_entries(3, &[(truth, 20)]);
        let engine = single_layer_engine(single_edit_config(), &entries);
        let read = CorrectionRead {
            read_ordinal: 0,
            bases: b"AATCGGTTA",
            phred: &[40, 40, 0, 40, 40, 40, 40, 40, 40],
        };
        let decision = engine.correct_read(read).unwrap();
        assert_eq!(
            engine.verify_decision_and_replay(read, &decision).unwrap(),
            decision.emitted_bases
        );

        let malformed = CorrectionRead {
            read_ordinal: 0,
            bases: b"AATCGGTTA",
            phred: &[],
        };
        let error = decision
            .journal
            .replay_transformation(malformed)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputFastqStructure);

        let mut forged = decision.clone();
        forged.journal.candidate_score_evaluations += 1;
        let error = engine
            .verify_decision_and_replay(read, &forged)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityArtifact);

        let mut forged_search_evidence = decision.clone();
        forged_search_evidence
            .journal
            .candidate_search_evidence
            .evaluated_candidate_set_digest[0] ^= 1;
        let error = engine
            .verify_decision_and_replay(read, &forged_search_evidence)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityArtifact);

        let mut forged_alternative = decision.clone();
        forged_alternative
            .journal
            .candidate_search_evidence
            .retained_alternatives[0]
            .affected_low_count_windows += 1;
        let error = engine
            .verify_decision_and_replay(read, &forged_alternative)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityArtifact);

        let other_source = CorrectionSourceDescriptor {
            source_snapshot_sha256: [9; 32],
            ..source_descriptor()
        };
        let layers = [TrustedSpectrum {
            k: 3,
            min_trusted_occurrences: 2,
            entries: &entries,
        }];
        let other_engine =
            QualityCorrectionEngine::new(single_edit_config(), other_source, &layers).unwrap();
        let error = other_engine
            .verify_decision_and_replay(read, &decision)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityArtifact);

        let other_config = CorrectionConfig {
            limits: CorrectionLimits {
                max_substitutions: 2,
                ..single_edit_config().limits
            },
            ..single_edit_config()
        };
        let other_engine =
            QualityCorrectionEngine::new(other_config, source_descriptor(), &layers).unwrap();
        let error = other_engine
            .verify_decision_and_replay(read, &decision)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityArtifact);
    }

    #[test]
    fn dominance_dimensions_are_consistent_with_candidate_ranking() {
        let layer = |trusted, unsupported, raw, affected| LayerEvidence {
            k: 3,
            possible_windows: trusted + unsupported,
            exact_windows: trusted,
            trusted_windows: trusted,
            unsupported_windows: unsupported,
            summed_occurrence_support: raw,
            affected_windows: affected,
            affected_trusted_windows: affected,
            affected_low_count_windows: 0,
            affected_low_count_support_sum: 0,
            affected_candidate_low_count_windows: 0,
            affected_candidate_low_count_support_sum: 0,
        };
        let evidence = |trusted, unsupported, raw, affected| CandidateEvidence {
            edit_count: 1,
            quality_penalty: 0,
            supporting_k_layers: 1,
            trusted_windows: trusted,
            unsupported_windows: unsupported,
            summed_occurrence_support: raw,
            affected_low_count_windows: 0,
            affected_low_count_support_sum: 0,
            affected_candidate_low_count_windows: 0,
            affected_candidate_low_count_support_sum: 0,
            layers: vec![layer(trusted, unsupported, raw, affected)],
        };
        let dominant = evidence(3, 0, 6, 1);
        let inferior = evidence(2, 1, 4, 100);
        let current = RankedCandidate {
            bases: b"TAAAA".to_vec(),
            changes: Vec::new(),
            evidence: inferior.clone(),
        };
        assert!(strictly_dominates(&dominant, &inferior));
        assert_eq!(rank_cmp(&dominant, b"AAAAA", &current), Ordering::Greater);

        let geometric_tie = evidence(3, 0, 6, 100);
        assert!(!strictly_dominates(&dominant, &geometric_tie));
        assert!(!strictly_dominates(&geometric_tie, &dominant));
    }

    fn symbols_to_dna(symbols: &[u8]) -> Vec<u8> {
        symbols
            .iter()
            .map(|symbol| DNA[usize::from(symbol % 4)])
            .collect()
    }

    /// Independent single-edit oracle: literal-string reverse complements,
    /// base-4 candidate iteration, and a direct BTreeMap lookup. It does not
    /// call the production enumerator, scorer, threshold gate, or dominance
    /// relation.
    fn brute_force_one_edit(
        read: &[u8],
        editable: usize,
        support: &BTreeMap<Vec<u8>, u64>,
    ) -> Option<Vec<u8>> {
        fn canonical_string(window: &[u8]) -> Vec<u8> {
            let reverse: Vec<u8> = window
                .iter()
                .rev()
                .map(|base| match base {
                    b'A' => b'T',
                    b'C' => b'G',
                    b'G' => b'C',
                    b'T' => b'A',
                    _ => b'N',
                })
                .collect();
            window.to_vec().min(reverse)
        }
        fn score(sequence: &[u8], support: &BTreeMap<Vec<u8>, u64>) -> (u64, u64, u64) {
            sequence.windows(3).fold((0, 0, 0), |mut total, window| {
                let count = support.get(&canonical_string(window)).copied().unwrap_or(0);
                total.0 += u64::from(count >= 2);
                total.1 += u64::from(count < 2);
                total.2 += count;
                total
            })
        }
        let baseline = score(read, support);
        if baseline.0 > 0 && baseline.1 == 0 {
            return Some(read.to_vec());
        }
        let mut qualified = Vec::new();
        for replacement in DNA {
            if replacement == read[editable] {
                continue;
            }
            let mut candidate = read.to_vec();
            candidate[editable] = replacement;
            let evidence = score(&candidate, support);
            let affected_start = editable.saturating_sub(2);
            let affected_end = editable.min(read.len() - 3);
            let affected_all_trusted = (affected_start..=affected_end).all(|start| {
                support
                    .get(&canonical_string(&candidate[start..start + 3]))
                    .copied()
                    .unwrap_or(0)
                    >= 2
            });
            if evidence.0 > baseline.0
                && evidence.1 <= baseline.1
                && evidence.2 > baseline.2
                && affected_all_trusted
            {
                qualified.push((candidate, evidence));
            }
        }
        let mut winners = Vec::new();
        for index in 0..qualified.len() {
            let (_, score) = &qualified[index];
            let dominates_all = qualified
                .iter()
                .enumerate()
                .all(|(other_index, (_, other))| {
                    index == other_index
                        || (score.0 >= other.0
                            && score.1 <= other.1
                            && score.2 >= other.2
                            && (score.0 > other.0 || score.1 < other.1 || score.2 > other.2))
                });
            if dominates_all {
                winners.push(qualified[index].0.clone());
            }
        }
        (winners.len() == 1).then(|| winners.remove(0))
    }

    fn literal_canonical(window: &[u8]) -> Vec<u8> {
        let reverse = window
            .iter()
            .rev()
            .map(|base| match base {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => b'N',
            })
            .collect::<Vec<_>>();
        window.to_vec().min(reverse)
    }

    fn base4_sequence(mut code: usize, length: usize) -> Vec<u8> {
        let mut sequence = vec![b'A'; length];
        for base in sequence.iter_mut().rev() {
            *base = DNA[code % 4];
            code /= 4;
        }
        sequence
    }

    /// Independent exhaustive oracle for the complete exact-base Hamming
    /// neighborhood at one k. It uses literal strings and its own threshold
    /// and Pareto implementation.
    fn exhaustive_oracle_k3(
        observed: &[u8],
        support: &BTreeMap<Vec<u8>, u64>,
        max_substitutions: usize,
    ) -> Option<Vec<u8>> {
        let score = |sequence: &[u8]| {
            let count = support
                .get(&literal_canonical(sequence))
                .copied()
                .unwrap_or(0);
            (u64::from(count >= 2), u64::from(count < 2), count)
        };
        let baseline = score(observed);
        if baseline.0 > 0 && baseline.1 == 0 {
            return Some(observed.to_vec());
        }
        let mut qualified = Vec::new();
        for code in 0..64 {
            let candidate = base4_sequence(code, 3);
            let edits = candidate
                .iter()
                .zip(observed)
                .filter(|(left, right)| left != right)
                .count();
            if edits == 0 || edits > max_substitutions {
                continue;
            }
            let evidence = score(&candidate);
            if evidence.0 >= baseline.0
                && evidence.1 <= baseline.1
                && evidence.2 >= baseline.2
                && evidence.0 - baseline.0 >= 1
                && evidence.2 - baseline.2 >= 1
                && evidence.0 == 1
            {
                qualified.push((candidate, evidence, edits));
            }
        }
        let dominates = |left: &(Vec<u8>, (u64, u64, u64), usize),
                         right: &(Vec<u8>, (u64, u64, u64), usize)| {
            left.1 .0 >= right.1 .0
                && left.1 .1 <= right.1 .1
                && left.1 .2 >= right.1 .2
                && left.2 <= right.2
                && (left.1 != right.1 || left.2 < right.2)
        };
        let winners = qualified
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                qualified.iter().enumerate().all(|(other_index, other)| {
                    *index == other_index || dominates(candidate, other)
                })
            })
            .map(|(_, candidate)| candidate.0.clone())
            .collect::<Vec<_>>();
        (winners.len() == 1).then(|| winners[0].clone())
    }

    #[test]
    fn exhaustive_exact_three_base_domain_matches_independent_oracle() {
        let config = CorrectionConfig {
            mode: CorrectionMode::ConsensusExperimental,
            limits: CorrectionLimits {
                max_substitutions: 2,
                ..CorrectionConfig::default().limits
            },
            ..CorrectionConfig::default()
        };
        for truth_code in 0..64 {
            let truth = base4_sequence(truth_code, 3);
            let entries = spectrum_entries(3, &[(&truth, 20)]);
            let engine = single_layer_engine(config, &entries);
            let mut support = BTreeMap::new();
            support.insert(literal_canonical(&truth), 20);
            for observed_code in 0..64 {
                let observed = base4_sequence(observed_code, 3);
                let quality = [0_u8; 3];
                let expected = exhaustive_oracle_k3(&observed, &support, 2);
                let decision = engine
                    .correct_read(CorrectionRead {
                        read_ordinal: observed_code as u64,
                        bases: &observed,
                        phred: &quality,
                    })
                    .unwrap();
                assert_eq!(
                    decision.emitted_bases,
                    expected,
                    "truth={:?}, observed={:?}",
                    String::from_utf8_lossy(&truth),
                    String::from_utf8_lossy(&observed)
                );
                if decision.journal.candidate_projection_performed {
                    assert_eq!(decision.journal.projected_candidates, 36);
                    assert!(decision.journal.evaluated_candidates <= 36);
                }
            }
        }
    }

    proptest! {
        #[test]
        fn production_decision_matches_independent_bruteforce_oracle(
            symbols in prop::collection::vec(0_u8..4, 7..13),
            raw_position in any::<usize>(),
        ) {
            let truth = symbols_to_dna(&symbols);
            let position = raw_position % truth.len();
            let mut observed = truth.clone();
            observed[position] = DNA[(usize::from(symbols[position]) + 1) % 4];
            let mut quality = vec![40_u8; truth.len()];
            quality[position] = 0;
            let entries = spectrum_entries(3, &[(&truth, 20)]);
            let spectrum = [TrustedSpectrum {
                k: 3,
                min_trusted_occurrences: 2,
                entries: &entries,
            }];
            let engine = QualityCorrectionEngine::new(
                single_edit_config(),
                source_descriptor(),
                &spectrum,
            ).unwrap();
            let decision = engine.correct_read(CorrectionRead {
                read_ordinal: 0,
                bases: &observed,
                phred: &quality,
            }).unwrap();
            let mut support = BTreeMap::new();
            for window in truth.windows(3) {
                let reverse: Vec<_> = window.iter().rev().map(|base| match base {
                    b'A' => b'T', b'C' => b'G', b'G' => b'C', b'T' => b'A', _ => b'N'
                }).collect();
                let key = window.to_vec().min(reverse);
                *support.entry(key).or_insert(0_u64) += 20;
            }
            let expected = brute_force_one_edit(&observed, position, &support);
            match expected {
                Some(sequence) => prop_assert_eq!(decision.emitted_bases.as_deref(), Some(sequence.as_slice())),
                None => prop_assert!(matches!(decision.journal.disposition, ReadDisposition::Quarantined(_))),
            }
        }
    }
}
