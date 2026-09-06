//! Exact original-read transition witnesses for one experimental graph k.
//!
//! A row in this ledger is backed by complete `(k + 1)`-mer observations and
//! immutable spool coordinates.  Hashes authenticate evidence; they never
//! replace complete packed DNA identity or exact coordinate comparison.
//! This module is intentionally disconnected from the stable CLI.

use super::wide_kmer::{validate_code, PackedKmer, MAX_PACKED_K, PACKED_KEY_BYTES};
use crate::error::{ErrorCode, Result, VeritasmError};
use crate::model::{Fragment, MateRole, ReadRecord, WindowStats};
use crate::spool::{MemoryBoundedNext, Spool};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const SOURCE_DOMAIN: &[u8] = b"veritasm:multik-source:v1\0";
const LEDGER_DOMAIN: &[u8] = b"veritasm:transition-ledger:v1\0";
const EVENT_SET_DOMAIN: &[u8] = b"veritasm:transition-event-set:v1\0";
const RUN_DIGEST_DOMAIN: &[u8] = b"veritasm:transition-run:v1\0";
const RUN_MAGIC: &[u8; 8] = b"VTWITR01";
const RUN_END: &[u8; 8] = b"VTWEND01";
const RUN_SCHEMA: u16 = 1;
const RUN_HEADER_BYTES: usize = 128;
const RUN_TRAILER_BYTES: usize = 48;
const EVENT_BYTES: usize = 88;
const RUN_DIRECTORY_PREFIX: &str = "experimental-transition-runs-";
const RUN_DIRECTORY_NAME_BYTES: u64 = 96;
const HASH_BUFFER_BYTES: usize = 16 * 1024;
const SPOOL_REPLAY_BUFFER_BYTES: u64 = 64 * 1024;
const RUN_FILENAME_BYTES: u64 = 40;
const PATH_SEPARATOR_ALLOWANCE_BYTES: u64 = 4;

/// A typed, finite resource envelope for one exact transition build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionLedgerLimits {
    pub max_events: u64,
    pub max_rows: u64,
    pub max_fragment_decode_bytes: u64,
    pub max_fragment_windows: u64,
    pub max_memory_bytes: u64,
    pub sort_buffer_bytes: u64,
    pub max_temp_bytes: u64,
    pub max_run_files: u64,
    pub merge_fan_in: u16,
    pub max_open_files: u16,
}

/// Scientific and operational choices for a one-k ledger construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionLedgerOptions {
    pub work_dir: PathBuf,
    /// Graph k.  The observed transition length is `k + 1`.
    pub k: u8,
    pub limits: TransitionLedgerLimits,
}

/// Path-free, fixed-width source/QC preimage shared by every child k.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionSourceDescriptor {
    pub spool_sha256: [u8; 32],
    pub spool_pretrailer_sha256: [u8; 32],
    pub spool_schema: u16,
    pub fragment_count: u64,
    pub read_count: u64,
    pub input_mode: u8,
    pub min_base_quality: u8,
}

/// Construct the common path-free source descriptor from registered spool
/// digests and authenticated metadata.
pub fn transition_source_descriptor(spool: &Spool) -> Result<TransitionSourceDescriptor> {
    Ok(TransitionSourceDescriptor {
        spool_sha256: decode_hex_32(&spool.sha256)?,
        spool_pretrailer_sha256: decode_hex_32(&spool.pretrailer_sha256)?,
        spool_schema: spool.schema_version(),
        fragment_count: spool.fragment_count,
        read_count: spool.read_count,
        input_mode: spool.input_mode_tag(),
        min_base_quality: spool.scientific_config().min_base_quality,
    })
}

/// Pure source-root constructor for multi-k orchestration and child binding.
pub fn transition_source_root_from_descriptor(descriptor: TransitionSourceDescriptor) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SOURCE_DOMAIN);
    digest.update(descriptor.spool_sha256);
    digest.update(descriptor.spool_pretrailer_sha256);
    digest.update(descriptor.spool_schema.to_le_bytes());
    digest.update(descriptor.fragment_count.to_le_bytes());
    digest.update(descriptor.read_count.to_le_bytes());
    digest.update([descriptor.input_mode]);
    digest.update([descriptor.min_base_quality]);
    digest.finalize().into()
}

/// Compute the shared source root directly from one registered spool.
pub fn transition_source_root(spool: &Spool) -> Result<[u8; 32]> {
    Ok(transition_source_root_from_descriptor(
        transition_source_descriptor(spool)?,
    ))
}

/// Orientation of the observed read spelling relative to the canonical q-mer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ObservedOrientation {
    Canonical = 0,
    ReverseComplement = 1,
    FixedPoint = 2,
}

impl ObservedOrientation {
    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            0 => Ok(Self::Canonical),
            1 => Ok(Self::ReverseComplement),
            2 => Ok(Self::FixedPoint),
            _ => integrity(format!("unknown transition orientation tag {tag}")),
        }
    }
}

/// One exact reduced physical transition and its two explicitly named units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionRow {
    pub canonical_qmer: PackedKmer,
    pub accepted_window_occurrences: u64,
    pub distinct_supplied_fragment_instances: u64,
    pub sorted_event_frames_sha256: [u8; 32],
}

/// Pathless numeric identity of a private run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TransitionRunId {
    pub generation: u32,
    pub run_ordinal: u64,
}

/// Authenticated summary of a final run whose private file was reclaimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionRunSummary {
    pub id: TransitionRunId,
    pub record_count: u64,
    pub byte_len: u64,
    pub sha256: [u8; 32],
}

/// A verified register-before-delete external-run replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionRunReplacement {
    pub output: TransitionRunSummary,
    pub predecessor_sha256: Vec<[u8; 32]>,
    pub reclaimed_bytes: u64,
    pub predecessors_reclaimed: bool,
}

/// Opaque source-backed exact rows and evidence about their bounded external build.
///
/// All integrity-bearing fields are private.  The only public constructor is
/// [`build_transition_ledger`], which consumes an authenticated [`Spool`].
/// Copying reported rows therefore cannot manufacture a ledger that is
/// accepted by a source-backed reconstruction API.
///
/// ```compile_fail
/// use veritasm::experimental::transition_witness::{TransitionLedger, TransitionRow};
/// fn replace_rows(ledger: &mut TransitionLedger, rows: Vec<TransitionRow>) {
///     ledger.rows = rows;
/// }
/// ```
///
/// ```compile_fail
/// use veritasm::experimental::transition_witness::TransitionLedger;
/// fn duplicate(ledger: &TransitionLedger) -> TransitionLedger {
///     ledger.clone()
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Clone))]
pub struct TransitionLedger {
    k: u8,
    q: u8,
    source_root: [u8; 32],
    transition_root: [u8; 32],
    window_stats: WindowStats,
    event_count: u64,
    rows: Vec<TransitionRow>,
    final_runs: Vec<TransitionRunSummary>,
    replacements: Vec<TransitionRunReplacement>,
    run_files_created: u64,
    open_files_high_water: u16,
    temporary_bytes_high_water: u64,
    temporary_bytes_final: u64,
    retained_result_bytes: u64,
}

/// Borrowed access to a structurally validated immutable ledger.
#[derive(Debug, Clone, Copy)]
pub struct TransitionLedgerView<'a> {
    k: u8,
    q: u8,
    source_root: [u8; 32],
    transition_root: [u8; 32],
    rows: &'a [TransitionRow],
}

impl TransitionLedger {
    pub const fn k(&self) -> u8 {
        self.k
    }

    pub const fn q(&self) -> u8 {
        self.q
    }

    pub const fn source_root(&self) -> [u8; 32] {
        self.source_root
    }

    pub const fn transition_root(&self) -> [u8; 32] {
        self.transition_root
    }

    pub const fn window_stats(&self) -> &WindowStats {
        &self.window_stats
    }

    pub const fn event_count(&self) -> u64 {
        self.event_count
    }

    pub fn rows(&self) -> &[TransitionRow] {
        &self.rows
    }

    pub fn final_runs(&self) -> &[TransitionRunSummary] {
        &self.final_runs
    }

    pub fn replacements(&self) -> &[TransitionRunReplacement] {
        &self.replacements
    }

    pub const fn run_files_created(&self) -> u64 {
        self.run_files_created
    }

    pub const fn open_files_high_water(&self) -> u16 {
        self.open_files_high_water
    }

    pub const fn temporary_bytes_high_water(&self) -> u64 {
        self.temporary_bytes_high_water
    }

    pub const fn temporary_bytes_final(&self) -> u64 {
        self.temporary_bytes_final
    }

    pub const fn retained_result_bytes(&self) -> u64 {
        self.retained_result_bytes
    }

    /// Validate all structural invariants before exposing rows to graph code.
    pub fn view(&self) -> Result<TransitionLedgerView<'_>> {
        validate_ledger_structure(self)?;
        Ok(TransitionLedgerView {
            k: self.k,
            q: self.q,
            source_root: self.source_root,
            transition_root: self.transition_root,
            rows: &self.rows,
        })
    }
}

impl TransitionLedgerView<'_> {
    pub const fn k(&self) -> u8 {
        self.k
    }

    pub const fn q(&self) -> u8 {
        self.q
    }

    pub const fn source_root(&self) -> [u8; 32] {
        self.source_root
    }

    pub const fn transition_root(&self) -> [u8; 32] {
        self.transition_root
    }

    pub const fn rows(&self) -> &[TransitionRow] {
        self.rows
    }

    /// Exact complete-key lookup; no digest or probabilistic membership can
    /// produce a positive result.
    pub fn find(&self, canonical_qmer: PackedKmer) -> Option<&TransitionRow> {
        self.rows
            .binary_search_by_key(&canonical_qmer, |row| row.canonical_qmer)
            .ok()
            .map(|index| &self.rows[index])
    }
}

/// Build one exact transition ledger from an authenticated immutable spool.
///
/// Multi-k orchestration intentionally calls this constructor once per sorted,
/// unique k.  That repeats authenticated replay but keeps each run domain and
/// its resource proof independent; it does not manufacture cross-k support.
pub fn build_transition_ledger(
    spool: &Spool,
    options: &TransitionLedgerOptions,
) -> Result<TransitionLedger> {
    let admitted = validate_options(options)?;
    let source_root = transition_source_root(spool)?;
    let guard = RunDirectoryGuard::create(&options.work_dir, options.limits.max_run_files)?;
    match build_inner(spool, options, admitted, source_root, guard.path()) {
        Ok(result) => guard.finish(result),
        Err(error) => guard.fail(error),
    }
}

/// Rebuild from the authenticated spool and compare every semantic and
/// operational field.  This is intentionally expensive and intended for
/// evidence-bundle validation, not routine lookup.
pub fn validate_transition_ledger_replay(
    spool: &Spool,
    options: &TransitionLedgerOptions,
    observed: &TransitionLedger,
) -> Result<()> {
    validate_ledger_structure(observed)?;
    let rebuilt = build_transition_ledger(spool, options)?;
    if &rebuilt != observed {
        return integrity("transition ledger differs from authenticated source replay");
    }
    Ok(())
}

/// Validate sorted rows, checked totals, q-mer widths, and the transition root
/// without consulting a source.  Source-backed claims additionally require
/// [`validate_transition_ledger_replay`].
pub fn validate_ledger_structure(ledger: &TransitionLedger) -> Result<()> {
    validate_graph_k(ledger.k)?;
    if ledger.q != ledger.k + 1 {
        return integrity("transition ledger q is not k + 1");
    }
    let mut previous = None;
    let mut events = 0_u64;
    for row in &ledger.rows {
        validate_code(row.canonical_qmer, ledger.q)?;
        if previous.is_some_and(|key| key >= row.canonical_qmer) {
            return integrity("transition rows are not strictly ordered by complete q-mer");
        }
        if row.accepted_window_occurrences == 0
            || row.distinct_supplied_fragment_instances == 0
            || row.distinct_supplied_fragment_instances > row.accepted_window_occurrences
        {
            return integrity("transition row has impossible support values");
        }
        events = checked_add(
            events,
            row.accepted_window_occurrences,
            "transition event sum",
        )?;
        previous = Some(row.canonical_qmer);
    }
    if events != ledger.event_count || events != ledger.window_stats.accepted {
        return integrity("transition event total disagrees with row or QC accounting");
    }
    validate_window_partition(&ledger.window_stats)?;
    let root = transition_root(ledger.source_root, ledger.k, &ledger.rows)?;
    if root != ledger.transition_root {
        return integrity("transition root does not authenticate the complete sorted rows");
    }
    if ledger.temporary_bytes_final != 0 {
        return integrity("successful transition ledger retained private temporary bytes");
    }
    Ok(())
}

/// Return the literal prefix and suffix k-mer spellings of a canonical q-mer.
/// These are oriented spellings, not canonical endpoint identities.
pub fn transition_endpoints(canonical_qmer: PackedKmer, k: u8) -> Result<(PackedKmer, PackedKmer)> {
    validate_graph_k(k)?;
    let q = k + 1;
    validate_code(canonical_qmer, q)?;
    let prefix = shift_right_two(canonical_qmer);
    let suffix = mask_to_length(canonical_qmer, k);
    Ok((prefix, suffix))
}

/// Validate the canonical parent orchestration order.  A multi-k parent calls
/// the one-k constructor in this order and retains every independent root.
pub fn validate_transition_k_values(ks: &[u8]) -> Result<()> {
    if ks.is_empty() {
        return config("transition k set must not be empty");
    }
    let mut previous = None;
    for &k in ks {
        validate_graph_k(k)?;
        if previous.is_some_and(|value| value >= k) {
            return config("transition k values must be sorted and unique");
        }
        previous = Some(k);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct AdmittedShape {
    chunk_records: usize,
    max_runs: usize,
    max_rows: usize,
}

fn validate_options(options: &TransitionLedgerOptions) -> Result<AdmittedShape> {
    validate_graph_k(options.k)?;
    let limits = options.limits;
    if limits.max_events == 0
        || limits.max_rows == 0
        || limits.max_fragment_decode_bytes == 0
        || limits.max_fragment_windows == 0
        || limits.sort_buffer_bytes == 0
        || limits.max_temp_bytes == 0
        || limits.max_run_files == 0
        || limits.max_memory_bytes == 0
    {
        return config("transition ledger limits must all be nonzero");
    }
    if limits.merge_fan_in < 2 {
        return config("transition merge fan-in must be at least two");
    }
    let required_open = limits
        .merge_fan_in
        .checked_add(1)
        .ok_or_else(|| overflow("transition open-file requirement overflow"))?
        .max(2);
    if limits.max_open_files < required_open {
        return config(format!(
            "transition build requires at least {required_open} open files"
        ));
    }
    let event_width = u64::try_from(size_of::<Event>())
        .map_err(|_| overflow("transition event width does not fit u64"))?;
    let chunk_records_u64 = limits.sort_buffer_bytes / event_width;
    if chunk_records_u64 == 0 {
        return config(format!(
            "transition sort buffer must hold at least one {}-byte in-memory event",
            event_width
        ));
    }
    let chunk_records = usize::try_from(chunk_records_u64)
        .map_err(|_| config_error("transition sort-buffer record count does not fit usize"))?;
    let max_runs = usize::try_from(limits.max_run_files)
        .map_err(|_| config_error("transition run-file limit does not fit usize"))?;
    let max_rows = usize::try_from(limits.max_rows)
        .map_err(|_| config_error("transition row limit does not fit usize"))?;

    let event_memory = checked_mul(chunk_records_u64, event_width, "event sort memory")?;
    let run_catalog_memory = checked_mul(
        checked_mul(
            limits.max_run_files,
            size_of::<RunMeta>() as u64,
            "run catalog memory",
        )?,
        2,
        "simultaneous run catalogs",
    )?;
    let replacement_headers = checked_mul(
        limits.max_run_files,
        size_of::<TransitionRunReplacement>() as u64,
        "replacement catalog memory",
    )?;
    let predecessor_digests = checked_mul(
        checked_mul(
            limits.max_run_files,
            u64::from(limits.merge_fan_in),
            "predecessor count",
        )?,
        32,
        "predecessor digest memory",
    )?;
    let row_memory = checked_mul(
        limits.max_rows,
        size_of::<TransitionRow>() as u64,
        "transition row memory",
    )?;
    let merge_memory = checked_mul(
        u64::from(limits.merge_fan_in),
        (size_of::<RunReader>() + size_of::<HeapItem>()) as u64,
        "merge reader/heap memory",
    )?;
    let path_memory = path_state_allowance(&options.work_dir)?;
    let required_memory = checked_sum(&[
        limits.max_fragment_decode_bytes,
        event_memory,
        run_catalog_memory,
        replacement_headers,
        predecessor_digests,
        row_memory,
        merge_memory,
        path_memory,
        HASH_BUFFER_BYTES as u64,
        SPOOL_REPLAY_BUFFER_BYTES,
        size_of::<TransitionLedger>() as u64,
        size_of::<TransitionRunSummary>() as u64,
        size_of::<Option<Event>>() as u64,
        (size_of::<Sha256>() * 4) as u64,
    ])?;
    if required_memory > limits.max_memory_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "transition resource envelope requires {required_memory} bytes, exceeding limit {}",
                limits.max_memory_bytes
            ),
        ));
    }
    Ok(AdmittedShape {
        chunk_records,
        max_runs,
        max_rows,
    })
}

fn validate_graph_k(k: u8) -> Result<()> {
    if (3..=126).contains(&k) {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!("transition graph k must be in 3..=126; received {k}"),
        ))
    }
}

fn build_inner(
    spool: &Spool,
    options: &TransitionLedgerOptions,
    admitted: AdmittedShape,
    source_root: [u8; 32],
    run_dir: &Path,
) -> Result<TransitionLedger> {
    let mut state = BuildState::new(options, admitted, source_root, run_dir)?;
    let mut events = fallible_vec(admitted.chunk_records, "transition sort buffer")?;
    let mut iterator = spool.iter()?;
    let mut stats = WindowStats::default();
    let mut event_count = 0_u64;
    let mut chunk_start = 0_u64;

    loop {
        let fragment = match iterator
            .next_with_decode_memory_limit(options.limits.max_fragment_decode_bytes)?
        {
            MemoryBoundedNext::Fragment { fragment, .. } => fragment,
            MemoryBoundedNext::End => break,
            MemoryBoundedNext::RequiresMemory(required) => {
                return Err(VeritasmError::new(
                    ErrorCode::ResourceMemory,
                    format!(
                        "transition replay needs {required} decoded-fragment bytes, exceeding limit {}",
                        options.limits.max_fragment_decode_bytes
                    ),
                ));
            }
        };
        scan_fragment(
            &fragment,
            spool.scientific_config().min_base_quality,
            options.k + 1,
            options.limits.max_fragment_windows,
            &mut stats,
            |event| {
                event_count = checked_add(event_count, 1, "transition event count")?;
                if event_count > options.limits.max_events {
                    return Err(VeritasmError::new(
                        ErrorCode::ResourceRetainedKeys,
                        format!(
                            "transition events exceed limit {}",
                            options.limits.max_events
                        ),
                    ));
                }
                events.push(event);
                if events.len() == admitted.chunk_records {
                    state.seal_initial(&mut events, chunk_start, event_count)?;
                    chunk_start = event_count;
                }
                Ok(())
            },
        )?;
    }
    if !events.is_empty() {
        state.seal_initial(&mut events, chunk_start, event_count)?;
    }
    if stats.accepted != event_count {
        return invariant("accepted-window and transition-event counts diverged");
    }
    validate_window_partition(&stats)?;
    if !iterator.is_authenticated() {
        return integrity("transition replay reached end without terminal spool authentication");
    }
    // The external merge needs one writer plus `merge_fan_in` readers.  End
    // replay authentication has already consumed the registered trailer and
    // exact EOF, so release the spool descriptor before opening that set.
    drop(iterator);

    let final_run = state.merge_all()?;
    let mut rows = fallible_vec(admitted.max_rows, "transition result rows")?;
    let mut final_runs = fallible_vec(1, "transition final-run summaries")?;
    if let Some(meta) = final_run {
        reduce_final_run(&state, meta, &mut rows)?;
        final_runs.push(meta.summary());
        state.delete_run(meta)?;
    }
    let transition_root = transition_root(source_root, options.k, &rows)?;
    let retained_result_bytes = checked_sum(&[
        size_of::<TransitionLedger>() as u64,
        (rows.capacity() as u64)
            .checked_mul(size_of::<TransitionRow>() as u64)
            .ok_or_else(|| overflow("retained transition-row bytes"))?,
        (final_runs.capacity() as u64)
            .checked_mul(size_of::<TransitionRunSummary>() as u64)
            .ok_or_else(|| overflow("retained final-run summary bytes"))?,
        (state.replacements.capacity() as u64)
            .checked_mul(size_of::<TransitionRunReplacement>() as u64)
            .ok_or_else(|| overflow("retained replacement header bytes"))?,
        state.retained_predecessor_digest_bytes()?,
    ])?;
    let result = TransitionLedger {
        k: options.k,
        q: options.k + 1,
        source_root,
        transition_root,
        window_stats: stats,
        event_count,
        rows,
        final_runs,
        replacements: state.replacements,
        run_files_created: state.run_files_created,
        open_files_high_water: state.open_files_high_water.max(2),
        temporary_bytes_high_water: state.temp.high_water,
        temporary_bytes_final: state.temp.live,
        retained_result_bytes,
    };
    validate_ledger_structure(&result)?;
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Event {
    canonical_qmer: PackedKmer,
    lane_ordinal: u32,
    fragment_ordinal: u64,
    mate_role: MateRole,
    normalized_id_digest: [u8; 32],
    read_offset: u64,
    orientation: ObservedOrientation,
}

impl Event {
    fn coordinate_cmp(&self, other: &Self) -> Ordering {
        self.lane_ordinal
            .cmp(&other.lane_ordinal)
            .then_with(|| self.fragment_ordinal.cmp(&other.fragment_ordinal))
            .then_with(|| self.mate_role.cmp(&other.mate_role))
            .then_with(|| self.normalized_id_digest.cmp(&other.normalized_id_digest))
            .then_with(|| self.read_offset.cmp(&other.read_offset))
    }

    fn same_coordinate(&self, other: &Self) -> bool {
        self.coordinate_cmp(other) == Ordering::Equal
    }

    fn same_fragment(&self, other: &Self) -> bool {
        self.lane_ordinal == other.lane_ordinal && self.fragment_ordinal == other.fragment_ordinal
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        self.canonical_qmer
            .cmp(&other.canonical_qmer)
            .then_with(|| self.coordinate_cmp(other))
            .then_with(|| self.orientation.cmp(&other.orientation))
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn scan_fragment<F>(
    fragment: &Fragment,
    minimum_quality: u8,
    q: u8,
    max_fragment_windows: u64,
    stats: &mut WindowStats,
    mut emit: F,
) -> Result<()>
where
    F: FnMut(Event) -> Result<()>,
{
    let mut possible = 0_u64;
    for read in &fragment.reads {
        let count = read
            .sequence
            .len()
            .saturating_sub(usize::from(q).saturating_sub(1));
        possible = checked_add(
            possible,
            u64::try_from(count).map_err(|_| overflow("fragment windows do not fit u64"))?,
            "fragment possible-window count",
        )?;
    }
    if possible > max_fragment_windows {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            format!(
                "fragment {} has {possible} possible transition windows, exceeding limit {max_fragment_windows}",
                fragment.ordinal
            ),
        ));
    }
    let mut role_mask = 0_u8;
    for read in &fragment.reads {
        let bit = 1_u8
            .checked_shl(u32::from(read.role.tag()))
            .ok_or_else(|| invariant_error("mate-role bit overflow"))?;
        if role_mask & bit != 0 {
            return integrity("one fragment contains a duplicate mate role");
        }
        role_mask |= bit;
        scan_read(
            read,
            fragment.lane_ordinal,
            fragment.ordinal,
            minimum_quality,
            q,
            stats,
            &mut emit,
        )?;
    }
    Ok(())
}

fn scan_read<F>(
    read: &ReadRecord,
    lane_ordinal: u32,
    fragment_ordinal: u64,
    minimum_quality: u8,
    q: u8,
    stats: &mut WindowStats,
    emit: &mut F,
) -> Result<()>
where
    F: FnMut(Event) -> Result<()>,
{
    if minimum_quality > 93 {
        return config("minimum base quality exceeds Phred+33 Q93");
    }
    if read
        .quality
        .as_ref()
        .is_some_and(|quality| quality.len() != read.sequence.len())
    {
        return integrity("spool read sequence and quality lengths differ");
    }
    let q_usize = usize::from(q);
    let mask = active_mask_words(q);
    let reverse_offset = 2 * u16::from(q - 1);
    let mut ambiguity_ring = [false; MAX_PACKED_K as usize];
    let mut quality_ring = [false; MAX_PACKED_K as usize];
    let mut ambiguity_count = 0_usize;
    let mut low_quality_count = 0_usize;
    let mut exact_run = 0_usize;
    let mut forward = [0_u64; 4];
    let mut reverse = [0_u64; 4];

    for (position, &base) in read.sequence.iter().enumerate() {
        let slot = position % q_usize;
        if position >= q_usize {
            ambiguity_count = ambiguity_count
                .checked_sub(usize::from(ambiguity_ring[slot]))
                .ok_or_else(|| invariant_error("ambiguity ring underflow"))?;
            low_quality_count = low_quality_count
                .checked_sub(usize::from(quality_ring[slot]))
                .ok_or_else(|| invariant_error("quality ring underflow"))?;
        }
        match classify_base(base, position)? {
            BaseClass::Exact(bits) => {
                forward = shift_left_two_words(forward, bits);
                mask_words(&mut forward, mask);
                reverse = shift_right_two_words(reverse);
                set_pair_words(&mut reverse, 3 - bits, reverse_offset)?;
                exact_run = exact_run.saturating_add(1).min(q_usize);
                ambiguity_ring[slot] = false;
            }
            BaseClass::Ambiguous => {
                forward = [0; 4];
                reverse = [0; 4];
                exact_run = 0;
                ambiguity_ring[slot] = true;
                ambiguity_count = ambiguity_count
                    .checked_add(1)
                    .ok_or_else(|| overflow("ambiguity-window count"))?;
            }
        }
        let low_quality = match read.quality.as_deref() {
            Some(quality) => {
                let value = quality[position];
                if !(33..=126).contains(&value) {
                    return integrity(format!(
                        "spool quality byte 0x{value:02x} at offset {position} is invalid"
                    ));
                }
                value - 33 < minimum_quality
            }
            None => false,
        };
        quality_ring[slot] = low_quality;
        low_quality_count = low_quality_count
            .checked_add(usize::from(low_quality))
            .ok_or_else(|| overflow("low-quality-window count"))?;

        if position + 1 < q_usize {
            continue;
        }
        increment(&mut stats.possible, "possible transition windows")?;
        match (ambiguity_count > 0, low_quality_count > 0) {
            (false, false) => {
                if exact_run != q_usize {
                    return invariant("accepted transition window lacks a complete rolling code");
                }
                increment(&mut stats.accepted, "accepted transition windows")?;
                let observed = PackedKmer::from_words(forward);
                let reverse_complement = PackedKmer::from_words(reverse);
                let (canonical_qmer, orientation) = match observed.cmp(&reverse_complement) {
                    Ordering::Less => (observed, ObservedOrientation::Canonical),
                    Ordering::Greater => {
                        (reverse_complement, ObservedOrientation::ReverseComplement)
                    }
                    Ordering::Equal => (observed, ObservedOrientation::FixedPoint),
                };
                let start = position + 1 - q_usize;
                emit(Event {
                    canonical_qmer,
                    lane_ordinal,
                    fragment_ordinal,
                    mate_role: read.role,
                    normalized_id_digest: read.normalized_id_digest,
                    read_offset: u64::try_from(start)
                        .map_err(|_| overflow("read offset does not fit u64"))?,
                    orientation,
                })?;
            }
            (true, false) => increment(
                &mut stats.ambiguity_only,
                "ambiguity-only transition windows",
            )?,
            (false, true) => increment(&mut stats.quality_only, "quality-only transition windows")?,
            (true, true) => increment(
                &mut stats.ambiguity_and_quality,
                "ambiguity-and-quality transition windows",
            )?,
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum BaseClass {
    Exact(u8),
    Ambiguous,
}

fn classify_base(base: u8, position: usize) -> Result<BaseClass> {
    match base.to_ascii_uppercase() {
        b'A' => Ok(BaseClass::Exact(0)),
        b'C' => Ok(BaseClass::Exact(1)),
        b'G' => Ok(BaseClass::Exact(2)),
        b'T' => Ok(BaseClass::Exact(3)),
        b'R' | b'Y' | b'S' | b'W' | b'K' | b'M' | b'B' | b'D' | b'H' | b'V' | b'N' => {
            Ok(BaseClass::Ambiguous)
        }
        _ => Err(VeritasmError::new(
            ErrorCode::InputNucleotide,
            format!("invalid nucleotide byte 0x{base:02x} at zero-based position {position}"),
        )),
    }
}

fn active_mask_words(length: u8) -> [u64; 4] {
    let bits = usize::from(length) * 2;
    let full = bits / 64;
    let partial = bits % 64;
    let mut mask = [0_u64; 4];
    for index in 0..full {
        mask[3 - index] = u64::MAX;
    }
    if partial != 0 {
        mask[3 - full] = (1_u64 << partial) - 1;
    }
    mask
}

fn shift_left_two_words(words: [u64; 4], pair: u8) -> [u64; 4] {
    [
        (words[0] << 2) | (words[1] >> 62),
        (words[1] << 2) | (words[2] >> 62),
        (words[2] << 2) | (words[3] >> 62),
        (words[3] << 2) | u64::from(pair),
    ]
}

fn shift_right_two_words(words: [u64; 4]) -> [u64; 4] {
    [
        words[0] >> 2,
        (words[1] >> 2) | (words[0] << 62),
        (words[2] >> 2) | (words[1] << 62),
        (words[3] >> 2) | (words[2] << 62),
    ]
}

fn shift_right_two(code: PackedKmer) -> PackedKmer {
    PackedKmer::from_words(shift_right_two_words(code.words()))
}

fn mask_words(words: &mut [u64; 4], mask: [u64; 4]) {
    for index in 0..4 {
        words[index] &= mask[index];
    }
}

fn mask_to_length(code: PackedKmer, length: u8) -> PackedKmer {
    let mut words = code.words();
    mask_words(&mut words, active_mask_words(length));
    PackedKmer::from_words(words)
}

fn set_pair_words(words: &mut [u64; 4], pair: u8, bit_offset: u16) -> Result<()> {
    let word_from_low = usize::from(bit_offset / 64);
    let within = u32::from(bit_offset % 64);
    if word_from_low >= 4 || within > 62 || pair > 3 {
        return invariant("transition reverse-complement bit placement is invalid");
    }
    words[3 - word_from_low] |= u64::from(pair) << within;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunHeader {
    source_root: [u8; 32],
    k: u8,
    generation: u32,
    run_ordinal: u64,
    first_event_ordinal: u64,
    event_ordinal_end: u64,
    record_count: u64,
    payload_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunMeta {
    id: TransitionRunId,
    header: RunHeader,
    byte_len: u64,
    sha256: [u8; 32],
}

impl RunMeta {
    fn summary(self) -> TransitionRunSummary {
        TransitionRunSummary {
            id: self.id,
            record_count: self.header.record_count,
            byte_len: self.byte_len,
            sha256: self.sha256,
        }
    }
}

#[derive(Debug)]
struct TempBytes {
    live: u64,
    high_water: u64,
    limit: u64,
}

impl TempBytes {
    fn reserve(&mut self, bytes: u64) -> Result<()> {
        let next = checked_add(self.live, bytes, "transition temporary-byte reservation")?;
        if next > self.limit {
            return Err(VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!(
                    "transition runs would use {next} bytes, exceeding limit {}",
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
            .ok_or_else(|| overflow("transition temporary-byte accounting underflow"))?;
        Ok(())
    }
}

struct BuildState<'a> {
    options: &'a TransitionLedgerOptions,
    source_root: [u8; 32],
    run_dir: &'a Path,
    runs: Vec<RunMeta>,
    replacements: Vec<TransitionRunReplacement>,
    next_run_ordinal: u64,
    run_files_created: u64,
    open_files_high_water: u16,
    temp: TempBytes,
    last_source_coordinate: Option<Event>,
}

impl<'a> BuildState<'a> {
    fn new(
        options: &'a TransitionLedgerOptions,
        admitted: AdmittedShape,
        source_root: [u8; 32],
        run_dir: &'a Path,
    ) -> Result<Self> {
        Ok(Self {
            options,
            source_root,
            run_dir,
            runs: fallible_vec(admitted.max_runs, "transition run catalog")?,
            replacements: fallible_vec(admitted.max_runs, "transition replacement catalog")?,
            next_run_ordinal: 0,
            run_files_created: 0,
            open_files_high_water: 0,
            temp: TempBytes {
                live: 0,
                high_water: 0,
                limit: options.limits.max_temp_bytes,
            },
            last_source_coordinate: None,
        })
    }

    fn seal_initial(
        &mut self,
        events: &mut Vec<Event>,
        first_event_ordinal: u64,
        event_ordinal_end: u64,
    ) -> Result<()> {
        if events.is_empty() {
            return invariant("cannot seal an empty transition run");
        }
        let count = u64::try_from(events.len())
            .map_err(|_| overflow("transition run record count does not fit u64"))?;
        if checked_add(first_event_ordinal, count, "initial run event interval")?
            != event_ordinal_end
        {
            return invariant("initial transition run interval is not contiguous");
        }
        // Spool replay emits lane/fragment/role/offset coordinates in one
        // strict source order. Validate that stream before q-mer sorting. This
        // is an exact O(1)-state uniqueness plane: every accepted event passes
        // here once, including across spill boundaries. Authenticated runs and
        // exact replay preserve that checked event multiset thereafter.
        validate_source_coordinate_stream(&mut self.last_source_coordinate, events)?;
        events.sort_unstable();
        validate_sorted_events(events)?;
        let header = self.next_header(0, first_event_ordinal, event_ordinal_end, count)?;
        let meta = self.write_verified_run(header, events.iter().copied())?;
        self.runs.push(meta);
        events.clear();
        Ok(())
    }

    fn next_header(
        &mut self,
        generation: u32,
        first_event_ordinal: u64,
        event_ordinal_end: u64,
        record_count: u64,
    ) -> Result<RunHeader> {
        if self.run_files_created >= self.options.limits.max_run_files {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRunCount,
                format!(
                    "transition run files exceed limit {}",
                    self.options.limits.max_run_files
                ),
            ));
        }
        let ordinal = self.next_run_ordinal;
        self.next_run_ordinal = checked_add(ordinal, 1, "transition run ordinal")?;
        self.run_files_created =
            checked_add(self.run_files_created, 1, "transition run-file count")?;
        Ok(RunHeader {
            source_root: self.source_root,
            k: self.options.k,
            generation,
            run_ordinal: ordinal,
            first_event_ordinal,
            event_ordinal_end,
            record_count,
            payload_bytes: checked_mul(count_u64(record_count), EVENT_BYTES as u64, "run payload")?,
        })
    }

    fn write_verified_run<I>(&mut self, header: RunHeader, events: I) -> Result<RunMeta>
    where
        I: IntoIterator<Item = Event>,
    {
        let projected = projected_run_bytes(header.record_count)?;
        self.temp.reserve(projected)?;
        let id = TransitionRunId {
            generation: header.generation,
            run_ordinal: header.run_ordinal,
        };
        let path = run_path(self.run_dir, id)?;
        let mut writer = RunWriter::create(&path, header)?;
        for event in events {
            writer.push(event)?;
        }
        let meta = writer.finish(&path)?;
        verify_complete_run(&path, meta)?;
        Ok(meta)
    }

    fn merge_all(&mut self) -> Result<Option<RunMeta>> {
        let mut generation = 1_u32;
        while self.runs.len() > 1 {
            let fan_in = usize::from(self.options.limits.merge_fan_in);
            let old_runs = std::mem::replace(
                &mut self.runs,
                fallible_vec(
                    usize::try_from(self.options.limits.max_run_files)
                        .map_err(|_| config_error("run limit does not fit usize"))?,
                    "next transition run catalog",
                )?,
            );
            let mut cursor = 0_usize;
            while cursor < old_runs.len() {
                let end = cursor.saturating_add(fan_in).min(old_runs.len());
                let group = &old_runs[cursor..end];
                if group.len() == 1 {
                    self.runs.push(group[0]);
                } else {
                    let output = self.merge_group(group, generation)?;
                    self.runs.push(output);
                }
                cursor = end;
            }
            generation = generation
                .checked_add(1)
                .ok_or_else(|| overflow("transition merge generation"))?;
        }
        Ok(self.runs.pop())
    }

    fn merge_group(&mut self, inputs: &[RunMeta], generation: u32) -> Result<RunMeta> {
        let first = inputs
            .first()
            .ok_or_else(|| invariant_error("empty transition merge group"))?;
        let last = inputs
            .last()
            .ok_or_else(|| invariant_error("empty transition merge group"))?;
        let mut expected_first = first.header.first_event_ordinal;
        let mut record_count = 0_u64;
        for input in inputs {
            if input.header.source_root != self.source_root
                || input.header.k != self.options.k
                || input.header.first_event_ordinal != expected_first
            {
                return integrity("transition merge inputs have incompatible domains or intervals");
            }
            expected_first = input.header.event_ordinal_end;
            record_count = checked_add(
                record_count,
                input.header.record_count,
                "merged transition record count",
            )?;
        }
        let header = self.next_header(
            generation,
            first.header.first_event_ordinal,
            last.header.event_ordinal_end,
            record_count,
        )?;
        let output_bytes = projected_run_bytes(record_count)?;
        self.temp.reserve(output_bytes)?;
        let output_id = TransitionRunId {
            generation,
            run_ordinal: header.run_ordinal,
        };
        let output_path = run_path(self.run_dir, output_id)?;
        let mut writer = RunWriter::create(&output_path, header)?;
        let mut readers = fallible_vec(inputs.len(), "transition merge readers")?;
        let mut heap = BinaryHeap::new();
        heap.try_reserve_exact(inputs.len()).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve transition merge heap: {cause}"),
            )
        })?;
        enforce_capacity(heap.capacity(), inputs.len(), "transition merge heap")?;
        for input in inputs {
            let path = run_path(self.run_dir, input.id)?;
            readers.push(RunReader::open(&path, *input)?);
        }
        self.open_files_high_water = self.open_files_high_water.max(
            u16::try_from(inputs.len() + 1)
                .map_err(|_| overflow("transition open-file high-water"))?,
        );
        for (index, reader) in readers.iter_mut().enumerate() {
            if let Some(event) = reader.next_event()? {
                heap.push(HeapItem {
                    event,
                    reader: index,
                });
            }
        }
        while let Some(item) = heap.pop() {
            writer.push(item.event)?;
            if let Some(event) = readers[item.reader].next_event()? {
                heap.push(HeapItem {
                    event,
                    reader: item.reader,
                });
            }
        }
        if readers.iter().any(|reader| !reader.authenticated) {
            return integrity("transition merge consumed an unauthenticated predecessor");
        }
        drop(readers);
        let output = writer.finish(&output_path)?;
        verify_complete_run(&output_path, output)?;

        let mut predecessors = fallible_vec(inputs.len(), "predecessor digest list")?;
        let mut reclaimed = 0_u64;
        for input in inputs {
            predecessors.push(input.sha256);
            reclaimed = checked_add(reclaimed, input.byte_len, "reclaimed transition bytes")?;
        }
        // The output is sealed, independently verified, and registered in the
        // local variable above before any predecessor is unlinked.
        for input in inputs {
            self.delete_run(*input)?;
        }
        self.replacements.push(TransitionRunReplacement {
            output: output.summary(),
            predecessor_sha256: predecessors,
            reclaimed_bytes: reclaimed,
            predecessors_reclaimed: true,
        });
        Ok(output)
    }

    fn delete_run(&mut self, meta: RunMeta) -> Result<()> {
        let path = run_path(self.run_dir, meta.id)?;
        fs::remove_file(&path).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!("cannot reclaim verified transition run: {cause}"),
            )
        })?;
        self.temp.release(meta.byte_len)
    }

    fn retained_predecessor_digest_bytes(&self) -> Result<u64> {
        let count = self.replacements.iter().try_fold(0_u64, |total, row| {
            checked_add(
                total,
                u64::try_from(row.predecessor_sha256.capacity())
                    .map_err(|_| overflow("predecessor digest capacity"))?,
                "predecessor digest capacity sum",
            )
        })?;
        checked_mul(count, 32, "retained predecessor digest bytes")
    }
}

fn count_u64(value: u64) -> u64 {
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeapItem {
    event: Event,
    reader: usize,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .event
            .cmp(&self.event)
            .then_with(|| other.reader.cmp(&self.reader))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct RunWriter {
    file: File,
    header: RunHeader,
    written: u64,
    previous: Option<Event>,
}

impl RunWriter {
    fn create(path: &Path, header: RunHeader) -> Result<Self> {
        validate_run_header(header)?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|cause| temporary_io("create transition run", cause))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|cause| temporary_io("set private transition-run permissions", cause))?;
        file.write_all(&[0_u8; RUN_HEADER_BYTES])
            .map_err(|cause| temporary_io("reserve transition run header", cause))?;
        Ok(Self {
            file,
            header,
            written: 0,
            previous: None,
        })
    }

    fn push(&mut self, event: Event) -> Result<()> {
        validate_code(event.canonical_qmer, self.header.k + 1)?;
        if let Some(previous) = self.previous {
            if previous >= event {
                return integrity("transition run events are not strictly ordered");
            }
            if previous.canonical_qmer == event.canonical_qmer && previous.same_coordinate(&event) {
                return integrity("duplicate source coordinate in one transition key");
            }
        }
        self.file
            .write_all(&encode_event(event))
            .map_err(|cause| temporary_io("write transition event", cause))?;
        self.written = checked_add(self.written, 1, "written transition records")?;
        if self.written > self.header.record_count {
            return integrity("transition writer exceeded declared record count");
        }
        self.previous = Some(event);
        Ok(())
    }

    fn finish(mut self, path: &Path) -> Result<RunMeta> {
        if self.written != self.header.record_count {
            return integrity("transition writer did not reach declared record count");
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|cause| temporary_io("rewind transition run header", cause))?;
        let header_bytes = encode_header(self.header);
        self.file
            .write_all(&header_bytes)
            .map_err(|cause| temporary_io("write transition run header", cause))?;
        self.file
            .flush()
            .map_err(|cause| temporary_io("flush transition run payload", cause))?;

        let digest = hash_run_prefix(&mut self.file, self.header)?;
        self.file
            .seek(SeekFrom::End(0))
            .map_err(|cause| temporary_io("seek transition run trailer", cause))?;
        self.file
            .write_all(&encode_trailer(self.header.record_count, digest))
            .map_err(|cause| temporary_io("write transition run trailer", cause))?;
        self.file
            .flush()
            .map_err(|cause| temporary_io("flush transition run trailer", cause))?;
        self.file
            .sync_all()
            .map_err(|cause| temporary_io("sync transition run", cause))?;
        let byte_len = self
            .file
            .metadata()
            .map_err(|cause| temporary_io("stat transition run", cause))?
            .len();
        let expected = projected_run_bytes(self.header.record_count)?;
        if byte_len != expected {
            return integrity("sealed transition run length is not its exact projection");
        }
        drop(self.file);
        let id = TransitionRunId {
            generation: self.header.generation,
            run_ordinal: self.header.run_ordinal,
        };
        let expected_name = run_path(
            path.parent()
                .ok_or_else(|| invariant_error("transition run path has no parent"))?,
            id,
        )?;
        if expected_name != path {
            return invariant("transition run path does not match numeric identity");
        }
        Ok(RunMeta {
            id,
            header: self.header,
            byte_len,
            sha256: digest,
        })
    }
}

struct RunReader {
    file: File,
    meta: RunMeta,
    remaining: u64,
    hasher: Sha256,
    previous: Option<Event>,
    authenticated: bool,
    failed: bool,
}

impl RunReader {
    fn open(path: &Path, meta: RunMeta) -> Result<Self> {
        let mut file =
            File::open(path).map_err(|cause| integrity_io("open transition run", cause))?;
        let length = file
            .metadata()
            .map_err(|cause| integrity_io("stat transition run", cause))?
            .len();
        if length != meta.byte_len || length != projected_run_bytes(meta.header.record_count)? {
            return integrity("registered transition run length changed");
        }
        let mut header_bytes = [0_u8; RUN_HEADER_BYTES];
        read_exact_integrity(&mut file, &mut header_bytes, "transition run header")?;
        let header = decode_header(header_bytes)?;
        if header != meta.header
            || meta.id.generation != header.generation
            || meta.id.run_ordinal != header.run_ordinal
        {
            return integrity("transition run header differs from registered metadata");
        }
        let mut hasher = Sha256::new();
        hasher.update(RUN_DIGEST_DOMAIN);
        hasher.update(header_bytes);
        Ok(Self {
            file,
            meta,
            remaining: header.record_count,
            hasher,
            previous: None,
            authenticated: false,
            failed: false,
        })
    }

    fn next_event(&mut self) -> Result<Option<Event>> {
        if self.failed {
            return integrity("cannot continue a failed transition-run reader");
        }
        if self.remaining == 0 {
            if let Err(error) = self.finish_authentication() {
                self.failed = true;
                return Err(error);
            }
            return Ok(None);
        }
        let mut bytes = [0_u8; EVENT_BYTES];
        if let Err(error) = read_exact_integrity(&mut self.file, &mut bytes, "transition event") {
            self.failed = true;
            return Err(error);
        }
        self.hasher.update(bytes);
        let event = decode_event(bytes, self.meta.header.k + 1)?;
        if let Some(previous) = self.previous {
            if previous >= event {
                self.failed = true;
                return integrity("transition run payload is not strictly ordered");
            }
            if previous.canonical_qmer == event.canonical_qmer && previous.same_coordinate(&event) {
                self.failed = true;
                return integrity("transition run repeats a source coordinate");
            }
        }
        self.previous = Some(event);
        self.remaining -= 1;
        Ok(Some(event))
    }

    fn finish_authentication(&mut self) -> Result<()> {
        if self.authenticated {
            return Ok(());
        }
        let mut trailer = [0_u8; RUN_TRAILER_BYTES];
        read_exact_integrity(&mut self.file, &mut trailer, "transition run trailer")?;
        let (records, digest) = decode_trailer(trailer)?;
        if records != self.meta.header.record_count {
            return integrity("transition run trailer record count changed");
        }
        let observed: [u8; 32] = self.hasher.clone().finalize().into();
        if digest != observed || digest != self.meta.sha256 {
            return integrity("transition run digest mismatch");
        }
        let mut probe = [0_u8; 1];
        if self
            .file
            .read(&mut probe)
            .map_err(|cause| integrity_io("read transition run EOF", cause))?
            != 0
        {
            return integrity("bytes follow the transition run trailer");
        }
        self.authenticated = true;
        Ok(())
    }
}

fn verify_complete_run(path: &Path, meta: RunMeta) -> Result<()> {
    let mut reader = RunReader::open(path, meta)?;
    while reader.next_event()?.is_some() {}
    if !reader.authenticated {
        return integrity("transition run verification did not authenticate EOF");
    }
    Ok(())
}

fn reduce_final_run(
    state: &BuildState<'_>,
    meta: RunMeta,
    rows: &mut Vec<TransitionRow>,
) -> Result<()> {
    let path = run_path(state.run_dir, meta.id)?;
    let mut reader = RunReader::open(&path, meta)?;
    let mut current_key = None;
    let mut occurrence_support = 0_u64;
    let mut fragment_support = 0_u64;
    let mut previous_event = None;
    let mut event_hasher = Sha256::new();
    event_hasher.update(EVENT_SET_DOMAIN);
    while let Some(event) = reader.next_event()? {
        if current_key != Some(event.canonical_qmer) {
            if let Some(key) = current_key {
                push_reduced_row(
                    rows,
                    state.options.limits.max_rows,
                    key,
                    occurrence_support,
                    fragment_support,
                    &event_hasher,
                )?;
            }
            current_key = Some(event.canonical_qmer);
            occurrence_support = 0;
            fragment_support = 0;
            previous_event = None;
            event_hasher = Sha256::new();
            event_hasher.update(EVENT_SET_DOMAIN);
            event_hasher.update(event.canonical_qmer.to_be_bytes());
        }
        if previous_event.is_some_and(|previous: Event| previous.same_coordinate(&event)) {
            return integrity("one transition has a duplicate source coordinate");
        }
        occurrence_support = checked_add(occurrence_support, 1, "transition occurrence support")?;
        if previous_event.is_none_or(|previous: Event| !previous.same_fragment(&event)) {
            fragment_support = checked_add(fragment_support, 1, "transition fragment support")?;
        }
        event_hasher.update(encode_event(event));
        previous_event = Some(event);
    }
    if let Some(key) = current_key {
        push_reduced_row(
            rows,
            state.options.limits.max_rows,
            key,
            occurrence_support,
            fragment_support,
            &event_hasher,
        )?;
    }
    if !reader.authenticated {
        return integrity("final transition reduction did not authenticate its input");
    }
    Ok(())
}

fn push_reduced_row(
    rows: &mut Vec<TransitionRow>,
    max_rows: u64,
    key: PackedKmer,
    occurrences: u64,
    fragments: u64,
    hasher: &Sha256,
) -> Result<()> {
    if u64::try_from(rows.len()).map_err(|_| overflow("transition row length"))? >= max_rows {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            format!("transition rows exceed limit {max_rows}"),
        ));
    }
    rows.push(TransitionRow {
        canonical_qmer: key,
        accepted_window_occurrences: occurrences,
        distinct_supplied_fragment_instances: fragments,
        sorted_event_frames_sha256: hasher.clone().finalize().into(),
    });
    Ok(())
}

fn encode_header(header: RunHeader) -> [u8; RUN_HEADER_BYTES] {
    let mut bytes = [0_u8; RUN_HEADER_BYTES];
    bytes[0..8].copy_from_slice(RUN_MAGIC);
    bytes[8..10].copy_from_slice(&RUN_SCHEMA.to_le_bytes());
    bytes[10..12].copy_from_slice(&(RUN_HEADER_BYTES as u16).to_le_bytes());
    bytes[12..14].copy_from_slice(&(EVENT_BYTES as u16).to_le_bytes());
    bytes[14] = header.k;
    bytes[16..20].copy_from_slice(&header.generation.to_le_bytes());
    bytes[24..32].copy_from_slice(&header.run_ordinal.to_le_bytes());
    bytes[32..40].copy_from_slice(&header.first_event_ordinal.to_le_bytes());
    bytes[40..48].copy_from_slice(&header.event_ordinal_end.to_le_bytes());
    bytes[48..56].copy_from_slice(&header.record_count.to_le_bytes());
    bytes[56..64].copy_from_slice(&header.payload_bytes.to_le_bytes());
    bytes[64..96].copy_from_slice(&header.source_root);
    bytes
}

fn decode_header(bytes: [u8; RUN_HEADER_BYTES]) -> Result<RunHeader> {
    if &bytes[0..8] != RUN_MAGIC {
        return integrity("transition run magic mismatch");
    }
    if read_u16(&bytes, 8)? != RUN_SCHEMA
        || usize::from(read_u16(&bytes, 10)?) != RUN_HEADER_BYTES
        || usize::from(read_u16(&bytes, 12)?) != EVENT_BYTES
    {
        return integrity("transition run schema or fixed width mismatch");
    }
    if bytes[15] != 0
        || bytes[20..24].iter().any(|byte| *byte != 0)
        || bytes[96..].iter().any(|byte| *byte != 0)
    {
        return integrity("transition run reserved header bytes are nonzero");
    }
    let mut source_root = [0_u8; 32];
    source_root.copy_from_slice(&bytes[64..96]);
    let header = RunHeader {
        source_root,
        k: bytes[14],
        generation: read_u32(&bytes, 16)?,
        run_ordinal: read_u64(&bytes, 24)?,
        first_event_ordinal: read_u64(&bytes, 32)?,
        event_ordinal_end: read_u64(&bytes, 40)?,
        record_count: read_u64(&bytes, 48)?,
        payload_bytes: read_u64(&bytes, 56)?,
    };
    validate_run_header(header)?;
    Ok(header)
}

fn validate_run_header(header: RunHeader) -> Result<()> {
    validate_graph_k(header.k)?;
    if header.record_count == 0
        || header.first_event_ordinal >= header.event_ordinal_end
        || checked_add(
            header.first_event_ordinal,
            header.record_count,
            "transition header interval",
        )? != header.event_ordinal_end
        || checked_mul(
            header.record_count,
            EVENT_BYTES as u64,
            "transition header payload",
        )? != header.payload_bytes
    {
        return integrity("transition run header has an invalid interval or payload length");
    }
    Ok(())
}

fn encode_event(event: Event) -> [u8; EVENT_BYTES] {
    let mut bytes = [0_u8; EVENT_BYTES];
    bytes[0..PACKED_KEY_BYTES].copy_from_slice(&event.canonical_qmer.to_be_bytes());
    bytes[32..36].copy_from_slice(&event.lane_ordinal.to_le_bytes());
    bytes[36..44].copy_from_slice(&event.fragment_ordinal.to_le_bytes());
    bytes[44] = event.mate_role.tag();
    bytes[45] = event.orientation as u8;
    bytes[48..56].copy_from_slice(&event.read_offset.to_le_bytes());
    bytes[56..88].copy_from_slice(&event.normalized_id_digest);
    bytes
}

fn decode_event(bytes: [u8; EVENT_BYTES], q: u8) -> Result<Event> {
    if bytes[46..48].iter().any(|byte| *byte != 0) {
        return integrity("transition event reserved bytes are nonzero");
    }
    let mut key_bytes = [0_u8; PACKED_KEY_BYTES];
    key_bytes.copy_from_slice(&bytes[0..32]);
    let canonical_qmer = PackedKmer::from_be_bytes(key_bytes);
    validate_code(canonical_qmer, q)?;
    let reverse = reverse_complement(canonical_qmer, q)?;
    if reverse < canonical_qmer {
        return integrity("transition event key is not canonical");
    }
    let mate_role = match bytes[44] {
        0 => MateRole::S,
        1 => MateRole::R1,
        2 => MateRole::R2,
        tag => return integrity(format!("unknown transition mate-role tag {tag}")),
    };
    let orientation = ObservedOrientation::from_tag(bytes[45])?;
    if (canonical_qmer == reverse) != (orientation == ObservedOrientation::FixedPoint) {
        return integrity("transition event fixed-point orientation is inconsistent");
    }
    let mut normalized_id_digest = [0_u8; 32];
    normalized_id_digest.copy_from_slice(&bytes[56..88]);
    Ok(Event {
        canonical_qmer,
        lane_ordinal: read_u32(&bytes, 32)?,
        fragment_ordinal: read_u64(&bytes, 36)?,
        mate_role,
        normalized_id_digest,
        read_offset: read_u64(&bytes, 48)?,
        orientation,
    })
}

fn encode_trailer(record_count: u64, digest: [u8; 32]) -> [u8; RUN_TRAILER_BYTES] {
    let mut bytes = [0_u8; RUN_TRAILER_BYTES];
    bytes[0..8].copy_from_slice(RUN_END);
    bytes[8..16].copy_from_slice(&record_count.to_le_bytes());
    bytes[16..48].copy_from_slice(&digest);
    bytes
}

fn decode_trailer(bytes: [u8; RUN_TRAILER_BYTES]) -> Result<(u64, [u8; 32])> {
    if &bytes[0..8] != RUN_END {
        return integrity("transition run trailer magic mismatch");
    }
    let records = read_u64(&bytes, 8)?;
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&bytes[16..48]);
    Ok((records, digest))
}

fn hash_run_prefix(file: &mut File, header: RunHeader) -> Result<[u8; 32]> {
    let prefix_bytes = checked_add(
        RUN_HEADER_BYTES as u64,
        header.payload_bytes,
        "transition run hash prefix",
    )?;
    file.seek(SeekFrom::Start(0))
        .map_err(|cause| temporary_io("rewind transition run for hashing", cause))?;
    let mut remaining = prefix_bytes;
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    hasher.update(RUN_DIGEST_DOMAIN);
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(HASH_BUFFER_BYTES as u64))
            .map_err(|_| overflow("transition hash read size"))?;
        file.read_exact(&mut buffer[..wanted])
            .map_err(|cause| temporary_io("hash transition run", cause))?;
        hasher.update(&buffer[..wanted]);
        remaining -= wanted as u64;
    }
    Ok(hasher.finalize().into())
}

fn projected_run_bytes(records: u64) -> Result<u64> {
    checked_sum(&[
        RUN_HEADER_BYTES as u64,
        checked_mul(records, EVENT_BYTES as u64, "projected transition payload")?,
        RUN_TRAILER_BYTES as u64,
    ])
}

fn validate_sorted_events(events: &[Event]) -> Result<()> {
    for pair in events.windows(2) {
        if pair[0] >= pair[1] {
            return integrity("transition sort buffer contains duplicate or unordered events");
        }
        if pair[0].canonical_qmer == pair[1].canonical_qmer && pair[0].same_coordinate(&pair[1]) {
            return integrity("transition sort buffer repeats a source coordinate");
        }
    }
    Ok(())
}

fn validate_source_coordinate_stream(previous: &mut Option<Event>, events: &[Event]) -> Result<()> {
    for event in events {
        if previous.is_some_and(|prior| prior.coordinate_cmp(event) != Ordering::Less) {
            return integrity(
                "transition source coordinates are duplicated or not in authenticated replay order",
            );
        }
        *previous = Some(*event);
    }
    Ok(())
}

fn reverse_complement(mut code: PackedKmer, length: u8) -> Result<PackedKmer> {
    validate_code(code, length)?;
    let mut reverse = [0_u64; 4];
    for _ in 0..length {
        let pair = (code.words()[3] & 0b11) as u8;
        reverse = shift_left_two_words(reverse, 3 - pair);
        code = shift_right_two(code);
    }
    Ok(PackedKmer::from_words(reverse))
}

fn transition_root(source_root: [u8; 32], k: u8, rows: &[TransitionRow]) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(LEDGER_DOMAIN);
    digest.update(source_root);
    digest.update([k]);
    digest.update(
        u64::try_from(rows.len())
            .map_err(|_| overflow("transition row count does not fit u64"))?
            .to_le_bytes(),
    );
    for row in rows {
        digest.update(row.canonical_qmer.to_be_bytes());
        digest.update(row.accepted_window_occurrences.to_le_bytes());
        digest.update(row.distinct_supplied_fragment_instances.to_le_bytes());
        digest.update(row.sorted_event_frames_sha256);
    }
    Ok(digest.finalize().into())
}

fn decode_hex_32(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        return integrity("registered spool digest is not 64 hexadecimal bytes");
    }
    let mut output = [0_u8; 32];
    let bytes = value.as_bytes();
    for index in 0..32 {
        let high = decode_hex_nibble(bytes[index * 2])?;
        let low = decode_hex_nibble(bytes[index * 2 + 1])?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

fn decode_hex_nibble(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => integrity("registered spool digest is not lowercase hexadecimal"),
    }
}

fn path_state_allowance(work_dir: &Path) -> Result<u64> {
    let work = u64::try_from(work_dir.as_os_str().as_encoded_bytes().len())
        .map_err(|_| overflow("transition work-directory path length"))?;
    let run_directory = checked_sum(&[
        work,
        PATH_SEPARATOR_ALLOWANCE_BYTES,
        RUN_DIRECTORY_NAME_BYTES,
    ])?;
    let run_path = checked_sum(&[
        run_directory,
        PATH_SEPARATOR_ALLOWANCE_BYTES,
        RUN_FILENAME_BYTES,
    ])?;
    checked_sum(&[
        run_directory,
        checked_mul(run_path, 2, "simultaneous transition run paths")?,
        RUN_FILENAME_BYTES,
    ])
}

fn run_path(directory: &Path, id: TransitionRunId) -> Result<PathBuf> {
    let filename_capacity = usize::try_from(RUN_FILENAME_BYTES)
        .map_err(|_| config_error("transition run filename length does not fit usize"))?;
    let mut filename = String::new();
    filename
        .try_reserve_exact(filename_capacity)
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve transition run filename: {cause}"),
            )
        })?;
    write!(
        filename,
        "tw-g{:010}-r{:020}.bin",
        id.generation, id.run_ordinal
    )
    .map_err(|_| invariant_error("cannot format fixed-width transition run filename"))?;
    if filename.len() != filename_capacity {
        return invariant("transition numeric run identity exceeded its fixed-width encoding");
    }
    enforce_capacity(
        filename.capacity(),
        filename_capacity,
        "transition run filename",
    )?;
    let directory_bytes = u64::try_from(directory.as_os_str().as_encoded_bytes().len())
        .map_err(|_| overflow("transition run-directory path length"))?;
    let admitted = checked_sum(&[
        directory_bytes,
        PATH_SEPARATOR_ALLOWANCE_BYTES,
        RUN_FILENAME_BYTES,
    ])?;
    let admitted = usize::try_from(admitted)
        .map_err(|_| config_error("transition run path does not fit usize"))?;
    let mut path = PathBuf::new();
    path.try_reserve_exact(admitted).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve transition run path: {cause}"),
        )
    })?;
    path.push(directory);
    path.push(filename);
    enforce_capacity(path.capacity(), admitted, "transition run path")?;
    Ok(path)
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
            .tempdir_in(work_dir)
            .map_err(|cause| temporary_io("create unique transition-run directory", cause))?;
        // Disarm tempfile's recursive pathname cleanup. From here onward this
        // guard removes only the identity-matching directory and bounded,
        // independently checked transition-run entries.
        let path = directory.keep();
        let initial_metadata = fs::symlink_metadata(&path)
            .map_err(|cause| temporary_io("stat unique transition-run directory", cause))?;
        if !initial_metadata.file_type().is_dir() || initial_metadata.file_type().is_symlink() {
            return integrity("new transition-run path is not a literal directory");
        }
        let initial_identity = DirectoryIdentity::from_metadata(&initial_metadata);
        let initialized = (|| {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|cause| {
                temporary_io("set private transition-run directory permissions", cause)
            })?;
            let metadata = fs::symlink_metadata(&path)
                .map_err(|cause| temporary_io("restat private transition-run directory", cause))?;
            let identity =
                validate_directory_metadata(&metadata, "validate unique transition-run directory")?;
            if !same_directory_object(identity, initial_identity) {
                return integrity("transition-run directory changed during initialization");
            }
            let anchor = open_directory_nofollow(&path)?;
            let descriptor_identity = validate_directory_metadata(
                &anchor.metadata().map_err(|cause| {
                    temporary_io("stat transition-run directory descriptor", cause)
                })?,
                "validate transition-run directory descriptor",
            )?;
            if descriptor_identity != identity {
                return integrity("transition-run directory changed while it was opened");
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
                            "transition-run directory initialization failed ({primary}); identity-safe cleanup also failed: {cleanup}"
                        ),
                    )),
                };
            }
        };
        let name_bytes = path
            .file_name()
            .ok_or_else(|| invariant_error("transition-run directory has no final component"))?
            .as_encoded_bytes()
            .len() as u64;
        if name_bytes > RUN_DIRECTORY_NAME_BYTES {
            let primary =
                invariant_error("unique transition-run directory exceeded its admitted name bound");
            return match remove_same_empty_directory(&path, identity) {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(VeritasmError::new(
                    ErrorCode::ResourceTemporaryBytes,
                    format!(
                        "transition-run directory name validation failed ({primary}); identity-safe cleanup also failed: {cleanup}"
                    ),
                )),
            };
        }
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

    fn finish<T>(mut self, result: T) -> Result<T> {
        verify_owned_run_directory(&self.path, self.identity)?;
        fs::remove_dir(&self.path).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!("cannot remove empty transition-run directory: {cause}"),
            )
        })?;
        self.armed = false;
        Ok(result)
    }

    fn fail<T>(mut self, error: VeritasmError) -> Result<T> {
        let cleanup =
            remove_owned_run_directory_contents(&self.path, self.identity, self.max_entries);
        self.armed = false;
        match cleanup {
            Ok(()) => Err(error),
            Err(cause) => Err(VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!(
                    "{}; transition-run cleanup also failed: {cause}",
                    error.context()
                ),
            )),
        }
    }
}

impl Drop for RunDirectoryGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ =
                remove_owned_run_directory_contents(&self.path, self.identity, self.max_entries);
        }
    }
}

const fn same_directory_object(left: DirectoryIdentity, right: DirectoryIdentity) -> bool {
    left.device == right.device && left.inode == right.inode && left.owner_uid == right.owner_uid
}

fn remove_same_empty_directory(path: &Path, expected: DirectoryIdentity) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|cause| temporary_io("stat provisional transition-run directory", cause))?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || !same_directory_object(DirectoryIdentity::from_metadata(&metadata), expected)
    {
        return integrity("provisional transition-run directory identity changed");
    }
    fs::remove_dir(path)
        .map_err(|cause| temporary_io("remove provisional transition-run directory", cause))
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
            format!("cannot open private transition-run directory: {cause}"),
        )
    })?;
    Ok(File::from(descriptor))
}

fn verify_owned_run_directory(path: &Path, expected: DirectoryIdentity) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|cause| temporary_io("stat owned transition-run directory", cause))?;
    let identity =
        validate_directory_metadata(&metadata, "validate owned transition-run directory")?;
    if identity != expected {
        return integrity("owned transition-run directory identity changed");
    }
    Ok(())
}

fn is_transition_run_filename(name: &std::ffi::OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    bytes.len() == RUN_FILENAME_BYTES as usize
        && &bytes[..4] == b"tw-g"
        && bytes[4..14].iter().all(u8::is_ascii_digit)
        && &bytes[14..16] == b"-r"
        && bytes[16..36].iter().all(u8::is_ascii_digit)
        && &bytes[36..] == b".bin"
}

fn open_cleanup_file_nofollow(path: &Path) -> Result<File> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let descriptor = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|cause| temporary_io("open transition-run cleanup entry", cause.into()))?;
    Ok(File::from(descriptor))
}

fn validate_cleanup_entry(entry: &fs::DirEntry, owner_uid: u32) -> Result<()> {
    if !is_transition_run_filename(&entry.file_name()) {
        return integrity("private transition-run directory contains an unknown entry name");
    }
    let path = entry.path();
    let path_metadata = fs::symlink_metadata(&path)
        .map_err(|cause| temporary_io("stat transition-run cleanup entry", cause))?;
    if !path_metadata.file_type().is_file()
        || path_metadata.file_type().is_symlink()
        || path_metadata.uid() != owner_uid
        || path_metadata.mode() & 0o777 != 0o600
    {
        return integrity(
            "private transition-run directory contains a non-private or non-regular entry",
        );
    }
    let path_identity = CleanupFileIdentity::from_metadata(&path_metadata);
    let file = open_cleanup_file_nofollow(&path)?;
    let descriptor_identity = CleanupFileIdentity::from_metadata(
        &file
            .metadata()
            .map_err(|cause| temporary_io("stat transition-run cleanup descriptor", cause))?,
    );
    if descriptor_identity != path_identity {
        return integrity("transition-run cleanup entry changed while it was opened");
    }
    Ok(())
}

fn scan_cleanup_entries(path: &Path, owner_uid: u32, max_entries: u64, remove: bool) -> Result<()> {
    let mut entries = 0_u64;
    for entry in fs::read_dir(path)
        .map_err(|cause| temporary_io("read private transition-run directory", cause))?
    {
        let entry = entry
            .map_err(|cause| temporary_io("read private transition-run directory entry", cause))?;
        entries = checked_add(entries, 1, "transition cleanup entry count")?;
        if entries > max_entries {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRunCount,
                format!("transition cleanup entry count exceeds admitted run limit {max_entries}"),
            ));
        }
        validate_cleanup_entry(&entry, owner_uid)?;
        if remove {
            fs::remove_file(entry.path())
                .map_err(|cause| temporary_io("remove validated transition-run entry", cause))?;
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
    // validates each entry again immediately before unlinking it.
    scan_cleanup_entries(path, expected.owner_uid, max_entries, false)?;
    verify_owned_run_directory(path, expected)?;
    scan_cleanup_entries(path, expected.owner_uid, max_entries, true)?;
    verify_owned_run_directory(path, expected)?;
    fs::remove_dir(path)
        .map_err(|cause| temporary_io("remove identity-verified transition-run directory", cause))
}

fn validate_window_partition(stats: &WindowStats) -> Result<()> {
    let classified = checked_sum(&[
        stats.accepted,
        stats.ambiguity_only,
        stats.quality_only,
        stats.ambiguity_and_quality,
    ])?;
    if classified != stats.possible {
        return integrity("transition QC classes do not partition possible windows");
    }
    Ok(())
}

fn fallible_vec<T>(capacity: usize, label: &'static str) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve {label}: {cause}"),
        )
    })?;
    enforce_capacity(values.capacity(), capacity, label)?;
    Ok(values)
}

fn enforce_capacity(actual: usize, admitted: usize, label: &'static str) -> Result<()> {
    if actual <= admitted {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("allocator returned {label} capacity {actual} above admitted {admitted}"),
        ))
    }
}

fn read_exact_integrity(file: &mut File, bytes: &mut [u8], label: &'static str) -> Result<()> {
    file.read_exact(bytes)
        .map_err(|cause| integrity_io(label, cause))
}

fn read_u16<const N: usize>(bytes: &[u8; N], offset: usize) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| overflow("u16 decode offset"))?;
    let encoded: [u8; 2] = bytes
        .get(offset..end)
        .ok_or_else(|| invariant_error("u16 decode outside fixed frame"))?
        .try_into()
        .map_err(|_| invariant_error("u16 fixed-frame conversion failed"))?;
    Ok(u16::from_le_bytes(encoded))
}

fn read_u32<const N: usize>(bytes: &[u8; N], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| overflow("u32 decode offset"))?;
    let encoded: [u8; 4] = bytes
        .get(offset..end)
        .ok_or_else(|| invariant_error("u32 decode outside fixed frame"))?
        .try_into()
        .map_err(|_| invariant_error("u32 fixed-frame conversion failed"))?;
    Ok(u32::from_le_bytes(encoded))
}

fn read_u64<const N: usize>(bytes: &[u8; N], offset: usize) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| overflow("u64 decode offset"))?;
    let encoded: [u8; 8] = bytes
        .get(offset..end)
        .ok_or_else(|| invariant_error("u64 decode outside fixed frame"))?
        .try_into()
        .map_err(|_| invariant_error("u64 fixed-frame conversion failed"))?;
    Ok(u64::from_le_bytes(encoded))
}

fn checked_add(left: u64, right: u64, label: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(label))
}

fn checked_mul(left: u64, right: u64, label: &'static str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| overflow(label))
}

fn checked_sum(values: &[u64]) -> Result<u64> {
    values.iter().try_fold(0_u64, |total, value| {
        checked_add(total, *value, "transition resource sum")
    })
}

fn increment(value: &mut u64, label: &'static str) -> Result<()> {
    *value = checked_add(*value, 1, label)?;
    Ok(())
}

fn temporary_io(label: &'static str, cause: std::io::Error) -> VeritasmError {
    VeritasmError::new(
        ErrorCode::ResourceTemporaryBytes,
        format!("{label}: {cause}"),
    )
}

fn integrity_io(label: &'static str, cause: std::io::Error) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegrityArtifact, format!("{label}: {cause}"))
}

fn config<T>(context: impl Into<String>) -> Result<T> {
    Err(config_error(context))
}

fn config_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ConfigurationInvalidLimit, context)
}

fn integrity<T>(context: impl Into<String>) -> Result<T> {
    Err(VeritasmError::new(ErrorCode::IntegrityArtifact, context))
}

fn invariant<T>(context: impl Into<String>) -> Result<T> {
    Err(invariant_error(context))
}

fn invariant_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

fn overflow(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig, SupportUnit};
    use crate::experimental::wide_kmer::{decode_mer, encode_exact_bases};
    use crate::spool::create_spool;

    fn scientific(minimum_quality: u8) -> ScientificConfig {
        ScientificConfig::resolve(
            5,
            Profile::RetainAll,
            SupportUnit::SuppliedFragmentInstance,
            None,
            minimum_quality,
            false,
        )
        .unwrap()
    }

    fn spool_limits() -> Limits {
        Limits {
            memory_budget_bytes: 32 * 1024 * 1024,
            max_spool_bytes: 16 * 1024 * 1024,
            max_temp_bytes: 32 * 1024 * 1024,
            ..Limits::default()
        }
    }

    fn ledger_limits(chunk_records: u64) -> TransitionLedgerLimits {
        TransitionLedgerLimits {
            max_events: 10_000,
            max_rows: 256,
            max_fragment_decode_bytes: 1 << 20,
            max_fragment_windows: 10_000,
            max_memory_bytes: 8 * 1024 * 1024,
            sort_buffer_bytes: chunk_records * size_of::<Event>() as u64,
            max_temp_bytes: 4 * 1024 * 1024,
            max_run_files: 128,
            merge_fan_in: 2,
            max_open_files: 3,
        }
    }

    fn options(work: &Path, k: u8, chunk_records: u64) -> TransitionLedgerOptions {
        TransitionLedgerOptions {
            work_dir: work.to_path_buf(),
            k,
            limits: ledger_limits(chunk_records),
        }
    }

    fn assert_no_transition_run_directories(work: &Path) {
        let leaked = fs::read_dir(work)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(RUN_DIRECTORY_PREFIX)
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert!(
            leaked.is_empty(),
            "leaked transition directories: {leaked:?}"
        );
    }

    fn paired_spool(root: &Path) -> Spool {
        let r1 = root.join("r1.fastq");
        let r2 = root.join("r2.fastq");
        fs::write(
            &r1,
            b"@same/1\nAAAAAA\n+\nIIIIII\n@same/1\nAAAAAA\n+\nIIIIII\n",
        )
        .unwrap();
        fs::write(
            &r2,
            b"@same/2\nTTTTTT\n+\nIIIIII\n@same/2\nTTTTTT\n+\nIIIIII\n",
        )
        .unwrap();
        create_spool(
            &InputSpec::Paired {
                read1: vec![r1],
                read2: vec![r2],
            },
            &scientific(20),
            &spool_limits(),
            root,
        )
        .unwrap()
    }

    fn single_spool(root: &Path, records: &[(&str, &str, Option<&str>)], quality: u8) -> Spool {
        let reads = root.join("reads.fastq");
        let mut bytes = Vec::new();
        for (id, sequence, qualities) in records {
            bytes.extend_from_slice(format!("@{id}\n{sequence}\n+\n").as_bytes());
            match qualities {
                Some(values) => bytes.extend_from_slice(values.as_bytes()),
                None => bytes.extend(std::iter::repeat_n(b'I', sequence.len())),
            }
            bytes.push(b'\n');
        }
        fs::write(&reads, bytes).unwrap();
        create_spool(
            &InputSpec::Single(vec![reads]),
            &scientific(quality),
            &spool_limits(),
            root,
        )
        .unwrap()
    }

    fn literal_reverse_complement(sequence: &[u8]) -> Vec<u8> {
        sequence
            .iter()
            .rev()
            .map(|base| match base {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => panic!("literal oracle received non-ACGT"),
            })
            .collect()
    }

    fn sequence_for_index(mut index: usize, length: usize) -> Vec<u8> {
        let mut result = vec![b'A'; length];
        for base in result.iter_mut().rev() {
            *base = b"ACGT"[index % 4];
            index /= 4;
        }
        result
    }

    #[test]
    fn literal_small_q_oracle_exhaustively_matches_key_and_orientation() {
        for k in [3_u8, 4] {
            let q = usize::from(k + 1);
            let total = 4_usize.pow(q as u32);
            for index in 0..total {
                let sequence = sequence_for_index(index, q);
                let reverse = literal_reverse_complement(&sequence);
                let expected = sequence.as_slice().min(reverse.as_slice()).to_vec();
                let expected_orientation = match sequence.cmp(&reverse) {
                    Ordering::Less => ObservedOrientation::Canonical,
                    Ordering::Greater => ObservedOrientation::ReverseComplement,
                    Ordering::Equal => ObservedOrientation::FixedPoint,
                };
                let read = ReadRecord {
                    role: MateRole::S,
                    normalized_id_digest: [7; 32],
                    sequence,
                    quality: None,
                };
                let mut events = Vec::new();
                scan_read(
                    &read,
                    0,
                    0,
                    0,
                    k + 1,
                    &mut WindowStats::default(),
                    &mut |event| {
                        events.push(event);
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(events.len(), 1);
                assert_eq!(
                    decode_mer(events[0].canonical_qmer, k + 1).unwrap(),
                    expected
                );
                assert_eq!(events[0].orientation, expected_orientation);

                let rc_code = encode_exact_bases(&reverse).unwrap();
                let rc_canonical = rc_code.min(reverse_complement(rc_code, k + 1).unwrap());
                assert_eq!(events[0].canonical_qmer, rc_canonical);
            }
        }
    }

    #[test]
    fn complete_coordinates_count_occurrences_but_deduplicate_fragments_across_mates() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let ledger = build_transition_ledger(&spool, &options(&work, 3, 2)).unwrap();
        assert_eq!(ledger.event_count, 12);
        assert_eq!(ledger.rows.len(), 1);
        assert_eq!(
            decode_mer(ledger.rows[0].canonical_qmer, 4).unwrap(),
            b"AAAA"
        );
        assert_eq!(ledger.rows[0].accepted_window_occurrences, 12);
        assert_eq!(ledger.rows[0].distinct_supplied_fragment_instances, 2);
        assert!(ledger.run_files_created > 1);
        assert!(ledger
            .replacements
            .iter()
            .all(|row| row.predecessors_reclaimed));
        assert_eq!(ledger.temporary_bytes_final, 0);
        assert_no_transition_run_directories(&work);
        validate_transition_ledger_replay(&spool, &options(&work, 3, 2), &ledger).unwrap();
    }

    #[test]
    fn read_mate_ambiguity_and_quality_boundaries_are_never_crossed() {
        let temp = tempfile::tempdir().unwrap();
        let spool = single_spool(
            temp.path(),
            &[("a", "AAAANAAAA", Some("IIIIII!II")), ("b", "AAA", None)],
            20,
        );
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let ledger = build_transition_ledger(&spool, &options(&work, 3, 32)).unwrap();
        assert_eq!(ledger.window_stats.possible, 6);
        assert_eq!(ledger.window_stats.accepted, 1);
        assert_eq!(ledger.window_stats.ambiguity_only, 2);
        assert_eq!(ledger.window_stats.quality_only, 1);
        assert_eq!(ledger.window_stats.ambiguity_and_quality, 2);
        assert_eq!(ledger.event_count, 1);
    }

    #[test]
    fn deterministic_spill_geometry_and_roots_exclude_work_paths() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let work_a = temp.path().join("a");
        let work_b = temp.path().join("a-much-longer-work-directory-name");
        fs::create_dir(&work_a).unwrap();
        fs::create_dir(&work_b).unwrap();
        let first = build_transition_ledger(&spool, &options(&work_a, 3, 2)).unwrap();
        let second = build_transition_ledger(&spool, &options(&work_b, 3, 2)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn event_limit_is_inclusive_and_one_below_fails_typed() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let mut exact = options(&work, 3, 32);
        exact.limits.max_events = 12;
        assert_eq!(
            build_transition_ledger(&spool, &exact).unwrap().event_count,
            12
        );
        exact.limits.max_events = 11;
        let error = build_transition_ledger(&spool, &exact).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceRetainedKeys);
        assert_no_transition_run_directories(&work);
    }

    #[test]
    fn replay_rejects_mutated_row_support_digest_and_source_fields() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let options = options(&work, 3, 3);
        let original = build_transition_ledger(&spool, &options).unwrap();

        let mut changed = original.clone();
        changed.rows[0].accepted_window_occurrences += 1;
        changed.event_count += 1;
        changed.window_stats.accepted += 1;
        changed.window_stats.possible += 1;
        changed.transition_root =
            transition_root(changed.source_root, changed.k, &changed.rows).unwrap();
        validate_ledger_structure(&changed).unwrap();
        assert_eq!(
            validate_transition_ledger_replay(&spool, &options, &changed)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        let mut changed = original.clone();
        changed.rows[0].sorted_event_frames_sha256[0] ^= 1;
        changed.transition_root =
            transition_root(changed.source_root, changed.k, &changed.rows).unwrap();
        validate_ledger_structure(&changed).unwrap();
        assert_eq!(
            validate_transition_ledger_replay(&spool, &options, &changed)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        let mut changed = original.clone();
        changed.source_root[0] ^= 1;
        changed.transition_root =
            transition_root(changed.source_root, changed.k, &changed.rows).unwrap();
        validate_ledger_structure(&changed).unwrap();
        assert_eq!(
            validate_transition_ledger_replay(&spool, &options, &changed)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut changed = original.clone();
        changed.rows[0].canonical_qmer = encode_exact_bases(b"CCCC").unwrap();
        changed.transition_root =
            transition_root(changed.source_root, changed.k, &changed.rows).unwrap();
        validate_ledger_structure(&changed).unwrap();
        assert_eq!(
            validate_transition_ledger_replay(&spool, &options, &changed)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );

        let mut changed = original;
        changed.k = 4;
        changed.q = 5;
        changed.transition_root =
            transition_root(changed.source_root, changed.k, &changed.rows).unwrap();
        validate_ledger_structure(&changed).unwrap();
        assert_eq!(
            validate_transition_ledger_replay(&spool, &options, &changed)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    fn event(sequence: &[u8], fragment: u64, role: MateRole, offset: u64) -> Event {
        let observed = encode_exact_bases(sequence).unwrap();
        let reverse = reverse_complement(observed, sequence.len() as u8).unwrap();
        let orientation = match observed.cmp(&reverse) {
            Ordering::Less => ObservedOrientation::Canonical,
            Ordering::Greater => ObservedOrientation::ReverseComplement,
            Ordering::Equal => ObservedOrientation::FixedPoint,
        };
        Event {
            canonical_qmer: observed.min(reverse),
            lane_ordinal: 0,
            fragment_ordinal: fragment,
            mate_role: role,
            normalized_id_digest: [fragment as u8; 32],
            read_offset: offset,
            orientation,
        }
    }

    #[test]
    fn fixed_run_rejects_payload_corruption_at_terminal_authentication() {
        let temp = tempfile::tempdir().unwrap();
        let path = run_path(
            temp.path(),
            TransitionRunId {
                generation: 0,
                run_ordinal: 0,
            },
        )
        .unwrap();
        let header = RunHeader {
            source_root: [9; 32],
            k: 3,
            generation: 0,
            run_ordinal: 0,
            first_event_ordinal: 0,
            event_ordinal_end: 1,
            record_count: 1,
            payload_bytes: EVENT_BYTES as u64,
        };
        let mut writer = RunWriter::create(&path, header).unwrap();
        writer.push(event(b"AAAA", 0, MateRole::S, 0)).unwrap();
        let meta = writer.finish(&path).unwrap();
        verify_complete_run(&path, meta).unwrap();

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.seek(SeekFrom::Start((RUN_HEADER_BYTES + 56) as u64))
            .unwrap();
        file.write_all(&[1]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut reader = RunReader::open(&path, meta).unwrap();
        assert!(reader.next_event().unwrap().is_some());
        assert_eq!(
            reader.next_event().unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn every_identity_coordinate_orientation_and_header_field_is_authenticated() {
        let temp = tempfile::tempdir().unwrap();
        let path = run_path(
            temp.path(),
            TransitionRunId {
                generation: 0,
                run_ordinal: 0,
            },
        )
        .unwrap();
        let header = RunHeader {
            source_root: [9; 32],
            k: 3,
            generation: 0,
            run_ordinal: 0,
            first_event_ordinal: 0,
            event_ordinal_end: 1,
            record_count: 1,
            payload_bytes: EVENT_BYTES as u64,
        };
        let mut writer = RunWriter::create(&path, header).unwrap();
        writer.push(event(b"AAAC", 7, MateRole::R1, 3)).unwrap();
        let meta = writer.finish(&path).unwrap();
        let original = fs::read(&path).unwrap();
        for offset in [
            RUN_HEADER_BYTES + 31, // complete q-mer
            RUN_HEADER_BYTES + 32, // lane
            RUN_HEADER_BYTES + 36, // fragment
            RUN_HEADER_BYTES + 44, // mate role
            RUN_HEADER_BYTES + 45, // orientation
            RUN_HEADER_BYTES + 48, // read offset
            RUN_HEADER_BYTES + 56, // normalized ID digest
            14,                    // k
            48,                    // record count
            64,                    // source root
        ] {
            let mut changed = original.clone();
            changed[offset] ^= 1;
            fs::write(&path, changed).unwrap();
            let rejected = match RunReader::open(&path, meta) {
                Err(_) => true,
                Ok(mut reader) => {
                    let mut failed = false;
                    loop {
                        match reader.next_event() {
                            Ok(Some(_)) => {}
                            Ok(None) => break,
                            Err(_) => {
                                failed = true;
                                break;
                            }
                        }
                    }
                    failed
                }
            };
            assert!(rejected, "mutation at run byte {offset} was accepted");
        }
        fs::write(&path, original).unwrap();
        verify_complete_run(&path, meta).unwrap();
    }

    #[test]
    fn writer_rejects_a_duplicate_complete_source_coordinate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("duplicate.bin");
        let header = RunHeader {
            source_root: [1; 32],
            k: 3,
            generation: 0,
            run_ordinal: 0,
            first_event_ordinal: 0,
            event_ordinal_end: 2,
            record_count: 2,
            payload_bytes: (EVENT_BYTES * 2) as u64,
        };
        let duplicate = event(b"AAAA", 0, MateRole::S, 0);
        let mut writer = RunWriter::create(&path, header).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        writer.push(duplicate).unwrap();
        assert_eq!(
            writer.push(duplicate).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn unique_run_directory_is_private_and_cleanup_is_identity_checked() {
        let temp = tempfile::tempdir().unwrap();
        let guard = RunDirectoryGuard::create(temp.path(), 8).unwrap();
        let path = guard.path().to_path_buf();
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(RUN_DIRECTORY_PREFIX));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        guard.finish(()).unwrap();
        assert!(!path.exists());

        let guard = RunDirectoryGuard::create(temp.path(), 8).unwrap();
        let owned = guard.path().to_path_buf();
        let displaced = temp.path().join("displaced-transition-directory");
        fs::rename(&owned, &displaced).unwrap();
        fs::create_dir(&owned).unwrap();
        fs::set_permissions(&owned, fs::Permissions::from_mode(0o700)).unwrap();
        let primary = VeritasmError::new(ErrorCode::IntegrityArtifact, "injected build failure");
        assert_eq!(
            guard.fail::<()>(primary).unwrap_err().code(),
            ErrorCode::ResourceTemporaryBytes
        );
        assert!(owned.is_dir(), "replacement directory must not be removed");
        assert!(displaced.is_dir(), "original directory remains recoverable");
    }

    #[test]
    fn error_cleanup_accepts_only_bounded_private_numeric_runs() {
        let temp = tempfile::tempdir().unwrap();
        let primary = || VeritasmError::new(ErrorCode::IntegrityArtifact, "injected build failure");

        let guard = RunDirectoryGuard::create(temp.path(), 1).unwrap();
        let path = guard.path().to_path_buf();
        let run = run_path(
            &path,
            TransitionRunId {
                generation: 0,
                run_ordinal: 0,
            },
        )
        .unwrap();
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&run)
            .unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .unwrap();
        drop(file);
        assert_eq!(
            guard.fail::<()>(primary()).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
        assert!(!path.exists());

        let guard = RunDirectoryGuard::create(temp.path(), 1).unwrap();
        let path = guard.path().to_path_buf();
        let unknown = path.join("unregistered-entry");
        fs::write(&unknown, b"preserve").unwrap();
        assert_eq!(
            guard.fail::<()>(primary()).unwrap_err().code(),
            ErrorCode::ResourceTemporaryBytes
        );
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve");
        fs::remove_file(unknown).unwrap();
        fs::remove_dir(path).unwrap();

        let guard = RunDirectoryGuard::create(temp.path(), 1).unwrap();
        let path = guard.path().to_path_buf();
        for run_ordinal in 0..2 {
            let run = run_path(
                &path,
                TransitionRunId {
                    generation: 0,
                    run_ordinal,
                },
            )
            .unwrap();
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(run)
                .unwrap();
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        assert_eq!(
            guard.fail::<()>(primary()).unwrap_err().code(),
            ErrorCode::ResourceTemporaryBytes
        );
        assert_eq!(fs::read_dir(&path).unwrap().count(), 2);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn source_coordinate_plane_rejects_cross_key_duplicates_across_spills() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let run_dir = work.join("manual-transition-runs");
        fs::create_dir_all(&run_dir).unwrap();
        let options = options(&work, 3, 1);
        let admitted = validate_options(&options).unwrap();
        let mut state = BuildState::new(&options, admitted, [5; 32], &run_dir).unwrap();

        let mut first = vec![event(b"AAAA", 0, MateRole::S, 0)];
        state.seal_initial(&mut first, 0, 1).unwrap();
        let mut duplicate_coordinate = vec![event(b"AAAC", 0, MateRole::S, 0)];
        assert_eq!(
            state
                .seal_initial(&mut duplicate_coordinate, 1, 2)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
        assert_eq!(state.run_files_created, 1);
    }

    #[test]
    fn source_root_literal_preimage_and_every_descriptor_field_are_bound() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let descriptor = transition_source_descriptor(&spool).unwrap();
        let mut literal = Sha256::new();
        literal.update(SOURCE_DOMAIN);
        literal.update(descriptor.spool_sha256);
        literal.update(descriptor.spool_pretrailer_sha256);
        literal.update(descriptor.spool_schema.to_le_bytes());
        literal.update(descriptor.fragment_count.to_le_bytes());
        literal.update(descriptor.read_count.to_le_bytes());
        literal.update([descriptor.input_mode]);
        literal.update([descriptor.min_base_quality]);
        let expected: [u8; 32] = literal.finalize().into();
        assert_eq!(transition_source_root(&spool).unwrap(), expected);

        let mut variants = Vec::new();
        let mut changed = descriptor;
        changed.spool_sha256[0] ^= 1;
        variants.push(changed);
        let mut changed = descriptor;
        changed.spool_pretrailer_sha256[0] ^= 1;
        variants.push(changed);
        let mut changed = descriptor;
        changed.spool_schema ^= 1;
        variants.push(changed);
        let mut changed = descriptor;
        changed.fragment_count ^= 1;
        variants.push(changed);
        let mut changed = descriptor;
        changed.read_count ^= 1;
        variants.push(changed);
        let mut changed = descriptor;
        changed.input_mode ^= 1;
        variants.push(changed);
        let mut changed = descriptor;
        changed.min_base_quality ^= 1;
        variants.push(changed);
        assert!(variants
            .into_iter()
            .all(|changed| { transition_source_root_from_descriptor(changed) != expected }));
    }

    #[test]
    fn registered_spool_mutation_is_rejected_before_replay_can_validate() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let options = options(&work, 3, 4);
        let ledger = build_transition_ledger(&spool, &options).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(spool.path()).unwrap().permissions();
            permissions.set_mode(0o600);
            fs::set_permissions(spool.path(), permissions).unwrap();
        }
        let mut bytes = fs::read(spool.path()).unwrap();
        let position = bytes
            .windows(6)
            .position(|window| window == b"AAAAAA")
            .unwrap();
        bytes[position] = b'C';
        fs::write(spool.path(), bytes).unwrap();
        let error = validate_transition_ledger_replay(&spool, &options, &ledger).unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegritySpool);
        assert_no_transition_run_directories(&work);
    }

    #[test]
    fn exact_temp_limit_is_inclusive_and_lower_boundary_cleans_up() {
        let temp = tempfile::tempdir().unwrap();
        let spool = single_spool(temp.path(), &[("a", "AAAAAA", None)], 0);
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let mut candidate = options(&work, 3, 32);
        let exact = projected_run_bytes(3).unwrap();
        candidate.limits.max_temp_bytes = exact;
        assert_eq!(
            build_transition_ledger(&spool, &candidate)
                .unwrap()
                .event_count,
            3
        );
        candidate.limits.max_temp_bytes = exact - 1;
        assert_eq!(
            build_transition_ledger(&spool, &candidate)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceTemporaryBytes
        );
        assert_no_transition_run_directories(&work);
    }

    #[test]
    fn empty_accepted_plane_has_a_deterministic_root_and_no_runs() {
        let temp = tempfile::tempdir().unwrap();
        let spool = single_spool(temp.path(), &[("short", "AAA", None)], 0);
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let ledger = build_transition_ledger(&spool, &options(&work, 3, 8)).unwrap();
        assert_eq!(ledger.event_count, 0);
        assert!(ledger.rows.is_empty());
        assert!(ledger.final_runs.is_empty());
        assert_eq!(ledger.run_files_created, 0);
        ledger.view().unwrap();
    }

    #[test]
    fn endpoint_decode_and_wide_transition_boundary_are_exact() {
        let sequence: Vec<u8> = (0..127).map(|index| b"ACGT"[index % 4]).collect();
        let observed = encode_exact_bases(&sequence).unwrap();
        let reverse = reverse_complement(observed, 127).unwrap();
        let canonical = observed.min(reverse);
        let decoded = decode_mer(canonical, 127).unwrap();
        let (prefix, suffix) = transition_endpoints(canonical, 126).unwrap();
        assert_eq!(decode_mer(prefix, 126).unwrap(), decoded[..126]);
        assert_eq!(decode_mer(suffix, 126).unwrap(), decoded[1..]);
        assert!(validate_transition_k_values(&[3, 63, 126]).is_ok());
        assert_eq!(
            validate_transition_k_values(&[3, 3]).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        assert_eq!(
            validate_transition_k_values(&[127]).unwrap_err().code(),
            ErrorCode::ConfigurationInvalidK
        );
    }

    #[test]
    fn run_row_sort_and_open_file_boundaries_fail_without_partial_results() {
        let temp = tempfile::tempdir().unwrap();
        let spool = single_spool(temp.path(), &[("a", "AAAAAA", None)], 0);
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();

        // Three one-event initial runs, one first-generation merge, and one
        // final merge require exactly five created files.
        let mut exact = options(&work, 3, 1);
        exact.limits.max_run_files = 5;
        let ledger = build_transition_ledger(&spool, &exact).unwrap();
        assert_eq!(ledger.run_files_created, 5);
        assert_eq!(ledger.open_files_high_water, 3);
        exact.limits.max_run_files = 4;
        assert_eq!(
            build_transition_ledger(&spool, &exact).unwrap_err().code(),
            ErrorCode::ResourceRunCount
        );
        assert_no_transition_run_directories(&work);

        let mut too_few_open = options(&work, 3, 1);
        too_few_open.limits.max_open_files = 2;
        assert_eq!(
            build_transition_ledger(&spool, &too_few_open)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        let mut too_small_sort = options(&work, 3, 1);
        too_small_sort.limits.sort_buffer_bytes = size_of::<Event>() as u64 - 1;
        assert_eq!(
            build_transition_ledger(&spool, &too_small_sort)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidLimit
        );

        let distinct_root = temp.path().join("distinct");
        fs::create_dir(&distinct_root).unwrap();
        let distinct = single_spool(&distinct_root, &[("d", "AAAACCCC", None)], 0);
        let distinct_work = distinct_root.join("work");
        fs::create_dir(&distinct_work).unwrap();
        let mut row_bound = options(&distinct_work, 3, 32);
        row_bound.limits.max_rows = 5;
        assert_eq!(
            build_transition_ledger(&distinct, &row_bound)
                .unwrap()
                .rows
                .len(),
            5
        );
        row_bound.limits.max_rows = 4;
        assert_eq!(
            build_transition_ledger(&distinct, &row_bound)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRetainedKeys
        );
        assert_no_transition_run_directories(&distinct_work);
    }
}
