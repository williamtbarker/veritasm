//! Input-role validation, physical-source identity checks, and strict transport decoding.
//!
//! FASTX parsing happens from a private decoded temporary file.  This extra bounded
//! staging pass is deliberate: the frozen source-digest framing places the byte
//! count before the bytes, so a non-seekable input cannot be hashed correctly in a
//! single pass.  It also ensures paired parsers observe immutable bytes.
//!
//! On Linux and macOS, alias decisions are checked against the device/inode of
//! the file descriptor that was actually opened.  Staging compares descriptor
//! metadata and a second content digest, and confirms the path still names that
//! descriptor.  These checks detect ordinary replacement and concurrent edits;
//! they are not a filesystem snapshot, so a privileged adversary able to change
//! and restore bytes between the final checks remains outside the threat model.

use crate::config::{InputSpec, Limits};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::model::MateRole;
use flate2::bufread::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error as StdError;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const RAW_DOMAIN: &[u8] = b"veritasm:raw-transport:v1\0";
const LOGICAL_DOMAIN: &[u8] = b"veritasm:logical-decoded:v1\0";
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// A path assigned to one deterministic lane/role slot.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PhysicalIdentity {
    device: u64,
    inode: u64,
}

#[cfg(not(unix))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PhysicalIdentity {
    canonical_path: PathBuf,
}

impl PhysicalIdentity {
    #[cfg(unix)]
    pub(crate) const fn owned(&self) -> Self {
        *self
    }

    #[cfg(not(unix))]
    pub(crate) fn owned(&self) -> Self {
        self.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    pub lane_ordinal: u32,
    pub role: MateRole,
    pub path: PathBuf,
    expected_identity: Option<PhysicalIdentity>,
}

impl SourceSpec {
    /// Capture the current physical source identity for later open-time checks.
    pub fn new(lane_ordinal: u32, role: MateRole, path: PathBuf) -> Result<Self> {
        let mut source = Self {
            lane_ordinal,
            role,
            path,
            expected_identity: None,
        };
        if source.path == Path::new("-") {
            return Ok(source);
        }
        if source.path.as_os_str().is_empty() {
            return Err(VeritasmError::new(
                ErrorCode::InputOpen,
                format!("{}: empty source path", source.label()),
            ));
        }
        let metadata = fs::metadata(&source.path).map_err(|error| {
            VeritasmError::new(
                ErrorCode::InputOpen,
                format!("{}: cannot inspect source: {error}", source.label()),
            )
        })?;
        if !metadata.is_file() {
            return Err(VeritasmError::new(
                ErrorCode::InputOpen,
                format!("{}: source is not a regular file", source.label()),
            ));
        }
        source.expected_identity = Some(physical_identity(&source.path, &metadata)?);
        Ok(source)
    }

    pub fn label(&self) -> String {
        format!("lane-{:06}-{}", self.lane_ordinal, self.role.as_str())
    }
}

/// Immutable, decoded bytes plus transport-level evidence for one source.
#[derive(Debug)]
pub struct PreparedSource {
    pub spec: SourceSpec,
    decoded: NamedTempFile,
    decoded_snapshot: FileSnapshot,
    pub raw_transport_digest: [u8; 32],
    pub logical_decoded_digest: [u8; 32],
    pub raw_bytes: u64,
    pub decoded_bytes: u64,
    pub gzip_members: u64,
    pub(crate) physical_identity: Option<PhysicalIdentity>,
}

/// Aggregate transport work admitted for sources that completed preparation.
///
/// Callers retain one value across every logical input role so raw bytes and
/// concatenated gzip members are bounded globally rather than per file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransportTotals {
    pub raw_transport_bytes: u64,
    pub decoded_input_bytes: u64,
    pub gzip_members: u64,
}

impl PreparedSource {
    pub(crate) fn open_decoded(&self) -> Result<DecodedReader> {
        let file = File::open(self.decoded.path()).map_err(|error| {
            VeritasmError::new(
                ErrorCode::InputRead,
                format!(
                    "{}: cannot open decoded staging file read-only: {error}",
                    self.spec.label()
                ),
            )
        })?;
        let metadata = file.metadata().map_err(|error| {
            source_changed(
                &self.spec,
                format!("cannot inspect decoded staging descriptor: {error}"),
            )
        })?;
        let observed = FileSnapshot::from_metadata(&metadata);
        if observed != self.decoded_snapshot || observed.length() != self.decoded_bytes {
            return Err(source_changed(
                &self.spec,
                "decoded staging identity or length changed before parsing",
            ));
        }
        Ok(DecodedReader::new(
            file,
            self.decoded_bytes,
            self.logical_decoded_digest,
            observed,
            self.spec.label(),
        ))
    }

    #[cfg(test)]
    pub(crate) fn decoded_path_for_test(&self) -> &Path {
        self.decoded.path()
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileSnapshot {
    identity: PhysicalIdentity,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(unix)]
impl FileSnapshot {
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            identity: PhysicalIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    pub(crate) const fn length(&self) -> u64 {
        self.length
    }

    pub(crate) const fn owned(&self) -> Self {
        *self
    }
}

#[cfg(not(unix))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSnapshot {
    length: u64,
    modified: Option<std::time::SystemTime>,
}

#[cfg(not(unix))]
impl FileSnapshot {
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }

    pub(crate) const fn length(&self) -> u64 {
        self.length
    }

    pub(crate) fn owned(&self) -> Self {
        self.clone()
    }
}

/// One descriptor-bound pass over a registered decoded FASTX staging file.
///
/// The reader hashes exactly the registered byte count and probes one byte at
/// the boundary. Successful parsing must call [`Self::finish_authentication`]
/// after observing EOF; dropping the reader early does not authenticate unread
/// bytes.
pub(crate) struct DecodedReader {
    file: File,
    hasher: Sha256,
    expected_length: u64,
    expected_digest: [u8; 32],
    bytes_read: u64,
    opened_snapshot: FileSnapshot,
    label: String,
    observed_eof: bool,
    authenticated: bool,
    failed: bool,
}

impl DecodedReader {
    fn new(
        file: File,
        expected_length: u64,
        expected_digest: [u8; 32],
        opened_snapshot: FileSnapshot,
        label: String,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(LOGICAL_DOMAIN);
        hasher.update(expected_length.to_le_bytes());
        Self {
            file,
            hasher,
            expected_length,
            expected_digest,
            bytes_read: 0,
            opened_snapshot,
            label,
            observed_eof: false,
            authenticated: false,
            failed: false,
        }
    }

    pub(crate) fn finish_authentication(&mut self) -> Result<()> {
        if self.authenticated {
            return Ok(());
        }
        if self.failed {
            return Err(self.authentication_error("decoded staging traversal previously failed"));
        }
        if self.bytes_read != self.expected_length {
            self.failed = true;
            return Err(self.authentication_error(format!(
                "decoded staging ended at {} bytes; expected {}",
                self.bytes_read, self.expected_length
            )));
        }
        if !self.observed_eof {
            let mut probe = [0u8; 1];
            let count = self.file.read(&mut probe).map_err(|error| {
                self.failed = true;
                self.authentication_error(format!(
                    "cannot authenticate decoded staging EOF: {error}"
                ))
            })?;
            if count != 0 {
                self.failed = true;
                return Err(self.authentication_error(
                    "decoded staging contains bytes beyond its registered length",
                ));
            }
            self.observed_eof = true;
        }
        let final_snapshot = self.file.metadata().map_err(|error| {
            self.failed = true;
            self.authentication_error(format!(
                "cannot reinspect decoded staging descriptor: {error}"
            ))
        })?;
        if FileSnapshot::from_metadata(&final_snapshot) != self.opened_snapshot {
            self.failed = true;
            return Err(
                self.authentication_error("decoded staging metadata changed during parsing")
            );
        }
        let observed: [u8; 32] = self.hasher.clone().finalize().into();
        if observed != self.expected_digest {
            self.failed = true;
            return Err(self.authentication_error("decoded staging SHA-256 changed during parsing"));
        }
        self.authenticated = true;
        Ok(())
    }

    fn authentication_error(&self, context: impl std::fmt::Display) -> VeritasmError {
        VeritasmError::new(ErrorCode::InputRead, format!("{}: {context}", self.label))
    }
}

impl Read for DecodedReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.failed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "decoded staging traversal previously failed",
            ));
        }
        let remaining = self
            .expected_length
            .checked_sub(self.bytes_read)
            .ok_or_else(|| io::Error::other("decoded staging byte count overflow"))?;
        if remaining == 0 {
            let mut probe = [0u8; 1];
            let count = self.file.read(&mut probe)?;
            if count != 0 {
                self.failed = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "decoded staging contains bytes beyond its registered length",
                ));
            }
            self.observed_eof = true;
            return Ok(0);
        }
        let wanted = usize::try_from(remaining.min(output.len() as u64))
            .map_err(|_| io::Error::other("decoded staging read size overflow"))?;
        let count = self.file.read(&mut output[..wanted])?;
        if count == 0 {
            self.failed = true;
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "decoded staging ended before its registered length",
            ));
        }
        self.bytes_read = self
            .bytes_read
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("decoded staging byte count overflow"))?;
        self.hasher.update(&output[..count]);
        Ok(count)
    }
}

#[cfg(unix)]
fn physical_identity(_path: &Path, metadata: &fs::Metadata) -> Result<PhysicalIdentity> {
    use std::os::unix::fs::MetadataExt;
    Ok(PhysicalIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn physical_identity(path: &Path, _metadata: &fs::Metadata) -> Result<PhysicalIdentity> {
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        VeritasmError::new(
            ErrorCode::InputOpen,
            format!("cannot canonicalize source: {error}"),
        )
    })?;
    Ok(PhysicalIdentity { canonical_path })
}

fn ensure_path_identity(spec: &SourceSpec, opened: &PhysicalIdentity) -> Result<()> {
    let metadata = fs::metadata(&spec.path).map_err(|error| {
        source_changed(
            spec,
            format!("cannot inspect source path after open: {error}"),
        )
    })?;
    let observed = physical_identity(&spec.path, &metadata)?;
    if &observed != opened {
        return Err(source_changed(
            spec,
            "source path no longer names the opened file",
        ));
    }
    Ok(())
}

fn source_changed(spec: &SourceSpec, context: impl std::fmt::Display) -> VeritasmError {
    VeritasmError::new(ErrorCode::InputRead, format!("{}: {context}", spec.label()))
}

/// Flatten input roles into the frozen lane order (`S`, or `R1` then `R2`).
pub fn ordered_sources(input: &InputSpec) -> Result<Vec<SourceSpec>> {
    let mut result = Vec::new();
    match input {
        InputSpec::Single(lanes) => {
            if lanes.is_empty() {
                return Err(VeritasmError::new(
                    ErrorCode::ConfigurationUnsupportedCombination,
                    "at least one single-end lane is required",
                ));
            }
            for (lane, path) in lanes.iter().enumerate() {
                result.push(SourceSpec::new(
                    u32::try_from(lane)
                        .map_err(|_| overflow("single-end lane ordinal exceeds u32"))?,
                    MateRole::S,
                    path.clone(),
                )?);
            }
        }
        InputSpec::Paired { read1, read2 } => {
            if read1.len() != read2.len() {
                return Err(VeritasmError::new(
                    ErrorCode::PairLaneCount,
                    "read1 and read2 lane counts differ",
                ));
            }
            if read1.is_empty() {
                return Err(VeritasmError::new(
                    ErrorCode::PairLaneCount,
                    "at least one paired lane is required",
                ));
            }
            for (lane, (r1, r2)) in read1.iter().zip(read2).enumerate() {
                let lane_ordinal =
                    u32::try_from(lane).map_err(|_| overflow("paired lane ordinal exceeds u32"))?;
                result.push(SourceSpec::new(lane_ordinal, MateRole::R1, r1.clone())?);
                result.push(SourceSpec::new(lane_ordinal, MateRole::R2, r2.clone())?);
            }
        }
    }
    reject_physical_aliases(&result)?;
    Ok(result)
}

/// Reject stdin reuse and aliases of the same Unix file object.
fn reject_physical_aliases(sources: &[SourceSpec]) -> Result<()> {
    let mut stdin_seen = false;
    let mut identities = BTreeSet::new();

    for source in sources {
        if source.path == Path::new("-") {
            if stdin_seen {
                return Err(VeritasmError::new(
                    ErrorCode::PairPhysicalSourceReuse,
                    "standard input may occupy only one source role",
                ));
            }
            stdin_seen = true;
            continue;
        }
        if let Some(identity) = &source.expected_identity {
            if !identities.insert(identity) {
                return Err(VeritasmError::new(
                    ErrorCode::PairPhysicalSourceReuse,
                    format!("{} aliases another input source", source.label()),
                ));
            }
        }
    }
    Ok(())
}

/// Decode one source to an immutable private file and calculate the frozen digests.
///
/// This compatibility entry point carries only the historical aggregate decoded
/// byte counter. A caller preparing more than one logical input role must use
/// [`prepare_source_with_totals`] so the raw-byte and gzip-member limits are also
/// aggregate. `base_temp_bytes` is the caller's already-live temporary allocation.
pub fn prepare_source(
    spec: SourceSpec,
    limits: &Limits,
    temp_dir: &Path,
    base_temp_bytes: u64,
    decoded_total: &mut u64,
) -> Result<PreparedSource> {
    let mut transport_totals = TransportTotals {
        decoded_input_bytes: *decoded_total,
        ..TransportTotals::default()
    };
    let outcome = prepare_source_with_totals(
        spec,
        limits,
        temp_dir,
        base_temp_bytes,
        &mut transport_totals,
    );
    *decoded_total = transport_totals.decoded_input_bytes;
    outcome
}

/// Prepare a source while enforcing aggregate transport limits across roles.
///
/// Retain the same `transport_totals` value across every logical source in one
/// operation. It advances only after a source has been completely decoded,
/// hashed, and mutation-checked.
pub fn prepare_source_with_totals(
    spec: SourceSpec,
    limits: &Limits,
    temp_dir: &Path,
    base_temp_bytes: u64,
    transport_totals: &mut TransportTotals,
) -> Result<PreparedSource> {
    prepare_source_with_hooks(
        spec,
        limits,
        temp_dir,
        base_temp_bytes,
        transport_totals,
        || {},
        || {},
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_source_with_hooks<BeforeOpen, AfterFirstPass>(
    spec: SourceSpec,
    limits: &Limits,
    temp_dir: &Path,
    base_temp_bytes: u64,
    transport_totals: &mut TransportTotals,
    before_open: BeforeOpen,
    after_first_pass: AfterFirstPass,
) -> Result<PreparedSource>
where
    BeforeOpen: FnOnce(),
    AfterFirstPass: FnOnce(),
{
    let raw_total_before = transport_totals.raw_transport_bytes;
    let gzip_members_before = transport_totals.gzip_members;
    let mut decoded_total = transport_totals.decoded_input_bytes;
    let (raw_path, staged_raw_len, raw_staging) = if spec.path == Path::new("-") {
        let mut staging = NamedTempFile::new_in(temp_dir).map_err(|error| {
            temporary_error(&spec, format!("cannot create stdin staging file: {error}"))
        })?;
        let mut raw_len = 0u64;
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let mut buffer = [0u8; COPY_BUFFER_BYTES];
        loop {
            let used = raw_total_before
                .checked_add(raw_len)
                .ok_or_else(|| overflow("aggregate raw transport byte count"))?;
            let request =
                bounded_read_request(used, limits.max_raw_transport_bytes, buffer.len(), &spec)?;
            let count = input.read(&mut buffer[..request]).map_err(|error| {
                VeritasmError::new(
                    ErrorCode::InputRead,
                    format!("{}: cannot read standard input: {error}", spec.label()),
                )
            })?;
            if count == 0 {
                break;
            }
            raw_len = raw_len
                .checked_add(u64::try_from(count).map_err(|_| overflow("stdin chunk length"))?)
                .ok_or_else(|| overflow("stdin transport byte count"))?;
            enforce_raw_transport_limit(limits, raw_total_before, raw_len, &spec)?;
            enforce_temp(limits, base_temp_bytes, raw_len, 0, &spec)?;
            staging.write_all(&buffer[..count]).map_err(|error| {
                temporary_error(&spec, format!("cannot stage standard input: {error}"))
            })?;
        }
        staging.flush().map_err(|error| {
            temporary_error(&spec, format!("cannot flush stdin staging file: {error}"))
        })?;
        let path = staging.path().to_path_buf();
        (path, Some(raw_len), Some(staging))
    } else {
        (spec.path.clone(), None, None)
    };

    before_open();
    let raw_file = File::open(&raw_path).map_err(|error| {
        VeritasmError::new(
            ErrorCode::InputOpen,
            format!("{}: cannot open source: {error}", spec.label()),
        )
    })?;
    let opened_metadata = raw_file.metadata().map_err(|error| {
        VeritasmError::new(
            ErrorCode::InputRead,
            format!("{}: cannot inspect opened source: {error}", spec.label()),
        )
    })?;
    if !opened_metadata.is_file() {
        return Err(VeritasmError::new(
            ErrorCode::InputOpen,
            format!("{}: opened source is not a regular file", spec.label()),
        ));
    }
    let opened_identity = physical_identity(&raw_path, &opened_metadata)?;
    if let Some(expected) = &spec.expected_identity {
        if expected != &opened_identity {
            return Err(source_changed(
                &spec,
                "source identity changed between validation and open",
            ));
        }
        ensure_path_identity(&spec, &opened_identity)?;
    }
    let opened_snapshot = FileSnapshot::from_metadata(&opened_metadata);
    let raw_len = staged_raw_len.unwrap_or(opened_metadata.len());
    enforce_raw_transport_limit(limits, raw_total_before, raw_len, &spec)?;
    let raw_extra = raw_staging.as_ref().map_or(0, |_| raw_len);
    let hashing = HashingReader::new(
        raw_file,
        RAW_DOMAIN,
        raw_len,
        raw_total_before,
        limits.max_raw_transport_bytes,
    );
    let mut transport = PrefixBuf::new(BufReader::with_capacity(COPY_BUFFER_BYTES, hashing));
    transport.ensure_prefix(2).map_err(|error| {
        transport_read_error(
            &spec,
            ErrorCode::InputRead,
            "inspect transport prefix",
            error,
        )
    })?;
    let gzip = transport.prefix().starts_with(&[0x1f, 0x8b]);
    let mut decoded = NamedTempFile::new_in(temp_dir).map_err(|error| {
        temporary_error(
            &spec,
            format!("cannot create decoded staging file: {error}"),
        )
    })?;
    let mut decoded_bytes = 0u64;
    let mut gzip_members = 0u64;

    if gzip {
        loop {
            transport.ensure_prefix(2).map_err(|error| {
                transport_read_error(
                    &spec,
                    ErrorCode::InputDecompression,
                    "read gzip member prefix",
                    error,
                )
            })?;
            let prefix = transport.prefix();
            if prefix.is_empty() {
                break;
            }
            if prefix.len() < 2 || !prefix.starts_with(&[0x1f, 0x8b]) {
                return Err(VeritasmError::new(
                    ErrorCode::InputDecompression,
                    format!(
                        "{}: non-gzip trailing bytes after gzip member",
                        spec.label()
                    ),
                ));
            }
            let next_source_members = gzip_members
                .checked_add(1)
                .ok_or_else(|| overflow("gzip member count"))?;
            let next_total_members = gzip_members_before
                .checked_add(next_source_members)
                .ok_or_else(|| overflow("aggregate gzip member count"))?;
            if next_total_members > limits.max_gzip_members {
                return Err(VeritasmError::new(
                    ErrorCode::InputGzipMemberLimit,
                    format!(
                        "{}: aggregate gzip members exceed {}",
                        spec.label(),
                        limits.max_gzip_members
                    ),
                ));
            }
            let mut member = GzDecoder::new(&mut transport);
            copy_decoded(
                &mut member,
                &mut decoded,
                &spec,
                limits,
                base_temp_bytes,
                raw_extra,
                &mut decoded_total,
                &mut decoded_bytes,
                ErrorCode::InputDecompression,
            )?;
            drop(member);
            gzip_members = next_source_members;
        }
    } else {
        copy_decoded(
            &mut transport,
            &mut decoded,
            &spec,
            limits,
            base_temp_bytes,
            raw_extra,
            &mut decoded_total,
            &mut decoded_bytes,
            ErrorCode::InputRead,
        )?;
    }

    decoded.flush().map_err(|error| {
        temporary_error(&spec, format!("cannot flush decoded staging file: {error}"))
    })?;
    decoded.as_file().sync_all().map_err(|error| {
        temporary_error(&spec, format!("cannot sync decoded staging file: {error}"))
    })?;

    let buffered = transport.into_inner();
    let hashing = buffered.into_inner();
    let (mut raw_file, raw_transport_digest, bytes_read) = hashing.finish();
    if bytes_read != raw_len {
        return Err(VeritasmError::new(
            ErrorCode::InputRead,
            format!(
                "{}: source length changed while reading (expected {raw_len}, consumed {})",
                spec.label(),
                bytes_read
            ),
        ));
    }
    after_first_pass();
    let final_metadata = raw_file.metadata().map_err(|error| {
        source_changed(&spec, format!("cannot reinspect opened source: {error}"))
    })?;
    if FileSnapshot::from_metadata(&final_metadata) != opened_snapshot {
        return Err(source_changed(
            &spec,
            "opened source metadata changed during staging",
        ));
    }
    if spec.expected_identity.is_some() {
        ensure_path_identity(&spec, &opened_identity)?;
    }
    raw_file
        .seek(SeekFrom::Start(0))
        .map_err(|error| source_changed(&spec, format!("cannot rewind opened source: {error}")))?;
    let replay_digest = framed_reader_digest(&mut raw_file, RAW_DOMAIN, raw_len, &spec)?;
    if replay_digest != raw_transport_digest {
        return Err(source_changed(
            &spec,
            "opened source content changed during staging",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(decoded.path(), fs::Permissions::from_mode(0o400)).map_err(
            |error| {
                temporary_error(
                    &spec,
                    format!("cannot make decoded staging file read-only: {error}"),
                )
            },
        )?;
    }
    let decoded_metadata = decoded.as_file().metadata().map_err(|error| {
        source_changed(
            &spec,
            format!("cannot inspect decoded staging descriptor: {error}"),
        )
    })?;
    let decoded_snapshot = FileSnapshot::from_metadata(&decoded_metadata);
    if decoded_snapshot.length() != decoded_bytes {
        return Err(source_changed(
            &spec,
            "decoded staging length changed before digest registration",
        ));
    }
    decoded
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|error| {
            source_changed(
                &spec,
                format!("cannot rewind decoded staging descriptor: {error}"),
            )
        })?;
    let logical_decoded_digest =
        framed_reader_digest(decoded.as_file_mut(), LOGICAL_DOMAIN, decoded_bytes, &spec)?;
    let final_decoded_metadata = decoded.as_file().metadata().map_err(|error| {
        source_changed(
            &spec,
            format!("cannot reinspect decoded staging descriptor: {error}"),
        )
    })?;
    if FileSnapshot::from_metadata(&final_decoded_metadata) != decoded_snapshot {
        return Err(source_changed(
            &spec,
            "decoded staging metadata changed during digest registration",
        ));
    }
    drop(raw_staging);

    let prepared_identity = spec.expected_identity.as_ref().map(|_| opened_identity);
    transport_totals.raw_transport_bytes = raw_total_before
        .checked_add(raw_len)
        .ok_or_else(|| overflow("aggregate raw transport byte count"))?;
    transport_totals.decoded_input_bytes = decoded_total;
    transport_totals.gzip_members = gzip_members_before
        .checked_add(gzip_members)
        .ok_or_else(|| overflow("aggregate gzip member count"))?;
    Ok(PreparedSource {
        spec,
        decoded,
        decoded_snapshot,
        raw_transport_digest,
        logical_decoded_digest,
        raw_bytes: raw_len,
        decoded_bytes,
        gzip_members,
        physical_identity: prepared_identity,
    })
}

#[allow(clippy::too_many_arguments)]
fn copy_decoded<R: Read>(
    input: &mut R,
    output: &mut NamedTempFile,
    spec: &SourceSpec,
    limits: &Limits,
    base_temp_bytes: u64,
    raw_extra: u64,
    decoded_total: &mut u64,
    source_decoded: &mut u64,
    read_error: ErrorCode,
) -> Result<()> {
    let mut buffer = [0u8; COPY_BUFFER_BYTES];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| transport_read_error(spec, read_error, "decode transport", error))?;
        if count == 0 {
            break;
        }
        let count = u64::try_from(count).map_err(|_| overflow("decoded chunk length"))?;
        let next_source = source_decoded
            .checked_add(count)
            .ok_or_else(|| overflow("source decoded byte count"))?;
        let next_total = decoded_total
            .checked_add(count)
            .ok_or_else(|| overflow("aggregate decoded byte count"))?;
        if next_total > limits.max_decoded_input_bytes {
            return Err(VeritasmError::new(
                ErrorCode::InputDecodedLimit,
                format!(
                    "{}: aggregate decoded input exceeds {} bytes",
                    spec.label(),
                    limits.max_decoded_input_bytes
                ),
            ));
        }
        enforce_temp(limits, base_temp_bytes, raw_extra, next_source, spec)?;
        output
            .write_all(&buffer[..usize::try_from(count).map_err(|_| overflow("decoded write"))?])
            .map_err(|error| {
                temporary_error(spec, format!("cannot write decoded staging file: {error}"))
            })?;
        *source_decoded = next_source;
        *decoded_total = next_total;
    }
    Ok(())
}

fn enforce_temp(
    limits: &Limits,
    base: u64,
    first: u64,
    second: u64,
    spec: &SourceSpec,
) -> Result<()> {
    let total = base
        .checked_add(first)
        .and_then(|value| value.checked_add(second))
        .ok_or_else(|| overflow("temporary byte accounting"))?;
    if total > limits.max_temp_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!(
                "{}: temporary data would exceed {} bytes",
                spec.label(),
                limits.max_temp_bytes
            ),
        ));
    }
    Ok(())
}

fn framed_reader_digest<R: Read>(
    input: &mut R,
    domain: &[u8],
    length: u64,
    spec: &SourceSpec,
) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(length.to_le_bytes());
    let mut buffer = [0u8; COPY_BUFFER_BYTES];
    let mut observed = 0u64;
    while observed < length {
        let remaining = length
            .checked_sub(observed)
            .ok_or_else(|| overflow("replay digest remaining-byte count"))?;
        let request = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| overflow("replay digest read request does not fit usize"))?;
        let count = input.read(&mut buffer[..request]).map_err(|error| {
            source_changed(spec, format!("cannot replay source digest: {error}"))
        })?;
        if count == 0 {
            return Err(source_changed(
                spec,
                format!(
                    "source length changed during digest replay (expected {length}, observed {observed})"
                ),
            ));
        }
        observed = observed
            .checked_add(u64::try_from(count).map_err(|_| overflow("replay digest chunk length"))?)
            .ok_or_else(|| overflow("replay digest byte count"))?;
        hasher.update(&buffer[..count]);
    }
    // Probe exactly one byte after the declared snapshot. This detects a file
    // that grew after the first pass without allowing a concurrent appender to
    // turn authentication into an unbounded read-to-EOF operation.
    let mut extra = [0u8; 1];
    let extra_count = input.read(&mut extra).map_err(|error| {
        source_changed(spec, format!("cannot probe replay source EOF: {error}"))
    })?;
    if extra_count != 0 {
        return Err(source_changed(
            spec,
            format!(
                "source length changed during digest replay (expected {length}, observed more than {length})"
            ),
        ));
    }
    Ok(hasher.finalize().into())
}

fn temporary_error(spec: &SourceSpec, context: String) -> VeritasmError {
    VeritasmError::new(
        ErrorCode::ResourceTemporaryBytes,
        format!("{}: {context}", spec.label()),
    )
}

fn enforce_raw_transport_limit(
    limits: &Limits,
    completed_raw_bytes: u64,
    source_raw_bytes: u64,
    spec: &SourceSpec,
) -> Result<()> {
    let aggregate = completed_raw_bytes
        .checked_add(source_raw_bytes)
        .ok_or_else(|| overflow("aggregate raw transport byte count"))?;
    if aggregate > limits.max_raw_transport_bytes {
        return Err(raw_transport_limit_error(
            spec,
            limits.max_raw_transport_bytes,
        ));
    }
    Ok(())
}

fn bounded_read_request(
    used: u64,
    maximum: u64,
    buffer_capacity: usize,
    spec: &SourceSpec,
) -> Result<usize> {
    let remaining = maximum
        .checked_sub(used)
        .ok_or_else(|| raw_transport_limit_error(spec, maximum))?;
    if remaining >= buffer_capacity as u64 {
        Ok(buffer_capacity)
    } else {
        usize::try_from(remaining)
            .map_err(|_| overflow("raw transport read allowance does not fit usize"))?
            .checked_add(1)
            .ok_or_else(|| overflow("raw transport limit probe length"))
    }
}

fn raw_transport_limit_error(spec: &SourceSpec, maximum: u64) -> VeritasmError {
    VeritasmError::new(
        ErrorCode::InputRawTransportLimit,
        format!(
            "{}: aggregate raw transport exceeds {maximum} bytes",
            spec.label()
        ),
    )
}

#[derive(Debug)]
struct RawTransportLimitIo {
    maximum: u64,
}

impl std::fmt::Display for RawTransportLimitIo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "aggregate raw transport exceeds {} bytes",
            self.maximum
        )
    }
}

impl StdError for RawTransportLimitIo {}

fn transport_read_error(
    spec: &SourceSpec,
    default_code: ErrorCode,
    action: &'static str,
    error: io::Error,
) -> VeritasmError {
    if let Some(limit) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<RawTransportLimitIo>())
    {
        return raw_transport_limit_error(spec, limit.maximum);
    }
    let mut cause: Option<&(dyn StdError + 'static)> = Some(&error);
    while let Some(current) = cause {
        if let Some(limit) = current.downcast_ref::<RawTransportLimitIo>() {
            return raw_transport_limit_error(spec, limit.maximum);
        }
        cause = current.source();
    }
    VeritasmError::new(
        default_code,
        format!("{}: cannot {action}: {error}", spec.label()),
    )
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes_read: u64,
    completed_raw_bytes: u64,
    maximum_raw_bytes: u64,
}

impl<R> HashingReader<R> {
    fn new(
        inner: R,
        domain: &[u8],
        expected_length: u64,
        completed_raw_bytes: u64,
        maximum_raw_bytes: u64,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(domain);
        hasher.update(expected_length.to_le_bytes());
        Self {
            inner,
            hasher,
            bytes_read: 0,
            completed_raw_bytes,
            maximum_raw_bytes,
        }
    }

    fn finish(self) -> (R, [u8; 32], u64) {
        (self.inner, self.hasher.finalize().into(), self.bytes_read)
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let used = self
            .completed_raw_bytes
            .checked_add(self.bytes_read)
            .ok_or_else(|| io::Error::other("raw byte count overflow"))?;
        let remaining = self.maximum_raw_bytes.checked_sub(used).ok_or_else(|| {
            io::Error::other(RawTransportLimitIo {
                maximum: self.maximum_raw_bytes,
            })
        })?;
        let request = if remaining >= buffer.len() as u64 {
            buffer.len()
        } else {
            usize::try_from(remaining)
                .map_err(|_| io::Error::other("raw read allowance does not fit usize"))?
                .checked_add(1)
                .ok_or_else(|| io::Error::other("raw limit probe length overflow"))?
        };
        let count = self.inner.read(&mut buffer[..request])?;
        if count as u64 > remaining {
            return Err(io::Error::other(RawTransportLimitIo {
                maximum: self.maximum_raw_bytes,
            }));
        }
        self.bytes_read = self
            .bytes_read
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("raw byte count overflow"))?;
        self.hasher.update(&buffer[..count]);
        Ok(count)
    }
}

/// A `BufRead` adapter that can accumulate an exact short prefix even when its
/// underlying reader returns only one byte per call. Bytes are replayed once.
struct PrefixBuf<R: BufRead> {
    inner: R,
    prefix: Vec<u8>,
    prefix_position: usize,
}

impl<R: BufRead> PrefixBuf<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            prefix: Vec::with_capacity(2),
            prefix_position: 0,
        }
    }

    fn ensure_prefix(&mut self, wanted: usize) -> io::Result<()> {
        if self.prefix_position > 0 {
            self.prefix.drain(..self.prefix_position);
            self.prefix_position = 0;
        }
        while self.prefix.len() < wanted {
            let available = self.inner.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let count = (wanted - self.prefix.len()).min(available.len());
            self.prefix.extend_from_slice(&available[..count]);
            self.inner.consume(count);
        }
        Ok(())
    }

    fn prefix(&self) -> &[u8] {
        &self.prefix[self.prefix_position..]
    }

    fn into_inner(self) -> R {
        debug_assert_eq!(self.prefix_position, self.prefix.len());
        self.inner
    }
}

impl<R: BufRead> Read for PrefixBuf<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = output.len().min(available.len());
        output[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl<R: BufRead> BufRead for PrefixBuf<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.prefix_position < self.prefix.len() {
            Ok(&self.prefix[self.prefix_position..])
        } else {
            self.inner.fill_buf()
        }
    }

    fn consume(&mut self, amount: usize) {
        if self.prefix_position < self.prefix.len() {
            let remaining = self.prefix.len() - self.prefix_position;
            let from_prefix = amount.min(remaining);
            self.prefix_position += from_prefix;
            if amount > from_prefix {
                self.inner.consume(amount - from_prefix);
            }
        } else {
            self.inner.consume(amount);
        }
    }
}

pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    struct OneByteBuf<'a> {
        bytes: &'a [u8],
        position: usize,
    }

    impl<'a> OneByteBuf<'a> {
        fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, position: 0 }
        }
    }

    impl Read for OneByteBuf<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if output.is_empty() {
                return Ok(0);
            }
            let available = self.fill_buf()?;
            if available.is_empty() {
                return Ok(0);
            }
            output[0] = available[0];
            self.consume(1);
            Ok(1)
        }
    }

    impl BufRead for OneByteBuf<'_> {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.position == self.bytes.len() {
                Ok(&[])
            } else {
                Ok(&self.bytes[self.position..self.position + 1])
            }
        }

        fn consume(&mut self, amount: usize) {
            self.position = self.position.saturating_add(amount).min(self.bytes.len());
        }
    }

    #[test]
    fn rejects_physical_aliases() {
        let directory = tempdir().unwrap();
        let original = directory.path().join("reads.fq");
        let alias = directory.path().join("alias.fq");
        fs::write(&original, b"@x\nA\n+\nI\n").unwrap();
        symlink(&original, &alias).unwrap();
        let error = ordered_sources(&InputSpec::Single(vec![original, alias])).unwrap_err();
        assert_eq!(error.code(), ErrorCode::PairPhysicalSourceReuse);
    }

    #[test]
    fn rejects_path_replacement_between_validation_and_open() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("reads.fq");
        let replacement = directory.path().join("replacement.fq");
        fs::write(&path, b"@old\nA\n+\nI\n").unwrap();
        fs::write(&replacement, b"@new\nC\n+\nI\n").unwrap();
        let spec = SourceSpec::new(0, MateRole::S, path.clone()).unwrap();
        let mut totals = TransportTotals::default();
        let error = prepare_source_with_hooks(
            spec,
            &Limits::default(),
            directory.path(),
            0,
            &mut totals,
            || fs::rename(&replacement, &path).unwrap(),
            || {},
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputRead);
        assert!(error
            .to_string()
            .contains("identity changed between validation and open"));
    }

    #[test]
    fn rejects_same_inode_mutation_during_staging() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("reads.fq");
        let original = b"@read\nACGT\n+\nIIII\n";
        let replacement = b"@read\nTGCA\n+\nIIII\n";
        assert_eq!(original.len(), replacement.len());
        fs::write(&path, original).unwrap();
        let spec = SourceSpec::new(0, MateRole::S, path.clone()).unwrap();
        let mut totals = TransportTotals::default();
        let error = prepare_source_with_hooks(
            spec,
            &Limits::default(),
            directory.path(),
            0,
            &mut totals,
            || {},
            || fs::write(&path, replacement).unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputRead);
        assert!(error.to_string().contains("changed during staging"));
    }

    #[test]
    fn accepts_concatenated_gzip_and_rejects_trailing_junk() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("reads.bin");
        let mut bytes = Vec::new();
        for part in [b">a\nAC\n".as_slice(), b">b\nGT\n".as_slice()] {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(part).unwrap();
            bytes.extend_from_slice(&encoder.finish().unwrap());
        }
        fs::write(&path, &bytes).unwrap();
        let spec = ordered_sources(&InputSpec::Single(vec![path.clone()]))
            .unwrap()
            .remove(0);
        let mut decoded_total = 0;
        let prepared = prepare_source(
            spec,
            &Limits::default(),
            directory.path(),
            0,
            &mut decoded_total,
        )
        .unwrap();
        assert_eq!(prepared.gzip_members, 2);
        assert_eq!(
            fs::read(prepared.decoded.path()).unwrap(),
            b">a\nAC\n>b\nGT\n"
        );

        bytes.push(b'x');
        fs::write(&path, bytes).unwrap();
        let spec = ordered_sources(&InputSpec::Single(vec![path]))
            .unwrap()
            .remove(0);
        let error =
            prepare_source(spec, &Limits::default(), directory.path(), 0, &mut 0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputDecompression);
    }

    fn prepare_bytes_with_transport_limits(
        directory: &Path,
        bytes: &[u8],
        maximum_raw_bytes: u64,
        maximum_gzip_members: u64,
    ) -> Result<PreparedSource> {
        let path = directory.join(format!(
            "transport-{}-{maximum_raw_bytes}-{maximum_gzip_members}",
            bytes.len()
        ));
        fs::write(&path, bytes).unwrap();
        let spec = SourceSpec::new(0, MateRole::S, path).unwrap();
        let limits = Limits {
            max_raw_transport_bytes: maximum_raw_bytes,
            max_gzip_members: maximum_gzip_members,
            ..Limits::default()
        };
        prepare_source_with_totals(spec, &limits, directory, 0, &mut TransportTotals::default())
    }

    #[test]
    fn raw_transport_limits_are_inclusive_for_plain_and_gzip_files() {
        let directory = tempdir().unwrap();
        let plain = b">plain\nACGT\n".to_vec();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b">gzip\nACGT\n").unwrap();
        let gzip = encoder.finish().unwrap();

        for bytes in [&plain, &gzip] {
            let exact = u64::try_from(bytes.len()).unwrap();
            let below = prepare_bytes_with_transport_limits(
                directory.path(),
                bytes,
                exact.checked_sub(1).unwrap(),
                8,
            )
            .unwrap_err();
            assert_eq!(below.code(), ErrorCode::InputRawTransportLimit);
            assert_eq!(
                prepare_bytes_with_transport_limits(directory.path(), bytes, exact, 8)
                    .unwrap()
                    .raw_bytes,
                exact
            );
            assert_eq!(
                prepare_bytes_with_transport_limits(directory.path(), bytes, exact + 1, 8)
                    .unwrap()
                    .raw_bytes,
                exact
            );
        }
    }

    #[test]
    fn raw_transport_limit_is_aggregate_across_sources() {
        let directory = tempdir().unwrap();
        let first_path = directory.path().join("first.fa");
        let second_path = directory.path().join("second.fa");
        let first_bytes = b">first\nACGT\n";
        let second_bytes = b">second\nTGCA\n";
        fs::write(&first_path, first_bytes).unwrap();
        fs::write(&second_path, second_bytes).unwrap();
        let exact = u64::try_from(first_bytes.len() + second_bytes.len()).unwrap();
        let limits = Limits {
            max_raw_transport_bytes: exact - 1,
            ..Limits::default()
        };
        let mut totals = TransportTotals::default();
        prepare_source_with_totals(
            SourceSpec::new(0, MateRole::S, first_path).unwrap(),
            &limits,
            directory.path(),
            0,
            &mut totals,
        )
        .unwrap();
        let error = prepare_source_with_totals(
            SourceSpec::new(1, MateRole::S, second_path).unwrap(),
            &limits,
            directory.path(),
            0,
            &mut totals,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputRawTransportLimit);
        assert_eq!(totals.raw_transport_bytes, first_bytes.len() as u64);
    }

    #[test]
    fn authoritative_counted_reader_is_inclusive_and_probes_only_one_extra_byte() {
        let mut exact = HashingReader::new(io::Cursor::new(b"abc"), RAW_DOMAIN, 3, 3, 6);
        let mut decoded = Vec::new();
        exact.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, b"abc");
        let (cursor, _, bytes_read) = exact.finish();
        assert_eq!(bytes_read, 3);
        assert_eq!(cursor.position(), 3);

        let mut over = HashingReader::new(io::Cursor::new(b"abcd"), RAW_DOMAIN, 4, 3, 6);
        let mut output = [0u8; 8];
        let cause = over.read(&mut output).unwrap_err();
        let spec = SourceSpec::new(0, MateRole::S, PathBuf::from("-")).unwrap();
        let error = transport_read_error(&spec, ErrorCode::InputRead, "test counted read", cause);
        assert_eq!(error.code(), ErrorCode::InputRawTransportLimit);
        let (cursor, _, bytes_read) = over.finish();
        assert_eq!(bytes_read, 0);
        assert_eq!(cursor.position(), 4);
    }

    #[test]
    fn digest_replay_reads_only_snapshot_length_plus_one_probe() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("growing.fa");
        fs::write(&path, b"abcdef").unwrap();
        let spec = SourceSpec::new(0, MateRole::S, path).unwrap();
        let mut growing = io::Cursor::new(b"abcdef".as_slice());
        let error = framed_reader_digest(&mut growing, RAW_DOMAIN, 3, &spec).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputRead);
        assert!(error.to_string().contains("observed more than 3"));
        assert_eq!(growing.position(), 4);

        let mut exact = io::Cursor::new(b"abc".as_slice());
        assert!(framed_reader_digest(&mut exact, RAW_DOMAIN, 3, &spec).is_ok());
        assert_eq!(exact.position(), 3);
    }

    fn concatenated_empty_members(count: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for _ in 0..count {
            let encoder = GzEncoder::new(Vec::new(), Compression::fast());
            bytes.extend_from_slice(&encoder.finish().unwrap());
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(b">payload\nACGT\n").unwrap();
        bytes.extend_from_slice(&encoder.finish().unwrap());
        bytes
    }

    #[test]
    fn gzip_member_limit_is_inclusive_and_stops_empty_member_storms() {
        let directory = tempdir().unwrap();
        let bytes = concatenated_empty_members(3);
        let exact_members = 4;
        let error = prepare_bytes_with_transport_limits(
            directory.path(),
            &bytes,
            bytes.len() as u64,
            exact_members - 1,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputGzipMemberLimit);
        assert_eq!(
            prepare_bytes_with_transport_limits(
                directory.path(),
                &bytes,
                bytes.len() as u64,
                exact_members,
            )
            .unwrap()
            .gzip_members,
            exact_members
        );
        assert_eq!(
            prepare_bytes_with_transport_limits(
                directory.path(),
                &bytes,
                bytes.len() as u64,
                exact_members + 1,
            )
            .unwrap()
            .gzip_members,
            exact_members
        );

        let storm = concatenated_empty_members(1_024);
        let error = prepare_bytes_with_transport_limits(
            directory.path(),
            &storm,
            storm.len() as u64,
            1_024,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputGzipMemberLimit);
    }

    #[test]
    fn gzip_member_limit_is_aggregate_across_sources() {
        let directory = tempdir().unwrap();
        let mut paths = Vec::new();
        for (name, payload) in [
            ("first", b">first\nACGT\n".as_slice()),
            ("second", b">second\nTGCA\n".as_slice()),
        ] {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(payload).unwrap();
            let path = directory.path().join(name);
            fs::write(&path, encoder.finish().unwrap()).unwrap();
            paths.push(path);
        }

        let limits = Limits {
            max_gzip_members: 1,
            ..Limits::default()
        };
        let mut totals = TransportTotals::default();
        prepare_source_with_totals(
            SourceSpec::new(0, MateRole::S, paths[0].clone()).unwrap(),
            &limits,
            directory.path(),
            0,
            &mut totals,
        )
        .unwrap();
        let error = prepare_source_with_totals(
            SourceSpec::new(1, MateRole::S, paths[1].clone()).unwrap(),
            &limits,
            directory.path(),
            0,
            &mut totals,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputGzipMemberLimit);
        assert_eq!(totals.gzip_members, 1);

        let exact_limits = Limits {
            max_gzip_members: 2,
            ..Limits::default()
        };
        let mut exact_totals = TransportTotals::default();
        for (ordinal, path) in paths.into_iter().enumerate() {
            prepare_source_with_totals(
                SourceSpec::new(u32::try_from(ordinal).unwrap(), MateRole::S, path).unwrap(),
                &exact_limits,
                directory.path(),
                0,
                &mut exact_totals,
            )
            .unwrap();
        }
        assert_eq!(exact_totals.gzip_members, 2);
    }

    #[test]
    fn detects_and_decodes_gzip_when_transport_arrives_one_byte_at_a_time() {
        let payload = b">one-byte-prefix\nACGT\n";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut transport = PrefixBuf::new(OneByteBuf::new(&compressed));
        transport.ensure_prefix(2).unwrap();
        assert_eq!(transport.prefix(), &[0x1f, 0x8b]);

        let mut decoded = Vec::new();
        let mut decoder = GzDecoder::new(&mut transport);
        decoder.read_to_end(&mut decoded).unwrap();
        drop(decoder);
        assert_eq!(decoded, payload);
        transport.ensure_prefix(2).unwrap();
        assert!(transport.prefix().is_empty());
    }

    #[test]
    fn rejects_crc_corruption_in_a_later_gzip_member() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("reads");
        let mut transport = Vec::new();
        for payload in [b">first\nACGT\n".as_slice(), b">second\nTGCA\n".as_slice()] {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(payload).unwrap();
            let mut member = encoder.finish().unwrap();
            if payload.starts_with(b">second") {
                let crc_position = member.len() - 8;
                member[crc_position] ^= 0x80;
            }
            transport.extend_from_slice(&member);
        }
        fs::write(&path, transport).unwrap();

        let spec = ordered_sources(&InputSpec::Single(vec![path]))
            .unwrap()
            .remove(0);
        let error =
            prepare_source(spec, &Limits::default(), directory.path(), 0, &mut 0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputDecompression);
    }

    #[test]
    fn rejects_truncated_and_crc_corrupt_gzip() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("reads");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b">x\nACGT\n").unwrap();
        let complete = encoder.finish().unwrap();

        fs::write(&path, &complete[..complete.len() - 3]).unwrap();
        let spec = ordered_sources(&InputSpec::Single(vec![path.clone()]))
            .unwrap()
            .remove(0);
        let error =
            prepare_source(spec, &Limits::default(), directory.path(), 0, &mut 0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputDecompression);

        let mut corrupt = complete;
        let crc_position = corrupt.len() - 8;
        corrupt[crc_position] ^= 0x80;
        fs::write(&path, corrupt).unwrap();
        let spec = ordered_sources(&InputSpec::Single(vec![path]))
            .unwrap()
            .remove(0);
        let error =
            prepare_source(spec, &Limits::default(), directory.path(), 0, &mut 0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputDecompression);
    }
}
