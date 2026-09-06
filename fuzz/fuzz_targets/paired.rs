#![no_main]

use libfuzzer_sys::fuzz_target;
use std::fs;
use veritasm::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
use veritasm::spool::create_spool;

const DELIMITER: &[u8] = b"\n===MATE===\n";

fn split_mates(data: &[u8]) -> (&[u8], &[u8]) {
    if let Some(position) = data
        .windows(DELIMITER.len())
        .position(|window| window == DELIMITER)
    {
        (&data[..position], &data[position + DELIMITER.len()..])
    } else {
        data.split_at(data.len() / 2)
    }
}

fn bounded_limits() -> Limits {
    Limits {
        max_header_bytes: 4_096,
        max_read_bases: 16_384,
        max_record_bytes: 32_768,
        max_decoded_input_bytes: 1 << 20,
        memory_budget_bytes: 32 << 20,
        max_spool_bytes: 1 << 20,
        max_temp_bytes: 4 << 20,
        sort_buffer_keys: 1_024,
        ..Limits::default()
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }
    let (first, second) = split_mates(data);
    let Ok(directory) = tempfile::tempdir() else {
        return;
    };
    let read1 = directory.path().join("r1.fastx");
    let read2 = directory.path().join("r2.fastx");
    if fs::write(&read1, first).is_err() || fs::write(&read2, second).is_err() {
        return;
    }
    let Ok(scientific) = ScientificConfig::resolve(
        3,
        Profile::RetainAll,
        SupportUnit::SuppliedFragmentInstance,
        None,
        20,
        true,
    ) else {
        return;
    };
    let input = InputSpec::Paired {
        read1: vec![read1],
        read2: vec![read2],
    };
    if let Ok(spool) = create_spool(&input, &scientific, &bounded_limits(), directory.path()) {
        spool
            .verify()
            .expect("a successfully produced paired spool must verify");
        let iter = spool
            .iter()
            .expect("a verified paired spool must open for a complete second pass");
        let mut observed_fragments = 0_u64;
        let mut observed_reads = 0_u64;
        for fragment in iter {
            let fragment =
                fragment.expect("a verified paired spool must fully iterate without error");
            assert_eq!(fragment.reads.len(), 2);
            observed_fragments = observed_fragments
                .checked_add(1)
                .expect("bounded fuzz fragment count");
            observed_reads = observed_reads
                .checked_add(u64::try_from(fragment.reads.len()).expect("bounded read count"))
                .expect("bounded fuzz read count");
        }
        assert_eq!(observed_fragments, spool.fragment_count());
        assert_eq!(observed_reads, spool.read_count());
    }
});
