//! Transactional reporting for the authenticated experimental multi-k portfolio.
//!
//! This module accepts reporting snapshots, not evidence capabilities.  Only
//! [`super::multik_pipeline`] may translate the opaque source-backed capability
//! chain into these one-way records.  A snapshot, FASTA, GFA, TSV, JSON, HTML,
//! or checksum manifest can never be promoted back into reconstruction input.
//!
//! ```compile_fail
//! use veritasm::experimental::multik_bundle::UnverifiedChildSnapshotPayload;
//! let _ = std::mem::size_of::<UnverifiedChildSnapshotPayload>();
//! ```

use crate::bundle::{verify_bundle_manifest_with_limits, BundleManifestLimits, RunLease};
use crate::error::{ErrorCode, Result, VeritasmError};
use flate2::read::GzDecoder;
use flate2::{Compression, GzBuilder};
use rustix::fs::{renameat_with, RenameFlags, CWD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const SNAPSHOT_SCHEMA: &str = "veritasm-experimental-multik-child-snapshot-v2";
const RUN_SCHEMA: &str = "veritasm-experimental-multik-run-v2";
const PARENT_ROOT_DOMAIN: &[u8] = b"veritasm:multik-parent:v3\0";
const OPERATIONAL_ROOT_DOMAIN: &[u8] = b"veritasm:multik-operational:v2\0";
const PAIR_REPORT_ROOT_DOMAIN: &[u8] = b"veritasm:multik-pair-report:v1\0";
const AGREEMENT_ROOT_DOMAIN: &[u8] = b"veritasm:multik-exact-agreement:v1\0";
const PRESENTATION_ID_DOMAIN: &[u8] = b"veritasm:multik-presentation-id:v1\0";
const SEGMENT_ID_DOMAIN: &[u8] = b"veritasm:read-witnessed-segment-id:v1\0";
const SEGMENT_PROVENANCE_DOMAIN: &[u8] = b"veritasm:read-witnessed-segment-provenance:v1\0";
const TRANSITION_SET_DOMAIN: &[u8] = b"veritasm:read-witnessed-segment-transitions:v1\0";
const SCIENTIFIC_LABEL: &str = "experimental; unqualified; research use only";
pub(crate) const SNAPSHOT_CODEC_MEMORY_BYTES: u64 = 1 << 20;

const RUN_SCHEMA_JSON: &[u8] = br##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://example.invalid/veritasm/experimental-multik-run-v2.schema.json",
  "title": "VeritAsm experimental multi-k run",
  "type": "object",
  "required": ["schema", "status", "qualification", "intended_use", "source_root", "parent_root", "operational_root", "exact_agreement_root", "profile", "pair_evidence", "quality_correction", "input_mode", "support_unit", "min_base_quality", "execution", "children", "totals", "limitations"],
  "properties": {
    "schema": {"const": "veritasm-experimental-multik-run-v2"},
    "status": {"const": "experimental"},
    "qualification": {"const": "unqualified"},
    "intended_use": {"const": "research_use_only"},
    "source_root": {"$ref": "#/$defs/digest"},
    "parent_root": {"$ref": "#/$defs/digest"},
    "operational_root": {"$ref": "#/$defs/digest"},
    "exact_agreement_root": {"$ref": "#/$defs/digest"},
    "profile": {"enum": ["diversity_preserving", "exact_agreement_consensus"]},
    "pair_evidence": {"enum": ["not_applicable_single_end", "unavailable_resource_limit", "authenticated_linear_unitig_only_unqualified"]},
    "quality_correction": {"const": "disabled_unqualified"},
    "input_mode": {"enum": ["single_end", "paired_end"]},
    "support_unit": {"enum": ["supplied_fragment_instance", "accepted_window_occurrence"]},
    "min_base_quality": {"type": "integer", "minimum": 0, "maximum": 93},
    "execution": {
      "type": "object",
      "required": ["execution_threads", "accounting_scope", "max_aggregate_accounted_memory_bytes", "projected_aggregate_accounted_memory_bytes", "max_aggregate_temp_bytes", "max_final_staging_bytes"],
      "properties": {
        "execution_threads": {"const": 1},
        "accounting_scope": {"const": "modelled_owned_payload_not_process_rss"},
        "max_aggregate_accounted_memory_bytes": {"$ref": "#/$defs/positive"},
        "projected_aggregate_accounted_memory_bytes": {"$ref": "#/$defs/positive"},
        "max_aggregate_temp_bytes": {"$ref": "#/$defs/positive"},
        "max_final_staging_bytes": {"$ref": "#/$defs/positive"}
      },
      "additionalProperties": false
    },
    "children": {
      "type": "array", "minItems": 1,
      "items": {
        "type": "object",
        "required": ["k", "q", "minimizer_length", "virtual_bucket_count", "source_equivalence_root", "retention_root", "retention_rule", "retention_minimum_support", "transition_root", "exact_edge_table_sha256", "raw_compacted_graph_root", "constrained_graph_root", "child_root", "child_authentication_root", "raw_keys", "retained_keys", "discarded_keys", "raw_support", "retained_support", "discarded_support", "topology_candidates", "admitted_topology_candidates", "excluded_no_original_read_witness", "ledger_rows", "eligible_ledger_rows", "excluded_endpoint_not_retained", "segments", "linear_segments", "closed_segments", "witnessed_links", "output_bases", "accounted_peak_bytes", "pair_evidence"],
        "properties": {
          "k": {"type": "integer", "minimum": 3, "maximum": 63}, "q": {"type": "integer", "minimum": 4, "maximum": 64},
          "minimizer_length": {"type": "integer", "minimum": 1, "maximum": 63}, "virtual_bucket_count": {"type": "integer", "minimum": 1},
          "source_equivalence_root": {"$ref": "#/$defs/digest"}, "retention_root": {"$ref": "#/$defs/digest"}, "transition_root": {"$ref": "#/$defs/digest"},
          "retention_rule": {"enum": ["retain_all", "inclusive_support"]}, "retention_minimum_support": {"type": ["integer", "null"], "minimum": 1},
          "exact_edge_table_sha256": {"$ref": "#/$defs/digest"}, "raw_compacted_graph_root": {"$ref": "#/$defs/digest"}, "constrained_graph_root": {"$ref": "#/$defs/digest"}, "child_root": {"$ref": "#/$defs/digest"}, "child_authentication_root": {"$ref": "#/$defs/digest"},
          "raw_keys": {"$ref": "#/$defs/count"}, "retained_keys": {"$ref": "#/$defs/count"}, "discarded_keys": {"$ref": "#/$defs/count"},
          "raw_support": {"$ref": "#/$defs/count"}, "retained_support": {"$ref": "#/$defs/count"}, "discarded_support": {"$ref": "#/$defs/count"},
          "topology_candidates": {"$ref": "#/$defs/count"}, "admitted_topology_candidates": {"$ref": "#/$defs/count"}, "excluded_no_original_read_witness": {"$ref": "#/$defs/count"},
          "ledger_rows": {"$ref": "#/$defs/count"}, "eligible_ledger_rows": {"$ref": "#/$defs/count"}, "excluded_endpoint_not_retained": {"$ref": "#/$defs/count"},
          "segments": {"$ref": "#/$defs/count"}, "linear_segments": {"$ref": "#/$defs/count"}, "closed_segments": {"$ref": "#/$defs/count"},
          "witnessed_links": {"$ref": "#/$defs/count"}, "output_bases": {"$ref": "#/$defs/count"}, "accounted_peak_bytes": {"$ref": "#/$defs/count"},
          "pair_evidence": {
            "type": "object",
            "required": ["state", "placement_domain", "report_root", "exact_pair_graph_root", "placement_producer_root", "authenticated_path_result_root", "supplied_fragments", "configured_fragment_limit", "authenticated_fragments", "calibration_fragments", "replay_fragments", "unavailable_possible_graph_junction_reads", "supported_unique_existing_paths", "trivial_within_unitig", "abstained", "indeterminate", "available_aggregate_constraints", "lane_models", "changes_sequence_or_graph"],
            "properties": {
              "state": {"enum": ["not_applicable_single_end", "unavailable_resource_limit", "authenticated_linear_unitig_only_unqualified"]},
              "placement_domain": {"enum": ["not_applicable", "unavailable_resource_limit", "linear_unitig_only"]},
              "report_root": {"$ref": "#/$defs/digest"},
              "exact_pair_graph_root": {"anyOf": [{"$ref": "#/$defs/digest"}, {"type": "null"}]},
              "placement_producer_root": {"anyOf": [{"$ref": "#/$defs/digest"}, {"type": "null"}]},
              "authenticated_path_result_root": {"anyOf": [{"$ref": "#/$defs/digest"}, {"type": "null"}]},
              "supplied_fragments": {"$ref": "#/$defs/count"}, "configured_fragment_limit": {"$ref": "#/$defs/count"}, "authenticated_fragments": {"$ref": "#/$defs/count"}, "calibration_fragments": {"$ref": "#/$defs/count"}, "replay_fragments": {"$ref": "#/$defs/count"},
              "unavailable_possible_graph_junction_reads": {"$ref": "#/$defs/count"}, "supported_unique_existing_paths": {"$ref": "#/$defs/count"}, "trivial_within_unitig": {"$ref": "#/$defs/count"}, "abstained": {"$ref": "#/$defs/count"}, "indeterminate": {"$ref": "#/$defs/count"}, "available_aggregate_constraints": {"$ref": "#/$defs/count"},
              "lane_models": {"type": "array"}, "changes_sequence_or_graph": {"const": false}
            },
            "additionalProperties": false
          }
        },
        "additionalProperties": false
      }
    },
    "totals": {
      "type": "object",
      "required": ["children", "child_segments", "child_links", "adjacency_rows", "transition_decisions", "child_output_bases", "exact_agreement_presentations"],
      "properties": {"children": {"$ref": "#/$defs/count"}, "child_segments": {"$ref": "#/$defs/count"}, "child_links": {"$ref": "#/$defs/count"}, "adjacency_rows": {"$ref": "#/$defs/count"}, "transition_decisions": {"$ref": "#/$defs/count"}, "child_output_bases": {"$ref": "#/$defs/count"}, "exact_agreement_presentations": {"$ref": "#/$defs/count"}},
      "additionalProperties": false
    },
    "limitations": {"type": "array", "minItems": 1, "uniqueItems": true, "items": {"type": "string", "minLength": 1}}
  },
  "$defs": {"digest": {"type": "string", "pattern": "^[0-9a-f]{64}$"}, "count": {"type": "integer", "minimum": 0}, "positive": {"type": "integer", "minimum": 1}},
  "additionalProperties": false
}
"##;

/// Presentation-only profile. Neither profile authorizes a cross-k sequence
/// change or chooses one child as preferable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortfolioProfile {
    DiversityPreserving,
    ExactAgreementConsensus,
}

impl PortfolioProfile {
    const fn tag(self) -> u8 {
        match self {
            Self::DiversityPreserving => 0,
            Self::ExactAgreementConsensus => 1,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DiversityPreserving => "diversity_preserving",
            Self::ExactAgreementConsensus => "exact_agreement_consensus",
        }
    }
}

/// Typed state for evidence planes intentionally excluded from Slice A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisabledCapabilityState {
    DisabledUnqualified,
}

/// Input-mode-specific state of the authenticated paired-evidence plane.
///
/// `AuthenticatedLinearUnitigOnlyUnqualified` means the complete opaque
/// producer/analyzer chain ran for every child. It is not graph-junction
/// placement, repeat resolution, scaffolding, or a sequence join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairEvidenceState {
    NotApplicableSingleEnd,
    UnavailableResourceLimit,
    AuthenticatedLinearUnitigOnlyUnqualified,
}

impl PairEvidenceState {
    const fn tag(self) -> u8 {
        match self {
            Self::NotApplicableSingleEnd => 0,
            Self::UnavailableResourceLimit => 1,
            Self::AuthenticatedLinearUnitigOnlyUnqualified => 2,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicableSingleEnd => "not_applicable_single_end",
            Self::UnavailableResourceLimit => "unavailable_resource_limit",
            Self::AuthenticatedLinearUnitigOnlyUnqualified => {
                "authenticated_linear_unitig_only_unqualified"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportPairLaneModel {
    pub lane_ordinal: u32,
    pub availability: String,
    pub calibration_included_anchors: u64,
    pub dominant_orientation: Option<String>,
    pub outer_span_p10: Option<u64>,
    pub outer_span_p90: Option<u64>,
    pub inner_gap_p10: Option<i64>,
    pub inner_gap_p90: Option<i64>,
}

/// Compact summary plus a canonical complete JSON document copied exactly
/// once from the opaque source-backed pair capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportPairEvidencePayload {
    pub state: PairEvidenceState,
    pub placement_domain: String,
    pub source_root: [u8; 32],
    pub pair_mapping_source_root: [u8; 32],
    pub source_equivalence_root: [u8; 32],
    pub compacted_graph_ancestry_root: [u8; 32],
    pub transition_root: [u8; 32],
    pub witnessed_child_root: [u8; 32],
    pub witnessed_child_authentication_root: [u8; 32],
    pub exact_pair_graph_root: [u8; 32],
    pub authenticated_pair_graph_root: [u8; 32],
    pub mapper_identity_root: [u8; 32],
    pub calibration_placement_root: [u8; 32],
    pub replay_placement_root: [u8; 32],
    pub placement_producer_root: [u8; 32],
    pub pair_path_result_root: [u8; 32],
    pub authenticated_pair_path_result_root: [u8; 32],
    pub graph_segments: u64,
    pub graph_links: u64,
    pub graph_sequence_bases: u64,
    pub supplied_fragments: u64,
    pub configured_fragment_limit: u64,
    pub authenticated_fragments: u64,
    pub authenticated_reads: u64,
    pub calibration_fragments: u64,
    pub replay_fragments: u64,
    pub unavailable_ineligible_reads: u64,
    pub unavailable_candidate_limit_reads: u64,
    pub unavailable_possible_graph_junction_reads: u64,
    pub placement_groups: u64,
    pub placements: u64,
    pub supported_unique_existing_paths: u64,
    pub trivial_within_unitig: u64,
    pub abstained: u64,
    pub indeterminate: u64,
    pub available_aggregate_constraints: u64,
    pub decision_rows: u64,
    pub aggregate_path_rows: u64,
    pub lane_models: Vec<ReportPairLaneModel>,
    /// Complete deterministic document containing mapper parameters and work
    /// telemetry, frozen lane models, every replay decision, and every
    /// aggregate existing-path annotation.
    pub authenticated_document_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportPairEvidence {
    pub report_root: [u8; 32],
    pub payload: ReportPairEvidencePayload,
}

impl ReportPairEvidence {
    pub(crate) fn not_applicable_single_end(source_root: [u8; 32]) -> Result<Self> {
        let payload = ReportPairEvidencePayload {
            state: PairEvidenceState::NotApplicableSingleEnd,
            placement_domain: "not_applicable".to_owned(),
            source_root,
            pair_mapping_source_root: [0; 32],
            source_equivalence_root: [0; 32],
            compacted_graph_ancestry_root: [0; 32],
            transition_root: [0; 32],
            witnessed_child_root: [0; 32],
            witnessed_child_authentication_root: [0; 32],
            exact_pair_graph_root: [0; 32],
            authenticated_pair_graph_root: [0; 32],
            mapper_identity_root: [0; 32],
            calibration_placement_root: [0; 32],
            replay_placement_root: [0; 32],
            placement_producer_root: [0; 32],
            pair_path_result_root: [0; 32],
            authenticated_pair_path_result_root: [0; 32],
            graph_segments: 0,
            graph_links: 0,
            graph_sequence_bases: 0,
            supplied_fragments: 0,
            configured_fragment_limit: 0,
            authenticated_fragments: 0,
            authenticated_reads: 0,
            calibration_fragments: 0,
            replay_fragments: 0,
            unavailable_ineligible_reads: 0,
            unavailable_candidate_limit_reads: 0,
            unavailable_possible_graph_junction_reads: 0,
            placement_groups: 0,
            placements: 0,
            supported_unique_existing_paths: 0,
            trivial_within_unitig: 0,
            abstained: 0,
            indeterminate: 0,
            available_aggregate_constraints: 0,
            decision_rows: 0,
            aggregate_path_rows: 0,
            lane_models: Vec::new(),
            authenticated_document_json: String::new(),
        };
        Self::from_payload(payload)
    }

    pub(crate) fn unavailable_resource_limit(
        source_root: [u8; 32],
        supplied_fragments: u64,
        configured_fragment_limit: u64,
    ) -> Result<Self> {
        let payload = ReportPairEvidencePayload {
            state: PairEvidenceState::UnavailableResourceLimit,
            placement_domain: "unavailable_resource_limit".to_owned(),
            source_root,
            pair_mapping_source_root: [0; 32],
            source_equivalence_root: [0; 32],
            compacted_graph_ancestry_root: [0; 32],
            transition_root: [0; 32],
            witnessed_child_root: [0; 32],
            witnessed_child_authentication_root: [0; 32],
            exact_pair_graph_root: [0; 32],
            authenticated_pair_graph_root: [0; 32],
            mapper_identity_root: [0; 32],
            calibration_placement_root: [0; 32],
            replay_placement_root: [0; 32],
            placement_producer_root: [0; 32],
            pair_path_result_root: [0; 32],
            authenticated_pair_path_result_root: [0; 32],
            graph_segments: 0,
            graph_links: 0,
            graph_sequence_bases: 0,
            supplied_fragments,
            configured_fragment_limit,
            authenticated_fragments: 0,
            authenticated_reads: 0,
            calibration_fragments: 0,
            replay_fragments: 0,
            unavailable_ineligible_reads: 0,
            unavailable_candidate_limit_reads: 0,
            unavailable_possible_graph_junction_reads: 0,
            placement_groups: 0,
            placements: 0,
            supported_unique_existing_paths: 0,
            trivial_within_unitig: 0,
            abstained: 0,
            indeterminate: 0,
            available_aggregate_constraints: 0,
            decision_rows: 0,
            aggregate_path_rows: 0,
            lane_models: Vec::new(),
            authenticated_document_json: String::new(),
        };
        Self::from_payload(payload)
    }

    pub(crate) fn from_payload(payload: ReportPairEvidencePayload) -> Result<Self> {
        let report_root = pair_report_root(&payload)?;
        Ok(Self {
            report_root,
            payload,
        })
    }
}

impl DisabledCapabilityState {
    const fn tag(self) -> u8 {
        0
    }

    pub const fn as_str(self) -> &'static str {
        "disabled_unqualified"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReportTopology {
    Linear,
    ClosedWalk,
}

impl ReportTopology {
    const fn tag(self) -> u8 {
        match self {
            Self::Linear => 0,
            Self::ClosedWalk => 1,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::ClosedWalk => "closed_walk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReportOrientation {
    Forward,
    ReverseComplement,
}

impl ReportOrientation {
    const fn gfa(self) -> char {
        match self {
            Self::Forward => '+',
            Self::ReverseComplement => '-',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReportEdgeOrientation {
    Canonical,
    ReverseComplement,
    SelfReverseComplement,
}

impl ReportEdgeOrientation {
    const fn tag(self) -> u8 {
        match self {
            Self::Canonical => 0,
            Self::ReverseComplement => 1,
            Self::SelfReverseComplement => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportEdgeStep {
    pub canonical_kmer: [u8; 32],
    pub orientation: ReportEdgeOrientation,
    pub support: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReportRetentionRule {
    RetainAll,
    InclusiveSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportRetention {
    pub rule: ReportRetentionRule,
    pub minimum_support: Option<u64>,
    pub raw_keys: u64,
    pub retained_keys: u64,
    pub discarded_keys: u64,
    pub raw_support: u64,
    pub retained_support: u64,
    pub discarded_support: u64,
    pub decision_ledger_root: [u8; 32],
    pub retained_table_root: [u8; 32],
    pub retention_root: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportSegment {
    pub id: [u8; 32],
    pub parent_unitig_id: [u8; 32],
    pub parent_start_step: u64,
    pub parent_end_step_exclusive: u64,
    pub topology: ReportTopology,
    pub sequence: Vec<u8>,
    pub steps: Vec<ReportEdgeStep>,
    pub edge_steps: u64,
    pub total_edge_support: u64,
    pub minimum_edge_support: u64,
    pub lower_median_edge_support: u64,
    pub maximum_edge_support: u64,
    pub internal_transition_rows: u64,
    pub sum_accepted_window_occurrences: u64,
    pub sum_distinct_supplied_fragment_instances: u64,
    pub ordered_transition_rows_sha256: [u8; 32],
    pub provenance_sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportTransitionEvidence {
    pub accepted_window_occurrences: u64,
    pub distinct_supplied_fragment_instances: u64,
    pub sorted_event_frames_sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportLink {
    pub from: [u8; 32],
    pub from_orientation: ReportOrientation,
    pub to: [u8; 32],
    pub to_orientation: ReportOrientation,
    pub overlap_bases: u8,
    pub canonical_qmer: [u8; 32],
    pub evidence: ReportTransitionEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportAdjacency {
    pub canonical_qmer: [u8; 32],
    pub evidence: ReportTransitionEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportTransitionDecision {
    pub canonical_qmer: [u8; 32],
    /// Versioned, tab-free origin encoding produced from the authenticated
    /// reconstruction decision enum.
    pub origin: String,
    pub status: String,
    pub evidence: Option<ReportTransitionEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportConservation {
    pub input_canonical_edges: u64,
    pub represented_canonical_edges: u64,
    pub input_edge_support: u64,
    pub represented_edge_support: u64,
    pub topology_candidates: u64,
    pub admitted_topology_candidates: u64,
    pub excluded_no_original_read_witness: u64,
    pub ledger_rows: u64,
    pub eligible_ledger_rows: u64,
    pub excluded_endpoint_not_retained: u64,
    pub segments: u64,
    pub linear_segments: u64,
    pub closed_segments: u64,
    pub witnessed_links: u64,
    pub output_bases: u64,
    pub accounted_peak_bytes: u64,
}

/// One-way report payload for one already authenticated child. This type is
/// deliberately named `Unverified`: constructing it does not confer source
/// ancestry and it cannot enter graph or reconstruction APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UnverifiedChildSnapshotPayload {
    pub schema: String,
    pub k: u8,
    pub q: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub support_unit: String,
    pub source_root: [u8; 32],
    pub source_equivalence_root: [u8; 32],
    pub retention: ReportRetention,
    pub transition_root: [u8; 32],
    pub exact_edge_table_sha256: [u8; 32],
    pub raw_compacted_graph_root: [u8; 32],
    pub constrained_graph_root: [u8; 32],
    pub child_root: [u8; 32],
    pub child_authentication_root: [u8; 32],
    pub segments: Vec<ReportSegment>,
    pub links: Vec<ReportLink>,
    pub adjacency_rows: Vec<ReportAdjacency>,
    pub transition_decisions: Vec<ReportTransitionDecision>,
    pub conservation: ReportConservation,
    pub pair_evidence: ReportPairEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiKBundleLimits {
    pub max_children: u16,
    pub max_segments: u64,
    pub max_links: u64,
    pub max_adjacency_rows: u64,
    pub max_transition_decisions: u64,
    pub max_pair_decisions: u64,
    pub max_pair_aggregate_paths: u64,
    pub max_pair_lanes: u64,
    pub max_pair_document_bytes: u64,
    pub max_output_bases: u64,
    /// Maximum canonical JSON bytes before deterministic private gzip encoding.
    pub max_snapshot_document_bytes: u64,
    /// Maximum bytes stored by one compressed private child snapshot.
    pub max_snapshot_bytes: u64,
    /// Sum of registered snapshot encodings admitted into the reporting pass.
    pub max_loaded_snapshot_bytes: u64,
    /// Sum of registered, explicitly accounted decoded child-report payloads.
    pub max_loaded_report_accounted_bytes: u64,
    /// Conservative owned-payload admission for exact presentation grouping.
    pub max_presentation_accounted_bytes: u64,
    /// Fallibly pre-admitted pointer index used while validating one child.
    pub max_validation_scratch_bytes: u64,
    pub max_staged_output_bytes: u64,
    pub max_manifest_bytes: u64,
}

impl Default for MultiKBundleLimits {
    fn default() -> Self {
        Self {
            max_children: 16,
            max_segments: 10_000_000,
            max_links: 20_000_000,
            max_adjacency_rows: 100_000_000,
            max_transition_decisions: 100_000_000,
            max_pair_decisions: 1_000_000,
            max_pair_aggregate_paths: 1_000_000,
            max_pair_lanes: 65_536,
            max_pair_document_bytes: 64 * 1024 * 1024,
            max_output_bases: 10_000_000_000,
            max_snapshot_document_bytes: 64 * 1024 * 1024,
            max_snapshot_bytes: 8 * 1024 * 1024,
            max_loaded_snapshot_bytes: 32 * 1024 * 1024,
            max_loaded_report_accounted_bytes: 128 * 1024 * 1024,
            max_presentation_accounted_bytes: 64 * 1024 * 1024,
            max_validation_scratch_bytes: 16 * 1024 * 1024,
            max_staged_output_bytes: 100 * 1024 * 1024 * 1024,
            max_manifest_bytes: 1024 * 1024,
        }
    }
}

/// Private-path reference to a generated and reverified child report. Paths
/// are operational only and are never hashed into scientific ancestry or
/// rendered into the output bundle.
#[derive(Debug)]
pub(crate) struct ChildSnapshotRef {
    path: PathBuf,
    file: File,
    identity: SnapshotFileIdentity,
    k: u8,
    child_root: [u8; 32],
    byte_len: u64,
    sha256: [u8; 32],
    max_bytes: u64,
    max_document_bytes: u64,
    accounted_bytes: u64,
    max_validation_scratch_bytes: u64,
}

impl ChildSnapshotRef {
    pub(crate) const fn k(&self) -> u8 {
        self.k
    }

    pub(crate) const fn child_root(&self) -> [u8; 32] {
        self.child_root
    }

    pub(crate) const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    pub(crate) const fn accounted_bytes(&self) -> u64 {
        self.accounted_bytes
    }

    fn load_verified(&self) -> Result<UnverifiedChildSnapshotPayload> {
        self.identity.validate_path_and_file(
            &self.path,
            &self.file,
            "child snapshot before verification",
        )?;
        let mut file = self.file.try_clone().map_err(|cause| {
            io_error(
                ErrorCode::IntegrityArtifact,
                "duplicate child snapshot descriptor",
                cause,
            )
        })?;
        file.seek(SeekFrom::Start(0)).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityArtifact,
                "seek child snapshot for descriptor-bound read",
                cause,
            )
        })?;
        let byte_len = usize::try_from(self.byte_len).map_err(|_| {
            error(
                ErrorCode::ResourceIntegerOverflow,
                "child snapshot length does not fit address space",
            )
        })?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(byte_len).map_err(|cause| {
            error(
                ErrorCode::ResourceMemory,
                format!("cannot reserve descriptor-bound child snapshot: {cause}"),
            )
        })?;
        enforce_limit(
            usize_to_u64(bytes.capacity(), "child snapshot buffer capacity")?,
            self.max_bytes,
            ErrorCode::ResourceMemory,
            "descriptor-bound child snapshot buffer",
        )?;
        bytes.resize(byte_len, 0);
        file.read_exact(&mut bytes).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityArtifact,
                "read registered child snapshot bytes",
                cause,
            )
        })?;
        let mut trailing = [0_u8; 1];
        if file.read(&mut trailing).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityArtifact,
                "check registered child snapshot length",
                cause,
            )
        })? != 0
        {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child snapshot grew after registration",
            ));
        }
        let observed_sha256: [u8; 32] = Sha256::digest(&bytes).into();
        if observed_sha256 != self.sha256 {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child snapshot bytes changed after registration",
            ));
        }
        let document_limit = self.max_document_bytes.checked_add(1).ok_or_else(|| {
            error(
                ErrorCode::ResourceIntegerOverflow,
                "child snapshot document read limit overflow",
            )
        })?;
        let decoder = GzDecoder::new(io::Cursor::new(&bytes));
        let payload: UnverifiedChildSnapshotPayload =
            serde_json::from_reader(decoder.take(document_limit)).map_err(|cause| {
                error(
                    ErrorCode::IntegrityArtifact,
                    format!("cannot decode registered compressed child snapshot: {cause}"),
                )
            })?;
        enforce_limit(
            actual_report_allocation_bytes(&payload)?,
            self.accounted_bytes,
            ErrorCode::ResourceMemory,
            "decoded child-report actual allocation",
        )?;
        if payload.k != self.k || payload.child_root != self.child_root {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child snapshot identity differs from its registered reference",
            ));
        }
        let decoder = GzDecoder::new(io::Cursor::new(&bytes));
        compare_snapshot_encoding(&mut decoder.take(document_limit), &payload)?;
        drop(bytes);
        validate_child(&payload, self.max_validation_scratch_bytes)?;
        self.identity.validate_path_and_file(
            &self.path,
            &self.file,
            "child snapshot after verification",
        )?;
        Ok(payload)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotFileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    uid: u32,
    mode: u32,
}

impl SnapshotFileIdentity {
    fn capture(file: &File, label: &'static str) -> Result<Self> {
        let metadata = file
            .metadata()
            .map_err(|cause| io_error(ErrorCode::IntegrityArtifact, label, cause))?;
        if !metadata.file_type().is_file() {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("{label} descriptor is not a regular file"),
            ));
        }
        let identity = Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            uid: metadata.uid(),
            mode: metadata.mode(),
        };
        if identity.mode & 0o777 != 0o600 {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("{label} permissions are not exactly 0600"),
            ));
        }
        Ok(identity)
    }

    fn validate_path_and_file(self, path: &Path, file: &File, label: &'static str) -> Result<()> {
        if Self::capture(file, label)? != self {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("{label} descriptor identity, length, owner, or mode changed"),
            ));
        }
        let metadata = fs::symlink_metadata(path)
            .map_err(|cause| io_error(ErrorCode::IntegrityArtifact, label, cause))?;
        if !metadata.file_type().is_file()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
            || metadata.len() != self.length
            || metadata.uid() != self.uid
            || metadata.mode() != self.mode
        {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("{label} pathname no longer names the registered regular file"),
            ));
        }
        Ok(())
    }
}

/// Write and independently re-read one child report before the graph
/// capability is released by the pipeline.
pub(crate) fn write_unverified_child_snapshot(
    work_dir: &Path,
    payload: &UnverifiedChildSnapshotPayload,
    max_snapshot_document_bytes: u64,
    max_snapshot_bytes: u64,
    max_validation_scratch_bytes: u64,
) -> Result<ChildSnapshotRef> {
    if max_snapshot_document_bytes == 0
        || max_snapshot_bytes == 0
        || max_validation_scratch_bytes == 0
    {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "maximum child snapshot bytes must be nonzero",
        ));
    }
    validate_child(payload, max_validation_scratch_bytes)?;
    let accounted_bytes = accounted_report_payload_bytes(payload)?;
    enforce_limit(
        actual_report_allocation_bytes(payload)?,
        accounted_bytes,
        ErrorCode::ResourceMemory,
        "generated child-report actual allocation",
    )?;
    let path = work_dir.join(format!("multik-child-k{:03}.snapshot.json.gz", payload.k));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "create child snapshot", cause))?;
    let compressed = BoundedDigestWriter::new(file, max_snapshot_bytes);
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(compressed, Compression::fast());
    let mut writer = BoundedForwardWriter::new(encoder, max_snapshot_document_bytes);
    let encoded = serde_json::to_writer(&mut writer, payload);
    let document_limit_exceeded = writer.limit_exceeded();
    let compressed_limit_exceeded = writer.inner().get_ref().limit_exceeded();
    if document_limit_exceeded || compressed_limit_exceeded {
        let _ = fs::remove_file(&path);
        return Err(error(
            ErrorCode::ResourceOutputBytes,
            "child snapshot document or compressed bytes exceed the configured limit",
        ));
    }
    if let Err(cause) = encoded {
        let _ = fs::remove_file(&path);
        return Err(error(
            ErrorCode::CommitWrite,
            format!("cannot stream compressed child snapshot encoding: {cause}"),
        ));
    }
    let mut encoder = writer.into_inner();
    if let Err(cause) = encoder.try_finish() {
        let limit_exceeded = encoder.get_ref().limit_exceeded();
        let _ = fs::remove_file(&path);
        return Err(if limit_exceeded {
            error(
                ErrorCode::ResourceOutputBytes,
                "compressed child snapshot bytes exceed the configured limit",
            )
        } else {
            io_error(
                ErrorCode::CommitWrite,
                "finish compressed child snapshot",
                cause,
            )
        });
    }
    let compressed = match encoder.finish() {
        Ok(compressed) => compressed,
        Err(cause) => {
            let _ = fs::remove_file(&path);
            return Err(io_error(
                ErrorCode::CommitWrite,
                "finalize compressed child snapshot",
                cause,
            ));
        }
    };
    let (file, byte_len, sha256) = match compressed.finish("child snapshot") {
        Ok(result) => result,
        Err(cause) => {
            let _ = fs::remove_file(&path);
            return Err(cause);
        }
    };
    let identity = SnapshotFileIdentity::capture(&file, "created child snapshot")?;
    if identity.length != byte_len {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "created child snapshot length differs from its writer count",
        ));
    }
    let reference = ChildSnapshotRef {
        path,
        file,
        identity,
        k: payload.k,
        child_root: payload.child_root,
        byte_len,
        sha256,
        max_bytes: max_snapshot_bytes,
        max_document_bytes: max_snapshot_document_bytes,
        accounted_bytes,
        max_validation_scratch_bytes,
    };
    reference.identity.validate_path_and_file(
        &reference.path,
        &reference.file,
        "created child snapshot",
    )?;
    let mut comparison = reference.file.try_clone().map_err(|cause| {
        io_error(
            ErrorCode::IntegrityArtifact,
            "duplicate created child snapshot descriptor",
            cause,
        )
    })?;
    comparison.seek(SeekFrom::Start(0)).map_err(|cause| {
        io_error(
            ErrorCode::IntegrityArtifact,
            "seek created child snapshot",
            cause,
        )
    })?;
    let document_limit = max_snapshot_document_bytes.checked_add(1).ok_or_else(|| {
        error(
            ErrorCode::ResourceIntegerOverflow,
            "created child snapshot document limit overflow",
        )
    })?;
    let decoder = GzDecoder::new(&mut comparison);
    compare_snapshot_encoding(&mut decoder.take(document_limit), payload)?;
    reference.identity.validate_path_and_file(
        &reference.path,
        &reference.file,
        "created child snapshot after comparison",
    )?;
    Ok(reference)
}

fn accounted_report_payload_bytes(payload: &UnverifiedChildSnapshotPayload) -> Result<u64> {
    let mut logical = checked_add(
        16_384,
        usize_to_u64(
            payload
                .segments
                .len()
                .checked_mul(size_of::<ReportSegment>())
                .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "segment bytes"))?,
            "segment bytes",
        )?,
        "decoded report bytes",
    )?;
    for segment in &payload.segments {
        logical = checked_add(
            logical,
            usize_to_u64(segment.sequence.len(), "sequence length")?,
            "decoded report sequence bytes",
        )?;
        logical = checked_add(
            logical,
            usize_to_u64(
                segment
                    .steps
                    .len()
                    .checked_mul(size_of::<ReportEdgeStep>())
                    .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "step bytes"))?,
                "step bytes",
            )?,
            "decoded report step bytes",
        )?;
    }
    for allocation in [
        payload.links.len().checked_mul(size_of::<ReportLink>()),
        payload
            .adjacency_rows
            .len()
            .checked_mul(size_of::<ReportAdjacency>()),
        payload
            .transition_decisions
            .len()
            .checked_mul(size_of::<ReportTransitionDecision>()),
    ] {
        logical = checked_add(
            logical,
            usize_to_u64(
                allocation.ok_or_else(|| {
                    error(
                        ErrorCode::ResourceIntegerOverflow,
                        "decoded report allocation",
                    )
                })?,
                "decoded report allocation",
            )?,
            "decoded report bytes",
        )?;
    }
    for decision in &payload.transition_decisions {
        logical = checked_add(
            logical,
            usize_to_u64(decision.origin.len(), "decision origin length")?,
            "decoded report decision bytes",
        )?;
        logical = checked_add(
            logical,
            usize_to_u64(decision.status.len(), "decision status length")?,
            "decoded report decision bytes",
        )?;
    }
    logical = checked_add(
        logical,
        logical_pair_report_bytes(&payload.pair_evidence)?,
        "decoded report pair-evidence bytes",
    )?;
    checked_add(
        logical,
        logical,
        "decoded report allocator-rounding allowance",
    )
}

fn actual_report_allocation_bytes(payload: &UnverifiedChildSnapshotPayload) -> Result<u64> {
    let mut bytes = checked_add(
        16_384,
        usize_to_u64(
            payload
                .segments
                .capacity()
                .checked_mul(size_of::<ReportSegment>())
                .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "segment capacity"))?,
            "segment capacity bytes",
        )?,
        "actual decoded report bytes",
    )?;
    for segment in &payload.segments {
        bytes = checked_add(
            bytes,
            usize_to_u64(segment.sequence.capacity(), "sequence capacity")?,
            "actual decoded report bytes",
        )?;
        bytes = checked_add(
            bytes,
            usize_to_u64(
                segment
                    .steps
                    .capacity()
                    .checked_mul(size_of::<ReportEdgeStep>())
                    .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "step capacity"))?,
                "step capacity bytes",
            )?,
            "actual decoded report bytes",
        )?;
    }
    for allocation in [
        payload
            .links
            .capacity()
            .checked_mul(size_of::<ReportLink>()),
        payload
            .adjacency_rows
            .capacity()
            .checked_mul(size_of::<ReportAdjacency>()),
        payload
            .transition_decisions
            .capacity()
            .checked_mul(size_of::<ReportTransitionDecision>()),
    ] {
        bytes = checked_add(
            bytes,
            usize_to_u64(
                allocation.ok_or_else(|| {
                    error(
                        ErrorCode::ResourceIntegerOverflow,
                        "actual decoded report allocation",
                    )
                })?,
                "actual decoded report allocation",
            )?,
            "actual decoded report bytes",
        )?;
    }
    for decision in &payload.transition_decisions {
        bytes = checked_add(
            bytes,
            usize_to_u64(decision.origin.capacity(), "decision origin capacity")?,
            "actual decoded report bytes",
        )?;
        bytes = checked_add(
            bytes,
            usize_to_u64(decision.status.capacity(), "decision status capacity")?,
            "actual decoded report bytes",
        )?;
    }
    bytes = checked_add(
        bytes,
        actual_pair_report_bytes(&payload.pair_evidence)?,
        "actual decoded pair-report bytes",
    )?;
    Ok(bytes)
}

fn logical_pair_report_bytes(report: &ReportPairEvidence) -> Result<u64> {
    let lanes = usize_to_u64(
        report
            .payload
            .lane_models
            .len()
            .checked_mul(size_of::<ReportPairLaneModel>())
            .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "pair lane bytes"))?,
        "pair lane bytes",
    )?;
    let strings = report.payload.lane_models.iter().try_fold(
        usize_to_u64(
            report.payload.authenticated_document_json.len(),
            "pair document length",
        )?,
        |sum, lane| {
            checked_add(
                sum,
                checked_add(
                    usize_to_u64(lane.availability.len(), "pair availability length")?,
                    usize_to_u64(
                        lane.dominant_orientation.as_deref().unwrap_or("").len(),
                        "pair orientation length",
                    )?,
                    "pair lane string bytes",
                )?,
                "pair report string bytes",
            )
        },
    )?;
    checked_add(
        checked_add(
            size_of::<ReportPairEvidence>() as u64,
            size_of::<ReportPairEvidencePayload>() as u64,
            "pair report headers",
        )?,
        checked_add(lanes, strings, "pair report dynamic bytes")?,
        "pair report bytes",
    )
}

fn actual_pair_report_bytes(report: &ReportPairEvidence) -> Result<u64> {
    let lanes = usize_to_u64(
        report
            .payload
            .lane_models
            .capacity()
            .checked_mul(size_of::<ReportPairLaneModel>())
            .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "pair lane capacity"))?,
        "pair lane capacity bytes",
    )?;
    let strings = report.payload.lane_models.iter().try_fold(
        usize_to_u64(
            report.payload.authenticated_document_json.capacity(),
            "pair document capacity",
        )?,
        |sum, lane| {
            checked_add(
                sum,
                checked_add(
                    usize_to_u64(lane.availability.capacity(), "pair availability capacity")?,
                    usize_to_u64(
                        lane.dominant_orientation
                            .as_ref()
                            .map_or(0, String::capacity),
                        "pair orientation capacity",
                    )?,
                    "pair lane string capacity bytes",
                )?,
                "pair report string capacity bytes",
            )
        },
    )?;
    checked_add(
        checked_add(
            size_of::<ReportPairEvidence>() as u64,
            size_of::<ReportPairEvidencePayload>() as u64,
            "pair report headers",
        )?,
        checked_add(lanes, strings, "pair report dynamic capacity bytes")?,
        "pair report capacity bytes",
    )
}

/// Inputs to the one-way portfolio rendering transaction.
#[derive(Debug)]
pub(crate) struct UnverifiedMultiKBundleData {
    pub source_root: [u8; 32],
    pub input_mode: String,
    pub support_unit: String,
    pub min_base_quality: u8,
    pub profile: PortfolioProfile,
    pub pair_evidence: PairEvidenceState,
    pub quality_correction: DisabledCapabilityState,
    pub execution: UnverifiedExecutionReport,
    pub children: Vec<ChildSnapshotRef>,
    pub limits: MultiKBundleLimits,
}

/// Operational, non-scientific execution facts rendered into the bundle and
/// bound by a separate operational root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnverifiedExecutionReport {
    pub execution_threads: u16,
    pub max_aggregate_accounted_memory_bytes: u64,
    pub projected_aggregate_accounted_memory_bytes: u64,
    pub max_aggregate_temp_bytes: u64,
    pub max_final_staging_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiKBundleOutcome {
    pub destination: PathBuf,
    pub parent_root: [u8; 32],
    pub operational_root: [u8; 32],
    pub manifest_sha256: [u8; 32],
    pub children: u64,
    pub segments: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PresentationKey {
    topology: ReportTopology,
    sequence: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Presentation {
    id: [u8; 32],
    key: PresentationKey,
    members: Vec<(u8, [u8; 32])>,
}

struct PreparedPortfolio {
    source_root: [u8; 32],
    input_mode: String,
    support_unit: String,
    min_base_quality: u8,
    profile: PortfolioProfile,
    pair_evidence: PairEvidenceState,
    quality_correction: DisabledCapabilityState,
    execution: UnverifiedExecutionReport,
    children: Vec<UnverifiedChildSnapshotPayload>,
    presentations: Vec<Presentation>,
    exact_agreement_root: [u8; 32],
    parent_root: [u8; 32],
    operational_root: [u8; 32],
    total_segments: u64,
    total_links: u64,
    total_adjacencies: u64,
    total_decisions: u64,
    total_output_bases: u64,
}

impl PreparedPortfolio {
    fn new(data: &UnverifiedMultiKBundleData) -> Result<Self> {
        validate_bundle_limits(data.limits)?;
        if data.source_root == [0; 32] {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "multi-k source root is unbound",
            ));
        }
        if !matches!(data.input_mode.as_str(), "single_end" | "paired_end") {
            return Err(error(
                ErrorCode::ConfigurationUnsupportedCombination,
                "multi-k input mode must be single_end or paired_end",
            ));
        }
        if !matches!(
            (data.input_mode.as_str(), data.pair_evidence),
            ("single_end", PairEvidenceState::NotApplicableSingleEnd)
                | (
                    "paired_end",
                    PairEvidenceState::AuthenticatedLinearUnitigOnlyUnqualified
                )
                | ("paired_end", PairEvidenceState::UnavailableResourceLimit)
        ) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "multi-k pair-evidence state is incompatible with input mode",
            ));
        }
        if !matches!(
            data.support_unit.as_str(),
            "supplied_fragment_instance" | "accepted_window_occurrence"
        ) {
            return Err(error(
                ErrorCode::ConfigurationInvalidSupport,
                "multi-k support unit is invalid",
            ));
        }
        if data.min_base_quality > 93 {
            return Err(error(
                ErrorCode::ConfigurationInvalidLimit,
                "multi-k minimum base quality is outside Q0..Q93",
            ));
        }
        if data.execution.execution_threads != 1
            || data.execution.max_aggregate_accounted_memory_bytes == 0
            || data.execution.projected_aggregate_accounted_memory_bytes == 0
            || data.execution.projected_aggregate_accounted_memory_bytes
                > data.execution.max_aggregate_accounted_memory_bytes
            || data.execution.max_aggregate_temp_bytes == 0
            || data.execution.max_final_staging_bytes == 0
        {
            return Err(error(
                ErrorCode::ConfigurationInvalidLimit,
                "multi-k execution report is inconsistent with serial aggregate admission",
            ));
        }
        enforce_limit(
            usize_to_u64(data.children.len(), "multi-k child count")?,
            u64::from(data.limits.max_children),
            ErrorCode::ResourceRetainedKeys,
            "multi-k children",
        )?;
        if data.children.is_empty() {
            return Err(error(
                ErrorCode::ConfigurationInvalidK,
                "multi-k portfolio requires at least one child",
            ));
        }

        let mut children = Vec::new();
        children
            .try_reserve_exact(data.children.len())
            .map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot reserve verified child reports: {cause}"),
                )
            })?;
        let mut previous_k = None;
        let mut total_segments = 0_u64;
        let mut total_links = 0_u64;
        let mut total_adjacencies = 0_u64;
        let mut total_decisions = 0_u64;
        let mut total_pair_decisions = 0_u64;
        let mut total_pair_aggregate_paths = 0_u64;
        let mut total_pair_lanes = 0_u64;
        let mut total_pair_document_bytes = 0_u64;
        let mut total_output_bases = 0_u64;
        let mut total_snapshot_bytes = 0_u64;
        let mut total_report_accounted_bytes = 0_u64;
        for snapshot in &data.children {
            if previous_k.is_some_and(|k| snapshot.k() <= k) {
                return Err(error(
                    ErrorCode::ConfigurationInvalidK,
                    "multi-k child snapshots are not strictly increasing by k",
                ));
            }
            total_snapshot_bytes = checked_add(
                total_snapshot_bytes,
                snapshot.byte_len(),
                "portfolio snapshot-byte count overflow",
            )?;
            enforce_limit(
                total_snapshot_bytes,
                data.limits.max_loaded_snapshot_bytes,
                ErrorCode::ResourceMemory,
                "multi-k loaded snapshot bytes",
            )?;
            total_report_accounted_bytes = checked_add(
                total_report_accounted_bytes,
                snapshot.accounted_bytes(),
                "portfolio decoded report-accounting overflow",
            )?;
            enforce_limit(
                total_report_accounted_bytes,
                data.limits.max_loaded_report_accounted_bytes,
                ErrorCode::ResourceMemory,
                "multi-k decoded child-report owned payload",
            )?;
            let child = snapshot.load_verified()?;
            if accounted_report_payload_bytes(&child)? != snapshot.accounted_bytes() {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "decoded child-report accounting differs from its registered reference",
                ));
            }
            if child.source_root != data.source_root
                || child.k != snapshot.k()
                || child.child_root != snapshot.child_root()
            {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "multi-k child snapshot has incompatible ancestry",
                ));
            }
            if child.pair_evidence.payload.state != data.pair_evidence {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "child pair-evidence state differs from the portfolio input mode",
                ));
            }
            previous_k = Some(child.k);
            total_segments = checked_add(
                total_segments,
                usize_to_u64(child.segments.len(), "child segment count")?,
                "portfolio segment count overflow",
            )?;
            total_links = checked_add(
                total_links,
                usize_to_u64(child.links.len(), "child link count")?,
                "portfolio link count overflow",
            )?;
            total_adjacencies = checked_add(
                total_adjacencies,
                usize_to_u64(child.adjacency_rows.len(), "child adjacency count")?,
                "portfolio adjacency count overflow",
            )?;
            total_decisions = checked_add(
                total_decisions,
                usize_to_u64(
                    child.transition_decisions.len(),
                    "child transition-decision count",
                )?,
                "portfolio transition-decision count overflow",
            )?;
            total_pair_decisions = checked_add(
                total_pair_decisions,
                child.pair_evidence.payload.decision_rows,
                "portfolio pair-decision count overflow",
            )?;
            total_pair_aggregate_paths = checked_add(
                total_pair_aggregate_paths,
                child.pair_evidence.payload.aggregate_path_rows,
                "portfolio pair aggregate-path count overflow",
            )?;
            total_pair_lanes = checked_add(
                total_pair_lanes,
                usize_to_u64(
                    child.pair_evidence.payload.lane_models.len(),
                    "child pair lane-model count",
                )?,
                "portfolio pair lane-model count overflow",
            )?;
            total_pair_document_bytes = checked_add(
                total_pair_document_bytes,
                usize_to_u64(
                    child
                        .pair_evidence
                        .payload
                        .authenticated_document_json
                        .len(),
                    "child pair document bytes",
                )?,
                "portfolio pair document bytes overflow",
            )?;
            total_output_bases = checked_add(
                total_output_bases,
                child.conservation.output_bases,
                "portfolio output-base count overflow",
            )?;
            children.push(child);
        }
        enforce_limit(
            total_segments,
            data.limits.max_segments,
            ErrorCode::ResourceRetainedKeys,
            "multi-k segments",
        )?;
        enforce_limit(
            total_links,
            data.limits.max_links,
            ErrorCode::ResourceRetainedKeys,
            "multi-k links",
        )?;
        enforce_limit(
            total_adjacencies,
            data.limits.max_adjacency_rows,
            ErrorCode::ResourceRetainedKeys,
            "multi-k adjacency rows",
        )?;
        enforce_limit(
            total_decisions,
            data.limits.max_transition_decisions,
            ErrorCode::ResourceRetainedKeys,
            "multi-k transition decisions",
        )?;
        enforce_limit(
            total_pair_decisions,
            data.limits.max_pair_decisions,
            ErrorCode::ResourceOutputBytes,
            "multi-k pair decision rows",
        )?;
        enforce_limit(
            total_pair_aggregate_paths,
            data.limits.max_pair_aggregate_paths,
            ErrorCode::ResourceOutputBytes,
            "multi-k pair aggregate paths",
        )?;
        enforce_limit(
            total_pair_lanes,
            data.limits.max_pair_lanes,
            ErrorCode::ResourceOutputBytes,
            "multi-k pair lane models",
        )?;
        enforce_limit(
            total_pair_document_bytes,
            data.limits.max_pair_document_bytes,
            ErrorCode::ResourceOutputBytes,
            "multi-k pair evidence document bytes",
        )?;
        enforce_limit(
            total_output_bases,
            data.limits.max_output_bases,
            ErrorCode::ResourceOutputBytes,
            "multi-k output bases",
        )?;

        let presentation_projection = projected_presentation_bytes(&children)?;
        enforce_limit(
            presentation_projection,
            data.limits.max_presentation_accounted_bytes,
            ErrorCode::ResourceMemory,
            "multi-k exact-presentation owned payload",
        )?;
        let presentations = exact_agreement_presentations(data.source_root, &children)?;
        let exact_agreement_root = exact_agreement_root(&presentations)?;
        let parent_root = parent_root(
            data.source_root,
            data.profile,
            &children,
            exact_agreement_root,
            data.pair_evidence,
            data.quality_correction,
        )?;
        let operational_root = operational_root(parent_root, data.execution, data.limits);
        Ok(Self {
            source_root: data.source_root,
            input_mode: data.input_mode.clone(),
            support_unit: data.support_unit.clone(),
            min_base_quality: data.min_base_quality,
            profile: data.profile,
            pair_evidence: data.pair_evidence,
            quality_correction: data.quality_correction,
            execution: data.execution,
            children,
            presentations,
            exact_agreement_root,
            parent_root,
            operational_root,
            total_segments,
            total_links,
            total_adjacencies,
            total_decisions,
            total_output_bases,
        })
    }
}

fn validate_bundle_limits(limits: MultiKBundleLimits) -> Result<()> {
    if limits.max_children == 0
        || limits.max_segments == 0
        || limits.max_links == 0
        || limits.max_adjacency_rows == 0
        || limits.max_transition_decisions == 0
        || limits.max_pair_decisions == 0
        || limits.max_pair_aggregate_paths == 0
        || limits.max_pair_lanes == 0
        || limits.max_pair_document_bytes == 0
        || limits.max_output_bases == 0
        || limits.max_snapshot_document_bytes == 0
        || limits.max_snapshot_bytes == 0
        || limits.max_loaded_snapshot_bytes == 0
        || limits.max_loaded_report_accounted_bytes == 0
        || limits.max_presentation_accounted_bytes == 0
        || limits.max_validation_scratch_bytes == 0
        || limits.max_staged_output_bytes == 0
        || limits.max_manifest_bytes == 0
    {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "all multi-k bundle limits must be nonzero",
        ));
    }
    Ok(())
}

fn validate_child(
    child: &UnverifiedChildSnapshotPayload,
    max_validation_scratch_bytes: u64,
) -> Result<()> {
    if child.schema != SNAPSHOT_SCHEMA {
        return Err(error(
            ErrorCode::IntegritySchema,
            "child snapshot schema is unsupported",
        ));
    }
    if !(3..=63).contains(&child.k)
        || child.q != child.k + 1
        || child.minimizer_length == 0
        || child.minimizer_length > child.k
        || child.virtual_bucket_count == 0
    {
        return Err(error(
            ErrorCode::ConfigurationInvalidK,
            "child k/q/minimizer/bucket configuration is invalid for Slice A",
        ));
    }
    if !matches!(
        child.support_unit.as_str(),
        "supplied_fragment_instance" | "accepted_window_occurrence"
    ) {
        return Err(error(
            ErrorCode::ConfigurationInvalidSupport,
            "child support unit is invalid",
        ));
    }
    for (label, root) in [
        ("source root", child.source_root),
        ("source-equivalence root", child.source_equivalence_root),
        (
            "retention decision root",
            child.retention.decision_ledger_root,
        ),
        ("retained-table root", child.retention.retained_table_root),
        ("retention root", child.retention.retention_root),
        ("transition root", child.transition_root),
        ("exact-edge-table root", child.exact_edge_table_sha256),
        ("raw compacted-graph root", child.raw_compacted_graph_root),
        ("constrained-graph root", child.constrained_graph_root),
        ("child root", child.child_root),
        ("child authentication root", child.child_authentication_root),
    ] {
        if root == [0; 32] {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("child {label} is unbound"),
            ));
        }
    }
    if checked_add(
        child.retention.retained_keys,
        child.retention.discarded_keys,
        "child retained-key conservation overflow",
    )? != child.retention.raw_keys
        || checked_add(
            child.retention.retained_support,
            child.retention.discarded_support,
            "child retained-support conservation overflow",
        )? != child.retention.raw_support
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child retention totals do not conserve keys and support",
        ));
    }
    match (child.retention.rule, child.retention.minimum_support) {
        (ReportRetentionRule::RetainAll, None) => {}
        (ReportRetentionRule::InclusiveSupport, Some(minimum)) if minimum > 0 => {}
        _ => {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child retention rule and threshold are inconsistent",
            ));
        }
    }
    if child.conservation.input_canonical_edges != child.conservation.represented_canonical_edges
        || child.conservation.input_edge_support != child.conservation.represented_edge_support
        || child.retention.retained_keys != child.conservation.input_canonical_edges
        || child.retention.retained_support != child.conservation.input_edge_support
        || child.conservation.segments != usize_to_u64(child.segments.len(), "child segment count")?
        || child.conservation.witnessed_links
            != usize_to_u64(child.links.len(), "child link count")?
        || child.conservation.ledger_rows
            != usize_to_u64(child.adjacency_rows.len(), "child adjacency-row count")?
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child reconstruction conservation disagrees with report rows",
        ));
    }
    let classified_segments = checked_add(
        child.conservation.linear_segments,
        child.conservation.closed_segments,
        "child topology count overflow",
    )?;
    let classified_candidates = checked_add(
        child.conservation.admitted_topology_candidates,
        child.conservation.excluded_no_original_read_witness,
        "child topology decision count overflow",
    )?;
    if classified_segments != child.conservation.segments
        || classified_candidates != child.conservation.topology_candidates
        || checked_add(
            child.conservation.eligible_ledger_rows,
            child.conservation.excluded_endpoint_not_retained,
            "child ledger-row classification overflow",
        )? != child.conservation.ledger_rows
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child reconstruction subclasses are inconsistent",
        ));
    }

    let mut previous_qmer = None;
    for row in &child.adjacency_rows {
        if !valid_canonical_qmer(row.canonical_qmer, child.q) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child adjacency q-mer is not a valid canonical packed value",
            ));
        }
        if previous_qmer.is_some_and(|qmer| qmer >= row.canonical_qmer) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child adjacency rows are duplicated or unsorted",
            ));
        }
        previous_qmer = Some(row.canonical_qmer);
        validate_transition_evidence(row.evidence)?;
    }

    let mut output_bases = 0_u64;
    let mut represented_edges = 0_u64;
    let mut represented_support = 0_u64;
    let mut previous_segment: Option<&ReportSegment> = None;
    for segment in &child.segments {
        validate_segment(
            child.k,
            child.child_root,
            segment,
            &child.adjacency_rows,
            max_validation_scratch_bytes,
        )?;
        if previous_segment.is_some_and(|previous| segment_order(previous, segment).is_gt()) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child segments are not in canonical output order",
            ));
        }
        previous_segment = Some(segment);
        output_bases = checked_add(
            output_bases,
            usize_to_u64(segment.sequence.len(), "segment sequence length")?,
            "child segment output-base sum overflow",
        )?;
        represented_edges = checked_add(
            represented_edges,
            segment.edge_steps,
            "child represented-edge sum overflow",
        )?;
        represented_support = checked_add(
            represented_support,
            segment.total_edge_support,
            "child represented-support sum overflow",
        )?;
    }
    if output_bases != child.conservation.output_bases
        || represented_edges != child.conservation.represented_canonical_edges
        || represented_support != child.conservation.represented_edge_support
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child segment totals disagree with reconstruction conservation",
        ));
    }
    let segment_scratch = usize_to_u64(
        child
            .segments
            .len()
            .checked_mul(size_of::<&ReportSegment>())
            .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "segment index bytes"))?,
        "segment validation index bytes",
    )?;
    enforce_limit(
        segment_scratch,
        max_validation_scratch_bytes,
        ErrorCode::ResourceMemory,
        "child validation scratch bytes",
    )?;
    let mut segment_by_id = Vec::new();
    segment_by_id
        .try_reserve_exact(child.segments.len())
        .map_err(|cause| {
            error(
                ErrorCode::ResourceMemory,
                format!("cannot reserve child validation segment index: {cause}"),
            )
        })?;
    segment_by_id.extend(child.segments.iter());
    segment_by_id.sort_unstable_by_key(|segment| segment.id);
    if segment_by_id
        .windows(2)
        .any(|window| window[0].id == window[1].id)
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child contains a duplicate witnessed-segment ID",
        ));
    }
    for link in &child.links {
        let from = segment_by_id
            .binary_search_by_key(&link.from, |segment| segment.id)
            .ok()
            .and_then(|index| segment_by_id.get(index).copied())
            .ok_or_else(|| {
                error(
                    ErrorCode::IntegrityArtifact,
                    "child link has an unknown from-segment",
                )
            })?;
        let to = segment_by_id
            .binary_search_by_key(&link.to, |segment| segment.id)
            .ok()
            .and_then(|index| segment_by_id.get(index).copied())
            .ok_or_else(|| {
                error(
                    ErrorCode::IntegrityArtifact,
                    "child link has an unknown to-segment",
                )
            })?;
        if link.overlap_bases != child.k - 1
            || link_qmer(
                child.k,
                from,
                link.from_orientation,
                to,
                link.to_orientation,
            )? != link.canonical_qmer
        {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child link overlap or exact transition spelling is invalid",
            ));
        }
        validate_transition_evidence(link.evidence)?;
        if !valid_canonical_qmer(link.canonical_qmer, child.q) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child link q-mer is not a valid canonical packed value",
            ));
        }
    }
    if child
        .links
        .windows(2)
        .any(|pair| link_order(&pair[0], &pair[1]) != std::cmp::Ordering::Less)
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child links are not in canonical output order",
        ));
    }
    for segment in &child.segments {
        if segment.topology == ReportTopology::ClosedWalk
            && !child.links.iter().any(|link| {
                link.from == segment.id
                    && link.from_orientation == ReportOrientation::Forward
                    && link.to == segment.id
                    && link.to_orientation == ReportOrientation::Forward
            })
        {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "closed-walk segment lacks its explicit witnessed closure link",
            ));
        }
    }
    for link in &child.links {
        if child
            .adjacency_rows
            .binary_search_by_key(&link.canonical_qmer, |row| row.canonical_qmer)
            .ok()
            .and_then(|index| child.adjacency_rows.get(index))
            .map(|row| row.evidence)
            != Some(link.evidence)
        {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child link lacks its exact reported original-read ledger row",
            ));
        }
    }
    let mut previous_decision: Option<(&[u8; 32], &str)> = None;
    let mut raw_candidates = 0_u64;
    let mut admitted_raw = 0_u64;
    let mut excluded_raw = 0_u64;
    let mut endpoint_excluded = 0_u64;
    for row in &child.transition_decisions {
        if !valid_canonical_qmer(row.canonical_qmer, child.q) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child decision q-mer is not a valid canonical packed value",
            ));
        }
        let origin_kind = validate_transition_origin(&row.origin)?;
        if !matches!(
            row.status.as_str(),
            "admitted_original_read"
                | "excluded_endpoint_not_retained"
                | "excluded_no_original_read_witness"
        ) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child transition decision status is invalid",
            ));
        }
        if previous_decision.is_some_and(|(qmer, origin)| {
            (qmer, origin) >= (&row.canonical_qmer, row.origin.as_str())
        }) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child transition decisions are duplicated or unsorted",
            ));
        }
        previous_decision = Some((&row.canonical_qmer, row.origin.as_str()));
        match (origin_kind, row.status.as_str(), row.evidence) {
            (ReportDecisionOriginKind::Raw, "admitted_original_read", Some(evidence)) => {
                validate_transition_evidence(evidence)?;
                raw_candidates = checked_add(raw_candidates, 1, "raw candidate count overflow")?;
                admitted_raw = checked_add(admitted_raw, 1, "admitted candidate count overflow")?;
            }
            (ReportDecisionOriginKind::Raw, "excluded_no_original_read_witness", None) => {
                raw_candidates = checked_add(raw_candidates, 1, "raw candidate count overflow")?;
                excluded_raw = checked_add(excluded_raw, 1, "excluded candidate count overflow")?;
            }
            (
                ReportDecisionOriginKind::LedgerRetainedNoRaw,
                "admitted_original_read",
                Some(evidence),
            ) => validate_transition_evidence(evidence)?,
            (
                ReportDecisionOriginKind::LedgerEndpointNotRetained,
                "excluded_endpoint_not_retained",
                Some(evidence),
            ) => {
                validate_transition_evidence(evidence)?;
                endpoint_excluded = checked_add(
                    endpoint_excluded,
                    1,
                    "endpoint-excluded decision count overflow",
                )?;
            }
            _ => {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "child transition decision has inconsistent origin, status, or evidence",
                ));
            }
        }
        let expected_adjacency = child
            .adjacency_rows
            .binary_search_by_key(&row.canonical_qmer, |entry| entry.canonical_qmer)
            .ok()
            .and_then(|index| child.adjacency_rows.get(index))
            .map(|entry| entry.evidence);
        match (row.evidence, expected_adjacency) {
            (Some(evidence), Some(expected)) if evidence == expected => {}
            (None, None) => {}
            _ => {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "child transition decision contradicts the reported exact ledger row",
                ));
            }
        }
    }
    if raw_candidates != child.conservation.topology_candidates
        || admitted_raw != child.conservation.admitted_topology_candidates
        || excluded_raw != child.conservation.excluded_no_original_read_witness
        || endpoint_excluded != child.conservation.excluded_endpoint_not_retained
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child transition-decision subclasses disagree with conservation totals",
        ));
    }
    validate_pair_evidence(child)?;
    Ok(())
}

fn validate_pair_evidence(child: &UnverifiedChildSnapshotPayload) -> Result<()> {
    let report = &child.pair_evidence;
    if report.report_root != pair_report_root(&report.payload)?
        || report.payload.source_root != child.source_root
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child pair-report root or source root is inconsistent",
        ));
    }
    let payload = &report.payload;
    match payload.state {
        PairEvidenceState::NotApplicableSingleEnd => {
            if payload.placement_domain != "not_applicable"
                || !payload.authenticated_document_json.is_empty()
                || !payload.lane_models.is_empty()
                || payload.graph_segments != 0
                || payload.graph_links != 0
                || payload.graph_sequence_bases != 0
                || payload.supplied_fragments != 0
                || payload.configured_fragment_limit != 0
                || payload.authenticated_fragments != 0
                || payload.authenticated_reads != 0
                || payload.calibration_fragments != 0
                || payload.replay_fragments != 0
                || payload.decision_rows != 0
                || payload.aggregate_path_rows != 0
                || [
                    payload.pair_mapping_source_root,
                    payload.source_equivalence_root,
                    payload.compacted_graph_ancestry_root,
                    payload.transition_root,
                    payload.witnessed_child_root,
                    payload.witnessed_child_authentication_root,
                    payload.exact_pair_graph_root,
                    payload.authenticated_pair_graph_root,
                    payload.mapper_identity_root,
                    payload.calibration_placement_root,
                    payload.replay_placement_root,
                    payload.placement_producer_root,
                    payload.pair_path_result_root,
                    payload.authenticated_pair_path_result_root,
                ]
                .into_iter()
                .any(|root| root != [0; 32])
            {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "single-end child contains paired-evidence claims",
                ));
            }
        }
        PairEvidenceState::UnavailableResourceLimit => {
            if payload.placement_domain != "unavailable_resource_limit"
                || !payload.authenticated_document_json.is_empty()
                || !payload.lane_models.is_empty()
                || payload.configured_fragment_limit == 0
                || payload.supplied_fragments <= payload.configured_fragment_limit
                || payload.graph_segments != 0
                || payload.graph_links != 0
                || payload.graph_sequence_bases != 0
                || payload.authenticated_fragments != 0
                || payload.authenticated_reads != 0
                || payload.calibration_fragments != 0
                || payload.replay_fragments != 0
                || payload.unavailable_ineligible_reads != 0
                || payload.unavailable_candidate_limit_reads != 0
                || payload.unavailable_possible_graph_junction_reads != 0
                || payload.placement_groups != 0
                || payload.placements != 0
                || payload.supported_unique_existing_paths != 0
                || payload.trivial_within_unitig != 0
                || payload.abstained != 0
                || payload.indeterminate != 0
                || payload.available_aggregate_constraints != 0
                || payload.decision_rows != 0
                || payload.aggregate_path_rows != 0
                || [
                    payload.pair_mapping_source_root,
                    payload.source_equivalence_root,
                    payload.compacted_graph_ancestry_root,
                    payload.transition_root,
                    payload.witnessed_child_root,
                    payload.witnessed_child_authentication_root,
                    payload.exact_pair_graph_root,
                    payload.authenticated_pair_graph_root,
                    payload.mapper_identity_root,
                    payload.calibration_placement_root,
                    payload.replay_placement_root,
                    payload.placement_producer_root,
                    payload.pair_path_result_root,
                    payload.authenticated_pair_path_result_root,
                ]
                .into_iter()
                .any(|root| root != [0; 32])
            {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "resource-unavailable child contains paired-evidence claims",
                ));
            }
        }
        PairEvidenceState::AuthenticatedLinearUnitigOnlyUnqualified => {
            if payload.placement_domain != "linear_unitig_only"
                || payload.authenticated_document_json.is_empty()
                || payload.supplied_fragments != payload.authenticated_fragments
                || payload.configured_fragment_limit < payload.supplied_fragments
                || payload.transition_root != child.transition_root
                || payload.witnessed_child_root != child.child_root
                || payload.witnessed_child_authentication_root != child.child_authentication_root
                || payload.graph_segments != child.conservation.segments
                || payload.graph_links != child.conservation.witnessed_links
                || payload.authenticated_reads
                    != payload
                        .authenticated_fragments
                        .checked_mul(2)
                        .ok_or_else(|| {
                            error(
                                ErrorCode::ResourceIntegerOverflow,
                                "paired authenticated-read count overflow",
                            )
                        })?
                || checked_add(
                    payload.calibration_fragments,
                    payload.replay_fragments,
                    "pair split conservation overflow",
                )? != payload.authenticated_fragments
                || payload.decision_rows != payload.replay_fragments
                || checked_add(
                    checked_add(
                        payload.supported_unique_existing_paths,
                        payload.trivial_within_unitig,
                        "pair decision summary overflow",
                    )?,
                    checked_add(
                        payload.abstained,
                        payload.indeterminate,
                        "pair decision summary overflow",
                    )?,
                    "pair decision summary overflow",
                )? != payload.decision_rows
                || payload.available_aggregate_constraints > payload.aggregate_path_rows
                || [
                    payload.pair_mapping_source_root,
                    payload.source_equivalence_root,
                    payload.compacted_graph_ancestry_root,
                    payload.transition_root,
                    payload.witnessed_child_root,
                    payload.witnessed_child_authentication_root,
                    payload.exact_pair_graph_root,
                    payload.authenticated_pair_graph_root,
                    payload.mapper_identity_root,
                    payload.calibration_placement_root,
                    payload.replay_placement_root,
                    payload.placement_producer_root,
                    payload.pair_path_result_root,
                    payload.authenticated_pair_path_result_root,
                ]
                .into_iter()
                .any(|root| root == [0; 32])
            {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "authenticated child pair evidence is internally inconsistent",
                ));
            }
            if payload
                .lane_models
                .windows(2)
                .any(|window| window[0].lane_ordinal >= window[1].lane_ordinal)
                || payload.lane_models.iter().any(|lane| {
                    lane.availability.is_empty()
                        || lane
                            .dominant_orientation
                            .as_deref()
                            .is_some_and(|value| !matches!(value, "FR" | "RF" | "FF" | "RR"))
                })
            {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "authenticated child lane-model summaries are invalid",
                ));
            }
            let mut deserializer =
                serde_json::Deserializer::from_str(&payload.authenticated_document_json);
            serde::de::IgnoredAny::deserialize(&mut deserializer).map_err(|cause| {
                error(
                    ErrorCode::IntegrityArtifact,
                    format!("authenticated pair document is invalid JSON: {cause}"),
                )
            })?;
            deserializer.end().map_err(|cause| {
                error(
                    ErrorCode::IntegrityArtifact,
                    format!("authenticated pair document has trailing JSON: {cause}"),
                )
            })?;
        }
    }
    Ok(())
}

fn link_qmer(
    k: u8,
    from: &ReportSegment,
    from_orientation: ReportOrientation,
    to: &ReportSegment,
    to_orientation: ReportOrientation,
) -> Result<[u8; 32]> {
    let k_usize = usize::from(k);
    let overlap = k_usize - 1;
    let from_length = from.sequence.len();
    for offset in 0..overlap {
        let from_base = oriented_segment_base(
            &from.sequence,
            from_orientation,
            from_length - overlap + offset,
        )?;
        let to_base = oriented_segment_base(&to.sequence, to_orientation, offset)?;
        if from_base != to_base {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child link endpoints do not have the declared exact overlap",
            ));
        }
    }
    let mut qmer = [0_u8; 64];
    for (offset, slot) in qmer[..k_usize].iter_mut().enumerate() {
        *slot = oriented_segment_base(
            &from.sequence,
            from_orientation,
            from_length - k_usize + offset,
        )?;
    }
    qmer[k_usize] = oriented_segment_base(&to.sequence, to_orientation, overlap)?;
    canonical_packed_bytes(&qmer[..=k_usize])
}

fn oriented_segment_base(
    sequence: &[u8],
    orientation: ReportOrientation,
    index: usize,
) -> Result<u8> {
    let base = match orientation {
        ReportOrientation::Forward => sequence.get(index).copied(),
        ReportOrientation::ReverseComplement => sequence
            .len()
            .checked_sub(index + 1)
            .and_then(|position| sequence.get(position).copied())
            .map(complement_base),
    };
    base.ok_or_else(|| {
        error(
            ErrorCode::IntegrityArtifact,
            "child link orientation addresses a base outside its segment",
        )
    })
}

const fn complement_base(base: u8) -> u8 {
    match base {
        b'A' => b'T',
        b'C' => b'G',
        b'G' => b'C',
        b'T' => b'A',
        _ => base,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportDecisionOriginKind {
    Raw,
    LedgerRetainedNoRaw,
    LedgerEndpointNotRetained,
}

fn validate_transition_origin(origin: &str) -> Result<ReportDecisionOriginKind> {
    validate_tsv_atom(origin, "transition origin")?;
    if origin == "ledger_retained_endpoints_no_raw_candidate" {
        return Ok(ReportDecisionOriginKind::LedgerRetainedNoRaw);
    }
    if origin == "ledger_endpoint_not_retained" {
        return Ok(ReportDecisionOriginKind::LedgerEndpointNotRetained);
    }
    let valid_raw = origin
        .strip_prefix("unitig_interior:")
        .and_then(|tail| tail.split_once(':'))
        .is_some_and(|(id, step)| is_hex_32(id) && step.parse::<u64>().is_ok())
        || origin
            .strip_prefix("closed_walk_closure:")
            .is_some_and(is_hex_32)
        || origin
            .strip_prefix("raw_compacted_link:")
            .is_some_and(validate_raw_link_origin);
    if valid_raw {
        Ok(ReportDecisionOriginKind::Raw)
    } else {
        Err(error(
            ErrorCode::IntegrityArtifact,
            "child transition decision origin is outside the versioned encoding",
        ))
    }
}

fn validate_raw_link_origin(tail: &str) -> bool {
    let mut fields = tail.split(':');
    let valid = fields.next().is_some_and(is_hex_32)
        && fields
            .next()
            .is_some_and(|value| matches!(value, "forward" | "reverse_complement"))
        && fields.next().is_some_and(is_hex_32)
        && fields
            .next()
            .is_some_and(|value| matches!(value, "forward" | "reverse_complement"))
        && fields
            .next()
            .is_some_and(|value| value.parse::<u8>().is_ok());
    valid && fields.next().is_none()
}

fn is_hex_32(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_canonical_qmer(encoded: [u8; 32], length: u8) -> bool {
    if !(4..=64).contains(&length) || encoded[..16].iter().any(|byte| *byte != 0) {
        return false;
    }
    let mut low = [0_u8; 16];
    low.copy_from_slice(&encoded[16..]);
    let value = u128::from_be_bytes(low);
    let bits = u32::from(length) * 2;
    if bits < 128 && value >= (1_u128 << bits) {
        return false;
    }
    value <= reverse_complement_u128(value, length)
}

fn reverse_complement_u128(mut value: u128, length: u8) -> u128 {
    let mut reverse = 0_u128;
    for _ in 0..length {
        reverse = (reverse << 2) | ((value & 0b11) ^ 0b11);
        value >>= 2;
    }
    reverse
}

fn encode_dna_u128(bases: &[u8]) -> Result<u128> {
    if bases.len() > 64 {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "report DNA window exceeds the 64-base Slice A encoding",
        ));
    }
    let mut value = 0_u128;
    for base in bases {
        let code = match base {
            b'A' => 0,
            b'C' => 1,
            b'G' => 2,
            b'T' => 3,
            _ => {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "report DNA window contains a non-exact base",
                ));
            }
        };
        value = (value << 2) | code;
    }
    Ok(value)
}

fn canonical_packed_bytes(bases: &[u8]) -> Result<[u8; 32]> {
    let length = u8::try_from(bases.len()).map_err(|_| {
        error(
            ErrorCode::ResourceIntegerOverflow,
            "report DNA window length does not fit u8",
        )
    })?;
    let literal = encode_dna_u128(bases)?;
    Ok(packed_u128_bytes(
        literal.min(reverse_complement_u128(literal, length)),
    ))
}

fn packed_u128_bytes(value: u128) -> [u8; 32] {
    let mut encoded = [0_u8; 32];
    encoded[16..].copy_from_slice(&value.to_be_bytes());
    encoded
}

fn validate_segment(
    k: u8,
    child_root: [u8; 32],
    segment: &ReportSegment,
    adjacency: &[ReportAdjacency],
    max_validation_scratch_bytes: u64,
) -> Result<()> {
    if segment.id == [0; 32]
        || segment.parent_unitig_id == [0; 32]
        || segment.ordered_transition_rows_sha256 == [0; 32]
        || segment.provenance_sha256 == [0; 32]
        || segment.sequence.len() < usize::from(k)
        || !segment
            .sequence
            .iter()
            .all(|base| matches!(base, b'A' | b'C' | b'G' | b'T'))
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child segment identity, roots, or exact DNA sequence is invalid",
        ));
    }
    let expected_steps = segment
        .sequence
        .len()
        .checked_sub(usize::from(k))
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "segment step overflow"))?;
    if segment.edge_steps != usize_to_u64(expected_steps, "segment edge-step count")?
        || segment.steps.len() != expected_steps
        || segment.parent_start_step >= segment.parent_end_step_exclusive
        || segment.parent_end_step_exclusive - segment.parent_start_step != segment.edge_steps
        || segment.minimum_edge_support == 0
        || segment.minimum_edge_support > segment.lower_median_edge_support
        || segment.lower_median_edge_support > segment.maximum_edge_support
        || segment.total_edge_support < segment.maximum_edge_support
        || segment.internal_transition_rows != segment.edge_steps - 1
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child segment support or transition summary is inconsistent",
        ));
    }

    let support_scratch = usize_to_u64(
        segment
            .steps
            .len()
            .checked_mul(size_of::<u64>())
            .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "support scratch bytes"))?,
        "support validation scratch bytes",
    )?;
    enforce_limit(
        support_scratch,
        max_validation_scratch_bytes,
        ErrorCode::ResourceMemory,
        "segment support validation scratch",
    )?;
    let mut support = Vec::new();
    support
        .try_reserve_exact(segment.steps.len())
        .map_err(|cause| {
            error(
                ErrorCode::ResourceMemory,
                format!("cannot reserve segment support validation scratch: {cause}"),
            )
        })?;
    let mut total_support = 0_u64;
    for (offset, step) in segment.steps.iter().enumerate() {
        if step.support == 0 {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child segment contains a zero-support exact edge",
            ));
        }
        let window_end = offset
            .checked_add(usize::from(k))
            .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "edge window overflow"))?;
        let literal = encode_dna_u128(&segment.sequence[offset..window_end])?;
        let reverse = reverse_complement_u128(literal, k);
        let canonical = literal.min(reverse);
        let expected_orientation = if literal == reverse {
            ReportEdgeOrientation::SelfReverseComplement
        } else if literal == canonical {
            ReportEdgeOrientation::Canonical
        } else {
            ReportEdgeOrientation::ReverseComplement
        };
        if step.canonical_kmer != packed_u128_bytes(canonical)
            || step.orientation != expected_orientation
        {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "child segment exact-edge record disagrees with its sequence spelling",
            ));
        }
        total_support = checked_add(
            total_support,
            step.support,
            "segment edge-support sum overflow",
        )?;
        support.push(step.support);
    }
    support.sort_unstable();
    if total_support != segment.total_edge_support
        || support.first().copied() != Some(segment.minimum_edge_support)
        || support.get((support.len() - 1) / 2).copied() != Some(segment.lower_median_edge_support)
        || support.last().copied() != Some(segment.maximum_edge_support)
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child segment support summary differs from complete exact-edge records",
        ));
    }

    let mut transition_digest = Sha256::new();
    transition_digest.update(TRANSITION_SET_DOMAIN);
    transition_digest.update(child_root);
    transition_digest.update(segment.internal_transition_rows.to_le_bytes());
    let mut occurrence_sum = 0_u64;
    let mut fragment_sum = 0_u64;
    for window in segment.sequence.windows(usize::from(k) + 1) {
        let qmer = canonical_packed_bytes(window)?;
        let evidence = adjacency
            .binary_search_by_key(&qmer, |row| row.canonical_qmer)
            .ok()
            .and_then(|index| adjacency.get(index))
            .map(|row| row.evidence)
            .ok_or_else(|| {
                error(
                    ErrorCode::IntegrityArtifact,
                    "child segment contains an adjacency absent from the exact original-read ledger",
                )
            })?;
        occurrence_sum = checked_add(
            occurrence_sum,
            evidence.accepted_window_occurrences,
            "segment transition-occurrence sum overflow",
        )?;
        fragment_sum = checked_add(
            fragment_sum,
            evidence.distinct_supplied_fragment_instances,
            "segment transition-fragment sum overflow",
        )?;
        hash_report_transition(&mut transition_digest, qmer, evidence);
    }
    if occurrence_sum != segment.sum_accepted_window_occurrences
        || fragment_sum != segment.sum_distinct_supplied_fragment_instances
        || <[u8; 32]>::from(transition_digest.finalize()) != segment.ordered_transition_rows_sha256
        || report_segment_id(k, child_root, segment) != segment.id
        || report_segment_provenance(k, child_root, segment) != segment.provenance_sha256
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "child segment evidence, identifier, or provenance does not match its complete preimage",
        ));
    }
    Ok(())
}

fn report_segment_id(k: u8, child_root: [u8; 32], segment: &ReportSegment) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SEGMENT_ID_DOMAIN);
    digest.update(child_root);
    digest.update([k, segment.topology.tag()]);
    hash_len_bytes(&mut digest, &segment.sequence);
    hash_report_steps(&mut digest, &segment.steps);
    digest.finalize().into()
}

fn report_segment_provenance(k: u8, child_root: [u8; 32], segment: &ReportSegment) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SEGMENT_PROVENANCE_DOMAIN);
    digest.update(child_root);
    digest.update(segment.id);
    digest.update(segment.parent_unitig_id);
    digest.update(segment.parent_start_step.to_le_bytes());
    digest.update(segment.parent_end_step_exclusive.to_le_bytes());
    digest.update([k, segment.topology.tag()]);
    hash_len_bytes(&mut digest, &segment.sequence);
    hash_report_steps(&mut digest, &segment.steps);
    digest.update(segment.total_edge_support.to_le_bytes());
    digest.update(segment.internal_transition_rows.to_le_bytes());
    digest.update(segment.sum_accepted_window_occurrences.to_le_bytes());
    digest.update(
        segment
            .sum_distinct_supplied_fragment_instances
            .to_le_bytes(),
    );
    digest.update(segment.ordered_transition_rows_sha256);
    digest.finalize().into()
}

fn hash_len_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn hash_report_steps(digest: &mut Sha256, steps: &[ReportEdgeStep]) {
    digest.update((steps.len() as u64).to_le_bytes());
    for step in steps {
        digest.update(step.canonical_kmer);
        digest.update([step.orientation.tag()]);
        digest.update(step.support.to_le_bytes());
    }
}

fn hash_report_transition(digest: &mut Sha256, qmer: [u8; 32], evidence: ReportTransitionEvidence) {
    digest.update(qmer);
    digest.update(evidence.accepted_window_occurrences.to_le_bytes());
    digest.update(evidence.distinct_supplied_fragment_instances.to_le_bytes());
    digest.update(evidence.sorted_event_frames_sha256);
}

fn validate_transition_evidence(evidence: ReportTransitionEvidence) -> Result<()> {
    if evidence.accepted_window_occurrences == 0
        || evidence.distinct_supplied_fragment_instances == 0
        || evidence.distinct_supplied_fragment_instances > evidence.accepted_window_occurrences
        || evidence.sorted_event_frames_sha256 == [0; 32]
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "original-read transition evidence is inconsistent",
        ));
    }
    Ok(())
}

fn segment_order(left: &ReportSegment, right: &ReportSegment) -> std::cmp::Ordering {
    right
        .sequence
        .len()
        .cmp(&left.sequence.len())
        .then_with(|| left.sequence.cmp(&right.sequence))
        .then_with(|| left.id.cmp(&right.id))
}

fn link_order(left: &ReportLink, right: &ReportLink) -> std::cmp::Ordering {
    left.from
        .cmp(&right.from)
        .then_with(|| left.from_orientation.cmp(&right.from_orientation))
        .then_with(|| left.to.cmp(&right.to))
        .then_with(|| left.to_orientation.cmp(&right.to_orientation))
        .then_with(|| left.canonical_qmer.cmp(&right.canonical_qmer))
}

fn exact_agreement_presentations(
    source_root: [u8; 32],
    children: &[UnverifiedChildSnapshotPayload],
) -> Result<Vec<Presentation>> {
    let mut groups = BTreeMap::<PresentationKey, Vec<(u8, [u8; 32])>>::new();
    for child in children {
        for segment in &child.segments {
            groups
                .entry(PresentationKey {
                    topology: segment.topology,
                    sequence: segment.sequence.clone(),
                })
                .or_default()
                .push((child.k, segment.id));
        }
    }
    let mut presentations = Vec::new();
    presentations
        .try_reserve_exact(groups.len())
        .map_err(|cause| {
            error(
                ErrorCode::ResourceMemory,
                format!("reserve presentations: {cause}"),
            )
        })?;
    let mut ids = BTreeMap::<[u8; 32], PresentationKey>::new();
    for (key, mut members) in groups {
        members.sort_unstable();
        if members.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                "exact-agreement group has duplicate child segment membership",
            ));
        }
        let id = presentation_id(source_root, &key, &members)?;
        if let Some(previous) = ids.insert(id, key.clone()) {
            if previous != key {
                return Err(error(
                    ErrorCode::IntegrityArtifact,
                    "unequal presentation preimages produced the same digest",
                ));
            }
        }
        presentations.push(Presentation { id, key, members });
    }
    presentations.sort_by(|left, right| {
        right
            .key
            .sequence
            .len()
            .cmp(&left.key.sequence.len())
            .then_with(|| left.key.sequence.cmp(&right.key.sequence))
            .then_with(|| left.key.topology.cmp(&right.key.topology))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(presentations)
}

fn projected_presentation_bytes(children: &[UnverifiedChildSnapshotPayload]) -> Result<u64> {
    let mut segment_count = 0_u64;
    let mut sequence_bytes = 0_u64;
    for child in children {
        segment_count = checked_add(
            segment_count,
            usize_to_u64(child.segments.len(), "presentation segment count")?,
            "presentation segment count overflow",
        )?;
        for segment in &child.segments {
            sequence_bytes = checked_add(
                sequence_bytes,
                usize_to_u64(segment.sequence.len(), "presentation sequence bytes")?,
                "presentation sequence-byte count overflow",
            )?;
        }
    }
    // BTree nodes and allocator rounding are implementation details. Admit
    // twice the explicit key/member/vector payload plus a fixed margin, then
    // keep this metric explicitly scoped to owned payload rather than RSS.
    let per_member = (size_of::<Presentation>()
        + size_of::<PresentationKey>()
        + size_of::<(u8, [u8; 32])>()
        + 128) as u64;
    let explicit = checked_add(
        sequence_bytes,
        segment_count
            .checked_mul(per_member)
            .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "presentation bytes"))?,
        "presentation bytes",
    )?;
    checked_add(
        explicit
            .checked_mul(2)
            .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, "presentation allowance"))?,
        16_384,
        "presentation allowance",
    )
}

fn presentation_id(
    source_root: [u8; 32],
    key: &PresentationKey,
    members: &[(u8, [u8; 32])],
) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(PRESENTATION_ID_DOMAIN);
    digest.update(source_root);
    digest.update([key.topology.tag()]);
    update_len_bytes(&mut digest, &key.sequence)?;
    digest.update(usize_to_u64(members.len(), "presentation member count")?.to_le_bytes());
    for (k, id) in members {
        digest.update([*k]);
        digest.update(id);
    }
    Ok(digest.finalize().into())
}

fn exact_agreement_root(presentations: &[Presentation]) -> Result<[u8; 32]> {
    let mut ordered = presentations
        .iter()
        .filter(|presentation| has_cross_k_agreement(presentation))
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.key
            .topology
            .cmp(&right.key.topology)
            .then_with(|| left.key.sequence.cmp(&right.key.sequence))
            .then_with(|| left.members.cmp(&right.members))
    });
    let mut digest = Sha256::new();
    digest.update(AGREEMENT_ROOT_DOMAIN);
    digest.update(usize_to_u64(ordered.len(), "agreement group count")?.to_le_bytes());
    for presentation in ordered {
        digest.update([presentation.key.topology.tag()]);
        update_len_bytes(&mut digest, &presentation.key.sequence)?;
        digest.update(
            usize_to_u64(presentation.members.len(), "agreement member count")?.to_le_bytes(),
        );
        for (k, id) in &presentation.members {
            digest.update([*k]);
            digest.update(id);
        }
    }
    Ok(digest.finalize().into())
}

fn parent_root(
    source_root: [u8; 32],
    profile: PortfolioProfile,
    children: &[UnverifiedChildSnapshotPayload],
    agreement_root: [u8; 32],
    pair_state: PairEvidenceState,
    correction_state: DisabledCapabilityState,
) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(PARENT_ROOT_DOMAIN);
    digest.update(source_root);
    digest.update([profile.tag()]);
    digest.update(usize_to_u64(children.len(), "parent child count")?.to_le_bytes());
    for child in children {
        digest.update([child.k]);
        digest.update(child.child_root);
        digest.update(child.child_authentication_root);
        digest.update(child.pair_evidence.report_root);
    }
    digest.update(agreement_root);
    digest.update([pair_state.tag(), correction_state.tag()]);
    Ok(digest.finalize().into())
}

fn operational_root(
    parent_root: [u8; 32],
    execution: UnverifiedExecutionReport,
    limits: MultiKBundleLimits,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(OPERATIONAL_ROOT_DOMAIN);
    digest.update(parent_root);
    digest.update(execution.execution_threads.to_le_bytes());
    digest.update(execution.max_aggregate_accounted_memory_bytes.to_le_bytes());
    digest.update(
        execution
            .projected_aggregate_accounted_memory_bytes
            .to_le_bytes(),
    );
    digest.update(execution.max_aggregate_temp_bytes.to_le_bytes());
    digest.update(execution.max_final_staging_bytes.to_le_bytes());
    digest.update(limits.max_children.to_le_bytes());
    digest.update(limits.max_segments.to_le_bytes());
    digest.update(limits.max_links.to_le_bytes());
    digest.update(limits.max_adjacency_rows.to_le_bytes());
    digest.update(limits.max_transition_decisions.to_le_bytes());
    digest.update(limits.max_pair_decisions.to_le_bytes());
    digest.update(limits.max_pair_aggregate_paths.to_le_bytes());
    digest.update(limits.max_pair_lanes.to_le_bytes());
    digest.update(limits.max_pair_document_bytes.to_le_bytes());
    digest.update(limits.max_output_bases.to_le_bytes());
    digest.update(limits.max_snapshot_document_bytes.to_le_bytes());
    digest.update(limits.max_snapshot_bytes.to_le_bytes());
    digest.update(limits.max_loaded_snapshot_bytes.to_le_bytes());
    digest.update(limits.max_loaded_report_accounted_bytes.to_le_bytes());
    digest.update(limits.max_presentation_accounted_bytes.to_le_bytes());
    digest.update(limits.max_validation_scratch_bytes.to_le_bytes());
    digest.update(limits.max_staged_output_bytes.to_le_bytes());
    digest.update(limits.max_manifest_bytes.to_le_bytes());
    digest.finalize().into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundleFailPoint {
    AfterStaging,
    AfterFasta,
    AfterManifest,
    BeforeCommit,
    DestinationAppearsBeforeCommit,
}

/// Render, verify, and atomically publish an experimental multi-k bundle using
/// the exact lease acquired before input was opened.
pub(crate) fn write_unverified_multik_bundle_with_lease(
    lease: RunLease,
    data: &UnverifiedMultiKBundleData,
) -> Result<MultiKBundleOutcome> {
    write_multik_bundle_inner(lease, data, None)
}

fn write_multik_bundle_inner(
    lease: RunLease,
    data: &UnverifiedMultiKBundleData,
    fail_point: Option<BundleFailPoint>,
) -> Result<MultiKBundleOutcome> {
    let prepared = PreparedPortfolio::new(data)?;
    let destination = lease.destination().to_path_buf();
    reject_existing(&destination)?;
    let parent = destination.parent().ok_or_else(|| {
        error(
            ErrorCode::DestinationUnsafePath,
            "multi-k destination has no parent directory",
        )
    })?;
    let staging = tempfile::Builder::new()
        .prefix(".veritasm-multik-stage-")
        .tempdir_in(parent)
        .map_err(|cause| {
            io_error(
                ErrorCode::CommitWrite,
                "create private multi-k staging directory",
                cause,
            )
        })?;
    fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700)).map_err(|cause| {
        io_error(
            ErrorCode::CommitWrite,
            "set multi-k staging permissions",
            cause,
        )
    })?;
    let result = render_verify_and_commit(
        &destination,
        staging.path(),
        &prepared,
        data.limits,
        fail_point,
    );
    match result {
        Ok(outcome) => {
            // The staging pathname no longer exists after the successful
            // no-replace rename; TempDir cleanup is therefore a no-op.
            drop(staging);
            drop(lease);
            Ok(outcome)
        }
        Err(primary) => match staging.close() {
            Ok(()) => Err(primary),
            Err(cleanup) => Err(error(
                ErrorCode::ResourceTemporaryBytes,
                format!(
                    "multi-k bundle failed ({primary}); private staging cleanup also failed: {cleanup}"
                ),
            )),
        },
    }
}

fn render_verify_and_commit(
    destination: &Path,
    staging: &Path,
    prepared: &PreparedPortfolio,
    limits: MultiKBundleLimits,
    fail_point: Option<BundleFailPoint>,
) -> Result<MultiKBundleOutcome> {
    injected(fail_point, BundleFailPoint::AfterStaging)?;
    let schema_dir = staging.join("schema");
    fs::create_dir(&schema_dir)
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "create schema directory", cause))?;
    fs::set_permissions(&schema_dir, fs::Permissions::from_mode(0o700))
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "set schema permissions", cause))?;
    serde_json::from_slice::<serde_json::Value>(RUN_SCHEMA_JSON).map_err(|cause| {
        error(
            ErrorCode::IntegritySchema,
            format!("embedded multi-k run schema is invalid: {cause}"),
        )
    })?;

    let mut stage = Stage::new(staging, limits.max_staged_output_bytes)?;
    stage.generate("segments.fasta", |writer| {
        write_segments_fasta(writer, prepared)
    })?;
    injected(fail_point, BundleFailPoint::AfterFasta)?;
    stage.generate("contigs.fasta", |writer| {
        write_contigs_fasta(writer, prepared)
    })?;
    stage.generate("assembly.gfa", |writer| write_gfa(writer, prepared))?;
    stage.generate("segment_evidence.tsv", |writer| {
        write_segment_evidence(writer, prepared)
    })?;
    stage.generate("adjacency_evidence.tsv", |writer| {
        write_adjacency_evidence(writer, prepared)
    })?;
    stage.generate("transition_decisions.tsv", |writer| {
        write_transition_decisions(writer, prepared)
    })?;
    stage.generate("profile_decisions.tsv", |writer| {
        write_profile_decisions(writer, prepared)
    })?;
    stage.generate("pair_evidence.tsv", |writer| {
        write_pair_evidence_tsv(writer, prepared)
    })?;
    stage.generate("pair_evidence.jsonl", |writer| {
        write_pair_evidence_jsonl(writer, prepared)
    })?;
    stage.generate("run.json", |writer| write_run_json(writer, prepared))?;
    stage.generate("report.html", |writer| write_report_html(writer, prepared))?;
    stage.bytes("schema/multik_run.schema.json", RUN_SCHEMA_JSON)?;

    validate_rendered_run(staging, prepared)?;
    let manifest = render_manifest(&stage.digests);
    let manifest_sha256 = sha256_array(manifest.as_bytes());
    let manifest_len = usize_to_u64(manifest.len(), "multi-k manifest length")?;
    enforce_limit(
        manifest_len,
        limits.max_manifest_bytes,
        ErrorCode::ResourceManifestBytes,
        "multi-k manifest bytes",
    )?;
    stage.bytes("manifest.sha256", manifest.as_bytes())?;
    injected(fail_point, BundleFailPoint::AfterManifest)?;
    let regular_files = usize_to_u64(stage.digests.len(), "multi-k artifact count")?;
    let hashed_bytes = stage.used.checked_sub(manifest_len).ok_or_else(|| {
        error(
            ErrorCode::InternalInvariant,
            "manifest exceeds staged bytes",
        )
    })?;
    verify_bundle_manifest_with_limits(
        staging,
        BundleManifestLimits {
            max_manifest_bytes: limits.max_manifest_bytes,
            max_regular_files: regular_files,
            max_directories: 1,
            max_path_depth: 2,
            max_hashed_artifact_bytes: hashed_bytes,
        },
    )?;
    sync_directory(&schema_dir)?;
    sync_directory(staging)?;
    injected(fail_point, BundleFailPoint::BeforeCommit)?;
    if fail_point == Some(BundleFailPoint::DestinationAppearsBeforeCommit) {
        fs::create_dir(destination).map_err(|cause| {
            io_error(
                ErrorCode::CommitWrite,
                "inject noncooperating multi-k destination",
                cause,
            )
        })?;
        fs::write(destination.join("sentinel"), b"preserve\n").map_err(|cause| {
            io_error(
                ErrorCode::CommitWrite,
                "write noncooperating multi-k sentinel",
                cause,
            )
        })?;
    }
    reject_existing_at_commit(destination)?;

    let outcome = MultiKBundleOutcome {
        destination: destination.to_path_buf(),
        parent_root: prepared.parent_root,
        operational_root: prepared.operational_root,
        manifest_sha256,
        children: usize_to_u64(prepared.children.len(), "multi-k outcome child count")?,
        segments: prepared.total_segments,
    };
    match renameat_with(CWD, staging, CWD, destination, RenameFlags::NOREPLACE) {
        Ok(()) => {}
        Err(cause) if cause == rustix::io::Errno::EXIST => {
            return Err(error(
                ErrorCode::DestinationExisting,
                "multi-k destination appeared before no-replace commit",
            ));
        }
        Err(cause)
            if cause == rustix::io::Errno::NOSYS
                || cause == rustix::io::Errno::NOTSUP
                || cause == rustix::io::Errno::OPNOTSUPP =>
        {
            return Err(error(
                ErrorCode::DestinationNoReplaceUnsupported,
                format!("filesystem does not support no-replace directory rename: {cause}"),
            ));
        }
        Err(cause) => {
            return Err(error(
                ErrorCode::CommitRenameNoReplace,
                format!("multi-k no-replace bundle commit failed: {cause}"),
            ));
        }
    }
    // Commit above is the final fallible success condition. Parent durability
    // is best-effort because a post-commit error cannot safely report failure.
    if let Some(parent) = destination.parent() {
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(outcome)
}

struct Stage<'a> {
    root: &'a Path,
    limit: u64,
    used: u64,
    digests: BTreeMap<String, String>,
}

impl<'a> Stage<'a> {
    fn new(root: &'a Path, limit: u64) -> Result<Self> {
        if limit == 0 {
            return Err(error(
                ErrorCode::ConfigurationInvalidLimit,
                "multi-k staged-output limit must be nonzero",
            ));
        }
        Ok(Self {
            root,
            limit,
            used: 0,
            digests: BTreeMap::new(),
        })
    }

    fn generate<F>(&mut self, relative: &'static str, render: F) -> Result<()>
    where
        F: FnOnce(&mut dyn Write) -> Result<()>,
    {
        validate_relative_path(relative)?;
        let path = self.root.join(relative);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|cause| io_error(ErrorCode::CommitWrite, relative, cause))?;
        let remaining = self.limit.checked_sub(self.used).ok_or_else(|| {
            error(
                ErrorCode::InternalInvariant,
                "multi-k staged byte accounting exceeded its admitted limit",
            )
        })?;
        let mut writer = BoundedDigestWriter::new(file, remaining);
        let rendered = render(&mut writer);
        if writer.limit_exceeded() {
            let _ = fs::remove_file(&path);
            return Err(error(
                ErrorCode::ResourceOutputBytes,
                format!("multi-k staged output exceeds its limit while writing {relative}"),
            ));
        }
        if let Err(cause) = rendered {
            let _ = fs::remove_file(&path);
            return Err(cause);
        }
        let (_file, length, digest) = match writer.finish(relative) {
            Ok(result) => result,
            Err(cause) => {
                let _ = fs::remove_file(&path);
                return Err(cause);
            }
        };
        self.used = checked_add(self.used, length, "multi-k staged-byte count overflow")?;
        if self
            .digests
            .insert(relative.to_owned(), hex_32(digest))
            .is_some()
        {
            let _ = fs::remove_file(&path);
            return Err(error(
                ErrorCode::InternalInvariant,
                "duplicate staged multi-k artifact path",
            ));
        }
        Ok(())
    }

    fn bytes(&mut self, relative: &'static str, bytes: &[u8]) -> Result<()> {
        self.generate(relative, |writer| {
            writer
                .write_all(bytes)
                .map_err(|cause| io_error(ErrorCode::CommitWrite, relative, cause))
        })
    }
}

struct BoundedForwardWriter<W> {
    inner: W,
    remaining: u64,
    limit_exceeded: bool,
}

impl<W> BoundedForwardWriter<W> {
    const fn new(inner: W, maximum_bytes: u64) -> Self {
        Self {
            inner,
            remaining: maximum_bytes,
            limit_exceeded: false,
        }
    }

    const fn limit_exceeded(&self) -> bool {
        self.limit_exceeded
    }

    const fn inner(&self) -> &W {
        &self.inner
    }

    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for BoundedForwardWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let requested = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("write request does not fit u64"))?;
        if requested > self.remaining {
            self.limit_exceeded = true;
            return Err(io::Error::other("configured document byte limit exceeded"));
        }
        let written = self.inner.write(bytes)?;
        self.remaining =
            self.remaining
                .checked_sub(u64::try_from(written).map_err(|_| {
                    io::Error::other("forwarded document byte count does not fit u64")
                })?)
                .ok_or_else(|| io::Error::other("forwarded document byte count underflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct BoundedDigestWriter {
    file: File,
    remaining: u64,
    written: u64,
    digest: Sha256,
    limit_exceeded: bool,
}

impl BoundedDigestWriter {
    fn new(file: File, maximum_bytes: u64) -> Self {
        Self {
            file,
            remaining: maximum_bytes,
            written: 0,
            digest: Sha256::new(),
            limit_exceeded: false,
        }
    }

    const fn limit_exceeded(&self) -> bool {
        self.limit_exceeded
    }

    fn finish(mut self, label: &'static str) -> Result<(File, u64, [u8; 32])> {
        self.file
            .flush()
            .map_err(|cause| io_error(ErrorCode::CommitFlush, label, cause))?;
        self.file
            .sync_all()
            .map_err(|cause| io_error(ErrorCode::CommitSync, label, cause))?;
        let digest = self.digest.finalize().into();
        Ok((self.file, self.written, digest))
    }
}

impl Write for BoundedDigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let requested = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("write request does not fit u64"))?;
        if requested > self.remaining {
            self.limit_exceeded = true;
            return Err(io::Error::other("configured output byte limit exceeded"));
        }
        let written = self.file.write(bytes)?;
        let written_u64 = u64::try_from(written)
            .map_err(|_| io::Error::other("written byte count does not fit u64"))?;
        self.remaining -= written_u64;
        self.written = self
            .written
            .checked_add(written_u64)
            .ok_or_else(|| io::Error::other("written byte count overflow"))?;
        self.digest.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn write_segments_fasta(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    for child in &prepared.children {
        for segment in &child.segments {
            writeln!(
                writer,
                ">{} k={} topology={} child_root={} status=experimental qualification=unqualified intended_use=research_use_only",
                segment_name(segment.id),
                child.k,
                segment.topology.as_str(),
                hex_32(child.child_root),
            )
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "segments.fasta", cause))?;
            write_wrapped_sequence(writer, &segment.sequence, "segments.fasta")?;
        }
    }
    Ok(())
}

fn write_contigs_fasta(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    match prepared.profile {
        PortfolioProfile::DiversityPreserving => {
            for child in &prepared.children {
                for segment in &child.segments {
                    writeln!(
                        writer,
                        ">{} profile=diversity_preserving k={} topology={} child_root={} exact_child_scoped=true status=experimental qualification=unqualified intended_use=research_use_only",
                        segment_name(segment.id),
                        child.k,
                        segment.topology.as_str(),
                        hex_32(child.child_root),
                    )
                    .map_err(|cause| io_error(ErrorCode::CommitWrite, "contigs.fasta", cause))?;
                    write_wrapped_sequence(writer, &segment.sequence, "contigs.fasta")?;
                }
            }
        }
        PortfolioProfile::ExactAgreementConsensus => {
            for presentation in prepared
                .presentations
                .iter()
                .filter(|presentation| has_cross_k_agreement(presentation))
            {
                let members = presentation
                    .members
                    .iter()
                    .map(|(k, id)| format!("k{k}:{}", segment_name(*id)))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(
                    writer,
                    ">{} profile=exact_agreement_consensus topology={} member_count={} members={} exact_byte_identity_only=true status=experimental qualification=unqualified intended_use=research_use_only",
                    presentation_name(presentation.id),
                    presentation.key.topology.as_str(),
                    presentation.members.len(),
                    members,
                )
                .map_err(|cause| io_error(ErrorCode::CommitWrite, "contigs.fasta", cause))?;
                write_wrapped_sequence(writer, &presentation.key.sequence, "contigs.fasta")?;
            }
        }
    }
    Ok(())
}

fn write_gfa(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    writeln!(
        writer,
        "H\tVN:Z:1.0\tTS:Z:experimental_unqualified_research_use_only\tXR:Z:no_cross_k_links"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "assembly.gfa", cause))?;
    for child in &prepared.children {
        for segment in &child.segments {
            writeln!(
                writer,
                "S\t{}\t{}\tKC:i:{}\tTP:Z:{}\tCR:Z:{}\tTR:Z:{}",
                segment_name(segment.id),
                std::str::from_utf8(&segment.sequence).map_err(|_| {
                    error(
                        ErrorCode::InternalInvariant,
                        "validated segment sequence ceased to be UTF-8 DNA",
                    )
                })?,
                child.k,
                segment.topology.as_str(),
                hex_32(child.child_root),
                hex_32(child.transition_root),
            )
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "assembly.gfa", cause))?;
        }
        for link in &child.links {
            writeln!(
                writer,
                "L\t{}\t{}\t{}\t{}\t{}M\tKC:i:{}\tQE:Z:{}\tEO:i:{}\tEF:i:{}",
                segment_name(link.from),
                link.from_orientation.gfa(),
                segment_name(link.to),
                link.to_orientation.gfa(),
                link.overlap_bases,
                child.k,
                hex_32(link.canonical_qmer),
                link.evidence.accepted_window_occurrences,
                link.evidence.distinct_supplied_fragment_instances,
            )
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "assembly.gfa", cause))?;
        }
    }
    Ok(())
}

fn write_segment_evidence(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    writeln!(
        writer,
        "segment_id\tk\tq\ttopology\tlength_bases\tedge_steps\ttotal_edge_support\tminimum_edge_support\tlower_median_edge_support\tmaximum_edge_support\tinternal_transition_rows\tsum_accepted_window_occurrences\tsum_distinct_supplied_fragment_instances\tordered_transition_rows_sha256\tsegment_provenance_sha256\tsource_root\tsource_equivalence_root\tretention_root\ttransition_root\texact_edge_table_sha256\traw_compacted_graph_root\tconstrained_graph_root\tchild_root\tstatus\tqualification\tintended_use\tlimitation"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "segment_evidence.tsv", cause))?;
    for child in &prepared.children {
        for segment in &child.segments {
            writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\texperimental\tunqualified\tresearch_use_only\tlocal_adjacencies_only_not_global_phase",
                segment_name(segment.id),
                child.k,
                child.q,
                segment.topology.as_str(),
                segment.sequence.len(),
                segment.edge_steps,
                segment.total_edge_support,
                segment.minimum_edge_support,
                segment.lower_median_edge_support,
                segment.maximum_edge_support,
                segment.internal_transition_rows,
                segment.sum_accepted_window_occurrences,
                segment.sum_distinct_supplied_fragment_instances,
                hex_32(segment.ordered_transition_rows_sha256),
                hex_32(segment.provenance_sha256),
                hex_32(child.source_root),
                hex_32(child.source_equivalence_root),
                hex_32(child.retention.retention_root),
                hex_32(child.transition_root),
                hex_32(child.exact_edge_table_sha256),
                hex_32(child.raw_compacted_graph_root),
                hex_32(child.constrained_graph_root),
                hex_32(child.child_root),
            )
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "segment_evidence.tsv", cause))?;
        }
    }
    Ok(())
}

fn write_adjacency_evidence(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    writeln!(
        writer,
        "k\tq\tcanonical_qmer_hex\taccepted_window_occurrences\tdistinct_supplied_fragment_instances\tsorted_event_frames_sha256\tsource_root\ttransition_root\tevidence_kind\tstatus\tqualification\tintended_use"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "adjacency_evidence.tsv", cause))?;
    for child in &prepared.children {
        for row in &child.adjacency_rows {
            writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\toriginal_read_transition\texperimental\tunqualified\tresearch_use_only",
                child.k,
                child.q,
                hex_32(row.canonical_qmer),
                row.evidence.accepted_window_occurrences,
                row.evidence.distinct_supplied_fragment_instances,
                hex_32(row.evidence.sorted_event_frames_sha256),
                hex_32(child.source_root),
                hex_32(child.transition_root),
            )
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "adjacency_evidence.tsv", cause))?;
        }
    }
    Ok(())
}

fn write_transition_decisions(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    writeln!(
        writer,
        "k\tq\tcanonical_qmer_hex\torigin\tdecision\taccepted_window_occurrences\tdistinct_supplied_fragment_instances\tsorted_event_frames_sha256\tchild_root\tstatus\tqualification\tintended_use"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "transition_decisions.tsv", cause))?;
    for child in &prepared.children {
        for row in &child.transition_decisions {
            let (occurrences, fragments, digest) = row.evidence.map_or_else(
                || {
                    (
                        "not_available".to_owned(),
                        "not_available".to_owned(),
                        "not_available".to_owned(),
                    )
                },
                |evidence| {
                    (
                        evidence.accepted_window_occurrences.to_string(),
                        evidence.distinct_supplied_fragment_instances.to_string(),
                        hex_32(evidence.sorted_event_frames_sha256),
                    )
                },
            );
            writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\texperimental\tunqualified\tresearch_use_only",
                child.k,
                child.q,
                hex_32(row.canonical_qmer),
                row.origin,
                row.status,
                occurrences,
                fragments,
                digest,
                hex_32(child.child_root),
            )
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "transition_decisions.tsv", cause))?;
        }
    }
    Ok(())
}

fn write_profile_decisions(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    writeln!(
        writer,
        "profile\tk\tchild_segment_id\tpresentation_id\tdecision\texact_byte_identity_only\tpreferred_child_selection\tstatus\tqualification\tintended_use"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "profile_decisions.tsv", cause))?;
    match prepared.profile {
        PortfolioProfile::DiversityPreserving => {
            for child in &prepared.children {
                for segment in &child.segments {
                    writeln!(
                        writer,
                        "diversity_preserving\t{}\t{}\t{}\tincluded_child_scoped\ttrue\tnone\texperimental\tunqualified\tresearch_use_only",
                        child.k,
                        segment_name(segment.id),
                        segment_name(segment.id),
                    )
                    .map_err(|cause| {
                        io_error(ErrorCode::CommitWrite, "profile_decisions.tsv", cause)
                    })?;
                }
            }
        }
        PortfolioProfile::ExactAgreementConsensus => {
            for presentation in &prepared.presentations {
                let decision = if !has_cross_k_agreement(presentation) {
                    "excluded_no_cross_k_exact_agreement"
                } else {
                    "collapsed_exact_duplicate_presentation"
                };
                for (k, id) in &presentation.members {
                    writeln!(
                        writer,
                        "exact_agreement_consensus\t{}\t{}\t{}\t{}\ttrue\tnone\texperimental\tunqualified\tresearch_use_only",
                        k,
                        segment_name(*id),
                        presentation_name(presentation.id),
                        decision,
                    )
                    .map_err(|cause| {
                        io_error(ErrorCode::CommitWrite, "profile_decisions.tsv", cause)
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn write_pair_evidence_tsv(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    writeln!(
        writer,
        "k\tstate\tplacement_domain\treport_root\tpair_graph_root\tplacement_producer_root\tauthenticated_path_result_root\tauthenticated_fragments\tcalibration_fragments\treplay_fragments\tlinear_unitig_unavailable_reads\tsupported_unique_existing_paths\ttrivial_within_unitig\tabstained\tindeterminate\tavailable_aggregate_constraints\tmodel_lanes\tsequence_modified\tstatus\tqualification\tintended_use"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "pair_evidence.tsv", cause))?;
    for child in &prepared.children {
        let pair = &child.pair_evidence;
        let payload = &pair.payload;
        writeln!(
            writer,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\tfalse\texperimental\tunqualified\tresearch_use_only",
            child.k,
            payload.state.as_str(),
            payload.placement_domain,
            hex_32(pair.report_root),
            optional_digest(payload.exact_pair_graph_root),
            optional_digest(payload.placement_producer_root),
            optional_digest(payload.authenticated_pair_path_result_root),
            payload.authenticated_fragments,
            payload.calibration_fragments,
            payload.replay_fragments,
            payload.unavailable_possible_graph_junction_reads,
            payload.supported_unique_existing_paths,
            payload.trivial_within_unitig,
            payload.abstained,
            payload.indeterminate,
            payload.available_aggregate_constraints,
            payload.lane_models.len(),
        )
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "pair_evidence.tsv", cause))?;
    }
    Ok(())
}

fn write_pair_evidence_jsonl(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    for child in &prepared.children {
        let pair = &child.pair_evidence;
        write!(
            writer,
            "{{\"k\":{},\"state\":\"{}\",\"report_root\":\"{}\",\"evidence\":",
            child.k,
            pair.payload.state.as_str(),
            hex_32(pair.report_root),
        )
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "pair_evidence.jsonl", cause))?;
        if pair.payload.authenticated_document_json.is_empty() {
            writer
                .write_all(b"null")
                .map_err(|cause| io_error(ErrorCode::CommitWrite, "pair_evidence.jsonl", cause))?;
        } else {
            writer
                .write_all(pair.payload.authenticated_document_json.as_bytes())
                .map_err(|cause| io_error(ErrorCode::CommitWrite, "pair_evidence.jsonl", cause))?;
        }
        writer
            .write_all(b"}\n")
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "pair_evidence.jsonl", cause))?;
    }
    Ok(())
}

fn optional_digest(value: [u8; 32]) -> String {
    if value == [0; 32] {
        "not_applicable".to_owned()
    } else {
        hex_32(value)
    }
}

fn nonzero_digest(value: [u8; 32]) -> Option<String> {
    (value != [0; 32]).then(|| hex_32(value))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunDocument {
    schema: String,
    status: String,
    qualification: String,
    intended_use: String,
    source_root: String,
    parent_root: String,
    operational_root: String,
    exact_agreement_root: String,
    profile: String,
    pair_evidence: String,
    quality_correction: String,
    input_mode: String,
    support_unit: String,
    min_base_quality: u8,
    execution: RunExecution,
    children: Vec<RunChild>,
    totals: RunTotals,
    limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunExecution {
    execution_threads: u16,
    accounting_scope: String,
    max_aggregate_accounted_memory_bytes: u64,
    projected_aggregate_accounted_memory_bytes: u64,
    max_aggregate_temp_bytes: u64,
    max_final_staging_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunChild {
    k: u8,
    q: u8,
    minimizer_length: u8,
    virtual_bucket_count: u32,
    source_equivalence_root: String,
    retention_root: String,
    transition_root: String,
    exact_edge_table_sha256: String,
    raw_compacted_graph_root: String,
    constrained_graph_root: String,
    child_root: String,
    child_authentication_root: String,
    retention_rule: String,
    retention_minimum_support: Option<u64>,
    raw_keys: u64,
    retained_keys: u64,
    discarded_keys: u64,
    raw_support: u64,
    retained_support: u64,
    discarded_support: u64,
    topology_candidates: u64,
    admitted_topology_candidates: u64,
    excluded_no_original_read_witness: u64,
    ledger_rows: u64,
    eligible_ledger_rows: u64,
    excluded_endpoint_not_retained: u64,
    segments: u64,
    linear_segments: u64,
    closed_segments: u64,
    witnessed_links: u64,
    output_bases: u64,
    accounted_peak_bytes: u64,
    pair_evidence: RunChildPairEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunChildPairEvidence {
    state: String,
    placement_domain: String,
    report_root: String,
    exact_pair_graph_root: Option<String>,
    placement_producer_root: Option<String>,
    authenticated_path_result_root: Option<String>,
    supplied_fragments: u64,
    configured_fragment_limit: u64,
    authenticated_fragments: u64,
    calibration_fragments: u64,
    replay_fragments: u64,
    unavailable_possible_graph_junction_reads: u64,
    supported_unique_existing_paths: u64,
    trivial_within_unitig: u64,
    abstained: u64,
    indeterminate: u64,
    available_aggregate_constraints: u64,
    lane_models: Vec<ReportPairLaneModel>,
    changes_sequence_or_graph: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunTotals {
    children: u64,
    child_segments: u64,
    child_links: u64,
    adjacency_rows: u64,
    transition_decisions: u64,
    child_output_bases: u64,
    exact_agreement_presentations: u64,
}

fn expected_run_document(prepared: &PreparedPortfolio) -> Result<RunDocument> {
    let mut children = Vec::new();
    children
        .try_reserve_exact(prepared.children.len())
        .map_err(|cause| {
            error(
                ErrorCode::ResourceMemory,
                format!("cannot reserve multi-k run child summaries: {cause}"),
            )
        })?;
    for child in &prepared.children {
        children.push(RunChild {
            k: child.k,
            q: child.q,
            minimizer_length: child.minimizer_length,
            virtual_bucket_count: child.virtual_bucket_count,
            source_equivalence_root: hex_32(child.source_equivalence_root),
            retention_root: hex_32(child.retention.retention_root),
            transition_root: hex_32(child.transition_root),
            exact_edge_table_sha256: hex_32(child.exact_edge_table_sha256),
            raw_compacted_graph_root: hex_32(child.raw_compacted_graph_root),
            constrained_graph_root: hex_32(child.constrained_graph_root),
            child_root: hex_32(child.child_root),
            child_authentication_root: hex_32(child.child_authentication_root),
            retention_rule: match child.retention.rule {
                ReportRetentionRule::RetainAll => "retain_all",
                ReportRetentionRule::InclusiveSupport => "inclusive_support",
            }
            .to_owned(),
            retention_minimum_support: child.retention.minimum_support,
            raw_keys: child.retention.raw_keys,
            retained_keys: child.retention.retained_keys,
            discarded_keys: child.retention.discarded_keys,
            raw_support: child.retention.raw_support,
            retained_support: child.retention.retained_support,
            discarded_support: child.retention.discarded_support,
            topology_candidates: child.conservation.topology_candidates,
            admitted_topology_candidates: child.conservation.admitted_topology_candidates,
            excluded_no_original_read_witness: child.conservation.excluded_no_original_read_witness,
            ledger_rows: child.conservation.ledger_rows,
            eligible_ledger_rows: child.conservation.eligible_ledger_rows,
            excluded_endpoint_not_retained: child.conservation.excluded_endpoint_not_retained,
            segments: child.conservation.segments,
            linear_segments: child.conservation.linear_segments,
            closed_segments: child.conservation.closed_segments,
            witnessed_links: child.conservation.witnessed_links,
            output_bases: child.conservation.output_bases,
            accounted_peak_bytes: child.conservation.accounted_peak_bytes,
            pair_evidence: RunChildPairEvidence {
                state: child.pair_evidence.payload.state.as_str().to_owned(),
                placement_domain: child.pair_evidence.payload.placement_domain.clone(),
                report_root: hex_32(child.pair_evidence.report_root),
                exact_pair_graph_root: nonzero_digest(
                    child.pair_evidence.payload.exact_pair_graph_root,
                ),
                placement_producer_root: nonzero_digest(
                    child.pair_evidence.payload.placement_producer_root,
                ),
                authenticated_path_result_root: nonzero_digest(
                    child
                        .pair_evidence
                        .payload
                        .authenticated_pair_path_result_root,
                ),
                supplied_fragments: child.pair_evidence.payload.supplied_fragments,
                configured_fragment_limit: child.pair_evidence.payload.configured_fragment_limit,
                authenticated_fragments: child.pair_evidence.payload.authenticated_fragments,
                calibration_fragments: child.pair_evidence.payload.calibration_fragments,
                replay_fragments: child.pair_evidence.payload.replay_fragments,
                unavailable_possible_graph_junction_reads: child
                    .pair_evidence
                    .payload
                    .unavailable_possible_graph_junction_reads,
                supported_unique_existing_paths: child
                    .pair_evidence
                    .payload
                    .supported_unique_existing_paths,
                trivial_within_unitig: child.pair_evidence.payload.trivial_within_unitig,
                abstained: child.pair_evidence.payload.abstained,
                indeterminate: child.pair_evidence.payload.indeterminate,
                available_aggregate_constraints: child
                    .pair_evidence
                    .payload
                    .available_aggregate_constraints,
                lane_models: child.pair_evidence.payload.lane_models.clone(),
                changes_sequence_or_graph: false,
            },
        });
    }
    Ok(RunDocument {
        schema: RUN_SCHEMA.to_owned(),
        status: "experimental".to_owned(),
        qualification: "unqualified".to_owned(),
        intended_use: "research_use_only".to_owned(),
        source_root: hex_32(prepared.source_root),
        parent_root: hex_32(prepared.parent_root),
        operational_root: hex_32(prepared.operational_root),
        exact_agreement_root: hex_32(prepared.exact_agreement_root),
        profile: prepared.profile.as_str().to_owned(),
        pair_evidence: prepared.pair_evidence.as_str().to_owned(),
        quality_correction: prepared.quality_correction.as_str().to_owned(),
        input_mode: prepared.input_mode.clone(),
        support_unit: prepared.support_unit.clone(),
        min_base_quality: prepared.min_base_quality,
        execution: RunExecution {
            execution_threads: prepared.execution.execution_threads,
            accounting_scope: "modelled_owned_payload_not_process_rss".to_owned(),
            max_aggregate_accounted_memory_bytes: prepared
                .execution
                .max_aggregate_accounted_memory_bytes,
            projected_aggregate_accounted_memory_bytes: prepared
                .execution
                .projected_aggregate_accounted_memory_bytes,
            max_aggregate_temp_bytes: prepared.execution.max_aggregate_temp_bytes,
            max_final_staging_bytes: prepared.execution.max_final_staging_bytes,
        },
        children,
        totals: RunTotals {
            children: usize_to_u64(prepared.children.len(), "run child count")?,
            child_segments: prepared.total_segments,
            child_links: prepared.total_links,
            adjacency_rows: prepared.total_adjacencies,
            transition_decisions: prepared.total_decisions,
            child_output_bases: prepared.total_output_bases,
            exact_agreement_presentations: usize_to_u64(
                prepared
                    .presentations
                    .iter()
                    .filter(|presentation| has_cross_k_agreement(presentation))
                    .count(),
                "exact-agreement presentation count",
            )?,
        },
        limitations: portfolio_limitations()
            .iter()
            .map(|limitation| (*limitation).to_owned())
            .collect(),
    })
}

fn has_cross_k_agreement(presentation: &Presentation) -> bool {
    presentation
        .members
        .windows(2)
        .any(|members| members[0].0 != members[1].0)
}

const fn portfolio_limitations() -> &'static [&'static str] {
    &[
        "experimental_unqualified_research_use_only",
        "independent_children_only_no_cross_k_splicing_or_preferred_child_selection",
        "exact_agreement_consensus_means_exact_byte_and_topology_deduplication_only",
        "local_original_read_adjacencies_do_not_establish_global_haplotype_phase",
        "paired_evidence_is_not_applicable_to_single_end_input",
        "paired_evidence_is_authenticated_but_unqualified_and_linear_unitig_only",
        "paired_evidence_never_changes_graph_or_sequence_and_cannot_create_scaffolds_or_joins",
        "quality_correction_is_disabled_unqualified",
        "reads_shorter_than_a_child_k_contribute_no_edge_to_that_child",
        "whole_resident_reporting_alpha_has_encoded_and_owned_payload_limits_not_an_rss_promise",
        "report_files_and_digests_are_not_promotable_evidence_capabilities",
        "no_accuracy_sensitivity_performance_or_clinical_claim_is_made",
    ]
}

fn write_run_json(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    let document = expected_run_document(prepared)?;
    serde_json::to_writer_pretty(&mut *writer, &document).map_err(|cause| {
        error(
            ErrorCode::CommitWrite,
            format!("cannot stream run.json: {cause}"),
        )
    })?;
    writer
        .write_all(b"\n")
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "run.json", cause))
}

fn validate_rendered_run(staging: &Path, prepared: &PreparedPortfolio) -> Result<()> {
    let expected = expected_run_document(prepared)?;
    let file = open_regular_file(
        &staging.join("run.json"),
        ErrorCode::IntegrityArtifact,
        "run.json",
    )?;
    let observed: RunDocument = serde_json::from_reader(file).map_err(|cause| {
        error(
            ErrorCode::IntegrityArtifact,
            format!("cannot decode staged run.json: {cause}"),
        )
    })?;
    if observed != expected {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            "staged run.json differs from typed portfolio data",
        ));
    }
    let schema: serde_json::Value = serde_json::from_slice(RUN_SCHEMA_JSON).map_err(|cause| {
        error(
            ErrorCode::IntegritySchema,
            format!("embedded multi-k schema is malformed: {cause}"),
        )
    })?;
    if schema.get("additionalProperties") != Some(&serde_json::Value::Bool(false)) {
        return Err(error(
            ErrorCode::IntegritySchema,
            "embedded multi-k run schema is not closed to unknown top-level fields",
        ));
    }
    Ok(())
}

fn write_report_html(writer: &mut dyn Write, prepared: &PreparedPortfolio) -> Result<()> {
    writeln!(
        writer,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>VeritAsm experimental multi-k report</title><style>body{{font:16px system-ui,sans-serif;max-width:1120px;margin:2rem auto;padding:0 1rem;color:#17202a}}.warning{{border:3px solid #9b1c1c;background:#fff2f2;padding:1rem}}code{{word-break:break-all}}table{{border-collapse:collapse;width:100%;margin:1rem 0}}th,td{{border:1px solid #777;padding:.45rem;text-align:left}}th{{background:#eef2f4}}caption{{font-weight:700;text-align:left;margin:.5rem 0}}li{{margin:.35rem 0}}</style></head><body>"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    writeln!(
        writer,
        "<main><h1>VeritAsm experimental multi-k report</h1><section class=\"warning\"><strong>{SCIENTIFIC_LABEL}</strong><p>This is an unqualified algorithm experiment, not a clinical diagnostic. It makes no accuracy, sensitivity, performance, sample-content, or superiority claim.</p></section>"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    writeln!(
        writer,
        "<h2>Run identity</h2><dl><dt>Profile</dt><dd>{}</dd><dt>Source root</dt><dd><code>{}</code></dd><dt>Parent root</dt><dd><code>{}</code></dd><dt>Operational root</dt><dd><code>{}</code></dd><dt>Execution threads</dt><dd>{}</dd><dt>Memory accounting</dt><dd>modelled owned payload, not process RSS</dd><dt>Pair evidence</dt><dd>{}</dd><dt>Quality correction</dt><dd>{}</dd></dl>",
        prepared.profile.as_str(),
        hex_32(prepared.source_root),
        hex_32(prepared.parent_root),
        hex_32(prepared.operational_root),
        prepared.execution.execution_threads,
        prepared.pair_evidence.as_str(),
        prepared.quality_correction.as_str(),
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    writeln!(
        writer,
        "<h2>Independent children</h2><table><caption>No child is selected over another and no cross-k sequence is constructed.</caption><thead><tr><th>k</th><th>segments</th><th>links</th><th>ledger rows</th><th>output bases</th><th>child root</th></tr></thead><tbody>"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    for child in &prepared.children {
        writeln!(
            writer,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td></tr>",
            child.k,
            child.conservation.segments,
            child.conservation.witnessed_links,
            child.conservation.ledger_rows,
            child.conservation.output_bases,
            hex_32(child.child_root),
        )
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    }
    writeln!(
        writer,
        "</tbody></table><h2>Paired evidence</h2><p>Paired evidence only annotates already existing graph paths. It never changes FASTA/GFA sequence, adds an edge, or creates a scaffold.</p><table><thead><tr><th>k</th><th>state</th><th>model lanes</th><th>unique existing paths</th><th>abstained</th><th>indeterminate</th><th>junction-domain unavailable reads</th><th>authenticated result root</th></tr></thead><tbody>"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    for child in &prepared.children {
        let pair = &child.pair_evidence.payload;
        writeln!(
            writer,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td></tr>",
            child.k,
            pair.state.as_str(),
            pair.lane_models.len(),
            pair.supported_unique_existing_paths,
            pair.abstained,
            pair.indeterminate,
            pair.unavailable_possible_graph_junction_reads,
            optional_digest(pair.authenticated_pair_path_result_root),
        )
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    }
    writeln!(writer, "</tbody></table><h2>Explicit limitations</h2><ul>")
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    for limitation in portfolio_limitations() {
        writeln!(writer, "<li><code>{limitation}</code></li>")
            .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))?;
    }
    writeln!(
        writer,
        "</ul><p>See <code>run.json</code>, evidence TSV files, <code>assembly.gfa</code>, and <code>manifest.sha256</code> for machine-readable records.</p></main></body></html>"
    )
    .map_err(|cause| io_error(ErrorCode::CommitWrite, "report.html", cause))
}

fn write_wrapped_sequence(
    writer: &mut dyn Write,
    sequence: &[u8],
    label: &'static str,
) -> Result<()> {
    for line in sequence.chunks(80) {
        writer
            .write_all(line)
            .and_then(|()| writer.write_all(b"\n"))
            .map_err(|cause| io_error(ErrorCode::CommitWrite, label, cause))?;
    }
    Ok(())
}

fn compare_snapshot_encoding(
    reader: &mut dyn Read,
    payload: &UnverifiedChildSnapshotPayload,
) -> Result<()> {
    let mut comparator = ExactReaderComparator { reader };
    serde_json::to_writer(&mut comparator, payload).map_err(|cause| {
        error(
            ErrorCode::IntegrityArtifact,
            format!("compare canonical child snapshot encoding: {cause}"),
        )
    })?;
    comparator.finish("child snapshot")
}

struct ExactReaderComparator<'a> {
    reader: &'a mut dyn Read,
}

impl ExactReaderComparator<'_> {
    fn finish(&mut self, label: &'static str) -> Result<()> {
        let mut byte = [0_u8; 1];
        match self.reader.read(&mut byte) {
            Ok(0) => Ok(()),
            Ok(_) => Err(error(
                ErrorCode::IntegrityArtifact,
                format!("{label} contains trailing bytes"),
            )),
            Err(cause) => Err(io_error(ErrorCode::IntegrityArtifact, label, cause)),
        }
    }
}

impl Write for ExactReaderComparator<'_> {
    fn write(&mut self, expected: &[u8]) -> io::Result<usize> {
        let mut offset = 0_usize;
        let mut observed = [0_u8; 8192];
        while offset < expected.len() {
            let length = observed.len().min(expected.len() - offset);
            self.reader.read_exact(&mut observed[..length])?;
            if observed[..length] != expected[offset..offset + length] {
                return Err(io::Error::other("bytes differ from canonical encoding"));
            }
            offset += length;
        }
        Ok(expected.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn open_regular_file(path: &Path, code: ErrorCode, label: &'static str) -> Result<File> {
    let metadata = fs::symlink_metadata(path).map_err(|cause| io_error(code, label, cause))?;
    if !metadata.file_type().is_file() {
        return Err(error(code, format!("{label} is not a regular file")));
    }
    let file = File::open(path).map_err(|cause| io_error(code, label, cause))?;
    if !file
        .metadata()
        .map_err(|cause| io_error(code, label, cause))?
        .is_file()
    {
        return Err(error(
            code,
            format!("{label} changed file type while opening"),
        ));
    }
    Ok(file)
}

fn render_manifest(digests: &BTreeMap<String, String>) -> String {
    let mut output = String::new();
    for (path, digest) in digests {
        let _ = writeln!(output, "{digest}  {path}");
    }
    output
}

fn sha256_array(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn pair_report_root(payload: &ReportPairEvidencePayload) -> Result<[u8; 32]> {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut writer = HashWriter(Sha256::new());
    writer.0.update(PAIR_REPORT_ROOT_DOMAIN);
    serde_json::to_writer(&mut writer, payload).map_err(|cause| {
        error(
            ErrorCode::IntegrityArtifact,
            format!("cannot hash canonical pair-report payload: {cause}"),
        )
    })?;
    Ok(writer.0.finalize().into())
}

fn hex_32(bytes: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn segment_name(id: [u8; 32]) -> String {
    format!("segment_{}", hex_32(id))
}

fn presentation_name(id: [u8; 32]) -> String {
    format!("presentation_{}", hex_32(id))
}

fn update_len_bytes(digest: &mut Sha256, bytes: &[u8]) -> Result<()> {
    digest.update(usize_to_u64(bytes.len(), "digest byte-string length")?.to_le_bytes());
    digest.update(bytes);
    Ok(())
}

fn validate_tsv_atom(value: &str, label: &'static str) -> Result<()> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\t' | b'\n' | b'\r'))
    {
        return Err(error(
            ErrorCode::IntegrityArtifact,
            format!("{label} is empty or contains a TSV control byte"),
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<()> {
    let parsed = Path::new(path);
    if path.is_empty()
        || path
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\t' | b'\n' | b'\r' | b'\\'))
        || parsed.is_absolute()
        || parsed
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(error(
            ErrorCode::IntegrityManifest,
            format!("unsafe multi-k artifact path: {path:?}"),
        ));
    }
    Ok(())
}

fn reject_existing(destination: &Path) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(error(
            ErrorCode::DestinationExisting,
            format!("multi-k destination already exists: {destination:?}"),
        )),
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(io_error(
            ErrorCode::DestinationUnsafePath,
            "inspect multi-k destination",
            cause,
        )),
    }
}

fn reject_existing_at_commit(destination: &Path) -> Result<()> {
    reject_existing(destination)
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|cause| io_error(ErrorCode::CommitSync, "sync multi-k directory", cause))
}

fn injected(observed: Option<BundleFailPoint>, expected: BundleFailPoint) -> Result<()> {
    if observed == Some(expected) {
        Err(error(
            ErrorCode::InternalUnexpected,
            format!("injected multi-k failure at {expected:?}"),
        ))
    } else {
        Ok(())
    }
}

fn enforce_limit(observed: u64, maximum: u64, code: ErrorCode, label: &'static str) -> Result<()> {
    if observed > maximum {
        Err(error(
            code,
            format!("{label} requires {observed}, exceeding limit {maximum}"),
        ))
    } else {
        Ok(())
    }
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| error(ErrorCode::ResourceIntegerOverflow, context))
}

fn usize_to_u64(value: usize, context: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| error(ErrorCode::ResourceIntegerOverflow, context))
}

fn error(code: ErrorCode, context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(code, context)
}

fn io_error(code: ErrorCode, context: &'static str, cause: io::Error) -> VeritasmError {
    error(code, format!("{context}: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn evidence(tag: u8) -> ReportTransitionEvidence {
        ReportTransitionEvidence {
            accepted_window_occurrences: 1,
            distinct_supplied_fragment_instances: 1,
            sorted_event_frames_sha256: [tag; 32],
        }
    }

    fn valid_child(k: u8, sequence: &[u8], root_tag: u8) -> UnverifiedChildSnapshotPayload {
        assert_eq!(sequence.len(), usize::from(k) + 1);
        let child_root = [root_tag; 32];
        let parent_unitig_id = [root_tag.wrapping_add(1); 32];
        let mut steps = Vec::new();
        for window in sequence.windows(usize::from(k)) {
            let literal = encode_dna_u128(window).unwrap();
            let reverse = reverse_complement_u128(literal, k);
            steps.push(ReportEdgeStep {
                canonical_kmer: packed_u128_bytes(literal.min(reverse)),
                orientation: match literal.cmp(&reverse) {
                    std::cmp::Ordering::Equal => ReportEdgeOrientation::SelfReverseComplement,
                    std::cmp::Ordering::Less => ReportEdgeOrientation::Canonical,
                    std::cmp::Ordering::Greater => ReportEdgeOrientation::ReverseComplement,
                },
                support: 1,
            });
        }
        let qmer = canonical_packed_bytes(sequence).unwrap();
        let transition_evidence = evidence(root_tag.wrapping_add(2));
        let mut transition_digest = Sha256::new();
        transition_digest.update(TRANSITION_SET_DOMAIN);
        transition_digest.update(child_root);
        transition_digest.update(1_u64.to_le_bytes());
        hash_report_transition(&mut transition_digest, qmer, transition_evidence);
        let mut segment = ReportSegment {
            id: [1; 32],
            parent_unitig_id,
            parent_start_step: 0,
            parent_end_step_exclusive: 2,
            topology: ReportTopology::Linear,
            sequence: sequence.to_vec(),
            steps,
            edge_steps: 2,
            total_edge_support: 2,
            minimum_edge_support: 1,
            lower_median_edge_support: 1,
            maximum_edge_support: 1,
            internal_transition_rows: 1,
            sum_accepted_window_occurrences: 1,
            sum_distinct_supplied_fragment_instances: 1,
            ordered_transition_rows_sha256: transition_digest.finalize().into(),
            provenance_sha256: [1; 32],
        };
        segment.id = report_segment_id(k, child_root, &segment);
        segment.provenance_sha256 = report_segment_provenance(k, child_root, &segment);
        let raw_keys = 2;
        UnverifiedChildSnapshotPayload {
            schema: SNAPSHOT_SCHEMA.to_owned(),
            k,
            q: k + 1,
            minimizer_length: k.min(3),
            virtual_bucket_count: 8,
            support_unit: "supplied_fragment_instance".to_owned(),
            source_root: [7; 32],
            source_equivalence_root: [root_tag.wrapping_add(3); 32],
            retention: ReportRetention {
                rule: ReportRetentionRule::RetainAll,
                minimum_support: None,
                raw_keys,
                retained_keys: raw_keys,
                discarded_keys: 0,
                raw_support: raw_keys,
                retained_support: raw_keys,
                discarded_support: 0,
                decision_ledger_root: [root_tag.wrapping_add(4); 32],
                retained_table_root: [root_tag.wrapping_add(5); 32],
                retention_root: [root_tag.wrapping_add(6); 32],
            },
            transition_root: [root_tag.wrapping_add(7); 32],
            exact_edge_table_sha256: [root_tag.wrapping_add(8); 32],
            raw_compacted_graph_root: [root_tag.wrapping_add(9); 32],
            constrained_graph_root: [root_tag.wrapping_add(10); 32],
            child_root,
            child_authentication_root: [root_tag.wrapping_add(11); 32],
            segments: vec![segment],
            links: Vec::new(),
            adjacency_rows: vec![ReportAdjacency {
                canonical_qmer: qmer,
                evidence: transition_evidence,
            }],
            transition_decisions: vec![ReportTransitionDecision {
                canonical_qmer: qmer,
                origin: format!("unitig_interior:{}:0", hex_32(parent_unitig_id)),
                status: "admitted_original_read".to_owned(),
                evidence: Some(transition_evidence),
            }],
            conservation: ReportConservation {
                input_canonical_edges: raw_keys,
                represented_canonical_edges: raw_keys,
                input_edge_support: raw_keys,
                represented_edge_support: raw_keys,
                topology_candidates: 1,
                admitted_topology_candidates: 1,
                excluded_no_original_read_witness: 0,
                ledger_rows: 1,
                eligible_ledger_rows: 1,
                excluded_endpoint_not_retained: 0,
                segments: 1,
                linear_segments: 1,
                closed_segments: 0,
                witnessed_links: 0,
                output_bases: sequence.len() as u64,
                accounted_peak_bytes: 1,
            },
            pair_evidence: ReportPairEvidence::not_applicable_single_end([7; 32]).unwrap(),
        }
    }

    fn execution() -> UnverifiedExecutionReport {
        UnverifiedExecutionReport {
            execution_threads: 1,
            max_aggregate_accounted_memory_bytes: 1 << 30,
            projected_aggregate_accounted_memory_bytes: 1 << 29,
            max_aggregate_temp_bytes: 1 << 30,
            max_final_staging_bytes: 1 << 28,
        }
    }

    fn data(
        children: Vec<ChildSnapshotRef>,
        profile: PortfolioProfile,
    ) -> UnverifiedMultiKBundleData {
        UnverifiedMultiKBundleData {
            source_root: [7; 32],
            input_mode: "single_end".to_owned(),
            support_unit: "supplied_fragment_instance".to_owned(),
            min_base_quality: 0,
            profile,
            pair_evidence: PairEvidenceState::NotApplicableSingleEnd,
            quality_correction: DisabledCapabilityState::DisabledUnqualified,
            execution: execution(),
            children,
            limits: MultiKBundleLimits::default(),
        }
    }

    fn snapshot(work: &Path, child: &UnverifiedChildSnapshotPayload) -> ChildSnapshotRef {
        write_unverified_child_snapshot(work, child, 1 << 20, 1 << 20, 1 << 20).unwrap()
    }

    #[test]
    fn snapshot_round_trip_and_sequence_tamper_rejection() {
        let work = tempfile::tempdir().unwrap();
        let child = valid_child(3, b"AAAC", 17);
        validate_child(&child, 1 << 20).unwrap();
        let reference = snapshot(work.path(), &child);
        assert_eq!(reference.load_verified().unwrap(), child);

        let mut sequence_tamper = child.clone();
        sequence_tamper.segments[0].sequence[1] = b'C';
        assert_eq!(
            validate_child(&sequence_tamper, 1 << 20)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut copied_root_tamper = child;
        copied_root_tamper.child_root[0] ^= 1;
        assert_eq!(
            validate_child(&copied_root_tamper, 1 << 20)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut pair_report_tamper = valid_child(3, b"AAAC", 19);
        pair_report_tamper.pair_evidence.report_root[0] ^= 1;
        assert_eq!(
            validate_child(&pair_report_tamper, 1 << 20)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn snapshot_descriptor_rejects_in_place_and_path_substitution() {
        let child = valid_child(3, b"AAAC", 31);

        let in_place_work = tempfile::tempdir().unwrap();
        let in_place = snapshot(in_place_work.path(), &child);
        let mut bytes = fs::read(&in_place.path).unwrap();
        bytes[0] ^= 1;
        fs::write(&in_place.path, bytes).unwrap();
        fs::set_permissions(&in_place.path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            in_place.load_verified().unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        let substituted_work = tempfile::tempdir().unwrap();
        let substituted = snapshot(substituted_work.path(), &child);
        let displaced = substituted_work.path().join("displaced.snapshot");
        fs::rename(&substituted.path, &displaced).unwrap();
        fs::copy(&displaced, &substituted.path).unwrap();
        fs::set_permissions(&substituted.path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            substituted.load_verified().unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn snapshot_descriptor_rejects_symlink_and_platform_fifo_without_opening_them() {
        let child = valid_child(3, b"AAAC", 37);

        let symlink_work = tempfile::tempdir().unwrap();
        let symlinked = snapshot(symlink_work.path(), &child);
        let original = symlink_work.path().join("symlink-original.snapshot");
        fs::rename(&symlinked.path, &original).unwrap();
        symlink(&original, &symlinked.path).unwrap();
        assert_eq!(
            symlinked.load_verified().unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        #[cfg(target_os = "linux")]
        {
            let fifo_work = tempfile::tempdir().unwrap();
            let fifo = snapshot(fifo_work.path(), &child);
            let fifo_original = fifo_work.path().join("fifo-original.snapshot");
            fs::rename(&fifo.path, &fifo_original).unwrap();
            rustix::fs::mkfifoat(CWD, &fifo.path, rustix::fs::Mode::from_raw_mode(0o600)).unwrap();
            assert_eq!(
                fifo.load_verified().unwrap_err().code(),
                ErrorCode::IntegrityArtifact
            );
        }
    }

    #[test]
    fn child_validation_scratch_has_exact_boundaries() {
        let child = valid_child(3, b"AAAC", 41);
        let required_support_bytes = (child.segments[0].steps.len() * size_of::<u64>()) as u64;
        assert_eq!(
            validate_child(&child, required_support_bytes - 1)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
        validate_child(&child, required_support_bytes).unwrap();
    }

    #[test]
    fn pair_resource_unavailability_requires_an_exact_exceeded_limit() {
        let mut child = valid_child(3, b"AAAC", 43);
        child.pair_evidence =
            ReportPairEvidence::unavailable_resource_limit(child.source_root, 8_193, 8_192)
                .unwrap();
        validate_child(&child, 1 << 20).unwrap();

        let mut invalid_payload = child.pair_evidence.payload.clone();
        invalid_payload.supplied_fragments = invalid_payload.configured_fragment_limit;
        child.pair_evidence = ReportPairEvidence::from_payload(invalid_payload).unwrap();
        assert_eq!(
            validate_child(&child, 1 << 20).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn unsupported_internal_adjacency_is_rejected_even_with_copied_evidence() {
        let mut child = valid_child(3, b"AAAC", 18);
        child.segments[0].sequence.copy_from_slice(b"AAGC");
        for (offset, window) in b"AAGC".windows(3).enumerate() {
            let literal = encode_dna_u128(window).unwrap();
            let reverse = reverse_complement_u128(literal, 3);
            child.segments[0].steps[offset].canonical_kmer =
                packed_u128_bytes(literal.min(reverse));
            child.segments[0].steps[offset].orientation = match literal.cmp(&reverse) {
                std::cmp::Ordering::Equal => ReportEdgeOrientation::SelfReverseComplement,
                std::cmp::Ordering::Less => ReportEdgeOrientation::Canonical,
                std::cmp::Ordering::Greater => ReportEdgeOrientation::ReverseComplement,
            };
        }
        child.segments[0].id = report_segment_id(3, child.child_root, &child.segments[0]);
        child.segments[0].provenance_sha256 =
            report_segment_provenance(3, child.child_root, &child.segments[0]);
        // The exact ledger remains the copied AAAC row. Independent sequence
        // replay must reject AAGC before trusting copied summary fields.
        assert_eq!(
            validate_child(&child, 1 << 20).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn exact_agreement_profile_excludes_singletons_and_all_unique_groups() {
        let singleton = Presentation {
            id: [1; 32],
            key: PresentationKey {
                topology: ReportTopology::Linear,
                sequence: b"AAAC".to_vec(),
            },
            members: vec![(3, [2; 32])],
        };
        let second = Presentation {
            id: [3; 32],
            key: PresentationKey {
                topology: ReportTopology::Linear,
                sequence: b"CCCG".to_vec(),
            },
            members: vec![(5, [4; 32])],
        };
        for presentations in [vec![singleton.clone()], vec![singleton, second]] {
            let prepared = PreparedPortfolio {
                source_root: [7; 32],
                input_mode: "single_end".to_owned(),
                support_unit: "supplied_fragment_instance".to_owned(),
                min_base_quality: 0,
                profile: PortfolioProfile::ExactAgreementConsensus,
                pair_evidence: PairEvidenceState::NotApplicableSingleEnd,
                quality_correction: DisabledCapabilityState::DisabledUnqualified,
                execution: execution(),
                children: Vec::new(),
                presentations,
                exact_agreement_root: [8; 32],
                parent_root: [9; 32],
                operational_root: [10; 32],
                total_segments: 0,
                total_links: 0,
                total_adjacencies: 0,
                total_decisions: 0,
                total_output_bases: 0,
            };
            let mut output = Vec::new();
            write_contigs_fasta(&mut output, &prepared).unwrap();
            assert!(output.is_empty());
        }
    }

    #[test]
    fn exact_agreement_requires_two_distinct_k_children() {
        let presentation = Presentation {
            id: [1; 32],
            key: PresentationKey {
                topology: ReportTopology::Linear,
                sequence: b"AAAC".to_vec(),
            },
            members: vec![(3, [2; 32]), (5, [3; 32])],
        };
        assert!(has_cross_k_agreement(&presentation));
    }

    #[test]
    fn stage_enforces_minus_exact_and_plus_byte_boundaries() {
        for (limit, succeeds) in [(2_u64, false), (3, true), (4, true)] {
            let root = tempfile::tempdir().unwrap();
            let mut stage = Stage::new(root.path(), limit).unwrap();
            let result = stage.bytes("x", b"abc");
            assert_eq!(result.is_ok(), succeeds, "limit={limit}");
            assert_eq!(root.path().join("x").exists(), succeeds);
            if succeeds {
                assert_eq!(stage.used, 3);
            } else {
                assert_eq!(result.unwrap_err().code(), ErrorCode::ResourceOutputBytes);
                assert_eq!(stage.used, 0);
            }
        }
    }

    #[test]
    fn transactional_failures_never_publish_partial_destination() {
        for fail_point in [
            BundleFailPoint::AfterStaging,
            BundleFailPoint::AfterFasta,
            BundleFailPoint::AfterManifest,
            BundleFailPoint::BeforeCommit,
        ] {
            let parent = tempfile::tempdir().unwrap();
            let work = tempfile::tempdir_in(parent.path()).unwrap();
            let reference = snapshot(work.path(), &valid_child(3, b"AAAC", 21));
            let destination = parent.path().join(format!("result-{fail_point:?}"));
            let lease = RunLease::acquire(&destination).unwrap();
            let result = write_multik_bundle_inner(
                lease,
                &data(vec![reference], PortfolioProfile::DiversityPreserving),
                Some(fail_point),
            );
            assert_eq!(result.unwrap_err().code(), ErrorCode::InternalUnexpected);
            assert!(!destination.exists());
        }
    }

    #[test]
    fn noncooperating_destination_is_preserved() {
        let parent = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir_in(parent.path()).unwrap();
        let reference = snapshot(work.path(), &valid_child(3, b"AAAC", 22));
        let destination = parent.path().join("result");
        let lease = RunLease::acquire(&destination).unwrap();
        let result = write_multik_bundle_inner(
            lease,
            &data(vec![reference], PortfolioProfile::DiversityPreserving),
            Some(BundleFailPoint::DestinationAppearsBeforeCommit),
        );
        assert_eq!(result.unwrap_err().code(), ErrorCode::DestinationExisting);
        assert_eq!(
            fs::read(destination.join("sentinel")).unwrap(),
            b"preserve\n"
        );
    }

    #[test]
    fn successful_bundle_is_deterministic_and_manifest_verifies() {
        let parent = tempfile::tempdir().unwrap();
        let mut roots = Vec::new();
        for ordinal in 0..2 {
            let work = tempfile::tempdir_in(parent.path()).unwrap();
            let reference = snapshot(work.path(), &valid_child(3, b"AAAC", 23));
            let destination = parent.path().join(format!("result-{ordinal}"));
            let lease = RunLease::acquire(&destination).unwrap();
            let result = write_unverified_multik_bundle_with_lease(
                lease,
                &data(vec![reference], PortfolioProfile::DiversityPreserving),
            )
            .unwrap();
            verify_bundle_manifest_with_limits(
                &destination,
                BundleManifestLimits {
                    max_manifest_bytes: 1 << 20,
                    max_regular_files: 13,
                    max_directories: 1,
                    max_path_depth: 2,
                    max_hashed_artifact_bytes: 1 << 30,
                },
            )
            .unwrap();
            roots.push((
                result.parent_root,
                result.operational_root,
                result.manifest_sha256,
                fs::read(destination.join("manifest.sha256")).unwrap(),
            ));
        }
        assert_eq!(roots[0], roots[1]);
    }

    #[test]
    fn malformed_snapshot_json_never_panics() {
        for length in 0..512_usize {
            let bytes = (0..length)
                .map(|offset| ((offset * 131 + length * 17) & 0xff) as u8)
                .collect::<Vec<_>>();
            let parsed = std::panic::catch_unwind(|| {
                serde_json::from_slice::<UnverifiedChildSnapshotPayload>(&bytes)
            });
            assert!(parsed.is_ok(), "parser panicked for length {length}");
            if let Ok(payload) = parsed.unwrap() {
                assert!(validate_child(&payload, 1 << 20).is_err());
            }
        }
    }
}
