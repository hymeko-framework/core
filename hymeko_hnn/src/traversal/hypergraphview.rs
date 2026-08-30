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
use std::marker::PhantomData;

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
    pub node_decl: Vec<DeclId>, // NodeId -> DeclId
    pub edge_decl: Vec<DeclId>, // EdgeId -> DeclId
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
    // This view enumerates every node/edge of the `Ir` in canonical id order, so
    // a dense coordinate `i` corresponds to canonical `NodeId::new(i)` /
    // `EdgeId::new(i)`. That 1:1 relationship is a property of *this* full view;
    // it is stated explicitly here so no numerical code re-derives it by treating
    // a canonical `raw()` as an array offset. A dense coordinate must never be
    // persisted or used to address the canonical `Ir` directly — map it first.

    /// The canonical `NodeId` a dense node coordinate came from.
    #[inline]
    #[must_use]
    pub fn dense_node_id(&self, nid: DenseNodeIndex) -> NodeId {
        NodeId::new(nid.as_usize())
    }

    /// The canonical `EdgeId` a dense edge coordinate came from.
    #[inline]
    #[must_use]
    pub fn dense_edge_id(&self, eid: DenseEdgeIndex) -> EdgeId {
        EdgeId::new(eid.as_usize())
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

    fn sort_reduce_pairs(mut triples: Vec<TensorInc<F, V>>, cfg: &AggCfg) -> Vec<TensorInc<F, V>> {
        if triples.len() >= Self::PAR_SORT_THRESHOLD {
            triples.par_sort_unstable_by_key(|x| (x.e.raw(), x.n.raw()));
        } else {
            triples.sort_unstable_by_key(|x| (x.e.raw(), x.n.raw()));
        }

        // reduce/dedup
        let mut out: Vec<TensorInc<F, V>> = Vec::with_capacity(triples.len());
        for inc in triples {
            if let Some(last) = out.last_mut()
                && last.e.raw() == inc.e.raw()
                && last.n.raw() == inc.n.raw()
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
        pairs: &[TensorInc<F, V>],
        num_edges: usize,
    ) -> (Vec<DenseNodeIndex>, Vec<i8>, Vec<V>, Vec<usize>) {
        // CSR: Edge -> Nodes. This is the representation-owner boundary where the
        // canonical node order *defines* the dense coordinate: `entry.n` is a
        // canonical `NodeId` and its `raw()` is the dense node index (the view is
        // 1:1 with the `Ir` node table). `try_from_ir` has already checked the
        // node count fits `u32`, so the narrowing cannot truncate.
        let mut flat_nodes = Vec::with_capacity(pairs.len());
        let mut flat_sign = Vec::with_capacity(pairs.len());
        let mut flat_w = Vec::with_capacity(pairs.len());
        let mut offsets = vec![0usize; num_edges + 1];

        for entry in pairs {
            debug_assert!(
                entry.n.raw() <= u32::MAX as usize,
                "node count checked in try_from_ir"
            );
            flat_nodes.push(DenseNodeIndex::from_u32(entry.n.raw() as u32));
            flat_sign.push(entry.s);
            flat_w.push(entry.w.clone());
            offsets[entry.e.raw() + 1] += 1;
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
        pairs: &[TensorInc<F, V>],
        num_nodes: usize,
    ) -> (Vec<DenseEdgeIndex>, Vec<i8>, Vec<V>, Vec<usize>)
    where
        V: IncVal<F>,
    {
        let nnz = pairs.len();

        let mut offsets = vec![0usize; num_nodes + 1];
        for entry in pairs {
            offsets[entry.n.raw() + 1] += 1;
        }
        for i in 0..num_nodes {
            offsets[i + 1] += offsets[i];
        }

        // Representation-owner boundary: `entry.e` (canonical `EdgeId`) defines the
        // dense edge coordinate; the edge count is checked to fit `u32` upfront.
        let mut flat_edges = vec![DenseEdgeIndex::from_u32(0); nnz];
        let mut flat_sign = vec![0i8; nnz];
        let mut flat_w = vec![V::zero(); nnz];

        // Running write cursor per node; starts at each bucket's base.
        let mut cursor = offsets.clone();
        for entry in pairs {
            let slot = cursor[entry.n.raw()];
            debug_assert!(
                entry.e.raw() <= u32::MAX as usize,
                "edge count checked in try_from_ir"
            );
            flat_edges[slot] = DenseEdgeIndex::from_u32(entry.e.raw() as u32);
            flat_sign[slot] = entry.s;
            flat_w[slot] = entry.w.clone();
            cursor[entry.n.raw()] = slot + 1;
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

        let node_decl: Vec<DeclId> = ir.node_records().map(|(_, n)| n.decl).collect();
        let edge_decl: Vec<DeclId> = ir.edge_records().map(|(_, e)| e.decl).collect();

        let triples = Self::try_collect_triples(ir, ex)?;

        // sort by (eid,nid)
        let pairs = Self::sort_reduce_pairs(triples, cfg);

        // Edge -> Nodes CSR
        let (flat_edge_nodes, flat_edge_sign, flat_edge_w, edge_offsets) =
            Self::build_csr_edge_to_nodes(&pairs, num_edges);

        // Node -> Edges CSR, by counting sort over the same (edge, node)-ordered
        // pairs -- no second sort, and `pairs` stays immutable.
        let (flat_node_edges, flat_node_sign, flat_node_w, node_offsets) =
            Self::build_csr_node_to_edges(&pairs, num_nodes);

        Ok(Self {
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
