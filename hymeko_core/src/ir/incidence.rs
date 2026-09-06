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

/// Reverse-incidence SoA CSR — the **lifetime-free** derived navigation payload.
///
/// `row[d..d+1]` bounds node `DeclId(d)`'s incidence coordinates inside the parallel
/// `arcs` / `slots` arrays (SoA avoids the `(HyperArcId, u32)` tuple pad).
///
/// Holds only *derived* data: `row` offsets plus the parallel `arcs` / `slots`
/// coordinate arrays. It borrows **nothing**, so it can be (a) *owned* alongside the
/// `Ir` it was derived from ([`PreparedIr`]) without the self-referential-lifetime
/// problem, and (b) paired with a `&Ir` at query time through a borrowed
/// [`IncidenceView`]. It is not a second source of truth: every coordinate is
/// validated as an exact bijection with the `Ir` incidences
/// ([`IncidenceIndexData::validate`]), it never serializes, and its fields are
/// private — the only constructor is the bijection-checked [`IncidenceIndexData::build`].
#[derive(Clone, Debug)]
pub struct IncidenceIndexData {
    row: Vec<u32>,
    arcs: Vec<HyperArcId>,
    slots: Vec<u32>,
}

/// Reverse-incidence index **borrowing** the `Ir` it indexes: the `&'ir Ir` paired
/// with the derived [`IncidenceIndexData`]. Same lifetime-bound guarantees as before
/// — it cannot be paired with another `Ir`, the `Ir` cannot be mutated while it
/// lives, and it hands back `IncidenceRef<'ir>` directly — now with the derived
/// payload factored into a reusable, *ownable* [`IncidenceIndexData`] (see
/// [`PreparedIr`] for the owned bundle). The public API is unchanged.
#[derive(Clone)]
pub struct IncidenceIndex<'ir> {
    ir: &'ir Ir,
    data: IncidenceIndexData,
}

impl core::fmt::Debug for IncidenceIndex<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Deliberately omit the borrowed `Ir` (avoid dumping the whole program).
        f.debug_struct("IncidenceIndex")
            .field("decls", &self.data.row.len().saturating_sub(1))
            .field("incidences", &self.data.arcs.len())
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
fn build_arrays(ir: &Ir) -> Result<IncidenceIndexData, IncidenceError> {
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

    Ok(IncidenceIndexData { row, arcs, slots })
}

/// Build the reverse-incidence index for `ir`, **borrowing** it. Thin wrapper
/// over [`build_arrays`] that pairs the lifetime-free [`IncidenceIndexData`] with
/// the `&Ir` it was derived from. See module docs.
///
/// # Errors
/// As [`build_arrays`].
pub fn build_incidence_index(ir: &Ir) -> Result<IncidenceIndex<'_>, IncidenceError> {
    Ok(IncidenceIndex {
        ir,
        data: build_arrays(ir)?,
    })
}

impl IncidenceIndexData {
    /// Build the lifetime-free reverse-incidence index from `ir`'s canonical incidences.
    ///
    /// # Errors
    /// As [`build_arrays`].
    pub fn build(ir: &Ir) -> Result<Self, IncidenceError> {
        build_arrays(ir)
    }

    #[inline]
    fn range(&self, node: DeclId) -> (usize, usize) {
        let d = node.raw();
        match (self.row.get(d), self.row.get(d + 1)) {
            (Some(&lo), Some(&hi)) => (lo as usize, hi as usize),
            _ => (0, 0),
        }
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

    /// Bytes retained by the index arrays (capacity-accurate; real `HyperArcId`).
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        self.row.capacity() * size_of::<u32>()
            + self.arcs.capacity() * size_of::<HyperArcId>()
            + self.slots.capacity() * size_of::<u32>()
    }

    /// Resolve one coordinate to a full [`IncidenceRef`] against `ir`. `O(1)`.
    ///
    /// # Preconditions
    /// `ir` **must** be the `Ir` this index was [built](Self::build) from — the
    /// [`IncidenceIndex`] / [`PreparedIr`] / [`IncidenceView`] wrappers enforce this
    /// at the type level; calling with a foreign `Ir` is a caller bug and may panic
    /// on an out-of-range coordinate.
    #[must_use]
    pub fn resolve<'ir>(&self, ir: &'ir Ir, arc: HyperArcId, slot: u32) -> IncidenceRef<'ir> {
        let arc_rec = &ir.arcs[arc.raw()];
        let sref = &arc_rec.refs[slot as usize];
        let edge = ir
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

    /// Fully-resolved incidences of `node` — each an `O(1)` [`IncidenceRef`].
    ///
    /// # Preconditions
    /// As [`Self::resolve`] — `ir` is the `Ir` this index was built from.
    pub fn incidences<'a, 'ir>(
        &'a self,
        ir: &'ir Ir,
        node: DeclId,
    ) -> impl Iterator<Item = IncidenceRef<'ir>> + 'a
    where
        'ir: 'a,
    {
        let (lo, hi) = self.range(node);
        (lo..hi).map(move |k| self.resolve(ir, self.arcs[k], self.slots[k]))
    }

    /// Prove the index is an exact bijection with the `ir` incidences: every
    /// `(arc, slot)` coordinate appears exactly once, lands in the reverse row of its
    /// true target, is in range, and none is missing, extra or duplicated. Diagnostic
    /// — call once, not per query.
    ///
    /// # Errors
    /// [`CoordError`] on the first coordinate that breaks the bijection.
    pub fn validate(&self, ir: &Ir) -> Result<(), CoordError> {
        let n = ir.decl_nodes.len();
        if self.row.len() != n + 1 {
            return Err(CoordError::RowLength {
                got: self.row.len(),
                expected: n + 1,
            });
        }
        // Canonical incidence position of (arc, slot) = arc_base[arc] + slot, a dense
        // 0..nnz id. Marking each exactly once proves a true bijection: duplicates on
        // the second mark, missing ones by the final sweep.
        let mut arc_base = vec![0usize; ir.arcs.len() + 1];
        for (i, a) in ir.arcs.iter().enumerate() {
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
                let ok_target = ir
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

    /// `y = B·x` (arc-major). See [`IncidenceIndex::node_to_arc`] for the contract.
    pub fn node_to_arc(&self, ir: &Ir, x: &[f64], y: &mut [f64]) {
        for (ai, arc) in ir.arcs.iter().enumerate() {
            let mut acc = 0.0;
            for sref in &arc.refs {
                acc += f64::from(sref.sign()) * scalar_weight(sref) * x[sref.target().raw()];
            }
            y[ai] = acc;
        }
    }

    /// `z = Bᵀ·y` (node-major via the reverse index). See [`IncidenceIndex::arc_to_node`].
    pub fn arc_to_node(&self, ir: &Ir, y: &[f64], z: &mut [f64]) {
        let n = ir.decl_nodes.len();
        for zi in z.iter_mut().take(n) {
            *zi = 0.0;
        }
        for d in 0..n {
            let (lo, hi) = self.range(DeclId::new(d));
            let mut acc = 0.0;
            for k in lo..hi {
                let arc = self.arcs[k];
                let sref = &ir.arcs[arc.raw()].refs[self.slots[k] as usize];
                acc += f64::from(sref.sign()) * scalar_weight(sref) * y[arc.raw()];
            }
            z[d] = acc;
        }
    }

    /// `z = Bᵀ·B·x`. See [`IncidenceIndex::star_operator`]. `y_scratch` caller-owned.
    pub fn star_operator(&self, ir: &Ir, x: &[f64], z: &mut [f64], y_scratch: &mut [f64]) {
        self.node_to_arc(ir, x, y_scratch);
        self.arc_to_node(ir, y_scratch, z);
    }

    /// Precompute the node-major signed coefficient (`sign · scalar_weight`) once per
    /// incidence, in reverse-index order (parallel to `arcs`/`slots`).
    ///
    /// # Errors
    /// [`OperatorPlanError::NonFiniteCoefficient`] on a `NaN`/`±∞` coefficient.
    fn build_coeff(&self, ir: &Ir) -> Result<Vec<f64>, OperatorPlanError> {
        let n = ir.decl_nodes.len();
        let mut coeff: Vec<f64> = Vec::with_capacity(self.arcs.len());
        for d in 0..n {
            let (lo, hi) = self.range(DeclId::new(d));
            for k in lo..hi {
                let arc = self.arcs[k];
                let sref = &ir.arcs[arc.raw()].refs[self.slots[k] as usize];
                let c = f64::from(sref.sign()) * scalar_weight(sref);
                if !c.is_finite() {
                    return Err(OperatorPlanError::NonFiniteCoefficient {
                        node: DeclId::new(d),
                    });
                }
                coeff.push(c);
            }
        }
        Ok(coeff)
    }
}

impl<'ir> IncidenceIndex<'ir> {
    /// The `Ir` this index borrows.
    #[must_use]
    pub fn ir(&self) -> &'ir Ir {
        self.ir
    }

    /// The derived, lifetime-free index payload (shared with [`PreparedIr`] / [`IncidenceView`]).
    #[must_use]
    pub fn data(&self) -> &IncidenceIndexData {
        &self.data
    }

    /// Incident arc ids of `node`, ascending arc id then ref slot; duplicates kept.
    #[must_use]
    pub fn incident_arcs(&self, node: DeclId) -> &[HyperArcId] {
        self.data.incident_arcs(node)
    }

    /// Incident ref slots of `node`, parallel to [`Self::incident_arcs`].
    #[must_use]
    pub fn incident_slots(&self, node: DeclId) -> &[u32] {
        self.data.incident_slots(node)
    }

    /// Fully-resolved incidences of `node` — each an `O(1)` [`IncidenceRef`].
    pub fn incidences(&self, node: DeclId) -> impl Iterator<Item = IncidenceRef<'ir>> + '_ {
        self.data.incidences(self.ir, node)
    }

    /// Resolve one coordinate to a full [`IncidenceRef`]. `O(1)`.
    #[must_use]
    pub fn resolve(&self, arc: HyperArcId, slot: u32) -> IncidenceRef<'ir> {
        self.data.resolve(self.ir, arc, slot)
    }

    /// Incidence degree of `node` (with multiplicity).
    #[must_use]
    pub fn degree(&self, node: DeclId) -> usize {
        self.data.degree(node)
    }

    /// Total incidences indexed (`nnz`).
    #[must_use]
    pub fn incidence_count(&self) -> usize {
        self.data.incidence_count()
    }

    /// Bytes retained by the index arrays (capacity-accurate; real `HyperArcId`).
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.data.retained_bytes()
    }

    /// Prove the index is an exact bijection with the `Ir` incidences. Diagnostic —
    /// call once, not per query.
    ///
    /// # Errors
    /// [`CoordError`] on the first coordinate that breaks the bijection.
    pub fn validate(&self) -> Result<(), CoordError> {
        self.data.validate(self.ir)
    }

    /// `y = B·x`: `y[arc] = Σ_slot sign·weight·x[target]`. Arc-major, output-
    /// partitioned by arc (each arc independent), allocation-free. `x` is indexed
    /// by `DeclId.raw()` (len ≥ decls), `y` by `HyperArcId.raw()` (len ≥ arcs).
    /// Deterministic FP order: refs left-to-right within each arc.
    pub fn node_to_arc(&self, x: &[f64], y: &mut [f64]) {
        self.data.node_to_arc(self.ir, x, y);
    }

    /// `z = Bᵀ·y`: `z[node] = Σ_(arc,slot)∈incident(node) sign·weight·y[arc]`.
    /// **Node-major via the reverse index** — output-partitioned by node, so it is
    /// allocation-free, deterministic, and (unlike an arc-major scatter) yields
    /// identical results serial or parallel. `z[0..decls]` is zeroed here.
    pub fn arc_to_node(&self, y: &[f64], z: &mut [f64]) {
        self.data.arc_to_node(self.ir, y, z);
    }

    /// `z = Bᵀ·B·x` — the signed star operator, no materialised graph or matrix.
    /// `y_scratch` is caller-owned (len ≥ arcs); no allocation.
    pub fn star_operator(&self, x: &[f64], z: &mut [f64], y_scratch: &mut [f64]) {
        self.data.star_operator(self.ir, x, z, y_scratch);
    }

    /// Prepare a caller-owned [`IncidenceOperatorPlan`] — precompute the node-major
    /// signed coefficient (`sign · scalar_weight`) once per incidence, so a later
    /// `Bᵀy` / `BᵀBx` apply skips the per-incidence `(arc,slot)` resolution, `sign()`
    /// match and `scalar_weight` fold (measured ~20× on 1e6 incidences, break-even
    /// ≈ 1.2 applies). The plan borrows this index's `Ir` + derived data, stores no
    /// topology or coordinate, and is not part of the canonical `Ir`.
    ///
    /// # Errors
    /// [`OperatorPlanError::NonFiniteCoefficient`] — a deliberate stricter contract:
    /// the plan rejects a `NaN`/`±∞` coefficient at prepare rather than silently
    /// propagating it through the operator.
    pub fn prepare_operator(&self) -> Result<IncidenceOperatorPlan<'_, 'ir>, OperatorPlanError> {
        let coeff = self.data.build_coeff(self.ir)?;
        Ok(IncidenceOperatorPlan {
            ir: self.ir,
            data: &self.data,
            coeff,
        })
    }
}

/// An `O(1)`, zero-allocation **borrowed** pairing of a `&Ir` with a borrowed
/// [`IncidenceIndexData`] — the same query + operator surface as [`IncidenceIndex`],
/// but holding only two references, so it is `Copy` and free to hand out from a
/// [`PreparedIr`] (`PreparedIr::incidences`). It cannot outlive either borrow and
/// cannot be paired with a foreign `Ir`/index by construction.
#[derive(Clone, Copy)]
pub struct IncidenceView<'a, 'ir: 'a> {
    ir: &'ir Ir,
    data: &'a IncidenceIndexData,
}

impl<'a, 'ir: 'a> IncidenceView<'a, 'ir> {
    /// The borrowed `Ir`.
    #[must_use]
    pub fn ir(&self) -> &'ir Ir {
        self.ir
    }

    /// The borrowed derived index payload.
    #[must_use]
    pub fn data(&self) -> &'a IncidenceIndexData {
        self.data
    }

    /// See [`IncidenceIndex::incident_arcs`].
    #[must_use]
    pub fn incident_arcs(&self, node: DeclId) -> &'a [HyperArcId] {
        self.data.incident_arcs(node)
    }

    /// See [`IncidenceIndex::incident_slots`].
    #[must_use]
    pub fn incident_slots(&self, node: DeclId) -> &'a [u32] {
        self.data.incident_slots(node)
    }

    /// See [`IncidenceIndex::incidences`].
    pub fn incidences(&self, node: DeclId) -> impl Iterator<Item = IncidenceRef<'ir>> + 'a {
        self.data.incidences(self.ir, node)
    }

    /// See [`IncidenceIndex::resolve`].
    #[must_use]
    pub fn resolve(&self, arc: HyperArcId, slot: u32) -> IncidenceRef<'ir> {
        self.data.resolve(self.ir, arc, slot)
    }

    /// See [`IncidenceIndex::degree`].
    #[must_use]
    pub fn degree(&self, node: DeclId) -> usize {
        self.data.degree(node)
    }

    /// See [`IncidenceIndex::incidence_count`].
    #[must_use]
    pub fn incidence_count(&self) -> usize {
        self.data.incidence_count()
    }

    /// See [`IncidenceIndex::retained_bytes`].
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.data.retained_bytes()
    }

    /// See [`IncidenceIndex::validate`].
    ///
    /// # Errors
    /// [`CoordError`] on the first coordinate that breaks the bijection.
    pub fn validate(&self) -> Result<(), CoordError> {
        self.data.validate(self.ir)
    }

    /// See [`IncidenceIndex::node_to_arc`].
    pub fn node_to_arc(&self, x: &[f64], y: &mut [f64]) {
        self.data.node_to_arc(self.ir, x, y);
    }

    /// See [`IncidenceIndex::arc_to_node`].
    pub fn arc_to_node(&self, y: &[f64], z: &mut [f64]) {
        self.data.arc_to_node(self.ir, y, z);
    }

    /// See [`IncidenceIndex::star_operator`].
    pub fn star_operator(&self, x: &[f64], z: &mut [f64], y_scratch: &mut [f64]) {
        self.data.star_operator(self.ir, x, z, y_scratch);
    }

    /// See [`IncidenceIndex::prepare_operator`].
    ///
    /// # Errors
    /// [`OperatorPlanError::NonFiniteCoefficient`].
    pub fn prepare_operator(&self) -> Result<IncidenceOperatorPlan<'a, 'ir>, OperatorPlanError> {
        let coeff = self.data.build_coeff(self.ir)?;
        Ok(IncidenceOperatorPlan {
            ir: self.ir,
            data: self.data,
            coeff,
        })
    }

    /// Borrowed handle for the implicit sparse signed star operator
    /// `A_star = BᵀB` — created `O(1)` / zero-alloc, never materialises `A_star`.
    #[must_use]
    pub fn star(&self) -> StarOperator<'a, 'ir> {
        StarOperator { view: *self }
    }
}

/// **Owned canonical bundle**: owns the `Ir` *and* the lifetime-free derived
/// [`IncidenceIndexData`], so it hands out an `O(1)`, zero-allocation borrowed
/// [`IncidenceView`] without the self-referential-lifetime problem (the index
/// borrows nothing — it is paired with the `Ir` only at view time). Not
/// self-referential, no `unsafe`, no pointer identity; the index cannot be paired
/// with a *different* `Ir`. Cheaply `Arc`-shareable (share the whole bundle).
///
/// The wire format carries only the canonical `Ir`; on load, rebuild via
/// [`PreparedIr::new`] — the derived index is never serialized (see the
/// serde-rebuild-parity tests).
pub struct PreparedIr {
    ir: Ir,
    data: IncidenceIndexData,
}

impl PreparedIr {
    /// Build the owned bundle: validate `ir` into its derived reverse-incidence index.
    ///
    /// # Errors
    /// As [`build_arrays`] ([`IncidenceError`]).
    pub fn new(ir: Ir) -> Result<Self, IncidenceError> {
        let data = IncidenceIndexData::build(&ir)?;
        Ok(Self { ir, data })
    }

    /// The owned `Ir`.
    #[must_use]
    pub fn ir(&self) -> &Ir {
        &self.ir
    }

    /// The owned derived index payload.
    #[must_use]
    pub fn index(&self) -> &IncidenceIndexData {
        &self.data
    }

    /// An `O(1)`, zero-allocation borrowed [`IncidenceView`] over the owned `Ir` +
    /// index (constructs a two-reference `Copy` value; allocates nothing).
    #[must_use]
    pub fn incidences(&self) -> IncidenceView<'_, '_> {
        IncidenceView {
            ir: &self.ir,
            data: &self.data,
        }
    }

    /// The borrowed star operator handle (`A_star = BᵀB`), `O(1)` / zero-alloc.
    #[must_use]
    pub fn star_operator(&self) -> StarOperator<'_, '_> {
        self.incidences().star()
    }

    /// Consume the bundle, returning the owned `Ir` (drops the derived index).
    #[must_use]
    pub fn into_ir(self) -> Ir {
        self.ir
    }
}

/// Borrowed handle for the implicit sparse signed star operator `A_star = BᵀB`
/// (`[[0, Bᵀ], [B, 0]]` on the bipartite node/arc space). Created `O(1)` /
/// zero-alloc from a [`PreparedIr`] or [`IncidenceView`]; it **never** materialises
/// the star adjacency, a clique / 2-section, or a copy of the payload in both
/// orientations — every apply is `O(nnz)` over the borrowed incidences.
#[derive(Clone, Copy)]
pub struct StarOperator<'a, 'ir: 'a> {
    view: IncidenceView<'a, 'ir>,
}

impl<'a, 'ir: 'a> StarOperator<'a, 'ir> {
    /// `y = B·x` (node→arc). See [`IncidenceIndex::node_to_arc`].
    pub fn node_to_arc(&self, x: &[f64], y: &mut [f64]) {
        self.view.node_to_arc(x, y);
    }

    /// `z = Bᵀ·y` (arc→node). See [`IncidenceIndex::arc_to_node`].
    pub fn arc_to_node(&self, y: &[f64], z: &mut [f64]) {
        self.view.arc_to_node(y, z);
    }

    /// `z = Bᵀ·B·x` — the full star apply. `y_scratch` caller-owned (len ≥ arcs);
    /// allocation-free. `O(nnz)`.
    pub fn apply(&self, x: &[f64], z: &mut [f64], y_scratch: &mut [f64]) {
        self.view.star_operator(x, z, y_scratch);
    }

    /// The [`IncidenceView`] this operator borrows.
    #[must_use]
    pub fn view(&self) -> IncidenceView<'a, 'ir> {
        self.view
    }
}

/// A prepared incidence operator could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorPlanError {
    /// A precomputed coefficient (`sign · scalar_weight`) was `NaN` or `±∞`. The plan rejects it (a
    /// deliberate contract stricter than the un-prepared operator, which propagates it).
    NonFiniteCoefficient {
        /// The node whose incidence produced the non-finite coefficient.
        node: DeclId,
    },
}

impl core::fmt::Display for OperatorPlanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NonFiniteCoefficient { node } => {
                write!(f, "non-finite prepared coefficient at node {}", node.raw())
            }
        }
    }
}
impl std::error::Error for OperatorPlanError {}

/// A caller-owned, reusable precompute of the node-major signed coefficients for the `Bᵀ` (arc→node)
/// operator. **Borrows** the `Ir` + the derived [`IncidenceIndexData`] (so it works identically whether the
/// source is an [`IncidenceIndex`], an [`IncidenceView`] or a [`PreparedIr`]); it is not part of the
/// canonical `Ir`, does not serialize, and cannot pair with a different index (the borrows enforce it at
/// compile time). Stores exactly **one `f64` per incidence** in node-major order and **copies no topology or
/// `(arc,slot)` coordinate** — the arc index for each incidence is read from the borrowed index. A warm apply
/// allocates nothing. For finite weights the results are **bitwise-identical** to the un-prepared operator
/// (the same `(sign·weight)·y` expression and fold order).
pub struct IncidenceOperatorPlan<'a, 'ir> {
    ir: &'ir Ir,
    data: &'a IncidenceIndexData,
    /// Node-major, one per incidence; `coeff.len() == data.incidence_count()`.
    coeff: Vec<f64>,
}

impl<'a, 'ir> IncidenceOperatorPlan<'a, 'ir> {
    /// The borrowed `Ir`.
    #[must_use]
    pub fn ir(&self) -> &'ir Ir {
        self.ir
    }

    /// The borrowed derived index payload.
    #[must_use]
    pub fn data(&self) -> &'a IncidenceIndexData {
        self.data
    }

    /// Bytes retained by the plan (capacity-accurate): 8 per incidence.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.coeff.capacity() * std::mem::size_of::<f64>()
    }

    /// `z = Bᵀ·y` using the prepared coefficients. Node-major, output-partitioned, allocation-free;
    /// bitwise-identical to [`IncidenceIndex::arc_to_node`] for finite weights. `z[0..decls]` is zeroed.
    pub fn arc_to_node(&self, y: &[f64], z: &mut [f64]) {
        let n = self.ir.decl_nodes.len();
        for zi in z.iter_mut().take(n) {
            *zi = 0.0;
        }
        for d in 0..n {
            let (lo, hi) = self.data.range(DeclId::new(d));
            let mut acc = 0.0;
            for k in lo..hi {
                acc += self.coeff[k] * y[self.data.arcs[k].raw()];
            }
            z[d] = acc;
        }
    }

    /// Rayon `z = Bᵀ·y`: each node's output cell is written exactly once (disjoint partition, no atomics),
    /// so the result is bitwise-identical to the serial [`Self::arc_to_node`].
    pub fn par_arc_to_node(&self, y: &[f64], z: &mut [f64]) {
        use rayon::prelude::*;
        let n = self.ir.decl_nodes.len();
        let data = self.data;
        let coeff = &self.coeff;
        z[..n].par_iter_mut().enumerate().for_each(|(d, zi)| {
            let (lo, hi) = data.range(DeclId::new(d));
            let mut acc = 0.0;
            for k in lo..hi {
                acc += coeff[k] * y[data.arcs[k].raw()];
            }
            *zi = acc;
        });
    }

    /// `z = Bᵀ·B·x` — the signed star operator with the prepared `Bᵀ`. `Bx` (`node_to_arc`, arc-major) is
    /// the un-prepared fast path (deliberately not slowed); only the expensive `Bᵀ` half is prepared.
    /// `y_scratch` is caller-owned (len ≥ arcs); allocation-free.
    pub fn star_operator(&self, x: &[f64], z: &mut [f64], y_scratch: &mut [f64]) {
        self.data.node_to_arc(self.ir, x, y_scratch);
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
        idx.data.slots[1] = 0;
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
        idx.data.slots[0] = 2;
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
        idx.data.slots[0] = 99;
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
        idx.data.row.pop();
        assert!(matches!(idx.validate(), Err(CoordError::RowLength { .. })));
    }
}
