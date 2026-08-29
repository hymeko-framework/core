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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BergeState {
    Node(NodeId),
    Edge(EdgeId),
}

/// Alias for [`BergeState`] using the Levi-graph naming convention.
pub type LeviState = BergeState;

pub enum BergeIter<'a> {
    Node(std::slice::Iter<'a, EdgeId>),
    Edge(std::slice::Iter<'a, NodeId>),
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
}

impl core::fmt::Display for ViewBuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DanglingArc { edge, arc } => write!(
                f,
                "edge {} lists arc {}, which does not exist",
                edge.0, arc.0
            ),
            Self::DanglingTarget { arc, target } => write!(
                f,
                "arc {} references declaration {}, which does not exist",
                arc.0, target.0
            ),
            Self::MissingNodeMapping { target } => write!(
                f,
                "declaration {} is a Node but has no node record",
                target.0
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
    /// Flat array of all incident edges.
    pub flat_node_edges: Vec<EdgeId>,
    /// Parallel array: sign/weight info per node-edge incidence (aligned with flat_node_edges)
    pub flat_node_sign: Vec<i8>,
    /// Offsets into flat_node_edges for each NodeId.
    pub node_offsets: Vec<usize>,
    /// Flat array of all incident nodes.
    pub flat_edge_nodes: Vec<NodeId>,
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

    /// Returns the [start, end) range for a node's incident edges.
    #[inline(always)]
    pub fn node_span(&self, nid: NodeId) -> (usize, usize) {
        let s = self.node_offsets[nid.0];
        let e = self.node_offsets[nid.0 + 1];
        (s, e)
    }

    /// Returns the [start, end) range for an edge's incident nodes.
    #[inline(always)]
    pub fn edge_span(&self, eid: EdgeId) -> (usize, usize) {
        let s = self.edge_offsets[eid.0];
        let e = self.edge_offsets[eid.0 + 1];
        (s, e)
    }

    /// Borrow the slice of incident edges for a given node.
    #[inline(always)]
    pub fn node_edges(&self, nid: NodeId) -> &[EdgeId] {
        let (s, e) = self.node_span(nid);
        &self.flat_node_edges[s..e]
    }

    /// Borrow the slice of incident nodes for a given edge.
    #[inline(always)]
    pub fn edge_nodes(&self, eid: EdgeId) -> &[NodeId] {
        let (s, e) = self.edge_span(eid);
        &self.flat_edge_nodes[s..e]
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
                    // M2b-corrective: absence here has three causes and only two
                    // are errors.
                    //
                    //   Edge / HyperArc declaration -> legitimate. This view is
                    //     vertex-centric and has always skipped edge-to-edge
                    //     references.
                    //   Node declaration, no record -> MissingNodeMapping.
                    //   Not in the declaration table at all -> DanglingTarget,
                    //     but ONLY when that table can answer the question.
                    //
                    // The last qualification is not a convenience. This consumer
                    // resolves references through `decl_to_node`; it has never
                    // read `decl_nodes`. An `Ir` whose declaration table is
                    // empty therefore cannot serve as an existence oracle here,
                    // and treating every reference in one as dangling would
                    // invent a requirement `from_ir` never had.
                    //
                    // `hymeko_hnn::workloads::HyperSpec::ir()` builds exactly
                    // such a partial `Ir`: nodes, edges, arcs and `decl_to_node`
                    // are populated, `decl_nodes` is left empty. That is a
                    // pre-existing defect in a synthetic fixture — the coherence
                    // validator would reject it — and it is recorded, not
                    // repaired here.
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
                            None if ir.decl_count() > 0 => {
                                return Err(ViewBuildError::DanglingTarget { arc: aid, target });
                            }
                            None => continue, // no declaration table to consult
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
            triples.par_sort_unstable_by_key(|x| (x.e.0, x.n.0));
        } else {
            triples.sort_unstable_by_key(|x| (x.e.0, x.n.0));
        }

        // reduce/dedup
        let mut out: Vec<TensorInc<F, V>> = Vec::with_capacity(triples.len());
        for inc in triples {
            if let Some(last) = out.last_mut()
                && last.e.0 == inc.e.0
                && last.n.0 == inc.n.0
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
    ) -> (Vec<NodeId>, Vec<i8>, Vec<V>, Vec<usize>) {
        // CSR: Edge -> Nodes
        let mut flat_nodes = Vec::with_capacity(pairs.len());
        let mut flat_sign = Vec::with_capacity(pairs.len());
        let mut flat_w = Vec::with_capacity(pairs.len());
        let mut offsets = vec![0usize; num_edges + 1];

        for entry in pairs {
            flat_nodes.push(entry.n);
            flat_sign.push(entry.s);
            flat_w.push(entry.w.clone());
            offsets[entry.e.0 + 1] += 1;
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
    /// Returns the same arrays the previous `sort_unstable_by_key(|x| (x.n.0,
    /// x.e.0))` implementation returned, element for element.
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
    ) -> (Vec<EdgeId>, Vec<i8>, Vec<V>, Vec<usize>)
    where
        V: IncVal<F>,
    {
        let nnz = pairs.len();

        let mut offsets = vec![0usize; num_nodes + 1];
        for entry in pairs {
            offsets[entry.n.0 + 1] += 1;
        }
        for i in 0..num_nodes {
            offsets[i + 1] += offsets[i];
        }

        let mut flat_edges = vec![EdgeId::new(0); nnz];
        let mut flat_sign = vec![0i8; nnz];
        let mut flat_w = vec![V::zero(); nnz];

        // Running write cursor per node; starts at each bucket's base.
        let mut cursor = offsets.clone();
        for entry in pairs {
            let slot = cursor[entry.n.0];
            flat_edges[slot] = entry.e;
            flat_sign[slot] = entry.s;
            flat_w[slot] = entry.w.clone();
            cursor[entry.n.0] = slot + 1;
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
                let start = self.hg.node_offsets[nid.0];
                let end = self.hg.node_offsets[nid.0 + 1];
                BergeIter::Node(self.hg.flat_node_edges[start..end].iter())
            }
            BergeState::Edge(eid) => {
                let start = self.hg.edge_offsets[eid.0];
                let end = self.hg.edge_offsets[eid.0 + 1];
                BergeIter::Edge(self.hg.flat_edge_nodes[start..end].iter())
            }
        }
    }
}
