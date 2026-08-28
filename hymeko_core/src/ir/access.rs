// APPROVED-CORE-EDIT: unified-ir-m2a-accessor-api (2026-08-28).
//
// Additive only. No existing field, method or type is changed; `ir.rs` is not
// edited at all. Inherent `impl` blocks may live in any module of the defining
// crate, so the accessor surface lives here and `Ir` itself is untouched.

//! Storage-independent read accessors for [`Ir`].
//!
//! This is the encapsulation boundary M2b consumers migrate toward. Call sites
//! that go through this module do not observe that `Ir` is backed by `Vec`s
//! indexed by ordinal ids, so the M2d/M1/M3 storage changes can happen behind
//! it without touching those call sites.
//!
//! # What already existed
//!
//! Two of the capabilities M2a was asked for were **already present and already
//! safe**, and are deliberately *not* duplicated here (`CLAUDE.md` §6.1):
//!
//! | Requirement | Existing method | Status |
//! |---|---|---|
//! | entity lookup by `DeclId` | [`Ir::decl_node`] | already `Option`-returning and `NONE`-guarded |
//! | declaration → record id | [`Ir::as_node`], [`Ir::as_edge`], [`Ir::as_arc`] | already `Option`-returning and `NONE`-guarded |
//!
//! Adding `declaration()` or `node_id_for_decl()` would have been a rename with
//! no behavioural difference. The repository's established term is **`decl`**
//! (`decl_nodes`, `decl_node`, `decl_kind`, `decl_children`, `decl_to_node`,
//! `DeclId`, `DeclNode`), and this module keeps it.
//!
//! # Storage assumptions
//!
//! Every method's *contract* is storage-independent. Implementations use
//! ordinal indexing internally — that is the point of the boundary — but no
//! contract promises it. Per method:
//!
//! | Method | Contract assumes | Implementation currently uses |
//! |---|---|---|
//! | [`Ir::node`] / [`Ir::edge`] / [`Ir::arc`] | id validity is decidable | dense `Vec::get` |
//! | [`Ir::node_for_decl`] and siblings | a declaration maps to at most one record | parallel `decl_to_*` map |
//! | [`Ir::decls`] / [`Ir::node_records`] / … | iteration is finite and yields each live entity once | dense enumerate |
//! | [`Ir::decl_count`] and siblings | counts are **live entities**, not capacity | `Vec::len` |
//! | [`Ir::contains_decl`] and siblings | id validity is decidable | range check |
//! | [`Ir::parent_of`] / [`Ir::first_child_of`] / [`Ir::next_sibling_of`] | hierarchy links resolve or are absent | checked index |
//!
//! None of these contracts assumes dense ids, absence of tombstones, ordinal
//! stability, or the existence of the parallel maps. A future generational or
//! tombstoned arena can satisfy all of them by skipping vacant slots, without a
//! single call-site change.
//!
//! # Iteration order
//!
//! Iteration is **deterministic for a given `Ir` snapshot** and is **not part of
//! program semantics**. Do not rely on it to order output, and do not treat two
//! IRs that iterate differently as different programs — the canonical hash
//! sorts before folding precisely because storage order is representational
//! (see `reports/2026-08-28-unified-ir-m05.md` §2). A future arena may skip
//! vacant slots, changing the sequence without changing meaning.
//!
//! # Absence, not error
//!
//! Every lookup returns [`Option`]. `None` means one of: the id is
//! `DeclId::NONE`; the id is out of range for this `Ir`; the declaration exists
//! but has no record of the requested kind; or the mapping and the record
//! disagree. These are expected outcomes of a read, not faults, so no error
//! type is introduced. For a structural diagnosis of *why* an `Ir` is
//! inconsistent, use
//! [`crate::ir::coherence::validate_hierarchy_index_coherence`] — this module
//! deliberately does not re-implement validation inside every lookup.

use crate::common::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use crate::ir::ir::{ArcRec, DeclKind, DeclNode, EdgeRec, Ir, NodeRec};

impl Ir {
    // -- record lookup by typed id -----------------------------------------

    /// The node record for `id`, or `None` if `id` does not name one here.
    #[inline]
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&NodeRec> {
        // No explicit `is_none()` guard: `NONE` is `usize::MAX`, and a `Vec`
        // length can never reach it, so the bounds check already rejects the
        // sentinel. Adding the guard is not merely redundant -- M2a.1 measured
        // it costing 4 extra instructions per lookup, because LLVM cannot
        // prove `x < len` implies `x != usize::MAX` and lowers the pair to
        // branchless `setne`/`setb`/`testb` instead of one predicted branch.
        self.nodes.get(id.raw())
    }

    /// The edge record for `id`, or `None` if `id` does not name one here.
    #[inline]
    #[must_use]
    pub fn edge(&self, id: EdgeId) -> Option<&EdgeRec> {
        // See `Ir::node` for why there is no sentinel guard here.
        self.edges.get(id.raw())
    }

    /// The arc record for `id`, or `None` if `id` does not name one here.
    #[inline]
    #[must_use]
    pub fn arc(&self, id: HyperArcId) -> Option<&ArcRec> {
        // See `Ir::node` for why there is no sentinel guard here.
        self.arcs.get(id.raw())
    }

    // -- declaration → record ----------------------------------------------

    /// The node record a declaration resolves to, with its id.
    ///
    /// Returns `None` when the declaration has no node mapping, when the mapped
    /// record is absent, **or when the record's `decl` back-pointer names a
    /// different declaration**. The last case is a real possibility in a
    /// malformed `Ir`, and returning the record anyway would be a silent wrong
    /// answer: the caller asked about `id`, and the record describes something
    /// else. Diagnosing *which* inconsistency it is belongs to
    /// [`crate::ir::coherence`], not to a lookup.
    #[inline]
    #[must_use]
    pub fn node_for_decl(&self, id: DeclId) -> Option<(NodeId, &NodeRec)> {
        let nid = self.as_node(id)?;
        let rec = self.node(nid)?;
        (rec.decl == id).then_some((nid, rec))
    }

    /// The edge record a declaration resolves to, with its id.
    ///
    /// Same consistency rule as [`Ir::node_for_decl`].
    #[inline]
    #[must_use]
    pub fn edge_for_decl(&self, id: DeclId) -> Option<(EdgeId, &EdgeRec)> {
        let eid = self.as_edge(id)?;
        let rec = self.edge(eid)?;
        (rec.decl == id).then_some((eid, rec))
    }

    /// The arc record a declaration resolves to, with its id.
    ///
    /// `ArcRec` has no `decl` back-pointer — its `in_edge` names the *owning
    /// edge's* declaration, not the arc's own — so unlike the two methods above
    /// this cannot verify a back-link. It checks presence only, and the
    /// asymmetry is a property of the record layout, not an oversight.
    #[inline]
    #[must_use]
    pub fn arc_for_decl(&self, id: DeclId) -> Option<(HyperArcId, &ArcRec)> {
        let aid = self.as_arc(id)?;
        let rec = self.arc(aid)?;
        Some((aid, rec))
    }

    // -- iteration ----------------------------------------------------------

    /// Every live declaration, with its id.
    ///
    /// Order is deterministic for one snapshot and carries no meaning; see the
    /// module documentation.
    pub fn decls(&self) -> impl Iterator<Item = (DeclId, &DeclNode)> + '_ {
        self.decl_nodes
            .iter()
            .enumerate()
            .map(|(i, d)| (DeclId::new(i), d))
    }

    /// Every live node record, with its id.
    pub fn node_records(&self) -> impl Iterator<Item = (NodeId, &NodeRec)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, r)| (NodeId::new(i), r))
    }

    /// Every live edge record, with its id.
    pub fn edge_records(&self) -> impl Iterator<Item = (EdgeId, &EdgeRec)> + '_ {
        self.edges
            .iter()
            .enumerate()
            .map(|(i, r)| (EdgeId::new(i), r))
    }

    /// Every live arc record, with its id.
    pub fn arc_records(&self) -> impl Iterator<Item = (HyperArcId, &ArcRec)> + '_ {
        self.arcs
            .iter()
            .enumerate()
            .map(|(i, r)| (HyperArcId::new(i), r))
    }

    // -- counts -------------------------------------------------------------

    /// Number of **live declarations**, not capacity.
    #[inline]
    #[must_use]
    pub fn decl_count(&self) -> usize {
        self.decl_nodes.len()
    }

    /// Number of **live node records**, not capacity.
    #[inline]
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of **live edge records**, not capacity.
    #[inline]
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Number of **live arc records**, not capacity.
    #[inline]
    #[must_use]
    pub fn arc_count(&self) -> usize {
        self.arcs.len()
    }

    // -- containment --------------------------------------------------------

    /// Whether `id` names a live declaration. Rejects `NONE` and out-of-range.
    #[inline]
    #[must_use]
    pub fn contains_decl(&self, id: DeclId) -> bool {
        self.decl_node(id).is_some()
    }

    /// Whether `id` names a live node record. Rejects `NONE` and out-of-range.
    #[inline]
    #[must_use]
    pub fn contains_node(&self, id: NodeId) -> bool {
        self.node(id).is_some()
    }

    /// Whether `id` names a live edge record. Rejects `NONE` and out-of-range.
    #[inline]
    #[must_use]
    pub fn contains_edge(&self, id: EdgeId) -> bool {
        self.edge(id).is_some()
    }

    /// Whether `id` names a live arc record. Rejects `NONE` and out-of-range.
    #[inline]
    #[must_use]
    pub fn contains_arc(&self, id: HyperArcId) -> bool {
        self.arc(id).is_some()
    }

    // -- hierarchy, total ----------------------------------------------------
    //
    // The existing `parent`, `first_child` and `next_sibling` are `NONE`-safe
    // but index directly, so an out-of-range id panics; `kind` and `decl_kind`
    // do not even guard `NONE`. Their signatures are relied on by existing
    // callers and are left exactly as they are. These are the total
    // alternatives, distinguished by the `_of` suffix.

    /// The parent of `id`, or `None` if `id` is absent or has no parent.
    ///
    /// Total: never panics, for any `DeclId`.
    #[inline]
    #[must_use]
    pub fn parent_of(&self, id: DeclId) -> Option<DeclId> {
        let p = self.decl_node(id)?.parent;
        p.is_some().then_some(p)
    }

    /// The first child of `id`, or `None` if `id` is absent or has no children.
    ///
    /// Total: never panics, for any `DeclId`.
    #[inline]
    #[must_use]
    pub fn first_child_of(&self, id: DeclId) -> Option<DeclId> {
        let c = self.decl_node(id)?.first_child;
        c.is_some().then_some(c)
    }

    /// The last child of `id`, or `None` if `id` is absent or has no children.
    ///
    /// Total: never panics, for any `DeclId`.
    #[inline]
    #[must_use]
    pub fn last_child_of(&self, id: DeclId) -> Option<DeclId> {
        let c = self.decl_node(id)?.last_child;
        c.is_some().then_some(c)
    }

    /// The next sibling of `id`, or `None` if `id` is absent or is last.
    ///
    /// Total: never panics, for any `DeclId`.
    #[inline]
    #[must_use]
    pub fn next_sibling_of(&self, id: DeclId) -> Option<DeclId> {
        let s = self.decl_node(id)?.next_sibling;
        s.is_some().then_some(s)
    }

    /// The kind of `id`, or `None` if `id` is absent.
    ///
    /// Total, unlike [`Ir::kind`] and [`Ir::decl_kind`], which panic on
    /// `DeclId::NONE` and on out-of-range ids.
    #[inline]
    #[must_use]
    pub fn kind_of(&self, id: DeclId) -> Option<DeclKind> {
        Some(self.decl_node(id)?.kind)
    }

    /// The children of `id`, as a total iterator.
    ///
    /// Differs from [`Ir::decl_children`] in more than naming: that iterator
    /// resolves each step through [`Ir::next_sibling`], which indexes directly
    /// and therefore panics if a sibling link points out of range. This one
    /// stops instead. On a well-formed `Ir` the two yield the same sequence;
    /// they differ only on malformed input, which is exactly when a read API
    /// should not abort the process.
    pub fn child_decls(&self, id: DeclId) -> ChildDecls<'_> {
        ChildDecls {
            ir: self,
            next: self.first_child_of(id).unwrap_or(DeclId::NONE),
            budget: self.decl_nodes.len(),
        }
    }
}

/// Total child iterator returned by [`Ir::child_decls`].
///
/// Terminates on absent ids and on a malformed sibling cycle: the walk is
/// bounded by the declaration count, which no acyclic chain can exceed.
pub struct ChildDecls<'a> {
    ir: &'a Ir,
    next: DeclId,
    budget: usize,
}

impl Iterator for ChildDecls<'_> {
    type Item = DeclId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next.is_none() || self.budget == 0 {
            return None;
        }
        let cur = self.next;
        // A dangling link ends the walk rather than panicking.
        let node = self.ir.decl_node(cur)?;
        self.next = node.next_sibling;
        self.budget -= 1;
        Some(cur)
    }
}
