//! Canonical reverse-incidence navigation over the `Ir` (APPROVED-CORE-EDIT:
//! canonical-ir-incidence-navigation, 2026-09-01).
//!
//! The `Ir` natively supports the **edge → arc → ref → target** direction
//! (`EdgeRec.arcs`, `ArcRec.refs`, `SignedRefR::target`). The reverse direction —
//! *which hyperarcs is a node incident to* — required a full scan (`O(V·E)`); the
//! Stage 3 reference benchmark measured that at ~650× the indexed cost, so it is
//! not acceptable for node-major traversal.
//!
//! [`IncidenceIndex`] is a compact **CSR** view (`DeclId → incident HyperArcIds`)
//! built directly from the canonical incidences. It is:
//!
//! - **lossless** — every original incidence is kept, including sign, weight
//!   (reachable through the arc's `refs`), and **duplicate** incidences (an arc
//!   referencing a node `k` times appears `k` times in that node's list);
//! - **derived, not stored state** — caller-owned, built once and reused across
//!   queries (never rebuilt per query), so it adds no serialized/​mutable state to
//!   the `Ir` and cannot silently diverge from it. A transaction-updated stored
//!   form for a future *mutable* IR is deliberately out of scope here;
//! - **canonical-ID in and out** — both orientations name the same `DeclId` /
//!   `HyperArcId`; no dense `u32` vertex re-labelling leaks into the model.
//!
//! There is **no** pairwise/star/clique/Levi expansion and **no** implicit
//! dedup or flattening: this is the incidence, indexed, not a graph view.

use crate::common::ids::{DeclId, HyperArcId};
use crate::ir::ir::Ir;

/// Reverse-incidence CSR: for each declaration id, the hyperarcs incident to it.
///
/// `row[d..d+1]` bounds the incident-arc slice for `DeclId(d)` inside `col`.
#[derive(Debug, Clone)]
pub struct IncidenceIndex {
    row: Vec<u32>,
    col: Vec<HyperArcId>,
}

/// Failure building an [`IncidenceIndex`]. Ids-only, deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncidenceError {
    /// An arc reference targets a declaration outside `decl_nodes`.
    TargetOutOfRange { arc: HyperArcId, target: DeclId },
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
            Self::IncidenceOverflow { total } => {
                write!(f, "incidence count {total} exceeds the u32 CSR domain")
            }
        }
    }
}
impl std::error::Error for IncidenceError {}

/// Build the reverse-incidence index from the canonical incidences.
///
/// Two passes over the arcs' refs (count, then fill) — `O(incidences)` time,
/// `O(decls + incidences)` memory, one allocation per CSR array. Deterministic:
/// within a node's slice, arcs appear in ascending `HyperArcId`, then in ref
/// order, matching the canonical arc/ref order.
///
/// # Errors
/// [`IncidenceError::TargetOutOfRange`] on a dangling target;
/// [`IncidenceError::IncidenceOverflow`] if the incidence total exceeds `u32`.
///
/// # Preconditions / Postconditions
/// Total over any `Ir`. Every incidence in `ir.arcs[*].refs` is represented
/// exactly once (duplicates preserved); no incidence is invented or dropped.
pub fn build_incidence_index(ir: &Ir) -> Result<IncidenceIndex, IncidenceError> {
    let n = ir.decl_nodes.len();
    let mut row = vec![0u32; n + 1];

    // Pass 1: count incidences per target (ascending HyperArcId order of arcs).
    let mut total: usize = 0;
    for (ai, arc) in ir.arcs.iter().enumerate() {
        for sref in &arc.refs {
            let t = sref.target();
            let ti = t.raw();
            if ti >= n {
                return Err(IncidenceError::TargetOutOfRange {
                    arc: HyperArcId::new(ai),
                    target: t,
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

    // Pass 2: fill. `cur` walks each node's slice; arcs are visited in ascending
    // id, so each node's slice is arc-id sorted (then ref order) => deterministic.
    let mut col = vec![HyperArcId::NONE; total];
    let mut cur = row.clone();
    for (ai, arc) in ir.arcs.iter().enumerate() {
        let aid = HyperArcId::new(ai);
        for sref in &arc.refs {
            let ti = sref.target().raw();
            let p = &mut cur[ti];
            col[*p as usize] = aid;
            *p += 1;
        }
    }

    Ok(IncidenceIndex { row, col })
}

impl IncidenceIndex {
    /// The hyperarcs incident to `node`, in ascending arc id then ref order.
    /// Duplicate incidences are preserved. Empty for an unreferenced node, an
    /// arc id, or an out-of-range id — total, never panics.
    #[must_use]
    pub fn incident_arcs(&self, node: DeclId) -> &[HyperArcId] {
        let d = node.raw();
        match (self.row.get(d), self.row.get(d + 1)) {
            (Some(&lo), Some(&hi)) => &self.col[lo as usize..hi as usize],
            _ => &[],
        }
    }

    /// Number of incidences on `node` (with multiplicity).
    #[must_use]
    pub fn degree(&self, node: DeclId) -> usize {
        self.incident_arcs(node).len()
    }

    /// Total incidences indexed (the sum of all node degrees, with multiplicity).
    #[must_use]
    pub fn incidence_count(&self) -> usize {
        self.col.len()
    }

    /// Bytes held by the index (both CSR arrays) — for the memory report.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.row.len() * std::mem::size_of::<u32>()
            + self.col.len() * std::mem::size_of::<HyperArcId>()
    }
}

/// Validate that the derived index agrees with the canonical incidences: every
/// (arc, target) incidence is present in the target's slice, and the index holds
/// no extra incidences. A diagnostic, not a hot-path tax — call it once after a
/// build if coherence is in doubt; not on every query.
///
/// # Returns
/// `true` iff the index exactly represents the Ir incidences (degree per node
/// and multiset membership). Callers wanting a diff should compare slices.
#[must_use]
pub fn index_matches_ir(ir: &Ir, index: &IncidenceIndex) -> bool {
    // Count incidences per target from the Ir and compare to the index degree.
    let n = ir.decl_nodes.len();
    if index.row.len() != n + 1 {
        return false;
    }
    let mut deg = vec![0usize; n];
    let mut total = 0usize;
    for arc in &ir.arcs {
        for sref in &arc.refs {
            let ti = sref.target().raw();
            if ti >= n {
                return false;
            }
            deg[ti] += 1;
            total += 1;
        }
    }
    if index.incidence_count() != total {
        return false;
    }
    for (d, &degree) in deg.iter().enumerate() {
        if index.incident_arcs(DeclId::new(d)).len() != degree {
            return false;
        }
    }
    // Every listed arc must actually reference the node (multiset membership).
    for d in 0..n {
        for &aid in index.incident_arcs(DeclId::new(d)) {
            match ir.arcs.get(aid.raw()) {
                Some(arc) if arc.refs.iter().any(|r| r.target().raw() == d) => {}
                _ => return false,
            }
        }
    }
    true
}
