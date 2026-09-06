//! Authenticated fixed-width runs for the experimental wide-key data plane.
//!
//! This format is deliberately disconnected from the stable count-run
//! schema. Integers are little-endian, exact packed keys are fixed-width
//! big-endian, and every run binds its source identity and routing domain in
//! a SHA-256 trailer. The digest detects accidental or untrusted-cache
//! corruption; it is not a MAC against a writer able to replace all bytes.

use super::partitioned_dbg::{route_minimizer, select_minimizer};
use super::wide_kmer::{canonical_code, validate_code, PackedKmer, PACKED_KEY_BYTES};
use crate::error::{ErrorCode, Result, VeritasmError};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

const RUN_MAGIC: &[u8; 8] = b"VTXRUN01";
const RUN_END: &[u8; 8] = b"VTXEND01";
const RUN_DIGEST_DOMAIN: &[u8] = b"veritasm:experimental-wide-run:v1\0";
const RUN_SCHEMA: u16 = 2;
const INTEGER_KEY_ENCODING: u8 = 1;
const HEADER_BYTES: usize = 128;
const TRAILER_BYTES: usize = 48;
/// Heap buffer used for the independent post-seal verification pass.
pub(crate) const RUN_VERIFICATION_BUFFER_BYTES: usize = 16 * 1024;
/// Hard ceiling for a user-configured run buffer on the supported 64-bit
/// targets. Larger buffering has no established benefit and can turn virtual
/// allocator overcommit into process-fatal page commitment.
pub(crate) const MAX_RUN_IO_BUFFER_BYTES: u64 = 64 * 1024 * 1024;
/// Exact fixed width of both occurrence and reduced records.
pub const RUN_RECORD_BYTES: u64 = 72;

/// A fixed-capacity writer whose allocation is reported through the crate's
/// typed resource-error channel instead of `BufWriter::with_capacity`'s
/// infallible allocation path.
///
/// The implementation deliberately exposes only the operations required by
/// the run codec. Buffered bytes are never allowed to exceed the admitted
/// allocation, and `into_inner` reports a failed final drain.
struct FallibleBufWriter<W: Write> {
    inner: W,
    buffer: Vec<u8>,
}

impl<W: Write> FallibleBufWriter<W> {
    fn from_buffer(inner: W, buffer: Vec<u8>) -> Self {
        Self { inner, buffer }
    }

    fn drain_buffer(&mut self) -> std::io::Result<()> {
        let mut written = 0;
        while written < self.buffer.len() {
            match self.inner.write(&self.buffer[written..]) {
                Ok(0) => {
                    if written > 0 {
                        self.buffer.drain(..written);
                    }
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "failed to drain experimental wide-run buffer",
                    ));
                }
                Ok(count) => written += count,
                Err(cause) => {
                    if written > 0 {
                        self.buffer.drain(..written);
                    }
                    return Err(cause);
                }
            }
        }
        self.buffer.clear();
        Ok(())
    }

    fn into_inner(mut self) -> std::io::Result<W> {
        self.drain_buffer()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for FallibleBufWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.buffer.is_empty() && bytes.len() >= self.buffer.capacity() {
            return self.inner.write(bytes);
        }
        if self.buffer.len() == self.buffer.capacity() {
            self.drain_buffer()?;
        }
        let available = self.buffer.capacity() - self.buffer.len();
        let accepted = available.min(bytes.len());
        self.buffer.extend_from_slice(&bytes[..accepted]);
        Ok(accepted)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.drain_buffer()?;
        self.inner.flush()
    }
}

/// A fixed-capacity reader paired with [`FallibleBufWriter`].
struct FallibleBufReader<R: Read> {
    inner: R,
    buffer: Vec<u8>,
    position: usize,
    available: usize,
}

impl<R: Read> FallibleBufReader<R> {
    fn new(inner: R, capacity: usize) -> Result<Self> {
        let mut buffer = allocate_exact_buffer(capacity, "wide-run reader buffer")?;
        // Capacity was fallibly reserved above, so this initializes existing
        // storage and cannot grow the allocation.
        buffer.resize(capacity, 0);
        Ok(Self {
            inner,
            buffer,
            position: 0,
            available: 0,
        })
    }
}

impl<R: Read> Read for FallibleBufReader<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.position == self.available {
            self.position = 0;
            self.available = 0;
            if output.len() >= self.buffer.len() {
                return self.inner.read(output);
            }
            self.available = self.inner.read(&mut self.buffer)?;
            if self.available == 0 {
                return Ok(0);
            }
        }
        let copied = (self.available - self.position).min(output.len());
        output[..copied].copy_from_slice(&self.buffer[self.position..self.position + copied]);
        self.position += copied;
        Ok(copied)
    }
}

fn allocate_exact_buffer(capacity: usize, label: &'static str) -> Result<Vec<u8>> {
    if capacity == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!("{label} must be nonzero"),
        ));
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot allocate {capacity} bytes for {label}: {cause}"),
        )
    })?;
    if buffer.capacity() > capacity {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "{label} capacity {} exceeds admitted capacity {capacity}",
                buffer.capacity()
            ),
        ));
    }
    Ok(buffer)
}

/// The interpretation of the final `u64` in each fixed-width record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum WideRunKind {
    /// The value is one globally assigned source-window ordinal.
    Observation = 1,
    /// The value is a checked exact support count.
    Reduced = 2,
}

impl WideRunKind {
    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::Observation),
            2 => Ok(Self::Reduced),
            _ => integrity(format!("unknown experimental wide-run kind tag {tag}")),
        }
    }
}

/// Scientific support unit authenticated by every XWR v2 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum WideRunSupportUnit {
    SuppliedFragmentInstance = 0,
    AcceptedWindowOccurrence = 1,
}

impl WideRunSupportUnit {
    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            0 => Ok(Self::SuppliedFragmentInstance),
            1 => Ok(Self::AcceptedWindowOccurrence),
            _ => integrity(format!(
                "unknown experimental wide-run support-unit tag {tag}"
            )),
        }
    }
}

/// Exact scientific and provenance domain shared by compatible runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideRunDomain {
    pub source_identity: [u8; 32],
    pub support_unit: WideRunSupportUnit,
    pub k: u8,
    pub minimizer_length: u8,
    pub virtual_bucket_count: u32,
    pub virtual_bucket: u32,
}

/// Parsed and independently checked run header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideRunHeader {
    pub domain: WideRunDomain,
    pub kind: WideRunKind,
    pub generation: u32,
    pub run_ordinal: u64,
    pub first_event_ordinal: u64,
    pub event_ordinal_end: u64,
    pub record_count: u64,
    pub support_mass: u64,
    pub payload_bytes: u64,
}

/// One complete full-key record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideRunRecord {
    pub key: PackedKmer,
    pub minimizer: PackedKmer,
    /// Source-window ordinal for occurrence runs; support for reduced runs.
    pub value: u64,
}

/// Verified metadata retained after a run has been sealed.
#[derive(Clone, Copy)]
pub struct WideRunMeta {
    pub id: WideRunId,
    pub header: WideRunHeader,
    pub byte_len: u64,
    pub sha256: [u8; 32],
    physical_identity: WideRunPhysicalIdentity,
}

impl std::fmt::Debug for WideRunMeta {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WideRunMeta")
            .field("id", &self.id)
            .field("header", &self.header)
            .field("byte_len", &self.byte_len)
            .field("sha256", &self.sha256)
            .finish()
    }
}

// Physical identity is deliberately operational-only: it protects reads and
// reclamation inside one private run directory, but must not affect scientific
// equality or any digest exposed by the external reducer.
impl PartialEq for WideRunMeta {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.header == other.header
            && self.byte_len == other.byte_len
            && self.sha256 == other.sha256
    }
}

impl Eq for WideRunMeta {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WideRunPhysicalIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
    mode: u32,
    byte_len: u64,
}

impl WideRunPhysicalIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner_uid: metadata.uid(),
            mode: metadata.mode(),
            byte_len: metadata.len(),
        }
    }
}

/// Fixed-size physical identity of one private run inside its owning directory.
///
/// Catalog entries intentionally do not own paths. Retaining and cloning
/// caller-length `PathBuf` values prevents a fixed per-run memory proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WideRunId {
    pub virtual_bucket: u32,
    pub generation: u32,
    pub run_ordinal: u64,
}

impl WideRunId {
    pub(crate) const fn from_plan(plan: WideRunPlan) -> Self {
        Self {
            virtual_bucket: plan.domain.virtual_bucket,
            generation: plan.generation,
            run_ordinal: plan.run_ordinal,
        }
    }

    fn from_header(header: WideRunHeader) -> Self {
        Self {
            virtual_bucket: header.domain.virtual_bucket,
            generation: header.generation,
            run_ordinal: header.run_ordinal,
        }
    }
}

fn private_run_identity(
    metadata: &std::fs::Metadata,
    action: &'static str,
) -> Result<WideRunPhysicalIdentity> {
    if !metadata.is_file() || metadata.mode() & 0o170000 != 0o100000 {
        return integrity(format!("{action}: path is not a regular file"));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return integrity(format!("{action}: private run permissions are not 0600"));
    }
    Ok(WideRunPhysicalIdentity::from_metadata(metadata))
}

fn open_private_run_nofollow(path: &Path, action: &'static str) -> Result<File> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let descriptor = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|cause| {
        VeritasmError::new(ErrorCode::IntegrityCountRun, format!("{action}: {cause}"))
    })?;
    let file = File::from(descriptor);
    private_run_identity(
        &file
            .metadata()
            .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, action, cause))?,
        action,
    )?;
    Ok(file)
}

fn private_run_path_identity(path: &Path, action: &'static str) -> Result<WideRunPhysicalIdentity> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, action, cause))?;
    if metadata.file_type().is_symlink() {
        return integrity(format!("{action}: run path is a symbolic link"));
    }
    private_run_identity(&metadata, action)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WideRunPlan {
    pub domain: WideRunDomain,
    pub kind: WideRunKind,
    pub generation: u32,
    pub run_ordinal: u64,
    pub first_event_ordinal: u64,
    pub event_ordinal_end: u64,
}

/// Streaming writer that seals a run only after rewriting its complete
/// header and hashing the exact header and payload bytes.
pub(crate) struct WideRunWriter<'a> {
    path: &'a Path,
    writer: Option<FallibleBufWriter<File>>,
    domain: WideRunDomain,
    kind: WideRunKind,
    generation: u32,
    run_ordinal: u64,
    first_event_ordinal: u64,
    event_ordinal_end: u64,
    record_count: u64,
    support_mass: u64,
    previous: Option<(PackedKmer, u64)>,
}

impl<'a> WideRunWriter<'a> {
    pub(crate) fn create(
        path: &'a Path,
        plan: WideRunPlan,
        io_buffer_bytes: usize,
    ) -> Result<Self> {
        validate_domain(plan.domain)?;
        if plan.first_event_ordinal >= plan.event_ordinal_end {
            return integrity("experimental wide run must cover a nonempty event-ordinal interval");
        }
        if io_buffer_bytes == 0 {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                "experimental wide-run I/O buffer must be nonzero",
            ));
        }
        // Admit the buffer before creating a persistent file. A rejected
        // allocation therefore cannot strand a path that looks like a run.
        let buffer = allocate_exact_buffer(io_buffer_bytes, "wide-run writer buffer")?;
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|cause| {
                io_error(ErrorCode::ResourceTemporaryBytes, "create wide run", cause)
            })?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "set private wide-run permissions",
                    cause,
                )
            })?;
        private_run_identity(
            &file.metadata().map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "stat created wide run",
                    cause,
                )
            })?,
            "validate created wide run",
        )?;
        let mut writer = FallibleBufWriter::from_buffer(file, buffer);
        writer.write_all(&[0_u8; HEADER_BYTES]).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "write wide-run header reservation",
                cause,
            )
        })?;
        Ok(Self {
            path,
            writer: Some(writer),
            domain: plan.domain,
            kind: plan.kind,
            generation: plan.generation,
            run_ordinal: plan.run_ordinal,
            first_event_ordinal: plan.first_event_ordinal,
            event_ordinal_end: plan.event_ordinal_end,
            record_count: 0,
            support_mass: 0,
            previous: None,
        })
    }

    pub(crate) fn push(&mut self, record: WideRunRecord) -> Result<()> {
        validate_record(self.domain, self.kind, record)?;
        let order_value = match self.kind {
            WideRunKind::Observation => record.value,
            WideRunKind::Reduced => 0,
        };
        let order = (record.key, order_value);
        if self.previous.is_some_and(|previous| previous >= order) {
            return integrity("experimental wide-run records are not strictly ordered");
        }
        if self.kind == WideRunKind::Observation
            && !(self.first_event_ordinal..self.event_ordinal_end).contains(&record.value)
        {
            return integrity("experimental wide-run occurrence lies outside its event interval");
        }
        self.previous = Some(order);
        let bytes = encode_record(record);
        self.writer
            .as_mut()
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "experimental wide-run writer was already finished",
                )
            })?
            .write_all(&bytes)
            .map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "write wide-run record",
                    cause,
                )
            })?;
        self.record_count = checked_add(self.record_count, 1, "wide-run record-count overflow")?;
        let increment = match self.kind {
            WideRunKind::Observation => 1,
            WideRunKind::Reduced => record.value,
        };
        self.support_mass = checked_add(
            self.support_mass,
            increment,
            "wide-run support-mass overflow",
        )?;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<WideRunMeta> {
        if self.record_count == 0 || self.support_mass == 0 {
            return integrity("experimental wide run cannot be empty");
        }
        let payload_bytes = self
            .record_count
            .checked_mul(RUN_RECORD_BYTES)
            .ok_or_else(|| overflow("wide-run payload byte count overflow"))?;
        let header = WideRunHeader {
            domain: self.domain,
            kind: self.kind,
            generation: self.generation,
            run_ordinal: self.run_ordinal,
            first_event_ordinal: self.first_event_ordinal,
            event_ordinal_end: self.event_ordinal_end,
            record_count: self.record_count,
            support_mass: self.support_mass,
            payload_bytes,
        };
        let encoded_header = encode_header(header)?;
        let mut file = self
            .writer
            .take()
            .ok_or_else(|| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "experimental wide-run writer lost its file",
                )
            })?
            .into_inner()
            .map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "flush wide-run payload",
                    cause,
                )
            })?;
        file.seek(SeekFrom::Start(0)).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "seek wide-run header",
                cause,
            )
        })?;
        file.write_all(&encoded_header).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "write wide-run header",
                cause,
            )
        })?;
        file.flush().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "flush wide-run header",
                cause,
            )
        })?;

        let authenticated_bytes = u64::try_from(HEADER_BYTES)
            .map_err(|_| overflow("wide-run header width does not fit in u64"))?
            .checked_add(payload_bytes)
            .ok_or_else(|| overflow("wide-run authenticated byte count overflow"))?;
        file.seek(SeekFrom::Start(0)).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "seek wide run for digest",
                cause,
            )
        })?;
        let mut hasher = Sha256::new();
        hasher.update(RUN_DIGEST_DOMAIN);
        let mut remaining = authenticated_bytes;
        let mut buffer = [0_u8; 16 * 1024];
        while remaining > 0 {
            let wanted = usize::try_from(remaining.min(buffer.len() as u64))
                .map_err(|_| overflow("wide-run digest chunk does not fit in usize"))?;
            file.read_exact(&mut buffer[..wanted]).map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityCountRun,
                    "read wide run while sealing digest",
                    cause,
                )
            })?;
            hasher.update(&buffer[..wanted]);
            remaining -= wanted as u64;
        }
        let digest: [u8; 32] = hasher.finalize().into();
        let byte_len = authenticated_bytes
            .checked_add(TRAILER_BYTES as u64)
            .ok_or_else(|| overflow("wide-run total byte count overflow"))?;
        file.write_all(RUN_END)
            .and_then(|()| file.write_all(&digest))
            .and_then(|()| file.write_all(&byte_len.to_le_bytes()))
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|cause| io_error(ErrorCode::ResourceTemporaryBytes, "seal wide run", cause))?;
        let sealed_identity = private_run_identity(
            &file.metadata().map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityCountRun,
                    "stat sealed wide-run descriptor",
                    cause,
                )
            })?,
            "validate sealed wide-run descriptor",
        )?;
        if sealed_identity.byte_len != byte_len {
            return integrity("sealed wide-run descriptor length changed unexpectedly");
        }

        let verified = verify_run(
            self.path,
            self.domain,
            self.kind,
            RUN_VERIFICATION_BUFFER_BYTES,
        )?;
        if verified.header != header
            || verified.byte_len != byte_len
            || verified.sha256 != digest
            || verified.physical_identity != sealed_identity
        {
            return integrity(
                "sealed experimental wide run disagrees with independent verification",
            );
        }
        Ok(verified)
    }
}

/// Streaming verified reader. A run is authenticated only after [`Self::finish`]
/// consumes and checks its trailer.
pub(crate) struct WideRunReader {
    reader: FallibleBufReader<File>,
    header: WideRunHeader,
    records_read: u64,
    support_read: u64,
    previous: Option<(PackedKmer, u64)>,
    hasher: Sha256,
    byte_len: u64,
    physical_identity: WideRunPhysicalIdentity,
    catalog_binding: Option<WideRunCatalogBinding>,
}

#[derive(Debug, Clone, Copy)]
struct WideRunCatalogBinding {
    header: WideRunHeader,
    byte_len: u64,
    sha256: [u8; 32],
    physical_identity: WideRunPhysicalIdentity,
}

impl WideRunReader {
    fn open(
        path: &Path,
        expected_domain: WideRunDomain,
        expected_kind: WideRunKind,
        io_buffer_bytes: usize,
    ) -> Result<Self> {
        if io_buffer_bytes == 0 {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidLimit,
                "experimental wide-run I/O buffer must be nonzero",
            ));
        }
        let path_identity = private_run_path_identity(path, "stat private wide-run path")?;
        let file = open_private_run_nofollow(path, "open private wide run")?;
        let physical_identity = private_run_identity(
            &file
                .metadata()
                .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, "stat wide run", cause))?,
            "validate opened wide run",
        )?;
        if path_identity != physical_identity {
            return integrity("experimental wide-run path changed while being opened");
        }
        let byte_len = physical_identity.byte_len;
        let mut reader = FallibleBufReader::new(file, io_buffer_bytes)?;
        let mut encoded_header = [0_u8; HEADER_BYTES];
        reader.read_exact(&mut encoded_header).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read experimental wide-run header",
                cause,
            )
        })?;
        let header = decode_header(encoded_header)?;
        if header.domain != expected_domain || header.kind != expected_kind {
            return integrity("experimental wide-run domain or kind mismatch");
        }
        let expected_len = (HEADER_BYTES as u64)
            .checked_add(header.payload_bytes)
            .and_then(|value| value.checked_add(TRAILER_BYTES as u64))
            .ok_or_else(|| overflow("wide-run expected length overflow"))?;
        if byte_len != expected_len {
            return integrity(format!(
                "experimental wide-run length {byte_len} differs from declared {expected_len}"
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(RUN_DIGEST_DOMAIN);
        hasher.update(encoded_header);
        Ok(Self {
            reader,
            header,
            records_read: 0,
            support_read: 0,
            previous: None,
            hasher,
            byte_len,
            physical_identity,
            catalog_binding: None,
        })
    }

    /// Open one run through the exact metadata entry that registered it.
    ///
    /// Header and file-length substitution is rejected before payload records
    /// are exposed. The cataloged SHA-256 is checked by [`Self::finish`], after
    /// the complete payload and trailer have been consumed.
    pub(crate) fn open_registered(
        path: &Path,
        expected: &WideRunMeta,
        expected_domain: WideRunDomain,
        expected_kind: WideRunKind,
        io_buffer_bytes: usize,
    ) -> Result<Self> {
        let mut reader = Self::open(path, expected_domain, expected_kind, io_buffer_bytes)?;
        if reader.header != expected.header
            || reader.byte_len != expected.byte_len
            || WideRunId::from_header(reader.header) != expected.id
            || reader.physical_identity != expected.physical_identity
        {
            return integrity(
                "experimental wide run differs from its registered header or byte length",
            );
        }
        reader.catalog_binding = Some(WideRunCatalogBinding {
            header: expected.header,
            byte_len: expected.byte_len,
            sha256: expected.sha256,
            physical_identity: expected.physical_identity,
        });
        Ok(reader)
    }

    pub(crate) fn next_record(&mut self) -> Result<Option<WideRunRecord>> {
        if self.records_read == self.header.record_count {
            return Ok(None);
        }
        let mut encoded = [0_u8; RUN_RECORD_BYTES as usize];
        self.reader.read_exact(&mut encoded).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read experimental wide-run record",
                cause,
            )
        })?;
        self.hasher.update(encoded);
        let record = decode_record(encoded);
        validate_record(self.header.domain, self.header.kind, record)?;
        let order_value = match self.header.kind {
            WideRunKind::Observation => record.value,
            WideRunKind::Reduced => 0,
        };
        let order = (record.key, order_value);
        if self.previous.is_some_and(|previous| previous >= order) {
            return integrity("experimental wide-run records are not strictly ordered");
        }
        if self.header.kind == WideRunKind::Observation
            && !(self.header.first_event_ordinal..self.header.event_ordinal_end)
                .contains(&record.value)
        {
            return integrity("experimental wide-run occurrence lies outside its event interval");
        }
        self.previous = Some(order);
        self.records_read = checked_add(self.records_read, 1, "wide-run reader record overflow")?;
        let increment = match self.header.kind {
            WideRunKind::Observation => 1,
            WideRunKind::Reduced => record.value,
        };
        self.support_read = checked_add(
            self.support_read,
            increment,
            "wide-run reader support overflow",
        )?;
        Ok(Some(record))
    }

    pub(crate) fn finish(mut self) -> Result<WideRunMeta> {
        while self.next_record()?.is_some() {}
        if self.support_read != self.header.support_mass {
            return integrity("experimental wide-run support subtotal mismatch");
        }
        let mut trailer = [0_u8; TRAILER_BYTES];
        self.reader.read_exact(&mut trailer).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read experimental wide-run trailer",
                cause,
            )
        })?;
        if &trailer[..8] != RUN_END {
            return integrity("experimental wide-run trailer magic mismatch");
        }
        let actual: [u8; 32] = self.hasher.finalize().into();
        if trailer[8..40] != actual {
            return integrity("experimental wide-run SHA-256 mismatch");
        }
        let trailer_len = read_u64(&trailer[40..48]);
        if trailer_len != self.byte_len {
            return integrity("experimental wide-run trailer length mismatch");
        }
        let mut extra = [0_u8; 1];
        if self
            .reader
            .read(&mut extra)
            .map_err(|cause| io_error(ErrorCode::IntegrityCountRun, "check wide-run EOF", cause))?
            != 0
        {
            return integrity("experimental wide run has trailing bytes");
        }
        let verified = WideRunMeta {
            id: WideRunId::from_header(self.header),
            header: self.header,
            byte_len: self.byte_len,
            sha256: actual,
            physical_identity: self.physical_identity,
        };
        if let Some(expected) = self.catalog_binding {
            if verified.header != expected.header
                || verified.byte_len != expected.byte_len
                || verified.sha256 != expected.sha256
                || verified.physical_identity != expected.physical_identity
            {
                return integrity("experimental wide run differs from its registered metadata");
            }
        }
        Ok(verified)
    }
}

pub(crate) fn verify_run(
    path: &Path,
    domain: WideRunDomain,
    kind: WideRunKind,
    io_buffer_bytes: usize,
) -> Result<WideRunMeta> {
    WideRunReader::open(path, domain, kind, io_buffer_bytes)?.finish()
}

/// Remove one run only while the literal path and a no-follow descriptor both
/// still identify the registered private file.
///
/// The private 0700 owner directory prevents other users from participating.
/// As with POSIX pathname unlink generally, a malicious process running as the
/// same UID can still race the final identity check and `unlink`; callers must
/// treat that actor as outside this accidental-substitution boundary.
pub(crate) fn remove_registered_run(path: &Path, expected: &WideRunMeta) -> Result<()> {
    let path_identity = private_run_path_identity(path, "stat registered wide run for cleanup")?;
    let file = open_private_run_nofollow(path, "open registered wide run for cleanup")?;
    let descriptor_identity = private_run_identity(
        &file.metadata().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat registered wide-run cleanup descriptor",
                cause,
            )
        })?,
        "validate registered wide-run cleanup descriptor",
    )?;
    if path_identity != expected.physical_identity
        || descriptor_identity != expected.physical_identity
    {
        return integrity("registered experimental wide-run identity changed before cleanup");
    }
    std::fs::remove_file(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "reclaim identity-verified experimental wide run",
            cause,
        )
    })
}

pub(crate) fn projected_run_bytes(record_count: u64) -> Result<u64> {
    (HEADER_BYTES as u64)
        .checked_add(
            record_count
                .checked_mul(RUN_RECORD_BYTES)
                .ok_or_else(|| overflow("projected wide-run payload overflow"))?,
        )
        .and_then(|value| value.checked_add(TRAILER_BYTES as u64))
        .ok_or_else(|| overflow("projected wide-run byte count overflow"))
}

fn validate_domain(domain: WideRunDomain) -> Result<()> {
    super::wide_kmer::validate_k(domain.k)?;
    if domain.minimizer_length == 0 || domain.minimizer_length > domain.k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental run minimizer length must be in 1..={}; received {}",
                domain.k, domain.minimizer_length
            ),
        ));
    }
    if domain.virtual_bucket_count == 0 || domain.virtual_bucket >= domain.virtual_bucket_count {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!(
                "experimental run bucket {} is outside virtual bucket count {}",
                domain.virtual_bucket, domain.virtual_bucket_count
            ),
        ));
    }
    Ok(())
}

fn validate_record(domain: WideRunDomain, kind: WideRunKind, record: WideRunRecord) -> Result<()> {
    if validate_code(record.key, domain.k).is_err() {
        return integrity("experimental wide-run key has inactive bits or an invalid length");
    }
    if canonical_code(record.key, domain.k).map_err(|_| {
        VeritasmError::new(
            ErrorCode::IntegrityCountRun,
            "experimental wide-run key cannot be canonicalized",
        )
    })? != record.key
    {
        return integrity("experimental wide-run key is not canonical");
    }
    let owner = select_minimizer(record.key, domain.k, domain.minimizer_length).map_err(|_| {
        VeritasmError::new(
            ErrorCode::IntegrityCountRun,
            "experimental wide-run minimizer owner cannot be recomputed",
        )
    })?;
    if owner.key != record.minimizer {
        return integrity("experimental wide-run key has the wrong complete minimizer");
    }
    if route_minimizer(
        record.minimizer,
        domain.minimizer_length,
        domain.virtual_bucket_count,
    )
    .map_err(|_| {
        VeritasmError::new(
            ErrorCode::IntegrityCountRun,
            "experimental wide-run minimizer route cannot be recomputed",
        )
    })? != domain.virtual_bucket
    {
        return integrity("experimental wide-run minimizer has the wrong virtual bucket");
    }
    if kind == WideRunKind::Reduced && record.value == 0 {
        return integrity("experimental reduced wide-run record has zero support");
    }
    Ok(())
}

fn encode_header(header: WideRunHeader) -> Result<[u8; HEADER_BYTES]> {
    validate_domain(header.domain)?;
    if header.record_count == 0
        || header.support_mass == 0
        || header.first_event_ordinal >= header.event_ordinal_end
    {
        return integrity("experimental wide-run header contains an empty range or count");
    }
    let expected_payload = header
        .record_count
        .checked_mul(RUN_RECORD_BYTES)
        .ok_or_else(|| overflow("wide-run header payload overflow"))?;
    if header.payload_bytes != expected_payload {
        return integrity("experimental wide-run header payload length mismatch");
    }
    if header.kind == WideRunKind::Observation && header.record_count != header.support_mass {
        return integrity("observation wide-run record and support totals differ");
    }
    let mut bytes = [0_u8; HEADER_BYTES];
    bytes[..8].copy_from_slice(RUN_MAGIC);
    bytes[8..10].copy_from_slice(&RUN_SCHEMA.to_le_bytes());
    bytes[10] = INTEGER_KEY_ENCODING;
    bytes[11] = header.kind as u8;
    bytes[12] = PACKED_KEY_BYTES as u8;
    bytes[13] = header.domain.k;
    bytes[14] = header.domain.minimizer_length;
    bytes[15] = header.domain.support_unit as u8;
    bytes[16..20].copy_from_slice(&header.domain.virtual_bucket.to_le_bytes());
    bytes[20..24].copy_from_slice(&header.generation.to_le_bytes());
    bytes[24..32].copy_from_slice(&header.run_ordinal.to_le_bytes());
    bytes[32..64].copy_from_slice(&header.domain.source_identity);
    bytes[64..72].copy_from_slice(&header.first_event_ordinal.to_le_bytes());
    bytes[72..80].copy_from_slice(&header.event_ordinal_end.to_le_bytes());
    bytes[80..88].copy_from_slice(&header.record_count.to_le_bytes());
    bytes[88..96].copy_from_slice(&header.support_mass.to_le_bytes());
    bytes[96..104].copy_from_slice(&header.payload_bytes.to_le_bytes());
    bytes[104..108].copy_from_slice(&header.domain.virtual_bucket_count.to_le_bytes());
    Ok(bytes)
}

fn decode_header(bytes: [u8; HEADER_BYTES]) -> Result<WideRunHeader> {
    if &bytes[..8] != RUN_MAGIC {
        return integrity("experimental wide-run magic mismatch");
    }
    if read_u16(&bytes[8..10]) != RUN_SCHEMA {
        return integrity("experimental wide-run schema mismatch");
    }
    if bytes[10] != INTEGER_KEY_ENCODING || bytes[12] != PACKED_KEY_BYTES as u8 {
        return integrity("experimental wide-run encoding-domain mismatch");
    }
    if bytes[108..].iter().any(|&byte| byte != 0) {
        return integrity("experimental wide-run reserved header bytes are nonzero");
    }
    let header = WideRunHeader {
        domain: WideRunDomain {
            source_identity: bytes[32..64].try_into().map_err(|_| {
                VeritasmError::new(
                    ErrorCode::InternalInvariant,
                    "wide-run source identity slice has the wrong width",
                )
            })?,
            support_unit: WideRunSupportUnit::from_tag(bytes[15])?,
            k: bytes[13],
            minimizer_length: bytes[14],
            virtual_bucket_count: read_u32(&bytes[104..108]),
            virtual_bucket: read_u32(&bytes[16..20]),
        },
        kind: WideRunKind::from_tag(bytes[11])?,
        generation: read_u32(&bytes[20..24]),
        run_ordinal: read_u64(&bytes[24..32]),
        first_event_ordinal: read_u64(&bytes[64..72]),
        event_ordinal_end: read_u64(&bytes[72..80]),
        record_count: read_u64(&bytes[80..88]),
        support_mass: read_u64(&bytes[88..96]),
        payload_bytes: read_u64(&bytes[96..104]),
    };
    // Re-encoding centralizes all structural checks and also proves that no
    // accepted non-reserved byte has an alternative representation.
    let canonical = encode_header(header).map_err(|_| {
        VeritasmError::new(
            ErrorCode::IntegrityCountRun,
            "experimental wide-run header contains invalid structural fields",
        )
    })?;
    if canonical != bytes {
        return integrity("experimental wide-run header is not canonical");
    }
    Ok(header)
}

fn encode_record(record: WideRunRecord) -> [u8; RUN_RECORD_BYTES as usize] {
    let mut bytes = [0_u8; RUN_RECORD_BYTES as usize];
    bytes[..32].copy_from_slice(&record.key.to_be_bytes());
    bytes[32..64].copy_from_slice(&record.minimizer.to_be_bytes());
    bytes[64..72].copy_from_slice(&record.value.to_le_bytes());
    bytes
}

fn decode_record(bytes: [u8; RUN_RECORD_BYTES as usize]) -> WideRunRecord {
    WideRunRecord {
        key: PackedKmer::from_be_bytes(bytes[..32].try_into().expect("fixed key width")),
        minimizer: PackedKmer::from_be_bytes(
            bytes[32..64].try_into().expect("fixed minimizer width"),
        ),
        value: read_u64(&bytes[64..72]),
    }
}

fn read_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().expect("fixed u16 width"))
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("fixed u32 width"))
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("fixed u64 width"))
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn overflow(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn integrity<T>(context: impl Into<String>) -> Result<T> {
    Err(VeritasmError::new(ErrorCode::IntegrityCountRun, context))
}

fn io_error(code: ErrorCode, action: &'static str, cause: std::io::Error) -> VeritasmError {
    VeritasmError::new(code, format!("cannot {action}: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimental::partitioned_dbg::{independent_window_oracle, route_minimizer};
    use std::fs;
    use std::io::{Cursor, Read, Write};
    use std::os::unix::fs::PermissionsExt;

    fn domain(window: &[u8], bucket_count: u32) -> (WideRunDomain, WideRunRecord) {
        let expected =
            independent_window_oracle(window, window.len() as u8, 3, bucket_count).unwrap();
        (
            WideRunDomain {
                source_identity: [7; 32],
                support_unit: WideRunSupportUnit::AcceptedWindowOccurrence,
                k: window.len() as u8,
                minimizer_length: 3,
                virtual_bucket_count: bucket_count,
                virtual_bucket: expected.bucket_id,
            },
            WideRunRecord {
                key: expected.key,
                minimizer: expected.owner.key,
                value: 4,
            },
        )
    }

    fn plan(
        domain: WideRunDomain,
        kind: WideRunKind,
        generation: u32,
        run_ordinal: u64,
        first_event_ordinal: u64,
        event_ordinal_end: u64,
    ) -> WideRunPlan {
        WideRunPlan {
            domain,
            kind,
            generation,
            run_ordinal,
            first_event_ordinal,
            event_ordinal_end,
        }
    }

    #[test]
    fn fallible_fixed_buffers_round_trip_without_capacity_growth() {
        let payload: Vec<u8> = (0_u16..4097)
            .map(|value| u8::try_from(value % 251).unwrap())
            .collect();
        for capacity in [1_usize, 2, 7, 64, 257] {
            let buffer = allocate_exact_buffer(capacity, "test writer").unwrap();
            assert_eq!(buffer.capacity(), capacity);
            let mut writer = FallibleBufWriter::from_buffer(Vec::new(), buffer);
            writer.write_all(&payload).unwrap();
            writer.flush().unwrap();
            assert_eq!(writer.into_inner().unwrap(), payload, "capacity {capacity}");

            let mut reader = FallibleBufReader::new(Cursor::new(&payload), capacity).unwrap();
            let mut decoded = Vec::new();
            reader.read_to_end(&mut decoded).unwrap();
            assert_eq!(decoded, payload, "capacity {capacity}");
        }
    }

    #[test]
    fn impossible_buffer_admission_is_typed_and_precedes_file_creation() {
        assert_eq!(
            allocate_exact_buffer(0, "test buffer").unwrap_err().code(),
            ErrorCode::ConfigurationInvalidLimit
        );
        assert_eq!(
            allocate_exact_buffer(usize::MAX, "test buffer")
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("must-not-exist.xwr");
        let (domain, _) = domain(b"AACGT", 11);
        assert_eq!(
            WideRunWriter::create(
                &path,
                plan(domain, WideRunKind::Observation, 0, 2, 4, 5),
                usize::MAX,
            )
            .err()
            .expect("impossible allocation must fail")
            .code(),
            ErrorCode::ResourceMemory
        );
        assert!(!path.exists());
    }

    #[test]
    fn fixed_width_run_round_trips_and_binds_source() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("roundtrip.xwr");
        let (domain, record) = domain(b"AACGT", 11);
        let mut writer = WideRunWriter::create(
            &path,
            plan(domain, WideRunKind::Observation, 0, 2, 4, 5),
            128,
        )
        .unwrap();
        writer.push(record).unwrap();
        let meta = writer.finish().unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(meta.header.record_count, 1);
        assert_eq!(meta.header.support_mass, 1);
        assert_eq!(meta.byte_len, projected_run_bytes(1).unwrap());
        assert_eq!(
            verify_run(&path, domain, WideRunKind::Observation, 127)
                .unwrap()
                .sha256,
            meta.sha256
        );
        let mut wrong = domain;
        wrong.source_identity[0] ^= 1;
        assert_eq!(
            verify_run(&path, wrong, WideRunKind::Observation, 128)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn create_new_collision_preserves_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("collision.xwr");
        fs::write(&path, b"existing").unwrap();
        let (domain, _) = domain(b"AACGT", 11);
        let error = WideRunWriter::create(
            &path,
            plan(domain, WideRunKind::Observation, 0, 2, 4, 5),
            128,
        )
        .err()
        .expect("create-new collision must fail");
        assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
        assert_eq!(fs::read(path).unwrap(), b"existing");
    }

    #[test]
    fn registered_reader_and_cleanup_reject_inode_substitution() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("registered.xwr");
        let displaced = temp.path().join("registered-original.xwr");
        let (domain, record) = domain(b"AACGT", 11);
        let mut writer = WideRunWriter::create(
            &path,
            plan(domain, WideRunKind::Observation, 0, 2, 4, 5),
            128,
        )
        .unwrap();
        writer.push(record).unwrap();
        let meta = writer.finish().unwrap();
        fs::rename(&path, &displaced).unwrap();
        fs::copy(&displaced, &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(
            WideRunReader::open_registered(&path, &meta, domain, WideRunKind::Observation, 128,)
                .err()
                .expect("replacement inode must not be read")
                .code(),
            ErrorCode::IntegrityCountRun
        );
        assert_eq!(
            remove_registered_run(&path, &meta).unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );
        assert!(path.exists());
        assert!(displaced.exists());
    }

    #[test]
    fn nofollow_reader_rejects_symbolic_link() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("target.xwr");
        let link = temp.path().join("link.xwr");
        let (domain, record) = domain(b"AACGT", 11);
        let mut writer = WideRunWriter::create(
            &path,
            plan(domain, WideRunKind::Observation, 0, 2, 4, 5),
            128,
        )
        .unwrap();
        writer.push(record).unwrap();
        writer.finish().unwrap();
        symlink(&path, &link).unwrap();
        assert_eq!(
            verify_run(&link, domain, WideRunKind::Observation, 128)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn corruption_truncation_schema_endian_and_trailing_bytes_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.xwr");
        let (domain, record) = domain(b"AACGT", 7);
        let mut writer = WideRunWriter::create(
            &original,
            plan(domain, WideRunKind::Observation, 0, 0, 4, 5),
            64,
        )
        .unwrap();
        writer.push(record).unwrap();
        writer.finish().unwrap();
        let bytes = fs::read(&original).unwrap();

        for (name, mutation) in [
            ("schema", 8_usize),
            ("encoding", 10),
            ("payload", HEADER_BYTES + 3),
            ("trailer", bytes.len() - 1),
        ] {
            let path = temp.path().join(format!("{name}.xwr"));
            let mut changed = bytes.clone();
            changed[mutation] ^= 1;
            fs::write(&path, changed).unwrap();
            assert_eq!(
                verify_run(&path, domain, WideRunKind::Observation, 64)
                    .unwrap_err()
                    .code(),
                ErrorCode::IntegrityCountRun,
                "mutation {name}"
            );
        }

        for cut in [0, 1, HEADER_BYTES - 1, HEADER_BYTES, bytes.len() - 1] {
            let path = temp.path().join(format!("truncated-{cut}.xwr"));
            fs::write(&path, &bytes[..cut]).unwrap();
            assert_eq!(
                verify_run(&path, domain, WideRunKind::Observation, 64)
                    .unwrap_err()
                    .code(),
                ErrorCode::IntegrityCountRun
            );
        }

        let trailing = temp.path().join("trailing.xwr");
        fs::write(&trailing, &bytes).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&trailing)
            .unwrap()
            .write_all(b"x")
            .unwrap();
        assert_eq!(
            verify_run(&trailing, domain, WideRunKind::Observation, 64)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn noncanonical_and_wrong_owner_records_are_rejected_before_sealing() {
        let temp = tempfile::tempdir().unwrap();
        let (domain, record) = domain(b"AACGT", 13);
        let reverse =
            super::super::wide_kmer::reverse_complement_code(record.key, domain.k).unwrap();
        assert_ne!(reverse, record.key);
        let noncanonical_path = temp.path().join("noncanonical.xwr");
        let mut writer = WideRunWriter::create(
            &noncanonical_path,
            plan(domain, WideRunKind::Observation, 0, 0, 4, 5),
            64,
        )
        .unwrap();
        assert_eq!(
            writer
                .push(WideRunRecord {
                    key: reverse,
                    ..record
                })
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityCountRun
        );

        let wrong_owner = independent_window_oracle(b"CCCCC", 5, 3, 13).unwrap();
        assert_ne!(wrong_owner.owner.key, record.minimizer);
        let owner_path = temp.path().join("owner.xwr");
        let mut writer = WideRunWriter::create(
            &owner_path,
            plan(domain, WideRunKind::Observation, 0, 1, 4, 5),
            64,
        )
        .unwrap();
        assert_eq!(
            writer
                .push(WideRunRecord {
                    minimizer: wrong_owner.owner.key,
                    ..record
                })
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn header_and_record_numeric_fields_use_declared_endianness() {
        let header = WideRunHeader {
            domain: WideRunDomain {
                source_identity: [0xab; 32],
                support_unit: WideRunSupportUnit::AcceptedWindowOccurrence,
                k: 5,
                minimizer_length: 3,
                virtual_bucket_count: 0x0102_0305,
                virtual_bucket: 0x0102_0304,
            },
            kind: WideRunKind::Reduced,
            generation: 0x0506_0708,
            run_ordinal: 0x0102_0304_0506_0708,
            first_event_ordinal: 1,
            event_ordinal_end: 2,
            record_count: 1,
            support_mass: 0x1112_1314_1516_1718,
            payload_bytes: RUN_RECORD_BYTES,
        };
        let bytes = encode_header(header).unwrap();
        assert_eq!(&bytes[16..20], &[4, 3, 2, 1]);
        assert_eq!(&bytes[20..24], &[8, 7, 6, 5]);
        assert_eq!(&bytes[24..32], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(&bytes[104..108], &[5, 3, 2, 1]);
        assert_eq!(decode_header(bytes).unwrap(), header);

        let expected = independent_window_oracle(b"AACGT", 5, 3, 9).unwrap();
        let record = WideRunRecord {
            key: expected.key,
            minimizer: expected.owner.key,
            value: 0x0102_0304_0506_0708,
        };
        let bytes = encode_record(record);
        assert_eq!(&bytes[..32], &record.key.to_be_bytes());
        assert_eq!(&bytes[64..72], &[8, 7, 6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn reduced_writer_rejects_duplicate_full_keys_even_if_counts_differ() {
        let temp = tempfile::tempdir().unwrap();
        let (domain, mut record) = domain(b"AACGT", 17);
        record.value = 2;
        let duplicate_path = temp.path().join("duplicate.xwr");
        let mut writer = WideRunWriter::create(
            &duplicate_path,
            plan(domain, WideRunKind::Reduced, 1, 0, 0, 8),
            64,
        )
        .unwrap();
        writer.push(record).unwrap();
        record.value = 3;
        assert_eq!(
            writer.push(record).unwrap_err().code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn routing_reference_is_complete_key_not_bucket_identity() {
        let first = independent_window_oracle(b"AACGT", 5, 3, 1).unwrap();
        let second = independent_window_oracle(b"CCGTA", 5, 3, 1).unwrap();
        assert_eq!(first.bucket_id, second.bucket_id);
        assert_ne!(first.key, second.key);
        assert_eq!(route_minimizer(first.owner.key, 3, 1).unwrap(), 0);
    }
}
