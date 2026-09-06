//! Exact external topology construction for the experimental wide-key graph.
//!
//! This module implements EC-1a from ADR 0024: an explicitly unverified,
//! capped oracle adapter turns an already materialized retained-edge table
//! into an authenticated `EDGE` stream, expands exact oriented handles into
//! `INCIDENCE` spill runs, and globally reduces those runs to literal
//! `NODE_STATE` records. It stops before chunking or local unitig compaction
//! and is not used by the stable CLI.
//!
//! Every identity-dependent operation uses complete packed DNA strings.
//! Minimizers route node states only.  The block digests below detect cache
//! corruption and mix-ups; they are not authentication against an actor able
//! to replace both data and expected digests.

use super::external_reduce::ExternalPartitionResult;
use super::external_run::WideRunSupportUnit;
use super::partitioned_dbg::{route_minimizer, select_minimizer};
use super::wide_kmer::{
    canonical_code, prefix_code, reverse_complement_code, suffix_code, validate_code, validate_k,
    PackedKmer,
};
use crate::error::{ErrorCode, Result, VeritasmError};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::mem::size_of;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const FILE_MAGIC: &[u8; 8] = b"VTEBLK01";
const BLOCK_MAGIC: &[u8; 8] = b"VTBLOCK1";
const TRAILER_MAGIC: &[u8; 8] = b"VTBLKEND";
const SCHEMA: u16 = 1;
const FILE_HEADER_BYTES: usize = 192;
const BLOCK_HEADER_BYTES: usize = 64;
const BLOCK_DIGEST_BYTES: usize = 32;
const TRAILER_BYTES: usize = 104;
const FILE_ROOT_DOMAIN: &[u8] = b"veritasm:external-cdbg:file-root:v1\0";
const BLOCK_ROOT_DOMAIN: &[u8] = b"veritasm:external-cdbg:block-root:v1\0";
const PARENT_ROOT_DOMAIN: &[u8] = b"veritasm:external-cdbg:parent-set:v1\0";
const EDGE_TABLE_ROOT_DOMAIN: &[u8] = b"veritasm:external-cdbg:edge-table:v1\0";
const NODE_TABLE_ROOT_DOMAIN: &[u8] = b"veritasm:external-cdbg:node-table:v1\0";
const NODE_SYMMETRY_ROOT_DOMAIN: &[u8] = b"veritasm:external-cdbg:node-symmetry:v1\0";
const ANCESTRY_ROOT_DOMAIN: &[u8] = b"veritasm:external-cdbg:ancestry:v1\0";
const RUN_DIRECTORY_PREFIX: &str = "experimental-external-cdbg-runs-";
const RUN_DIRECTORY_RANDOM_BYTES: usize = 16;
const RUN_FILENAME_BYTES: usize = 42;
const GLOBAL_PARTITION: u32 = u32::MAX;
const RESERVED_HEADER_BYTES: usize = 34;
const RESERVED_BLOCK_BYTES: usize = 8;
const PATH_ALLOWANCE_BYTES: u64 = 128;
const RUN_CATALOG_MULTIPLIER: u64 = 3;
const ANCESTRY_LINK_MULTIPLIER: u64 = 2;

/// Physical packed-key width authenticated by each external topology run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ExternalKeyWidth {
    W64 = 1,
    W128 = 2,
    W256 = 3,
}

impl ExternalKeyWidth {
    fn for_length(length: u8) -> Result<Self> {
        if length <= 31 {
            Ok(Self::W64)
        } else if length <= 63 {
            Ok(Self::W128)
        } else if length <= 127 {
            Ok(Self::W256)
        } else {
            Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidK,
                format!("external cDBG packed length exceeds 127: {length}"),
            ))
        }
    }

    const fn bytes(self) -> usize {
        match self {
            Self::W64 => 8,
            Self::W128 => 16,
            Self::W256 => 32,
        }
    }

    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::W64),
            2 => Ok(Self::W128),
            3 => Ok(Self::W256),
            _ => integrity(format!("unknown external cDBG key-width tag {tag}")),
        }
    }
}

/// VTEBLK01 record families implemented by EC-1a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum ExternalRecordKind {
    Edge = 1,
    Incidence = 2,
    NodeState = 3,
    /// Internal literal-node projection used to verify reverse-complement
    /// symmetry and the plan-independent node-table root.
    NodeAudit = 4,
}

impl ExternalRecordKind {
    fn from_tag(tag: u16) -> Result<Self> {
        match tag {
            1 => Ok(Self::Edge),
            2 => Ok(Self::Incidence),
            3 => Ok(Self::NodeState),
            4 => Ok(Self::NodeAudit),
            _ => integrity(format!("unknown external cDBG record-kind tag {tag}")),
        }
    }
}

/// Direction of one literal handle at a literal `(k-1)`-mer node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum IncidenceDirection {
    Incoming = 0,
    Outgoing = 1,
}

impl IncidenceDirection {
    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            0 => Ok(Self::Incoming),
            1 => Ok(Self::Outgoing),
            _ => integrity(format!("unknown external cDBG incidence direction {tag}")),
        }
    }
}

/// Hard limits for one isolated EC-1a construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalCdbgLimits {
    pub max_memory_bytes: u64,
    pub sort_buffer_bytes: u64,
    pub max_temp_bytes: u64,
    pub max_run_files: u64,
    pub max_edges: u64,
    pub max_handles: u64,
    pub max_incidences: u64,
    pub max_nodes: u64,
    pub merge_fan_in: u16,
    pub max_open_files: u16,
    pub block_payload_bytes: u32,
}

/// Complete scientific binding and operational plan for EC-1a.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCdbgOptions {
    pub work_dir: PathBuf,
    pub k: u8,
    pub node_minimizer_length: u8,
    pub virtual_partition_count: u32,
    pub source_root: [u8; 32],
    pub scientific_config_root: [u8; 32],
    pub support_unit: WideRunSupportUnit,
    pub limits: ExternalCdbgLimits,
}

/// One retained canonical k-mer and its checked support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalEdgeRecord {
    pub key: PackedKmer,
    pub support: u64,
}

/// One exact literal-node incidence of one oriented handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalIncidenceRecord {
    pub node: PackedKmer,
    pub handle: PackedKmer,
    pub direction: IncidenceDirection,
    pub fixed_edge: bool,
}

/// Globally reduced state for one exact, noncanonicalized literal node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalNodeStateRecord {
    pub node: PackedKmer,
    pub owner_minimizer: PackedKmer,
    pub owner_partition: u32,
    pub incoming_mask: u8,
    pub outgoing_mask: u8,
    pub reverse_fixed: bool,
    pub incident_fixed_edge: bool,
    pub hard_boundary: bool,
}

/// Explicit acknowledgement that a materialized reducer result is only a
/// capped oracle adapter, not authenticated proof that its rows derive from
/// the named read source.
///
/// The adapter validates complete keys, routing, support totals, and source
/// labels. It cannot authenticate the derivation because
/// [`ExternalPartitionResult`] does not own its predecessor run bytes. A
/// production provenance path must consume an owned authenticated stream.
#[derive(Debug, Clone, Copy)]
pub struct UnverifiedMaterializedEdgeAdapter<'a> {
    input: &'a ExternalPartitionResult,
}

impl<'a> UnverifiedMaterializedEdgeAdapter<'a> {
    /// Acknowledge the unverified provenance boundary around `input`.
    pub const fn new(input: &'a ExternalPartitionResult) -> Self {
        Self { input }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExternalNodeAuditRecord {
    node: PackedKmer,
    incoming_mask: u8,
    outgoing_mask: u8,
    reverse_fixed: bool,
    incident_fixed_edge: bool,
    hard_boundary: bool,
}

/// Path-free numeric identity of one private run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalRunId {
    pub kind: ExternalRecordKind,
    pub generation: u32,
    pub ordinal: u64,
}

/// Authenticated public summary of a retained run.  It never owns a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalRunSummary {
    pub id: ExternalRunId,
    pub record_count: u64,
    pub payload_bytes: u64,
    pub byte_len: u64,
    pub content_root: [u8; 32],
    pub parent_set_root: [u8; 32],
}

/// One complete immediate-parent row in the path-free replacement ledger.
/// Rows are ordered by child run ID and then parent run ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalAncestryLink {
    pub child_id: ExternalRunId,
    pub child_content_root: [u8; 32],
    pub child_parent_set_root: [u8; 32],
    pub parent_id: ExternalRunId,
    pub parent_virtual_partition: u32,
    pub parent_content_root: [u8; 32],
}

/// Checked conservation and operational evidence from EC-1a.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalCdbgStats {
    pub retained_edges: u64,
    pub support_mass: u64,
    pub oriented_handles: u64,
    pub incidences: u64,
    pub literal_nodes: u64,
    pub fixed_edges: u64,
    pub hard_boundary_nodes: u64,
    pub reverse_complement_nodes_verified: u64,
    pub ancestry_links: u64,
    pub run_files_created: u64,
    pub predecessor_files_reclaimed: u64,
    pub temporary_bytes_final: u64,
    pub temporary_bytes_high_water: u64,
    pub open_files_high_water: u16,
}

/// Owned authenticated topology seam produced by EC-1a.
///
/// Only the final `EDGE` and `NODE_STATE` files survive success.  Dropping the
/// owner removes its private run directory on a best-effort basis; callers
/// that require an error on cleanup should call [`Self::cleanup`].
#[derive(Debug)]
pub struct ExternalCdbgTopology {
    run_dir: PathBuf,
    run_dir_identity: DirectoryIdentity,
    edge_run: RunMeta,
    node_run: RunMeta,
    block_payload_bytes: usize,
    max_cleanup_entries: u64,
    armed: bool,
    k: u8,
    node_minimizer_length: u8,
    virtual_partition_count: u32,
    source_root: [u8; 32],
    scientific_config_root: [u8; 32],
    support_unit: WideRunSupportUnit,
    edge_table_root: [u8; 32],
    node_table_root: [u8; 32],
    node_symmetry_root: [u8; 32],
    edge_run_summary: ExternalRunSummary,
    node_run_summary: ExternalRunSummary,
    incidence_content_root: [u8; 32],
    ancestry_root: [u8; 32],
    ancestry_links: Vec<ExternalAncestryLink>,
    stats: ExternalCdbgStats,
}

impl ExternalCdbgTopology {
    /// Graph k-mer length authenticated by every retained run.
    pub const fn k(&self) -> u8 {
        self.k
    }

    /// Operational literal-node minimizer length.
    pub const fn node_minimizer_length(&self) -> u8 {
        self.node_minimizer_length
    }

    /// Operational virtual owner-partition count.
    pub const fn virtual_partition_count(&self) -> u32 {
        self.virtual_partition_count
    }

    /// Accepted-source identity bound by the scientific table roots.
    pub const fn source_root(&self) -> [u8; 32] {
        self.source_root
    }

    /// Scientific-configuration identity bound by every retained run.
    pub const fn scientific_config_root(&self) -> [u8; 32] {
        self.scientific_config_root
    }

    /// Exact support unit authenticated by every retained run.
    pub const fn support_unit(&self) -> WideRunSupportUnit {
        self.support_unit
    }

    /// Plan-independent canonical retained-edge table root.
    pub const fn edge_table_root(&self) -> [u8; 32] {
        self.edge_table_root
    }

    /// Plan-independent literal-node table root recomputed from the final run.
    pub const fn node_table_root(&self) -> [u8; 32] {
        self.node_table_root
    }

    /// Plan-independent evidence that the literal node table passed the exact
    /// reverse-complement mask and flag audit.
    pub const fn node_symmetry_root(&self) -> [u8; 32] {
        self.node_symmetry_root
    }

    /// Authenticated retained EDGE run summary.
    pub const fn edge_run_summary(&self) -> ExternalRunSummary {
        self.edge_run_summary
    }

    /// Authenticated retained NODE_STATE run summary.
    pub const fn node_run_summary(&self) -> ExternalRunSummary {
        self.node_run_summary
    }

    /// Content root of the fully reduced incidence predecessor.
    pub const fn incidence_content_root(&self) -> [u8; 32] {
        self.incidence_content_root
    }

    /// Canonical root of the complete immediate-parent ledger.
    pub const fn ancestry_root(&self) -> [u8; 32] {
        self.ancestry_root
    }

    /// Complete path-free immediate-parent ledger.
    pub fn ancestry_links(&self) -> &[ExternalAncestryLink] {
        &self.ancestry_links
    }

    /// Checked conservation and operational evidence for this construction.
    pub const fn stats(&self) -> ExternalCdbgStats {
        self.stats
    }

    /// Visit every retained edge, authenticating the complete stream before
    /// returning success.
    pub fn visit_edges<F>(&self, mut visitor: F) -> Result<()>
    where
        F: FnMut(ExternalEdgeRecord) -> Result<()>,
    {
        let mut reader = BlockReader::<ExternalEdgeRecord>::open_registered(
            &run_path(&self.run_dir, self.edge_run.id)?,
            self.edge_run,
            self.block_payload_bytes,
            self.virtual_partition_count,
        )?;
        while let Some(record) = reader.next_record()? {
            visitor(record)?;
        }
        reader.finish()
    }

    /// Visit every node state in deterministic `(owner_partition,node)` order,
    /// authenticating the complete stream before returning success.
    pub fn visit_node_states<F>(&self, mut visitor: F) -> Result<()>
    where
        F: FnMut(ExternalNodeStateRecord) -> Result<()>,
    {
        let mut reader = BlockReader::<ExternalNodeStateRecord>::open_registered(
            &run_path(&self.run_dir, self.node_run.id)?,
            self.node_run,
            self.block_payload_bytes,
            self.virtual_partition_count,
        )?;
        while let Some(record) = reader.next_record()? {
            visitor(record)?;
        }
        reader.finish()
    }

    /// Materialize a small node table only when the caller supplies a cap.
    pub fn materialize_node_states(
        &self,
        max_records: u64,
    ) -> Result<Vec<ExternalNodeStateRecord>> {
        if self.node_run.header.record_count > max_records {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRetainedKeys,
                format!(
                    "external cDBG node table has {} records, exceeding materialization cap {max_records}",
                    self.node_run.header.record_count
                ),
            ));
        }
        let capacity = as_usize(
            self.node_run.header.record_count,
            "node materialization count",
        )?;
        let mut records = Vec::new();
        records.try_reserve_exact(capacity).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve capped external cDBG node materialization: {cause}"),
            )
        })?;
        enforce_capacity(records.capacity(), capacity, "node materialization")?;
        self.visit_node_states(|record| {
            records.push(record);
            Ok(())
        })?;
        Ok(records)
    }

    /// Remove all retained private files and surface any cleanup failure.
    pub fn cleanup(mut self) -> Result<()> {
        let cleanup = remove_owned_run_directory(
            &self.run_dir,
            self.run_dir_identity,
            self.max_cleanup_entries,
        );
        self.armed = false;
        cleanup.map_err(|error| {
            VeritasmError::new(
                error.code(),
                format!(
                    "cannot clean retained external cDBG private directory {}: {error}",
                    self.run_dir.display()
                ),
            )
        })
    }
}

impl Drop for ExternalCdbgTopology {
    fn drop(&mut self) {
        if self.armed {
            let _ = remove_owned_run_directory(
                &self.run_dir,
                self.run_dir_identity,
                self.max_cleanup_entries,
            );
        }
    }
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

    // rustix exposes native stat field widths, which differ between Linux
    // and Darwin; conversions that are checked on one target are identities
    // on another.
    #[allow(clippy::useless_conversion)]
    fn from_stat(stat: &rustix::fs::Stat) -> Result<Self> {
        Ok(Self {
            device: u64::try_from(stat.st_dev)
                .map_err(|_| overflow("external cDBG directory device does not fit u64"))?,
            inode: u64::try_from(stat.st_ino)
                .map_err(|_| overflow("external cDBG directory inode does not fit u64"))?,
            owner_uid: u32::try_from(stat.st_uid)
                .map_err(|_| overflow("external cDBG directory owner does not fit u32"))?,
            mode: u32::from(stat.st_mode),
        })
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

    // See DirectoryIdentity::from_stat for the cross-target width rationale.
    #[allow(clippy::useless_conversion)]
    fn from_stat(stat: &rustix::fs::Stat) -> Result<Self> {
        Ok(Self {
            device: u64::try_from(stat.st_dev)
                .map_err(|_| overflow("external cDBG cleanup device does not fit u64"))?,
            inode: u64::try_from(stat.st_ino)
                .map_err(|_| overflow("external cDBG cleanup inode does not fit u64"))?,
            owner_uid: u32::try_from(stat.st_uid)
                .map_err(|_| overflow("external cDBG cleanup owner does not fit u32"))?,
            mode: u32::from(stat.st_mode),
            byte_len: u64::try_from(stat.st_size)
                .map_err(|_| integrity_error("external cDBG cleanup file has negative length"))?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DescriptorIdentity {
    device: u64,
    inode: u64,
    byte_len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl DescriptorIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            byte_len: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

fn open_existing_regular_nofollow(
    path: &Path,
    writable: bool,
    code: ErrorCode,
    operation: &str,
) -> Result<File> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let access = if writable {
        OFlags::RDWR
    } else {
        OFlags::RDONLY
    };
    let descriptor = openat(
        CWD,
        path,
        access | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|cause| VeritasmError::new(code, format!("{operation}: {cause}")))?;
    let file = File::from(descriptor);
    if !file
        .metadata()
        .map_err(|cause| io_error(code, operation, cause))?
        .is_file()
    {
        return integrity(format!("{operation}: path is not a regular file"));
    }
    Ok(file)
}

fn regular_path_identity(
    path: &Path,
    code: ErrorCode,
    operation: &str,
) -> Result<DescriptorIdentity> {
    let metadata = fs::symlink_metadata(path).map_err(|cause| io_error(code, operation, cause))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return integrity(format!("{operation}: path is not a literal regular file"));
    }
    Ok(DescriptorIdentity::from_metadata(&metadata))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunDomain {
    kind: ExternalRecordKind,
    key_width: ExternalKeyWidth,
    k: u8,
    node_minimizer_length: u8,
    support_unit: WideRunSupportUnit,
    virtual_partition: u32,
    generation: u32,
    run_ordinal: u64,
    source_root: [u8; 32],
    scientific_config_root: [u8; 32],
    parent_set_root: [u8; 32],
    record_width: u16,
    max_block_payload: u32,
    /// Operational manifest field.  VTEBLK01 stores the selected partition
    /// (or `u32::MAX` for a global run), so the partition count is supplied by
    /// the descriptor-bound catalog and is not serialized in the header.
    owner_partition_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileHeader {
    domain: RunDomain,
    record_count: u64,
    block_count: u64,
    payload_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunMeta {
    id: ExternalRunId,
    header: FileHeader,
    byte_len: u64,
    content_root: [u8; 32],
    descriptor: DescriptorIdentity,
}

impl RunMeta {
    const fn summary(self) -> ExternalRunSummary {
        ExternalRunSummary {
            id: self.id,
            record_count: self.header.record_count,
            payload_bytes: self.header.payload_bytes,
            byte_len: self.byte_len,
            content_root: self.content_root,
            parent_set_root: self.header.domain.parent_set_root,
        }
    }
}

trait RecordCodec: Copy + Eq {
    const KIND: ExternalRecordKind;

    fn record_width(domain: RunDomain) -> Result<usize>;
    fn validate(self, domain: RunDomain) -> Result<()>;
    fn encode(self, domain: RunDomain, destination: &mut [u8]) -> Result<()>;
    fn decode(domain: RunDomain, source: &[u8]) -> Result<Self>;
    fn compare(left: &Self, right: &Self) -> Ordering;
}

fn support_unit_tag(unit: WideRunSupportUnit) -> u8 {
    match unit {
        WideRunSupportUnit::SuppliedFragmentInstance => 0,
        WideRunSupportUnit::AcceptedWindowOccurrence => 1,
    }
}

fn support_unit_from_tag(tag: u8) -> Result<WideRunSupportUnit> {
    match tag {
        0 => Ok(WideRunSupportUnit::SuppliedFragmentInstance),
        1 => Ok(WideRunSupportUnit::AcceptedWindowOccurrence),
        _ => integrity(format!("unknown external cDBG support-unit tag {tag}")),
    }
}

fn write_packed(
    destination: &mut [u8],
    offset: &mut usize,
    value: PackedKmer,
    length: u8,
    width: ExternalKeyWidth,
) -> Result<()> {
    validate_code(value, length)?;
    let bytes = value.to_be_bytes();
    let count = width.bytes();
    let start = 32_usize
        .checked_sub(count)
        .ok_or_else(|| overflow("packed-key width underflow"))?;
    let end = checked_usize_add(*offset, count, "packed-key destination offset overflow")?;
    if end > destination.len() {
        return integrity("external cDBG record encoder exceeded destination width");
    }
    destination[*offset..end].copy_from_slice(&bytes[start..]);
    *offset = end;
    Ok(())
}

fn read_packed(
    source: &[u8],
    offset: &mut usize,
    length: u8,
    width: ExternalKeyWidth,
) -> Result<PackedKmer> {
    let count = width.bytes();
    let end = checked_usize_add(*offset, count, "packed-key source offset overflow")?;
    if end > source.len() {
        return integrity("external cDBG record decoder exceeded source width");
    }
    let mut full = [0_u8; 32];
    full[32 - count..].copy_from_slice(&source[*offset..end]);
    *offset = end;
    let value = PackedKmer::from_be_bytes(full);
    validate_code(value, length).map_err(|error| {
        integrity_error(format!(
            "external cDBG packed field is invalid for length {length}: {error}"
        ))
    })?;
    Ok(value)
}

fn record_width_for(kind: ExternalRecordKind, key_width: ExternalKeyWidth, m: u8) -> Result<u16> {
    let key = key_width.bytes();
    let width = match kind {
        ExternalRecordKind::Edge => checked_usize_add(key, 8, "EDGE width overflow")?,
        ExternalRecordKind::Incidence => checked_usize_add(
            key.checked_mul(2)
                .ok_or_else(|| overflow("INCIDENCE key width overflow"))?,
            2,
            "INCIDENCE width overflow",
        )?,
        ExternalRecordKind::NodeState => {
            let minimizer = ExternalKeyWidth::for_length(m)?.bytes();
            checked_usize_add(
                checked_usize_add(key, minimizer, "NODE_STATE key width overflow")?,
                8,
                "NODE_STATE width overflow",
            )?
        }
        ExternalRecordKind::NodeAudit => checked_usize_add(key, 4, "NODE_AUDIT width overflow")?,
    };
    u16::try_from(width).map_err(|_| overflow("external cDBG record width does not fit u16"))
}

impl RecordCodec for ExternalEdgeRecord {
    const KIND: ExternalRecordKind = ExternalRecordKind::Edge;

    fn record_width(domain: RunDomain) -> Result<usize> {
        Ok(usize::from(record_width_for(
            Self::KIND,
            domain.key_width,
            domain.node_minimizer_length,
        )?))
    }

    fn validate(self, domain: RunDomain) -> Result<()> {
        validate_code(self.key, domain.k)?;
        if canonical_code(self.key, domain.k)? != self.key {
            return integrity("EDGE key is not canonical");
        }
        if self.support == 0 {
            return integrity("EDGE support must be nonzero");
        }
        Ok(())
    }

    fn encode(self, domain: RunDomain, destination: &mut [u8]) -> Result<()> {
        self.validate(domain)?;
        let mut offset = 0;
        write_packed(
            destination,
            &mut offset,
            self.key,
            domain.k,
            domain.key_width,
        )?;
        let end = checked_usize_add(offset, 8, "EDGE support offset overflow")?;
        destination
            .get_mut(offset..end)
            .ok_or_else(|| integrity_error("EDGE encoder exceeded record width"))?
            .copy_from_slice(&self.support.to_le_bytes());
        if end != destination.len() {
            return integrity("EDGE encoder did not fill its record");
        }
        Ok(())
    }

    fn decode(domain: RunDomain, source: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let key = read_packed(source, &mut offset, domain.k, domain.key_width)?;
        let end = checked_usize_add(offset, 8, "EDGE support offset overflow")?;
        let support = u64::from_le_bytes(
            source
                .get(offset..end)
                .ok_or_else(|| integrity_error("EDGE decoder exceeded record width"))?
                .try_into()
                .map_err(|_| integrity_error("EDGE support has the wrong width"))?,
        );
        if end != source.len() {
            return integrity("EDGE decoder did not consume its record");
        }
        let record = Self { key, support };
        record
            .validate(domain)
            .map_err(|error| integrity_error(format!("invalid decoded EDGE record: {error}")))?;
        Ok(record)
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        left.key.cmp(&right.key)
    }
}

impl RecordCodec for ExternalIncidenceRecord {
    const KIND: ExternalRecordKind = ExternalRecordKind::Incidence;

    fn record_width(domain: RunDomain) -> Result<usize> {
        Ok(usize::from(record_width_for(
            Self::KIND,
            domain.key_width,
            domain.node_minimizer_length,
        )?))
    }

    fn validate(self, domain: RunDomain) -> Result<()> {
        let node_length = domain
            .k
            .checked_sub(1)
            .ok_or_else(|| overflow("INCIDENCE node length underflow"))?;
        validate_code(self.node, node_length)?;
        validate_code(self.handle, domain.k)?;
        let expected_node = match self.direction {
            IncidenceDirection::Incoming => suffix_code(self.handle, domain.k)?,
            IncidenceDirection::Outgoing => prefix_code(self.handle, domain.k)?,
        };
        if expected_node != self.node {
            return integrity("INCIDENCE node is not the declared handle endpoint");
        }
        let fixed = reverse_complement_code(self.handle, domain.k)? == self.handle;
        if fixed != self.fixed_edge {
            return integrity("INCIDENCE fixed-edge flag disagrees with its exact handle");
        }
        Ok(())
    }

    fn encode(self, domain: RunDomain, destination: &mut [u8]) -> Result<()> {
        self.validate(domain)?;
        let mut offset = 0;
        write_packed(
            destination,
            &mut offset,
            self.node,
            domain.k - 1,
            domain.key_width,
        )?;
        write_packed(
            destination,
            &mut offset,
            self.handle,
            domain.k,
            domain.key_width,
        )?;
        let direction = destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("INCIDENCE direction exceeded record width"))?;
        *direction = self.direction as u8;
        offset = checked_usize_add(offset, 1, "INCIDENCE direction offset overflow")?;
        let fixed = destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("INCIDENCE fixed flag exceeded record width"))?;
        *fixed = u8::from(self.fixed_edge);
        offset = checked_usize_add(offset, 1, "INCIDENCE fixed offset overflow")?;
        if offset != destination.len() {
            return integrity("INCIDENCE encoder did not fill its record");
        }
        Ok(())
    }

    fn decode(domain: RunDomain, source: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let node = read_packed(source, &mut offset, domain.k - 1, domain.key_width)?;
        let handle = read_packed(source, &mut offset, domain.k, domain.key_width)?;
        let direction = IncidenceDirection::from_tag(
            *source
                .get(offset)
                .ok_or_else(|| integrity_error("INCIDENCE direction is missing"))?,
        )?;
        offset = checked_usize_add(offset, 1, "INCIDENCE direction offset overflow")?;
        let fixed_edge = match source
            .get(offset)
            .copied()
            .ok_or_else(|| integrity_error("INCIDENCE fixed flag is missing"))?
        {
            0 => false,
            1 => true,
            value => return integrity(format!("invalid INCIDENCE fixed-edge flag {value}")),
        };
        offset = checked_usize_add(offset, 1, "INCIDENCE fixed offset overflow")?;
        if offset != source.len() {
            return integrity("INCIDENCE decoder did not consume its record");
        }
        let record = Self {
            node,
            handle,
            direction,
            fixed_edge,
        };
        record.validate(domain).map_err(|error| {
            integrity_error(format!("invalid decoded INCIDENCE record: {error}"))
        })?;
        Ok(record)
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        (left.node, left.direction, left.handle).cmp(&(right.node, right.direction, right.handle))
    }
}

impl RecordCodec for ExternalNodeStateRecord {
    const KIND: ExternalRecordKind = ExternalRecordKind::NodeState;

    fn record_width(domain: RunDomain) -> Result<usize> {
        Ok(usize::from(record_width_for(
            Self::KIND,
            domain.key_width,
            domain.node_minimizer_length,
        )?))
    }

    fn validate(self, domain: RunDomain) -> Result<()> {
        let node_length = domain
            .k
            .checked_sub(1)
            .ok_or_else(|| overflow("NODE_STATE node length underflow"))?;
        validate_code(self.node, node_length)?;
        validate_code(self.owner_minimizer, domain.node_minimizer_length)?;
        if self.incoming_mask & !0x0f != 0 || self.outgoing_mask & !0x0f != 0 {
            return integrity("NODE_STATE has a degree bit outside the four DNA bases");
        }
        if self.incoming_mask == 0 && self.outgoing_mask == 0 {
            return integrity("NODE_STATE cannot have zero total degree");
        }
        let owner = node_owner(
            self.node,
            node_length,
            domain.node_minimizer_length,
            domain.owner_partition_count,
        )?;
        if owner.0 != self.owner_minimizer || owner.1 != self.owner_partition {
            return integrity("NODE_STATE owner fields disagree with its exact literal node");
        }
        let reverse_fixed = reverse_complement_code(self.node, node_length)? == self.node;
        if reverse_fixed != self.reverse_fixed {
            return integrity("NODE_STATE reverse-fixed flag disagrees with its exact node");
        }
        let degree_boundary =
            self.incoming_mask.count_ones() != 1 || self.outgoing_mask.count_ones() != 1;
        if self.hard_boundary != (degree_boundary || self.reverse_fixed || self.incident_fixed_edge)
        {
            return integrity("NODE_STATE hard-boundary flag is internally inconsistent");
        }
        Ok(())
    }

    fn encode(self, domain: RunDomain, destination: &mut [u8]) -> Result<()> {
        self.validate(domain)?;
        let mut offset = 0;
        write_packed(
            destination,
            &mut offset,
            self.node,
            domain.k - 1,
            domain.key_width,
        )?;
        let minimizer_width = ExternalKeyWidth::for_length(domain.node_minimizer_length)?;
        write_packed(
            destination,
            &mut offset,
            self.owner_minimizer,
            domain.node_minimizer_length,
            minimizer_width,
        )?;
        let partition_end = checked_usize_add(offset, 4, "NODE_STATE owner offset overflow")?;
        destination
            .get_mut(offset..partition_end)
            .ok_or_else(|| integrity_error("NODE_STATE owner exceeded record width"))?
            .copy_from_slice(&self.owner_partition.to_le_bytes());
        offset = partition_end;
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_STATE incoming mask is missing"))? =
            self.incoming_mask;
        offset = checked_usize_add(offset, 1, "NODE_STATE incoming offset overflow")?;
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_STATE outgoing mask is missing"))? =
            self.outgoing_mask;
        offset = checked_usize_add(offset, 1, "NODE_STATE outgoing offset overflow")?;
        let degree_boundary =
            self.incoming_mask.count_ones() != 1 || self.outgoing_mask.count_ones() != 1;
        let flags = u8::from(self.reverse_fixed)
            | (u8::from(self.incident_fixed_edge) << 1)
            | (u8::from(self.hard_boundary) << 2)
            | (u8::from(degree_boundary) << 3);
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_STATE flags are missing"))? = flags;
        offset = checked_usize_add(offset, 1, "NODE_STATE flags offset overflow")?;
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_STATE reserved byte is missing"))? = 0;
        offset = checked_usize_add(offset, 1, "NODE_STATE reserved offset overflow")?;
        if offset != destination.len() {
            return integrity("NODE_STATE encoder did not fill its record");
        }
        Ok(())
    }

    fn decode(domain: RunDomain, source: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let node = read_packed(source, &mut offset, domain.k - 1, domain.key_width)?;
        let minimizer_width = ExternalKeyWidth::for_length(domain.node_minimizer_length)?;
        let owner_minimizer = read_packed(
            source,
            &mut offset,
            domain.node_minimizer_length,
            minimizer_width,
        )?;
        let partition_end = checked_usize_add(offset, 4, "NODE_STATE owner offset overflow")?;
        let owner_partition = u32::from_le_bytes(
            source
                .get(offset..partition_end)
                .ok_or_else(|| integrity_error("NODE_STATE owner is missing"))?
                .try_into()
                .map_err(|_| integrity_error("NODE_STATE owner has the wrong width"))?,
        );
        offset = partition_end;
        let incoming_mask = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_STATE incoming mask is missing"))?;
        offset = checked_usize_add(offset, 1, "NODE_STATE incoming offset overflow")?;
        let outgoing_mask = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_STATE outgoing mask is missing"))?;
        offset = checked_usize_add(offset, 1, "NODE_STATE outgoing offset overflow")?;
        let flags = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_STATE flags are missing"))?;
        offset = checked_usize_add(offset, 1, "NODE_STATE flags offset overflow")?;
        if flags & !0x0f != 0 {
            return integrity("NODE_STATE has an unknown flag bit");
        }
        let reserved = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_STATE reserved byte is missing"))?;
        if reserved != 0 {
            return integrity("NODE_STATE reserved byte is nonzero");
        }
        offset = checked_usize_add(offset, 1, "NODE_STATE reserved offset overflow")?;
        if offset != source.len() {
            return integrity("NODE_STATE decoder did not consume its record");
        }
        let degree_boundary = incoming_mask.count_ones() != 1 || outgoing_mask.count_ones() != 1;
        if ((flags >> 3) & 1 != 0) != degree_boundary {
            return integrity("NODE_STATE encoded degree-boundary bit is inconsistent");
        }
        let record = Self {
            node,
            owner_minimizer,
            owner_partition,
            incoming_mask,
            outgoing_mask,
            reverse_fixed: flags & 1 != 0,
            incident_fixed_edge: flags & 2 != 0,
            hard_boundary: flags & 4 != 0,
        };
        record.validate(domain).map_err(|error| {
            integrity_error(format!("invalid decoded NODE_STATE record: {error}"))
        })?;
        Ok(record)
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        (left.owner_partition, left.node).cmp(&(right.owner_partition, right.node))
    }
}

impl RecordCodec for ExternalNodeAuditRecord {
    const KIND: ExternalRecordKind = ExternalRecordKind::NodeAudit;

    fn record_width(domain: RunDomain) -> Result<usize> {
        Ok(usize::from(record_width_for(
            Self::KIND,
            domain.key_width,
            domain.node_minimizer_length,
        )?))
    }

    fn validate(self, domain: RunDomain) -> Result<()> {
        let node_length = domain
            .k
            .checked_sub(1)
            .ok_or_else(|| overflow("NODE_AUDIT node length underflow"))?;
        validate_code(self.node, node_length)?;
        if self.incoming_mask & !0x0f != 0 || self.outgoing_mask & !0x0f != 0 {
            return integrity("NODE_AUDIT has a degree bit outside the four DNA bases");
        }
        if self.incoming_mask == 0 && self.outgoing_mask == 0 {
            return integrity("NODE_AUDIT cannot have zero total degree");
        }
        let reverse_fixed = reverse_complement_code(self.node, node_length)? == self.node;
        if reverse_fixed != self.reverse_fixed {
            return integrity("NODE_AUDIT reverse-fixed flag disagrees with its exact node");
        }
        let degree_boundary =
            self.incoming_mask.count_ones() != 1 || self.outgoing_mask.count_ones() != 1;
        if self.hard_boundary != (degree_boundary || self.reverse_fixed || self.incident_fixed_edge)
        {
            return integrity("NODE_AUDIT hard-boundary flag is internally inconsistent");
        }
        Ok(())
    }

    fn encode(self, domain: RunDomain, destination: &mut [u8]) -> Result<()> {
        self.validate(domain)?;
        let mut offset = 0;
        write_packed(
            destination,
            &mut offset,
            self.node,
            domain.k - 1,
            domain.key_width,
        )?;
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT incoming mask is missing"))? =
            self.incoming_mask;
        offset = checked_usize_add(offset, 1, "NODE_AUDIT incoming offset overflow")?;
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT outgoing mask is missing"))? =
            self.outgoing_mask;
        offset = checked_usize_add(offset, 1, "NODE_AUDIT outgoing offset overflow")?;
        let degree_boundary =
            self.incoming_mask.count_ones() != 1 || self.outgoing_mask.count_ones() != 1;
        let flags = u8::from(self.reverse_fixed)
            | (u8::from(self.incident_fixed_edge) << 1)
            | (u8::from(self.hard_boundary) << 2)
            | (u8::from(degree_boundary) << 3);
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT flags are missing"))? = flags;
        offset = checked_usize_add(offset, 1, "NODE_AUDIT flags offset overflow")?;
        *destination
            .get_mut(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT reserved byte is missing"))? = 0;
        offset = checked_usize_add(offset, 1, "NODE_AUDIT reserved offset overflow")?;
        if offset != destination.len() {
            return integrity("NODE_AUDIT encoder did not fill its record");
        }
        Ok(())
    }

    fn decode(domain: RunDomain, source: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let node = read_packed(source, &mut offset, domain.k - 1, domain.key_width)?;
        let incoming_mask = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT incoming mask is missing"))?;
        offset = checked_usize_add(offset, 1, "NODE_AUDIT incoming offset overflow")?;
        let outgoing_mask = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT outgoing mask is missing"))?;
        offset = checked_usize_add(offset, 1, "NODE_AUDIT outgoing offset overflow")?;
        let flags = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT flags are missing"))?;
        offset = checked_usize_add(offset, 1, "NODE_AUDIT flags offset overflow")?;
        if flags & !0x0f != 0 {
            return integrity("NODE_AUDIT has an unknown flag bit");
        }
        let reserved = *source
            .get(offset)
            .ok_or_else(|| integrity_error("NODE_AUDIT reserved byte is missing"))?;
        if reserved != 0 {
            return integrity("NODE_AUDIT reserved byte is nonzero");
        }
        offset = checked_usize_add(offset, 1, "NODE_AUDIT reserved offset overflow")?;
        if offset != source.len() {
            return integrity("NODE_AUDIT decoder did not consume its record");
        }
        let degree_boundary = incoming_mask.count_ones() != 1 || outgoing_mask.count_ones() != 1;
        if ((flags >> 3) & 1 != 0) != degree_boundary {
            return integrity("NODE_AUDIT encoded degree-boundary bit is inconsistent");
        }
        let record = Self {
            node,
            incoming_mask,
            outgoing_mask,
            reverse_fixed: flags & 1 != 0,
            incident_fixed_edge: flags & 2 != 0,
            hard_boundary: flags & 4 != 0,
        };
        record.validate(domain).map_err(|error| {
            integrity_error(format!("invalid decoded NODE_AUDIT record: {error}"))
        })?;
        Ok(record)
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        left.node.cmp(&right.node)
    }
}

fn encode_header(header: FileHeader) -> Result<[u8; FILE_HEADER_BYTES]> {
    validate_domain(header.domain)?;
    let mut encoded = [0_u8; FILE_HEADER_BYTES];
    encoded[0..8].copy_from_slice(FILE_MAGIC);
    encoded[8..10].copy_from_slice(&SCHEMA.to_le_bytes());
    encoded[10..12].copy_from_slice(&(header.domain.kind as u16).to_le_bytes());
    encoded[12] = header.domain.key_width as u8;
    encoded[13] = header.domain.k;
    encoded[14] = header.domain.node_minimizer_length;
    encoded[15] = support_unit_tag(header.domain.support_unit);
    encoded[16..20].copy_from_slice(&header.domain.virtual_partition.to_le_bytes());
    encoded[20..24].copy_from_slice(&header.domain.generation.to_le_bytes());
    encoded[24..32].copy_from_slice(&header.domain.run_ordinal.to_le_bytes());
    encoded[32..64].copy_from_slice(&header.domain.source_root);
    encoded[64..96].copy_from_slice(&header.domain.scientific_config_root);
    encoded[96..128].copy_from_slice(&header.domain.parent_set_root);
    encoded[128..130].copy_from_slice(&header.domain.record_width.to_le_bytes());
    encoded[130..134].copy_from_slice(&header.domain.max_block_payload.to_le_bytes());
    encoded[134..142].copy_from_slice(&header.record_count.to_le_bytes());
    encoded[142..150].copy_from_slice(&header.block_count.to_le_bytes());
    encoded[150..158].copy_from_slice(&header.payload_bytes.to_le_bytes());
    debug_assert_eq!(FILE_HEADER_BYTES - 158, RESERVED_HEADER_BYTES);
    Ok(encoded)
}

fn decode_header(
    encoded: &[u8; FILE_HEADER_BYTES],
    owner_partition_count: u32,
) -> Result<FileHeader> {
    if owner_partition_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG reader owner-partition count must be nonzero",
        ));
    }
    if &encoded[0..8] != FILE_MAGIC {
        return integrity("external cDBG file magic is invalid");
    }
    if read_u16(&encoded[8..10])? != SCHEMA {
        return integrity("external cDBG file schema is unsupported");
    }
    if encoded[158..].iter().any(|&byte| byte != 0) {
        return integrity("external cDBG file header reserved bytes are nonzero");
    }
    let kind = ExternalRecordKind::from_tag(read_u16(&encoded[10..12])?)?;
    let key_width = ExternalKeyWidth::from_tag(encoded[12])?;
    let k = encoded[13];
    let node_minimizer_length = encoded[14];
    let support_unit = support_unit_from_tag(encoded[15])?;
    let virtual_partition = read_u32(&encoded[16..20])?;
    let generation = read_u32(&encoded[20..24])?;
    let run_ordinal = read_u64(&encoded[24..32])?;
    let mut source_root = [0_u8; 32];
    source_root.copy_from_slice(&encoded[32..64]);
    let mut scientific_config_root = [0_u8; 32];
    scientific_config_root.copy_from_slice(&encoded[64..96]);
    let mut parent_set_root = [0_u8; 32];
    parent_set_root.copy_from_slice(&encoded[96..128]);
    let domain = RunDomain {
        kind,
        key_width,
        k,
        node_minimizer_length,
        support_unit,
        virtual_partition,
        generation,
        run_ordinal,
        source_root,
        scientific_config_root,
        parent_set_root,
        record_width: read_u16(&encoded[128..130])?,
        max_block_payload: read_u32(&encoded[130..134])?,
        owner_partition_count,
    };
    validate_domain(domain)
        .map_err(|error| integrity_error(format!("invalid external cDBG file domain: {error}")))?;
    let header = FileHeader {
        domain,
        record_count: read_u64(&encoded[134..142])?,
        block_count: read_u64(&encoded[142..150])?,
        payload_bytes: read_u64(&encoded[150..158])?,
    };
    validate_header_totals(header)
        .map_err(|error| integrity_error(format!("invalid external cDBG file totals: {error}")))?;
    Ok(header)
}

fn validate_domain(domain: RunDomain) -> Result<()> {
    validate_k(domain.k)?;
    if domain.node_minimizer_length == 0 || domain.node_minimizer_length >= domain.k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "external cDBG node minimizer must be in 1..k; received {} for k={}",
                domain.node_minimizer_length, domain.k
            ),
        ));
    }
    if domain.key_width != ExternalKeyWidth::for_length(domain.k)? {
        return integrity("external cDBG key-width tag disagrees with k");
    }
    if domain.owner_partition_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG owner partition count must be nonzero",
        ));
    }
    if domain.virtual_partition != GLOBAL_PARTITION
        && domain.virtual_partition >= domain.owner_partition_count
    {
        return integrity("external cDBG run partition is outside the operational partition plan");
    }
    let expected = record_width_for(domain.kind, domain.key_width, domain.node_minimizer_length)?;
    if domain.record_width != expected {
        return integrity("external cDBG record width disagrees with its typed schema");
    }
    if domain.max_block_payload < u32::from(domain.record_width) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG block payload cannot hold one record",
        ));
    }
    Ok(())
}

fn validate_header_totals(header: FileHeader) -> Result<()> {
    let expected_payload = header
        .record_count
        .checked_mul(u64::from(header.domain.record_width))
        .ok_or_else(|| overflow("external cDBG header payload total overflow"))?;
    if expected_payload != header.payload_bytes {
        return integrity("external cDBG header record and payload totals disagree");
    }
    if header.record_count == 0 {
        if header.block_count != 0 || header.payload_bytes != 0 {
            return integrity("empty external cDBG stream has nonzero totals");
        }
    } else if header.block_count == 0 {
        return integrity("nonempty external cDBG stream has no blocks");
    }
    Ok(())
}

fn encode_block_header(
    ordinal: u64,
    record_count: u32,
    payload_bytes: u32,
    previous_digest: [u8; 32],
) -> [u8; BLOCK_HEADER_BYTES] {
    let mut encoded = [0_u8; BLOCK_HEADER_BYTES];
    encoded[0..8].copy_from_slice(BLOCK_MAGIC);
    encoded[8..16].copy_from_slice(&ordinal.to_le_bytes());
    encoded[16..20].copy_from_slice(&record_count.to_le_bytes());
    encoded[20..24].copy_from_slice(&payload_bytes.to_le_bytes());
    encoded[24..56].copy_from_slice(&previous_digest);
    debug_assert_eq!(BLOCK_HEADER_BYTES - 56, RESERVED_BLOCK_BYTES);
    encoded
}

fn block_digest(domain: RunDomain, block_header: &[u8; 64], payload: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(BLOCK_ROOT_DOMAIN);
    digest.update((domain.kind as u16).to_le_bytes());
    digest.update([
        domain.key_width as u8,
        domain.k,
        domain.node_minimizer_length,
    ]);
    digest.update([support_unit_tag(domain.support_unit)]);
    digest.update(domain.virtual_partition.to_le_bytes());
    digest.update(domain.generation.to_le_bytes());
    digest.update(domain.run_ordinal.to_le_bytes());
    digest.update(domain.source_root);
    digest.update(domain.scientific_config_root);
    digest.update(domain.parent_set_root);
    digest.update(domain.record_width.to_le_bytes());
    digest.update(domain.max_block_payload.to_le_bytes());
    digest.update(block_header);
    digest.update(payload);
    digest.finalize().into()
}

fn file_content_root(
    header: FileHeader,
    encoded_header: &[u8; FILE_HEADER_BYTES],
    final_block_digest: [u8; 32],
    file_length: u64,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(FILE_ROOT_DOMAIN);
    digest.update(encoded_header);
    digest.update(TRAILER_MAGIC);
    digest.update(header.record_count.to_le_bytes());
    digest.update(header.block_count.to_le_bytes());
    digest.update(header.payload_bytes.to_le_bytes());
    digest.update(final_block_digest);
    digest.update(file_length.to_le_bytes());
    digest.finalize().into()
}

struct BlockWriter<R: RecordCodec> {
    file: Option<File>,
    domain: RunDomain,
    payload: Vec<u8>,
    records_in_block: u32,
    record_count: u64,
    block_count: u64,
    payload_bytes: u64,
    previous_block_digest: [u8; 32],
    previous_record: Option<R>,
}

#[derive(Debug, Clone, Copy)]
struct ProvisionalFileSeal {
    header: FileHeader,
    final_block_digest: [u8; 32],
    final_byte_len: u64,
    provisional_byte_len: u64,
    descriptor: DescriptorIdentity,
}

impl<R: RecordCodec> BlockWriter<R> {
    fn create(path: &Path, domain: RunDomain) -> Result<Self> {
        validate_domain(domain)?;
        if domain.kind != R::KIND {
            return integrity("typed external cDBG writer kind mismatch");
        }
        let expected_width = R::record_width(domain)?;
        if expected_width != usize::from(domain.record_width) {
            return integrity("typed external cDBG writer record-width mismatch");
        }
        let block_capacity = as_usize(
            u64::from(domain.max_block_payload),
            "external cDBG block capacity",
        )?;
        let mut payload = Vec::new();
        payload.try_reserve_exact(block_capacity).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve external cDBG writer block: {cause}"),
            )
        })?;
        enforce_capacity(
            payload.capacity(),
            block_capacity,
            "external cDBG writer block",
        )?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "create external cDBG run",
                    cause,
                )
            })?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "set private external cDBG run permissions",
                    cause,
                )
            })?;
        file.write_all(&[0_u8; FILE_HEADER_BYTES])
            .map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "reserve external cDBG file header",
                    cause,
                )
            })?;
        Ok(Self {
            file: Some(file),
            domain,
            payload,
            records_in_block: 0,
            record_count: 0,
            block_count: 0,
            payload_bytes: 0,
            previous_block_digest: [0_u8; 32],
            previous_record: None,
        })
    }

    fn push(&mut self, record: R) -> Result<()> {
        record.validate(self.domain)?;
        if self
            .previous_record
            .is_some_and(|previous| R::compare(&previous, &record) != Ordering::Less)
        {
            return integrity("external cDBG run records are not strictly ordered");
        }
        let width = usize::from(self.domain.record_width);
        let next_len =
            checked_usize_add(self.payload.len(), width, "block payload offset overflow")?;
        if next_len
            > as_usize(
                u64::from(self.domain.max_block_payload),
                "external cDBG block cap",
            )?
        {
            self.flush_block()?;
        }
        let start = self.payload.len();
        let end = checked_usize_add(start, width, "record payload offset overflow")?;
        self.payload.resize(end, 0);
        record.encode(self.domain, &mut self.payload[start..end])?;
        self.records_in_block = self
            .records_in_block
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG records-per-block overflow"))?;
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG record total overflow"))?;
        self.previous_record = Some(record);
        Ok(())
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.records_in_block == 0 {
            if !self.payload.is_empty() {
                return integrity("empty external cDBG block retained payload bytes");
            }
            return Ok(());
        }
        let payload_bytes = u32::try_from(self.payload.len())
            .map_err(|_| overflow("external cDBG block payload does not fit u32"))?;
        let expected = self
            .records_in_block
            .checked_mul(u32::from(self.domain.record_width))
            .ok_or_else(|| overflow("external cDBG block record-byte total overflow"))?;
        if expected != payload_bytes || payload_bytes > self.domain.max_block_payload {
            return integrity("external cDBG writer block totals are inconsistent");
        }
        let header = encode_block_header(
            self.block_count,
            self.records_in_block,
            payload_bytes,
            self.previous_block_digest,
        );
        let digest = block_digest(self.domain, &header, &self.payload);
        let file = self.file.as_mut().ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "external cDBG writer was already finished",
            )
        })?;
        file.write_all(&header).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "write external cDBG block header",
                cause,
            )
        })?;
        file.write_all(&self.payload).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "write external cDBG block payload",
                cause,
            )
        })?;
        file.write_all(&digest).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "write external cDBG block digest",
                cause,
            )
        })?;
        self.payload_bytes = self
            .payload_bytes
            .checked_add(u64::from(payload_bytes))
            .ok_or_else(|| overflow("external cDBG payload total overflow"))?;
        self.block_count = self
            .block_count
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG block total overflow"))?;
        self.previous_block_digest = digest;
        self.records_in_block = 0;
        self.payload.clear();
        Ok(())
    }

    /// Close a derived run with an invalid all-zero file header and no
    /// trailer. Such bytes cannot be opened as VTEBLK01 and therefore cannot
    /// be mistaken for a registered artifact before every parent reader has
    /// authenticated its trailer and exact EOF.
    fn finish_provisional(mut self) -> Result<ProvisionalFileSeal> {
        self.flush_block()?;
        let header = FileHeader {
            domain: self.domain,
            record_count: self.record_count,
            block_count: self.block_count,
            payload_bytes: self.payload_bytes,
        };
        validate_header_totals(header)?;
        let file_length = projected_file_bytes(
            header.record_count,
            header.domain.record_width,
            header.domain.max_block_payload,
        )?;
        let provisional_byte_len = file_length
            .checked_sub(TRAILER_BYTES as u64)
            .ok_or_else(|| overflow("external cDBG provisional file length underflow"))?;
        let mut file = self.file.take().ok_or_else(|| {
            VeritasmError::new(
                ErrorCode::InternalInvariant,
                "external cDBG writer lost its file",
            )
        })?;
        file.flush().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "flush provisional external cDBG run",
                cause,
            )
        })?;
        file.sync_all().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "sync provisional external cDBG run",
                cause,
            )
        })?;
        let metadata = file.metadata().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat provisional external cDBG run",
                cause,
            )
        })?;
        if metadata.len() != provisional_byte_len {
            return integrity(format!(
                "provisional external cDBG run length {} differs from projected {provisional_byte_len}",
                metadata.len()
            ));
        }
        let descriptor = DescriptorIdentity::from_metadata(&metadata);
        drop(file);
        Ok(ProvisionalFileSeal {
            header,
            final_block_digest: self.previous_block_digest,
            final_byte_len: file_length,
            provisional_byte_len,
            descriptor,
        })
    }
}

fn seal_provisional_file(path: &Path, seal: ProvisionalFileSeal) -> Result<[u8; 32]> {
    let mut file = open_existing_regular_nofollow(
        path,
        true,
        ErrorCode::ResourceTemporaryBytes,
        "open provisional external cDBG run for sealing",
    )?;
    let identity = DescriptorIdentity::from_metadata(&file.metadata().map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat provisional external cDBG descriptor",
            cause,
        )
    })?);
    let path_identity = regular_path_identity(
        path,
        ErrorCode::ResourceTemporaryBytes,
        "stat provisional external cDBG path",
    )?;
    if identity != seal.descriptor || path_identity != seal.descriptor {
        return integrity("external cDBG provisional descriptor identity changed before sealing");
    }
    if identity.byte_len != seal.provisional_byte_len {
        return integrity("external cDBG provisional descriptor length changed before sealing");
    }
    let mut placeholder = [0_u8; FILE_HEADER_BYTES];
    file.read_exact(&mut placeholder).map_err(|cause| {
        io_error(
            ErrorCode::IntegrityCountRun,
            "read provisional external cDBG placeholder header",
            cause,
        )
    })?;
    if placeholder.iter().any(|&byte| byte != 0) {
        return integrity("external cDBG provisional header became publishable before sealing");
    }
    file.seek(SeekFrom::Start(0)).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "seek provisional external cDBG header",
            cause,
        )
    })?;
    let encoded_header = encode_header(seal.header)?;
    let content_root = file_content_root(
        seal.header,
        &encoded_header,
        seal.final_block_digest,
        seal.final_byte_len,
    );
    file.write_all(&encoded_header).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "write sealed external cDBG file header",
            cause,
        )
    })?;
    let mut trailer = [0_u8; TRAILER_BYTES];
    trailer[0..8].copy_from_slice(TRAILER_MAGIC);
    trailer[8..16].copy_from_slice(&seal.header.record_count.to_le_bytes());
    trailer[16..24].copy_from_slice(&seal.header.block_count.to_le_bytes());
    trailer[24..32].copy_from_slice(&seal.header.payload_bytes.to_le_bytes());
    trailer[32..64].copy_from_slice(&seal.final_block_digest);
    trailer[64..96].copy_from_slice(&content_root);
    trailer[96..104].copy_from_slice(&seal.final_byte_len.to_le_bytes());
    file.seek(SeekFrom::End(0)).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "seek provisional external cDBG trailer",
            cause,
        )
    })?;
    file.write_all(&trailer).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "write sealed external cDBG file trailer",
            cause,
        )
    })?;
    file.flush().map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "flush sealed external cDBG run",
            cause,
        )
    })?;
    file.sync_all().map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "sync sealed external cDBG run",
            cause,
        )
    })?;
    let actual_length = file
        .metadata()
        .map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat sealed external cDBG run",
                cause,
            )
        })?
        .len();
    if actual_length != seal.final_byte_len {
        return integrity(format!(
            "sealed external cDBG run length {actual_length} differs from projected {}",
            seal.final_byte_len
        ));
    }
    Ok(content_root)
}

struct BlockReader<R: RecordCodec> {
    file: File,
    identity: DescriptorIdentity,
    header: FileHeader,
    expected: Option<RunMeta>,
    payload: Vec<u8>,
    cursor: usize,
    blocks_read: u64,
    records_read: u64,
    payload_read: u64,
    previous_block_digest: [u8; 32],
    previous_record: Option<R>,
}

impl<R: RecordCodec> BlockReader<R> {
    fn open_registered(
        path: &Path,
        expected: RunMeta,
        admitted_block_bytes: usize,
        owner_partition_count: u32,
    ) -> Result<Self> {
        Self::open(
            path,
            Some(expected),
            admitted_block_bytes,
            owner_partition_count,
        )
    }

    fn open_unregistered(
        path: &Path,
        admitted_block_bytes: usize,
        owner_partition_count: u32,
    ) -> Result<Self> {
        Self::open(path, None, admitted_block_bytes, owner_partition_count)
    }

    fn open(
        path: &Path,
        expected: Option<RunMeta>,
        admitted_block_bytes: usize,
        owner_partition_count: u32,
    ) -> Result<Self> {
        let mut file = open_existing_regular_nofollow(
            path,
            false,
            ErrorCode::IntegrityCountRun,
            "open external cDBG run",
        )?;
        let descriptor_metadata = file.metadata().map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "stat external cDBG run descriptor",
                cause,
            )
        })?;
        let identity = DescriptorIdentity::from_metadata(&descriptor_metadata);
        let path_identity = regular_path_identity(
            path,
            ErrorCode::IntegrityCountRun,
            "stat external cDBG run path",
        )?;
        if identity != path_identity {
            return integrity("external cDBG run path and opened descriptor identities differ");
        }
        if let Some(meta) = expected {
            if identity != meta.descriptor || identity.byte_len != meta.byte_len {
                return integrity("external cDBG registered descriptor identity changed");
            }
        }
        let mut encoded_header = [0_u8; FILE_HEADER_BYTES];
        file.read_exact(&mut encoded_header).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read external cDBG file header",
                cause,
            )
        })?;
        let header = decode_header(&encoded_header, owner_partition_count)?;
        if header.domain.kind != R::KIND {
            return integrity("external cDBG reader record-kind mismatch");
        }
        if R::record_width(header.domain).map_err(|error| {
            integrity_error(format!("invalid external cDBG typed width: {error}"))
        })? != usize::from(header.domain.record_width)
        {
            return integrity("external cDBG reader record-width mismatch");
        }
        if let Some(meta) = expected {
            if header != meta.header || ExternalRunId::from_domain(header.domain) != meta.id {
                return integrity("external cDBG registered header changed");
            }
        }
        let projected = projected_file_bytes(
            header.record_count,
            header.domain.record_width,
            header.domain.max_block_payload,
        )
        .map_err(|error| {
            integrity_error(format!("invalid external cDBG projected length: {error}"))
        })?;
        if projected != identity.byte_len {
            return integrity(format!(
                "external cDBG descriptor length {} differs from authenticated projection {projected}",
                identity.byte_len
            ));
        }
        let block_capacity = as_usize(
            u64::from(header.domain.max_block_payload),
            "external cDBG reader block capacity",
        )?;
        if block_capacity > admitted_block_bytes {
            return Err(VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!(
                    "external cDBG run requests {block_capacity} block bytes, exceeding admitted {admitted_block_bytes}"
                ),
            ));
        }
        let mut payload = Vec::new();
        payload.try_reserve_exact(block_capacity).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve external cDBG reader block: {cause}"),
            )
        })?;
        enforce_capacity(
            payload.capacity(),
            block_capacity,
            "external cDBG reader block",
        )?;
        Ok(Self {
            file,
            identity,
            header,
            expected,
            payload,
            cursor: 0,
            blocks_read: 0,
            records_read: 0,
            payload_read: 0,
            previous_block_digest: [0_u8; 32],
            previous_record: None,
        })
    }

    fn next_record(&mut self) -> Result<Option<R>> {
        if self.cursor == self.payload.len() {
            if self.blocks_read == self.header.block_count {
                return Ok(None);
            }
            self.load_block()?;
        }
        let width = usize::from(self.header.domain.record_width);
        let end = checked_usize_add(self.cursor, width, "reader record offset overflow")?;
        let record = R::decode(
            self.header.domain,
            self.payload
                .get(self.cursor..end)
                .ok_or_else(|| integrity_error("external cDBG record crosses block boundary"))?,
        )?;
        if self
            .previous_record
            .is_some_and(|previous| R::compare(&previous, &record) != Ordering::Less)
        {
            return integrity("external cDBG run record order is not strictly increasing");
        }
        self.previous_record = Some(record);
        self.cursor = end;
        self.records_read = self
            .records_read
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG reader record total overflow"))?;
        Ok(Some(record))
    }

    fn load_block(&mut self) -> Result<()> {
        let mut encoded = [0_u8; BLOCK_HEADER_BYTES];
        self.file.read_exact(&mut encoded).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read external cDBG block header",
                cause,
            )
        })?;
        if &encoded[0..8] != BLOCK_MAGIC {
            return integrity("external cDBG block magic is invalid");
        }
        if encoded[56..].iter().any(|&byte| byte != 0) {
            return integrity("external cDBG block reserved bytes are nonzero");
        }
        let ordinal = read_u64(&encoded[8..16])?;
        if ordinal != self.blocks_read {
            return integrity("external cDBG block ordinal is not contiguous");
        }
        let record_count = read_u32(&encoded[16..20])?;
        let payload_bytes = read_u32(&encoded[20..24])?;
        if record_count == 0 {
            return integrity("external cDBG file contains an empty block");
        }
        let expected_payload = record_count
            .checked_mul(u32::from(self.header.domain.record_width))
            .ok_or_else(|| integrity_error("external cDBG block payload check overflow"))?;
        if payload_bytes != expected_payload || payload_bytes > self.header.domain.max_block_payload
        {
            return integrity("external cDBG block payload totals are inconsistent");
        }
        if encoded[24..56] != self.previous_block_digest {
            return integrity("external cDBG previous-block digest link is invalid");
        }
        let payload_len = as_usize(u64::from(payload_bytes), "external cDBG block payload")?;
        self.payload.resize(payload_len, 0);
        self.file.read_exact(&mut self.payload).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read external cDBG block payload",
                cause,
            )
        })?;
        let mut stored_digest = [0_u8; BLOCK_DIGEST_BYTES];
        self.file.read_exact(&mut stored_digest).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read external cDBG block digest",
                cause,
            )
        })?;
        let computed = block_digest(self.header.domain, &encoded, &self.payload);
        if stored_digest != computed {
            return integrity("external cDBG block digest mismatch");
        }
        if stored_digest == [0_u8; 32] {
            return integrity("nonempty external cDBG block used the empty-stream digest sentinel");
        }
        self.previous_block_digest = stored_digest;
        self.cursor = 0;
        self.blocks_read = self
            .blocks_read
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG reader block total overflow"))?;
        self.payload_read = self
            .payload_read
            .checked_add(u64::from(payload_bytes))
            .ok_or_else(|| overflow("external cDBG reader payload total overflow"))?;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        while self.next_record()?.is_some() {}
        if self.records_read != self.header.record_count
            || self.blocks_read != self.header.block_count
            || self.payload_read != self.header.payload_bytes
        {
            return integrity("external cDBG observed totals disagree with its header");
        }
        if self.cursor != self.payload.len() {
            return integrity("external cDBG reader stopped within a block");
        }
        let mut trailer = [0_u8; TRAILER_BYTES];
        self.file.read_exact(&mut trailer).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "read external cDBG trailer",
                cause,
            )
        })?;
        if &trailer[0..8] != TRAILER_MAGIC {
            return integrity("external cDBG trailer magic is invalid");
        }
        if read_u64(&trailer[8..16])? != self.header.record_count
            || read_u64(&trailer[16..24])? != self.header.block_count
            || read_u64(&trailer[24..32])? != self.header.payload_bytes
        {
            return integrity("external cDBG trailer totals disagree with its header");
        }
        let mut final_digest = [0_u8; 32];
        final_digest.copy_from_slice(&trailer[32..64]);
        if final_digest != self.previous_block_digest {
            return integrity("external cDBG trailer final-block digest is invalid");
        }
        if self.header.block_count == 0 {
            if final_digest != [0_u8; 32] {
                return integrity("empty external cDBG stream has a nonzero final-block digest");
            }
        } else if final_digest == [0_u8; 32] {
            return integrity("nonempty external cDBG stream has the zero final-block sentinel");
        }
        let mut stored_root = [0_u8; 32];
        stored_root.copy_from_slice(&trailer[64..96]);
        let stored_length = read_u64(&trailer[96..104])?;
        if stored_length != self.identity.byte_len {
            return integrity("external cDBG trailer length disagrees with its descriptor");
        }
        let encoded_header = encode_header(self.header)?;
        let computed_root =
            file_content_root(self.header, &encoded_header, final_digest, stored_length);
        if stored_root != computed_root {
            return integrity("external cDBG file content root mismatch");
        }
        if let Some(expected) = self.expected {
            if stored_root != expected.content_root || stored_length != expected.byte_len {
                return integrity("external cDBG registered trailer changed");
            }
        }
        let mut eof = [0_u8; 1];
        if self.file.read(&mut eof).map_err(|cause| {
            io_error(
                ErrorCode::IntegrityCountRun,
                "check external cDBG exact EOF",
                cause,
            )
        })? != 0
        {
            return integrity("external cDBG run has trailing bytes");
        }
        let final_identity =
            DescriptorIdentity::from_metadata(&self.file.metadata().map_err(|cause| {
                io_error(
                    ErrorCode::IntegrityCountRun,
                    "restat external cDBG run descriptor",
                    cause,
                )
            })?);
        if final_identity != self.identity {
            return integrity("external cDBG descriptor identity changed during verification");
        }
        Ok(())
    }
}

impl ExternalRunId {
    const fn from_domain(domain: RunDomain) -> Self {
        Self {
            kind: domain.kind,
            generation: domain.generation,
            ordinal: domain.run_ordinal,
        }
    }
}

fn projected_file_bytes(record_count: u64, record_width: u16, block_cap: u32) -> Result<u64> {
    if record_width == 0 || block_cap < u32::from(record_width) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG block cap cannot hold one typed record",
        ));
    }
    let records_per_block = u64::from(block_cap / u32::from(record_width));
    if records_per_block == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG block has zero record capacity",
        ));
    }
    let block_count = if record_count == 0 {
        0
    } else {
        record_count
            .checked_add(records_per_block - 1)
            .ok_or_else(|| overflow("external cDBG block-count rounding overflow"))?
            / records_per_block
    };
    let payload = record_count
        .checked_mul(u64::from(record_width))
        .ok_or_else(|| overflow("external cDBG projected payload overflow"))?;
    let framed_blocks = block_count
        .checked_mul((BLOCK_HEADER_BYTES + BLOCK_DIGEST_BYTES) as u64)
        .ok_or_else(|| overflow("external cDBG projected block framing overflow"))?;
    (FILE_HEADER_BYTES as u64)
        .checked_add(payload)
        .and_then(|value| value.checked_add(framed_blocks))
        .and_then(|value| value.checked_add(TRAILER_BYTES as u64))
        .ok_or_else(|| overflow("external cDBG projected file length overflow"))
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
            .ok_or_else(|| overflow("external cDBG temporary-byte total overflow"))?;
        if next > self.limit {
            return Err(VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!(
                    "external cDBG temporary files would use {next} bytes, exceeding limit {}",
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
            .ok_or_else(|| overflow("external cDBG temporary-byte accounting underflow"))?;
        Ok(())
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
                    "create unique external cDBG run directory",
                    cause,
                )
            })?;
        // Disarm tempfile's recursive pathname cleanup immediately. From this
        // point onward cleanup removes only a still-owned empty directory or
        // separately validated, bounded VTE files.
        let path = directory.keep();
        let initial_metadata = fs::symlink_metadata(&path).map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat unique external cDBG run directory",
                cause,
            )
        })?;
        if !initial_metadata.file_type().is_dir() || initial_metadata.file_type().is_symlink() {
            return integrity("new external cDBG run path is not a literal directory");
        }
        let initial_identity = DirectoryIdentity::from_metadata(&initial_metadata);
        let initialized = (|| {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "set private external cDBG run-directory permissions",
                    cause,
                )
            })?;
            let path_metadata = fs::symlink_metadata(&path).map_err(|cause| {
                io_error(
                    ErrorCode::ResourceTemporaryBytes,
                    "restat private external cDBG run directory",
                    cause,
                )
            })?;
            let identity = validate_directory_metadata(
                &path_metadata,
                "validate new external cDBG private run directory",
            )?;
            if !same_directory_object(identity, initial_identity) {
                return integrity("external cDBG run directory changed during initialization");
            }
            let anchor = open_directory_nofollow(&path)?;
            let anchor_identity = validate_directory_metadata(
                &anchor.metadata().map_err(|cause| {
                    io_error(
                        ErrorCode::ResourceTemporaryBytes,
                        "stat external cDBG run-directory descriptor",
                        cause,
                    )
                })?,
                "validate external cDBG run-directory descriptor",
            )?;
            if anchor_identity != identity {
                return integrity("external cDBG run directory changed while it was opened");
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
                            "external cDBG run-directory initialization failed ({primary}); identity-safe cleanup also failed: {cleanup}"
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

    fn transfer(mut self) -> (PathBuf, DirectoryIdentity) {
        self.armed = false;
        (std::mem::take(&mut self.path), self.identity)
    }

    fn cleanup_after_error<T>(mut self, primary: VeritasmError) -> Result<T> {
        match remove_owned_run_directory(&self.path, self.identity, self.max_entries) {
            Ok(()) => {
                self.armed = false;
                Err(primary)
            }
            Err(cleanup) => {
                self.armed = false;
                Err(VeritasmError::new(
                    ErrorCode::ResourceTemporaryBytes,
                    format!(
                        "external cDBG build failed ({primary}); private-directory cleanup also failed for {}: {cleanup}",
                        self.path.display()
                    ),
                ))
            }
        }
    }
}

impl Drop for RunDirectoryGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = remove_owned_run_directory(&self.path, self.identity, self.max_entries);
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
            "stat provisional external cDBG run directory",
            cause,
        )
    })?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || !same_directory_object(DirectoryIdentity::from_metadata(&metadata), expected)
    {
        return integrity("provisional external cDBG run-directory identity changed");
    }
    fs::remove_dir(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "remove provisional empty external cDBG run directory",
            cause,
        )
    })
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
            format!("cannot open private external cDBG run directory: {cause}"),
        )
    })?;
    Ok(File::from(descriptor))
}

fn verify_owned_run_directory(path: &Path, expected: DirectoryIdentity) -> Result<()> {
    let path_metadata = fs::symlink_metadata(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat owned external cDBG run directory",
            cause,
        )
    })?;
    let path_identity =
        validate_directory_metadata(&path_metadata, "validate owned external cDBG run directory")?;
    if path_identity != expected {
        return integrity("external cDBG owned run-directory identity changed before cleanup");
    }
    let anchor = open_directory_nofollow(path)?;
    let descriptor_identity = validate_directory_metadata(
        &anchor.metadata().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat owned external cDBG run-directory descriptor",
                cause,
            )
        })?,
        "validate owned external cDBG run-directory descriptor",
    )?;
    if descriptor_identity != expected {
        return integrity("external cDBG owned run directory changed while it was opened");
    }
    Ok(())
}

fn is_external_cdbg_run_filename(bytes: &[u8]) -> bool {
    bytes.len() == RUN_FILENAME_BYTES
        && bytes.first() == Some(&b'k')
        && bytes[1..4].iter().all(u8::is_ascii_digit)
        && &bytes[4..6] == b"-g"
        && bytes[6..16].iter().all(u8::is_ascii_digit)
        && &bytes[16..18] == b"-r"
        && bytes[18..38].iter().all(u8::is_ascii_digit)
        && &bytes[38..] == b".vte"
}

fn open_cleanup_file_nofollow(directory: &File, name: &std::ffi::CStr) -> Result<File> {
    use rustix::fs::{openat, Mode, OFlags};

    let descriptor = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot open external cDBG cleanup entry: {cause}"),
        )
    })?;
    Ok(File::from(descriptor))
}

fn stat_cleanup_entry(directory: &File, name: &std::ffi::CStr) -> Result<CleanupFileIdentity> {
    use rustix::fs::{statat, AtFlags};

    let stat = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot stat anchored external cDBG cleanup entry: {cause}"),
        )
    })?;
    CleanupFileIdentity::from_stat(&stat)
}

fn validate_cleanup_entry(
    directory: &File,
    name: &std::ffi::CStr,
    owner_uid: u32,
) -> Result<CleanupFileIdentity> {
    use rustix::fs::FileType;

    if !is_external_cdbg_run_filename(name.to_bytes()) {
        return integrity("private external cDBG run directory contains an unknown entry name");
    }
    let path_identity = stat_cleanup_entry(directory, name)?;
    // `rustix::fs::RawMode` follows the target ABI (`u32` on Linux and
    // `u16` on Darwin), while `std::os::unix::fs::MetadataExt::mode` is
    // exposed as `u32` on both.  Refuse a value that the target mode type
    // cannot represent instead of relying on a truncating cast.
    #[cfg(target_vendor = "apple")]
    let raw_mode = u16::try_from(path_identity.mode).map_err(|_| {
        integrity_error("external cDBG cleanup mode is not representable on this target")
    })?;
    #[cfg(not(target_vendor = "apple"))]
    let raw_mode = path_identity.mode;
    if FileType::from_raw_mode(raw_mode) != FileType::RegularFile
        || path_identity.owner_uid != owner_uid
        || path_identity.mode & 0o777 != 0o600
    {
        return integrity(
            "private external cDBG run directory contains a non-private or non-regular entry",
        );
    }
    let file = open_cleanup_file_nofollow(directory, name)?;
    let descriptor_identity =
        CleanupFileIdentity::from_metadata(&file.metadata().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat external cDBG cleanup descriptor",
                cause,
            )
        })?);
    if descriptor_identity != path_identity {
        return integrity("external cDBG cleanup entry changed while it was opened");
    }
    Ok(path_identity)
}

fn scan_cleanup_entries(
    directory: &File,
    owner_uid: u32,
    max_entries: u64,
    remove: bool,
) -> Result<()> {
    use rustix::fs::{unlinkat, AtFlags, Dir};

    let mut directory_entries = Dir::read_from(directory).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot open anchored external cDBG directory stream: {cause}"),
        )
    })?;
    let mut entries = 0_u64;
    while let Some(entry) = directory_entries.read() {
        let entry = entry.map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceTemporaryBytes,
                format!("cannot read anchored external cDBG directory entry: {cause}"),
            )
        })?;
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        entries = entries
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG cleanup entry count overflow"))?;
        if entries > max_entries {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRunCount,
                format!(
                    "external cDBG cleanup entry count exceeds admitted run limit {max_entries}"
                ),
            ));
        }
        let identity = validate_cleanup_entry(directory, name, owner_uid)?;
        if remove {
            // Recheck the name relative to the already authenticated
            // directory descriptor immediately before anchored unlink.
            if stat_cleanup_entry(directory, name)? != identity {
                return integrity("external cDBG cleanup entry changed before anchored unlink");
            }
            unlinkat(directory, name, AtFlags::empty()).map_err(|cause| {
                VeritasmError::new(
                    ErrorCode::ResourceTemporaryBytes,
                    format!("cannot unlink validated external cDBG cleanup entry: {cause}"),
                )
            })?;
        }
    }
    Ok(())
}

fn remove_owned_run_directory(
    path: &Path,
    expected: DirectoryIdentity,
    max_entries: u64,
) -> Result<()> {
    use rustix::fs::{openat, statat, unlinkat, AtFlags, Mode, OFlags};

    verify_owned_run_directory(path, expected)?;
    let parent = path
        .parent()
        .ok_or_else(|| integrity_error("external cDBG run directory has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| integrity_error("external cDBG run directory has no final name"))?;
    let parent_anchor = open_directory_nofollow(parent)?;
    let descriptor = openat(
        &parent_anchor,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot open anchored external cDBG run directory: {cause}"),
        )
    })?;
    let directory_anchor = File::from(descriptor);
    let descriptor_identity = validate_directory_metadata(
        &directory_anchor.metadata().map_err(|cause| {
            io_error(
                ErrorCode::ResourceTemporaryBytes,
                "stat anchored external cDBG run-directory descriptor",
                cause,
            )
        })?,
        "validate anchored external cDBG run-directory descriptor",
    )?;
    if descriptor_identity != expected {
        return integrity("external cDBG run directory changed before anchored cleanup");
    }
    // Validate every bounded entry before deleting any. The second pass
    // revalidates each name immediately before unlinking it, always relative
    // to the same authenticated directory descriptor.
    scan_cleanup_entries(&directory_anchor, expected.owner_uid, max_entries, false)?;
    scan_cleanup_entries(&directory_anchor, expected.owner_uid, max_entries, true)?;
    let final_stat = statat(&parent_anchor, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot restat anchored external cDBG run directory: {cause}"),
        )
    })?;
    if DirectoryIdentity::from_stat(&final_stat)? != expected {
        return integrity("external cDBG run-directory name changed before anchored removal");
    }
    unlinkat(&parent_anchor, name, AtFlags::REMOVEDIR).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("cannot remove anchored external cDBG run directory: {cause}"),
        )
    })
}

fn remove_registered_run_file(path: &Path, expected: DescriptorIdentity) -> Result<()> {
    let path_identity = regular_path_identity(
        path,
        ErrorCode::ResourceTemporaryBytes,
        "stat authenticated external cDBG predecessor path",
    )?;
    let file = open_existing_regular_nofollow(
        path,
        false,
        ErrorCode::ResourceTemporaryBytes,
        "open authenticated external cDBG predecessor for cleanup",
    )?;
    let descriptor = DescriptorIdentity::from_metadata(&file.metadata().map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat authenticated external cDBG predecessor descriptor",
            cause,
        )
    })?);
    if descriptor != expected || path_identity != expected {
        return integrity("external cDBG predecessor identity changed before cleanup");
    }
    fs::remove_file(path).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "reclaim identity-verified external cDBG predecessor",
            cause,
        )
    })
}

struct BuildState<'a> {
    options: &'a ExternalCdbgOptions,
    run_dir: &'a Path,
    retained_source_root: [u8; 32],
    next_run_ordinal: u64,
    run_files_created: u64,
    reclaimed_files: u64,
    open_files_high_water: u16,
    temp: TempLedger,
    fault: FaultInjector,
    ancestry: Vec<ExternalAncestryLink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    Disabled,
    AfterVerifiedRun,
    BeforePredecessorCleanup,
    CorruptParentAfterProvisional,
}

#[derive(Debug, Clone, Copy)]
struct FaultInjector {
    point: FaultPoint,
    successful_hits_before_failure: u64,
}

impl FaultInjector {
    const fn disabled() -> Self {
        Self {
            point: FaultPoint::Disabled,
            successful_hits_before_failure: 0,
        }
    }

    fn triggers(&mut self, point: FaultPoint) -> bool {
        if self.point != point {
            return false;
        }
        if self.successful_hits_before_failure > 0 {
            self.successful_hits_before_failure -= 1;
            return false;
        }
        self.point = FaultPoint::Disabled;
        true
    }

    fn hit(&mut self, point: FaultPoint) -> Result<()> {
        if !self.triggers(point) {
            return Ok(());
        }
        Err(VeritasmError::new(
            ErrorCode::ResourceTemporaryBytes,
            format!("injected external cDBG fault at {point:?}"),
        ))
    }
}

fn corrupt_parent_after_provisional_if_injected(
    state: &mut BuildState<'_>,
    parent: RunMeta,
) -> Result<()> {
    if !state
        .fault
        .triggers(FaultPoint::CorruptParentAfterProvisional)
    {
        return Ok(());
    }
    let path = run_path(state.run_dir, parent.id)?;
    let mut file = open_existing_regular_nofollow(
        &path,
        true,
        ErrorCode::ResourceTemporaryBytes,
        "open injected external cDBG parent-corruption target",
    )?;
    let offset = parent
        .byte_len
        .checked_sub(1)
        .ok_or_else(|| overflow("external cDBG injected corruption offset underflow"))?;
    file.seek(SeekFrom::Start(offset)).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "seek injected external cDBG parent-corruption target",
            cause,
        )
    })?;
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "read injected external cDBG parent-corruption byte",
            cause,
        )
    })?;
    byte[0] ^= 0x80;
    file.seek(SeekFrom::Start(offset)).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "reseek injected external cDBG parent-corruption target",
            cause,
        )
    })?;
    file.write_all(&byte).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "write injected external cDBG parent-corruption byte",
            cause,
        )
    })?;
    file.sync_all().map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "sync injected external cDBG parent corruption",
            cause,
        )
    })
}

struct PendingRun<R: RecordCodec> {
    id: ExternalRunId,
    projected_bytes: u64,
    expected_records: u64,
    writer: BlockWriter<R>,
}

#[derive(Debug, Clone, Copy)]
struct ProvisionalRun<R: RecordCodec> {
    id: ExternalRunId,
    projected_bytes: u64,
    expected_records: u64,
    seal: ProvisionalFileSeal,
    codec: PhantomData<R>,
}

impl BuildState<'_> {
    fn begin_run<R: RecordCodec>(
        &mut self,
        generation: u32,
        expected_records: u64,
        parents: &[RunMeta],
    ) -> Result<PendingRun<R>> {
        if self.run_files_created >= self.options.limits.max_run_files {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRunCount,
                format!(
                    "external cDBG would create more than {} run files",
                    self.options.limits.max_run_files
                ),
            ));
        }
        let id = ExternalRunId {
            kind: R::KIND,
            generation,
            ordinal: self.next_run_ordinal,
        };
        self.next_run_ordinal = self
            .next_run_ordinal
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG run ordinal overflow"))?;
        let key_width = ExternalKeyWidth::for_length(self.options.k)?;
        let domain = RunDomain {
            kind: R::KIND,
            key_width,
            k: self.options.k,
            node_minimizer_length: self.options.node_minimizer_length,
            support_unit: self.options.support_unit,
            virtual_partition: GLOBAL_PARTITION,
            generation,
            run_ordinal: id.ordinal,
            source_root: self.retained_source_root,
            scientific_config_root: self.options.scientific_config_root,
            parent_set_root: parent_set_root(parents)?,
            record_width: record_width_for(R::KIND, key_width, self.options.node_minimizer_length)?,
            max_block_payload: self.options.limits.block_payload_bytes,
            owner_partition_count: self.options.virtual_partition_count,
        };
        let projected_bytes = projected_file_bytes(
            expected_records,
            domain.record_width,
            domain.max_block_payload,
        )?;
        self.temp.reserve(projected_bytes)?;
        let path = run_path(self.run_dir, id)?;
        let writer = BlockWriter::create(&path, domain)?;
        self.run_files_created = self
            .run_files_created
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG created-run total overflow"))?;
        self.open_files_high_water = self.open_files_high_water.max(1);
        Ok(PendingRun {
            id,
            projected_bytes,
            expected_records,
            writer,
        })
    }

    fn finish_run<R: RecordCodec>(
        &mut self,
        pending: PendingRun<R>,
        parents: &[RunMeta],
    ) -> Result<RunMeta> {
        let provisional = self.finish_pending_provisional(pending)?;
        self.seal_registered_run(provisional, parents)
    }

    fn finish_pending_provisional<R: RecordCodec>(
        &mut self,
        pending: PendingRun<R>,
    ) -> Result<ProvisionalRun<R>> {
        let PendingRun {
            id,
            projected_bytes,
            expected_records,
            writer,
        } = pending;
        let seal = writer.finish_provisional()?;
        if seal.header.record_count != expected_records
            || seal.final_byte_len != projected_bytes
            || ExternalRunId::from_domain(seal.header.domain) != id
        {
            return integrity("provisional external cDBG run disagrees with its construction plan");
        }
        Ok(ProvisionalRun {
            id,
            projected_bytes,
            expected_records,
            seal,
            codec: PhantomData,
        })
    }

    /// Promote a pathless provisional run only after its caller has
    /// authenticated every parent reader through trailer and exact EOF.
    fn seal_registered_run<R: RecordCodec>(
        &mut self,
        provisional: ProvisionalRun<R>,
        parents: &[RunMeta],
    ) -> Result<RunMeta> {
        let ProvisionalRun {
            id,
            projected_bytes,
            expected_records,
            seal,
            codec: _,
        } = provisional;
        if seal.header.record_count != expected_records
            || seal.final_byte_len != projected_bytes
            || ExternalRunId::from_domain(seal.header.domain) != id
            || parent_set_root(parents)? != seal.header.domain.parent_set_root
        {
            return integrity("external cDBG provisional registration plan changed");
        }
        let path = run_path(self.run_dir, id)?;
        let content_root = seal_provisional_file(&path, seal)?;
        let reader = BlockReader::<R>::open_unregistered(
            &path,
            as_usize(
                u64::from(self.options.limits.block_payload_bytes),
                "external cDBG verification block cap",
            )?,
            self.options.virtual_partition_count,
        )?;
        let descriptor = reader.identity;
        reader.finish()?;
        let meta = RunMeta {
            id,
            header: seal.header,
            byte_len: seal.final_byte_len,
            content_root,
            descriptor,
        };
        let registered = BlockReader::<R>::open_registered(
            &path,
            meta,
            as_usize(
                u64::from(self.options.limits.block_payload_bytes),
                "external cDBG registered verification block cap",
            )?,
            self.options.virtual_partition_count,
        )?;
        registered.finish()?;
        self.fault.hit(FaultPoint::AfterVerifiedRun)?;
        self.record_ancestry(meta, parents)?;
        Ok(meta)
    }

    fn seal_slice<R: RecordCodec>(
        &mut self,
        generation: u32,
        records: &[R],
        parents: &[RunMeta],
    ) -> Result<RunMeta> {
        let count = u64::try_from(records.len())
            .map_err(|_| overflow("external cDBG slice record count does not fit u64"))?;
        let mut pending = self.begin_run::<R>(generation, count, parents)?;
        for &record in records {
            pending.writer.push(record)?;
        }
        self.finish_run(pending, parents)
    }

    fn delete_run(&mut self, run: RunMeta) -> Result<()> {
        self.fault.hit(FaultPoint::BeforePredecessorCleanup)?;
        remove_registered_run_file(&run_path(self.run_dir, run.id)?, run.descriptor)?;
        self.temp.release(run.byte_len)?;
        self.reclaimed_files = self
            .reclaimed_files
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG reclaimed-file total overflow"))?;
        Ok(())
    }

    fn record_ancestry(&mut self, child: RunMeta, parents: &[RunMeta]) -> Result<()> {
        if parent_set_root(parents)? != child.header.domain.parent_set_root {
            return integrity("external cDBG registered parent list disagrees with its root");
        }
        let maximum_links = self
            .options
            .limits
            .max_run_files
            .checked_mul(ANCESTRY_LINK_MULTIPLIER)
            .ok_or_else(|| overflow("external cDBG ancestry-link limit overflow"))?;
        for &parent in parents {
            let next_len = self
                .ancestry
                .len()
                .checked_add(1)
                .ok_or_else(|| overflow("external cDBG ancestry ledger length overflow"))?;
            if u64::try_from(next_len)
                .map_err(|_| overflow("external cDBG ancestry length does not fit u64"))?
                > maximum_links
            {
                return Err(VeritasmError::new(
                    ErrorCode::ResourceRunCount,
                    "external cDBG ancestry ledger exceeded its admitted fixed-row bound",
                ));
            }
            if self.ancestry.len() == self.ancestry.capacity() {
                self.ancestry.try_reserve_exact(1).map_err(|cause| {
                    VeritasmError::new(
                        ErrorCode::ResourceMemory,
                        format!("cannot grow external cDBG ancestry ledger: {cause}"),
                    )
                })?;
                if u64::try_from(self.ancestry.capacity())
                    .map_err(|_| overflow("external cDBG ancestry capacity does not fit u64"))?
                    > maximum_links
                {
                    return Err(VeritasmError::new(
                        ErrorCode::ResourceMemory,
                        "allocator exceeded the admitted external cDBG ancestry capacity",
                    ));
                }
            }
            self.ancestry.push(ExternalAncestryLink {
                child_id: child.id,
                child_content_root: child.content_root,
                child_parent_set_root: child.header.domain.parent_set_root,
                parent_id: parent.id,
                parent_virtual_partition: parent.header.domain.virtual_partition,
                parent_content_root: parent.content_root,
            });
        }
        Ok(())
    }
}

fn parent_set_root(parents: &[RunMeta]) -> Result<[u8; 32]> {
    if parents.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return integrity("external cDBG parent set is not strictly ordered by numeric run ID");
    }
    let mut digest = Sha256::new();
    digest.update(PARENT_ROOT_DOMAIN);
    digest.update(
        u64::try_from(parents.len())
            .map_err(|_| overflow("external cDBG parent count does not fit u64"))?
            .to_le_bytes(),
    );
    for parent in parents {
        digest.update((parent.id.kind as u16).to_le_bytes());
        digest.update(parent.header.domain.virtual_partition.to_le_bytes());
        digest.update(parent.id.generation.to_le_bytes());
        digest.update(parent.id.ordinal.to_le_bytes());
        digest.update(parent.content_root);
    }
    Ok(digest.finalize().into())
}

fn ancestry_root(links: &[ExternalAncestryLink]) -> Result<[u8; 32]> {
    if links.windows(2).any(|pair| {
        pair[0].child_id > pair[1].child_id
            || (pair[0].child_id == pair[1].child_id && pair[0].parent_id >= pair[1].parent_id)
    }) {
        return integrity("external cDBG ancestry rows are not in canonical numeric order");
    }
    let mut digest = Sha256::new();
    digest.update(ANCESTRY_ROOT_DOMAIN);
    digest.update(
        u64::try_from(links.len())
            .map_err(|_| overflow("external cDBG ancestry row count does not fit u64"))?
            .to_le_bytes(),
    );
    for link in links {
        digest.update((link.child_id.kind as u16).to_le_bytes());
        digest.update(link.child_id.generation.to_le_bytes());
        digest.update(link.child_id.ordinal.to_le_bytes());
        digest.update(link.child_content_root);
        digest.update(link.child_parent_set_root);
        digest.update((link.parent_id.kind as u16).to_le_bytes());
        digest.update(link.parent_virtual_partition.to_le_bytes());
        digest.update(link.parent_id.generation.to_le_bytes());
        digest.update(link.parent_id.ordinal.to_le_bytes());
        digest.update(link.parent_content_root);
    }
    Ok(digest.finalize().into())
}

fn run_path(run_dir: &Path, id: ExternalRunId) -> Result<PathBuf> {
    let mut name = String::new();
    name.try_reserve_exact(72).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve external cDBG numeric run filename: {cause}"),
        )
    })?;
    if name.capacity() > 72 {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            "allocator exceeded the admitted external cDBG run-filename capacity",
        ));
    }
    write!(
        &mut name,
        "k{:03}-g{:010}-r{:020}.vte",
        id.kind as u16, id.generation, id.ordinal
    )
    .map_err(|_| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "cannot format external cDBG numeric run filename",
        )
    })?;
    let requested = run_dir
        .as_os_str()
        .len()
        .checked_add(name.len() + 1)
        .ok_or_else(|| overflow("external cDBG run path length overflow"))?;
    let mut path = PathBuf::new();
    path.try_reserve_exact(requested).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve external cDBG run path: {cause}"),
        )
    })?;
    if path.capacity() > requested {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            "allocator exceeded the admitted external cDBG run-path capacity",
        ));
    }
    path.push(run_dir);
    path.push(name);
    Ok(path)
}

fn node_owner(
    node: PackedKmer,
    node_length: u8,
    minimizer_length: u8,
    partition_count: u32,
) -> Result<(PackedKmer, u32)> {
    if minimizer_length == 0 || minimizer_length > node_length {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            "external cDBG node minimizer is outside the literal node",
        ));
    }
    validate_code(node, node_length)?;
    let canonical = canonical_code(node, node_length)?;
    let mut rolling = PackedKmer::ZERO;
    let mut best = None;
    for position in 0..node_length {
        rolling = shift_left_pair(rolling, packed_pair_at(canonical, node_length, position)?);
        if position + 1 < minimizer_length {
            continue;
        }
        rolling = mask_to_length(rolling, minimizer_length);
        let candidate = canonical_code(rolling, minimizer_length)?;
        best = Some(best.map_or(candidate, |current: PackedKmer| current.min(candidate)));
    }
    let minimizer = best.ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::InternalInvariant,
            "external cDBG minimizer scan produced no candidate",
        )
    })?;
    let partition = route_minimizer(minimizer, minimizer_length, partition_count)?;
    Ok((minimizer, partition))
}

fn packed_pair_at(code: PackedKmer, length: u8, position: u8) -> Result<u8> {
    validate_code(code, length)?;
    if position >= length {
        return integrity("external cDBG packed-base position is outside its string");
    }
    let offset = u16::from(length - position - 1) * 2;
    let word_from_low = usize::from(offset / 64);
    let shift = u32::from(offset % 64);
    Ok(((code.words()[3 - word_from_low] >> shift) & 0b11) as u8)
}

fn shift_left_pair(code: PackedKmer, pair: u8) -> PackedKmer {
    let words = code.words();
    PackedKmer::from_words([
        (words[0] << 2) | (words[1] >> 62),
        (words[1] << 2) | (words[2] >> 62),
        (words[2] << 2) | (words[3] >> 62),
        (words[3] << 2) | u64::from(pair),
    ])
}

fn mask_to_length(code: PackedKmer, length: u8) -> PackedKmer {
    let active_bits = u16::from(length) * 2;
    let full_words = usize::from(active_bits / 64);
    let partial_bits = u32::from(active_bits % 64);
    let mut mask = [0_u64; 4];
    for index in 0..full_words {
        mask[3 - index] = u64::MAX;
    }
    if partial_bits != 0 {
        mask[3 - full_words] = (1_u64 << partial_bits) - 1;
    }
    let words = code.words();
    PackedKmer::from_words([
        words[0] & mask[0],
        words[1] & mask[1],
        words[2] & mask[2],
        words[3] & mask[3],
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeapItem<R: RecordCodec> {
    record: R,
    reader_index: usize,
}

impl<R: RecordCodec> Ord for HeapItem<R> {
    fn cmp(&self, other: &Self) -> Ordering {
        R::compare(&other.record, &self.record)
            .then_with(|| other.reader_index.cmp(&self.reader_index))
    }
}

impl<R: RecordCodec> PartialOrd for HeapItem<R> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn push_catalog(catalog: &mut Vec<RunMeta>, run: RunMeta, limit: u64) -> Result<()> {
    let next_len = catalog
        .len()
        .checked_add(1)
        .ok_or_else(|| overflow("external cDBG run catalog length overflow"))?;
    if u64::try_from(next_len)
        .map_err(|_| overflow("external cDBG run catalog length does not fit u64"))?
        > limit
    {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRunCount,
            format!("external cDBG run catalog would exceed {limit} entries"),
        ));
    }
    if catalog.len() == catalog.capacity() {
        catalog.try_reserve_exact(1).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot grow external cDBG run catalog: {cause}"),
            )
        })?;
        if u64::try_from(catalog.capacity())
            .map_err(|_| overflow("external cDBG run catalog capacity does not fit u64"))?
            > limit
        {
            return Err(VeritasmError::new(
                ErrorCode::ResourceMemory,
                "allocator exceeded the admitted external cDBG run-catalog capacity",
            ));
        }
    }
    catalog.push(run);
    Ok(())
}

fn push_provisional_catalog<R: RecordCodec>(
    catalog: &mut Vec<ProvisionalRun<R>>,
    run: ProvisionalRun<R>,
    limit: u64,
) -> Result<()> {
    let next_len = catalog
        .len()
        .checked_add(1)
        .ok_or_else(|| overflow("external cDBG provisional catalog length overflow"))?;
    if u64::try_from(next_len)
        .map_err(|_| overflow("external cDBG provisional catalog length does not fit u64"))?
        > limit
    {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRunCount,
            format!("external cDBG provisional catalog would exceed {limit} entries"),
        ));
    }
    if catalog.len() == catalog.capacity() {
        catalog.try_reserve_exact(1).map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot grow external cDBG provisional catalog: {cause}"),
            )
        })?;
        if u64::try_from(catalog.capacity())
            .map_err(|_| overflow("external cDBG provisional capacity does not fit u64"))?
            > limit
        {
            return Err(VeritasmError::new(
                ErrorCode::ResourceMemory,
                "allocator exceeded the admitted external cDBG provisional-catalog capacity",
            ));
        }
    }
    catalog.push(run);
    Ok(())
}

fn provision_sorted_buffer<R: RecordCodec>(
    state: &mut BuildState<'_>,
    buffer: &mut Vec<R>,
    generation: u32,
    parents: &[RunMeta],
    catalog: &mut Vec<ProvisionalRun<R>>,
) -> Result<()> {
    buffer.sort_unstable_by(R::compare);
    let count = u64::try_from(buffer.len())
        .map_err(|_| overflow("external cDBG provisional slice count does not fit u64"))?;
    let mut pending = state.begin_run::<R>(generation, count, parents)?;
    for &record in buffer.iter() {
        pending.writer.push(record)?;
    }
    let run = state.finish_pending_provisional(pending)?;
    push_provisional_catalog(catalog, run, state.options.limits.max_run_files)?;
    buffer.clear();
    Ok(())
}

fn register_provisional_catalog<R: RecordCodec>(
    state: &mut BuildState<'_>,
    provisionals: Vec<ProvisionalRun<R>>,
    parents: &[RunMeta],
) -> Result<Vec<RunMeta>> {
    if provisionals.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return integrity("external cDBG provisional runs are not in canonical numeric-ID order");
    }
    let mut runs = Vec::new();
    runs.try_reserve_exact(provisionals.len())
        .map_err(|cause| {
            VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("cannot reserve registered external cDBG run catalog: {cause}"),
            )
        })?;
    enforce_capacity(
        runs.capacity(),
        provisionals.len(),
        "registered external cDBG run catalog",
    )?;
    for provisional in provisionals {
        let run = state.seal_registered_run(provisional, parents)?;
        runs.push(run);
    }
    Ok(runs)
}

fn merge_group<R: RecordCodec>(
    state: &mut BuildState<'_>,
    parents: &[RunMeta],
    generation: u32,
) -> Result<RunMeta> {
    if parents.len() < 2 {
        return integrity("external cDBG merge group must contain at least two runs");
    }
    if parents.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return integrity("external cDBG merge parents are not in numeric ID order");
    }
    let expected_records = parents.iter().try_fold(0_u64, |total, parent| {
        total
            .checked_add(parent.header.record_count)
            .ok_or_else(|| overflow("external cDBG merge record total overflow"))
    })?;
    let mut pending = state.begin_run::<R>(generation, expected_records, parents)?;
    let block_bytes = as_usize(
        u64::from(state.options.limits.block_payload_bytes),
        "external cDBG merge block capacity",
    )?;
    let mut readers = Vec::new();
    readers.try_reserve_exact(parents.len()).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve external cDBG merge readers: {cause}"),
        )
    })?;
    enforce_capacity(
        readers.capacity(),
        parents.len(),
        "external cDBG merge readers",
    )?;
    let mut heap = BinaryHeap::new();
    heap.try_reserve_exact(parents.len()).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve external cDBG merge heap: {cause}"),
        )
    })?;
    enforce_capacity(heap.capacity(), parents.len(), "external cDBG merge heap")?;
    for (reader_index, &parent) in parents.iter().enumerate() {
        let mut reader = BlockReader::<R>::open_registered(
            &run_path(state.run_dir, parent.id)?,
            parent,
            block_bytes,
            state.options.virtual_partition_count,
        )?;
        if let Some(record) = reader.next_record()? {
            heap.push(HeapItem {
                record,
                reader_index,
            });
        }
        readers.push(reader);
    }
    let opened = u16::try_from(parents.len() + 1)
        .map_err(|_| overflow("external cDBG merge open-file count does not fit u16"))?;
    if opened > state.options.limits.max_open_files {
        return Err(VeritasmError::new(
            ErrorCode::ResourceOpenFiles,
            "external cDBG merge exceeded its admitted open-file count",
        ));
    }
    state.open_files_high_water = state.open_files_high_water.max(opened);
    while let Some(item) = heap.pop() {
        pending.writer.push(item.record)?;
        if let Some(record) = readers[item.reader_index].next_record()? {
            heap.push(HeapItem {
                record,
                reader_index: item.reader_index,
            });
        }
    }
    for reader in readers {
        reader.finish()?;
    }
    let output = state.finish_run(pending, parents)?;
    for &parent in parents {
        state.delete_run(parent)?;
    }
    Ok(output)
}

fn merge_all<R: RecordCodec>(
    state: &mut BuildState<'_>,
    mut current: Vec<RunMeta>,
) -> Result<RunMeta> {
    if current.is_empty() {
        return integrity("external cDBG merge received no runs");
    }
    let fan_in = usize::from(state.options.limits.merge_fan_in);
    let mut generation = current
        .iter()
        .map(|run| run.id.generation)
        .max()
        .ok_or_else(|| integrity_error("external cDBG merge lost its first generation"))?
        .checked_add(1)
        .ok_or_else(|| overflow("external cDBG merge generation overflow"))?;
    while current.len() > 1 {
        current = merge_generation::<R>(state, current, generation, fan_in)?;
        generation = generation
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG merge generation overflow"))?;
    }
    current
        .pop()
        .ok_or_else(|| integrity_error("external cDBG merge lost its final run"))
}

fn merge_generation<R: RecordCodec>(
    state: &mut BuildState<'_>,
    mut current: Vec<RunMeta>,
    generation: u32,
    fan_in: usize,
) -> Result<Vec<RunMeta>> {
    if current.len() < 2 || fan_in < 2 {
        return integrity("external cDBG merge generation has an invalid input plan");
    }
    current.sort_unstable_by_key(|run| run.id);
    let group_count = current
        .len()
        .checked_add(fan_in - 1)
        .ok_or_else(|| overflow("external cDBG merge group rounding overflow"))?
        / fan_in;
    let mut next = Vec::new();
    next.try_reserve_exact(group_count).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve next external cDBG merge catalog: {cause}"),
        )
    })?;
    enforce_capacity(
        next.capacity(),
        group_count,
        "next external cDBG merge catalog",
    )?;
    for group in current.chunks(fan_in) {
        let output = if group.len() == 1 {
            group[0]
        } else {
            merge_group::<R>(state, group, generation)?
        };
        next.push(output);
    }
    Ok(next)
}

fn merge_node_audit_pair(
    state: &mut BuildState<'_>,
    mut direct: Vec<RunMeta>,
    mut mirror: Vec<RunMeta>,
) -> Result<(RunMeta, RunMeta)> {
    if direct.is_empty() || mirror.is_empty() || direct.len() != mirror.len() {
        return integrity("external cDBG node-audit catalogs have inconsistent cardinality");
    }
    let fan_in = usize::from(state.options.limits.merge_fan_in);
    let mut generation = direct
        .iter()
        .chain(&mirror)
        .map(|run| run.id.generation)
        .max()
        .ok_or_else(|| integrity_error("external cDBG node audit has no first generation"))?
        .checked_add(1)
        .ok_or_else(|| overflow("external cDBG node-audit merge generation overflow"))?;
    while direct.len() > 1 || mirror.len() > 1 {
        if direct.len() != mirror.len() {
            return integrity("external cDBG paired node-audit merge plans diverged");
        }
        direct = merge_generation::<ExternalNodeAuditRecord>(state, direct, generation, fan_in)?;
        mirror = merge_generation::<ExternalNodeAuditRecord>(state, mirror, generation, fan_in)?;
        generation = generation
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG node-audit merge generation overflow"))?;
    }
    let direct = direct
        .pop()
        .ok_or_else(|| integrity_error("external cDBG lost direct node-audit run"))?;
    let mirror = mirror
        .pop()
        .ok_or_else(|| integrity_error("external cDBG lost mirror node-audit run"))?;
    Ok((direct, mirror))
}

struct TopologyParts {
    edge_run: RunMeta,
    node_run: RunMeta,
    edge_table_root: [u8; 32],
    node_table_root: [u8; 32],
    node_symmetry_root: [u8; 32],
    incidence_content_root: [u8; 32],
    ancestry_root: [u8; 32],
    ancestry_links: Vec<ExternalAncestryLink>,
    stats: ExternalCdbgStats,
}

struct NodeReduction {
    runs: Vec<RunMeta>,
    table_root: [u8; 32],
    literal_nodes: u64,
    hard_boundaries: u64,
    observed_incidences: u64,
}

#[derive(Debug)]
struct NodeAuditResult {
    table_root: [u8; 32],
    symmetry_root: [u8; 32],
    verified_nodes: u64,
}

/// Build EC-1a from an explicitly unverified materialized reducer adapter.
///
/// This is deliberately a capped bridge for tests and small independent
/// oracles.  It first admits a second edge vector under `max_edges` and the
/// memory budget. The type name prevents callers from mistaking source labels
/// in the materialized result for authenticated provenance. A future retained-
/// run reducer must call the streaming core directly rather than using this
/// adapter.
pub fn build_external_cdbg_from_unverified_materialized(
    options: &ExternalCdbgOptions,
    adapter: UnverifiedMaterializedEdgeAdapter<'_>,
) -> Result<ExternalCdbgTopology> {
    build_external_cdbg_from_unverified_materialized_with_fault(
        options,
        adapter,
        FaultInjector::disabled(),
    )
}

fn build_external_cdbg_from_unverified_materialized_with_fault(
    options: &ExternalCdbgOptions,
    adapter: UnverifiedMaterializedEdgeAdapter<'_>,
    fault: FaultInjector,
) -> Result<ExternalCdbgTopology> {
    let input = adapter.input;
    let baseline_memory = validate_options(options)?;
    validate_adapter_binding(options, input)?;
    let edge_count = u64::try_from(input.edge_counts.len())
        .map_err(|_| overflow("external cDBG adapter edge count does not fit u64"))?;
    if edge_count > options.limits.max_edges {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            format!(
                "external cDBG adapter received {edge_count} edges, exceeding cap {}",
                options.limits.max_edges
            ),
        ));
    }
    let adapter_bytes = edge_count
        .checked_mul(size_of::<ExternalEdgeRecord>() as u64)
        .ok_or_else(|| overflow("external cDBG adapter edge-buffer bytes overflow"))?;
    let admitted = baseline_memory
        .checked_add(adapter_bytes)
        .ok_or_else(|| overflow("external cDBG adapter memory total overflow"))?;
    if admitted > options.limits.max_memory_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "external cDBG adapter requires at least {admitted} accounted bytes, exceeding limit {}",
                options.limits.max_memory_bytes
            ),
        ));
    }
    let capacity = as_usize(edge_count, "external cDBG adapter edge count")?;
    let mut edges = Vec::new();
    edges.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve external cDBG adapter edge buffer: {cause}"),
        )
    })?;
    enforce_capacity(
        edges.capacity(),
        capacity,
        "external cDBG adapter edge buffer",
    )?;
    let mut supplied_support = 0_u64;
    for row in &input.edge_counts {
        validate_code(row.key, options.k).map_err(|error| {
            integrity_error(format!(
                "external cDBG adapter received an invalid packed edge: {error}"
            ))
        })?;
        if canonical_code(row.key, options.k).map_err(|error| {
            integrity_error(format!(
                "external cDBG adapter cannot canonicalize an input edge: {error}"
            ))
        })? != row.key
            || row.support == 0
        {
            return integrity("external cDBG adapter received a noncanonical or zero-support edge");
        }
        let owner = select_minimizer(row.key, options.k, input.minimizer_length)?;
        let bucket = route_minimizer(
            owner.key,
            input.minimizer_length,
            input.virtual_bucket_count,
        )?;
        if owner.key != row.minimizer || bucket != row.bucket_id {
            return integrity("external cDBG adapter row routing disagrees with its exact key");
        }
        supplied_support = supplied_support
            .checked_add(row.support)
            .ok_or_else(|| overflow("external cDBG adapter support total overflow"))?;
        edges.push(ExternalEdgeRecord {
            key: row.key,
            support: row.support,
        });
    }
    if supplied_support != input.support_events {
        return integrity("external cDBG adapter support mass disagrees with its source result");
    }
    edges.sort_unstable_by_key(|record| record.key);
    if edges.windows(2).any(|pair| pair[0].key >= pair[1].key) {
        return integrity("external cDBG adapter retained-edge keys are not unique");
    }
    let edge_table_root = edge_table_root(options, &edges, supplied_support)?;
    let guard = RunDirectoryGuard::create(&options.work_dir, options.limits.max_run_files)?;
    let outcome = build_topology_inner(
        options,
        edges,
        edge_table_root,
        supplied_support,
        &guard.path,
        fault,
    );
    match outcome {
        Ok(parts) => {
            let block_payload_bytes = as_usize(
                u64::from(options.limits.block_payload_bytes),
                "external cDBG retained block capacity",
            )?;
            let (run_dir, run_dir_identity) = guard.transfer();
            Ok(ExternalCdbgTopology {
                run_dir,
                run_dir_identity,
                edge_run: parts.edge_run,
                node_run: parts.node_run,
                block_payload_bytes,
                max_cleanup_entries: options.limits.max_run_files,
                armed: true,
                k: options.k,
                node_minimizer_length: options.node_minimizer_length,
                virtual_partition_count: options.virtual_partition_count,
                source_root: options.source_root,
                scientific_config_root: options.scientific_config_root,
                support_unit: options.support_unit,
                edge_table_root: parts.edge_table_root,
                node_table_root: parts.node_table_root,
                node_symmetry_root: parts.node_symmetry_root,
                edge_run_summary: parts.edge_run.summary(),
                node_run_summary: parts.node_run.summary(),
                incidence_content_root: parts.incidence_content_root,
                ancestry_root: parts.ancestry_root,
                ancestry_links: parts.ancestry_links,
                stats: parts.stats,
            })
        }
        Err(primary) => guard.cleanup_after_error(primary),
    }
}

fn validate_adapter_binding(
    options: &ExternalCdbgOptions,
    input: &ExternalPartitionResult,
) -> Result<()> {
    if input.k != options.k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationProfileConflict,
            "external cDBG k disagrees with its retained-edge input",
        ));
    }
    if input.source_identity != options.source_root {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationProfileConflict,
            "external cDBG source root disagrees with its retained-edge input",
        ));
    }
    if input.support_unit != options.support_unit {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationProfileConflict,
            "external cDBG support unit disagrees with its retained-edge input",
        ));
    }
    let rows = u64::try_from(input.edge_counts.len())
        .map_err(|_| overflow("external cDBG input row count does not fit u64"))?;
    if rows != input.distinct_kmers {
        return integrity("external cDBG input distinct count disagrees with its rows");
    }
    if input.minimizer_length == 0
        || input.minimizer_length > input.k
        || input.virtual_bucket_count == 0
    {
        return integrity("external cDBG input routing domain is invalid");
    }
    Ok(())
}

fn validate_options(options: &ExternalCdbgOptions) -> Result<u64> {
    validate_k(options.k)?;
    if options.node_minimizer_length == 0 || options.node_minimizer_length >= options.k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "external cDBG node minimizer must be in 1..k; received {} for k={}",
                options.node_minimizer_length, options.k
            ),
        ));
    }
    if options.virtual_partition_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG virtual partition count must be nonzero",
        ));
    }
    let metadata = fs::metadata(&options.work_dir).map_err(|cause| {
        io_error(
            ErrorCode::ResourceTemporaryBytes,
            "stat external cDBG work directory",
            cause,
        )
    })?;
    if !metadata.is_dir() {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG work path is not a directory",
        ));
    }
    let limits = options.limits;
    if limits.max_memory_bytes == 0
        || limits.sort_buffer_bytes == 0
        || limits.max_temp_bytes == 0
        || limits.max_run_files == 0
        || limits.max_edges == 0
        || limits.max_handles == 0
        || limits.max_incidences == 0
        || limits.max_nodes == 0
    {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG limits must all be nonzero",
        ));
    }
    if limits.merge_fan_in < 2 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG merge fan-in must be at least two",
        ));
    }
    let merge_open = limits
        .merge_fan_in
        .checked_add(1)
        .ok_or_else(|| overflow("external cDBG merge open-file limit overflow"))?;
    if merge_open > limits.max_open_files {
        return Err(VeritasmError::new(
            ErrorCode::ResourceOpenFiles,
            format!(
                "external cDBG merge needs {merge_open} files but max_open_files is {}",
                limits.max_open_files
            ),
        ));
    }
    let key_width = ExternalKeyWidth::for_length(options.k)?;
    let maximum_record = [
        record_width_for(
            ExternalRecordKind::Edge,
            key_width,
            options.node_minimizer_length,
        )?,
        record_width_for(
            ExternalRecordKind::Incidence,
            key_width,
            options.node_minimizer_length,
        )?,
        record_width_for(
            ExternalRecordKind::NodeState,
            key_width,
            options.node_minimizer_length,
        )?,
        record_width_for(
            ExternalRecordKind::NodeAudit,
            key_width,
            options.node_minimizer_length,
        )?,
    ]
    .into_iter()
    .max()
    .ok_or_else(|| integrity_error("external cDBG has no record widths"))?;
    if limits.block_payload_bytes < u32::from(maximum_record) {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "external cDBG block payload cannot hold its widest record",
        ));
    }
    let widest_sort_record =
        size_of::<ExternalIncidenceRecord>().max(size_of::<ExternalNodeStateRecord>()) as u64;
    if limits.sort_buffer_bytes < widest_sort_record {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!(
                "external cDBG sort buffer must hold at least one record ({widest_sort_record} bytes)"
            ),
        ));
    }
    let fan_in = u64::from(limits.merge_fan_in);
    let block_buffers = u64::from(limits.block_payload_bytes)
        .checked_mul(
            fan_in
                .checked_add(1)
                .ok_or_else(|| overflow("external cDBG buffer-count overflow"))?,
        )
        .ok_or_else(|| overflow("external cDBG block-buffer bytes overflow"))?;
    let widest_reader = [
        size_of::<BlockReader<ExternalEdgeRecord>>(),
        size_of::<BlockReader<ExternalIncidenceRecord>>(),
        size_of::<BlockReader<ExternalNodeStateRecord>>(),
        size_of::<BlockReader<ExternalNodeAuditRecord>>(),
    ]
    .into_iter()
    .max()
    .ok_or_else(|| integrity_error("external cDBG has no reader object widths"))?
        as u64;
    let reader_objects = fan_in
        .checked_mul(widest_reader)
        .ok_or_else(|| overflow("external cDBG reader-object bytes overflow"))?;
    let widest_heap_item = [
        size_of::<HeapItem<ExternalEdgeRecord>>(),
        size_of::<HeapItem<ExternalIncidenceRecord>>(),
        size_of::<HeapItem<ExternalNodeStateRecord>>(),
        size_of::<HeapItem<ExternalNodeAuditRecord>>(),
    ]
    .into_iter()
    .max()
    .ok_or_else(|| integrity_error("external cDBG has no merge-heap object widths"))?
        as u64;
    let heap_objects = fan_in
        .checked_mul(widest_heap_item)
        .ok_or_else(|| overflow("external cDBG merge-heap bytes overflow"))?;
    let catalog_entry_bytes = (size_of::<RunMeta>() as u64)
        .max(size_of::<ProvisionalRun<ExternalNodeAuditRecord>>() as u64);
    let catalogs = limits
        .max_run_files
        .checked_mul(catalog_entry_bytes)
        .and_then(|value| value.checked_mul(RUN_CATALOG_MULTIPLIER))
        .ok_or_else(|| overflow("external cDBG run-catalog bytes overflow"))?;
    let ancestry = limits
        .max_run_files
        .checked_mul(ANCESTRY_LINK_MULTIPLIER)
        .and_then(|value| value.checked_mul(size_of::<ExternalAncestryLink>() as u64))
        .ok_or_else(|| overflow("external cDBG ancestry-ledger bytes overflow"))?;
    let path_bytes = u64::try_from(options.work_dir.as_os_str().len())
        .map_err(|_| overflow("external cDBG work path length does not fit u64"))?
        .checked_add(PATH_ALLOWANCE_BYTES)
        .and_then(|value| value.checked_mul(3))
        .ok_or_else(|| overflow("external cDBG path-buffer bytes overflow"))?;
    let baseline = limits
        .sort_buffer_bytes
        .checked_add(block_buffers)
        .and_then(|value| value.checked_add(reader_objects))
        .and_then(|value| value.checked_add(heap_objects))
        .and_then(|value| value.checked_add(catalogs))
        .and_then(|value| value.checked_add(ancestry))
        .and_then(|value| value.checked_add(path_bytes))
        .and_then(|value| value.checked_add(4096))
        .ok_or_else(|| overflow("external cDBG accounted memory total overflow"))?;
    if baseline > limits.max_memory_bytes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "external cDBG requires at least {baseline} accounted bytes, exceeding limit {}",
                limits.max_memory_bytes
            ),
        ));
    }
    as_usize(
        u64::from(limits.block_payload_bytes),
        "external cDBG block payload limit",
    )?;
    as_usize(limits.max_run_files, "external cDBG run-file limit")?;
    Ok(baseline)
}

fn edge_table_root(
    options: &ExternalCdbgOptions,
    edges: &[ExternalEdgeRecord],
    support_mass: u64,
) -> Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(EDGE_TABLE_ROOT_DOMAIN);
    digest.update(options.source_root);
    digest.update(options.scientific_config_root);
    digest.update([options.k, support_unit_tag(options.support_unit)]);
    digest.update(
        u64::try_from(edges.len())
            .map_err(|_| overflow("external cDBG edge-table row count does not fit u64"))?
            .to_le_bytes(),
    );
    digest.update(support_mass.to_le_bytes());
    for edge in edges {
        digest.update(edge.key.to_be_bytes());
        digest.update(edge.support.to_le_bytes());
    }
    Ok(digest.finalize().into())
}

fn recompute_edge_table_root_from_run(
    state: &BuildState<'_>,
    edge_run: RunMeta,
) -> Result<([u8; 32], u64)> {
    let block_bytes = as_usize(
        u64::from(state.options.limits.block_payload_bytes),
        "external cDBG retained EDGE verification block capacity",
    )?;
    let mut reader = BlockReader::<ExternalEdgeRecord>::open_registered(
        &run_path(state.run_dir, edge_run.id)?,
        edge_run,
        block_bytes,
        state.options.virtual_partition_count,
    )?;
    let mut support_mass = 0_u64;
    while let Some(edge) = reader.next_record()? {
        support_mass = support_mass
            .checked_add(edge.support)
            .ok_or_else(|| overflow("external cDBG final EDGE support total overflow"))?;
    }
    reader.finish()?;

    // The support total precedes rows in the canonical root. Reinitialize the
    // fixed prefix after the complete authenticated scan rather than caching
    // rows or trusting the pre-seal value.
    let mut root = Sha256::new();
    root.update(EDGE_TABLE_ROOT_DOMAIN);
    root.update(state.options.source_root);
    root.update(state.options.scientific_config_root);
    root.update([
        state.options.k,
        support_unit_tag(state.options.support_unit),
    ]);
    root.update(edge_run.header.record_count.to_le_bytes());
    root.update(support_mass.to_le_bytes());

    // SHA-256 state cannot concatenate a second digest in place of the exact
    // row byte stream. Authenticate a second bounded pass so the canonical
    // root is computed without materializing retained rows.
    let mut second = BlockReader::<ExternalEdgeRecord>::open_registered(
        &run_path(state.run_dir, edge_run.id)?,
        edge_run,
        block_bytes,
        state.options.virtual_partition_count,
    )?;
    while let Some(edge) = second.next_record()? {
        root.update(edge.key.to_be_bytes());
        root.update(edge.support.to_le_bytes());
    }
    second.finish()?;
    Ok((root.finalize().into(), support_mass))
}

fn complement_degree_mask(mask: u8) -> Result<u8> {
    if mask & !0x0f != 0 {
        return integrity("external cDBG degree mask contains a non-DNA bit");
    }
    Ok(mask.reverse_bits() >> 4)
}

fn node_audit_projection(
    state: ExternalNodeStateRecord,
    k: u8,
    reverse_complement: bool,
) -> Result<ExternalNodeAuditRecord> {
    if reverse_complement {
        Ok(ExternalNodeAuditRecord {
            node: reverse_complement_code(state.node, k - 1)?,
            incoming_mask: complement_degree_mask(state.outgoing_mask)?,
            outgoing_mask: complement_degree_mask(state.incoming_mask)?,
            reverse_fixed: state.reverse_fixed,
            incident_fixed_edge: state.incident_fixed_edge,
            hard_boundary: state.hard_boundary,
        })
    } else {
        Ok(ExternalNodeAuditRecord {
            node: state.node,
            incoming_mask: state.incoming_mask,
            outgoing_mask: state.outgoing_mask,
            reverse_fixed: state.reverse_fixed,
            incident_fixed_edge: state.incident_fixed_edge,
            hard_boundary: state.hard_boundary,
        })
    }
}

/// Project the final owner-routed node table into literal-node order. Runs stay
/// structurally unpublishable until the complete NODE_STATE parent has passed
/// its trailer, exact-EOF, and descriptor checks.
fn build_node_audit_projection_runs(
    state: &mut BuildState<'_>,
    node_run: RunMeta,
    reverse_complement: bool,
) -> Result<Vec<RunMeta>> {
    let capacity = sort_capacity::<ExternalNodeAuditRecord>(
        state.options.limits.sort_buffer_bytes,
        "node-audit sort",
    )?;
    let mut buffer = allocate_sort_buffer(capacity, "node-audit sort buffer")?;
    let mut provisionals = Vec::new();
    let block_bytes = as_usize(
        u64::from(state.options.limits.block_payload_bytes),
        "external cDBG node-audit parent block capacity",
    )?;
    let mut reader = BlockReader::<ExternalNodeStateRecord>::open_registered(
        &run_path(state.run_dir, node_run.id)?,
        node_run,
        block_bytes,
        state.options.virtual_partition_count,
    )?;
    let mut projected = 0_u64;
    while let Some(record) = reader.next_record()? {
        if buffer.len() == buffer.capacity() {
            state.open_files_high_water = state.open_files_high_water.max(2);
            provision_sorted_buffer(state, &mut buffer, 0, &[node_run], &mut provisionals)?;
        }
        buffer.push(node_audit_projection(
            record,
            state.options.k,
            reverse_complement,
        )?);
        projected = projected
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG node-audit projection count overflow"))?;
        if projected > state.options.limits.max_nodes {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRetainedKeys,
                "external cDBG node limit exceeded during final audit",
            ));
        }
    }
    reader.finish()?;
    if projected != node_run.header.record_count {
        return integrity("external cDBG node-audit projection count is not conserved");
    }
    if !buffer.is_empty() {
        provision_sorted_buffer(state, &mut buffer, 0, &[node_run], &mut provisionals)?;
    }
    let mut runs = register_provisional_catalog(state, provisionals, &[node_run])?;
    if runs.is_empty() {
        let empty = state.seal_slice::<ExternalNodeAuditRecord>(0, &[], &[node_run])?;
        push_catalog(&mut runs, empty, state.options.limits.max_run_files)?;
    }
    Ok(runs)
}

fn audit_final_node_table(
    state: &mut BuildState<'_>,
    node_run: RunMeta,
    edge_table_root: [u8; 32],
    observed_incidences: u64,
) -> Result<NodeAuditResult> {
    // Create and register all generation-zero leaves before either merge so
    // ancestry rows remain globally ordered by (kind,generation,ordinal).
    let direct_runs = build_node_audit_projection_runs(state, node_run, false)?;
    let mirror_runs = build_node_audit_projection_runs(state, node_run, true)?;
    let (direct_run, mirror_run) = merge_node_audit_pair(state, direct_runs, mirror_runs)?;

    if direct_run.header.record_count != node_run.header.record_count
        || mirror_run.header.record_count != node_run.header.record_count
    {
        return integrity("external cDBG final node-audit counts are not conserved");
    }
    if state.options.limits.max_open_files < 2 {
        return Err(VeritasmError::new(
            ErrorCode::ResourceOpenFiles,
            "external cDBG node symmetry audit needs two open files",
        ));
    }
    state.open_files_high_water = state.open_files_high_water.max(2);
    let block_bytes = as_usize(
        u64::from(state.options.limits.block_payload_bytes),
        "external cDBG final node-audit block capacity",
    )?;
    let mut direct = BlockReader::<ExternalNodeAuditRecord>::open_registered(
        &run_path(state.run_dir, direct_run.id)?,
        direct_run,
        block_bytes,
        state.options.virtual_partition_count,
    )?;
    let mut mirror = BlockReader::<ExternalNodeAuditRecord>::open_registered(
        &run_path(state.run_dir, mirror_run.id)?,
        mirror_run,
        block_bytes,
        state.options.virtual_partition_count,
    )?;

    let mut table = Sha256::new();
    table.update(NODE_TABLE_ROOT_DOMAIN);
    table.update(state.options.source_root);
    table.update(state.options.scientific_config_root);
    table.update(edge_table_root);
    table.update([
        state.options.k,
        support_unit_tag(state.options.support_unit),
    ]);
    let mut verified_nodes = 0_u64;
    loop {
        let direct_record = direct.next_record()?;
        let mirror_record = mirror.next_record()?;
        match (direct_record, mirror_record) {
            (Some(actual), Some(expected)) => {
                if actual != expected {
                    return integrity(
                        "external cDBG NODE_STATE reverse-complement mask/flag mirror mismatch",
                    );
                }
                table.update(actual.node.to_be_bytes());
                table.update([actual.incoming_mask, actual.outgoing_mask]);
                table.update([
                    u8::from(actual.reverse_fixed),
                    u8::from(actual.incident_fixed_edge),
                    u8::from(actual.hard_boundary),
                ]);
                verified_nodes = verified_nodes
                    .checked_add(1)
                    .ok_or_else(|| overflow("external cDBG verified node count overflow"))?;
                if verified_nodes > state.options.limits.max_nodes {
                    return Err(VeritasmError::new(
                        ErrorCode::ResourceRetainedKeys,
                        "external cDBG node limit exceeded during symmetry verification",
                    ));
                }
            }
            (None, None) => break,
            _ => {
                return integrity(
                    "external cDBG direct and reverse-complement node audits differ in length",
                )
            }
        }
    }
    direct.finish()?;
    mirror.finish()?;

    table.update(verified_nodes.to_le_bytes());
    table.update(observed_incidences.to_le_bytes());
    let table_root: [u8; 32] = table.finalize().into();
    let mut symmetry = Sha256::new();
    symmetry.update(NODE_SYMMETRY_ROOT_DOMAIN);
    symmetry.update(state.options.source_root);
    symmetry.update(state.options.scientific_config_root);
    symmetry.update(edge_table_root);
    symmetry.update([
        state.options.k,
        support_unit_tag(state.options.support_unit),
    ]);
    symmetry.update(table_root);
    symmetry.update(verified_nodes.to_le_bytes());
    symmetry.update(observed_incidences.to_le_bytes());
    let symmetry_root = symmetry.finalize().into();

    state.delete_run(direct_run)?;
    state.delete_run(mirror_run)?;
    Ok(NodeAuditResult {
        table_root,
        symmetry_root,
        verified_nodes,
    })
}

fn sort_capacity<R>(bytes: u64, label: &str) -> Result<usize> {
    let record_bytes = u64::try_from(size_of::<R>())
        .map_err(|_| overflow("external cDBG record object width does not fit u64"))?;
    if record_bytes == 0 {
        return integrity("external cDBG sort record unexpectedly has zero width");
    }
    let capacity = bytes / record_bytes;
    if capacity == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!("external cDBG {label} buffer cannot hold one record"),
        ));
    }
    as_usize(capacity, label)
}

fn allocate_sort_buffer<R>(capacity: usize, label: &str) -> Result<Vec<R>> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(capacity).map_err(|cause| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("cannot reserve external cDBG {label}: {cause}"),
        )
    })?;
    enforce_capacity(buffer.capacity(), capacity, label)?;
    Ok(buffer)
}

fn build_topology_inner(
    options: &ExternalCdbgOptions,
    edges: Vec<ExternalEdgeRecord>,
    edge_table_root: [u8; 32],
    expected_support: u64,
    run_dir: &Path,
    fault: FaultInjector,
) -> Result<TopologyParts> {
    let mut state = BuildState {
        options,
        run_dir,
        retained_source_root: edge_table_root,
        next_run_ordinal: 0,
        run_files_created: 0,
        reclaimed_files: 0,
        open_files_high_water: 0,
        temp: TempLedger {
            live: 0,
            high_water: 0,
            limit: options.limits.max_temp_bytes,
        },
        fault,
        ancestry: Vec::new(),
    };
    let edge_run = state.seal_slice::<ExternalEdgeRecord>(0, &edges, &[])?;
    drop(edges);
    let (retained_root, retained_support) = recompute_edge_table_root_from_run(&state, edge_run)?;
    if retained_root != edge_table_root || retained_support != expected_support {
        return integrity(
            "external cDBG retained EDGE stream disagrees with its pre-seal scientific root",
        );
    }

    let (incidence_runs, oriented_handles, incidences, fixed_edges, observed_support) =
        expand_incidence_runs(&mut state, edge_run)?;
    if observed_support != expected_support {
        return integrity("external cDBG EDGE support changed while expanding handles");
    }
    if incidences
        != oriented_handles
            .checked_mul(2)
            .ok_or_else(|| overflow("external cDBG handle/incidence conservation overflow"))?
    {
        return integrity("external cDBG handle/incidence conservation failed");
    }
    let incidence_run = merge_all::<ExternalIncidenceRecord>(&mut state, incidence_runs)?;
    if incidence_run.header.record_count != incidences {
        return integrity("external cDBG merged incidence count is not conserved");
    }
    let incidence_content_root = incidence_run.content_root;
    let node_reduction = reduce_node_runs(&mut state, incidence_run, edge_table_root)?;
    if node_reduction.observed_incidences != incidences {
        return integrity("external cDBG node reduction lost or duplicated an incidence");
    }
    let node_run = merge_all::<ExternalNodeStateRecord>(&mut state, node_reduction.runs)?;
    if node_run.header.record_count != node_reduction.literal_nodes {
        return integrity("external cDBG merged node count is not conserved");
    }
    let node_audit = audit_final_node_table(
        &mut state,
        node_run,
        edge_table_root,
        node_reduction.observed_incidences,
    )?;
    if node_audit.table_root != node_reduction.table_root
        || node_audit.verified_nodes != node_reduction.literal_nodes
    {
        return integrity(
            "external cDBG final NODE_STATE stream disagrees with its pre-seal scientific root",
        );
    }
    state.delete_run(incidence_run)?;
    let expected_final_temp = edge_run
        .byte_len
        .checked_add(node_run.byte_len)
        .ok_or_else(|| overflow("external cDBG final temporary-byte total overflow"))?;
    if state.temp.live != expected_final_temp {
        return integrity("external cDBG final temporary-byte accounting is inconsistent");
    }
    let expected_created_files = state
        .reclaimed_files
        .checked_add(2)
        .ok_or_else(|| overflow("external cDBG final file-count conservation overflow"))?;
    if state.run_files_created != expected_created_files {
        return integrity("external cDBG final run-file ownership is not conserved");
    }
    if state.open_files_high_water > options.limits.max_open_files {
        return integrity("external cDBG measured open-file high-water exceeded its admission");
    }
    let ancestry_root = ancestry_root(&state.ancestry)?;
    let ancestry_link_count = u64::try_from(state.ancestry.len())
        .map_err(|_| overflow("external cDBG ancestry-link count does not fit u64"))?;
    let ancestry_links = std::mem::take(&mut state.ancestry);
    Ok(TopologyParts {
        edge_run,
        node_run,
        edge_table_root,
        node_table_root: node_audit.table_root,
        node_symmetry_root: node_audit.symmetry_root,
        incidence_content_root,
        ancestry_root,
        ancestry_links,
        stats: ExternalCdbgStats {
            retained_edges: edge_run.header.record_count,
            support_mass: observed_support,
            oriented_handles,
            incidences,
            literal_nodes: node_reduction.literal_nodes,
            fixed_edges,
            hard_boundary_nodes: node_reduction.hard_boundaries,
            reverse_complement_nodes_verified: node_audit.verified_nodes,
            ancestry_links: ancestry_link_count,
            run_files_created: state.run_files_created,
            predecessor_files_reclaimed: state.reclaimed_files,
            temporary_bytes_final: state.temp.live,
            temporary_bytes_high_water: state.temp.high_water,
            open_files_high_water: state.open_files_high_water,
        },
    })
}

fn expand_incidence_runs(
    state: &mut BuildState<'_>,
    edge_run: RunMeta,
) -> Result<(Vec<RunMeta>, u64, u64, u64, u64)> {
    let capacity = sort_capacity::<ExternalIncidenceRecord>(
        state.options.limits.sort_buffer_bytes,
        "incidence sort",
    )?;
    let mut buffer = allocate_sort_buffer(capacity, "incidence sort buffer")?;
    let mut provisionals = Vec::new();
    let block_bytes = as_usize(
        u64::from(state.options.limits.block_payload_bytes),
        "external cDBG edge-reader block capacity",
    )?;
    let mut reader = BlockReader::<ExternalEdgeRecord>::open_registered(
        &run_path(state.run_dir, edge_run.id)?,
        edge_run,
        block_bytes,
        state.options.virtual_partition_count,
    )?;
    let mut handles = 0_u64;
    let mut incidences = 0_u64;
    let mut fixed_edges = 0_u64;
    let mut support_mass = 0_u64;
    while let Some(edge) = reader.next_record()? {
        support_mass = support_mass
            .checked_add(edge.support)
            .ok_or_else(|| overflow("external cDBG EDGE support total overflow"))?;
        let reverse = reverse_complement_code(edge.key, state.options.k)?;
        let fixed = reverse == edge.key;
        if fixed {
            fixed_edges = fixed_edges
                .checked_add(1)
                .ok_or_else(|| overflow("external cDBG fixed-edge count overflow"))?;
        }
        emit_handle_incidences(
            state,
            edge.key,
            fixed,
            edge_run,
            &mut buffer,
            &mut provisionals,
            &mut handles,
            &mut incidences,
        )?;
        if !fixed {
            emit_handle_incidences(
                state,
                reverse,
                false,
                edge_run,
                &mut buffer,
                &mut provisionals,
                &mut handles,
                &mut incidences,
            )?;
        }
    }
    reader.finish()?;
    if !buffer.is_empty() {
        provision_sorted_buffer(state, &mut buffer, 0, &[edge_run], &mut provisionals)?;
    }
    let mut runs = register_provisional_catalog(state, provisionals, &[edge_run])?;
    if runs.is_empty() {
        let empty = state.seal_slice::<ExternalIncidenceRecord>(0, &[], &[edge_run])?;
        push_catalog(&mut runs, empty, state.options.limits.max_run_files)?;
    }
    Ok((runs, handles, incidences, fixed_edges, support_mass))
}

#[allow(clippy::too_many_arguments)]
fn emit_handle_incidences(
    state: &mut BuildState<'_>,
    handle: PackedKmer,
    fixed_edge: bool,
    edge_run: RunMeta,
    buffer: &mut Vec<ExternalIncidenceRecord>,
    provisionals: &mut Vec<ProvisionalRun<ExternalIncidenceRecord>>,
    handles: &mut u64,
    incidences: &mut u64,
) -> Result<()> {
    *handles = handles
        .checked_add(1)
        .ok_or_else(|| overflow("external cDBG handle count overflow"))?;
    if *handles > state.options.limits.max_handles {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            "external cDBG handle limit exceeded during expansion",
        ));
    }
    let outgoing = ExternalIncidenceRecord {
        node: prefix_code(handle, state.options.k)?,
        handle,
        direction: IncidenceDirection::Outgoing,
        fixed_edge,
    };
    let incoming = ExternalIncidenceRecord {
        node: suffix_code(handle, state.options.k)?,
        handle,
        direction: IncidenceDirection::Incoming,
        fixed_edge,
    };
    for record in [outgoing, incoming] {
        if buffer.len() == buffer.capacity() {
            state.open_files_high_water = state.open_files_high_water.max(2);
            provision_sorted_buffer(state, buffer, 0, &[edge_run], provisionals)?;
            corrupt_parent_after_provisional_if_injected(state, edge_run)?;
        }
        buffer.push(record);
        *incidences = incidences
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG incidence count overflow"))?;
        if *incidences > state.options.limits.max_incidences {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRetainedKeys,
                "external cDBG incidence limit exceeded during expansion",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct NodeAccumulator {
    node: PackedKmer,
    incoming_mask: u8,
    outgoing_mask: u8,
    incident_fixed_edge: bool,
}

impl NodeAccumulator {
    fn new(node: PackedKmer) -> Self {
        Self {
            node,
            incoming_mask: 0,
            outgoing_mask: 0,
            incident_fixed_edge: false,
        }
    }

    fn add(&mut self, record: ExternalIncidenceRecord, k: u8) -> Result<()> {
        if record.node != self.node {
            return integrity("external cDBG incidence was reduced in the wrong node group");
        }
        let base = match record.direction {
            IncidenceDirection::Incoming => packed_pair_at(record.handle, k, 0)?,
            IncidenceDirection::Outgoing => packed_pair_at(record.handle, k, k - 1)?,
        };
        let bit = 1_u8
            .checked_shl(u32::from(base))
            .ok_or_else(|| overflow("external cDBG degree-mask shift overflow"))?;
        let mask = match record.direction {
            IncidenceDirection::Incoming => &mut self.incoming_mask,
            IncidenceDirection::Outgoing => &mut self.outgoing_mask,
        };
        if *mask & bit != 0 {
            return integrity(
                "external cDBG node has two exact handles with the same direction and terminal base",
            );
        }
        *mask |= bit;
        self.incident_fixed_edge |= record.fixed_edge;
        Ok(())
    }

    fn finish(self, options: &ExternalCdbgOptions) -> Result<ExternalNodeStateRecord> {
        let node_length = options
            .k
            .checked_sub(1)
            .ok_or_else(|| overflow("external cDBG node length underflow"))?;
        let (owner_minimizer, owner_partition) = node_owner(
            self.node,
            node_length,
            options.node_minimizer_length,
            options.virtual_partition_count,
        )?;
        let reverse_fixed = reverse_complement_code(self.node, node_length)? == self.node;
        let degree_boundary =
            self.incoming_mask.count_ones() != 1 || self.outgoing_mask.count_ones() != 1;
        let hard_boundary = degree_boundary || reverse_fixed || self.incident_fixed_edge;
        let record = ExternalNodeStateRecord {
            node: self.node,
            owner_minimizer,
            owner_partition,
            incoming_mask: self.incoming_mask,
            outgoing_mask: self.outgoing_mask,
            reverse_fixed,
            incident_fixed_edge: self.incident_fixed_edge,
            hard_boundary,
        };
        let domain = RunDomain {
            kind: ExternalRecordKind::NodeState,
            key_width: ExternalKeyWidth::for_length(options.k)?,
            k: options.k,
            node_minimizer_length: options.node_minimizer_length,
            support_unit: options.support_unit,
            virtual_partition: GLOBAL_PARTITION,
            generation: 0,
            run_ordinal: 0,
            source_root: options.source_root,
            scientific_config_root: options.scientific_config_root,
            parent_set_root: [0_u8; 32],
            record_width: record_width_for(
                ExternalRecordKind::NodeState,
                ExternalKeyWidth::for_length(options.k)?,
                options.node_minimizer_length,
            )?,
            max_block_payload: options.limits.block_payload_bytes,
            owner_partition_count: options.virtual_partition_count,
        };
        record.validate(domain)?;
        Ok(record)
    }
}

fn reduce_node_runs(
    state: &mut BuildState<'_>,
    incidence_run: RunMeta,
    edge_table_root: [u8; 32],
) -> Result<NodeReduction> {
    let capacity = sort_capacity::<ExternalNodeStateRecord>(
        state.options.limits.sort_buffer_bytes,
        "node-state sort",
    )?;
    let mut buffer = allocate_sort_buffer(capacity, "node-state sort buffer")?;
    let mut provisionals = Vec::new();
    let block_bytes = as_usize(
        u64::from(state.options.limits.block_payload_bytes),
        "external cDBG incidence-reader block capacity",
    )?;
    let mut reader = BlockReader::<ExternalIncidenceRecord>::open_registered(
        &run_path(state.run_dir, incidence_run.id)?,
        incidence_run,
        block_bytes,
        state.options.virtual_partition_count,
    )?;
    let mut table_digest = Sha256::new();
    table_digest.update(NODE_TABLE_ROOT_DOMAIN);
    table_digest.update(state.options.source_root);
    table_digest.update(state.options.scientific_config_root);
    table_digest.update(edge_table_root);
    table_digest.update([
        state.options.k,
        support_unit_tag(state.options.support_unit),
    ]);
    let mut accumulator = None;
    let mut literal_nodes = 0_u64;
    let mut hard_boundaries = 0_u64;
    let mut observed_incidences = 0_u64;
    while let Some(incidence) = reader.next_record()? {
        observed_incidences = observed_incidences
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG reduced-incidence count overflow"))?;
        if observed_incidences > state.options.limits.max_incidences {
            return Err(VeritasmError::new(
                ErrorCode::ResourceRetainedKeys,
                "external cDBG incidence limit exceeded during node reduction",
            ));
        }
        if accumulator
            .as_ref()
            .is_some_and(|current: &NodeAccumulator| current.node != incidence.node)
        {
            let finished = accumulator
                .take()
                .ok_or_else(|| integrity_error("external cDBG lost its node accumulator"))?
                .finish(state.options)?;
            emit_node_state(
                state,
                finished,
                incidence_run,
                &mut table_digest,
                &mut buffer,
                &mut provisionals,
                &mut literal_nodes,
                &mut hard_boundaries,
            )?;
        }
        if accumulator.is_none() {
            accumulator = Some(NodeAccumulator::new(incidence.node));
        }
        accumulator
            .as_mut()
            .ok_or_else(|| integrity_error("external cDBG node accumulator is missing"))?
            .add(incidence, state.options.k)?;
    }
    if let Some(current) = accumulator {
        emit_node_state(
            state,
            current.finish(state.options)?,
            incidence_run,
            &mut table_digest,
            &mut buffer,
            &mut provisionals,
            &mut literal_nodes,
            &mut hard_boundaries,
        )?;
    }
    reader.finish()?;
    if !buffer.is_empty() {
        provision_sorted_buffer(state, &mut buffer, 0, &[incidence_run], &mut provisionals)?;
    }
    let mut runs = register_provisional_catalog(state, provisionals, &[incidence_run])?;
    if runs.is_empty() {
        let empty = state.seal_slice::<ExternalNodeStateRecord>(0, &[], &[incidence_run])?;
        push_catalog(&mut runs, empty, state.options.limits.max_run_files)?;
    }
    table_digest.update(literal_nodes.to_le_bytes());
    table_digest.update(observed_incidences.to_le_bytes());
    Ok(NodeReduction {
        runs,
        table_root: table_digest.finalize().into(),
        literal_nodes,
        hard_boundaries,
        observed_incidences,
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_node_state(
    state: &mut BuildState<'_>,
    node: ExternalNodeStateRecord,
    incidence_run: RunMeta,
    table_digest: &mut Sha256,
    buffer: &mut Vec<ExternalNodeStateRecord>,
    provisionals: &mut Vec<ProvisionalRun<ExternalNodeStateRecord>>,
    literal_nodes: &mut u64,
    hard_boundaries: &mut u64,
) -> Result<()> {
    *literal_nodes = literal_nodes
        .checked_add(1)
        .ok_or_else(|| overflow("external cDBG literal-node count overflow"))?;
    if *literal_nodes > state.options.limits.max_nodes {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            "external cDBG literal-node limit exceeded during reduction",
        ));
    }
    if node.hard_boundary {
        *hard_boundaries = hard_boundaries
            .checked_add(1)
            .ok_or_else(|| overflow("external cDBG hard-boundary count overflow"))?;
    }
    table_digest.update(node.node.to_be_bytes());
    table_digest.update([node.incoming_mask, node.outgoing_mask]);
    table_digest.update([
        u8::from(node.reverse_fixed),
        u8::from(node.incident_fixed_edge),
        u8::from(node.hard_boundary),
    ]);
    if buffer.len() == buffer.capacity() {
        state.open_files_high_water = state.open_files_high_water.max(2);
        provision_sorted_buffer(state, buffer, 0, &[incidence_run], provisionals)?;
    }
    buffer.push(node);
    Ok(())
}

fn read_u16(bytes: &[u8]) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes.try_into().map_err(|_| {
        integrity_error("external cDBG u16 field has the wrong width")
    })?))
}

fn read_u32(bytes: &[u8]) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| {
        integrity_error("external cDBG u32 field has the wrong width")
    })?))
}

fn read_u64(bytes: &[u8]) -> Result<u64> {
    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
        integrity_error("external cDBG u64 field has the wrong width")
    })?))
}

fn checked_usize_add(left: usize, right: usize, context: &str) -> Result<usize> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

fn as_usize(value: u64, context: &str) -> Result<usize> {
    usize::try_from(value).map_err(|_| overflow(format!("{context} does not fit usize")))
}

fn enforce_capacity(actual: usize, admitted: usize, label: &str) -> Result<()> {
    if actual < admitted {
        return Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            format!("external cDBG {label} capacity {actual} is below requested {admitted}"),
        ));
    }
    if actual > admitted {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("external cDBG {label} capacity {actual} exceeds admitted {admitted}"),
        ));
    }
    Ok(())
}

fn integrity<T>(context: impl Into<String>) -> Result<T> {
    Err(integrity_error(context))
}

fn integrity_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegrityCountRun, context)
}

fn overflow(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn io_error(code: ErrorCode, operation: &str, cause: std::io::Error) -> VeritasmError {
    VeritasmError::new(code, format!("{operation}: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimental::external_reduce::ExactSupportCount;
    use crate::experimental::wide_kmer::{decode_mer, encode_exact_bases};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    const TEST_SOURCE_ROOT: [u8; 32] = [0x41; 32];
    const TEST_CONFIG_ROOT: [u8; 32] = [0x93; 32];

    fn input(k: u8, minimizer_length: u8, sequences: &[(&str, u64)]) -> ExternalPartitionResult {
        let bucket_count = 17;
        let mut exact = BTreeMap::<PackedKmer, u64>::new();
        for &(sequence, support) in sequences {
            assert_eq!(sequence.len(), usize::from(k));
            let encoded = encode_exact_bases(sequence.as_bytes()).expect("encode test edge");
            let key = canonical_code(encoded, k).expect("canonicalize test edge");
            let entry = exact.entry(key).or_default();
            *entry = entry.checked_add(support).expect("test support fits");
        }
        let mut edge_counts = Vec::new();
        let mut support_events = 0_u64;
        for (key, support) in exact {
            let owner = select_minimizer(key, k, minimizer_length).expect("select test minimizer");
            let bucket_id = route_minimizer(owner.key, minimizer_length, bucket_count)
                .expect("route test minimizer");
            support_events = support_events
                .checked_add(support)
                .expect("test support fits");
            edge_counts.push(ExactSupportCount {
                bucket_id,
                minimizer: owner.key,
                key,
                support,
            });
        }
        edge_counts.sort_unstable_by_key(|row| (row.bucket_id, row.key));
        ExternalPartitionResult {
            k,
            minimizer_length,
            virtual_bucket_count: bucket_count,
            source_identity: TEST_SOURCE_ROOT,
            support_unit: WideRunSupportUnit::AcceptedWindowOccurrence,
            support_events,
            distinct_kmers: edge_counts.len() as u64,
            edge_counts,
            final_runs: Vec::new(),
            replacements: Vec::new(),
            run_files_created: 0,
            open_files_high_water: 0,
            temporary_bytes_final: 0,
            temporary_bytes_high_water: 0,
        }
    }

    fn options(work_dir: &Path, k: u8) -> ExternalCdbgOptions {
        ExternalCdbgOptions {
            work_dir: work_dir.to_path_buf(),
            k,
            node_minimizer_length: (k - 1).min(3),
            virtual_partition_count: 11,
            source_root: TEST_SOURCE_ROOT,
            scientific_config_root: TEST_CONFIG_ROOT,
            support_unit: WideRunSupportUnit::AcceptedWindowOccurrence,
            limits: ExternalCdbgLimits {
                max_memory_bytes: 16 * 1024 * 1024,
                sort_buffer_bytes: 512,
                max_temp_bytes: 64 * 1024 * 1024,
                max_run_files: 512,
                max_edges: 1024,
                max_handles: 2048,
                max_incidences: 4096,
                max_nodes: 4096,
                merge_fan_in: 2,
                max_open_files: 3,
                block_payload_bytes: 128,
            },
        }
    }

    fn build(directory: &TempDir, input: &ExternalPartitionResult) -> ExternalCdbgTopology {
        build_external_cdbg_from_unverified_materialized(
            &options(directory.path(), input.k),
            UnverifiedMaterializedEdgeAdapter::new(input),
        )
        .expect("build test topology")
    }

    fn run_directories(work_dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(work_dir)
            .expect("read test work directory")
            .map(|entry| entry.expect("read test work entry").path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(RUN_DIRECTORY_PREFIX))
            })
            .collect()
    }

    fn create_cleanup_test_file(run_dir: &Path, ordinal: u64, permissions: u32) -> PathBuf {
        let path = run_path(
            run_dir,
            ExternalRunId {
                kind: ExternalRecordKind::Edge,
                generation: 0,
                ordinal,
            },
        )
        .expect("derive cleanup test path");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(permissions)
            .open(&path)
            .expect("create cleanup test file");
        file.write_all(b"cleanup-test")
            .expect("write cleanup test file");
        file.sync_all().expect("sync cleanup test file");
        fs::set_permissions(&path, fs::Permissions::from_mode(permissions))
            .expect("set cleanup test permissions");
        path
    }

    fn reverse_ascii(sequence: &[u8]) -> Vec<u8> {
        sequence
            .iter()
            .rev()
            .map(|base| match base {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => panic!("test oracle received a non-ACGT base"),
            })
            .collect()
    }

    fn base_bit(base: u8) -> u8 {
        1 << match base {
            b'A' => 0,
            b'C' => 1,
            b'G' => 2,
            b'T' => 3,
            _ => panic!("test oracle received a non-ACGT base"),
        }
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct OracleNode {
        incoming: u8,
        outgoing: u8,
        incident_fixed: bool,
    }

    fn literal_oracle(input: &ExternalPartitionResult) -> BTreeMap<PackedKmer, OracleNode> {
        let mut nodes = BTreeMap::<PackedKmer, OracleNode>::new();
        for edge in &input.edge_counts {
            let canonical = decode_mer(edge.key, input.k).expect("decode oracle edge");
            let reverse = reverse_ascii(&canonical);
            let fixed = canonical == reverse;
            let handle_count = if fixed { 1 } else { 2 };
            for handle in [&canonical, &reverse].into_iter().take(handle_count) {
                let prefix =
                    encode_exact_bases(&handle[..handle.len() - 1]).expect("encode oracle prefix");
                let suffix = encode_exact_bases(&handle[1..]).expect("encode oracle suffix");
                let source = nodes.entry(prefix).or_default();
                source.outgoing |= base_bit(handle[handle.len() - 1]);
                source.incident_fixed |= fixed;
                let target = nodes.entry(suffix).or_default();
                target.incoming |= base_bit(handle[0]);
                target.incident_fixed |= fixed;
            }
        }
        nodes
    }

    fn canonical_ascii_kmers(k: u8) -> Vec<String> {
        let count = 4_usize.pow(u32::from(k));
        let alphabet = *b"ACGT";
        let mut canonical = Vec::new();
        for mut value in 0..count {
            let mut sequence = vec![b'A'; usize::from(k)];
            for position in (0..usize::from(k)).rev() {
                sequence[position] = alphabet[value & 3];
                value >>= 2;
            }
            if sequence <= reverse_ascii(&sequence) {
                canonical.push(String::from_utf8(sequence).expect("ASCII k-mer is UTF-8"));
            }
        }
        canonical
    }

    fn assert_matches_literal_oracle(
        topology: &ExternalCdbgTopology,
        source: &ExternalPartitionResult,
    ) {
        let expected = literal_oracle(source);
        let actual = topology
            .materialize_node_states(expected.len() as u64)
            .expect("materialize exhaustive node table");
        assert_eq!(actual.len(), expected.len());
        for node in actual {
            let oracle = expected
                .get(&node.node)
                .expect("exhaustive output node exists in ASCII oracle");
            assert_eq!(node.incoming_mask, oracle.incoming);
            assert_eq!(node.outgoing_mask, oracle.outgoing);
            assert_eq!(node.incident_fixed_edge, oracle.incident_fixed);
        }
        assert_eq!(
            topology.stats.reverse_complement_nodes_verified,
            expected.len() as u64
        );
    }

    #[test]
    fn exhaustive_k3_singletons_and_pairs_and_k4_singletons_match_ascii_oracle() {
        let directory = tempfile::tempdir().expect("create exhaustive-oracle directory");
        let k3 = canonical_ascii_kmers(3);
        assert_eq!(k3.len(), 32);
        for left in 0..k3.len() {
            for right in left..k3.len() {
                let records = if left == right {
                    vec![(k3[left].as_str(), 1_u64)]
                } else {
                    vec![(k3[left].as_str(), 1_u64), (k3[right].as_str(), 3_u64)]
                };
                let source = input(3, 2, &records);
                let topology = build(&directory, &source);
                assert_matches_literal_oracle(&topology, &source);
                topology.cleanup().expect("clean exhaustive k3 topology");
            }
        }

        let k4 = canonical_ascii_kmers(4);
        assert_eq!(k4.len(), 136);
        for edge in &k4 {
            let source = input(4, 2, &[(edge.as_str(), 5)]);
            let topology = build(&directory, &source);
            assert_matches_literal_oracle(&topology, &source);
            topology.cleanup().expect("clean exhaustive k4 topology");
        }
        assert!(run_directories(directory.path()).is_empty());
    }

    #[test]
    fn literal_oracle_and_all_conservation_totals_match() {
        let source = input(3, 2, &[("AAC", 5), ("ACG", 7), ("GGA", 11)]);
        let directory = tempfile::tempdir().expect("create test directory");
        let topology = build(&directory, &source);
        let expected = literal_oracle(&source);
        let actual = topology
            .materialize_node_states(100)
            .expect("materialize node states");
        assert_eq!(actual.len(), expected.len());
        for node in actual {
            let oracle = expected.get(&node.node).expect("node exists in oracle");
            assert_eq!(node.incoming_mask, oracle.incoming);
            assert_eq!(node.outgoing_mask, oracle.outgoing);
            assert_eq!(node.incident_fixed_edge, oracle.incident_fixed);
            let spelling = decode_mer(node.node, source.k - 1).expect("decode oracle node");
            let reverse_fixed = spelling == reverse_ascii(&spelling);
            assert_eq!(node.reverse_fixed, reverse_fixed);
            assert_eq!(
                node.hard_boundary,
                oracle.incoming.count_ones() != 1
                    || oracle.outgoing.count_ones() != 1
                    || reverse_fixed
                    || oracle.incident_fixed
            );
            assert!(node.owner_partition < topology.virtual_partition_count);
        }
        assert_eq!(topology.stats.retained_edges, source.distinct_kmers);
        assert_eq!(topology.stats.support_mass, source.support_events);
        assert_eq!(
            topology.stats.incidences,
            topology.stats.oriented_handles * 2
        );
        assert_eq!(topology.stats.literal_nodes, expected.len() as u64);
        assert_eq!(
            topology.stats.reverse_complement_nodes_verified,
            topology.stats.literal_nodes
        );
        assert_eq!(
            topology.stats.run_files_created,
            topology.stats.predecessor_files_reclaimed + 2
        );
        assert_eq!(
            topology.stats.temporary_bytes_final,
            topology.edge_run_summary.byte_len + topology.node_run_summary.byte_len
        );
        assert!(topology.stats.temporary_bytes_high_water >= topology.stats.temporary_bytes_final);
        assert!(topology.stats.open_files_high_water <= 3);
        assert_eq!(
            topology.stats.ancestry_links,
            topology.ancestry_links.len() as u64
        );
        assert_eq!(
            topology.ancestry_root,
            ancestry_root(&topology.ancestry_links).expect("recompute ancestry root")
        );
        assert_eq!(
            topology.edge_run.header.domain.source_root,
            topology.edge_table_root
        );
        assert_eq!(
            topology.node_run.header.domain.source_root,
            topology.edge_table_root
        );
        assert!(topology
            .ancestry_links
            .iter()
            .any(|link| link.parent_content_root == topology.incidence_content_root));
        for links in topology
            .ancestry_links
            .chunk_by(|left, right| left.child_id == right.child_id)
        {
            let mut digest = Sha256::new();
            digest.update(PARENT_ROOT_DOMAIN);
            digest.update((links.len() as u64).to_le_bytes());
            for link in links {
                digest.update((link.parent_id.kind as u16).to_le_bytes());
                digest.update(link.parent_virtual_partition.to_le_bytes());
                digest.update(link.parent_id.generation.to_le_bytes());
                digest.update(link.parent_id.ordinal.to_le_bytes());
                digest.update(link.parent_content_root);
            }
            let recomputed: [u8; 32] = digest.finalize().into();
            assert_eq!(recomputed, links[0].child_parent_set_root);
        }
        let mut streamed_edges = 0_u64;
        let mut streamed_support = 0_u64;
        topology
            .visit_edges(|edge| {
                streamed_edges += 1;
                streamed_support += edge.support;
                Ok(())
            })
            .expect("authenticate edge stream");
        assert_eq!(streamed_edges, source.distinct_kmers);
        assert_eq!(streamed_support, source.support_events);

        let mut independent_node_root = Sha256::new();
        independent_node_root.update(NODE_TABLE_ROOT_DOMAIN);
        independent_node_root.update(topology.source_root);
        independent_node_root.update(topology.scientific_config_root);
        independent_node_root.update(topology.edge_table_root);
        independent_node_root.update([topology.k, support_unit_tag(topology.support_unit)]);
        for (&node, oracle) in &expected {
            let spelling = decode_mer(node, source.k - 1).expect("decode rooted oracle node");
            let reverse_fixed = spelling == reverse_ascii(&spelling);
            let hard_boundary = oracle.incoming.count_ones() != 1
                || oracle.outgoing.count_ones() != 1
                || reverse_fixed
                || oracle.incident_fixed;
            independent_node_root.update(node.to_be_bytes());
            independent_node_root.update([oracle.incoming, oracle.outgoing]);
            independent_node_root.update([
                u8::from(reverse_fixed),
                u8::from(oracle.incident_fixed),
                u8::from(hard_boundary),
            ]);
        }
        independent_node_root.update((expected.len() as u64).to_le_bytes());
        independent_node_root.update(topology.stats.incidences.to_le_bytes());
        let expected_node_root: [u8; 32] = independent_node_root.finalize().into();
        assert_eq!(topology.node_table_root, expected_node_root);

        let mut independent_symmetry_root = Sha256::new();
        independent_symmetry_root.update(NODE_SYMMETRY_ROOT_DOMAIN);
        independent_symmetry_root.update(topology.source_root);
        independent_symmetry_root.update(topology.scientific_config_root);
        independent_symmetry_root.update(topology.edge_table_root);
        independent_symmetry_root.update([topology.k, support_unit_tag(topology.support_unit)]);
        independent_symmetry_root.update(expected_node_root);
        independent_symmetry_root.update((expected.len() as u64).to_le_bytes());
        independent_symmetry_root.update(topology.stats.incidences.to_le_bytes());
        let expected_symmetry_root: [u8; 32] = independent_symmetry_root.finalize().into();
        assert_eq!(topology.node_symmetry_root, expected_symmetry_root);
        topology.cleanup().expect("clean topology");
    }

    #[test]
    fn skew_and_spill_plan_do_not_change_scientific_outputs() {
        let source = input(4, 2, &[("AAAA", 2), ("AAAC", 3), ("AAAG", 5), ("AAAT", 7)]);
        let small_directory = tempfile::tempdir().expect("create small-plan directory");
        let mut small = options(small_directory.path(), 4);
        small.limits.sort_buffer_bytes =
            size_of::<ExternalIncidenceRecord>().max(size_of::<ExternalNodeStateRecord>()) as u64;
        small.limits.block_payload_bytes = 48;
        small.limits.merge_fan_in = 2;
        small.limits.max_open_files = 3;
        let small_topology = build_external_cdbg_from_unverified_materialized(
            &small,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect("build small plan");

        let large_directory = tempfile::tempdir().expect("create large-plan directory");
        let mut large = options(large_directory.path(), 4);
        large.limits.sort_buffer_bytes = 4096;
        large.limits.block_payload_bytes = 512;
        large.limits.merge_fan_in = 4;
        large.limits.max_open_files = 5;
        let large_topology = build_external_cdbg_from_unverified_materialized(
            &large,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect("build large plan");

        assert_eq!(
            small_topology.edge_table_root,
            large_topology.edge_table_root
        );
        assert_eq!(
            small_topology.node_table_root,
            large_topology.node_table_root
        );
        assert_eq!(
            small_topology.node_symmetry_root,
            large_topology.node_symmetry_root
        );
        assert_eq!(
            small_topology
                .materialize_node_states(100)
                .expect("materialize small plan"),
            large_topology
                .materialize_node_states(100)
                .expect("materialize large plan")
        );
        assert!(small_topology.stats.run_files_created > large_topology.stats.run_files_created);
        let aaa = encode_exact_bases(b"AAA").expect("encode skew node");
        let skew = small_topology
            .materialize_node_states(100)
            .expect("materialize skew plan")
            .into_iter()
            .find(|node| node.node == aaa)
            .expect("find skew node");
        assert_eq!(skew.outgoing_mask, 0b1111);
        assert!(skew.hard_boundary);
        small_topology.cleanup().expect("clean small plan");
        large_topology.cleanup().expect("clean large plan");
    }

    #[test]
    fn packed_width_boundaries_are_exact_through_k127() {
        for k in [31_u8, 32, 33, 63, 64, 65, 126, 127] {
            let sequence: String = (0..k)
                .map(|position| match position % 4 {
                    0 => 'A',
                    1 => 'C',
                    2 => 'G',
                    _ => 'T',
                })
                .collect();
            let source = input(k, 3, &[(sequence.as_str(), 1)]);
            let directory = tempfile::tempdir().expect("create width test directory");
            let topology = build(&directory, &source);
            let expected_width = if k <= 31 {
                ExternalKeyWidth::W64
            } else if k <= 63 {
                ExternalKeyWidth::W128
            } else {
                ExternalKeyWidth::W256
            };
            assert_eq!(topology.edge_run.header.domain.key_width, expected_width);
            assert_eq!(
                topology.edge_run.header.domain.record_width,
                expected_width.bytes() as u16 + 8
            );
            assert_eq!(topology.stats.retained_edges, 1);
            assert_eq!(
                topology.stats.incidences,
                topology.stats.oriented_handles * 2
            );
            topology.cleanup().expect("clean width topology");
        }
    }

    #[test]
    fn fixed_edges_and_reverse_fixed_nodes_are_hard_boundaries() {
        let fixed_source = input(4, 2, &[("ATAT", 9)]);
        let fixed_directory = tempfile::tempdir().expect("create fixed-edge directory");
        let fixed_topology = build(&fixed_directory, &fixed_source);
        assert_eq!(fixed_topology.stats.fixed_edges, 1);
        assert_eq!(fixed_topology.stats.oriented_handles, 1);
        for node in fixed_topology
            .materialize_node_states(10)
            .expect("materialize fixed-edge nodes")
        {
            assert!(node.incident_fixed_edge);
            assert!(node.hard_boundary);
        }
        fixed_topology.cleanup().expect("clean fixed topology");

        let node_source = input(3, 2, &[("ATA", 4)]);
        let node_directory = tempfile::tempdir().expect("create fixed-node directory");
        let node_topology = build(&node_directory, &node_source);
        for node in node_topology
            .materialize_node_states(10)
            .expect("materialize reverse-fixed nodes")
        {
            assert!(node.reverse_fixed);
            assert!(node.hard_boundary);
        }
        node_topology.cleanup().expect("clean fixed-node topology");
    }

    #[test]
    fn exact_fixed_edge_caps_admit_the_actual_one_handle_and_two_incidences() {
        let source = input(4, 2, &[("ATAT", 9)]);
        let directory = tempfile::tempdir().expect("create exact-cap directory");
        let mut configured = options(directory.path(), 4);
        configured.limits.max_edges = 1;
        configured.limits.max_handles = 1;
        configured.limits.max_incidences = 2;
        configured.limits.max_nodes = 2;
        let topology = build_external_cdbg_from_unverified_materialized(
            &configured,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect("actual fixed-edge counts fit their exact caps");
        assert_eq!(topology.stats.retained_edges, 1);
        assert_eq!(topology.stats.fixed_edges, 1);
        assert_eq!(topology.stats.oriented_handles, 1);
        assert_eq!(topology.stats.incidences, 2);
        assert_eq!(topology.stats.literal_nodes, 2);
        topology.cleanup().expect("clean exact-cap topology");
    }

    #[test]
    fn final_node_audit_rejects_valid_but_reverse_complement_asymmetric_masks() {
        let directory = tempfile::tempdir().expect("create symmetry-audit directory");
        let configured = options(directory.path(), 3);
        let guard = RunDirectoryGuard::create(directory.path(), configured.limits.max_run_files)
            .expect("create private run dir");
        let edge_root = [0x5d; 32];
        let mut state = BuildState {
            options: &configured,
            run_dir: &guard.path,
            retained_source_root: edge_root,
            next_run_ordinal: 0,
            run_files_created: 0,
            reclaimed_files: 0,
            open_files_high_water: 0,
            temp: TempLedger {
                live: 0,
                high_water: 0,
                limit: configured.limits.max_temp_bytes,
            },
            fault: FaultInjector::disabled(),
            ancestry: Vec::new(),
        };
        let mut records = Vec::new();
        for (spelling, incoming_mask, outgoing_mask) in
            [(b"AA".as_slice(), 0b0001, 0b0010), (b"TT", 0b0100, 0b0100)]
        {
            let node = encode_exact_bases(spelling).expect("encode asymmetric node");
            let (owner_minimizer, owner_partition) = node_owner(
                node,
                configured.k - 1,
                configured.node_minimizer_length,
                configured.virtual_partition_count,
            )
            .expect("route asymmetric node");
            records.push(ExternalNodeStateRecord {
                node,
                owner_minimizer,
                owner_partition,
                incoming_mask,
                outgoing_mask,
                reverse_fixed: false,
                incident_fixed_edge: false,
                hard_boundary: false,
            });
        }
        records.sort_unstable_by_key(|record| (record.owner_partition, record.node));
        let node_run = state
            .seal_slice::<ExternalNodeStateRecord>(0, &records, &[])
            .expect("seal valid asymmetric node table");
        let error = audit_final_node_table(&mut state, node_run, edge_root, 4)
            .expect_err("reverse-complement mask mismatch must be fatal");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        drop(state);
        drop(guard);
        assert!(run_directories(directory.path()).is_empty());
    }

    fn flip_byte(path: &Path, offset: u64) {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("open mutation target");
        file.seek(SeekFrom::Start(offset))
            .expect("seek mutation target");
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).expect("read mutation byte");
        byte[0] ^= 0x5a;
        file.seek(SeekFrom::Start(offset))
            .expect("reseek mutation target");
        file.write_all(&byte).expect("write mutation byte");
        file.sync_all().expect("sync mutation target");
    }

    #[test]
    fn codec_rejects_header_payload_chain_trailer_eof_and_inode_mutations() {
        let source = input(4, 2, &[("AAAA", 1), ("AAAC", 1), ("AAAG", 1), ("AAAT", 1)]);
        let offsets = [
            13_u64,
            96_u64,
            158_u64,
            (FILE_HEADER_BYTES + BLOCK_HEADER_BYTES) as u64,
            (FILE_HEADER_BYTES + BLOCK_HEADER_BYTES + 16 + BLOCK_DIGEST_BYTES + 24) as u64,
        ];
        for offset in offsets {
            let directory = tempfile::tempdir().expect("create mutation directory");
            let mut configured = options(directory.path(), 4);
            configured.limits.block_payload_bytes = 24;
            let topology = build_external_cdbg_from_unverified_materialized(
                &configured,
                UnverifiedMaterializedEdgeAdapter::new(&source),
            )
            .expect("build mutation topology");
            let path = run_path(&topology.run_dir, topology.edge_run.id).expect("derive edge path");
            flip_byte(&path, offset);
            let error = match BlockReader::<ExternalEdgeRecord>::open_unregistered(
                &path,
                configured.limits.block_payload_bytes as usize,
                configured.virtual_partition_count,
            ) {
                Ok(reader) => reader.finish().expect_err("mutation must fail"),
                Err(error) => error,
            };
            assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        }

        let directory = tempfile::tempdir().expect("create trailer directory");
        let topology = build(&directory, &source);
        let path = run_path(&topology.run_dir, topology.edge_run.id).expect("derive trailer path");
        flip_byte(&path, topology.edge_run.byte_len - 40);
        let reader = BlockReader::<ExternalEdgeRecord>::open_unregistered(
            &path,
            topology.block_payload_bytes,
            topology.virtual_partition_count,
        )
        .expect("open trailer mutation");
        assert_eq!(
            reader
                .finish()
                .expect_err("trailer mutation must fail")
                .code(),
            ErrorCode::IntegrityCountRun
        );

        let directory = tempfile::tempdir().expect("create EOF directory");
        let topology = build(&directory, &source);
        let path = run_path(&topology.run_dir, topology.edge_run.id).expect("derive EOF path");
        OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open EOF target")
            .write_all(&[0])
            .expect("append trailing byte");
        let error = match BlockReader::<ExternalEdgeRecord>::open_unregistered(
            &run_path(&topology.run_dir, topology.edge_run.id).expect("derive EOF path"),
            topology.block_payload_bytes,
            topology.virtual_partition_count,
        ) {
            Ok(_) => panic!("trailing byte must fail before allocation"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);

        let directory = tempfile::tempdir().expect("create inode directory");
        let topology = build(&directory, &source);
        let path = run_path(&topology.run_dir, topology.edge_run.id).expect("derive inode path");
        let bytes = fs::read(&path).expect("read inode target");
        fs::remove_file(&path).expect("remove inode target");
        fs::write(&path, bytes).expect("replace inode target");
        assert_eq!(
            topology
                .visit_edges(|_| Ok(()))
                .expect_err("descriptor replacement must fail")
                .code(),
            ErrorCode::IntegrityCountRun
        );
    }

    #[test]
    fn every_single_byte_mutation_truncation_and_append_of_a_run_is_rejected() {
        let source = input(4, 2, &[("ACGT", 7), ("TGCA", 11)]);
        let directory = tempfile::tempdir().expect("create exhaustive-codec directory");
        let topology = build(&directory, &source);
        let edge_path =
            run_path(&topology.run_dir, topology.edge_run.id).expect("derive exhaustive edge path");
        let original = fs::read(edge_path).expect("read exhaustive codec source");
        let probe_path = topology.run_dir.join("exhaustive-corruption-probe.vte");
        let reject_probe = |path: &Path| match BlockReader::<ExternalEdgeRecord>::open_unregistered(
            path,
            topology.block_payload_bytes,
            topology.virtual_partition_count,
        ) {
            Ok(reader) => reader.finish().is_err(),
            Err(_) => true,
        };

        for offset in 0..original.len() {
            let mut mutated = original.clone();
            mutated[offset] ^= 0x80;
            fs::write(&probe_path, mutated).expect("write single-byte corruption probe");
            assert!(
                reject_probe(&probe_path),
                "accepted byte mutation at {offset}"
            );
        }
        for length in 0..original.len() {
            fs::write(&probe_path, &original[..length]).expect("write truncation probe");
            assert!(reject_probe(&probe_path), "accepted truncation at {length}");
        }
        let mut appended = original;
        appended.push(0);
        fs::write(&probe_path, appended).expect("write append probe");
        assert!(reject_probe(&probe_path), "accepted trailing byte");
        fs::remove_file(probe_path).expect("remove exhaustive corruption probe");
        topology.cleanup().expect("clean exhaustive-codec topology");
    }

    #[test]
    fn empty_stream_uses_the_unique_zero_block_sentinel() {
        let source = input(3, 2, &[]);
        let directory = tempfile::tempdir().expect("create empty directory");
        let topology = build(&directory, &source);
        assert_eq!(topology.stats.retained_edges, 0);
        assert_eq!(topology.stats.oriented_handles, 0);
        assert_eq!(topology.stats.incidences, 0);
        assert_eq!(topology.stats.literal_nodes, 0);
        topology
            .visit_edges(|_| panic!("empty edge stream yielded a row"))
            .expect("verify empty edges");
        topology
            .visit_node_states(|_| panic!("empty node stream yielded a row"))
            .expect("verify empty nodes");
        topology.cleanup().expect("clean empty topology");
    }

    #[test]
    fn limits_faults_and_existing_directories_fail_without_residue_or_clobbering() {
        let source = input(3, 2, &[("AAC", 1), ("ACG", 1)]);

        let directory = tempfile::tempdir().expect("create edge-limit directory");
        let mut configured = options(directory.path(), 3);
        configured.limits.max_edges = 1;
        let error = build_external_cdbg_from_unverified_materialized(
            &configured,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect_err("edge limit must fail");
        assert_eq!(error.code(), ErrorCode::ResourceRetainedKeys);
        assert!(run_directories(directory.path()).is_empty());

        let directory = tempfile::tempdir().expect("create memory-limit directory");
        let mut configured = options(directory.path(), 3);
        configured.limits.max_memory_bytes = 1;
        let error = build_external_cdbg_from_unverified_materialized(
            &configured,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect_err("owned-memory limit must fail");
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
        assert!(run_directories(directory.path()).is_empty());

        let directory = tempfile::tempdir().expect("create temp-limit directory");
        let mut configured = options(directory.path(), 3);
        configured.limits.max_temp_bytes = 1;
        let error = build_external_cdbg_from_unverified_materialized(
            &configured,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect_err("temporary-byte limit must fail");
        assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
        assert!(run_directories(directory.path()).is_empty());

        let directory = tempfile::tempdir().expect("create run-limit directory");
        let mut configured = options(directory.path(), 3);
        configured.limits.max_run_files = 1;
        configured.limits.sort_buffer_bytes =
            size_of::<ExternalIncidenceRecord>().max(size_of::<ExternalNodeStateRecord>()) as u64;
        let error = build_external_cdbg_from_unverified_materialized(
            &configured,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect_err("run-file limit must fail");
        assert_eq!(error.code(), ErrorCode::ResourceRunCount);
        assert!(run_directories(directory.path()).is_empty());

        for fault in [
            FaultInjector {
                point: FaultPoint::AfterVerifiedRun,
                successful_hits_before_failure: 0,
            },
            FaultInjector {
                point: FaultPoint::BeforePredecessorCleanup,
                successful_hits_before_failure: 1,
            },
        ] {
            let directory = tempfile::tempdir().expect("create fault directory");
            let mut configured = options(directory.path(), 3);
            configured.limits.sort_buffer_bytes = size_of::<ExternalIncidenceRecord>()
                .max(size_of::<ExternalNodeStateRecord>())
                as u64;
            let error = build_external_cdbg_from_unverified_materialized_with_fault(
                &configured,
                UnverifiedMaterializedEdgeAdapter::new(&source),
                fault,
            )
            .expect_err("injected fault must fail");
            assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
            assert!(run_directories(directory.path()).is_empty());
        }

        let directory = tempfile::tempdir().expect("create late-corruption directory");
        let mut configured = options(directory.path(), 3);
        configured.limits.sort_buffer_bytes =
            size_of::<ExternalIncidenceRecord>().max(size_of::<ExternalNodeStateRecord>()) as u64;
        let error = build_external_cdbg_from_unverified_materialized_with_fault(
            &configured,
            UnverifiedMaterializedEdgeAdapter::new(&source),
            FaultInjector {
                point: FaultPoint::CorruptParentAfterProvisional,
                successful_hits_before_failure: 0,
            },
        )
        .expect_err("late parent corruption must invalidate all provisional children");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        assert!(run_directories(directory.path()).is_empty());

        let directory = tempfile::tempdir().expect("create collision directory");
        let occupied = directory.path().join(RUN_DIRECTORY_PREFIX);
        fs::create_dir(&occupied).expect("create occupied run directory");
        let marker = occupied.join("owned-by-caller");
        fs::write(&marker, b"keep").expect("write caller marker");
        let topology = build_external_cdbg_from_unverified_materialized(
            &options(directory.path(), 3),
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect("unique private run directory must avoid occupied prefix sibling");
        assert_eq!(fs::read(marker).expect("read caller marker"), b"keep");
        assert_ne!(topology.run_dir, occupied);
        topology
            .cleanup()
            .expect("clean unique private run directory");
    }

    #[test]
    fn private_paths_have_restrictive_permissions_and_unique_ownership() {
        let source = input(3, 2, &[("AAC", 1), ("ACG", 1)]);
        let directory = tempfile::tempdir().expect("create private-path directory");
        let first = build(&directory, &source);
        let second = build(&directory, &source);
        assert_ne!(first.run_dir, second.run_dir);
        assert_eq!(
            fs::symlink_metadata(&first.run_dir)
                .expect("stat private run directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for path in [
            run_path(&first.run_dir, first.edge_run.id).expect("derive private edge path"),
            run_path(&first.run_dir, first.node_run.id).expect("derive private node path"),
        ] {
            assert_eq!(
                fs::symlink_metadata(path)
                    .expect("stat private run file")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        first.cleanup().expect("clean first private topology");
        second.cleanup().expect("clean second private topology");
        assert!(run_directories(directory.path()).is_empty());
    }

    #[test]
    fn cleanup_validates_every_entry_before_deleting_any_file() {
        let directory = tempfile::tempdir().expect("create cleanup-validation directory");
        let guard = RunDirectoryGuard::create(directory.path(), 2)
            .expect("create cleanup-validation run directory");
        let (run_dir, identity) = guard.transfer();
        let valid = create_cleanup_test_file(&run_dir, 1, 0o600);
        let unknown = run_dir.join("not-an-owned-run-file");
        fs::write(&unknown, b"must survive failed validation").expect("write unknown entry");
        fs::set_permissions(&unknown, fs::Permissions::from_mode(0o600))
            .expect("set unknown-entry permissions");

        let error = remove_owned_run_directory(&run_dir, identity, 2)
            .expect_err("an unknown entry must stop cleanup");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        assert!(valid.exists(), "first pass must not delete a valid file");
        assert!(unknown.exists(), "unknown entry must be preserved");

        fs::remove_file(unknown).expect("remove test-owned unknown entry");
        remove_owned_run_directory(&run_dir, identity, 2)
            .expect("remove fully validated cleanup directory");
        assert!(!run_dir.exists());
    }

    #[test]
    fn cleanup_rejects_permissions_and_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("create cleanup-shape directory");
        let outside = directory.path().join("outside-must-survive");
        fs::write(&outside, b"outside").expect("write outside marker");

        let guard = RunDirectoryGuard::create(directory.path(), 2)
            .expect("create cleanup-shape run directory");
        let (run_dir, identity) = guard.transfer();
        let private = create_cleanup_test_file(&run_dir, 1, 0o600);
        let non_private = create_cleanup_test_file(&run_dir, 2, 0o640);
        let error = remove_owned_run_directory(&run_dir, identity, 2)
            .expect_err("non-private mode must stop cleanup");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        assert!(private.exists());
        assert!(non_private.exists());

        fs::remove_file(non_private).expect("remove non-private test entry");
        let link = run_path(
            &run_dir,
            ExternalRunId {
                kind: ExternalRecordKind::NodeState,
                generation: 0,
                ordinal: 2,
            },
        )
        .expect("derive cleanup symlink path");
        symlink(&outside, &link).expect("create cleanup symlink");
        let error = remove_owned_run_directory(&run_dir, identity, 2)
            .expect_err("symlink must stop cleanup");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        assert!(private.exists());
        assert_eq!(fs::read(&outside).expect("read outside marker"), b"outside");

        fs::remove_file(link).expect("remove test symlink");
        remove_owned_run_directory(&run_dir, identity, 2)
            .expect("remove directory after invalid entry is gone");
        assert_eq!(
            fs::read(outside).expect("read preserved outside marker"),
            b"outside"
        );
    }

    #[test]
    fn cleanup_rejects_changed_directory_mode_and_entry_count_overflow() {
        let directory = tempfile::tempdir().expect("create cleanup-limit directory");
        let guard = RunDirectoryGuard::create(directory.path(), 1)
            .expect("create cleanup-limit run directory");
        let (run_dir, identity) = guard.transfer();
        let first = create_cleanup_test_file(&run_dir, 1, 0o600);
        let second = create_cleanup_test_file(&run_dir, 2, 0o600);

        fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o750))
            .expect("change run-directory mode");
        let error = remove_owned_run_directory(&run_dir, identity, 1)
            .expect_err("changed directory mode must stop cleanup");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        assert!(first.exists());
        assert!(second.exists());

        fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o700))
            .expect("restore run-directory mode");
        let error = remove_owned_run_directory(&run_dir, identity, 1)
            .expect_err("entry count above the admitted cap must stop cleanup");
        assert_eq!(error.code(), ErrorCode::ResourceRunCount);
        assert!(first.exists());
        assert!(second.exists());

        remove_owned_run_directory(&run_dir, identity, 2)
            .expect("exact entry cap must admit cleanup");
        assert!(!run_dir.exists());
    }

    #[test]
    fn cleanup_refuses_a_replaced_directory_path() {
        let source = input(3, 2, &[("AAC", 1)]);
        let directory = tempfile::tempdir().expect("create cleanup-race directory");
        let topology = build(&directory, &source);
        let owned_path = topology.run_dir.clone();
        let owned_identity = topology.run_dir_identity;
        let displaced_path = directory.path().join("displaced-owned-run-directory");
        fs::rename(&owned_path, &displaced_path).expect("displace owned run directory");
        fs::create_dir(&owned_path).expect("install replacement directory");
        let marker = owned_path.join("must-survive");
        fs::write(&marker, b"replacement").expect("write replacement marker");

        let error = topology
            .cleanup()
            .expect_err("cleanup must reject a replaced directory path");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        assert_eq!(
            fs::read(&marker).expect("read replacement marker"),
            b"replacement"
        );

        fs::remove_dir_all(&owned_path).expect("remove test replacement directory");
        remove_owned_run_directory(&displaced_path, owned_identity, 64)
            .expect("remove identity-verified displaced directory");
    }

    #[test]
    fn provisional_crash_residue_is_unpublishable_and_does_not_block_a_new_build() {
        let source = input(3, 2, &[("AAC", 1)]);
        let directory = tempfile::tempdir().expect("create crash-residue directory");
        let configured = options(directory.path(), 3);
        let guard = RunDirectoryGuard::create(directory.path(), configured.limits.max_run_files)
            .expect("create provisional dir");
        let edge_root = [0xa7; 32];
        let mut state = BuildState {
            options: &configured,
            run_dir: &guard.path,
            retained_source_root: edge_root,
            next_run_ordinal: 0,
            run_files_created: 0,
            reclaimed_files: 0,
            open_files_high_water: 0,
            temp: TempLedger {
                live: 0,
                high_water: 0,
                limit: configured.limits.max_temp_bytes,
            },
            fault: FaultInjector::disabled(),
            ancestry: Vec::new(),
        };
        let mut pending = state
            .begin_run::<ExternalEdgeRecord>(0, 1, &[])
            .expect("begin provisional edge run");
        pending
            .writer
            .push(ExternalEdgeRecord {
                key: source.edge_counts[0].key,
                support: source.edge_counts[0].support,
            })
            .expect("write provisional edge");
        let provisional = state
            .finish_pending_provisional(pending)
            .expect("finish invalid-header provisional");
        let provisional_path =
            run_path(state.run_dir, provisional.id).expect("derive provisional crash-residue path");
        let error = match BlockReader::<ExternalEdgeRecord>::open_unregistered(
            &provisional_path,
            configured.limits.block_payload_bytes as usize,
            configured.virtual_partition_count,
        ) {
            Ok(_) => panic!("a provisional run must not parse as VTEBLK01"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);
        drop(state);
        let (stale_path, stale_identity) = guard.transfer();

        let topology = build(&directory, &source);
        assert_ne!(topology.run_dir, stale_path);
        assert!(stale_path.exists());
        topology.cleanup().expect("clean post-crash topology");
        assert!(stale_path.exists());
        remove_owned_run_directory(&stale_path, stale_identity, configured.limits.max_run_files)
            .expect("remove simulated owned crash residue");
        assert!(run_directories(directory.path()).is_empty());
    }

    #[test]
    fn capped_materialization_and_node_limit_are_enforced() {
        let source = input(3, 2, &[("AAC", 1), ("ACG", 1)]);
        let directory = tempfile::tempdir().expect("create cap directory");
        let topology = build(&directory, &source);
        let error = topology
            .materialize_node_states(1)
            .expect_err("materialization cap must fail");
        assert_eq!(error.code(), ErrorCode::ResourceRetainedKeys);
        topology.cleanup().expect("clean capped topology");

        let directory = tempfile::tempdir().expect("create node-limit directory");
        let mut configured = options(directory.path(), 3);
        configured.limits.max_nodes = 1;
        let error = build_external_cdbg_from_unverified_materialized(
            &configured,
            UnverifiedMaterializedEdgeAdapter::new(&source),
        )
        .expect_err("node limit must fail");
        assert_eq!(error.code(), ErrorCode::ResourceRetainedKeys);
        assert!(run_directories(directory.path()).is_empty());
    }

    #[test]
    fn fixed_byte_layout_round_trips_without_native_struct_serialization() {
        let source = input(32, 3, &[("ACGTACGTACGTACGTACGTACGTACGTACGT", 13)]);
        let directory = tempfile::tempdir().expect("create layout directory");
        let topology = build(&directory, &source);
        let path = run_path(&topology.run_dir, topology.edge_run.id).expect("derive layout path");
        let bytes = fs::read(path).expect("read layout run");
        assert_eq!(&bytes[0..8], FILE_MAGIC);
        assert_eq!(read_u16(&bytes[8..10]).expect("read schema"), SCHEMA);
        assert_eq!(
            read_u16(&bytes[10..12]).expect("read kind"),
            ExternalRecordKind::Edge as u16
        );
        assert_eq!(bytes[12], ExternalKeyWidth::W128 as u8);
        assert_eq!(bytes[13], 32);
        assert_eq!(&bytes[158..FILE_HEADER_BYTES], &[0; RESERVED_HEADER_BYTES]);
        let mut header_bytes = [0_u8; FILE_HEADER_BYTES];
        header_bytes.copy_from_slice(&bytes[..FILE_HEADER_BYTES]);
        assert_eq!(
            decode_header(&header_bytes, topology.virtual_partition_count)
                .expect("decode typed header"),
            topology.edge_run.header
        );
        let trailer_start = bytes.len() - TRAILER_BYTES;
        assert_eq!(&bytes[trailer_start..trailer_start + 8], TRAILER_MAGIC);
        assert_eq!(
            read_u64(&bytes[trailer_start + 96..]).expect("read trailer length"),
            bytes.len() as u64
        );
        assert_eq!(topology.edge_run.header.domain.record_width, 24);
        assert_eq!(
            topology.node_run.header.domain.record_width,
            (16 + 8 + 8) as u16
        );
        topology.cleanup().expect("clean layout topology");
    }

    #[test]
    fn input_permutation_changes_neither_exact_tables_nor_scientific_roots() {
        let source = input(5, 2, &[("AACGT", 2), ("CCGTA", 3), ("TGCAT", 5)]);
        let mut permuted = source.clone();
        permuted.edge_counts.reverse();
        let first_directory = tempfile::tempdir().expect("create first permutation directory");
        let second_directory = tempfile::tempdir().expect("create second permutation directory");
        let first = build(&first_directory, &source);
        let second = build(&second_directory, &permuted);
        assert_eq!(first.edge_table_root, second.edge_table_root);
        assert_eq!(first.node_table_root, second.node_table_root);
        assert_eq!(
            first
                .materialize_node_states(100)
                .expect("materialize first permutation"),
            second
                .materialize_node_states(100)
                .expect("materialize second permutation")
        );
        first.cleanup().expect("clean first permutation");
        second.cleanup().expect("clean second permutation");
    }

    #[test]
    fn duplicate_terminal_sides_and_support_overflow_are_typed_failures() {
        let handle = encode_exact_bases(b"AAC").expect("encode duplicate-side handle");
        let incidence = ExternalIncidenceRecord {
            node: encode_exact_bases(b"AA").expect("encode duplicate-side node"),
            handle,
            direction: IncidenceDirection::Outgoing,
            fixed_edge: false,
        };
        let mut accumulator = NodeAccumulator::new(incidence.node);
        accumulator.add(incidence, 3).expect("add first side");
        let error = accumulator
            .add(incidence, 3)
            .expect_err("duplicate side must fail before setting a second bit");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);

        let configured = options(Path::new("."), 3);
        let width = ExternalKeyWidth::W64;
        let domain = RunDomain {
            kind: ExternalRecordKind::Edge,
            key_width: width,
            k: 3,
            node_minimizer_length: 2,
            support_unit: configured.support_unit,
            virtual_partition: GLOBAL_PARTITION,
            generation: 0,
            run_ordinal: 0,
            source_root: [0; 32],
            scientific_config_root: [0; 32],
            parent_set_root: [0; 32],
            record_width: 16,
            max_block_payload: 128,
            owner_partition_count: configured.virtual_partition_count,
        };
        let mut invalid_packed = [0_u8; 16];
        invalid_packed[0] = 0x80;
        invalid_packed[8..16].copy_from_slice(&1_u64.to_le_bytes());
        let error = ExternalEdgeRecord::decode(domain, &invalid_packed)
            .expect_err("inactive high bits must be a run-integrity failure");
        assert_eq!(error.code(), ErrorCode::IntegrityCountRun);

        let mut overflowing = input(3, 2, &[("AAC", 1), ("ACG", 1)]);
        overflowing.edge_counts[0].support = u64::MAX;
        overflowing.edge_counts[1].support = 1;
        overflowing.support_events = u64::MAX;
        let directory = tempfile::tempdir().expect("create overflow directory");
        let error = build_external_cdbg_from_unverified_materialized(
            &options(directory.path(), 3),
            UnverifiedMaterializedEdgeAdapter::new(&overflowing),
        )
        .expect_err("support overflow must fail");
        assert_eq!(error.code(), ErrorCode::ResourceIntegerOverflow);
        assert!(run_directories(directory.path()).is_empty());
    }
}
