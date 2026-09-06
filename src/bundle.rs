//! Deterministic artifact rendering and transactional result-directory commit.
//!
//! Stable evidence-bearing bundles can only be minted by crate-internal,
//! validated assembly pipelines. External callers may verify a committed
//! bundle, but cannot construct its evidence model or invoke its writer:
//!
//! ```compile_fail
//! use veritasm::bundle::{write_bundle, BundleData};
//! ```

use crate::audit::{indexed_mapper_memory_budget, MapperDescriptor};
use crate::config::{Limits, ScientificConfig, SupportUnit};
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::graph::{
    bytes_for, conservative_allocation_bytes, ensure_phase_budget, try_vec_with_capacity,
};
use crate::indexed_mapper::{
    IndexedExactMapper, ALGORITHM_ID as INDEXED_MAPPER_ID,
    ALGORITHM_VERSION as INDEXED_MAPPER_VERSION, DEFAULT_SEED_LENGTH,
    TARGET_UNIVERSE as INDEXED_TARGET_UNIVERSE,
};
use crate::model::{
    AvailabilityU64, GraphLink, MateRole, PairEndpoint, PairLaneStateCount, PairLink,
    SourceSummary, StateCount, Topology, TransformRecord, Unitig, WindowStats,
};
use crate::report;
use rustix::fs::{renameat_with, RenameFlags, CWD};
use serde::ser::SerializeSeq;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const ARTIFACT_SCHEMA_VERSION: &str = "1.0";
const UNITIG_SCHEMA_VERSION: &str = "1.1";
const ASSEMBLY_GFA_SCHEMA_VERSION: &str = "1.1";
const BUNDLE_SCHEMA_VERSION: &str = "1.2";
const PAIR_LINK_SCHEMA_VERSION: &str = "1.1";
const DISCLAIMER_VERSION: &str = "veritasm-disclaimer-1";
const DISCLAIMER: &str = "VeritAsm is unvalidated research software. It reconstructs algorithm-defined unitigs and reports internal consistency from the same supplied reads used for construction. It does not detect or identify an organism; establish biological presence or absence, viability, infectivity, sample sterility, or product safety; determine product disposition; or replace a validated or compendial method. Regulatory and standards references describe the surrounding workflow only and do not state or imply compliance, approval, clearance, qualification, validation, or fitness for a regulated purpose.";
const COMPLETE_MESSAGE: &str = "Software run completed under the recorded parameters.";
const EMPTY_MESSAGE: &str =
    "No sequence met the recorded assembly rules; this is not biological absence.";
const SCIENTIFIC_SCOPE: &str = "algorithmic_unitig_reconstruction";
const EVIDENCE_SOURCE: &str = "construction_reads";
const EVIDENCE_INTERPRETATION: &str = "internal_consistency_not_independent_validation";
const BIOLOGICAL_CALL: &str = "not_performed";
const TAXONOMY: &str = "not_performed";
const CONTROL_CONTEXT: &str = "not_supplied";
const LIMITATIONS: [&str; 4] = [
    "Closed graph walks are not audited by construction-read remapping.",
    "Construction-read remapping is internal consistency, not independent validation.",
    "Pairs are reported as observations and never create joins.",
    "Results do not establish biological identity, presence, absence, viability, infectivity, safety, or product disposition.",
];

const UNITIG_HEADER: &str = "schema_version\tunitig_id\tlength_bases\tk\ttopology\tedge_steps\tcanonical_kmers\tsupport_unit\tretention_min_support\tminimum_represented_key_support\tlower_median_represented_key_support\tmaximum_represented_key_support\tenumeration_complete_read_placements\tsingle_group_read_instances\tmulti_group_read_instances_with_group\tplacement_enumeration_status\tsequence_sha256\n";
const PAIR_LINK_HEADER: &str = "schema_version\tk\tlane_ordinal\tsegment_a\tend_a\tstrand_a\tend_distance_a\tmate_role_a\tsegment_b\tend_b\tstrand_b\tend_distance_b\tmate_role_b\tsupplied_fragment_instances\tstatus\n";
const PAIR_SUMMARY_HEADER: &str = "schema_version\tk\tstate\tsupplied_fragment_instances\n";
const TRANSFORM_HEADER: &str = "schema_version\tstage_order\talgorithm_id\talgorithm_version\tsupport_unit\tparameters_json\tinput_distinct_canonical_keys\toutput_distinct_canonical_keys\tinput_support_mass\toutput_support_mass\tremoved_key_count\tremoved_support_mass\tdecision_set_sha256\tpre_state_sha256\tpost_state_sha256\tstatus\n";

const SCIENTIFIC_PATHS: [&str; 6] = [
    "assembly.gfa",
    "pair_audit_summary.tsv",
    "pair_links.tsv",
    "transform_summary.tsv",
    "unitig_evidence.tsv",
    "unitigs.fasta",
];

const SCHEMAS: [(&str, &[u8]); 7] = [
    (
        "schema/assembly_gfa.schema.json",
        include_bytes!("../schema/assembly_gfa.schema.json"),
    ),
    (
        "schema/manifest.json",
        include_bytes!("../schema/manifest.json"),
    ),
    (
        "schema/pair_audit_summary.schema.json",
        include_bytes!("../schema/pair_audit_summary.schema.json"),
    ),
    (
        "schema/pair_links.schema.json",
        include_bytes!("../schema/pair_links.schema.json"),
    ),
    (
        "schema/run.schema.json",
        include_bytes!("../schema/run.schema.json"),
    ),
    (
        "schema/transform_summary.schema.json",
        include_bytes!("../schema/transform_summary.schema.json"),
    ),
    (
        "schema/unitig_evidence.schema.json",
        include_bytes!("../schema/unitig_evidence.schema.json"),
    ),
];

/// Global input arity for a stable bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BundleInputMode {
    SingleEnd,
    PairedEnd,
}

impl BundleInputMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SingleEnd => "single_end",
            Self::PairedEnd => "paired_end",
        }
    }
}

/// One exact support-histogram bin, shared with the exact counter so the
/// pipeline can transfer ownership without duplicating an unbounded vector.
pub type SupportHistogramBin = crate::count::HistogramBin;

/// Complete typed input needed to render a stable result bundle.
#[derive(Debug, Clone)]
pub(crate) struct BundleData {
    pub input_mode: BundleInputMode,
    pub sources: Vec<SourceSummary>,
    pub supplied_fragment_instances: u64,
    pub supplied_read_instances: u64,
    pub supplied_bases: u64,
    pub raw_transport_bytes: u64,
    pub decoded_input_bytes: u64,
    pub inferred_mate_roles: u64,
    pub gzip_sources: u64,
    pub gzip_members: u64,
    pub spool_sha256: String,
    pub spool_bytes: u64,
    pub scientific: ScientificConfig,
    pub limits: Limits,
    pub windows: WindowStats,
    pub observed_distinct_canonical_keys: u64,
    pub observed_support_mass: u64,
    pub retained_distinct_canonical_keys: u64,
    pub retained_support_mass: u64,
    pub support_histogram: Vec<SupportHistogramBin>,
    pub oriented_handles: u64,
    pub self_reverse_complement_keys: u64,
    pub unitigs: Vec<Unitig>,
    pub graph_links: Vec<GraphLink>,
    pub mapper: MapperDescriptor,
    pub read_state_counts: Vec<StateCount>,
    pub pair_state_counts: Vec<StateCount>,
    pub pair_lane_state_counts: Vec<PairLaneStateCount>,
    pub pair_links: Vec<PairLink>,
    pub transformations: Vec<TransformRecord>,
}

/// Information returned after the no-replace commit linearization point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundleOutcome {
    pub destination: PathBuf,
    pub scientific_artifacts_digest: String,
}

/// Deterministic resource limits for checksum-manifest verification.
///
/// `max_regular_files` includes `manifest.sha256`; `max_directories` excludes
/// the bundle root; `max_path_depth` counts relative path components, so a
/// direct child has depth one; and `max_hashed_artifact_bytes` excludes the
/// separately bounded manifest itself. Limits are inclusive.
///
/// The conservative public default admits a 1 MiB manifest, 4,096 regular
/// files, 1,024 directories, 32 path components, and 16 GiB of artifact bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleManifestLimits {
    /// Maximum bytes read from `manifest.sha256`.
    pub max_manifest_bytes: u64,
    /// Maximum regular files, including `manifest.sha256`.
    pub max_regular_files: u64,
    /// Maximum directories below the bundle root.
    pub max_directories: u64,
    /// Maximum relative path components for any entry.
    pub max_path_depth: u32,
    /// Maximum aggregate bytes hashed across listed artifacts.
    pub max_hashed_artifact_bytes: u64,
}

impl Default for BundleManifestLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 1 << 20,
            max_regular_files: 4_096,
            max_directories: 1_024,
            max_path_depth: 32,
            max_hashed_artifact_bytes: 16 << 30,
        }
    }
}

impl BundleManifestLimits {
    fn validate(self) -> Result<Self> {
        if self.max_manifest_bytes == 0 || self.max_regular_files == 0 || self.max_path_depth == 0 {
            return Err(error(
                ErrorCode::ConfigurationInvalidLimit,
                "bundle manifest limits require positive manifest bytes, regular files, and path depth",
            ));
        }
        Ok(self)
    }
}

/// Render, verify, and atomically commit a new result directory.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_bundle(destination: &Path, data: &BundleData) -> Result<BundleOutcome> {
    let lease = RunLease::acquire(destination)?;
    write_bundle_with_lease_inner(lease, data, None)
}

/// An exact cooperative lease for one normalized, not-yet-existing destination.
///
/// Assembly pipelines acquire this before opening input or creating work files,
/// then transfer it to [`write_bundle_with_lease`]. The held file object, not a
/// PID claim, establishes cooperative ownership until commit or failure.
#[derive(Debug)]
#[must_use = "dropping the run lease releases the destination for another writer"]
pub(crate) struct RunLease {
    destination: PathBuf,
    _lock: LockGuard,
}

impl RunLease {
    /// Normalize and reserve a new destination without consuming input work.
    pub(crate) fn acquire(destination: &Path) -> Result<Self> {
        let destination = normalized_destination(destination)?;
        reject_existing(&destination)?;
        let lock = LockGuard::acquire(&destination)?;
        // A noncooperating creator can race the first check. Fail before work
        // while the exact owned lock still protects cooperative writers.
        reject_existing(&destination)?;
        Ok(Self {
            destination,
            _lock: lock,
        })
    }

    /// Canonical same-parent destination bound to this lease.
    pub(crate) fn destination(&self) -> &Path {
        &self.destination
    }
}

/// Render, verify, and commit using a lease acquired before upstream work.
pub(crate) fn write_bundle_with_lease(lease: RunLease, data: &BundleData) -> Result<BundleOutcome> {
    write_bundle_with_lease_inner(lease, data, None)
}

/// Verify the checksum manifest and exact regular-file inventory of a committed bundle.
///
/// This is an integrity check, not a biological validation. On supported Unix
/// platforms, traversal is anchored to one opened bundle-directory descriptor;
/// each entry is opened without following its final symbolic link and with
/// nonblocking semantics, classified with `fstat`, and hashed through that exact
/// descriptor. It rejects malformed, unsorted, duplicated, missing, modified,
/// unlisted, unsafe, symbolic-link, and non-regular entries. Typed artifact/schema
/// conformance is already checked before commit and is not reconstructed by this
/// manifest-only operation.
///
/// A mutable namespace cannot be frozen by a read-only verifier. Verification
/// therefore requires a quiescent, trusted directory or an immutable filesystem
/// snapshot. Metadata-change checks reject ordinary concurrent mutation during a
/// read, but the result does not promise that pathnames remain unchanged after the
/// function returns.
pub fn verify_bundle_manifest(root: &Path) -> Result<()> {
    verify_manifest(root)
}

/// Verify a committed bundle under caller-supplied deterministic resource limits.
///
/// This has the same integrity scope and mutable-namespace limitation as
/// [`verify_bundle_manifest`]. A caller verifying an intentionally larger bundle
/// may raise individual limits while retaining explicit bounds. All verifier-owned
/// collections grow fallibly, so raised limits remain resource admissions rather
/// than caller-controlled infallible allocation requests.
pub fn verify_bundle_manifest_with_limits(root: &Path, limits: BundleManifestLimits) -> Result<()> {
    verify_manifest_with_limits(root, limits.validate()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailPoint {
    AfterLock,
    AfterStagingDirectory,
    AfterSchemaDirectory,
    AfterFasta,
    AfterGfa,
    AfterUnitigEvidence,
    AfterPairLinks,
    AfterPairSummary,
    AfterTransforms,
    AfterRun,
    AfterReport,
    AfterSchema(u8),
    AfterArtifactValidation,
    AfterManifest,
    AfterManifestVerification,
    AfterSchemaDirectorySync,
    AfterStagingDirectorySync,
    BeforeCommit,
    DestinationAppearsBeforeCommit,
}

fn write_bundle_with_lease_inner(
    lease: RunLease,
    data: &BundleData,
    fail_point: Option<FailPoint>,
) -> Result<BundleOutcome> {
    let prepared = Prepared::new(data)?;
    let destination = lease.destination().to_path_buf();
    reject_existing(&destination)?;
    injected(fail_point, FailPoint::AfterLock)?;

    let parent = destination.parent().ok_or_else(|| {
        error(
            ErrorCode::DestinationUnsafePath,
            "destination has no parent",
        )
    })?;
    let staging = tempfile::Builder::new()
        .prefix(".veritasm-stage-")
        .tempdir_in(parent)
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "create staging directory", cause))?;
    fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "set staging permissions", cause))?;
    injected(fail_point, FailPoint::AfterStagingDirectory)?;
    let schema_dir = staging.path().join("schema");
    fs::create_dir(&schema_dir)
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "create schema directory", cause))?;
    fs::set_permissions(&schema_dir, fs::Permissions::from_mode(0o700))
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "set schema permissions", cause))?;
    injected(fail_point, FailPoint::AfterSchemaDirectory)?;

    let mut stage = Stage::new(
        staging.path(),
        data.limits.max_staged_output_bytes,
        data.limits.max_temp_bytes,
        // The pipeline drops its spool and count files before entering the
        // bundle transaction. `spool_bytes` is retained provenance, not live
        // temporary storage at this phase boundary.
        0,
    )?;
    stage.generate("unitigs.fasta", |writer| prepared.write_fasta(writer))?;
    injected(fail_point, FailPoint::AfterFasta)?;
    stage.generate("assembly.gfa", |writer| prepared.write_gfa(writer))?;
    injected(fail_point, FailPoint::AfterGfa)?;
    stage.generate("unitig_evidence.tsv", |writer| {
        prepared.write_unitig_evidence(writer)
    })?;
    injected(fail_point, FailPoint::AfterUnitigEvidence)?;
    stage.generate("pair_links.tsv", |writer| prepared.write_pair_links(writer))?;
    injected(fail_point, FailPoint::AfterPairLinks)?;
    stage.generate("pair_audit_summary.tsv", |writer| {
        prepared.write_pair_summary(writer)
    })?;
    injected(fail_point, FailPoint::AfterPairSummary)?;
    stage.generate("transform_summary.tsv", |writer| {
        prepared.write_transforms(writer)
    })?;
    injected(fail_point, FailPoint::AfterTransforms)?;

    let scientific_digest = scientific_digest(&stage.digests)?;
    let run = prepared.run_document(&scientific_digest);
    stage.generate("run.json", |writer| {
        serde_json::to_writer_pretty(&mut *writer, &run).map_err(json_to_io)?;
        writer.write_all(b"\n")
    })?;
    injected(fail_point, FailPoint::AfterRun)?;

    stage.generate("report.html", |writer| {
        report::write_html(writer, &prepared.report_data())
    })?;
    injected(fail_point, FailPoint::AfterReport)?;
    for (index, (path, bytes)) in SCHEMAS.into_iter().enumerate() {
        serde_json::from_slice::<Value>(bytes).map_err(|cause| {
            error(
                ErrorCode::IntegritySchema,
                format!("embedded {path} is invalid JSON: {cause}"),
            )
        })?;
        stage.bytes(path, bytes)?;
        injected(
            fail_point,
            FailPoint::AfterSchema(
                u8::try_from(index).map_err(|_| invariant("schema index exceeds u8"))?,
            ),
        )?;
    }

    validate_artifacts(staging.path(), &prepared, &scientific_digest)?;
    injected(fail_point, FailPoint::AfterArtifactValidation)?;
    let manifest = render_manifest(&stage.digests);
    let manifest_bytes = u64::try_from(manifest.len())
        .map_err(|_| overflow("bundle manifest length does not fit u64"))?;
    stage.bytes("manifest.sha256", manifest.as_bytes())?;
    injected(fail_point, FailPoint::AfterManifest)?;
    let regular_files = u64::try_from(stage.digests.len())
        .map_err(|_| overflow("bundle file count does not fit u64"))?;
    let max_path_depth = stage
        .digests
        .keys()
        .map(|path| Path::new(path).components().count())
        .max()
        .and_then(|depth| u32::try_from(depth).ok())
        .ok_or_else(|| overflow("bundle path depth does not fit u32"))?;
    let hashed_artifact_bytes = stage
        .used
        .checked_sub(manifest_bytes)
        .ok_or_else(|| invariant("manifest exceeds staged output byte count"))?;
    let maximum_generated_directories = regular_files
        .checked_mul(u64::from(max_path_depth))
        .ok_or_else(|| overflow("bundle directory-count bound overflow"))?;
    verify_manifest_with_limits(
        staging.path(),
        BundleManifestLimits {
            max_manifest_bytes: manifest_bytes,
            max_regular_files: regular_files,
            // One file can have at most `max_path_depth` parent components.
            // Multiplying by the file count is a conservative bound even when
            // no generated paths share a directory prefix.
            max_directories: maximum_generated_directories,
            max_path_depth,
            max_hashed_artifact_bytes: hashed_artifact_bytes,
        },
    )?;
    injected(fail_point, FailPoint::AfterManifestVerification)?;
    sync_directory(&schema_dir)?;
    injected(fail_point, FailPoint::AfterSchemaDirectorySync)?;
    sync_directory(staging.path())?;
    injected(fail_point, FailPoint::AfterStagingDirectorySync)?;
    injected(fail_point, FailPoint::BeforeCommit)?;
    if fail_point == Some(FailPoint::DestinationAppearsBeforeCommit) {
        fs::create_dir(&destination).map_err(|cause| {
            io_error(
                ErrorCode::CommitWrite,
                "inject noncooperating destination creation",
                cause,
            )
        })?;
        fs::write(destination.join("noncooperating-sentinel"), b"preserve").map_err(|cause| {
            io_error(
                ErrorCode::CommitWrite,
                "inject noncooperating destination sentinel",
                cause,
            )
        })?;
    }

    let outcome = BundleOutcome {
        destination: destination.clone(),
        scientific_artifacts_digest: scientific_digest,
    };
    match renameat_with(
        CWD,
        staging.path(),
        CWD,
        &destination,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {}
        Err(cause) if cause == rustix::io::Errno::EXIST => {
            return Err(error(
                ErrorCode::DestinationExisting,
                "destination appeared before no-replace commit",
            ));
        }
        Err(cause)
            if cause == rustix::io::Errno::NOSYS
                || cause == rustix::io::Errno::NOTSUP
                || cause == rustix::io::Errno::OPNOTSUPP =>
        {
            return Err(error(
                ErrorCode::DestinationNoReplaceUnsupported,
                format!("filesystem does not support no-replace directory rename: {cause}"),
            ));
        }
        Err(cause) => {
            return Err(error(
                ErrorCode::CommitRenameNoReplace,
                format!("no-replace bundle commit failed: {cause}"),
            ));
        }
    }

    // The successful rename above is the last operation allowed to affect success.
    let _ = File::open(parent).and_then(|directory| directory.sync_all());
    drop(lease);
    Ok(outcome)
}

#[cfg(test)]
fn write_bundle_inner(
    destination: &Path,
    data: &BundleData,
    fail_point: Option<FailPoint>,
) -> Result<BundleOutcome> {
    let lease = RunLease::acquire(destination)?;
    write_bundle_with_lease_inner(lease, data, fail_point)
}

fn injected(selected: Option<FailPoint>, here: FailPoint) -> Result<()> {
    if selected == Some(here) {
        Err(error(
            ErrorCode::CommitValidate,
            format!("injected pre-commit failure at {here:?}"),
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug)]
struct LockGuard {
    path: PathBuf,
    file: File,
}

impl LockGuard {
    fn acquire(destination: &Path) -> Result<Self> {
        Self::acquire_inner(destination, None)
    }

    fn acquire_inner(destination: &Path, fail_point: Option<LockFailPoint>) -> Result<Self> {
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                error(
                    ErrorCode::DestinationUnsafePath,
                    "destination name is not UTF-8",
                )
            })?;
        let path = destination.with_file_name(format!(".{name}.veritasm.lock"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|cause| {
                if cause.kind() == io::ErrorKind::AlreadyExists {
                    error(
                        ErrorCode::DestinationLocked,
                        "bundle lock already exists; VeritAsm will not remove a possibly live or stale lock automatically",
                    )
                } else {
                    io_error(ErrorCode::CommitWrite, "acquire bundle lock", cause)
                }
            })?;

        // Establish ownership immediately after `create_new`.  Every later
        // initialization failure therefore runs `Drop` instead of stranding a
        // lock that no process owns.
        let mut guard = Self { path, file };
        injected_lock(fail_point, LockFailPoint::Create, ErrorCode::CommitWrite)?;
        writeln!(
            guard.file,
            "schema=veritasm-lock-v1\npid={}\ndestination={name}",
            std::process::id()
        )
        .map_err(|cause| io_error(ErrorCode::CommitWrite, "write bundle lock", cause))?;
        injected_lock(fail_point, LockFailPoint::Write, ErrorCode::CommitFlush)?;
        guard
            .file
            .flush()
            .map_err(|cause| io_error(ErrorCode::CommitFlush, "flush bundle lock", cause))?;
        injected_lock(fail_point, LockFailPoint::Flush, ErrorCode::CommitSync)?;
        guard
            .file
            .sync_all()
            .map_err(|cause| io_error(ErrorCode::CommitSync, "sync bundle lock", cause))?;
        injected_lock(fail_point, LockFailPoint::Sync, ErrorCode::CommitSync)?;
        Ok(guard)
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // A pathname can be unlinked and replaced while this guard remains
        // alive.  Request cleanup only when it still names the exact file
        // object created and held by this guard.  Failure to prove ownership
        // fails closed and may leave a lock for manual review.  The trusted-
        // parent assumption remains necessary because stat-plus-unlink is not
        // one atomic operation against a malicious namespace writer.
        let Ok(owned) = self.file.metadata() else {
            return;
        };
        let Ok(current) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if current.file_type().is_file()
            && owned.dev() == current.dev()
            && owned.ino() == current.ino()
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockFailPoint {
    Create,
    Write,
    Flush,
    Sync,
}

fn injected_lock(
    selected: Option<LockFailPoint>,
    here: LockFailPoint,
    code: ErrorCode,
) -> Result<()> {
    if selected == Some(here) {
        Err(error(
            code,
            format!("injected lock-initialization failure at {here:?}"),
        ))
    } else {
        Ok(())
    }
}

struct Stage<'a> {
    root: &'a Path,
    maximum_staged_output: u64,
    maximum_temporary: u64,
    baseline_temporary: u64,
    used: u64,
    digests: BTreeMap<String, String>,
}

impl<'a> Stage<'a> {
    fn new(
        root: &'a Path,
        maximum_staged_output: u64,
        maximum_temporary: u64,
        baseline_temporary: u64,
    ) -> Result<Self> {
        if baseline_temporary > maximum_temporary {
            return Err(error(
                ErrorCode::ResourceTemporaryBytes,
                "completed spool already exceeds the aggregate temporary-byte limit",
            ));
        }
        Ok(Self {
            root,
            maximum_staged_output,
            maximum_temporary,
            baseline_temporary,
            used: 0,
            digests: BTreeMap::new(),
        })
    }

    fn bytes(&mut self, relative: &str, bytes: &[u8]) -> Result<()> {
        self.generate(relative, |writer| writer.write_all(bytes))
    }

    fn generate<F>(&mut self, relative: &str, generate: F) -> Result<()>
    where
        F: FnOnce(&mut dyn Write) -> io::Result<()>,
    {
        validate_relative_path(relative)?;
        if self.digests.contains_key(relative) {
            return Err(error(
                ErrorCode::InternalInvariant,
                format!("artifact generated twice: {relative}"),
            ));
        }
        let path = self.root.join(relative);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|cause| io_error(ErrorCode::CommitWrite, relative, cause))?;
        let mut writer = LimitedWriter {
            file,
            used: &mut self.used,
            maximum_staged_output: self.maximum_staged_output,
            maximum_temporary: self.maximum_temporary,
            baseline_temporary: self.baseline_temporary,
            hasher: Sha256::new(),
            length: 0,
        };
        generate(&mut writer).map_err(|cause| {
            let code = if cause.to_string() == "maximum staged output bytes exceeded" {
                ErrorCode::ResourceOutputBytes
            } else if cause.to_string() == "maximum aggregate temporary bytes exceeded" {
                ErrorCode::ResourceTemporaryBytes
            } else if cause.to_string().contains("overflow") {
                ErrorCode::ResourceIntegerOverflow
            } else {
                ErrorCode::CommitWrite
            };
            io_error(code, relative, cause)
        })?;
        writer
            .flush()
            .map_err(|cause| io_error(ErrorCode::CommitFlush, relative, cause))?;
        writer
            .file
            .sync_all()
            .map_err(|cause| io_error(ErrorCode::CommitSync, relative, cause))?;
        let expected_length = writer.length;
        let expected_digest = hex_lower(&writer.hasher.finalize());
        drop(writer.file);
        let (actual_length, actual_digest) = digest_file(&path)?;
        if actual_length != expected_length || actual_digest != expected_digest {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("artifact changed after close: {relative}"),
            ));
        }
        self.digests.insert(relative.to_owned(), actual_digest);
        Ok(())
    }
}

struct LimitedWriter<'a> {
    file: File,
    used: &'a mut u64,
    maximum_staged_output: u64,
    maximum_temporary: u64,
    baseline_temporary: u64,
    hasher: Sha256,
    length: u64,
}

impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let byte_count = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("write length does not fit u64"))?;
        let projected = self
            .used
            .checked_add(byte_count)
            .ok_or_else(|| io::Error::other("staged output byte count overflow"))?;
        if projected > self.maximum_staged_output {
            return Err(io::Error::other("maximum staged output bytes exceeded"));
        }
        let aggregate = self
            .baseline_temporary
            .checked_add(projected)
            .ok_or_else(|| io::Error::other("aggregate temporary byte count overflow"))?;
        if aggregate > self.maximum_temporary {
            return Err(io::Error::other(
                "maximum aggregate temporary bytes exceeded",
            ));
        }
        let written = self.file.write(bytes)?;
        let written_u64 = u64::try_from(written)
            .map_err(|_| io::Error::other("written length does not fit u64"))?;
        *self.used = self
            .used
            .checked_add(written_u64)
            .ok_or_else(|| io::Error::other("staged output byte count overflow"))?;
        self.length = self
            .length
            .checked_add(written_u64)
            .ok_or_else(|| io::Error::other("artifact length overflow"))?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

struct Prepared<'a> {
    input_mode: BundleInputMode,
    sources: &'a [SourceSummary],
    source_lanes: u64,
    fragments: u64,
    reads: u64,
    bases: u64,
    raw_transport_bytes: u64,
    decoded_input_bytes: u64,
    inferred_mate_roles: u64,
    gzip_sources: u64,
    gzip_members: u64,
    spool_sha256: &'a str,
    spool_bytes: u64,
    scientific: &'a ScientificConfig,
    limits: &'a Limits,
    windows: &'a WindowStats,
    observed_distinct: u64,
    observed_support_mass: u64,
    retained_distinct: u64,
    retained_support_mass: u64,
    histogram: &'a [SupportHistogramBin],
    oriented_handles: u64,
    self_reverse_complement_keys: u64,
    unitigs: &'a [Unitig],
    graph_links: &'a [GraphLink],
    mapper: &'a MapperDescriptor,
    read_states: &'a [StateCount],
    global_enumeration_status: &'static str,
    pair_states: &'a [StateCount],
    pair_lane_states: &'a [PairLaneStateCount],
    pair_mode: &'static str,
    pair_links: &'a [PairLink],
    transforms: &'a [TransformRecord],
}

impl<'a> Prepared<'a> {
    fn new(data: &'a BundleData) -> Result<Self> {
        data.scientific.validate()?;
        data.limits.validate(data.scientific.k)?;
        ensure_output_phase_memory(data)?;
        validate_sha256(&data.spool_sha256, "spool SHA-256")?;
        let lane_count = validate_sources(data)?;
        validate_input_telemetry(data)?;
        validate_windows(&data.windows)?;
        validate_histogram(data)?;
        let unitig_index = validate_unitigs(data)?;
        validate_graph(data, &unitig_index)?;
        validate_mapper(data)?;
        let global_enumeration_status = validate_read_states(data)?;
        validate_unitig_audit(data, &data.unitigs, global_enumeration_status)?;
        let pair_mode = validate_pair_states(data, lane_count)?;
        validate_pair_links(data, &unitig_index, lane_count)?;
        validate_transforms(data)?;

        Ok(Self {
            input_mode: data.input_mode,
            sources: &data.sources,
            source_lanes: lane_count,
            fragments: data.supplied_fragment_instances,
            reads: data.supplied_read_instances,
            bases: data.supplied_bases,
            raw_transport_bytes: data.raw_transport_bytes,
            decoded_input_bytes: data.decoded_input_bytes,
            inferred_mate_roles: data.inferred_mate_roles,
            gzip_sources: data.gzip_sources,
            gzip_members: data.gzip_members,
            spool_sha256: &data.spool_sha256,
            spool_bytes: data.spool_bytes,
            scientific: &data.scientific,
            limits: &data.limits,
            windows: &data.windows,
            observed_distinct: data.observed_distinct_canonical_keys,
            observed_support_mass: data.observed_support_mass,
            retained_distinct: data.retained_distinct_canonical_keys,
            retained_support_mass: data.retained_support_mass,
            histogram: &data.support_histogram,
            oriented_handles: data.oriented_handles,
            self_reverse_complement_keys: data.self_reverse_complement_keys,
            unitigs: &data.unitigs,
            graph_links: &data.graph_links,
            mapper: &data.mapper,
            read_states: &data.read_state_counts,
            global_enumeration_status,
            pair_states: &data.pair_state_counts,
            pair_lane_states: &data.pair_lane_state_counts,
            pair_mode,
            pair_links: &data.pair_links,
            transforms: &data.transformations,
        })
    }

    fn report_data(&self) -> report::ReportData<'_> {
        let linear_unitigs = self
            .unitigs
            .iter()
            .filter(|unitig| unitig.topology == Topology::Linear)
            .count() as u64;
        let closed_graph_walks = self.unitigs.len() as u64 - linear_unitigs;
        report::ReportData {
            status_code: if self.unitigs.is_empty() {
                "software_run_complete_no_unitigs_under_parameters"
            } else {
                "software_run_complete"
            },
            status_message: if self.unitigs.is_empty() {
                EMPTY_MESSAGE
            } else {
                COMPLETE_MESSAGE
            },
            input_mode: self.input_mode.as_str(),
            fragments: self.fragments,
            reads: self.reads,
            bases: self.bases,
            raw_transport_bytes: self.raw_transport_bytes,
            max_raw_transport_bytes: self.limits.max_raw_transport_bytes,
            decoded_input_bytes: self.decoded_input_bytes,
            inferred_mate_roles: self.inferred_mate_roles,
            gzip_sources: self.gzip_sources,
            gzip_members: self.gzip_members,
            max_gzip_members: self.limits.max_gzip_members,
            windows: self.windows,
            k: self.scientific.k,
            profile: self.scientific.profile.as_str(),
            support_unit: self.scientific.support_unit.as_str(),
            retention_min_support: self.scientific.min_support,
            min_base_quality: self.scientific.min_base_quality,
            remap: self.scientific.remap,
            retained_canonical_keys: self.retained_distinct,
            linear_unitigs,
            closed_graph_walks,
            enumeration_status: self.global_enumeration_status,
            pair_mode: self.pair_mode,
            pair_lane_states: self.pair_lane_states,
            mapper_algorithm_id: self.mapper.algorithm_id,
            mapper_algorithm_version: self.mapper.algorithm_version,
            mapper_execution_status: self.mapper.execution_status,
            mapper_seed_length: self.mapper.seed_length,
            mapper_linear_targets: self.mapper.linear_targets,
            mapper_index_postings: self.mapper.index_postings,
            mapper_accounted_index_bytes: self.mapper.accounted_index_bytes,
            mapper_target_universe: self.mapper.target_universe,
            scientific_scope: SCIENTIFIC_SCOPE,
            evidence_source: EVIDENCE_SOURCE,
            evidence_interpretation: EVIDENCE_INTERPRETATION,
            biological_call: BIOLOGICAL_CALL,
            taxonomy: TAXONOMY,
            control_context: CONTROL_CONTEXT,
            unitigs: self.unitigs,
            maximum_rows: self.limits.html_max_unitig_rows,
            limitations: &LIMITATIONS,
            disclaimer: DISCLAIMER,
        }
    }

    fn write_fasta(&self, writer: &mut dyn Write) -> io::Result<()> {
        for unitig in self.unitigs {
            writeln!(
                writer,
                ">{} schema_version={} length_bases={} k={} topology={} edge_steps={} canonical_kmers={} support_unit={} retention_min_support={} minimum_represented_key_support={} lower_median_represented_key_support={} maximum_represented_key_support={} placement_enumeration_status={} sequence_sha256={}",
                unitig.id,
                UNITIG_SCHEMA_VERSION,
                unitig.sequence.len(),
                self.scientific.k,
                unitig.topology.as_str(),
                unitig.edge_steps,
                unitig.canonical_kmers,
                self.scientific.support_unit.as_str(),
                self.scientific.min_support,
                unitig.minimum_support,
                unitig.lower_median_support,
                unitig.maximum_support,
                unitig.placement_enumeration_status,
                unitig.sequence_sha256
            )?;
            for line in unitig.sequence.chunks(80) {
                writer.write_all(line)?;
                writer.write_all(b"\n")?;
            }
        }
        Ok(())
    }

    fn write_gfa(&self, writer: &mut dyn Write) -> io::Result<()> {
        writeln!(
            writer,
            "H\tVN:Z:1.0\tPN:Z:veritasm\tPV:Z:{}\tSC:Z:{}",
            env!("CARGO_PKG_VERSION"),
            ASSEMBLY_GFA_SCHEMA_VERSION
        )?;
        for unitig in self.unitigs {
            writeln!(
                writer,
                "S\t{}\t{}\tTP:Z:{}\tES:i:{}\tCK:i:{}\tSH:H:{}",
                unitig.id,
                String::from_utf8_lossy(&unitig.sequence),
                unitig.topology.as_str(),
                unitig.edge_steps,
                unitig.canonical_kmers,
                unitig.sequence_sha256.to_ascii_uppercase()
            )?;
        }
        let overlap = u16::from(self.scientific.k) - 1;
        for link in self.graph_links {
            writeln!(
                writer,
                "L\t{}\t{}\t{}\t{}\t{}M",
                link.from_segment,
                link.from_orientation,
                link.to_segment,
                link.to_orientation,
                overlap
            )?;
        }
        Ok(())
    }

    fn write_unitig_evidence(&self, writer: &mut dyn Write) -> io::Result<()> {
        writer.write_all(UNITIG_HEADER.as_bytes())?;
        for unitig in self.unitigs {
            writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                UNITIG_SCHEMA_VERSION,
                unitig.id,
                unitig.sequence.len(),
                self.scientific.k,
                unitig.topology.as_str(),
                unitig.edge_steps,
                unitig.canonical_kmers,
                self.scientific.support_unit.as_str(),
                self.scientific.min_support,
                unitig.minimum_support,
                unitig.lower_median_support,
                unitig.maximum_support,
                availability_tsv(&unitig.enumeration_complete_read_placements),
                availability_tsv(&unitig.single_group_read_instances),
                availability_tsv(&unitig.multi_group_read_instances_with_group),
                unitig.placement_enumeration_status,
                unitig.sequence_sha256
            )?;
        }
        Ok(())
    }

    fn write_pair_links(&self, writer: &mut dyn Write) -> io::Result<()> {
        writer.write_all(PAIR_LINK_HEADER.as_bytes())?;
        for link in self.pair_links {
            writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\texact_unique_placement_pair_observation",
                PAIR_LINK_SCHEMA_VERSION,
                self.scientific.k,
                link.lane_ordinal,
                link.a.segment,
                link.a.end,
                link.a.strand,
                link.a.end_distance,
                link.a.mate_role.as_str(),
                link.b.segment,
                link.b.end,
                link.b.strand,
                link.b.end_distance,
                link.b.mate_role.as_str(),
                link.supplied_fragment_instances
            )?;
        }
        Ok(())
    }

    fn write_pair_summary(&self, writer: &mut dyn Write) -> io::Result<()> {
        writer.write_all(PAIR_SUMMARY_HEADER.as_bytes())?;
        for state in self.pair_states {
            writeln!(
                writer,
                "{}\t{}\t{}\t{}",
                ARTIFACT_SCHEMA_VERSION, self.scientific.k, state.state, state.count
            )?;
        }
        Ok(())
    }

    fn write_transforms(&self, writer: &mut dyn Write) -> io::Result<()> {
        writer.write_all(TRANSFORM_HEADER.as_bytes())?;
        for transform in self.transforms {
            writeln!(
                writer,
                "{}\t{}\t{}\t1\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                ARTIFACT_SCHEMA_VERSION,
                transform.stage_order,
                transform.algorithm_id,
                self.scientific.support_unit.as_str(),
                transform.parameters_json,
                transform.input_distinct,
                transform.output_distinct,
                transform.input_support_mass,
                transform.output_support_mass,
                transform.removed_key_count,
                transform.removed_support_mass,
                transform.decision_set_sha256,
                transform.pre_state_sha256,
                transform.post_state_sha256,
                transform.status
            )?;
        }
        Ok(())
    }

    fn run_document<'s>(&'s self, scientific_digest: &'s str) -> RunDocument<'s> {
        let complete_audit = self.global_enumeration_status == "placement_enumeration_complete";
        let candidate_limited = self.global_enumeration_status == "indeterminate_candidate_limit";
        let placement = |selector: fn(&Unitig) -> &AvailabilityU64| {
            if complete_audit {
                U64OrNaDoc::Decimal(
                    self.unitigs
                        .iter()
                        .filter(|unitig| unitig.topology == Topology::Linear)
                        .filter_map(|unitig| match selector(unitig) {
                            AvailabilityU64::Value(value) => Some(*value),
                            AvailabilityU64::NotAvailable(_) => None,
                        })
                        .sum::<u64>()
                        .to_string(),
                )
            } else {
                U64OrNaDoc::NotAvailable(NotAvailableDoc {
                    status: "not_available",
                    reason: if candidate_limited {
                        "candidate_limit"
                    } else {
                        "remap_disabled"
                    },
                })
            }
        };
        let linear = self
            .unitigs
            .iter()
            .filter(|unitig| unitig.topology == Topology::Linear)
            .count() as u64;
        let closed = self.unitigs.len() as u64 - linear;
        let pair_support = self
            .pair_links
            .iter()
            .map(|link| link.supplied_fragment_instances)
            .sum::<u64>();
        RunDocument {
            schema_version: BUNDLE_SCHEMA_VERSION,
            software: SoftwareDoc {
                name: "veritasm",
                version: env!("CARGO_PKG_VERSION"),
                rust_msrv: "1.85",
            },
            status: StatusDoc {
                code: if self.unitigs.is_empty() {
                    "software_run_complete_no_unitigs_under_parameters"
                } else {
                    "software_run_complete"
                },
                message: if self.unitigs.is_empty() {
                    EMPTY_MESSAGE
                } else {
                    COMPLETE_MESSAGE
                },
            },
            input: InputDoc {
                mode: self.input_mode.as_str(),
                lane_count_decimal: self.source_lanes.to_string(),
                fragments_decimal: self.fragments.to_string(),
                reads_decimal: self.reads.to_string(),
                bases_decimal: self.bases.to_string(),
                raw_transport_bytes_decimal: self.raw_transport_bytes.to_string(),
                decoded_input_bytes_decimal: self.decoded_input_bytes.to_string(),
                inferred_mate_roles_decimal: self.inferred_mate_roles.to_string(),
                gzip_sources_decimal: self.gzip_sources.to_string(),
                gzip_members_decimal: self.gzip_members.to_string(),
                sources: SourcesDoc(self.sources),
            },
            spool: SpoolDoc {
                schema_version: "1",
                sha256: self.spool_sha256,
                fragments_decimal: self.fragments.to_string(),
                reads_decimal: self.reads.to_string(),
                bytes_decimal: self.spool_bytes.to_string(),
            },
            parameters: ParametersDoc {
                k: self.scientific.k,
                profile: self.scientific.profile.as_str(),
                support_unit: self.scientific.support_unit.as_str(),
                retention_min_support_decimal: self.scientific.min_support.to_string(),
                min_base_quality: self.scientific.min_base_quality,
                remap: self.scientific.remap,
                limits: LimitsDoc::from(self.limits),
            },
            windows: WindowsDoc::from(self.windows),
            counting: CountingDoc {
                algorithm_id: "exact_external_range_partition",
                algorithm_version: "1",
                path: "no_sieve_exact",
                observed_distinct_canonical_keys_decimal: self.observed_distinct.to_string(),
                observed_support_mass_decimal: self.observed_support_mass.to_string(),
                retained_distinct_canonical_keys_decimal: self.retained_distinct.to_string(),
                retained_support_mass_decimal: self.retained_support_mass.to_string(),
                support_histogram: HistogramDocs(self.histogram),
            },
            graph: GraphDoc {
                retained_canonical_keys_decimal: self.retained_distinct.to_string(),
                retained_support_mass_decimal: self.retained_support_mass.to_string(),
                oriented_handles_decimal: self.oriented_handles.to_string(),
                self_reverse_complement_keys_decimal: self.self_reverse_complement_keys.to_string(),
                unitigs_decimal: (self.unitigs.len() as u64).to_string(),
                linear_unitigs_decimal: linear.to_string(),
                closed_graph_walks_decimal: closed.to_string(),
                graph_links_decimal: (self.graph_links.len() as u64).to_string(),
            },
            read_audit: ReadAuditDoc {
                mapper: MapperDoc {
                    algorithm_id: self.mapper.algorithm_id,
                    algorithm_version: self.mapper.algorithm_version,
                    target_universe: self.mapper.target_universe,
                    seed_length: self.mapper.seed_length,
                    execution_status: self.mapper.execution_status,
                    linear_targets_decimal: self.mapper.linear_targets.to_string(),
                    index_postings_decimal: self.mapper.index_postings.to_string(),
                    accounted_index_bytes_decimal: self.mapper.accounted_index_bytes.to_string(),
                    max_mapping_candidates_decimal: self.limits.max_mapping_candidates.to_string(),
                },
                global_enumeration_status: self.global_enumeration_status,
                state_counts: ReadStateDocs(self.read_states),
                placement_totals: PlacementTotalsDoc {
                    enumeration_complete_read_placements: placement(|unitig| {
                        &unitig.enumeration_complete_read_placements
                    }),
                    single_group_read_instances: placement(|unitig| {
                        &unitig.single_group_read_instances
                    }),
                    multi_group_read_instance_unitig_memberships: placement(|unitig| {
                        &unitig.multi_group_read_instances_with_group
                    }),
                },
            },
            pair_audit: PairAuditDoc {
                mode: self.pair_mode,
                state_counts: PairStateDocs(self.pair_states),
                lane_state_counts: PairLaneStateDocs(self.pair_lane_states),
                link_groups_decimal: (self.pair_links.len() as u64).to_string(),
                link_support_decimal: pair_support.to_string(),
                reconciliation: PairReconciliationDoc {
                    states_sum_to_input_fragments: true,
                    lane_states_sum_to_lane_fragments: true,
                    lane_state_sums_equal_global_states: true,
                    link_support_equals_cross_unitig_state: true,
                    lane_link_support_equals_lane_cross_unitig_state: true,
                },
            },
            transformations: TransformDocs {
                records: self.transforms,
                support_unit: self.scientific.support_unit,
            },
            interpretation: InterpretationDoc {
                scientific_scope: SCIENTIFIC_SCOPE,
                evidence_source: EVIDENCE_SOURCE,
                evidence_interpretation: EVIDENCE_INTERPRETATION,
                support_unit: self.scientific.support_unit.as_str(),
                biological_call: BIOLOGICAL_CALL,
                taxonomy: TAXONOMY,
                control_context: CONTROL_CONTEXT,
                disclaimer: DisclaimerDoc {
                    version: DISCLAIMER_VERSION,
                    text: DISCLAIMER,
                },
            },
            warnings: EmptyArray,
            limitations: &LIMITATIONS,
            scientific_artifacts_digest: scientific_digest,
        }
    }
}

struct UnitigIndex<'a> {
    by_id: Vec<&'a Unitig>,
}

impl<'a> UnitigIndex<'a> {
    fn new(unitigs: &'a [Unitig]) -> Result<Self> {
        let mut by_id = try_vec_with_capacity(unitigs.len(), "allocate bundle unitig-ID index")?;
        by_id.extend(unitigs);
        by_id.sort_unstable_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
        if by_id.windows(2).any(|window| window[0].id == window[1].id) {
            return Err(invariant("duplicate unitig identifier"));
        }
        Ok(Self { by_id })
    }

    fn get(&self, id: &str) -> Option<&'a Unitig> {
        self.by_id
            .binary_search_by(|unitig| unitig.id.as_bytes().cmp(id.as_bytes()))
            .ok()
            .map(|index| self.by_id[index])
    }
}

const OUTPUT_TRANSIENT_PAYLOAD_BYTES: u64 = 256 << 10;
const NESTED_ALLOCATION_ALLOWANCE_BYTES: u64 = 64;
const OUTPUT_TRANSIENT_TOP_LEVEL_ALLOCATIONS: u64 = 16;

fn ensure_output_phase_memory(data: &BundleData) -> Result<()> {
    let mut payload = OUTPUT_TRANSIENT_PAYLOAD_BYTES;
    let mut top_level_allocations = OUTPUT_TRANSIENT_TOP_LEVEL_ALLOCATIONS;
    let mut nested_allocations = 0u64;

    account_top_vec::<SourceSummary>(
        data.sources.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "source table",
    )?;
    for source in &data.sources {
        account_nested_bytes(
            source.raw_transport_sha256.capacity(),
            &mut payload,
            &mut nested_allocations,
            "source transport digest",
        )?;
        account_nested_bytes(
            source.logical_decoded_sha256.capacity(),
            &mut payload,
            &mut nested_allocations,
            "source decoded digest",
        )?;
    }
    account_nested_bytes(
        data.spool_sha256.capacity(),
        &mut payload,
        &mut nested_allocations,
        "spool digest",
    )?;
    account_top_vec::<SupportHistogramBin>(
        data.support_histogram.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "support histogram",
    )?;
    account_top_vec::<Unitig>(
        data.unitigs.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "unitig table",
    )?;
    for unitig in &data.unitigs {
        account_nested_bytes(
            unitig.id.capacity(),
            &mut payload,
            &mut nested_allocations,
            "unitig identifier",
        )?;
        account_nested_bytes(
            unitig.sequence.capacity(),
            &mut payload,
            &mut nested_allocations,
            "unitig sequence",
        )?;
        account_nested_bytes(
            unitig.sequence_sha256.capacity(),
            &mut payload,
            &mut nested_allocations,
            "unitig sequence digest",
        )?;
    }
    account_top_vec::<GraphLink>(
        data.graph_links.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "graph-link table",
    )?;
    for link in &data.graph_links {
        account_nested_bytes(
            link.from_segment.capacity(),
            &mut payload,
            &mut nested_allocations,
            "graph-link source identifier",
        )?;
        account_nested_bytes(
            link.to_segment.capacity(),
            &mut payload,
            &mut nested_allocations,
            "graph-link target identifier",
        )?;
    }
    account_top_vec::<StateCount>(
        data.read_state_counts.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "read-state table",
    )?;
    account_top_vec::<StateCount>(
        data.pair_state_counts.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "pair-state table",
    )?;
    account_top_vec::<PairLaneStateCount>(
        data.pair_lane_state_counts.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "pair-lane-state table",
    )?;
    account_top_vec::<PairLink>(
        data.pair_links.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "pair-link table",
    )?;
    for link in &data.pair_links {
        for endpoint in [&link.a, &link.b] {
            account_nested_bytes(
                endpoint.segment.capacity(),
                &mut payload,
                &mut nested_allocations,
                "pair endpoint identifier",
            )?;
        }
    }
    account_top_vec::<TransformRecord>(
        data.transformations.capacity(),
        &mut payload,
        &mut top_level_allocations,
        "transformation table",
    )?;
    for transform in &data.transformations {
        for (capacity, label) in [
            (
                transform.parameters_json.capacity(),
                "transformation parameters",
            ),
            (
                transform.decision_set_sha256.capacity(),
                "transformation decision digest",
            ),
            (
                transform.pre_state_sha256.capacity(),
                "transformation pre-state digest",
            ),
            (
                transform.post_state_sha256.capacity(),
                "transformation post-state digest",
            ),
        ] {
            account_nested_bytes(capacity, &mut payload, &mut nested_allocations, label)?;
        }
    }

    // Prepared::new owns this pointer-only sorted index while the complete
    // BundleData remains live. Exact renderers and validators stream from it.
    let index_bytes = bytes_for::<&Unitig>(data.unitigs.len(), "bundle unitig-ID index")?;
    payload = payload
        .checked_add(index_bytes)
        .ok_or_else(|| overflow("bundle output payload estimate"))?;
    if !data.unitigs.is_empty() {
        top_level_allocations = top_level_allocations
            .checked_add(1)
            .ok_or_else(|| overflow("bundle output allocation count"))?;
    }
    let nested_allowance = nested_allocations
        .checked_mul(NESTED_ALLOCATION_ALLOWANCE_BYTES)
        .ok_or_else(|| overflow("bundle nested-allocation allowance"))?;
    payload = payload
        .checked_add(nested_allowance)
        .ok_or_else(|| overflow("bundle output payload estimate"))?;
    let required = conservative_allocation_bytes(payload, top_level_allocations)?;
    ensure_phase_budget(
        required,
        data.limits.memory_budget_bytes,
        "bundle preparation and streaming output",
    )
}

fn account_top_vec<T>(
    capacity: usize,
    payload: &mut u64,
    allocations: &mut u64,
    label: &'static str,
) -> Result<()> {
    let bytes = bytes_for::<T>(capacity, label)?;
    *payload = payload
        .checked_add(bytes)
        .ok_or_else(|| overflow("bundle output payload estimate"))?;
    if capacity != 0 {
        *allocations = allocations
            .checked_add(1)
            .ok_or_else(|| overflow("bundle output allocation count"))?;
    }
    Ok(())
}

fn account_nested_bytes(
    capacity: usize,
    payload: &mut u64,
    allocations: &mut u64,
    label: &'static str,
) -> Result<()> {
    let bytes = u64::try_from(capacity).map_err(|_| {
        error(
            ErrorCode::ResourceIntegerOverflow,
            format!("{label} capacity does not fit u64"),
        )
    })?;
    *payload = payload
        .checked_add(bytes)
        .ok_or_else(|| overflow("bundle output payload estimate"))?;
    if capacity != 0 {
        *allocations = allocations
            .checked_add(1)
            .ok_or_else(|| overflow("bundle nested-allocation count"))?;
    }
    Ok(())
}

fn availability_tsv(value: &AvailabilityU64) -> String {
    match value {
        AvailabilityU64::Value(value) => value.to_string(),
        AvailabilityU64::NotAvailable(_) => "NA".to_owned(),
    }
}

#[derive(Serialize)]
struct RunDocument<'a> {
    schema_version: &'static str,
    software: SoftwareDoc,
    status: StatusDoc,
    input: InputDoc<'a>,
    spool: SpoolDoc<'a>,
    parameters: ParametersDoc,
    windows: WindowsDoc,
    counting: CountingDoc<'a>,
    graph: GraphDoc,
    read_audit: ReadAuditDoc<'a>,
    pair_audit: PairAuditDoc<'a>,
    transformations: TransformDocs<'a>,
    interpretation: InterpretationDoc,
    warnings: EmptyArray,
    limitations: &'static [&'static str],
    scientific_artifacts_digest: &'a str,
}

#[derive(Serialize)]
struct SoftwareDoc {
    name: &'static str,
    version: &'static str,
    rust_msrv: &'static str,
}

#[derive(Serialize)]
struct StatusDoc {
    code: &'static str,
    message: &'static str,
}

#[derive(Serialize)]
struct InputDoc<'a> {
    mode: &'static str,
    lane_count_decimal: String,
    fragments_decimal: String,
    reads_decimal: String,
    bases_decimal: String,
    raw_transport_bytes_decimal: String,
    decoded_input_bytes_decimal: String,
    inferred_mate_roles_decimal: String,
    gzip_sources_decimal: String,
    gzip_members_decimal: String,
    sources: SourcesDoc<'a>,
}

struct SourcesDoc<'a>(&'a [SourceSummary]);

impl Serialize for SourcesDoc<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for source in self.0 {
            sequence.serialize_element(&SourceDoc::from(source))?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
struct SourceDoc<'a> {
    lane_ordinal: u32,
    role: &'static str,
    label: String,
    format: &'static str,
    raw_transport_sha256: &'a str,
    logical_decoded_sha256: &'a str,
    records_decimal: String,
    bases_decimal: String,
}

impl<'a> From<&'a SourceSummary> for SourceDoc<'a> {
    fn from(source: &'a SourceSummary) -> Self {
        Self {
            lane_ordinal: source.lane_ordinal,
            role: source.role.as_str(),
            label: source.label(),
            format: source.format.as_str(),
            raw_transport_sha256: &source.raw_transport_sha256,
            logical_decoded_sha256: &source.logical_decoded_sha256,
            records_decimal: source.records.to_string(),
            bases_decimal: source.bases.to_string(),
        }
    }
}

#[derive(Serialize)]
struct SpoolDoc<'a> {
    schema_version: &'static str,
    sha256: &'a str,
    fragments_decimal: String,
    reads_decimal: String,
    bytes_decimal: String,
}

#[derive(Serialize)]
struct ParametersDoc {
    k: u8,
    profile: &'static str,
    support_unit: &'static str,
    retention_min_support_decimal: String,
    min_base_quality: u8,
    remap: bool,
    limits: LimitsDoc,
}

#[derive(Serialize)]
struct LimitsDoc {
    max_header_bytes_decimal: String,
    max_read_bases_decimal: String,
    max_record_bytes_decimal: String,
    max_raw_transport_bytes_decimal: String,
    max_decoded_input_bytes_decimal: String,
    max_gzip_members_decimal: String,
    batch_fragments_decimal: String,
    memory_budget_bytes_decimal: String,
    max_spool_bytes_decimal: String,
    max_temp_bytes_decimal: String,
    partition_prefix_bits: u8,
    sort_buffer_keys_decimal: String,
    merge_fan_in: u8,
    max_count_open_files: u8,
    max_runs_decimal: String,
    max_manifest_bytes_decimal: String,
    max_retained_kmers_decimal: String,
    max_mapping_candidates_decimal: String,
    max_staged_output_bytes_decimal: String,
    html_max_unitig_rows_decimal: String,
}

impl From<&Limits> for LimitsDoc {
    fn from(limits: &Limits) -> Self {
        Self {
            max_header_bytes_decimal: limits.max_header_bytes.to_string(),
            max_read_bases_decimal: limits.max_read_bases.to_string(),
            max_record_bytes_decimal: limits.max_record_bytes.to_string(),
            max_raw_transport_bytes_decimal: limits.max_raw_transport_bytes.to_string(),
            max_decoded_input_bytes_decimal: limits.max_decoded_input_bytes.to_string(),
            max_gzip_members_decimal: limits.max_gzip_members.to_string(),
            batch_fragments_decimal: limits.batch_fragments.to_string(),
            memory_budget_bytes_decimal: limits.memory_budget_bytes.to_string(),
            max_spool_bytes_decimal: limits.max_spool_bytes.to_string(),
            max_temp_bytes_decimal: limits.max_temp_bytes.to_string(),
            partition_prefix_bits: limits.partition_prefix_bits,
            sort_buffer_keys_decimal: limits.sort_buffer_keys.to_string(),
            merge_fan_in: limits.merge_fan_in,
            max_count_open_files: limits.max_count_open_files,
            max_runs_decimal: limits.max_runs.to_string(),
            max_manifest_bytes_decimal: limits.max_manifest_bytes.to_string(),
            max_retained_kmers_decimal: limits.max_retained_kmers.to_string(),
            max_mapping_candidates_decimal: limits.max_mapping_candidates.to_string(),
            max_staged_output_bytes_decimal: limits.max_staged_output_bytes.to_string(),
            html_max_unitig_rows_decimal: limits.html_max_unitig_rows.to_string(),
        }
    }
}

#[derive(Serialize)]
struct WindowsDoc {
    possible_decimal: String,
    accepted_decimal: String,
    ambiguity_only_decimal: String,
    quality_only_decimal: String,
    ambiguity_and_quality_decimal: String,
    fasta_quality_rule: &'static str,
}

impl From<&WindowStats> for WindowsDoc {
    fn from(windows: &WindowStats) -> Self {
        Self {
            possible_decimal: windows.possible.to_string(),
            accepted_decimal: windows.accepted.to_string(),
            ambiguity_only_decimal: windows.ambiguity_only.to_string(),
            quality_only_decimal: windows.quality_only.to_string(),
            ambiguity_and_quality_decimal: windows.ambiguity_and_quality.to_string(),
            fasta_quality_rule: "not_applicable_to_fasta",
        }
    }
}

#[derive(Serialize)]
struct CountingDoc<'a> {
    algorithm_id: &'static str,
    algorithm_version: &'static str,
    path: &'static str,
    observed_distinct_canonical_keys_decimal: String,
    observed_support_mass_decimal: String,
    retained_distinct_canonical_keys_decimal: String,
    retained_support_mass_decimal: String,
    support_histogram: HistogramDocs<'a>,
}

#[derive(Serialize)]
struct HistogramDoc {
    support_decimal: String,
    distinct_canonical_keys_decimal: String,
}

struct HistogramDocs<'a>(&'a [SupportHistogramBin]);

impl Serialize for HistogramDocs<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for bin in self.0 {
            sequence.serialize_element(&HistogramDoc {
                support_decimal: bin.support.to_string(),
                distinct_canonical_keys_decimal: bin.distinct_canonical_keys.to_string(),
            })?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
struct GraphDoc {
    retained_canonical_keys_decimal: String,
    retained_support_mass_decimal: String,
    oriented_handles_decimal: String,
    self_reverse_complement_keys_decimal: String,
    unitigs_decimal: String,
    linear_unitigs_decimal: String,
    closed_graph_walks_decimal: String,
    graph_links_decimal: String,
}

#[derive(Serialize)]
struct ReadAuditDoc<'a> {
    mapper: MapperDoc,
    global_enumeration_status: &'static str,
    state_counts: ReadStateDocs<'a>,
    placement_totals: PlacementTotalsDoc,
}

#[derive(Serialize)]
struct MapperDoc {
    algorithm_id: &'static str,
    algorithm_version: &'static str,
    target_universe: &'static str,
    seed_length: u8,
    execution_status: &'static str,
    linear_targets_decimal: String,
    index_postings_decimal: String,
    accounted_index_bytes_decimal: String,
    max_mapping_candidates_decimal: String,
}

#[derive(Serialize)]
struct ReadStateDoc {
    mate_role: &'static str,
    state: &'static str,
    read_instances_decimal: String,
}

struct ReadStateDocs<'a>(&'a [StateCount]);

impl Serialize for ReadStateDocs<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for state in self.0 {
            sequence.serialize_element(&ReadStateDoc::from(state))?;
        }
        sequence.end()
    }
}

impl From<&StateCount> for ReadStateDoc {
    fn from(state: &StateCount) -> Self {
        Self {
            mate_role: state.role.map_or("INVALID", MateRole::as_str),
            state: state.state,
            read_instances_decimal: state.count.to_string(),
        }
    }
}

#[derive(Serialize)]
struct PlacementTotalsDoc {
    enumeration_complete_read_placements: U64OrNaDoc,
    single_group_read_instances: U64OrNaDoc,
    multi_group_read_instance_unitig_memberships: U64OrNaDoc,
}

#[derive(Serialize)]
#[serde(untagged)]
enum U64OrNaDoc {
    Decimal(String),
    NotAvailable(NotAvailableDoc),
}

#[derive(Serialize)]
struct NotAvailableDoc {
    status: &'static str,
    reason: &'static str,
}

#[derive(Serialize)]
struct PairAuditDoc<'a> {
    mode: &'static str,
    state_counts: PairStateDocs<'a>,
    lane_state_counts: PairLaneStateDocs<'a>,
    link_groups_decimal: String,
    link_support_decimal: String,
    reconciliation: PairReconciliationDoc,
}

#[derive(Serialize)]
struct PairStateDoc {
    state: &'static str,
    supplied_fragment_instances_decimal: String,
}

struct PairStateDocs<'a>(&'a [StateCount]);

impl Serialize for PairStateDocs<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for state in self.0 {
            sequence.serialize_element(&PairStateDoc::from(state))?;
        }
        sequence.end()
    }
}

impl From<&StateCount> for PairStateDoc {
    fn from(state: &StateCount) -> Self {
        Self {
            state: state.state,
            supplied_fragment_instances_decimal: state.count.to_string(),
        }
    }
}

#[derive(Serialize)]
struct PairLaneStateDoc {
    lane_ordinal: u32,
    state: &'static str,
    supplied_fragment_instances_decimal: String,
}

struct PairLaneStateDocs<'a>(&'a [PairLaneStateCount]);

impl Serialize for PairLaneStateDocs<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for state in self.0 {
            sequence.serialize_element(&PairLaneStateDoc {
                lane_ordinal: state.lane_ordinal,
                state: state.state,
                supplied_fragment_instances_decimal: state.count.to_string(),
            })?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
struct PairReconciliationDoc {
    states_sum_to_input_fragments: bool,
    lane_states_sum_to_lane_fragments: bool,
    lane_state_sums_equal_global_states: bool,
    link_support_equals_cross_unitig_state: bool,
    lane_link_support_equals_lane_cross_unitig_state: bool,
}

#[derive(Serialize)]
struct TransformDoc<'a> {
    stage_order: u8,
    algorithm_id: &'static str,
    algorithm_version: &'static str,
    support_unit: &'static str,
    parameters_json: &'a str,
    input_distinct_canonical_keys_decimal: String,
    output_distinct_canonical_keys_decimal: String,
    input_support_mass_decimal: String,
    output_support_mass_decimal: String,
    removed_key_count_decimal: String,
    removed_support_mass_decimal: String,
    decision_set_sha256: &'a str,
    pre_state_sha256: &'a str,
    post_state_sha256: &'a str,
    status: &'static str,
}

impl<'a> TransformDoc<'a> {
    fn new(transform: &'a TransformRecord, support_unit: SupportUnit) -> Self {
        Self {
            stage_order: transform.stage_order,
            algorithm_id: transform.algorithm_id,
            algorithm_version: "1",
            support_unit: support_unit.as_str(),
            parameters_json: &transform.parameters_json,
            input_distinct_canonical_keys_decimal: transform.input_distinct.to_string(),
            output_distinct_canonical_keys_decimal: transform.output_distinct.to_string(),
            input_support_mass_decimal: transform.input_support_mass.to_string(),
            output_support_mass_decimal: transform.output_support_mass.to_string(),
            removed_key_count_decimal: transform.removed_key_count.to_string(),
            removed_support_mass_decimal: transform.removed_support_mass.to_string(),
            decision_set_sha256: &transform.decision_set_sha256,
            pre_state_sha256: &transform.pre_state_sha256,
            post_state_sha256: &transform.post_state_sha256,
            status: transform.status,
        }
    }
}

struct TransformDocs<'a> {
    records: &'a [TransformRecord],
    support_unit: SupportUnit,
}

impl Serialize for TransformDocs<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.records.len()))?;
        for transform in self.records {
            sequence.serialize_element(&TransformDoc::new(transform, self.support_unit))?;
        }
        sequence.end()
    }
}

#[derive(Clone, Copy)]
struct EmptyArray;

impl Serialize for EmptyArray {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_seq(Some(0))?.end()
    }
}

#[derive(Serialize)]
struct InterpretationDoc {
    scientific_scope: &'static str,
    evidence_source: &'static str,
    evidence_interpretation: &'static str,
    support_unit: &'static str,
    biological_call: &'static str,
    taxonomy: &'static str,
    control_context: &'static str,
    disclaimer: DisclaimerDoc,
}

#[derive(Serialize)]
struct DisclaimerDoc {
    version: &'static str,
    text: &'static str,
}

fn validate_sources(data: &BundleData) -> Result<u64> {
    if data.sources.is_empty() {
        return Err(invariant("bundle has no logical input sources"));
    }
    if data.sources.windows(2).any(|window| {
        (window[0].lane_ordinal, window[0].role) >= (window[1].lane_ordinal, window[1].role)
    }) {
        return Err(invariant(
            "logical input sources are not in strict lane/role order",
        ));
    }
    let mut records = 0u64;
    let mut bases = 0u64;
    for source in &data.sources {
        if source.lane_ordinal > 999_999 || source.records == 0 {
            return Err(invariant("source lane or record count violates schema"));
        }
        validate_sha256(&source.raw_transport_sha256, "source transport SHA-256")?;
        validate_sha256(&source.logical_decoded_sha256, "source decoded SHA-256")?;
        records = checked_add(records, source.records, "source record total")?;
        bases = checked_add(bases, source.bases, "source base total")?;
    }
    if records != data.supplied_read_instances || bases != data.supplied_bases {
        return Err(invariant(
            "source totals do not reconcile to supplied reads and bases",
        ));
    }

    let mut fragments = 0u64;
    let mut lane_count = 0u64;
    let mut offset = 0usize;
    while offset < data.sources.len() {
        let expected_lane =
            u32::try_from(lane_count).map_err(|_| invariant("lane count does not fit u32"))?;
        let lane = data.sources[offset].lane_ordinal;
        if lane != expected_lane {
            return Err(invariant("lane ordinals must be contiguous from zero"));
        }
        let start = offset;
        while offset < data.sources.len() && data.sources[offset].lane_ordinal == lane {
            offset += 1;
        }
        let lane_sources = &data.sources[start..offset];
        match data.input_mode {
            BundleInputMode::SingleEnd => {
                if lane_sources.len() != 1 || lane_sources[0].role != MateRole::S {
                    return Err(invariant("single-end lanes must contain exactly role S"));
                }
                fragments = checked_add(
                    fragments,
                    lane_sources[0].records,
                    "single-end fragment total",
                )?;
            }
            BundleInputMode::PairedEnd => {
                if lane_sources.len() != 2
                    || lane_sources[0].role != MateRole::R1
                    || lane_sources[1].role != MateRole::R2
                    || lane_sources[0].format != lane_sources[1].format
                    || lane_sources[0].records != lane_sources[1].records
                {
                    return Err(invariant(
                        "paired lanes must contain synchronized R1/R2 sources of one format",
                    ));
                }
                fragments =
                    checked_add(fragments, lane_sources[0].records, "paired fragment total")?;
            }
        }
        lane_count = checked_add(lane_count, 1, "lane count")?;
    }
    if fragments != data.supplied_fragment_instances {
        return Err(invariant(
            "source records do not reconcile to supplied fragments",
        ));
    }
    Ok(lane_count)
}

fn validate_input_telemetry(data: &BundleData) -> Result<()> {
    let source_count = u64::try_from(data.sources.len())
        .map_err(|_| overflow("logical source count does not fit u64"))?;
    if data.spool_bytes == 0
        || data.spool_bytes > data.limits.max_spool_bytes
        || data.spool_bytes > data.limits.max_temp_bytes
    {
        return Err(invariant(
            "spool byte telemetry violates configured resource limits",
        ));
    }
    if data.raw_transport_bytes == 0
        || data.raw_transport_bytes > data.limits.max_raw_transport_bytes
        || data.decoded_input_bytes < data.supplied_bases
        || data.decoded_input_bytes > data.limits.max_decoded_input_bytes
        || data.inferred_mate_roles > data.supplied_read_instances
        || data.gzip_sources > source_count
        || data.gzip_members < data.gzip_sources
        || data.gzip_members > data.limits.max_gzip_members
        || (data.gzip_sources == 0) != (data.gzip_members == 0)
    {
        return Err(invariant(
            "aggregate input transport telemetry is inconsistent",
        ));
    }
    Ok(())
}

fn validate_windows(windows: &WindowStats) -> Result<()> {
    let rejected = checked_add(
        checked_add(
            windows.ambiguity_only,
            windows.quality_only,
            "window rejection partition",
        )?,
        windows.ambiguity_and_quality,
        "window rejection partition",
    )?;
    let partition = checked_add(windows.accepted, rejected, "window accounting partition")?;
    if partition != windows.possible {
        return Err(invariant(
            "window accounting does not partition possible windows",
        ));
    }
    Ok(())
}

fn validate_histogram(data: &BundleData) -> Result<()> {
    let histogram = &data.support_histogram;
    let mut prior = None;
    let mut observed_distinct = 0u64;
    let mut observed_mass = 0u64;
    let mut retained_distinct = 0u64;
    let mut retained_mass = 0u64;
    for bin in histogram {
        if bin.support == 0
            || bin.distinct_canonical_keys == 0
            || prior.is_some_and(|value| value >= bin.support)
        {
            return Err(invariant(
                "support histogram contains an invalid or duplicate bin",
            ));
        }
        prior = Some(bin.support);
        observed_distinct = checked_add(
            observed_distinct,
            bin.distinct_canonical_keys,
            "histogram distinct total",
        )?;
        let mass = checked_mul(
            bin.support,
            bin.distinct_canonical_keys,
            "histogram support mass",
        )?;
        observed_mass = checked_add(observed_mass, mass, "histogram support mass")?;
        if bin.support >= data.scientific.min_support {
            retained_distinct = checked_add(
                retained_distinct,
                bin.distinct_canonical_keys,
                "retained histogram distinct total",
            )?;
            retained_mass = checked_add(retained_mass, mass, "retained histogram support mass")?;
        }
    }
    if observed_distinct != data.observed_distinct_canonical_keys
        || observed_mass != data.observed_support_mass
        || retained_distinct != data.retained_distinct_canonical_keys
        || retained_mass != data.retained_support_mass
    {
        return Err(invariant(
            "support histogram does not reconcile to count totals",
        ));
    }
    if data.retained_distinct_canonical_keys > data.limits.max_retained_kmers {
        return Err(invariant("retained graph exceeds configured key limit"));
    }
    match data.scientific.support_unit {
        SupportUnit::AcceptedWindowOccurrence
            if data.observed_support_mass != data.windows.accepted =>
        {
            return Err(invariant(
                "occurrence support mass does not equal accepted windows",
            ));
        }
        SupportUnit::SuppliedFragmentInstance
            if data.observed_support_mass > data.windows.accepted =>
        {
            return Err(invariant(
                "fragment support mass exceeds accepted window occurrences",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn validate_unitigs(data: &BundleData) -> Result<UnitigIndex<'_>> {
    let unitigs = &data.unitigs;
    if unitigs.windows(2).any(|window| {
        window[1]
            .sequence
            .len()
            .cmp(&window[0].sequence.len())
            .then_with(|| window[0].sequence.cmp(&window[1].sequence))
            .then_with(|| window[0].id.as_bytes().cmp(window[1].id.as_bytes()))
            .is_gt()
    }) {
        return Err(invariant("unitigs are not in the frozen output order"));
    }
    let mut represented = 0u64;
    let mut minimum_support_mass = 0u64;
    let mut maximum_support_mass = 0u64;
    for unitig in unitigs {
        if unitig.sequence.iter().any(|base| !b"ACGT".contains(base)) {
            return Err(invariant("emitted unitig contains a non-ACGT byte"));
        }
        let length = u64::try_from(unitig.sequence.len())
            .map_err(|_| invariant("unitig length does not fit u64"))?;
        let expected_length = unitig
            .edge_steps
            .checked_add(u64::from(data.scientific.k) - 1)
            .ok_or_else(|| invariant("unitig length arithmetic overflow"))?;
        if length != expected_length
            || unitig.edge_steps == 0
            || unitig.edge_steps > 1_000_000_000
            || unitig.canonical_kmers == 0
            || unitig.canonical_kmers > 500_000_000
            || unitig.canonical_kmers != unitig.edge_steps
        {
            return Err(invariant(
                "unitig length or graph counts violate the schema",
            ));
        }
        if unitig.minimum_support < data.scientific.min_support
            || unitig.minimum_support > unitig.lower_median_support
            || unitig.lower_median_support > unitig.maximum_support
        {
            return Err(invariant("unitig support summary is inconsistent"));
        }
        for support in [
            unitig.minimum_support,
            unitig.lower_median_support,
            unitig.maximum_support,
        ] {
            if data
                .support_histogram
                .binary_search_by_key(&support, |bin| bin.support)
                .is_err()
            {
                return Err(invariant(
                    "unitig support summary is absent from the retained support histogram",
                ));
            }
        }
        minimum_support_mass = checked_add(
            minimum_support_mass,
            checked_mul(
                unitig.canonical_kmers,
                unitig.minimum_support,
                "unitig minimum support-mass product overflow",
            )?,
            "unitig minimum support-mass sum overflow",
        )?;
        maximum_support_mass = checked_add(
            maximum_support_mass,
            checked_mul(
                unitig.canonical_kmers,
                unitig.maximum_support,
                "unitig maximum support-mass product overflow",
            )?,
            "unitig maximum support-mass sum overflow",
        )?;
        let sequence_digest = sha256_bytes(&unitig.sequence);
        if sequence_digest != unitig.sequence_sha256 {
            return Err(invariant("unitig sequence SHA-256 does not match sequence"));
        }
        let mut identity = Sha256::new();
        identity.update(b"veritasm:unitig:v1\0");
        identity.update([data.scientific.k, unitig.topology.tag()]);
        identity.update(length.to_le_bytes());
        identity.update(&unitig.sequence);
        let expected_id = format!("utg-{}", hex_lower(&identity.finalize()));
        if unitig.id != expected_id {
            return Err(invariant(
                "unitig identifier digest does not match its preimage",
            ));
        }
        represented = checked_add(
            represented,
            unitig.canonical_kmers,
            "unitig represented-key total",
        )?;
    }
    if represented != data.retained_distinct_canonical_keys
        || (unitigs.is_empty() != (data.retained_distinct_canonical_keys == 0))
    {
        return Err(invariant(
            "unitig backing keys do not conserve the retained graph",
        ));
    }
    // These are necessary aggregate bounds only. The histogram does not retain
    // the assignment of individual support values to unitig backing keys, so
    // satisfying them is not evidence that every per-unitig summary is exact.
    if data.retained_support_mass < minimum_support_mass
        || data.retained_support_mass > maximum_support_mass
    {
        return Err(invariant(
            "retained support mass lies outside conservative unitig summary bounds",
        ));
    }
    UnitigIndex::new(unitigs)
}

fn validate_graph(data: &BundleData, by_id: &UnitigIndex<'_>) -> Result<()> {
    if data.self_reverse_complement_keys > data.retained_distinct_canonical_keys {
        return Err(invariant(
            "self-reverse-complement key count exceeds retained keys",
        ));
    }
    let expected_handles = checked_mul(
        data.retained_distinct_canonical_keys,
        2,
        "oriented handle count",
    )?
    .checked_sub(data.self_reverse_complement_keys)
    .ok_or_else(|| invariant("oriented handle count underflow"))?;
    if expected_handles != data.oriented_handles {
        return Err(invariant("oriented handle count does not reconcile"));
    }
    let links = &data.graph_links;
    if links.windows(2).any(|window| window[0] >= window[1]) {
        return Err(invariant(
            "GFA graph links are duplicated or not in canonical order",
        ));
    }
    let overlap = usize::from(data.scientific.k - 1);
    for link in links {
        if !matches!(link.from_orientation, '+' | '-') || !matches!(link.to_orientation, '+' | '-')
        {
            return Err(invariant("invalid GFA graph-link orientation"));
        }
        let from = by_id
            .get(link.from_segment.as_str())
            .ok_or_else(|| invariant("GFA link references an unknown source segment"))?;
        let to = by_id
            .get(link.to_segment.as_str())
            .ok_or_else(|| invariant("GFA link references an unknown target segment"))?;
        for offset in 0..overlap {
            let from_base = oriented_base(
                &from.sequence,
                link.from_orientation,
                from.sequence.len() - overlap + offset,
            )?;
            let to_base = oriented_base(&to.sequence, link.to_orientation, offset)?;
            if from_base != to_base {
                return Err(invariant(
                    "GFA link does not have the declared exact overlap",
                ));
            }
        }
        let forward_key = (
            link.from_segment.as_str(),
            orientation_rank(link.from_orientation),
            link.to_segment.as_str(),
            orientation_rank(link.to_orientation),
        );
        let reverse_key = (
            link.to_segment.as_str(),
            orientation_rank(flip_orientation(link.to_orientation)),
            link.from_segment.as_str(),
            orientation_rank(flip_orientation(link.from_orientation)),
        );
        if reverse_key < forward_key {
            return Err(invariant(
                "GFA link is not the canonical relation representative",
            ));
        }
    }
    Ok(())
}

fn validate_mapper(data: &BundleData) -> Result<()> {
    let mapper = &data.mapper;
    if mapper.algorithm_id != INDEXED_MAPPER_ID
        || mapper.algorithm_version != INDEXED_MAPPER_VERSION
        || mapper.target_universe != INDEXED_TARGET_UNIVERSE
        || usize::from(mapper.seed_length) != DEFAULT_SEED_LENGTH
    {
        return Err(invariant(
            "construction-read mapper descriptor does not match the stable indexed algorithm",
        ));
    }
    let plan = IndexedExactMapper::plan(&data.unitigs, DEFAULT_SEED_LENGTH)?;
    let linear_targets = u64::try_from(plan.linear_targets())
        .map_err(|_| invariant("linear mapper target count does not fit u64"))?;
    let index_postings = u64::try_from(plan.posting_count())
        .map_err(|_| invariant("mapper posting count does not fit u64"))?;
    if data.scientific.remap {
        let maximum_accounted_index_bytes =
            indexed_mapper_memory_budget(&data.unitigs, data.limits.memory_budget_bytes)?;
        if mapper.execution_status != "executed"
            || mapper.linear_targets != linear_targets
            || mapper.index_postings != index_postings
            || mapper.accounted_index_bytes < plan.resident_bound_bytes()
            || mapper.accounted_index_bytes > maximum_accounted_index_bytes
        {
            return Err(invariant(
                "executed mapper descriptor does not reconcile to emitted linear targets",
            ));
        }
    } else if mapper.execution_status != "not_requested"
        || mapper.linear_targets != 0
        || mapper.index_postings != 0
        || mapper.accounted_index_bytes != 0
    {
        return Err(invariant(
            "disabled remapping has an executed or allocated mapper descriptor",
        ));
    }
    Ok(())
}

fn validate_read_states(data: &BundleData) -> Result<&'static str> {
    const ENABLED_STATES: [&str; 5] = [
        "ineligible_ambiguity_or_quality",
        "indeterminate_candidate_limit",
        "unmapped",
        "single_placement_group",
        "multiple_placement_groups",
    ];
    let roles: &[MateRole] = match data.input_mode {
        BundleInputMode::SingleEnd => &[MateRole::S],
        BundleInputMode::PairedEnd => &[MateRole::R1, MateRole::R2],
    };
    let expected_states: &[&str] = if data.scientific.remap {
        &ENABLED_STATES
    } else {
        &["not_requested"]
    };
    if data.read_state_counts.len() != roles.len().saturating_mul(expected_states.len()) {
        return Err(invariant(
            "read-audit state set is not the exact conditional set",
        ));
    }
    let mut any_indeterminate = false;
    let mut row_index = 0usize;
    for &role in roles {
        let mut role_sum = 0u64;
        for &state in expected_states {
            let provided = &data.read_state_counts[row_index];
            row_index += 1;
            if provided.role != Some(role) || provided.state != state {
                return Err(invariant(
                    "read-audit rows are not in the exact conditional order",
                ));
            }
            role_sum = checked_add(role_sum, provided.count, "read-audit role state total")?;
            any_indeterminate |= state == "indeterminate_candidate_limit" && provided.count != 0;
        }
        let supplied = data
            .sources
            .iter()
            .filter(|source| source.role == role)
            .try_fold(0u64, |total, source| {
                checked_add(total, source.records, "read-role source total")
            })?;
        if supplied != role_sum {
            return Err(invariant(
                "read-audit state counts do not reconcile to supplied reads by role",
            ));
        }
    }
    let status = if !data.scientific.remap {
        "unavailable_remap_disabled"
    } else if any_indeterminate {
        "indeterminate_candidate_limit"
    } else {
        "placement_enumeration_complete"
    };
    Ok(status)
}

fn validate_unitig_audit(data: &BundleData, unitigs: &[Unitig], global_status: &str) -> Result<()> {
    let (linear_status, unavailable_reason) = match global_status {
        "placement_enumeration_complete" => ("placement_enumeration_complete", None),
        "indeterminate_candidate_limit" => {
            ("indeterminate_candidate_limit", Some("candidate_limit"))
        }
        "unavailable_remap_disabled" => ("unavailable_remap_disabled", Some("remap_disabled")),
        _ => return Err(invariant("unknown global placement enumeration status")),
    };
    let complete_multiple_state = if global_status == "placement_enumeration_complete" {
        data.read_state_counts
            .iter()
            .filter(|row| row.state == "multiple_placement_groups")
            .try_fold(0u64, |total, row| {
                checked_add(total, row.count, "multiple-placement read-state total")
            })?
    } else {
        0
    };
    let mut placement_sum = 0u64;
    let mut single_sum = 0u64;
    let mut multi_membership_sum = 0u64;
    for unitig in unitigs {
        let fields = [
            &unitig.enumeration_complete_read_placements,
            &unitig.single_group_read_instances,
            &unitig.multi_group_read_instances_with_group,
        ];
        if unitig.topology == Topology::ClosedGraphWalk {
            if unitig.placement_enumeration_status != "unavailable_closed_walk_audit_unsupported"
                || fields.iter().any(|field| {
                    !matches!(
                        field,
                        AvailabilityU64::NotAvailable("closed_walk_audit_unsupported")
                    )
                })
            {
                return Err(invariant("closed-walk audit availability is inconsistent"));
            }
            continue;
        }
        if unitig.placement_enumeration_status != linear_status {
            return Err(invariant("linear-unitig audit status is inconsistent"));
        }
        match unavailable_reason {
            Some(reason)
                if fields.iter().any(|field| {
                    !matches!(field, AvailabilityU64::NotAvailable(actual) if *actual == reason)
                }) =>
            {
                return Err(invariant("linear-unitig NA reason is inconsistent"));
            }
            None => {
                let values = match (
                    &unitig.enumeration_complete_read_placements,
                    &unitig.single_group_read_instances,
                    &unitig.multi_group_read_instances_with_group,
                ) {
                    (
                        AvailabilityU64::Value(placements),
                        AvailabilityU64::Value(single),
                        AvailabilityU64::Value(multi),
                    ) => (*placements, *single, *multi),
                    _ => return Err(invariant("complete audit contains an NA aggregate")),
                };
                let minimum_unitig_placements = checked_add(
                    values.1,
                    values.2,
                    "minimum per-unitig placement total",
                )?;
                if values.0 < minimum_unitig_placements || values.2 > complete_multiple_state {
                    return Err(invariant(
                        "per-unitig placements do not reconcile to read memberships",
                    ));
                }
                placement_sum = checked_add(placement_sum, values.0, "unitig placement total")?;
                single_sum = checked_add(single_sum, values.1, "unitig single-read total")?;
                multi_membership_sum = checked_add(
                    multi_membership_sum,
                    values.2,
                    "unitig multiple-read membership total",
                )?;
            }
            Some(_) => {}
        }
    }
    if global_status == "placement_enumeration_complete" {
        let state = |name: &str| -> Result<u64> {
            data.read_state_counts
                .iter()
                .filter(|row| row.state == name)
                .try_fold(0u64, |total, row| {
                    checked_add(total, row.count, "read-audit state total")
                })
        };
        let single_state = state("single_placement_group")?;
        let multiple_state = complete_multiple_state;
        if single_sum != single_state {
            return Err(invariant(
                "unitig single-group total does not reconcile to read states",
            ));
        }
        let minimum_multiple_placements =
            checked_mul(multiple_state, 2, "minimum multiple-group placement total")?;
        let minimum_placements = checked_add(
            single_state,
            minimum_multiple_placements,
            "minimum complete placement total",
        )?;
        let membership_floor = checked_add(
            single_state,
            multi_membership_sum,
            "placement membership floor",
        )?;
        let mapped_reads = checked_add(single_state, multiple_state, "mapped read-instance total")?;
        let maximum_placements = checked_mul(
            mapped_reads,
            data.limits.max_mapping_candidates,
            "maximum complete placement total",
        )?;
        let linear_unitigs = u64::try_from(
            unitigs
                .iter()
                .filter(|unitig| unitig.topology == Topology::Linear)
                .count(),
        )
        .map_err(|_| overflow("linear unitig count does not fit u64"))?;
        let maximum_memberships = checked_mul(
            multiple_state,
            linear_unitigs,
            "maximum multiple-read membership total",
        )?;
        if (multiple_state == 0 && (placement_sum != single_state || multi_membership_sum != 0))
            || multi_membership_sum < multiple_state
            || multi_membership_sum > maximum_memberships
            || placement_sum < minimum_placements
            || placement_sum < membership_floor
            || placement_sum > maximum_placements
        {
            return Err(invariant(
                "unitig placement totals do not reconcile to read-state semantics",
            ));
        }
    }
    Ok(())
}

fn validate_pair_states(data: &BundleData, lane_count: u64) -> Result<&'static str> {
    let (mode, states): (&str, &[&str]) = match (data.input_mode, data.scientific.remap) {
        (BundleInputMode::SingleEnd, _) => ("not_paired_input", &["not_paired_input"]),
        (BundleInputMode::PairedEnd, false) => ("remap_not_requested", &["remap_not_requested"]),
        (BundleInputMode::PairedEnd, true) => (
            "paired_remap_enabled",
            &[
                "mate_ineligible",
                "mate_indeterminate_candidate_limit",
                "mate_unmapped",
                "mate_multiple_placement_groups",
                "same_linear_unitig",
                "endpoint_tie",
                "cross_unitig_observation",
            ],
        ),
    };
    if data.pair_state_counts.len() != states.len() {
        return Err(invariant(
            "pair-audit state set is not the exact conditional set",
        ));
    }
    let mut total = 0u64;
    for (&expected, provided) in states.iter().zip(&data.pair_state_counts) {
        if provided.role.is_some() || provided.state != expected {
            return Err(invariant(
                "pair-audit rows are not in the exact conditional order",
            ));
        }
        total = checked_add(total, provided.count, "pair-audit state total")?;
    }
    if total != data.supplied_fragment_instances {
        return Err(invariant(
            "pair-audit state counts do not reconcile to supplied fragments",
        ));
    }

    let lane_count = usize::try_from(lane_count)
        .map_err(|_| invariant("pair-audit lane count does not fit usize"))?;
    let expected_lane_rows = lane_count
        .checked_mul(states.len())
        .ok_or_else(|| invariant("pair-lane state row count overflow"))?;
    if data.pair_lane_state_counts.len() != expected_lane_rows {
        return Err(invariant(
            "pair-lane state set is not the exact conditional set",
        ));
    }
    let mut lane_state_totals = [0u64; 7];
    let mut lane_fragment_total = 0u64;
    for lane_index in 0..lane_count {
        let lane_ordinal = u32::try_from(lane_index)
            .map_err(|_| invariant("pair-audit lane ordinal does not fit u32"))?;
        let source_index = match data.input_mode {
            BundleInputMode::SingleEnd => lane_index,
            BundleInputMode::PairedEnd => lane_index
                .checked_mul(2)
                .ok_or_else(|| invariant("paired source index overflow"))?,
        };
        let lane_fragments = data
            .sources
            .get(source_index)
            .ok_or_else(|| invariant("pair-lane state names an absent source lane"))?
            .records;
        lane_fragment_total = checked_add(
            lane_fragment_total,
            lane_fragments,
            "pair-lane fragment total",
        )?;
        let row_offset = lane_index
            .checked_mul(states.len())
            .ok_or_else(|| invariant("pair-lane state row offset overflow"))?;
        let mut lane_sum = 0u64;
        for (state_index, &expected_state) in states.iter().enumerate() {
            let provided = data
                .pair_lane_state_counts
                .get(row_offset + state_index)
                .ok_or_else(|| invariant("pair-lane state row is missing"))?;
            if provided.lane_ordinal != lane_ordinal || provided.state != expected_state {
                return Err(invariant(
                    "pair-lane rows are not in exact lane and conditional-state order",
                ));
            }
            lane_sum = checked_add(lane_sum, provided.count, "pair-lane state total")?;
            lane_state_totals[state_index] = checked_add(
                lane_state_totals[state_index],
                provided.count,
                "pair-lane global state total",
            )?;
        }
        if lane_sum != lane_fragments {
            return Err(invariant(
                "pair-lane state counts do not reconcile to source-lane fragments",
            ));
        }
    }
    if lane_fragment_total != data.supplied_fragment_instances {
        return Err(invariant(
            "pair-lane fragment totals do not reconcile to supplied fragments",
        ));
    }
    for (state_index, global) in data.pair_state_counts.iter().enumerate() {
        if lane_state_totals[state_index] != global.count {
            return Err(invariant(
                "pair-lane state totals do not reconcile to global pair states",
            ));
        }
    }
    Ok(mode)
}

fn validate_pair_links(
    data: &BundleData,
    unitigs: &UnitigIndex<'_>,
    lane_count: u64,
) -> Result<()> {
    let links = &data.pair_links;
    if links.windows(2).any(|window| {
        (window[0].lane_ordinal, &window[0].a, &window[0].b)
            >= (window[1].lane_ordinal, &window[1].a, &window[1].b)
    }) {
        return Err(invariant(
            "pair lane/endpoint groups are duplicated or not in canonical order",
        ));
    }
    let mut support = 0u64;
    for link in links {
        if data.input_mode != BundleInputMode::PairedEnd
            || u64::from(link.lane_ordinal) >= lane_count
        {
            return Err(invariant(
                "pair-link lane is not a paired lane in the immutable source catalogue",
            ));
        }
        validate_pair_endpoint(&link.a, unitigs)?;
        validate_pair_endpoint(&link.b, unitigs)?;
        if link.a.segment == link.b.segment
            || link.a.mate_role == link.b.mate_role
            || link.a > link.b
            || link.supplied_fragment_instances == 0
        {
            return Err(invariant(
                "pair endpoint group violates canonical constraints",
            ));
        }
        support = checked_add(
            support,
            link.supplied_fragment_instances,
            "pair-link support total",
        )?;
    }
    let cross = data
        .pair_state_counts
        .iter()
        .find(|state| state.state == "cross_unitig_observation")
        .map_or(0, |state| state.count);
    if support != cross || links.is_empty() != (cross == 0) {
        return Err(invariant(
            "pair-link support does not reconcile to cross-unitig pair state",
        ));
    }
    let lane_count = usize::try_from(lane_count)
        .map_err(|_| invariant("pair-link lane count does not fit usize"))?;
    let states_per_lane = if data.input_mode == BundleInputMode::PairedEnd && data.scientific.remap
    {
        7usize
    } else {
        1usize
    };
    let mut next_link = links.iter().peekable();
    for lane_index in 0..lane_count {
        let lane_ordinal = u32::try_from(lane_index)
            .map_err(|_| invariant("pair-link lane ordinal does not fit u32"))?;
        let mut lane_support = 0u64;
        while next_link
            .peek()
            .is_some_and(|link| link.lane_ordinal == lane_ordinal)
        {
            let link = next_link
                .next()
                .ok_or_else(|| invariant("pair-link iterator ended unexpectedly"))?;
            lane_support = checked_add(
                lane_support,
                link.supplied_fragment_instances,
                "pair-link lane support total",
            )?;
        }
        let expected_cross = if states_per_lane == 7 {
            let row = lane_index
                .checked_mul(states_per_lane)
                .and_then(|offset| offset.checked_add(6))
                .ok_or_else(|| invariant("pair-link lane-state index overflow"))?;
            data.pair_lane_state_counts
                .get(row)
                .ok_or_else(|| invariant("pair-link lane cross-unitig state is missing"))?
                .count
        } else {
            0
        };
        if lane_support != expected_cross {
            return Err(invariant(
                "pair-link lane support does not reconcile to lane cross-unitig state",
            ));
        }
    }
    if next_link.next().is_some() {
        return Err(invariant(
            "pair-link lane is outside the immutable source catalogue",
        ));
    }
    Ok(())
}

fn validate_pair_endpoint(endpoint: &PairEndpoint, unitigs: &UnitigIndex<'_>) -> Result<()> {
    let segment = unitigs
        .get(endpoint.segment.as_str())
        .ok_or_else(|| invariant("invalid pair endpoint"))?;
    let segment_length = u64::try_from(segment.sequence.len())
        .map_err(|_| invariant("unitig length does not fit u64"))?;
    if segment.topology != Topology::Linear
        || endpoint.end_distance >= segment_length
        || !matches!(endpoint.end, 'L' | 'R')
        || !matches!(endpoint.strand, '+' | '-')
        || !matches!(endpoint.mate_role, MateRole::R1 | MateRole::R2)
    {
        return Err(invariant("invalid pair endpoint"));
    }
    Ok(())
}

fn validate_transforms(data: &BundleData) -> Result<()> {
    let transforms = &data.transformations;
    if transforms.len() != 2
        || transforms[0].stage_order != 0
        || transforms[0].algorithm_id != "exact_count_observation"
        || transforms[1].stage_order != 1
        || transforms[1].algorithm_id != "absolute_support_retention"
    {
        return Err(invariant(
            "transformation journal does not contain exact stages 0 and 1",
        ));
    }
    for transform in transforms {
        validate_sha256(&transform.decision_set_sha256, "decision-set SHA-256")?;
        validate_sha256(&transform.pre_state_sha256, "pre-state SHA-256")?;
        validate_sha256(&transform.post_state_sha256, "post-state SHA-256")?;
        serde_json::from_str::<Value>(&transform.parameters_json).map_err(|cause| {
            invariant(format!(
                "transformation parameters are invalid JSON: {cause}"
            ))
        })?;
    }
    let observation = &transforms[0];
    let expected_empty_observation =
        empty_decision_digest(data.scientific.k, data.scientific.support_unit, 0);
    if observation.parameters_json != "{}"
        || observation.input_distinct != data.observed_distinct_canonical_keys
        || observation.output_distinct != data.observed_distinct_canonical_keys
        || observation.input_support_mass != data.observed_support_mass
        || observation.output_support_mass != data.observed_support_mass
        || observation.removed_key_count != 0
        || observation.removed_support_mass != 0
        || observation.decision_set_sha256 != expected_empty_observation
        || observation.pre_state_sha256 != observation.post_state_sha256
        || observation.status != "software_stage_complete_no_change"
    {
        return Err(invariant("exact-count observation stage is inconsistent"));
    }
    let retention = &transforms[1];
    let expected_parameters = format!(
        "{{\"retention_min_support_decimal\":\"{}\",\"support_unit\":\"{}\"}}",
        data.scientific.min_support,
        data.scientific.support_unit.as_str()
    );
    let removed_keys = data
        .observed_distinct_canonical_keys
        .checked_sub(data.retained_distinct_canonical_keys)
        .ok_or_else(|| invariant("retention distinct count underflow"))?;
    let removed_mass = data
        .observed_support_mass
        .checked_sub(data.retained_support_mass)
        .ok_or_else(|| invariant("retention support mass underflow"))?;
    let expected_status = if removed_keys == 0 {
        "software_stage_complete_no_change"
    } else {
        "software_stage_complete_with_change"
    };
    if retention.parameters_json != expected_parameters
        || retention.input_distinct != data.observed_distinct_canonical_keys
        || retention.output_distinct != data.retained_distinct_canonical_keys
        || retention.input_support_mass != data.observed_support_mass
        || retention.output_support_mass != data.retained_support_mass
        || retention.removed_key_count != removed_keys
        || retention.removed_support_mass != removed_mass
        || retention.pre_state_sha256 != observation.post_state_sha256
        || retention.status != expected_status
    {
        return Err(invariant("absolute-retention stage is inconsistent"));
    }
    if removed_keys == 0 {
        if retention.decision_set_sha256
            != empty_decision_digest(data.scientific.k, data.scientific.support_unit, 1)
        {
            return Err(invariant(
                "no-op retention has the wrong empty decision-set digest",
            ));
        }
        if retention.post_state_sha256 != retention.pre_state_sha256 {
            return Err(invariant(
                "no-op retention changes the exact count state digest",
            ));
        }
    }
    Ok(())
}

fn empty_decision_digest(k: u8, support_unit: SupportUnit, stage: u8) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"veritasm:decision-set:v1\0");
    hasher.update([k, support_unit.tag(), stage]);
    hasher.update(0u64.to_le_bytes());
    hex_lower(&hasher.finalize())
}

fn validate_artifacts(root: &Path, prepared: &Prepared<'_>, scientific_digest: &str) -> Result<()> {
    validate_fasta(&root.join("unitigs.fasta"), prepared)?;
    validate_gfa(&root.join("assembly.gfa"), prepared)?;
    validate_rendered_artifact(
        &root.join("unitig_evidence.tsv"),
        "unitig_evidence.tsv",
        |writer| prepared.write_unitig_evidence(writer),
    )?;
    validate_rendered_artifact(&root.join("pair_links.tsv"), "pair_links.tsv", |writer| {
        prepared.write_pair_links(writer)
    })?;
    validate_rendered_artifact(
        &root.join("pair_audit_summary.tsv"),
        "pair_audit_summary.tsv",
        |writer| prepared.write_pair_summary(writer),
    )?;
    validate_rendered_artifact(
        &root.join("transform_summary.tsv"),
        "transform_summary.tsv",
        |writer| prepared.write_transforms(writer),
    )?;
    validate_run_stream(
        &root.join("run.json"),
        &prepared.run_document(scientific_digest),
    )?;
    validate_report_stream(&root.join("report.html"), &prepared.report_data())?;
    for (relative, expected) in SCHEMAS {
        validate_exact_artifact(&root.join(relative), relative, expected)?;
    }
    Ok(())
}

fn validate_fasta(path: &Path, expected: &Prepared<'_>) -> Result<()> {
    let mut reader = buffered_artifact(path, "unitigs.fasta")?;
    for unitig in expected.unitigs {
        let header = format!(
            ">{} schema_version={} length_bases={} k={} topology={} edge_steps={} canonical_kmers={} support_unit={} retention_min_support={} minimum_represented_key_support={} lower_median_represented_key_support={} maximum_represented_key_support={} placement_enumeration_status={} sequence_sha256={}\n",
            unitig.id,
            UNITIG_SCHEMA_VERSION,
            unitig.sequence.len(),
            expected.scientific.k,
            unitig.topology.as_str(),
            unitig.edge_steps,
            unitig.canonical_kmers,
            expected.scientific.support_unit.as_str(),
            expected.scientific.min_support,
            unitig.minimum_support,
            unitig.lower_median_support,
            unitig.maximum_support,
            unitig.placement_enumeration_status,
            unitig.sequence_sha256
        );
        expect_bytes(&mut reader, header.as_bytes(), "unitigs.fasta header")?;
        for line in unitig.sequence.chunks(80) {
            expect_bytes(&mut reader, line, "unitigs.fasta sequence")?;
            expect_bytes(&mut reader, b"\n", "unitigs.fasta sequence LF")?;
        }
    }
    expect_eof(&mut reader, "unitigs.fasta")
}

fn validate_gfa(path: &Path, expected: &Prepared<'_>) -> Result<()> {
    let mut reader = buffered_artifact(path, "assembly.gfa")?;
    let header = format!(
        "H\tVN:Z:1.0\tPN:Z:veritasm\tPV:Z:{}\tSC:Z:{}\n",
        env!("CARGO_PKG_VERSION"),
        ASSEMBLY_GFA_SCHEMA_VERSION
    );
    expect_bytes(&mut reader, header.as_bytes(), "assembly.gfa H record")?;
    for unitig in expected.unitigs {
        let prefix = format!("S\t{}\t", unitig.id);
        expect_bytes(&mut reader, prefix.as_bytes(), "assembly.gfa S identifier")?;
        expect_bytes(&mut reader, &unitig.sequence, "assembly.gfa S sequence")?;
        let suffix = format!(
            "\tTP:Z:{}\tES:i:{}\tCK:i:{}\tSH:H:{}\n",
            unitig.topology.as_str(),
            unitig.edge_steps,
            unitig.canonical_kmers,
            unitig.sequence_sha256.to_ascii_uppercase()
        );
        expect_bytes(&mut reader, suffix.as_bytes(), "assembly.gfa S tags")?;
    }
    let overlap = u16::from(expected.scientific.k) - 1;
    for link in expected.graph_links {
        let record = format!(
            "L\t{}\t{}\t{}\t{}\t{}M\n",
            link.from_segment, link.from_orientation, link.to_segment, link.to_orientation, overlap
        );
        expect_bytes(&mut reader, record.as_bytes(), "assembly.gfa L record")?;
    }
    expect_eof(&mut reader, "assembly.gfa")
}

fn buffered_artifact(path: &Path, label: &str) -> Result<BufReader<File>> {
    File::open(path)
        .map(BufReader::new)
        .map_err(|cause| io_error(ErrorCode::CommitReopen, format!("reopen {label}"), cause))
}

fn expect_bytes(
    reader: &mut BufReader<File>,
    mut expected: &[u8],
    context: &'static str,
) -> Result<()> {
    while !expected.is_empty() {
        let available = reader
            .fill_buf()
            .map_err(|cause| io_error(ErrorCode::CommitReopen, context, cause))?;
        if available.is_empty() {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("{context} ended before the expected bytes"),
            ));
        }
        let compared = available.len().min(expected.len());
        if available[..compared] != expected[..compared] {
            return Err(error(
                ErrorCode::IntegrityArtifact,
                format!("{context} differs from the typed bundle data"),
            ));
        }
        reader.consume(compared);
        expected = &expected[compared..];
    }
    Ok(())
}

fn expect_eof(reader: &mut BufReader<File>, context: &'static str) -> Result<()> {
    let available = reader
        .fill_buf()
        .map_err(|cause| io_error(ErrorCode::CommitReopen, context, cause))?;
    if available.is_empty() {
        Ok(())
    } else {
        Err(error(
            ErrorCode::IntegrityArtifact,
            format!("{context} contains trailing bytes"),
        ))
    }
}

fn validate_rendered_artifact<F>(path: &Path, label: &'static str, render: F) -> Result<()>
where
    F: FnOnce(&mut dyn Write) -> io::Result<()>,
{
    let mut comparator = ArtifactComparator::open(path, label)?;
    render(&mut comparator).map_err(|cause| {
        io_error(
            ErrorCode::IntegrityArtifact,
            format!("compare {label} with typed bundle data"),
            cause,
        )
    })?;
    comparator.finish()
}

fn validate_exact_artifact(path: &Path, label: &'static str, expected: &[u8]) -> Result<()> {
    let mut reader = buffered_artifact(path, label)?;
    expect_bytes(&mut reader, expected, "embedded schema bytes")?;
    expect_eof(&mut reader, label)
}

fn validate_run_stream(path: &Path, expected: &RunDocument<'_>) -> Result<()> {
    let mut comparator = ArtifactComparator::open(path, "run.json")?;
    serde_json::to_writer_pretty(&mut comparator, expected).map_err(|cause| {
        error(
            ErrorCode::IntegrityArtifact,
            format!("compare run.json with typed run data: {cause}"),
        )
    })?;
    comparator.write_all(b"\n").map_err(|cause| {
        io_error(
            ErrorCode::IntegrityArtifact,
            "compare run.json final LF",
            cause,
        )
    })?;
    comparator.finish()
}

fn validate_report_stream(path: &Path, expected: &report::ReportData<'_>) -> Result<()> {
    let mut comparator = ArtifactComparator::open(path, "report.html")?;
    report::write_html(&mut comparator, expected).map_err(|cause| {
        io_error(
            ErrorCode::IntegrityArtifact,
            "compare report.html with typed report data",
            cause,
        )
    })?;
    comparator.finish()
}

struct ArtifactComparator {
    reader: BufReader<File>,
    label: &'static str,
}

impl ArtifactComparator {
    fn open(path: &Path, label: &'static str) -> Result<Self> {
        Ok(Self {
            reader: buffered_artifact(path, label)?,
            label,
        })
    }

    fn finish(&mut self) -> Result<()> {
        expect_eof(&mut self.reader, self.label)
    }
}

impl Write for ArtifactComparator {
    fn write(&mut self, mut expected: &[u8]) -> io::Result<usize> {
        let requested = expected.len();
        while !expected.is_empty() {
            let available = self.reader.fill_buf()?;
            if available.is_empty() {
                return Err(io::Error::other(format!(
                    "{} ended before the typed rendering",
                    self.label
                )));
            }
            let compared = available.len().min(expected.len());
            if available[..compared] != expected[..compared] {
                return Err(io::Error::other(format!(
                    "{} differs from the typed rendering",
                    self.label
                )));
            }
            self.reader.consume(compared);
            expected = &expected[compared..];
        }
        Ok(requested)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn scientific_digest(digests: &BTreeMap<String, String>) -> Result<String> {
    let mut hasher = Sha256::new();
    for path in SCIENTIFIC_PATHS {
        let digest = digests
            .get(path)
            .ok_or_else(|| invariant(format!("missing scientific artifact digest: {path}")))?;
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn render_manifest(digests: &BTreeMap<String, String>) -> String {
    let mut output = String::new();
    for (path, digest) in digests {
        let _ = writeln!(output, "{digest}  {path}");
    }
    output
}

fn verify_manifest(root: &Path) -> Result<()> {
    verify_manifest_with_limits(root, BundleManifestLimits::default())
}

fn verify_manifest_with_limits(root: &Path, limits: BundleManifestLimits) -> Result<()> {
    verify_manifest_with_limits_and_hook(root, limits, None)
}

type BeforeBundleEntryOpen<'a> = Option<&'a mut dyn FnMut(&str)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BundleDescriptorSnapshot {
    device: u64,
    inode: u64,
    byte_len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl BundleDescriptorSnapshot {
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

#[derive(Debug)]
struct ManifestEntry {
    path: String,
    digest: [u8; 32],
}

#[derive(Debug)]
struct ObservedArtifact {
    path: String,
    digest: [u8; 32],
}

struct BundleDirectoryFrame {
    file: File,
    entries: rustix::fs::Dir,
    relative: String,
    depth: u32,
    snapshot: BundleDescriptorSnapshot,
}

fn verify_manifest_with_limits_and_hook(
    root: &Path,
    limits: BundleManifestLimits,
    mut before_entry_open: BeforeBundleEntryOpen<'_>,
) -> Result<()> {
    let root_file = open_bundle_root(root)?;
    let root_snapshot = descriptor_snapshot(&root_file, "bundle root")?;
    if !root_file
        .metadata()
        .map_err(|cause| io_error(ErrorCode::CommitReopen, "inspect opened bundle root", cause))?
        .is_dir()
    {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "bundle root is not a directory",
        ));
    }

    let mut manifest_file = open_bundle_entry(&root_file, c"manifest.sha256", "manifest.sha256")?;
    let manifest_metadata = manifest_file
        .metadata()
        .map_err(|cause| io_error(ErrorCode::CommitReopen, "inspect manifest.sha256", cause))?;
    if !manifest_metadata.is_file() {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "manifest.sha256 is not a regular file",
        ));
    }
    let manifest_snapshot = BundleDescriptorSnapshot::from_metadata(&manifest_metadata);
    if manifest_snapshot.byte_len > limits.max_manifest_bytes {
        return Err(error(
            ErrorCode::ResourceManifestBytes,
            "manifest.sha256 exceeds the verification byte limit",
        ));
    }
    let listed = read_opened_manifest(&mut manifest_file, manifest_snapshot, limits)?;
    let mut observed = scan_and_hash_bundle(
        root_file,
        root_snapshot,
        manifest_snapshot,
        limits,
        &mut before_entry_open,
    )?;
    observed.sort_unstable_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));

    if observed.len() != listed.len() {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "manifest has a missing or unlisted regular file",
        ));
    }
    for (expected, actual) in listed.iter().zip(&observed) {
        if expected.path != actual.path {
            return Err(error(
                ErrorCode::IntegrityManifest,
                "manifest has a missing or unlisted regular file",
            ));
        }
        if expected.digest != actual.digest {
            return Err(error(
                ErrorCode::IntegrityManifest,
                format!("manifest digest mismatch: {}", expected.path),
            ));
        }
    }
    Ok(())
}

fn open_bundle_root(root: &Path) -> Result<File> {
    let descriptor = rustix::fs::openat(
        CWD,
        root,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .map_err(|cause| {
        error(
            ErrorCode::IntegrityManifest,
            format!("open bundle root without following symbolic links: {cause}"),
        )
    })?;
    Ok(File::from(descriptor))
}

fn open_bundle_entry(parent: &File, name: &CStr, relative: &str) -> Result<File> {
    let descriptor = rustix::fs::openat(
        parent,
        name,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .map_err(|cause| {
        error(
            ErrorCode::IntegrityManifest,
            format!("open bundle entry {relative:?} without following symbolic links: {cause}"),
        )
    })?;
    Ok(File::from(descriptor))
}

fn descriptor_snapshot(file: &File, label: &str) -> Result<BundleDescriptorSnapshot> {
    file.metadata()
        .map(|metadata| BundleDescriptorSnapshot::from_metadata(&metadata))
        .map_err(|cause| io_error(ErrorCode::CommitReopen, format!("inspect {label}"), cause))
}

fn verify_unchanged_descriptor(
    file: &File,
    expected: BundleDescriptorSnapshot,
    label: &str,
) -> Result<()> {
    if descriptor_snapshot(file, label)? != expected {
        return Err(error(
            ErrorCode::IntegrityManifest,
            format!("{label} metadata changed during verification"),
        ));
    }
    Ok(())
}

fn read_opened_manifest(
    file: &mut File,
    snapshot: BundleDescriptorSnapshot,
    limits: BundleManifestLimits,
) -> Result<Vec<ManifestEntry>> {
    let mut listed = Vec::new();
    let mut total_bytes = 0u64;
    let mut line_bytes = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|cause| io_error(ErrorCode::CommitReopen, "read manifest.sha256", cause))?;
        if read == 0 {
            break;
        }
        let read_u64 =
            u64::try_from(read).map_err(|_| overflow("manifest read length does not fit u64"))?;
        total_bytes = total_bytes
            .checked_add(read_u64)
            .ok_or_else(|| overflow("manifest byte count overflow"))?;
        if total_bytes > limits.max_manifest_bytes {
            return Err(error(
                ErrorCode::ResourceManifestBytes,
                "manifest.sha256 exceeds the verification byte limit",
            ));
        }

        let mut start = 0usize;
        for (index, byte) in buffer[..read].iter().copied().enumerate() {
            if byte != b'\n' {
                continue;
            }
            append_manifest_line_bytes(&mut line_bytes, &buffer[start..index])?;
            parse_manifest_line(&line_bytes, &mut listed, limits)?;
            line_bytes.clear();
            start = index
                .checked_add(1)
                .ok_or_else(|| overflow("manifest buffer index overflow"))?;
        }
        append_manifest_line_bytes(&mut line_bytes, &buffer[start..read])?;
    }

    if total_bytes == 0 || !line_bytes.is_empty() {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "manifest lacks final LF",
        ));
    }
    if total_bytes != snapshot.byte_len {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "manifest.sha256 length changed while being read",
        ));
    }
    verify_unchanged_descriptor(file, snapshot, "manifest.sha256")?;
    Ok(listed)
}

fn append_manifest_line_bytes(line: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    line.try_reserve(bytes.len()).map_err(|cause| {
        error(
            ErrorCode::ResourceMemory,
            format!("grow bounded manifest line buffer: {cause}"),
        )
    })?;
    line.extend_from_slice(bytes);
    Ok(())
}

fn parse_manifest_line(
    line_bytes: &[u8],
    listed: &mut Vec<ManifestEntry>,
    limits: BundleManifestLimits,
) -> Result<()> {
    let line = std::str::from_utf8(line_bytes).map_err(|_| {
        error(
            ErrorCode::IntegrityManifest,
            "manifest contains non-UTF-8 bytes",
        )
    })?;
    let (digest, path) = line
        .split_once("  ")
        .ok_or_else(|| error(ErrorCode::IntegrityManifest, "malformed manifest line"))?;
    let digest = decode_lowercase_sha256(digest).ok_or_else(|| {
        error(
            ErrorCode::IntegrityManifest,
            "manifest digest is not lowercase SHA-256",
        )
    })?;
    validate_relative_path(path)?;
    let path_depth = u32::try_from(Path::new(path).components().count()).map_err(|_| {
        error(
            ErrorCode::ResourceMemory,
            "manifest path depth does not fit u32",
        )
    })?;
    if path_depth > limits.max_path_depth {
        return Err(error(
            ErrorCode::ResourceMemory,
            "manifest path exceeds the verification path-depth limit",
        ));
    }
    if listed
        .last()
        .is_some_and(|previous| previous.path.as_bytes() >= path.as_bytes())
    {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "manifest paths are duplicated or not strictly sorted",
        ));
    }
    let projected_regular_files = u64::try_from(listed.len())
        .map_err(|_| overflow("manifest entry count does not fit u64"))?
        .checked_add(2)
        .ok_or_else(|| overflow("manifest entry count overflow"))?;
    if projected_regular_files > limits.max_regular_files {
        return Err(error(
            ErrorCode::ResourceMemory,
            "manifest lists more files than the verification file-count limit",
        ));
    }

    let mut owned_path = String::new();
    owned_path.try_reserve_exact(path.len()).map_err(|cause| {
        error(
            ErrorCode::ResourceMemory,
            format!("allocate bounded manifest path: {cause}"),
        )
    })?;
    owned_path.push_str(path);
    listed.try_reserve(1).map_err(|cause| {
        error(
            ErrorCode::ResourceMemory,
            format!("grow bounded manifest inventory: {cause}"),
        )
    })?;
    listed.push(ManifestEntry {
        path: owned_path,
        digest,
    });
    Ok(())
}

fn decode_lowercase_sha256(value: &str) -> Option<[u8; 32]> {
    if !is_lowercase_sha256(value) {
        return None;
    }
    let mut digest = [0u8; 32];
    for (output, pair) in digest.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *output = lowercase_hex_nibble(pair[0])?
            .checked_mul(16)?
            .checked_add(lowercase_hex_nibble(pair[1])?)?;
    }
    Some(digest)
}

const fn lowercase_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn scan_and_hash_bundle(
    root_file: File,
    root_snapshot: BundleDescriptorSnapshot,
    manifest_snapshot: BundleDescriptorSnapshot,
    limits: BundleManifestLimits,
    before_entry_open: &mut BeforeBundleEntryOpen<'_>,
) -> Result<Vec<ObservedArtifact>> {
    let root_entries = rustix::fs::Dir::read_from(&root_file).map_err(|cause| {
        error(
            ErrorCode::IntegrityManifest,
            format!("read anchored bundle root: {cause}"),
        )
    })?;
    let mut pending = Vec::new();
    pending.try_reserve(1).map_err(|cause| {
        error(
            ErrorCode::ResourceMemory,
            format!("reserve anchored bundle traversal stack: {cause}"),
        )
    })?;
    pending.push(BundleDirectoryFrame {
        file: root_file,
        entries: root_entries,
        relative: String::new(),
        depth: 0,
        snapshot: root_snapshot,
    });

    let mut output = Vec::new();
    let mut regular_file_count = 0u64;
    let mut directory_count = 0u64;
    let mut hashed_artifact_bytes = 0u64;
    let mut manifest_seen = false;

    while !pending.is_empty() {
        let next_entry = match pending.last_mut() {
            Some(frame) => frame.entries.next(),
            None => break,
        };
        let Some(entry) = next_entry else {
            let frame = pending
                .pop()
                .ok_or_else(|| invariant("bundle traversal stack unexpectedly empty"))?;
            verify_unchanged_descriptor(&frame.file, frame.snapshot, "bundle directory")?;
            continue;
        };
        let entry = entry.map_err(|cause| {
            error(
                ErrorCode::IntegrityManifest,
                format!("read anchored bundle directory entry: {cause}"),
            )
        })?;
        let name_bytes = entry.file_name().to_bytes();
        if matches!(name_bytes, b"." | b"..") {
            continue;
        }
        let name = std::str::from_utf8(name_bytes).map_err(|_| {
            error(
                ErrorCode::IntegrityManifest,
                "bundle entry name is not UTF-8",
            )
        })?;
        let frame = pending
            .last()
            .ok_or_else(|| invariant("bundle traversal stack unexpectedly empty"))?;
        let depth = frame
            .depth
            .checked_add(1)
            .ok_or_else(|| error(ErrorCode::ResourceMemory, "bundle traversal depth overflow"))?;
        if depth > limits.max_path_depth {
            return Err(error(
                ErrorCode::ResourceMemory,
                "bundle entry exceeds the verification path-depth limit",
            ));
        }
        let relative = join_bundle_relative(&frame.relative, name)?;
        if let Some(hook) = before_entry_open.as_deref_mut() {
            hook(&relative);
        }
        let opened = open_bundle_entry(&frame.file, entry.file_name(), &relative)?;
        let metadata = opened.metadata().map_err(|cause| {
            io_error(
                ErrorCode::CommitReopen,
                format!("inspect opened bundle entry {relative:?}"),
                cause,
            )
        })?;
        let snapshot = BundleDescriptorSnapshot::from_metadata(&metadata);

        if metadata.is_dir() {
            directory_count = directory_count.checked_add(1).ok_or_else(|| {
                error(ErrorCode::ResourceMemory, "bundle directory count overflow")
            })?;
            if directory_count > limits.max_directories {
                return Err(error(
                    ErrorCode::ResourceMemory,
                    "bundle exceeds the verification directory-count limit",
                ));
            }
            let entries = rustix::fs::Dir::read_from(&opened).map_err(|cause| {
                error(
                    ErrorCode::IntegrityManifest,
                    format!("read anchored bundle directory {relative:?}: {cause}"),
                )
            })?;
            pending.try_reserve(1).map_err(|cause| {
                error(
                    ErrorCode::ResourceMemory,
                    format!("grow anchored bundle traversal stack: {cause}"),
                )
            })?;
            pending.push(BundleDirectoryFrame {
                file: opened,
                entries,
                relative,
                depth,
                snapshot,
            });
            continue;
        }
        if !metadata.is_file() {
            return Err(error(
                ErrorCode::IntegrityManifest,
                format!("bundle entry is not a regular file: {relative}"),
            ));
        }
        regular_file_count = regular_file_count.checked_add(1).ok_or_else(|| {
            error(
                ErrorCode::ResourceMemory,
                "bundle regular-file count overflow",
            )
        })?;
        if regular_file_count > limits.max_regular_files {
            return Err(error(
                ErrorCode::ResourceMemory,
                "bundle exceeds the verification regular-file-count limit",
            ));
        }

        if relative == "manifest.sha256" {
            if manifest_seen || snapshot != manifest_snapshot {
                return Err(error(
                    ErrorCode::IntegrityManifest,
                    "manifest.sha256 identity or metadata changed during verification",
                ));
            }
            manifest_seen = true;
            continue;
        }

        let remaining = limits
            .max_hashed_artifact_bytes
            .checked_sub(hashed_artifact_bytes)
            .ok_or_else(|| {
                error(
                    ErrorCode::ResourceOutputBytes,
                    "bundle artifacts exceed the verification hash-byte limit",
                )
            })?;
        let (length, digest) = digest_opened_file_limited(opened, snapshot, remaining, &relative)?;
        hashed_artifact_bytes = hashed_artifact_bytes.checked_add(length).ok_or_else(|| {
            error(
                ErrorCode::ResourceOutputBytes,
                "bundle artifact hash-byte count overflow",
            )
        })?;
        output.try_reserve(1).map_err(|cause| {
            error(
                ErrorCode::ResourceMemory,
                format!("grow bounded observed bundle inventory: {cause}"),
            )
        })?;
        output.push(ObservedArtifact {
            path: relative,
            digest,
        });
    }
    if !manifest_seen {
        return Err(error(
            ErrorCode::IntegrityManifest,
            "manifest.sha256 disappeared during verification",
        ));
    }
    Ok(output)
}

fn join_bundle_relative(parent: &str, name: &str) -> Result<String> {
    let separator = usize::from(!parent.is_empty());
    let length = parent
        .len()
        .checked_add(separator)
        .and_then(|value| value.checked_add(name.len()))
        .ok_or_else(|| overflow("bundle relative path length overflow"))?;
    let mut relative = String::new();
    relative.try_reserve_exact(length).map_err(|cause| {
        error(
            ErrorCode::ResourceMemory,
            format!("allocate bounded bundle relative path: {cause}"),
        )
    })?;
    if !parent.is_empty() {
        relative.push_str(parent);
        relative.push('/');
    }
    relative.push_str(name);
    validate_relative_path(&relative)?;
    Ok(relative)
}

fn digest_opened_file_limited(
    mut file: File,
    snapshot: BundleDescriptorSnapshot,
    maximum_bytes: u64,
    label: &str,
) -> Result<(u64, [u8; 32])> {
    if snapshot.byte_len > maximum_bytes {
        return Err(error(
            ErrorCode::ResourceOutputBytes,
            "bundle artifacts exceed the verification hash-byte limit",
        ));
    }
    let mut buffer = [0u8; 64 * 1024];
    let mut length = 0u64;
    let mut hasher = Sha256::new();
    loop {
        let remaining = maximum_bytes.checked_sub(length).ok_or_else(|| {
            error(
                ErrorCode::ResourceOutputBytes,
                "bundle artifacts exceed the verification hash-byte limit",
            )
        })?;
        let request = if remaining >= buffer.len() as u64 {
            buffer.len()
        } else {
            usize::try_from(remaining)
                .map_err(|_| overflow("remaining artifact hash-byte limit does not fit usize"))?
                .checked_add(1)
                .ok_or_else(|| overflow("artifact hash read request overflow"))?
        };
        let read = file.read(&mut buffer[..request]).map_err(|cause| {
            io_error(
                ErrorCode::CommitReopen,
                format!("read opened bundle artifact {label:?}"),
                cause,
            )
        })?;
        if read == 0 {
            break;
        }
        let read_u64 =
            u64::try_from(read).map_err(|_| overflow("artifact read length does not fit u64"))?;
        if read_u64 > remaining {
            return Err(error(
                ErrorCode::ResourceOutputBytes,
                "bundle artifacts exceed the verification hash-byte limit",
            ));
        }
        length = length
            .checked_add(read_u64)
            .ok_or_else(|| overflow("artifact byte total overflow"))?;
        hasher.update(&buffer[..read]);
    }
    if length != snapshot.byte_len {
        return Err(error(
            ErrorCode::IntegrityManifest,
            format!("bundle artifact length changed while hashing: {label}"),
        ));
    }
    verify_unchanged_descriptor(&file, snapshot, "bundle artifact")?;
    Ok((length, hasher.finalize().into()))
}

#[cfg(test)]
fn regular_files(root: &Path) -> Result<BTreeSet<String>> {
    let mut output = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|cause| io_error(ErrorCode::CommitReopen, "read staged directory", cause))?
        {
            let entry = entry
                .map_err(|cause| io_error(ErrorCode::CommitReopen, "read staged entry", cause))?;
            let file_type = entry.file_type().map_err(|cause| {
                io_error(ErrorCode::CommitReopen, "inspect staged entry", cause)
            })?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| invariant("staged path escaped bundle root"))?
                    .to_str()
                    .ok_or_else(|| error(ErrorCode::IntegrityManifest, "staged path is not UTF-8"))?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                output.insert(relative);
            }
        }
    }
    Ok(output)
}

fn normalized_destination(destination: &Path) -> Result<PathBuf> {
    if destination.as_os_str().is_empty()
        || destination
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(error(
            ErrorCode::DestinationUnsafePath,
            "destination must be a nonempty normalized path without . or .. components",
        ));
    }
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            error(
                ErrorCode::DestinationUnsafePath,
                "destination has no UTF-8 name",
            )
        })?;
    if name
        .bytes()
        .any(|byte| matches!(byte, b'\0' | b'\t' | b'\n' | b'\r' | b'\\'))
    {
        return Err(error(
            ErrorCode::DestinationUnsafePath,
            "destination name contains a forbidden byte",
        ));
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new(""));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let canonical_parent = parent.canonicalize().map_err(|cause| {
        io_error(
            ErrorCode::DestinationUnsafePath,
            "canonicalize destination parent",
            cause,
        )
    })?;
    if !canonical_parent.is_dir() {
        return Err(error(
            ErrorCode::DestinationUnsafePath,
            "destination parent is not a directory",
        ));
    }
    Ok(canonical_parent.join(name))
}

fn reject_existing(destination: &Path) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(error(
            ErrorCode::DestinationExisting,
            format!("destination already exists: {destination:?}"),
        )),
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(io_error(
            ErrorCode::DestinationUnsafePath,
            "inspect destination",
            cause,
        )),
    }
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\t' | b'\n' | b'\r' | b'\\'))
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(error(
            ErrorCode::IntegrityManifest,
            format!("unsafe relative artifact path: {path:?}"),
        ));
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<(u64, String)> {
    let file = File::open(path)
        .map_err(|cause| io_error(ErrorCode::CommitReopen, "reopen staged artifact", cause))?;
    let mut reader = BufReader::new(file);
    let mut buffer = [0u8; 64 * 1024];
    let mut length = 0u64;
    let mut hasher = Sha256::new();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|cause| io_error(ErrorCode::CommitReopen, "read staged artifact", cause))?;
        if read == 0 {
            break;
        }
        length = checked_add(
            length,
            u64::try_from(read).map_err(|_| invariant("file read length overflow"))?,
            "artifact byte total",
        )?;
        hasher.update(&buffer[..read]);
    }
    Ok((length, hex_lower(&hasher.finalize())))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|cause| io_error(ErrorCode::CommitSync, "sync staging directory", cause))
}

fn oriented_base(sequence: &[u8], orientation: char, index: usize) -> Result<u8> {
    let base = match orientation {
        '+' => *sequence
            .get(index)
            .ok_or_else(|| invariant("oriented segment index is out of range"))?,
        '-' => {
            let one_past = index
                .checked_add(1)
                .ok_or_else(|| invariant("oriented segment index overflow"))?;
            let source = sequence
                .len()
                .checked_sub(one_past)
                .ok_or_else(|| invariant("oriented segment index is out of range"))?;
            match sequence[source] {
                b'A' => b'T',
                b'C' => b'G',
                b'G' => b'C',
                b'T' => b'A',
                _ => return Err(invariant("invalid base in oriented segment")),
            }
        }
        _ => return Err(invariant("invalid segment orientation")),
    };
    if !matches!(base, b'A' | b'C' | b'G' | b'T') {
        return Err(invariant("invalid base in oriented segment"));
    }
    Ok(base)
}

const fn orientation_rank(orientation: char) -> u8 {
    match orientation {
        '+' => 0,
        '-' => 1,
        _ => 2,
    }
}

const fn flip_orientation(orientation: char) -> char {
    match orientation {
        '+' => '-',
        '-' => '+',
        _ => orientation,
    }
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if !is_lowercase_sha256(value) {
        return Err(invariant(format!("{label} is not lowercase SHA-256")));
    }
    Ok(())
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn checked_add(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or_else(|| invariant(context))
}

fn checked_mul(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| invariant(context))
}

fn json_to_io(cause: serde_json::Error) -> io::Error {
    io::Error::other(cause)
}

fn invariant(context: impl Into<String>) -> VeritasmError {
    error(ErrorCode::InternalInvariant, context)
}

fn error(code: ErrorCode, context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(code, context)
}

fn io_error(code: ErrorCode, context: impl AsRef<str>, cause: io::Error) -> VeritasmError {
    error(code, format!("{}: {cause}", context.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;
    use crate::model::FastxFormat;
    use std::sync::{Arc, Barrier};

    fn unitig(sequence: &[u8], topology: Topology, k: u8, canonical_kmers: u64) -> Unitig {
        let length = u64::try_from(sequence.len()).unwrap();
        let mut identity = Sha256::new();
        identity.update(b"veritasm:unitig:v1\0");
        identity.update([k, topology.tag()]);
        identity.update(length.to_le_bytes());
        identity.update(sequence);
        let closed = topology == Topology::ClosedGraphWalk;
        Unitig {
            id: format!("utg-{}", hex_lower(&identity.finalize())),
            sequence: sequence.to_vec(),
            topology,
            edge_steps: length - u64::from(k) + 1,
            canonical_kmers,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: if closed {
                AvailabilityU64::NotAvailable("closed_walk_audit_unsupported")
            } else {
                AvailabilityU64::Value(1)
            },
            single_group_read_instances: if closed {
                AvailabilityU64::NotAvailable("closed_walk_audit_unsupported")
            } else {
                AvailabilityU64::Value(1)
            },
            multi_group_read_instances_with_group: if closed {
                AvailabilityU64::NotAvailable("closed_walk_audit_unsupported")
            } else {
                AvailabilityU64::Value(0)
            },
            placement_enumeration_status: if closed {
                "unavailable_closed_walk_audit_unsupported"
            } else {
                "placement_enumeration_complete"
            },
            sequence_sha256: sha256_bytes(sequence),
        }
    }

    fn transforms(k: u8, observed: u64, mass: u64) -> Vec<TransformRecord> {
        let support = SupportUnit::SuppliedFragmentInstance;
        let state = "2".repeat(64);
        vec![
            TransformRecord {
                stage_order: 0,
                algorithm_id: "exact_count_observation",
                parameters_json: "{}".to_owned(),
                input_distinct: observed,
                output_distinct: observed,
                input_support_mass: mass,
                output_support_mass: mass,
                removed_key_count: 0,
                removed_support_mass: 0,
                decision_set_sha256: empty_decision_digest(k, support, 0),
                pre_state_sha256: state.clone(),
                post_state_sha256: state.clone(),
                status: "software_stage_complete_no_change",
            },
            TransformRecord {
                stage_order: 1,
                algorithm_id: "absolute_support_retention",
                parameters_json: "{\"retention_min_support_decimal\":\"1\",\"support_unit\":\"supplied_fragment_instance\"}".to_owned(),
                input_distinct: observed,
                output_distinct: observed,
                input_support_mass: mass,
                output_support_mass: mass,
                removed_key_count: 0,
                removed_support_mass: 0,
                decision_set_sha256: empty_decision_digest(k, support, 1),
                pre_state_sha256: state.clone(),
                post_state_sha256: state,
                status: "software_stage_complete_no_change",
            },
        ]
    }

    fn sample(empty: bool, closed: bool) -> BundleData {
        let k = 3;
        let sequence: &[u8] = if empty {
            b"AA"
        } else if closed {
            b"AACAA"
        } else {
            b"AAC"
        };
        let distinct = if empty {
            0
        } else if closed {
            3
        } else {
            1
        };
        let possible = distinct;
        let assembled = if empty {
            Vec::new()
        } else {
            vec![unitig(
                sequence,
                if closed {
                    Topology::ClosedGraphWalk
                } else {
                    Topology::Linear
                },
                k,
                distinct,
            )]
        };
        let graph_links = if closed {
            vec![GraphLink {
                from_segment: assembled[0].id.clone(),
                from_orientation: '+',
                to_segment: assembled[0].id.clone(),
                to_orientation: '+',
            }]
        } else {
            Vec::new()
        };
        let mapped_state = if empty || closed {
            "unmapped"
        } else {
            "single_placement_group"
        };
        let mapper_plan = IndexedExactMapper::plan(&assembled, DEFAULT_SEED_LENGTH).unwrap();
        BundleData {
            input_mode: BundleInputMode::SingleEnd,
            sources: vec![SourceSummary {
                lane_ordinal: 0,
                role: MateRole::S,
                format: FastxFormat::Fasta,
                raw_transport_sha256: "0".repeat(64),
                logical_decoded_sha256: "1".repeat(64),
                records: 1,
                bases: sequence.len() as u64,
            }],
            supplied_fragment_instances: 1,
            supplied_read_instances: 1,
            supplied_bases: sequence.len() as u64,
            raw_transport_bytes: sequence.len() as u64 + 3,
            decoded_input_bytes: sequence.len() as u64 + 3,
            inferred_mate_roles: 0,
            gzip_sources: 0,
            gzip_members: 0,
            spool_sha256: "3".repeat(64),
            spool_bytes: 64,
            scientific: ScientificConfig {
                k,
                profile: Profile::RetainAll,
                support_unit: SupportUnit::SuppliedFragmentInstance,
                min_support: 1,
                min_base_quality: 20,
                remap: true,
            },
            limits: Limits::default(),
            windows: WindowStats {
                possible,
                accepted: possible,
                ambiguity_only: 0,
                quality_only: 0,
                ambiguity_and_quality: 0,
            },
            observed_distinct_canonical_keys: distinct,
            observed_support_mass: distinct,
            retained_distinct_canonical_keys: distinct,
            retained_support_mass: distinct,
            support_histogram: if empty {
                Vec::new()
            } else {
                vec![SupportHistogramBin {
                    support: 1,
                    distinct_canonical_keys: distinct,
                }]
            },
            oriented_handles: distinct * 2,
            self_reverse_complement_keys: 0,
            unitigs: assembled,
            graph_links,
            mapper: MapperDescriptor {
                algorithm_id: INDEXED_MAPPER_ID,
                algorithm_version: INDEXED_MAPPER_VERSION,
                target_universe: INDEXED_TARGET_UNIVERSE,
                seed_length: 15,
                execution_status: "executed",
                linear_targets: u64::from(!empty && !closed),
                index_postings: 0,
                accounted_index_bytes: mapper_plan.resident_bound_bytes(),
            },
            read_state_counts: [
                "ineligible_ambiguity_or_quality",
                "indeterminate_candidate_limit",
                "unmapped",
                "single_placement_group",
                "multiple_placement_groups",
            ]
            .into_iter()
            .map(|state| StateCount {
                role: Some(MateRole::S),
                state,
                count: u64::from(state == mapped_state),
            })
            .collect(),
            pair_state_counts: vec![StateCount {
                role: None,
                state: "not_paired_input",
                count: 1,
            }],
            pair_lane_state_counts: vec![PairLaneStateCount {
                lane_ordinal: 0,
                state: "not_paired_input",
                count: 1,
            }],
            pair_links: Vec::new(),
            transformations: transforms(k, distinct, distinct),
        }
    }

    fn two_key_sample(
        bins: &[(u64, u64)],
        minimum_support: u64,
        lower_median_support: u64,
        maximum_support: u64,
    ) -> BundleData {
        let mut data = sample(false, false);
        let histogram = bins
            .iter()
            .map(|&(support, distinct_canonical_keys)| SupportHistogramBin {
                support,
                distinct_canonical_keys,
            })
            .collect::<Vec<_>>();
        let distinct = histogram.iter().fold(0u64, |total, bin| {
            total.checked_add(bin.distinct_canonical_keys).unwrap()
        });
        let mass = histogram.iter().fold(0u64, |total, bin| {
            total
                .checked_add(
                    bin.support
                        .checked_mul(bin.distinct_canonical_keys)
                        .unwrap(),
                )
                .unwrap()
        });
        assert_eq!(distinct, 2);

        data.sources[0].records = 3;
        data.sources[0].bases = 12;
        data.supplied_fragment_instances = 3;
        data.supplied_read_instances = 3;
        data.supplied_bases = 12;
        data.raw_transport_bytes = 15;
        data.decoded_input_bytes = 15;
        data.windows = WindowStats {
            possible: 6,
            accepted: 6,
            ambiguity_only: 0,
            quality_only: 0,
            ambiguity_and_quality: 0,
        };
        data.observed_distinct_canonical_keys = distinct;
        data.observed_support_mass = mass;
        data.retained_distinct_canonical_keys = distinct;
        data.retained_support_mass = mass;
        data.support_histogram = histogram;
        data.oriented_handles = distinct * 2;
        let mut assembled = unitig(b"AACG", Topology::Linear, 3, distinct);
        assembled.minimum_support = minimum_support;
        assembled.lower_median_support = lower_median_support;
        assembled.maximum_support = maximum_support;
        assembled.enumeration_complete_read_placements = AvailabilityU64::Value(3);
        assembled.single_group_read_instances = AvailabilityU64::Value(3);
        data.unitigs = vec![assembled];
        for state in &mut data.read_state_counts {
            state.count = u64::from(state.state == "single_placement_group") * 3;
        }
        data.pair_state_counts[0].count = 3;
        data.pair_lane_state_counts[0].count = 3;
        data.transformations = transforms(3, distinct, mass);
        data
    }

    fn complete_audit_with_single_and_multiple_reads() -> BundleData {
        let mut data = sample(false, false);
        data.sources[0].records = 2;
        data.sources[0].bases = 6;
        data.supplied_fragment_instances = 2;
        data.supplied_read_instances = 2;
        data.supplied_bases = 6;
        data.raw_transport_bytes = 9;
        data.decoded_input_bytes = 9;
        data.windows.possible = 2;
        data.windows.accepted = 2;
        for state in &mut data.read_state_counts {
            state.count = u64::from(matches!(
                state.state,
                "single_placement_group" | "multiple_placement_groups"
            ));
        }
        data.pair_state_counts[0].count = 2;
        data.pair_lane_state_counts[0].count = 2;
        data.unitigs[0].enumeration_complete_read_placements = AvailabilityU64::Value(3);
        data.unitigs[0].single_group_read_instances = AvailabilityU64::Value(1);
        data.unitigs[0].multi_group_read_instances_with_group = AvailabilityU64::Value(1);
        data
    }

    fn pair_validation_sample(lane_fragments: &[u64], lane_cross: &[u64]) -> BundleData {
        assert_eq!(lane_fragments.len(), lane_cross.len());
        let mut data = sample(false, false);
        data.input_mode = BundleInputMode::PairedEnd;
        data.sources = lane_fragments
            .iter()
            .enumerate()
            .flat_map(|(lane, &fragments)| {
                [MateRole::R1, MateRole::R2]
                    .into_iter()
                    .map(move |role| SourceSummary {
                        lane_ordinal: u32::try_from(lane).unwrap(),
                        role,
                        format: FastxFormat::Fasta,
                        raw_transport_sha256: "0".repeat(64),
                        logical_decoded_sha256: "1".repeat(64),
                        records: fragments,
                        bases: fragments.checked_mul(3).unwrap(),
                    })
            })
            .collect();
        data.supplied_fragment_instances = lane_fragments.iter().sum();
        data.supplied_read_instances = data.supplied_fragment_instances.checked_mul(2).unwrap();
        data.supplied_bases = data.supplied_read_instances.checked_mul(3).unwrap();
        let states = [
            "mate_ineligible",
            "mate_indeterminate_candidate_limit",
            "mate_unmapped",
            "mate_multiple_placement_groups",
            "same_linear_unitig",
            "endpoint_tie",
            "cross_unitig_observation",
        ];
        let cross_total = lane_cross.iter().sum::<u64>();
        data.pair_state_counts = states
            .into_iter()
            .enumerate()
            .map(|(index, state)| StateCount {
                role: None,
                state,
                count: if index == 2 {
                    data.supplied_fragment_instances - cross_total
                } else if index == 6 {
                    cross_total
                } else {
                    0
                },
            })
            .collect();
        data.pair_lane_state_counts = lane_fragments
            .iter()
            .zip(lane_cross)
            .enumerate()
            .flat_map(|(lane, (&fragments, &cross))| {
                states
                    .into_iter()
                    .enumerate()
                    .map(move |(index, state)| PairLaneStateCount {
                        lane_ordinal: u32::try_from(lane).unwrap(),
                        state,
                        count: if index == 2 {
                            fragments - cross
                        } else if index == 6 {
                            cross
                        } else {
                            0
                        },
                    })
            })
            .collect();
        data.pair_links.clear();
        data
    }

    fn pair_link_between(
        first: &Unitig,
        second: &Unitig,
        lane_ordinal: u32,
        support: u64,
    ) -> PairLink {
        let mut ordered = [first, second];
        ordered.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        PairLink {
            lane_ordinal,
            a: PairEndpoint {
                segment: ordered[0].id.clone(),
                end: 'L',
                strand: '+',
                end_distance: 0,
                mate_role: MateRole::R1,
            },
            b: PairEndpoint {
                segment: ordered[1].id.clone(),
                end: 'R',
                strand: '-',
                end_distance: 0,
                mate_role: MateRole::R2,
            },
            supplied_fragment_instances: support,
        }
    }

    fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        regular_files(root)
            .unwrap()
            .into_iter()
            .map(|path| (path.clone(), fs::read(root.join(path)).unwrap()))
            .collect()
    }

    fn restore_snapshot(root: &Path, files: &BTreeMap<String, Vec<u8>>) {
        fs::create_dir(root).unwrap();
        for (relative, bytes) in files {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, bytes).unwrap();
        }
    }

    fn manifest_fixture(root: &Path, artifacts: &[(String, Vec<u8>)]) -> BundleManifestLimits {
        fs::create_dir(root).unwrap();
        let mut digests = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let mut max_path_depth = 1u32;
        let mut hashed_artifact_bytes = 0u64;
        for (relative, bytes) in artifacts {
            let components = relative.split('/').collect::<Vec<_>>();
            assert!(components.iter().all(|component| !component.is_empty()));
            max_path_depth = max_path_depth.max(u32::try_from(components.len()).unwrap());
            for end in 1..components.len() {
                directories.insert(components[..end].join("/"));
            }
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            digests.insert(relative.clone(), sha256_bytes(bytes));
            hashed_artifact_bytes = hashed_artifact_bytes
                .checked_add(u64::try_from(bytes.len()).unwrap())
                .unwrap();
        }
        let manifest = render_manifest(&digests);
        fs::write(root.join("manifest.sha256"), &manifest).unwrap();
        BundleManifestLimits {
            max_manifest_bytes: u64::try_from(manifest.len()).unwrap(),
            max_regular_files: u64::try_from(artifacts.len()).unwrap() + 1,
            max_directories: u64::try_from(directories.len()).unwrap(),
            max_path_depth,
            max_hashed_artifact_bytes: hashed_artifact_bytes,
        }
    }

    #[test]
    fn empty_and_closed_only_bundles_follow_contract() {
        for (empty, closed) in [(true, false), (false, true)] {
            let parent = tempfile::tempdir().unwrap();
            let destination = parent.path().join("result");
            write_bundle(&destination, &sample(empty, closed)).unwrap();
            verify_manifest(&destination).unwrap();
            if empty {
                assert_eq!(
                    fs::metadata(destination.join("unitigs.fasta"))
                        .unwrap()
                        .len(),
                    0
                );
                assert_eq!(
                    fs::read_to_string(destination.join("assembly.gfa")).unwrap(),
                    format!(
                        "H\tVN:Z:1.0\tPN:Z:veritasm\tPV:Z:{}\tSC:Z:{}\n",
                        env!("CARGO_PKG_VERSION"),
                        ASSEMBLY_GFA_SCHEMA_VERSION
                    )
                );
                assert_eq!(
                    fs::read_to_string(destination.join("unitig_evidence.tsv")).unwrap(),
                    UNITIG_HEADER
                );
                assert_eq!(
                    fs::read_to_string(destination.join("pair_links.tsv")).unwrap(),
                    PAIR_LINK_HEADER
                );
                assert_eq!(
                    fs::read_to_string(destination.join("pair_audit_summary.tsv")).unwrap(),
                    format!("{PAIR_SUMMARY_HEADER}1.0\t3\tnot_paired_input\t1\n")
                );
            } else {
                let gfa = fs::read_to_string(destination.join("assembly.gfa")).unwrap();
                assert!(gfa.lines().any(|line| line.starts_with("L\t")));
                let evidence = fs::read_to_string(destination.join("unitig_evidence.tsv")).unwrap();
                assert!(
                    evidence.contains("\tNA\tNA\tNA\tunavailable_closed_walk_audit_unsupported\t")
                );
            }
        }
    }

    #[test]
    fn complete_bundle_bytes_are_deterministic() {
        let parent = tempfile::tempdir().unwrap();
        let first = parent.path().join("first");
        let second = parent.path().join("second");
        let data = sample(false, false);
        write_bundle(&first, &data).unwrap();
        write_bundle(&second, &data).unwrap();
        assert_eq!(snapshot(&first), snapshot(&second));
    }

    #[test]
    fn streaming_gfa_validator_compares_every_ordered_record_byte() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("assembly.gfa");
        let data = sample(false, true);
        let prepared = Prepared::new(&data).unwrap();
        let mut expected = Vec::new();
        prepared.write_gfa(&mut expected).unwrap();
        fs::write(&path, &expected).unwrap();
        validate_gfa(&path, &prepared).unwrap();

        let expected_text = std::str::from_utf8(&expected).unwrap();
        for (from, to) in [
            ("H\t", "X\t"),
            ("VN:Z:1.0", "VN:Z:2.0"),
            ("PN:Z:veritasm", "PN:Z:other"),
            ("SC:Z:1.1", "SC:Z:2.0"),
            ("\tAACAA\t", "\tAATAA\t"),
            ("TP:Z:closed_graph_walk", "TP:Z:linear"),
            ("ES:i:3", "ES:i:4"),
            ("CK:i:3", "CK:i:2"),
            ("SH:H:", "SH:Z:"),
            ("\t+\t", "\t-\t"),
            ("\t2M\n", "\t1M\n"),
        ] {
            let corrupted = expected_text.replacen(from, to, 1);
            assert_ne!(
                corrupted, expected_text,
                "test mutation must change the artifact"
            );
            fs::write(&path, corrupted).unwrap();
            assert_eq!(
                validate_gfa(&path, &prepared).unwrap_err().code(),
                ErrorCode::IntegrityArtifact
            );
        }

        let id = &prepared.unitigs[0].id;
        let invalid_id = format!("utg-{}", "0".repeat(64));
        let link = format!("L\t{id}\t+\t{id}\t+\t2M\n");
        for (from, to) in [
            (
                format!("PV:Z:{}", env!("CARGO_PKG_VERSION")),
                "PV:Z:invalid".to_owned(),
            ),
            (format!("S\t{id}\t"), format!("X\t{id}\t")),
            (format!("S\t{id}\t"), format!("S\t{invalid_id}\t")),
            (link.clone(), format!("X\t{id}\t+\t{id}\t+\t2M\n")),
            (link.clone(), format!("L\t{invalid_id}\t+\t{id}\t+\t2M\n")),
            (link.clone(), format!("L\t{id}\t+\t{invalid_id}\t+\t2M\n")),
            (link.clone(), format!("L\t{id}\t+\t{id}\t-\t2M\n")),
        ] {
            let corrupted = expected_text.replacen(from.as_str(), to.as_str(), 1);
            assert_ne!(
                corrupted, expected_text,
                "test mutation must change the artifact"
            );
            fs::write(&path, corrupted).unwrap();
            assert_eq!(
                validate_gfa(&path, &prepared).unwrap_err().code(),
                ErrorCode::IntegrityArtifact
            );
        }

        let records = expected_text.split_inclusive('\n').collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        fs::write(&path, format!("{}{}{}", records[0], records[2], records[1])).unwrap();
        assert_eq!(
            validate_gfa(&path, &prepared).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        let mut trailing = expected;
        trailing.extend_from_slice(b"H\tVN:Z:1.0\n");
        fs::write(&path, trailing).unwrap();
        assert_eq!(
            validate_gfa(&path, &prepared).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn typed_streaming_validators_reject_mutated_tsv_json_and_html() {
        let parent = tempfile::tempdir().unwrap();
        let data = sample(false, false);
        let prepared = Prepared::new(&data).unwrap();

        let tsv_path = parent.path().join("unitig_evidence.tsv");
        let mut tsv = Vec::new();
        prepared.write_unitig_evidence(&mut tsv).unwrap();
        fs::write(&tsv_path, &tsv).unwrap();
        validate_rendered_artifact(&tsv_path, "unitig_evidence.tsv", |writer| {
            prepared.write_unitig_evidence(writer)
        })
        .unwrap();
        tsv[0] = b'2';
        fs::write(&tsv_path, tsv).unwrap();
        assert_eq!(
            validate_rendered_artifact(&tsv_path, "unitig_evidence.tsv", |writer| {
                prepared.write_unitig_evidence(writer)
            })
            .unwrap_err()
            .code(),
            ErrorCode::IntegrityArtifact
        );

        let digest = "4".repeat(64);
        let run_path = parent.path().join("run.json");
        let run = prepared.run_document(&digest);
        let mut run_bytes = Vec::new();
        serde_json::to_writer_pretty(&mut run_bytes, &run).unwrap();
        run_bytes.push(b'\n');
        let run_text = std::str::from_utf8(&run_bytes).unwrap();
        for expected in [
            "\"raw_transport_bytes_decimal\": \"6\"",
            "\"decoded_input_bytes_decimal\": \"6\"",
            "\"inferred_mate_roles_decimal\": \"0\"",
            "\"gzip_sources_decimal\": \"0\"",
            "\"gzip_members_decimal\": \"0\"",
            "\"max_raw_transport_bytes_decimal\": \"1099511627776\"",
            "\"max_gzip_members_decimal\": \"1000000\"",
            "\"algorithm_id\": \"literal_rarest_seed_zero_mismatch\"",
            "\"seed_length\": 15",
            "\"execution_status\": \"executed\"",
            "\"linear_targets_decimal\": \"1\"",
            "\"index_postings_decimal\": \"0\"",
        ] {
            assert!(run_text.contains(expected));
        }
        fs::write(&run_path, &run_bytes).unwrap();
        validate_run_stream(&run_path, &run).unwrap();
        let first_digit = run_bytes.iter().position(|byte| *byte == b'1').unwrap();
        run_bytes[first_digit] = b'2';
        fs::write(&run_path, run_bytes).unwrap();
        assert_eq!(
            validate_run_stream(&run_path, &run).unwrap_err().code(),
            ErrorCode::IntegrityArtifact
        );

        let report_path = parent.path().join("report.html");
        let report_data = prepared.report_data();
        let mut report_bytes = Vec::new();
        report::write_html(&mut report_bytes, &report_data).unwrap();
        fs::write(&report_path, &report_bytes).unwrap();
        validate_report_stream(&report_path, &report_data).unwrap();
        report_bytes[0] = b'X';
        fs::write(&report_path, report_bytes).unwrap();
        assert_eq!(
            validate_report_stream(&report_path, &report_data)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityArtifact
        );
    }

    #[test]
    fn bundle_memory_admission_counts_live_owned_capacities_before_output() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let mut data = sample(false, false);
        data.limits.memory_budget_bytes = 32 << 20;
        let target_capacity = (40usize << 20) / std::mem::size_of::<Unitig>();
        data.unitigs
            .try_reserve_exact(target_capacity.saturating_sub(data.unitigs.len()))
            .unwrap();

        let failure = write_bundle(&destination, &data).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::ResourceMemory);
        assert!(!destination.exists());
    }

    #[test]
    fn mapper_descriptor_must_reconcile_to_the_indexed_target_instance() {
        let valid = sample(false, false);
        validate_mapper(&valid).unwrap();

        let mut wrong_algorithm = valid.clone();
        wrong_algorithm.mapper.algorithm_id = "exhaustive_zero_mismatch";
        assert_eq!(
            validate_mapper(&wrong_algorithm).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );

        let mut wrong_seed = valid.clone();
        wrong_seed.mapper.seed_length = 14;
        assert_eq!(
            validate_mapper(&wrong_seed).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );

        let mut wrong_target_count = valid.clone();
        wrong_target_count.mapper.linear_targets = 2;
        assert_eq!(
            validate_mapper(&wrong_target_count).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );

        let mut under_accounted = valid;
        under_accounted.mapper.accounted_index_bytes -= 1;
        assert_eq!(
            validate_mapper(&under_accounted).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );

        let mut over_budget = sample(false, false);
        over_budget.mapper.accounted_index_bytes = u64::MAX;
        assert_eq!(
            validate_mapper(&over_budget).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn pair_link_identity_excludes_support_from_uniqueness_key() {
        let first = unitig(b"AAC", Topology::Linear, 3, 1);
        let second = unitig(b"AAG", Topology::Linear, 3, 1);
        let link = pair_link_between(&first, &second, 0, 1);
        let mut duplicate = link.clone();
        duplicate.supplied_fragment_instances = 2;
        let unitigs = [first, second];
        let index = UnitigIndex::new(&unitigs).unwrap();
        let mut data = pair_validation_sample(&[3], &[3]);
        data.pair_links = vec![link, duplicate];

        assert_eq!(
            validate_pair_links(&data, &index, 1).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn pair_links_are_validated_and_reconciled_per_catalogued_lane() {
        let first = unitig(b"AAC", Topology::Linear, 3, 1);
        let second = unitig(b"AAG", Topology::Linear, 3, 1);
        let unitigs = [first, second];
        let index = UnitigIndex::new(&unitigs).unwrap();
        let mut valid = pair_validation_sample(&[1, 1], &[1, 1]);
        valid.pair_links = vec![
            pair_link_between(&unitigs[0], &unitigs[1], 0, 1),
            pair_link_between(&unitigs[0], &unitigs[1], 1, 1),
        ];
        let lanes = validate_sources(&valid).unwrap();
        assert_eq!(lanes, 2);
        validate_pair_states(&valid, lanes).unwrap();
        validate_pair_links(&valid, &index, lanes).unwrap();

        let mut pooled_into_lane_zero = valid.clone();
        pooled_into_lane_zero.pair_links = vec![pair_link_between(&unitigs[0], &unitigs[1], 0, 2)];
        assert_eq!(
            validate_pair_links(&pooled_into_lane_zero, &index, lanes)
                .unwrap_err()
                .code(),
            ErrorCode::InternalInvariant
        );

        let mut absent_lane = pair_validation_sample(&[1], &[1]);
        absent_lane.pair_links = vec![pair_link_between(&unitigs[0], &unitigs[1], 7, 1)];
        assert_eq!(
            validate_pair_links(&absent_lane, &index, 1)
                .unwrap_err()
                .code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn pair_lane_summary_requires_every_zero_state_in_exact_order() {
        let mut missing_zero = pair_validation_sample(&[1, 1], &[1, 1]);
        missing_zero.pair_lane_state_counts.remove(0);
        assert_eq!(
            validate_pair_states(&missing_zero, 2).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );

        let mut wrong_global = pair_validation_sample(&[1, 1], &[1, 1]);
        wrong_global.pair_state_counts[6].count = 1;
        wrong_global.pair_state_counts[2].count = 1;
        assert_eq!(
            validate_pair_states(&wrong_global, 2).unwrap_err().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn complete_audit_rejects_unreconciled_placement_and_membership_totals() {
        let mut extra_placement = sample(false, false);
        extra_placement.unitigs[0].enumeration_complete_read_placements = AvailabilityU64::Value(2);
        assert_eq!(
            Prepared::new(&extra_placement).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );

        let mut extra_membership = sample(false, false);
        extra_membership.unitigs[0].multi_group_read_instances_with_group =
            AvailabilityU64::Value(1);
        assert_eq!(
            Prepared::new(&extra_membership).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn complete_unitig_audit_reconciles_placements_and_memberships_per_unitig() {
        let valid = complete_audit_with_single_and_multiple_reads();
        Prepared::new(&valid).unwrap();

        let mut too_few_placements = valid.clone();
        too_few_placements.unitigs[0].enumeration_complete_read_placements =
            AvailabilityU64::Value(1);
        assert_eq!(
            Prepared::new(&too_few_placements).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );

        let mut too_many_multiple_memberships = valid;
        too_many_multiple_memberships.unitigs[0].multi_group_read_instances_with_group =
            AvailabilityU64::Value(2);
        assert_eq!(
            Prepared::new(&too_many_multiple_memberships)
                .err()
                .unwrap()
                .code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn no_op_retention_cannot_change_the_state_digest() {
        let mut data = sample(false, false);
        data.transformations[1].post_state_sha256 = "4".repeat(64);

        assert_eq!(
            Prepared::new(&data).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn unitig_backing_keys_must_equal_edge_steps() {
        let mut data = sample(false, false);
        data.unitigs[0].canonical_kmers = data.unitigs[0].edge_steps + 1;

        assert_eq!(
            Prepared::new(&data).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );

        let mut data = sample(false, false);
        data.unitigs[0].edge_steps = data.unitigs[0].canonical_kmers + 1;
        data.unitigs[0].sequence.push(b'A');
        data.unitigs[0].sequence_sha256 = sha256_bytes(&data.unitigs[0].sequence);
        let length = u64::try_from(data.unitigs[0].sequence.len()).unwrap();
        let mut identity = Sha256::new();
        identity.update(b"veritasm:unitig:v1\0");
        identity.update([data.scientific.k, data.unitigs[0].topology.tag()]);
        identity.update(length.to_le_bytes());
        identity.update(&data.unitigs[0].sequence);
        data.unitigs[0].id = format!("utg-{}", hex_lower(&identity.finalize()));

        assert_eq!(
            Prepared::new(&data).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn every_unitig_support_statistic_must_exist_in_the_retained_histogram() {
        let valid = two_key_sample(&[(1, 1), (3, 1)], 1, 1, 3);
        Prepared::new(&valid).unwrap();

        for (minimum, median, maximum) in [(2, 3, 3), (1, 2, 3), (1, 1, 2)] {
            let invalid = two_key_sample(&[(1, 1), (3, 1)], minimum, median, maximum);
            assert_eq!(
                Prepared::new(&invalid).err().unwrap().code(),
                ErrorCode::InternalInvariant
            );
        }
    }

    #[test]
    fn retained_support_mass_must_fit_conservative_unitig_bounds() {
        let below_minimum = two_key_sample(&[(1, 1), (3, 1)], 3, 3, 3);
        assert_eq!(
            Prepared::new(&below_minimum).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );

        let above_maximum = two_key_sample(&[(1, 1), (3, 1)], 1, 1, 1);
        assert_eq!(
            Prepared::new(&above_maximum).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn unitig_support_mass_bounds_are_inclusive() {
        for support in [1, 3] {
            let data = two_key_sample(&[(support, 2)], support, support, support);
            Prepared::new(&data).unwrap();
        }
    }

    #[test]
    fn unitig_support_mass_arithmetic_is_checked() {
        let mut data = two_key_sample(
            &[(1, 1), (u64::MAX - 1, 1)],
            u64::MAX - 1,
            u64::MAX - 1,
            u64::MAX - 1,
        );
        data.windows.possible = u64::MAX;
        data.windows.accepted = u64::MAX;
        validate_histogram(&data).unwrap();

        assert_eq!(
            validate_unitigs(&data).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn stage_enforces_output_and_aggregate_temporary_byte_limits() {
        let parent = tempfile::tempdir().unwrap();
        assert_eq!(
            Stage::new(parent.path(), 100, 3, 4).err().unwrap().code(),
            ErrorCode::ResourceTemporaryBytes
        );

        for (maximum, succeeds) in [(3, false), (4, true), (5, true)] {
            let output_root = parent.path().join(format!("output-limit-{maximum}"));
            fs::create_dir(&output_root).unwrap();
            let mut stage = Stage::new(&output_root, maximum, 100, 0).unwrap();
            let result = stage.generate("artifact", |writer| writer.write_all(b"1234"));
            assert_eq!(result.is_ok(), succeeds, "output limit {maximum}");
            if let Err(error) = result {
                assert_eq!(error.code(), ErrorCode::ResourceOutputBytes);
            }
        }

        for (maximum, succeeds) in [(3, false), (4, true), (5, true)] {
            let aggregate_root = parent.path().join(format!("aggregate-limit-{maximum}"));
            fs::create_dir(&aggregate_root).unwrap();
            let mut stage = Stage::new(&aggregate_root, 100, maximum, 2).unwrap();
            let result = stage.generate("artifact", |writer| writer.write_all(b"12"));
            assert_eq!(result.is_ok(), succeeds, "aggregate limit {maximum}");
            if let Err(error) = result {
                assert_eq!(error.code(), ErrorCode::ResourceTemporaryBytes);
            }
        }
    }

    #[test]
    fn historical_spool_bytes_are_not_charged_after_phase_drop() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let mut data = sample(false, false);
        data.limits.max_temp_bytes = 1 << 20;
        data.spool_bytes = data.limits.max_temp_bytes;

        write_bundle(&destination, &data).unwrap();
        verify_manifest(&destination).unwrap();
    }

    #[test]
    fn spool_byte_telemetry_obeys_both_limits_inclusively() {
        const BOUNDARY: u64 = 1 << 20;

        let mut exact_boundary = sample(false, false);
        exact_boundary.limits.max_spool_bytes = BOUNDARY;
        exact_boundary.limits.max_temp_bytes = BOUNDARY;
        exact_boundary.spool_bytes = BOUNDARY;
        Prepared::new(&exact_boundary).unwrap();

        let mut empty = exact_boundary.clone();
        empty.spool_bytes = 0;
        assert_eq!(
            Prepared::new(&empty).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );

        let mut over_spool = exact_boundary.clone();
        over_spool.limits.max_temp_bytes = BOUNDARY + 1;
        over_spool.spool_bytes = BOUNDARY + 1;
        assert_eq!(
            Prepared::new(&over_spool).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );

        let mut over_temp = exact_boundary;
        over_temp.limits.max_spool_bytes = BOUNDARY + 1;
        over_temp.spool_bytes = BOUNDARY + 1;
        assert_eq!(
            Prepared::new(&over_temp).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn transport_telemetry_obeys_effective_limits_inclusively() {
        const RAW_BOUNDARY: u64 = 1 << 20;

        let mut exact_boundary = sample(false, false);
        exact_boundary.limits.max_raw_transport_bytes = RAW_BOUNDARY;
        exact_boundary.raw_transport_bytes = RAW_BOUNDARY;
        exact_boundary.limits.max_gzip_members = 1;
        exact_boundary.gzip_sources = 1;
        exact_boundary.gzip_members = 1;
        Prepared::new(&exact_boundary).unwrap();

        let mut raw_over = exact_boundary.clone();
        raw_over.raw_transport_bytes = RAW_BOUNDARY + 1;
        assert_eq!(
            Prepared::new(&raw_over).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );

        let mut members_over = exact_boundary;
        members_over.gzip_members = 2;
        assert_eq!(
            Prepared::new(&members_over).err().unwrap().code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn existing_destination_is_never_modified() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"keep").unwrap();
        let error = write_bundle(&destination, &sample(false, false)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::DestinationExisting);
        assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"keep");
    }

    #[test]
    fn bundle_rejects_a_manually_invalid_scientific_config_before_output() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let mut data = sample(false, false);
        data.scientific.k = 2;

        let error = write_bundle(&destination, &data).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigurationInvalidK);
        assert!(!destination.exists());
    }

    #[test]
    fn pair_endpoint_distance_must_be_inside_its_linear_segment() {
        let target = unitig(b"AAC", Topology::Linear, 3, 1);
        let mut endpoint = PairEndpoint {
            segment: target.id.clone(),
            end: 'L',
            strand: '+',
            end_distance: 2,
            mate_role: MateRole::R1,
        };
        let unitigs = UnitigIndex::new(std::slice::from_ref(&target)).unwrap();
        validate_pair_endpoint(&endpoint, &unitigs).unwrap();

        endpoint.end_distance = 3;
        assert_eq!(
            validate_pair_endpoint(&endpoint, &unitigs)
                .unwrap_err()
                .code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn every_injected_precommit_failure_leaves_no_result() {
        let mut points = vec![
            FailPoint::AfterLock,
            FailPoint::AfterStagingDirectory,
            FailPoint::AfterSchemaDirectory,
            FailPoint::AfterFasta,
            FailPoint::AfterGfa,
            FailPoint::AfterUnitigEvidence,
            FailPoint::AfterPairLinks,
            FailPoint::AfterPairSummary,
            FailPoint::AfterTransforms,
            FailPoint::AfterRun,
            FailPoint::AfterReport,
            FailPoint::AfterArtifactValidation,
            FailPoint::AfterManifest,
            FailPoint::AfterManifestVerification,
            FailPoint::AfterSchemaDirectorySync,
            FailPoint::AfterStagingDirectorySync,
            FailPoint::BeforeCommit,
        ];
        points.extend((0..SCHEMAS.len()).map(|index| FailPoint::AfterSchema(index as u8)));

        for point in points {
            let parent = tempfile::tempdir().unwrap();
            let destination = parent.path().join("result");
            let error =
                write_bundle_inner(&destination, &sample(false, false), Some(point)).unwrap_err();
            assert_eq!(error.code(), ErrorCode::CommitValidate, "at {point:?}");
            assert!(!destination.exists());
            assert_eq!(
                fs::read_dir(parent.path()).unwrap().count(),
                0,
                "at {point:?}"
            );
        }
    }

    #[test]
    fn every_injected_lock_initialization_failure_cleans_the_owned_lock() {
        for (point, expected_code) in [
            (LockFailPoint::Create, ErrorCode::CommitWrite),
            (LockFailPoint::Write, ErrorCode::CommitFlush),
            (LockFailPoint::Flush, ErrorCode::CommitSync),
            (LockFailPoint::Sync, ErrorCode::CommitSync),
        ] {
            let parent = tempfile::tempdir().unwrap();
            let destination = parent.path().join("result");
            let error = LockGuard::acquire_inner(&destination, Some(point)).unwrap_err();
            assert_eq!(error.code(), expected_code, "at {point:?}");
            assert_eq!(
                fs::read_dir(parent.path()).unwrap().count(),
                0,
                "at {point:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn dangling_destination_symlink_is_treated_as_an_existing_result() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        symlink(parent.path().join("missing-target"), &destination).unwrap();

        let error = write_bundle(&destination, &sample(false, false)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::DestinationExisting);
        assert!(fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn public_manifest_verifier_rejects_inventory_and_manifest_mutations() {
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        write_bundle(&original, &sample(false, false)).unwrap();
        verify_bundle_manifest(&original).unwrap();
        let files = snapshot(&original);

        let modified = parent.path().join("modified");
        restore_snapshot(&modified, &files);
        fs::write(modified.join("unitigs.fasta"), b"changed\n").unwrap();
        assert_eq!(
            verify_bundle_manifest(&modified).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        let missing = parent.path().join("missing");
        restore_snapshot(&missing, &files);
        fs::remove_file(missing.join("report.html")).unwrap();
        assert_eq!(
            verify_bundle_manifest(&missing).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        let unlisted = parent.path().join("unlisted");
        restore_snapshot(&unlisted, &files);
        fs::write(unlisted.join("unexpected"), b"not listed").unwrap();
        assert_eq!(
            verify_bundle_manifest(&unlisted).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        let duplicated = parent.path().join("duplicated");
        restore_snapshot(&duplicated, &files);
        let manifest = fs::read_to_string(duplicated.join("manifest.sha256")).unwrap();
        let first = manifest.lines().next().unwrap();
        fs::write(
            duplicated.join("manifest.sha256"),
            format!("{manifest}{first}\n"),
        )
        .unwrap();
        assert_eq!(
            verify_bundle_manifest(&duplicated).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        let unsorted = parent.path().join("unsorted");
        restore_snapshot(&unsorted, &files);
        let manifest = fs::read_to_string(unsorted.join("manifest.sha256")).unwrap();
        let mut lines = manifest.lines().collect::<Vec<_>>();
        lines.swap(0, 1);
        fs::write(
            unsorted.join("manifest.sha256"),
            format!("{}\n", lines.join("\n")),
        )
        .unwrap();
        assert_eq!(
            verify_bundle_manifest(&unsorted).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        let no_lf = parent.path().join("no-final-lf");
        restore_snapshot(&no_lf, &files);
        let mut manifest = fs::read(no_lf.join("manifest.sha256")).unwrap();
        assert_eq!(manifest.pop(), Some(b'\n'));
        fs::write(no_lf.join("manifest.sha256"), manifest).unwrap();
        assert_eq!(
            verify_bundle_manifest(&no_lf).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        let invalid_digest = parent.path().join("invalid-digest");
        restore_snapshot(&invalid_digest, &files);
        let manifest = fs::read_to_string(invalid_digest.join("manifest.sha256")).unwrap();
        let (_, rest) = manifest.split_at(64);
        fs::write(
            invalid_digest.join("manifest.sha256"),
            format!("{}{rest}", "x".repeat(64)),
        )
        .unwrap();
        assert_eq!(
            verify_bundle_manifest(&invalid_digest).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        let unsafe_path = parent.path().join("unsafe-path");
        restore_snapshot(&unsafe_path, &files);
        let manifest = fs::read_to_string(unsafe_path.join("manifest.sha256")).unwrap();
        let (first, rest) = manifest.split_once('\n').unwrap();
        let (digest, _) = first.split_once("  ").unwrap();
        fs::write(
            unsafe_path.join("manifest.sha256"),
            format!("{digest}  ../escape\n{rest}"),
        )
        .unwrap();
        assert_eq!(
            verify_bundle_manifest(&unsafe_path).unwrap_err().code(),
            ErrorCode::IntegrityManifest
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let symlinked = parent.path().join("symlinked");
            restore_snapshot(&symlinked, &files);
            fs::remove_file(symlinked.join("report.html")).unwrap();
            symlink(original.join("report.html"), symlinked.join("report.html")).unwrap();
            assert_eq!(
                verify_bundle_manifest(&symlinked).unwrap_err().code(),
                ErrorCode::IntegrityManifest
            );
        }
    }

    // rustix exposes mkfifoat on Linux but not on Darwin. The verifier's
    // no-follow regular-file checks are still compiled on Apple; this
    // construction-specific regression runs where the safe creation API is
    // available.
    #[cfg(target_os = "linux")]
    #[test]
    fn manifest_verifier_rejects_a_static_fifo_without_blocking() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("fifo-bundle");
        let limits = manifest_fixture(&root, &[("artifact".to_owned(), b"evidence".to_vec())]);
        let artifact = root.join("artifact");
        fs::remove_file(&artifact).unwrap();
        rustix::fs::mkfifoat(CWD, &artifact, rustix::fs::Mode::from_raw_mode(0o600)).unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        let verifier_root = root.clone();
        let verifier = std::thread::spawn(move || {
            let result = verify_bundle_manifest_with_limits(&verifier_root, limits)
                .map_err(|failure| failure.code());
            let _ = sender.send(result);
        });
        let failure = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("nonblocking FIFO classification must return promptly")
            .unwrap_err();
        verifier.join().unwrap();
        assert_eq!(failure, ErrorCode::IntegrityManifest);
    }

    #[test]
    fn manifest_verifier_rejects_concurrent_symlink_substitution_at_open() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("symlink-race-bundle");
        let limits = manifest_fixture(&root, &[("artifact".to_owned(), b"evidence".to_vec())]);
        let artifact = root.join("artifact");
        let outside = parent.path().join("outside");
        fs::write(&outside, b"evidence").unwrap();
        let ready = Arc::new(Barrier::new(2));
        let changed = Arc::new(Barrier::new(2));
        let worker_ready = Arc::clone(&ready);
        let worker_changed = Arc::clone(&changed);
        let worker = std::thread::spawn(move || {
            worker_ready.wait();
            fs::remove_file(&artifact).unwrap();
            symlink(&outside, &artifact).unwrap();
            worker_changed.wait();
        });

        let mut hook_fired = false;
        let mut hook = |relative: &str| {
            if !hook_fired && relative == "artifact" {
                hook_fired = true;
                ready.wait();
                changed.wait();
            }
        };
        let failure =
            verify_manifest_with_limits_and_hook(&root, limits, Some(&mut hook)).unwrap_err();
        worker.join().unwrap();
        assert!(hook_fired);
        assert_eq!(failure.code(), ErrorCode::IntegrityManifest);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn manifest_verifier_rejects_concurrent_fifo_substitution_without_blocking() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("fifo-race-bundle");
        let limits = manifest_fixture(&root, &[("artifact".to_owned(), b"evidence".to_vec())]);
        let artifact = root.join("artifact");
        let ready = Arc::new(Barrier::new(2));
        let changed = Arc::new(Barrier::new(2));
        let worker_ready = Arc::clone(&ready);
        let worker_changed = Arc::clone(&changed);
        let worker = std::thread::spawn(move || {
            worker_ready.wait();
            fs::remove_file(&artifact).unwrap();
            rustix::fs::mkfifoat(CWD, &artifact, rustix::fs::Mode::from_raw_mode(0o600)).unwrap();
            worker_changed.wait();
        });

        let (sender, receiver) = std::sync::mpsc::channel();
        let verifier_ready = Arc::clone(&ready);
        let verifier_changed = Arc::clone(&changed);
        let verifier = std::thread::spawn(move || {
            let mut hook_fired = false;
            let mut hook = |relative: &str| {
                if !hook_fired && relative == "artifact" {
                    hook_fired = true;
                    verifier_ready.wait();
                    verifier_changed.wait();
                }
            };
            let result = verify_manifest_with_limits_and_hook(&root, limits, Some(&mut hook))
                .map_err(|failure| failure.code());
            let _ = sender.send((hook_fired, result));
        });
        let (hook_fired, result) = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("concurrently substituted FIFO must not block open");
        worker.join().unwrap();
        verifier.join().unwrap();
        assert!(hook_fired);
        assert_eq!(result.unwrap_err(), ErrorCode::IntegrityManifest);
    }

    #[test]
    fn public_manifest_limits_are_inclusive_at_every_boundary() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("bounded");
        let exact = manifest_fixture(
            &root,
            &[
                ("nested/deeper/a".to_owned(), b"abc".to_vec()),
                ("top".to_owned(), b"defgh".to_vec()),
            ],
        );

        macro_rules! assert_boundary {
            ($field:ident, $code:expr) => {{
                let mut below = exact;
                below.$field = exact.$field.checked_sub(1).unwrap();
                assert_eq!(
                    verify_bundle_manifest_with_limits(&root, below)
                        .unwrap_err()
                        .code(),
                    $code,
                    "below {}",
                    stringify!($field)
                );
                verify_bundle_manifest_with_limits(&root, exact).unwrap();
                let mut above = exact;
                above.$field = exact.$field.checked_add(1).unwrap();
                verify_bundle_manifest_with_limits(&root, above).unwrap();
            }};
        }

        assert_boundary!(max_manifest_bytes, ErrorCode::ResourceManifestBytes);
        assert_boundary!(max_regular_files, ErrorCode::ResourceMemory);
        assert_boundary!(max_directories, ErrorCode::ResourceMemory);
        assert_boundary!(max_path_depth, ErrorCode::ResourceMemory);
        assert_boundary!(max_hashed_artifact_bytes, ErrorCode::ResourceOutputBytes);
    }

    #[test]
    fn raised_manifest_limits_do_not_request_limit_sized_allocations() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("small-bundle");
        manifest_fixture(&root, &[("artifact".to_owned(), b"evidence".to_vec())]);

        verify_bundle_manifest_with_limits(
            &root,
            BundleManifestLimits {
                max_manifest_bytes: u64::MAX,
                max_regular_files: u64::MAX,
                max_directories: u64::MAX,
                max_path_depth: u32::MAX,
                max_hashed_artifact_bytes: u64::MAX,
            },
        )
        .unwrap();
    }

    #[test]
    fn public_manifest_default_rejects_an_oversized_manifest_before_parsing() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("oversized-manifest");
        fs::create_dir(&root).unwrap();
        let maximum = BundleManifestLimits::default().max_manifest_bytes;
        let oversized = usize::try_from(maximum.checked_add(1).unwrap()).unwrap();
        fs::write(root.join("manifest.sha256"), vec![b'x'; oversized]).unwrap();

        let failure = verify_bundle_manifest(&root).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::ResourceManifestBytes);
        assert!(failure.context().contains("verification byte limit"));
    }

    #[test]
    fn public_manifest_file_limit_bounds_a_large_flat_inventory() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("many-files");
        let artifacts = (0u8..64)
            .map(|index| (format!("artifact-{index:03}"), vec![index]))
            .collect::<Vec<_>>();
        let exact = manifest_fixture(&root, &artifacts);
        assert_eq!(exact.max_regular_files, 65);
        verify_bundle_manifest(&root).unwrap();

        let mut insufficient = exact;
        insufficient.max_regular_files -= 1;
        let failure = verify_bundle_manifest_with_limits(&root, insufficient).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::ResourceMemory);
        assert!(failure.context().contains("file-count limit"));
    }

    #[test]
    fn public_manifest_depth_limit_bounds_listed_paths_and_iterative_traversal() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("deep-tree");
        let default = BundleManifestLimits::default();
        let artifact_depth = default.max_path_depth.checked_add(1).unwrap();
        let mut parts = (0..artifact_depth - 1)
            .map(|depth| format!("d{depth:02}"))
            .collect::<Vec<_>>();
        parts.push("artifact".to_owned());
        let relative = parts.join("/");
        let exact = manifest_fixture(&root, &[(relative, b"bounded".to_vec())]);
        assert_eq!(exact.max_path_depth, artifact_depth);

        let failure = verify_bundle_manifest(&root).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::ResourceMemory);
        assert!(failure.context().contains("path-depth limit"));
        verify_bundle_manifest_with_limits(&root, exact).unwrap();

        let unlisted_root = parent.path().join("unlisted-deep-tree");
        manifest_fixture(
            &unlisted_root,
            &[("artifact".to_owned(), b"bounded".to_vec())],
        );
        let mut unlisted = unlisted_root.clone();
        for depth in 0..artifact_depth {
            unlisted.push(format!("u{depth:02}"));
        }
        fs::create_dir_all(&unlisted).unwrap();
        let traversal_failure = verify_bundle_manifest(&unlisted_root).unwrap_err();
        assert_eq!(traversal_failure.code(), ErrorCode::ResourceMemory);
        assert!(traversal_failure.context().contains("path-depth limit"));
    }

    #[test]
    fn limited_file_digest_detects_limit_plus_one_without_hashing_unbounded_input() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("artifact");
        let bytes = vec![b'z'; 64 * 1024 + 1];
        fs::write(&path, &bytes).unwrap();
        let exact = u64::try_from(bytes.len()).unwrap();
        let digest = |maximum_bytes| {
            let file = File::open(&path).unwrap();
            let snapshot = descriptor_snapshot(&file, "test artifact").unwrap();
            digest_opened_file_limited(file, snapshot, maximum_bytes, "test artifact")
        };

        assert_eq!(
            digest(exact - 1).unwrap_err().code(),
            ErrorCode::ResourceOutputBytes
        );
        assert_eq!(digest(exact).unwrap().0, exact);
        assert_eq!(digest(exact + 1).unwrap().0, exact);
    }

    #[test]
    fn invalid_public_manifest_limits_fail_before_filesystem_access() {
        let missing = Path::new("definitely-missing-bundle");
        for limits in [
            BundleManifestLimits {
                max_manifest_bytes: 0,
                ..BundleManifestLimits::default()
            },
            BundleManifestLimits {
                max_regular_files: 0,
                ..BundleManifestLimits::default()
            },
            BundleManifestLimits {
                max_path_depth: 0,
                ..BundleManifestLimits::default()
            },
        ] {
            assert_eq!(
                verify_bundle_manifest_with_limits(missing, limits)
                    .unwrap_err()
                    .code(),
                ErrorCode::ConfigurationInvalidLimit
            );
        }
    }

    #[test]
    fn lock_cleanup_preserves_a_pre_drop_replacement_file() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let guard = LockGuard::acquire(&destination).unwrap();
        let lock_path = parent.path().join(".result.veritasm.lock");
        fs::remove_file(&lock_path).unwrap();
        fs::write(&lock_path, b"replacement-owned-by-another-writer\n").unwrap();

        drop(guard);

        assert_eq!(
            fs::read(&lock_path).unwrap(),
            b"replacement-owned-by-another-writer\n"
        );
    }

    #[test]
    fn existing_lock_is_preserved_and_requires_manual_review() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let lock_path = parent.path().join(".result.veritasm.lock");
        let stale_record = b"schema=veritasm-lock-v1\npid=999999\ndestination=result\n";
        fs::write(&lock_path, stale_record).unwrap();

        let error = write_bundle(&destination, &sample(false, false)).unwrap_err();

        assert_eq!(error.code(), ErrorCode::DestinationLocked);
        assert!(error.context().contains("will not remove"));
        assert_eq!(fs::read(&lock_path).unwrap(), stale_record);
        assert!(!destination.exists());
    }

    #[test]
    fn leased_writer_consumes_the_exact_early_lease_without_relocking() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let lock_path = parent.path().join(".result.veritasm.lock");
        let lease = RunLease::acquire(&destination).unwrap();
        assert!(lock_path.is_file());

        let competing = write_bundle(&destination, &sample(false, false)).unwrap_err();
        assert_eq!(competing.code(), ErrorCode::DestinationLocked);
        assert!(!destination.exists());

        write_bundle_with_lease(lease, &sample(false, false)).unwrap();
        assert!(destination.is_dir());
        assert!(!lock_path.exists());
        verify_bundle_manifest(&destination).unwrap();
    }

    #[test]
    fn convenience_writer_acquires_lease_before_bundle_validation() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let _lease = RunLease::acquire(&destination).unwrap();
        let mut invalid = sample(false, false);
        invalid.scientific.k = 2;

        let error = write_bundle(&destination, &invalid).unwrap_err();
        assert_eq!(error.code(), ErrorCode::DestinationLocked);
        assert!(!destination.exists());
    }

    #[test]
    fn noncooperating_destination_creation_wins_without_replacement() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");

        let error = write_bundle_inner(
            &destination,
            &sample(false, false),
            Some(FailPoint::DestinationAppearsBeforeCommit),
        )
        .unwrap_err();

        assert_eq!(error.code(), ErrorCode::DestinationExisting);
        assert_eq!(
            fs::read(destination.join("noncooperating-sentinel")).unwrap(),
            b"preserve"
        );
        assert!(!parent.path().join(".result.veritasm.lock").exists());
        let names = fs::read_dir(parent.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, [std::ffi::OsString::from("result")]);
    }

    #[test]
    fn cooperative_concurrent_writers_commit_once() {
        let parent = tempfile::tempdir().unwrap();
        let destination = parent.path().join("result");
        let barrier = Arc::new(Barrier::new(2));
        let data = Arc::new(sample(false, false));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let destination = destination.clone();
            let barrier = Arc::clone(&barrier);
            let data = Arc::clone(&data);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                write_bundle(&destination, &data)
            }));
        }
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        verify_manifest(&destination).unwrap();
    }
}
