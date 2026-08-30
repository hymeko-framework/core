use crate::traversal::dense_index::{DenseEdgeIndex, DenseNodeIndex};
use crate::traversal::graphview::GraphView;
use hymeko::common::ids::HyperArcId;
use hymeko::common::ids::{DeclId, EdgeId, NodeId};
use hymeko::ir::ir::{DeclKind, Ir, SignedRefR};
use hymeko::tensor::aggregation::{AggCfg, agg_sign};
use hymeko::tensor::common::Real;
use hymeko::tensor::representations::tensor_coo::TensorInc;
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal, RefValueExtractor};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::marker::PhantomData;

/// A single incidence translated to view-local dense coordinates.
///
/// **Transient** — built during `try_from_ir`, consumed by the CSR builders, and
/// dropped. Never stored resident. Carries dense coordinates only, so every sort
/// key, offset, cursor and flat-array write downstream is representation-
/// independent (no canonical `raw()` reaches them).
#[cfg_attr(test, derive(Debug))]
struct DenseInc<V> {
    e: DenseEdgeIndex,
    n: DenseNodeIndex,
    s: i8,
    w: V,
}

/// Bipartite vertex of the hypergraph's incidence graph.
///
/// This is the **Levi graph** of a hypergraph H = (V, E) — the bipartite
/// graph on vertex set V ⊔ E whose edges encode incidence. `Levi` (1942),
/// `Berge` (Hypergraphs, 1973), and `König` representation are all names
/// for essentially this construction; our code uses `Berge*` identifiers
/// for historical reasons, with [`LeviState`] / [`LeviView`] re-exports
/// for readers who prefer the Levi-graph terminology.
/// Traversal state is **view-local**: it carries dense view coordinates
/// ([`DenseNodeIndex`]/[`DenseEdgeIndex`]), not canonical `NodeId`/`EdgeId`. Map
/// a state back to a canonical entity with the view's `dense_*` methods when
/// provenance is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BergeState {
    Node(DenseNodeIndex),
    Edge(DenseEdgeIndex),
}

/// Alias for [`BergeState`] using the Levi-graph naming convention.
pub type LeviState = BergeState;

pub enum BergeIter<'a> {
    Node(std::slice::Iter<'a, DenseEdgeIndex>),
    Edge(std::slice::Iter<'a, DenseNodeIndex>),
}

/// Alias for [`BergeIter`] using the Levi-graph naming convention.
pub type LeviIter<'a> = BergeIter<'a>;

impl<'a> Iterator for BergeIter<'a> {
    type Item = BergeState;
    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            BergeIter::Node(it) => it.next().map(|&e| BergeState::Edge(e)),
            BergeIter::Edge(it) => it.next().map(|&n| BergeState::Node(n)),
        }
    }
}

/// Why a [`HyperGraphView`] could not be built from an `Ir`.
///
/// Every variant means the `Ir` is structurally malformed — it names something
/// that is not there, or contradicts itself. None of them is reachable from a
/// well-formed `Ir`, so a caller that has validated its input (see
/// `hymeko::ir::coherence`) can treat these as impossible.
///
/// Carries ids only, never caller-supplied text, so a malformed `Ir` cannot
/// make the error echo a large payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewBuildError {
    /// An edge lists an arc that does not exist.
    DanglingArc { edge: EdgeId, arc: HyperArcId },
    /// A reference names a declaration that does not exist.
    DanglingTarget { arc: HyperArcId, target: DeclId },
    /// A reference names a `Node` declaration that has no node record.
    ///
    /// Distinct from a reference to an *Edge* declaration, which is a
    /// legitimate edge-to-edge reference this vertex-centric view has always
    /// skipped and which is **not** an error.
    MissingNodeMapping { target: DeclId },
    /// The `Ir` has more nodes than a `u32` dense view coordinate can address.
    /// Carries the bounded numeric count only.
    NodeCountOverflow { count: usize },
    /// The `Ir` has more edges than a `u32` dense view coordinate can address.
    EdgeCountOverflow { count: usize },
    /// An incidence names a `NodeId` that is not in the view's canonical→dense
    /// map (i.e. not one of the enumerated node records). Carries the raw id
    /// value only (bounded numeric), never source text.
    NodeNotInView { raw: usize },
    /// An incidence names an `EdgeId` that is not in the view's canonical→dense
    /// map. Carries the raw id value only.
    EdgeNotInView { raw: usize },
}

impl core::fmt::Display for ViewBuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DanglingArc { edge, arc } => write!(
                f,
                "edge {} lists arc {}, which does not exist",
                edge.raw(),
                arc.raw()
            ),
            Self::DanglingTarget { arc, target } => write!(
                f,
                "arc {} references declaration {}, which does not exist",
                arc.raw(),
                target.raw()
            ),
            Self::MissingNodeMapping { target } => write!(
                f,
                "declaration {} is a Node but has no node record",
                target.raw()
            ),
            Self::NodeCountOverflow { count } => write!(
                f,
                "node count {count} exceeds the u32 dense view-index domain"
            ),
            Self::EdgeCountOverflow { count } => write!(
                f,
                "edge count {count} exceeds the u32 dense view-index domain"
            ),
            Self::NodeNotInView { raw } => {
                write!(f, "node id {raw} is not mapped into this view")
            }
            Self::EdgeNotInView { raw } => {
                write!(f, "edge id {raw} is not mapped into this view")
            }
        }
    }
}

impl std::error::Error for ViewBuildError {}

pub struct HyperGraphView<V: IncVal<F>, EW: EdgeWeight<V, F>, F: Real>
where
    F: Real,
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
{
    /// Dense position → the canonical `NodeId` it came from. The real provenance,
    /// read by [`Self::dense_node_id`] — a dense coordinate is never turned back
    /// into a canonical id by arithmetic. Independent of any dense==raw
    /// coincidence, so it stays correct if canonical ids become non-contiguous,
    /// non-zero-based, permuted, or generational.
    pub node_canonical: Vec<NodeId>,
    /// Dense position → the canonical `EdgeId` it came from. See `node_canonical`.
    pub edge_canonical: Vec<EdgeId>,
    /// Dense **position** → declaration. (Not "NodeId → DeclId": the index is a
    /// dense view coordinate, not a canonical id.)
    pub node_decl: Vec<DeclId>,
    /// Dense **position** → declaration. See `node_decl`.
    pub edge_decl: Vec<DeclId>,
    /// Flat array of all incident edges, as **dense view coordinates**.
    pub flat_node_edges: Vec<DenseEdgeIndex>,
    /// Parallel array: sign/weight info per node-edge incidence (aligned with flat_node_edges)
    pub flat_node_sign: Vec<i8>,
    /// Offsets into flat_node_edges for each NodeId.
    pub node_offsets: Vec<usize>,
    /// Flat array of all incident nodes, as **dense view coordinates**.
    pub flat_edge_nodes: Vec<DenseNodeIndex>,
    /// Parallel array: sign per edge-node incidence (aligned with flat_edge_nodes)
    pub flat_edge_sign: Vec<i8>,
    /// Offsets into flat_edge_nodes for each EdgeId.
    pub edge_offsets: Vec<usize>,
    /// Optional weights (e.g., for GNNs), aligned with the corresponding flat arrays.
    pub flat_node_w: Vec<V>, // aligned with flat_node_edges
    pub flat_edge_w: Vec<V>,  // aligned with flat_edge_nodes
    pub edge_weight: Vec<EW>, // per EdgeId (global)
    /// This PhantomData is needed to tell the compiler that HyperGraphView is generic over F,
    /// even though it doesn't directly contain any F values.
    /// This allows us to use F in trait bounds and method signatures without causing compilation errors about unused type parameters.
    /// (sucks, I know)
    pub _phantom: core::marker::PhantomData<F>,
}

impl<V, EW, F> HyperGraphView<V, EW, F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    const PAR_SORT_THRESHOLD: usize = 8192;
    #[inline(always)]
    fn sign_of(r: &SignedRefR) -> i8 {
        match r {
            SignedRefR::Plus(_) => 1,
            SignedRefR::Minus(_) => -1,
            SignedRefR::Neutral(_) => 0,
        }
    }

    /// Number of nodes in the view.
    #[inline(always)]
    pub fn num_nodes(&self) -> usize {
        // node_offsets has length = num_nodes + 1
        self.node_offsets.len().saturating_sub(1)
    }

    /// Number of edges in the view.
    #[inline(always)]
    pub fn num_edges(&self) -> usize {
        self.edge_offsets.len().saturating_sub(1)
    }

    /// Every dense node coordinate in the view, `0..num_nodes`.
    #[inline]
    pub fn node_indices(&self) -> impl Iterator<Item = DenseNodeIndex> {
        (0..self.num_nodes() as u32).map(DenseNodeIndex::from_u32)
    }

    /// Every dense edge coordinate in the view, `0..num_edges`.
    #[inline]
    pub fn edge_indices(&self) -> impl Iterator<Item = DenseEdgeIndex> {
        (0..self.num_edges() as u32).map(DenseEdgeIndex::from_u32)
    }

    /// Returns the `[start, end)` range for a node's incident edges, by **dense
    /// coordinate**. This is a view-local array offset — not a canonical id.
    #[inline(always)]
    pub fn node_span(&self, nid: DenseNodeIndex) -> (usize, usize) {
        let s = self.node_offsets[nid.as_usize()];
        let e = self.node_offsets[nid.as_usize() + 1];
        (s, e)
    }

    /// Returns the `[start, end)` range for an edge's incident nodes, by **dense
    /// coordinate**.
    #[inline(always)]
    pub fn edge_span(&self, eid: DenseEdgeIndex) -> (usize, usize) {
        let s = self.edge_offsets[eid.as_usize()];
        let e = self.edge_offsets[eid.as_usize() + 1];
        (s, e)
    }

    /// Borrow the dense incident-edge coordinates for a dense node coordinate.
    #[inline(always)]
    pub fn node_edges(&self, nid: DenseNodeIndex) -> &[DenseEdgeIndex] {
        let (s, e) = self.node_span(nid);
        &self.flat_node_edges[s..e]
    }

    /// Borrow the dense incident-node coordinates for a dense edge coordinate.
    #[inline(always)]
    pub fn edge_nodes(&self, eid: DenseEdgeIndex) -> &[DenseNodeIndex] {
        let (s, e) = self.edge_span(eid);
        &self.flat_edge_nodes[s..e]
    }

    // ── dense → canonical provenance ────────────────────────────────────────
    //
    // These read the provenance vectors captured at construction. They do NOT
    // reconstruct a canonical id from a dense coordinate (the former
    // `NodeId::new(dense)` was only correct by the accidental dense==raw
    // coincidence). A dense coordinate must never be persisted or used to address
    // the canonical `Ir` directly — map it through here first.

    /// The canonical `NodeId` a dense node coordinate came from (read from the
    /// stored provenance).
    #[inline]
    #[must_use]
    pub fn dense_node_id(&self, nid: DenseNodeIndex) -> NodeId {
        self.node_canonical[nid.as_usize()]
    }

    /// The canonical `EdgeId` a dense edge coordinate came from (read from the
    /// stored provenance).
    #[inline]
    #[must_use]
    pub fn dense_edge_id(&self, eid: DenseEdgeIndex) -> EdgeId {
        self.edge_canonical[eid.as_usize()]
    }

    /// The declaration a dense node coordinate came from.
    #[inline]
    #[must_use]
    pub fn dense_node_decl(&self, nid: DenseNodeIndex) -> DeclId {
        self.node_decl[nid.as_usize()]
    }

    /// The declaration a dense edge coordinate came from.
    #[inline]
    #[must_use]
    pub fn dense_edge_decl(&self, eid: DenseEdgeIndex) -> DeclId {
        self.edge_decl[eid.as_usize()]
    }

    fn try_collect_triples<E>(ir: &Ir, ex: &E) -> Result<Vec<TensorInc<F, V>>, ViewBuildError>
    where
        E: RefValueExtractor<F, V>,
    {
        let mut out = Vec::with_capacity(ir.arc_count() * 2);

        // M2b: dense scan through the typed-id iterator, which yields the
        // `EdgeId` directly instead of reconstructing it from an enumeration
        // index. Measured identical to raw `Vec` iteration (M2a.1 SCAN gate).
        for (eid, edge) in ir.edge_records() {
            for &aid in &edge.arcs {
                // M2b-corrective: an arc an edge claims to own but which does
                // not exist is malformed input, not something to skip. Skipping
                // silently produced a view with fewer incidences than the IR
                // describes, and nothing downstream could tell.
                // `let / else`, not `ok_or`: `ok_or` takes its argument by value and
                // would therefore build the error struct on every successful
                // iteration too. Error construction belongs on the malformed
                // path only.
                let Some(arc) = ir.arc(aid) else {
                    return Err(ViewBuildError::DanglingArc {
                        edge: eid,
                        arc: aid,
                    });
                };
                for r in &arc.refs {
                    let (target, _sgn) = match r {
                        SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => {
                            (a.target, Self::sign_of(r))
                        }
                    };
                    // M2b: MAP-ONLY read. `as_node` reports the mapping and
                    // nothing more — deliberately NOT `node_for_decl`, which
                    // would additionally resolve the record and validate its
                    // back-link. Substituting it here would change both the
                    // work done and the meaning on malformed input, and is the
                    // substitution the migration rules explicitly forbid.
                    //
                    // Absence has three causes and only one is legitimate:
                    //
                    //   Edge / HyperArc declaration -> legitimate. This view is
                    //     vertex-centric and has always skipped edge-to-edge
                    //     references.
                    //   Node declaration, no record -> MissingNodeMapping.
                    //   Not in the declaration table -> DanglingTarget.
                    //
                    // The last case used to be excused whenever `decl_count()`
                    // was zero, on the argument that an `Ir` with no declaration
                    // table cannot serve as an existence oracle. The only
                    // producer of such an `Ir` was a synthetic fixture that
                    // should never have been incoherent; it is now built
                    // through `HyperSpec::program_with_weights`, which fills
                    // every table. Keeping the escape would have left
                    // `UIR-M2B-06` — malformed IR is never silently accepted —
                    // false for the one input shape that reached it.
                    //
                    // One lookup, not two. Classification runs only when the
                    // mapping is absent; the successful path does exactly the
                    // work it did before this phase.
                    let Some(nid) = ir.as_node(target) else {
                        match ir.kind_of(target) {
                            Some(DeclKind::Node) => {
                                return Err(ViewBuildError::MissingNodeMapping { target });
                            }
                            Some(_) => continue, // edge-to-edge reference: skip, as always
                            None => {
                                return Err(ViewBuildError::DanglingTarget { arc: aid, target });
                            }
                        }
                    };

                    let sgn = Self::sign_of(r);
                    let v = ex.value_of(r);
                    out.push(TensorInc {
                        e: eid,
                        n: nid,
                        s: sgn,
                        w: v,
                        _pd: PhantomData,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Translate canonical incidences to dense view coordinates through the
    /// explicit maps. A canonical id absent from a map is a **typed error** —
    /// never a panic, a silent drop, or a default coordinate. The maps are
    /// transient (construction-only); nothing here is stored resident.
    fn translate_to_dense(
        triples: Vec<TensorInc<F, V>>,
        node_map: &FxHashMap<NodeId, DenseNodeIndex>,
        edge_map: &FxHashMap<EdgeId, DenseEdgeIndex>,
    ) -> Result<Vec<DenseInc<V>>, ViewBuildError> {
        let mut out = Vec::with_capacity(triples.len());
        for t in triples {
            let n = *node_map
                .get(&t.n)
                .ok_or(ViewBuildError::NodeNotInView { raw: t.n.raw() })?;
            let e = *edge_map
                .get(&t.e)
                .ok_or(ViewBuildError::EdgeNotInView { raw: t.e.raw() })?;
            out.push(DenseInc {
                e,
                n,
                s: t.s,
                w: t.w,
            });
        }
        Ok(out)
    }

    fn sort_reduce_pairs(mut triples: Vec<DenseInc<V>>, cfg: &AggCfg) -> Vec<DenseInc<V>> {
        // Sort by (dense edge, dense node) — dense view coordinates, not canonical
        // ids. For a full view this yields the same order as the old raw-id sort
        // (because the enumeration assigns dense == position), but the key is now
        // representation-independent.
        if triples.len() >= Self::PAR_SORT_THRESHOLD {
            triples.par_sort_unstable_by_key(|x| (x.e.as_u32(), x.n.as_u32()));
        } else {
            triples.sort_unstable_by_key(|x| (x.e.as_u32(), x.n.as_u32()));
        }

        // reduce/dedup
        let mut out: Vec<DenseInc<V>> = Vec::with_capacity(triples.len());
        for inc in triples {
            if let Some(last) = out.last_mut()
                && last.e == inc.e
                && last.n == inc.n
            {
                let new_w = V::agg(cfg, &last.w, &inc.w);
                let wa = last.w.as_scalar();
                let wb = inc.w.as_scalar();
                let new_s = agg_sign(cfg, last.s, inc.s, wa, wb);
                last.w = new_w;
                last.s = new_s;
            } else {
                out.push(inc);
            }
        }
        out
    }

    fn build_csr_edge_to_nodes(
        pairs: &[DenseInc<V>],
        num_edges: usize,
    ) -> (Vec<DenseNodeIndex>, Vec<i8>, Vec<V>, Vec<usize>) {
        // CSR: Edge -> Nodes. Everything here is a dense view coordinate; no
        // canonical id and no `raw()` appear. `pairs` are already dense
        // (translated through the checked map in `try_from_ir`).
        let mut flat_nodes = Vec::with_capacity(pairs.len());
        let mut flat_sign = Vec::with_capacity(pairs.len());
        let mut flat_w = Vec::with_capacity(pairs.len());
        let mut offsets = vec![0usize; num_edges + 1];

        for entry in pairs {
            flat_nodes.push(entry.n);
            flat_sign.push(entry.s);
            flat_w.push(entry.w.clone());
            offsets[entry.e.as_usize() + 1] += 1;
        }
        for i in 0..num_edges {
            offsets[i + 1] += offsets[i];
        }

        (flat_nodes, flat_sign, flat_w, offsets)
    }

    /// Build the node-first CSR by **counting sort**, not by re-sorting.
    ///
    /// # Preconditions
    /// `pairs` is ordered by `(edge, node)` and deduplicated — i.e. exactly
    /// what [`Self::sort_reduce_pairs`] returns.
    ///
    /// # Postconditions
    /// Returns the same arrays the previous `sort_unstable_by_key(|x| (x.n.raw(),
    /// x.e.raw()))` implementation returned, element for element.
    ///
    /// Why that holds: counting sort is **stable**, so scattering `pairs` into
    /// per-node buckets in input order leaves each node's slice in increasing
    /// edge order. Combined with the bucket ordering by node, that is precisely
    /// `(node, edge)` order — and since the keys are unique after
    /// deduplication, the sorted order is unique, so the comparison sort could
    /// not have produced anything else.
    ///
    /// This replaces an `O(nnz log nnz)` comparison sort (parallel above
    /// `PAR_SORT_THRESHOLD`) with an `O(nnz + num_nodes)` pass, and no longer
    /// needs `&mut` access to `pairs`. The `(node, edge)` ordering it produces
    /// is load-bearing beyond construction: the parallel scatter in
    /// `tensor::message_passing` depends on it for bitwise-identical results.
    fn build_csr_node_to_edges(
        pairs: &[DenseInc<V>],
        num_nodes: usize,
    ) -> (Vec<DenseEdgeIndex>, Vec<i8>, Vec<V>, Vec<usize>)
    where
        V: IncVal<F>,
    {
        let nnz = pairs.len();

        // Dense only: `entry.n`/`entry.e` are view coordinates, no `raw()`.
        let mut offsets = vec![0usize; num_nodes + 1];
        for entry in pairs {
            offsets[entry.n.as_usize() + 1] += 1;
        }
        for i in 0..num_nodes {
            offsets[i + 1] += offsets[i];
        }

        let mut flat_edges = vec![DenseEdgeIndex::from_u32(0); nnz];
        let mut flat_sign = vec![0i8; nnz];
        let mut flat_w = vec![V::zero(); nnz];

        // Running write cursor per node; starts at each bucket's base.
        let mut cursor = offsets.clone();
        for entry in pairs {
            let slot = cursor[entry.n.as_usize()];
            flat_edges[slot] = entry.e;
            flat_sign[slot] = entry.s;
            flat_w[slot] = entry.w.clone();
            cursor[entry.n.as_usize()] = slot + 1;
        }

        (flat_edges, flat_sign, flat_w, offsets)
    }

    /// Build incidence tables from IR (single source of truth).
    ///
    /// # Panics
    ///
    /// Panics if the `Ir` is structurally malformed — a dangling arc, a
    /// reference to a declaration that does not exist, or a `Node` declaration
    /// with no node record. This preserves the pre-M2b fail-fast contract for
    /// existing callers.
    ///
    /// **New code should call [`HyperGraphView::try_from_ir`]**, which reports
    /// the same conditions as a typed [`ViewBuildError`] instead. This wrapper
    /// exists for source compatibility and delegates to it.
    pub fn from_ir<E>(ir: &Ir, cfg: &AggCfg, ex: &E) -> Self
    where
        E: RefValueExtractor<F, V>,
    {
        match Self::try_from_ir(ir, cfg, ex) {
            Ok(v) => v,
            Err(e) => panic!("HyperGraphView::from_ir on a malformed Ir: {e}"),
        }
    }

    /// Build incidence tables from IR, reporting malformed input.
    ///
    /// # Errors
    ///
    /// Returns [`ViewBuildError`] when the `Ir` names something that does not
    /// exist or contradicts itself. A well-formed `Ir` never produces one, and
    /// the result is bit-identical to [`HyperGraphView::from_ir`] — the checked
    /// path adds validation, not different arithmetic or ordering.
    ///
    /// A reference to an *Edge* declaration is **not** an error: this view is
    /// vertex-centric and has always skipped edge-to-edge references.
    pub fn try_from_ir<E>(ir: &Ir, cfg: &AggCfg, ex: &E) -> Result<Self, ViewBuildError>
    where
        E: RefValueExtractor<F, V>,
    {
        // M2b: live-entity counts and typed-id scans.
        let num_nodes = ir.node_count();
        let num_edges = ir.edge_count();

        // M2c.1: dense view coordinates are u32. Reject a graph too large to
        // address densely *before* allocating any incidence array.
        if u32::try_from(num_nodes).is_err() {
            return Err(ViewBuildError::NodeCountOverflow { count: num_nodes });
        }
        if u32::try_from(num_edges).is_err() {
            return Err(ViewBuildError::EdgeCountOverflow { count: num_edges });
        }

        // M2c.1 corrective: capture the *real* canonical provenance by enumerating
        // the node/edge records, and build an explicit, checked canonical→dense
        // map. The dense coordinate of a record is its enumeration position — not
        // its `raw()`. These provenance vectors and maps make construction correct
        // even if canonical ids are non-contiguous, non-zero-based, permuted, or
        // later generational.
        let node_canonical: Vec<NodeId> = ir.node_records().map(|(nid, _)| nid).collect();
        let edge_canonical: Vec<EdgeId> = ir.edge_records().map(|(eid, _)| eid).collect();
        let node_decl: Vec<DeclId> = ir.node_records().map(|(_, n)| n.decl).collect();
        let edge_decl: Vec<DeclId> = ir.edge_records().map(|(_, e)| e.decl).collect();

        // Transient (construction-only) canonical→dense maps. Not stored: no
        // resident reverse map exists until a consumer measurably needs one.
        let node_map: FxHashMap<NodeId, DenseNodeIndex> = node_canonical
            .iter()
            .enumerate()
            .map(|(i, &nid)| (nid, DenseNodeIndex::from_u32(i as u32)))
            .collect();
        let edge_map: FxHashMap<EdgeId, DenseEdgeIndex> = edge_canonical
            .iter()
            .enumerate()
            .map(|(i, &eid)| (eid, DenseEdgeIndex::from_u32(i as u32)))
            .collect();

        // Collect canonical incidences, then translate every one to dense through
        // the explicit map *before* any sort/offset/cursor/fill.
        let triples = Self::try_collect_triples(ir, ex)?;
        let dense = Self::translate_to_dense(triples, &node_map, &edge_map)?;

        // sort by (dense edge, dense node)
        let pairs = Self::sort_reduce_pairs(dense, cfg);

        // Edge -> Nodes CSR (dense only)
        let (flat_edge_nodes, flat_edge_sign, flat_edge_w, edge_offsets) =
            Self::build_csr_edge_to_nodes(&pairs, num_edges);

        // Node -> Edges CSR, by counting sort over the same (edge, node)-ordered
        // pairs -- no second sort, and `pairs` stays immutable.
        let (flat_node_edges, flat_node_sign, flat_node_w, node_offsets) =
            Self::build_csr_node_to_edges(&pairs, num_nodes);

        Ok(Self {
            node_canonical,
            edge_canonical,
            node_decl,
            edge_decl,
            flat_node_edges,
            flat_node_sign,
            flat_node_w,
            node_offsets,
            flat_edge_nodes,
            flat_edge_sign,
            flat_edge_w,
            edge_offsets,
            edge_weight: vec![EW::one(); num_edges],
            _phantom: PhantomData,
        })
    }
}

/// Bipartite incidence view over a [`HyperGraphView`] — the **Levi graph**
/// of the hypergraph. Walks between `BergeState::Node(_)` and
/// `BergeState::Edge(_)` alternate, giving classical graph algorithms a
/// uniform notion of "neighbour" without flattening the hypergraph into a
/// lossy 2-section.
///
/// The name `Berge` is kept for historical reasons; [`LeviView`] is a type
/// alias for readers who prefer the Levi-graph terminology.
pub struct BergeView<'a, V, EW, F>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    pub hg: &'a HyperGraphView<V, EW, F>,
}

/// Alias for [`BergeView`] using the Levi-graph naming convention.
pub type LeviView<'a, V, EW, F> = BergeView<'a, V, EW, F>;

impl<'a, V, EW, F> GraphView for BergeView<'a, V, EW, F>
where
    F: Real,
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
{
    type Node = BergeState;
    type NeighIter<'b>
        = BergeIter<'b>
    where
        Self: 'b;

    #[inline(always)]
    fn neighbors<'b>(&'b self, s: BergeState) -> Self::NeighIter<'b> {
        match s {
            BergeState::Node(nid) => {
                let start = self.hg.node_offsets[nid.as_usize()];
                let end = self.hg.node_offsets[nid.as_usize() + 1];
                BergeIter::Node(self.hg.flat_node_edges[start..end].iter())
            }
            BergeState::Edge(eid) => {
                let start = self.hg.edge_offsets[eid.as_usize()];
                let end = self.hg.edge_offsets[eid.as_usize() + 1];
                BergeIter::Edge(self.hg.flat_edge_nodes[start..end].iter())
            }
        }
    }
}

#[cfg(test)]
mod corrective_tests {
    use super::*;
    use hymeko::tensor::tensor_val::EdgeWScalar;

    type V = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

    fn inc(e: usize, n: usize) -> TensorInc<f32, f32> {
        TensorInc {
            e: EdgeId::new(e),
            n: NodeId::new(n),
            s: 1,
            w: 1.0f32,
            _pd: PhantomData,
        }
    }

    #[test]
    fn translate_reports_missing_node_mapping_typed_no_panic() {
        // A canonical node id absent from the map is a typed error — never a
        // panic, silent drop, or default coordinate.
        let node_map: FxHashMap<NodeId, DenseNodeIndex> = FxHashMap::default();
        let mut edge_map: FxHashMap<EdgeId, DenseEdgeIndex> = FxHashMap::default();
        edge_map.insert(EdgeId::new(0), DenseEdgeIndex::from_u32(0));
        let err = V::translate_to_dense(vec![inc(0, 5)], &node_map, &edge_map).unwrap_err();
        assert!(matches!(err, ViewBuildError::NodeNotInView { raw: 5 }));
    }

    #[test]
    fn translate_reports_missing_edge_mapping_typed_no_panic() {
        let mut node_map: FxHashMap<NodeId, DenseNodeIndex> = FxHashMap::default();
        node_map.insert(NodeId::new(5), DenseNodeIndex::from_u32(0));
        let edge_map: FxHashMap<EdgeId, DenseEdgeIndex> = FxHashMap::default();
        let err = V::translate_to_dense(vec![inc(7, 5)], &node_map, &edge_map).unwrap_err();
        assert!(matches!(err, ViewBuildError::EdgeNotInView { raw: 7 }));
    }

    #[test]
    fn translate_maps_non_contiguous_ids_to_dense_positions() {
        // Non-contiguous canonical ids -> dense 0..n by map position, not by raw.
        let mut node_map: FxHashMap<NodeId, DenseNodeIndex> = FxHashMap::default();
        node_map.insert(NodeId::new(700), DenseNodeIndex::from_u32(0));
        node_map.insert(NodeId::new(3), DenseNodeIndex::from_u32(1));
        let mut edge_map: FxHashMap<EdgeId, DenseEdgeIndex> = FxHashMap::default();
        edge_map.insert(EdgeId::new(42), DenseEdgeIndex::from_u32(0));
        let out =
            V::translate_to_dense(vec![inc(42, 700), inc(42, 3)], &node_map, &edge_map).unwrap();
        assert_eq!(out[0].n, DenseNodeIndex::from_u32(0)); // NodeId(700) -> dense 0
        assert_eq!(out[1].n, DenseNodeIndex::from_u32(1)); // NodeId(3)   -> dense 1
        assert_eq!(out[0].e, DenseEdgeIndex::from_u32(0));
    }
}
