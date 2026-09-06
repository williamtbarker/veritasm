#![no_main]

use flate2::write::GzEncoder;
use flate2::Compression;
use libfuzzer_sys::fuzz_target;
use std::fs;
use std::io::Write;
use veritasm::config::Limits;
use veritasm::dna::reverse_complement;
use veritasm::fastx::FastxReader;
use veritasm::input::{prepare_source, SourceSpec};
use veritasm::model::MateRole;

fn gzip(payload: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload).ok()?;
    encoder.finish().ok()
}

fn transport(data: &[u8]) -> Option<Vec<u8>> {
    match data.split_first() {
        Some((b'V', payload)) => gzip(payload),
        Some((b'M', payload)) => {
            let middle = payload.len() / 2;
            let mut bytes = gzip(&payload[..middle])?;
            bytes.extend(gzip(&payload[middle..])?);
            Some(bytes)
        }
        Some((b'T', payload)) => {
            let mut bytes = gzip(payload)?;
            bytes.extend_from_slice(b"trailing-junk");
            Some(bytes)
        }
        Some((b'C', payload)) => {
            let mut bytes = gzip(payload)?;
            if let Some(last) = bytes.last_mut() {
                *last ^= 1;
            }
            Some(bytes)
        }
        Some((b'X', payload)) => {
            let mut bytes = gzip(payload)?;
            bytes.truncate(bytes.len().saturating_sub(8));
            Some(bytes)
        }
        _ => Some(data.to_vec()),
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
        ..Limits::default()
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }
    let Some(bytes) = transport(data) else {
        return;
    };
    let Ok(directory) = tempfile::tempdir() else {
        return;
    };
    let input = directory.path().join("transport.bin");
    if fs::write(&input, bytes).is_err() {
        return;
    }
    let limits = bounded_limits();
    let Ok(source) = SourceSpec::new(0, MateRole::S, input) else {
        return;
    };
    let mut decoded_total = 0;
    let Ok(prepared) = prepare_source(source, &limits, directory.path(), 0, &mut decoded_total)
    else {
        return;
    };
    if let Ok(mut reader) = FastxReader::open(&prepared, &limits, limits.memory_budget_bytes / 2) {
        loop {
            match reader.next_record() {
                Ok(Some(record)) => {
                    reverse_complement(&record.sequence)
                        .expect("a decoded parser-returned sequence must be valid IUPAC DNA");
                }
                Ok(None) | Err(_) => break,
            }
        }
    }
});
