use crate::common::ids::{AnnotationId, BaseListId, DeclId, EdgeId, HyperArcId, NodeId, SymId};
use crate::ir::meta::Meta;
use crate::ir::reference::wire as reference_wire;
use crate::ir::table::IrTable;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclKind {
    Node,
    Edge,
    HyperArc,
}

/// One declaration schema: construction/wire inputs use `DeclNode<AnnoR>`;
/// canonical tables use the default, model-local `AnnotationId` payload.
/// Serialize the owning `Ir` to retain logical annotations in the wire. Compact
/// records deliberately cannot serialize their handles as annotation contents.
/// ```compile_fail
/// use hymeko::ir::ir::DeclNode;
/// fn serializable<T: serde::Serialize>() {}
/// serializable::<DeclNode>(); // serialize the owning Ir instead
/// ```
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct DeclNode<A = AnnotationId> {
    pub kind: DeclKind,
    pub name: SymId,
    pub parent: DeclId,
    pub first_child: DeclId,
    pub last_child: DeclId,
    pub next_sibling: DeclId,
    pub anno: A,
}

impl<A: AsRef<AnnoR>> Serialize for DeclNode<A> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("DeclNode", 7)?;
        fields.serialize_field("kind", &self.kind)?;
        fields.serialize_field("name", &self.name)?;
        fields.serialize_field("parent", &self.parent)?;
        fields.serialize_field("first_child", &self.first_child)?;
        fields.serialize_field("last_child", &self.last_child)?;
        fields.serialize_field("next_sibling", &self.next_sibling)?;
        fields.serialize_field("anno", self.anno.as_ref())?;
        fields.end()
    }
}

impl<A> DeclNode<A> {
    /// Construct an unlinked root declaration with the supplied metadata.
    /// This does not allocate an ID, intern its name, or create a record binding.
    pub fn root(kind: DeclKind, name: SymId, anno: A) -> Self {
        Self {
            kind,
            name,
            anno,
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
        }
    }
    pub(crate) fn map_annotation<B>(self, map: impl FnOnce(A) -> B) -> DeclNode<B> {
        DeclNode {
            kind: self.kind,
            name: self.name,
            parent: self.parent,
            first_child: self.first_child,
            last_child: self.last_child,
            next_sibling: self.next_sibling,
            anno: map(self.anno),
        }
    }
}

/// Canonical records. Compiled snapshots share tables; mutable access detaches
/// the affected table so older snapshots retain their records and symbol IDs.
#[derive(Debug, Default, Clone)]
pub struct Ir {
    pub meta: Option<Meta>,
    // Unified Arena
    pub decl_nodes: IrTable<DeclNode>,

    /// Optional canonical source paths. Metadata, excluded from model hashes.
    pub files: IrTable<String>,
    /// File-table indices keyed by DeclId, never NodeId. Empty means legacy IR
    /// without provenance; u32::MAX means a synthetic declaration with no source.
    pub decl_file: IrTable<u32>,

    // “konkrét” táblák
    pub nodes: IrTable<NodeRec>, // NodeId indexel
    pub edges: IrTable<EdgeRec>, // EdgeId indexel
    pub arcs: IrTable<ArcRec>,   // ArcId indexel
    pub(crate) annotations: IrTable<AnnoR>,
    pub(crate) base_lists: IrTable<Box<[SignedRefR]>>,
    pub(crate) ref_weights: IrTable<Vec<ValueR>>,

    // Compact declaration dispatch (option D, 2026-08-31): one 8-byte slot per
    // decl replaces the former three `Vec<Option<Id>>` maps (48 B/decl -> 8 B/decl).
    // Storage is PRIVATE; the public read contract is the accessor API
    // (`as_node`/`as_edge`/`as_arc`, `node_for_decl`/..., `contains_*`); writes go
    // through the checked typed mutators `bind_{node,edge,arc}_decl`. See the
    // `dispatch` module for the checked encoding. The legacy three-map serde wire is
    // preserved byte-for-byte by the manual `Serialize`/`Deserialize` impls below.
    pub(crate) decl_to_record: IrTable<u64>,

    // Canonical namespace roots (U2.1, 2026-08-31): the import namespace prefix
    // lowering applies to each imported module's root declarations -- the one piece
    // of the hierarchy the intrusive links do not encode (an imported root has
    // `parent == NONE`), and which the v1 canonical hash embeds via the path.
    // Stored so the `Ir` is a complete post-lowering authority.
    //
    // Invariant: sorted ascending by `DeclId`; each `DeclId` is a live link-root;
    // prefixes are non-empty (single-module roots absent -> zero cost). Storage is
    // PRIVATE; reads via `namespace_prefix`, writes via `record_namespace_root`.
    pub(crate) namespace_roots: IrTable<(DeclId, Box<[SymId]>)>,

    // APPROVED-CORE-EDIT: executable-model-contract (2026-09-06).
    //
    // The OPTIONAL executable-model facet has one slot per declaration keyed by `DeclId`.
    // It is a side table rather than a `DeclNode` field for two
    // reasons: `DeclNode` has 17 literal construction sites across core and its consumers, and the
    // struct-of-arrays layout is what the surrounding code already uses for per-declaration data.
    //
    // MOST DECLARATIONS ARE NOT MODEL ELEMENTS. The tracked corpus describes service networks,
    // robotics meta-graphs and P-graph structures; none of them carries a facet, and for those
    // documents this vector stays empty and costs nothing. A document is an *executable model*
    // exactly when at least one declaration carries a facet, and only then does
    // `coherence::validate_model_contract` engage.
    //
    // Storage is PRIVATE: reads via `model_facet`, writes via `set_model_facet`. The wire is
    // additive and conditional (see `Serialize`), so a document without facets serializes
    // byte-identically to the pre-facet form.
    pub(crate) decl_model: IrTable<Option<ModelFacet>>,
}

/// What role a tensor plays in an executable model.
///
/// A closed set: a declaration that is part of a model is exactly one of these, and a reader that
/// does not recognise a variant is reading a newer schema, not guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TensorRole {
    /// A model input, supplied by the caller.
    Input,
    /// A model output, produced by the model.
    Output,
    /// A learnable parameter.
    Parameter,
    /// Persistent state carried across invocations.
    State,
    /// An internal value, neither supplied nor returned.
    Intermediate,
}

/// Element type of a tensor.
///
/// Deliberately small and closed. This is a *type tag*, not a numeric tower: core gains no
/// arithmetic, no storage and no runtime tensor framework from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DType {
    F32,
    F64,
    I32,
    I64,
    U8,
    Bool,
}

/// One dimension of a shape.
///
/// Rank-general by construction, so the later vision work is not blocked; the first consumer
/// exercises only the small ranks an MLP needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Extent {
    /// A known extent.
    Static(u64),
    /// A named extent, resolved by the consumer (batch size, sequence length).
    Symbolic(SymId),
}

/// The tensor contract of a declaration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TensorContract {
    pub role: TensorRole,
    /// Dimensions, outermost first. An empty shape is a scalar.
    pub shape: Vec<Extent>,
    pub dtype: DType,
}

/// The operator contract of a declaration.
///
/// `identity` is an interned symbol in a **typed position**: core owns the fact that this
/// declaration *is* an operator and that this symbol *is* its identity. Core deliberately does not
/// own the vocabulary — what `linear` or `relu` mean is the consuming framework's, and an operator
/// registry in core is explicitly out of scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperatorContract {
    pub identity: SymId,
}

/// The optional executable-model facet of a declaration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelFacet {
    Tensor(TensorContract),
    Operator(OperatorContract),
}

/// Checked encode/decode for the compact declaration-dispatch slot (option D).
///
/// A slot is a `u64`: `0` means NONE; otherwise the top two bits carry the
/// [`DeclKind`] tag (`Node=0`, `Edge=1`, `HyperArc=2`; `3` is reserved/invalid) and
/// the low 62 bits carry `raw_record_index + 1` (so `0` stays reserved for NONE).
/// Every conversion is checked: an index that does not fit the 62-bit domain, a
/// reserved tag, or a `u64`->`usize` overflow is a typed [`DispatchError`] — never a
/// silent truncation, and never `unsafe`.
pub mod dispatch {
    use super::{DeclId, DeclKind};

    pub(crate) const NONE: u64 = 0;
    const TAG_SHIFT: u32 = 62;
    const INDEX_MASK: u64 = (1u64 << TAG_SHIFT) - 1; // low 62 bits
    pub(crate) const TAG_NODE: u64 = 0;
    pub(crate) const TAG_EDGE: u64 = 1;
    pub(crate) const TAG_ARC: u64 = 2;
    pub(crate) const TAG_RESERVED: u64 = 3;

    /// A declaration-dispatch slot could not be encoded or decoded. Ids-only,
    /// bounded numeric metadata; deterministic.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DispatchError {
        InvalidDeclaration {
            decl: DeclId,
        },
        KindMismatch {
            decl: DeclId,
            expected: DeclKind,
        },
        RecordOutOfRange {
            kind: DeclKind,
            index: usize,
        },
        WrongBacklink {
            decl: DeclId,
        },
        ConflictingBinding {
            decl: DeclId,
        },
        InvalidHierarchy {
            decl: DeclId,
        },
        /// A record index does not fit the 62-bit dispatch index domain.
        IndexOverflow {
            index: usize,
        },
        /// A slot carries the reserved (invalid) tag `3`.
        ReservedTag {
            slot: u64,
        },
        /// A non-NONE slot whose index field is `0` (i.e. `raw+1 == 0`): malformed.
        MalformedZeroIndex {
            slot: u64,
        },
        /// A slot's decoded `u64` index did not fit `usize` on this target.
        UsizeOverflow {
            value: u64,
        },
    }

    impl core::fmt::Display for DispatchError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::InvalidDeclaration { decl } => write!(f, "invalid declaration {decl:?}"),
                Self::KindMismatch { decl, expected } => write!(f, "{decl:?} is not {expected:?}"),
                Self::RecordOutOfRange { kind, index } => {
                    write!(f, "{kind:?} record {index} out of range")
                }
                Self::WrongBacklink { decl } => write!(f, "record does not belong to {decl:?}"),
                Self::ConflictingBinding { decl } => write!(f, "conflicting binding for {decl:?}"),
                Self::InvalidHierarchy { decl } => write!(f, "invalid hierarchy at {decl:?}"),
                Self::IndexOverflow { index } => {
                    write!(f, "record index {index} exceeds the 62-bit dispatch domain")
                }
                Self::ReservedTag { slot } => {
                    write!(f, "dispatch slot {slot:#x} carries the reserved tag")
                }
                Self::MalformedZeroIndex { slot } => {
                    write!(
                        f,
                        "dispatch slot {slot:#x} has a non-NONE tag but a zero index"
                    )
                }
                Self::UsizeOverflow { value } => {
                    write!(
                        f,
                        "dispatch index {value} does not fit usize on this target"
                    )
                }
            }
        }
    }
    impl std::error::Error for DispatchError {}

    #[inline(always)]
    pub(crate) const fn tag_of(kind: DeclKind) -> u64 {
        match kind {
            DeclKind::Node => TAG_NODE,
            DeclKind::Edge => TAG_EDGE,
            DeclKind::HyperArc => TAG_ARC,
        }
    }

    /// Checked encode `(kind, raw index) -> slot`. `Err` on 62-bit index overflow.
    #[inline]
    pub(crate) fn encode(kind: DeclKind, index: usize) -> Result<u64, DispatchError> {
        let bumped = (index as u64)
            .checked_add(1)
            .ok_or(DispatchError::IndexOverflow { index })?;
        if bumped > INDEX_MASK {
            return Err(DispatchError::IndexOverflow { index });
        }
        Ok((tag_of(kind) << TAG_SHIFT) | bumped)
    }

    /// Fully checked decode `slot -> Option<(tag, raw index)>`. `None` slot -> `Ok(None)`.
    /// Reserved tag / malformed-zero-index / usize overflow -> typed `Err`. Used on the
    /// validation/deserialize paths.
    #[inline]
    pub(crate) fn decode_checked(slot: u64) -> Result<Option<(u64, usize)>, DispatchError> {
        if slot == NONE {
            return Ok(None);
        }
        let tag = slot >> TAG_SHIFT;
        if tag == TAG_RESERVED {
            return Err(DispatchError::ReservedTag { slot });
        }
        let bumped = slot & INDEX_MASK;
        if bumped == 0 {
            return Err(DispatchError::MalformedZeroIndex { slot });
        }
        let value = bumped - 1;
        let index = usize::try_from(value).map_err(|_| DispatchError::UsizeOverflow { value })?;
        Ok(Some((tag, index)))
    }

    /// Hot-path decode for one expected tag on a coherence-validated `Ir`: total,
    /// allocation-free, single load. Returns the raw index iff the slot is non-NONE
    /// and carries `want_tag`; otherwise `None`. Well-formedness (tag != reserved,
    /// index representable) is guaranteed by construction (checked `encode`) and by
    /// the coherence validator, so this path needs no error arm. On a 64-bit target
    /// `usize == u64`, so the mask-and-subtract is lossless.
    #[inline(always)]
    pub(crate) fn decode_tag(slot: u64, want_tag: u64) -> Option<usize> {
        if slot != NONE && (slot >> TAG_SHIFT) == want_tag {
            // low 62 bits hold raw+1 >= 1 for a well-formed slot.
            Some(((slot & INDEX_MASK) - 1) as usize)
        } else {
            None
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn none_slot_decodes_to_absence() {
            assert_eq!(NONE, 0);
            assert_eq!(decode_checked(NONE), Ok(None));
            assert_eq!(decode_tag(NONE, TAG_NODE), None);
            assert_eq!(decode_tag(NONE, TAG_EDGE), None);
            assert_eq!(decode_tag(NONE, TAG_ARC), None);
        }

        #[test]
        fn encode_zero_index_sets_only_the_tag_bit_plus_one() {
            assert_eq!(encode(DeclKind::Node, 0), Ok(1));
            assert_eq!(encode(DeclKind::Edge, 0), Ok((TAG_EDGE << TAG_SHIFT) | 1));
            assert_eq!(
                encode(DeclKind::HyperArc, 0),
                Ok((TAG_ARC << TAG_SHIFT) | 1)
            );
        }

        #[test]
        fn encode_decode_round_trips_each_kind() {
            for &(k, tag) in &[
                (DeclKind::Node, TAG_NODE),
                (DeclKind::Edge, TAG_EDGE),
                (DeclKind::HyperArc, TAG_ARC),
            ] {
                for &i in &[0usize, 1, 2, 63, 64, 65, 1000, 1_000_000] {
                    let slot = encode(k, i).expect("in-domain index encodes");
                    assert_eq!(decode_checked(slot), Ok(Some((tag, i))));
                    assert_eq!(decode_tag(slot, tag), Some(i));
                    let other = (tag + 1) % 3; // a different, valid tag
                    assert_eq!(decode_tag(slot, other), None);
                }
            }
        }

        #[test]
        fn max_representable_index_encodes_and_overflow_is_rejected() {
            let max_index = (INDEX_MASK - 1) as usize; // raw+1 == INDEX_MASK
            let slot = encode(DeclKind::Node, max_index).expect("max index fits");
            assert_eq!(slot & INDEX_MASK, INDEX_MASK);
            assert_eq!(decode_checked(slot), Ok(Some((TAG_NODE, max_index))));
            let over = max_index + 1;
            assert_eq!(
                encode(DeclKind::Node, over),
                Err(DispatchError::IndexOverflow { index: over })
            );
            assert_eq!(
                encode(DeclKind::Edge, usize::MAX),
                Err(DispatchError::IndexOverflow { index: usize::MAX })
            );
        }

        #[test]
        fn reserved_tag_is_always_an_error_but_never_a_false_hit() {
            let slot = (TAG_RESERVED << TAG_SHIFT) | 1;
            assert_eq!(
                decode_checked(slot),
                Err(DispatchError::ReservedTag { slot })
            );
            assert_eq!(decode_tag(slot, TAG_NODE), None);
            assert_eq!(decode_tag(slot, TAG_EDGE), None);
            assert_eq!(decode_tag(slot, TAG_ARC), None);
        }

        #[test]
        fn non_none_zero_index_is_malformed() {
            let slot = TAG_EDGE << TAG_SHIFT; // tag set, index bits zero, != NONE
            assert_ne!(slot, NONE);
            assert_eq!(
                decode_checked(slot),
                Err(DispatchError::MalformedZeroIndex { slot })
            );
        }

        #[test]
        fn every_dispatch_error_formats_nonempty() {
            let errs = [
                DispatchError::IndexOverflow { index: 9 },
                DispatchError::ReservedTag {
                    slot: (TAG_RESERVED << TAG_SHIFT) | 1,
                },
                DispatchError::MalformedZeroIndex {
                    slot: TAG_EDGE << TAG_SHIFT,
                },
                DispatchError::UsizeOverflow { value: u64::MAX },
            ];
            for e in errs {
                assert!(!e.to_string().is_empty());
                assert!(!format!("{e:?}").is_empty());
            }
        }
    }
}

/// Every table of an [`Ir`]: the one list [`Ir::shrink_to_fit`] and its test read, so neither can
/// miss a table the other has.
macro_rules! ir_tables {
    ($apply:ident!($($arg:tt)*)) => {
        $apply!($($arg)*; decl_nodes, files, decl_file, nodes, edges, arcs,
            decl_to_record, namespace_roots, decl_model, annotations, base_lists, ref_weights)
    };
}

impl Ir {
    /// Publish completed tables without copying records. Owned construction and
    /// edits stay cheap; all subsequent snapshot clones share these allocations.
    /// ```
    /// use hymeko::ir::ir::Ir;
    /// let live = Ir::default().into_shared();
    /// let snapshot = live.clone();
    /// assert_eq!(live.decl_count(), snapshot.decl_count());
    /// ```
    pub fn into_shared(mut self) -> Self {
        macro_rules! share {
            ($ir:expr; $($table:ident),*) => { $( $ir.$table.share(); )* };
        }
        ir_tables!(share!(self));
        self
    }

    pub(crate) fn validate_source_provenance(&self) -> Result<(), IrDecodeError> {
        if self.files.is_empty() && self.decl_file.is_empty() {
            return Ok(());
        }
        if self.decl_file.len() != self.decl_nodes.len()
            || self
                .decl_file
                .iter()
                .any(|&id| id != u32::MAX && id as usize >= self.files.len())
        {
            return Err(IrDecodeError::InvalidSourceProvenance);
        }
        Ok(())
    }

    pub fn new(meta: Meta) -> Self {
        Self {
            meta: Some(meta),
            ..Default::default()
        }
    }

    /// Record an imported module's namespace prefix for a root declaration (U2.1).
    ///
    /// Crate-private construction seam, called by lowering while the resolution
    /// `Index` is still available. `prefix` is the namespace segments applied
    /// before the root's own name; empty prefixes (single-module roots) are not
    /// recorded. Lowering visits declarations in ascending `DeclId`, so entries
    /// are appended in order and the sorted invariant holds (asserted in debug).
    ///
    /// # Preconditions
    /// `root` is a live link-root (its `parent` is `NONE` or a non-Node/Edge).
    /// # Postconditions
    /// On a non-empty `prefix`, `namespace_prefix(root)` returns it thereafter.
    pub(crate) fn record_namespace_root(&mut self, root: DeclId, prefix: &[SymId]) {
        if prefix.is_empty() {
            return;
        }
        debug_assert!(
            self.namespace_roots
                .last()
                .is_none_or(|(d, _)| d.raw() < root.raw()),
            "namespace roots must be recorded in ascending DeclId order"
        );
        self.namespace_roots
            .push((root, prefix.to_vec().into_boxed_slice()));
    }

    /// The import namespace prefix of a root declaration, or `None` for a
    /// single-module (unprefixed) root.
    ///
    /// Total read-only accessor; `O(log R)` over the namespace-root table
    /// (`R` = number of imported roots), no allocation, no `HashMap`.
    #[must_use]
    pub fn namespace_prefix(&self, root: DeclId) -> Option<&[SymId]> {
        self.namespace_roots
            .binary_search_by(|(d, _)| d.raw().cmp(&root.raw()))
            .ok()
            .map(|i| &*self.namespace_roots[i].1)
    }

    // M2a.1: no explicit `is_none()` guard on the Option-returning lookups.
    // `NONE` is `usize::MAX` and a `Vec` length can never reach it, so the bounds
    // check already rejects the sentinel. The guard was not merely redundant --
    // it cost 4 extra instructions per lookup, because LLVM cannot prove
    // `x < len` implies `x != usize::MAX` and lowered the pair to branchless
    // `setne`/`setb`/`testb` rather than a single predicted branch. The guards on
    // `parent`/`first_child`/`next_sibling` are NOT redundant and are retained:
    // those index directly afterward and would panic.
    #[inline]
    pub fn decl_node(&self, id: DeclId) -> Option<&DeclNode> {
        self.decl_nodes.get_record(id.raw())
    }

    #[inline]
    pub fn decl_node_unchecked(&self, id: DeclId) -> &DeclNode {
        assert!(id.is_some(), "attempted to dereference DeclId::NONE");
        &self.decl_nodes[id.0]
    }

    pub fn decl_children(&self, parent: DeclId) -> DeclChildren<'_> {
        self.child_decls(parent)
    }

    pub fn decl_kind(&self, did: DeclId) -> DeclKind {
        self.decl_nodes[did.0].kind
    }

    #[inline]
    pub fn first_child(&self, item: DeclId) -> DeclId {
        if item.is_none() {
            return DeclId::NONE;
        }
        self.decl_nodes[item.0].first_child
    }

    #[inline]
    pub fn next_sibling(&self, item: DeclId) -> DeclId {
        if item.is_none() {
            return DeclId::NONE;
        }
        self.decl_nodes[item.0].next_sibling
    }

    #[inline]
    pub fn parent(&self, item: DeclId) -> DeclId {
        if item.is_none() {
            return DeclId::NONE;
        }
        self.decl_nodes[item.0].parent
    }

    #[inline]
    pub fn kind(&self, item: DeclId) -> DeclKind {
        self.decl_nodes[item.0].kind
    }

    /// Children iterator for *any* item (node/edge/arc).
    pub fn children(&self, parent: DeclId) -> DeclChildren<'_> {
        self.decl_children(parent)
    }

    #[inline]
    pub fn as_node(&self, d: DeclId) -> Option<NodeId> {
        dispatch::decode_tag(*self.decl_to_record.get(d.raw())?, dispatch::TAG_NODE)
            .map(NodeId::new)
    }
    #[inline]
    pub fn as_edge(&self, d: DeclId) -> Option<EdgeId> {
        dispatch::decode_tag(*self.decl_to_record.get(d.raw())?, dispatch::TAG_EDGE)
            .map(EdgeId::new)
    }
    #[inline]
    pub fn as_arc(&self, d: DeclId) -> Option<HyperArcId> {
        dispatch::decode_tag(*self.decl_to_record.get(d.raw())?, dispatch::TAG_ARC)
            .map(HyperArcId::new)
    }

    /// Append a declaration to the arena and its (NONE) dispatch slot, returning its
    /// `DeclId`. The construction seam for callers that build an `Ir` by hand (the
    /// dispatch storage is private); pair with `bind_*_decl` to bind the record.
    #[inline]
    pub fn push_decl(&mut self, node: DeclNode<AnnoR>) -> DeclId {
        let node = node.map_annotation(|anno| self.store_annotation(anno));
        self.push_decl_slot(node, dispatch::NONE)
    }

    /// Shared append primitive. Construction supplies an already checked slot;
    /// raw declaration append deliberately keeps the unbound NONE value.
    #[inline]
    fn push_decl_slot(&mut self, node: DeclNode, slot: u64) -> DeclId {
        let id = DeclId::new(self.decl_nodes.len());
        if !self.files.is_empty() || !self.decl_file.is_empty() {
            self.decl_file.push(u32::MAX);
        }
        self.decl_nodes.push(node);
        self.decl_to_record.push(slot);
        // The facet table is grown lazily: it stays EMPTY for a document with no model facet, so a
        // non-model program pays nothing and serializes byte-identically to the pre-facet form.
        if !self.decl_model.is_empty() {
            self.decl_model.push(None);
        }
        id
    }

    /// The executable-model facet of a declaration, if it has one.
    ///
    /// `None` for every declaration of a document that is not an executable model, which is the
    /// ordinary case.
    #[must_use]
    pub fn model_facet(&self, d: DeclId) -> Option<&ModelFacet> {
        self.decl_model.get(d.raw()).and_then(Option::as_ref)
    }

    /// Attach (or clear) a declaration's model facet.
    ///
    /// Grows the table on first use, so the empty-table fast path survives until a document
    /// actually declares a model.
    ///
    /// # Preconditions
    /// `d` indexes a live declaration.
    /// # Postconditions
    /// `model_facet(d)` returns what was set.
    pub fn set_model_facet(&mut self, d: DeclId, facet: Option<ModelFacet>) {
        debug_assert!(d.raw() < self.decl_nodes.len(), "decl out of range");
        if self.decl_model.is_empty() {
            if facet.is_none() {
                return;
            }
            self.decl_model.resize(self.decl_nodes.len(), None);
        }
        if self.decl_model.len() < self.decl_nodes.len() {
            self.decl_model.resize(self.decl_nodes.len(), None);
        }
        self.decl_model[d.raw()] = facet;
    }

    /// Length of the facet table, for the coherence length check.
    #[must_use]
    pub fn model_facet_table_len(&self) -> usize {
        self.decl_model.len()
    }

    /// Whether this `Ir` declares an executable model at all.
    ///
    /// The switch that decides whether the model-contract validator engages.
    #[must_use]
    pub fn declares_executable_model(&self) -> bool {
        self.decl_model.iter().any(Option::is_some)
    }

    /// Checked typed binding: `decl (kind Node) -> node record`. Checks the
    /// declaration kind, record range/backlink and index representability, and
    /// refuses a second, *different* binding. On any error the slot is left exactly
    /// as it was (no half-written state).
    #[inline]
    pub fn bind_node_decl(
        &mut self,
        d: DeclId,
        nid: NodeId,
    ) -> Result<(), dispatch::DispatchError> {
        self.bind_decl(d, DeclKind::Node, nid.raw())
    }
    #[inline]
    pub fn bind_edge_decl(
        &mut self,
        d: DeclId,
        eid: EdgeId,
    ) -> Result<(), dispatch::DispatchError> {
        self.bind_decl(d, DeclKind::Edge, eid.raw())
    }
    #[inline]
    pub fn bind_arc_decl(
        &mut self,
        d: DeclId,
        aid: HyperArcId,
    ) -> Result<(), dispatch::DispatchError> {
        self.bind_decl(d, DeclKind::HyperArc, aid.raw())
    }

    /// Shared checked binder. Encodes first (so an index overflow aborts before any
    /// write), then refuses overwriting an existing slot with a *different* value.
    fn bind_decl(
        &mut self,
        d: DeclId,
        kind: DeclKind,
        index: usize,
    ) -> Result<(), dispatch::DispatchError> {
        let slot = dispatch::encode(kind, index)?;
        let node = self
            .decl_node(d)
            .ok_or(dispatch::DispatchError::InvalidDeclaration { decl: d })?;
        if node.kind != kind {
            return Err(dispatch::DispatchError::KindMismatch {
                decl: d,
                expected: kind,
            });
        }
        let backlink = match kind {
            DeclKind::Node => self.nodes.get(index).map(|r| r.decl == d),
            DeclKind::Edge => self.edges.get(index).map(|r| r.decl == d),
            DeclKind::HyperArc => self.arcs.get(index).map(|_| true),
        }
        .ok_or(dispatch::DispatchError::RecordOutOfRange { kind, index })?;
        if !backlink {
            return Err(dispatch::DispatchError::WrongBacklink { decl: d });
        }
        let current = *self
            .decl_to_record
            .get(d.raw())
            .ok_or(dispatch::DispatchError::InvalidDeclaration { decl: d })?;
        if current != dispatch::NONE && current != slot {
            return Err(dispatch::DispatchError::ConflictingBinding { decl: d });
        }
        if current != slot {
            self.decl_to_record[d.raw()] = slot;
        }
        Ok(())
    }

    /// Checked batch commit: the hierarchy and bindings stay unchanged and no
    /// edit occurs unless every target, including a retained value, is a bound
    /// leaf node. Kept records never request mutable table access. Value-domain checks
    /// (including symbol ownership) belong to the compiled model's interner.
    pub(crate) fn set_leaf_values(
        &mut self,
        values: &mut Vec<LeafValueEdit>,
    ) -> Result<(), dispatch::DispatchError> {
        for edit in values.iter() {
            let id = edit.declaration();
            let node = self
                .decl_node(id)
                .ok_or(dispatch::DispatchError::InvalidDeclaration { decl: id })?;
            if self.node_for_decl(id).is_none()
                || node.first_child.is_some()
                || self.annotation(node.anno).is_none()
            {
                return Err(dispatch::DispatchError::InvalidHierarchy { decl: id });
            }
        }
        for edit in values.drain(..) {
            if let LeafValueEdit::Replace { declaration, value } = edit {
                let id = self.decl_nodes[declaration.raw()].anno;
                if id.is_some() {
                    self.annotations[id.raw()].value = value;
                } else if value.is_some() {
                    let id = self.store_annotation(AnnoR {
                        tags: Vec::new(),
                        value,
                    });
                    self.decl_nodes[declaration.raw()].anno = id;
                }
            }
        }
        Ok(())
    }

    /// Release every table's spare capacity (APPROVED-CORE-EDIT: ir-shrink-after-lowering,
    /// 2026-09-29).
    ///
    /// Lowering sizes the declaration tables for the named declarations and then appends the
    /// anonymous arcs, and the record tables grow as they are filled, so a lowered IR carries
    /// spare capacity of up to its own size: 354 MiB for a 424k-hyperedge graph a consumer kept
    /// after moving it out of its compiled program. Each table is reallocated to its length; the
    /// allocator shrinks a block in place (glibc splits it, or remaps a large block's pages), so
    /// no element is copied to compact the IR, and none changes.
    ///
    /// # Postconditions
    /// Every table's capacity equals its length; every element and every identity is unchanged.
    pub fn shrink_to_fit(&mut self) {
        macro_rules! shrink {
            ($ir:expr; $($table:ident),*) => { $( $ir.$table.shrink_to_fit(); )* };
        }
        ir_tables!(shrink!(self));
    }

    pub fn preallocate_from_index(&mut self, total_named_decls: usize) {
        if !self.files.is_empty() || !self.decl_file.is_empty() {
            self.decl_file.resize(total_named_decls, u32::MAX);
        }
        let default_node = DeclNode::root(DeclKind::Node, SymId::new(0), AnnotationId::NONE);
        // Allocate exact capacity for Nodes and Edges
        self.decl_nodes.resize(total_named_decls, default_node);
        self.decl_to_record
            .resize(total_named_decls, dispatch::NONE);
        if !self.decl_model.is_empty() {
            self.decl_model.resize(total_named_decls, None);
        }
    }

    pub fn push_anonymous_arc(&mut self) -> DeclId {
        self.push_decl(DeclNode::root(
            DeclKind::HyperArc,
            SymId::new(0),
            AnnoR::default(),
        ))
    }
}

/// Expected record counts. Hints allocate neither live entities nor payloads.
/// Underestimates retain ordinary Vec growth; these are capacities, not quotas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrCapacities {
    pub declarations: usize,
    pub nodes: usize,
    pub edges: usize,
    pub arcs: usize,
}

/// An arc's owning hyperedge and its independent declaration containment.
/// Flat editor inputs may use NONE as parent while retaining a real owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArcPlacement {
    pub owner: DeclId,
    pub parent: DeclId,
}

impl ArcPlacement {
    /// The source language places each arc declaration under its owning edge.
    pub fn under_owner(owner: DeclId) -> Self {
        Self {
            owner,
            parent: owner,
        }
    }
}

/// Input adapter for the single checked IR construction owner.
/// Adapters supply data through `populate`; allocation and record/link mutation
/// belong to `IrConstruction`. `finish` packages the IR with input-owned metadata.
pub trait IrBuilder: Sized {
    type Output;
    type Error;
    fn capacities(&self) -> IrCapacities;
    fn populate(&mut self, construction: &mut IrConstruction<'_>) -> Result<(), Self::Error>;
    fn finish(self, ir: Ir) -> Self::Output;

    /// Retain input without constructing any IR tables. The first explicit load
    /// uses this same builder; subsequent loads reuse its result, including errors.
    fn defer(self) -> LazyIr<Self> {
        LazyIr {
            capacities: self.capacities(),
            pending: Some(self),
            result: None,
        }
    }

    /// Build a fresh canonical IR through the shared construction capability.
    /// Failure drops only this candidate; no partial IR is returned.
    fn build(mut self) -> Result<Self::Output, Self::Error> {
        let mut ir = Ir::default();
        self.populate(&mut IrConstruction::with_capacities(
            &mut ir,
            self.capacities(),
        ))?;
        Ok(self.finish(ir))
    }
}

/// Demand construction of one whole input through the canonical builder.
/// This retains the input adapter, not a second graph representation. It is not
/// a disk pager: the first load constructs the whole model. Dropping an unloaded
/// holder drops its input without allocating canonical record tables.
pub struct LazyIr<B: IrBuilder> {
    capacities: IrCapacities,
    pending: Option<B>,
    result: Option<Result<B::Output, B::Error>>,
}

impl<B: IrBuilder> LazyIr<B> {
    pub fn capacities(&self) -> IrCapacities {
        self.capacities
    }

    pub fn is_loaded(&self) -> bool {
        self.result.is_some()
    }

    /// Construct at most once, retaining the typed error on failure as well.
    /// A panic from an input adapter poisons the holder; unwinding is not a retry.
    pub fn load(&mut self) -> Result<&B::Output, &B::Error> {
        if let Some(builder) = self.pending.take() {
            self.result = Some(builder.build());
        }
        match &self.result {
            Some(result) => result.as_ref(),
            None => panic!("input adapter panicked during lazy IR construction"),
        }
    }
}

/// Exclusive construction capability. No raw mutable arena escapes this type.
/// Each operation checks everything that can fail before committing its links,
/// record and dispatch slot.
pub struct IrConstruction<'a> {
    ir: &'a mut Ir,
    parent: Option<ConstructionParent>,
}

/// Valid only for this exclusive construction borrow. No logical IR field is
/// externally mutable while the cursor exists; link_child maintains its tail.
#[derive(Clone, Copy)]
struct ConstructionParent {
    id: DeclId,
    last: DeclId,
}

impl<'a> IrConstruction<'a> {
    /// Borrow the canonical storage exclusively. Operations check their own
    /// construction preconditions; this does not validate the complete model.
    pub fn new(ir: &'a mut Ir) -> Self {
        Self { ir, parent: None }
    }

    /// Plan capacity without creating records. Allocation is deferred until
    /// each table first needs mutable storage; unused tables remain empty.
    pub fn with_capacities(ir: &'a mut Ir, capacity: IrCapacities) -> Self {
        ir.decl_nodes.defer_capacity(capacity.declarations);
        ir.decl_to_record.defer_capacity(capacity.declarations);
        ir.nodes.defer_capacity(capacity.nodes);
        ir.edges.defer_capacity(capacity.edges);
        ir.arcs.defer_capacity(capacity.arcs);
        Self { ir, parent: None }
    }

    /// Set construction metadata without exposing the record tables.
    pub fn set_meta(&mut self, meta: Meta) {
        self.ir.meta = Some(meta);
    }

    /// Materialize the stable named-ID domain needed by indexed lowering.
    /// This is separate from a capacity hint: these slots are addressable
    /// placeholders and will be filled by resolved declarations.
    /// # Errors
    /// Rejects a nonempty or inconsistent construction domain without changing it.
    pub fn prepare_named_slots(&mut self, count: usize) -> Result<(), dispatch::DispatchError> {
        if !self.ir.decl_nodes.is_empty()
            || !self.ir.decl_to_record.is_empty()
            || !self.ir.nodes.is_empty()
            || !self.ir.edges.is_empty()
            || !self.ir.arcs.is_empty()
        {
            return Err(dispatch::DispatchError::InvalidDeclaration {
                decl: DeclId::new(self.ir.decl_count()),
            });
        }
        self.ir.preallocate_from_index(count);
        Ok(())
    }

    /// Append a root node, its record and its checked dispatch binding.
    /// Names and bases are already-resolved caller data; no interning or lowering
    /// is performed. Existing IDs, metadata and hierarchy links are unchanged.
    /// # Errors
    /// Returns a dispatch error without changing storage if parallel declaration
    /// tables disagree or the record index cannot be represented.
    pub fn append_node(
        &mut self,
        name: SymId,
        anno: AnnoR,
        bases: Vec<ReferenceDraft>,
    ) -> Result<DeclId, dispatch::DispatchError> {
        self.append_root(DeclKind::Node, name, anno, bases)
    }

    /// Append a root edge and its bound record, with the same contract as
    /// [`Self::append_node`]. The edge initially owns no arcs.
    pub fn append_edge(
        &mut self,
        name: SymId,
        anno: AnnoR,
        bases: Vec<ReferenceDraft>,
    ) -> Result<DeclId, dispatch::DispatchError> {
        self.append_root(DeclKind::Edge, name, anno, bases)
    }

    fn append_root(
        &mut self,
        kind: DeclKind,
        name: SymId,
        anno: AnnoR,
        bases: Vec<ReferenceDraft>,
    ) -> Result<DeclId, dispatch::DispatchError> {
        self.append_named(DeclNode::root(kind, name, anno), bases, &[])
    }

    /// Append a named node or edge under its declared parent. Namespace prefixes
    /// apply only to roots. Child links and the concrete binding are committed
    /// by the same owner used by indexed lowering.
    /// # Errors
    /// Rejects invalid parallel storage, hierarchy, kind or dispatch capacity
    /// before creating any declaration, concrete record or link.
    pub fn append_named(
        &mut self,
        node: DeclNode<AnnoR>,
        bases: Vec<ReferenceDraft>,
        namespace: &[SymId],
    ) -> Result<DeclId, dispatch::DispatchError> {
        let id = self.check_append()?;
        let slot = self.check_named(id, &node, namespace)?;
        self.commit_named(id, node, bases, namespace, slot);
        Ok(id)
    }

    fn check_append(&self) -> Result<DeclId, dispatch::DispatchError> {
        let id = DeclId::new(self.ir.decl_count());
        if self.ir.decl_to_record.len() != id.raw() {
            return Err(dispatch::DispatchError::InvalidDeclaration { decl: id });
        }
        Ok(id)
    }

    /// Append final data once, or replace the slot reserved by indexed lowering.
    /// Both callers validate admission before preparing records or payloads.
    fn commit_declaration(&mut self, id: DeclId, node: DeclNode, slot: u64) {
        let parent = node.parent;
        if id.raw() == self.ir.decl_count() {
            let appended = self.ir.push_decl_slot(node, slot);
            debug_assert_eq!(appended, id);
        } else {
            self.ir.decl_nodes[id.raw()] = node;
            self.ir.decl_to_record[id.raw()] = slot;
        }
        self.link_child(parent, id);
    }

    fn check_parent(
        &mut self,
        child: DeclId,
        parent: DeclId,
    ) -> Result<(), dispatch::DispatchError> {
        if parent.is_none() {
            return Ok(());
        }
        let invalid = dispatch::DispatchError::InvalidHierarchy { decl: parent };
        if parent == child {
            return Err(invalid);
        }
        if self.parent.is_some_and(|checked| checked.id == parent) {
            return Ok(());
        }
        let node = self.ir.decl_node(parent).ok_or(invalid)?;
        if self.ir.as_node(parent).is_none() && self.ir.as_edge(parent).is_none() {
            return Err(invalid);
        }
        match (node.first_child.is_none(), node.last_child.is_none()) {
            (true, true) => {}
            (false, false) => {
                let last = self.ir.decl_node(node.last_child).ok_or(invalid)?;
                if last.parent != parent || last.next_sibling.is_some() {
                    return Err(invalid);
                }
            }
            _ => return Err(invalid),
        }
        self.parent = Some(ConstructionParent {
            id: parent,
            last: node.last_child,
        });
        Ok(())
    }

    fn link_child(&mut self, parent: DeclId, child: DeclId) {
        if parent.is_none() {
            return;
        }
        let last = self
            .parent
            .filter(|checked| checked.id == parent)
            .map_or_else(
                || self.ir.decl_nodes[parent.raw()].last_child,
                |checked| checked.last,
            );
        if last.is_none() {
            self.ir.decl_nodes[parent.raw()].first_child = child;
        } else {
            self.ir.decl_nodes[last.raw()].next_sibling = child;
        }
        self.ir.decl_nodes[parent.raw()].last_child = child;
        self.parent = Some(ConstructionParent {
            id: parent,
            last: child,
        });
    }

    pub(crate) fn named(
        &mut self,
        id: DeclId,
        node: DeclNode<AnnoR>,
        bases: Vec<ReferenceDraft>,
        namespace: &[SymId],
    ) -> Result<(), dispatch::DispatchError> {
        let current = self
            .ir
            .decl_node(id)
            .ok_or(dispatch::DispatchError::InvalidDeclaration { decl: id })?;
        if self.ir.decl_to_record.get(id.raw()) != Some(&dispatch::NONE) {
            return Err(dispatch::DispatchError::ConflictingBinding { decl: id });
        }
        if current.parent.is_some()
            || current.first_child.is_some()
            || current.last_child.is_some()
            || current.next_sibling.is_some()
        {
            return Err(dispatch::DispatchError::InvalidHierarchy { decl: id });
        }
        let slot = self.check_named(id, &node, namespace)?;
        self.commit_named(id, node, bases, namespace, slot);
        Ok(())
    }

    fn check_named(
        &mut self,
        id: DeclId,
        node: &DeclNode<AnnoR>,
        namespace: &[SymId],
    ) -> Result<u64, dispatch::DispatchError> {
        if node.first_child.is_some() || node.last_child.is_some() || node.next_sibling.is_some() {
            return Err(dispatch::DispatchError::InvalidHierarchy { decl: id });
        }
        self.check_parent(id, node.parent)?;
        if !namespace.is_empty()
            && (node.parent.is_some()
                || self
                    .ir
                    .namespace_roots
                    .last()
                    .is_some_and(|(d, _)| d.raw() >= id.raw()))
        {
            return Err(dispatch::DispatchError::InvalidHierarchy { decl: id });
        }
        let index = match node.kind {
            DeclKind::Node => self.ir.nodes.len(),
            DeclKind::Edge => self.ir.edges.len(),
            DeclKind::HyperArc => {
                return Err(dispatch::DispatchError::KindMismatch {
                    decl: id,
                    expected: DeclKind::Node,
                });
            }
        };
        dispatch::encode(node.kind, index)
    }

    fn commit_named(
        &mut self,
        id: DeclId,
        node: DeclNode<AnnoR>,
        bases: Vec<ReferenceDraft>,
        namespace: &[SymId],
        slot: u64,
    ) {
        let bases = if bases.is_empty() {
            drop(bases); // Preserve the established empty-draft fast path.
            BaseListId::NONE
        } else {
            self.ir.store_base_drafts(bases)
        };
        let node = node.map_annotation(|anno| self.ir.store_annotation(anno));
        match node.kind {
            DeclKind::Node => self.ir.nodes.push(NodeRec::new(id, bases)),
            DeclKind::Edge => self.ir.edges.push(EdgeRec::new(id, bases)),
            DeclKind::HyperArc => unreachable!("kind checked before commit"),
        }
        self.commit_declaration(id, node, slot);
        self.ir.record_namespace_root(id, namespace);
    }

    /// Append an anonymous hyperarc owned by a bound edge. Its declaration,
    /// owner list, hierarchy link and dispatch entry are committed together.
    /// # Errors
    /// Rejects an invalid owner, hierarchy or record index before mutation.
    pub fn append_arc(
        &mut self,
        placement: ArcPlacement,
        anno: AnnoR,
        refs: Vec<ReferenceDraft>,
    ) -> Result<DeclId, dispatch::DispatchError> {
        self.append_arc_iter(placement, anno, refs)
    }

    /// Stream already-resolved references into the canonical compact storage.
    /// This is the implementation also used by the Vec input adapter. No draft
    /// vector is required, and no second construction pipeline is introduced.
    /// # Errors
    /// Checks owner, parallel dispatch, hierarchy and ID capacity before consuming
    /// the iterator or changing records. A caller-supplied iterator panic is not
    /// a checked error and may leave unused entries in the private payload tables.
    pub fn append_arc_iter(
        &mut self,
        placement: ArcPlacement,
        anno: AnnoR,
        refs: impl IntoIterator<Item = ReferenceDraft>,
    ) -> Result<DeclId, dispatch::DispatchError> {
        let id = self.check_append()?;
        let (edge, _) = self.ir.edge_for_decl(placement.owner).ok_or(
            dispatch::DispatchError::KindMismatch {
                decl: placement.owner,
                expected: DeclKind::Edge,
            },
        )?;
        self.check_parent(id, placement.parent)?;
        let aid = HyperArcId::new(self.ir.arcs.len());
        let slot = dispatch::encode(DeclKind::HyperArc, aid.raw())?;
        let refs = self.ir.store_references(refs);
        let declaration_anno = self.ir.store_annotation(anno.clone());
        let anno = self.ir.store_annotation(anno);
        let mut node = DeclNode::root(DeclKind::HyperArc, SymId::new(0), declaration_anno);
        node.parent = placement.parent;
        self.ir.arcs.push(ArcRec {
            anno,
            in_edge: placement.owner,
            refs,
        });
        self.commit_declaration(id, node, slot);
        self.ir.edges[edge.raw()].arcs.push(aid);
        Ok(id)
    }
}

/// Compatibility name used by Nagare's `DeclTreeView` associated iterator.
/// All child traversal is implemented by `access::ChildDecls`.
pub type DeclChildren<'a> = crate::ir::access::ChildDecls<'a>;

#[derive(Debug, Clone)]
pub struct NodeRec {
    pub decl: DeclId,
    pub bases: BaseListId,
}
impl NodeRec {
    pub fn new(decl: DeclId, bases: BaseListId) -> Self {
        Self { decl, bases }
    }
}

#[derive(Debug, Clone)]
pub struct EdgeRec {
    pub decl: DeclId,
    pub bases: BaseListId,
    pub arcs: Vec<HyperArcId>,
}
impl EdgeRec {
    pub fn new(decl: DeclId, bases: BaseListId) -> Self {
        Self {
            decl,
            bases,
            arcs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArcRec {
    pub anno: AnnotationId,
    pub in_edge: DeclId, // melyik edge scope-jában volt (általában EdgeDecl)
    pub refs: Vec<SignedRefR>, // DeclId-kra mutat (determinista)
                         // később: attribútumok/value külön, lásd lent
}

impl ArcRec {
    /// Construct a raw, unowned hyperarc record with default annotation.
    /// No declaration, owner edge or dispatch binding is created. This preserves
    /// the representation used by direct-incidence construction and test inputs.
    pub fn unowned(refs: Vec<SignedRefR>) -> Self {
        Self {
            anno: AnnotationId::NONE,
            in_edge: DeclId::NONE,
            refs,
        }
    }
}

pub use super::reference::{
    IncidenceSign, PortDirection, RefAtomR, ReferenceDraft, ReferenceError, ReferenceView,
    SignedRefR,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueR {
    Str(SymId),
    Num(f64),
    List(Vec<ValueR>),
    Ref(DeclId), // AST Ref(path) -> IR Ref(DeclId)
}

impl ValueR {
    /// Exact representation equality within one symbol/declaration ID domain.
    /// This does not validate IDs or finite numbers. In particular, signed zero
    /// and NaN payloads compare by bits, independently of ordinary PartialEq.
    pub(crate) fn bitwise_eq(&self, other: &Self) -> bool {
        let (Self::List(left), Self::List(right)) = (self, other) else {
            return self.scalar_bits_eq(other);
        };
        if left.len() != right.len() {
            return false;
        }
        let mut parents = Vec::new();
        let mut current = left.iter().zip(right.iter());
        loop {
            match current.next() {
                Some((Self::List(left), Self::List(right))) => {
                    if left.len() != right.len() {
                        return false;
                    }
                    if !left.is_empty() {
                        parents.push(current);
                        current = left.iter().zip(right.iter());
                    }
                }
                Some((left, right)) => {
                    if !left.scalar_bits_eq(right) {
                        return false;
                    }
                }
                None => match parents.pop() {
                    Some(parent) => current = parent,
                    None => return true,
                },
            }
        }
    }

    fn scalar_bits_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Num(left), Self::Num(right)) => left.to_bits() == right.to_bits(),
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::Ref(left), Self::Ref(right)) => left == right,
            _ => false,
        }
    }
}

/// Resolved private batch intent. Keep retains validation while avoiding both
/// payload cloning during staging and copy-on-write detachment during commit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LeafValueEdit {
    Keep(DeclId),
    Replace {
        declaration: DeclId,
        value: Option<ValueR>,
    },
}

impl LeafValueEdit {
    pub(crate) fn declaration(&self) -> DeclId {
        match self {
            Self::Keep(declaration) | Self::Replace { declaration, .. } => *declaration,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnnoR {
    pub tags: Vec<SymId>,
    pub value: Option<ValueR>,
}

impl AsRef<AnnoR> for AnnoR {
    fn as_ref(&self) -> &AnnoR {
        self
    }
}

// ============================================================================
// Serde boundary adapter (2026-08-31): keep the legacy three-map wire byte-for-byte
// while the in-memory `Ir` stores only the compact `decl_to_record`. Serialize is
// manual and BORROWED — the large fields (`decl_nodes`, `nodes`, `edges`, `arcs`,
// meta, hashes) are serialized by reference and never cloned; only the three legacy
// `Vec<Option<Id>>` maps are reconstructed temporarily (the sole compatibility
// allocation). Deserialize reads the legacy three-map wire and is fail-closed:
// every structural inconsistency is a typed error, never a silent reinterpretation.
// ============================================================================

use serde::de::Error as _;
use serde::ser::SerializeStruct as _;

/// Fail-closed decode error when compacting the legacy three-map serde wire into the
/// single `decl_to_record`. Ids-only, bounded, deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrDecodeError {
    /// Source metadata has missing fields, mismatched lengths or invalid file IDs.
    InvalidSourceProvenance,
    /// The three legacy maps do not all share the `decl_nodes` length.
    InvalidMapLength {
        decl_nodes: usize,
        node: usize,
        edge: usize,
        arc: usize,
    },
    /// The same declaration is bound in more than one legacy map.
    MultipleBindings { decl: DeclId },
    /// A declaration's legacy binding does not match its `DeclKind`.
    KindMismatch { decl: DeclId, kind: DeclKind },
    /// A bound record index is outside its concrete table.
    RecordOutOfRange { decl: DeclId },
    /// The bound record's `decl` back-pointer does not point back to `decl`.
    WrongBacklink { decl: DeclId },
    /// The record index did not fit the compact 62-bit dispatch domain.
    DispatchEncode(dispatch::DispatchError),
    /// Namespace metadata carries a schema version this build does not understand.
    UnknownNamespaceSchema { schema: u16 },
    /// A namespace root's `DeclId` is outside `decl_nodes`.
    NamespaceRootOutOfRange { decl: DeclId },
    /// The same declaration appears twice in the namespace-root table.
    DuplicateNamespaceRoot { decl: DeclId },
    /// A namespace root carries an empty prefix (single-module roots must be absent).
    EmptyNamespacePrefix { decl: DeclId },
    /// The namespace-root table is not sorted ascending by `DeclId`.
    UnsortedNamespaceRoots,
}

impl core::fmt::Display for IrDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSourceProvenance => {
                write!(f, "invalid declaration source-file provenance")
            }
            Self::InvalidMapLength {
                decl_nodes,
                node,
                edge,
                arc,
            } => write!(
                f,
                "legacy map length mismatch: decl_nodes={decl_nodes} node={node} edge={edge} arc={arc}"
            ),
            Self::MultipleBindings { decl } => {
                write!(
                    f,
                    "decl {} is bound in more than one legacy map",
                    decl.raw()
                )
            }
            Self::KindMismatch { decl, kind } => {
                write!(
                    f,
                    "decl {} legacy binding disagrees with kind {kind:?}",
                    decl.raw()
                )
            }
            Self::RecordOutOfRange { decl } => {
                write!(f, "decl {} bound to an out-of-range record", decl.raw())
            }
            Self::WrongBacklink { decl } => {
                write!(f, "decl {} record back-pointer does not match", decl.raw())
            }
            Self::DispatchEncode(e) => write!(f, "dispatch encode failed: {e:?}"),
            Self::UnknownNamespaceSchema { schema } => {
                write!(f, "unknown namespace metadata schema version {schema}")
            }
            Self::NamespaceRootOutOfRange { decl } => {
                write!(f, "namespace root decl {} is out of range", decl.raw())
            }
            Self::DuplicateNamespaceRoot { decl } => {
                write!(
                    f,
                    "namespace root decl {} appears more than once",
                    decl.raw()
                )
            }
            Self::EmptyNamespacePrefix { decl } => {
                write!(f, "namespace root decl {} has an empty prefix", decl.raw())
            }
            Self::UnsortedNamespaceRoots => {
                write!(f, "namespace-root table is not sorted by DeclId")
            }
        }
    }
}
impl std::error::Error for IrDecodeError {}

/// Wire schema version for the canonical namespace-root metadata (U2.1).
const NAMESPACE_SCHEMA: u16 = 1;

/// Borrowed serialize view of the namespace-root table (no clone of the prefixes).
#[derive(Serialize)]
struct NamespaceWireRef<'a> {
    schema: u16,
    roots: &'a [(DeclId, Box<[SymId]>)],
}

impl Serialize for Ir {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.validate_source_provenance()
            .map_err(serde::ser::Error::custom)?;
        // Reconstruct the three legacy maps for the wire -- the ONLY compat allocation.
        let n = self.decl_to_record.len();
        let mut d2n: Vec<Option<NodeId>> = vec![None; n];
        let mut d2e: Vec<Option<EdgeId>> = vec![None; n];
        let mut d2a: Vec<Option<HyperArcId>> = vec![None; n];
        for (d, &slot) in self.decl_to_record.iter().enumerate() {
            if let Some((tag, idx)) =
                dispatch::decode_checked(slot).map_err(serde::ser::Error::custom)?
            {
                match tag {
                    dispatch::TAG_NODE => d2n[d] = Some(NodeId::new(idx)),
                    dispatch::TAG_EDGE => d2e[d] = Some(EdgeId::new(idx)),
                    dispatch::TAG_ARC => d2a[d] = Some(HyperArcId::new(idx)),
                    _ => return Err(serde::ser::Error::custom("reserved dispatch tag")),
                }
            }
        }
        // Encode the current IR fields; derived digest caches are not model state.
        let has_ns = !self.namespace_roots.is_empty();
        let has_model = self.decl_model.iter().any(Option::is_some);
        let has_sources = !self.files.is_empty() || !self.decl_file.is_empty();
        let fields =
            8 + usize::from(has_ns) + usize::from(has_model) + 2 * usize::from(has_sources);
        let mut st = s.serialize_struct("Ir", fields)?;
        st.serialize_field("meta", &self.meta)?;
        st.serialize_field(
            "decl_nodes",
            &reference_wire::Records {
                ir: self,
                values: &self.decl_nodes,
            },
        )?;
        st.serialize_field(
            "nodes",
            &reference_wire::Records {
                ir: self,
                values: &self.nodes,
            },
        )?;
        st.serialize_field(
            "edges",
            &reference_wire::Records {
                ir: self,
                values: &self.edges,
            },
        )?;
        st.serialize_field(
            "arcs",
            &reference_wire::Records {
                ir: self,
                values: &self.arcs,
            },
        )?;
        st.serialize_field("decl_to_node", &d2n)?;
        st.serialize_field("decl_to_edge", &d2e)?;
        st.serialize_field("decl_to_arc", &d2a)?;
        if has_sources {
            st.serialize_field("files", &self.files)?;
            st.serialize_field("decl_file", &self.decl_file)?;
        }
        if has_model {
            st.serialize_field("decl_model", &self.decl_model)?;
        }
        if has_ns {
            st.serialize_field(
                "namespace_roots",
                &NamespaceWireRef {
                    schema: NAMESPACE_SCHEMA,
                    roots: &self.namespace_roots,
                },
            )?;
        }
        st.end()
    }
}

/// Owned shadow of the namespace metadata read from the wire.
#[derive(Deserialize)]
struct NamespaceWire {
    schema: u16,
    roots: Vec<(DeclId, Vec<SymId>)>,
}

/// Owned shadow with the legacy layout, used only to read the wire before compacting.
#[derive(Deserialize)]
struct IrWire {
    #[serde(default)]
    files: Option<Vec<String>>,
    #[serde(default)]
    decl_file: Option<Vec<u32>>,
    meta: Option<Meta>,
    decl_nodes: Vec<DeclNode<AnnoR>>,
    nodes: Vec<reference_wire::NodeInput>,
    edges: Vec<reference_wire::EdgeInput>,
    arcs: Vec<reference_wire::ArcInput>,
    decl_to_node: Vec<Option<NodeId>>,
    decl_to_edge: Vec<Option<EdgeId>>,
    decl_to_arc: Vec<Option<HyperArcId>>,
    #[serde(default)]
    namespace_roots: Option<NamespaceWire>,
    /// APPROVED-CORE-EDIT: executable-model-contract. Absent on every pre-facet payload, which
    /// `serde(default)` turns into the empty table -- i.e. "no declaration is a model element".
    #[serde(default)]
    decl_model: Vec<Option<ModelFacet>>,
}

impl Ir {
    /// Fail-closed compaction of the legacy three-map wire into the compact form.
    fn try_from_wire(w: IrWire) -> Result<Self, IrDecodeError> {
        let n = w.decl_nodes.len();
        let (files, decl_file) = match (w.files, w.decl_file) {
            (None, None) => (Vec::new(), Vec::new()),
            (Some(files), Some(ids))
                if ids.len() == n
                    && ids
                        .iter()
                        .all(|&id| id == u32::MAX || (id as usize) < files.len()) =>
            {
                (files, ids)
            }
            _ => return Err(IrDecodeError::InvalidSourceProvenance),
        };
        if w.decl_to_node.len() != n || w.decl_to_edge.len() != n || w.decl_to_arc.len() != n {
            return Err(IrDecodeError::InvalidMapLength {
                decl_nodes: n,
                node: w.decl_to_node.len(),
                edge: w.decl_to_edge.len(),
                arc: w.decl_to_arc.len(),
            });
        }
        let mut decl_to_record = vec![dispatch::NONE; n];
        for d in 0..n {
            let did = DeclId::new(d);
            let (nn, ee, aa) = (w.decl_to_node[d], w.decl_to_edge[d], w.decl_to_arc[d]);
            let count =
                usize::from(nn.is_some()) + usize::from(ee.is_some()) + usize::from(aa.is_some());
            if count > 1 {
                return Err(IrDecodeError::MultipleBindings { decl: did });
            }
            let kind = w.decl_nodes[d].kind;
            let bound = match (kind, nn, ee, aa) {
                (DeclKind::Node, Some(nid), None, None) => {
                    if nid.raw() >= w.nodes.len() {
                        return Err(IrDecodeError::RecordOutOfRange { decl: did });
                    }
                    if w.nodes[nid.raw()].decl != did {
                        return Err(IrDecodeError::WrongBacklink { decl: did });
                    }
                    Some((DeclKind::Node, nid.raw()))
                }
                (DeclKind::Edge, None, Some(eid), None) => {
                    if eid.raw() >= w.edges.len() {
                        return Err(IrDecodeError::RecordOutOfRange { decl: did });
                    }
                    if w.edges[eid.raw()].decl != did {
                        return Err(IrDecodeError::WrongBacklink { decl: did });
                    }
                    Some((DeclKind::Edge, eid.raw()))
                }
                (DeclKind::HyperArc, None, None, Some(aid)) => {
                    if aid.raw() >= w.arcs.len() {
                        return Err(IrDecodeError::RecordOutOfRange { decl: did });
                    }
                    // ArcRec carries no `decl` back-pointer; range is the invariant here.
                    Some((DeclKind::HyperArc, aid.raw()))
                }
                (_, None, None, None) => None, // permitted partial/malformed test states
                _ => return Err(IrDecodeError::KindMismatch { decl: did, kind }),
            };
            if let Some((k, idx)) = bound {
                decl_to_record[d] =
                    dispatch::encode(k, idx).map_err(IrDecodeError::DispatchEncode)?;
            }
        }
        let namespace_roots = match w.namespace_roots {
            None => Vec::new(),
            Some(nw) => {
                if nw.schema != NAMESPACE_SCHEMA {
                    return Err(IrDecodeError::UnknownNamespaceSchema { schema: nw.schema });
                }
                let mut out: Vec<(DeclId, Box<[SymId]>)> = Vec::with_capacity(nw.roots.len());
                let mut prev: Option<usize> = None;
                for (root, prefix) in nw.roots {
                    if root.raw() >= n {
                        return Err(IrDecodeError::NamespaceRootOutOfRange { decl: root });
                    }
                    if prefix.is_empty() {
                        return Err(IrDecodeError::EmptyNamespacePrefix { decl: root });
                    }
                    match prev {
                        Some(pp) if root.raw() == pp => {
                            return Err(IrDecodeError::DuplicateNamespaceRoot { decl: root });
                        }
                        Some(pp) if root.raw() < pp => {
                            return Err(IrDecodeError::UnsortedNamespaceRoots);
                        }
                        _ => {}
                    }
                    prev = Some(root.raw());
                    out.push((root, prefix.into_boxed_slice()));
                }
                out
            }
        };
        let mut ir = Ir {
            files: files.into(),
            decl_file: decl_file.into(),
            meta: w.meta,
            decl_nodes: IrTable::default(),
            nodes: IrTable::default(),
            edges: IrTable::default(),
            arcs: IrTable::default(),
            annotations: IrTable::default(),
            base_lists: IrTable::default(),
            ref_weights: IrTable::default(),
            decl_to_record: decl_to_record.into(),
            namespace_roots: namespace_roots.into(),
            // Fail-closed: a facet table that disagrees with the declaration count is a malformed
            // payload, not something to pad silently. An ABSENT table (the pre-facet wire) is the
            // legitimate empty case and stays empty.
            decl_model: {
                let m = w.decl_model;
                if !m.is_empty() && m.len() != n {
                    return Err(IrDecodeError::InvalidMapLength {
                        decl_nodes: n,
                        node: m.len(),
                        edge: m.len(),
                        arc: m.len(),
                    });
                }
                m.into()
            },
        };
        for node in w.decl_nodes {
            let node = node.map_annotation(|anno| ir.store_annotation(anno));
            ir.decl_nodes.push(node);
        }
        let nodes: Vec<_> = w
            .nodes
            .into_iter()
            .map(|node| node.store(&mut ir))
            .collect();
        ir.nodes = nodes.into();
        let edges: Vec<_> = w
            .edges
            .into_iter()
            .map(|edge| edge.store(&mut ir))
            .collect();
        ir.edges = edges.into();
        let arcs: Vec<_> = w.arcs.into_iter().map(|arc| arc.store(&mut ir)).collect();
        ir.arcs = arcs.into();
        Ok(ir)
    }
}

impl<'de> Deserialize<'de> for Ir {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = IrWire::deserialize(d)?;
        Ir::try_from_wire(wire).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod compaction_coverage {
    use super::*;
    use crate::common::ids::{DeclId, NodeId, SymId};

    struct PublicationCase {
        indexed: bool,
        streamed: bool,
    }
    impl PublicationCase {
        fn build(&self) -> Ir {
            let mut ir = Ir::default();
            ir.files.push("construction.hymeko".into());
            let anno = AnnoR {
                tags: vec![SymId::new(7)],
                value: Some(ValueR::Num(3.5)),
            };
            let mut construction = IrConstruction::with_capacities(
                &mut ir,
                IrCapacities {
                    declarations: 3,
                    nodes: 1,
                    edges: 1,
                    arcs: 1,
                },
            );
            if self.indexed {
                construction.prepare_named_slots(2).unwrap();
            }
            for (raw, kind) in [DeclKind::Node, DeclKind::Edge].into_iter().enumerate() {
                let mut node = DeclNode::root(kind, SymId::new(raw), anno.clone());
                if raw == 1 {
                    node.parent = DeclId::new(0);
                }
                let namespace = if raw == 0 {
                    &[SymId::new(9)][..]
                } else {
                    &[][..]
                };
                if self.indexed {
                    construction
                        .named(DeclId::new(raw), node, vec![], namespace)
                        .unwrap();
                } else {
                    assert_eq!(
                        construction
                            .append_named(node, vec![], namespace)
                            .unwrap()
                            .raw(),
                        raw
                    );
                }
                if raw == 0 {
                    let declarations = construction.ir.decl_count();
                    construction.ir.decl_model.resize(declarations, None);
                }
            }
            let refs: Vec<_> = [
                None,
                Some(vec![]),
                Some(vec![ValueR::Num(-0.0), ValueR::Num(2.0)]),
            ]
            .into_iter()
            .map(|weights| {
                let mut atom = RefAtomR::plain(DeclId::new(0)).with_weights(weights);
                atom.anno = anno.clone();
                atom.direction = PortDirection::In;
                ReferenceDraft::new(IncidenceSign::Minus, atom)
            })
            .collect();
            let placement = ArcPlacement::under_owner(DeclId::new(1));
            if self.streamed {
                let mut refs = refs.into_iter();
                construction
                    .append_arc_iter(placement, anno, std::iter::from_fn(|| refs.next()))
                    .unwrap();
            } else {
                construction.append_arc(placement, anno, refs).unwrap();
            }
            ir
        }
    }

    #[test]
    fn final_publication_matches_indexed_slots_and_streamed_wire_payloads() {
        let expected = PublicationCase {
            indexed: true,
            streamed: false,
        }
        .build();
        let wire = serde_json::to_vec(&expected).unwrap();
        for indexed in [false, true] {
            for streamed in [false, true] {
                let ir = PublicationCase { indexed, streamed }.build();
                assert_eq!(serde_json::to_vec(&ir).unwrap(), wire);
                assert_eq!(ir.decl_file.len(), 3);
                assert_eq!(ir.decl_model.len(), 3);
                assert_eq!(ir.arcs[0].refs.len(), 3);
                assert_eq!(ir.arcs[0].refs.capacity(), 3);
                assert_eq!(
                    ir.child_decls(DeclId::new(0)).collect::<Vec<_>>(),
                    vec![DeclId::new(1)]
                );
                assert_eq!(
                    ir.child_decls(DeclId::new(1)).collect::<Vec<_>>(),
                    vec![DeclId::new(2)]
                );
                let published = ir.into_shared();
                let before = serde_json::to_vec(&published).unwrap();
                let mut next = published.clone();
                let mut child = DeclNode::root(DeclKind::Node, SymId::new(10), AnnoR::default());
                child.parent = DeclId::new(0);
                IrConstruction::new(&mut next)
                    .append_named(child, Vec::with_capacity(8), &[])
                    .unwrap();
                assert!(next.nodes.last().unwrap().bases.is_none());
                assert_eq!(serde_json::to_vec(&published).unwrap(), before);
                assert_eq!(next.decl_count(), 4);
            }
        }
    }

    #[test]
    fn arc_admission_rejects_faults_before_consuming_stream_or_mutating_ir() {
        use std::cell::Cell;
        for fault in 0..3 {
            let mut ir = PublicationCase {
                indexed: false,
                streamed: true,
            }
            .build();
            let mut placement = ArcPlacement::under_owner(DeclId::new(1));
            match fault {
                0 => placement.owner = DeclId::NONE,
                1 => placement.parent = DeclId::new(999),
                _ => ir.decl_to_record.push(dispatch::NONE),
            }
            let before = format!("{ir:?}");
            let consumed = Cell::new(0);
            let refs = std::iter::once_with(|| {
                consumed.set(consumed.get() + 1);
                ReferenceDraft::plain(DeclId::new(0), IncidenceSign::Plus)
            });
            assert!(
                IrConstruction::new(&mut ir)
                    .append_arc_iter(placement, AnnoR::default(), refs)
                    .is_err()
            );
            assert_eq!(consumed.get(), 0);
            assert_eq!(format!("{ir:?}"), before);
            assert!(
                IrConstruction::new(&mut ir)
                    .append_arc(placement, AnnoR::default(), vec![])
                    .is_err()
            );
            assert_eq!(format!("{ir:?}"), before);
        }
    }

    #[test]
    fn validated_parent_cursor_preserves_mixed_order_and_rechecks_new_scopes() {
        let mut ir = Ir::default();
        let mut expected = [Vec::new(), Vec::new(), Vec::new()];
        {
            let mut build = IrConstruction::new(&mut ir);
            let a = build
                .append_node(SymId::new(0), AnnoR::default(), vec![])
                .unwrap();
            let b = build
                .append_node(SymId::new(1), AnnoR::default(), vec![])
                .unwrap();
            let mut edge = DeclNode::root(DeclKind::Edge, SymId::new(2), AnnoR::default());
            edge.parent = a;
            let owner = build.append_named(edge, vec![], &[]).unwrap();
            expected[0].push(owner);
            for (step, parent) in [a, a, b, a, owner, owner, b, a].into_iter().enumerate() {
                let mut node =
                    DeclNode::root(DeclKind::Node, SymId::new(step + 3), AnnoR::default());
                node.parent = parent;
                let child = build.append_named(node, vec![], &[]).unwrap();
                expected[parent.raw()].push(child);
                assert_eq!(build.parent.map(|p| (p.id, p.last)), Some((parent, child)));
                let before = format!("{:?}", build.ir);
                let mut bad = DeclNode::root(DeclKind::HyperArc, SymId::new(99), AnnoR::default());
                bad.parent = parent;
                assert!(build.append_named(bad, vec![], &[]).is_err());
                assert_eq!(format!("{:?}", build.ir), before);
            }
            let arc = build
                .append_arc_iter(
                    ArcPlacement::under_owner(owner),
                    AnnoR::default(),
                    std::iter::empty(),
                )
                .unwrap();
            expected[owner.raw()].push(arc);
        }
        for (parent, children) in expected.iter().enumerate() {
            assert_eq!(
                ir.child_decls(DeclId::new(parent)).collect::<Vec<_>>(),
                *children
            );
        }
        let last = ir.decl_nodes[0].last_child;
        ir.decl_nodes[last.raw()].next_sibling = DeclId::new(0);
        let before = format!("{ir:?}");
        let mut node = DeclNode::root(DeclKind::Node, SymId::new(100), AnnoR::default());
        node.parent = DeclId::new(0);
        assert!(
            IrConstruction::new(&mut ir)
                .append_named(node, vec![], &[])
                .is_err()
        );
        assert_eq!(format!("{ir:?}"), before);
    }

    struct ConstructionCase;
    impl IrBuilder for ConstructionCase {
        type Output = Ir;
        type Error = dispatch::DispatchError;
        fn capacities(&self) -> IrCapacities {
            IrCapacities {
                declarations: 64,
                nodes: 32,
                edges: 8,
                arcs: 8,
            }
        }
        fn populate(&mut self, construction: &mut IrConstruction<'_>) -> Result<(), Self::Error> {
            assert_eq!(construction.ir.decl_nodes.capacity(), 0);
            let root = construction.append_node(SymId::new(0), AnnoR::default(), vec![])?;
            let mut node = DeclNode::root(DeclKind::Node, SymId::new(1), AnnoR::default());
            node.parent = root;
            let child = construction.append_named(node, vec![], &[])?;
            let mut edge = DeclNode::root(DeclKind::Edge, SymId::new(2), AnnoR::default());
            edge.parent = root;
            let edge = construction.append_named(edge, vec![], &[])?;
            construction.append_arc(
                ArcPlacement::under_owner(edge),
                AnnoR::default(),
                vec![ReferenceDraft::plain(child, IncidenceSign::Plus)],
            )?;
            Ok(())
        }
        fn finish(self, ir: Ir) -> Ir {
            ir
        }
    }

    #[test]
    fn builder_owns_population_capacity_hierarchy_and_bindings() {
        let ir = ConstructionCase.build().unwrap();
        assert_eq!(
            (
                ir.decl_count(),
                ir.nodes.len(),
                ir.edges.len(),
                ir.arcs.len()
            ),
            (4, 2, 1, 1)
        );
        assert!(ir.decl_nodes.capacity() >= 64);
        assert!(ir.nodes.capacity() >= 32);
        assert!(ir.edges.capacity() >= 8);
        assert!(ir.arcs.capacity() >= 8);
        assert_eq!(
            ir.decl_children(DeclId::new(0)).collect::<Vec<_>>(),
            vec![DeclId::new(1), DeclId::new(2)]
        );
        assert_eq!(
            ir.decl_children(DeclId::new(2)).collect::<Vec<_>>(),
            vec![DeclId::new(3)]
        );
        assert_eq!(ir.node_for_decl(DeclId::new(1)).unwrap().0, NodeId::new(1));
        assert_eq!(ir.arcs[0].refs[0].target(), DeclId::new(1));
    }

    struct CountedInput<'a> {
        calls: &'a std::cell::Cell<usize>,
        fail: bool,
    }
    impl IrBuilder for CountedInput<'_> {
        type Output = Ir;
        type Error = &'static str;
        fn capacities(&self) -> IrCapacities {
            ConstructionCase.capacities()
        }
        fn populate(&mut self, construction: &mut IrConstruction<'_>) -> Result<(), Self::Error> {
            self.calls.set(self.calls.get() + 1);
            ConstructionCase
                .populate(construction)
                .map_err(|_| "construction")?;
            if self.fail {
                Err("input failure")
            } else {
                Ok(())
            }
        }
        fn finish(self, ir: Ir) -> Ir {
            ir
        }
    }

    #[test]
    fn lazy_input_loads_once_and_caches_success_or_failure() {
        for fail in [false, true] {
            let calls = std::cell::Cell::new(0);
            {
                let unloaded = CountedInput {
                    calls: &calls,
                    fail,
                }
                .defer();
                assert!(!unloaded.is_loaded());
                assert_eq!(unloaded.capacities().declarations, 64);
            }
            assert_eq!(
                calls.get(),
                0,
                "dropping an unloaded input must not populate"
            );
            let mut lazy = CountedInput {
                calls: &calls,
                fail,
            }
            .defer();
            assert_eq!(calls.get(), 0, "capacity planning must not populate");
            for _ in 0..3 {
                match lazy.load() {
                    Ok(ir) => {
                        assert!(!fail);
                        assert_eq!(
                            serde_json::to_value(ir).unwrap(),
                            serde_json::to_value(ConstructionCase.build().unwrap()).unwrap()
                        );
                    }
                    Err(error) => {
                        assert!(fail);
                        assert_eq!(*error, "input failure");
                    }
                }
                assert!(lazy.is_loaded());
                assert_eq!(calls.get(), 1, "a retained outcome must never repopulate");
            }
        }
    }

    #[test]
    fn append_named_rejects_invalid_input_before_allocating_or_writing() {
        let mut ir = Ir::default();
        let mut construction = IrConstruction::with_capacities(
            &mut ir,
            IrCapacities {
                declarations: 64,
                ..Default::default()
            },
        );
        let mut invalid = DeclNode::root(DeclKind::Node, SymId::new(0), AnnoR::default());
        invalid.parent = DeclId::new(17);
        assert!(construction.append_named(invalid, vec![], &[]).is_err());
        assert_eq!(ir.decl_nodes.capacity(), 0);
        assert_eq!(ir.decl_to_record.capacity(), 0);
        assert_eq!(ir.decl_count(), 0);
        let mut ir = ConstructionCase.build().unwrap();
        let before = serde_json::to_vec(&ir).unwrap();
        assert!(IrConstruction::new(&mut ir).prepare_named_slots(1).is_err());
        assert_eq!(serde_json::to_vec(&ir).unwrap(), before);
    }

    #[test]
    fn canonical_reference_layout_is_compact() {
        assert!(
            std::mem::size_of::<SignedRefR>() <= 32,
            "canonical SignedRefR occupies {} bytes; payloads must live in IR-owned tables",
            std::mem::size_of::<SignedRefR>()
        );
    }

    #[test]
    fn value_bitwise_equality_distinguishes_all_representations_iteratively() {
        let values = [
            ValueR::Num(0.0),
            ValueR::Num(-0.0),
            ValueR::Num(1.0),
            ValueR::Num(f64::from_bits(0x7ff8_0000_0000_0001)),
            ValueR::Num(f64::from_bits(0x7ff8_0000_0000_0002)),
            ValueR::Str(SymId::new(0)),
            ValueR::Str(SymId::new(1)),
            ValueR::Ref(DeclId::new(0)),
            ValueR::Ref(DeclId::new(1)),
            ValueR::List(vec![]),
            ValueR::List(vec![ValueR::Num(0.0)]),
            ValueR::List(vec![ValueR::Num(-0.0)]),
            ValueR::List(vec![ValueR::List(vec![])]),
            ValueR::List(vec![ValueR::List(vec![ValueR::Num(1.0)])]),
            ValueR::List(vec![ValueR::List(vec![ValueR::Num(1.0)]), ValueR::Num(2.0)]),
            ValueR::List(vec![ValueR::List(vec![ValueR::Num(1.0)]), ValueR::Num(3.0)]),
        ];
        for (i, left) in values.iter().enumerate() {
            for (j, right) in values.iter().enumerate() {
                assert_eq!(left.bitwise_eq(right), i == j, "representations {i}, {j}");
            }
        }
        let mut deep = [ValueR::Num(0.0), ValueR::Num(0.0), ValueR::Num(-0.0)];
        for value in &mut deep {
            for _ in 0..1024 {
                *value = ValueR::List(vec![std::mem::replace(value, ValueR::Num(0.0))]);
            }
        }
        assert!(deep[0].bitwise_eq(&deep[1]));
        assert!(!deep[0].bitwise_eq(&deep[2]));
        // Tear down iteratively too: this exercises comparison, not recursive Drop.
        for mut value in deep {
            while let ValueR::List(mut children) = value {
                value = children.pop().unwrap();
            }
        }
    }

    #[test]
    fn record_constructors_preserve_the_explicit_wire_shape() {
        let anno = AnnoR {
            tags: vec![SymId::new(7)],
            value: Some(ValueR::Num(-0.0)),
        };
        for kind in [DeclKind::Node, DeclKind::Edge, DeclKind::HyperArc] {
            // Independent explicit record: keep this literal as the constructor oracle.
            let expected = DeclNode {
                kind,
                name: SymId::new(3),
                parent: DeclId::NONE,
                first_child: DeclId::NONE,
                last_child: DeclId::NONE,
                next_sibling: DeclId::NONE,
                anno: anno.clone(),
            };
            let actual = DeclNode::root(kind, SymId::new(3), anno.clone());
            assert_eq!(
                serde_json::to_value(expected).unwrap(),
                serde_json::to_value(actual).unwrap()
            );
        }
        let atom = RefAtomR {
            target: DeclId::new(19),
            anno,
            weights: None,
            direction: PortDirection::In,
        };
        let weighted = atom.clone().with_weights(Some(vec![]));
        assert_eq!(weighted.target, atom.target);
        assert_eq!(weighted.anno, atom.anno);
        assert_eq!(weighted.direction, PortDirection::In);
        assert_eq!(weighted.weights, Some(vec![]));
        assert_eq!(weighted.with_weights(None), atom);
        let mut owner = Ir::default();
        let refs = vec![owner.store_reference(ReferenceDraft::new(IncidenceSign::Minus, atom))];
        let expected = ArcRec {
            in_edge: DeclId::NONE,
            anno: crate::common::ids::AnnotationId::NONE,
            refs: refs.clone(),
        };
        assert_eq!(
            serde_json::to_value(reference_wire::Record {
                ir: &owner,
                value: &ArcRec::unowned(refs)
            })
            .unwrap(),
            serde_json::to_value(reference_wire::Record {
                ir: &owner,
                value: &expected
            })
            .unwrap()
        );
    }

    #[test]
    fn root_construction_preserves_dispatch_domains_and_side_tables() {
        let mut ir = Ir::default();
        ir.files.push("source.hymeko".to_owned());
        let base = IrConstruction::new(&mut ir)
            .append_node(SymId::new(0), AnnoR::default(), vec![])
            .unwrap();
        ir.decl_model.resize(1, None);
        let bases = vec![ReferenceDraft::plain(base, IncidenceSign::Plus)];
        let anno = AnnoR {
            tags: vec![SymId::new(8)],
            value: Some(ValueR::Num(3.0)),
        };
        let edge = IrConstruction::new(&mut ir)
            .append_edge(SymId::new(2), anno.clone(), bases.clone())
            .unwrap();
        let node = IrConstruction::new(&mut ir)
            .append_node(SymId::new(4), anno.clone(), bases.clone())
            .unwrap();
        assert_eq!((base.raw(), edge.raw(), node.raw()), (0, 1, 2));
        assert_eq!(ir.as_node(node), Some(NodeId::new(1)));
        assert_eq!(ir.as_edge(edge), Some(EdgeId::new(0)));
        let expected = vec![SignedRefR::plain(base, IncidenceSign::Plus)];
        assert_eq!(&ir[ir.nodes[1].bases], expected);
        assert_eq!(&ir[ir.edges[0].bases], expected);
        assert_eq!(ir[ir.decl_nodes[node.raw()].anno], anno);
        assert_eq!(&*ir.decl_file, &[u32::MAX; 3]);
        assert_eq!(ir.decl_model.len(), 3);
        assert!(ir.decl_nodes.iter().all(|n| n.parent.is_none()
            && n.first_child.is_none()
            && n.last_child.is_none()
            && n.next_sibling.is_none()));
        let declarations = ir.decl_count();
        ir.arcs.push(ArcRec::unowned(vec![]));
        assert_eq!(
            ir.decl_count(),
            declarations,
            "unowned arc has no invented declaration"
        );
        let anonymous = ir.push_anonymous_arc();
        assert_eq!(anonymous.raw(), declarations);
        assert_eq!(ir.decl_nodes[anonymous.raw()].kind, DeclKind::HyperArc);
        assert!(ir.as_arc(anonymous).is_none());
        assert_eq!(ir.decl_model.len(), declarations + 1);
        assert_eq!(ir.decl_file.len(), declarations + 1);
    }

    #[test]
    fn root_construction_rejects_broken_parallel_storage_without_mutation() {
        let mut ir = one_node_ir();
        ir.decl_to_record.clear();
        let before = format!("{ir:?}");
        let error = IrConstruction::new(&mut ir)
            .append_node(SymId::new(1), AnnoR::default(), vec![])
            .unwrap_err();
        assert_eq!(
            error,
            dispatch::DispatchError::InvalidDeclaration {
                decl: DeclId::new(1)
            }
        );
        assert_eq!(format!("{ir:?}"), before);
    }

    fn one_node_ir() -> Ir {
        let mut ir = Ir::default();
        let d = ir.push_decl(DeclNode::root(
            DeclKind::Node,
            SymId::new(0),
            AnnoR::default(),
        ));
        ir.nodes
            .push(NodeRec::new(d, crate::common::ids::BaseListId::NONE));
        ir.bind_node_decl(d, NodeId::new(0)).unwrap();
        ir
    }

    #[test]
    fn checked_binding_rejects_faults_without_changing_state() {
        let mut ir = one_node_ir();
        let d = DeclId::new(0);
        ir.nodes
            .push(NodeRec::new(d, crate::common::ids::BaseListId::NONE));
        let before = serde_json::to_vec(&ir).unwrap();
        for error in [
            ir.bind_node_decl(DeclId::NONE, NodeId::new(0)),
            ir.bind_edge_decl(d, EdgeId::new(0)),
            ir.bind_node_decl(d, NodeId::new(2)),
            ir.bind_node_decl(d, NodeId::new(1)),
        ] {
            assert!(error.is_err());
        }
        ir.bind_node_decl(d, NodeId::new(0)).unwrap(); // idempotence retains caches
        assert_eq!(serde_json::to_vec(&ir).unwrap(), before);
        ir.nodes[1].decl = DeclId::new(99);
        assert_eq!(
            ir.bind_node_decl(d, NodeId::new(1)),
            Err(dispatch::DispatchError::WrongBacklink { decl: d })
        );
    }

    #[test]
    fn construction_commits_links_records_namespaces_and_invalidates_caches() {
        let mut ir = Ir::default();
        ir.preallocate_from_index(2);
        let root = DeclId::new(0);
        let edge = DeclId::new(1);
        let mut node = ir.decl_nodes[0].map_annotation(|id| ir[id].clone());
        node.name = SymId::new(1);
        let mut construction = IrConstruction::new(&mut ir);
        construction
            .named(root, node.clone(), vec![], &[SymId::new(0)])
            .unwrap();
        node.parent = root;
        node.kind = DeclKind::Edge;
        node.name = SymId::new(2);
        construction.named(edge, node, vec![], &[]).unwrap();
        let anno = AnnoR {
            tags: vec![],
            value: Some(ValueR::Num(4.0)),
        };
        construction
            .append_arc(
                ArcPlacement::under_owner(edge),
                anno.clone(),
                vec![ReferenceDraft::plain(root, IncidenceSign::Plus)],
            )
            .unwrap();
        assert_eq!(ir.child_decls(root).collect::<Vec<_>>(), vec![edge]);
        let arc = ir.child_decls(edge).next().unwrap();
        let (aid, record) = ir.arc_for_decl(arc).unwrap();
        assert_eq!(record.in_edge, edge);
        assert_eq!(ir.edges[0].arcs, vec![aid]);
        assert_eq!(ir[record.anno], *ir.decl_annotation(arc).unwrap());
        assert_eq!(ir.namespace_prefix(root), Some([SymId::new(0)].as_slice()));
    }

    #[test]
    fn rejected_construction_and_leaf_updates_do_not_damage_existing_ir() {
        let mut ir = one_node_ir();
        ir.preallocate_from_index(2);
        let mut node = ir.decl_nodes[1].map_annotation(|id| ir[id].clone());
        node.parent = DeclId::new(99);
        let before = serde_json::to_vec(&ir).unwrap();
        let mut construction = IrConstruction::new(&mut ir);
        assert!(
            construction
                .named(DeclId::new(1), node.clone(), vec![], &[])
                .is_err()
        );
        node.parent = DeclId::NONE;
        assert!(
            construction
                .named(DeclId::new(0), node, vec![], &[])
                .is_err()
        );
        assert!(
            construction
                .append_arc(
                    ArcPlacement::under_owner(DeclId::new(0)),
                    AnnoR::default(),
                    vec![]
                )
                .is_err()
        );
        assert_eq!(serde_json::to_vec(&ir).unwrap(), before);
        let mut values = vec![
            LeafValueEdit::Replace {
                declaration: DeclId::new(0),
                value: Some(ValueR::Num(2.0)),
            },
            LeafValueEdit::Keep(DeclId::new(1)),
        ];
        let (allocation, capacity) = (values.as_ptr(), values.capacity());
        let payload = values.clone();
        assert!(ir.set_leaf_values(&mut values).is_err());
        assert_eq!(values, payload);
        assert_eq!(serde_json::to_vec(&ir).unwrap(), before);
        values.pop();
        ir.set_leaf_values(&mut values).unwrap();
        assert!(values.is_empty());
        assert_eq!((values.as_ptr(), values.capacity()), (allocation, capacity));
        assert_eq!(
            ir.decl_annotation(crate::common::ids::DeclId::new(0))
                .unwrap()
                .value,
            Some(ValueR::Num(2.0))
        );
    }

    #[test]
    fn every_ir_decode_error_formats_nonempty() {
        let errs = [
            IrDecodeError::InvalidMapLength {
                decl_nodes: 3,
                node: 2,
                edge: 3,
                arc: 3,
            },
            IrDecodeError::MultipleBindings {
                decl: DeclId::new(1),
            },
            IrDecodeError::KindMismatch {
                decl: DeclId::new(1),
                kind: DeclKind::Node,
            },
            IrDecodeError::RecordOutOfRange {
                decl: DeclId::new(1),
            },
            IrDecodeError::WrongBacklink {
                decl: DeclId::new(1),
            },
            IrDecodeError::DispatchEncode(dispatch::DispatchError::IndexOverflow { index: 1 }),
        ];
        for e in errs {
            assert!(!e.to_string().is_empty());
            assert!(!format!("{e:?}").is_empty());
        }
    }

    #[test]
    fn a_reserved_dispatch_slot_is_rejected_by_coherence_and_serialize() {
        let mut ir = one_node_ir();
        // A reserved-tag slot is unreachable through the public mutators; inject
        // it directly to exercise the fail-closed paths.
        ir.decl_to_record[0] = (dispatch::TAG_RESERVED << 62) | 1;

        // Coherence flags it rather than silently accepting.
        let idx = crate::resolution::resolve::Index::new();
        let it = crate::resolution::interner::Interner::new();
        let report = crate::ir::coherence::validate_hierarchy_index_coherence(
            &ir,
            &idx,
            &it,
            crate::ir::coherence::CoherenceLimits::default(),
        )
        .expect_err("reserved slot must be reported");
        assert!(
            report.findings.iter().any(|f| matches!(
                f,
                crate::ir::coherence::Finding::ReservedDispatchSlot { .. }
            )),
            "{:?}",
            report.findings
        );

        // Serialization is fail-closed: the reserved slot cannot reach the wire.
        assert!(serde_json::to_string(&ir).is_err());
    }
}

#[cfg(test)]
mod shrink_coverage {
    use super::*;

    /// `shrink_to_fit` leaves every table, the crate-private ones included, at capacity equal to
    /// length, and changes no element (ir-shrink-after-lowering).
    #[test]
    fn a_shrunk_ir_keeps_its_elements_and_no_spare_capacity() {
        let mut ir = Ir::default();
        ir.preallocate_from_index(3);
        for _ in 0..5 {
            ir.push_anonymous_arc();
        }
        // Slack in public and crate-private tables alike, as a growing lowering leaves it.
        ir.decl_nodes.reserve(32);
        ir.decl_to_record.reserve(32);
        ir.arcs.reserve(16);
        ir.files.reserve(16);
        let before = serde_json::to_string(&ir).expect("serializes");
        assert!(ir.decl_nodes.capacity() > ir.decl_nodes.len());
        assert!(ir.decl_to_record.capacity() > ir.decl_to_record.len());
        ir.shrink_to_fit();
        assert_eq!(serde_json::to_string(&ir).expect("serializes"), before);
        macro_rules! capacities {
            ($ir:expr; $($table:ident),*) => {
                [$( (stringify!($table), $ir.$table.capacity(), $ir.$table.len()) ),*]
            };
        }
        for (table, capacity, len) in ir_tables!(capacities!(ir)) {
            assert_eq!(capacity, len, "{table}");
        }
    }
}
