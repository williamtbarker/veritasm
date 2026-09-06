//! Shared typed records at stable module boundaries.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FastxFormat {
    Fasta,
    Fastq,
}

impl FastxFormat {
    pub const fn tag(self) -> u8 {
        match self {
            Self::Fasta => 0,
            Self::Fastq => 1,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fasta => "fasta",
            Self::Fastq => "fastq",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum MateRole {
    S,
    R1,
    R2,
}

impl MateRole {
    pub const fn tag(self) -> u8 {
        match self {
            Self::S => 0,
            Self::R1 => 1,
            Self::R2 => 2,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S => "S",
            Self::R1 => "R1",
            Self::R2 => "R2",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRecord {
    pub role: MateRole,
    pub normalized_id_digest: [u8; 32],
    pub sequence: Vec<u8>,
    pub quality: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub ordinal: u64,
    pub lane_ordinal: u32,
    pub reads: Vec<ReadRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceSummary {
    pub lane_ordinal: u32,
    pub role: MateRole,
    pub format: FastxFormat,
    pub raw_transport_sha256: String,
    pub logical_decoded_sha256: String,
    pub records: u64,
    pub bases: u64,
}

impl SourceSummary {
    pub fn label(&self) -> String {
        format!("lane-{:06}-{}", self.lane_ordinal, self.role.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WindowStats {
    pub possible: u64,
    pub accepted: u64,
    pub ambiguity_only: u64,
    pub quality_only: u64,
    pub ambiguity_and_quality: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct KmerCount {
    pub key: u128,
    pub support: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topology {
    Linear,
    ClosedGraphWalk,
}

impl Topology {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::ClosedGraphWalk => "closed_graph_walk",
        }
    }

    pub const fn tag(self) -> u8 {
        match self {
            Self::Linear => 0,
            Self::ClosedGraphWalk => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvailabilityU64 {
    Value(u64),
    NotAvailable(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unitig {
    pub id: String,
    pub sequence: Vec<u8>,
    pub topology: Topology,
    pub edge_steps: u64,
    pub canonical_kmers: u64,
    pub minimum_support: u64,
    pub lower_median_support: u64,
    pub maximum_support: u64,
    pub enumeration_complete_read_placements: AvailabilityU64,
    pub single_group_read_instances: AvailabilityU64,
    pub multi_group_read_instances_with_group: AvailabilityU64,
    pub placement_enumeration_status: &'static str,
    pub sequence_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GraphLink {
    pub from_segment: String,
    pub from_orientation: char,
    pub to_segment: String,
    pub to_orientation: char,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PairEndpoint {
    pub segment: String,
    pub end: char,
    pub strand: char,
    pub end_distance: u64,
    pub mate_role: MateRole,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PairLink {
    pub lane_ordinal: u32,
    pub a: PairEndpoint,
    pub b: PairEndpoint,
    pub supplied_fragment_instances: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLaneStateCount {
    pub lane_ordinal: u32,
    pub state: &'static str,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCount {
    pub role: Option<MateRole>,
    pub state: &'static str,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformRecord {
    pub stage_order: u8,
    pub algorithm_id: &'static str,
    pub parameters_json: String,
    pub input_distinct: u64,
    pub output_distinct: u64,
    pub input_support_mass: u64,
    pub output_support_mass: u64,
    pub removed_key_count: u64,
    pub removed_support_mass: u64,
    pub decision_set_sha256: String,
    pub pre_state_sha256: String,
    pub post_state_sha256: String,
    pub status: &'static str,
}
