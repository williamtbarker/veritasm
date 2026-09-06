#![no_main]

use libfuzzer_sys::fuzz_target;
use sha2::{Digest, Sha256};
use std::fs;
use veritasm::bundle::verify_bundle_manifest;

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 16_384 {
        return;
    }
    let Ok(directory) = tempfile::tempdir() else {
        return;
    };
    let root = directory.path().join("bundle");
    if fs::create_dir(&root).is_err() {
        return;
    }
    let (mode, payload) = data.split_first().unwrap_or((&0, &[]));
    if fs::write(root.join("artifact"), payload).is_err() {
        return;
    }
    let digest = lower_hex(&Sha256::digest(payload));
    let manifest = match *mode {
        b'V' => format!("{digest}  artifact\n").into_bytes(),
        b'D' => format!("{digest}  artifact\n{digest}  artifact\n").into_bytes(),
        b'U' => format!("{digest}  ../artifact\n").into_bytes(),
        b'N' => format!("{digest}  artifact").into_bytes(),
        b'X' => format!("{}  artifact\n", "x".repeat(64)).into_bytes(),
        _ => data.to_vec(),
    };
    if fs::write(root.join("manifest.sha256"), manifest).is_ok() {
        let result = verify_bundle_manifest(&root);
        match *mode {
            b'V' => assert!(result.is_ok(), "valid generated manifest was rejected"),
            b'D' | b'U' | b'N' | b'X' => {
                assert!(
                    result.is_err(),
                    "known-invalid generated manifest was accepted"
                );
            }
            _ => {}
        }
    }
});
