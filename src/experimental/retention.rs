//! Exact authenticated support retention for experimental external counts.
//!
//! This module is an isolated research substrate. It authenticates a complete
//! route-ordered [`ExternalPartitionResult`], records one decision for every
//! full key in global key order, and materializes the retained rows without
//! allowing minimizers, buckets, hashes, or probabilistic membership to decide
//! inclusion. It is not wired into the stable CLI or assembly pipeline.

use super::external_reduce::{ExactSupportCount, ExternalPartitionResult};
use super::external_run::WideRunSupportUnit;
use super::partitioned_dbg::{route_minimizer, select_minimizer};
use super::spool_external::{
    build_spool_external_counts, SpoolExternalOptions, SpoolExternalResult,
};
use super::transition_witness::{
    transition_source_root_from_descriptor, TransitionSourceDescriptor,
};
use super::wide_kmer::{canonical_code, validate_code, validate_k, PackedKmer};
#[cfg(test)]
use crate::config::SupportUnit;
use crate::error::{ErrorCode, Result, VeritasmError};
use crate::spool::Spool;
use sha2::{Digest, Sha256};
use std::mem::size_of;

/// Version of the scientific retention/root contract in this module.
pub const RETENTION_ALGORITHM_VERSION: u16 = 1;

const RAW_TABLE_DOMAIN: &[u8] = b"veritasm:retention-raw-table:v1\0";
const RAW_AUTHENTICATION_DOMAIN: &[u8] = b"veritasm:retention-raw-authentication:v1\0";
const DECISION_LEDGER_DOMAIN: &[u8] = b"veritasm:retention-decision-ledger:v1\0";
const RETAINED_TABLE_DOMAIN: &[u8] = b"veritasm:retention-retained-table:v1\0";
const RETENTION_DOMAIN: &[u8] = b"veritasm:retention-artifact:v1\0";
const OPERATIONAL_DOMAIN: &[u8] = b"veritasm:retention-operational:v1\0";
const SOURCE_EQUIVALENCE_DOMAIN: &[u8] = b"veritasm:retention-source-equivalence:v1\0";
const SOURCE_OPERATIONAL_DOMAIN: &[u8] = b"veritasm:retention-source-operational:v1\0";
const ARTIFACT_OPERATIONAL_DOMAIN: &[u8] = b"veritasm:retention-artifact-operational:v1\0";

/// Exact retention choice. Threshold comparison is inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionRule {
    /// Keep every authenticated row, including support-one rows.
    RetainAll,
    /// Keep a row exactly when `row.support >= minimum_support`.
    InclusiveSupport { minimum_support: u64 },
}

impl RetentionRule {
    /// Return the inclusive minimum when this is a threshold rule.
    pub const fn minimum_support(self) -> Option<u64> {
        match self {
            Self::RetainAll => None,
            Self::InclusiveSupport { minimum_support } => Some(minimum_support),
        }
    }

    const fn tag(self) -> u8 {
        match self {
            Self::RetainAll => 0,
            Self::InclusiveSupport { .. } => 1,
        }
    }

    const fn digest_threshold(self) -> u64 {
        match self {
            Self::RetainAll => 0,
            Self::InclusiveSupport { minimum_support } => minimum_support,
        }
    }

    const fn retains(self, support: u64) -> bool {
        match self {
            Self::RetainAll => true,
            Self::InclusiveSupport { minimum_support } => support >= minimum_support,
        }
    }

    fn validate(self) -> Result<()> {
        if matches!(self, Self::InclusiveSupport { minimum_support: 0 }) {
            Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidSupport,
                "experimental inclusive retention support must be at least one",
            ))
        } else {
            Ok(())
        }
    }
}

/// Finite input, output, and transformation-owned heap limits.
///
/// `max_accounted_bytes` covers the exact-key decision index, retained output
/// vector, and the at-most-`k` byte minimizer decoder used by upstream routing
/// validation. The caller-owned raw table, returned artifact after transfer,
/// allocator metadata, stack values, and bounded diagnostic strings are not
/// included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionLimits {
    pub max_raw_keys: u64,
    pub max_retained_keys: u64,
    pub max_accounted_bytes: u64,
}

/// Internal authentication of a structurally valid raw table.
///
/// This alone is not source provenance: only [`SpoolAuthenticatedRaw`] can
/// carry source equivalence, and its public constructor performs the replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawCountAuthentication {
    algorithm_version: u16,
    common_source_root: [u8; 32],
    raw_binding: [u8; 32],
    raw_table_root: [u8; 32],
    k: u8,
    minimizer_length: u8,
    virtual_bucket_count: u32,
    support_unit: WideRunSupportUnit,
    raw_key_count: u64,
    raw_support: u64,
    authentication_root: [u8; 32],
}

/// Internal result of an exact transformation over a structurally
/// authenticated, but not necessarily source-derived, raw table.
#[derive(Debug, PartialEq, Eq)]
struct RetentionCore {
    algorithm_version: u16,
    authentication: RawCountAuthentication,
    rule: RetentionRule,
    raw_key_count: u64,
    retained_key_count: u64,
    discarded_key_count: u64,
    raw_support: u64,
    retained_support: u64,
    discarded_support: u64,
    decision_ledger_root: [u8; 32],
    retained_table_root: [u8; 32],
    retention_root: [u8; 32],
    limits: RetentionLimits,
    decision_index_capacity: u64,
    retained_table_capacity: u64,
    projected_peak_bytes: u64,
    accounted_peak_bytes: u64,
    operational_root: [u8; 32],
    retained: ExternalPartitionResult,
}

impl RetentionCore {
    const fn algorithm_version(&self) -> u16 {
        self.algorithm_version
    }

    const fn rule(&self) -> RetentionRule {
        self.rule
    }

    const fn raw_key_count(&self) -> u64 {
        self.raw_key_count
    }

    const fn retained_key_count(&self) -> u64 {
        self.retained_key_count
    }

    const fn discarded_key_count(&self) -> u64 {
        self.discarded_key_count
    }

    const fn raw_support(&self) -> u64 {
        self.raw_support
    }

    const fn retained_support(&self) -> u64 {
        self.retained_support
    }

    const fn discarded_support(&self) -> u64 {
        self.discarded_support
    }

    const fn decision_ledger_root(&self) -> [u8; 32] {
        self.decision_ledger_root
    }

    const fn retained_table_root(&self) -> [u8; 32] {
        self.retained_table_root
    }

    /// Scientific identity of this transformation and adapter source.
    const fn retention_root(&self) -> [u8; 32] {
        self.retention_root
    }

    const fn limits(&self) -> RetentionLimits {
        self.limits
    }

    const fn projected_peak_bytes(&self) -> u64 {
        self.projected_peak_bytes
    }

    const fn accounted_peak_bytes(&self) -> u64 {
        self.accounted_peak_bytes
    }

    /// Return the retained materialized adapter after checking all invariants
    /// that can be established without the raw table.
    fn retained_counts(&self) -> Result<&ExternalPartitionResult> {
        self.validate_invariants()?;
        Ok(&self.retained)
    }

    /// Recompute every scientific decision from the supplied raw table and
    /// compare every retained row, root, total, and operational field.
    fn validate_against_raw(
        &self,
        raw: &ExternalPartitionResult,
        authentication: &RawCountAuthentication,
    ) -> Result<()> {
        self.validate_invariants()?;
        if authentication != &self.authentication {
            return integrity("retention authentication does not match the artifact seal");
        }
        let rebuilt = retain_unverified_counts(raw, authentication, self.rule, self.limits)?;
        if self != &rebuilt {
            return integrity(
                "retention artifact differs from a complete replay of the authenticated raw table",
            );
        }
        Ok(())
    }

    fn validate_invariants(&self) -> Result<()> {
        self.rule.validate()?;
        validate_authentication(&self.authentication)?;
        if self.algorithm_version != RETENTION_ALGORITHM_VERSION {
            return integrity("retention artifact has an unsupported algorithm version");
        }
        if self.raw_key_count != self.authentication.raw_key_count
            || self.raw_support != self.authentication.raw_support
        {
            return integrity("retention raw totals disagree with the authenticated raw table");
        }
        validate_conservation(
            self.raw_key_count,
            self.retained_key_count,
            self.discarded_key_count,
            "retention key conservation",
        )?;
        validate_conservation(
            self.raw_support,
            self.retained_support,
            self.discarded_support,
            "retention support conservation",
        )?;
        validate_materialized_adapter(self)?;
        let retained_table_root = retained_table_root_from_rows(
            &self.authentication,
            self.rule,
            RetentionTotals::from_artifact(self),
            &self.retained.edge_counts,
        );
        if retained_table_root != self.retained_table_root {
            return integrity("retained table root does not authenticate its materialized rows");
        }
        let expected_retention_root = retention_root(
            &self.authentication,
            self.rule,
            RetentionTotals::from_artifact(self),
            self.decision_ledger_root,
            self.retained_table_root,
        );
        if expected_retention_root != self.retention_root {
            return integrity("retention scientific root does not match its preimage");
        }
        let projection = project_memory(
            self.raw_key_count,
            self.retained_key_count,
            self.authentication.k,
        )?;
        if projection.peak_bytes != self.projected_peak_bytes {
            return integrity("retention projected memory does not match its cardinalities");
        }
        if self.decision_index_capacity != self.raw_key_count {
            return integrity(
                "retention decision-index capacity differs from admitted cardinality",
            );
        }
        if self.retained_table_capacity != self.retained_key_count
            || self.retained_table_capacity != u64_from_usize(self.retained.edge_counts.capacity())?
        {
            return integrity("retained table capacity differs from admitted cardinality");
        }
        let actual = actual_peak_memory(
            self.authentication.k,
            self.raw_key_count,
            self.decision_index_capacity,
            self.retained_table_capacity,
        )?;
        if actual != self.accounted_peak_bytes || actual > self.projected_peak_bytes {
            return integrity("retention actual memory accounting exceeds its admission");
        }
        enforce_limits(
            self.raw_key_count,
            self.retained_key_count,
            self.projected_peak_bytes,
            self.limits,
        )?;
        let expected_operational_root = operational_root(
            self.retention_root,
            self.limits,
            self.decision_index_capacity,
            self.retained_table_capacity,
            self.projected_peak_bytes,
            self.accounted_peak_bytes,
        );
        if expected_operational_root != self.operational_root {
            return integrity("retention operational root does not match its resource preimage");
        }
        Ok(())
    }
}

/// Opaque proof that a raw table was produced by a complete spool replay.
///
/// The type has no public constructor or mutable fields. Its existence is the
/// capability distinction between arbitrary structurally valid rows and rows
/// derived by [`authenticate_spool_external_counts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceEquivalence {
    algorithm_version: u16,
    source_descriptor: TransitionSourceDescriptor,
    common_source_root: [u8; 32],
    raw_binding: [u8; 32],
    raw_table_root: [u8; 32],
    raw_authentication_root: [u8; 32],
    root: [u8; 32],
}

impl SourceEquivalence {
    pub const fn common_source_root(&self) -> [u8; 32] {
        self.common_source_root
    }

    pub const fn raw_binding(&self) -> [u8; 32] {
        self.raw_binding
    }

    pub const fn raw_table_root(&self) -> [u8; 32] {
        self.raw_table_root
    }

    pub const fn root(&self) -> [u8; 32] {
        self.root
    }

    fn validate(&self) -> Result<()> {
        if self.algorithm_version != RETENTION_ALGORITHM_VERSION {
            return integrity("source equivalence has an unsupported algorithm version");
        }
        if transition_source_root_from_descriptor(self.source_descriptor) != self.common_source_root
        {
            return integrity("source equivalence root disagrees with its spool descriptor");
        }
        if source_equivalence_root(
            self.source_descriptor,
            self.common_source_root,
            self.raw_binding,
            self.raw_table_root,
            self.raw_authentication_root,
        ) != self.root
        {
            return integrity("source equivalence root does not match its preimage");
        }
        Ok(())
    }
}

/// Opaque raw-count capability minted only by an actual spool replay.
///
/// This type does not implement `Clone` and exposes no mutable raw-table view.
/// A structurally consistent caller-created [`SpoolExternalResult`] cannot be
/// converted into this capability through the public API.
#[derive(Debug, PartialEq, Eq)]
pub struct SpoolAuthenticatedRaw {
    source_equivalence: SourceEquivalence,
    authentication: RawCountAuthentication,
    authentication_limits: RetentionLimits,
    operational_root: [u8; 32],
    raw: SpoolExternalResult,
}

impl SpoolAuthenticatedRaw {
    pub const fn source_equivalence(&self) -> SourceEquivalence {
        self.source_equivalence
    }

    pub const fn raw_key_count(&self) -> u64 {
        self.authentication.raw_key_count
    }

    pub const fn raw_support(&self) -> u64 {
        self.authentication.raw_support
    }

    pub const fn support_unit(&self) -> WideRunSupportUnit {
        self.authentication.support_unit
    }

    pub const fn k(&self) -> u8 {
        self.authentication.k
    }

    pub const fn minimizer_length(&self) -> u8 {
        self.authentication.minimizer_length
    }

    pub const fn virtual_bucket_count(&self) -> u32 {
        self.authentication.virtual_bucket_count
    }

    pub const fn authentication_limits(&self) -> RetentionLimits {
        self.authentication_limits
    }

    pub const fn operational_root(&self) -> [u8; 32] {
        self.operational_root
    }

    /// Return upstream operational evidence after rechecking the complete
    /// source capability. The result is borrowed and cannot mutate the seal.
    pub fn spool_external_result(&self) -> Result<&SpoolExternalResult> {
        self.validate_invariants()?;
        Ok(&self.raw)
    }

    fn validate_invariants(&self) -> Result<()> {
        self.source_equivalence.validate()?;
        validate_spool_result(&self.raw)?;
        validate_authentication(&self.authentication)?;
        let replayed = authenticate_unverified_spool_result(&self.raw, self.authentication_limits)?;
        if replayed != self.authentication
            || self.source_equivalence.common_source_root != replayed.common_source_root
            || self.source_equivalence.raw_binding != replayed.raw_binding
            || self.source_equivalence.raw_table_root != replayed.raw_table_root
            || self.source_equivalence.raw_authentication_root != replayed.authentication_root
        {
            return integrity("spool-authenticated raw capability is internally inconsistent");
        }
        let expected =
            source_operational_root(self.source_equivalence.root, self.authentication_limits);
        if expected != self.operational_root {
            return integrity("spool-authenticated raw operational root is inconsistent");
        }
        Ok(())
    }
}

/// Opaque spool-backed exact-retention result.
///
/// This is the only public retained artifact. It carries a non-forgeable
/// [`SourceEquivalence`], does not implement `Clone`, has no constructor, and
/// exposes only checked borrowed data.
#[derive(Debug, PartialEq, Eq)]
pub struct RetainedCountArtifact {
    source_equivalence: SourceEquivalence,
    source_authentication_limits: RetentionLimits,
    source_operational_root: [u8; 32],
    operational_root: [u8; 32],
    core: RetentionCore,
}

impl RetainedCountArtifact {
    pub const fn algorithm_version(&self) -> u16 {
        self.core.algorithm_version()
    }

    pub const fn source_equivalence(&self) -> SourceEquivalence {
        self.source_equivalence
    }

    pub const fn common_source_root(&self) -> [u8; 32] {
        self.source_equivalence.common_source_root
    }

    pub const fn raw_binding(&self) -> [u8; 32] {
        self.source_equivalence.raw_binding
    }

    pub const fn raw_table_root(&self) -> [u8; 32] {
        self.source_equivalence.raw_table_root
    }

    pub const fn k(&self) -> u8 {
        self.core.authentication.k
    }

    pub const fn minimizer_length(&self) -> u8 {
        self.core.authentication.minimizer_length
    }

    pub const fn virtual_bucket_count(&self) -> u32 {
        self.core.authentication.virtual_bucket_count
    }

    pub const fn support_unit(&self) -> WideRunSupportUnit {
        self.core.authentication.support_unit
    }

    pub const fn rule(&self) -> RetentionRule {
        self.core.rule()
    }

    pub const fn raw_key_count(&self) -> u64 {
        self.core.raw_key_count()
    }

    pub const fn retained_key_count(&self) -> u64 {
        self.core.retained_key_count()
    }

    pub const fn discarded_key_count(&self) -> u64 {
        self.core.discarded_key_count()
    }

    pub const fn raw_support(&self) -> u64 {
        self.core.raw_support()
    }

    pub const fn retained_support(&self) -> u64 {
        self.core.retained_support()
    }

    pub const fn discarded_support(&self) -> u64 {
        self.core.discarded_support()
    }

    pub const fn decision_ledger_root(&self) -> [u8; 32] {
        self.core.decision_ledger_root()
    }

    pub const fn retained_table_root(&self) -> [u8; 32] {
        self.core.retained_table_root()
    }

    pub const fn retention_root(&self) -> [u8; 32] {
        self.core.retention_root()
    }

    pub const fn operational_root(&self) -> [u8; 32] {
        self.operational_root
    }

    pub const fn limits(&self) -> RetentionLimits {
        self.core.limits()
    }

    pub const fn projected_peak_bytes(&self) -> u64 {
        self.core.projected_peak_bytes()
    }

    pub const fn accounted_peak_bytes(&self) -> u64 {
        self.core.accounted_peak_bytes()
    }

    /// Return the retained materialized adapter after validating its complete
    /// self-contained integrity preimage and spool-source capability.
    pub fn retained_counts(&self) -> Result<&ExternalPartitionResult> {
        self.validate_invariants()?;
        self.core.retained_counts()
    }

    /// Recheck every decision and retained row against an existing
    /// spool-authenticated raw capability, without another physical replay.
    pub fn validate_against_authenticated_raw(&self, raw: &SpoolAuthenticatedRaw) -> Result<()> {
        self.validate_invariants()?;
        raw.validate_invariants()?;
        if self.source_equivalence != raw.source_equivalence
            || self.source_authentication_limits != raw.authentication_limits
            || self.source_operational_root != raw.operational_root
        {
            return integrity("retained artifact and raw capability have different sources");
        }
        let raw_view = raw.raw.validated()?;
        self.core
            .validate_against_raw(raw_view.external_counts(), &raw.authentication)
    }

    /// Perform a fresh exact spool replay and compare the full artifact.
    pub fn validate_against_spool(
        &self,
        spool: &Spool,
        options: &SpoolExternalOptions,
    ) -> Result<()> {
        let raw =
            authenticate_spool_external_counts(spool, options, self.source_authentication_limits)?;
        self.validate_against_authenticated_raw(&raw)
    }

    fn validate_invariants(&self) -> Result<()> {
        self.source_equivalence.validate()?;
        self.core.validate_invariants()?;
        if self.source_equivalence.common_source_root != self.core.authentication.common_source_root
            || self.source_equivalence.raw_binding != self.core.authentication.raw_binding
            || self.source_equivalence.raw_table_root != self.core.authentication.raw_table_root
            || self.source_equivalence.raw_authentication_root
                != self.core.authentication.authentication_root
        {
            return integrity("retained artifact lost its spool-source equivalence");
        }
        if source_operational_root(
            self.source_equivalence.root,
            self.source_authentication_limits,
        ) != self.source_operational_root
        {
            return integrity("retained artifact source operational root is inconsistent");
        }
        let expected = artifact_operational_root(
            self.source_equivalence.root,
            self.source_operational_root,
            self.core.operational_root,
        );
        if expected != self.operational_root {
            return integrity("retained artifact operational root is inconsistent");
        }
        Ok(())
    }
}

/// Replay one immutable spool and mint an opaque source-authenticated raw
/// capability.
///
/// A public caller cannot pass a free-standing `SpoolExternalResult` here:
/// the only accepted inputs are an opaque [`Spool`] and the exact replay
/// options. The physical spool is independently verified by its builder.
///
/// ```compile_fail
/// use veritasm::experimental::retention::{
///     authenticate_spool_external_counts, RetentionLimits,
/// };
/// use veritasm::experimental::spool_external::{
///     SpoolExternalOptions, SpoolExternalResult,
/// };
/// # fn cannot_promote(
/// #     fake: &SpoolExternalResult,
/// #     options: &SpoolExternalOptions,
/// #     limits: RetentionLimits,
/// # ) {
/// let _ = authenticate_spool_external_counts(fake, options, limits);
/// # }
/// ```
pub fn authenticate_spool_external_counts(
    spool: &Spool,
    options: &SpoolExternalOptions,
    limits: RetentionLimits,
) -> Result<SpoolAuthenticatedRaw> {
    let raw = build_spool_external_counts(spool, options)?;
    let authentication = authenticate_unverified_spool_result(&raw, limits)?;
    let raw_view = raw.validated()?;
    let mut source_equivalence = SourceEquivalence {
        algorithm_version: RETENTION_ALGORITHM_VERSION,
        source_descriptor: raw_view.source_descriptor(),
        common_source_root: authentication.common_source_root,
        raw_binding: authentication.raw_binding,
        raw_table_root: authentication.raw_table_root,
        raw_authentication_root: authentication.authentication_root,
        root: [0_u8; 32],
    };
    source_equivalence.root = source_equivalence_root(
        source_equivalence.source_descriptor,
        source_equivalence.common_source_root,
        source_equivalence.raw_binding,
        source_equivalence.raw_table_root,
        source_equivalence.raw_authentication_root,
    );
    let operational_root = source_operational_root(source_equivalence.root, limits);
    let capability = SpoolAuthenticatedRaw {
        source_equivalence,
        authentication,
        authentication_limits: limits,
        operational_root,
        raw,
    };
    capability.validate_invariants()?;
    Ok(capability)
}

/// Retain exact keys from an already spool-authenticated raw capability.
pub fn retain_spool_authenticated_counts(
    input: &SpoolAuthenticatedRaw,
    rule: RetentionRule,
    limits: RetentionLimits,
) -> Result<RetainedCountArtifact> {
    input.validate_invariants()?;
    let raw_view = input.raw.validated()?;
    let core = retain_unverified_counts(
        raw_view.external_counts(),
        &input.authentication,
        rule,
        limits,
    )?;
    let operational_root = artifact_operational_root(
        input.source_equivalence.root,
        input.operational_root,
        core.operational_root,
    );
    let artifact = RetainedCountArtifact {
        source_equivalence: input.source_equivalence,
        source_authentication_limits: input.authentication_limits,
        source_operational_root: input.operational_root,
        operational_root,
        core,
    };
    artifact.validate_invariants()?;
    Ok(artifact)
}

/// Convenience wrapper that performs the physical spool replay and exact
/// retention in one call.
pub fn retain_spool_external_counts(
    spool: &Spool,
    options: &SpoolExternalOptions,
    rule: RetentionRule,
    limits: RetentionLimits,
) -> Result<RetainedCountArtifact> {
    let input = authenticate_spool_external_counts(spool, options, limits)?;
    retain_spool_authenticated_counts(&input, rule, limits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetentionTotals {
    raw_keys: u64,
    retained_keys: u64,
    discarded_keys: u64,
    raw_support: u64,
    retained_support: u64,
    discarded_support: u64,
}

impl RetentionTotals {
    const fn from_artifact(artifact: &RetentionCore) -> Self {
        Self {
            raw_keys: artifact.raw_key_count,
            retained_keys: artifact.retained_key_count,
            discarded_keys: artifact.discarded_key_count,
            raw_support: artifact.raw_support,
            retained_support: artifact.retained_support,
            discarded_support: artifact.discarded_support,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemoryProjection {
    decision_index_bytes: u64,
    retained_table_bytes: u64,
    routing_scratch_bytes: u64,
    peak_bytes: u64,
}

/// Authenticate a generic exact-count result against caller-supplied source
/// and raw-binding identities.
fn authenticate_unverified_external_counts(
    raw: &ExternalPartitionResult,
    common_source_root: [u8; 32],
    expected_raw_binding: [u8; 32],
    limits: RetentionLimits,
) -> Result<RawCountAuthentication> {
    let raw_key_count = u64_from_usize(raw.edge_counts.len())?;
    enforce_raw_limit(raw_key_count, limits.max_raw_keys)?;
    validate_raw_configuration(raw)?;
    enforce_routing_scratch(raw_key_count, raw.k, limits.max_accounted_bytes)?;
    let raw_support = validate_raw_result(raw, expected_raw_binding, raw_key_count)?;
    let raw_table_root = raw_table_root(raw, common_source_root, raw_support);
    let mut authentication = RawCountAuthentication {
        algorithm_version: RETENTION_ALGORITHM_VERSION,
        common_source_root,
        raw_binding: expected_raw_binding,
        raw_table_root,
        k: raw.k,
        minimizer_length: raw.minimizer_length,
        virtual_bucket_count: raw.virtual_bucket_count,
        support_unit: raw.support_unit,
        raw_key_count,
        raw_support,
        authentication_root: [0_u8; 32],
    };
    authentication.authentication_root = raw_authentication_root(&authentication);
    validate_authentication(&authentication)?;
    Ok(authentication)
}

/// Authenticate exact counts returned by a complete spool bridge result.
fn authenticate_unverified_spool_result(
    input: &SpoolExternalResult,
    limits: RetentionLimits,
) -> Result<RawCountAuthentication> {
    let view = input.validated()?;
    authenticate_unverified_external_counts(
        view.external_counts(),
        view.source_root(),
        view.binding_sha256(),
        limits,
    )
}

/// Apply exact retention to an authenticated raw table.
///
/// Time is `O(E log E)` for `E` raw exact keys. Heap usage is pre-admitted and
/// is `max(E * size_of::<usize>(), R * size_of::<ExactSupportCount>(), k)` for
/// `R` retained keys; the two variable-sized vectors are never live together.
fn retain_unverified_counts(
    raw: &ExternalPartitionResult,
    authentication: &RawCountAuthentication,
    rule: RetentionRule,
    limits: RetentionLimits,
) -> Result<RetentionCore> {
    rule.validate()?;
    validate_authentication(authentication)?;
    let raw_key_count = u64_from_usize(raw.edge_counts.len())?;
    enforce_raw_limit(raw_key_count, limits.max_raw_keys)?;
    validate_raw_configuration(raw)?;
    enforce_routing_scratch(raw_key_count, raw.k, limits.max_accounted_bytes)?;
    let raw_support = validate_raw_result(raw, authentication.raw_binding, raw_key_count)?;
    let raw_table_root = raw_table_root(raw, authentication.common_source_root, raw_support);
    if raw.k != authentication.k
        || raw.minimizer_length != authentication.minimizer_length
        || raw.virtual_bucket_count != authentication.virtual_bucket_count
        || raw.support_unit != authentication.support_unit
        || raw_key_count != authentication.raw_key_count
        || raw_support != authentication.raw_support
        || raw_table_root != authentication.raw_table_root
    {
        return integrity("raw exact-count table no longer matches its authentication seal");
    }

    let totals = retention_totals(raw, rule, raw_key_count, raw_support)?;
    let projection = project_memory(raw_key_count, totals.retained_keys, raw.k)?;
    enforce_limits(
        raw_key_count,
        totals.retained_keys,
        projection.peak_bytes,
        limits,
    )?;

    let raw_len = usize_from_u64(raw_key_count, "raw exact-key count")?;
    let mut decision_order = try_vec::<usize>(raw_len, "retention decision index")?;
    decision_order.extend(0..raw_len);
    decision_order.sort_unstable_by_key(|&index| raw.edge_counts[index].key);
    if decision_order
        .windows(2)
        .any(|pair| raw.edge_counts[pair[0]].key >= raw.edge_counts[pair[1]].key)
    {
        return integrity("raw table contains a repeated or unordered global full key");
    }
    let decision_index_capacity = u64_from_usize(decision_order.capacity())?;
    let decision_ledger_root =
        decision_ledger_root(authentication, rule, totals, raw, &decision_order);
    drop(decision_order);

    let retained_len = usize_from_u64(totals.retained_keys, "retained exact-key count")?;
    let mut retained_rows = try_vec::<ExactSupportCount>(retained_len, "retained exact-key table")?;
    for row in &raw.edge_counts {
        if rule.retains(row.support) {
            retained_rows.push(*row);
        }
    }
    if retained_rows.len() != retained_len {
        return invariant("retained table materialization differs from its admitted cardinality");
    }
    let retained_table_capacity = u64_from_usize(retained_rows.capacity())?;
    let retained_table_root =
        retained_table_root_from_rows(authentication, rule, totals, &retained_rows);
    let retention_root = retention_root(
        authentication,
        rule,
        totals,
        decision_ledger_root,
        retained_table_root,
    );
    let accounted_peak_bytes = actual_peak_memory(
        raw.k,
        raw_key_count,
        decision_index_capacity,
        retained_table_capacity,
    )?;
    if accounted_peak_bytes > projection.peak_bytes {
        return Err(memory_error(
            "allocator capacity exceeded the pre-admitted retention memory projection",
        ));
    }
    let operational_root = operational_root(
        retention_root,
        limits,
        decision_index_capacity,
        retained_table_capacity,
        projection.peak_bytes,
        accounted_peak_bytes,
    );
    let retained = ExternalPartitionResult {
        k: raw.k,
        minimizer_length: raw.minimizer_length,
        virtual_bucket_count: raw.virtual_bucket_count,
        source_identity: retention_root,
        support_unit: raw.support_unit,
        support_events: totals.retained_support,
        distinct_kmers: totals.retained_keys,
        edge_counts: retained_rows,
        // Materialized adapter only: none of the raw run telemetry is claimed
        // to survive this in-memory scientific transformation.
        final_runs: Vec::new(),
        replacements: Vec::new(),
        run_files_created: 0,
        open_files_high_water: 0,
        temporary_bytes_final: 0,
        temporary_bytes_high_water: 0,
    };
    let artifact = RetentionCore {
        algorithm_version: RETENTION_ALGORITHM_VERSION,
        authentication: *authentication,
        rule,
        raw_key_count: totals.raw_keys,
        retained_key_count: totals.retained_keys,
        discarded_key_count: totals.discarded_keys,
        raw_support: totals.raw_support,
        retained_support: totals.retained_support,
        discarded_support: totals.discarded_support,
        decision_ledger_root,
        retained_table_root,
        retention_root,
        limits,
        decision_index_capacity,
        retained_table_capacity,
        projected_peak_bytes: projection.peak_bytes,
        accounted_peak_bytes,
        operational_root,
        retained,
    };
    artifact.validate_invariants()?;
    Ok(artifact)
}

fn validate_raw_result(
    raw: &ExternalPartitionResult,
    expected_raw_binding: [u8; 32],
    raw_key_count: u64,
) -> Result<u64> {
    validate_raw_configuration(raw)?;
    if raw.source_identity != expected_raw_binding {
        return integrity("raw exact-count source identity differs from its expected binding");
    }
    if raw.distinct_kmers != raw_key_count {
        return integrity("raw distinct-key total differs from its materialized table");
    }
    let mut previous_route: Option<(u32, PackedKmer)> = None;
    let mut support_sum = 0_u64;
    for row in &raw.edge_counts {
        if row.support == 0 {
            return integrity("raw exact-count row has zero support");
        }
        validate_code(row.key, raw.k)
            .map_err(|_| integrity_error("raw exact-count key has invalid inactive bits"))?;
        let canonical = canonical_code(row.key, raw.k)
            .map_err(|_| integrity_error("raw exact-count key cannot be canonicalized"))?;
        if canonical != row.key {
            return integrity("raw exact-count key is not canonical");
        }
        let owner = select_minimizer(row.key, raw.k, raw.minimizer_length)
            .map_err(|_| integrity_error("raw exact-count minimizer cannot be reproduced"))?;
        let bucket = route_minimizer(owner.key, raw.minimizer_length, raw.virtual_bucket_count)
            .map_err(|_| integrity_error("raw exact-count route cannot be reproduced"))?;
        if row.minimizer != owner.key || row.bucket_id != bucket {
            return integrity("raw exact-count row has an incorrect minimizer or bucket");
        }
        let route = (row.bucket_id, row.key);
        if previous_route.is_some_and(|previous| previous >= route) {
            return integrity("raw exact-count table is not in strict canonical route order");
        }
        previous_route = Some(route);
        support_sum = support_sum
            .checked_add(row.support)
            .ok_or_else(|| overflow("raw exact-count support total overflow"))?;
    }
    if support_sum != raw.support_events {
        return integrity("raw support total differs from the complete exact-count table");
    }
    Ok(support_sum)
}

fn validate_raw_configuration(raw: &ExternalPartitionResult) -> Result<()> {
    validate_k(raw.k)?;
    if raw.minimizer_length == 0 || raw.minimizer_length > raw.k {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidK,
            format!(
                "experimental retention minimizer length must be in 1..={}; received {}",
                raw.k, raw.minimizer_length
            ),
        ));
    }
    if raw.virtual_bucket_count == 0 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            "experimental retention virtual bucket count must be at least one",
        ));
    }
    Ok(())
}

fn validate_authentication(authentication: &RawCountAuthentication) -> Result<()> {
    if authentication.algorithm_version != RETENTION_ALGORITHM_VERSION {
        return integrity("raw authentication has an unsupported algorithm version");
    }
    validate_k(authentication.k)?;
    if authentication.minimizer_length == 0
        || authentication.minimizer_length > authentication.k
        || authentication.virtual_bucket_count == 0
    {
        return integrity("raw authentication contains an invalid graph configuration");
    }
    if raw_authentication_root(authentication) != authentication.authentication_root {
        return integrity("raw authentication root does not match its preimage");
    }
    Ok(())
}

fn validate_materialized_adapter(artifact: &RetentionCore) -> Result<()> {
    let retained = &artifact.retained;
    let authentication = &artifact.authentication;
    if retained.k != authentication.k
        || retained.minimizer_length != authentication.minimizer_length
        || retained.virtual_bucket_count != authentication.virtual_bucket_count
        || retained.support_unit != authentication.support_unit
        || retained.source_identity != artifact.retention_root
        || retained.distinct_kmers != artifact.retained_key_count
        || retained.support_events != artifact.retained_support
    {
        return integrity("retained materialized adapter metadata is inconsistent");
    }
    if !retained.final_runs.is_empty()
        || !retained.replacements.is_empty()
        || retained.run_files_created != 0
        || retained.open_files_high_water != 0
        || retained.temporary_bytes_final != 0
        || retained.temporary_bytes_high_water != 0
    {
        return integrity("retained adapter falsely carries external-run operational evidence");
    }
    if u64_from_usize(retained.edge_counts.len())? != artifact.retained_key_count {
        return integrity("retained adapter cardinality differs from its materialized rows");
    }
    let mut support = 0_u64;
    let mut previous_route: Option<(u32, PackedKmer)> = None;
    for row in &retained.edge_counts {
        if row.support == 0 || !artifact.rule.retains(row.support) {
            return integrity("retained adapter contains a row excluded by its exact rule");
        }
        validate_code(row.key, retained.k)
            .map_err(|_| integrity_error("retained key has invalid inactive bits"))?;
        if canonical_code(row.key, retained.k)
            .map_err(|_| integrity_error("retained key cannot be canonicalized"))?
            != row.key
        {
            return integrity("retained adapter contains a noncanonical key");
        }
        let owner = select_minimizer(row.key, retained.k, retained.minimizer_length)
            .map_err(|_| integrity_error("retained minimizer cannot be reproduced"))?;
        let bucket = route_minimizer(
            owner.key,
            retained.minimizer_length,
            retained.virtual_bucket_count,
        )
        .map_err(|_| integrity_error("retained route cannot be reproduced"))?;
        if row.minimizer != owner.key || row.bucket_id != bucket {
            return integrity("retained adapter has an incorrect minimizer or bucket");
        }
        let route = (row.bucket_id, row.key);
        if previous_route.is_some_and(|previous| previous >= route) {
            return integrity("retained adapter is not in strict canonical route order");
        }
        previous_route = Some(route);
        support = support
            .checked_add(row.support)
            .ok_or_else(|| overflow("retained support total overflow"))?;
    }
    if support != artifact.retained_support {
        return integrity("retained adapter support differs from its complete table");
    }
    Ok(())
}

fn validate_spool_result(input: &SpoolExternalResult) -> Result<()> {
    input.validated().map(|_| ())
}

fn retention_totals(
    raw: &ExternalPartitionResult,
    rule: RetentionRule,
    raw_keys: u64,
    raw_support: u64,
) -> Result<RetentionTotals> {
    let mut retained_keys = 0_u64;
    let mut retained_support = 0_u64;
    for row in &raw.edge_counts {
        if rule.retains(row.support) {
            retained_keys = retained_keys
                .checked_add(1)
                .ok_or_else(|| overflow("retained exact-key count overflow"))?;
            retained_support = retained_support
                .checked_add(row.support)
                .ok_or_else(|| overflow("retained support total overflow"))?;
        }
    }
    let discarded_keys = raw_keys
        .checked_sub(retained_keys)
        .ok_or_else(|| invariant_error("retained key count exceeds raw key count"))?;
    let discarded_support = raw_support
        .checked_sub(retained_support)
        .ok_or_else(|| invariant_error("retained support exceeds raw support"))?;
    Ok(RetentionTotals {
        raw_keys,
        retained_keys,
        discarded_keys,
        raw_support,
        retained_support,
        discarded_support,
    })
}

fn project_memory(raw_keys: u64, retained_keys: u64, k: u8) -> Result<MemoryProjection> {
    let decision_index_bytes = checked_mul(
        raw_keys,
        u64_from_usize(size_of::<usize>())?,
        "retention decision-index bytes",
    )?;
    let retained_table_bytes = checked_mul(
        retained_keys,
        u64_from_usize(size_of::<ExactSupportCount>())?,
        "retained exact-table bytes",
    )?;
    let routing_scratch_bytes = if raw_keys == 0 { 0 } else { u64::from(k) };
    Ok(MemoryProjection {
        decision_index_bytes,
        retained_table_bytes,
        routing_scratch_bytes,
        peak_bytes: decision_index_bytes
            .max(retained_table_bytes)
            .max(routing_scratch_bytes),
    })
}

fn actual_peak_memory(
    k: u8,
    raw_keys: u64,
    decision_index_capacity: u64,
    retained_table_capacity: u64,
) -> Result<u64> {
    let decision_bytes = checked_mul(
        decision_index_capacity,
        u64_from_usize(size_of::<usize>())?,
        "actual decision-index bytes",
    )?;
    let retained_bytes = checked_mul(
        retained_table_capacity,
        u64_from_usize(size_of::<ExactSupportCount>())?,
        "actual retained-table bytes",
    )?;
    Ok(decision_bytes
        .max(retained_bytes)
        .max(if raw_keys == 0 { 0 } else { u64::from(k) }))
}

fn enforce_limits(
    raw_keys: u64,
    retained_keys: u64,
    projected_bytes: u64,
    limits: RetentionLimits,
) -> Result<()> {
    enforce_raw_limit(raw_keys, limits.max_raw_keys)?;
    if retained_keys > limits.max_retained_keys {
        return Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            format!(
                "retention would materialize {retained_keys} keys, exceeding limit {}",
                limits.max_retained_keys
            ),
        ));
    }
    if projected_bytes > limits.max_accounted_bytes {
        return Err(memory_error(format!(
            "retention requires {projected_bytes} accounted bytes, exceeding limit {}",
            limits.max_accounted_bytes
        )));
    }
    Ok(())
}

fn enforce_raw_limit(raw_keys: u64, limit: u64) -> Result<()> {
    if raw_keys > limit {
        Err(VeritasmError::new(
            ErrorCode::ResourceRetainedKeys,
            format!("raw exact table has {raw_keys} keys, exceeding retention limit {limit}"),
        ))
    } else {
        Ok(())
    }
}

fn enforce_routing_scratch(raw_keys: u64, k: u8, memory_limit: u64) -> Result<()> {
    let required = if raw_keys == 0 { 0 } else { u64::from(k) };
    if required > memory_limit {
        Err(memory_error(format!(
            "raw routing validation requires {required} bytes, exceeding retention memory limit {memory_limit}"
        )))
    } else {
        Ok(())
    }
}

fn raw_table_root(
    raw: &ExternalPartitionResult,
    common_source_root: [u8; 32],
    raw_support: u64,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(RAW_TABLE_DOMAIN);
    digest.update(RETENTION_ALGORITHM_VERSION.to_le_bytes());
    digest.update(common_source_root);
    digest.update(raw.source_identity);
    update_graph_domain(
        &mut digest,
        raw.k,
        raw.minimizer_length,
        raw.virtual_bucket_count,
        raw.support_unit,
    );
    digest.update(raw.distinct_kmers.to_le_bytes());
    digest.update(raw_support.to_le_bytes());
    for row in &raw.edge_counts {
        digest.update(row.bucket_id.to_le_bytes());
        digest.update(row.minimizer.to_be_bytes());
        digest.update(row.key.to_be_bytes());
        digest.update(row.support.to_le_bytes());
    }
    digest.finalize().into()
}

fn raw_authentication_root(authentication: &RawCountAuthentication) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(RAW_AUTHENTICATION_DOMAIN);
    digest.update(authentication.algorithm_version.to_le_bytes());
    digest.update(authentication.common_source_root);
    digest.update(authentication.raw_binding);
    digest.update(authentication.raw_table_root);
    update_graph_domain(
        &mut digest,
        authentication.k,
        authentication.minimizer_length,
        authentication.virtual_bucket_count,
        authentication.support_unit,
    );
    digest.update(authentication.raw_key_count.to_le_bytes());
    digest.update(authentication.raw_support.to_le_bytes());
    digest.finalize().into()
}

fn decision_ledger_root(
    authentication: &RawCountAuthentication,
    rule: RetentionRule,
    totals: RetentionTotals,
    raw: &ExternalPartitionResult,
    global_key_order: &[usize],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    update_retention_preamble(
        &mut digest,
        DECISION_LEDGER_DOMAIN,
        authentication,
        rule,
        totals,
    );
    for &index in global_key_order {
        let row = &raw.edge_counts[index];
        digest.update(row.key.to_be_bytes());
        digest.update(row.support.to_le_bytes());
        digest.update([u8::from(rule.retains(row.support))]);
    }
    digest.finalize().into()
}

fn retained_table_root_from_rows(
    authentication: &RawCountAuthentication,
    rule: RetentionRule,
    totals: RetentionTotals,
    rows: &[ExactSupportCount],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    update_retention_preamble(
        &mut digest,
        RETAINED_TABLE_DOMAIN,
        authentication,
        rule,
        totals,
    );
    // Rows remain in strict, deterministic (bucket, full-key) route order so
    // the checked borrowed adapter can be authenticated without another heap
    // allocation. Only complete keys and exact supports are scientific data.
    for row in rows {
        digest.update(row.key.to_be_bytes());
        digest.update(row.support.to_le_bytes());
    }
    digest.finalize().into()
}

fn retention_root(
    authentication: &RawCountAuthentication,
    rule: RetentionRule,
    totals: RetentionTotals,
    decision_ledger_root: [u8; 32],
    retained_table_root: [u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    update_retention_preamble(&mut digest, RETENTION_DOMAIN, authentication, rule, totals);
    digest.update(decision_ledger_root);
    digest.update(retained_table_root);
    digest.finalize().into()
}

fn operational_root(
    retention_root: [u8; 32],
    limits: RetentionLimits,
    decision_index_capacity: u64,
    retained_table_capacity: u64,
    projected_peak_bytes: u64,
    accounted_peak_bytes: u64,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(OPERATIONAL_DOMAIN);
    digest.update(RETENTION_ALGORITHM_VERSION.to_le_bytes());
    digest.update(retention_root);
    digest.update(limits.max_raw_keys.to_le_bytes());
    digest.update(limits.max_retained_keys.to_le_bytes());
    digest.update(limits.max_accounted_bytes.to_le_bytes());
    digest.update(decision_index_capacity.to_le_bytes());
    digest.update(retained_table_capacity.to_le_bytes());
    digest.update(projected_peak_bytes.to_le_bytes());
    digest.update(accounted_peak_bytes.to_le_bytes());
    digest.finalize().into()
}

fn source_equivalence_root(
    descriptor: TransitionSourceDescriptor,
    common_source_root: [u8; 32],
    raw_binding: [u8; 32],
    raw_table_root: [u8; 32],
    raw_authentication_root: [u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SOURCE_EQUIVALENCE_DOMAIN);
    digest.update(RETENTION_ALGORITHM_VERSION.to_le_bytes());
    digest.update(descriptor.spool_sha256);
    digest.update(descriptor.spool_pretrailer_sha256);
    digest.update(descriptor.spool_schema.to_le_bytes());
    digest.update(descriptor.fragment_count.to_le_bytes());
    digest.update(descriptor.read_count.to_le_bytes());
    digest.update([descriptor.input_mode]);
    digest.update([descriptor.min_base_quality]);
    digest.update(common_source_root);
    digest.update(raw_binding);
    digest.update(raw_table_root);
    digest.update(raw_authentication_root);
    digest.finalize().into()
}

fn source_operational_root(source_equivalence_root: [u8; 32], limits: RetentionLimits) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SOURCE_OPERATIONAL_DOMAIN);
    digest.update(RETENTION_ALGORITHM_VERSION.to_le_bytes());
    digest.update(source_equivalence_root);
    digest.update(limits.max_raw_keys.to_le_bytes());
    digest.update(limits.max_retained_keys.to_le_bytes());
    digest.update(limits.max_accounted_bytes.to_le_bytes());
    digest.finalize().into()
}

fn artifact_operational_root(
    source_equivalence_root: [u8; 32],
    source_operational_root: [u8; 32],
    core_operational_root: [u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(ARTIFACT_OPERATIONAL_DOMAIN);
    digest.update(RETENTION_ALGORITHM_VERSION.to_le_bytes());
    digest.update(source_equivalence_root);
    digest.update(source_operational_root);
    digest.update(core_operational_root);
    digest.finalize().into()
}

fn update_retention_preamble(
    digest: &mut Sha256,
    domain: &[u8],
    authentication: &RawCountAuthentication,
    rule: RetentionRule,
    totals: RetentionTotals,
) {
    digest.update(domain);
    digest.update(RETENTION_ALGORITHM_VERSION.to_le_bytes());
    digest.update(authentication.common_source_root);
    digest.update(authentication.raw_binding);
    digest.update(authentication.raw_table_root);
    digest.update(authentication.authentication_root);
    update_graph_domain(
        digest,
        authentication.k,
        authentication.minimizer_length,
        authentication.virtual_bucket_count,
        authentication.support_unit,
    );
    digest.update([rule.tag()]);
    digest.update(rule.digest_threshold().to_le_bytes());
    digest.update(totals.raw_keys.to_le_bytes());
    digest.update(totals.retained_keys.to_le_bytes());
    digest.update(totals.discarded_keys.to_le_bytes());
    digest.update(totals.raw_support.to_le_bytes());
    digest.update(totals.retained_support.to_le_bytes());
    digest.update(totals.discarded_support.to_le_bytes());
}

fn update_graph_domain(
    digest: &mut Sha256,
    k: u8,
    minimizer_length: u8,
    virtual_bucket_count: u32,
    support_unit: WideRunSupportUnit,
) {
    digest.update([k]);
    digest.update([minimizer_length]);
    digest.update(virtual_bucket_count.to_le_bytes());
    digest.update([support_unit as u8]);
}

fn validate_conservation(
    raw: u64,
    retained: u64,
    discarded: u64,
    label: &'static str,
) -> Result<()> {
    let reproduced = retained
        .checked_add(discarded)
        .ok_or_else(|| overflow(label))?;
    if reproduced != raw {
        integrity(label)
    } else {
        Ok(())
    }
}

fn try_vec<T>(capacity: usize, label: &'static str) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|cause| memory_error(format!("cannot reserve experimental {label}: {cause}")))?;
    if values.capacity() > capacity {
        return Err(memory_error(format!(
            "allocator returned experimental {label} capacity {} above admitted capacity {capacity}",
            values.capacity()
        )));
    }
    Ok(values)
}

fn checked_mul(left: u64, right: u64, label: &'static str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| overflow(label))
}

fn usize_from_u64(value: u64, label: &'static str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| memory_error(format!("experimental {label} {value} does not fit usize")))
}

fn u64_from_usize(value: usize) -> Result<u64> {
    u64::try_from(value).map_err(|_| overflow("platform usize does not fit u64"))
}

fn memory_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn overflow(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn integrity<T>(context: impl Into<String>) -> Result<T> {
    Err(integrity_error(context))
}

fn integrity_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::IntegrityArtifact, context)
}

fn invariant<T>(context: impl Into<String>) -> Result<T> {
    Err(invariant_error(context))
}

fn invariant_error(context: impl Into<String>) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSpec, Limits, Profile, ScientificConfig};
    use crate::experimental::external_reduce::ExternalPartitionLimits;
    use crate::experimental::wide_kmer::{decode_mer_string, encode_kmer, reverse_complement_code};
    use crate::spool::create_spool;
    use proptest::prelude::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    const SOURCE_ROOT: [u8; 32] = [0x11; 32];
    const RAW_BINDING: [u8; 32] = [0x22; 32];
    const K: u8 = 5;
    const M: u8 = 3;
    const BUCKETS: u32 = 5;

    fn limits() -> RetentionLimits {
        RetentionLimits {
            max_raw_keys: 1_000,
            max_retained_keys: 1_000,
            max_accounted_bytes: 1 << 20,
        }
    }

    fn raw_counts(
        rows: &[(&str, u64)],
        support_unit: WideRunSupportUnit,
    ) -> ExternalPartitionResult {
        let mut edge_counts: Vec<_> = rows
            .iter()
            .map(|(sequence, support)| {
                assert_eq!(sequence.len(), usize::from(K));
                let encoded = encode_kmer(sequence.as_bytes()).unwrap();
                let key = canonical_code(encoded, K).unwrap();
                let owner = select_minimizer(key, K, M).unwrap();
                ExactSupportCount {
                    bucket_id: route_minimizer(owner.key, M, BUCKETS).unwrap(),
                    minimizer: owner.key,
                    key,
                    support: *support,
                }
            })
            .collect();
        edge_counts.sort_unstable_by_key(|row| (row.bucket_id, row.key));
        assert!(!edge_counts
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key));
        let support_events = edge_counts.iter().map(|row| row.support).sum();
        ExternalPartitionResult {
            k: K,
            minimizer_length: M,
            virtual_bucket_count: BUCKETS,
            source_identity: RAW_BINDING,
            support_unit,
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

    fn authenticate(raw: &ExternalPartitionResult) -> RawCountAuthentication {
        authenticate_unverified_external_counts(raw, SOURCE_ROOT, RAW_BINDING, limits()).unwrap()
    }

    fn retain_core(raw: &ExternalPartitionResult, rule: RetentionRule) -> RetentionCore {
        let authentication = authenticate(raw);
        retain_unverified_counts(raw, &authentication, rule, limits()).unwrap()
    }

    fn assert_code<T>(result: Result<T>, expected: ErrorCode) {
        match result {
            Ok(_) => panic!("expected {expected}"),
            Err(error) => assert_eq!(error.code(), expected, "{}", error.context()),
        }
    }

    fn to_hex(root: [u8; 32]) -> String {
        let mut encoded = String::new();
        for byte in root {
            use std::fmt::Write as _;
            write!(&mut encoded, "{byte:02x}").unwrap();
        }
        encoded
    }

    #[test]
    fn literal_oracle_retains_only_full_keys_meeting_inclusive_support() {
        let literals = [
            ("AAAAA", 1),
            ("AAAAC", 2),
            ("AAACC", 3),
            ("AACCC", 4),
            ("ACGTA", 5),
        ];
        let raw = raw_counts(&literals, WideRunSupportUnit::AcceptedWindowOccurrence);
        let retained = retain_core(&raw, RetentionRule::InclusiveSupport { minimum_support: 3 });
        let actual: BTreeMap<_, _> = retained
            .retained_counts()
            .unwrap()
            .edge_counts
            .iter()
            .map(|row| (decode_mer_string(row.key, K).unwrap(), row.support))
            .collect();
        let expected: BTreeMap<_, _> = literals
            .iter()
            .filter(|(_, support)| *support >= 3)
            .map(|(literal, support)| {
                let canonical =
                    canonical_code(encode_kmer(literal.as_bytes()).unwrap(), K).unwrap();
                (decode_mer_string(canonical, K).unwrap(), *support)
            })
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(retained.raw_key_count(), 5);
        assert_eq!(retained.retained_key_count(), 3);
        assert_eq!(retained.discarded_key_count(), 2);
        assert_eq!(retained.raw_support(), 15);
        assert_eq!(retained.retained_support(), 12);
        assert_eq!(retained.discarded_support(), 3);
        retained
            .validate_against_raw(&raw, &authenticate(&raw))
            .unwrap();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn exact_predicate_and_conservation_match_independent_support_oracle(
            supports in prop::collection::vec(1_u64..1_000_000, 4),
            threshold in 1_u64..1_000_000,
        ) {
            let rows = [
                ("AAAAA", supports[0]),
                ("AAAAC", supports[1]),
                ("AAACC", supports[2]),
                ("AACCC", supports[3]),
            ];
            let raw = raw_counts(&rows, WideRunSupportUnit::AcceptedWindowOccurrence);
            let artifact = retain_core(
                &raw,
                RetentionRule::InclusiveSupport {
                    minimum_support: threshold,
                },
            );
            let expected_keys = supports.iter().filter(|&&support| support >= threshold).count() as u64;
            let expected_support = supports
                .iter()
                .filter(|&&support| support >= threshold)
                .copied()
                .sum::<u64>();
            prop_assert_eq!(artifact.retained_key_count(), expected_keys);
            prop_assert_eq!(artifact.retained_support(), expected_support);
            prop_assert_eq!(
                artifact.raw_key_count(),
                artifact.retained_key_count() + artifact.discarded_key_count()
            );
            prop_assert_eq!(
                artifact.raw_support(),
                artifact.retained_support() + artifact.discarded_support()
            );
        }
    }

    #[test]
    fn inclusive_threshold_boundaries_and_retain_all_semantics_are_exact() {
        let raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        let threshold = retain_core(&raw, RetentionRule::InclusiveSupport { minimum_support: 2 });
        let supports: Vec<_> = threshold
            .retained_counts()
            .unwrap()
            .edge_counts
            .iter()
            .map(|row| row.support)
            .collect();
        assert_eq!(supports.len(), 2);
        assert!(supports.contains(&2));
        assert!(supports.contains(&3));

        let retain_all = retain_core(&raw, RetentionRule::RetainAll);
        let threshold_one =
            retain_core(&raw, RetentionRule::InclusiveSupport { minimum_support: 1 });
        assert_eq!(
            retain_all.retained_counts().unwrap().edge_counts,
            threshold_one.retained_counts().unwrap().edge_counts
        );
        assert_ne!(
            retain_all.decision_ledger_root(),
            threshold_one.decision_ledger_root()
        );
        assert_ne!(
            retain_all.retained_table_root(),
            threshold_one.retained_table_root()
        );
        assert_ne!(retain_all.retention_root(), threshold_one.retention_root());

        assert_code(
            retain_unverified_counts(
                &raw,
                &authenticate(&raw),
                RetentionRule::InclusiveSupport { minimum_support: 0 },
                limits(),
            ),
            ErrorCode::ConfigurationInvalidSupport,
        );
    }

    #[test]
    fn empty_and_all_discarded_tables_are_complete_authenticated_results() {
        let empty = raw_counts(&[], WideRunSupportUnit::AcceptedWindowOccurrence);
        let empty_artifact = retain_core(&empty, RetentionRule::RetainAll);
        assert_eq!(empty_artifact.raw_key_count(), 0);
        assert_eq!(empty_artifact.retained_key_count(), 0);
        assert_eq!(empty_artifact.projected_peak_bytes(), 0);
        assert!(empty_artifact
            .retained_counts()
            .unwrap()
            .edge_counts
            .is_empty());
        for root in [
            empty_artifact.authentication.raw_table_root,
            empty_artifact.authentication.authentication_root,
            empty_artifact.decision_ledger_root(),
            empty_artifact.retained_table_root(),
            empty_artifact.retention_root(),
            empty_artifact.operational_root,
        ] {
            assert_ne!(root, [0_u8; 32]);
        }

        let raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        let discarded = retain_core(&raw, RetentionRule::InclusiveSupport { minimum_support: 3 });
        assert_eq!(discarded.retained_key_count(), 0);
        assert_eq!(discarded.discarded_key_count(), 2);
        assert_eq!(discarded.retained_support(), 0);
        assert_eq!(discarded.discarded_support(), 3);
        assert!(discarded.retained_counts().unwrap().edge_counts.is_empty());
    }

    #[test]
    fn support_unit_is_preserved_and_bound_into_every_scientific_identity() {
        let rows = [("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3)];
        let occurrence = raw_counts(&rows, WideRunSupportUnit::AcceptedWindowOccurrence);
        let fragment = raw_counts(&rows, WideRunSupportUnit::SuppliedFragmentInstance);
        let occurrence_artifact = retain_core(&occurrence, RetentionRule::RetainAll);
        let fragment_artifact = retain_core(&fragment, RetentionRule::RetainAll);
        assert_eq!(
            occurrence_artifact.retained_counts().unwrap().support_unit,
            WideRunSupportUnit::AcceptedWindowOccurrence
        );
        assert_eq!(
            fragment_artifact.retained_counts().unwrap().support_unit,
            WideRunSupportUnit::SuppliedFragmentInstance
        );
        assert_ne!(
            occurrence_artifact.authentication.raw_table_root,
            fragment_artifact.authentication.raw_table_root
        );
        assert_ne!(
            occurrence_artifact.decision_ledger_root(),
            fragment_artifact.decision_ledger_root()
        );
        assert_ne!(
            occurrence_artifact.retention_root(),
            fragment_artifact.retention_root()
        );
    }

    #[test]
    fn raw_table_rejects_permutations_noncanonical_keys_bad_routes_and_totals() {
        let base = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );

        let mut permuted = base.clone();
        permuted.edge_counts.reverse();
        assert_code(
            authenticate_unverified_external_counts(&permuted, SOURCE_ROOT, RAW_BINDING, limits()),
            ErrorCode::IntegrityArtifact,
        );

        let mut noncanonical = base.clone();
        noncanonical.edge_counts[0].key =
            reverse_complement_code(noncanonical.edge_counts[0].key, K).unwrap();
        assert_ne!(
            noncanonical.edge_counts[0].key,
            canonical_code(noncanonical.edge_counts[0].key, K).unwrap()
        );
        assert_code(
            authenticate_unverified_external_counts(
                &noncanonical,
                SOURCE_ROOT,
                RAW_BINDING,
                limits(),
            ),
            ErrorCode::IntegrityArtifact,
        );

        let mut bad_route = base.clone();
        bad_route.edge_counts[0].bucket_id = (bad_route.edge_counts[0].bucket_id + 1) % BUCKETS;
        assert_code(
            authenticate_unverified_external_counts(&bad_route, SOURCE_ROOT, RAW_BINDING, limits()),
            ErrorCode::IntegrityArtifact,
        );

        let mut bad_minimizer = base.clone();
        bad_minimizer.edge_counts[0].minimizer = PackedKmer::ZERO;
        if bad_minimizer.edge_counts[0].minimizer == base.edge_counts[0].minimizer {
            bad_minimizer.edge_counts[0].minimizer = PackedKmer::from_u128(1);
        }
        assert_code(
            authenticate_unverified_external_counts(
                &bad_minimizer,
                SOURCE_ROOT,
                RAW_BINDING,
                limits(),
            ),
            ErrorCode::IntegrityArtifact,
        );

        let mut zero = base.clone();
        zero.edge_counts[0].support = 0;
        zero.support_events -= 1;
        assert_code(
            authenticate_unverified_external_counts(&zero, SOURCE_ROOT, RAW_BINDING, limits()),
            ErrorCode::IntegrityArtifact,
        );

        let mut wrong_total = base.clone();
        wrong_total.support_events += 1;
        assert_code(
            authenticate_unverified_external_counts(
                &wrong_total,
                SOURCE_ROOT,
                RAW_BINDING,
                limits(),
            ),
            ErrorCode::IntegrityArtifact,
        );

        let mut wrong_distinct = base.clone();
        wrong_distinct.distinct_kmers += 1;
        assert_code(
            authenticate_unverified_external_counts(
                &wrong_distinct,
                SOURCE_ROOT,
                RAW_BINDING,
                limits(),
            ),
            ErrorCode::IntegrityArtifact,
        );

        let mut wrong_source = base;
        wrong_source.source_identity[0] ^= 1;
        assert_code(
            authenticate_unverified_external_counts(
                &wrong_source,
                SOURCE_ROOT,
                RAW_BINDING,
                limits(),
            ),
            ErrorCode::IntegrityArtifact,
        );
    }

    #[test]
    fn authentication_detects_post_seal_row_and_root_mutation() {
        let raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        let authentication = authenticate(&raw);

        let mut changed = raw.clone();
        changed.edge_counts[0].support += 1;
        changed.support_events += 1;
        assert_code(
            retain_unverified_counts(
                &changed,
                &authentication,
                RetentionRule::RetainAll,
                limits(),
            ),
            ErrorCode::IntegrityArtifact,
        );

        let mut broken_seal = authentication;
        broken_seal.authentication_root[0] ^= 1;
        assert_code(
            retain_unverified_counts(&raw, &broken_seal, RetentionRule::RetainAll, limits()),
            ErrorCode::IntegrityArtifact,
        );
    }

    #[test]
    fn artifact_detects_rows_totals_source_and_complete_resealed_ledger_tampering() {
        let raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        let authentication = authenticate(&raw);

        let mut row = retain_core(&raw, RetentionRule::RetainAll);
        row.retained.edge_counts[0].support += 1;
        assert_code(row.retained_counts(), ErrorCode::IntegrityArtifact);

        let mut totals = retain_core(&raw, RetentionRule::RetainAll);
        totals.retained_support += 1;
        assert_code(totals.retained_counts(), ErrorCode::IntegrityArtifact);

        let mut source = retain_core(&raw, RetentionRule::RetainAll);
        source.retained.source_identity[0] ^= 1;
        assert_code(source.retained_counts(), ErrorCode::IntegrityArtifact);

        let mut root = retain_core(&raw, RetentionRule::RetainAll);
        root.retained_table_root[0] ^= 1;
        assert_code(root.retained_counts(), ErrorCode::IntegrityArtifact);

        // Simulate an in-crate adversary that can recompute all self-contained
        // roots but does not possess a different authenticated raw table.
        let mut ledger = retain_core(&raw, RetentionRule::RetainAll);
        ledger.decision_ledger_root[0] ^= 1;
        ledger.retention_root = retention_root(
            &ledger.authentication,
            ledger.rule,
            RetentionTotals::from_artifact(&ledger),
            ledger.decision_ledger_root,
            ledger.retained_table_root,
        );
        ledger.retained.source_identity = ledger.retention_root;
        ledger.operational_root = operational_root(
            ledger.retention_root,
            ledger.limits,
            ledger.decision_index_capacity,
            ledger.retained_table_capacity,
            ledger.projected_peak_bytes,
            ledger.accounted_peak_bytes,
        );
        ledger.validate_invariants().unwrap();
        assert_code(
            ledger.validate_against_raw(&raw, &authentication),
            ErrorCode::IntegrityArtifact,
        );
    }

    #[test]
    fn cardinality_and_memory_limits_have_inclusive_exact_boundaries() {
        let raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        let authentication = authenticate(&raw);
        let generous = retain_unverified_counts(
            &raw,
            &authentication,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            limits(),
        )
        .unwrap();

        let exact = RetentionLimits {
            max_raw_keys: 3,
            max_retained_keys: 2,
            max_accounted_bytes: generous.projected_peak_bytes(),
        };
        let exact_authentication =
            authenticate_unverified_external_counts(&raw, SOURCE_ROOT, RAW_BINDING, exact).unwrap();
        retain_unverified_counts(
            &raw,
            &exact_authentication,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            exact,
        )
        .unwrap();

        let raw_too_small = RetentionLimits {
            max_raw_keys: 2,
            ..exact
        };
        assert_code(
            authenticate_unverified_external_counts(&raw, SOURCE_ROOT, RAW_BINDING, raw_too_small),
            ErrorCode::ResourceRetainedKeys,
        );

        let routing_scratch_too_small = RetentionLimits {
            max_accounted_bytes: u64::from(K) - 1,
            ..exact
        };
        assert_code(
            authenticate_unverified_external_counts(
                &raw,
                SOURCE_ROOT,
                RAW_BINDING,
                routing_scratch_too_small,
            ),
            ErrorCode::ResourceMemory,
        );

        let retained_too_small = RetentionLimits {
            max_retained_keys: 1,
            ..exact
        };
        assert_code(
            retain_unverified_counts(
                &raw,
                &authentication,
                RetentionRule::InclusiveSupport { minimum_support: 2 },
                retained_too_small,
            ),
            ErrorCode::ResourceRetainedKeys,
        );

        let memory_too_small = RetentionLimits {
            max_accounted_bytes: generous.projected_peak_bytes() - 1,
            ..exact
        };
        assert_code(
            retain_unverified_counts(
                &raw,
                &authentication,
                RetentionRule::InclusiveSupport { minimum_support: 2 },
                memory_too_small,
            ),
            ErrorCode::ResourceMemory,
        );

        let empty = raw_counts(&[], WideRunSupportUnit::AcceptedWindowOccurrence);
        let zero_limits = RetentionLimits {
            max_raw_keys: 0,
            max_retained_keys: 0,
            max_accounted_bytes: 0,
        };
        let empty_authentication =
            authenticate_unverified_external_counts(&empty, SOURCE_ROOT, RAW_BINDING, zero_limits)
                .unwrap();
        let empty_artifact = retain_unverified_counts(
            &empty,
            &empty_authentication,
            RetentionRule::RetainAll,
            zero_limits,
        )
        .unwrap();
        assert_eq!(empty_artifact.accounted_peak_bytes(), 0);
    }

    #[test]
    fn support_overflow_is_checked_before_projection() {
        let mut raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 1)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        raw.edge_counts[0].support = u64::MAX;
        raw.edge_counts[1].support = 1;
        raw.support_events = u64::MAX;
        assert_code(
            authenticate_unverified_external_counts(&raw, SOURCE_ROOT, RAW_BINDING, limits()),
            ErrorCode::ResourceIntegerOverflow,
        );
    }

    #[test]
    fn deterministic_rebuild_preserves_all_scientific_and_operational_roots() {
        let raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3), ("AACCC", 4)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        let authentication = authenticate(&raw);
        let first = retain_unverified_counts(
            &raw,
            &authentication,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            limits(),
        )
        .unwrap();
        let second = retain_unverified_counts(
            &raw,
            &authentication,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            limits(),
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn operational_limits_never_change_successful_scientific_roots() {
        let raw = raw_counts(
            &[("AAAAA", 1), ("AAAAC", 2), ("AAACC", 3)],
            WideRunSupportUnit::AcceptedWindowOccurrence,
        );
        let authentication = authenticate(&raw);
        let generous = retain_unverified_counts(
            &raw,
            &authentication,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            limits(),
        )
        .unwrap();
        let exact_limits = RetentionLimits {
            max_raw_keys: generous.raw_key_count(),
            max_retained_keys: generous.retained_key_count(),
            max_accounted_bytes: generous.projected_peak_bytes(),
        };
        let exact = retain_unverified_counts(
            &raw,
            &authentication,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            exact_limits,
        )
        .unwrap();
        assert_eq!(
            generous.decision_ledger_root(),
            exact.decision_ledger_root()
        );
        assert_eq!(generous.retained_table_root(), exact.retained_table_root());
        assert_eq!(generous.retention_root(), exact.retention_root());
        assert_ne!(generous.operational_root, exact.operational_root);
    }

    #[test]
    fn empty_root_vectors_are_frozen_and_rule_specific() {
        let empty = raw_counts(&[], WideRunSupportUnit::AcceptedWindowOccurrence);
        let authentication = authenticate(&empty);
        let retain_all =
            retain_unverified_counts(&empty, &authentication, RetentionRule::RetainAll, limits())
                .unwrap();
        let threshold_one = retain_unverified_counts(
            &empty,
            &authentication,
            RetentionRule::InclusiveSupport { minimum_support: 1 },
            limits(),
        )
        .unwrap();

        // Constants are frozen after the first independently inspected test
        // execution; changing framing requires an algorithm-version change.
        assert_eq!(
            to_hex(authentication.raw_table_root),
            "2d6e7f82358a9cc628dc90834d8e96f5c14361cb4bb62063f48ecf742ea46702"
        );
        assert_eq!(
            to_hex(authentication.authentication_root),
            "057a50c607e177b0446e0ecc254f59668183dcdd09e40da1d71613d544d55951"
        );
        assert_eq!(
            to_hex(retain_all.decision_ledger_root()),
            "71e810093e54360967e412a9ea64774ea600570f8d97787f5686886ed4db5da2"
        );
        assert_eq!(
            to_hex(retain_all.retained_table_root()),
            "45b368b3d9486520f1573e188575d2db384a3e7a9339467e6138772050a9921b"
        );
        assert_eq!(
            to_hex(retain_all.retention_root()),
            "28f3a3520212d62e517efc76e1150eab83c6b4c23e183d4dc7bf472b2d73973d"
        );
        assert_eq!(
            to_hex(threshold_one.decision_ledger_root()),
            "999a5ecd5be4b1de647b749883568bf6885db5ae30ca14d2bac7b3bcb69ba91a"
        );
        assert_eq!(
            to_hex(threshold_one.retained_table_root()),
            "7d50154a30cab39ccf470ee48913656aba65fc397b2e193525e0de2cbab0a4c2"
        );
        assert_eq!(
            to_hex(threshold_one.retention_root()),
            "d4f2191a3f6f86e27f471f403926166070ad8513164bca13b80bdfcb2fee4bce"
        );
    }

    fn scientific() -> ScientificConfig {
        ScientificConfig::resolve(
            K,
            Profile::RetainAll,
            SupportUnit::AcceptedWindowOccurrence,
            None,
            0,
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

    fn spool_options(work_dir: &Path) -> SpoolExternalOptions {
        SpoolExternalOptions {
            work_dir: work_dir.to_path_buf(),
            k: K,
            minimizer_length: M,
            virtual_bucket_count: BUCKETS,
            support_unit: SupportUnit::AcceptedWindowOccurrence,
            max_fragment_decode_bytes: 1 << 20,
            max_fragment_windows: 10_000,
            limits: external_limits(),
        }
    }

    #[test]
    fn public_capability_requires_spool_and_replays_before_retention() {
        let signature: fn(
            &Spool,
            &SpoolExternalOptions,
            RetentionLimits,
        ) -> Result<SpoolAuthenticatedRaw> = authenticate_spool_external_counts;
        let _ = signature;

        let temp = tempfile::tempdir().unwrap();
        let input_path = temp.path().join("reads.fasta");
        fs::write(&input_path, b">r1\nAAAAAACGTAAAAA\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &spool_limits(),
            temp.path(),
        )
        .unwrap();
        let work = temp.path().join("retention-work");
        fs::create_dir(&work).unwrap();
        let options = spool_options(&work);
        let raw = authenticate_spool_external_counts(&spool, &options, limits()).unwrap();
        let upstream = raw.spool_external_result().unwrap().validated().unwrap();
        assert_eq!(upstream.integrity_verification_calls(), 3);
        assert_eq!(upstream.integrity_physical_read_passes(), 3);
        assert_eq!(upstream.scientific_replay_passes(), 2);
        assert_eq!(upstream.total_physical_spool_read_passes(), 5);
        let artifact = retain_spool_authenticated_counts(
            &raw,
            RetentionRule::InclusiveSupport { minimum_support: 2 },
            limits(),
        )
        .unwrap();
        artifact.validate_against_authenticated_raw(&raw).unwrap();
        artifact.validate_against_spool(&spool, &options).unwrap();
        assert_eq!(
            artifact.source_equivalence().root(),
            raw.source_equivalence().root()
        );
        assert_eq!(
            artifact.retained_counts().unwrap().source_identity,
            artifact.retention_root()
        );
        assert!(artifact.retained_key_count() < artifact.raw_key_count());
    }

    #[test]
    fn tampered_spool_capability_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let input_path = temp.path().join("reads.fasta");
        fs::write(&input_path, b">r1\nAAAAAACGTAAAAA\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &spool_limits(),
            temp.path(),
        )
        .unwrap();
        let work = temp.path().join("tamper-work");
        fs::create_dir(&work).unwrap();
        let mut capability =
            authenticate_spool_external_counts(&spool, &spool_options(&work), limits()).unwrap();
        capability.authentication.raw_binding[0] ^= 1;
        assert_code(
            capability.spool_external_result(),
            ErrorCode::IntegrityArtifact,
        );
    }

    #[test]
    fn public_artifact_detects_source_equivalence_and_operational_root_tampering() {
        let temp = tempfile::tempdir().unwrap();
        let input_path = temp.path().join("reads.fasta");
        fs::write(&input_path, b">r1\nAAAAAACGTAAAAA\n").unwrap();
        let spool = create_spool(
            &InputSpec::Single(vec![input_path]),
            &scientific(),
            &spool_limits(),
            temp.path(),
        )
        .unwrap();
        let work = temp.path().join("artifact-work");
        fs::create_dir(&work).unwrap();
        let raw =
            authenticate_spool_external_counts(&spool, &spool_options(&work), limits()).unwrap();

        let mut source =
            retain_spool_authenticated_counts(&raw, RetentionRule::RetainAll, limits()).unwrap();
        source.source_equivalence.root[0] ^= 1;
        assert_code(source.retained_counts(), ErrorCode::IntegrityArtifact);

        let mut operational =
            retain_spool_authenticated_counts(&raw, RetentionRule::RetainAll, limits()).unwrap();
        operational.operational_root[0] ^= 1;
        assert_code(operational.retained_counts(), ErrorCode::IntegrityArtifact);
    }
}
