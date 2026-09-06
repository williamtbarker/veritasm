#![no_main]

use libfuzzer_sys::fuzz_target;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use veritasm::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
use veritasm::spool::create_spool;

const TRAILER_BYTES: usize = 64;

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
    if data.len() > 16_384 {
        return;
    }
    let Ok(directory) = tempfile::tempdir() else {
        return;
    };
    let input = directory.path().join("input.fasta");
    if fs::write(&input, b">seed\nACGTRYSWKMBDHVNACGT\n").is_err() {
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
    let Ok(spool) = create_spool(
        &InputSpec::Single(vec![input]),
        &scientific,
        &bounded_limits(),
        directory.path(),
    ) else {
        return;
    };
    let Ok(mut bytes) = fs::read(spool.path()) else {
        return;
    };
    let pristine = bytes.clone();
    let trailer = bytes.len() - TRAILER_BYTES;

    match data.split_first() {
        Some((b'V', _)) => {}
        Some((b'R', raw)) => bytes = raw.to_vec(),
        Some((b'O', _)) => {
            bytes[trailer + 8..trailer + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        }
        _ => {
            for chunk in data.chunks(3).take(32) {
                let first = usize::from(chunk[0]);
                let second = usize::from(*chunk.get(1).unwrap_or(&0));
                let index = (first | (second << 8)) % trailer;
                bytes[index] ^= *chunk.get(2).unwrap_or(&1) | 1;
            }
        }
    }
    if bytes != pristine {
        #[cfg(unix)]
        if fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).is_err() {
            return;
        }
        if fs::write(spool.path(), &bytes).is_err() {
            return;
        }
        assert!(
            spool.verify().is_err(),
            "post-seal spool mutation was accepted without source reauthentication"
        );
        assert!(
            spool.iter().is_err(),
            "post-seal spool mutation opened for scientific replay"
        );
        return;
    }

    spool
        .verify()
        .expect("an unchanged produced spool must verify");
    let iter = spool
        .iter()
        .expect("a verified spool must open for a complete second pass");
    let mut observed_fragments = 0_u64;
    let mut observed_reads = 0_u64;
    for fragment in iter {
        let fragment = fragment.expect("a verified spool must fully iterate without error");
        observed_fragments = observed_fragments
            .checked_add(1)
            .expect("bounded fuzz fragment count");
        observed_reads = observed_reads
            .checked_add(u64::try_from(fragment.reads.len()).expect("bounded read count"))
            .expect("bounded fuzz read count");
    }
    assert_eq!(observed_fragments, spool.fragment_count());
    assert_eq!(observed_reads, spool.read_count());
});
