//! PoW-0 — the static computational spine, end to end.
//!
//! Proves the existing public path composes:
//!
//! ```text
//! HyMeKo source -> RealParser -> ModuleStore/lowering -> canonical Ir
//!   -> coherence validation -> HyperGraphView -> star-expansion CSR
//!   -> CPU serial & Rayon SpMM -> WGPU resident SpMM + activation -> readback
//!   -> numerical & structural equivalence
//! ```
//!
//! It composes existing library APIs only; it introduces no IR, view, CSR, SpMM,
//! activation, ID, transaction, snapshot, workload, CLI or GPU abstraction. The
//! `.hymeko` fixture under `tests/data/` is a human-auditable golden, not a
//! generator.
//!
//! **Scope:** PoW-0 is the *static* spine. It does NOT prove mutation,
//! transactions, CBOR or distributed synchronisation — those are PoW-1 and are
//! BLOCKED on the core approvals listed in `docs/platform-proof-of-work.md`.
//!
//! Source/parse/lower/view/CSR/CPU tests never skip. Only the GPU tests use the
//! existing adapter-unavailable skip convention (`backend_or_skip`).

use std::path::PathBuf;
use std::sync::Arc;

use hymeko::common::ids::{EdgeId, NodeId};
use hymeko::ir::coherence::{CoherenceLimits, validate_hierarchy_index_coherence};
use hymeko::module_store::module_store::{CompiledProgram, ModuleStore};
use hymeko::module_store::source_provider::StdFsProvider;
use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
use hymeko::tensor::representations::tensor_csr::TensorCsr;
use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
use hymeko::util::real_parser::RealParser;

use hymeko_hnn::tensor::representations::tensor_csr_representations::star_expansion_csr;
use hymeko_hnn::traversal::hypergraphview::HyperGraphView;

use hymeko_wgpu::testing::backend_or_skip;
use hymeko_wgpu::{ActivationKind, GpuTensor, WorkgroupShape};

// ── the golden demonstrator ─────────────────────────────────────────────────

type Store = ModuleStore<StdFsProvider, RealParser>;
type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

/// Frozen canonical hash of the golden fixture at this core revision. Compiled
/// through the product path (`hymeko_core` binary) at authoring time.
const FROZEN_CANON_HASH: &str = "00d4c01ad686f4f83dbe7d6a48f4a2e7bdc2b757bbe5f61771c45cac9d509ade";

// Expected shape of the golden (verified by the product CLI at authoring time).
const EXPECT_DECLS: usize = 19;
const EXPECT_NODES: usize = 9;
const EXPECT_EDGES: usize = 5;
const EXPECT_ARCS: usize = 5;
const EXPECT_INCIDENCES: usize = 14; // 4 + 3 + 3 + 2 + 2

const ACTIVATION_WG: u32 = 256; // hymeko_wgpu DEFAULT_ACTIVATION_WORKGROUP

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/pow0_service_network.hymeko")
}

fn hex(h: &hymeko::ir::hash::HashId) -> String {
    h.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn cfg() -> AggCfg {
    AggCfg {
        weight: WeightAgg::Sum,
        sign: SignAgg::PreferNonNeutral,
        clamp01: false,
    }
}

/// Compile the golden through the real parser + ModuleStore + lowering.
fn compile_golden() -> (Store, Arc<CompiledProgram>) {
    let mut store = Store::new(StdFsProvider::new(), RealParser);
    let compiled = store
        .compile(&fixture_path())
        .expect("golden fixture compiles through the real pipeline");
    (store, compiled)
}

fn build_view(compiled: &CompiledProgram) -> View {
    View::try_from_ir(&compiled.ir, &cfg(), &ScalarWeightExtractor)
        .expect("HyperGraphView builds from the canonical IR")
}

fn incidence_count(ir: &hymeko::ir::ir::Ir) -> usize {
    ir.arcs.iter().map(|a| a.refs.len()).sum()
}

/// Deterministic feature matrix `[dim x fd]`, spanning positive and negative so
/// ReLU is a non-trivial gate. Pure function of (row, col) — no RNG.
fn deterministic_features(dim: usize, fd: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim * fd];
    for r in 0..dim {
        for c in 0..fd {
            let t = ((r * 7 + c * 13) % 17) as f32;
            v[r * fd + c] = t * 0.125 - 1.0; // range [-1.0, 1.0)
        }
    }
    v
}

fn relu(v: f32) -> f32 {
    if v > 0.0 { v } else { 0.0 }
}

/// Structural NaN/Inf + abs-or-rel tolerance, matching the existing spmm/act
/// contract (ABS = REL = 1e-5). A local copy: the production helper lives in
/// another test binary and is not importable.
fn assert_close(got: &[f32], want: &[f32], ctx: &str) {
    const ABS: f32 = 1e-5;
    const REL: f32 = 1e-5;
    assert_eq!(got.len(), want.len(), "{ctx}: length mismatch");
    for i in 0..want.len() {
        let (g, w) = (got[i], want[i]);
        if w.is_nan() || g.is_nan() {
            assert!(w.is_nan() && g.is_nan(), "{ctx}: NaN disagreement at {i}");
            continue;
        }
        if w.is_infinite() || g.is_infinite() {
            assert!(
                w.is_infinite() && g.is_infinite() && w.signum() == g.signum(),
                "{ctx}: infinity disagreement at {i}"
            );
            continue;
        }
        let abs = (g - w).abs();
        let rel = if w.abs() > 0.0 { abs / w.abs() } else { abs };
        assert!(
            abs <= ABS || rel <= REL,
            "{ctx}: mismatch at {i}: got={g} want={w} abs={abs:e} rel={rel:e}"
        );
    }
}

// ── 1. real source ingestion (never skips) ──────────────────────────────────

#[test]
fn pow0_source_ingests_to_the_expected_canonical_shape() {
    let (_store, compiled) = compile_golden();
    let ir = &compiled.ir;
    assert_eq!(ir.decl_nodes.len(), EXPECT_DECLS, "decls");
    assert_eq!(ir.nodes.len(), EXPECT_NODES, "nodes");
    assert_eq!(ir.edges.len(), EXPECT_EDGES, "edges");
    assert_eq!(ir.arcs.len(), EXPECT_ARCS, "arcs");
    assert_eq!(incidence_count(ir), EXPECT_INCIDENCES, "incidences");
}

// ── 2. canonical consistency (never skips) ──────────────────────────────────

#[test]
fn pow0_canonical_identity_is_coherent_frozen_and_deterministic() {
    let (store, compiled) = compile_golden();

    // Hierarchy/index coherence passes.
    validate_hierarchy_index_coherence(
        &compiled.ir,
        &compiled.idx,
        &store.it,
        CoherenceLimits::default(),
    )
    .expect("the golden IR is coherent");

    // Canonical hash is frozen.
    let got = hex(&compiled.canon_hash);
    assert_eq!(got, FROZEN_CANON_HASH, "canonical hash drifted");

    // Compiling the same source twice yields identical canonical identity and
    // an identical entity/name mapping.
    let (store2, compiled2) = compile_golden();
    assert_eq!(
        got,
        hex(&compiled2.canon_hash),
        "canonical hash is not deterministic across compiles"
    );
    assert_eq!(
        compiled.ir.decl_nodes.len(),
        compiled2.ir.decl_nodes.len(),
        "decl count differs across compiles"
    );
    for (d, (n1, n2)) in compiled
        .ir
        .decl_nodes
        .iter()
        .zip(compiled2.ir.decl_nodes.iter())
        .enumerate()
    {
        assert_eq!(
            store.it.resolve(n1.name),
            store2.it.resolve(n2.name),
            "name mapping differs at decl {d}"
        );
        assert_eq!(n1.kind, n2.kind, "kind differs at decl {d}");
    }
}

// ── 3. derived-view construction (never skips) ──────────────────────────────

#[test]
fn pow0_view_orientations_agree_and_rebuild_is_equivalent() {
    let (_store, compiled) = compile_golden();
    let view = build_view(&compiled);

    assert_eq!(view.num_nodes(), EXPECT_NODES, "view node count");
    assert_eq!(view.num_edges(), EXPECT_EDGES, "view edge count");

    // Both incidence orientations must contain the same (node, edge) multiset.
    let mut edge_major: Vec<(usize, usize)> = Vec::new();
    for e in 0..view.num_edges() {
        for &n in view.edge_nodes(EdgeId::new(e)) {
            edge_major.push((n.0, e));
        }
    }
    let mut node_major: Vec<(usize, usize)> = Vec::new();
    for n in 0..view.num_nodes() {
        for &e in view.node_edges(NodeId::new(n)) {
            node_major.push((n, e.0));
        }
    }
    edge_major.sort_unstable();
    node_major.sort_unstable();
    assert_eq!(
        edge_major, node_major,
        "node-major and edge-major incidence multisets disagree"
    );
    assert_eq!(
        edge_major.len(),
        EXPECT_INCIDENCES,
        "incidence count via view"
    );

    // Deterministic rebuild: a second view is structurally identical.
    let view2 = build_view(&compiled);
    assert_eq!(view.flat_edge_nodes, view2.flat_edge_nodes, "rebuild nodes");
    assert_eq!(view.flat_edge_sign, view2.flat_edge_sign, "rebuild signs");
    assert_eq!(view.num_nodes(), view2.num_nodes());
    assert_eq!(view.num_edges(), view2.num_edges());
}

// ── 4. CSR construction vs an independent structural oracle (never skips) ────

/// Decode a CSR into a sorted set of (row, col) pairs.
fn csr_pairs(csr: &TensorCsr<f32>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for r in 0..csr.num_rows {
        for idx in csr.row_ptr[r]..csr.row_ptr[r + 1] {
            out.push((r, csr.col_ind[idx]));
        }
    }
    out.sort_unstable();
    out
}

#[test]
fn pow0_star_csr_matches_the_signed_bipartite_definition() {
    let (_store, compiled) = compile_golden();
    let view = build_view(&compiled);
    let csr = star_expansion_csr(&view);

    let dim = view.num_nodes() + view.num_edges();
    // Shape invariants.
    assert_eq!(csr.num_rows, dim, "square star dim rows");
    assert_eq!(csr.num_cols, dim, "square star dim cols");
    assert_eq!(csr.row_ptr.len(), dim + 1, "row_ptr length");
    assert_eq!(
        *csr.row_ptr.last().unwrap(),
        csr.col_ind.len(),
        "nnz == last row_ptr"
    );
    assert_eq!(csr.col_ind.len(), csr.val.len(), "col_ind/val aligned");
    // Row pointers monotonic; indices in range; sorted within a row; finite vals.
    for r in 0..dim {
        assert!(
            csr.row_ptr[r] <= csr.row_ptr[r + 1],
            "row_ptr monotonic at {r}"
        );
        let (s, e) = (csr.row_ptr[r], csr.row_ptr[r + 1]);
        for idx in s..e {
            assert!(csr.col_ind[idx] < dim, "col in range at {idx}");
            assert!(csr.val[idx].is_finite(), "value finite at {idx}");
            if idx + 1 < e {
                assert!(
                    csr.col_ind[idx] < csr.col_ind[idx + 1],
                    "sorted cols in row {r}"
                );
            }
        }
    }

    // Independent oracle: derive the expected (row,col) set from the star
    // DEFINITION (+ : node->edge, - : edge->node, neutral : both) using the
    // view's public incidence fields — NOT by calling star_expansion_csr again.
    let n = view.num_nodes();
    let mut expected: Vec<(usize, usize)> = Vec::new();
    for e in 0..view.num_edges() {
        let e_v = n + e;
        let (s, eend) = view.edge_span(EdgeId::new(e));
        for p in s..eend {
            let n_v = view.flat_edge_nodes[p].0;
            match view.flat_edge_sign[p] {
                1 => expected.push((n_v, e_v)),
                -1 => expected.push((e_v, n_v)),
                _ => {
                    expected.push((n_v, e_v));
                    expected.push((e_v, n_v));
                }
            }
        }
    }
    expected.sort_unstable();
    assert_eq!(
        csr_pairs(&csr),
        expected,
        "star CSR structure vs definition"
    );
}

// ── 5. CPU execution: serial vs Rayon under the numerical contract ──────────

#[test]
fn pow0_cpu_spmm_serial_and_rayon_agree_at_multiple_feature_dims() {
    let (_store, compiled) = compile_golden();
    let view = build_view(&compiled);
    let csr = star_expansion_csr(&view);
    let dim = csr.num_rows;

    for &fd in &[8usize, 32, 128] {
        let x = deterministic_features(dim, fd);
        let mut y_serial = vec![f32::NAN; dim * fd];
        let mut y_par = vec![f32::NAN; dim * fd];
        csr.spmm_into_serial(&x, fd, &mut y_serial);
        csr.spmm_into_parallel(&x, fd, &mut y_par);
        // The CSR contract requires the Rayon path to be bitwise-identical to
        // serial (row-disjoint output, no reassociation).
        assert_eq!(y_serial, y_par, "serial vs Rayon SpMM diverged at fd={fd}");
        assert!(
            y_serial.iter().all(|v| v.is_finite()),
            "finite output fd={fd}"
        );
    }
}

// ── 6. WGPU resident SpMM + activation vs the CPU oracle (skips w/o adapter) ─

fn cpu_spmm_then_relu(csr: &TensorCsr<f32>, x: &[f32], fd: usize) -> Vec<f32> {
    let mut y = vec![0.0f32; csr.num_rows * fd];
    csr.spmm_into_serial(x, fd, &mut y);
    y.iter().map(|&v| relu(v)).collect()
}

#[test]
fn pow0_gpu_resident_spmm_activation_matches_cpu() {
    let (_store, compiled) = compile_golden();
    let view = build_view(&compiled);
    let csr = star_expansion_csr(&view);
    let dim = csr.num_rows;

    let Some(backend) = backend_or_skip("pow0_resident") else {
        return;
    };

    for &fd in &[8usize, 32, 128] {
        let x = deterministic_features(dim, fd);
        let cpu = cpu_spmm_then_relu(&csr, &x, fd);

        let res = backend.resources();
        let matrix = res.upload_csr_from("pow0_csr", &csr).expect("csr upload");
        let shape = matrix.shape();
        let input = res
            .upload_tensor("x", shape.input_shape(fd), &x)
            .expect("x upload");
        // Poisoned resident intermediates: the kernels must fully overwrite them.
        let poison = vec![f32::from_bits(0x7fc0_0000); shape.rows * fd]; // qNaN
        let y: GpuTensor<f32> = res
            .upload_tensor("y", shape.output_shape(fd), &poison)
            .expect("y alloc");
        let z: GpuTensor<f32> = res
            .upload_tensor("z", shape.output_shape(fd), &poison)
            .expect("z alloc");

        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&y)
            .workgroup(WorkgroupShape::default())
            .prepare()
            .expect("spmm prepare");
        let act = backend
            .operations()
            .activation()
            .kind(ActivationKind::Relu)
            .input(&y)
            .output(&z)
            .workgroup(ACTIVATION_WG)
            .prepare()
            .expect("activation prepare");

        // One encoder, one submit: y stays resident between the two kernels.
        let mut enc = backend.encoder(Some("pow0_resident"));
        spmm.encode(&mut enc).expect("encode spmm");
        act.encode(&mut enc).expect("encode activation");
        let idx = backend.submit(enc);
        backend.wait(idx).expect("submit");

        let gpu = backend.read_tensor(&z).expect("readback");
        assert_close(&gpu, &cpu, &format!("pow0 resident fd={fd}"));
    }
}

// ── 7. arena/session reuse: two ops, result unchanged (skips w/o adapter) ────

#[test]
fn pow0_repeated_gpu_ops_reuse_buffers_without_changing_the_result() {
    let (_store, compiled) = compile_golden();
    let view = build_view(&compiled);
    let csr = star_expansion_csr(&view);
    let dim = csr.num_rows;
    let fd = 32usize;

    let Some(backend) = backend_or_skip("pow0_reuse") else {
        return;
    };
    let x = deterministic_features(dim, fd);
    let cpu = cpu_spmm_then_relu(&csr, &x, fd);

    // Run the resident chain twice on the same backend (arena/session reuse the
    // underlying buffers across submissions); the result must be identical and
    // equal to the CPU oracle both times. Deeper provenance-laundering rejection
    // is proven type-level in tests/arena.rs and tests/session.rs (cited in the
    // PoW-0 traceability), not re-implemented here.
    let mut prev: Option<Vec<f32>> = None;
    for round in 0..2 {
        let res = backend.resources();
        let matrix = res.upload_csr_from("csr", &csr).expect("csr");
        let shape = matrix.shape();
        let input = res
            .upload_tensor("x", shape.input_shape(fd), &x)
            .expect("x");
        let poison = vec![f32::from_bits(0x7fc0_0000); shape.rows * fd];
        let y: GpuTensor<f32> = res
            .upload_tensor("y", shape.output_shape(fd), &poison)
            .expect("y");
        let z: GpuTensor<f32> = res
            .upload_tensor("z", shape.output_shape(fd), &poison)
            .expect("z");
        let spmm = backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&input)
            .output(&y)
            .workgroup(WorkgroupShape::default())
            .prepare()
            .expect("spmm");
        let act = backend
            .operations()
            .activation()
            .kind(ActivationKind::Relu)
            .input(&y)
            .output(&z)
            .workgroup(ACTIVATION_WG)
            .prepare()
            .expect("act");
        let mut enc = backend.encoder(Some("pow0_reuse"));
        spmm.encode(&mut enc).expect("enc spmm");
        act.encode(&mut enc).expect("enc act");
        backend.wait(backend.submit(enc)).expect("submit");
        let out = backend.read_tensor(&z).expect("readback");
        assert_close(&out, &cpu, &format!("pow0 reuse round {round}"));
        if let Some(p) = &prev {
            assert_eq!(p, &out, "GPU result changed between rounds (state leak)");
        }
        prev = Some(out);
    }
}

// ── 8. full-pipeline repeatability (never skips the CPU spine) ──────────────

#[test]
fn pow0_full_pipeline_is_repeatable() {
    let run = || {
        let (_store, compiled) = compile_golden();
        let view = build_view(&compiled);
        let csr = star_expansion_csr(&view);
        let dim = csr.num_rows;
        let x = deterministic_features(dim, 32);
        let mut y = vec![f32::NAN; dim * 32];
        csr.spmm_into_serial(&x, 32, &mut y);
        (hex(&compiled.canon_hash), csr_pairs(&csr), y)
    };
    let (h1, s1, y1) = run();
    let (h2, s2, y2) = run();
    assert_eq!(h1, h2, "canonical hash not repeatable");
    assert_eq!(s1, s2, "CSR structure not repeatable");
    assert_eq!(y1, y2, "CPU SpMM values not repeatable");
}

// ── Phase D — negative & boundary (never skip; these are pure/CPU) ──────────

#[test]
fn pow0_missing_source_file_is_a_typed_error() {
    let mut store = Store::new(StdFsProvider::new(), RealParser);
    let missing =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/pow0_does_not_exist.hymeko");
    match store.compile(&missing) {
        Err(hymeko::module_store::module_store::ModuleLoadError::Io(_)) => {}
        Err(other) => panic!("expected an Io error, got {other:?}"),
        Ok(_) => panic!("compiling a missing file must fail"),
    }
}

#[test]
fn pow0_parser_rejects_malformed_source() {
    // A syntactically broken in-memory source through the same RealParser.
    use hymeko::module_store::module_store::HymekoParser;
    let p = RealParser;
    assert!(
        p.parse("this is ) not ( valid hymeko {{{").is_err(),
        "the real parser must reject malformed source"
    );
}

#[test]
fn pow0_empty_and_singleton_edges_build_valid_csr() {
    use hymeko::module_store::source_provider::MemProvider;

    // Empty hypergraph: a scope with nodes but no hyperedges.
    let empty_src = "pow0_empty\n{}\nroot\n{\n    a{}\n    b{}\n}\n";
    // Singleton hyperedge: a scope with one node and a one-incidence edge.
    let singleton_src = "pow0_singleton\n{}\nroot\n{\n    a{}\n    @e { ( + a[1.0] ); }\n}\n";

    for (name, src, want_edges) in [
        ("empty", empty_src, 0usize),
        ("singleton", singleton_src, 1),
    ] {
        let mut provider = MemProvider::default();
        let path = PathBuf::from(format!("{name}.hymeko"));
        provider.insert_file(path.clone(), src.to_string());
        let mut store = ModuleStore::new(provider, RealParser);
        let compiled = store
            .compile(&path)
            .unwrap_or_else(|e| panic!("{name} compiles: {e:?}"));
        assert_eq!(compiled.ir.edges.len(), want_edges, "{name} edge count");

        let view = View::try_from_ir(&compiled.ir, &cfg(), &ScalarWeightExtractor)
            .unwrap_or_else(|e| panic!("{name} view builds: {e:?}"));
        let csr = star_expansion_csr(&view);
        // A valid (possibly empty) CSR: monotonic row_ptr, aligned arrays.
        assert_eq!(csr.row_ptr.len(), csr.num_rows + 1, "{name} row_ptr len");
        assert_eq!(csr.col_ind.len(), csr.val.len(), "{name} aligned");
        assert_eq!(
            *csr.row_ptr.last().unwrap(),
            csr.col_ind.len(),
            "{name} nnz"
        );
        assert!(csr.val.iter().all(|v| v.is_finite()), "{name} finite");
    }
}

// NOTE (traceability, not re-implemented here — Phase D references):
//   * malformed canonical IR rejected by the strict view builder:
//       hymeko_core tests unified_ir::validation::derived_views::{
//         try_from_ir_reports_a_dangling_arc, _dangling_target,
//         _a_node_declaration_with_no_record, _rejects_an_ir_with_no_declaration_table }
//   * duplicate incidences distinct (U3) and weight bit-faithfulness (U4):
//       unified_ir::verification::hash_coverage / frozen_hash
//   * dims not divisible by the workgroup, invalid CSR rejected before submit,
//     stale arena operand/provenance forbidden at the type level:
//       hymeko_wgpu tests spmm::{test_irregular_degree_and_non_multiple_shapes,
//       workgroup rejection}, arena.rs, session.rs
// These existing named tests already prove those requirements; PoW-0 cites them
// rather than duplicating them (CLAUDE.md Sec. 6.1).
