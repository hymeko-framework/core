//! Compact canonical references; variable payloads belong to their owning Ir.
use super::ir::{AnnoR, Ir, ValueR};
use crate::common::ids::{AnnotationId, BaseListId, DeclId, WeightVectorId};
use serde::{Deserialize, Serialize};

pub(crate) mod wire;

/// Algebraic incidence sign. This is independent of operator-port direction.
#[repr(i8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncidenceSign {
    Plus = 1,
    Minus = -1,
    #[default]
    Neutral = 0,
}

/// Data flow through an executable operator's port, never inferred from sign.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PortDirection {
    #[default]
    Unspecified = 0,
    In = 1,
    Out = 2,
}
impl PortDirection {
    #[must_use]
    pub const fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

/// Owned construction payload, consumed into the IR's attribute tables.
/// Canonical incidence arrays store SignedRefR instead of embedding this draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefAtomR {
    pub target: DeclId,
    pub anno: AnnoR,
    pub weights: Option<Vec<ValueR>>,
    #[serde(default, skip_serializing_if = "PortDirection::is_unspecified")]
    pub direction: PortDirection,
}
impl RefAtomR {
    #[must_use]
    pub fn plain(target: DeclId) -> Self {
        Self {
            target,
            anno: AnnoR::default(),
            weights: None,
            direction: PortDirection::Unspecified,
        }
    }
    #[must_use]
    pub fn with_weights(mut self, weights: Option<Vec<ValueR>>) -> Self {
        self.weights = weights;
        self
    }

    /// Compare logical payloads without cloning or folding distinct float bits.
    fn matches(&self, stored: ReferenceView<'_>) -> bool {
        self.target == stored.target
            && self.direction == stored.direction
            && self.anno.tags == stored.anno.tags
            && match (&self.anno.value, &stored.anno.value) {
                (None, None) => true,
                (Some(left), Some(right)) => left.bitwise_eq(right),
                _ => false,
            }
            && match (self.weights.as_deref(), stored.weights) {
                (None, None) => true,
                (Some(left), Some(right)) => {
                    left.len() == right.len()
                        && left.iter().zip(right).all(|(a, b)| a.bitwise_eq(b))
                }
                _ => false,
            }
    }
}

/// Resolved input for checked construction, before its payloads have an IR owner.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceDraft {
    pub sign: IncidenceSign,
    pub atom: RefAtomR,
}
impl ReferenceDraft {
    pub fn new(sign: IncidenceSign, atom: RefAtomR) -> Self {
        Self { sign, atom }
    }
    pub fn plain(target: DeclId, sign: IncidenceSign) -> Self {
        Self::new(sign, RefAtomR::plain(target))
    }
    pub fn sign(&self) -> i8 {
        self.sign as i8
    }
    pub fn target(&self) -> DeclId {
        self.atom.target
    }
    pub fn atom(&self) -> &RefAtomR {
        &self.atom
    }
}

/// A canonical reference in one IR's ID domain. Payload IDs are not portable
/// between models. Serialize through Ir so the existing logical wire is emitted.
/// Empty annotation and absent weights use NONE and allocate no payload entry.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignedRefR {
    target: DeclId,
    annotation: AnnotationId,
    weights: WeightVectorId,
    sign: IncidenceSign,
    direction: PortDirection,
}

const _: () = {
    assert!(std::mem::size_of::<SignedRefR>() <= 32);
    assert!(std::mem::size_of::<IncidenceSign>() == 1);
    assert!(std::mem::size_of::<PortDirection>() == 1);
};
impl SignedRefR {
    #[must_use]
    pub fn plain(target: DeclId, sign: IncidenceSign) -> Self {
        Self {
            target,
            annotation: AnnotationId::NONE,
            weights: WeightVectorId::NONE,
            sign,
            direction: PortDirection::Unspecified,
        }
    }
    #[inline]
    pub fn target(&self) -> DeclId {
        self.target
    }
    #[inline]
    pub fn sign(&self) -> i8 {
        self.sign as i8
    }
    #[inline]
    pub fn sign_kind(&self) -> IncidenceSign {
        self.sign
    }
    #[inline]
    pub fn direction(&self) -> PortDirection {
        self.direction
    }
    #[inline]
    pub fn annotation_id(&self) -> AnnotationId {
        self.annotation
    }
    #[inline]
    pub fn weight_vector_id(&self) -> WeightVectorId {
        self.weights
    }
    #[must_use]
    pub fn with_target(mut self, target: DeclId) -> Self {
        self.target = target;
        self
    }
    #[must_use]
    pub fn with_sign(mut self, sign: IncidenceSign) -> Self {
        self.sign = sign;
        self
    }
    #[must_use]
    pub fn with_direction(mut self, direction: PortDirection) -> Self {
        self.direction = direction;
        self
    }
}

/// Borrowed logical payload. No annotation or weight vector is cloned by a read.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceView<'a> {
    pub target: DeclId,
    pub anno: &'a AnnoR,
    pub weights: Option<&'a [ValueR]>,
    pub direction: PortDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceError {
    Annotation(AnnotationId),
    Weights(WeightVectorId),
}
impl std::fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Annotation(id) => write!(f, "unknown reference annotation {}", id.raw()),
            Self::Weights(id) => write!(f, "unknown reference weight vector {}", id.raw()),
        }
    }
}
impl std::error::Error for ReferenceError {}

static EMPTY_ANNOTATION: AnnoR = AnnoR {
    tags: Vec::new(),
    value: None,
};

impl Ir {
    /// Store an owned annotation in this IR. Empty input uses NONE and no slot.
    /// Nonempty calls create distinct slots; no content interning is performed.
    pub fn store_annotation(&mut self, anno: AnnoR) -> AnnotationId {
        if anno.tags.is_empty() && anno.value.is_none() {
            AnnotationId::NONE
        } else {
            let id = AnnotationId::new(self.annotations.len());
            self.annotations.push(anno);
            id
        }
    }

    /// Borrow a model-local annotation. NONE resolves to the empty annotation;
    /// an out-of-range ID returns None. Foreign numeric IDs are not detectable.
    pub fn annotation(&self, id: AnnotationId) -> Option<&AnnoR> {
        if id.is_none() {
            Some(&EMPTY_ANNOTATION)
        } else {
            self.annotations.get_record(id.raw())
        }
    }

    /// Borrow a declaration's logical annotation without materializing payloads.
    /// Returns None for an absent declaration or an invalid payload handle.
    pub fn decl_annotation(&self, declaration: DeclId) -> Option<&AnnoR> {
        self.annotation(self.decl_node(declaration)?.anno)
    }

    /// Explicit owned construction input for copying a declaration. Ordinary
    /// readers should borrow decl_node/decl_annotation instead. Invalid
    /// declarations or annotation handles return None without allocating.
    pub fn decl_draft(&self, declaration: DeclId) -> Option<crate::ir::ir::DeclNode<AnnoR>> {
        let node = *self.decl_node(declaration)?;
        let anno = self.annotation(node.anno)?.clone();
        Some(node.map_annotation(|_| anno))
    }

    /// Mutable raw-IR access under the exclusive annotation-slot invariant.
    /// Coherence admission checks that declarations/arcs do not alias payloads.
    /// First access to an empty annotation allocates its slot. Published
    /// snapshots remain isolated by the same IrTable COW as record mutation.
    /// # Errors
    /// Rejects absent declarations and invalid payload IDs before mutation.
    pub fn decl_annotation_mut(
        &mut self,
        declaration: DeclId,
    ) -> Result<&mut AnnoR, crate::ir::ir::dispatch::DispatchError> {
        use crate::ir::ir::dispatch::DispatchError;
        let mut id = self
            .decl_node(declaration)
            .ok_or(DispatchError::InvalidDeclaration { decl: declaration })?
            .anno;
        if id.is_none() {
            id = AnnotationId::new(self.annotations.len());
            self.annotations.push(AnnoR::default());
            self.decl_nodes[declaration.raw()].anno = id;
        } else if self.annotation(id).is_none() {
            return Err(DispatchError::InvalidHierarchy { decl: declaration });
        }
        Ok(&mut self.annotations[id.raw()])
    }

    /// Replace a declaration's annotation without retaining historical values.
    /// Nonempty declaration/arc slots must be exclusive, as checked by coherence
    /// admission. Raw record mutation must preserve this ownership invariant.
    /// # Errors
    /// Rejects an absent declaration or invalid payload ID before mutation.
    pub fn set_decl_annotation(
        &mut self,
        declaration: DeclId,
        anno: AnnoR,
    ) -> Result<(), crate::ir::ir::dispatch::DispatchError> {
        use crate::ir::ir::dispatch::DispatchError;
        let id = self
            .decl_node(declaration)
            .ok_or(DispatchError::InvalidDeclaration { decl: declaration })?
            .anno;
        if id.is_none() && anno.tags.is_empty() && anno.value.is_none() {
            return Ok(());
        }
        *self.decl_annotation_mut(declaration)? = anno;
        Ok(())
    }

    /// Store an ordered base list. Empty input uses NONE without a table entry.
    /// References and their payload IDs must belong to this IR; order and
    /// multiplicity are preserved. An exact match reuses the latest nonempty
    /// list; other inputs use an exact boxed slice. IDs need not be distinct.
    /// No declaration is created or rebound.
    pub fn store_bases(&mut self, bases: Vec<SignedRefR>) -> BaseListId {
        if bases.is_empty() {
            BaseListId::NONE
        } else if let Some(id) = self.matching_bases(|stored| stored == bases) {
            id
        } else {
            self.append_bases(bases)
        }
    }

    /// Check a single existing candidate, never scan or index the whole pool.
    /// Payloads are resolved afresh, so symbol rewrites cannot stale a cache key.
    fn matching_bases(&self, matches: impl FnOnce(&[SignedRefR]) -> bool) -> Option<BaseListId> {
        let last = self.base_lists.len().checked_sub(1)?;
        matches(&self.base_lists[last]).then_some(BaseListId::new(last))
    }

    fn append_bases(&mut self, bases: Vec<SignedRefR>) -> BaseListId {
        let id = BaseListId::new(self.base_lists.len());
        self.base_lists.push(bases.into_boxed_slice());
        id
    }

    /// Shared construction/decode boundary. Reuse before allocating canonical
    /// reference payloads or resizing the owned draft vector's allocation.
    pub(crate) fn store_base_drafts(&mut self, bases: Vec<ReferenceDraft>) -> BaseListId {
        if bases.is_empty() {
            return BaseListId::NONE;
        }
        if let Some(id) = self.matching_bases(|stored| {
            stored.len() == bases.len()
                && stored.iter().zip(&bases).all(|(reference, draft)| {
                    reference.target() == draft.atom.target
                        && reference.sign_kind() == draft.sign
                        && reference.direction() == draft.atom.direction
                        && self
                            .reference_atom(reference)
                            .is_ok_and(|atom| draft.atom.matches(atom))
                })
        }) {
            return id;
        }
        let references = self.store_references(bases);
        self.append_bases(references)
    }

    /// Borrow a model-local base list. NONE denotes an empty list; an invalid
    /// nonempty handle returns None. No contiguous table copy is requested.
    pub fn bases(&self, id: BaseListId) -> Option<&[SignedRefR]> {
        if id.is_none() {
            Some(&[])
        } else {
            self.base_lists.get_record(id.raw()).map(AsRef::as_ref)
        }
    }

    /// Move a construction payload into this model. The target's semantic
    /// validity is checked at construction/admission, as for a plain reference.
    /// None and an explicitly present empty vector remain distinct.
    pub fn store_reference(&mut self, draft: ReferenceDraft) -> SignedRefR {
        let ReferenceDraft { sign, atom } = draft;
        let RefAtomR {
            target,
            anno,
            weights,
            direction,
        } = atom;
        let annotation = self.store_annotation(anno);
        let weights = weights.map_or(WeightVectorId::NONE, |values| {
            let id = WeightVectorId::new(self.ref_weights.len());
            self.ref_weights.push(values);
            id
        });
        SignedRefR {
            target,
            annotation,
            weights,
            sign,
            direction,
        }
    }

    pub(crate) fn store_references(
        &mut self,
        drafts: impl IntoIterator<Item = ReferenceDraft>,
    ) -> Vec<SignedRefR> {
        drafts
            .into_iter()
            .map(|draft| self.store_reference(draft))
            .collect::<Vec<_>>()
            // Vec's in-place collect can retain the larger draft allocation.
            // Compact once at this owner boundary, including per-record slack.
            .into_boxed_slice()
            .into_vec()
    }

    /// Resolve a reference's payload IDs in this model without allocation.
    /// # Errors
    /// Rejects out-of-range annotation/weight IDs. Numeric IDs cannot distinguish
    /// a foreign model when it happens to contain the same numeric indices.
    pub fn reference_atom(
        &self,
        reference: &SignedRefR,
    ) -> Result<ReferenceView<'_>, ReferenceError> {
        let anno = self
            .annotation(reference.annotation)
            .ok_or(ReferenceError::Annotation(reference.annotation))?;
        let weights = if reference.weights.is_none() {
            None
        } else {
            Some(
                self.ref_weights
                    .get(reference.weights.raw())
                    .ok_or(ReferenceError::Weights(reference.weights))?
                    .as_slice(),
            )
        };
        Ok(ReferenceView {
            target: reference.target,
            anno,
            weights,
            direction: reference.direction,
        })
    }

    /// Explicit owned edit/copy input; ordinary readers should use reference_atom.
    pub fn reference_draft(
        &self,
        reference: &SignedRefR,
    ) -> Result<ReferenceDraft, ReferenceError> {
        let atom = self.reference_atom(reference)?;
        Ok(ReferenceDraft::new(
            reference.sign,
            RefAtomR {
                target: atom.target,
                anno: atom.anno.clone(),
                weights: atom.weights.map(<[ValueR]>::to_vec),
                direction: atom.direction,
            },
        ))
    }
}

impl std::ops::Index<AnnotationId> for Ir {
    type Output = AnnoR;
    /// # Panics
    /// Panics for an invalid nonempty model-local ID. Use annotation for checked access.
    fn index(&self, id: AnnotationId) -> &Self::Output {
        match self.annotation(id) {
            Some(value) => value,
            None => panic!("annotation ID out of range: {}", id.raw()),
        }
    }
}

impl std::ops::Index<BaseListId> for Ir {
    type Output = [SignedRefR];
    /// # Panics
    /// Panics for an invalid nonempty model-local ID. Use bases for checked access.
    fn index(&self, id: BaseListId) -> &Self::Output {
        match self.bases(id) {
            Some(value) => value,
            None => panic!("base-list ID out of range: {}", id.raw()),
        }
    }
}

#[cfg(test)]
mod tests;
