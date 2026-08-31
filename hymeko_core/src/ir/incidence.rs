//! Canonical reverse-incidence navigation and the sparse signed star operator
//! (APPROVED-CORE-EDIT: canonical-ir-incidence-navigation, 2026-09-01;
//! corrective pass 2026-09-01).
//!
//! The `Ir` natively walks **edge → arc → ref → target** (`EdgeRec.arcs`,
//! `ArcRec.refs`, `SignedRefR`). This module adds the reverse **node → incidence**
//! direction and the vector-form operator, both **directly on the `Ir`** — no
//! dense adjacency, no pairwise / clique / 2-section expansion, no synthetic star
//! vertex, no dense vertex re-numbering, no `Ir → SignedGraph` conversion.
//!
//! # The sparse signed incidence operator `B`
//!
//! The canonical incidences define a ragged **signed sparse** operator
//! `B[arc, target]`: `ArcRec.refs` is its row orientation; the reverse index is
//! the `Bᵀ` access permutation. Neither is a pairwise graph or a materialised
//! matrix. `node_to_arc` = `Bx`, `arc_to_node` = `Bᵀy`, `star_operator` = `BᵀBx`,
//! all `O(nnz)` (`nnz` = incidence count), allocation-free with caller-owned
//! output/scratch.
//!
//! # [`IncidenceIndex`]
//!
//! A compact **SoA CSR** whose every entry is the exact incidence coordinate
//! `(HyperArcId, ref_slot)` — so one incidence resolves in `O(1)` to its arc,
//! `SignedRefR`, target, sign, weights and annotation without rescanning the arc.
//! It **borrows the `Ir` it was built from** (`IncidenceIndex<'ir>`), so it cannot
//! be paired with another `Ir`, the `Ir` cannot be mutated while it lives, and its
//! methods hand back `IncidenceRef<'ir>` directly. Lossless (sign / weight /
//! duplicate incidence / order preserved), canonical IDs in and out. Derived and
//! read-only; never a second source of truth. No `unsafe`, no pointer identity.

use crate::common::ids::{DeclId, EdgeId, HyperArcId};
use crate::ir::ir::{AnnoR, Ir, SignedRefR, ValueR};

/// One incidence, resolved from its `(arc, slot)` coordinate. Canonical IDs; the
/// weights and annotation borrow the `Ir`.
#[derive(Clone, Copy)]
pub struct IncidenceRef<'ir> {
    pub arc: HyperArcId,
    pub edge: EdgeId,
    pub slot: u32,
    pub target: DeclId,
    pub sign: i8,
    pub weights: Option<&'ir [ValueR]>,
    pub anno: &'ir AnnoR,
}

/// Building or navigating an [`IncidenceIndex`] failed. Ids-only, deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncidenceError {
    /// An arc reference targets a declaration outside `decl_nodes`.
    TargetOutOfRange { arc: HyperArcId, target: DeclId },
    /// A ref slot does not fit the `u32` coordinate domain.
    SlotOverflow { arc: HyperArcId, slot: usize },
    /// More incidences than fit a `u32` CSR offset.
    IncidenceOverflow { total: usize },
}

impl core::fmt::Display for IncidenceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TargetOutOfRange { arc, target } => write!(
                f,
                "arc {} references out-of-range target decl {}",
                arc.raw(),
                target.raw()
            ),
            Self::SlotOverflow { arc, slot } => {
                write!(
                    f,
                    "arc {} ref slot {slot} exceeds the u32 domain",
                    arc.raw()
                )
            }
            Self::IncidenceOverflow { total } => {
                write!(f, "incidence count {total} exceeds the u32 CSR domain")
            }
        }
    }
}
impl std::error::Error for IncidenceError {}

/// An incoherence between the index coordinates and the `Ir` incidences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordError {
    /// The `row` array length disagrees with the declaration count.
    RowLength { got: usize, expected: usize },
    /// A coordinate `(arc, slot)` names a non-existent arc or out-of-range slot.
    CoordOutOfRange {
        node: DeclId,
        arc: HyperArcId,
        slot: u32,
    },
    /// The coordinate at reverse-row `node` targets a different declaration.
    TargetMismatch {
        node: DeclId,
        arc: HyperArcId,
        slot: u32,
        actual: DeclId,
    },
    /// The same incidence coordinate `(arc, slot)` appears more than once.
    DuplicateCoordinate { arc: HyperArcId, slot: u32 },
    /// The index is not a bijection with the Ir incidences (missing / extra
    /// coordinate — counts disagree).
    NotABijection {
        ir_incidences: usize,
        index_incidences: usize,
    },
}

impl core::fmt::Display for CoordError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RowLength { got, expected } => {
                write!(f, "reverse row length {got} != decl count + 1 = {expected}")
            }
            Self::CoordOutOfRange { node, arc, slot } => write!(
                f,
                "node {} coordinate (arc {}, slot {slot}) is out of range",
                node.raw(),
                arc.raw()
            ),
            Self::TargetMismatch {
                node,
                arc,
                slot,
                actual,
            } => write!(
                f,
                "node {} coordinate (arc {}, slot {slot}) targets {} instead",
                node.raw(),
                arc.raw(),
                actual.raw()
            ),
            Self::DuplicateCoordinate { arc, slot } => write!(
                f,
                "incidence coordinate (arc {}, slot {slot}) appears more than once",
                arc.raw()
            ),
            Self::NotABijection {
                ir_incidences,
                index_incidences,
            } => write!(
                f,
                "index is not a bijection: Ir has {ir_incidences} incidences, index has {index_incidences}"
            ),
        }
    }
}
impl std::error::Error for CoordError {}

/// Reverse-incidence SoA CSR bound to the `Ir` it indexes.
///
/// `row[d..d+1]` bounds node `DeclId(d)`'s incidence coordinates inside the
/// parallel `arcs` / `slots` arrays (SoA avoids the `(HyperArcId,u32)` tuple pad).
#[derive(Clone)]
pub struct IncidenceIndex<'ir> {
    ir: &'ir Ir,
    row: Vec<u32>,
    arcs: Vec<HyperArcId>,
    slots: Vec<u32>,
}

impl core::fmt::Debug for IncidenceIndex<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Deliberately omit the borrowed `Ir` (avoid dumping the whole program).
        f.debug_struct("IncidenceIndex")
            .field("decls", &self.row.len().saturating_sub(1))
            .field("incidences", &self.arcs.len())
            .finish_non_exhaustive()
    }
}

/// Scalar-weight policy (explicit, testable): no weights → `1.0`; otherwise the
/// **product** of the `ValueR::Num` components (a `List` is flattened one level);
/// non-numeric components are ignored, an all-non-numeric weight is `1.0`.
#[must_use]
pub fn scalar_weight(sref: &SignedRefR) -> f64 {
    match &sref.atom().weights {
        None => 1.0,
        Some(ws) => {
            let mut prod = 1.0;
            let mut any = false;
            for w in ws {
                match w {
                    ValueR::Num(x) => {
                        prod *= *x;
                        any = true;
                    }
                    ValueR::List(inner) => {
                        for v in inner {
                            if let ValueR::Num(x) = v {
                                prod *= *x;
                                any = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if any { prod } else { 1.0 }
        }
    }
}

/// Build the reverse-incidence index from the canonical incidences of `ir`.
///
/// Two passes over the arcs' refs (count, then fill) — `O(nnz)` time,
/// `O(decls + nnz)` memory, one allocation per array. Deterministic: within a
/// node's slice, coordinates appear in ascending `HyperArcId`, then ref-slot
/// order — the canonical arc/ref order.
///
/// # Errors
/// [`IncidenceError::TargetOutOfRange`] (dangling target),
/// [`IncidenceError::SlotOverflow`] (ref slot ≥ 2³²),
/// [`IncidenceError::IncidenceOverflow`] (nnz ≥ 2³²).
pub fn build_incidence_index(ir: &Ir) -> Result<IncidenceIndex<'_>, IncidenceError> {
    let n = ir.decl_nodes.len();
    let mut row = vec![0u32; n + 1];

    let mut total: usize = 0;
    for (ai, arc) in ir.arcs.iter().enumerate() {
        for (slot, sref) in arc.refs.iter().enumerate() {
            let t = sref.target();
            let ti = t.raw();
            if ti >= n {
                return Err(IncidenceError::TargetOutOfRange {
                    arc: HyperArcId::new(ai),
                    target: t,
                });
            }
            if u32::try_from(slot).is_err() {
                return Err(IncidenceError::SlotOverflow {
                    arc: HyperArcId::new(ai),
                    slot,
                });
            }
            row[ti + 1] += 1;
            total += 1;
        }
    }
    if u32::try_from(total).is_err() {
        return Err(IncidenceError::IncidenceOverflow { total });
    }
    for i in 0..n {
        row[i + 1] += row[i];
    }

    let mut arcs = vec![HyperArcId::NONE; total];
    let mut slots = vec![0u32; total];
    let mut cur = row.clone();
    for (ai, arc) in ir.arcs.iter().enumerate() {
        let aid = HyperArcId::new(ai);
        for (slot, sref) in arc.refs.iter().enumerate() {
            let ti = sref.target().raw();
            let p = &mut cur[ti];
            arcs[*p as usize] = aid;
            slots[*p as usize] = slot as u32;
            *p += 1;
        }
    }

    Ok(IncidenceIndex {
        ir,
        row,
        arcs,
        slots,
    })
}

impl<'ir> IncidenceIndex<'ir> {
    /// The `Ir` this index borrows.
    #[must_use]
    pub fn ir(&self) -> &'ir Ir {
        self.ir
    }

    /// Incident arc ids of `node`, ascending arc id then ref slot; duplicates kept.
    #[must_use]
    pub fn incident_arcs(&self, node: DeclId) -> &[HyperArcId] {
        let (lo, hi) = self.range(node);
        &self.arcs[lo..hi]
    }

    /// Incident ref slots of `node`, parallel to [`Self::incident_arcs`].
    #[must_use]
    pub fn incident_slots(&self, node: DeclId) -> &[u32] {
        let (lo, hi) = self.range(node);
        &self.slots[lo..hi]
    }

    /// Fully-resolved incidences of `node` — each an `O(1)` [`IncidenceRef`].
    pub fn incidences(&self, node: DeclId) -> impl Iterator<Item = IncidenceRef<'ir>> + '_ {
        let (lo, hi) = self.range(node);
        (lo..hi).map(move |k| self.resolve(self.arcs[k], self.slots[k]))
    }

    /// Resolve one coordinate to a full [`IncidenceRef`]. `O(1)`.
    #[must_use]
    pub fn resolve(&self, arc: HyperArcId, slot: u32) -> IncidenceRef<'ir> {
        let arc_rec = &self.ir.arcs[arc.raw()];
        let sref = &arc_rec.refs[slot as usize];
        let edge = self
            .ir
            .edge_for_decl(arc_rec.in_edge)
            .map_or(EdgeId::NONE, |(e, _)| e);
        IncidenceRef {
            arc,
            edge,
            slot,
            target: sref.target(),
            sign: sref.sign(),
            weights: sref.atom().weights.as_deref(),
            anno: &sref.atom().anno,
        }
    }

    /// Incidence degree of `node` (with multiplicity).
    #[must_use]
    pub fn degree(&self, node: DeclId) -> usize {
        let (lo, hi) = self.range(node);
        hi - lo
    }

    /// Total incidences indexed (`nnz`).
    #[must_use]
    pub fn incidence_count(&self) -> usize {
        self.arcs.len()
    }

    #[inline]
    fn range(&self, node: DeclId) -> (usize, usize) {
        let d = node.raw();
        match (self.row.get(d), self.row.get(d + 1)) {
            (Some(&lo), Some(&hi)) => (lo as usize, hi as usize),
            _ => (0, 0),
        }
    }

    // ---- memory accounting (real production sizes, not a u32 prototype) ----

    /// Bytes retained by the index arrays (capacity-accurate; real `HyperArcId`).
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        self.row.capacity() * size_of::<u32>()
            + self.arcs.capacity() * size_of::<HyperArcId>()
            + self.slots.capacity() * size_of::<u32>()
    }

    // ---- exact coherence validator (bijection of coordinates) ----

    /// Prove the index is an exact bijection with the `Ir` incidences: every
    /// `(arc, slot)` coordinate appears exactly once, lands in the reverse row of
    /// its true target, is in range, and none is missing, extra or duplicated.
    /// Diagnostic — call once, not per query.
    ///
    /// # Errors
    /// [`CoordError`] on the first coordinate that breaks the bijection.
    pub fn validate(&self) -> Result<(), CoordError> {
        let n = self.ir.decl_nodes.len();
        if self.row.len() != n + 1 {
            return Err(CoordError::RowLength {
                got: self.row.len(),
                expected: n + 1,
            });
        }
        // Canonical incidence position of (arc, slot) = arc_base[arc] + slot, a
        // dense 0..nnz id. Marking each exactly once proves a true bijection:
        // duplicates are caught on the second mark, missing ones by the final
        // sweep. (The old degree + any(target) check missed a coordinate swap that
        // duplicated one incidence and dropped another with the same target.)
        let mut arc_base = vec![0usize; self.ir.arcs.len() + 1];
        for (i, a) in self.ir.arcs.iter().enumerate() {
            arc_base[i + 1] = arc_base[i] + a.refs.len();
        }
        let ir_nnz = *arc_base.last().unwrap_or(&0);
        let mut seen = vec![false; ir_nnz];

        let mut index_nnz = 0usize;
        for d in 0..n {
            let node = DeclId::new(d);
            let (lo, hi) = self.range(node);
            for k in lo..hi {
                let arc = self.arcs[k];
                let slot = self.slots[k];
                let ok_target = self
                    .ir
                    .arcs
                    .get(arc.raw())
                    .and_then(|a| a.refs.get(slot as usize))
                    .map(SignedRefR::target);
                match ok_target {
                    None => return Err(CoordError::CoordOutOfRange { node, arc, slot }),
                    Some(t) if t != node => {
                        return Err(CoordError::TargetMismatch {
                            node,
                            arc,
                            slot,
                            actual: t,
                        });
                    }
                    Some(_) => {}
                }
                let pos = arc_base[arc.raw()] + slot as usize;
                if seen[pos] {
                    return Err(CoordError::DuplicateCoordinate { arc, slot });
                }
                seen[pos] = true;
                index_nnz += 1;
            }
        }
        if index_nnz != ir_nnz || seen.iter().any(|&s| !s) {
            return Err(CoordError::NotABijection {
                ir_incidences: ir_nnz,
                index_incidences: index_nnz,
            });
        }
        Ok(())
    }

    // ---- sparse signed star operator (B, no materialization) ----

    /// `y = B·x`: `y[arc] = Σ_slot sign·weight·x[target]`. Arc-major, output-
    /// partitioned by arc (each arc independent), allocation-free. `x` is indexed
    /// by `DeclId.raw()` (len ≥ decls), `y` by `HyperArcId.raw()` (len ≥ arcs).
    /// Deterministic FP order: refs left-to-right within each arc.
    pub fn node_to_arc(&self, x: &[f64], y: &mut [f64]) {
        for (ai, arc) in self.ir.arcs.iter().enumerate() {
            let mut acc = 0.0;
            for sref in &arc.refs {
                acc += f64::from(sref.sign()) * scalar_weight(sref) * x[sref.target().raw()];
            }
            y[ai] = acc;
        }
    }

    /// `z = Bᵀ·y`: `z[node] = Σ_(arc,slot)∈incident(node) sign·weight·y[arc]`.
    /// **Node-major via the reverse index** — output-partitioned by node, so it is
    /// allocation-free, deterministic, and (unlike an arc-major scatter) yields
    /// identical results serial or parallel. `z[0..decls]` is zeroed here.
    pub fn arc_to_node(&self, y: &[f64], z: &mut [f64]) {
        let n = self.ir.decl_nodes.len();
        for zi in z.iter_mut().take(n) {
            *zi = 0.0;
        }
        for d in 0..n {
            let (lo, hi) = self.range(DeclId::new(d));
            let mut acc = 0.0;
            for k in lo..hi {
                let arc = self.arcs[k];
                let sref = &self.ir.arcs[arc.raw()].refs[self.slots[k] as usize];
                acc += f64::from(sref.sign()) * scalar_weight(sref) * y[arc.raw()];
            }
            z[d] = acc;
        }
    }

    /// `z = Bᵀ·B·x` — the signed star operator, no materialised graph or matrix.
    /// `y_scratch` is caller-owned (len ≥ arcs); no allocation.
    pub fn star_operator(&self, x: &[f64], z: &mut [f64], y_scratch: &mut [f64]) {
        self.node_to_arc(x, y_scratch);
        self.arc_to_node(y_scratch, z);
    }
}

#[cfg(test)]
mod fault_tests {
    use super::*;
    use crate::common::ids::{EdgeId, NodeId, SymId};
    use crate::ir::ir::{AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, NodeRec, RefAtomR};

    /// Minimal Ir: nodes a(0), b(1), edge e(2), one arc with refs [a, a, b].
    fn tiny() -> Ir {
        let mut ir = Ir::default();
        for kind in [DeclKind::Node, DeclKind::Node, DeclKind::Edge] {
            ir.push_decl(DeclNode {
                kind,
                name: SymId::new(0),
                parent: DeclId::NONE,
                first_child: DeclId::NONE,
                last_child: DeclId::NONE,
                next_sibling: DeclId::NONE,
                anno: AnnoR::default(),
            });
            ir.decl_hash.push(None);
        }
        ir.nodes.push(NodeRec::new(DeclId::new(0), Vec::new()));
        ir.nodes.push(NodeRec::new(DeclId::new(1), Vec::new()));
        ir.bind_node_decl(DeclId::new(0), NodeId::new(0)).unwrap();
        ir.bind_node_decl(DeclId::new(1), NodeId::new(1)).unwrap();
        ir.edges.push(EdgeRec::new(DeclId::new(2), Vec::new()));
        ir.bind_edge_decl(DeclId::new(2), EdgeId::new(0)).unwrap();
        let arc_decl = ir.push_anonymous_arc();
        let atom = |t: usize| RefAtomR {
            target: DeclId::new(t),
            anno: AnnoR::default(),
            weights: None,
        };
        ir.arcs.push(ArcRec {
            anno: AnnoR::default(),
            in_edge: DeclId::new(2),
            refs: vec![
                SignedRefR::Plus(atom(0)),
                SignedRefR::Minus(atom(0)),
                SignedRefR::Neutral(atom(1)),
            ],
        });
        ir.bind_arc_decl(arc_decl, HyperArcId::new(0)).unwrap();
        ir
    }

    #[test]
    fn valid_index_is_a_bijection() {
        let ir = tiny();
        let idx = build_incidence_index(&ir).unwrap();
        assert!(idx.validate().is_ok());
        // a has two coords, b one
        assert_eq!(idx.degree(DeclId::new(0)), 2);
        assert_eq!(idx.degree(DeclId::new(1)), 1);
    }

    #[test]
    fn duplicated_coordinate_is_caught() {
        let ir = tiny();
        let mut idx = build_incidence_index(&ir).unwrap();
        // a's row is [(arc0,slot0),(arc0,slot1)]; make both slot 0 -> duplicate,
        // target still a so it is NOT a mere TargetMismatch.
        idx.slots[1] = 0;
        assert_eq!(
            idx.validate(),
            Err(CoordError::DuplicateCoordinate {
                arc: HyperArcId::new(0),
                slot: 0,
            })
        );
    }

    #[test]
    fn swapped_coordinate_keeping_degree_is_caught() {
        let ir = tiny();
        let mut idx = build_incidence_index(&ir).unwrap();
        // point a's first coordinate at (arc0, slot2) which targets b: degree
        // unchanged, but the target no longer matches a's reverse row.
        idx.slots[0] = 2;
        assert_eq!(
            idx.validate(),
            Err(CoordError::TargetMismatch {
                node: DeclId::new(0),
                arc: HyperArcId::new(0),
                slot: 2,
                actual: DeclId::new(1),
            })
        );
    }

    #[test]
    fn out_of_range_slot_is_caught() {
        let ir = tiny();
        let mut idx = build_incidence_index(&ir).unwrap();
        idx.slots[0] = 99;
        assert_eq!(
            idx.validate(),
            Err(CoordError::CoordOutOfRange {
                node: DeclId::new(0),
                arc: HyperArcId::new(0),
                slot: 99,
            })
        );
    }

    #[test]
    fn truncated_row_is_caught() {
        let ir = tiny();
        let mut idx = build_incidence_index(&ir).unwrap();
        idx.row.pop();
        assert!(matches!(idx.validate(), Err(CoordError::RowLength { .. })));
    }
}
