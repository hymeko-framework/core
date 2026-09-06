use crate::common::ids::{DeclId, EdgeId, HyperArcId, NodeId, SymId};
use crate::ir::hash::HashId;
use crate::ir::meta::Meta;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclKind {
    Node,
    Edge,
    HyperArc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclNode {
    pub kind: DeclKind,
    pub name: SymId,
    pub parent: DeclId,
    pub first_child: DeclId,
    pub last_child: DeclId,
    pub next_sibling: DeclId,
    pub anno: AnnoR,
}

#[derive(Debug, Default, Clone)]
pub struct Ir {
    pub meta: Option<Meta>,
    pub doc_hash: Option<HashId>,
    pub decl_hash: Vec<Option<HashId>>,
    pub arc_hash: Vec<Option<HashId>>,
    // Unified Arena
    pub decl_nodes: Vec<DeclNode>,

    // “konkrét” táblák
    pub nodes: Vec<NodeRec>, // NodeId indexel
    pub edges: Vec<EdgeRec>, // EdgeId indexel
    pub arcs: Vec<ArcRec>,   // ArcId indexel

    // Compact declaration dispatch (option D, 2026-08-31): one 8-byte slot per
    // decl replaces the former three `Vec<Option<Id>>` maps (48 B/decl -> 8 B/decl).
    // Storage is PRIVATE; the public read contract is the accessor API
    // (`as_node`/`as_edge`/`as_arc`, `node_for_decl`/..., `contains_*`); writes go
    // through the crate-private typed mutators `bind_{node,edge,arc}_decl`. See the
    // `dispatch` module for the checked encoding. The legacy three-map serde wire is
    // preserved byte-for-byte by the manual `Serialize`/`Deserialize` impls below.
    pub(crate) decl_to_record: Vec<u64>,

    // Canonical namespace roots (U2.1, 2026-08-31): the import namespace prefix
    // lowering applies to each imported module's root declarations -- the one piece
    // of the hierarchy the intrusive links do not encode (an imported root has
    // `parent == NONE`), and which the v1 canonical hash embeds via the path.
    // Stored so the `Ir` is a complete post-lowering authority.
    //
    // Invariant: sorted ascending by `DeclId`; each `DeclId` is a live link-root;
    // prefixes are non-empty (single-module roots absent -> zero cost). Storage is
    // PRIVATE; reads via `namespace_prefix`, writes via `record_namespace_root`.
    pub(crate) namespace_roots: Vec<(DeclId, Box<[SymId]>)>,

    // APPROVED-CORE-EDIT: executable-model-contract (2026-09-06).
    //
    // The OPTIONAL executable-model facet, one slot per declaration, keyed by `DeclId` exactly as
    // `decl_hash` and `arc_hash` are. It is a side table rather than a `DeclNode` field for two
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
    pub(crate) decl_model: Vec<Option<ModelFacet>>,
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
    use super::DeclKind;

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
        /// A record index does not fit the 62-bit dispatch index domain.
        IndexOverflow { index: usize },
        /// A slot carries the reserved (invalid) tag `3`.
        ReservedTag { slot: u64 },
        /// A non-NONE slot whose index field is `0` (i.e. `raw+1 == 0`): malformed.
        MalformedZeroIndex { slot: u64 },
        /// A slot's decoded `u64` index did not fit `usize` on this target.
        UsizeOverflow { value: u64 },
    }

    impl core::fmt::Display for DispatchError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
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

impl Ir {
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
        self.decl_nodes.get(id.raw())
    }

    #[inline]
    pub fn decl_node_unchecked(&self, id: DeclId) -> &DeclNode {
        assert!(id.is_some(), "attempted to dereference DeclId::NONE");
        &self.decl_nodes[id.0]
    }

    pub fn decl_children(&self, parent: DeclId) -> DeclChildren<'_> {
        let next = if parent.is_none() {
            DeclId::NONE
        } else {
            self.decl_nodes[parent.0].first_child
        };
        DeclChildren { ir: self, next }
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
    pub fn push_decl(&mut self, node: DeclNode) -> DeclId {
        let id = DeclId::new(self.decl_nodes.len());
        self.decl_nodes.push(node);
        self.decl_to_record.push(dispatch::NONE);
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

    /// Crate-private typed binding: `decl (kind Node) -> node record`. Checks the
    /// declaration kind, checks the index is representable (checked encode), and
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
        let slot = dispatch::encode(kind, index)?; // checked; no partial state on Err
        let cur = self.decl_to_record[d.raw()];
        debug_assert!(
            cur == dispatch::NONE || cur == slot,
            "conflicting re-binding of decl {} : {cur:#x} -> {slot:#x}",
            d.raw()
        );
        self.decl_to_record[d.raw()] = slot;
        Ok(())
    }

    pub fn preallocate_from_index(&mut self, total_named_decls: usize) {
        let default_node = DeclNode {
            kind: DeclKind::Node,
            name: SymId::new(0),
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        };
        // Allocate exact capacity for Nodes and Edges
        self.decl_nodes.resize(total_named_decls, default_node);
        self.decl_to_record
            .resize(total_named_decls, dispatch::NONE);
        self.decl_hash.resize(total_named_decls, None);
        if !self.decl_model.is_empty() {
            self.decl_model.resize(total_named_decls, None);
        }
    }

    pub fn push_anonymous_arc(&mut self) -> DeclId {
        let id = DeclId::new(self.decl_nodes.len());
        self.decl_nodes.push(DeclNode {
            kind: DeclKind::HyperArc,
            name: SymId::new(0),
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        self.decl_to_record.push(dispatch::NONE);
        self.decl_hash.push(None);
        if !self.decl_model.is_empty() {
            self.decl_model.push(None);
        }
        id
    }
}

pub struct DeclChildren<'a> {
    ir: &'a Ir,
    next: DeclId,
}

impl<'a> Iterator for DeclChildren<'a> {
    type Item = DeclId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next.is_none() {
            return None;
        }
        let cur = self.next;
        self.next = self.ir.next_sibling(cur);
        Some(cur)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRec {
    pub decl: DeclId,
    pub bases: Vec<SignedRefR>,
}
impl NodeRec {
    pub fn new(decl: DeclId, bases: Vec<SignedRefR>) -> Self {
        Self { decl, bases }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeRec {
    pub decl: DeclId,
    pub bases: Vec<SignedRefR>,
    pub arcs: Vec<HyperArcId>,
}
impl EdgeRec {
    pub fn new(decl: DeclId, bases: Vec<SignedRefR>) -> Self {
        Self {
            decl,
            bases,
            arcs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcRec {
    pub anno: AnnoR,
    pub in_edge: DeclId, // melyik edge scope-jában volt (általában EdgeDecl)
    pub refs: Vec<SignedRefR>, // DeclId-kra mutat (determinista)
                         // később: attribútumok/value külön, lásd lent
}

/// Which way data flows across a port, as an axis **independent of the algebraic sign**.
///
/// APPROVED-CORE-EDIT: executable-model-contract (2026-09-06).
///
/// `SignedRefR::{Plus, Minus, Neutral}` is the signed-hypergraph *algebra*: it says how an incidence
/// enters a balance or a cycle product. It is **not** direction, and direction is never derived from
/// it. A production incidence may legitimately be `Minus` and `In`, `Minus` and `Out`, `Plus` and
/// `In`, or `Plus` and `Out`; conflating the two axes is the error this type exists to make
/// impossible.
///
/// `Unspecified` is the only value a legacy incidence receives. Nothing invents a direction for an
/// edge that never declared one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PortDirection {
    /// No direction was declared. Every pre-2026-09-06 incidence is this.
    #[default]
    Unspecified,
    /// The referenced declaration is consumed by the operator.
    In,
    /// The referenced declaration is produced by the operator.
    Out,
}

impl PortDirection {
    /// Whether no direction was declared. Used by the serde skip so the legacy wire is unchanged.
    #[must_use]
    pub const fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefAtomR {
    pub target: DeclId,
    pub anno: AnnoR,
    pub weights: Option<Vec<ValueR>>,
    /// Data-flow direction, orthogonal to the sign.
    ///
    /// `Unspecified` is both the default and *skipped on the wire*, so a document that declares no
    /// direction — every pre-2026-09-06 document — serializes byte-identically to the pre-field
    /// form. A caught regression, not a design guess: emitting it unconditionally grew the wire for
    /// legacy payloads, which `a_pre_direction_payload_decodes_as_unspecified` detected.
    #[serde(default, skip_serializing_if = "PortDirection::is_unspecified")]
    pub direction: PortDirection,
}

impl RefAtomR {
    /// A reference with no annotation, no weights and no declared direction.
    ///
    /// The shape every pre-2026-09-06 construction site had; provided so those sites read as what
    /// they are rather than repeating three defaults.
    #[must_use]
    pub fn plain(target: DeclId) -> Self {
        Self {
            target,
            anno: AnnoR::default(),
            weights: None,
            direction: PortDirection::Unspecified,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SignedRefR {
    Plus(RefAtomR),
    Minus(RefAtomR),
    Neutral(RefAtomR),
}

impl SignedRefR {
    // Extract inner atom (RefAtomR) regardless of sign
    #[inline]
    pub fn atom(&self) -> &RefAtomR {
        match self {
            Self::Plus(r) | Self::Minus(r) | Self::Neutral(r) => r,
        }
    }

    // Target 'DeclId' of this reference
    #[inline]
    pub fn target(&self) -> DeclId {
        self.atom().target
    }

    // Sign as i8
    #[inline]
    pub fn sign(&self) -> i8 {
        match self {
            Self::Plus(_) => 1,
            Self::Minus(_) => -1,
            Self::Neutral(_) => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueR {
    Str(SymId),
    Num(f64),
    List(Vec<ValueR>),
    Ref(DeclId), // AST Ref(path) -> IR Ref(DeclId)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnnoR {
    pub tags: Vec<SymId>,
    pub value: Option<ValueR>,
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
        // Field order/names identical to the former derived form. With no namespace
        // metadata (single-module) the wire keeps exactly the legacy 11 fields ->
        // byte-identical to the pre-U2.1 form (and the frozen serde KAT).
        let has_ns = !self.namespace_roots.is_empty();
        // APPROVED-CORE-EDIT: executable-model-contract. Additive and CONDITIONAL, following the
        // `namespace_roots` precedent: a document with no facet emits the legacy field set, so its
        // bytes -- and the frozen serde KAT -- are unchanged.
        let has_model = self.decl_model.iter().any(Option::is_some);
        let fields = 11 + usize::from(has_ns) + usize::from(has_model);
        let mut st = s.serialize_struct("Ir", fields)?;
        st.serialize_field("meta", &self.meta)?;
        st.serialize_field("doc_hash", &self.doc_hash)?;
        st.serialize_field("decl_hash", &self.decl_hash)?;
        st.serialize_field("arc_hash", &self.arc_hash)?;
        st.serialize_field("decl_nodes", &self.decl_nodes)?;
        st.serialize_field("nodes", &self.nodes)?;
        st.serialize_field("edges", &self.edges)?;
        st.serialize_field("arcs", &self.arcs)?;
        st.serialize_field("decl_to_node", &d2n)?;
        st.serialize_field("decl_to_edge", &d2e)?;
        st.serialize_field("decl_to_arc", &d2a)?;
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
    meta: Option<Meta>,
    doc_hash: Option<HashId>,
    decl_hash: Vec<Option<HashId>>,
    arc_hash: Vec<Option<HashId>>,
    decl_nodes: Vec<DeclNode>,
    nodes: Vec<NodeRec>,
    edges: Vec<EdgeRec>,
    arcs: Vec<ArcRec>,
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
        Ok(Ir {
            meta: w.meta,
            doc_hash: w.doc_hash,
            decl_hash: w.decl_hash,
            arc_hash: w.arc_hash,
            decl_nodes: w.decl_nodes,
            nodes: w.nodes,
            edges: w.edges,
            arcs: w.arcs,
            decl_to_record,
            namespace_roots,
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
                m
            },
        })
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

    fn one_node_ir() -> Ir {
        let mut ir = Ir::default();
        let d = ir.push_decl(DeclNode {
            kind: DeclKind::Node,
            name: SymId::new(0),
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        ir.decl_hash.push(None);
        ir.nodes.push(NodeRec::new(d, Vec::new()));
        ir.bind_node_decl(d, NodeId::new(0)).unwrap();
        ir
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
