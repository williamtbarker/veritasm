#![cfg(unix)]

use std::fs;
use std::process::Command;
use tempfile::TempDir;
use veritasm::config::{Limits, Profile, ScientificConfig, SupportUnit};
use veritasm::count::{CountOptions, CountWriter};
use veritasm::dna::encode_kmer;

const LOW_FD_CHILD: &str = "VERITASM_LOW_FD_COUNT_CHILD";

#[test]
fn fan_in_merge_completes_with_low_descriptor_limit() {
    if std::env::var_os(LOW_FD_CHILD).is_some() {
        run_low_fd_count_child();
        return;
    }

    // Run the exact counter itself in a child copy of this integration-test
    // executable. POSIX sh lowers RLIMIT_NOFILE before exec; positional
    // parameters avoid interpolating paths into shell source.
    let result = Command::new("sh")
        .arg("-c")
        .arg("ulimit -n 24 || exit 125; exec \"$@\"")
        .arg("veritasm-low-fd")
        .arg(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("fan_in_merge_completes_with_low_descriptor_limit")
        .arg("--nocapture")
        .env(LOW_FD_CHILD, "1")
        .output()
        .unwrap();

    assert_ne!(
        result.status.code(),
        Some(125),
        "sh could not lower RLIMIT_NOFILE"
    );
    assert!(
        result.status.success(),
        "low-descriptor count failed: status={:?}, stdout={}, stderr={}",
        result.status.code(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn run_low_fd_count_child() {
    let temporary = TempDir::new().unwrap();
    let scientific = ScientificConfig::resolve(
        3,
        Profile::RetainAll,
        SupportUnit::SuppliedFragmentInstance,
        Some(1),
        0,
        false,
    )
    .unwrap();
    let limits = Limits {
        partition_prefix_bits: 2,
        sort_buffer_keys: 1_024,
        ..Limits::default()
    };
    let options = CountOptions::from_config(temporary.path(), &scientific, &limits, 0).unwrap();
    let mut writer = CountWriter::new(options).unwrap();
    let aaa = encode_kmer(b"AAA").unwrap();
    let fragments = 20_000_u64;
    for ordinal in 0..fragments {
        // Independent fragment ordinals are essential here. A single long
        // homopolymer read is deduplicated to one key in fragment mode and
        // therefore does not exercise spill or fan-in.
        writer.observe_fragment(ordinal, &[aaa]).unwrap();
    }
    let result = writer.finish(fragments).unwrap();

    // Twenty raw runs require two pass-zero groups (16 + 4) and one pass-one
    // group (2), so this is direct instrumentation of fan-in and merge depth.
    assert_eq!(result.run_files_created, 23);
    assert_eq!(result.retained.len(), 1);
    assert_eq!(result.retained[0].support, fragments);

    let count_dir = temporary.path().join("count");
    let live_runs = fs::read_dir(&count_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("bin"))
        .collect::<Vec<_>>();
    assert_eq!(live_runs.len(), 1, "obsolete merge generations survived");
    assert_eq!(
        live_runs[0].file_name().and_then(|name| name.to_str()),
        Some("p000-m001-g000000000000.bin")
    );

    let manifest = fs::read_to_string(count_dir.join("runs.manifest")).unwrap();
    let ancestry = manifest
        .lines()
        .filter(|line| line.starts_with("ancestry\t"))
        .collect::<Vec<_>>();
    assert_eq!(ancestry.len(), 3);
    assert!(ancestry[0].starts_with("ancestry\tp000-m000-g000000000000.bin\t"));
    assert!(ancestry[0].contains("\t16\t"));
    assert!(ancestry[1].starts_with("ancestry\tp000-m000-g000000000001.bin\t"));
    assert!(ancestry[1].contains("\t4\t"));
    assert!(ancestry[2].starts_with("ancestry\tp000-m001-g000000000000.bin\t"));
    assert!(ancestry[2].contains("\t2\t"));
}
