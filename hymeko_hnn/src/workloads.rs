//! Deterministic synthetic workloads for benchmarks and correctness tests.
//!
//! One home for the workload construction used by
//! `hymeko_core/benches/csr_bench.rs`, `hymeko_hnn/benches/hnn_bench.rs`, and
//! the tensor regression tests in `hymeko_core/tests/`. Three consumers across
//! two crates is exactly the case CLAUDE.md §6.1 says to unify rather than
//! copy, and a `#[path]`-shared bench helper cannot cross a crate boundary.
//!
//! This module deliberately does **not** re-implement hypergraph generation.
//! [`hymeko::generators`] already owns the canonical families (Steiner triple
//! systems, sunflowers, complete `r`-uniform) and the [`HypergraphDesign`]
//! interchange type, and its module docs state that higher layers should
//! *adapt* a design rather than re-implement one. What is added here is the
//! part the canonical families cannot express — parametric scale with
//! **irregular degree** — plus the design → [`HyperGraphView`] adapter.
//!
//! # Determinism
//!
//! Every generator is a pure function of its spec, including the seed. No
//! system entropy, no clock, no thread-count dependence (CLAUDE.md §3). The
//! same spec produces bit-identical data on every run and every host, which is
//! what makes a saved Criterion baseline comparable across sessions.

use core::marker::PhantomData;

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use hymeko::common::pathkey::PathKey;
use hymeko::generators::HypergraphDesign;
use hymeko::ir::ir::{AnnoR, ArcRec, DeclKind, EdgeRec, Ir, NodeRec, RefAtomR, SignedRefR, ValueR};
use hymeko::resolution::interner::Interner;
use hymeko::resolution::resolve::Index;
use hymeko::tensor::common::Real;
use hymeko::tensor::representations::tensor_csr::TensorCsr;
use hymeko::tensor::tensor_val::EdgeWScalar;

use crate::traversal::hypergraphview::HyperGraphView;

// ===========================================================================
// Deterministic RNG
// ===========================================================================

/// 64-bit linear congruential generator (Knuth MMIX constants).
///
/// Not cryptographic and not intended to be. It exists so workloads are
/// reproducible without pulling `rand` into a `lockdown: full` crate's
/// dependency set.
///
/// # Invariants
/// The output sequence depends only on the seed passed to [`Lcg::new`].
#[derive(Clone, Debug)]
pub struct Lcg(u64);

impl Lcg {
    /// Seed the generator. Any seed is valid, including zero.
    #[inline]
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Next raw 64-bit value.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// Uniform-ish index in `0..bound`. A `bound` of 0 is treated as 1.
    ///
    /// Uses the high bits (the low bits of an LCG are notoriously weak) and a
    /// modulo reduction. The residual modulo bias is irrelevant for workload
    /// shaping and keeps the function allocation- and branch-free.
    #[inline]
    pub fn below(&mut self, bound: usize) -> usize {
        let b = bound.max(1);
        ((self.next_u64() >> 11) as usize) % b
    }

    /// Value in `[0, 1)` with 24 bits of mantissa entropy.
    #[inline]
    pub fn unit_f32(&mut self) -> f32 {
        const SCALE: f32 = 1.0 / (1u32 << 24) as f32;
        ((self.next_u64() >> 40) as u32) as f32 * SCALE
    }

    /// Value in `[0.5, 1.5)` — the standard weight range for these workloads.
    ///
    /// Bounded away from zero and spanning less than one octave, so that
    /// `f32` accumulation over ~10^6 terms stays well conditioned and a
    /// tolerance-based assertion is meaningful (CLAUDE.md §5, numerical
    /// stability).
    #[inline]
    pub fn weight_f32(&mut self) -> f32 {
        0.5 + self.unit_f32()
    }
}

// ===========================================================================
// Degree shaping
// ===========================================================================

/// How row / edge cardinalities are distributed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegreeShape {
    /// Every row or edge has exactly the requested average degree.
    ///
    /// The best case for load balancing — used to isolate raw throughput from
    /// scheduling effects.
    Uniform,
    /// Heavy-tailed: a few hubs, a long thin tail.
    ///
    /// Distribution, fixed and documented so the benchmark is legible:
    /// 5 % at `8×avg`, 15 % at `2×avg`, 60 % at `avg`, 20 % at `avg/4`.
    /// This is the case that stresses work-stealing and makes a naive
    /// equal-count partition underperform `par_chunks_mut` over rows.
    Irregular,
}

impl DegreeShape {
    /// Draw one degree. Always returns at least 1.
    #[inline]
    fn sample(self, rng: &mut Lcg, avg: usize) -> usize {
        let avg = avg.max(1);
        match self {
            Self::Uniform => avg,
            Self::Irregular => match rng.below(100) {
                0..=4 => avg * 8,
                5..=19 => avg * 2,
                20..=79 => avg,
                _ => 1 + avg / 4,
            }
            .max(1),
        }
    }
}

// ===========================================================================
// Sparse matrix workloads
// ===========================================================================

/// Specification of a synthetic CSR matrix.
///
/// # Invariants
/// The produced [`TensorCsr`] has sorted, deduplicated column indices within
/// every row — the layout a real assembled matrix has, and the one whose cache
/// behaviour the benchmark is meant to reflect.
#[derive(Clone, Copy, Debug)]
pub struct SparseSpec {
    pub rows: usize,
    pub cols: usize,
    pub avg_nnz_per_row: usize,
    pub shape: DegreeShape,
    pub seed: u64,
}

impl SparseSpec {
    /// Latency regime: small enough to sit in L2, where parallel fork/join
    /// overhead is expected to dominate the useful work.
    #[must_use]
    pub const fn small() -> Self {
        Self {
            rows: 1_000,
            cols: 1_000,
            avg_nnz_per_row: 8,
            shape: DegreeShape::Uniform,
            seed: 0x5EED_0001,
        }
    }

    /// Mid regime: exceeds L2, fits L3.
    #[must_use]
    pub const fn medium() -> Self {
        Self {
            rows: 50_000,
            cols: 50_000,
            avg_nnz_per_row: 16,
            shape: DegreeShape::Uniform,
            seed: 0x5EED_0002,
        }
    }

    /// Throughput / memory-bandwidth regime, with irregular rows so the
    /// parallel partitioning is actually tested rather than flattered.
    #[must_use]
    pub const fn large() -> Self {
        Self {
            rows: 200_000,
            cols: 200_000,
            avg_nnz_per_row: 20,
            shape: DegreeShape::Irregular,
            seed: 0x5EED_0003,
        }
    }

    /// Materialise the matrix.
    ///
    /// # Postconditions
    /// `row_ptr.len() == rows + 1`, `row_ptr` is non-decreasing,
    /// `row_ptr[rows] == col_ind.len() == val.len()`, and each row's slice of
    /// `col_ind` is strictly increasing.
    #[must_use]
    pub fn build(&self) -> TensorCsr<f32> {
        let mut rng = Lcg::new(self.seed);
        let mut row_ptr = Vec::with_capacity(self.rows + 1);
        let mut col_ind: Vec<usize> = Vec::with_capacity(self.rows * self.avg_nnz_per_row);
        let mut val: Vec<f32> = Vec::with_capacity(self.rows * self.avg_nnz_per_row);
        let mut scratch: Vec<usize> = Vec::new();

        row_ptr.push(0usize);
        for _ in 0..self.rows {
            let degree = self
                .shape
                .sample(&mut rng, self.avg_nnz_per_row)
                .min(self.cols);

            scratch.clear();
            for _ in 0..degree {
                scratch.push(rng.below(self.cols));
            }
            scratch.sort_unstable();
            scratch.dedup();

            for &c in &scratch {
                col_ind.push(c);
                val.push(rng.weight_f32());
            }
            row_ptr.push(col_ind.len());
        }

        TensorCsr {
            num_rows: self.rows,
            num_cols: self.cols,
            row_ptr,
            col_ind,
            val,
        }
    }

    /// Dense input vector matching this matrix's column count.
    #[must_use]
    pub fn dense_vector(&self) -> Vec<f32> {
        let mut rng = Lcg::new(self.seed ^ 0xD0D0_D0D0);
        (0..self.cols).map(|_| rng.weight_f32()).collect()
    }

    /// Dense row-major `cols × feature_dim` input block.
    #[must_use]
    pub fn dense_block(&self, feature_dim: usize) -> Vec<f32> {
        let mut rng = Lcg::new(self.seed ^ 0xB10C_B10C);
        (0..self.cols * feature_dim)
            .map(|_| rng.weight_f32())
            .collect()
    }
}

// ===========================================================================
// Hypergraph workloads
// ===========================================================================

/// Which incidence signs the generated hypergraph carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignPattern {
    /// Every incidence `+1`. Degenerate but useful as a control.
    AllPlus,
    /// Alternating `+1` / `-1` by position.
    Alternating,
    /// `+1` / `-1` / `0` cycling by `(node + edge) % 3`.
    ///
    /// The only pattern that exercises all three match arms in
    /// [`crate::tensor::conv::signed_hgnn`], including the neutral arm that
    /// contributes to *both* channels.
    Mixed,
}

impl SignPattern {
    #[inline]
    fn sign_of(self, position: usize, node: usize, edge: usize) -> i8 {
        match self {
            Self::AllPlus => 1,
            Self::Alternating => {
                if position.is_multiple_of(2) {
                    1
                } else {
                    -1
                }
            }
            Self::Mixed => match (node + edge) % 3 {
                0 => 1,
                1 => -1,
                _ => 0,
            },
        }
    }
}

/// Specification of a synthetic hypergraph.
#[derive(Clone, Copy, Debug)]
pub struct HyperSpec {
    pub nodes: usize,
    pub edges: usize,
    pub avg_cardinality: usize,
    pub shape: DegreeShape,
    pub signs: SignPattern,
    pub seed: u64,
}

impl HyperSpec {
    /// Latency regime.
    #[must_use]
    pub const fn small() -> Self {
        Self {
            nodes: 1_000,
            edges: 500,
            avg_cardinality: 4,
            shape: DegreeShape::Uniform,
            signs: SignPattern::Mixed,
            seed: 0x11AB_0001,
        }
    }

    /// Mid regime.
    #[must_use]
    pub const fn medium() -> Self {
        Self {
            nodes: 50_000,
            edges: 25_000,
            avg_cardinality: 8,
            shape: DegreeShape::Uniform,
            signs: SignPattern::Mixed,
            seed: 0x11AB_0002,
        }
    }

    /// Throughput regime with irregular hyperedge cardinality.
    #[must_use]
    pub const fn large() -> Self {
        Self {
            nodes: 200_000,
            edges: 100_000,
            avg_cardinality: 12,
            shape: DegreeShape::Irregular,
            signs: SignPattern::Mixed,
            seed: 0x11AB_0003,
        }
    }

    /// Build the index-level design.
    ///
    /// Returns [`hymeko::generators::HypergraphDesign`] — the canonical
    /// interchange type — rather than a private struct, so a design produced
    /// here is interchangeable with `steiner_design` / `sunflower_design` /
    /// `complete_uniform_design`.
    ///
    /// # Postconditions
    /// Every member index is `< nodes`; every edge is non-empty with sorted,
    /// deduplicated members.
    #[must_use]
    pub fn design(&self) -> HypergraphDesign {
        let mut rng = Lcg::new(self.seed);
        let mut edges = Vec::with_capacity(self.edges);
        let mut members: Vec<usize> = Vec::new();

        for _ in 0..self.edges {
            let card = self
                .shape
                .sample(&mut rng, self.avg_cardinality)
                .min(self.nodes);
            members.clear();
            for _ in 0..card {
                members.push(rng.below(self.nodes));
            }
            members.sort_unstable();
            members.dedup();
            edges.push(members.clone());
        }

        HypergraphDesign {
            n_vertices: self.nodes,
            edges,
        }
    }

    /// Build the design and adapt it straight to a view.
    #[must_use]
    pub fn build(&self) -> HyperGraphView<f32, EdgeWScalar<f32>, f32> {
        // M2c.1 corrective: build a coherent `Ir` and go through the real,
        // representation-independent `try_from_ir` path — no fabricated canonical
        // ids and no false provenance. The `Ir`-build cost is isolated from the
        // resident view operations in the benchmark by pre-building the `Ir`
        // outside the timed region.
        HyperGraphView::from_ir(
            &self.ir(),
            &hymeko::tensor::aggregation::AggCfg {
                weight: hymeko::tensor::aggregation::WeightAgg::Sum,
                sign: hymeko::tensor::aggregation::SignAgg::PreferNonNeutral,
                clamp01: false,
            },
            &hymeko::tensor::tensor_val::ScalarWeightExtractor,
        )
    }

    /// Deterministic node-feature vector (`nodes` entries).
    #[must_use]
    pub fn node_vector(&self) -> Vec<f32> {
        let mut rng = Lcg::new(self.seed ^ 0xF00D_F00D);
        (0..self.nodes).map(|_| rng.weight_f32()).collect()
    }

    /// Deterministic row-major `nodes × feature_dim` node-feature block.
    #[must_use]
    pub fn node_block(&self, feature_dim: usize) -> Vec<f32> {
        let mut rng = Lcg::new(self.seed ^ 0xFEED_FEED);
        (0..self.nodes * feature_dim)
            .map(|_| rng.weight_f32())
            .collect()
    }
}

// ===========================================================================
// Design -> view adapter
// ===========================================================================

impl<F: Real> HyperGraphView<F, EdgeWScalar<F>, F> {
    /// Adapt an index-level [`HypergraphDesign`] into a full incidence view.
    ///
    /// This is the concrete-scalar counterpart to
    /// [`HyperGraphView::from_ir`]: `from_ir` is the production path from
    /// compiled HyMeKo source, this is the path from a combinatorial design
    /// (`hymeko::generators`) or a synthetic spec ([`HyperSpec`]).
    ///
    /// # Preconditions
    /// Every member index in `design.edges` is `< design.n_vertices`.
    ///
    /// # Postconditions
    /// Both incidence orientations are materialised and mutually consistent:
    /// they contain the same `(edge, node, sign, weight)` multiset, the
    /// edge-first arrays ordered by `(edge, node)` and the node-first arrays by
    /// `(node, edge)`. `edge_offsets.len() == num_edges + 1` and
    /// `node_offsets.len() == num_nodes + 1`, both non-decreasing.
    ///
    /// That mutual consistency is the property the parallel scatter relies on:
    /// because the node-first array is ordered by `(node, edge)`, iterating a
    /// node's own incidences visits its contributing edges in the *same*
    /// increasing-edge order that an edge-major scan would deliver them, so the
    /// two accumulate identically — bitwise, not merely within tolerance.
    #[must_use]
    pub fn from_design(design: &HypergraphDesign, signs: SignPattern, seed: u64) -> Self {
        let num_nodes = design.n_vertices;
        let num_edges = design.edges.len();

        let nnz: usize = design.edges.iter().map(Vec::len).sum();
        let mut rng = Lcg::new(seed ^ 0xA5A5_A5A5);

        // --- edge-first CSR: members are already sorted per edge, and edges
        // --- are emitted in order, so this is (edge, node)-ordered by
        // --- construction. No sort needed.
        let mut flat_edge_nodes: Vec<crate::traversal::dense_index::DenseNodeIndex> =
            Vec::with_capacity(nnz);
        let mut flat_edge_sign: Vec<i8> = Vec::with_capacity(nnz);
        let mut flat_edge_w: Vec<F> = Vec::with_capacity(nnz);
        let mut edge_offsets: Vec<usize> = Vec::with_capacity(num_edges + 1);

        edge_offsets.push(0);
        for (e, members) in design.edges.iter().enumerate() {
            for (position, &v) in members.iter().enumerate() {
                debug_assert!(v < num_nodes, "member index out of range");
                flat_edge_nodes.push(crate::traversal::dense_index::DenseNodeIndex::from_u32(
                    v as u32,
                ));
                flat_edge_sign.push(signs.sign_of(position, v, e));
                flat_edge_w.push(F::from_other(rng.weight_f32()));
            }
            edge_offsets.push(flat_edge_nodes.len());
        }

        // --- node-first CSR: counting sort over the same triples. Stable by
        // --- construction, and since the edge-first array is already in
        // --- increasing-edge order, each node's slice comes out in increasing
        // --- edge order too.
        let mut node_counts = vec![0usize; num_nodes + 1];
        for n in &flat_edge_nodes {
            node_counts[n.as_usize() + 1] += 1;
        }
        for i in 0..num_nodes {
            node_counts[i + 1] += node_counts[i];
        }
        let node_offsets = node_counts.clone();

        let mut flat_node_edges: Vec<crate::traversal::dense_index::DenseEdgeIndex> =
            vec![crate::traversal::dense_index::DenseEdgeIndex::from_u32(0); nnz];
        let mut flat_node_sign: Vec<i8> = vec![0; nnz];
        let mut flat_node_w: Vec<F> = vec![F::zero(); nnz];
        let mut cursor = node_counts;

        for e in 0..num_edges {
            for p in edge_offsets[e]..edge_offsets[e + 1] {
                let v = flat_edge_nodes[p].as_usize();
                let slot = cursor[v];
                flat_node_edges[slot] =
                    crate::traversal::dense_index::DenseEdgeIndex::from_u32(e as u32);
                flat_node_sign[slot] = flat_edge_sign[p];
                flat_node_w[slot] = flat_edge_w[p];
                cursor[v] = slot + 1;
            }
        }

        Self {
            // Benchmark-only synthetic provenance: this view has NO backing `Ir`,
            // so its canonical coordinates are the identity `0..n`. Honestly
            // fixture-local, not provenance from a real `Ir` — kept only to isolate
            // pure CSR-construction cost in `hnn_bench` (the production path is
            // `try_from_ir`, which build() now uses).
            node_canonical: (0..num_nodes).map(NodeId::new).collect(),
            edge_canonical: (0..num_edges).map(EdgeId::new).collect(),
            node_decl: (0..num_nodes).map(DeclId::new).collect(),
            edge_decl: (0..num_edges).map(DeclId::new).collect(),
            flat_node_edges,
            flat_node_sign,
            flat_node_w,
            node_offsets,
            flat_edge_nodes,
            flat_edge_sign,
            flat_edge_w,
            edge_offsets,
            edge_weight: vec![EdgeWScalar(F::one()); num_edges],
            _phantom: PhantomData,
        }
    }
}

// ===========================================================================
// IR workloads (production construction path)
// ===========================================================================

/// A synthetic workload as a **complete, coherent** declaration program.
///
/// The three pieces travel together because none of them is meaningful alone:
/// `ir.decl_nodes[d].name` is a [`SymId`] that only the `interner` can resolve,
/// and `hymeko::ir::coherence::validate_hierarchy_index_coherence` compares the
/// intrusive hierarchy against the path `index`. Returning only the `Ir` — which
/// is what [`HyperSpec::ir`] does for callers that need nothing else — discards
/// exactly the tables that make the `Ir` checkable.
///
/// # Invariants
///
/// Every value produced by [`HyperSpec::program`] satisfies all of these, and
/// `workload_coherence` in `hymeko_core/tests/unified_ir` asserts them:
///
/// 1. the five declaration-parallel arrays (`decl_nodes`, `decl_hash`,
///    `decl_to_node`, `decl_to_edge`, `decl_to_arc`) have equal length;
/// 2. `decl_nodes` holds one declaration per node, per edge **and** per arc;
/// 3. `NodeRec::decl` / `EdgeRec::decl` point back at the declaration that maps
///    to them, and `decl_to_node` / `decl_to_edge` / `decl_to_arc` each map only
///    declarations of the matching `DeclKind`;
/// 4. every arc declaration is parented to its edge declaration and appears in
///    that edge's intrusive child chain, with `last_child` reachable;
/// 5. `parent` links are acyclic and agree with path-prefix parenthood;
/// 6. every `SignedRefR` target is a `Node` declaration that resolves through
///    `decl_to_node`;
/// 7. every `EdgeRec::arcs` entry indexes an existing `ArcRec`;
/// 8. `index` holds exactly one path per named (`Node`/`Edge`) declaration;
///    arc declarations are anonymous, which is what production lowering also
///    produces and what the coherence validator explicitly permits.
pub struct HyperSpecProgram {
    pub ir: Ir,
    pub index: Index,
    pub interner: Interner,
}

impl HyperSpec {
    /// Build a synthetic [`Ir`] whose lowering reproduces this spec.
    ///
    /// [`HyperGraphView::from_design`] is the *benchmark* construction path;
    /// [`HyperGraphView::from_ir`] is the **production** one, and it does
    /// strictly more work — collect triples, sort by `(edge, node)`, reduce
    /// duplicates, build the edge-first CSR, re-sort by `(node, edge)`, build
    /// the node-first CSR. Benchmarking construction without this would measure
    /// the wrong function.
    ///
    /// Declaration ids are laid out as `0..nodes` for nodes,
    /// `nodes..nodes + edges` for edges, and `nodes + edges..` for the one
    /// anonymous arc declaration per hyperedge — the same order production
    /// lowering produces, which preallocates every *named* declaration and then
    /// pushes arc declarations in encounter order. Each reference gets an
    /// explicit numeric weight so `ScalarWeightExtractor` exercises its real
    /// parsing path rather than the `None => one()` shortcut.
    ///
    /// Discards the `Index` and `Interner`; use [`HyperSpec::program`] when the
    /// declaration program has to be validated or hashed.
    ///
    /// # Postconditions
    /// `ir.nodes.len() == self.nodes`, `ir.edges.len() == self.edges`, every
    /// `SignedRefR` targets a `DeclId` that maps to a `NodeId`, and the result
    /// satisfies every invariant listed on [`HyperSpecProgram`].
    #[must_use]
    pub fn ir(&self) -> Ir {
        self.program_with_weights(true).ir
    }

    /// Same as [`HyperSpec::ir`], but with every reference's `weights` set to
    /// `None` instead of `Some(vec![Num])`.
    ///
    /// This exists as a **discriminating test**, not as a convenience.
    /// `hymeko::tensor::tensor_val::extract_ref_weight_scalar` returns early on
    /// `None` but builds a `Vec<F>` on `Some` — one heap allocation per
    /// incidence. Benchmarking `from_ir` over both IRs isolates that allocation
    /// from every other cost in the construction path, which is otherwise
    /// impossible to attribute without a profiler.
    ///
    /// Both IRs are **structurally identical** — same declarations, same ids,
    /// same hierarchy, same reference targets and signs, and the weight RNG is
    /// advanced in both so the two share one stream. Any timing difference is
    /// attributable to the weight-extraction path alone.
    #[must_use]
    pub fn ir_unweighted(&self) -> Ir {
        self.program_with_weights(false).ir
    }

    /// The full declaration program: `Ir` plus the `Index` and `Interner` that
    /// make it checkable and hashable.
    ///
    /// # Postconditions
    /// Satisfies every invariant listed on [`HyperSpecProgram`].
    #[must_use]
    pub fn program(&self) -> HyperSpecProgram {
        self.program_with_weights(true)
    }

    /// Unweighted counterpart of [`HyperSpec::program`].
    ///
    /// # Postconditions
    /// Structurally identical to [`HyperSpec::program`] — see
    /// [`HyperSpec::ir_unweighted`].
    #[must_use]
    pub fn program_unweighted(&self) -> HyperSpecProgram {
        self.program_with_weights(false)
    }

    /// Deterministic declaration name for node `i`.
    ///
    /// Workload identity is a documented function of the index, never of the
    /// seed: the seed shapes *topology and weights*, and letting it reach the
    /// names as well would make two structurally identical workloads hash
    /// differently for no semantic reason.
    fn node_name(i: usize) -> String {
        format!("n{i}")
    }

    /// Deterministic declaration name for edge `j`. See [`HyperSpec::node_name`].
    fn edge_name(j: usize) -> String {
        format!("e{j}")
    }

    /// Deterministic declaration name for arc `k`.
    ///
    /// Arc declarations are anonymous in the `Index` — production lowering emits
    /// them through `Ir::push_anonymous_arc` and never gives them a path — but
    /// they still carry a name symbol. `__arc{k}` matches the convention the
    /// unified-IR test builder already uses, so the two fixture families print
    /// the same way.
    fn arc_name(k: usize) -> String {
        format!("__arc{k}")
    }

    /// The single place every table of a synthetic workload is built.
    ///
    /// Deliberately one function rather than a per-table set: the five
    /// declaration-parallel arrays, the three id maps, the three record tables
    /// and the intrusive hierarchy are one invariant, not eight, and splitting
    /// them is how the previous version came to populate `decl_to_node` while
    /// leaving `decl_nodes` empty.
    ///
    /// Reuses the two mechanisms production lowering uses to keep the parallel
    /// arrays in step — [`Ir::preallocate_from_index`] for the named
    /// declarations and [`Ir::push_anonymous_arc`] for each arc — rather than
    /// resizing five vectors by hand. What is *not* reused is
    /// `hymeko::ir::lower`, which needs a parsed `parser::ast::Description`;
    /// synthesising one for a 200 000-node workload would mean generating and
    /// re-parsing source text, and would couple this fixture to the parser's
    /// version drift.
    fn program_with_weights(&self, weighted: bool) -> HyperSpecProgram {
        let design = self.design();
        let num_nodes = design.n_vertices;
        let num_edges = design.edges.len();
        let mut rng = Lcg::new(self.seed ^ 0x1234_5678);

        let mut ir = Ir::default();
        let mut interner = Interner::new();
        let mut index = Index::new();

        // Sizes decl_nodes, decl_hash, decl_to_node, decl_to_edge and
        // decl_to_arc together. Arc declarations are pushed afterwards, exactly
        // as `lower::lower_arc` does.
        ir.preallocate_from_index(num_nodes + num_edges);

        // --- node declarations ---------------------------------------------
        ir.nodes.reserve(num_nodes);
        for i in 0..num_nodes {
            let did = DeclId::new(i);
            let name = interner.intern(&Self::node_name(i));
            let decl = &mut ir.decl_nodes[i];
            decl.kind = DeclKind::Node;
            decl.name = name;
            decl.parent = DeclId::NONE;

            index.by_path.insert(PathKey(vec![name]), did);
            ir.decl_to_node[i] = Some(NodeId::new(ir.nodes.len()));
            ir.nodes.push(NodeRec::new(did, Vec::new()));
        }

        // --- edge declarations ---------------------------------------------
        ir.edges.reserve(num_edges);
        for j in 0..num_edges {
            let slot = num_nodes + j;
            let did = DeclId::new(slot);
            let name = interner.intern(&Self::edge_name(j));
            let decl = &mut ir.decl_nodes[slot];
            decl.kind = DeclKind::Edge;
            decl.name = name;
            decl.parent = DeclId::NONE;

            index.by_path.insert(PathKey(vec![name]), did);
            ir.decl_to_edge[slot] = Some(EdgeId::new(ir.edges.len()));
            ir.edges.push(EdgeRec::new(did, Vec::new()));
        }

        // --- arcs, their declarations, and the edge child chain -------------
        ir.arcs.reserve(num_edges);
        for (e, members) in design.edges.iter().enumerate() {
            let edge_decl = DeclId::new(num_nodes + e);

            let refs: Vec<SignedRefR> = members
                .iter()
                .enumerate()
                .map(|(position, &v)| {
                    // Drawn unconditionally so the weighted and unweighted
                    // variants consume the same RNG stream and stay
                    // structurally identical.
                    let w = rng.weight_f32() as f64;
                    let atom = RefAtomR {
                        target: DeclId::new(v),
                        anno: AnnoR::default(),
                        weights: if weighted {
                            Some(vec![ValueR::Num(w)])
                        } else {
                            None
                        },
                    };
                    match self.signs.sign_of(position, v, e) {
                        1 => SignedRefR::Plus(atom),
                        -1 => SignedRefR::Minus(atom),
                        _ => SignedRefR::Neutral(atom),
                    }
                })
                .collect();

            let aid = HyperArcId::new(ir.arcs.len());
            ir.arcs.push(ArcRec {
                anno: AnnoR::default(),
                in_edge: edge_decl,
                refs,
            });

            // Appends the declaration and extends all five parallel arrays.
            let arc_decl = ir.push_anonymous_arc();
            let arc_name = interner.intern(&Self::arc_name(e));
            let decl = &mut ir.decl_nodes[arc_decl.raw()];
            decl.name = arc_name;
            decl.parent = edge_decl;
            ir.decl_to_arc[arc_decl.raw()] = Some(aid);

            // One arc per hyperedge, so the edge's child chain is the
            // degenerate single-child case of `lower::link_decl_child`:
            // first == last, no sibling. Written out rather than reimplementing
            // the append walk, and guarded so that adding a second arc per edge
            // fails loudly here instead of silently producing a broken chain.
            debug_assert!(
                ir.decl_nodes[edge_decl.raw()].first_child.is_none(),
                "workload edges carry exactly one arc; the append case is not implemented"
            );
            let edge_node = &mut ir.decl_nodes[edge_decl.raw()];
            edge_node.first_child = arc_decl;
            edge_node.last_child = arc_decl;

            ir.edges[e].arcs.push(aid);
        }

        // Not one of the declaration-parallel arrays — it is indexed by
        // `HyperArcId` — but the merkle pass expects one slot per `ArcRec`.
        ir.arc_hash.resize(ir.arcs.len(), None);

        HyperSpecProgram {
            ir,
            index,
            interner,
        }
    }
}
