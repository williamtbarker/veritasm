//! Bounded deterministic spill and exact reduction for experimental wide keys.
//!
//! The implementation consumes post-QC exact DNA segments, assigns one
//! immutable event ordinal to every source window, spills fixed-width records,
//! and performs bounded-fan-in merges. Full packed keys decide equality;
//! minimizers and virtual buckets route work only. This remains an isolated
//! occurrence-support research slice and is not called by the stable CLI.

use super::external_run::{
    projected_run_bytes, remove_registered_run, WideRunDomain, WideRunId, WideRunKind, WideRunMeta,
    WideRunPlan, WideRunReader, WideRunRecord, WideRunSupportUnit, WideRunWriter,
    MAX_RUN_IO_BUFFER_BYTES, RUN_VERIFICATION_BUFFER_BYTES,
};
use super::partitioned_dbg::{
    accepted_segments_source_identity, independent_window_oracle, AcceptedSegment,
};
use super::wide_kmer::{validate_code, PackedKmer};
use crate::error::{ErrorCode, Result, VeritasmError};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::mem::size_of;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const RUN_DIRECTORY_PREFIX: &str = "experimental-wide-runs-";
const RUN_DIRECTORY_RANDOM_BYTES: usize = 16;
const RUN_DIRECTORY_MAX_NAME_BYTES: u64 =
    (RUN_DIRECTORY_PREFIX.len() + RUN_DIRECTORY_RANDOM_BYTES) as u64;
const MAX_RUN_FILENAME_BYTES: u64 = 68;
const PATH_SEPARATOR_ALLOWANCE_BYTES: u64 = 4;
const MAX_SIMULTANEOUS_RUN_CATALOGS: u64 = 4;

/// Hard limits for one external occurrence-count experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalPartitionLimits {
    pub max_segments: u64,
    pub max_input_bases: u64,
    pub max_windows: u64,
    pub max_distinct_kmers: u64,
    pub max_memory_bytes: u64,
    pub sort_buffer_bytes: u64,
    pub io_buffer_bytes: u64,
    pub max_temp_bytes: u64,
    pub max_run_files: u64,
    pub merge_fan_in: u16,
    pub max_open_files: u16,
}

/// Complete configuration, including the expected immutable source identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalPartitionOptions {
    pub work_dir: PathBuf,
    pub k: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub source_identity: [u8; 32],
    pub support_unit: WideRunSupportUnit,
    pub limits: ExternalPartitionLimits,
}

/// One exact full-key support result under the result's authenticated unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactSupportCount {
    pub bucket_id: u32,
    pub minimizer: PackedKmer,
    pub key: PackedKmer,
    pub support: u64,
}

/// One verified replacement relation. The output is registered before any
/// predecessor file is reclaimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReplacement {
    pub output_sha256: [u8; 32],
    pub predecessor_sha256: Vec<[u8; 32]>,
    pub support_mass: u64,
    pub reclaimed_bytes: u64,
    pub predecessors_reclaimed: bool,
}

/// Materialized exact result plus operational evidence for the external run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalPartitionResult {
    pub k: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub source_identity: [u8; 32],
    pub support_unit: WideRunSupportUnit,
    pub support_events: u64,
    pub distinct_kmers: u64,
    pub edge_counts: Vec<ExactSupportCount>,
    /// Authenticated summaries of the final runs. Their private files are
    /// reclaimed before success; these rows do not transfer file ownership.
    pub final_runs: Vec<WideRunMeta>,
    pub replacements: Vec<RunReplacement>,
    pub run_files_created: u64,
    pub open_files_high_water: u16,
    pub temporary_bytes_final: u64,
    pub temporary_bytes_high_water: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Observation {
    bucket_id: u32,
    minimizer: PackedKmer,
    key: PackedKmer,
    event_ordinal: u64,
}

#[derive(Debug)]
struct TempLedger {
    live: u64,
    high_water: u64,
    limit: u64,
}

impl TempLedger {
    fn reserve(&mut self, bytes: u64) -> Result<()> {
        let next = self
            .live
            .checked_add(bytes)
            .ok_or_else(|| overflow("experimental temporary byte count overflow"))?;
        if next > self.limit {
            return Err(VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!(
                    "experimental wide runs would use {next} temporary bytes, exceeding limit {}",
                    self.limit
                ),
            ));
        }
        self.live = next;
        self.high_water = self.high_water.max(next);
        Ok(())
    }

    fn release(&mut self, bytes: u64) -> Result<()> {
        self.live = self
            .live
            .checked_sub(bytes)
            .ok_or_else(|| overflow("experimental temporary byte accounting underflow"))?;
        Ok(())
    }
}

struct BuildState<'a> {
    options: &'a ExternalPartitionOptions,
    run_dir: &'a Path,
    io_buffer_bytes: usize,
    run_files_created: u64,
    open_files_high_water: u16,
    temp: TempLedger,
    /// Caller-owned-by-contract payload that remains live through final
    /// materialization (for example, a bridge-owned source binding path).
    additional_final_bytes: u64,
    replacements: Vec<RunReplacement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
    mode: u32,
}

impl DirectoryIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner_uid: metadata.uid(),
            mode: metadata.mode(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CleanupFileIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
    mode: u32,
    byte_len: u64,
}

impl CleanupFileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner_uid: metadata.uid(),
            mode: metadata.mode(),
            byte_len: metadata.len(),
        }
    }
}

struct RunDirectoryGuard {
    path: PathBuf,
    identity: DirectoryIdentity,
    max_entries: u64,
    armed: bool,
}

impl RunDirectoryGuard {
    fn create(work_dir: &Path, max_entries: u64) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix(RUN_DIRECTORY_PREFIX)
            .rand_bytes(RUN_DIRECTORY_RANDOM_BYTES)
            .tempdir_in(work_dir)
            .map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "create unique experimental wide-run directory",
                    cause,
                )
            })?;
        // Disarm tempfile's recursive pathname cleanup immediately. From this
        // point onward we remove only an identity-matching empty directory or
        // the separately validated bounded XWR entries below.
        let path = directory.keep();
        let initial_metadata = fs::symlink_metadata(&path).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat unique experimental wide-run directory",
                cause,
            )
        })?;
        if !initial_metadata.file_type().is_dir() || initial_metadata.file_type().is_symlink() {
            return integrity("new experimental wide-run path is not a literal directory");
        }
        let initial_identity = DirectoryIdentity::from_metadata(&initial_metadata);
        let initialized = (|| {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "set private experimental wide-run directory permissions",
                    cause,
                )
            })?;
            let path_metadata = fs::symlink_metadata(&path).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "restat private experimental wide-run directory",
                    cause,
                )
            })?;
            let identity = validate_directory_metadata(
                &path_metadata,
                "validate unique experimental wide-run directory",
            )?;
            if !same_directory_object(identity, initial_identity) {
                return integrity("experimental wide-run directory changed during initialization");
            }
            let anchor = open_directory_nofollow(&path)?;
            let anchor_identity = validate_directory_metadata(
                &anchor.metadata().map_err(|cause| {
                    io_error(
                        ErrorCode::ResourceTemporaryBytes,
                        "stat experimental wide-run directory descriptor",
                        cause,
                    )
                })?,
                "validate experimental wide-run directory descriptor",
            )?;
            if anchor_identity != identity {
                return integrity("experimental wide-run directory changed while it was opened");
            }
            Ok(identity)
        })();
        let identity = match initialized {
            Ok(identity) => identity,
            Err(primary) => {
                return match remove_same_empty_directory(&path, initial_identity) {
                    Ok(()) => Err(primary),
                    Err(cleanup) => Err(VeritasmError::new(
                        ErrorCode::ResourceTemporaryBytes,
                        format!(
                            "experimental wide-run directory initialization failed ({primary}); identity-safe cleanup also failed: {cleanup}"
                        ),
                    )),
                };
            }
        };
        Ok(Self {
            path,
            identity,
            max_entries,
            armed: true,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn remove_empty(mut self) -> Result<()> {
        verify_owned_run_directory(&self.path, self.identity)?;
        fs::remove_dir(&self.path).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "remove empty experimental wide-run directory",
                cause,
            )
        })?;
        self.armed = false;
        Ok(())
    }

    fn cleanup_after_error<T>(mut self, primary: VeritasmError) -> Result<T> {
        match remove_owned_run_directory_contents(&self.path, self.identity, self.max_entries) {
            Ok(()) => {
                self.armed = false;
                Err(primary)
            }
            Err(cleanup) => {
                self.armed = false;
                Err(VeritasmError::new(
                    ErrorCode::ResourceTemporaryBytes,
                    format!(
                        "experimental wide-run build failed ({primary}); cleanup of retained private directory {} also failed: {cleanup}",
                        self.path.display()
                    ),
                ))
            }
        }
    }
}

const fn same_directory_object(left: DirectoryIdentity, right: DirectoryIdentity) -> bool {
    left.device == right.device && left.inode == right.inode && left.owner_uid == right.owner_uid
}

fn remove_same_empty_directory(path: &Path, expected: DirectoryIdentity) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat provisional experimental wide-run directory",
            cause,
        )
    })?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || !same_directory_object(DirectoryIdentity::from_metadata(&metadata), expected)
    {
        return integrity("provisional experimental wide-run directory identity changed");
    }
    fs::remove_dir(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "remove provisional empty experimental wide-run directory",
            cause,
        )
    })
}

impl Drop for RunDirectoryGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ =
                remove_owned_run_directory_contents(&self.path, self.identity, self.max_entries);
        }
    }
}

fn validate_directory_metadata(
    metadata: &fs::Metadata,
    action: &'static str,
) -> Result<DirectoryIdentity> {
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return integrity(format!("{action}: path is not a literal directory"));
    }
    if metadata.mode() & 0o777 != 0o700 {
        return integrity(format!(
            "{action}: private directory permissions are not 0700"
        ));
    }
    Ok(DirectoryIdentity::from_metadata(metadata))
}

fn open_directory_nofollow(path: &Path) -> Result<File> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let descriptor = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot open private experimental wide-run directory: {cause}"),
        )
    })?;
    Ok(File::from(descriptor))
}

fn verify_owned_run_directory(path: &Path, expected: DirectoryIdentity) -> Result<()> {
    let path_metadata = fs::symlink_metadata(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat owned experimental wide-run directory",
            cause,
        )
    })?;
    let path_identity = validate_directory_metadata(
        &path_metadata,
        "validate owned experimental wide-run directory",
    )?;
    if path_identity != expected {
        return integrity("experimental wide-run directory identity changed before cleanup");
    }
    Ok(())
}

fn is_wide_run_filename(name: &std::ffi::OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    bytes.len() == MAX_RUN_FILENAME_BYTES as usize
        && bytes.starts_with(b"bucket-")
        && bytes[7..17].iter().all(u8::is_ascii_digit)
        && &bytes[17..29] == b"-generation-"
        && bytes[29..39].iter().all(u8::is_ascii_digit)
        && &bytes[39..44] == b"-run-"
        && bytes[44..64].iter().all(u8::is_ascii_digit)
        && &bytes[64..] == b".xwr"
}

fn open_cleanup_file_nofollow(path: &Path) -> Result<File> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let descriptor = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot open experimental wide-run cleanup entry: {cause}"),
        )
    })?;
    Ok(File::from(descriptor))
}

fn validate_cleanup_entry(entry: &fs::DirEntry, owner_uid: u32) -> Result<CleanupFileIdentity> {
    if !is_wide_run_filename(&entry.file_name()) {
        return integrity("private experimental run directory contains an unknown entry name");
    }
    let path = entry.path();
    let path_metadata = fs::symlink_metadata(&path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat experimental wide-run cleanup entry",
            cause,
        )
    })?;
    if !path_metadata.file_type().is_file()
        || path_metadata.file_type().is_symlink()
        || path_metadata.uid() != owner_uid
        || path_metadata.mode() & 0o777 != 0o600
    {
        return integrity(
            "private experimental run directory contains a non-private or non-regular entry",
        );
    }
    let path_identity = CleanupFileIdentity::from_metadata(&path_metadata);
    let file = open_cleanup_file_nofollow(&path)?;
    let descriptor_metadata = file.metadata().map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat experimental wide-run cleanup descriptor",
            cause,
        )
    })?;
    let descriptor_identity = CleanupFileIdentity::from_metadata(&descriptor_metadata);
    if descriptor_identity != path_identity {
        return integrity("experimental wide-run cleanup entry changed while it was opened");
    }
    Ok(path_identity)
}

fn scan_cleanup_entries(path: &Path, owner_uid: u32, max_entries: u64, remove: bool) -> Result<()> {
    let mut entries = 0_u64;
    for entry in fs::read_dir(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "read private experimental wide-run directory",
            cause,
        )
    })? {
        let entry = entry.map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "read private experimental wide-run directory entry",
                cause,
            )
        })?;
        entries = entries
            .checked_add(1)
            .ok_or_else(|| overflow("experimental cleanup entry count overflow"))?;
        if entries > max_entries {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRunCount,
                format!(
                    "experimental cleanup entry count exceeds admitted run limit {max_entries}"
                ),
            ));
        }
        validate_cleanup_entry(&entry, owner_uid)?;
        if remove {
            fs::remove_file(entry.path()).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "remove validated experimental wide-run cleanup entry",
                    cause,
                )
            })?;
        }
    }
    Ok(())
}

fn remove_owned_run_directory_contents(
    path: &Path,
    expected: DirectoryIdentity,
    max_entries: u64,
) -> Result<()> {
    verify_owned_run_directory(path, expected)?;
    // Validate every bounded entry before deleting any. The second pass
    // revalidates each entry immediately before unlinking it.
    scan_cleanup_entries(path, expected.owner_uid, max_entries, false)?;
    verify_owned_run_directory(path, expected)?;
    scan_cleanup_entries(path, expected.owner_uid, max_entries, true)?;
    verify_owned_run_directory(path, expected)?;
    fs::remove_dir(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "remove identity-verified experimental wide-run directory",
            cause,
        )
    })
}

/// Build exact wide-key occurrence counts through authenticated external runs.
///
/// The caller must compute `options.source_identity` with
/// [`accepted_segments_source_identity`] from the same complete segment set.
/// A mismatch fails before any work directory is created.
pub fn build_external_partitioned_counts(
    options: &ExternalPartitionOptions,
    segments: &[AcceptedSegment<'_>],
) -> Result<ExternalPartitionResult> {
    if options.support_unit != WideRunSupportUnit::AcceptedWindowOccurrence {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationProfileConflict,
            "accepted-segment occurrence wrapper requires accepted-window-occurrence support",
        ));
    }
    let admission = validate_options_and_input(options, segments)?;
    let actual_source = accepted_segments_source_identity(segments)?;
    if actual_source != options.source_identity {
        return Err(VeritasmError::new(
            ErrorCode::IntegritySpool,
            "experimental external partition source identity mismatch",
        ));
    }

    build_external_partitioned_observation_stream(
        options,
        admission.accepted_windows,
        admission.additional_scan_bytes,
        0,
        |emit| {
            let mut order = sorted_segment_order(segments)?;
            let k = usize::from(options.k);
            for index in order.drain(..) {
                let segment = segments[index];
                if segment.bases.len() < k {
                    continue;
                }
                for position in 0..=segment.bases.len() - k {
                    let end = position
                        .checked_add(k)
                        .ok_or_else(|| overflow("experimental external window end overflow"))?;
                    let window = segment.bases.get(position..end).ok_or_else(|| {
                        VeritasmError::new(
                            ErrorCode::InternalInvariant,
                            "experimental external source window is out of bounds",
                        )
                    })?;
                    let proof = independent_window_oracle(
                        window,
                        options.k,
                        options.minimizer_length,
                        options.virtual_bucket_count,
                    )?;
                    emit(proof.key, proof.owner.key, proof.bucket_id)?;
                }
            }
            Ok(())
        },
    )
}

/// Feed pre-canonicalized exact keys into the authenticated external reducer.
///
/// This crate-private entry point exists for immutable streaming sources such
/// as `VTSPOOL1`. The caller must bind `options.source_identity` to the
/// complete verified source and every scientific option that changes which
/// keys are emitted. `expected_events` is checked exactly after the producer
/// returns; it is not trusted as a substitute for stream conservation.
#[allow(
    dead_code,
    reason = "retained as the independent slow exact-routing fallback and test oracle"
)]
pub(crate) fn build_external_partitioned_key_stream<F>(
    options: &ExternalPartitionOptions,
    source_items: u64,
    input_bases: u64,
    expected_events: u64,
    additional_scan_bytes: u64,
    additional_final_bytes: u64,
    produce: F,
) -> Result<ExternalPartitionResult>
where
    F: FnOnce(&mut dyn FnMut(PackedKmer) -> Result<()>) -> Result<()>,
{
    let additional_scan_bytes = key_stream_scan_bytes(options, additional_scan_bytes)?;
    let admission = validate_common_options(
        options,
        source_items,
        input_bases,
        expected_events,
        additional_scan_bytes,
    )?;
    build_external_partitioned_observation_stream(
        options,
        expected_events,
        admission.additional_scan_bytes,
        additional_final_bytes,
        |emit| {
            produce(&mut |key| {
                let owner = super::partitioned_dbg::select_minimizer(
                    key,
                    options.k,
                    options.minimizer_length,
                )?;
                let bucket_id = super::partitioned_dbg::route_minimizer(
                    owner.key,
                    options.minimizer_length,
                    options.virtual_bucket_count,
                )?;
                emit(key, owner.key, bucket_id)
            })
        },
    )
}

/// Feed pre-canonicalized exact keys with precomputed exact minimizer owners
/// into the authenticated external reducer.
///
/// This is the allocation-free routing companion to
/// [`build_external_partitioned_key_stream`]. It remains crate-private: the
/// authenticated spool bridge is responsible for producing both values from
/// one fused rolling scan. Complete keys still decide equality, the reducer
/// rejects one key acquiring different owners within a bucket, and the spool
/// result validator independently recomputes every distinct retained route
/// with the slow exact selector before exposing the result.
pub(crate) fn build_external_partitioned_routed_key_stream<F>(
    options: &ExternalPartitionOptions,
    source_items: u64,
    input_bases: u64,
    expected_events: u64,
    additional_scan_bytes: u64,
    additional_final_bytes: u64,
    produce: F,
) -> Result<ExternalPartitionResult>
where
    F: FnOnce(&mut dyn FnMut(PackedKmer, PackedKmer) -> Result<()>) -> Result<()>,
{
    // Reuse the slower path's conservative routing-workspace charge even
    // though the fused producer does not allocate or decode per event.
    let additional_scan_bytes = key_stream_scan_bytes(options, additional_scan_bytes)?;
    let admission = validate_common_options(
        options,
        source_items,
        input_bases,
        expected_events,
        additional_scan_bytes,
    )?;
    build_external_partitioned_observation_stream(
        options,
        expected_events,
        admission.additional_scan_bytes,
        additional_final_bytes,
        |emit| {
            produce(&mut |key, minimizer| {
                validate_code(key, options.k)?;
                let bucket_id = super::partitioned_dbg::route_minimizer(
                    minimizer,
                    options.minimizer_length,
                    options.virtual_bucket_count,
                )?;
                emit(key, minimizer, bucket_id)
            })
        },
    )
}

/// Preflight a streaming source against a borrowed work-directory path.
///
/// This lets a bridge admit its complete path-sensitive phase before it owns
/// a cloned `PathBuf`. The scalar template's own `work_dir` is ignored.
pub(crate) fn preflight_external_key_stream(
    options: &ExternalPartitionOptions,
    work_dir: &Path,
    source_items: u64,
    input_bases: u64,
    additional_scan_bytes: u64,
) -> Result<()> {
    let additional_scan_bytes = key_stream_scan_bytes(options, additional_scan_bytes)?;
    validate_common_options_for_work_dir(
        options,
        work_dir,
        source_items,
        input_bases,
        0,
        additional_scan_bytes,
    )?;
    Ok(())
}

fn key_stream_scan_bytes(
    options: &ExternalPartitionOptions,
    caller_scan_bytes: u64,
) -> Result<u64> {
    caller_scan_bytes
        .checked_add(u64::from(options.k))
        .ok_or_else(|| overflow("experimental key-stream routing workspace overflow"))
}

fn build_external_partitioned_observation_stream<F>(
    options: &ExternalPartitionOptions,
    expected_events: u64,
    additional_scan_bytes: u64,
    additional_final_bytes: u64,
    produce: F,
) -> Result<ExternalPartitionResult>
where
    F: FnOnce(&mut dyn FnMut(PackedKmer, PackedKmer, u32) -> Result<()>) -> Result<()>,
{
    let guard = RunDirectoryGuard::create(&options.work_dir, options.limits.max_run_files)?;

    let result = (|| {
        let run_dir = guard.path();
        let io_buffer_bytes = as_usize(options.limits.io_buffer_bytes, "wide-run I/O buffer")?;
        let mut state = BuildState {
            options,
            run_dir,
            io_buffer_bytes,
            run_files_created: 0,
            open_files_high_water: 0,
            temp: TempLedger {
                live: 0,
                high_water: 0,
                limit: options.limits.max_temp_bytes,
            },
            additional_final_bytes,
            replacements: Vec::new(),
        };
        state
            .replacements
            .try_reserve_exact(as_usize(
                options.limits.max_run_files.min(1024),
                "initial experimental replacement ledger capacity",
            )?)
            .map_err(|cause| {
                resource_memory(format!(
                    "cannot reserve experimental replacement ledger: {cause}"
                ))
            })?;
        enforce_capacity_limit(
            state.replacements.capacity(),
            options.limits.max_run_files,
            "experimental replacement ledger capacity",
        )?;

        let observation_capacity = validate_scan_phase_memory(options, additional_scan_bytes)?;
        let mut observations = Vec::new();
        observations
            .try_reserve_exact(observation_capacity)
            .map_err(|cause| {
                resource_memory(format!(
                    "cannot reserve experimental external sort buffer: {cause}"
                ))
            })?;
        if observations
            .capacity()
            .checked_mul(size_of::<Observation>())
            .ok_or_else(|| overflow("experimental external sort-buffer capacity overflow"))?
            > as_usize(options.limits.sort_buffer_bytes, "wide sort-buffer bytes")?
        {
            return Err(resource_memory(
                "allocator returned an experimental sort buffer above its byte admission"
                    .to_string(),
            ));
        }

        let mut initial_runs = Vec::new();
        let mut next_event = 0_u64;
        produce(&mut |key, minimizer, bucket_id| {
            if next_event >= expected_events {
                return invariant("experimental event producer exceeded its declared cardinality");
            }
            observations.push(Observation {
                bucket_id,
                minimizer,
                key,
                event_ordinal: next_event,
            });
            next_event = checked_add(
                next_event,
                1,
                "experimental external event ordinal overflow",
            )?;
            if observations.len() == observation_capacity {
                flush_observations(&mut state, &mut observations, &mut initial_runs)?;
            }
            Ok(())
        })?;
        flush_observations(&mut state, &mut observations, &mut initial_runs)?;
        if next_event != expected_events {
            return invariant("experimental external event-stream conservation failed");
        }

        // `drain` and `clear` retain allocation capacity. These scan-phase
        // buffers are no longer needed and must not remain live while the
        // separately admitted merge phase allocates readers and heaps.
        drop(observations);

        initial_runs.sort_unstable_by_key(|run| {
            (
                run.header.domain.virtual_bucket,
                run.header.first_event_ordinal,
                run.header.run_ordinal,
            )
        });
        let final_runs = reduce_all_buckets(&mut state, initial_runs)?;
        let edge_counts = materialize_final_counts(&mut state, &final_runs, next_event)?;
        reclaim_final_runs(&mut state, &final_runs)?;
        let distinct_kmers = u64::try_from(edge_counts.len())
            .map_err(|_| overflow("experimental distinct k-mer count does not fit in u64"))?;
        Ok(ExternalPartitionResult {
            k: options.k,
            minimizer_length: options.minimizer_length,
            virtual_bucket_count: options.virtual_bucket_count,
            source_identity: options.source_identity,
            support_unit: options.support_unit,
            support_events: next_event,
            distinct_kmers,
            edge_counts,
            final_runs,
            replacements: state.replacements,
            run_files_created: state.run_files_created,
            open_files_high_water: state.open_files_high_water,
            temporary_bytes_final: state.temp.live,
            temporary_bytes_high_water: state.temp.high_water,
        })
    })();

    match result {
        Ok(value) => {
            guard.remove_empty()?;
            Ok(value)
        }
        Err(error) => guard.cleanup_after_error(error),
    }
}

#[derive(Debug, Clone, Copy)]
struct Admission {
    accepted_windows: u64,
    additional_scan_bytes: u64,
}

fn validate_options_and_input(
    options: &ExternalPartitionOptions,
    segments: &[AcceptedSegment<'_>],
) -> Result<Admission> {
    let segment_count = u64::try_from(segments.len())
        .map_err(|_| overflow("experimental segment count does not fit in u64"))?;
    let mut input_bases = 0_u64;
    let mut windows = 0_u64;
    let k = usize::from(options.k);
    for &segment in segments {
        for (position, &base) in segment.bases.iter().enumerate() {
            if !matches!(base.to_ascii_uppercase(), b'A' | b'C' | b'G' | b'T') {
                return Err(VeritasmError::new(
                    ErrorCode::InputNucleotide,
                    format!(
                        "experimental accepted segment {}/{} contains non-ACGT byte 0x{base:02x} at zero-based position {position}",
                        segment.source_ordinal, segment.segment_ordinal
                    ),
                ));
            }
        }
        let bases = u64::try_from(segment.bases.len())
            .map_err(|_| overflow("experimental segment length does not fit in u64"))?;
        input_bases = checked_add(input_bases, bases, "experimental input-base overflow")?;
        if segment.bases.len() >= k {
            windows = checked_add(
                windows,
                u64::try_from(segment.bases.len() - k + 1)
                    .map_err(|_| overflow("experimental window count does not fit in u64"))?,
                "experimental accepted-window overflow",
            )?;
        }
    }
    let order_bytes = segment_count
        .checked_mul(size_of::<usize>() as u64)
        .ok_or_else(|| overflow("experimental segment-order bytes overflow"))?;
    let k_workspace = u64::from(options.k)
        .checked_mul(2)
        .ok_or_else(|| overflow("experimental canonical-window workspace overflow"))?;
    let candidate_workspace = u64::from(options.k)
        .checked_add(
            u64::from(options.minimizer_length)
                .checked_mul(3)
                .ok_or_else(|| overflow("experimental minimizer-oracle workspace overflow"))?,
        )
        .ok_or_else(|| overflow("experimental source-oracle workspace overflow"))?;
    let oracle_workspace = k_workspace.max(candidate_workspace);
    let scan_bytes = checked_add(
        order_bytes,
        oracle_workspace,
        "experimental scan-source workspace overflow",
    )?;
    let mut admission =
        validate_common_options(options, segment_count, input_bases, windows, scan_bytes)?;
    admission.accepted_windows = windows;
    Ok(admission)
}

fn validate_common_options(
    options: &ExternalPartitionOptions,
    source_items: u64,
    input_bases: u64,
    events: u64,
    additional_scan_bytes: u64,
) -> Result<Admission> {
    validate_common_options_for_work_dir(
        options,
        &options.work_dir,
        source_items,
        input_bases,
        events,
        additional_scan_bytes,
    )
}

fn validate_common_options_for_work_dir(
    options: &ExternalPartitionOptions,
    work_dir: &Path,
    source_items: u64,
    input_bases: u64,
    events: u64,
    additional_scan_bytes: u64,
) -> Result<Admission> {
    super::wide_kmer::validate_k(options.k)?;
    if options.minimizer_length == 0 || options.minimizer_length > options.k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental minimizer length must be in 1..={}; received {}",
                options.k, options.minimizer_length
            ),
        ));
    }
    if options.virtual_bucket_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "experimental virtual bucket count must be nonzero",
        ));
    }
    let limits = options.limits;
    if limits.max_memory_bytes == 0
        || limits.sort_buffer_bytes < size_of::<Observation>() as u64
        || limits.io_buffer_bytes == 0
        || limits.io_buffer_bytes > MAX_RUN_IO_BUFFER_BYTES
        || limits.max_temp_bytes == 0
        || limits.max_run_files == 0
        || limits.merge_fan_in < 2
        || limits.max_open_files < 3
        || limits
            .merge_fan_in
            .checked_add(1)
            .is_none_or(|open| open > limits.max_open_files)
    {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "invalid experimental external-partition resource limits",
        ));
    }
    enforce_limit(
        source_items,
        limits.max_segments,
        ErrorCode::ResourceMemory,
        "experimental source item count",
    )?;
    enforce_limit(
        input_bases,
        limits.max_input_bases,
        ErrorCode::ResourceMemory,
        "experimental input bases",
    )?;
    enforce_limit(
        events,
        limits.max_windows,
        ErrorCode::ResourceRetainedKeys,
        "experimental accepted support events",
    )?;
    validate_merge_phase_memory_for_work_dir(options, work_dir)?;
    validate_scan_phase_memory_for_work_dir(options, work_dir, additional_scan_bytes)?;
    Ok(Admission {
        accepted_windows: events,
        additional_scan_bytes,
    })
}

fn validate_scan_phase_memory(
    options: &ExternalPartitionOptions,
    additional_scan_bytes: u64,
) -> Result<usize> {
    validate_scan_phase_memory_for_work_dir(options, &options.work_dir, additional_scan_bytes)
}

fn validate_scan_phase_memory_for_work_dir(
    options: &ExternalPartitionOptions,
    work_dir: &Path,
    additional_scan_bytes: u64,
) -> Result<usize> {
    let limits = options.limits;
    let observation_capacity = as_usize(
        limits.sort_buffer_bytes / size_of::<Observation>() as u64,
        "experimental observation capacity",
    )?;
    let run_state = run_state_allowance(options)?;
    let path_state = path_state_allowance_for(work_dir)?;
    let run_io = limits
        .io_buffer_bytes
        .max(RUN_VERIFICATION_BUFFER_BYTES as u64);
    let observation_bytes = (observation_capacity as u64)
        .checked_mul(size_of::<Observation>() as u64)
        .ok_or_else(|| overflow("experimental observation-buffer bytes overflow"))?;
    enforce_limit(
        checked_add(
            checked_add(
                additional_scan_bytes,
                observation_bytes,
                "experimental scan payload overflow",
            )?,
            checked_add(
                checked_add(
                    run_state,
                    path_state,
                    "experimental scan run-state overflow",
                )?,
                run_io,
                "experimental scan run-I/O overflow",
            )?,
            "experimental scan-phase memory overflow",
        )?,
        limits.max_memory_bytes,
        ErrorCode::ResourceMemory,
        "experimental scan-phase owned payload",
    )?;
    Ok(observation_capacity)
}

fn validate_merge_phase_memory_for_work_dir(
    options: &ExternalPartitionOptions,
    work_dir: &Path,
) -> Result<()> {
    let limits = options.limits;
    let run_state = run_state_allowance(options)?;
    let path_state = path_state_allowance_for(work_dir)?;
    let merge_buffers = u64::from(limits.merge_fan_in + 1)
        .checked_mul(limits.io_buffer_bytes)
        .ok_or_else(|| overflow("experimental merge-buffer bytes overflow"))?;
    let merge_buffers = merge_buffers.max(RUN_VERIFICATION_BUFFER_BYTES as u64);
    let cursor_width = u64::try_from(
        size_of::<Option<WideRunReader>>()
            + size_of::<Reverse<(PackedKmer, usize, PackedKmer, u64)>>(),
    )
    .map_err(|_| overflow("experimental merge-cursor width does not fit u64"))?;
    let merge_cursors = u64::from(limits.merge_fan_in)
        .checked_mul(cursor_width)
        .ok_or_else(|| overflow("experimental merge-cursor bytes overflow"))?;
    enforce_limit(
        checked_add(
            checked_add(
                merge_buffers,
                merge_cursors,
                "experimental merge payload overflow",
            )?,
            checked_add(
                run_state,
                path_state,
                "experimental merge run-state overflow",
            )?,
            "experimental merge-phase memory overflow",
        )?,
        limits.max_memory_bytes,
        ErrorCode::ResourceMemory,
        "experimental merge-phase owned payload",
    )?;
    Ok(())
}

fn run_state_allowance(options: &ExternalPartitionOptions) -> Result<u64> {
    let per_run_catalogs = (size_of::<WideRunMeta>() as u64)
        .checked_mul(MAX_SIMULTANEOUS_RUN_CATALOGS)
        .ok_or_else(|| overflow("experimental run-catalog type allowance overflow"))?;
    let per_run = checked_add(
        checked_add(
            per_run_catalogs,
            size_of::<RunReplacement>() as u64,
            "experimental replacement-state allowance overflow",
        )?,
        size_of::<[u8; 32]>() as u64,
        "experimental predecessor-digest allowance overflow",
    )?;
    options
        .limits
        .max_run_files
        .checked_mul(per_run)
        .ok_or_else(|| overflow("experimental fixed run-state allowance overflow"))
}

fn path_state_allowance(options: &ExternalPartitionOptions) -> Result<u64> {
    path_state_allowance_for(&options.work_dir)
}

fn path_state_allowance_for(work_dir: &Path) -> Result<u64> {
    let run_dir = run_directory_capacity_for(work_dir)?;
    let run_path = run_path_capacity(run_dir)?;
    checked_add(
        checked_add(
            run_dir,
            run_path
                .checked_mul(2)
                .ok_or_else(|| overflow("experimental simultaneous run paths overflow"))?,
            "experimental owned path allowance overflow",
        )?,
        MAX_RUN_FILENAME_BYTES,
        "experimental run filename workspace overflow",
    )
}

fn run_directory_capacity(options: &ExternalPartitionOptions) -> Result<u64> {
    run_directory_capacity_for(&options.work_dir)
}

fn run_directory_capacity_for(work_dir: &Path) -> Result<u64> {
    let work = u64::try_from(work_dir.as_os_str().as_encoded_bytes().len())
        .map_err(|_| overflow("experimental work-directory path length does not fit u64"))?;
    checked_add(
        checked_add(
            work,
            PATH_SEPARATOR_ALLOWANCE_BYTES,
            "experimental run-directory path allowance overflow",
        )?,
        RUN_DIRECTORY_MAX_NAME_BYTES,
        "experimental run-directory path allowance overflow",
    )
}

fn run_path_capacity(run_directory_capacity: u64) -> Result<u64> {
    checked_add(
        checked_add(
            run_directory_capacity,
            PATH_SEPARATOR_ALLOWANCE_BYTES,
            "experimental run path allowance overflow",
        )?,
        MAX_RUN_FILENAME_BYTES,
        "experimental run path allowance overflow",
    )
}

fn sorted_segment_order(segments: &[AcceptedSegment<'_>]) -> Result<Vec<usize>> {
    let mut order = Vec::new();
    order.try_reserve_exact(segments.len()).map_err(|cause| {
        resource_memory(format!(
            "cannot reserve experimental external segment order: {cause}"
        ))
    })?;
    if order.capacity() > segments.len() {
        return Err(resource_memory(
            "allocator returned an experimental segment order above its admission".to_string(),
        ));
    }
    order.extend(0..segments.len());
    order.sort_unstable_by_key(|&index| {
        (
            segments[index].source_ordinal,
            segments[index].segment_ordinal,
        )
    });
    for pair in order.windows(2) {
        let left = segments[pair[0]];
        let right = segments[pair[1]];
        if (left.source_ordinal, left.segment_ordinal)
            == (right.source_ordinal, right.segment_ordinal)
        {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                "duplicate experimental external segment coordinate",
            ));
        }
    }
    Ok(order)
}

fn flush_observations(
    state: &mut BuildState<'_>,
    observations: &mut Vec<Observation>,
    runs: &mut Vec<WideRunMeta>,
) -> Result<()> {
    if observations.is_empty() {
        return Ok(());
    }
    observations.sort_unstable_by_key(|row| (row.bucket_id, row.key, row.event_ordinal));
    let mut start = 0_usize;
    while start < observations.len() {
        let bucket = observations[start].bucket_id;
        let mut end = start + 1;
        while end < observations.len() && observations[end].bucket_id == bucket {
            end += 1;
        }
        let slice = &observations[start..end];
        let first_event = slice
            .iter()
            .map(|row| row.event_ordinal)
            .min()
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "nonempty observation slice had no first event",
                )
            })?;
        let event_end = slice
            .iter()
            .map(|row| row.event_ordinal)
            .max()
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "nonempty observation slice had no final event",
                )
            })?
            .checked_add(1)
            .ok_or_else(|| overflow("experimental event interval overflow"))?;
        let record_count = u64::try_from(slice.len())
            .map_err(|_| overflow("experimental spill record count does not fit in u64"))?;
        let bytes = projected_run_bytes(record_count)?;
        state.temp.reserve(bytes)?;
        let run_ordinal = state.claim_run()?;
        let domain = state.domain(bucket);
        let plan = WideRunPlan {
            domain,
            kind: WideRunKind::Observation,
            generation: 0,
            run_ordinal,
            first_event_ordinal: first_event,
            event_ordinal_end: event_end,
        };
        let path = state.run_path(WideRunId::from_plan(plan))?;
        let mut writer = WideRunWriter::create(&path, plan, state.io_buffer_bytes)?;
        state.observe_open_files(1)?;
        for row in slice {
            writer.push(WideRunRecord {
                key: row.key,
                minimizer: row.minimizer,
                value: row.event_ordinal,
            })?;
        }
        let meta = writer.finish()?;
        if meta.byte_len != bytes {
            return invariant("experimental initial run byte accounting mismatch");
        }
        reserve_catalog_entry(state, runs)?;
        runs.push(meta);
        start = end;
    }
    observations.clear();
    Ok(())
}

impl BuildState<'_> {
    fn domain(&self, bucket: u32) -> WideRunDomain {
        WideRunDomain {
            source_identity: self.options.source_identity,
            support_unit: self.options.support_unit,
            k: self.options.k,
            minimizer_length: self.options.minimizer_length,
            virtual_bucket_count: self.options.virtual_bucket_count,
            virtual_bucket: bucket,
        }
    }

    fn claim_run(&mut self) -> Result<u64> {
        if self.run_files_created >= self.options.limits.max_run_files {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRunCount,
                format!(
                    "experimental run count would exceed configured limit {}",
                    self.options.limits.max_run_files
                ),
            ));
        }
        let ordinal = self.run_files_created;
        self.run_files_created = checked_add(
            self.run_files_created,
            1,
            "experimental run-file count overflow",
        )?;
        Ok(ordinal)
    }

    fn run_path(&self, id: WideRunId) -> Result<PathBuf> {
        let admitted_filename =
            as_usize(MAX_RUN_FILENAME_BYTES, "experimental run filename capacity")?;
        let mut filename = String::new();
        filename
            .try_reserve_exact(admitted_filename)
            .map_err(|cause| {
                resource_memory(format!("cannot reserve experimental run filename: {cause}"))
            })?;
        write!(
            filename,
            "bucket-{:010}-generation-{:010}-run-{:020}.xwr",
            id.virtual_bucket, id.generation, id.run_ordinal
        )
        .map_err(|_| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "cannot format fixed-width experimental run filename",
            )
        })?;
        if filename.len() != admitted_filename || filename.capacity() > admitted_filename {
            return Err(resource_memory(
                "allocator returned an experimental run filename above its admission".to_string(),
            ));
        }

        let admitted_path = as_usize(
            run_path_capacity(run_directory_capacity(self.options)?)?,
            "experimental run path capacity",
        )?;
        let mut path = PathBuf::new();
        path.try_reserve_exact(admitted_path).map_err(|cause| {
            resource_memory(format!("cannot reserve experimental run path: {cause}"))
        })?;
        path.push(self.run_dir);
        path.push(filename);
        if path.capacity() > admitted_path {
            return Err(resource_memory(
                "allocator returned an experimental run path above its admission".to_string(),
            ));
        }
        Ok(path)
    }

    fn observe_open_files(&mut self, open: usize) -> Result<()> {
        let open = u16::try_from(open)
            .map_err(|_| overflow("experimental open-file count does not fit in u16"))?;
        if open > self.options.limits.max_open_files {
            return Err(VeritasmError::new(
                ErrorCode::ResourceOpenFiles,
                format!(
                    "experimental merge requires {open} open files, exceeding limit {}",
                    self.options.limits.max_open_files
                ),
            ));
        }
        self.open_files_high_water = self.open_files_high_water.max(open);
        Ok(())
    }
}

fn reserve_catalog_entry(state: &BuildState<'_>, catalog: &mut Vec<WideRunMeta>) -> Result<()> {
    let projected_entries = u64::try_from(catalog.len())
        .map_err(|_| overflow("experimental run catalog length does not fit in u64"))?
        .checked_add(1)
        .ok_or_else(|| overflow("experimental run catalog length overflow"))?;
    let projected_bytes = projected_entries
        .checked_mul(size_of::<WideRunMeta>() as u64)
        .ok_or_else(|| overflow("experimental run catalog byte allowance overflow"))?;
    enforce_limit(
        projected_bytes,
        state.options.limits.max_memory_bytes,
        ErrorCode::ResourceMemory,
        "experimental run catalog allowance",
    )?;
    catalog.try_reserve_exact(1).map_err(|cause| {
        resource_memory(format!("cannot grow experimental run catalog: {cause}"))
    })?;
    enforce_capacity_limit(
        catalog.capacity(),
        state.options.limits.max_run_files,
        "experimental run catalog capacity",
    )?;
    let actual_bytes = u64::try_from(catalog.capacity())
        .map_err(|_| overflow("experimental run catalog capacity does not fit u64"))?
        .checked_mul(size_of::<WideRunMeta>() as u64)
        .ok_or_else(|| overflow("experimental run catalog capacity overflow"))?;
    enforce_limit(
        actual_bytes,
        state.options.limits.max_memory_bytes,
        ErrorCode::ResourceMemory,
        "experimental actual run catalog payload",
    )
}

fn reduce_all_buckets(
    state: &mut BuildState<'_>,
    runs: Vec<WideRunMeta>,
) -> Result<Vec<WideRunMeta>> {
    let mut final_runs = Vec::new();
    let mut start = 0_usize;
    while start < runs.len() {
        let bucket = runs[start].header.domain.virtual_bucket;
        let mut end = start + 1;
        while end < runs.len() && runs[end].header.domain.virtual_bucket == bucket {
            end += 1;
        }
        let mut current = Vec::new();
        current.try_reserve_exact(end - start).map_err(|cause| {
            resource_memory(format!(
                "cannot reserve experimental bucket run list: {cause}"
            ))
        })?;
        enforce_capacity_limit(
            current.capacity(),
            state.options.limits.max_run_files,
            "experimental current-run catalog capacity",
        )?;
        current.extend(runs[start..end].iter().cloned());
        validate_run_sequence(&current, WideRunKind::Observation)?;
        let mut input_kind = WideRunKind::Observation;
        let mut generation = 1_u32;
        loop {
            let fan_in = usize::from(state.options.limits.merge_fan_in);
            let mut next = Vec::new();
            let groups = current.len().div_ceil(fan_in);
            next.try_reserve_exact(groups).map_err(|cause| {
                resource_memory(format!(
                    "cannot reserve experimental merge output list: {cause}"
                ))
            })?;
            enforce_capacity_limit(
                next.capacity(),
                state.options.limits.max_run_files,
                "experimental next-run catalog capacity",
            )?;
            for group in current.chunks(fan_in) {
                next.push(merge_group(state, group, input_kind, generation)?);
            }
            if next.len() == 1 {
                reserve_catalog_entry(state, &mut final_runs)?;
                final_runs.push(next.pop().ok_or_else(|| {
                    VeritasmError::new(
                        ErrorCode::InternalInvariant,
                        "experimental final merge run disappeared",
                    )
                })?);
                break;
            }
            current = next;
            validate_run_sequence(&current, WideRunKind::Reduced)?;
            input_kind = WideRunKind::Reduced;
            generation = generation
                .checked_add(1)
                .ok_or_else(|| overflow("experimental merge generation overflow"))?;
        }
        start = end;
    }
    final_runs.sort_unstable_by_key(|run| run.header.domain.virtual_bucket);
    Ok(final_runs)
}

fn validate_run_sequence(runs: &[WideRunMeta], kind: WideRunKind) -> Result<()> {
    let mut previous_end = None;
    let mut bucket = None;
    for run in runs {
        if run.header.kind != kind {
            return invariant("experimental merge input kind mismatch");
        }
        if bucket
            .replace(run.header.domain.virtual_bucket)
            .is_some_and(|value| value != run.header.domain.virtual_bucket)
        {
            return invariant("experimental merge sequence crosses virtual buckets");
        }
        if previous_end.is_some_and(|end| end > run.header.first_event_ordinal) {
            return invariant("experimental merge input event intervals overlap");
        }
        previous_end = Some(run.header.event_ordinal_end);
    }
    Ok(())
}

fn merge_group(
    state: &mut BuildState<'_>,
    inputs: &[WideRunMeta],
    input_kind: WideRunKind,
    generation: u32,
) -> Result<WideRunMeta> {
    if inputs.is_empty() {
        return invariant("experimental merge group is empty");
    }
    state.observe_open_files(inputs.len() + 1)?;
    let bucket = inputs[0].header.domain.virtual_bucket;
    let domain = state.domain(bucket);
    let first_event = inputs
        .iter()
        .map(|run| run.header.first_event_ordinal)
        .min()
        .ok_or_else(|| VeritasmError::new(ErrorCode::InternalInvariant, "empty merge interval"))?;
    let event_end = inputs
        .iter()
        .map(|run| run.header.event_ordinal_end)
        .max()
        .ok_or_else(|| VeritasmError::new(ErrorCode::InternalInvariant, "empty merge interval"))?;
    let mut maximum_records = 0_u64;
    let mut expected_occurrences = 0_u64;
    let mut predecessor_bytes = 0_u64;
    let mut predecessor_sha256 = Vec::new();
    predecessor_sha256
        .try_reserve_exact(inputs.len())
        .map_err(|cause| {
            resource_memory(format!(
                "cannot reserve experimental predecessor digest list: {cause}"
            ))
        })?;
    enforce_capacity_limit(
        predecessor_sha256.capacity(),
        u64::try_from(inputs.len())
            .map_err(|_| overflow("experimental merge input count does not fit u64"))?,
        "experimental predecessor digest capacity",
    )?;
    for input in inputs {
        if input.header.domain != domain || input.header.kind != input_kind {
            return invariant("experimental merge input domain mismatch");
        }
        maximum_records = checked_add(
            maximum_records,
            input.header.record_count,
            "experimental merge record upper-bound overflow",
        )?;
        expected_occurrences = checked_add(
            expected_occurrences,
            input.header.support_mass,
            "experimental merge support subtotal overflow",
        )?;
        predecessor_bytes = checked_add(
            predecessor_bytes,
            input.byte_len,
            "experimental predecessor byte subtotal overflow",
        )?;
        predecessor_sha256.push(input.sha256);
    }
    let reserved_output_bytes = projected_run_bytes(maximum_records)?;
    state.temp.reserve(reserved_output_bytes)?;
    let run_ordinal = state.claim_run()?;
    let plan = WideRunPlan {
        domain,
        kind: WideRunKind::Reduced,
        generation,
        run_ordinal,
        first_event_ordinal: first_event,
        event_ordinal_end: event_end,
    };
    let output_path = state.run_path(WideRunId::from_plan(plan))?;
    let mut writer = WideRunWriter::create(&output_path, plan, state.io_buffer_bytes)?;

    let mut readers: Vec<Option<WideRunReader>> = Vec::new();
    readers.try_reserve_exact(inputs.len()).map_err(|cause| {
        resource_memory(format!(
            "cannot reserve experimental merge readers: {cause}"
        ))
    })?;
    enforce_capacity_limit(
        readers.capacity(),
        u64::from(state.options.limits.merge_fan_in),
        "experimental merge-reader capacity",
    )?;
    for input in inputs {
        let input_path = state.run_path(input.id)?;
        readers.push(Some(WideRunReader::open_registered(
            &input_path,
            input,
            domain,
            input_kind,
            state.io_buffer_bytes,
        )?));
    }
    let mut heap: BinaryHeap<Reverse<(PackedKmer, usize, PackedKmer, u64)>> = BinaryHeap::new();
    heap.try_reserve_exact(inputs.len()).map_err(|cause| {
        resource_memory(format!("cannot reserve experimental merge heap: {cause}"))
    })?;
    enforce_capacity_limit(
        heap.capacity(),
        u64::from(state.options.limits.merge_fan_in),
        "experimental merge-heap capacity",
    )?;
    for index in 0..readers.len() {
        prime_reader(&mut readers, &mut heap, index)?;
    }

    let mut active_key = None;
    let mut active_minimizer = PackedKmer::ZERO;
    let mut active_count = 0_u64;
    while let Some(Reverse((key, input_index, minimizer, value))) = heap.pop() {
        let increment = match input_kind {
            WideRunKind::Observation => 1,
            WideRunKind::Reduced => value,
        };
        if active_key == Some(key) {
            if active_minimizer != minimizer {
                return invariant("one complete key acquired two minimizer owners during merge");
            }
            active_count = checked_add(
                active_count,
                increment,
                "experimental merged occurrence count overflow",
            )?;
        } else {
            if let Some(previous_key) = active_key {
                writer.push(WideRunRecord {
                    key: previous_key,
                    minimizer: active_minimizer,
                    value: active_count,
                })?;
            }
            active_key = Some(key);
            active_minimizer = minimizer;
            active_count = increment;
        }
        prime_reader(&mut readers, &mut heap, input_index)?;
    }
    if let Some(key) = active_key {
        writer.push(WideRunRecord {
            key,
            minimizer: active_minimizer,
            value: active_count,
        })?;
    }
    for reader in readers.into_iter().flatten() {
        reader.finish()?;
    }
    let output = writer.finish()?;
    if output.header.support_mass != expected_occurrences {
        return invariant("experimental merge did not conserve support");
    }
    if output.byte_len > reserved_output_bytes {
        return invariant("experimental merge exceeded its reserved output bytes");
    }
    state
        .temp
        .release(reserved_output_bytes - output.byte_len)?;

    state.replacements.try_reserve_exact(1).map_err(|cause| {
        resource_memory(format!(
            "cannot grow experimental replacement ledger: {cause}"
        ))
    })?;
    enforce_capacity_limit(
        state.replacements.capacity(),
        state.options.limits.max_run_files,
        "experimental replacement ledger capacity",
    )?;
    state.replacements.push(RunReplacement {
        output_sha256: output.sha256,
        predecessor_sha256,
        support_mass: output.header.support_mass,
        reclaimed_bytes: 0,
        predecessors_reclaimed: false,
    });
    let replacement_index = state.replacements.len() - 1;
    for input in inputs {
        let input_path = state.run_path(input.id)?;
        remove_registered_run(&input_path, input)?;
        state.temp.release(input.byte_len)?;
    }
    let replacement = &mut state.replacements[replacement_index];
    replacement.reclaimed_bytes = predecessor_bytes;
    replacement.predecessors_reclaimed = true;
    Ok(output)
}

fn prime_reader(
    readers: &mut [Option<WideRunReader>],
    heap: &mut BinaryHeap<Reverse<(PackedKmer, usize, PackedKmer, u64)>>,
    index: usize,
) -> Result<()> {
    let next = readers
        .get_mut(index)
        .and_then(Option::as_mut)
        .ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "experimental merge reader index is unavailable",
            )
        })?
        .next_record()?;
    if let Some(record) = next {
        heap.push(Reverse((record.key, index, record.minimizer, record.value)));
    } else {
        let reader = readers[index].take().ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "experimental exhausted merge reader disappeared",
            )
        })?;
        reader.finish()?;
    }
    Ok(())
}

fn materialize_final_counts(
    state: &mut BuildState<'_>,
    runs: &[WideRunMeta],
    support_events: u64,
) -> Result<Vec<ExactSupportCount>> {
    let mut total_records = 0_u64;
    for run in runs {
        total_records = checked_add(
            total_records,
            run.header.record_count,
            "experimental final record count overflow",
        )?;
    }
    enforce_limit(
        total_records,
        state.options.limits.max_distinct_kmers,
        ErrorCode::ResourceRetainedKeys,
        "experimental distinct exact keys",
    )?;
    validate_final_materialization_memory(
        state.options,
        total_records,
        state.additional_final_bytes,
    )?;
    let capacity = as_usize(total_records, "experimental final-count capacity")?;
    let mut counts = Vec::new();
    counts.try_reserve_exact(capacity).map_err(|cause| {
        resource_memory(format!(
            "cannot reserve experimental final exact counts: {cause}"
        ))
    })?;
    enforce_capacity_limit(
        counts.capacity(),
        total_records,
        "experimental final-count capacity",
    )?;
    let mut support_total = 0_u64;
    let mut previous = None;
    for run in runs {
        state.observe_open_files(1)?;
        let domain = state.domain(run.header.domain.virtual_bucket);
        let path = state.run_path(run.id)?;
        let mut reader = WideRunReader::open_registered(
            &path,
            run,
            domain,
            WideRunKind::Reduced,
            state.io_buffer_bytes,
        )?;
        while let Some(record) = reader.next_record()? {
            let order = (domain.virtual_bucket, record.key);
            if previous.is_some_and(|value| value >= order) {
                return invariant("experimental final exact counts are not globally ordered");
            }
            previous = Some(order);
            support_total = checked_add(
                support_total,
                record.value,
                "experimental final support total overflow",
            )?;
            counts.push(ExactSupportCount {
                bucket_id: domain.virtual_bucket,
                minimizer: record.minimizer,
                key: record.key,
                support: record.value,
            });
        }
        reader.finish()?;
    }
    if support_total != support_events {
        return invariant("experimental final runs do not conserve support events");
    }
    Ok(counts)
}

fn final_materialization_memory_bytes(
    options: &ExternalPartitionOptions,
    total_records: u64,
    additional_final_bytes: u64,
) -> Result<u64> {
    let result_bytes = total_records
        .checked_mul(size_of::<ExactSupportCount>() as u64)
        .ok_or_else(|| overflow("experimental final-count bytes overflow"))?;
    let retained_state = checked_add(
        run_state_allowance(options)?,
        path_state_allowance(options)?,
        "experimental final retained-state overflow",
    )?;
    checked_add(
        checked_add(
            checked_add(
                result_bytes,
                retained_state,
                "experimental final result/state overflow",
            )?,
            additional_final_bytes,
            "experimental final result/state overflow",
        )?,
        options.limits.io_buffer_bytes,
        "experimental final materialization bytes overflow",
    )
}

fn validate_final_materialization_memory(
    options: &ExternalPartitionOptions,
    total_records: u64,
    additional_final_bytes: u64,
) -> Result<()> {
    enforce_limit(
        final_materialization_memory_bytes(options, total_records, additional_final_bytes)?,
        options.limits.max_memory_bytes,
        ErrorCode::ResourceMemory,
        "experimental final-count owned payload",
    )
}

fn reclaim_final_runs(state: &mut BuildState<'_>, runs: &[WideRunMeta]) -> Result<()> {
    for run in runs {
        let path = state.run_path(run.id)?;
        remove_registered_run(&path, run)?;
        state.temp.release(run.byte_len)?;
    }
    if state.temp.live != 0 {
        return invariant("experimental final-run reclamation left temporary bytes live");
    }
    Ok(())
}

fn enforce_limit(observed: u64, limit: u64, code: ErrorCode, label: &'static str) -> Result<()> {
    if observed <= limit {
        Ok(())
    } else {
        Err(VeritasmError::new(
            code,
            format!("{label} {observed} exceeds configured limit {limit}"),
        ))
    }
}

fn enforce_capacity_limit(capacity: usize, limit: u64, label: &'static str) -> Result<()> {
    let capacity = u64::try_from(capacity)
        .map_err(|_| overflow("experimental allocation capacity does not fit u64"))?;
    enforce_limit(capacity, limit, ErrorCode::ResourceMemory, label)
}

fn as_usize(value: u64, label: &'static str) -> Result<usize> {
    usize::try_from(value).map_err(|_| {
        resource_memory(format!(
            "{label} {value} does not fit this platform's usize"
        ))
    })
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn overflow(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn invariant<T>(context: &'static str) -> Result<T> {
    Err(VeritasmError::new(ErrorCode::InternalInvariant, context))
}

fn integrity<T>(context: impl Into<String>) -> Result<T> {
    Err(VeritasmError::new(ErrorCode::IntegrityCountRun, context))
}

fn resource_memory(context: String) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn io_error(code: ErrorCode, action: &'static str, cause: std::io::Error) -> VeritasmError {
    VeritasmError::new(code, format!("cannot {action}: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimental::partitioned_dbg::{
        build_partitioned_dbg, PartitionConfig, PartitionLimits,
    };
    use proptest::prelude::*;

    fn limits() -> ExternalPartitionLimits {
        ExternalPartitionLimits {
            max_segments: 100,
            max_input_bases: 100_000,
            max_windows: 100_000,
            max_distinct_kmers: 100_000,
            max_memory_bytes: 8 * 1024 * 1024,
            sort_buffer_bytes: size_of::<Observation>() as u64 * 2,
            io_buffer_bytes: 128,
            max_temp_bytes: 32 * 1024 * 1024,
            max_run_files: 1_000,
            merge_fan_in: 2,
            max_open_files: 3,
        }
    }

    fn options(work_dir: &Path, segments: &[AcceptedSegment<'_>]) -> ExternalPartitionOptions {
        ExternalPartitionOptions {
            work_dir: work_dir.to_path_buf(),
            k: 5,
            minimizer_length: 3,
            virtual_bucket_count: 3,
            source_identity: accepted_segments_source_identity(segments).unwrap(),
            support_unit: WideRunSupportUnit::AcceptedWindowOccurrence,
            limits: limits(),
        }
    }

    fn owned_run_directories(work_dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(work_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    name.as_encoded_bytes()
                        .starts_with(RUN_DIRECTORY_PREFIX.as_bytes())
                })
            })
            .collect()
    }

    fn oracle(
        segments: &[AcceptedSegment<'_>],
        k: u8,
        m: u8,
        buckets: u32,
    ) -> Vec<ExactSupportCount> {
        let result = build_partitioned_dbg(
            PartitionConfig {
                k,
                minimizer_length: m,
                virtual_bucket_count: buckets,
                limits: PartitionLimits {
                    max_input_bases: 100_000,
                    max_windows: 100_000,
                    max_super_kmers: 100_000,
                    max_distinct_kmers: 100_000,
                    max_accounted_bytes: 64 * 1024 * 1024,
                },
            },
            segments,
        )
        .unwrap();
        result
            .edge_counts
            .into_iter()
            .map(|row| ExactSupportCount {
                bucket_id: row.bucket_id,
                minimizer: row.minimizer,
                key: row.key,
                support: row.occurrences,
            })
            .collect()
    }

    #[test]
    fn forced_multi_pass_external_reduction_matches_independent_oracle() {
        let temp = tempfile::tempdir().unwrap();
        let segments = [
            AcceptedSegment {
                source_ordinal: 9,
                segment_ordinal: 0,
                bases: b"ACGTTGCAAGTCCTAGGCTAACGT",
            },
            AcceptedSegment {
                source_ordinal: 2,
                segment_ordinal: 1,
                bases: b"TTTTACGTTTTACGTCCGTA",
            },
        ];
        let options = options(temp.path(), &segments);
        let result = build_external_partitioned_counts(&options, &segments).unwrap();
        assert_eq!(
            result.edge_counts,
            oracle(
                &segments,
                options.k,
                options.minimizer_length,
                options.virtual_bucket_count
            )
        );
        assert!(result.run_files_created > result.final_runs.len() as u64 + 2);
        assert_eq!(result.open_files_high_water, 3);
        assert!(result.replacements.iter().all(|replacement| {
            replacement.predecessors_reclaimed && replacement.reclaimed_bytes > 0
        }));
        assert_eq!(
            result
                .edge_counts
                .iter()
                .map(|row| row.support)
                .sum::<u64>(),
            result.support_events
        );
        assert_eq!(result.temporary_bytes_final, 0);
        assert!(owned_run_directories(temp.path()).is_empty());
    }

    #[test]
    fn input_order_and_routing_collisions_do_not_change_full_key_counts() {
        let first = AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACGTCCGTATTTACGTA",
        };
        let second = AcceptedSegment {
            source_ordinal: 4,
            segment_ordinal: 2,
            bases: b"CCGTAAACGTTGCA",
        };
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let forward = [first, second];
        let reverse = [second, first];
        let mut options_a = options(a.path(), &forward);
        options_a.virtual_bucket_count = 1;
        let mut options_b = options(b.path(), &reverse);
        options_b.virtual_bucket_count = 1;
        let result_a = build_external_partitioned_counts(&options_a, &forward).unwrap();
        let result_b = build_external_partitioned_counts(&options_b, &reverse).unwrap();
        assert_eq!(result_a.source_identity, result_b.source_identity);
        assert_eq!(result_a.edge_counts, result_b.edge_counts);
        assert_eq!(result_a.run_files_created, result_b.run_files_created);
        assert_eq!(result_a.replacements, result_b.replacements);
        assert_eq!(
            format!("{:?}", result_a.final_runs),
            format!("{:?}", result_b.final_runs)
        );
        assert_eq!(
            result_a
                .final_runs
                .iter()
                .map(|run| (run.header.domain.virtual_bucket, run.sha256))
                .collect::<Vec<_>>(),
            result_b
                .final_runs
                .iter()
                .map(|run| (run.header.domain.virtual_bucket, run.sha256))
                .collect::<Vec<_>>()
        );
        assert!(result_a.edge_counts.len() > 1);
        assert!(result_a.edge_counts.iter().all(|row| row.bucket_id == 0));
    }

    #[test]
    fn wide_k_127_spills_exact_complete_keys() {
        let temp = tempfile::tempdir().unwrap();
        let sequence: Vec<u8> = (0..150).map(|index| b"ACGTTGCA"[index % 8]).collect();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: &sequence,
        }];
        let mut options = options(temp.path(), &segments);
        options.k = 127;
        options.minimizer_length = 31;
        options.virtual_bucket_count = 7;
        options.source_identity = accepted_segments_source_identity(&segments).unwrap();
        let result = build_external_partitioned_counts(&options, &segments).unwrap();
        assert_eq!(result.support_events, 24);
        assert_eq!(result.edge_counts, oracle(&segments, 127, 31, 7));
    }

    #[test]
    fn source_mismatch_and_resource_limits_fail_without_persisting_runs() {
        let temp = tempfile::tempdir().unwrap();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACGTCCGTA",
        }];
        let mut mismatch = options(temp.path(), &segments);
        mismatch.source_identity[0] ^= 1;
        assert_eq!(
            build_external_partitioned_counts(&mismatch, &segments)
                .unwrap_err()
                .code(),
            ErrorCode::IntegritySpool
        );
        assert!(owned_run_directories(temp.path()).is_empty());

        let temp = tempfile::tempdir().unwrap();
        let mut limited = options(temp.path(), &segments);
        limited.limits.max_temp_bytes = 1;
        assert_eq!(
            build_external_partitioned_counts(&limited, &segments)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceTemporaryBytes
        );
        assert!(owned_run_directories(temp.path()).is_empty());
    }

    #[test]
    fn exchanged_valid_run_contents_fail_closed_before_publish_or_reclamation() {
        let temp = tempfile::tempdir().unwrap();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACGTCCGTA",
        }];
        let mut options = options(temp.path(), &segments);
        options.virtual_bucket_count = 1;
        let run_dir = temp.path().join("runs");
        fs::create_dir(&run_dir).unwrap();
        let domain = WideRunDomain {
            source_identity: options.source_identity,
            support_unit: options.support_unit,
            k: options.k,
            minimizer_length: options.minimizer_length,
            virtual_bucket_count: 1,
            virtual_bucket: 0,
        };
        let first_proof = independent_window_oracle(b"AACGT", 5, 3, 1).unwrap();
        let second_proof = independent_window_oracle(b"CCGTA", 5, 3, 1).unwrap();
        assert_ne!(first_proof.key, second_proof.key);
        let first_plan = WideRunPlan {
            domain,
            kind: WideRunKind::Observation,
            generation: 0,
            run_ordinal: 0,
            first_event_ordinal: 4,
            event_ordinal_end: 5,
        };
        let second_plan = WideRunPlan {
            run_ordinal: 1,
            first_event_ordinal: 5,
            event_ordinal_end: 6,
            ..first_plan
        };
        let first_path =
            run_dir.join("bucket-0000000000-generation-0000000000-run-00000000000000000000.xwr");
        let second_path =
            run_dir.join("bucket-0000000000-generation-0000000000-run-00000000000000000001.xwr");
        let mut first_writer = WideRunWriter::create(&first_path, first_plan, 128).unwrap();
        first_writer
            .push(WideRunRecord {
                key: first_proof.key,
                minimizer: first_proof.owner.key,
                value: 4,
            })
            .unwrap();
        let first_meta = first_writer.finish().unwrap();
        let mut second_writer = WideRunWriter::create(&second_path, second_plan, 128).unwrap();
        second_writer
            .push(WideRunRecord {
                key: second_proof.key,
                minimizer: second_proof.owner.key,
                value: 5,
            })
            .unwrap();
        let second_meta = second_writer.finish().unwrap();
        assert_eq!(first_meta.byte_len, second_meta.byte_len);
        assert_ne!(first_meta.sha256, second_meta.sha256);

        let exchange_path = run_dir.join("exchange.xwr");
        fs::rename(&first_path, &exchange_path).unwrap();
        fs::rename(&second_path, &first_path).unwrap();
        fs::rename(&exchange_path, &second_path).unwrap();

        let predecessor_bytes = first_meta.byte_len + second_meta.byte_len;
        let mut state = BuildState {
            options: &options,
            run_dir: &run_dir,
            io_buffer_bytes: 128,
            run_files_created: 2,
            open_files_high_water: 0,
            temp: TempLedger {
                live: predecessor_bytes,
                high_water: predecessor_bytes,
                limit: options.limits.max_temp_bytes,
            },
            additional_final_bytes: 0,
            replacements: Vec::new(),
        };
        let error = merge_group(
            &mut state,
            &[first_meta, second_meta],
            WideRunKind::Observation,
            1,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        assert!(state.replacements.is_empty());
        assert!(first_path.is_file());
        assert!(second_path.is_file());
        assert!(state.temp.live >= predecessor_bytes);

        let successor = state
            .run_path(WideRunId {
                virtual_bucket: 0,
                generation: 1,
                run_ordinal: 2,
            })
            .unwrap();
        assert_eq!(
            super::super::external_run::verify_run(&successor, domain, WideRunKind::Reduced, 128,)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn open_file_and_memory_envelopes_are_validated_before_work() {
        let temp = tempfile::tempdir().unwrap();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACGTCCGTA",
        }];
        let mut invalid = options(temp.path(), &segments);
        invalid.limits.max_open_files = 2;
        assert_eq!(
            build_external_partitioned_counts(&invalid, &segments)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        invalid = options(temp.path(), &segments);
        invalid.limits.io_buffer_bytes = MAX_RUN_IO_BUFFER_BYTES + 1;
        assert_eq!(
            build_external_partitioned_counts(&invalid, &segments)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        invalid = options(temp.path(), &segments);
        invalid.limits.max_memory_bytes = 1;
        assert_eq!(
            build_external_partitioned_counts(&invalid, &segments)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn empty_source_succeeds_with_zero_scientific_limits_and_no_run() {
        let temp = tempfile::tempdir().unwrap();
        let segments: [AcceptedSegment<'_>; 0] = [];
        let mut options = options(temp.path(), &segments);
        options.limits.max_segments = 0;
        options.limits.max_input_bases = 0;
        options.limits.max_windows = 0;
        options.limits.max_distinct_kmers = 0;
        let result = build_external_partitioned_counts(&options, &segments).unwrap();
        assert_eq!(result.support_events, 0);
        assert!(result.edge_counts.is_empty());
        assert!(result.final_runs.is_empty());
        assert_eq!(result.run_files_created, 0);
        assert_eq!(result.temporary_bytes_final, 0);
    }

    #[test]
    fn stale_legacy_run_directory_is_never_reused_or_clobbered() {
        let temp = tempfile::tempdir().unwrap();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACGTCCGTA",
        }];
        let run_dir = temp.path().join("experimental-wide-runs");
        fs::create_dir(&run_dir).unwrap();
        let sentinel = run_dir.join("sentinel");
        fs::write(&sentinel, b"preserve").unwrap();
        let result =
            build_external_partitioned_counts(&options(temp.path(), &segments), &segments).unwrap();
        assert!(!result.edge_counts.is_empty());
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
        assert!(owned_run_directories(temp.path()).is_empty());
    }

    #[test]
    fn unique_run_directories_are_private_and_do_not_collide() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let first = RunDirectoryGuard::create(temp.path(), 4).unwrap();
        let second = RunDirectoryGuard::create(temp.path(), 4).unwrap();
        assert_ne!(first.path(), second.path());
        for path in [first.path(), second.path()] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert!(path
                .file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(RUN_DIRECTORY_PREFIX.as_bytes()));
            assert_eq!(
                path.file_name().unwrap().as_encoded_bytes().len(),
                RUN_DIRECTORY_MAX_NAME_BYTES as usize
            );
        }
        first.remove_empty().unwrap();
        second.remove_empty().unwrap();
        assert!(owned_run_directories(temp.path()).is_empty());
    }

    #[test]
    fn renamed_directory_substitution_is_reported_without_deleting_replacement() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let guard = RunDirectoryGuard::create(temp.path(), 4).unwrap();
        let owned_path = guard.path().to_path_buf();
        let displaced = temp.path().join("displaced-owned-run-directory");
        fs::rename(&owned_path, &displaced).unwrap();
        fs::create_dir(&owned_path).unwrap();
        fs::set_permissions(&owned_path, fs::Permissions::from_mode(0o700)).unwrap();
        let marker = owned_path.join("replacement-marker");
        fs::write(&marker, b"do-not-delete").unwrap();

        let error = guard
            .cleanup_after_error::<()>(VeritasmError::new(
                ErrorCode::InternalInvariant,
                "injected primary failure",
            ))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
        assert_eq!(fs::read(&marker).unwrap(), b"do-not-delete");

        fs::remove_file(marker).unwrap();
        fs::remove_dir(owned_path).unwrap();
        fs::remove_dir(displaced).unwrap();
    }

    #[test]
    fn unexpected_cleanup_entry_is_reported_and_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let guard = RunDirectoryGuard::create(temp.path(), 4).unwrap();
        let owned_path = guard.path().to_path_buf();
        let sentinel = owned_path.join("unexpected-sentinel");
        fs::write(&sentinel, b"preserve").unwrap();

        let error = guard
            .cleanup_after_error::<()>(VeritasmError::new(
                ErrorCode::InternalInvariant,
                "injected primary failure",
            ))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
        assert!(error.to_string().contains("cleanup"));
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");

        fs::remove_file(sentinel).unwrap();
        fs::remove_dir(owned_path).unwrap();
    }

    #[test]
    fn changed_directory_permissions_are_reported_without_cleanup() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let guard = RunDirectoryGuard::create(temp.path(), 4).unwrap();
        let owned_path = guard.path().to_path_buf();
        fs::set_permissions(&owned_path, fs::Permissions::from_mode(0o750)).unwrap();

        let error = guard
            .cleanup_after_error::<()>(VeritasmError::new(
                ErrorCode::InternalInvariant,
                "injected primary failure",
            ))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
        assert!(owned_path.exists());

        fs::set_permissions(&owned_path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(owned_path).unwrap();
    }

    #[test]
    fn owned_memory_admission_has_exact_boundaries_and_charges_long_paths() {
        let temp = tempfile::tempdir().unwrap();
        let segments: [AcceptedSegment<'_>; 0] = [];
        let mut candidate = options(temp.path(), &segments);
        candidate.limits.max_run_files = 8;
        candidate.limits.io_buffer_bytes = 1;

        let additional_scan = 37_u64;
        let observation_capacity =
            candidate.limits.sort_buffer_bytes / size_of::<Observation>() as u64;
        let scan_required = additional_scan
            + observation_capacity * size_of::<Observation>() as u64
            + run_state_allowance(&candidate).unwrap()
            + path_state_allowance(&candidate).unwrap()
            + RUN_VERIFICATION_BUFFER_BYTES as u64;
        for (budget, accepted) in [
            (scan_required - 1, false),
            (scan_required, true),
            (scan_required + 1, true),
        ] {
            candidate.limits.max_memory_bytes = budget;
            assert_eq!(
                validate_scan_phase_memory(&candidate, additional_scan).is_ok(),
                accepted
            );
        }

        let cursor_width = (size_of::<Option<WideRunReader>>()
            + size_of::<Reverse<(PackedKmer, usize, PackedKmer, u64)>>())
            as u64;
        let merge_required = RUN_VERIFICATION_BUFFER_BYTES as u64
            + u64::from(candidate.limits.merge_fan_in) * cursor_width
            + run_state_allowance(&candidate).unwrap()
            + path_state_allowance(&candidate).unwrap();
        for (budget, accepted) in [
            (merge_required - 1, false),
            (merge_required, true),
            (merge_required + 1, true),
        ] {
            candidate.limits.max_memory_bytes = budget;
            assert_eq!(
                validate_merge_phase_memory_for_work_dir(&candidate, &candidate.work_dir).is_ok(),
                accepted
            );
        }

        let final_required = final_materialization_memory_bytes(&candidate, 3, 0).unwrap();
        for (budget, accepted) in [
            (final_required - 1, false),
            (final_required, true),
            (final_required + 1, true),
        ] {
            candidate.limits.max_memory_bytes = budget;
            assert_eq!(
                validate_final_materialization_memory(&candidate, 3, 0).is_ok(),
                accepted
            );
        }

        let per_run_state = 4 * size_of::<WideRunMeta>() as u64
            + size_of::<RunReplacement>() as u64
            + size_of::<[u8; 32]>() as u64;
        let mut one_more_run = candidate.clone();
        one_more_run.limits.max_run_files += 1;
        assert_eq!(
            run_state_allowance(&one_more_run).unwrap() - run_state_allowance(&candidate).unwrap(),
            per_run_state
        );

        let mut long_path = temp.path().to_path_buf();
        for _ in 0..10 {
            long_path.push("x".repeat(100));
        }
        let mut long = candidate.clone();
        long.work_dir = long_path;
        assert!(path_state_allowance(&long).unwrap() > path_state_allowance(&candidate).unwrap());
        candidate.limits.max_memory_bytes = scan_required;
        long.limits.max_memory_bytes = scan_required;
        assert!(validate_scan_phase_memory(&candidate, additional_scan).is_ok());
        assert_eq!(
            validate_scan_phase_memory(&long, additional_scan)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn randomized_spill_layouts_match_the_source_window_oracle(
            symbols in prop::collection::vec(0_u8..4, 0..40),
            k_seed in 3_u8..13,
            minimizer_seed in 1_u8..13,
            buckets in 1_u32..6,
            buffer_records in 1_u64..6,
            fan_in in 2_u16..5,
        ) {
            let sequence: Vec<u8> = symbols
                .iter()
                .map(|symbol| b"ACGT"[usize::from(*symbol)])
                .collect();
            let k = k_seed.min(sequence.len().max(3) as u8);
            let minimizer_length = 1 + ((minimizer_seed - 1) % k);
            let segments = [AcceptedSegment {
                source_ordinal: 3,
                segment_ordinal: 7,
                bases: &sequence,
            }];
            let temp = tempfile::tempdir().unwrap();
            let mut options = options(temp.path(), &segments);
            options.k = k;
            options.minimizer_length = minimizer_length;
            options.virtual_bucket_count = buckets;
            options.limits.sort_buffer_bytes = buffer_records * size_of::<Observation>() as u64;
            options.limits.merge_fan_in = fan_in;
            options.limits.max_open_files = fan_in + 1;
            let result = build_external_partitioned_counts(&options, &segments).unwrap();
            let expected = oracle(&segments, k, minimizer_length, buckets);
            prop_assert_eq!(
                &result.edge_counts,
                &expected
            );
            prop_assert_eq!(
                result.support_events,
                result.edge_counts.iter().map(|row| row.support).sum::<u64>()
            );
            prop_assert!(result.open_files_high_water <= options.limits.max_open_files);
        }
    }
}
