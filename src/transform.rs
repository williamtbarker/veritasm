//! Exact transformation-journal records for stable schema 1.0.

use crate::config::ScientificConfig;
use crate::count::{empty_decision_digest, CountResult};
use crate::error::{ErrorCode, Result, VeritasmError};
use crate::model::TransformRecord;

pub fn build_transform_records(
    counts: &CountResult,
    scientific: &ScientificConfig,
) -> Result<Vec<TransformRecord>> {
    if counts.retained_distinct != counts.retained.len() as u64 {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            "retained vector length disagrees with exact count summary",
        ));
    }
    let stage_zero = TransformRecord {
        stage_order: 0,
        algorithm_id: "exact_count_observation",
        parameters_json: "{}".to_owned(),
        input_distinct: counts.observed_distinct,
        output_distinct: counts.observed_distinct,
        input_support_mass: counts.observed_support_mass,
        output_support_mass: counts.observed_support_mass,
        removed_key_count: 0,
        removed_support_mass: 0,
        decision_set_sha256: empty_decision_digest(scientific.k, scientific.support_unit.tag(), 0),
        pre_state_sha256: counts.observed_state_sha256.clone(),
        post_state_sha256: counts.observed_state_sha256.clone(),
        status: "software_stage_complete_no_change",
    };
    let changed = counts.removed_distinct != 0;
    let stage_one = TransformRecord {
        stage_order: 1,
        algorithm_id: "absolute_support_retention",
        parameters_json: format!(
            "{{\"retention_min_support_decimal\":\"{}\",\"support_unit\":\"{}\"}}",
            scientific.min_support,
            scientific.support_unit.as_str()
        ),
        input_distinct: counts.observed_distinct,
        output_distinct: counts.retained_distinct,
        input_support_mass: counts.observed_support_mass,
        output_support_mass: counts.retained_support_mass,
        removed_key_count: counts.removed_distinct,
        removed_support_mass: counts.removed_support_mass,
        decision_set_sha256: counts.removed_decision_sha256.clone(),
        pre_state_sha256: counts.observed_state_sha256.clone(),
        post_state_sha256: counts.retained_state_sha256.clone(),
        status: if changed {
            "software_stage_complete_with_change"
        } else {
            "software_stage_complete_no_change"
        },
    };
    Ok(vec![stage_zero, stage_one])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Profile, SupportUnit};
    use crate::count::HistogramBin;

    #[test]
    fn emits_exact_two_stages() {
        let scientific = ScientificConfig::resolve(
            31,
            Profile::Thresholded,
            SupportUnit::SuppliedFragmentInstance,
            Some(2),
            20,
            true,
        )
        .unwrap();
        let counts = CountResult {
            observed_distinct: 2,
            observed_support_mass: 3,
            retained_distinct: 1,
            retained_support_mass: 2,
            removed_distinct: 1,
            removed_support_mass: 1,
            histogram: vec![HistogramBin {
                support: 1,
                distinct_canonical_keys: 1,
            }],
            retained: Vec::new(),
            observed_state_sha256: "a".repeat(64),
            retained_state_sha256: "b".repeat(64),
            removed_decision_sha256: "c".repeat(64),
            manifest_sha256: "d".repeat(64),
            temporary_bytes: 0,
            run_files_created: 0,
        };
        let error = build_transform_records(&counts, &scientific).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InternalInvariant);
    }
}
