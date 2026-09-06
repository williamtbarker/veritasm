#![no_main]

use libfuzzer_sys::fuzz_target;
use std::fs;
use veritasm::bundle::verify_bundle_manifest;
use veritasm::config::{AssembleConfig, InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
use veritasm::{assemble, ErrorCode};

fn allowed_first_run_error(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::ResourceMemory
            | ErrorCode::ResourceSpoolBytes
            | ErrorCode::ResourceTemporaryBytes
            | ErrorCode::ResourceOpenFiles
            | ErrorCode::ResourceRunCount
            | ErrorCode::ResourceManifestBytes
            | ErrorCode::ResourceRetainedKeys
            | ErrorCode::ResourceMappingCandidates
            | ErrorCode::ResourceOutputBytes
            | ErrorCode::ResourceIntegerOverflow
            | ErrorCode::DestinationNoReplaceUnsupported
            | ErrorCode::CommitWrite
            | ErrorCode::CommitFlush
            | ErrorCode::CommitSync
            | ErrorCode::CommitReopen
            | ErrorCode::CommitValidate
            | ErrorCode::CommitManifest
            | ErrorCode::CommitRenameNoReplace
    )
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 4_096 {
        return;
    }
    let Ok(directory) = tempfile::tempdir() else {
        return;
    };
    let input = directory.path().join("reads.fasta");
    let mut fasta = b">render-transaction-seed\n".to_vec();
    if data.is_empty() {
        fasta.extend_from_slice(b"ACG");
    } else {
        const BASES: &[u8; 5] = b"ACGTN";
        fasta.extend(data.iter().map(|byte| BASES[usize::from(*byte % 5)]));
    }
    fasta.push(b'\n');
    if fs::write(&input, fasta).is_err() {
        return;
    }
    let Ok(scientific) = ScientificConfig::resolve(
        3,
        Profile::RetainAll,
        SupportUnit::SuppliedFragmentInstance,
        None,
        0,
        data.first().is_some_and(|byte| byte & 1 != 0),
    ) else {
        return;
    };
    let output = directory.path().join("result");
    let config = AssembleConfig {
        input: InputSpec::Single(vec![input]),
        output_dir: output.clone(),
        scientific,
        limits: Limits {
            max_header_bytes: 4_096,
            max_read_bases: 16_384,
            max_record_bytes: 32_768,
            max_decoded_input_bytes: 1 << 20,
            batch_fragments: 1,
            memory_budget_bytes: 32 << 20,
            max_spool_bytes: 1 << 20,
            max_temp_bytes: 4 << 20,
            partition_prefix_bits: 0,
            sort_buffer_keys: 1_024,
            max_manifest_bytes: 1 << 20,
            max_retained_kmers: 16_384,
            max_mapping_candidates: 4_096,
            max_staged_output_bytes: 1 << 20,
            html_max_unitig_rows: 256,
            ..Limits::default()
        },
        threads: 1,
    };
    match assemble(&config) {
        Ok(_) => {
            assert!(verify_bundle_manifest(&output).is_ok());
            let second = assemble(&config).expect_err("a committed destination cannot be replaced");
            assert_eq!(second.code(), ErrorCode::DestinationExisting);
            assert!(verify_bundle_manifest(&output).is_ok());
        }
        Err(error) => {
            assert!(
                !output.exists(),
                "an assembly error left a normal committed destination"
            );
            assert!(
                allowed_first_run_error(error.code()),
                "valid generated bundle input returned unexpected error code {}",
                error.code()
            );
        }
    }
});
