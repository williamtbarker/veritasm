//! Deterministic exact external counting of canonical k-mer observations.
//!
//! The stable counter uses full `u128` keys. It writes bounded sorted runs,
//! merges at fixed fan-in, registers authenticated parent/child ancestry before
//! reclaiming obsolete run generations, and applies the declared support
//! threshold only after complete exact counts are available. No Bloom
//! structure participates.

use crate::config::{Limits, ScientificConfig};
use crate::dna::{canonical_code, validate_k};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::model::KmerCount;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const RUN_MAGIC: &[u8; 8] = b"VTRUN001";
const RUN_END: &[u8; 8] = b"VTREND01";
const RUN_DOMAIN: &[u8] = b"veritasm:count-run:v1\0";
const RUN_HEADER_BYTES: u64 = 20;
const RUN_RECORD_BYTES: u64 = 24;
const RUN_TRAILER_BYTES: u64 = 40;
const COUNT_IO_BUFFER_BYTES: usize = 64 << 10;
// Deterministic allocation-admission allowances. They deliberately include
// slack for collection nodes, paths, allocator size classes, and the live
// readers described below; they are not measured-RSS claims.
const CONSERVATIVE_HISTOGRAM_TREE_BIN_BYTES: u64 = 512;
const COUNT_FIXED_ALLOCATION_BYTES: u64 = 256 << 10;
const SUMMARY_FIXED_ALLOCATION_BYTES: u64 = 64 << 10;
const ALLOCATION_MARGIN_DENOMINATOR: u64 = 8;
const CONCURRENT_COUNT_SHARE_DIVISOR: u64 = 2;
const MIN_MEMORY_BUDGET_BYTES: u64 = 32 << 20;
const MAX_MEMORY_BUDGET_BYTES: u64 = 64 << 30;
const MIN_TEMP_BYTES: u64 = 1 << 20;
const MIN_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_MANIFEST_BYTES: u64 = 16 << 30;
const MIN_SORT_BUFFER_KEYS: usize = 1_024;
const MAX_SORT_BUFFER_KEYS: usize = 268_435_456;
const MAX_RUNS: u64 = 10_000_000;
const MAX_RETAINED_KMERS: u64 = 500_000_000;

#[derive(Debug, Clone)]
pub struct CountOptions {
    pub work_dir: PathBuf,
    pub k: u8,
    pub support_unit_tag: u8,
    pub min_support: u64,
    pub partition_prefix_bits: u8,
    pub sort_buffer_keys: usize,
    pub merge_fan_in: usize,
    pub max_runs: u64,
    pub max_manifest_bytes: u64,
    pub max_temp_bytes: u64,
    pub existing_temp_bytes: u64,
    pub memory_budget_bytes: u64,
    pub max_retained_kmers: u64,
}

impl CountOptions {
    pub fn from_config(
        work_dir: &Path,
        scientific: &ScientificConfig,
        limits: &Limits,
        existing_temp_bytes: u64,
    ) -> Result<Self> {
        validate_k(scientific.k)?;
        let sort_buffer_keys = usize::try_from(limits.sort_buffer_keys)
            .map_err(|_| overflow("sort-buffer-keys does not fit usize"))?;
        let options = Self {
            work_dir: work_dir.to_path_buf(),
            k: scientific.k,
            support_unit_tag: scientific.support_unit.tag(),
            min_support: scientific.min_support,
            partition_prefix_bits: limits.partition_prefix_bits,
            sort_buffer_keys,
            merge_fan_in: usize::from(limits.merge_fan_in),
            max_runs: limits.max_runs,
            max_manifest_bytes: limits.max_manifest_bytes,
            max_temp_bytes: limits.max_temp_bytes,
            existing_temp_bytes,
            memory_budget_bytes: limits.memory_budget_bytes,
            max_retained_kmers: limits.max_retained_kmers,
        };
        validate_options(&options)?;
        Ok(options)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistogramBin {
    pub support: u64,
    pub distinct_canonical_keys: u64,
}

#[derive(Debug, Clone)]
pub struct CountResult {
    pub observed_distinct: u64,
    pub observed_support_mass: u64,
    pub retained_distinct: u64,
    pub retained_support_mass: u64,
    pub removed_distinct: u64,
    pub removed_support_mass: u64,
    pub histogram: Vec<HistogramBin>,
    pub retained: Vec<KmerCount>,
    pub observed_state_sha256: String,
    pub retained_state_sha256: String,
    pub removed_decision_sha256: String,
    pub manifest_sha256: String,
    pub temporary_bytes: u64,
    pub run_files_created: u64,
}

#[derive(Debug, Clone)]
struct RunMeta {
    path: PathBuf,
    partition: u16,
    records: u64,
    bytes: u64,
    sha256: String,
}

/// Incremental exact counter. Fragment scans must arrive in strict ordinal order.
pub struct CountWriter {
    options: CountOptions,
    count_dir: PathBuf,
    manifest_path: PathBuf,
    manifest_bytes: u64,
    manifest_hasher: Sha256,
    buffer: Vec<(u16, u128)>,
    initial_runs: Vec<Vec<RunMeta>>,
    flush_ordinal: u64,
    run_files_created: u64,
    temporary_bytes: u64,
    next_fragment_ordinal: u64,
    observation_failed: bool,
}

impl CountWriter {
    pub fn new(options: CountOptions) -> Result<Self> {
        validate_options(&options)?;
        let count_dir = options.work_dir.join("count");
        fs::create_dir(&count_dir).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "create count work directory",
                cause,
            )
        })?;
        let manifest_path = count_dir.join("runs.manifest");
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&manifest_path)
            .map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityManifest,
                    "create count run manifest",
                    cause,
                )
            })?;
        let partitions = 1_usize << options.partition_prefix_bits;
        let capacity = options.sort_buffer_keys.min(1_048_576);
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(capacity).map_err(|cause| {
            error(
                ErrorCode::ResourceMemory,
                format!("cannot allocate exact-count sort buffer: {cause}"),
            )
        })?;
        let mut initial_runs = Vec::new();
        initial_runs
            .try_reserve_exact(partitions)
            .map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot allocate exact-count partition table: {cause}"),
                )
            })?;
        initial_runs.resize_with(partitions, Vec::new);
        Ok(Self {
            temporary_bytes: options.existing_temp_bytes,
            options,
            count_dir,
            manifest_path,
            manifest_bytes: 0,
            manifest_hasher: Sha256::new(),
            buffer,
            initial_runs,
            flush_ordinal: 0,
            run_files_created: 0,
            next_fragment_ordinal: 0,
            observation_failed: false,
        })
    }

    /// Add one already-scanned fragment's accepted canonical observations.
    pub fn observe_fragment(&mut self, ordinal: u64, keys: &[u128]) -> Result<()> {
        if self.observation_failed {
            return Err(error(
                ErrorCode::IntegrityOrdinalCoverage,
                "exact counter cannot continue after an observation failure",
            ));
        }
        let result = (|| {
            if ordinal != self.next_fragment_ordinal {
                return Err(error(
                    ErrorCode::IntegrityOrdinalCoverage,
                    format!(
                        "expected fragment ordinal {}, received {ordinal}",
                        self.next_fragment_ordinal
                    ),
                ));
            }
            self.next_fragment_ordinal = self
                .next_fragment_ordinal
                .checked_add(1)
                .ok_or_else(|| overflow("fragment ordinal overflow"))?;
            for &key in keys {
                validate_canonical_key(key, self.options.k)?;
                let partition =
                    partition_for(key, self.options.k, self.options.partition_prefix_bits);
                if self.buffer.len() == self.buffer.capacity()
                    && self.buffer.len() < self.options.sort_buffer_keys
                {
                    let additional = self
                        .options
                        .sort_buffer_keys
                        .checked_sub(self.buffer.capacity())
                        .ok_or_else(|| overflow("sort buffer capacity subtraction underflow"))?;
                    self.buffer.try_reserve_exact(additional).map_err(|cause| {
                        error(
                            ErrorCode::ResourceMemory,
                            format!("cannot grow exact-count sort buffer: {cause}"),
                        )
                    })?;
                }
                self.buffer.push((partition, key));
                if self.buffer.len() == self.options.sort_buffer_keys {
                    self.flush_buffer()?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.observation_failed = true;
        }
        result
    }

    pub fn finish(mut self, expected_fragments: u64) -> Result<CountResult> {
        if self.observation_failed {
            return Err(error(
                ErrorCode::IntegrityOrdinalCoverage,
                "exact counter cannot finish after an observation failure",
            ));
        }
        if self.next_fragment_ordinal != expected_fragments {
            return Err(error(
                ErrorCode::IntegrityOrdinalCoverage,
                format!(
                    "counted {} fragments but spool declares {expected_fragments}",
                    self.next_fragment_ordinal
                ),
            ));
        }
        self.flush_buffer()?;
        // Observation is complete, so the configured sort allocation is no
        // longer needed. Release it before merge summaries begin retaining
        // exact keys and histogram bins under the same phase budget.
        self.buffer = Vec::new();
        let mut final_runs = Vec::new();
        final_runs
            .try_reserve_exact(self.initial_runs.len())
            .map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot allocate final count-run inventory: {cause}"),
                )
            })?;
        let initial_runs = std::mem::take(&mut self.initial_runs);
        for (partition, runs) in initial_runs.into_iter().enumerate() {
            final_runs.push(self.merge_partition(partition as u16, runs)?);
        }
        OpenOptions::new()
            .append(true)
            .open(&self.manifest_path)
            .map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityManifest,
                    "open final count manifest",
                    cause,
                )
            })?
            .sync_all()
            .map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityManifest,
                    "sync count run manifest",
                    cause,
                )
            })?;
        let manifest_sha256 = sha256_file(&self.manifest_path)?;
        let expected_manifest_sha256 = lower_hex(&self.manifest_hasher.clone().finalize());
        if manifest_sha256 != expected_manifest_sha256 {
            return Err(error(
                ErrorCode::IntegrityManifest,
                "count run manifest changed after it was written",
            ));
        }
        self.summarize(&final_runs, manifest_sha256)
    }

    fn flush_buffer(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        // Move the buffer out of `self` so grouped records can stream directly
        // into run construction. Keeping a second `Vec<KmerCount>` here would
        // allow one configured sort buffer to consume its memory budget twice.
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.sort_unstable();
        let mut start = 0;
        while start < buffer.len() {
            let partition = buffer[start].0;
            let mut end = start + 1;
            while end < buffer.len() && buffer[end].0 == partition {
                end += 1;
            }
            let name = format!("p{partition:03}-r{:012}.bin", self.flush_ordinal);
            let records = buffer[start..end]
                .chunk_by(|left, right| left.1 == right.1)
                .map(|chunk| {
                    Ok(KmerCount {
                        key: chunk[0].1,
                        support: u64::try_from(chunk.len())
                            .map_err(|_| overflow("within-run support does not fit u64"))?,
                    })
                });
            let meta = self.write_run(&name, partition, records)?;
            let partition_runs = &mut self.initial_runs[usize::from(partition)];
            partition_runs.try_reserve(1).map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot grow exact-count run inventory: {cause}"),
                )
            })?;
            partition_runs.push(meta);
            start = end;
        }
        self.flush_ordinal = self
            .flush_ordinal
            .checked_add(1)
            .ok_or_else(|| overflow("count flush ordinal overflow"))?;
        buffer.clear();
        self.buffer = buffer;
        Ok(())
    }

    fn merge_partition(
        &mut self,
        partition: u16,
        mut runs: Vec<RunMeta>,
    ) -> Result<Option<RunMeta>> {
        if runs.is_empty() {
            return Ok(None);
        }
        let mut pass = 0_u32;
        while runs.len() > 1 {
            let next_length = runs.len().div_ceil(self.options.merge_fan_in);
            let mut next = Vec::new();
            next.try_reserve_exact(next_length).map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot allocate merge-pass run inventory: {cause}"),
                )
            })?;
            for (group, chunk) in runs.chunks(self.options.merge_fan_in).enumerate() {
                let name = format!("p{partition:03}-m{pass:03}-g{group:012}.bin");
                let records = merge_run_records(
                    chunk,
                    self.options.k,
                    self.options.partition_prefix_bits,
                    partition,
                )?;
                let replacement = self.write_run(&name, partition, records)?;
                // The replacement has been completely written, synced,
                // structurally verified, checksummed, and registered above.
                // Record its exact parents before reclaiming their payloads so
                // the append-only manifest remains a deterministic ancestry
                // ledger even though obsolete generations are no longer live.
                self.record_merge_ancestry(&name, &replacement, chunk)?;
                self.reclaim_predecessors(chunk)?;
                next.push(replacement);
            }
            runs = next;
            pass = pass
                .checked_add(1)
                .ok_or_else(|| overflow("merge pass overflow"))?;
        }
        Ok(runs.pop())
    }

    fn write_run<I>(&mut self, name: &str, partition: u16, records: I) -> Result<RunMeta>
    where
        I: IntoIterator<Item = Result<KmerCount>>,
    {
        self.before_run()?;
        let payload_path = self.count_dir.join(format!(".{name}.payload"));
        let final_path = self.count_dir.join(name);
        let mut payload = BufWriter::with_capacity(
            COUNT_IO_BUFFER_BYTES,
            File::create(&payload_path).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "create count run payload",
                    cause,
                )
            })?,
        );
        let mut count = 0_u64;
        let mut previous = None;
        for record in records {
            let record = record?;
            if record.support == 0 || previous.is_some_and(|key| key >= record.key) {
                return Err(error(
                    ErrorCode::IntegrityCountRun,
                    "count run records are not strictly ordered positive counts",
                ));
            }
            validate_canonical_key(record.key, self.options.k)?;
            if partition_for(
                record.key,
                self.options.k,
                self.options.partition_prefix_bits,
            ) != partition
            {
                return Err(error(
                    ErrorCode::IntegrityCountRun,
                    "count run record does not belong to its numeric partition",
                ));
            }
            let projected_payload = count
                .checked_add(1)
                .and_then(|value| value.checked_mul(RUN_RECORD_BYTES))
                .ok_or_else(|| overflow("count payload byte estimate overflow"))?;
            let projected_total = self
                .temporary_bytes
                .checked_add(projected_payload)
                .ok_or_else(|| overflow("temporary payload accounting overflow"))?;
            if projected_total > self.options.max_temp_bytes {
                return Err(error(
                    ErrorCode::ResourceTemporaryBytes,
                    "temporary byte limit exceeded by count payload",
                ));
            }
            payload
                .write_all(&record.key.to_be_bytes())
                .and_then(|()| payload.write_all(&record.support.to_le_bytes()))
                .map_err(|cause| {
                    io_error(
                        ErrorCode::ResourceTemporaryBytes,
                        "write count run payload",
                        cause,
                    )
                })?;
            count = count
                .checked_add(1)
                .ok_or_else(|| overflow("count run record count overflow"))?;
            previous = Some(record.key);
        }
        payload.flush().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "flush count run payload",
                cause,
            )
        })?;
        drop(payload);

        let payload_bytes = count
            .checked_mul(RUN_RECORD_BYTES)
            .ok_or_else(|| overflow("count run payload size overflow"))?;
        let run_bytes = RUN_HEADER_BYTES
            .checked_add(payload_bytes)
            .and_then(|value| value.checked_add(RUN_TRAILER_BYTES))
            .ok_or_else(|| overflow("count run size overflow"))?;
        self.reserve_temp(
            payload_bytes
                .checked_add(run_bytes)
                .ok_or_else(|| overflow("payload and run byte accounting overflow"))?,
        )?;

        let mut writer = BufWriter::with_capacity(
            COUNT_IO_BUFFER_BYTES,
            File::create(&final_path).map_err(|cause| {
                io_error(ErrorCode::ResourceTemporaryBytes, "create count run", cause)
            })?,
        );
        let mut digest = Sha256::new();
        digest.update(RUN_DOMAIN);
        let mut header = Vec::with_capacity(RUN_HEADER_BYTES as usize);
        header.extend_from_slice(RUN_MAGIC);
        header.push(self.options.k);
        header.push(self.options.partition_prefix_bits);
        header.extend_from_slice(&partition.to_le_bytes());
        header.extend_from_slice(&count.to_le_bytes());
        writer.write_all(&header).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "write count run header",
                cause,
            )
        })?;
        digest.update(&header);
        let mut reader = BufReader::with_capacity(
            COUNT_IO_BUFFER_BYTES,
            File::open(&payload_path).map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityCountRun,
                    "reopen count run payload",
                    cause,
                )
            })?,
        );
        let mut block = [0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut block).map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityCountRun,
                    "read count run payload",
                    cause,
                )
            })?;
            if read == 0 {
                break;
            }
            writer.write_all(&block[..read]).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "copy count run payload",
                    cause,
                )
            })?;
            digest.update(&block[..read]);
        }
        writer
            .write_all(RUN_END)
            .and_then(|()| writer.write_all(&digest.finalize()))
            .and_then(|()| writer.flush())
            .map_err(|cause| {
                io_error(ErrorCode::ResourceTemporaryBytes, "finish count run", cause)
            })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, "sync count run", cause))?;
        drop(reader);
        drop(writer);
        fs::remove_file(&payload_path).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "remove private count payload",
                cause,
            )
        })?;
        self.temporary_bytes = self
            .temporary_bytes
            .checked_sub(payload_bytes)
            .ok_or_else(|| overflow("temporary payload subtraction underflow"))?;
        let verified = verify_run_and_hash(
            &final_path,
            self.options.k,
            self.options.partition_prefix_bits,
            partition,
        )?;
        if verified.records != count {
            return Err(error(
                ErrorCode::InternalInvariant,
                "verified count-run cardinality changed after construction",
            ));
        }
        let meta = RunMeta {
            path: final_path,
            partition,
            records: count,
            bytes: run_bytes,
            sha256: verified.sha256,
        };
        self.record_manifest(name, &meta)?;
        Ok(meta)
    }

    fn before_run(&mut self) -> Result<()> {
        let next = self
            .run_files_created
            .checked_add(1)
            .ok_or_else(|| overflow("count run count overflow"))?;
        if next > self.options.max_runs {
            return Err(error(
                ErrorCode::ResourceRunCount,
                "count run limit exceeded",
            ));
        }
        let required = observation_allocation_bytes(&self.options, next)?;
        let allowed = self.options.memory_budget_bytes / CONCURRENT_COUNT_SHARE_DIVISOR;
        if required > allowed {
            return Err(error(
                ErrorCode::ResourceMemory,
                format!(
                    "exact-count observation allocation estimate {required} bytes exceeds its concurrent half-budget share {allowed} bytes"
                ),
            ));
        }
        self.run_files_created = next;
        Ok(())
    }

    fn reserve_temp(&mut self, bytes: u64) -> Result<()> {
        let next = self
            .temporary_bytes
            .checked_add(bytes)
            .ok_or_else(|| overflow("temporary byte accounting overflow"))?;
        if next > self.options.max_temp_bytes {
            return Err(error(
                ErrorCode::ResourceTemporaryBytes,
                "temporary byte limit exceeded by exact count runs",
            ));
        }
        self.temporary_bytes = next;
        Ok(())
    }

    fn record_manifest(&mut self, name: &str, meta: &RunMeta) -> Result<()> {
        let line = format!(
            "{name}\t{}\t{}\t{}\t{}\n",
            meta.partition, meta.records, meta.bytes, meta.sha256
        );
        self.append_manifest_line(&line)
    }

    fn record_merge_ancestry(
        &mut self,
        child_name: &str,
        child: &RunMeta,
        parents: &[RunMeta],
    ) -> Result<()> {
        let registered_child_name = child
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                error(
                    ErrorCode::InternalInvariant,
                    "replacement count run has no UTF-8 filename",
                )
            })?;
        if registered_child_name != child_name
            || parents.is_empty()
            || parents.len() > self.options.merge_fan_in
            || parents
                .iter()
                .any(|parent| parent.partition != child.partition)
        {
            return Err(error(
                ErrorCode::InternalInvariant,
                "replacement count-run ancestry is inconsistent",
            ));
        }
        let mut line = String::new();
        line.try_reserve(256_usize.saturating_add(parents.len().saturating_mul(192)))
            .map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot allocate count-run ancestry record: {cause}"),
                )
            })?;
        line.push_str("ancestry\t");
        line.push_str(child_name);
        line.push('\t');
        line.push_str(&child.sha256);
        line.push('\t');
        line.push_str(&child.partition.to_string());
        line.push('\t');
        line.push_str(&parents.len().to_string());
        for parent in parents {
            let parent_name = parent
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    error(
                        ErrorCode::InternalInvariant,
                        "count-run parent has no UTF-8 filename",
                    )
                })?;
            line.push('\t');
            line.push_str(parent_name);
            line.push('\t');
            line.push_str(&parent.sha256);
            line.push('\t');
            line.push_str(&parent.records.to_string());
            line.push('\t');
            line.push_str(&parent.bytes.to_string());
        }
        line.push('\n');
        self.append_manifest_line(&line)
    }

    fn append_manifest_line(&mut self, line: &str) -> Result<()> {
        let next = self
            .manifest_bytes
            .checked_add(line.len() as u64)
            .ok_or_else(|| overflow("count manifest byte count overflow"))?;
        if next > self.options.max_manifest_bytes {
            return Err(error(
                ErrorCode::ResourceManifestBytes,
                "count run manifest limit exceeded",
            ));
        }
        self.reserve_temp(line.len() as u64)?;
        OpenOptions::new()
            .append(true)
            .open(&self.manifest_path)
            .and_then(|mut manifest| manifest.write_all(line.as_bytes()))
            .map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityManifest,
                    "append count run manifest",
                    cause,
                )
            })?;
        self.manifest_hasher.update(line.as_bytes());
        self.manifest_bytes = next;
        Ok(())
    }

    fn reclaim_predecessors(&mut self, predecessors: &[RunMeta]) -> Result<()> {
        let predecessor_bytes = predecessors.iter().try_fold(0_u64, |sum, predecessor| {
            sum.checked_add(predecessor.bytes)
                .ok_or_else(|| overflow("predecessor count-run byte sum overflow"))
        })?;
        if predecessor_bytes > self.temporary_bytes {
            return Err(error(
                ErrorCode::InternalInvariant,
                "predecessor count-run bytes exceed live temporary-byte accounting",
            ));
        }
        for predecessor in predecessors {
            fs::remove_file(&predecessor.path).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "reclaim verified predecessor count run",
                    cause,
                )
            })?;
            self.temporary_bytes = self
                .temporary_bytes
                .checked_sub(predecessor.bytes)
                .ok_or_else(|| overflow("predecessor count-run byte subtraction underflow"))?;
        }
        Ok(())
    }

    fn summarize(
        &self,
        final_runs: &[Option<RunMeta>],
        manifest_sha256: String,
    ) -> Result<CountResult> {
        let mut histogram = BTreeMap::<u64, u64>::new();
        let mut observed_distinct = 0_u64;
        let mut observed_support_mass = 0_u64;
        let mut retained_distinct = 0_u64;
        let mut retained_support_mass = 0_u64;
        let mut previous = None;

        for record in iter_final_records(
            final_runs,
            self.options.k,
            self.options.partition_prefix_bits,
        )? {
            let record = record?;
            if previous.is_some_and(|key| key >= record.key) {
                return Err(error(
                    ErrorCode::IntegrityCountRun,
                    "final partition streams are not globally ordered and disjoint",
                ));
            }
            previous = Some(record.key);
            observed_distinct = observed_distinct
                .checked_add(1)
                .ok_or_else(|| overflow("observed distinct-key count overflow"))?;
            observed_support_mass = observed_support_mass
                .checked_add(record.support)
                .ok_or_else(|| overflow("observed support mass overflow"))?;
            let retained_next = if record.support >= self.options.min_support {
                if retained_distinct == self.options.max_retained_kmers {
                    return Err(error(
                        ErrorCode::ResourceRetainedKeys,
                        "retained canonical-key limit exceeded",
                    ));
                }
                retained_distinct
                    .checked_add(1)
                    .ok_or_else(|| overflow("retained distinct-key count overflow"))?
            } else {
                retained_distinct
            };
            let new_histogram_bin = !histogram.contains_key(&record.support);
            let histogram_bins_next = histogram
                .len()
                .checked_add(usize::from(new_histogram_bin))
                .ok_or_else(|| overflow("histogram bin count overflow"))?;
            let retained_capacity_next = usize::try_from(retained_next)
                .map_err(|_| overflow("retained key count does not fit usize"))?;

            // Admit the eventual retained vector, the tree and output forms of
            // the histogram, the final-run inventory, and one lazy run reader
            // before a new BTreeMap node can allocate.
            ensure_summary_budget(
                &self.options,
                retained_capacity_next,
                histogram_bins_next,
                histogram_bins_next,
                final_runs.len(),
            )?;

            let bin = histogram.entry(record.support).or_default();
            *bin = bin
                .checked_add(1)
                .ok_or_else(|| overflow("support histogram bin overflow"))?;
            if record.support >= self.options.min_support {
                retained_distinct = retained_next;
                retained_support_mass = retained_support_mass
                    .checked_add(record.support)
                    .ok_or_else(|| overflow("retained support mass overflow"))?;
            }
        }
        let removed_distinct = observed_distinct
            .checked_sub(retained_distinct)
            .ok_or_else(|| overflow("removed distinct-key subtraction underflow"))?;
        let removed_support_mass = observed_support_mass
            .checked_sub(retained_support_mass)
            .ok_or_else(|| overflow("removed support subtraction underflow"))?;

        let retained_length = usize::try_from(retained_distinct)
            .map_err(|_| overflow("retained key count does not fit usize"))?;
        let mut retained = Vec::new();
        retained
            .try_reserve_exact(retained_length)
            .map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot allocate retained canonical-key table: {cause}"),
                )
            })?;
        ensure_summary_budget(
            &self.options,
            retained.capacity(),
            histogram.len(),
            histogram.len(),
            final_runs.len(),
        )?;

        // The three digests have different, count-prefixed preimages, but all
        // consume the same globally sorted record stream. Update them in one
        // authenticated traversal after the first counting/histogram pass.
        // This preserves every digest byte while avoiding two complete extra
        // reads of the final partition generation.
        let (observed_state_sha256, retained_state_sha256, removed_decision_sha256) =
            state_digests_and_collect(
                StateDigestPlan {
                    k: self.options.k,
                    support_unit_tag: self.options.support_unit_tag,
                    observed_count: observed_distinct,
                    retained_count: retained_distinct,
                    removed_count: removed_distinct,
                    minimum_support: self.options.min_support,
                },
                iter_final_records(
                    final_runs,
                    self.options.k,
                    self.options.partition_prefix_bits,
                )?,
                &mut retained,
            )?;
        if retained.len() != retained_length {
            return Err(error(
                ErrorCode::IntegrityCountRun,
                "retained canonical-key cardinality changed between verified passes",
            ));
        }

        let mut histogram_rows = Vec::new();
        histogram_rows
            .try_reserve_exact(histogram.len())
            .map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("cannot allocate exact support histogram output: {cause}"),
                )
            })?;
        ensure_summary_budget(
            &self.options,
            retained.capacity(),
            histogram.len(),
            histogram_rows.capacity(),
            final_runs.len(),
        )?;
        for (support, distinct_canonical_keys) in histogram {
            if histogram_rows.len() == histogram_rows.capacity() {
                return Err(error(
                    ErrorCode::InternalInvariant,
                    "preallocated exact support histogram output is too small",
                ));
            }
            histogram_rows.push(HistogramBin {
                support,
                distinct_canonical_keys,
            });
        }

        Ok(CountResult {
            observed_distinct,
            observed_support_mass,
            retained_distinct,
            retained_support_mass,
            removed_distinct,
            removed_support_mass,
            histogram: histogram_rows,
            retained,
            observed_state_sha256,
            retained_state_sha256,
            removed_decision_sha256,
            manifest_sha256,
            temporary_bytes: self.temporary_bytes,
            run_files_created: self.run_files_created,
        })
    }
}

fn validate_options(options: &CountOptions) -> Result<()> {
    validate_k(options.k)?;
    if options.work_dir.as_os_str().is_empty() {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "count work directory must not be empty",
        ));
    }
    if !matches!(options.support_unit_tag, 0 | 1) {
        return Err(error(
            ErrorCode::ConfigurationUnsupportedCombination,
            "support-unit tag must be 0 or 1",
        ));
    }
    if options.min_support == 0 {
        return Err(error(
            ErrorCode::ConfigurationInvalidSupport,
            "minimum support must be at least 1",
        ));
    }
    if options.partition_prefix_bits > 8
        || u16::from(options.partition_prefix_bits) > 2 * u16::from(options.k)
    {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "partition-prefix-bits is invalid for k",
        ));
    }
    if !(MIN_SORT_BUFFER_KEYS..=MAX_SORT_BUFFER_KEYS).contains(&options.sort_buffer_keys) {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "sort-buffer-keys is outside the stable domain",
        ));
    }
    let buffer_bytes = u64::try_from(options.sort_buffer_keys)
        .map_err(|_| overflow("sort-buffer-keys does not fit u64"))?
        .checked_mul(
            u64::try_from(std::mem::size_of::<(u16, u128)>())
                .map_err(|_| overflow("sort-buffer element size does not fit u64"))?,
        )
        .ok_or_else(|| overflow("sort buffer byte estimate overflow"))?;
    if !(MIN_MEMORY_BUDGET_BYTES..=MAX_MEMORY_BUDGET_BYTES).contains(&options.memory_budget_bytes) {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "aggregate memory budget is outside the stable domain",
        ));
    }
    let concurrent_count_budget = options.memory_budget_bytes / CONCURRENT_COUNT_SHARE_DIVISOR;
    if buffer_bytes > concurrent_count_budget {
        return Err(error(
            ErrorCode::ResourceMemory,
            "sort buffer exceeds the half-budget reserved for counting while scan state is live",
        ));
    }
    let initial_count_allocation = observation_allocation_bytes(options, 0)?;
    if initial_count_allocation > concurrent_count_budget {
        return Err(error(
            ErrorCode::ResourceMemory,
            format!(
                "exact-count observation allocation estimate {initial_count_allocation} bytes exceeds its concurrent half-budget share {concurrent_count_budget} bytes"
            ),
        ));
    }
    if options.merge_fan_in != 16 {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "stable counter merge fan-in must be 16",
        ));
    }
    if !(1..=MAX_RUNS).contains(&options.max_runs) {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "max-runs is outside the stable domain",
        ));
    }
    if !(MIN_MANIFEST_BYTES..=MAX_MANIFEST_BYTES).contains(&options.max_manifest_bytes) {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "max-manifest-bytes is outside the stable domain",
        ));
    }
    if options.max_temp_bytes < MIN_TEMP_BYTES {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "max-temp-bytes is outside the stable domain",
        ));
    }
    if options.existing_temp_bytes > options.max_temp_bytes {
        return Err(error(
            ErrorCode::ResourceTemporaryBytes,
            "existing temporary bytes exceed max-temp-bytes",
        ));
    }
    if !(1..=MAX_RETAINED_KMERS).contains(&options.max_retained_kmers) {
        return Err(error(
            ErrorCode::ConfigurationInvalidLimit,
            "max-retained-kmers is outside the stable domain",
        ));
    }
    Ok(())
}

fn observation_allocation_bytes(options: &CountOptions, run_count: u64) -> Result<u64> {
    let buffer_bytes = u64::try_from(options.sort_buffer_keys)
        .map_err(|_| overflow("sort-buffer-keys does not fit u64"))?
        .checked_mul(
            u64::try_from(std::mem::size_of::<(u16, u128)>())
                .map_err(|_| overflow("sort-buffer element size does not fit u64"))?,
        )
        .ok_or_else(|| overflow("sort buffer byte estimate overflow"))?;
    let partition_count = 1_u64
        .checked_shl(u32::from(options.partition_prefix_bits))
        .ok_or_else(|| overflow("count partition count overflow"))?;
    let partition_table_bytes = partition_count
        .checked_mul(
            u64::try_from(std::mem::size_of::<Vec<RunMeta>>())
                .map_err(|_| overflow("partition table element size does not fit u64"))?,
        )
        .ok_or_else(|| overflow("count partition table byte estimate overflow"))?;
    let run_meta_bytes = run_count
        .checked_mul(conservative_run_meta_bytes(options)?)
        .ok_or_else(|| overflow("run metadata estimate overflow"))?;
    // A merge group can hold one buffered payload writer alongside the
    // configured fan-in of buffered authenticated readers. Raw-run creation
    // needs fewer buffers, so this is the deterministic phase maximum.
    let io_buffer_bytes = count_io_buffer_bytes(
        options
            .merge_fan_in
            .checked_add(1)
            .ok_or_else(|| overflow("count I/O buffer count overflow"))?,
    )?;
    let payload = buffer_bytes
        .checked_add(partition_table_bytes)
        .and_then(|value| value.checked_add(run_meta_bytes))
        .and_then(|value| value.checked_add(io_buffer_bytes))
        .ok_or_else(|| overflow("exact-count observation allocation estimate overflow"))?;
    allocation_with_margin(payload, COUNT_FIXED_ALLOCATION_BYTES)
}

fn summary_allocation_bytes(
    options: &CountOptions,
    retained_capacity: usize,
    histogram_bins: usize,
    histogram_output_capacity: usize,
    final_partition_count: usize,
) -> Result<u64> {
    let retained_bytes =
        allocation_bytes::<KmerCount>(retained_capacity, "retained canonical-key allocation")?;
    let histogram_tree_bytes = u64::try_from(histogram_bins)
        .map_err(|_| overflow("histogram bin count does not fit u64"))?
        .checked_mul(CONSERVATIVE_HISTOGRAM_TREE_BIN_BYTES)
        .ok_or_else(|| overflow("histogram tree allocation estimate overflow"))?;
    let histogram_output_bytes =
        allocation_bytes::<HistogramBin>(histogram_output_capacity, "histogram output allocation")?;
    let final_run_inventory_bytes = u64::try_from(final_partition_count)
        .map_err(|_| overflow("final partition count does not fit u64"))?
        .checked_mul(conservative_run_meta_bytes(options)?)
        .ok_or_else(|| overflow("final run inventory allocation estimate overflow"))?;
    let payload = retained_bytes
        .checked_add(histogram_tree_bytes)
        .and_then(|value| value.checked_add(histogram_output_bytes))
        .and_then(|value| value.checked_add(final_run_inventory_bytes))
        .ok_or_else(|| overflow("count summary allocation estimate overflow"))?;
    let fixed = SUMMARY_FIXED_ALLOCATION_BYTES
        .checked_add(count_io_buffer_bytes(1)?)
        .ok_or_else(|| overflow("count summary fixed allocation estimate overflow"))?;
    allocation_with_margin(payload, fixed)
}

fn allocation_bytes<T>(count: usize, context: &'static str) -> Result<u64> {
    count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(|| {
            error(
                ErrorCode::ResourceIntegerOverflow,
                format!("{context} overflow"),
            )
        })
        .and_then(|bytes| {
            u64::try_from(bytes).map_err(|_| {
                error(
                    ErrorCode::ResourceIntegerOverflow,
                    format!("{context} does not fit u64"),
                )
            })
        })
}

fn count_io_buffer_bytes(count: usize) -> Result<u64> {
    u64::try_from(count)
        .map_err(|_| overflow("count I/O buffer count does not fit u64"))?
        .checked_mul(
            u64::try_from(COUNT_IO_BUFFER_BYTES)
                .map_err(|_| overflow("count I/O buffer size does not fit u64"))?,
        )
        .ok_or_else(|| overflow("count I/O buffer byte estimate overflow"))
}

fn conservative_run_meta_bytes(options: &CountOptions) -> Result<u64> {
    let struct_slots = u64::try_from(std::mem::size_of::<RunMeta>())
        .map_err(|_| overflow("run metadata structure size does not fit u64"))?
        .checked_mul(2)
        .ok_or_else(|| overflow("run metadata vector-slack estimate overflow"))?;
    let path_bytes = u64::try_from(options.work_dir.as_os_str().as_encoded_bytes().len())
        .map_err(|_| overflow("count work-directory length does not fit u64"))?
        .checked_add(64)
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| overflow("run metadata path allocation estimate overflow"))?;
    struct_slots
        .checked_add(path_bytes)
        .and_then(|value| value.checked_add(128))
        .ok_or_else(|| overflow("per-run metadata allocation estimate overflow"))
}

fn allocation_with_margin(payload: u64, fixed: u64) -> Result<u64> {
    let margin = payload
        .checked_add(ALLOCATION_MARGIN_DENOMINATOR - 1)
        .ok_or_else(|| overflow("allocation margin rounding overflow"))?
        / ALLOCATION_MARGIN_DENOMINATOR;
    payload
        .checked_add(margin)
        .and_then(|value| value.checked_add(fixed))
        .ok_or_else(|| overflow("conservative allocation estimate overflow"))
}

fn ensure_summary_budget(
    options: &CountOptions,
    retained_capacity: usize,
    histogram_bins: usize,
    histogram_output_capacity: usize,
    final_partition_count: usize,
) -> Result<()> {
    let required = summary_allocation_bytes(
        options,
        retained_capacity,
        histogram_bins,
        histogram_output_capacity,
        final_partition_count,
    )?;
    if required > options.memory_budget_bytes {
        return Err(error(
            ErrorCode::ResourceMemory,
            format!(
                "exact-count summary allocation estimate {required} bytes exceeds phase memory budget {} bytes",
                options.memory_budget_bytes
            ),
        ));
    }
    Ok(())
}

fn validate_canonical_key(key: u128, k: u8) -> Result<()> {
    let active_bits = 2 * u32::from(k);
    let mask = (1_u128 << active_bits) - 1;
    if key & !mask != 0 || canonical_code(key, k)? != key {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count event is not a valid canonical full key",
        ));
    }
    Ok(())
}

fn partition_for(key: u128, k: u8, prefix_bits: u8) -> u16 {
    if prefix_bits == 0 {
        0
    } else {
        ((key >> (2 * u32::from(k) - u32::from(prefix_bits))) & ((1_u128 << prefix_bits) - 1))
            as u16
    }
}

struct MergeRecords {
    readers: Vec<RunReader>,
    heap: BinaryHeap<Reverse<(u128, usize, u64)>>,
}

impl Iterator for MergeRecords {
    type Item = Result<KmerCount>;

    fn next(&mut self) -> Option<Self::Item> {
        let Reverse((key, index, support)) = self.heap.pop()?;
        let mut combined = support;
        if let Err(error) = push_next(&mut self.readers, &mut self.heap, index) {
            return Some(Err(error));
        }
        while self
            .heap
            .peek()
            .is_some_and(|Reverse((next, _, _))| *next == key)
        {
            let Reverse((_, next_index, next_support)) = self.heap.pop().expect("heap peeked");
            combined = match combined.checked_add(next_support) {
                Some(value) => value,
                None => return Some(Err(overflow("merged k-mer support overflow"))),
            };
            if let Err(error) = push_next(&mut self.readers, &mut self.heap, next_index) {
                return Some(Err(error));
            }
        }
        Some(Ok(KmerCount {
            key,
            support: combined,
        }))
    }
}

fn merge_run_records(
    runs: &[RunMeta],
    k: u8,
    prefix_bits: u8,
    partition: u16,
) -> Result<MergeRecords> {
    let mut readers = Vec::new();
    readers.try_reserve_exact(runs.len()).map_err(|cause| {
        error(
            ErrorCode::ResourceMemory,
            format!("cannot allocate merge reader inventory: {cause}"),
        )
    })?;
    for run in runs {
        if run.partition != partition {
            return Err(error(
                ErrorCode::IntegrityCountRun,
                "merge group crosses numeric partitions",
            ));
        }
        readers.push(RunReader::open(run, k, prefix_bits, partition)?);
    }
    let mut heap = BinaryHeap::new();
    heap.try_reserve_exact(readers.len()).map_err(|cause| {
        error(
            ErrorCode::ResourceMemory,
            format!("cannot allocate merge heap: {cause}"),
        )
    })?;
    for index in 0..readers.len() {
        push_next(&mut readers, &mut heap, index)?;
    }
    Ok(MergeRecords { readers, heap })
}

fn push_next(
    readers: &mut [RunReader],
    heap: &mut BinaryHeap<Reverse<(u128, usize, u64)>>,
    index: usize,
) -> Result<()> {
    if let Some(record) = readers[index].next_record()? {
        heap.push(Reverse((record.key, index, record.support)));
    }
    Ok(())
}

struct RunReader {
    reader: BufReader<File>,
    remaining: u64,
    previous: Option<u128>,
    k: u8,
    prefix_bits: u8,
    partition: u16,
    digest: Sha256,
    file_digest: Sha256,
    expected_file_sha256: String,
    authenticated: bool,
    failed: bool,
}

impl RunReader {
    fn open(run: &RunMeta, k: u8, prefix_bits: u8, partition: u16) -> Result<Self> {
        let file = File::open(&run.path)
            .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, "open count run", cause))?;
        let actual_bytes = file
            .metadata()
            .map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityCountRun,
                    "inspect opened count run",
                    cause,
                )
            })?
            .len();
        let mut reader = BufReader::with_capacity(COUNT_IO_BUFFER_BYTES, file);
        let mut header = [0_u8; RUN_HEADER_BYTES as usize];
        reader.read_exact(&mut header).map_err(|cause| {
            io_error(ErrorCode::IntegrityCountRun, "read count run header", cause)
        })?;
        let records = validate_run_header(&header, k, prefix_bits, partition)?;
        let expected_bytes = framed_run_bytes(records)?;
        if actual_bytes != expected_bytes
            || actual_bytes != run.bytes
            || records != run.records
            || run.partition != partition
        {
            return Err(error(
                ErrorCode::IntegrityCountRun,
                "count run frame disagrees with its registered metadata",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(RUN_DOMAIN);
        digest.update(header);
        let mut file_digest = Sha256::new();
        file_digest.update(header);
        Ok(Self {
            reader,
            remaining: records,
            previous: None,
            k,
            prefix_bits,
            partition,
            digest,
            file_digest,
            expected_file_sha256: run.sha256.clone(),
            authenticated: false,
            failed: false,
        })
    }

    fn next_record(&mut self) -> Result<Option<KmerCount>> {
        if self.failed {
            return Err(error(
                ErrorCode::IntegrityCountRun,
                "count run reader cannot continue after an integrity failure",
            ));
        }
        if self.remaining == 0 {
            if !self.authenticated {
                self.finish_authentication()?;
            }
            return Ok(None);
        }
        let result = (|| {
            let mut bytes = [0_u8; RUN_RECORD_BYTES as usize];
            self.reader.read_exact(&mut bytes).map_err(|cause| {
                io_error(ErrorCode::IntegrityCountRun, "read count run record", cause)
            })?;
            self.digest.update(bytes);
            self.file_digest.update(bytes);
            let mut key = [0_u8; 16];
            key.copy_from_slice(&bytes[..16]);
            let mut support = [0_u8; 8];
            support.copy_from_slice(&bytes[16..]);
            let record = KmerCount {
                key: u128::from_be_bytes(key),
                support: u64::from_le_bytes(support),
            };
            validate_canonical_key(record.key, self.k)?;
            if partition_for(record.key, self.k, self.prefix_bits) != self.partition {
                return Err(error(
                    ErrorCode::IntegrityCountRun,
                    "count run record is outside its declared numeric partition",
                ));
            }
            if record.support == 0 || self.previous.is_some_and(|value| value >= record.key) {
                return Err(error(
                    ErrorCode::IntegrityCountRun,
                    "count run record ordering or support is invalid",
                ));
            }
            self.previous = Some(record.key);
            self.remaining -= 1;
            // Authenticate the complete frame before returning its final
            // record. Earlier records may already have fed a private successor
            // payload, but that successor cannot be registered or reclaim its
            // parents unless this terminal check succeeds.
            if self.remaining == 0 {
                self.finish_authentication()?;
            }
            Ok(Some(record))
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn finish_authentication(&mut self) -> Result<()> {
        let result =
            (|| {
                let mut trailer = [0_u8; RUN_TRAILER_BYTES as usize];
                self.reader.read_exact(&mut trailer).map_err(|cause| {
                    io_error(
                        ErrorCode::IntegrityCountRun,
                        "read count run trailer",
                        cause,
                    )
                })?;
                self.file_digest.update(trailer);
                if &trailer[..8] != RUN_END {
                    return Err(error(
                        ErrorCode::IntegrityCountRun,
                        "count run trailer magic mismatch",
                    ));
                }
                let actual = self.digest.clone().finalize();
                if actual[..] != trailer[8..] {
                    return Err(error(
                        ErrorCode::IntegrityCountRun,
                        "count run checksum mismatch",
                    ));
                }
                let mut extra = [0_u8; 1];
                if self.reader.read(&mut extra).map_err(|cause| {
                    io_error(ErrorCode::IntegrityCountRun, "check run EOF", cause)
                })? != 0
                {
                    return Err(error(
                        ErrorCode::IntegrityCountRun,
                        "count run has trailing bytes",
                    ));
                }
                let file_sha256 = lower_hex(&self.file_digest.clone().finalize());
                if file_sha256 != self.expected_file_sha256 {
                    return Err(error(
                        ErrorCode::IntegrityCountRun,
                        "count run bytes disagree with registered SHA-256",
                    ));
                }
                self.authenticated = true;
                Ok(())
            })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

struct FinalRecords<'a> {
    runs: &'a [Option<RunMeta>],
    reader: Option<RunReader>,
    index: usize,
    k: u8,
    prefix_bits: u8,
}

impl Iterator for FinalRecords<'_> {
    type Item = Result<KmerCount>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let run = self.runs.get(self.index)?;
            if run.is_none() {
                self.index += 1;
                continue;
            }
            if self.reader.is_none() {
                let run = run.as_ref().expect("checked as present");
                match RunReader::open(run, self.k, self.prefix_bits, self.index as u16) {
                    Ok(reader) => self.reader = Some(reader),
                    Err(error) => {
                        self.index = self.runs.len();
                        return Some(Err(error));
                    }
                }
            }
            match self.reader.as_mut().expect("opened above").next_record() {
                Ok(Some(record)) => return Some(Ok(record)),
                Ok(None) => {
                    self.reader = None;
                    self.index += 1;
                }
                Err(error) => {
                    self.reader = None;
                    self.index = self.runs.len();
                    return Some(Err(error));
                }
            }
        }
    }
}

fn iter_final_records(
    runs: &[Option<RunMeta>],
    k: u8,
    prefix_bits: u8,
) -> Result<FinalRecords<'_>> {
    let expected = 1_usize << prefix_bits;
    if runs.len() != expected {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "final partition inventory length is invalid",
        ));
    }
    Ok(FinalRecords {
        runs,
        reader: None,
        index: 0,
        k,
        prefix_bits,
    })
}

struct VerifiedRun {
    records: u64,
    sha256: String,
}

fn verify_run_and_hash(path: &Path, k: u8, prefix_bits: u8, partition: u16) -> Result<VerifiedRun> {
    let file = File::open(path)
        .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, "verify count run", cause))?;
    let actual_bytes = file
        .metadata()
        .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, "stat opened count run", cause))?
        .len();
    let mut reader = BufReader::with_capacity(COUNT_IO_BUFFER_BYTES, file);
    let mut header = [0_u8; RUN_HEADER_BYTES as usize];
    reader.read_exact(&mut header).map_err(|cause| {
        io_error(
            ErrorCode::IntegrityCountRun,
            "verify count run header",
            cause,
        )
    })?;
    let records = validate_run_header(&header, k, prefix_bits, partition)?;
    if actual_bytes != framed_run_bytes(records)? {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count run length disagrees with header",
        ));
    }
    let mut run_digest = Sha256::new();
    run_digest.update(RUN_DOMAIN);
    run_digest.update(header);
    let mut file_digest = Sha256::new();
    file_digest.update(header);
    let mut remaining = records.checked_mul(RUN_RECORD_BYTES).ok_or_else(|| {
        error(
            ErrorCode::IntegrityCountRun,
            "count run payload length overflows u64",
        )
    })?;
    let mut block = [0_u8; 64 * 1024];
    while remaining != 0 {
        let wanted = usize::try_from(remaining.min(block.len() as u64))
            .map_err(|_| overflow("run verification chunk does not fit usize"))?;
        reader.read_exact(&mut block[..wanted]).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "verify count run payload",
                cause,
            )
        })?;
        run_digest.update(&block[..wanted]);
        file_digest.update(&block[..wanted]);
        remaining -= wanted as u64;
    }
    let mut trailer = [0_u8; RUN_TRAILER_BYTES as usize];
    reader.read_exact(&mut trailer).map_err(|cause| {
        io_error(
            ErrorCode::IntegrityCountRun,
            "verify count run trailer",
            cause,
        )
    })?;
    file_digest.update(trailer);
    if &trailer[..8] != RUN_END {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count run trailer magic mismatch",
        ));
    }
    let actual = run_digest.finalize();
    if actual[..] != trailer[8..] {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count run checksum mismatch",
        ));
    }
    let mut extra = [0_u8; 1];
    if reader
        .read(&mut extra)
        .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, "check run EOF", cause))?
        != 0
    {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count run has trailing bytes",
        ));
    }
    Ok(VerifiedRun {
        records,
        sha256: lower_hex(&file_digest.finalize()),
    })
}

#[cfg(test)]
fn verify_run(path: &Path, k: u8, prefix_bits: u8, partition: u16) -> Result<()> {
    verify_run_and_hash(path, k, prefix_bits, partition).map(|_| ())
}

fn validate_run_header(
    header: &[u8; RUN_HEADER_BYTES as usize],
    k: u8,
    prefix_bits: u8,
    partition: u16,
) -> Result<u64> {
    if &header[..8] != RUN_MAGIC || header[8] != k {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count run magic or k mismatch",
        ));
    }
    if header[9] != prefix_bits {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count run prefix width mismatch",
        ));
    }
    if u16::from_le_bytes([header[10], header[11]]) != partition {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count run numeric partition mismatch",
        ));
    }
    let mut count = [0_u8; 8];
    count.copy_from_slice(&header[12..20]);
    Ok(u64::from_le_bytes(count))
}

fn framed_run_bytes(records: u64) -> Result<u64> {
    RUN_HEADER_BYTES
        .checked_add(records.checked_mul(RUN_RECORD_BYTES).ok_or_else(|| {
            error(
                ErrorCode::IntegrityCountRun,
                "count run record count overflows its framed length",
            )
        })?)
        .and_then(|value| value.checked_add(RUN_TRAILER_BYTES))
        .ok_or_else(|| {
            error(
                ErrorCode::IntegrityCountRun,
                "count run declared size overflows u64",
            )
        })
}

#[derive(Debug, Clone, Copy)]
struct StateDigestPlan {
    k: u8,
    support_unit_tag: u8,
    observed_count: u64,
    retained_count: u64,
    removed_count: u64,
    minimum_support: u64,
}

fn state_digests_and_collect<I>(
    plan: StateDigestPlan,
    records: I,
    retained: &mut Vec<KmerCount>,
) -> Result<(String, String, String)>
where
    I: IntoIterator<Item = Result<KmerCount>>,
{
    let mut observed_digest = Sha256::new();
    observed_digest.update(b"veritasm:graph-state:v1\0");
    observed_digest.update([plan.k, plan.support_unit_tag]);
    observed_digest.update(plan.observed_count.to_le_bytes());

    let mut retained_digest = Sha256::new();
    retained_digest.update(b"veritasm:graph-state:v1\0");
    retained_digest.update([plan.k, plan.support_unit_tag]);
    retained_digest.update(plan.retained_count.to_le_bytes());

    let mut removed_digest = Sha256::new();
    removed_digest.update(b"veritasm:decision-set:v1\0");
    removed_digest.update([plan.k, plan.support_unit_tag, 1]);
    removed_digest.update(plan.removed_count.to_le_bytes());

    let mut actual_observed = 0_u64;
    let mut actual_retained = 0_u64;
    let mut actual_removed = 0_u64;
    for record in records {
        let record = record?;
        observed_digest.update(record.key.to_be_bytes());
        observed_digest.update(record.support.to_le_bytes());
        actual_observed = actual_observed
            .checked_add(1)
            .ok_or_else(|| overflow("observed state digest record count overflow"))?;
        if record.support >= plan.minimum_support {
            if retained.len() == retained.capacity() {
                return Err(error(
                    ErrorCode::InternalInvariant,
                    "preallocated retained canonical-key table is too small",
                ));
            }
            retained_digest.update(record.key.to_be_bytes());
            retained_digest.update(record.support.to_le_bytes());
            retained.push(record);
            actual_retained = actual_retained
                .checked_add(1)
                .ok_or_else(|| overflow("retained state digest record count overflow"))?;
        } else {
            removed_digest.update(record.key.to_be_bytes());
            removed_digest.update(record.support.to_le_bytes());
            actual_removed = actual_removed
                .checked_add(1)
                .ok_or_else(|| overflow("decision digest record count overflow"))?;
        }
    }
    if actual_observed != plan.observed_count
        || actual_retained != plan.retained_count
        || actual_removed != plan.removed_count
    {
        return Err(error(
            ErrorCode::IntegrityCountRun,
            "count state changed between verified count-summary passes",
        ));
    }
    Ok((
        lower_hex(&observed_digest.finalize()),
        lower_hex(&retained_digest.finalize()),
        lower_hex(&removed_digest.finalize()),
    ))
}

pub fn empty_decision_digest(k: u8, support_unit_tag: u8, stage_order: u8) -> String {
    let mut digest = Sha256::new();
    digest.update(b"veritasm:decision-set:v1\0");
    digest.update([k, support_unit_tag, stage_order]);
    digest.update(0_u64.to_le_bytes());
    lower_hex(&digest.finalize())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::with_capacity(
        COUNT_IO_BUFFER_BYTES,
        File::open(path)
            .map_err(|cause| io_error(ErrorCode::IntegrityManifest, "open for checksum", cause))?,
    );
    let mut digest = Sha256::new();
    let mut block = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut block)
            .map_err(|cause| io_error(ErrorCode::IntegrityManifest, "read for checksum", cause))?;
        if count == 0 {
            break;
        }
        digest.update(&block[..count]);
    }
    Ok(lower_hex(&digest.finalize()))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn error(code: ErrorCode, context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(code, context)
}

fn io_error(code: ErrorCode, context: &'static str, cause: std::io::Error) -> VeritasmError {
    let code = match cause.raw_os_error() {
        Some(raw)
            if raw == rustix::io::Errno::MFILE.raw_os_error()
                || raw == rustix::io::Errno::NFILE.raw_os_error() =>
        {
            ErrorCode::ResourceOpenFiles
        }
        _ => code,
    };
    error(code, format!("{context}: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Profile, SupportUnit};
    use crate::dna::{canonical_code, encode_kmer};
    use std::collections::BTreeMap;
    use std::io::{Seek, SeekFrom, Write};
    use tempfile::TempDir;

    fn config(min_support: u64) -> (ScientificConfig, Limits, TempDir) {
        let scientific = ScientificConfig::resolve(
            3,
            if min_support == 1 {
                Profile::RetainAll
            } else {
                Profile::Thresholded
            },
            SupportUnit::SuppliedFragmentInstance,
            Some(min_support),
            0,
            true,
        )
        .unwrap();
        let limits = Limits {
            partition_prefix_bits: 2,
            sort_buffer_keys: 1_024,
            ..Limits::default()
        };
        let temp = TempDir::new().unwrap();
        (scientific, limits, temp)
    }

    fn occurrence_config(min_support: u64) -> (ScientificConfig, Limits, TempDir) {
        let scientific = ScientificConfig::resolve(
            3,
            if min_support == 1 {
                Profile::RetainAll
            } else {
                Profile::Custom
            },
            SupportUnit::AcceptedWindowOccurrence,
            Some(min_support),
            0,
            true,
        )
        .unwrap();
        let limits = Limits {
            partition_prefix_bits: 6,
            sort_buffer_keys: 1_024,
            ..Limits::default()
        };
        let temp = TempDir::new().unwrap();
        (scientific, limits, temp)
    }

    fn exact_oracle(fragments: &[Vec<u128>]) -> Vec<KmerCount> {
        let mut counts = BTreeMap::<u128, u64>::new();
        for fragment in fragments {
            for &key in fragment {
                *counts.entry(key).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .map(|(key, support)| KmerCount { key, support })
            .collect()
    }

    #[test]
    fn exact_counts_and_retention_match_oracle() {
        let (scientific, limits, temp) = config(2);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let aaa = encode_kmer(b"AAA").unwrap();
        let aac = encode_kmer(b"AAC").unwrap();
        writer.observe_fragment(0, &[aaa, aac]).unwrap();
        writer.observe_fragment(1, &[aaa]).unwrap();
        let result = writer.finish(2).unwrap();
        assert_eq!(result.observed_distinct, 2);
        assert_eq!(result.observed_support_mass, 3);
        assert_eq!(
            result.retained,
            vec![KmerCount {
                key: aaa,
                support: 2
            }]
        );
        assert_eq!(
            result.histogram,
            vec![
                HistogramBin {
                    support: 1,
                    distinct_canonical_keys: 1
                },
                HistogramBin {
                    support: 2,
                    distinct_canonical_keys: 1
                }
            ]
        );
    }

    #[test]
    fn empty_event_stream_has_framed_digests() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let writer = CountWriter::new(options).unwrap();
        let result = writer.finish(0).unwrap();
        assert_eq!(result.observed_distinct, 0);
        assert!(result.retained.is_empty());
        assert_eq!(result.observed_state_sha256.len(), 64);
    }

    #[test]
    fn ordinal_gap_fails_closed() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let error = writer.observe_fragment(1, &[]).unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityOrdinalCoverage);
        assert_eq!(
            writer.observe_fragment(0, &[]).unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
        assert_eq!(
            writer.finish(0).unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
    }

    #[test]
    fn invalid_observation_poisons_the_public_counter_state() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let valid = encode_kmer(b"AAA").unwrap();
        let invalid_for_k3 = 1_u128 << 64;

        assert_eq!(
            writer
                .observe_fragment(0, &[valid, invalid_for_k3])
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityCountRun
        );
        assert_eq!(
            writer.observe_fragment(1, &[valid]).unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
        assert_eq!(
            writer.finish(1).unwrap_err().code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
    }

    #[test]
    fn public_options_are_fully_revalidated_at_construction() {
        let (scientific, limits, temp) = config(1);
        let base = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();

        fn constructor_error(options: CountOptions) -> ErrorCode {
            match CountWriter::new(options) {
                Ok(_) => panic!("invalid public CountOptions unexpectedly constructed a writer"),
                Err(error) => error.code(),
            }
        }

        let mut invalid = base.clone();
        invalid.k = 2;
        assert_eq!(constructor_error(invalid), ErrorCode::ConfigurationInvalidK);
        let mut invalid = base.clone();
        invalid.support_unit_tag = 2;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationUnsupportedCombination
        );
        let mut invalid = base.clone();
        invalid.min_support = 0;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidSupport
        );
        let mut invalid = base.clone();
        invalid.partition_prefix_bits = 7;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.sort_buffer_keys = 0;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.merge_fan_in = 15;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.max_runs = 0;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.max_manifest_bytes = 0;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.max_temp_bytes = 0;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.existing_temp_bytes = invalid.max_temp_bytes + 1;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ResourceTemporaryBytes
        );
        let mut invalid = base.clone();
        invalid.memory_budget_bytes = 0;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.max_retained_kmers = 0;
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base.clone();
        invalid.work_dir = PathBuf::new();
        assert_eq!(
            constructor_error(invalid),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut invalid = base;
        invalid.sort_buffer_keys = 2_000_000;
        invalid.memory_budget_bytes = MIN_MEMORY_BUDGET_BYTES;
        assert_eq!(constructor_error(invalid), ErrorCode::ResourceMemory);
    }

    #[test]
    fn observation_buffer_and_run_inventory_stay_inside_the_concurrent_half_budget() {
        let (scientific, limits, temp) = config(1);
        let mut options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        options.memory_budget_bytes = MIN_MEMORY_BUDGET_BYTES;
        let tuple_bytes = std::mem::size_of::<(u16, u128)>();
        options.sort_buffer_keys = usize::try_from(
            (options.memory_budget_bytes / CONCURRENT_COUNT_SHARE_DIVISOR)
                / u64::try_from(tuple_bytes).unwrap(),
        )
        .unwrap();
        assert_eq!(
            match CountWriter::new(options) {
                Ok(_) => panic!("half-budget buffer left no room for count bookkeeping"),
                Err(error) => error.code(),
            },
            ErrorCode::ResourceMemory
        );

        let second_temp = TempDir::new().unwrap();
        let mut options =
            CountOptions::from_config(second_temp.path(), &scientific, &limits, 0).unwrap();
        options.memory_budget_bytes = MIN_MEMORY_BUDGET_BYTES;
        options.sort_buffer_keys = MIN_SORT_BUFFER_KEYS;
        let allowed = options.memory_budget_bytes / CONCURRENT_COUNT_SHARE_DIVISOR;
        let mut admitted_runs = 0_u64;
        while observation_allocation_bytes(&options, admitted_runs + 1).unwrap() <= allowed {
            admitted_runs += 1;
        }
        assert!(admitted_runs > 0);
        let mut writer = CountWriter::new(options).unwrap();
        writer.run_files_created = admitted_runs;
        assert_eq!(
            writer.before_run().unwrap_err().code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn summary_budget_is_checked_before_a_new_histogram_bin_allocates() {
        let scientific = ScientificConfig::resolve(
            15,
            Profile::RetainAll,
            SupportUnit::SuppliedFragmentInstance,
            Some(1),
            0,
            true,
        )
        .unwrap();
        let limits = Limits {
            memory_budget_bytes: MIN_MEMORY_BUDGET_BYTES,
            partition_prefix_bits: 0,
            sort_buffer_keys: MIN_SORT_BUFFER_KEYS as u64,
            ..Limits::default()
        };
        let temporary = TempDir::new().unwrap();
        let options = CountOptions::from_config(temporary.path(), &scientific, &limits, 0).unwrap();

        let mut low = 0_usize;
        let mut high = 1_usize;
        while summary_allocation_bytes(&options, high, high, high, 1).unwrap()
            <= options.memory_budget_bytes
        {
            high = high.checked_mul(2).unwrap();
        }
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if summary_allocation_bytes(&options, middle, middle, middle, 1).unwrap()
                <= options.memory_budget_bytes
            {
                low = middle;
            } else {
                high = middle;
            }
        }
        assert!(low > 0);
        assert!(
            summary_allocation_bytes(&options, low, low, low, 1).unwrap()
                <= options.memory_budget_bytes
        );
        assert!(
            summary_allocation_bytes(&options, high, high, high, 1).unwrap()
                > options.memory_budget_bytes
        );

        let mut writer = CountWriter::new(options).unwrap();
        writer.buffer = Vec::new();
        let records = (0_u128..(1_u128 << 30))
            .filter(|key| canonical_code(*key, 15).unwrap() == *key)
            .take(high)
            .enumerate()
            .map(|(index, key)| {
                Ok(KmerCount {
                    key,
                    support: u64::try_from(index + 1).unwrap(),
                })
            });
        let run = writer
            .write_run("summary-boundary.bin", 0, records)
            .unwrap();
        let error = writer.summarize(&[Some(run)], "0".repeat(64)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn retained_capacity_and_lazy_reader_overlap_have_a_hard_budget_boundary() {
        let scientific = ScientificConfig::resolve(
            15,
            Profile::RetainAll,
            SupportUnit::SuppliedFragmentInstance,
            Some(1),
            0,
            true,
        )
        .unwrap();
        let limits = Limits {
            memory_budget_bytes: MIN_MEMORY_BUDGET_BYTES,
            partition_prefix_bits: 0,
            sort_buffer_keys: MIN_SORT_BUFFER_KEYS as u64,
            ..Limits::default()
        };
        let temporary = TempDir::new().unwrap();
        let options = CountOptions::from_config(temporary.path(), &scientific, &limits, 0).unwrap();

        let mut low = 0_usize;
        let mut high = 1_usize;
        while ensure_summary_budget(&options, high, 1, 1, 1).is_ok() {
            high = high.checked_mul(2).unwrap();
        }
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if ensure_summary_budget(&options, middle, 1, 1, 1).is_ok() {
                low = middle;
            } else {
                high = middle;
            }
        }
        assert!(low > 0);
        ensure_summary_budget(&options, low, 1, 1, 1).unwrap();
        assert_eq!(
            ensure_summary_budget(&options, high, 1, 1, 1)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn skewed_partition_forces_fan_in_sixteen_multi_pass_merge() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let aaa = encode_kmer(b"AAA").unwrap();
        let fragments = 20_000_u64;
        for ordinal in 0..fragments {
            writer.observe_fragment(ordinal, &[aaa]).unwrap();
        }
        let result = writer.finish(fragments).unwrap();
        assert_eq!(
            result.retained,
            vec![KmerCount {
                key: aaa,
                support: fragments
            }]
        );
        // 20 raw runs, two pass-zero runs, and one pass-one run.
        assert_eq!(result.run_files_created, 23);
        assert_eq!(result.observed_support_mass, fragments);
        let count_dir = temp.path().join("count");
        let live_files = fs::read_dir(&count_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        let live_runs = live_files
            .iter()
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("bin"))
            .collect::<Vec<_>>();
        assert_eq!(live_runs.len(), 1);
        assert_eq!(
            live_runs[0].file_name().and_then(|name| name.to_str()),
            Some("p000-m001-g000000000000.bin")
        );
        let live_bytes = live_files.iter().try_fold(0_u64, |sum, path| {
            sum.checked_add(fs::metadata(path).unwrap().len())
        });
        assert_eq!(result.temporary_bytes, live_bytes.unwrap());

        let manifest = fs::read_to_string(count_dir.join("runs.manifest")).unwrap();
        assert_eq!(
            manifest
                .lines()
                .filter(|line| line.starts_with("ancestry\t"))
                .count(),
            3
        );
    }

    #[test]
    fn failed_merge_authentication_preserves_all_predecessors() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let aaa = encode_kmer(b"AAA").unwrap();
        let events = vec![aaa; 2_048];
        writer.observe_fragment(0, &events).unwrap();
        assert_eq!(writer.initial_runs[0].len(), 2);
        let predecessors = writer.initial_runs[0]
            .iter()
            .map(|run| run.path.clone())
            .collect::<Vec<_>>();

        let corrupt = &predecessors[1];
        let trailer_offset = fs::metadata(corrupt).unwrap().len() - RUN_TRAILER_BYTES;
        let mut file = OpenOptions::new().write(true).open(corrupt).unwrap();
        file.seek(SeekFrom::Start(trailer_offset + 8)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        drop(file);

        assert_eq!(
            writer.finish(1).unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );
        for predecessor in predecessors {
            assert!(predecessor.is_file());
        }
        assert!(fs::read_dir(temp.path().join("count"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .all(|name| !name.contains("-m000-")));
    }

    #[test]
    fn combined_digest_pass_matches_independent_framing() {
        let (scientific, limits, temp) = config(2);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let aaa = encode_kmer(b"AAA").unwrap();
        let aac = encode_kmer(b"AAC").unwrap();
        writer.observe_fragment(0, &[aaa, aac]).unwrap();
        writer.observe_fragment(1, &[aaa]).unwrap();
        let result = writer.finish(2).unwrap();
        let observed = [
            KmerCount {
                key: aaa,
                support: 2,
            },
            KmerCount {
                key: aac,
                support: 1,
            },
        ];

        fn digest(
            domain: &[u8],
            fixed: &[u8],
            records: impl IntoIterator<Item = KmerCount>,
        ) -> String {
            let records = records.into_iter().collect::<Vec<_>>();
            let mut hasher = Sha256::new();
            hasher.update(domain);
            hasher.update(fixed);
            hasher.update((records.len() as u64).to_le_bytes());
            for record in records {
                hasher.update(record.key.to_be_bytes());
                hasher.update(record.support.to_le_bytes());
            }
            lower_hex(&hasher.finalize())
        }

        assert_eq!(
            result.observed_state_sha256,
            digest(
                b"veritasm:graph-state:v1\0",
                &[3, scientific.support_unit.tag()],
                observed
            )
        );
        assert_eq!(
            result.retained_state_sha256,
            digest(
                b"veritasm:graph-state:v1\0",
                &[3, scientific.support_unit.tag()],
                observed.into_iter().filter(|record| record.support >= 2)
            )
        );
        assert_eq!(
            result.removed_decision_sha256,
            digest(
                b"veritasm:decision-set:v1\0",
                &[3, scientific.support_unit.tag(), 1],
                observed.into_iter().filter(|record| record.support < 2)
            )
        );
    }

    #[test]
    fn occurrence_counts_and_digests_ignore_partition_and_flush_layout() {
        let (scientific, mut limits, first_temp) = occurrence_config(2);
        let canonical: Vec<u128> = (0_u128..64)
            .filter(|&key| canonical_code(key, 3).unwrap() == key)
            .collect();
        let fragments: Vec<Vec<u128>> = (0..2_500)
            .map(|ordinal| {
                let key = canonical[ordinal % canonical.len()];
                vec![key, key, canonical[(ordinal * 7 + 3) % canonical.len()]]
            })
            .collect();
        let oracle = exact_oracle(&fragments);

        let mut first = CountWriter::new(
            CountOptions::from_config(first_temp.path(), &scientific, &limits, 0).unwrap(),
        )
        .unwrap();
        for (ordinal, keys) in fragments.iter().enumerate() {
            first.observe_fragment(ordinal as u64, keys).unwrap();
        }
        let first = first.finish(fragments.len() as u64).unwrap();

        limits.partition_prefix_bits = 0;
        limits.sort_buffer_keys = 2_048;
        let second_temp = TempDir::new().unwrap();
        let mut second = CountWriter::new(
            CountOptions::from_config(second_temp.path(), &scientific, &limits, 0).unwrap(),
        )
        .unwrap();
        for (ordinal, keys) in fragments.iter().enumerate() {
            second.observe_fragment(ordinal as u64, keys).unwrap();
        }
        let second = second.finish(fragments.len() as u64).unwrap();

        assert_eq!(first.retained, oracle);
        assert_eq!(second.retained, oracle);
        assert_eq!(first.histogram, second.histogram);
        assert_eq!(first.observed_state_sha256, second.observed_state_sha256);
        assert_eq!(first.retained_state_sha256, second.retained_state_sha256);
        assert_eq!(
            first.removed_decision_sha256,
            second.removed_decision_sha256
        );
    }

    #[test]
    fn corrupted_completed_run_fails_before_summary() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        writer
            .observe_fragment(0, &[encode_kmer(b"AAA").unwrap()])
            .unwrap();
        writer.flush_buffer().unwrap();
        let path = writer
            .initial_runs
            .iter()
            .flatten()
            .next()
            .unwrap()
            .path
            .clone();
        let mut file = OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(RUN_HEADER_BYTES)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        drop(file);

        assert_eq!(
            writer.finish(1).unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn every_single_byte_count_run_corruption_is_rejected() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let run = writer
            .write_run(
                "single-byte-corruption.bin",
                0,
                [
                    Ok(KmerCount {
                        key: encode_kmer(b"AAA").unwrap(),
                        support: 1,
                    }),
                    Ok(KmerCount {
                        key: encode_kmer(b"AAC").unwrap(),
                        support: 2,
                    }),
                ],
            )
            .unwrap();
        let pristine = fs::read(&run.path).unwrap();
        for index in 0..pristine.len() {
            let mut corrupted = pristine.clone();
            corrupted[index] ^= 1;
            fs::write(&run.path, corrupted).unwrap();
            assert_eq!(
                verify_run(&run.path, 3, 2, 0).unwrap_err().code(),
                ErrorCode::IntegrityCountRun,
                "mutation at byte {index} was accepted"
            );
        }
    }

    #[test]
    fn reauthenticated_count_runs_still_enforce_record_semantics() {
        fn resign(bytes: &mut [u8]) {
            let trailer = bytes.len() - RUN_TRAILER_BYTES as usize;
            let mut digest = Sha256::new();
            digest.update(RUN_DOMAIN);
            digest.update(&bytes[..trailer]);
            bytes[trailer + 8..].copy_from_slice(&digest.finalize());
        }

        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let run = writer
            .write_run(
                "semantic-corruption.bin",
                0,
                [
                    Ok(KmerCount {
                        key: encode_kmer(b"AAA").unwrap(),
                        support: 1,
                    }),
                    Ok(KmerCount {
                        key: encode_kmer(b"AAC").unwrap(),
                        support: 2,
                    }),
                ],
            )
            .unwrap();
        let pristine = fs::read(&run.path).unwrap();

        let mut zero_support = pristine.clone();
        zero_support[RUN_HEADER_BYTES as usize + 16..RUN_HEADER_BYTES as usize + 24].fill(0);
        resign(&mut zero_support);
        fs::write(&run.path, zero_support).unwrap();
        let mut reader = RunReader::open(&run, 3, 2, 0).unwrap();
        assert_eq!(
            reader.next_record().unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );

        let mut duplicate_key = pristine.clone();
        let first_key =
            duplicate_key[RUN_HEADER_BYTES as usize..RUN_HEADER_BYTES as usize + 16].to_vec();
        duplicate_key[RUN_HEADER_BYTES as usize + RUN_RECORD_BYTES as usize
            ..RUN_HEADER_BYTES as usize + RUN_RECORD_BYTES as usize + 16]
            .copy_from_slice(&first_key);
        resign(&mut duplicate_key);
        fs::write(&run.path, duplicate_key).unwrap();
        let mut reader = RunReader::open(&run, 3, 2, 0).unwrap();
        assert!(reader.next_record().unwrap().is_some());
        assert_eq!(
            reader.next_record().unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );

        let mut out_of_width = pristine;
        out_of_width[RUN_HEADER_BYTES as usize] = 0x80;
        resign(&mut out_of_width);
        fs::write(&run.path, out_of_width).unwrap();
        let mut reader = RunReader::open(&run, 3, 2, 0).unwrap();
        assert_eq!(
            reader.next_record().unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn authenticated_reader_binds_the_registered_whole_file_digest() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let run = writer
            .write_run(
                "registered-digest.bin",
                0,
                [Ok(KmerCount {
                    key: encode_kmer(b"AAA").unwrap(),
                    support: 1,
                })],
            )
            .unwrap();
        assert_eq!(run.bytes, 84);
        assert_eq!(
            run.sha256,
            "9a670474bf8fecf27a91c626e15bbeba6ba104d68ee267792a9cd557a34be951"
        );
        let mut bytes = fs::read(&run.path).unwrap();
        bytes[RUN_HEADER_BYTES as usize + 16..RUN_HEADER_BYTES as usize + 24]
            .copy_from_slice(&2_u64.to_le_bytes());
        let trailer = bytes.len() - RUN_TRAILER_BYTES as usize;
        let mut digest = Sha256::new();
        digest.update(RUN_DOMAIN);
        digest.update(&bytes[..trailer]);
        bytes[trailer + 8..].copy_from_slice(&digest.finalize());
        fs::write(&run.path, bytes).unwrap();

        // The changed count is structurally valid and its internal run digest
        // has been recomputed. It still cannot replace the exact bytes that
        // were registered in the append-only manifest/RunMeta lineage.
        let mut reader = RunReader::open(&run, 3, 2, 0).unwrap();
        assert_eq!(
            reader.next_record().unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn count_manifest_mutation_is_not_adopted_as_new_provenance() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let aaa = encode_kmer(b"AAA").unwrap();
        writer.observe_fragment(0, &[aaa; 1_024]).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&writer.manifest_path)
            .unwrap()
            .write_all(b"externally-mutated\n")
            .unwrap();

        assert_eq!(
            writer.finish(1).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );
    }

    #[test]
    fn count_temporary_accounting_obeys_limit_minus_one_limit_and_plus_one() {
        fn count_once(
            scientific: &ScientificConfig,
            limits: &Limits,
            directory: &Path,
            existing: u64,
        ) -> Result<CountResult> {
            let options = CountOptions::from_config(directory, scientific, limits, existing)?;
            let mut writer = CountWriter::new(options)?;
            writer.observe_fragment(0, &[encode_kmer(b"AAA").unwrap()])?;
            writer.finish(1)
        }

        let (scientific, mut limits, reference_dir) = config(1);
        let reference = count_once(&scientific, &limits, reference_dir.path(), 0).unwrap();
        let charged = reference.temporary_bytes;
        limits.max_temp_bytes = MIN_TEMP_BYTES;
        assert!(charged > 0 && charged < limits.max_temp_bytes);

        for (spare, succeeds) in [(u64::MAX, false), (0, true), (1, true)] {
            let directory = TempDir::new().unwrap();
            let exact_existing = limits.max_temp_bytes - charged;
            let existing = if spare == u64::MAX {
                exact_existing + 1
            } else {
                exact_existing - spare
            };
            let result = count_once(&scientific, &limits, directory.path(), existing);
            assert_eq!(result.is_ok(), succeeds, "spare-byte case {spare}");
            if let Err(error) = result {
                assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
            }
        }
    }

    #[test]
    fn run_records_must_match_their_declared_numeric_partition() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let error = writer
            .write_run(
                "wrong-partition.bin",
                0,
                [Ok(KmerCount {
                    key: encode_kmer(b"CCA").unwrap(),
                    support: 1,
                })],
            )
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
    }

    #[test]
    fn lazy_final_iterator_advances_at_eof_and_is_terminal_after_error() {
        let (scientific, limits, temp) = config(1);
        let options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let aaa = encode_kmer(b"AAA").unwrap();
        let cca = encode_kmer(b"CCA").unwrap();
        writer.observe_fragment(0, &[aaa, cca]).unwrap();
        writer.flush_buffer().unwrap();

        let final_runs = writer
            .initial_runs
            .iter()
            .map(|runs| runs.first().cloned())
            .collect::<Vec<_>>();
        let corrupt_path = final_runs[1].as_ref().unwrap().path.clone();
        let mut file = OpenOptions::new().write(true).open(corrupt_path).unwrap();
        file.seek(SeekFrom::Start(RUN_HEADER_BYTES)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        drop(file);

        let mut records = iter_final_records(&final_runs, 3, 2).unwrap();
        assert_eq!(records.next().unwrap().unwrap().key, aaa);
        assert_eq!(
            records.next().unwrap().unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );
        assert!(records.next().is_none());
        assert!(records.next().is_none());
    }

    #[test]
    fn retained_key_and_run_caps_fail_closed() {
        let (scientific, limits, temp) = config(1);
        let mut options = CountOptions::from_config(temp.path(), &scientific, &limits, 0).unwrap();
        options.max_retained_kmers = 1;
        let mut writer = CountWriter::new(options).unwrap();
        writer
            .observe_fragment(
                0,
                &[encode_kmer(b"AAA").unwrap(), encode_kmer(b"AAC").unwrap()],
            )
            .unwrap();
        assert_eq!(
            writer.finish(1).unwrap_err().code(),
            ErrorCode::ResourceRetainedKeys
        );

        let second_temp = TempDir::new().unwrap();
        let mut second_limits = limits;
        second_limits.sort_buffer_keys = 1_024;
        second_limits.max_runs = 1;
        let options =
            CountOptions::from_config(second_temp.path(), &scientific, &second_limits, 0).unwrap();
        let mut writer = CountWriter::new(options).unwrap();
        let events = vec![encode_kmer(b"AAA").unwrap(); 2_048];
        assert_eq!(
            writer.observe_fragment(0, &events).unwrap_err().code(),
            ErrorCode::ResourceRunCount
        );
    }

    #[test]
    fn descriptor_exhaustion_is_a_resource_error() {
        for errno in [rustix::io::Errno::MFILE, rustix::io::Errno::NFILE] {
            let error = io_error(
                ErrorCode::IntegrityCountRun,
                "open count run",
                std::io::Error::from_raw_os_error(errno.raw_os_error()),
            );
            assert_eq!(error.code(), ErrorCode::ResourceOpenFiles);
        }
    }
}
