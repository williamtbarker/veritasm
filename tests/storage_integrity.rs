use proptest::prelude::*;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use veritasm::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
use veritasm::count::{CountOptions, CountWriter};
use veritasm::dna::encode_kmer;
use veritasm::spool::create_spool;
use veritasm::ErrorCode;

fn scientific() -> ScientificConfig {
    ScientificConfig::resolve(
        3,
        Profile::RetainAll,
        SupportUnit::SuppliedFragmentInstance,
        None,
        20,
        true,
    )
    .unwrap()
}

fn bounded_limits() -> Limits {
    Limits {
        memory_budget_bytes: 32 << 20,
        partition_prefix_bits: 0,
        sort_buffer_keys: 1_024,
        ..Limits::default()
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn public_spool_verifier_rejects_every_sampled_single_bit_mutation(
        symbols in prop::collection::vec(0_u8..15, 3..128),
        offset in any::<usize>(),
        bit in 0_u8..8,
    ) {
        const IUPAC: &[u8; 15] = b"ACGTRYSWKMBDHVN";
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("reads.fasta");
        let mut fasta = b">property-read\n".to_vec();
        fasta.extend(symbols.into_iter().map(|index| IUPAC[usize::from(index)]));
        fasta.push(b'\n');
        fs::write(&input, fasta).unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input]),
            &scientific(),
            &bounded_limits(),
            directory.path(),
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let length = fs::metadata(spool.path()).unwrap().len() as usize;
        let selected = offset % length;
        let mut file = OpenOptions::new().write(true).open(spool.path()).unwrap();
        file.seek(SeekFrom::Start(selected as u64)).unwrap();
        let original = fs::read(spool.path()).unwrap()[selected];
        file.write_all(&[original ^ (1_u8 << bit)]).unwrap();
        file.flush().unwrap();

        prop_assert_eq!(spool.verify().unwrap_err().code(), ErrorCode::IntegritySpool);
    }

    #[test]
    fn public_counter_rejects_every_sampled_single_bit_run_mutation(
        offset in any::<usize>(),
        bit in 0_u8..8,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let options = CountOptions::from_config(
            directory.path(),
            &scientific(),
            &bounded_limits(),
            0,
        )
        .unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let aaa = encode_kmer(b"AAA").unwrap();
        writer.observe_fragment(0, &[aaa; 1_024]).unwrap();
        let run = fs::read_dir(directory.path().join("count"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "bin"))
            .unwrap();
        let length = fs::metadata(&run).unwrap().len() as usize;
        let selected = offset % length;
        let original = fs::read(&run).unwrap()[selected];
        let mut file = OpenOptions::new().write(true).open(&run).unwrap();
        file.seek(SeekFrom::Start(selected as u64)).unwrap();
        file.write_all(&[original ^ (1_u8 << bit)]).unwrap();
        file.flush().unwrap();

        prop_assert_eq!(writer.finish(1).unwrap_err().code(), ErrorCode::IntegrityCountRun);
    }
}
