//! Validated stable configuration and profile expansion.

use crate::error::{ErrorCode, Result, VeritasmError};
use serde::Serialize;
use std::path::PathBuf;

/// Fixed Rayon worker-stack reservation used by the stable executable.
pub const WORKER_STACK_BYTES: usize = 1 << 20;
const WORKER_STACK_BUDGET_DIVISOR: u64 = 4;
const MAX_WORKER_THREADS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    RetainAll,
    Thresholded,
    Custom,
}

impl Profile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RetainAll => "retain_all",
            Self::Thresholded => "thresholded",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportUnit {
    SuppliedFragmentInstance,
    AcceptedWindowOccurrence,
}

impl SupportUnit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SuppliedFragmentInstance => "supplied_fragment_instance",
            Self::AcceptedWindowOccurrence => "accepted_window_occurrence",
        }
    }

    pub const fn tag(self) -> u8 {
        match self {
            Self::SuppliedFragmentInstance => 0,
            Self::AcceptedWindowOccurrence => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputSpec {
    Single(Vec<PathBuf>),
    Paired {
        read1: Vec<PathBuf>,
        read2: Vec<PathBuf>,
    },
}

impl InputSpec {
    pub fn lane_count(&self) -> usize {
        match self {
            Self::Single(lanes) => lanes.len(),
            Self::Paired { read1, .. } => read1.len(),
        }
    }

    pub const fn mode_tag(&self) -> u8 {
        match self {
            Self::Single(_) => 0,
            Self::Paired { .. } => 1,
        }
    }

    pub const fn mode_name(&self) -> &'static str {
        match self {
            Self::Single(_) => "single_end",
            Self::Paired { .. } => "paired_end",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScientificConfig {
    pub k: u8,
    pub profile: Profile,
    pub support_unit: SupportUnit,
    pub min_support: u64,
    pub min_base_quality: u8,
    pub remap: bool,
}

impl ScientificConfig {
    pub fn resolve(
        k: u8,
        profile: Profile,
        support_unit: SupportUnit,
        explicit_min_support: Option<u64>,
        min_base_quality: u8,
        remap: bool,
    ) -> Result<Self> {
        Self::validate_k_and_quality(k, min_base_quality)?;
        let min_support = match (profile, explicit_min_support) {
            (Profile::RetainAll, None | Some(1)) => 1,
            (Profile::RetainAll, Some(value)) => value,
            (Profile::Thresholded, None) => 2,
            (Profile::Thresholded, Some(value)) => value,
            (Profile::Custom, None) => {
                return Err(VeritasmError::new(
                    ErrorCode::ConfigurationProfileConflict,
                    "custom requires an explicit --min-support",
                ));
            }
            (Profile::Custom, Some(value)) => value,
        };
        let config = Self {
            k,
            profile,
            support_unit,
            min_support,
            min_base_quality,
            remap,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate a fully materialized scientific configuration.
    ///
    /// Public library callers can construct this type directly, so profile
    /// expansion at the CLI boundary is not sufficient to protect assembly
    /// and bundle invariants.
    pub fn validate(&self) -> Result<()> {
        Self::validate_k_and_quality(self.k, self.min_base_quality)?;
        match self.profile {
            Profile::RetainAll if self.min_support != 1 => Err(VeritasmError::new(
                ErrorCode::ConfigurationProfileConflict,
                format!(
                    "retain-all fixes min-support=1; received {}",
                    self.min_support
                ),
            )),
            Profile::Thresholded if self.min_support < 2 => Err(VeritasmError::new(
                ErrorCode::ConfigurationProfileConflict,
                format!(
                    "thresholded requires min-support>=2; received {}",
                    self.min_support
                ),
            )),
            Profile::Custom if self.min_support == 0 => Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidSupport,
                "min-support must be at least 1",
            )),
            _ => Ok(()),
        }
    }

    fn validate_k_and_quality(k: u8, min_base_quality: u8) -> Result<()> {
        if !(3..=63).contains(&k) {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidK,
                format!("k must be in 3..=63; received {k}"),
            ));
        }
        if min_base_quality > 93 {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                format!("min-base-quality must be in 0..=93; received {min_base_quality}"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub max_header_bytes: u64,
    pub max_read_bases: u64,
    pub max_record_bytes: u64,
    pub max_raw_transport_bytes: u64,
    pub max_decoded_input_bytes: u64,
    pub max_gzip_members: u64,
    pub batch_fragments: u64,
    pub memory_budget_bytes: u64,
    pub max_spool_bytes: u64,
    pub max_temp_bytes: u64,
    pub partition_prefix_bits: u8,
    pub sort_buffer_keys: u64,
    pub merge_fan_in: u8,
    pub max_count_open_files: u8,
    pub max_runs: u64,
    pub max_manifest_bytes: u64,
    pub max_retained_kmers: u64,
    pub max_mapping_candidates: u64,
    pub max_staged_output_bytes: u64,
    pub html_max_unitig_rows: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_header_bytes: 1 << 20,
            max_read_bases: 10 << 20,
            max_record_bytes: 24 << 20,
            max_raw_transport_bytes: 1 << 40,
            max_decoded_input_bytes: 1 << 40,
            max_gzip_members: 1_000_000,
            batch_fragments: 4_096,
            memory_budget_bytes: 512 << 20,
            max_spool_bytes: 1 << 40,
            max_temp_bytes: 2 << 40,
            partition_prefix_bits: 6,
            sort_buffer_keys: 1_048_576,
            merge_fan_in: 16,
            max_count_open_files: 17,
            max_runs: 1_000_000,
            max_manifest_bytes: 1 << 30,
            max_retained_kmers: 10_000_000,
            max_mapping_candidates: 10_000,
            max_staged_output_bytes: 100 << 30,
            html_max_unitig_rows: 1_000,
        }
    }
}

impl Limits {
    /// Maximum worker count admitted by both the stable hard ceiling and the
    /// dedicated quarter-budget stack reservation.
    pub fn maximum_worker_threads(&self) -> usize {
        let stack_bytes = u64::try_from(WORKER_STACK_BYTES).unwrap_or(u64::MAX);
        let by_budget = self
            .memory_budget_bytes
            .checked_div(WORKER_STACK_BUDGET_DIVISOR)
            .and_then(|share| share.checked_div(stack_bytes))
            .unwrap_or(0);
        usize::try_from(by_budget.min(MAX_WORKER_THREADS as u64))
            .unwrap_or(MAX_WORKER_THREADS)
            .max(1)
    }

    pub fn validate(&self, k: u8) -> Result<()> {
        let bad = self.max_header_bytes == 0
            || self.max_header_bytes > 64 << 20
            || self.max_read_bases == 0
            || self.max_read_bases > 1 << 30
            || self.max_record_bytes < 2
            || self.max_record_bytes > 2 << 30
            || self.max_raw_transport_bytes < 1 << 20
            || self.max_decoded_input_bytes < 1 << 20
            || self.max_gzip_members == 0
            || self.batch_fragments == 0
            || self.batch_fragments > 1_048_576
            || self.memory_budget_bytes < 32 << 20
            || self.memory_budget_bytes > 64 << 30
            || self.max_spool_bytes < 1 << 20
            || self.max_temp_bytes < 1 << 20
            || self.partition_prefix_bits > 8
            || u16::from(self.partition_prefix_bits) > 2 * u16::from(k)
            || !(1_024..=268_435_456).contains(&self.sort_buffer_keys)
            || self.merge_fan_in != 16
            || self.max_count_open_files != 17
            || !(1..=10_000_000).contains(&self.max_runs)
            || !(1 << 20..=16 << 30).contains(&self.max_manifest_bytes)
            || !(1..=500_000_000).contains(&self.max_retained_kmers)
            || !(1..=10_000_000).contains(&self.max_mapping_candidates)
            || self.max_staged_output_bytes < 1 << 20
            || self.html_max_unitig_rows > 100_000;
        if bad {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                "one or more resource limits are outside the stable 0.1 domain",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AssembleConfig {
    pub input: InputSpec,
    pub output_dir: PathBuf,
    pub scientific: ScientificConfig,
    pub limits: Limits,
    pub threads: usize,
}

impl AssembleConfig {
    pub fn validate(&self) -> Result<()> {
        self.scientific.validate()?;
        self.limits.validate(self.scientific.k)?;
        let maximum_worker_threads = self.limits.maximum_worker_threads();
        if self.threads == 0
            || self.threads > MAX_WORKER_THREADS
            || self.threads > maximum_worker_threads
        {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                format!(
                    "threads must be in 1..={maximum_worker_threads} for the configured memory budget (stable hard maximum {MAX_WORKER_THREADS})"
                ),
            ));
        }
        match &self.input {
            InputSpec::Single(lanes) if lanes.is_empty() => Err(VeritasmError::new(
                ErrorCode::ConfigurationUnsupportedCombination,
                "at least one single-end lane is required",
            )),
            InputSpec::Paired { read1, read2 } if read1.len() != read2.len() => {
                Err(VeritasmError::new(
                    ErrorCode::PairLaneCount,
                    "read1 and read2 lane counts differ",
                ))
            }
            InputSpec::Paired { read1, .. } if read1.is_empty() => Err(VeritasmError::new(
                ErrorCode::PairLaneCount,
                "at least one paired lane is required",
            )),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_expand_without_hidden_overrides() {
        assert_eq!(
            ScientificConfig::resolve(
                31,
                Profile::Thresholded,
                SupportUnit::SuppliedFragmentInstance,
                None,
                20,
                true,
            )
            .unwrap()
            .min_support,
            2
        );
        assert!(ScientificConfig::resolve(
            31,
            Profile::RetainAll,
            SupportUnit::SuppliedFragmentInstance,
            Some(2),
            20,
            true,
        )
        .is_err());
    }

    #[test]
    fn manually_constructed_scientific_configs_are_validated() {
        let valid = ScientificConfig {
            k: 31,
            profile: Profile::Custom,
            support_unit: SupportUnit::SuppliedFragmentInstance,
            min_support: 1,
            min_base_quality: 20,
            remap: true,
        };
        valid.validate().unwrap();

        let mut invalid_k = valid.clone();
        invalid_k.k = 2;
        assert_eq!(
            invalid_k.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidK
        );

        let mut invalid_quality = valid.clone();
        invalid_quality.min_base_quality = 94;
        assert_eq!(
            invalid_quality.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );

        let mut invalid_support = valid.clone();
        invalid_support.min_support = 0;
        assert_eq!(
            invalid_support.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidSupport
        );

        let mut profile_conflict = valid;
        profile_conflict.profile = Profile::RetainAll;
        profile_conflict.min_support = 2;
        assert_eq!(
            profile_conflict.validate().unwrap_err().code(),
            ErrorCode::ConfigurationProfileConflict
        );
    }

    #[test]
    fn assemble_config_rejects_a_manually_invalid_scientific_config_first() {
        let config = AssembleConfig {
            input: InputSpec::Single(vec![PathBuf::from("reads.fastq")]),
            output_dir: PathBuf::from("result"),
            scientific: ScientificConfig {
                k: 2,
                profile: Profile::RetainAll,
                support_unit: SupportUnit::SuppliedFragmentInstance,
                min_support: 1,
                min_base_quality: 20,
                remap: true,
            },
            limits: Limits::default(),
            threads: 1,
        };
        assert_eq!(
            config.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidK
        );
    }

    #[test]
    fn worker_stacks_have_a_dedicated_budget_share() {
        let limits = Limits {
            memory_budget_bytes: 32 << 20,
            ..Limits::default()
        };
        assert_eq!(limits.maximum_worker_threads(), 8);

        let base = AssembleConfig {
            input: InputSpec::Single(vec![PathBuf::from("reads.fastq")]),
            output_dir: PathBuf::from("result"),
            scientific: ScientificConfig {
                k: 31,
                profile: Profile::RetainAll,
                support_unit: SupportUnit::SuppliedFragmentInstance,
                min_support: 1,
                min_base_quality: 20,
                remap: true,
            },
            limits,
            threads: 8,
        };
        base.validate().unwrap();

        let mut excessive = base;
        excessive.threads = 9;
        assert_eq!(
            excessive.validate().unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
    }

    #[test]
    fn transport_limits_have_explicit_valid_domains() {
        let mut limits = Limits::default();
        limits.validate(31).unwrap();

        limits.max_raw_transport_bytes = (1 << 20) - 1;
        assert_eq!(
            limits.validate(31).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        limits.max_raw_transport_bytes = 1 << 20;
        limits.max_gzip_members = 0;
        assert_eq!(
            limits.validate(31).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        limits.max_gzip_members = 1;
        limits.validate(31).unwrap();
    }
}
