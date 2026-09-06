#![no_main]

use libfuzzer_sys::fuzz_target;
use sha2::{Digest, Sha256};
use std::fs;
use veritasm::config::{Limits, Profile, ScientificConfig, SupportUnit};
use veritasm::count::{CountOptions, CountWriter};
use veritasm::dna::encode_kmer;

const RUN_TRAILER_BYTES: usize = 40;
const RUN_DOMAIN: &[u8] = b"veritasm:count-run:v1\0";

fn config() -> (ScientificConfig, Limits) {
    let scientific = ScientificConfig::resolve(
        3,
        Profile::RetainAll,
        SupportUnit::AcceptedWindowOccurrence,
        None,
        0,
        false,
    )
    .expect("frozen fuzz configuration");
    let limits = Limits {
        memory_budget_bytes: 32 << 20,
        partition_prefix_bits: 0,
        sort_buffer_keys: 1_024,
        max_temp_bytes: 4 << 20,
        ..Limits::default()
    };
    (scientific, limits)
}

fn run_path(directory: &std::path::Path) -> Option<std::path::PathBuf> {
    fs::read_dir(directory.join("count"))
        .ok()?
        .filter_map(|entry| entry.ok().map(|value| value.path()))
        .find(|path| path.extension().is_some_and(|extension| extension == "bin"))
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 16_384 {
        return;
    }
    let Ok(directory) = tempfile::tempdir() else {
        return;
    };
    let (scientific, limits) = config();
    let Ok(options) = CountOptions::from_config(directory.path(), &scientific, &limits, 0) else {
        return;
    };
    let Ok(mut writer) = CountWriter::new(options) else {
        return;
    };
    let aaa = encode_kmer(b"AAA").expect("fixed canonical k-mer");
    let aac = encode_kmer(b"AAC").expect("fixed canonical k-mer");

    if data.first() == Some(&b'O') {
        let mut failed = false;
        let mut successful = 0_u64;
        for (position, chunk) in data[1..].chunks(17).take(128).enumerate() {
            let ordinal = if chunk.first().is_some_and(|byte| byte & 1 == 0) {
                successful
            } else {
                position as u64
            };
            let key = if chunk.get(1).is_some_and(|byte| byte & 1 == 0) {
                aaa
            } else {
                u128::from_be_bytes({
                    let mut bytes = [0_u8; 16];
                    for (to, from) in bytes.iter_mut().zip(chunk.iter().skip(1)) {
                        *to = *from;
                    }
                    bytes
                })
            };
            let result = writer.observe_fragment(ordinal, &[key]);
            assert!(!failed || result.is_err());
            if result.is_ok() {
                successful += 1;
            } else {
                failed = true;
            }
        }
        let result = writer.finish(successful);
        assert!(!failed || result.is_err());
        return;
    }

    let events = (0..1_024)
        .map(|index| if index & 1 == 0 { aaa } else { aac })
        .collect::<Vec<_>>();
    if writer.observe_fragment(0, &events).is_err() {
        return;
    }
    let Some(path) = run_path(directory.path()) else {
        return;
    };
    let Ok(mut bytes) = fs::read(&path) else {
        return;
    };
    let pristine = bytes.clone();
    match data.split_first() {
        Some((b'V', _)) => {}
        Some((b'R', raw)) => bytes = raw.to_vec(),
        _ => {
            let trailer = bytes.len() - RUN_TRAILER_BYTES;
            for chunk in data.chunks(3).take(32) {
                let first = usize::from(chunk[0]);
                let second = usize::from(*chunk.get(1).unwrap_or(&0));
                let index = (first | (second << 8)) % trailer;
                bytes[index] ^= *chunk.get(2).unwrap_or(&1) | 1;
            }
            if data.first() == Some(&b'S') {
                let mut digest = Sha256::new();
                digest.update(RUN_DOMAIN);
                digest.update(&bytes[..trailer]);
                bytes[trailer + 8..].copy_from_slice(&digest.finalize());
            }
        }
    }
    let mutated = bytes != pristine;
    if fs::write(path, bytes).is_err() {
        return;
    }
    let result = writer.finish(1);
    if mutated {
        assert!(
            result.is_err(),
            "modified count-run bytes were accepted against their registered identity"
        );
    } else {
        let result = result.expect("an unchanged produced count run must finish");
        assert_eq!(result.observed_distinct, 2);
        assert_eq!(result.observed_support_mass, 1_024);
        assert_eq!(result.retained_distinct, 2);
        assert_eq!(result.retained_support_mass, 1_024);
    }
});
