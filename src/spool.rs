//! Deterministic private fragment spool (`VTSPOOL1`).

use crate::config::{InputSpec, Limits, ScientificConfig, SupportUnit};
use crate::dna::{FragmentScan, KmerScan};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::fastx::{FastxReader, ParsedRecord, READER_BUFFER_BYTES};
#[cfg(test)]
use crate::input::prepare_source;
use crate::input::{
    lower_hex, ordered_sources, prepare_source_with_totals, FileSnapshot, PhysicalIdentity,
    PreparedSource, TransportTotals,
};
use crate::model::{FastxFormat, Fragment, MateRole, ReadRecord, SourceSummary};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use tempfile::{NamedTempFile, TempPath};

const MAGIC: &[u8; 8] = b"VTSPOOL1";
const TRAILER_MAGIC: &[u8; 8] = b"VTSEND01";
const SCHEMA_VERSION: u16 = 1;
const PRETRAILER_DOMAIN: &[u8] = b"veritasm:spool-pretrailer:v1\0";
const HEADER_BYTES: u64 = 34;
const SOURCE_DESCRIPTOR_BYTES: u64 = 86;
const TRAILER_BYTES: u64 = 64;
const IO_BUFFER_BYTES: usize = 64 * 1024;
/// A verifier call performs one bounded forward traversal of the registered
/// descriptor. The bridge uses this constant for operational I/O accounting.
pub(crate) const INTEGRITY_VERIFIER_PHYSICAL_READ_PASSES_PER_CALL: u8 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpoolStats {
    pub raw_transport_bytes: u64,
    pub decoded_input_bytes: u64,
    pub bases: u64,
    pub inferred_mate_roles: u64,
    pub gzip_sources: u64,
    pub gzip_members: u64,
}

/// A completed immutable spool. Its temporary path is removed on drop.
///
/// Registered identity and cardinality fields are not mutable through the
/// public API.
///
/// ```compile_fail
/// use veritasm::spool::Spool;
/// # fn cannot_mutate_registered_identity(spool: &mut Spool) {
/// spool.sha256.clear();
/// # }
/// ```
#[derive(Debug)]
pub struct Spool {
    path: TempPath,
    pub(crate) sha256: String,
    pub(crate) pretrailer_sha256: String,
    pub(crate) fragment_count: u64,
    pub(crate) read_count: u64,
    pub(crate) sources: Vec<SourceSummary>,
    pub(crate) stats: SpoolStats,
    scientific: ScientificConfig,
    limits: Limits,
    input_mode_tag: u8,
    registered_snapshot: FileSnapshot,
}

impl Spool {
    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }

    /// Registered whole-file digest. The value is immutable to external
    /// callers and is rechecked by [`Self::verify`] and [`Self::iter`].
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Registered pre-trailer digest covered by authenticated replay.
    pub fn pretrailer_sha256(&self) -> &str {
        &self.pretrailer_sha256
    }

    pub const fn fragment_count(&self) -> u64 {
        self.fragment_count
    }

    pub const fn read_count(&self) -> u64 {
        self.read_count
    }

    pub fn sources(&self) -> &[SourceSummary] {
        &self.sources
    }

    pub const fn stats(&self) -> &SpoolStats {
        &self.stats
    }

    /// Embedded scientific configuration for crate-internal authenticated
    /// replay consumers. This does not expose a way to mutate spool identity.
    pub(crate) const fn scientific_config(&self) -> &ScientificConfig {
        &self.scientific
    }

    /// Stable input-mode tag covered by the spool pretrailer digest.
    pub(crate) const fn input_mode_tag(&self) -> u8 {
        self.input_mode_tag
    }

    /// Exact on-disk spool schema covered by the registered digests.
    pub(crate) const fn schema_version(&self) -> u16 {
        SCHEMA_VERSION
    }

    /// Registered byte length captured from the owned descriptor after the
    /// spool was sealed and independently verified. Resource accounting must
    /// use this immutable value rather than restatting the pathname, which
    /// could have been replaced after verification.
    pub(crate) const fn registered_byte_len(&self) -> u64 {
        self.registered_snapshot.length()
    }

    /// Re-read every byte, descriptor, length, ordinal, record, and checksum.
    pub fn verify(&self) -> Result<()> {
        verify_spool(self)
    }

    /// Verify and replay one opened descriptor with terminal authentication.
    pub fn iter(&self) -> Result<SpoolIter> {
        admit_spool_io_buffers(&self.limits, 1)?;
        let file = open_spool_nofollow(self.path(), "open spool for iteration")?;
        let (verified_trailer, mut file) = verify_opened_spool(self, file)?;
        file.seek(SeekFrom::Start(0))
            .map_err(integrity_io("rewind verified spool descriptor"))?;
        let expected_whole = decode_hex_32(&self.sha256)?;
        let expected_pretrailer = decode_hex_32(&self.pretrailer_sha256)?;
        let authenticating = SpoolAuthenticatingReader::new(
            file,
            self.registered_snapshot.length(),
            verified_trailer.pretrailer_length,
            expected_whole,
            expected_pretrailer,
            self.registered_snapshot.owned(),
            SpoolReadPassKind::ScientificReplay,
        );
        let mut reader = BufReader::with_capacity(IO_BUFFER_BYTES, authenticating);
        let header = read_header(&mut reader)?;
        validate_registered_header(self, &header)?;
        for expected in &self.sources {
            if read_source_descriptor(&mut reader)? != *expected {
                return Err(integrity("spool source descriptor mismatch during replay"));
            }
        }
        let lane_count = lane_count(&header)?;
        Ok(SpoolIter {
            reader,
            remaining: header.fragment_count,
            next_ordinal: 0,
            mode_tag: header.mode_tag,
            lane_count,
            limits: self.limits.clone(),
            k: self.scientific.k,
            support_unit: self.scientific.support_unit,
            failed: false,
            pending_body_length: None,
            expected_trailer: verified_trailer,
            authenticated: false,
        })
    }
}

/// One allocation-aware spool read outcome used by bounded stream consumers.
#[derive(Debug)]
pub(crate) enum MemoryBoundedNext {
    Fragment {
        fragment: Fragment,
        memory_bytes: u64,
    },
    RequiresMemory(u64),
    End,
}

#[derive(Debug, Clone, Copy)]
enum AdmissionModel {
    DecodeOnly,
    Scan,
}

#[derive(Debug, Clone, Copy)]
enum SpoolReadPassKind {
    IntegrityVerification,
    ScientificReplay,
}

#[cfg(test)]
thread_local! {
    static COMPLETED_SPOOL_READ_PASSES: std::cell::Cell<(u64, u64)> =
        const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
fn record_completed_spool_read_pass(kind: SpoolReadPassKind) {
    COMPLETED_SPOOL_READ_PASSES.with(|counts| {
        let (integrity, scientific) = counts.get();
        counts.set(match kind {
            SpoolReadPassKind::IntegrityVerification => (integrity.saturating_add(1), scientific),
            SpoolReadPassKind::ScientificReplay => (integrity, scientific.saturating_add(1)),
        });
    });
}

#[cfg(not(test))]
fn record_completed_spool_read_pass(_kind: SpoolReadPassKind) {}

#[cfg(test)]
pub(crate) fn reset_completed_spool_read_passes() {
    COMPLETED_SPOOL_READ_PASSES.with(|counts| counts.set((0, 0)));
}

#[cfg(test)]
pub(crate) fn completed_spool_read_passes() -> (u64, u64) {
    COMPLETED_SPOOL_READ_PASSES.with(std::cell::Cell::get)
}

struct SpoolAuthenticatingReader {
    file: File,
    whole_hasher: Sha256,
    pretrailer_hasher: Sha256,
    expected_length: u64,
    pretrailer_length: u64,
    expected_whole: [u8; 32],
    expected_pretrailer: [u8; 32],
    bytes_read: u64,
    opened_snapshot: FileSnapshot,
    observed_eof: bool,
    authenticated: bool,
    failed: bool,
    pass_kind: SpoolReadPassKind,
}

impl SpoolAuthenticatingReader {
    fn new(
        file: File,
        expected_length: u64,
        pretrailer_length: u64,
        expected_whole: [u8; 32],
        expected_pretrailer: [u8; 32],
        opened_snapshot: FileSnapshot,
        pass_kind: SpoolReadPassKind,
    ) -> Self {
        let mut pretrailer_hasher = Sha256::new();
        pretrailer_hasher.update(PRETRAILER_DOMAIN);
        pretrailer_hasher.update(pretrailer_length.to_le_bytes());
        Self {
            file,
            whole_hasher: Sha256::new(),
            pretrailer_hasher,
            expected_length,
            pretrailer_length,
            expected_whole,
            expected_pretrailer,
            bytes_read: 0,
            opened_snapshot,
            observed_eof: false,
            authenticated: false,
            failed: false,
            pass_kind,
        }
    }

    fn finish_authentication(&mut self) -> Result<()> {
        if self.authenticated {
            return Ok(());
        }
        if self.failed || self.bytes_read != self.expected_length || !self.observed_eof {
            self.failed = true;
            return Err(integrity(
                "spool replay did not consume its registered length and exact EOF",
            ));
        }
        let metadata = self
            .file
            .metadata()
            .map_err(integrity_io("reinspect replayed spool descriptor"))?;
        if FileSnapshot::from_metadata(&metadata) != self.opened_snapshot {
            self.failed = true;
            return Err(integrity("spool metadata changed during replay"));
        }
        let observed_whole: [u8; 32] = self.whole_hasher.clone().finalize().into();
        let observed_pretrailer: [u8; 32] = self.pretrailer_hasher.clone().finalize().into();
        if observed_whole != self.expected_whole {
            self.failed = true;
            return Err(integrity("whole-spool SHA-256 mismatch during replay"));
        }
        if observed_pretrailer != self.expected_pretrailer {
            self.failed = true;
            return Err(integrity("spool pretrailer SHA-256 mismatch during replay"));
        }
        self.authenticated = true;
        record_completed_spool_read_pass(self.pass_kind);
        Ok(())
    }

    fn into_authenticated_file(mut self) -> Result<File> {
        self.finish_authentication()?;
        Ok(self.file)
    }
}

impl Read for SpoolAuthenticatingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.failed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "spool replay previously failed",
            ));
        }
        let remaining = self
            .expected_length
            .checked_sub(self.bytes_read)
            .ok_or_else(|| io::Error::other("spool replay byte count overflow"))?;
        if remaining == 0 {
            let mut probe = [0u8; 1];
            let count = self.file.read(&mut probe)?;
            if count != 0 {
                self.failed = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bytes follow the registered spool length",
                ));
            }
            self.observed_eof = true;
            return Ok(0);
        }
        let wanted = usize::try_from(remaining.min(output.len() as u64))
            .map_err(|_| io::Error::other("spool replay read size overflow"))?;
        let count = self.file.read(&mut output[..wanted])?;
        if count == 0 {
            self.failed = true;
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "spool ended before its registered length",
            ));
        }
        let start = self.bytes_read;
        self.bytes_read = self
            .bytes_read
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("spool replay byte count overflow"))?;
        self.whole_hasher.update(&output[..count]);
        if start < self.pretrailer_length {
            let pretrailer_count = usize::try_from(
                self.pretrailer_length
                    .checked_sub(start)
                    .ok_or_else(|| io::Error::other("spool pretrailer byte count overflow"))?
                    .min(count as u64),
            )
            .map_err(|_| io::Error::other("spool pretrailer read size overflow"))?;
            self.pretrailer_hasher.update(&output[..pretrailer_count]);
        }
        Ok(count)
    }
}

/// Counts bytes delivered to the structural decoder, independently of any
/// read-ahead performed by its buffered, digesting inner reader.
struct LogicalCountingReader<R> {
    inner: R,
    bytes_read: u64,
}

impl<R> LogicalCountingReader<R> {
    const fn new(inner: R) -> Self {
        Self {
            inner,
            bytes_read: 0,
        }
    }

    const fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for LogicalCountingReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(output)?;
        self.bytes_read = self
            .bytes_read
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("spool structural byte count overflow"))?;
        Ok(count)
    }
}

pub struct SpoolIter {
    reader: BufReader<SpoolAuthenticatingReader>,
    remaining: u64,
    next_ordinal: u64,
    mode_tag: u8,
    lane_count: u32,
    limits: Limits,
    k: u8,
    support_unit: SupportUnit,
    failed: bool,
    pending_body_length: Option<u64>,
    expected_trailer: Trailer,
    authenticated: bool,
}

impl SpoolIter {
    /// Read a fragment only when its conservative scan-workspace bound fits.
    ///
    /// The fixed-width body length is consumed and retained without allocating
    /// the fragment when the caller must first release an existing batch. This
    /// admission includes the decoded fragment and the two k-mer vectors that
    /// can coexist during scanning; consumers that only decode should use
    /// [`Self::next_with_decode_memory_limit`].
    pub(crate) fn next_with_memory_limit(
        &mut self,
        available_bytes: u64,
    ) -> Result<MemoryBoundedNext> {
        self.next_with_admission_limit(available_bytes, AdmissionModel::Scan)
    }

    /// Read a fragment only when its conservative decoded-fragment bound fits.
    ///
    /// This deliberately excludes k-mer scan vectors. It is for streaming
    /// consumers, such as read-backed audit, that separately admit their own
    /// derived per-fragment allocations. Like scan admission, an insufficient
    /// allowance retains only the fixed-width body length and does not decode
    /// or allocate the variable-length fragment body.
    pub(crate) fn next_with_decode_memory_limit(
        &mut self,
        available_bytes: u64,
    ) -> Result<MemoryBoundedNext> {
        self.next_with_admission_limit(available_bytes, AdmissionModel::DecodeOnly)
    }

    fn next_with_admission_limit(
        &mut self,
        available_bytes: u64,
        admission: AdmissionModel,
    ) -> Result<MemoryBoundedNext> {
        if self.failed {
            return Ok(MemoryBoundedNext::End);
        }
        if self.remaining == 0 {
            if let Err(error) = self.finish_authentication() {
                self.failed = true;
                return Err(error);
            }
            return Ok(MemoryBoundedNext::End);
        }
        let body_length = match self.pending_body_length {
            Some(length) => length,
            None => match read_array::<8, _>(&mut self.reader, "fragment body length") {
                Ok(bytes) => u64::from_le_bytes(bytes),
                Err(error) => {
                    self.failed = true;
                    return Err(error);
                }
            },
        };
        let memory_bytes = match admission {
            AdmissionModel::DecodeOnly => {
                fragment_memory_bound(body_length, self.mode_tag, &self.limits)
            }
            AdmissionModel::Scan => scan_admission_memory_bound(
                body_length,
                self.mode_tag,
                &self.limits,
                self.k,
                self.support_unit,
            ),
        };
        let memory_bytes = match memory_bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                self.failed = true;
                return Err(error);
            }
        };
        if memory_bytes > available_bytes {
            self.pending_body_length = Some(body_length);
            return Ok(MemoryBoundedNext::RequiresMemory(memory_bytes));
        }
        self.pending_body_length = None;
        match read_fragment_body(
            &mut self.reader,
            body_length,
            self.next_ordinal,
            self.mode_tag,
            self.lane_count,
            &self.limits,
        ) {
            Ok(fragment) => {
                self.next_ordinal += 1;
                self.remaining -= 1;
                if self.remaining == 0 {
                    if let Err(error) = self.finish_authentication() {
                        self.failed = true;
                        return Err(error);
                    }
                }
                Ok(MemoryBoundedNext::Fragment {
                    fragment,
                    memory_bytes,
                })
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    /// True only after every registered byte, trailer field, and EOF has been
    /// authenticated on the descriptor used for replay.
    pub const fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    fn finish_authentication(&mut self) -> Result<()> {
        if self.authenticated {
            return Ok(());
        }
        if self.pending_body_length.is_some() {
            return Err(integrity(
                "cannot authenticate spool with a deferred fragment body",
            ));
        }
        let observed_trailer = read_trailer(&mut self.reader)?;
        if observed_trailer != self.expected_trailer {
            return Err(integrity("spool trailer changed during replay"));
        }
        let mut probe = [0u8; 1];
        let count = self
            .reader
            .read(&mut probe)
            .map_err(integrity_io("authenticate spool EOF"))?;
        if count != 0 {
            return Err(integrity("unexpected bytes follow the spool trailer"));
        }
        self.reader.get_mut().finish_authentication()?;
        self.authenticated = true;
        Ok(())
    }
}

impl Iterator for SpoolIter {
    type Item = Result<Fragment>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_with_memory_limit(u64::MAX) {
            Ok(MemoryBoundedNext::Fragment { fragment, .. }) => Some(Ok(fragment)),
            Ok(MemoryBoundedNext::End) => None,
            Ok(MemoryBoundedNext::RequiresMemory(_)) => Some(Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                "u64::MAX could not admit a bounded spool fragment",
            ))),
            Err(error) => Some(Err(error)),
        }
    }
}

/// Parse all sources once into a private payload and construct `VTSPOOL1`.
pub fn create_spool(
    input: &InputSpec,
    scientific: &ScientificConfig,
    limits: &Limits,
    temp_dir: &Path,
) -> Result<Spool> {
    limits.validate(scientific.k)?;
    fs::create_dir_all(temp_dir).map_err(|error| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot create temporary directory: {error}"),
        )
    })?;
    let source_specs = ordered_sources(input)?;
    let source_count = u32::try_from(source_specs.len())
        .map_err(|_| overflow("spool source count exceeds u32"))?;
    let fixed_bytes = HEADER_BYTES
        .checked_add(
            SOURCE_DESCRIPTOR_BYTES
                .checked_mul(u64::from(source_count))
                .ok_or_else(|| overflow("spool source descriptor bytes"))?,
        )
        .and_then(|value| value.checked_add(TRAILER_BYTES))
        .ok_or_else(|| overflow("spool fixed bytes"))?;
    enforce_spool_limit(fixed_bytes, limits)?;

    admit_spool_io_buffers(limits, 1)?;
    let mut payload = NamedTempFile::new_in(temp_dir).map_err(temporary_create_error)?;
    let mut payload_writer = BufWriter::with_capacity(IO_BUFFER_BYTES, payload.as_file_mut());
    let mut payload_bytes = 0u64;
    let mut transport_totals = TransportTotals::default();
    let mut fragment_count = 0u64;
    let mut read_count = 0u64;
    let mut source_summaries = Vec::with_capacity(source_specs.len());
    let mut stats = SpoolStats::default();
    let mut opened_identities = BTreeSet::new();

    if input.mode_tag() == 0 {
        for spec in source_specs {
            let prepared = prepare_source_with_totals(
                spec,
                limits,
                temp_dir,
                payload_bytes,
                &mut transport_totals,
            )?;
            register_opened_identity(&prepared, &mut opened_identities)?;
            // The parser can transiently hold an accumulated record plus one
            // bounded physical line. Reserve half the budget for that overlap.
            let parser_memory = parser_memory_share(limits, 1)?;
            let mut reader = FastxReader::open(&prepared, limits, parser_memory)?;
            while let Some(record) = reader.next_record()? {
                let next_payload = projected_payload_bytes(payload_bytes, &[&record])?;
                enforce_growth(fixed_bytes, next_payload, prepared.decoded_bytes, limits)?;
                write_fragment_body(
                    &mut payload_writer,
                    fragment_count,
                    prepared.spec.lane_ordinal,
                    &[(MateRole::S, record)],
                )?;
                payload_bytes = next_payload;
                fragment_count = checked_add(fragment_count, 1, "fragment count")?;
                read_count = checked_add(read_count, 1, "read count")?;
            }
            ensure_source_authenticated(&prepared, &reader)?;
            source_summaries.push(source_summary(&prepared, &reader));
            accumulate_source_stats(&mut stats, &prepared, &reader)?;
        }
    } else {
        // Two completed mates and one in-progress physical line can overlap.
        let per_mate_memory = parser_memory_share(limits, 2)?;
        for pair in source_specs.chunks_exact(2) {
            let first = prepare_source_with_totals(
                pair[0].clone(),
                limits,
                temp_dir,
                payload_bytes,
                &mut transport_totals,
            )?;
            register_opened_identity(&first, &mut opened_identities)?;
            let second_base = payload_bytes
                .checked_add(first.decoded_bytes)
                .ok_or_else(|| overflow("paired staging byte accounting"))?;
            let second = prepare_source_with_totals(
                pair[1].clone(),
                limits,
                temp_dir,
                second_base,
                &mut transport_totals,
            )?;
            register_opened_identity(&second, &mut opened_identities)?;
            let mut reader1 = FastxReader::open(&first, limits, per_mate_memory)?;
            let mut reader2 = FastxReader::open(&second, limits, per_mate_memory)?;
            if reader1.format() != reader2.format() {
                return Err(VeritasmError::new(
                    ErrorCode::PairFormat,
                    format!(
                        "lane {} has {} R1 and {} R2",
                        first.spec.lane_ordinal,
                        reader1.format().as_str(),
                        reader2.format().as_str()
                    ),
                ));
            }
            loop {
                let record1 = reader1.next_record()?;
                let record2 = reader2.next_record()?;
                match (record1, record2) {
                    (None, None) => break,
                    (Some(_), None) | (None, Some(_)) => {
                        return Err(VeritasmError::new(
                            ErrorCode::PairOrderOrCardinality,
                            format!(
                                "lane {} mates have different record counts",
                                first.spec.lane_ordinal
                            ),
                        ));
                    }
                    (Some(record1), Some(record2)) => {
                        if record1.normalized_identity != record2.normalized_identity {
                            return Err(VeritasmError::new(
                                ErrorCode::PairIdentity,
                                format!(
                                    "lane {} pair {} has different normalized identities",
                                    first.spec.lane_ordinal,
                                    reader1.records()
                                ),
                            ));
                        }
                        let live_decoded = first
                            .decoded_bytes
                            .checked_add(second.decoded_bytes)
                            .ok_or_else(|| overflow("paired decoded staging bytes"))?;
                        let next_payload =
                            projected_payload_bytes(payload_bytes, &[&record1, &record2])?;
                        enforce_growth(fixed_bytes, next_payload, live_decoded, limits)?;
                        write_fragment_body(
                            &mut payload_writer,
                            fragment_count,
                            first.spec.lane_ordinal,
                            &[(MateRole::R1, record1), (MateRole::R2, record2)],
                        )?;
                        payload_bytes = next_payload;
                        fragment_count = checked_add(fragment_count, 1, "fragment count")?;
                        read_count = checked_add(read_count, 2, "read count")?;
                    }
                }
            }
            ensure_source_authenticated(&first, &reader1)?;
            ensure_source_authenticated(&second, &reader2)?;
            source_summaries.push(source_summary(&first, &reader1));
            source_summaries.push(source_summary(&second, &reader2));
            accumulate_source_stats(&mut stats, &first, &reader1)?;
            accumulate_source_stats(&mut stats, &second, &reader2)?;
        }
    }
    if stats.raw_transport_bytes != transport_totals.raw_transport_bytes
        || stats.gzip_members != transport_totals.gzip_members
    {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            "prepared-source transport totals do not reconcile with spool statistics",
        ));
    }
    stats.decoded_input_bytes = transport_totals.decoded_input_bytes;
    payload_writer
        .flush()
        .map_err(temporary_write_error("flush payload"))?;
    drop(payload_writer);
    payload
        .as_file()
        .sync_all()
        .map_err(temporary_write_error("sync payload"))?;

    let pretrailer_length = HEADER_BYTES
        .checked_add(
            SOURCE_DESCRIPTOR_BYTES
                .checked_mul(u64::from(source_count))
                .ok_or_else(|| overflow("spool descriptor length"))?,
        )
        .and_then(|value| value.checked_add(payload_bytes))
        .ok_or_else(|| overflow("spool pretrailer length"))?;
    let complete_length = pretrailer_length
        .checked_add(TRAILER_BYTES)
        .ok_or_else(|| overflow("complete spool length"))?;
    enforce_spool_limit(complete_length, limits)?;
    let simultaneous = payload_bytes
        .checked_add(complete_length)
        .ok_or_else(|| overflow("spool construction temporary bytes"))?;
    if simultaneous > limits.max_temp_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            "payload plus completed spool exceeds max-temp-bytes",
        ));
    }

    admit_spool_io_buffers(limits, 2)?;
    let mut completed = NamedTempFile::new_in(temp_dir).map_err(temporary_create_error)?;
    let pretrailer_digest;
    {
        let mut output = BufWriter::with_capacity(IO_BUFFER_BYTES, completed.as_file_mut());
        pretrailer_digest = {
            let mut writer = FramedWriter::new(&mut output, PRETRAILER_DOMAIN, pretrailer_length);
            write_header(
                &mut writer,
                scientific,
                input.mode_tag(),
                source_count,
                fragment_count,
                read_count,
            )?;
            for source in &source_summaries {
                write_source_descriptor(&mut writer, source)?;
            }
            let payload_file = payload
                .reopen()
                .map_err(temporary_read_error("reopen payload"))?;
            let mut payload_reader = BufReader::with_capacity(IO_BUFFER_BYTES, payload_file);
            io::copy(&mut payload_reader, &mut writer)
                .map_err(temporary_write_error("copy payload into spool"))?;
            writer
                .flush()
                .map_err(temporary_write_error("flush spool pretrailer"))?;
            if writer.bytes_written != pretrailer_length {
                return Err(integrity("pretrailer byte count does not match framing"));
            }
            writer.finalize()
        };
        output
            .write_all(TRAILER_MAGIC)
            .and_then(|_| output.write_all(&pretrailer_length.to_le_bytes()))
            .and_then(|_| output.write_all(&fragment_count.to_le_bytes()))
            .and_then(|_| output.write_all(&read_count.to_le_bytes()))
            .and_then(|_| output.write_all(&pretrailer_digest))
            .map_err(temporary_write_error("write spool trailer"))?;
        output
            .flush()
            .map_err(temporary_write_error("flush completed spool"))?;
        output
            .get_ref()
            .sync_all()
            .map_err(temporary_write_error("sync completed spool"))?;
    }
    let path = completed.into_temp_path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).map_err(|error| {
            VeritasmError::new(
                ErrorCode::IntegritySpool,
                format!("cannot make completed spool read-only: {error}"),
            )
        })?;
    }
    let mut registered_file = open_spool_nofollow(&path, "open completed spool for registration")?;
    let registered_metadata = registered_file
        .metadata()
        .map_err(integrity_io("inspect completed spool for registration"))?;
    let registered_snapshot = FileSnapshot::from_metadata(&registered_metadata);
    if registered_snapshot.length() != complete_length {
        return Err(integrity(
            "completed spool length changed before registration",
        ));
    }
    let spool_digest = sha256_exact_file(&mut registered_file, complete_length)?;
    let final_registered_metadata = registered_file
        .metadata()
        .map_err(integrity_io("reinspect completed spool after registration"))?;
    if FileSnapshot::from_metadata(&final_registered_metadata) != registered_snapshot {
        return Err(integrity(
            "completed spool metadata changed during registration",
        ));
    }

    let spool = Spool {
        path,
        sha256: lower_hex(&spool_digest),
        pretrailer_sha256: lower_hex(&pretrailer_digest),
        fragment_count,
        read_count,
        sources: source_summaries,
        stats,
        scientific: scientific.clone(),
        limits: limits.clone(),
        input_mode_tag: input.mode_tag(),
        registered_snapshot,
    };
    spool.verify()?;
    Ok(spool)
}

fn source_summary(source: &PreparedSource, reader: &FastxReader) -> SourceSummary {
    SourceSummary {
        lane_ordinal: source.spec.lane_ordinal,
        role: source.spec.role,
        format: reader.format(),
        raw_transport_sha256: lower_hex(&source.raw_transport_digest),
        logical_decoded_sha256: lower_hex(&source.logical_decoded_digest),
        records: reader.records(),
        bases: reader.bases(),
    }
}

fn ensure_source_authenticated(source: &PreparedSource, reader: &FastxReader) -> Result<()> {
    if !reader.is_authenticated() {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!(
                "{}: FASTX parser ended without authenticating decoded input",
                source.spec.label()
            ),
        ));
    }
    Ok(())
}

fn register_opened_identity(
    source: &PreparedSource,
    identities: &mut BTreeSet<PhysicalIdentity>,
) -> Result<()> {
    if let Some(identity) = &source.physical_identity {
        if !identities.insert(identity.owned()) {
            return Err(VeritasmError::new(
                ErrorCode::PairPhysicalSourceReuse,
                format!(
                    "{} aliases a source that was already opened",
                    source.spec.label()
                ),
            ));
        }
    }
    Ok(())
}

fn accumulate_source_stats(
    stats: &mut SpoolStats,
    source: &PreparedSource,
    reader: &FastxReader,
) -> Result<()> {
    stats.raw_transport_bytes = checked_add(
        stats.raw_transport_bytes,
        source.raw_bytes,
        "raw transport byte total",
    )?;
    stats.bases = checked_add(stats.bases, reader.bases(), "input base total")?;
    stats.inferred_mate_roles = checked_add(
        stats.inferred_mate_roles,
        reader.inferred_roles(),
        "inferred role total",
    )?;
    if source.gzip_members > 0 {
        stats.gzip_sources = checked_add(stats.gzip_sources, 1, "gzip source total")?;
        stats.gzip_members =
            checked_add(stats.gzip_members, source.gzip_members, "gzip member total")?;
    }
    Ok(())
}

fn projected_payload_bytes(current: u64, records: &[&ParsedRecord]) -> Result<u64> {
    let body = fragment_body_length(records)?;
    current
        .checked_add(8)
        .and_then(|value| value.checked_add(body))
        .ok_or_else(|| overflow("fragment payload length"))
}

fn fragment_body_length(records: &[&ParsedRecord]) -> Result<u64> {
    let mut body = 8u64 + 4 + 1;
    for record in records {
        let sequence =
            u64::try_from(record.sequence.len()).map_err(|_| overflow("spool sequence length"))?;
        body = body
            .checked_add(1 + 32 + 8)
            .and_then(|value| value.checked_add(sequence))
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| overflow("spool fragment body"))?;
        if let Some(quality) = &record.quality {
            let quality =
                u64::try_from(quality.len()).map_err(|_| overflow("spool quality length"))?;
            body = body
                .checked_add(8)
                .and_then(|value| value.checked_add(quality))
                .ok_or_else(|| overflow("spool fragment body"))?;
        }
    }
    Ok(body)
}

fn write_fragment_body<W: Write>(
    output: &mut W,
    ordinal: u64,
    lane: u32,
    records: &[(MateRole, ParsedRecord)],
) -> Result<()> {
    let references: Vec<&ParsedRecord> = records.iter().map(|(_, record)| record).collect();
    let body_length = fragment_body_length(&references)?;
    write_all_temp(output, &body_length.to_le_bytes(), "fragment body length")?;
    write_all_temp(output, &ordinal.to_le_bytes(), "fragment ordinal")?;
    write_all_temp(output, &lane.to_le_bytes(), "fragment lane")?;
    let read_count = u8::try_from(records.len()).map_err(|_| overflow("fragment read count"))?;
    write_all_temp(output, &[read_count], "fragment read count")?;
    for (role, record) in records {
        write_all_temp(output, &[role.tag()], "read role")?;
        write_all_temp(
            output,
            &record.normalized_identity_digest,
            "normalized identity digest",
        )?;
        let sequence_length =
            u64::try_from(record.sequence.len()).map_err(|_| overflow("spool sequence length"))?;
        write_all_temp(output, &sequence_length.to_le_bytes(), "sequence length")?;
        write_all_temp(output, &record.sequence, "sequence")?;
        match &record.quality {
            None => write_all_temp(output, &[0], "quality presence")?,
            Some(quality) => {
                if quality.len() != record.sequence.len() {
                    return Err(integrity("sequence and quality differ before spooling"));
                }
                write_all_temp(output, &[1], "quality presence")?;
                let quality_length =
                    u64::try_from(quality.len()).map_err(|_| overflow("spool quality length"))?;
                write_all_temp(output, &quality_length.to_le_bytes(), "quality length")?;
                write_all_temp(output, quality, "quality")?;
            }
        }
    }
    Ok(())
}

fn write_header<W: Write>(
    output: &mut W,
    scientific: &ScientificConfig,
    mode_tag: u8,
    source_count: u32,
    fragment_count: u64,
    read_count: u64,
) -> Result<()> {
    write_integrity(output, MAGIC, "spool magic")?;
    write_integrity(output, &SCHEMA_VERSION.to_le_bytes(), "spool schema")?;
    write_integrity(output, &[scientific.k], "spool k")?;
    write_integrity(
        output,
        &[scientific.min_base_quality],
        "spool base-quality threshold",
    )?;
    write_integrity(output, &[mode_tag], "spool input mode")?;
    write_integrity(
        output,
        &[scientific.support_unit.tag()],
        "spool support unit",
    )?;
    write_integrity(output, &source_count.to_le_bytes(), "spool source count")?;
    write_integrity(
        output,
        &fragment_count.to_le_bytes(),
        "spool fragment count",
    )?;
    write_integrity(output, &read_count.to_le_bytes(), "spool read count")?;
    Ok(())
}

fn write_source_descriptor<W: Write>(output: &mut W, source: &SourceSummary) -> Result<()> {
    let raw = decode_hex_32(&source.raw_transport_sha256)?;
    let logical = decode_hex_32(&source.logical_decoded_sha256)?;
    write_integrity(output, &source.lane_ordinal.to_le_bytes(), "source lane")?;
    write_integrity(output, &[source.role.tag()], "source role")?;
    write_integrity(output, &[source.format.tag()], "source format")?;
    write_integrity(output, &raw, "source raw digest")?;
    write_integrity(output, &logical, "source logical digest")?;
    write_integrity(output, &source.records.to_le_bytes(), "source record count")?;
    write_integrity(output, &source.bases.to_le_bytes(), "source base count")?;
    Ok(())
}

fn verify_spool(spool: &Spool) -> Result<()> {
    admit_spool_io_buffers(&spool.limits, 1)?;
    let file = open_spool_nofollow(spool.path(), "open spool")?;
    verify_opened_spool(spool, file).map(|_| ())
}

/// Authenticate and structurally decode one opened spool in one bounded,
/// forward descriptor traversal. The returned descriptor is positioned at
/// exact EOF and has not been replaced by a path reopen.
fn verify_opened_spool(spool: &Spool, mut file: File) -> Result<(Trailer, File)> {
    let metadata = file
        .metadata()
        .map_err(integrity_io("inspect opened spool"))?;
    let opened_snapshot = FileSnapshot::from_metadata(&metadata);
    if opened_snapshot != spool.registered_snapshot {
        return Err(integrity("completed spool identity or metadata changed"));
    }
    let registered_length = spool.registered_snapshot.length();
    if registered_length > spool.limits.max_spool_bytes {
        return Err(integrity("completed spool exceeds recorded maximum"));
    }
    if registered_length < HEADER_BYTES + TRAILER_BYTES {
        return Err(integrity("completed spool is truncated"));
    }
    let pretrailer_length = registered_length
        .checked_sub(TRAILER_BYTES)
        .ok_or_else(|| integrity("completed spool is truncated"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(integrity_io("rewind spool for authenticated verification"))?;
    let expected_whole = decode_hex_32(&spool.sha256)?;
    let expected_pretrailer = decode_hex_32(&spool.pretrailer_sha256)?;
    let authenticating = SpoolAuthenticatingReader::new(
        file,
        registered_length,
        pretrailer_length,
        expected_whole,
        expected_pretrailer,
        opened_snapshot.owned(),
        SpoolReadPassKind::IntegrityVerification,
    );
    let buffered = BufReader::with_capacity(IO_BUFFER_BYTES, authenticating);
    let mut counted = LogicalCountingReader::new(buffered);

    {
        // The structural parser is physically unable to consume the fixed
        // trailer as fragment payload, even when lengths are attacker chosen.
        let mut pretrailer = (&mut counted).take(pretrailer_length);
        let header = read_header(&mut pretrailer)?;
        validate_registered_header(spool, &header)?;
        for expected in &spool.sources {
            let observed = read_source_descriptor(&mut pretrailer)?;
            if &observed != expected {
                return Err(integrity("spool source descriptor mismatch"));
            }
        }
        let lane_count = lane_count(&header)?;
        for ordinal in 0..header.fragment_count {
            read_fragment(
                &mut pretrailer,
                ordinal,
                header.mode_tag,
                lane_count,
                &spool.limits,
            )?;
        }
        if pretrailer.limit() != 0 {
            return Err(integrity(
                "spool payload does not end at the trailer boundary",
            ));
        }
    }
    if counted.bytes_read() != pretrailer_length {
        return Err(integrity(
            "spool payload does not end at the trailer boundary",
        ));
    }
    let trailer = read_trailer(&mut counted)?;
    if trailer.pretrailer_length != pretrailer_length {
        return Err(integrity("spool trailer length does not match file length"));
    }
    if trailer.fragment_count != spool.fragment_count || trailer.read_count != spool.read_count {
        return Err(integrity("spool trailer count mismatch"));
    }
    if trailer.digest != expected_pretrailer {
        return Err(integrity("spool pretrailer SHA-256 mismatch"));
    }
    if counted.bytes_read() != registered_length {
        return Err(integrity("spool trailer does not have its fixed width"));
    }
    let mut probe = [0u8; 1];
    if counted
        .read(&mut probe)
        .map_err(integrity_io("probe verified spool EOF"))?
        != 0
    {
        return Err(integrity("unexpected bytes follow the spool trailer"));
    }
    let buffered = counted.into_inner();
    let authenticating = buffered.into_inner();
    let file = authenticating.into_authenticated_file()?;
    Ok((trailer, file))
}

fn validate_registered_header(spool: &Spool, header: &Header) -> Result<()> {
    if header.schema != SCHEMA_VERSION
        || header.k != spool.scientific.k
        || header.min_base_quality != spool.scientific.min_base_quality
        || header.mode_tag != spool.input_mode_tag
        || header.support_tag != spool.scientific.support_unit.tag()
        || u64::from(header.source_count)
            != u64::try_from(spool.sources.len()).map_err(|_| overflow("source count"))?
        || header.fragment_count != spool.fragment_count
        || header.read_count != spool.read_count
    {
        return Err(integrity(
            "spool header differs from its completed manifest",
        ));
    }
    Ok(())
}

fn lane_count(header: &Header) -> Result<u32> {
    if header.mode_tag == 0 {
        Ok(header.source_count)
    } else if header.source_count % 2 == 0 {
        Ok(header.source_count / 2)
    } else {
        Err(integrity("paired spool has an odd source count"))
    }
}

#[derive(Debug)]
struct Header {
    schema: u16,
    k: u8,
    min_base_quality: u8,
    mode_tag: u8,
    support_tag: u8,
    source_count: u32,
    fragment_count: u64,
    read_count: u64,
}

fn read_header<R: Read>(input: &mut R) -> Result<Header> {
    if read_array::<8, _>(input, "spool magic")? != *MAGIC {
        return Err(integrity("invalid spool magic"));
    }
    let schema = u16::from_le_bytes(read_array(input, "spool schema")?);
    let k = read_u8(input, "spool k")?;
    let min_base_quality = read_u8(input, "spool base-quality threshold")?;
    let mode_tag = read_u8(input, "spool input mode")?;
    let support_tag = read_u8(input, "spool support unit")?;
    if !matches!(mode_tag, 0 | 1) || !matches!(support_tag, 0 | 1) {
        return Err(integrity("unknown spool mode or support tag"));
    }
    Ok(Header {
        schema,
        k,
        min_base_quality,
        mode_tag,
        support_tag,
        source_count: u32::from_le_bytes(read_array(input, "spool source count")?),
        fragment_count: u64::from_le_bytes(read_array(input, "spool fragment count")?),
        read_count: u64::from_le_bytes(read_array(input, "spool read count")?),
    })
}

fn read_source_descriptor<R: Read>(input: &mut R) -> Result<SourceSummary> {
    let lane_ordinal = u32::from_le_bytes(read_array(input, "source lane")?);
    let role = match read_u8(input, "source role")? {
        0 => MateRole::S,
        1 => MateRole::R1,
        2 => MateRole::R2,
        _ => return Err(integrity("invalid source role tag")),
    };
    let format = match read_u8(input, "source format")? {
        0 => FastxFormat::Fasta,
        1 => FastxFormat::Fastq,
        _ => return Err(integrity("invalid source format tag")),
    };
    let raw = read_array::<32, _>(input, "source raw digest")?;
    let logical = read_array::<32, _>(input, "source logical digest")?;
    Ok(SourceSummary {
        lane_ordinal,
        role,
        format,
        raw_transport_sha256: lower_hex(&raw),
        logical_decoded_sha256: lower_hex(&logical),
        records: u64::from_le_bytes(read_array(input, "source record count")?),
        bases: u64::from_le_bytes(read_array(input, "source base count")?),
    })
}

fn read_fragment<R: Read>(
    input: &mut R,
    expected_ordinal: u64,
    mode_tag: u8,
    lane_count: u32,
    limits: &Limits,
) -> Result<Fragment> {
    let body_length = u64::from_le_bytes(read_array(input, "fragment body length")?);
    fragment_memory_bound(body_length, mode_tag, limits)?;
    read_fragment_body(
        input,
        body_length,
        expected_ordinal,
        mode_tag,
        lane_count,
        limits,
    )
}

fn read_fragment_body<R: Read>(
    input: &mut R,
    body_length: u64,
    expected_ordinal: u64,
    mode_tag: u8,
    lane_count: u32,
    limits: &Limits,
) -> Result<Fragment> {
    let maximum = maximum_body_length(mode_tag, limits)?;
    if body_length < 13 || body_length > maximum {
        return Err(integrity("fragment body length is outside recorded bounds"));
    }
    let mut body = input.take(body_length);
    let ordinal = u64::from_le_bytes(read_array(&mut body, "fragment ordinal")?);
    if ordinal != expected_ordinal {
        return Err(integrity(
            "fragment ordinal is missing, duplicated, or reordered",
        ));
    }
    let lane_ordinal = u32::from_le_bytes(read_array(&mut body, "fragment lane")?);
    if lane_ordinal >= lane_count {
        return Err(integrity("fragment lane ordinal is out of range"));
    }
    let read_count = read_u8(&mut body, "fragment read count")?;
    let expected_reads = if mode_tag == 0 { 1 } else { 2 };
    if read_count != expected_reads {
        return Err(integrity("fragment read count disagrees with input mode"));
    }
    let mut reads = Vec::with_capacity(usize::from(read_count));
    for read_index in 0..read_count {
        let role = match read_u8(&mut body, "fragment read role")? {
            0 => MateRole::S,
            1 => MateRole::R1,
            2 => MateRole::R2,
            _ => return Err(integrity("invalid fragment read role")),
        };
        let expected_role = match (mode_tag, read_index) {
            (0, 0) => MateRole::S,
            (1, 0) => MateRole::R1,
            (1, 1) => MateRole::R2,
            _ => return Err(integrity("invalid fragment role position")),
        };
        if role != expected_role {
            return Err(integrity("fragment read roles are not in frozen order"));
        }
        let normalized_id_digest = read_array(&mut body, "normalized identity digest")?;
        let sequence_length = u64::from_le_bytes(read_array(&mut body, "sequence length")?);
        if sequence_length == 0 || sequence_length > limits.max_read_bases {
            return Err(integrity(
                "spooled sequence length is outside recorded bounds",
            ));
        }
        let sequence = read_vec(&mut body, sequence_length, "sequence")?;
        if !sequence.iter().all(|base| is_upper_iupac(*base)) {
            return Err(integrity("spooled sequence is not normalized IUPAC DNA"));
        }
        let quality = match read_u8(&mut body, "quality presence")? {
            0 => None,
            1 => {
                let quality_length = u64::from_le_bytes(read_array(&mut body, "quality length")?);
                if quality_length != sequence_length || quality_length > limits.max_read_bases {
                    return Err(integrity("spooled quality length differs from sequence"));
                }
                let values = read_vec(&mut body, quality_length, "quality")?;
                if !values.iter().all(|value| (33..=126).contains(value)) {
                    return Err(integrity("spooled quality is outside Phred+33"));
                }
                Some(values)
            }
            _ => return Err(integrity("invalid quality-presence tag")),
        };
        reads.push(ReadRecord {
            role,
            normalized_id_digest,
            sequence,
            quality,
        });
    }
    if body.limit() != 0 {
        return Err(integrity(
            "fragment body contains trailing or missing fields",
        ));
    }
    Ok(Fragment {
        ordinal,
        lane_ordinal,
        reads,
    })
}

fn fragment_memory_bound(body_length: u64, mode_tag: u8, limits: &Limits) -> Result<u64> {
    let maximum = maximum_body_length(mode_tag, limits)?;
    if body_length < 13 || body_length > maximum {
        return Err(integrity("fragment body length is outside recorded bounds"));
    }
    let read_count = match mode_tag {
        0 => 1_u64,
        1 => 2_u64,
        _ => return Err(integrity("invalid spool input mode")),
    };
    let memory_bytes = body_length
        .checked_add(128)
        .and_then(|bytes| bytes.checked_add(read_count * 128))
        .ok_or_else(|| overflow("spool fragment memory bound"))?;
    if memory_bytes > limits.memory_budget_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            "spooled fragment exceeds the configured live-memory budget",
        ));
    }
    Ok(memory_bytes)
}

/// Conservative allocator-byte estimate for one admitted scan fragment.
///
/// This is deliberately an allocation model, not a promise about process RSS:
/// allocator metadata, thread stacks, and the runtime are outside the explicit
/// phase budget.  Before the body is decoded, its framed length is an upper
/// bound on the number of possible (and therefore accepted) windows.  The
/// estimate charges sixteen bytes per window for both allocations that can
/// coexist after `scan_fragment` pre-reserves exactly: the retained aggregate
/// `FragmentScan` vector and one read-local `KmerScan` vector.  In fragment
/// support mode, sorting and deduplication are in place and add no heap vector,
/// but the full pre-dedup aggregate remains charged.
fn scan_admission_memory_bound(
    body_length: u64,
    mode_tag: u8,
    limits: &Limits,
    k: u8,
    support_unit: SupportUnit,
) -> Result<u64> {
    let fragment_bytes = fragment_memory_bound(body_length, mode_tag, limits)?;
    let key_bytes = u64::try_from(std::mem::size_of::<u128>())
        .map_err(|_| overflow("k-mer element size does not fit u64"))?;
    let vector_data = body_length
        .checked_mul(key_bytes)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| overflow("fragment scan vector byte estimate"))?;
    let fixed_scan_bytes = u64::try_from(
        std::mem::size_of::<FragmentScan>()
            + std::mem::size_of::<KmerScan>()
            + 2 * std::mem::size_of::<Vec<bool>>(),
    )
    .map_err(|_| overflow("fragment scan metadata size does not fit u64"))?;
    // Vec<bool> is bit-packed today, but charging one byte per ring slot keeps
    // this estimate independent of that implementation detail.
    let ring_bytes = u64::from(k)
        .checked_mul(2)
        .ok_or_else(|| overflow("fragment scan rolling-ring byte estimate"))?;
    let mode_workspace = match support_unit {
        SupportUnit::SuppliedFragmentInstance => 0_u64,
        SupportUnit::AcceptedWindowOccurrence => 0_u64,
    };
    fragment_bytes
        .checked_add(vector_data)
        .and_then(|bytes| bytes.checked_add(fixed_scan_bytes))
        .and_then(|bytes| bytes.checked_add(ring_bytes))
        .and_then(|bytes| bytes.checked_add(mode_workspace))
        .ok_or_else(|| overflow("fragment scan admission byte estimate"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Trailer {
    pretrailer_length: u64,
    fragment_count: u64,
    read_count: u64,
    digest: [u8; 32],
}

fn read_trailer<R: Read>(input: &mut R) -> Result<Trailer> {
    if read_array::<8, _>(input, "trailer magic")? != *TRAILER_MAGIC {
        return Err(integrity("invalid spool trailer magic"));
    }
    Ok(Trailer {
        pretrailer_length: u64::from_le_bytes(read_array(input, "pretrailer length")?),
        fragment_count: u64::from_le_bytes(read_array(input, "trailer fragment count")?),
        read_count: u64::from_le_bytes(read_array(input, "trailer read count")?),
        digest: read_array(input, "pretrailer digest")?,
    })
}

fn maximum_body_length(mode_tag: u8, limits: &Limits) -> Result<u64> {
    let reads = if mode_tag == 0 { 1u64 } else { 2u64 };
    let per_read = 1u64
        .checked_add(32)
        .and_then(|value| value.checked_add(8))
        .and_then(|value| value.checked_add(limits.max_read_bases))
        .and_then(|value| value.checked_add(1 + 8))
        .and_then(|value| value.checked_add(limits.max_read_bases))
        .ok_or_else(|| overflow("maximum spool read body"))?;
    13u64
        .checked_add(
            per_read
                .checked_mul(reads)
                .ok_or_else(|| overflow("maximum spool fragment body"))?,
        )
        .ok_or_else(|| overflow("maximum spool fragment body"))
}

fn enforce_growth(fixed: u64, payload: u64, live_decoded: u64, limits: &Limits) -> Result<()> {
    let complete = fixed
        .checked_add(payload)
        .ok_or_else(|| overflow("projected spool size"))?;
    enforce_spool_limit(complete, limits)?;
    let temp = payload
        .checked_add(live_decoded)
        .ok_or_else(|| overflow("temporary input and payload bytes"))?;
    if temp > limits.max_temp_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            "decoded inputs plus fragment payload exceed max-temp-bytes",
        ));
    }
    Ok(())
}

fn enforce_spool_limit(length: u64, limits: &Limits) -> Result<()> {
    if length > limits.max_spool_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceSpoolBytes,
            format!(
                "completed spool would exceed {} bytes",
                limits.max_spool_bytes
            ),
        ));
    }
    Ok(())
}

fn parser_memory_share(limits: &Limits, concurrent_readers: u64) -> Result<u64> {
    if concurrent_readers == 0 {
        return Err(integrity("parser reader count must be positive"));
    }
    let io_buffer = u64::try_from(IO_BUFFER_BYTES)
        .map_err(|_| overflow("spool I/O buffer size does not fit u64"))?;
    let reader_buffers = u64::try_from(READER_BUFFER_BYTES)
        .map_err(|_| overflow("FASTX reader buffer size does not fit u64"))?
        .checked_mul(concurrent_readers)
        .ok_or_else(|| overflow("FASTX reader buffer allocation estimate"))?;
    // Input parsing retains its established half-budget share. The buffered
    // payload writer now lives concurrently with one SE parser or both PE
    // parsers, so admit it first and divide only the remainder.
    let parser_phase = limits.memory_budget_bytes / 2;
    let fixed_buffers = io_buffer
        .checked_add(reader_buffers)
        .ok_or_else(|| overflow("parser fixed-buffer allocation estimate"))?;
    let available = parser_phase.checked_sub(fixed_buffers).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            "spool and FASTX I/O buffers exceed the parser phase memory share",
        )
    })?;
    let per_reader = available / concurrent_readers;
    if per_reader == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            "no memory remains for FASTX parsing after spool I/O admission",
        ));
    }
    Ok(per_reader)
}

fn admit_spool_io_buffers(limits: &Limits, buffer_count: u64) -> Result<()> {
    let buffer_bytes = u64::try_from(IO_BUFFER_BYTES)
        .map_err(|_| overflow("spool I/O buffer size does not fit u64"))?
        .checked_mul(buffer_count)
        .ok_or_else(|| overflow("spool I/O buffer allocation estimate"))?;
    if buffer_bytes > limits.memory_budget_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            "spool I/O buffers exceed the configured memory budget",
        ));
    }
    Ok(())
}

fn sha256_exact_file(file: &mut File, length: u64) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hash_exact_prefix(file, length, &mut hasher)?;
    let mut probe = [0u8; 1];
    if file
        .read(&mut probe)
        .map_err(integrity_io("probe spool EOF during SHA-256"))?
        != 0
    {
        return Err(integrity("completed spool exceeds its registered length"));
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
fn sha256_file(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path).map_err(integrity_io("open spool for SHA-256"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; IO_BUFFER_BYTES];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(integrity_io("read spool for SHA-256"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().into())
}

fn hash_exact_prefix<R: Read>(
    input: &mut R,
    mut remaining: u64,
    hasher: &mut Sha256,
) -> Result<()> {
    let mut buffer = [0u8; IO_BUFFER_BYTES];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(IO_BUFFER_BYTES as u64))
            .map_err(|_| overflow("spool digest chunk"))?;
        let count = input
            .read(&mut buffer[..wanted])
            .map_err(integrity_io("read spool pretrailer"))?;
        if count == 0 {
            return Err(integrity("spool ended inside its pretrailer digest range"));
        }
        hasher.update(&buffer[..count]);
        remaining -= u64::try_from(count).map_err(|_| overflow("spool digest chunk"))?;
    }
    Ok(())
}

struct FramedWriter<W> {
    inner: W,
    hasher: Sha256,
    bytes_written: u64,
}

impl<W> FramedWriter<W> {
    fn new(inner: W, domain: &[u8], length: u64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(domain);
        hasher.update(length.to_le_bytes());
        Self {
            inner,
            hasher,
            bytes_written: 0,
        }
    }

    fn finalize(self) -> [u8; 32] {
        self.hasher.finalize().into()
    }
}

impl<W: Write> Write for FramedWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..count]);
        self.bytes_written = self
            .bytes_written
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("framed byte count overflow"))?;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn read_array<const N: usize, R: Read>(input: &mut R, field: &'static str) -> Result<[u8; N]> {
    let mut result = [0u8; N];
    input.read_exact(&mut result).map_err(|error| {
        VeritasmError::new(
            ErrorCode::IntegritySpool,
            format!("cannot read {field}: {error}"),
        )
    })?;
    Ok(result)
}

fn read_u8<R: Read>(input: &mut R, field: &'static str) -> Result<u8> {
    Ok(read_array::<1, _>(input, field)?[0])
}

fn read_vec<R: Read>(input: &mut R, length: u64, field: &'static str) -> Result<Vec<u8>> {
    let length = usize::try_from(length).map_err(|_| overflow("spool record allocation"))?;
    let mut result = Vec::new();
    result.try_reserve_exact(length).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot allocate {length} bytes for spooled {field}"),
        )
    })?;
    result.resize(length, 0);
    input.read_exact(&mut result).map_err(|error| {
        VeritasmError::new(
            ErrorCode::IntegritySpool,
            format!("cannot read spooled {field}: {error}"),
        )
    })?;
    Ok(result)
}

fn decode_hex_32(value: &str) -> Result<[u8; 32]> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        return Err(integrity("digest is not 64 lowercase hexadecimal bytes"));
    }
    let mut result = [0u8; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        result[index] = (high << 4) | low;
    }
    Ok(result)
}

fn hex_nibble(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(integrity("digest contains non-lowercase-hexadecimal bytes")),
    }
}

fn is_upper_iupac(base: u8) -> bool {
    matches!(
        base,
        b'A' | b'C'
            | b'G'
            | b'T'
            | b'R'
            | b'Y'
            | b'S'
            | b'W'
            | b'K'
            | b'M'
            | b'B'
            | b'D'
            | b'H'
            | b'V'
            | b'N'
    )
}

fn write_all_temp<W: Write>(output: &mut W, bytes: &[u8], field: &'static str) -> Result<()> {
    output.write_all(bytes).map_err(|error| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot write temporary {field}: {error}"),
        )
    })
}

fn write_integrity<W: Write>(output: &mut W, bytes: &[u8], field: &'static str) -> Result<()> {
    output.write_all(bytes).map_err(|error| {
        VeritasmError::new(
            ErrorCode::IntegritySpool,
            format!("cannot write {field}: {error}"),
        )
    })
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn integrity(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegritySpool, context)
}

fn integrity_io(context: &'static str) -> impl FnOnce(io::Error) -> VeritasmError {
    move |error| integrity(format!("cannot {context}: {error}"))
}

fn open_spool_nofollow(path: &Path, action: &'static str) -> Result<File> {
    #[cfg(unix)]
    {
        use rustix::fs::{openat, Mode, OFlags, CWD};

        let descriptor = openat(
            CWD,
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|error| integrity(format!("cannot {action}: {error}")))?;
        let file = File::from(descriptor);
        let metadata = file
            .metadata()
            .map_err(integrity_io("inspect opened spool descriptor"))?;
        if !metadata.is_file() {
            return Err(integrity("opened spool descriptor is not a regular file"));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let file = File::open(path).map_err(integrity_io(action))?;
        if !file
            .metadata()
            .map_err(integrity_io("inspect opened spool descriptor"))?
            .is_file()
        {
            return Err(integrity("opened spool descriptor is not a regular file"));
        }
        Ok(file)
    }
}

fn temporary_create_error(error: io::Error) -> VeritasmError {
    VeritasmError::new(
        ErrorCode::ResourceTemporaryBytes,
        format!("cannot create temporary spool file: {error}"),
    )
}

fn temporary_read_error(context: &'static str) -> impl FnOnce(io::Error) -> VeritasmError {
    move |error| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot {context}: {error}"),
        )
    }
}

fn temporary_write_error(context: &'static str) -> impl FnOnce(io::Error) -> VeritasmError {
    move |error| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot {context}: {error}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Profile, SupportUnit};
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

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

    fn reauthenticate(spool: &mut Spool, bytes: &mut [u8]) {
        let trailer_start = bytes.len() - TRAILER_BYTES as usize;
        let pretrailer_length = trailer_start as u64;
        bytes[trailer_start + 8..trailer_start + 16]
            .copy_from_slice(&pretrailer_length.to_le_bytes());
        let mut framed = Sha256::new();
        framed.update(PRETRAILER_DOMAIN);
        framed.update(pretrailer_length.to_le_bytes());
        framed.update(&bytes[..trailer_start]);
        let pretrailer_digest = framed.finalize();
        bytes[trailer_start + 32..].copy_from_slice(&pretrailer_digest);
        fs::write(spool.path(), &*bytes).unwrap();
        spool.pretrailer_sha256 = lower_hex(&pretrailer_digest);
        spool.sha256 = lower_hex(&sha256_file(spool.path()).unwrap());
        spool.registered_snapshot =
            FileSnapshot::from_metadata(&fs::metadata(spool.path()).unwrap());
    }

    fn single_end_sequence_offset(bytes: &[u8], fragment_index: usize) -> usize {
        let mut cursor = HEADER_BYTES as usize + SOURCE_DESCRIPTOR_BYTES as usize;
        for index in 0..=fragment_index {
            let body_length = usize::try_from(u64::from_le_bytes(
                bytes[cursor..cursor + 8].try_into().unwrap(),
            ))
            .unwrap();
            if index == fragment_index {
                return cursor + 8 + 54;
            }
            cursor += 8 + body_length;
        }
        unreachable!()
    }

    fn overwrite_spool(spool: &Spool, offset: usize, replacement: &[u8]) {
        #[cfg(unix)]
        fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let mut file = OpenOptions::new().write(true).open(spool.path()).unwrap();
        file.seek(SeekFrom::Start(offset as u64)).unwrap();
        file.write_all(replacement).unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
    }

    /// Replace the complete test payload and register only its physical
    /// metadata, deliberately retaining the original authenticated digests.
    /// This bypasses the cheap metadata rejection so corruption tests exercise
    /// the forward digest/decoder pass itself.
    fn replace_bytes_without_reauthenticating(spool: &mut Spool, bytes: &[u8]) {
        #[cfg(unix)]
        fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(spool.path(), bytes).unwrap();
        #[cfg(unix)]
        fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o400)).unwrap();
        spool.registered_snapshot =
            FileSnapshot::from_metadata(&fs::metadata(spool.path()).unwrap());
    }

    #[test]
    fn final_fragment_is_withheld_after_mutation_before_first_traversal() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">only\nACGT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let bytes = fs::read(spool.path()).unwrap();
        let sequence = single_end_sequence_offset(&bytes, 0);
        assert_eq!(&bytes[sequence..sequence + 4], b"ACGT");

        let mut iterator = spool.iter().unwrap();
        assert!(!iterator.is_authenticated());
        overwrite_spool(&spool, sequence, b"TGCA");
        let error = iterator.next().unwrap().unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegritySpool);
        assert!(!iterator.is_authenticated());
    }

    #[test]
    fn final_fragment_is_withheld_after_mutation_between_fragments() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">first\nAAAA\n>second\nCCCC\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let bytes = fs::read(spool.path()).unwrap();
        let second_sequence = single_end_sequence_offset(&bytes, 1);
        assert_eq!(&bytes[second_sequence..second_sequence + 4], b"CCCC");

        let mut iterator = spool.iter().unwrap();
        assert_eq!(iterator.next().unwrap().unwrap().reads[0].sequence, b"AAAA");
        assert!(!iterator.is_authenticated());
        overwrite_spool(&spool, second_sequence, b"GGGG");
        let error = iterator.next().unwrap().unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegritySpool);
        assert!(!iterator.is_authenticated());
    }

    #[test]
    fn trailer_mutation_withholds_the_only_fragment() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">only\nACGT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let length = fs::metadata(spool.path()).unwrap().len() as usize;
        let mut iterator = spool.iter().unwrap();
        overwrite_spool(&spool, length - 1, &[0xff]);

        let error = iterator.next().unwrap().unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegritySpool);
        assert!(!iterator.is_authenticated());
    }

    #[test]
    fn authentication_state_distinguishes_early_stop_from_complete_replay() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">first\nAAAA\n>second\nCCCC\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();

        let mut early = spool.iter().unwrap();
        assert!(!early.is_authenticated());
        assert!(early.next().unwrap().is_ok());
        assert!(!early.is_authenticated());
        drop(early);

        let mut complete = spool.iter().unwrap();
        assert!(complete.next().unwrap().is_ok());
        assert!(complete.next().unwrap().is_ok());
        assert!(complete.is_authenticated());
        assert!(complete.next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn path_replacement_after_iter_open_never_adopts_replacement() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">only\nACGT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let original = fs::read(spool.path()).unwrap();
        let displaced = directory.path().join("displaced-spool");
        let mut iterator = spool.iter().unwrap();
        fs::rename(spool.path(), &displaced).unwrap();
        fs::write(spool.path(), vec![0u8; original.len()]).unwrap();

        let error = iterator.next().unwrap().unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntegritySpool);
        let descriptor_snapshot =
            FileSnapshot::from_metadata(&iterator.reader.get_ref().file.metadata().unwrap());
        let displaced_snapshot = FileSnapshot::from_metadata(&fs::metadata(&displaced).unwrap());
        let replacement_snapshot =
            FileSnapshot::from_metadata(&fs::metadata(spool.path()).unwrap());
        assert_eq!(descriptor_snapshot, displaced_snapshot);
        assert_ne!(descriptor_snapshot, replacement_snapshot);
        assert!(!iterator.is_authenticated());
    }

    #[test]
    fn round_trips_single_end_spool() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.data");
        fs::write(&input_path, b">first\naCgTN\n>second\nTTT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        assert_eq!(spool.fragment_count, 2);
        assert_eq!(spool.read_count, 2);
        let fragments = spool.iter().unwrap().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(fragments[0].reads[0].sequence, b"ACGTN");
        assert_eq!(fragments[1].reads[0].sequence, b"TTT");
        assert_eq!(spool.sources[0].records, 2);
        assert_eq!(spool.stats.bases, 8);
    }

    #[test]
    fn opened_descriptor_identity_rejects_sequential_aliases() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.data");
        fs::write(&input_path, b">first\nACGT\n").unwrap();
        let spec = crate::input::SourceSpec::new(0, MateRole::S, input_path).unwrap();
        let first = prepare_source(
            spec.clone(),
            &Limits::default(),
            directory.path(),
            0,
            &mut 0,
        )
        .unwrap();
        let second = prepare_source(spec, &Limits::default(), directory.path(), 0, &mut 0).unwrap();
        let mut identities = BTreeSet::new();
        register_opened_identity(&first, &mut identities).unwrap();
        let error = register_opened_identity(&second, &mut identities).unwrap_err();
        assert_eq!(error.code(), ErrorCode::PairPhysicalSourceReuse);
    }

    #[test]
    fn allocation_aware_iterator_defers_body_until_budget_is_available() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.data");
        fs::write(&input_path, b">first\nACGTACGT\n>second\nTTTAAACC\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let mut fragments = spool.iter().unwrap();
        let required = match fragments.next_with_memory_limit(0).unwrap() {
            MemoryBoundedNext::RequiresMemory(required) => required,
            other => panic!("expected deferred fragment, got {other:?}"),
        };
        assert!(required > 0);
        assert!(matches!(
            fragments.next_with_memory_limit(required - 1).unwrap(),
            MemoryBoundedNext::RequiresMemory(observed) if observed == required
        ));
        match fragments.next_with_memory_limit(required).unwrap() {
            MemoryBoundedNext::Fragment {
                fragment,
                memory_bytes,
            } => {
                assert_eq!(fragment.ordinal, 0);
                assert_eq!(fragment.reads[0].sequence, b"ACGTACGT");
                assert_eq!(memory_bytes, required);
            }
            other => panic!("expected admitted fragment, got {other:?}"),
        }
        assert_eq!(fragments.next().unwrap().unwrap().ordinal, 1);
        assert!(fragments.next().is_none());

        let mut above_limit = spool.iter().unwrap();
        match above_limit.next_with_memory_limit(required + 1).unwrap() {
            MemoryBoundedNext::Fragment {
                fragment,
                memory_bytes,
            } => {
                assert_eq!(fragment.ordinal, 0);
                assert_eq!(memory_bytes, required);
            }
            other => panic!("expected admission with one spare byte, got {other:?}"),
        }
    }

    #[test]
    fn decode_only_admission_excludes_scan_workspace_and_defers_body() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.data");
        fs::write(&input_path, b">first\nACGTACGTACGTACGT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();

        let mut decoded = spool.iter().unwrap();
        let decode_required = match decoded.next_with_decode_memory_limit(0).unwrap() {
            MemoryBoundedNext::RequiresMemory(required) => required,
            other => panic!("expected deferred decode, got {other:?}"),
        };
        assert!(decode_required > 0);
        assert!(matches!(
            decoded
                .next_with_decode_memory_limit(decode_required - 1)
                .unwrap(),
            MemoryBoundedNext::RequiresMemory(observed) if observed == decode_required
        ));

        let mut scanned = spool.iter().unwrap();
        let scan_required = match scanned.next_with_memory_limit(0).unwrap() {
            MemoryBoundedNext::RequiresMemory(required) => required,
            other => panic!("expected deferred scan, got {other:?}"),
        };
        assert!(scan_required > decode_required);
        assert!(matches!(
            scanned.next_with_memory_limit(decode_required).unwrap(),
            MemoryBoundedNext::RequiresMemory(observed) if observed == scan_required
        ));

        match decoded
            .next_with_decode_memory_limit(decode_required)
            .unwrap()
        {
            MemoryBoundedNext::Fragment {
                fragment,
                memory_bytes,
            } => {
                assert_eq!(fragment.ordinal, 0);
                assert_eq!(fragment.reads[0].sequence, b"ACGTACGTACGTACGT");
                assert_eq!(memory_bytes, decode_required);
            }
            other => panic!("expected admitted decode, got {other:?}"),
        }
        assert!(matches!(
            decoded.next_with_decode_memory_limit(u64::MAX).unwrap(),
            MemoryBoundedNext::End
        ));
    }

    #[test]
    fn scan_admission_charges_pre_dedup_and_occurrence_vectors_equally() {
        let limits = Limits::default();
        let body = 257;
        let fragment = scan_admission_memory_bound(
            body,
            0,
            &limits,
            31,
            SupportUnit::SuppliedFragmentInstance,
        )
        .unwrap();
        let occurrence = scan_admission_memory_bound(
            body,
            0,
            &limits,
            31,
            SupportUnit::AcceptedWindowOccurrence,
        )
        .unwrap();
        assert_eq!(fragment, occurrence);
        assert!(fragment > fragment_memory_bound(body, 0, &limits).unwrap());
    }

    #[test]
    fn synchronizes_pairs_and_rejects_identity_mismatch() {
        let directory = tempdir().unwrap();
        let r1 = directory.path().join("r1.fq");
        let r2 = directory.path().join("r2.fq");
        fs::write(&r1, b"@a/1\nACG\n+\nIII\n").unwrap();
        fs::write(&r2, b"@b/2\nCGT\n+\nIII\n").unwrap();
        let error = create_spool(
            &InputSpec::Paired {
                read1: vec![r1],
                read2: vec![r2],
            },
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PairIdentity);
    }

    #[test]
    fn rejects_role_format_and_cardinality_failures() {
        let directory = tempdir().unwrap();
        let r1 = directory.path().join("r1.fq");
        let r2 = directory.path().join("r2.fq");

        fs::write(&r1, b"@a/2\nACG\n+\nIII\n").unwrap();
        fs::write(&r2, b"@a/2\nCGT\n+\nIII\n").unwrap();
        let error = create_spool(
            &InputSpec::Paired {
                read1: vec![r1.clone()],
                read2: vec![r2.clone()],
            },
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PairRole);

        fs::write(&r1, b">a/1\nACG\n").unwrap();
        fs::write(&r2, b"@a/2\nCGT\n+\nIII\n").unwrap();
        let error = create_spool(
            &InputSpec::Paired {
                read1: vec![r1.clone()],
                read2: vec![r2.clone()],
            },
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PairFormat);

        fs::write(&r1, b"@a/1\nACG\n+\nIII\n@b/1\nACG\n+\nIII\n").unwrap();
        fs::write(&r2, b"@a/2\nCGT\n+\nIII\n").unwrap();
        let error = create_spool(
            &InputSpec::Paired {
                read1: vec![r1],
                read2: vec![r2],
            },
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PairOrderOrCardinality);
    }

    #[test]
    fn byte_identical_sources_create_byte_identical_spools() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let bytes = b"@x\nACGT\n+\nIIII\n";
        fs::write(&first, bytes).unwrap();
        fs::write(&second, bytes).unwrap();
        let left = create_spool(
            &InputSpec::Single(vec![first]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let right = create_spool(
            &InputSpec::Single(vec![second]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        assert_eq!(left.sha256, right.sha256);
        assert_eq!(
            fs::read(left.path()).unwrap(),
            fs::read(right.path()).unwrap()
        );
    }

    #[test]
    fn detects_post_completion_corruption() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">x\nACGT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut file = OpenOptions::new().write(true).open(spool.path()).unwrap();
        file.seek(SeekFrom::Start(20)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.flush().unwrap();
        assert_eq!(
            spool.verify().unwrap_err().code(),
            ErrorCode::IntegritySpool
        );
    }

    #[test]
    fn one_pass_verifier_rejects_a_bit_flip_at_every_registered_byte_offset() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">x\nACGTNACGT\n").unwrap();
        let mut spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let pristine = fs::read(spool.path()).unwrap();

        for offset in 0..pristine.len() {
            let mut corrupted = pristine.clone();
            corrupted[offset] ^= 1;
            replace_bytes_without_reauthenticating(&mut spool, &corrupted);
            assert_eq!(
                spool.verify().unwrap_err().code(),
                ErrorCode::IntegritySpool,
                "bit flip at byte offset {offset} was accepted"
            );
        }

        replace_bytes_without_reauthenticating(&mut spool, &pristine);
        spool.verify().unwrap();
    }

    #[test]
    fn one_pass_verifier_rejects_every_truncation_boundary_and_an_append() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">x\nACGTNACGT\n").unwrap();
        let mut spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let pristine = fs::read(spool.path()).unwrap();

        for retained_length in 0..pristine.len() {
            replace_bytes_without_reauthenticating(&mut spool, &pristine[..retained_length]);
            assert_eq!(
                spool.verify().unwrap_err().code(),
                ErrorCode::IntegritySpool,
                "truncation at byte boundary {retained_length} was accepted"
            );
        }

        let mut appended = pristine.clone();
        appended.push(0);
        replace_bytes_without_reauthenticating(&mut spool, &appended);
        assert_eq!(
            spool.verify().unwrap_err().code(),
            ErrorCode::IntegritySpool
        );
        replace_bytes_without_reauthenticating(&mut spool, &pristine);
        spool.verify().unwrap();
    }

    #[test]
    fn completed_pass_counter_matches_one_verifier_and_one_replay_traversal() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">x\nACGT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();

        reset_completed_spool_read_passes();
        spool.verify().unwrap();
        assert_eq!(completed_spool_read_passes(), (1, 0));

        reset_completed_spool_read_passes();
        let fragments = spool.iter().unwrap().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(fragments.len(), 1);
        assert_eq!(completed_spool_read_passes(), (1, 1));
    }

    #[cfg(unix)]
    #[test]
    fn nofollow_open_rejects_a_symlink_to_the_registered_spool_inode() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">x\nACGT\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        let displaced = directory.path().join("displaced-spool");
        fs::rename(spool.path(), &displaced).unwrap();
        symlink(&displaced, spool.path()).unwrap();

        assert_eq!(
            spool.verify().unwrap_err().code(),
            ErrorCode::IntegritySpool
        );
        assert_eq!(
            spool.iter().err().unwrap().code(),
            ErrorCode::IntegritySpool
        );

        fs::remove_file(spool.path()).unwrap();
        fs::rename(displaced, spool.path()).unwrap();
    }

    #[test]
    fn reauthenticated_structural_spool_corruption_still_fails_closed() {
        const FIRST_FRAGMENT_LENGTH: usize =
            HEADER_BYTES as usize + SOURCE_DESCRIPTOR_BYTES as usize;
        const FIRST_BODY: usize = FIRST_FRAGMENT_LENGTH + 8;
        const ORDINAL: usize = FIRST_BODY;
        const LANE: usize = FIRST_BODY + 8;
        const READ_COUNT: usize = FIRST_BODY + 12;
        const ROLE: usize = FIRST_BODY + 13;
        const SEQUENCE_LENGTH: usize = FIRST_BODY + 46;
        const SEQUENCE: usize = FIRST_BODY + 54;
        const QUALITY_PRESENCE: usize = SEQUENCE + 4;

        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">x\nACGT\n").unwrap();
        let mut spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let pristine = fs::read(spool.path()).unwrap();
        assert_eq!(pristine[SEQUENCE..SEQUENCE + 4], *b"ACGT");

        for (offset, replacement, label) in [
            (FIRST_FRAGMENT_LENGTH, 0_u8, "body length"),
            (ORDINAL, 1, "ordinal"),
            (LANE, 1, "lane"),
            (READ_COUNT, 2, "read count"),
            (ROLE, 1, "role"),
            (SEQUENCE_LENGTH, 0, "sequence length"),
            (SEQUENCE, b'X', "sequence alphabet"),
            (QUALITY_PRESENCE, 2, "quality presence"),
        ] {
            let mut corrupted = pristine.clone();
            corrupted[offset] = replacement;
            reauthenticate(&mut spool, &mut corrupted);
            assert_eq!(
                spool.verify().unwrap_err().code(),
                ErrorCode::IntegritySpool,
                "reauthenticated {label} corruption was accepted"
            );
        }
    }

    #[test]
    fn overflowing_trailer_length_is_a_typed_integrity_error() {
        let directory = tempdir().unwrap();
        let input_path = directory.path().join("reads.fa");
        fs::write(&input_path, b">x\nACGT\n").unwrap();
        let mut spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &Limits::default(),
            directory.path(),
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(spool.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let mut bytes = fs::read(spool.path()).unwrap();
        let trailer_start = bytes.len() - TRAILER_BYTES as usize;
        bytes[trailer_start + 8..trailer_start + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        fs::write(spool.path(), bytes).unwrap();
        spool.sha256 = lower_hex(&sha256_file(spool.path()).unwrap());
        spool.registered_snapshot =
            FileSnapshot::from_metadata(&fs::metadata(spool.path()).unwrap());

        assert_eq!(
            spool.verify().unwrap_err().code(),
            ErrorCode::IntegritySpool
        );
    }
}
