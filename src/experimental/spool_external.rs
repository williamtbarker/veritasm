//! Authenticated streaming bridge from `VTSPOOL1` to exact wide-key counts.
//!
//! This module is deliberately absent from the stable CLI. It re-verifies the
//! immutable spool for every pass, reproduces the stable ambiguity and
//! Phred+33 base-quality window rules, and binds all external runs to the
//! complete spool plus both embedded and requested scientific configuration.

use super::external_reduce::{
    build_external_partitioned_routed_key_stream, preflight_external_key_stream,
    ExternalPartitionLimits, ExternalPartitionOptions, ExternalPartitionResult,
};
use super::external_run::{WideRunKind, WideRunSupportUnit};
use super::partitioned_dbg::{route_minimizer, select_minimizer};
use super::transition_witness::{
    transition_source_descriptor, transition_source_root_from_descriptor,
    TransitionSourceDescriptor,
};
use super::wide_kmer::{
    canonical_code, scan_canonical_kmers, scan_canonical_kmers_with_minimizers, validate_code,
    PackedKmer, FUSED_MINIMIZER_SCAN_SCRATCH_BYTES,
};
use crate::config::SupportUnit;
use crate::error::{ErrorCode, Result, VeritasmError};
use crate::model::{Fragment, WindowStats};
use crate::spool::{MemoryBoundedNext, Spool, INTEGRITY_VERIFIER_PHYSICAL_READ_PASSES_PER_CALL};
use sha2::{Digest, Sha256};
use std::mem::size_of;
use std::path::{Path, PathBuf};

const BINDING_DOMAIN: &[u8] = b"veritasm:experimental-spool-external-binding:v1\0";
const SPOOL_READER_BUFFER_BYTES: u64 = 64 * 1024;
const SCIENTIFIC_REPLAY_PASSES: u8 = 2;
const INTEGRITY_VERIFICATION_CALLS: u8 = 3;
const INTEGRITY_PHYSICAL_READ_PASSES: u8 =
    INTEGRITY_VERIFICATION_CALLS * INTEGRITY_VERIFIER_PHYSICAL_READ_PASSES_PER_CALL;
/// Three one-pass verifier calls plus two terminal scientific replays.
pub const TOTAL_PHYSICAL_SPOOL_READ_PASSES: u8 = 5;

/// Explicit limits and scientific choices for one spool replay experiment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolExternalOptions {
    pub work_dir: PathBuf,
    pub k: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub support_unit: SupportUnit,
    /// Maximum decoded allocation admitted for one immutable fragment.
    pub max_fragment_decode_bytes: u64,
    /// Maximum possible windows admitted across both reads of one fragment.
    pub max_fragment_windows: u64,
    /// Whole experiment limits. `max_memory_bytes` includes bridge scratch;
    /// `max_temp_bytes` includes the already-live spool.
    pub limits: ExternalPartitionLimits,
}

/// Source/config binding and exact replay evidence returned by the bridge.
///
/// Fields are intentionally not a public construction or mutation surface;
/// callers obtain one checked immutable view with [`Self::validated`].
///
/// ```compile_fail
/// use veritasm::experimental::spool_external::SpoolExternalResult;
/// # fn cannot_read_unchecked(result: &SpoolExternalResult) {
/// let _ = result.source_root;
/// # }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct SpoolExternalResult {
    /// Path-free spool/QC preimage shared with original-read transition
    /// ledgers.  This is distinct from the child configuration binding below.
    source_descriptor: TransitionSourceDescriptor,
    source_root: [u8; 32],
    spool_sha256: String,
    spool_pretrailer_sha256: String,
    binding_sha256: [u8; 32],
    support_unit: SupportUnit,
    window_stats: WindowStats,
    support_events: u64,
    /// QC/key replay traversals, excluding the integrity traversal performed
    /// by `Spool::verify` before each iterator is opened.
    scientific_replay_passes: u8,
    /// Logical verifier invocations: one explicit call and one for each of the
    /// two authenticated scientific replay iterators.
    integrity_verification_calls: u8,
    /// Physical full-file-equivalent reads performed inside those verifier
    /// calls. Each verifier hashes both digest preimages while structurally
    /// decoding one forward descriptor traversal.
    integrity_physical_read_passes: u8,
    temporary_bytes_high_water_including_spool: u64,
    open_files_high_water_including_spool: u16,
    external: ExternalPartitionResult,
}

/// A checked, immutable view of one source-produced spool/external artifact.
/// The view cannot be constructed without validating every exact-count row.
#[derive(Debug, Clone, Copy)]
pub struct SpoolExternalView<'a> {
    result: &'a SpoolExternalResult,
}

impl SpoolExternalResult {
    /// Validate the complete source/configuration binding, exact table,
    /// conservation, and operational ledger before exposing borrowed data.
    pub fn validated(&self) -> Result<SpoolExternalView<'_>> {
        validate_spool_external_result(self)?;
        Ok(SpoolExternalView { result: self })
    }
}

impl<'a> SpoolExternalView<'a> {
    pub const fn source_descriptor(self) -> TransitionSourceDescriptor {
        self.result.source_descriptor
    }

    pub const fn source_root(self) -> [u8; 32] {
        self.result.source_root
    }

    pub fn spool_sha256(self) -> &'a str {
        &self.result.spool_sha256
    }

    pub fn spool_pretrailer_sha256(self) -> &'a str {
        &self.result.spool_pretrailer_sha256
    }

    pub const fn binding_sha256(self) -> [u8; 32] {
        self.result.binding_sha256
    }

    pub const fn support_unit(self) -> SupportUnit {
        self.result.support_unit
    }

    pub const fn window_stats(self) -> &'a WindowStats {
        &self.result.window_stats
    }

    pub const fn support_events(self) -> u64 {
        self.result.support_events
    }

    pub const fn scientific_replay_passes(self) -> u8 {
        self.result.scientific_replay_passes
    }

    pub const fn integrity_verification_calls(self) -> u8 {
        self.result.integrity_verification_calls
    }

    pub const fn integrity_physical_read_passes(self) -> u8 {
        self.result.integrity_physical_read_passes
    }

    pub const fn total_physical_spool_read_passes(self) -> u8 {
        TOTAL_PHYSICAL_SPOOL_READ_PASSES
    }

    pub const fn temporary_bytes_high_water_including_spool(self) -> u64 {
        self.result.temporary_bytes_high_water_including_spool
    }

    pub const fn open_files_high_water_including_spool(self) -> u16 {
        self.result.open_files_high_water_including_spool
    }

    /// Borrow the exact materialized result. Arbitrary independently-created
    /// `ExternalPartitionResult` values are unverified adapters; only this
    /// checked borrow carries the spool-backed source capability.
    pub const fn external_counts(self) -> &'a ExternalPartitionResult {
        &self.result.external
    }
}

/// Re-verify and replay one immutable spool into exact external counts.
///
/// Occurrence mode emits every accepted read window. Fragment mode sorts and
/// deduplicates complete keys across all reads of one supplied fragment before
/// emitting them, so mate overlap, repeated windows, and QC-separated exact
/// regions cannot increment one key more than once for that fragment.
pub fn build_spool_external_counts(
    spool: &Spool,
    options: &SpoolExternalOptions,
) -> Result<SpoolExternalResult> {
    validate_bridge_options(options)?;
    let source_descriptor = transition_source_descriptor(spool)?;
    let source_root = transition_source_root_from_descriptor(source_descriptor);
    let scan_memory = bridge_scan_memory(spool, options)?;
    if SPOOL_READER_BUFFER_BYTES > options.limits.max_memory_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "spool integrity-verification reader buffer requires {SPOOL_READER_BUFFER_BYTES} bytes, exceeding limit {}",
                options.limits.max_memory_bytes
            ),
        ));
    }

    // These checks depend only on declared limits and immutable header
    // metadata. They deliberately run before the first physical spool read so
    // an impossible experiment cannot consume I/O before admission.
    let preflight_options = external_options_with_work_dir(spool, options, PathBuf::new());
    preflight_external_key_stream(
        &preflight_options,
        &options.work_dir,
        spool.read_count,
        spool.stats.bases,
        scan_memory,
    )?;
    let mut external_options = external_options_for(spool, options)?;

    spool.verify()?;
    let spool_bytes = spool.registered_byte_len();
    if spool_bytes >= options.limits.max_temp_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!(
                "verified spool uses {spool_bytes} bytes, leaving no external-run allowance under total temporary limit {}",
                options.limits.max_temp_bytes
            ),
        ));
    }

    let binding = external_options.source_identity;
    external_options.limits.max_temp_bytes = external_options
        .limits
        .max_temp_bytes
        .checked_sub(spool_bytes)
        .ok_or_else(|| overflow("spool/external temporary-byte allowance underflow"))?;

    let (expected_stats, expected_events) =
        replay_spool(spool, options, None, "first spool evidence pass")?;
    enforce_window_limit(expected_stats.possible, options.limits.max_windows)?;

    let mut replay_stats = WindowStats::default();
    let mut replay_events = 0_u64;
    let external = build_external_partitioned_routed_key_stream(
        &external_options,
        spool.read_count,
        spool.stats.bases,
        expected_events,
        scan_memory,
        scan_memory,
        |emit| {
            let (stats, events) =
                replay_spool(spool, options, Some(emit), "second spool count pass")?;
            replay_stats = stats;
            replay_events = events;
            if replay_stats != expected_stats || replay_events != expected_events {
                return Err(VeritasmError::new(
                    ErrorCode::IntegritySpool,
                    "verified spool replay changed between evidence and count passes",
                ));
            }
            Ok(())
        },
    )?;
    let temporary_bytes_high_water_including_spool = spool_bytes
        .checked_add(external.temporary_bytes_high_water)
        .ok_or_else(|| overflow("spool/external temporary high-water overflow"))?;
    if temporary_bytes_high_water_including_spool > options.limits.max_temp_bytes {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            "spool/external temporary high-water exceeded its admitted limit",
        ));
    }
    // Conservatively charge one spool descriptor concurrently with at least
    // one external-run descriptor. The one-pass verifier itself opens only
    // the registered spool descriptor.
    let open_files_high_water_including_spool = external.open_files_high_water.max(2);
    if open_files_high_water_including_spool > options.limits.max_open_files {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            "spool/external open-file high-water exceeded its admitted limit",
        ));
    }
    let spool_sha256 = clone_admitted_string(&spool.sha256, "spool SHA-256 result")?;
    let spool_pretrailer_sha256 =
        clone_admitted_string(&spool.pretrailer_sha256, "spool pre-trailer SHA-256 result")?;
    let result = SpoolExternalResult {
        source_descriptor,
        source_root,
        spool_sha256,
        spool_pretrailer_sha256,
        binding_sha256: binding,
        support_unit: options.support_unit,
        window_stats: expected_stats,
        support_events: expected_events,
        scientific_replay_passes: SCIENTIFIC_REPLAY_PASSES,
        integrity_verification_calls: INTEGRITY_VERIFICATION_CALLS,
        integrity_physical_read_passes: INTEGRITY_PHYSICAL_READ_PASSES,
        temporary_bytes_high_water_including_spool,
        open_files_high_water_including_spool,
        external,
    };
    result.validated()?;
    Ok(result)
}

fn validate_spool_external_result(input: &SpoolExternalResult) -> Result<()> {
    if transition_source_root_from_descriptor(input.source_descriptor) != input.source_root {
        return spool_integrity("spool bridge source root does not match its descriptor");
    }
    if !matches_lower_hex(
        input.spool_sha256.as_bytes(),
        &input.source_descriptor.spool_sha256,
    ) || !matches_lower_hex(
        input.spool_pretrailer_sha256.as_bytes(),
        &input.source_descriptor.spool_pretrailer_sha256,
    ) {
        return spool_integrity("spool bridge registered digests do not match its descriptor");
    }
    let expected_reads = match input.source_descriptor.input_mode {
        0 => input.source_descriptor.fragment_count,
        1 => input
            .source_descriptor
            .fragment_count
            .checked_mul(2)
            .ok_or_else(|| overflow("paired spool descriptor read-count overflow"))?,
        _ => return spool_integrity("spool bridge descriptor has an invalid input mode"),
    };
    if input.source_descriptor.read_count != expected_reads {
        return spool_integrity("spool bridge read and fragment counts are inconsistent");
    }

    let classified = input
        .window_stats
        .accepted
        .checked_add(input.window_stats.ambiguity_only)
        .and_then(|value| value.checked_add(input.window_stats.quality_only))
        .and_then(|value| value.checked_add(input.window_stats.ambiguity_and_quality))
        .ok_or_else(|| overflow("spool bridge window accounting overflow"))?;
    if classified != input.window_stats.possible {
        return spool_integrity("spool bridge window accounting is not exhaustive");
    }

    let expected_unit = match input.support_unit {
        SupportUnit::SuppliedFragmentInstance => WideRunSupportUnit::SuppliedFragmentInstance,
        SupportUnit::AcceptedWindowOccurrence => WideRunSupportUnit::AcceptedWindowOccurrence,
    };
    let external = &input.external;
    super::wide_kmer::validate_k(external.k)
        .map_err(|_| spool_integrity_error("spool bridge external k is invalid"))?;
    if external.minimizer_length == 0
        || external.minimizer_length > external.k
        || external.virtual_bucket_count == 0
        || external.source_identity != input.binding_sha256
        || external.support_unit != expected_unit
        || external.support_events != input.support_events
    {
        return spool_integrity("spool bridge and external scientific metadata disagree");
    }
    let edge_count = u64::try_from(external.edge_counts.len())
        .map_err(|_| overflow("external exact-key count does not fit u64"))?;
    if external.distinct_kmers != edge_count {
        return spool_integrity("external distinct-key total differs from its exact table");
    }
    let mut previous_route = None;
    let mut edge_support = 0_u64;
    for row in &external.edge_counts {
        if row.support == 0 {
            return spool_integrity("external exact-count row has zero support");
        }
        validate_code(row.key, external.k)
            .map_err(|_| spool_integrity_error("external exact-count key is invalid"))?;
        if canonical_code(row.key, external.k).map_err(|_| {
            spool_integrity_error("external exact-count key cannot be canonicalized")
        })? != row.key
        {
            return spool_integrity("external exact-count key is not canonical");
        }
        let owner = select_minimizer(row.key, external.k, external.minimizer_length)
            .map_err(|_| spool_integrity_error("external minimizer cannot be recomputed"))?;
        let bucket = route_minimizer(
            owner.key,
            external.minimizer_length,
            external.virtual_bucket_count,
        )
        .map_err(|_| spool_integrity_error("external route cannot be recomputed"))?;
        if row.minimizer != owner.key || row.bucket_id != bucket {
            return spool_integrity("external exact-count row has an incorrect route");
        }
        let route = (row.bucket_id, row.key);
        if previous_route.is_some_and(|previous| previous >= route) {
            return spool_integrity("external exact-count table is not strictly route ordered");
        }
        previous_route = Some(route);
        edge_support = edge_support
            .checked_add(row.support)
            .ok_or_else(|| overflow("external exact-count support overflow"))?;
    }
    if edge_support != external.support_events {
        return spool_integrity("external exact-count support is not conserved");
    }

    let mut previous_bucket = None;
    let mut final_records = 0_u64;
    let mut final_support = 0_u64;
    let mut final_bytes = 0_u64;
    for run in &external.final_runs {
        let domain = run.header.domain;
        if run.header.kind != WideRunKind::Reduced
            || domain.source_identity != external.source_identity
            || domain.support_unit != external.support_unit
            || domain.k != external.k
            || domain.minimizer_length != external.minimizer_length
            || domain.virtual_bucket_count != external.virtual_bucket_count
            || previous_bucket.is_some_and(|previous| previous >= domain.virtual_bucket)
        {
            return spool_integrity("external final-run summary has an inconsistent domain");
        }
        previous_bucket = Some(domain.virtual_bucket);
        final_records = final_records
            .checked_add(run.header.record_count)
            .ok_or_else(|| overflow("external final-run record total overflow"))?;
        final_support = final_support
            .checked_add(run.header.support_mass)
            .ok_or_else(|| overflow("external final-run support total overflow"))?;
        final_bytes = final_bytes
            .checked_add(run.byte_len)
            .ok_or_else(|| overflow("external final-run byte total overflow"))?;
    }
    if final_records != external.distinct_kmers || final_support != external.support_events {
        return spool_integrity("external final-run summaries do not conserve the exact table");
    }
    if final_bytes > external.temporary_bytes_high_water {
        return spool_integrity("external final-run bytes exceed reported temporary high-water");
    }
    for replacement in &external.replacements {
        if replacement.predecessor_sha256.is_empty()
            || replacement.support_mass == 0
            || replacement.reclaimed_bytes == 0
            || !replacement.predecessors_reclaimed
        {
            return spool_integrity("external replacement ledger is incomplete");
        }
    }
    let final_run_count = u64::try_from(external.final_runs.len())
        .map_err(|_| overflow("external final-run count does not fit u64"))?;
    let replacement_count = u64::try_from(external.replacements.len())
        .map_err(|_| overflow("external replacement count does not fit u64"))?;
    if external.run_files_created < final_run_count
        || external.run_files_created < replacement_count
        || external.temporary_bytes_final != 0
        || (external.run_files_created > 0 && external.open_files_high_water == 0)
    {
        return spool_integrity("external operational ledger is inconsistent");
    }

    match input.support_unit {
        SupportUnit::AcceptedWindowOccurrence => {
            if input.support_events != input.window_stats.accepted {
                return spool_integrity("occurrence support differs from accepted spool windows");
            }
        }
        SupportUnit::SuppliedFragmentInstance => {
            if input.support_events > input.window_stats.accepted {
                return spool_integrity("fragment support exceeds accepted spool windows");
            }
        }
    }
    let total_physical_passes = input
        .scientific_replay_passes
        .checked_add(input.integrity_physical_read_passes)
        .ok_or_else(|| overflow("spool physical traversal count overflow"))?;
    if input.scientific_replay_passes != SCIENTIFIC_REPLAY_PASSES
        || input.integrity_verification_calls != INTEGRITY_VERIFICATION_CALLS
        || input.integrity_physical_read_passes != INTEGRITY_PHYSICAL_READ_PASSES
        || total_physical_passes != TOTAL_PHYSICAL_SPOOL_READ_PASSES
    {
        return spool_integrity("spool bridge under-reports its physical traversal contract");
    }
    if input.temporary_bytes_high_water_including_spool < external.temporary_bytes_high_water
        || input.open_files_high_water_including_spool < external.open_files_high_water.max(2)
    {
        return spool_integrity("spool bridge operational accounting is inconsistent");
    }
    Ok(())
}

fn matches_lower_hex(encoded: &[u8], expected: &[u8; 32]) -> bool {
    if encoded.len() != 64 {
        return false;
    }
    for (index, byte) in expected.iter().enumerate() {
        let high = encoded[index * 2];
        let low = encoded[index * 2 + 1];
        if high != lower_hex_nibble(byte >> 4) || low != lower_hex_nibble(byte & 0x0f) {
            return false;
        }
    }
    true
}

const fn lower_hex_nibble(value: u8) -> u8 {
    if value < 10 {
        b'0' + value
    } else {
        b'a' + (value - 10)
    }
}

fn spool_integrity<T>(context: impl Into<String>) -> Result<T> {
    Err(spool_integrity_error(context))
}

fn spool_integrity_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegritySpool, context)
}

fn external_options_for(
    spool: &Spool,
    options: &SpoolExternalOptions,
) -> Result<ExternalPartitionOptions> {
    let work_dir = clone_admitted_path(&options.work_dir)?;
    Ok(external_options_with_work_dir(spool, options, work_dir))
}

fn external_options_with_work_dir(
    spool: &Spool,
    options: &SpoolExternalOptions,
    work_dir: PathBuf,
) -> ExternalPartitionOptions {
    ExternalPartitionOptions {
        work_dir,
        k: options.k,
        minimizer_length: options.minimizer_length,
        virtual_bucket_count: options.virtual_bucket_count,
        source_identity: spool_external_binding(spool, options),
        support_unit: match options.support_unit {
            SupportUnit::SuppliedFragmentInstance => WideRunSupportUnit::SuppliedFragmentInstance,
            SupportUnit::AcceptedWindowOccurrence => WideRunSupportUnit::AcceptedWindowOccurrence,
        },
        limits: options.limits,
    }
}

fn replay_spool(
    spool: &Spool,
    options: &SpoolExternalOptions,
    mut emit: Option<&mut dyn FnMut(PackedKmer, PackedKmer) -> Result<()>>,
    pass: &'static str,
) -> Result<(WindowStats, u64)> {
    let mut iterator = spool.iter()?;
    let mut stats = WindowStats::default();
    let mut support_events = 0_u64;
    loop {
        let fragment =
            match iterator.next_with_decode_memory_limit(options.max_fragment_decode_bytes)? {
                MemoryBoundedNext::Fragment { fragment, .. } => fragment,
                MemoryBoundedNext::End => break,
                MemoryBoundedNext::RequiresMemory(required) => {
                    return Err(VeritasmError::new(
                        ErrorCode::ResourceMemory,
                        format!(
                            "{pass} needs {required} decoded-fragment bytes, exceeding limit {}",
                            options.max_fragment_decode_bytes
                        ),
                    ));
                }
            };
        let (fragment_stats, fragment_events) = if let Some(sink) = emit.as_mut() {
            scan_fragment_wide_routed(&fragment, spool, options, *sink)?
        } else {
            scan_fragment_wide(&fragment, spool, options)?
        };
        add_stats(&mut stats, &fragment_stats)?;
        enforce_window_limit(stats.possible, options.limits.max_windows)?;
        support_events = checked_add(
            support_events,
            fragment_events,
            "spool external support-event overflow",
        )?;
    }
    Ok((stats, support_events))
}

fn scan_fragment_wide(
    fragment: &Fragment,
    spool: &Spool,
    options: &SpoolExternalOptions,
) -> Result<(WindowStats, u64)> {
    let possible = fragment_possible_windows(fragment, options)?;
    let capacity = usize::try_from(possible).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            "fragment windows do not fit usize",
        )
    })?;
    let mut fragment_keys = Vec::new();
    if options.support_unit == SupportUnit::SuppliedFragmentInstance {
        fragment_keys.try_reserve_exact(capacity).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve fragment-wide exact-key set: {cause}"),
            )
        })?;
        enforce_exact_capacity(
            fragment_keys.capacity(),
            capacity,
            "spool fragment-distinct key vector",
        )?;
    }
    let mut stats = WindowStats::default();
    let mut emitted = 0_u64;
    for read in &fragment.reads {
        let scan = scan_canonical_kmers(
            &read.sequence,
            read.quality.as_deref(),
            options.k,
            spool.scientific_config().min_base_quality,
        )?;
        let read_windows = read
            .sequence
            .len()
            .saturating_sub(usize::from(options.k).saturating_sub(1));
        enforce_exact_capacity(
            scan.kmers.capacity(),
            read_windows,
            "spool read-window key vector",
        )?;
        add_stats(&mut stats, &scan.stats)?;
        match options.support_unit {
            SupportUnit::AcceptedWindowOccurrence => {
                emitted = checked_add(
                    emitted,
                    u64::try_from(scan.kmers.len())
                        .map_err(|_| overflow("occurrence event count does not fit in u64"))?,
                    "occurrence event overflow",
                )?;
            }
            SupportUnit::SuppliedFragmentInstance => fragment_keys.extend(scan.kmers),
        }
    }
    if options.support_unit == SupportUnit::SuppliedFragmentInstance {
        fragment_keys.sort_unstable();
        fragment_keys.dedup();
        emitted = u64::try_from(fragment_keys.len())
            .map_err(|_| overflow("fragment-distinct key count does not fit u64"))?;
    }
    Ok((stats, emitted))
}

fn scan_fragment_wide_routed(
    fragment: &Fragment,
    spool: &Spool,
    options: &SpoolExternalOptions,
    emit: &mut dyn FnMut(PackedKmer, PackedKmer) -> Result<()>,
) -> Result<(WindowStats, u64)> {
    let possible = fragment_possible_windows(fragment, options)?;
    let capacity = usize::try_from(possible).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            "fragment windows do not fit usize",
        )
    })?;
    let mut fragment_keys: Vec<(PackedKmer, PackedKmer)> = Vec::new();
    if options.support_unit == SupportUnit::SuppliedFragmentInstance {
        fragment_keys.try_reserve_exact(capacity).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve fragment-wide routed exact-key set: {cause}"),
            )
        })?;
        enforce_exact_capacity(
            fragment_keys.capacity(),
            capacity,
            "spool fragment-distinct routed-key vector",
        )?;
    }

    let mut stats = WindowStats::default();
    let mut emitted = 0_u64;
    for read in &fragment.reads {
        let read_stats = scan_canonical_kmers_with_minimizers(
            &read.sequence,
            read.quality.as_deref(),
            options.k,
            options.minimizer_length,
            spool.scientific_config().min_base_quality,
            |key, minimizer| {
                match options.support_unit {
                    SupportUnit::AcceptedWindowOccurrence => {
                        emit(key, minimizer)?;
                        emitted = checked_add(emitted, 1, "routed occurrence event overflow")?;
                    }
                    SupportUnit::SuppliedFragmentInstance => {
                        fragment_keys.push((key, minimizer));
                    }
                }
                Ok(())
            },
        )?;
        add_stats(&mut stats, &read_stats)?;
    }

    if options.support_unit == SupportUnit::SuppliedFragmentInstance {
        fragment_keys.sort_unstable();
        for adjacent in fragment_keys.windows(2) {
            if adjacent[0].0 == adjacent[1].0 && adjacent[0].1 != adjacent[1].1 {
                return Err(VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "one fragment exact key acquired two fused minimizer owners",
                ));
            }
        }
        fragment_keys.dedup_by_key(|row| row.0);
        emitted = u64::try_from(fragment_keys.len())
            .map_err(|_| overflow("fragment-distinct routed-key count does not fit u64"))?;
        for (key, minimizer) in fragment_keys {
            emit(key, minimizer)?;
        }
    }
    Ok((stats, emitted))
}

fn fragment_possible_windows(fragment: &Fragment, options: &SpoolExternalOptions) -> Result<u64> {
    let mut possible = 0_u64;
    for read in &fragment.reads {
        let windows = read
            .sequence
            .len()
            .saturating_sub(usize::from(options.k).saturating_sub(1));
        possible = checked_add(
            possible,
            u64::try_from(windows)
                .map_err(|_| overflow("fragment window count does not fit in u64"))?,
            "fragment possible-window overflow",
        )?;
    }
    if possible > options.max_fragment_windows {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "fragment {} has {possible} possible windows, exceeding per-fragment limit {}",
                fragment.ordinal, options.max_fragment_windows
            ),
        ));
    }
    Ok(possible)
}

fn bridge_scan_memory(spool: &Spool, options: &SpoolExternalOptions) -> Result<u64> {
    let key_bytes = u64::try_from(size_of::<PackedKmer>())
        .map_err(|_| overflow("wide key width does not fit u64"))?;
    let oracle_scan_vectors = options
        .max_fragment_windows
        .checked_mul(key_bytes)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| overflow("spool replay scan-vector allowance overflow"))?;
    let routed_key_bytes = u64::try_from(size_of::<(PackedKmer, PackedKmer)>())
        .map_err(|_| overflow("routed wide key width does not fit u64"))?;
    let routed_scan_vector = options
        .max_fragment_windows
        .checked_mul(routed_key_bytes)
        .ok_or_else(|| overflow("spool replay routed-vector allowance overflow"))?;
    let scan_vectors = oracle_scan_vectors.max(routed_scan_vector);
    let word_bits = usize::BITS as u64;
    let ring_words = u64::from(options.k)
        .checked_add(word_bits - 1)
        .ok_or_else(|| overflow("spool replay rolling-ring word count overflow"))?
        / word_bits;
    let ring_bytes = ring_words
        .checked_mul(size_of::<usize>() as u64)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| overflow("spool replay rolling-ring allowance overflow"))?;
    let work_dir_bytes = u64::try_from(options.work_dir.as_os_str().as_encoded_bytes().len())
        .map_err(|_| overflow("spool replay work-directory path does not fit u64"))?;
    let result_digest_bytes = spool
        .sha256
        .len()
        .checked_add(spool.pretrailer_sha256.len())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| overflow("spool result digest length overflow"))?;
    options
        .max_fragment_decode_bytes
        .checked_add(scan_vectors)
        .and_then(|bytes| bytes.checked_add(SPOOL_READER_BUFFER_BYTES))
        .and_then(|bytes| bytes.checked_add(ring_bytes))
        .and_then(|bytes| bytes.checked_add(FUSED_MINIMIZER_SCAN_SCRATCH_BYTES))
        .and_then(|bytes| bytes.checked_add(work_dir_bytes))
        .and_then(|bytes| bytes.checked_add(result_digest_bytes))
        .ok_or_else(|| overflow("spool replay memory allowance overflow"))
}

fn clone_admitted_path(path: &Path) -> Result<PathBuf> {
    let admitted = path.as_os_str().as_encoded_bytes().len();
    let mut copy = PathBuf::new();
    copy.try_reserve_exact(admitted).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve spool external work-directory path: {cause}"),
        )
    })?;
    copy.push(path);
    enforce_exact_capacity(
        copy.capacity(),
        admitted,
        "spool external work-directory path",
    )?;
    Ok(copy)
}

fn clone_admitted_string(value: &str, label: &'static str) -> Result<String> {
    let mut copy = String::new();
    copy.try_reserve_exact(value.len()).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve {label}: {cause}"),
        )
    })?;
    copy.push_str(value);
    enforce_exact_capacity(copy.capacity(), value.len(), label)?;
    Ok(copy)
}

fn enforce_exact_capacity(actual: usize, admitted: usize, label: &'static str) -> Result<()> {
    if actual <= admitted {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "allocator returned {label} capacity {actual} above admitted capacity {admitted}"
            ),
        ))
    }
}

fn validate_bridge_options(options: &SpoolExternalOptions) -> Result<()> {
    super::wide_kmer::validate_k(options.k)?;
    if options.max_fragment_decode_bytes == 0 || options.max_fragment_windows == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "spool external fragment decode and window limits must be nonzero",
        ));
    }
    if options.limits.max_open_files < 3 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "spool external counting requires at least three open-file slots",
        ));
    }
    Ok(())
}

fn spool_external_binding(spool: &Spool, options: &SpoolExternalOptions) -> [u8; 32] {
    let scientific = spool.scientific_config();
    let mut digest = Sha256::new();
    digest.update(BINDING_DOMAIN);
    digest.update(spool.sha256.as_bytes());
    digest.update(spool.pretrailer_sha256.as_bytes());
    digest.update(spool.fragment_count.to_le_bytes());
    digest.update(spool.read_count.to_le_bytes());
    digest.update([spool.input_mode_tag()]);
    digest.update([scientific.k]);
    digest.update([scientific.profile as u8]);
    digest.update([scientific.support_unit.tag()]);
    digest.update(scientific.min_support.to_le_bytes());
    digest.update([scientific.min_base_quality]);
    digest.update([u8::from(scientific.remap)]);
    digest.update([options.k]);
    digest.update([options.minimizer_length]);
    digest.update(options.virtual_bucket_count.to_le_bytes());
    digest.update([options.support_unit.tag()]);
    digest.finalize().into()
}

fn enforce_window_limit(observed: u64, limit: u64) -> Result<()> {
    if observed <= limit {
        Ok(())
    } else {
        Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            format!("spool replay possible windows {observed} exceeds limit {limit}"),
        ))
    }
}

fn add_stats(total: &mut WindowStats, addend: &WindowStats) -> Result<()> {
    total.possible = checked_add(total.possible, addend.possible, "possible-window overflow")?;
    total.accepted = checked_add(total.accepted, addend.accepted, "accepted-window overflow")?;
    total.ambiguity_only = checked_add(
        total.ambiguity_only,
        addend.ambiguity_only,
        "ambiguity-only window overflow",
    )?;
    total.quality_only = checked_add(
        total.quality_only,
        addend.quality_only,
        "quality-only window overflow",
    )?;
    total.ambiguity_and_quality = checked_add(
        total.ambiguity_and_quality,
        addend.ambiguity_and_quality,
        "ambiguity-and-quality window overflow",
    )?;
    Ok(())
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn overflow(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig};
    use crate::dna::scan_fragment;
    use crate::experimental::wide_kmer::{canonical_code, encode_kmer};
    use crate::model::KmerCount;
    use crate::spool::{
        completed_spool_read_passes, create_spool, reset_completed_spool_read_passes,
    };
    use std::collections::BTreeMap;
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

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

    fn external_limits() -> ExternalPartitionLimits {
        ExternalPartitionLimits {
            max_segments: 100,
            max_input_bases: 1_000_000,
            max_windows: 1_000_000,
            max_distinct_kmers: 1_000_000,
            max_memory_bytes: 16 * 1024 * 1024,
            sort_buffer_bytes: 4 * 1024,
            io_buffer_bytes: 128,
            max_temp_bytes: 32 * 1024 * 1024,
            max_run_files: 1_000,
            merge_fan_in: 2,
            max_open_files: 3,
        }
    }

    fn options(work_dir: &Path, support_unit: SupportUnit, k: u8) -> SpoolExternalOptions {
        SpoolExternalOptions {
            work_dir: work_dir.to_path_buf(),
            k,
            minimizer_length: 3,
            virtual_bucket_count: 5,
            support_unit,
            max_fragment_decode_bytes: 1 << 20,
            max_fragment_windows: 10_000,
            limits: external_limits(),
        }
    }

    fn has_external_run_directory(work_dir: &Path) -> bool {
        fs::read_dir(work_dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .as_encoded_bytes()
                .starts_with(b"experimental-wide-runs-")
        })
    }

    fn paired_spool(root: &Path) -> Spool {
        fs::create_dir_all(root).unwrap();
        let r1 = root.join("r1.fastq");
        let r2 = root.join("r2.fastq");
        fs::write(
            &r1,
            b"@dup/1\nAAAAANAAAAA\n+\nIIIIIIIIIII\n@dup/1\nACGTACGTACG\n+\nIIIII!IIIII\n",
        )
        .unwrap();
        fs::write(
            &r2,
            b"@dup/2\nTTTTTNTTTTT\n+\nIIII!IIIIII\n@dup/2\nCGTACGTACGT\n+\nIIIIIIIIIII\n",
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

    fn make_spool_writable(path: &Path) {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn stable_oracle(
        spool: &Spool,
        support_unit: SupportUnit,
        k: u8,
    ) -> (Vec<KmerCount>, WindowStats) {
        let mut counts = BTreeMap::new();
        let mut stats = WindowStats::default();
        for fragment in spool.iter().unwrap() {
            let scan = scan_fragment(
                &fragment.unwrap(),
                k,
                spool.scientific_config().min_base_quality,
                support_unit,
            )
            .unwrap();
            add_stats(&mut stats, &scan.stats).unwrap();
            for key in scan.kmers {
                *counts.entry(key).or_insert(0_u64) += 1;
            }
        }
        (
            counts
                .into_iter()
                .map(|(key, support)| KmerCount { key, support })
                .collect(),
            stats,
        )
    }

    fn narrow_result(result: &SpoolExternalResult) -> Vec<KmerCount> {
        let mut counts: Vec<_> = result
            .external
            .edge_counts
            .iter()
            .map(|row| KmerCount {
                key: row.key.as_u128().unwrap(),
                support: row.support,
            })
            .collect();
        counts.sort_unstable();
        counts
    }

    #[test]
    fn narrow_occurrence_and_fragment_modes_match_stable_scan_and_preserve_qc_ledger() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        for support in [
            SupportUnit::AcceptedWindowOccurrence,
            SupportUnit::SuppliedFragmentInstance,
        ] {
            let work = temp.path().join(format!("work-{}", support.tag()));
            fs::create_dir(&work).unwrap();
            reset_completed_spool_read_passes();
            let result = build_spool_external_counts(&spool, &options(&work, support, 5)).unwrap();
            assert_eq!(completed_spool_read_passes(), (3, 2));
            let (oracle, stats) = stable_oracle(&spool, support, 5);
            assert_eq!(narrow_result(&result), oracle);
            assert_eq!(result.window_stats, stats);
            assert_eq!(
                result.support_events,
                oracle.iter().map(|row| row.support).sum::<u64>()
            );
            assert_eq!(result.external.support_events, result.support_events);
            assert_eq!(result.external.support_unit as u8, support.tag());
            assert_eq!(result.scientific_replay_passes, 2);
            assert_eq!(result.integrity_verification_calls, 3);
            assert_eq!(result.integrity_physical_read_passes, 3);
            assert_eq!(result.external.temporary_bytes_final, 0);
            assert!(!has_external_run_directory(&work));
            assert!(stats.ambiguity_only > 0);
            assert!(stats.quality_only > 0);
            assert!(stats.ambiguity_and_quality > 0);
            assert_eq!(
                stats.possible,
                stats.accepted
                    + stats.ambiguity_only
                    + stats.quality_only
                    + stats.ambiguity_and_quality
            );
        }
    }

    #[test]
    fn validated_view_reports_all_five_physical_read_passes_and_rejects_tampering() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let work = temp.path().join("view-work");
        fs::create_dir(&work).unwrap();
        let mut result = build_spool_external_counts(
            &spool,
            &options(&work, SupportUnit::AcceptedWindowOccurrence, 5),
        )
        .unwrap();
        let view = result.validated().unwrap();
        assert_eq!(view.scientific_replay_passes(), 2);
        assert_eq!(view.integrity_verification_calls(), 3);
        assert_eq!(view.integrity_physical_read_passes(), 3);
        assert_eq!(view.total_physical_spool_read_passes(), 5);
        assert_eq!(view.external_counts().support_events, view.support_events());

        result.source_descriptor.spool_sha256[0] ^= 1;
        assert_eq!(
            result.validated().unwrap_err().code(),
            ErrorCode::IntegritySpool
        );
        result.source_descriptor.spool_sha256[0] ^= 1;
        result.external.edge_counts[0].support += 1;
        assert_eq!(
            result.validated().unwrap_err().code(),
            ErrorCode::IntegritySpool
        );
        result.external.edge_counts[0].support -= 1;
        result.integrity_physical_read_passes = 2;
        assert_eq!(
            result.validated().unwrap_err().code(),
            ErrorCode::IntegritySpool
        );
    }

    #[test]
    fn duplicate_identifiers_are_distinct_fragments_but_mate_and_repeat_keys_are_deduplicated() {
        let temp = tempfile::tempdir().unwrap();
        let r1 = temp.path().join("r1.fastq");
        let r2 = temp.path().join("r2.fastq");
        fs::write(
            &r1,
            b"@same/1\nAAAAAAAA\n+\nIIIIIIII\n@same/1\nAAAAAAAA\n+\nIIIIIIII\n",
        )
        .unwrap();
        fs::write(
            &r2,
            b"@same/2\nTTTTTTTT\n+\nIIIIIIII\n@same/2\nTTTTTTTT\n+\nIIIIIIII\n",
        )
        .unwrap();
        let spool = create_spool(
            &InputSpec::Paired {
                read1: vec![r1],
                read2: vec![r2],
            },
            &scientific(0),
            &spool_limits(),
            temp.path(),
        )
        .unwrap();
        let work = temp.path().join("fragment-work");
        fs::create_dir(&work).unwrap();
        let result = build_spool_external_counts(
            &spool,
            &options(&work, SupportUnit::SuppliedFragmentInstance, 5),
        )
        .unwrap();
        assert_eq!(result.window_stats.accepted, 16);
        assert_eq!(result.support_events, 2);
        assert_eq!(result.external.edge_counts.len(), 1);
        assert_eq!(result.external.edge_counts[0].support, 2);
    }

    #[test]
    fn wide_k_matches_literal_source_window_enumeration() {
        let temp = tempfile::tempdir().unwrap();
        let fasta = temp.path().join("wide.fasta");
        let sequence = b"ACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGT";
        let mut bytes = b">wide\n".to_vec();
        bytes.extend_from_slice(sequence);
        bytes.push(b'\n');
        fs::write(&fasta, bytes).unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![fasta]),
            &scientific(0),
            &spool_limits(),
            temp.path(),
        )
        .unwrap();
        let work = temp.path().join("wide-work");
        fs::create_dir(&work).unwrap();
        let result = build_spool_external_counts(
            &spool,
            &options(&work, SupportUnit::AcceptedWindowOccurrence, 65),
        )
        .unwrap();
        let mut oracle = BTreeMap::new();
        for window in sequence.windows(65) {
            let key = canonical_code(encode_kmer(window).unwrap(), 65).unwrap();
            *oracle.entry(key).or_insert(0_u64) += 1;
        }
        let observed: BTreeMap<_, _> = result
            .external
            .edge_counts
            .iter()
            .map(|row| (row.key, row.support))
            .collect();
        assert_eq!(observed, oracle);
        assert_eq!(
            result.window_stats.accepted,
            (sequence.len() - 65 + 1) as u64
        );
    }

    #[test]
    fn spill_batching_and_thread_count_do_not_change_exact_scientific_result() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let run = |name: &str, sort_bytes: u64, threads: usize| {
            let work = temp.path().join(name);
            fs::create_dir(&work).unwrap();
            let mut candidate = options(&work, SupportUnit::AcceptedWindowOccurrence, 5);
            candidate.limits.sort_buffer_bytes = sort_bytes;
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| build_spool_external_counts(&spool, &candidate).unwrap())
        };
        let one = run("one", 4 * 1024, 1);
        let four = run("four", 16 * 1024, 4);
        assert_eq!(
            one.source_descriptor,
            transition_source_descriptor(&spool).unwrap()
        );
        assert_eq!(
            one.source_root,
            crate::experimental::transition_witness::transition_source_root(&spool).unwrap()
        );
        assert_eq!(one.source_root, four.source_root);
        assert_eq!(one.binding_sha256, four.binding_sha256);
        assert_eq!(one.window_stats, four.window_stats);
        assert_eq!(one.external.edge_counts, four.external.edge_counts);
        assert_eq!(one.support_events, four.support_events);
    }

    #[test]
    fn corrupt_and_truncated_spools_fail_before_external_work() {
        let temp = tempfile::tempdir().unwrap();
        let corrupt = paired_spool(&temp.path().join("corrupt-source"));
        make_spool_writable(corrupt.path());
        let mut file = OpenOptions::new().write(true).open(corrupt.path()).unwrap();
        file.seek(SeekFrom::Start(20)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        let corrupt_work = temp.path().join("corrupt-work");
        fs::create_dir(&corrupt_work).unwrap();
        assert_eq!(
            build_spool_external_counts(
                &corrupt,
                &options(&corrupt_work, SupportUnit::AcceptedWindowOccurrence, 5),
            )
            .unwrap_err()
            .code(),
            ErrorCode::IntegritySpool
        );
        assert!(!has_external_run_directory(&corrupt_work));

        let truncated = paired_spool(&temp.path().join("truncated-source"));
        make_spool_writable(truncated.path());
        let len = fs::metadata(truncated.path()).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(truncated.path())
            .unwrap()
            .set_len(len - 1)
            .unwrap();
        let truncated_work = temp.path().join("truncated-work");
        fs::create_dir(&truncated_work).unwrap();
        assert_eq!(
            build_spool_external_counts(
                &truncated,
                &options(&truncated_work, SupportUnit::AcceptedWindowOccurrence, 5),
            )
            .unwrap_err()
            .code(),
            ErrorCode::IntegritySpool
        );
        assert!(!has_external_run_directory(&truncated_work));
    }

    #[test]
    fn fragment_temp_and_run_limits_fail_closed_at_edges() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());

        let memory_work = temp.path().join("memory-work");
        fs::create_dir(&memory_work).unwrap();
        let mut memory = options(&memory_work, SupportUnit::SuppliedFragmentInstance, 5);
        memory.max_fragment_windows = 1;
        assert_eq!(
            build_spool_external_counts(&spool, &memory)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
        assert!(!has_external_run_directory(&memory_work));

        let temp_work = temp.path().join("temp-work");
        fs::create_dir(&temp_work).unwrap();
        let mut temporary = options(&temp_work, SupportUnit::AcceptedWindowOccurrence, 5);
        temporary.limits.max_temp_bytes = fs::metadata(spool.path()).unwrap().len();
        assert_eq!(
            build_spool_external_counts(&spool, &temporary)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceTemporaryBytes
        );
        assert!(!has_external_run_directory(&temp_work));

        let run_work = temp.path().join("run-work");
        fs::create_dir(&run_work).unwrap();
        let mut runs = options(&run_work, SupportUnit::AcceptedWindowOccurrence, 5);
        runs.limits.sort_buffer_bytes = 80;
        runs.limits.max_run_files = 1;
        assert_eq!(
            build_spool_external_counts(&spool, &runs)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRunCount
        );
        assert!(!has_external_run_directory(&run_work));
    }

    #[test]
    fn impossible_memory_is_rejected_before_a_corrupt_spool_is_read() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(&temp.path().join("corrupt-source"));
        make_spool_writable(spool.path());
        let mut file = OpenOptions::new().write(true).open(spool.path()).unwrap();
        file.seek(SeekFrom::Start(20)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();

        let work = temp.path().join("memory-work");
        fs::create_dir(&work).unwrap();
        let mut candidate = options(&work, SupportUnit::AcceptedWindowOccurrence, 5);
        candidate.limits.max_memory_bytes = SPOOL_READER_BUFFER_BYTES - 1;
        assert_eq!(
            build_spool_external_counts(&spool, &candidate)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
        assert!(!has_external_run_directory(&work));
    }

    #[test]
    fn global_window_limit_is_enforced_during_the_first_replay() {
        let temp = tempfile::tempdir().unwrap();
        let spool = paired_spool(temp.path());
        let work = temp.path().join("window-work");
        fs::create_dir(&work).unwrap();
        let mut candidate = options(&work, SupportUnit::AcceptedWindowOccurrence, 5);
        candidate.limits.max_windows = 1;
        assert_eq!(
            build_spool_external_counts(&spool, &candidate)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRetainedKeys
        );
        assert!(!has_external_run_directory(&work));
    }
}
