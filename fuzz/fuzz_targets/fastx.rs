#![no_main]

use libfuzzer_sys::fuzz_target;
use std::fs;
use veritasm::config::Limits;
use veritasm::dna::{reverse_complement, scan_canonical_kmers};
use veritasm::fastx::FastxReader;
use veritasm::input::{prepare_source, SourceSpec};
use veritasm::model::MateRole;

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

fn materialize(data: &[u8]) -> Vec<u8> {
    match data.first() {
        Some(b'H') => {
            let mut bytes = vec![b'@'];
            bytes.extend(std::iter::repeat_n(b'x', 65_536));
            bytes.extend_from_slice(b"\nACG\n+\nIII\n");
            bytes
        }
        Some(b'S') => {
            let mut bytes = b">x\n".to_vec();
            bytes.extend(std::iter::repeat_n(b'A', 65_536));
            bytes.push(b'\n');
            bytes
        }
        Some(b'Q') => {
            let mut bytes = b"@x\nACGTACGT\n+\n".to_vec();
            bytes.extend(std::iter::repeat_n(b'I', 65_536));
            bytes.push(b'\n');
            bytes
        }
        _ => data.to_vec(),
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }
    let Ok(directory) = tempfile::tempdir() else {
        return;
    };
    let input = directory.path().join("input.fastx");
    if fs::write(&input, materialize(data)).is_err() {
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
    let Ok(mut reader) = FastxReader::open(&prepared, &limits, limits.memory_budget_bytes / 2)
    else {
        return;
    };
    let k = 3 + data.first().copied().unwrap_or(0) % 61;
    let minimum_quality = data.get(1).copied().unwrap_or(20) % 94;
    loop {
        match reader.next_record() {
            Ok(Some(record)) => {
                scan_canonical_kmers(
                    &record.sequence,
                    record.quality.as_deref(),
                    k,
                    minimum_quality,
                )
                .expect("a parser-returned record must satisfy the DNA scanner contract");
                let reverse = reverse_complement(&record.sequence)
                    .expect("a parser-returned sequence must be valid IUPAC DNA");
                assert_eq!(
                    reverse_complement(&reverse)
                        .expect("the reverse complement must remain valid IUPAC DNA"),
                    record.sequence,
                    "IUPAC reverse complement must be an involution"
                );
            }
            Ok(None) | Err(_) => break,
        }
    }
});
