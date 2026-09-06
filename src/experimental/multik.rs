//! Independent exact multi-k child layers and cross-layer evidence.
//!
//! This module is an in-memory experimental library slice. It is deliberately
//! disconnected from the stable assembly command and emits no FASTA, GFA, or
//! projected sequence. Every child is rebuilt independently from the same
//! caller-supplied post-QC [`AcceptedSegment`] values. Cross-layer rows annotate
//! exact child edges; they never add a key, increment child support, choose a
//! path, or merge sequence.
//!
//! Relations are constructed only between consecutive configured k values:
//!
//! - [`RelationEvidence::Supports`] says that one exact higher-k edge contains
//!   a particular directed adjacency of two oriented lower-k edges.
//! - [`RelationEvidence::Contains`] records one exact lower-edge embedding in
//!   a higher-k canonical spelling.
//! - [`RelationEvidence::Conflicts`] records every exact higher-k witness in a
//!   group having at least two incompatible immediate bases on one canonical
//!   side of a lower edge. This is a sequence-context conflict, not a claim of
//!   a variant, strain, or biological origin.
//! - [`RelationEvidence::Unresolved`] counts lower-edge occurrence sides that
//!   no accepted higher-k window can cover within the same immutable segment.
//!
//! Higher-k occurrence counts remain evidence belonging to the higher child;
//! they are not reinterpreted as fragment, molecule, abundance, or confidence
//! counts. The current child substrate has occurrence support only.

use super::partitioned_dbg::{
    build_partitioned_dbg, AcceptedSegment, PartitionConfig, PartitionedDbg,
};
use super::wide_kmer::{
    canonical_code, decode_mer, encode_exact_bases, reverse_complement_code, validate_code,
    PackedKmer,
};
use crate::error::{ErrorCode, Result, VeritasmError};
use std::cmp::Ordering;
use std::mem::size_of;

/// Explicit parent-level limits for the in-memory multi-k experiment.
///
/// Child allocations remain governed by each [`PartitionConfig`]. The total
/// child ceiling is conservatively admitted by summing those configured child
/// ceilings before any child is built. Relation accounting covers vector
/// payloads owned by this module and its largest validation scratch, but not
/// allocator metadata, stacks, caller-owned segments, or whole-process RSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiKLimits {
    /// Maximum number of independent exact child layers.
    pub max_children: u16,
    /// Maximum number of accepted source segments indexed by the parent.
    pub max_segments: u64,
    /// Maximum sum of child `max_accounted_bytes` limits.
    pub max_total_child_accounted_bytes: u64,
    /// Maximum accepted windows materialized by the independent slice oracle
    /// for any one child.
    pub max_oracle_windows_per_child: u64,
    /// Maximum conservative number of candidate relation/scratch events.
    pub max_relation_events: u64,
    /// Maximum conservative number of returned relation rows.
    pub max_relation_records: u64,
    /// Maximum projected relation and validation-scratch vector payload.
    pub max_relation_accounted_bytes: u64,
}

/// Frozen child configurations and parent limits.
///
/// Child k values must be strictly increasing and therefore unique. At least
/// two children are required. Minimizer and virtual-bucket choices may differ
/// by child and affect only that child's independently built partition result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiKConfig {
    pub children: Vec<PartitionConfig>,
    pub limits: MultiKLimits,
}

/// Orientation of one exact lower-k spelling relative to its canonical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeOrientation {
    Canonical,
    ReverseComplement,
}

/// One exact oriented child edge.
///
/// Both fields are retained intentionally. Validation proves that `key` is a
/// complete canonical key and that canonicalizing `spelling` produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrientedEdge {
    pub key: PackedKmer,
    pub spelling: PackedKmer,
    pub orientation: EdgeOrientation,
}

/// A side in the canonical orientation of a lower-k edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CanonicalSide {
    Left,
    Right,
    /// The edge is self-reverse-complemental, so left and right are exchanged
    /// by an orientation symmetry and cannot be distinguished canonically.
    FixedPoint,
}

impl CanonicalSide {
    const fn flip(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::FixedPoint => Self::FixedPoint,
        }
    }
}

/// One higher edge proves an existing directed lower-edge adjacency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SupportsRelation {
    pub higher_key: PackedKmer,
    pub from: OrientedEdge,
    pub to: OrientedEdge,
    /// Start of `from` in the canonical higher-k spelling. `to` starts one
    /// base later.
    pub first_offset: u8,
    pub higher_window_occurrences: u64,
}

/// One exact lower-edge embedding inside an observed higher edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContainsRelation {
    pub higher_key: PackedKmer,
    pub lower: OrientedEdge,
    /// Start of the oriented lower spelling in the canonical higher spelling.
    pub offset: u8,
    pub higher_window_occurrences: u64,
}

/// One witness in a canonical-side group containing incompatible bases.
///
/// All rows for the same `(lower key, canonical side)` are retained when that
/// group contains at least two distinct bases. `embedded` is the exact
/// orientation seen inside `higher_key`; `canonical_base` is complemented and
/// side-normalized when `embedded` is reverse-complement oriented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConflictsRelation {
    pub higher_key: PackedKmer,
    pub embedded: OrientedEdge,
    /// Literal side of `embedded` inside the canonical higher spelling.
    pub embedded_side: CanonicalSide,
    pub canonical_side: CanonicalSide,
    pub canonical_base: u8,
    pub offset: u8,
    pub higher_window_occurrences: u64,
}

/// Lower-edge occurrence sides not coverable by a higher-k window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnresolvedRelation {
    pub lower_key: PackedKmer,
    pub canonical_side: CanonicalSide,
    pub lower_window_occurrences: u64,
}

/// Typed, non-projecting cross-layer evidence.
///
/// Declaration order is the deterministic within-layer-pair serialization
/// order: supports, contains, conflicts, then unresolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationEvidence {
    Supports(SupportsRelation),
    Contains(ContainsRelation),
    Conflicts(ConflictsRelation),
    Unresolved(UnresolvedRelation),
}

/// One relation between consecutive independent child layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MultiKRelation {
    pub lower_k: u8,
    pub higher_k: u8,
    pub evidence: RelationEvidence,
}

/// Deterministic result of the experimental multi-k evidence plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiKEvidenceLattice {
    pub segment_count: u64,
    pub input_bases: u64,
    /// Identity of the complete sorted accepted-segment stream shared by all
    /// independent children.
    pub source_identity: [u8; 32],
    /// Sum of the child partition builders' projected payload values.
    pub child_projected_payload_bytes: u64,
    /// Conservative pre-materialization relation/scratch event bound.
    pub relation_event_upper_bound: u64,
    /// Conservative vector-payload bound admitted for relations and the
    /// largest source-validation scratch.
    pub relation_projected_payload_bytes: u64,
    /// Strictly increasing, independent exact child graphs.
    pub children: Vec<PartitionedDbg>,
    /// Strictly ordered cross-layer annotations; never a path projection.
    pub relations: Vec<MultiKRelation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EdgeCountEntry {
    key: PackedKmer,
    occurrences: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ContextCandidate {
    lower_key: PackedKmer,
    canonical_side: CanonicalSide,
    canonical_base: u8,
    higher_key: PackedKmer,
    embedded: OrientedEdge,
    embedded_side: CanonicalSide,
    offset: u8,
    higher_window_occurrences: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct UnresolvedAtom {
    lower_key: PackedKmer,
    canonical_side: CanonicalSide,
}

#[derive(Debug, Clone, Copy, Default)]
struct RelationBounds {
    events: u64,
    records: u64,
    maximum_contexts: u64,
    maximum_unresolved_atoms: u64,
    maximum_pair_edge_indices: u64,
    maximum_core_records: u64,
    maximum_unresolved_records: u64,
    maximum_oracle_windows: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SupportValidationWork {
    edge_index_builds: u64,
    support_relations: u64,
}

/// Build independent child graphs and a non-projecting evidence lattice.
///
/// The accepted segments are borrowed and never modified. Their order is not
/// semantic: unique `(source_ordinal, segment_ordinal)` coordinates define the
/// canonical order. Each child is checked against an independent exact slice
/// recount before cross-layer evidence is accepted.
pub fn build_multik_evidence_lattice(
    config: &MultiKConfig,
    segments: &[AcceptedSegment<'_>],
) -> Result<MultiKEvidenceLattice> {
    validate_config(config)?;
    let segment_count = u64::try_from(segments.len())
        .map_err(|_| overflow("experimental multi-k segment count does not fit in u64"))?;
    enforce_limit(
        segment_count,
        config.limits.max_segments,
        ErrorCode::ResourceRetainedKeys,
        "experimental multi-k accepted segments",
    )?;
    let input_bases = count_input_bases(segments)?;
    let child_window_counts = preflight_parent_allocations(config, segments, segment_count)?;
    let segment_order = validate_and_order_segments(segments)?;

    let mut children = Vec::new();
    children
        .try_reserve_exact(config.children.len())
        .map_err(|cause| {
            resource_memory(format!("cannot reserve multi-k child layers: {cause}"))
        })?;
    enforce_exact_capacity(
        children.capacity(),
        config.children.len(),
        "multi-k child-layer vector",
    )?;
    let mut child_projected_payload_bytes = 0_u64;
    for (child_config, &planned_windows) in config.children.iter().zip(child_window_counts.iter()) {
        let child = build_partitioned_dbg(*child_config, segments)?;
        child.validate_against_segments(segments)?;
        if child.accepted_windows != planned_windows {
            return invariant("experimental multi-k child window preflight changed during build");
        }
        let oracle_scratch_bytes = projected_oracle_recount_bytes(child.accepted_windows)?;
        enforce_limit(
            oracle_scratch_bytes,
            config.limits.max_relation_accounted_bytes,
            ErrorCode::ResourceMemory,
            "experimental multi-k source-oracle scratch bytes",
        )?;
        let source_counts = slice_oracle_counts(
            segments,
            &segment_order,
            child.k,
            config.limits.max_oracle_windows_per_child,
        )?;
        let child_counts = edge_count_index(&child)?;
        if source_counts != child_counts {
            return invariant("experimental multi-k child differs from exact source-slice oracle");
        }
        child_projected_payload_bytes = checked_add(
            child_projected_payload_bytes,
            child.projected_payload_bytes,
            "experimental multi-k child projected-payload sum overflow",
        )?;
        enforce_limit(
            child_projected_payload_bytes,
            config.limits.max_total_child_accounted_bytes,
            ErrorCode::ResourceMemory,
            "experimental multi-k child projected payload bytes",
        )?;
        children.push(child);
    }
    drop(child_window_counts);
    let source_identity = children[0].source_identity;
    if children
        .iter()
        .any(|child| child.source_identity != source_identity)
    {
        return invariant("experimental multi-k children do not share one source identity");
    }

    let bounds = relation_bounds(&children)?;
    enforce_limit(
        bounds.events,
        config.limits.max_relation_events,
        ErrorCode::ResourceRetainedKeys,
        "experimental multi-k relation events",
    )?;
    enforce_limit(
        bounds.records,
        config.limits.max_relation_records,
        ErrorCode::ResourceRetainedKeys,
        "experimental multi-k relation records",
    )?;
    let relation_projected_payload_bytes =
        projected_relation_bytes(bounds, segments.len(), config.children.len())?;
    enforce_limit(
        relation_projected_payload_bytes,
        config.limits.max_relation_accounted_bytes,
        ErrorCode::ResourceMemory,
        "experimental multi-k relation projected payload bytes",
    )?;

    let relation_capacity = as_usize(bounds.records, "multi-k relation-record capacity")?;
    let mut relations = Vec::new();
    relations
        .try_reserve_exact(relation_capacity)
        .map_err(|cause| resource_memory(format!("cannot reserve multi-k relations: {cause}")))?;
    enforce_exact_capacity(
        relations.capacity(),
        relation_capacity,
        "multi-k relation vector",
    )?;

    for pair in children.windows(2) {
        let lower = &pair[0];
        let higher = &pair[1];
        let lower_index = edge_count_index(lower)?;
        let mut core = derive_core_relations(lower, higher, &lower_index)?;
        relations.append(&mut core);
        let mut unresolved =
            derive_unresolved_relations(lower, higher, segments, &segment_order, &lower_index)?;
        relations.append(&mut unresolved);
    }
    relations.sort_unstable();
    enforce_exact_capacity(
        relations.capacity(),
        relation_capacity,
        "multi-k materialized relation vector",
    )?;
    if relations.windows(2).any(|pair| pair[0] >= pair[1]) {
        return invariant("experimental multi-k relations are not strictly unique after sorting");
    }

    let lattice = MultiKEvidenceLattice {
        segment_count,
        input_bases,
        source_identity,
        child_projected_payload_bytes,
        relation_event_upper_bound: bounds.events,
        relation_projected_payload_bytes,
        children,
        relations,
    };
    lattice.validate_against_segments(segments)?;
    Ok(lattice)
}

fn preflight_parent_allocations(
    config: &MultiKConfig,
    segments: &[AcceptedSegment<'_>],
    segment_count: u64,
) -> Result<Vec<u64>> {
    let child_count = u64::try_from(config.children.len())
        .map_err(|_| overflow("experimental multi-k child count does not fit u64"))?;
    let mut front_bytes = 0_u64;
    front_bytes = checked_payload_add::<usize>(front_bytes, segment_count, "segment order")?;
    front_bytes =
        checked_payload_add::<PartitionedDbg>(front_bytes, child_count, "child vector payload")?;
    front_bytes = checked_payload_add::<u64>(front_bytes, child_count, "child window preflight")?;
    enforce_limit(
        front_bytes,
        config.limits.max_relation_accounted_bytes,
        ErrorCode::ResourceMemory,
        "experimental multi-k parent front-matter bytes",
    )?;

    let mut counts = Vec::new();
    counts
        .try_reserve_exact(config.children.len())
        .map_err(|cause| {
            resource_memory(format!(
                "cannot reserve multi-k child-window preflight: {cause}"
            ))
        })?;
    enforce_exact_capacity(
        counts.capacity(),
        config.children.len(),
        "multi-k child-window preflight vector",
    )?;
    for child in &config.children {
        let k = usize::from(child.k);
        let windows = segments.iter().try_fold(0_u64, |total, segment| {
            let count = segment
                .bases
                .len()
                .checked_sub(k)
                .and_then(|difference| difference.checked_add(1))
                .unwrap_or(0);
            checked_add(
                total,
                u64::try_from(count).map_err(|_| {
                    overflow("experimental multi-k preflight windows do not fit u64")
                })?,
                "experimental multi-k preflight window sum overflow",
            )
        })?;
        enforce_limit(
            windows,
            config.limits.max_oracle_windows_per_child,
            ErrorCode::ResourceRetainedKeys,
            "experimental multi-k per-child oracle windows",
        )?;
        let phase_bytes = checked_add(
            front_bytes,
            projected_oracle_recount_bytes(windows)?,
            "experimental multi-k oracle/front-matter overflow",
        )?;
        enforce_limit(
            phase_bytes,
            config.limits.max_relation_accounted_bytes,
            ErrorCode::ResourceMemory,
            "experimental multi-k pre-build oracle phase bytes",
        )?;
        counts.push(windows);
    }
    Ok(counts)
}

impl MultiKEvidenceLattice {
    /// Validate child identity, relation completeness, conflict grouping, and
    /// the rule that every `supports` adjacency occurs inside one exact
    /// observed higher-k child edge.
    pub fn validate_invariants(&self) -> Result<()> {
        if self.children.len() < 2 {
            return invariant("experimental multi-k lattice has fewer than two children");
        }
        for child in &self.children {
            child.validate_invariants()?;
            let _ = edge_count_index(child)?;
        }
        for child in &self.children {
            let child_segment_count = u64::try_from(child.segments.len()).map_err(|_| {
                overflow("experimental multi-k child segment count does not fit in u64")
            })?;
            if child.source_identity != self.source_identity
                || child.input_bases != self.input_bases
                || child_segment_count != self.segment_count
            {
                return invariant(
                    "experimental multi-k child source metadata disagrees with parent",
                );
            }
        }
        let projected_child_sum = self.children.iter().try_fold(0_u64, |total, child| {
            checked_add(
                total,
                child.projected_payload_bytes,
                "experimental multi-k child projected-payload validation overflow",
            )
        })?;
        if projected_child_sum != self.child_projected_payload_bytes {
            return invariant("experimental multi-k child projected-payload total is wrong");
        }
        let bounds = relation_bounds(&self.children)?;
        if bounds.events != self.relation_event_upper_bound {
            return invariant("experimental multi-k relation event bound is wrong");
        }
        if u64::try_from(self.relations.len())
            .map_err(|_| overflow("experimental multi-k relation count does not fit in u64"))?
            > bounds.records
        {
            return invariant("experimental multi-k relation count exceeds its conservative bound");
        }
        let segment_count = usize::try_from(self.segment_count)
            .map_err(|_| resource_memory("multi-k segment count does not fit usize".to_owned()))?;
        if projected_relation_bytes(bounds, segment_count, self.children.len())?
            != self.relation_projected_payload_bytes
        {
            return invariant("experimental multi-k relation payload bound is wrong");
        }
        for pair in self.children.windows(2) {
            if pair[0].k >= pair[1].k {
                return invariant("experimental multi-k children are not strictly ordered by k");
            }
        }
        if self.relations.windows(2).any(|pair| pair[0] >= pair[1]) {
            return invariant("experimental multi-k relation rows are not strictly ordered");
        }

        for pair in self.children.windows(2) {
            let lower = &pair[0];
            let higher = &pair[1];
            let lower_index = edge_count_index(lower)?;
            let higher_index = edge_count_index(higher)?;
            let expected = derive_core_relations(lower, higher, &lower_index)?;
            let (relation_start, relation_end) =
                relation_pair_bounds(&self.relations, lower.k, higher.k);
            let pair_relations = &self.relations[relation_start..relation_end];
            let actual_core_count = pair_relations
                .iter()
                .filter(|relation| !matches!(relation.evidence, RelationEvidence::Unresolved(_)))
                .count();
            if actual_core_count != expected.len()
                || expected
                    .iter()
                    .any(|relation| pair_relations.binary_search(relation).is_err())
            {
                return invariant(
                    "experimental multi-k core relations are incomplete or contain unsupported rows",
                );
            }

            for relation in pair_relations {
                validate_relation_structure(relation, lower, higher, &lower_index, &higher_index)?;
            }
        }

        for relation in &self.relations {
            let _ =
                consecutive_child_pair_index(&self.children, relation.lower_k, relation.higher_k)?;
        }
        self.validate_no_unsupported_adjacencies()
    }

    /// Independently replay every support relation inside its complete
    /// higher-k canonical spelling.
    ///
    /// This oracle accepts no adjacency on key hashes, minimizers, bucket IDs,
    /// support magnitude, or small-k traversability. Other relation variants
    /// create no adjacency and are checked by [`Self::validate_invariants`].
    /// Sorted child-edge indexes are built once per supported adjacent layer
    /// pair, never once per support row.
    pub fn validate_no_unsupported_adjacencies(&self) -> Result<()> {
        let work = self.validate_no_unsupported_adjacencies_indexed()?;
        let maximum_index_builds = u64::try_from(self.children.len().saturating_sub(1))
            .map_err(|_| overflow("experimental multi-k child-pair count does not fit in u64"))?
            .checked_mul(2)
            .ok_or_else(|| overflow("experimental multi-k support index-build bound overflow"))?;
        if work.edge_index_builds > maximum_index_builds {
            return invariant(
                "experimental multi-k support validation rebuilt a child index within one pair",
            );
        }
        Ok(())
    }

    fn validate_no_unsupported_adjacencies_indexed(&self) -> Result<SupportValidationWork> {
        if self.children.windows(2).any(|pair| pair[0].k >= pair[1].k) {
            return invariant("experimental multi-k children are not strictly ordered by k");
        }
        if self.relations.windows(2).any(|pair| pair[0] >= pair[1]) {
            return invariant("experimental multi-k relation rows are not strictly ordered");
        }

        let mut expected_support_relations = 0_u64;
        for relation in &self.relations {
            if !matches!(relation.evidence, RelationEvidence::Supports(_)) {
                continue;
            }
            expected_support_relations = checked_add(
                expected_support_relations,
                1,
                "experimental multi-k support-relation count overflow",
            )?;
            let _ =
                consecutive_child_pair_index(&self.children, relation.lower_k, relation.higher_k)?;
        }

        let mut work = SupportValidationWork::default();
        for pair in self.children.windows(2) {
            let lower = &pair[0];
            let higher = &pair[1];
            let (relation_start, relation_end) =
                relation_pair_bounds(&self.relations, lower.k, higher.k);
            let relations = &self.relations[relation_start..relation_end];
            if !relations
                .iter()
                .any(|relation| matches!(relation.evidence, RelationEvidence::Supports(_)))
            {
                continue;
            }
            let lower_index = edge_count_index(lower)?;
            let higher_index = edge_count_index(higher)?;
            work.edge_index_builds = checked_add(
                work.edge_index_builds,
                2,
                "experimental multi-k support index-build count overflow",
            )?;
            for relation in relations {
                let RelationEvidence::Supports(support) = relation.evidence else {
                    continue;
                };
                validate_support_relation(support, lower, higher, &lower_index, &higher_index)?;
                work.support_relations = checked_add(
                    work.support_relations,
                    1,
                    "experimental multi-k validated-support count overflow",
                )?;
            }
        }
        if work.support_relations != expected_support_relations {
            return invariant(
                "experimental multi-k support validation did not visit every support relation",
            );
        }
        Ok(work)
    }

    /// Recount every child from source slices and recompute all unresolved
    /// source-boundary rows.
    ///
    /// This is the strongest in-memory oracle exposed by the slice. It must be
    /// given the same immutable logical segments; changing coordinates or
    /// bases is an integrity error rather than a request to reuse the lattice.
    pub fn validate_against_segments(&self, segments: &[AcceptedSegment<'_>]) -> Result<()> {
        self.validate_invariants()?;
        let order = validate_and_order_segments(segments)?;
        let observed_segment_count = u64::try_from(segments.len())
            .map_err(|_| overflow("experimental multi-k segment count does not fit in u64"))?;
        if observed_segment_count != self.segment_count
            || count_input_bases(segments)? != self.input_bases
        {
            return invariant("experimental multi-k source-segment totals changed");
        }

        for child in &self.children {
            child.validate_against_segments(segments)?;
            let source_counts =
                slice_oracle_counts(segments, &order, child.k, child.accepted_windows)?;
            if source_counts != edge_count_index(child)? {
                return invariant(
                    "experimental multi-k child is not an exact source-slice recount",
                );
            }
        }

        for pair in self.children.windows(2) {
            let lower = &pair[0];
            let higher = &pair[1];
            let lower_index = edge_count_index(lower)?;
            let expected =
                derive_unresolved_relations(lower, higher, segments, &order, &lower_index)?;
            let (relation_start, relation_end) =
                relation_pair_bounds(&self.relations, lower.k, higher.k);
            let pair_relations = &self.relations[relation_start..relation_end];
            let actual_count = pair_relations
                .iter()
                .filter(|relation| matches!(relation.evidence, RelationEvidence::Unresolved(_)))
                .count();
            if actual_count != expected.len()
                || expected
                    .iter()
                    .any(|relation| pair_relations.binary_search(relation).is_err())
            {
                return invariant("experimental multi-k unresolved rows differ from source oracle");
            }
        }
        Ok(())
    }
}

fn consecutive_child_pair_index(
    children: &[PartitionedDbg],
    lower_k: u8,
    higher_k: u8,
) -> Result<usize> {
    let lower_index = children
        .binary_search_by_key(&lower_k, |child| child.k)
        .map_err(|_| {
            invariant_error("experimental multi-k relation names a nonconsecutive child pair")
        })?;
    let higher_index = lower_index
        .checked_add(1)
        .ok_or_else(|| overflow("experimental multi-k child-pair index overflow"))?;
    if children.get(higher_index).map(|child| child.k) != Some(higher_k) {
        return invariant("experimental multi-k relation names a nonconsecutive child pair");
    }
    Ok(lower_index)
}

fn relation_pair_bounds(relations: &[MultiKRelation], lower_k: u8, higher_k: u8) -> (usize, usize) {
    let pair_key = (lower_k, higher_k);
    let start =
        relations.partition_point(|relation| (relation.lower_k, relation.higher_k) < pair_key);
    let end =
        relations.partition_point(|relation| (relation.lower_k, relation.higher_k) <= pair_key);
    (start, end)
}

fn validate_config(config: &MultiKConfig) -> Result<()> {
    let child_count = u16::try_from(config.children.len()).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ConfigurationUnsupportedCombination,
            "experimental multi-k child count does not fit in u16",
        )
    })?;
    if child_count < 2 {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationUnsupportedCombination,
            "experimental multi-k evidence requires at least two child k values",
        ));
    }
    if child_count > config.limits.max_children {
        return Err(VeritasmError::new(
            ErrorCode::ConfigurationInvalidLimit,
            format!(
                "experimental multi-k child count {child_count} exceeds configured limit {}",
                config.limits.max_children
            ),
        ));
    }

    let mut prior_k = None;
    let mut configured_child_bytes = 0_u64;
    for child in &config.children {
        if prior_k.is_some_and(|prior| prior >= child.k) {
            return Err(VeritasmError::new(
                ErrorCode::ConfigurationInvalidK,
                "experimental multi-k child k values must be strictly increasing and unique",
            ));
        }
        prior_k = Some(child.k);
        configured_child_bytes = checked_add(
            configured_child_bytes,
            child.limits.max_accounted_bytes,
            "experimental multi-k configured child-byte sum overflow",
        )?;
    }
    enforce_limit(
        configured_child_bytes,
        config.limits.max_total_child_accounted_bytes,
        ErrorCode::ResourceMemory,
        "experimental multi-k configured child accounted bytes",
    )
}

fn validate_and_order_segments(segments: &[AcceptedSegment<'_>]) -> Result<Vec<usize>> {
    let mut order = Vec::new();
    order.try_reserve_exact(segments.len()).map_err(|cause| {
        resource_memory(format!("cannot reserve multi-k segment order: {cause}"))
    })?;
    enforce_exact_capacity(
        order.capacity(),
        segments.len(),
        "multi-k segment-order vector",
    )?;
    order.extend(0..segments.len());
    order.sort_unstable_by_key(|&index| {
        (
            segments[index].source_ordinal,
            segments[index].segment_ordinal,
        )
    });

    let mut previous = None;
    for &index in &order {
        let segment = segments[index];
        let coordinate = (segment.source_ordinal, segment.segment_ordinal);
        if previous == Some(coordinate) {
            return Err(VeritasmError::new(
                ErrorCode::IntegrityOrdinalCoverage,
                format!(
                    "duplicate experimental multi-k accepted-segment coordinate {}/{}",
                    coordinate.0, coordinate.1
                ),
            ));
        }
        previous = Some(coordinate);
        for (position, &base) in segment.bases.iter().enumerate() {
            if exact_base_bits(base).is_none() {
                return Err(VeritasmError::new(
                    ErrorCode::InputNucleotide,
                    format!(
                        "experimental multi-k accepted segment {}/{} contains non-ACGT byte 0x{base:02x} at zero-based position {position}",
                        coordinate.0, coordinate.1
                    ),
                ));
            }
        }
    }
    Ok(order)
}

fn count_input_bases(segments: &[AcceptedSegment<'_>]) -> Result<u64> {
    segments.iter().try_fold(0_u64, |total, segment| {
        let length = u64::try_from(segment.bases.len())
            .map_err(|_| overflow("experimental multi-k segment length does not fit in u64"))?;
        checked_add(
            total,
            length,
            "experimental multi-k input-base count overflow",
        )
    })
}

fn edge_count_index(child: &PartitionedDbg) -> Result<Vec<EdgeCountEntry>> {
    let mut index = Vec::new();
    index
        .try_reserve_exact(child.edge_counts.len())
        .map_err(|cause| resource_memory(format!("cannot reserve multi-k edge index: {cause}")))?;
    enforce_exact_capacity(
        index.capacity(),
        child.edge_counts.len(),
        "multi-k edge-index vector",
    )?;
    for row in &child.edge_counts {
        validate_canonical_key(row.key, child.k)?;
        if row.occurrences == 0 {
            return invariant("experimental multi-k child contains a zero-support edge");
        }
        index.push(EdgeCountEntry {
            key: row.key,
            occurrences: row.occurrences,
        });
    }
    index.sort_unstable_by_key(|entry| entry.key);
    if index.windows(2).any(|pair| pair[0].key >= pair[1].key) {
        return invariant("experimental multi-k child edge keys are not globally unique");
    }
    Ok(index)
}

fn slice_oracle_counts(
    segments: &[AcceptedSegment<'_>],
    order: &[usize],
    k: u8,
    max_windows: u64,
) -> Result<Vec<EdgeCountEntry>> {
    let k_usize = usize::from(k);
    let window_count = segments.iter().try_fold(0_u64, |total, segment| {
        let count = segment
            .bases
            .len()
            .checked_sub(k_usize)
            .and_then(|difference| difference.checked_add(1))
            .unwrap_or(0);
        let count = u64::try_from(count)
            .map_err(|_| overflow("experimental multi-k oracle window count does not fit u64"))?;
        checked_add(
            total,
            count,
            "experimental multi-k oracle-window sum overflow",
        )
    })?;
    enforce_limit(
        window_count,
        max_windows,
        ErrorCode::ResourceRetainedKeys,
        "experimental multi-k oracle windows",
    )?;

    let oracle_capacity = as_usize(window_count, "multi-k oracle windows")?;
    let mut keys = Vec::new();
    keys.try_reserve_exact(oracle_capacity).map_err(|cause| {
        resource_memory(format!("cannot reserve multi-k slice oracle: {cause}"))
    })?;
    enforce_exact_capacity(
        keys.capacity(),
        oracle_capacity,
        "multi-k source-oracle key vector",
    )?;
    for &index in order {
        for window in segments[index].bases.windows(k_usize) {
            let spelling = encode_exact_bases(window)?;
            keys.push(canonical_code(spelling, k)?);
        }
    }
    keys.sort_unstable();
    let count_capacity = keys.len();
    let mut counts: Vec<EdgeCountEntry> = Vec::new();
    counts.try_reserve_exact(count_capacity).map_err(|cause| {
        resource_memory(format!("cannot reserve multi-k oracle counts: {cause}"))
    })?;
    enforce_exact_capacity(
        counts.capacity(),
        count_capacity,
        "multi-k source-oracle count vector",
    )?;
    for key in keys {
        if let Some(last) = counts.last_mut().filter(|last| last.key == key) {
            last.occurrences = checked_add(
                last.occurrences,
                1,
                "experimental multi-k oracle occurrence count overflow",
            )?;
        } else {
            counts.push(EdgeCountEntry {
                key,
                occurrences: 1,
            });
        }
    }
    Ok(counts)
}

fn derive_core_relations(
    lower: &PartitionedDbg,
    higher: &PartitionedDbg,
    lower_index: &[EdgeCountEntry],
) -> Result<Vec<MultiKRelation>> {
    let delta = higher
        .k
        .checked_sub(lower.k)
        .ok_or_else(|| invariant_error("experimental multi-k child order moved backwards"))?;
    if delta == 0 {
        return invariant("experimental multi-k adjacent children have equal k");
    }
    let high_distinct = u64::try_from(higher.edge_counts.len())
        .map_err(|_| overflow("experimental multi-k high-edge count does not fit in u64"))?;
    let contains = checked_mul(
        high_distinct,
        u64::from(delta) + 1,
        "experimental multi-k containment count overflow",
    )?;
    let supports = checked_mul(
        high_distinct,
        u64::from(delta),
        "experimental multi-k support-relation count overflow",
    )?;
    let relation_capacity = checked_add(
        checked_add(
            contains,
            supports,
            "experimental multi-k core count overflow",
        )?,
        checked_mul(
            contains,
            2,
            "experimental multi-k conflict upper bound overflow",
        )?,
        "experimental multi-k core relation upper bound overflow",
    )?;
    let context_capacity = checked_mul(
        contains,
        2,
        "experimental multi-k context-candidate upper bound overflow",
    )?;

    let relation_capacity = as_usize(relation_capacity, "multi-k core relation capacity")?;
    let mut relations = Vec::new();
    relations
        .try_reserve_exact(relation_capacity)
        .map_err(|cause| {
            resource_memory(format!("cannot reserve multi-k core relations: {cause}"))
        })?;
    enforce_exact_capacity(
        relations.capacity(),
        relation_capacity,
        "multi-k core-relation vector",
    )?;
    let context_capacity = as_usize(context_capacity, "multi-k context capacity")?;
    let mut contexts = Vec::new();
    contexts
        .try_reserve_exact(context_capacity)
        .map_err(|cause| resource_memory(format!("cannot reserve multi-k contexts: {cause}")))?;
    enforce_exact_capacity(
        contexts.capacity(),
        context_capacity,
        "multi-k context-candidate vector",
    )?;

    let lower_length = usize::from(lower.k);
    for higher_count in &higher.edge_counts {
        let higher_sequence = validate_canonical_key(higher_count.key, higher.k)?;
        let last_offset = usize::from(delta);
        for offset in 0..=last_offset {
            let lower_sequence = &higher_sequence[offset..offset + lower_length];
            let embedded = oriented_edge(lower_sequence, lower.k)?;
            find_edge(lower_index, embedded.key).ok_or_else(|| {
                invariant_error("higher child contains an edge absent from the lower child")
            })?;
            let offset_u8 = u8::try_from(offset).map_err(|_| {
                overflow("experimental multi-k embedding offset does not fit in u8")
            })?;
            relations.push(MultiKRelation {
                lower_k: lower.k,
                higher_k: higher.k,
                evidence: RelationEvidence::Contains(ContainsRelation {
                    higher_key: higher_count.key,
                    lower: embedded,
                    offset: offset_u8,
                    higher_window_occurrences: higher_count.occurrences,
                }),
            });

            if offset > 0 {
                contexts.push(context_candidate(
                    higher_count,
                    embedded,
                    lower.k,
                    offset_u8,
                    CanonicalSide::Left,
                    higher_sequence[offset - 1],
                )?);
            }
            if offset + lower_length < higher_sequence.len() {
                contexts.push(context_candidate(
                    higher_count,
                    embedded,
                    lower.k,
                    offset_u8,
                    CanonicalSide::Right,
                    higher_sequence[offset + lower_length],
                )?);
            }

            if offset < last_offset {
                let next_sequence = &higher_sequence[offset + 1..offset + 1 + lower_length];
                let next = oriented_edge(next_sequence, lower.k)?;
                if find_edge(lower_index, next.key).is_none() {
                    return invariant(
                        "higher child supports an adjacency to an edge absent from the lower child",
                    );
                }
                relations.push(MultiKRelation {
                    lower_k: lower.k,
                    higher_k: higher.k,
                    evidence: RelationEvidence::Supports(SupportsRelation {
                        higher_key: higher_count.key,
                        from: embedded,
                        to: next,
                        first_offset: offset_u8,
                        higher_window_occurrences: higher_count.occurrences,
                    }),
                });
            }
        }
    }

    contexts.sort_unstable();
    let mut start = 0_usize;
    while start < contexts.len() {
        let group = (contexts[start].lower_key, contexts[start].canonical_side);
        let mut end = start + 1;
        while end < contexts.len()
            && (contexts[end].lower_key, contexts[end].canonical_side) == group
        {
            end += 1;
        }
        let first_base = contexts[start].canonical_base;
        if contexts[start..end]
            .iter()
            .any(|candidate| candidate.canonical_base != first_base)
        {
            for candidate in &contexts[start..end] {
                relations.push(MultiKRelation {
                    lower_k: lower.k,
                    higher_k: higher.k,
                    evidence: RelationEvidence::Conflicts(ConflictsRelation {
                        higher_key: candidate.higher_key,
                        embedded: candidate.embedded,
                        embedded_side: candidate.embedded_side,
                        canonical_side: candidate.canonical_side,
                        canonical_base: candidate.canonical_base,
                        offset: candidate.offset,
                        higher_window_occurrences: candidate.higher_window_occurrences,
                    }),
                });
            }
        }
        start = end;
    }
    relations.sort_unstable();
    if relations.windows(2).any(|pair| pair[0] >= pair[1]) {
        return invariant("experimental multi-k core relation derivation produced duplicates");
    }
    enforce_exact_capacity(
        relations.capacity(),
        relation_capacity,
        "multi-k materialized core-relation vector",
    )?;
    enforce_exact_capacity(
        contexts.capacity(),
        context_capacity,
        "multi-k materialized context-candidate vector",
    )?;
    Ok(relations)
}

fn context_candidate(
    higher: &super::partitioned_dbg::ExactEdgeCount,
    embedded: OrientedEdge,
    lower_k: u8,
    offset: u8,
    observed_side: CanonicalSide,
    observed_base: u8,
) -> Result<ContextCandidate> {
    let (canonical_side, canonical_base) =
        normalize_context(embedded, lower_k, observed_side, observed_base)?;
    Ok(ContextCandidate {
        lower_key: embedded.key,
        canonical_side,
        canonical_base,
        higher_key: higher.key,
        embedded,
        embedded_side: observed_side,
        offset,
        higher_window_occurrences: higher.occurrences,
    })
}

fn derive_unresolved_relations(
    lower: &PartitionedDbg,
    higher: &PartitionedDbg,
    segments: &[AcceptedSegment<'_>],
    order: &[usize],
    lower_index: &[EdgeCountEntry],
) -> Result<Vec<MultiKRelation>> {
    let atom_capacity = checked_mul(
        lower.accepted_windows,
        2,
        "experimental multi-k unresolved-atom count overflow",
    )?;
    let atom_capacity = as_usize(atom_capacity, "multi-k unresolved atom capacity")?;
    let mut atoms = Vec::new();
    atoms.try_reserve_exact(atom_capacity).map_err(|cause| {
        resource_memory(format!("cannot reserve multi-k unresolved atoms: {cause}"))
    })?;
    enforce_exact_capacity(
        atoms.capacity(),
        atom_capacity,
        "multi-k unresolved-atom vector",
    )?;
    let lower_length = usize::from(lower.k);
    let higher_length = usize::from(higher.k);

    for &segment_index in order {
        let sequence = segments[segment_index].bases;
        for (start, window) in sequence.windows(lower_length).enumerate() {
            let edge = oriented_edge(window, lower.k)?;
            if find_edge(lower_index, edge.key).is_none() {
                return invariant("source lower edge is absent from its independently built child");
            }
            let (raw_left, raw_right) =
                covering_flanks(sequence.len(), start, lower_length, higher_length);
            if !raw_left {
                atoms.push(UnresolvedAtom {
                    lower_key: edge.key,
                    canonical_side: normalize_edge_side(edge, lower.k, CanonicalSide::Left)?,
                });
            }
            if !raw_right {
                atoms.push(UnresolvedAtom {
                    lower_key: edge.key,
                    canonical_side: normalize_edge_side(edge, lower.k, CanonicalSide::Right)?,
                });
            }
        }
    }
    atoms.sort_unstable();

    let relation_capacity = atoms.len();
    let mut relations = Vec::new();
    relations
        .try_reserve_exact(relation_capacity)
        .map_err(|cause| {
            resource_memory(format!("cannot reserve multi-k unresolved rows: {cause}"))
        })?;
    enforce_exact_capacity(
        relations.capacity(),
        relation_capacity,
        "multi-k unresolved-relation vector",
    )?;
    let mut cursor = 0_usize;
    while cursor < atoms.len() {
        let atom = atoms[cursor];
        let mut end = cursor + 1;
        while end < atoms.len() && atoms[end] == atom {
            end += 1;
        }
        let count = u64::try_from(end - cursor)
            .map_err(|_| overflow("experimental multi-k unresolved count does not fit in u64"))?;
        let total = find_edge(lower_index, atom.lower_key).ok_or_else(|| {
            invariant_error("unresolved row names an edge absent from the lower child")
        })?;
        let maximum = unresolved_side_maximum(atom.canonical_side, total.occurrences)?;
        if count > maximum {
            return invariant("unresolved side count exceeds lower-edge occurrence support");
        }
        relations.push(MultiKRelation {
            lower_k: lower.k,
            higher_k: higher.k,
            evidence: RelationEvidence::Unresolved(UnresolvedRelation {
                lower_key: atom.lower_key,
                canonical_side: atom.canonical_side,
                lower_window_occurrences: count,
            }),
        });
        cursor = end;
    }
    enforce_exact_capacity(
        relations.capacity(),
        relation_capacity,
        "multi-k materialized unresolved-relation vector",
    )?;
    Ok(relations)
}

fn covering_flanks(
    segment_length: usize,
    lower_start: usize,
    lower_length: usize,
    higher_length: usize,
) -> (bool, bool) {
    if segment_length < higher_length {
        return (false, false);
    }
    let lower_end = lower_start + lower_length;
    let minimum_start = lower_end.saturating_sub(higher_length);
    let maximum_start = lower_start.min(segment_length - higher_length);
    if minimum_start > maximum_start {
        return (false, false);
    }
    let left = minimum_start < lower_start;
    let right = maximum_start + higher_length > lower_end;
    (left, right)
}

fn validate_relation_structure(
    relation: &MultiKRelation,
    lower: &PartitionedDbg,
    higher: &PartitionedDbg,
    lower_index: &[EdgeCountEntry],
    higher_index: &[EdgeCountEntry],
) -> Result<()> {
    match relation.evidence {
        RelationEvidence::Supports(support) => {
            validate_support_relation(support, lower, higher, lower_index, higher_index)
        }
        RelationEvidence::Contains(containment) => {
            let sequence = validate_higher_witness(
                containment.higher_key,
                containment.higher_window_occurrences,
                higher,
                higher_index,
            )?;
            validate_oriented_edge(containment.lower, lower.k, lower_index)?;
            let offset = usize::from(containment.offset);
            let end = offset
                .checked_add(usize::from(lower.k))
                .ok_or_else(|| overflow("experimental multi-k containment range overflow"))?;
            if sequence.get(offset..end) != Some(&decode_oriented(containment.lower, lower.k)?[..])
            {
                return invariant("experimental multi-k containment is not present in higher edge");
            }
            Ok(())
        }
        RelationEvidence::Conflicts(conflict) => {
            let sequence = validate_higher_witness(
                conflict.higher_key,
                conflict.higher_window_occurrences,
                higher,
                higher_index,
            )?;
            validate_oriented_edge(conflict.embedded, lower.k, lower_index)?;
            let offset = usize::from(conflict.offset);
            let lower_length = usize::from(lower.k);
            let end = offset
                .checked_add(lower_length)
                .ok_or_else(|| overflow("experimental multi-k conflict range overflow"))?;
            if sequence.get(offset..end) != Some(&decode_oriented(conflict.embedded, lower.k)?[..])
            {
                return invariant(
                    "experimental multi-k conflict anchor is absent from higher edge",
                );
            }
            if conflict.embedded_side == CanonicalSide::FixedPoint {
                return invariant(
                    "experimental multi-k conflict uses fixed-point as a literal embedded side",
                );
            }
            let observed_base = match conflict.embedded_side {
                CanonicalSide::Left => offset
                    .checked_sub(1)
                    .and_then(|position| sequence.get(position))
                    .copied(),
                CanonicalSide::Right => sequence.get(end).copied(),
                CanonicalSide::FixedPoint => None,
            }
            .ok_or_else(|| {
                invariant_error("experimental multi-k conflict has no claimed flank in higher edge")
            })?;
            let (canonical_side, canonical_base) = normalize_context(
                conflict.embedded,
                lower.k,
                conflict.embedded_side,
                observed_base,
            )?;
            if canonical_side != conflict.canonical_side
                || canonical_base != conflict.canonical_base
            {
                return invariant("experimental multi-k conflict base does not match higher edge");
            }
            Ok(())
        }
        RelationEvidence::Unresolved(unresolved) => {
            if unresolved.lower_window_occurrences == 0 {
                return invariant("experimental multi-k unresolved row has zero occurrences");
            }
            validate_canonical_key(unresolved.lower_key, lower.k)?;
            let total = find_edge(lower_index, unresolved.lower_key).ok_or_else(|| {
                invariant_error("experimental multi-k unresolved edge is absent from lower child")
            })?;
            let maximum = unresolved_side_maximum(unresolved.canonical_side, total.occurrences)?;
            if unresolved.lower_window_occurrences > maximum {
                return invariant("experimental multi-k unresolved count exceeds lower support");
            }
            Ok(())
        }
    }
}

fn validate_support_relation(
    support: SupportsRelation,
    lower: &PartitionedDbg,
    higher: &PartitionedDbg,
    lower_index: &[EdgeCountEntry],
    higher_index: &[EdgeCountEntry],
) -> Result<()> {
    let higher_sequence = validate_higher_witness(
        support.higher_key,
        support.higher_window_occurrences,
        higher,
        higher_index,
    )?;
    validate_oriented_edge(support.from, lower.k, lower_index)?;
    validate_oriented_edge(support.to, lower.k, lower_index)?;
    let from = decode_oriented(support.from, lower.k)?;
    let to = decode_oriented(support.to, lower.k)?;
    if from[1..] != to[..to.len() - 1] {
        return invariant("experimental multi-k support row has no exact lower-edge overlap");
    }
    let offset = usize::from(support.first_offset);
    let lower_length = usize::from(lower.k);
    let from_end = offset
        .checked_add(lower_length)
        .ok_or_else(|| overflow("experimental multi-k support range overflow"))?;
    let to_end = from_end
        .checked_add(1)
        .ok_or_else(|| overflow("experimental multi-k support range overflow"))?;
    if higher_sequence.get(offset..from_end) != Some(&from[..])
        || higher_sequence.get(offset + 1..to_end) != Some(&to[..])
    {
        return invariant(
            "experimental multi-k support adjacency is not replayable from its higher edge",
        );
    }
    Ok(())
}

fn validate_higher_witness(
    key: PackedKmer,
    occurrences: u64,
    higher: &PartitionedDbg,
    higher_index: &[EdgeCountEntry],
) -> Result<Vec<u8>> {
    if occurrences == 0 {
        return invariant("experimental multi-k higher witness has zero occurrences");
    }
    let observed = find_edge(higher_index, key).ok_or_else(|| {
        invariant_error("experimental multi-k witness is absent from higher child")
    })?;
    if observed.occurrences != occurrences {
        return invariant(
            "experimental multi-k witness occurrence count differs from higher child",
        );
    }
    validate_canonical_key(key, higher.k)
}

fn validate_oriented_edge(edge: OrientedEdge, k: u8, index: &[EdgeCountEntry]) -> Result<()> {
    validate_canonical_key(edge.key, k)?;
    validate_code(edge.spelling, k)?;
    if canonical_code(edge.spelling, k)? != edge.key {
        return invariant("experimental multi-k oriented spelling has the wrong canonical key");
    }
    let reverse = reverse_complement_code(edge.key, k)?;
    match edge.orientation {
        EdgeOrientation::Canonical if edge.spelling != edge.key => {
            return invariant("experimental multi-k canonical orientation has another spelling")
        }
        EdgeOrientation::ReverseComplement if reverse == edge.key || edge.spelling != reverse => {
            return invariant("experimental multi-k reverse orientation is not the unique mate")
        }
        _ => {}
    }
    if find_edge(index, edge.key).is_none() {
        return invariant("experimental multi-k oriented edge is absent from lower child");
    }
    Ok(())
}

fn validate_canonical_key(key: PackedKmer, k: u8) -> Result<Vec<u8>> {
    validate_code(key, k)?;
    if canonical_code(key, k)? != key {
        return invariant("experimental multi-k edge key is not canonical");
    }
    let sequence = decode_mer(key, k)?;
    if encode_exact_bases(&sequence)? != key {
        return invariant("experimental multi-k key fails exact sequence round trip");
    }
    Ok(sequence)
}

fn oriented_edge(sequence: &[u8], k: u8) -> Result<OrientedEdge> {
    if sequence.len() != usize::from(k) {
        return invariant("experimental multi-k oriented edge has the wrong sequence length");
    }
    let spelling = encode_exact_bases(sequence)?;
    let key = canonical_code(spelling, k)?;
    let orientation = if spelling == key {
        EdgeOrientation::Canonical
    } else {
        EdgeOrientation::ReverseComplement
    };
    Ok(OrientedEdge {
        key,
        spelling,
        orientation,
    })
}

fn decode_oriented(edge: OrientedEdge, k: u8) -> Result<Vec<u8>> {
    decode_mer(edge.spelling, k)
}

fn normalize_context(
    edge: OrientedEdge,
    k: u8,
    observed_side: CanonicalSide,
    observed_base: u8,
) -> Result<(CanonicalSide, u8)> {
    let base = normalize_base(observed_base)?;
    if observed_side == CanonicalSide::FixedPoint {
        return invariant("literal context side cannot be fixed-point");
    }
    if reverse_complement_code(edge.key, k)? == edge.key {
        return Ok((CanonicalSide::FixedPoint, base.min(complement_base(base)?)));
    }
    Ok(match edge.orientation {
        EdgeOrientation::Canonical => (observed_side, base),
        EdgeOrientation::ReverseComplement => (observed_side.flip(), complement_base(base)?),
    })
}

fn normalize_edge_side(
    edge: OrientedEdge,
    k: u8,
    observed_side: CanonicalSide,
) -> Result<CanonicalSide> {
    if observed_side == CanonicalSide::FixedPoint {
        return invariant("literal edge side cannot be fixed-point");
    }
    if reverse_complement_code(edge.key, k)? == edge.key {
        return Ok(CanonicalSide::FixedPoint);
    }
    Ok(match edge.orientation {
        EdgeOrientation::Canonical => observed_side,
        EdgeOrientation::ReverseComplement => observed_side.flip(),
    })
}

fn unresolved_side_maximum(side: CanonicalSide, occurrences: u64) -> Result<u64> {
    match side {
        CanonicalSide::Left | CanonicalSide::Right => Ok(occurrences),
        CanonicalSide::FixedPoint => checked_mul(
            occurrences,
            2,
            "experimental multi-k fixed-point unresolved maximum overflow",
        ),
    }
}

fn find_edge(index: &[EdgeCountEntry], key: PackedKmer) -> Option<EdgeCountEntry> {
    index
        .binary_search_by_key(&key, |entry| entry.key)
        .ok()
        .map(|position| index[position])
}

fn relation_bounds(children: &[PartitionedDbg]) -> Result<RelationBounds> {
    let mut bounds = RelationBounds::default();
    for child in children {
        bounds.maximum_oracle_windows = bounds.maximum_oracle_windows.max(child.accepted_windows);
    }
    for pair in children.windows(2) {
        let lower = &pair[0];
        let higher = &pair[1];
        let delta = higher
            .k
            .checked_sub(lower.k)
            .ok_or_else(|| invariant_error("experimental multi-k child order moved backwards"))?;
        let high_distinct = u64::try_from(higher.edge_counts.len())
            .map_err(|_| overflow("experimental multi-k edge count does not fit in u64"))?;
        let contains = checked_mul(
            high_distinct,
            u64::from(delta) + 1,
            "experimental multi-k containment upper bound overflow",
        )?;
        let supports = checked_mul(
            high_distinct,
            u64::from(delta),
            "experimental multi-k support upper bound overflow",
        )?;
        let contexts = checked_mul(
            contains,
            2,
            "experimental multi-k context upper bound overflow",
        )?;
        let unresolved = checked_mul(
            lower.accepted_windows,
            2,
            "experimental multi-k unresolved upper bound overflow",
        )?;
        let pair_events = checked_add(
            checked_add(
                contains,
                supports,
                "experimental multi-k pair-event sum overflow",
            )?,
            checked_add(
                contexts,
                unresolved,
                "experimental multi-k pair-event sum overflow",
            )?,
            "experimental multi-k pair-event sum overflow",
        )?;
        bounds.events = checked_add(
            bounds.events,
            pair_events,
            "experimental multi-k total relation-event bound overflow",
        )?;
        bounds.records = checked_add(
            bounds.records,
            pair_events,
            "experimental multi-k total relation-record bound overflow",
        )?;
        bounds.maximum_contexts = bounds.maximum_contexts.max(contexts);
        bounds.maximum_unresolved_atoms = bounds.maximum_unresolved_atoms.max(unresolved);
        bounds.maximum_core_records = bounds.maximum_core_records.max(checked_add(
            checked_add(
                contains,
                supports,
                "experimental multi-k core-record bound overflow",
            )?,
            contexts,
            "experimental multi-k core-record bound overflow",
        )?);
        bounds.maximum_unresolved_records = bounds.maximum_unresolved_records.max(unresolved);
        let pair_indices = checked_add(
            u64::try_from(lower.edge_counts.len())
                .map_err(|_| overflow("experimental multi-k edge count does not fit in u64"))?,
            u64::try_from(higher.edge_counts.len())
                .map_err(|_| overflow("experimental multi-k edge count does not fit in u64"))?,
            "experimental multi-k pair edge-index bound overflow",
        )?;
        bounds.maximum_pair_edge_indices = bounds.maximum_pair_edge_indices.max(pair_indices);
    }
    Ok(bounds)
}

fn projected_relation_bytes(
    bounds: RelationBounds,
    segment_count: usize,
    child_count: usize,
) -> Result<u64> {
    let mut total = 0_u64;
    total = checked_payload_add::<MultiKRelation>(total, bounds.records, "relation rows")?;
    total = checked_payload_add::<ContextCandidate>(
        total,
        bounds.maximum_contexts,
        "context candidates",
    )?;
    total = checked_payload_add::<UnresolvedAtom>(
        total,
        bounds.maximum_unresolved_atoms,
        "unresolved atoms",
    )?;
    total = checked_payload_add::<EdgeCountEntry>(
        total,
        bounds.maximum_pair_edge_indices,
        "pair edge indices",
    )?;
    total = checked_payload_add::<MultiKRelation>(
        total,
        bounds.maximum_core_records,
        "core-relation validation scratch",
    )?;
    total = checked_payload_add::<MultiKRelation>(
        total,
        bounds.maximum_unresolved_records,
        "unresolved-relation validation scratch",
    )?;
    total = checked_payload_add::<PackedKmer>(
        total,
        bounds.maximum_oracle_windows,
        "source-slice oracle keys",
    )?;
    total = checked_payload_add::<EdgeCountEntry>(
        total,
        bounds.maximum_oracle_windows,
        "source-slice oracle counts",
    )?;
    total = checked_payload_add::<usize>(
        total,
        u64::try_from(segment_count)
            .map_err(|_| overflow("experimental multi-k segment count does not fit in u64"))?,
        "segment order",
    )?;
    total = checked_payload_add::<PartitionedDbg>(
        total,
        u64::try_from(child_count)
            .map_err(|_| overflow("experimental multi-k child count does not fit in u64"))?,
        "child vector headers",
    )?;
    let vector_headers = checked_mul(
        u64::try_from(size_of::<Vec<u8>>())
            .map_err(|_| overflow("experimental multi-k vector-header size does not fit u64"))?,
        8,
        "experimental multi-k vector-header allowance overflow",
    )?;
    checked_add(
        total,
        vector_headers,
        "experimental multi-k projected relation payload overflow",
    )
}

/// Upper bound for the source recount while it is sorted and compared with a
/// separately materialized child-edge index. This check is deliberately made
/// before `slice_oracle_counts` reserves any window-sized allocation.
fn projected_oracle_recount_bytes(window_count: u64) -> Result<u64> {
    let mut total = 0_u64;
    total = checked_payload_add::<PackedKmer>(total, window_count, "source-oracle keys")?;
    total = checked_payload_add::<EdgeCountEntry>(
        total,
        window_count,
        "source-oracle collapsed counts",
    )?;
    checked_payload_add::<EdgeCountEntry>(total, window_count, "child comparison index")
}

fn checked_payload_add<T>(total: u64, count: u64, label: &'static str) -> Result<u64> {
    let width = u64::try_from(size_of::<T>())
        .map_err(|_| overflow("experimental multi-k record size does not fit in u64"))?;
    let bytes = checked_mul(
        count,
        width,
        "experimental multi-k projected-payload multiplication overflow",
    )?;
    total.checked_add(bytes).ok_or_else(|| {
        VeritasmError::new(
            ErrorCode::ResourceIntegerOverflow,
            format!("experimental multi-k projected-payload addition overflow for {label}"),
        )
    })
}

fn enforce_exact_capacity(actual: usize, admitted: usize, label: &'static str) -> Result<()> {
    if actual <= admitted {
        Ok(())
    } else {
        Err(resource_memory(format!(
            "allocator returned {label} capacity {actual} above admitted capacity {admitted}"
        )))
    }
}

fn exact_base_bits(base: u8) -> Option<u8> {
    match base.to_ascii_uppercase() {
        b'A' => Some(0),
        b'C' => Some(1),
        b'G' => Some(2),
        b'T' => Some(3),
        _ => None,
    }
}

fn normalize_base(base: u8) -> Result<u8> {
    match base.to_ascii_uppercase() {
        normalized @ (b'A' | b'C' | b'G' | b'T') => Ok(normalized),
        _ => Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            "experimental multi-k exact context contains a non-ACGT base",
        )),
    }
}

fn complement_base(base: u8) -> Result<u8> {
    match base {
        b'A' => Ok(b'T'),
        b'C' => Ok(b'G'),
        b'G' => Ok(b'C'),
        b'T' => Ok(b'A'),
        _ => Err(VeritasmError::new(
            ErrorCode::InternalInvariant,
            "experimental multi-k context complement received a non-ACGT base",
        )),
    }
}

fn enforce_limit(observed: u64, limit: u64, code: ErrorCode, label: &'static str) -> Result<()> {
    match observed.cmp(&limit) {
        Ordering::Less | Ordering::Equal => Ok(()),
        Ordering::Greater => Err(VeritasmError::new(
            code,
            format!("{label} {observed} exceeds configured limit {limit}"),
        )),
    }
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

fn checked_mul(left: u64, right: u64, context: &'static str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| overflow(context))
}

fn overflow(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceIntegerOverflow, context)
}

fn resource_memory(context: String) -> VeritasmError {
    VeritasmError::new(ErrorCode::ResourceMemory, context)
}

fn invariant_error(context: &'static str) -> VeritasmError {
    VeritasmError::new(ErrorCode::InternalInvariant, context)
}

fn invariant<T>(context: &'static str) -> Result<T> {
    Err(invariant_error(context))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimental::partitioned_dbg::PartitionLimits;
    use proptest::prelude::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn child(k: u8, minimizer_length: u8) -> PartitionConfig {
        PartitionConfig {
            k,
            minimizer_length,
            virtual_bucket_count: 7,
            limits: PartitionLimits {
                max_input_bases: 100_000,
                max_windows: 100_000,
                max_super_kmers: 100_000,
                max_distinct_kmers: 100_000,
                max_accounted_bytes: 64 * 1024 * 1024,
            },
        }
    }

    fn config(specifications: &[(u8, u8)]) -> MultiKConfig {
        MultiKConfig {
            children: specifications
                .iter()
                .map(|&(k, minimizer)| child(k, minimizer))
                .collect(),
            limits: MultiKLimits {
                max_children: 16,
                max_segments: 100_000,
                max_total_child_accounted_bytes: 1024 * 1024 * 1024,
                max_oracle_windows_per_child: 100_000,
                max_relation_events: 1_000_000,
                max_relation_records: 1_000_000,
                max_relation_accounted_bytes: 512 * 1024 * 1024,
            },
        }
    }

    fn relation_kind_counts(lattice: &MultiKEvidenceLattice) -> [usize; 4] {
        let mut counts = [0_usize; 4];
        for relation in &lattice.relations {
            match relation.evidence {
                RelationEvidence::Supports(_) => counts[0] += 1,
                RelationEvidence::Contains(_) => counts[1] += 1,
                RelationEvidence::Conflicts(_) => counts[2] += 1,
                RelationEvidence::Unresolved(_) => counts[3] += 1,
            }
        }
        counts
    }

    fn dna(symbols: &[u8]) -> Vec<u8> {
        symbols
            .iter()
            .map(|symbol| match symbol % 4 {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                _ => b'T',
            })
            .collect()
    }

    #[test]
    fn builds_all_typed_rows_without_projecting_sequence() {
        let segments = [
            AcceptedSegment {
                source_ordinal: 2,
                segment_ordinal: 0,
                bases: b"CCCAAAGCC",
            },
            AcceptedSegment {
                source_ordinal: 1,
                segment_ordinal: 0,
                bases: b"CCCAAACCC",
            },
            AcceptedSegment {
                source_ordinal: 3,
                segment_ordinal: 0,
                bases: b"AAA",
            },
        ];
        let lattice = build_multik_evidence_lattice(&config(&[(3, 2), (5, 3)]), &segments)
            .expect("multi-k lattice should build");
        assert_eq!(lattice.children.len(), 2);
        assert_eq!(lattice.children[0].k, 3);
        assert_eq!(lattice.children[1].k, 5);
        assert!(
            lattice.relation_event_upper_bound >= u64::try_from(lattice.relations.len()).unwrap()
        );
        assert!(lattice.relation_projected_payload_bytes > 0);

        let counts = relation_kind_counts(&lattice);
        assert!(counts.into_iter().all(|count| count > 0), "{counts:?}");
        lattice.validate_against_segments(&segments).unwrap();

        let mut conflict_bases: BTreeMap<(PackedKmer, CanonicalSide), BTreeSet<u8>> =
            BTreeMap::new();
        for relation in &lattice.relations {
            if let RelationEvidence::Conflicts(conflict) = relation.evidence {
                conflict_bases
                    .entry((conflict.embedded.key, conflict.canonical_side))
                    .or_default()
                    .insert(conflict.canonical_base);
            }
        }
        assert!(conflict_bases.values().all(|bases| bases.len() >= 2));
    }

    #[test]
    fn self_reverse_complement_edge_has_one_fixed_side_with_two_terminal_events() {
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AGCT",
        }];
        let lattice = build_multik_evidence_lattice(&config(&[(4, 2), (5, 2)]), &segments).unwrap();
        let key = encode_exact_bases(b"AGCT").unwrap();
        let unresolved: Vec<_> = lattice
            .relations
            .iter()
            .filter_map(|relation| match relation.evidence {
                RelationEvidence::Unresolved(row) => Some(row),
                _ => None,
            })
            .collect();
        assert_eq!(
            unresolved,
            vec![UnresolvedRelation {
                lower_key: key,
                canonical_side: CanonicalSide::FixedPoint,
                lower_window_occurrences: 2,
            }]
        );
        lattice.validate_against_segments(&segments).unwrap();
    }

    #[test]
    fn child_and_relation_order_ignore_input_order_and_rayon_pool_size() {
        let first = AcceptedSegment {
            source_ordinal: 7,
            segment_ordinal: 1,
            bases: b"CCCAAACCCGGGTTT",
        };
        let second = AcceptedSegment {
            source_ordinal: 2,
            segment_ordinal: 0,
            bases: b"AACCGGTTAACCGGT",
        };
        let one_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let four_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let parameters = config(&[(3, 2), (5, 3), (8, 4)]);
        let one = one_pool
            .install(|| build_multik_evidence_lattice(&parameters, &[first, second]).unwrap());
        let four = four_pool
            .install(|| build_multik_evidence_lattice(&parameters, &[second, first]).unwrap());
        assert_eq!(one, four);
    }

    #[test]
    fn support_oracle_rejects_an_adjacency_not_replayed_by_higher_edge() {
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"ACCGTTAGCTAACCGG",
        }];
        let mut lattice =
            build_multik_evidence_lattice(&config(&[(4, 2), (7, 3)]), &segments).unwrap();
        let mut wrong_pair = lattice.clone();
        let relabelled = wrong_pair
            .relations
            .iter_mut()
            .find(|relation| matches!(relation.evidence, RelationEvidence::Supports(_)))
            .unwrap();
        relabelled.higher_k = relabelled.higher_k.checked_add(1).unwrap();
        assert_eq!(
            wrong_pair
                .validate_no_unsupported_adjacencies()
                .unwrap_err()
                .code(),
            ErrorCode::InternalInvariant
        );

        let relation = lattice
            .relations
            .iter_mut()
            .find(|relation| matches!(relation.evidence, RelationEvidence::Supports(_)))
            .unwrap();
        if let RelationEvidence::Supports(mut support) = relation.evidence {
            support.first_offset = u8::MAX;
            relation.evidence = RelationEvidence::Supports(support);
        }
        let error = lattice.validate_no_unsupported_adjacencies().unwrap_err();
        assert_eq!(error.code(), ErrorCode::InternalInvariant);
    }

    #[test]
    fn support_validation_builds_each_child_index_once_per_layer_pair() {
        let mut state = 0xbb67_ae85_84ca_a73b_u64;
        let sequence = (0..2_048)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                b"ACGT"[usize::try_from(state & 3).unwrap()]
            })
            .collect::<Vec<_>>();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: &sequence,
        }];
        let lattice =
            build_multik_evidence_lattice(&config(&[(17, 17), (18, 18)]), &segments).unwrap();
        let expected_supports = lattice
            .relations
            .iter()
            .filter(|relation| matches!(relation.evidence, RelationEvidence::Supports(_)))
            .count();
        assert!(expected_supports > 1_900, "fixture lacks support rows");

        let work = lattice
            .validate_no_unsupported_adjacencies_indexed()
            .unwrap();
        assert_eq!(work.edge_index_builds, 2);
        assert_eq!(
            work.support_relations,
            u64::try_from(expected_supports).unwrap()
        );
    }

    #[test]
    fn source_oracle_rejects_changed_bases_and_child_counts() {
        let original = b"AACCGGTTAACCGGTT".to_vec();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: &original,
        }];
        let mut lattice =
            build_multik_evidence_lattice(&config(&[(5, 3), (9, 4)]), &segments).unwrap();

        let changed = b"AACCGGATAACCGGTT".to_vec();
        let changed_segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: &changed,
        }];
        assert_eq!(
            lattice
                .validate_against_segments(&changed_segments)
                .unwrap_err()
                .code(),
            ErrorCode::IntegritySpool
        );

        lattice.children[0].edge_counts[0].occurrences += 1;
        assert_eq!(
            lattice
                .validate_against_segments(&segments)
                .unwrap_err()
                .code(),
            ErrorCode::InternalInvariant
        );
    }

    #[test]
    fn validates_wide_exact_children_across_u128_boundary() {
        let sequence: Vec<u8> = (0..80)
            .map(|position| match (position * 11 + position / 3) % 4 {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                _ => b'T',
            })
            .collect();
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: &sequence,
        }];
        let lattice =
            build_multik_evidence_lattice(&config(&[(63, 31), (65, 32)]), &segments).unwrap();
        assert!(lattice
            .children
            .iter()
            .all(|child| !child.edge_counts.is_empty()));
        assert!(lattice
            .relations
            .iter()
            .any(|relation| matches!(relation.evidence, RelationEvidence::Supports(_))));
        lattice.validate_no_unsupported_adjacencies().unwrap();
    }

    #[test]
    fn configuration_and_resource_limits_fail_closed() {
        let segment = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACCGGTTAACCGGTT",
        }];

        let mut parameters = config(&[(5, 3)]);
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationUnsupportedCombination
        );

        parameters = config(&[(7, 3), (5, 3)]);
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidK
        );

        parameters = config(&[(5, 3), (5, 3)]);
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigurationInvalidK
        );

        parameters = config(&[(5, 3), (7, 3)]);
        parameters.limits.max_total_child_accounted_bytes = 1;
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );

        parameters = config(&[(5, 3), (7, 3)]);
        parameters.limits.max_segments = 0;
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRetainedKeys
        );

        parameters = config(&[(5, 3), (7, 3)]);
        parameters.limits.max_oracle_windows_per_child = 0;
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRetainedKeys
        );

        parameters = config(&[(5, 3), (7, 3)]);
        parameters.limits.max_relation_events = 0;
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRetainedKeys
        );

        parameters = config(&[(5, 3), (7, 3)]);
        parameters.limits.max_relation_records = 0;
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRetainedKeys
        );

        parameters = config(&[(5, 3), (7, 3)]);
        parameters.limits.max_relation_accounted_bytes = 1;
        assert_eq!(
            build_multik_evidence_lattice(&parameters, &segment)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );

        let ambiguous = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACNGGTT",
        }];
        assert_eq!(
            build_multik_evidence_lattice(&config(&[(3, 2), (5, 3)]), &ambiguous)
                .unwrap_err()
                .code(),
            ErrorCode::InputNucleotide
        );

        let duplicates = [
            AcceptedSegment {
                source_ordinal: 4,
                segment_ordinal: 2,
                bases: b"AACCGGTT",
            },
            AcceptedSegment {
                source_ordinal: 4,
                segment_ordinal: 2,
                bases: b"TTGGCCAA",
            },
        ];
        assert_eq!(
            build_multik_evidence_lattice(&config(&[(3, 2), (5, 3)]), &duplicates)
                .unwrap_err()
                .code(),
            ErrorCode::IntegrityOrdinalCoverage
        );
    }

    #[test]
    fn parent_and_oracle_front_matter_is_admitted_before_child_construction() {
        let segments = [AcceptedSegment {
            source_ordinal: 0,
            segment_ordinal: 0,
            bases: b"AACCGGTTAACCGGTT",
        }];
        let mut parameters = config(&[(5, 3), (7, 3)]);
        let child_count = parameters.children.len() as u64;
        let front_bytes = size_of::<usize>() as u64
            + child_count * size_of::<PartitionedDbg>() as u64
            + child_count * size_of::<u64>() as u64;
        let maximum_windows = (segments[0].bases.len() - 5 + 1) as u64;
        let required = front_bytes + projected_oracle_recount_bytes(maximum_windows).unwrap();

        for (budget, accepted) in [
            (required - 1, false),
            (required, true),
            (required + 1, true),
        ] {
            parameters.limits.max_relation_accounted_bytes = budget;
            assert_eq!(
                preflight_parent_allocations(&parameters, &segments, 1).is_ok(),
                accepted
            );
        }

        parameters.limits.max_relation_accounted_bytes = required;
        parameters.limits.max_oracle_windows_per_child = maximum_windows - 1;
        assert_eq!(
            preflight_parent_allocations(&parameters, &segments, 1)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceRetainedKeys
        );
    }

    proptest! {
        #[test]
        fn reverse_complement_preserves_children_and_canonicalized_relations(
            symbols in prop::collection::vec(0_u8..4, 10..40),
            lower_seed in 3_u8..8,
            delta_seed in 1_u8..4,
        ) {
            let sequence = dna(&symbols);
            let lower_k = lower_seed.min((sequence.len() - 1) as u8);
            let higher_k = (lower_k + delta_seed).min(sequence.len() as u8);
            prop_assume!(higher_k > lower_k);
            let reverse = crate::dna::reverse_complement(&sequence).unwrap();
            let forward_segments = [AcceptedSegment {
                source_ordinal: 0,
                segment_ordinal: 0,
                bases: &sequence,
            }];
            let reverse_segments = [AcceptedSegment {
                source_ordinal: 0,
                segment_ordinal: 0,
                bases: &reverse,
            }];
            let parameters = config(&[(lower_k, 2), (higher_k, 2)]);
            let forward = build_multik_evidence_lattice(&parameters, &forward_segments).unwrap();
            let reverse = build_multik_evidence_lattice(&parameters, &reverse_segments).unwrap();
            for (forward_child, reverse_child) in
                forward.children.iter().zip(&reverse.children)
            {
                prop_assert_eq!(&forward_child.edge_counts, &reverse_child.edge_counts);
                prop_assert_eq!(&forward_child.buckets, &reverse_child.buckets);
            }
            prop_assert_eq!(forward.relations, reverse.relations);
        }
    }
}
