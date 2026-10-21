//! Read-only execution coordinates over the same validated canonical projection.
use super::{
    IncidenceProjection, IncidenceRow, IncidenceRowVisitor, ProjectionData, ProjectionTargets,
};
use std::sync::Arc;

/// How numeric execution coordinates are assigned; IR identifiers never change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IncidenceOrder {
    /// Existing canonical declaration/node coordinates.
    #[default]
    Canonical,
    /// First occurrence in the existing arc/member sequence, then isolated IDs.
    /// Active coordinates are invariant under a bijective relabeling that keeps
    /// that incidence sequence. This is not a graph-isomorphism canonicalizer.
    FirstEncounter,
}

/// An immutable numeric view, borrowing the canonical IR through its projection.
///
/// Hyperarc order, coefficient slots and duplicate incidences are unchanged.
/// Row targets are execution slots; `canonical_index` maps them back to the
/// original projection's domain. The original projection remains available for
/// operations whose floating-point ordering must be canonical.
pub struct IncidenceLayout<'ir> {
    canonical: IncidenceProjection<'ir>,
    data: Arc<ProjectionData>,
    canonical_of: Option<Box<[usize]>>,
}
impl<'ir> IncidenceLayout<'ir> {
    pub(super) fn new(canonical: IncidenceProjection<'ir>, order: IncidenceOrder) -> Self {
        if order == IncidenceOrder::Canonical {
            return Self {
                data: Arc::clone(&canonical.data),
                canonical,
                canonical_of: None,
            };
        }
        let mut coordinate_of = vec![usize::MAX; canonical.domain_len()];
        let mut canonical_of = Vec::with_capacity(canonical.domain_len());
        let mut targets = Vec::with_capacity(canonical.incidence_count());
        for row in canonical.rows() {
            for target in row.iter_targets() {
                let coordinate = &mut coordinate_of[target];
                if *coordinate == usize::MAX {
                    *coordinate = canonical_of.len();
                    canonical_of.push(target);
                }
                targets.push(*coordinate);
            }
        }
        // Unreferenced coordinates remain in the domain and result vectors.
        for (target, &coordinate) in coordinate_of.iter().enumerate() {
            if coordinate == usize::MAX {
                canonical_of.push(target);
            }
        }
        let data = Arc::new(ProjectionData {
            domain: canonical.data.domain,
            domain_len: canonical.domain_len(),
            offsets: canonical.data.offsets.clone(),
            uniform_arity: canonical.data.uniform_arity,
            targets: ProjectionTargets::build(&canonical.data.offsets, targets),
        });
        Self {
            canonical,
            data,
            canonical_of: Some(canonical_of.into_boxed_slice()),
        }
    }
    /// The unchanged original view, with canonical numeric targets.
    pub fn projection(&self) -> &IncidenceProjection<'ir> {
        &self.canonical
    }
    /// Chosen coordinate assignment.
    pub fn order(&self) -> IncidenceOrder {
        if self.canonical_of.is_some() {
            IncidenceOrder::FirstEncounter
        } else {
            IncidenceOrder::Canonical
        }
    }
    /// Canonical index for an execution coordinate, including isolated entries.
    /// # Panics
    /// If `coordinate` is outside the projection's domain.
    #[inline]
    pub fn canonical_index(&self, coordinate: usize) -> usize {
        assert!(coordinate < self.canonical.domain_len());
        self.canonical_of
            .as_ref()
            .map_or(coordinate, |order| order[coordinate])
    }
    /// Explicit execution-to-canonical mapping, or None for the identity view.
    pub fn canonical_indices(&self) -> Option<&[usize]> {
        self.canonical_of.as_deref()
    }
    /// Lossless contiguous ranges in execution coordinates; no expansion.
    pub fn range_count(&self) -> usize {
        self.data.targets.range_count()
    }
    /// Buffers owned in addition to the canonical projection shared by this view.
    pub fn additional_bytes(&self) -> usize {
        self.canonical_of.as_ref().map_or(0, |order| {
            self.data.retained_bytes() + size_of_val(order.as_ref())
        })
    }
    /// Rows retain canonical hyperarc order and coefficient-slot offsets.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = IncidenceRow<'_>> {
        self.data.rows()
    }
    /// Apply the same row dispatcher used by the canonical projection.
    #[inline(always)]
    pub fn for_each_row(&self, visitor: &mut impl IncidenceRowVisitor) {
        self.data.for_each_row(visitor);
    }
}
