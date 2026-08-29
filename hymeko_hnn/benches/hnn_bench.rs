//! Hypergraph message-passing and convolution benchmarks.
//!
//! Same discipline as `hymeko_core/benches/csr_bench.rs`: seeded workloads
//! from [`hymeko_hnn::workloads`], construction hoisted out of the timed
//! closure, `black_box` on every input, declared `Throughput`.
//!
//! Throughput unit is the **incidence** (one `(node, edge)` pair) for the
//! message-passing kernels, and `incidences * d_out` for the convolutions —
//! that is the quantity the inner loops actually traverse, so elem/s is
//! comparable across scales and across the gather/scatter/conv boundary.
//!
//! `view_construction` is the deliberate exception to "keep allocation out of
//! the timed region": there, allocation *is* the subject.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use hymeko::common::ids::NodeId;
use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
use hymeko_hnn::tensor::conv::gcn_clique::GcnCliqueLayer;
use hymeko_hnn::tensor::conv::hgnn::HgnnLayer;
use hymeko_hnn::tensor::conv::signed_hgnn::SignedHgnnLayer;
use hymeko_hnn::tensor::message_passing::{
    CliqueStepCfg, clique_diag, gather_edges_from_nodes, gather_edges_from_nodes_parallel,
    gather_edges_from_nodes_serial, implicit_clique_step, scatter_nodes_from_edges,
    scatter_nodes_from_edges_parallel, scatter_nodes_from_edges_serial,
};
use hymeko_hnn::tensor::message_passing::{build_explicit_a, remove_self_effect};
use hymeko_hnn::tensor::representations::tensor_csr_representations::{
    clique_expansion_csr, star_expansion_csr,
};
use hymeko_hnn::tensor::tensor::compute_bipartite_degrees;
use hymeko_hnn::traversal::graph_traversal::{bfs, bfs_dense, dfs_preorder, dfs_preorder_dense};
use hymeko_hnn::traversal::hypergraphview::{BergeState, BergeView, HyperGraphView};
use hymeko_hnn::workloads::{DegreeShape, HyperSpec, SignPattern};

type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

fn scales() -> [(&'static str, HyperSpec); 3] {
    [
        ("small", HyperSpec::small()),
        ("medium", HyperSpec::medium()),
        ("large", HyperSpec::large()),
    ]
}

fn incidences(hg: &View) -> usize {
    hg.flat_edge_nodes.len()
}

/// Sample-size policy, stated once so every group applies it identically.
fn tune(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>, label: &str) {
    if label == "large" {
        group
            .sample_size(10)
            .measurement_time(Duration::from_secs(8));
    } else {
        group
            .sample_size(50)
            .measurement_time(Duration::from_secs(5));
    }
}

// ---------------------------------------------------------------------------
// Message passing
// ---------------------------------------------------------------------------

fn bench_gather(c: &mut Criterion) {
    let mut group = c.benchmark_group("mp_gather");

    for (label, spec) in scales() {
        let hg = spec.build();
        let x = spec.node_vector();
        let mut out = vec![0.0f32; hg.num_edges()];

        group.throughput(Throughput::Elements(incidences(&hg) as u64));
        tune(&mut group, label);

        group.bench_function(BenchmarkId::new("serial", label), |b| {
            b.iter(|| {
                gather_edges_from_nodes(
                    black_box(&hg),
                    black_box(&x),
                    black_box(&mut out),
                    black_box(true),
                );
            });
        });
    }

    group.finish();
}

fn bench_scatter(c: &mut Criterion) {
    let mut group = c.benchmark_group("mp_scatter");

    for (label, spec) in scales() {
        let hg = spec.build();
        let x_edges: Vec<f32> = (0..hg.num_edges())
            .map(|e| 0.5 + (e % 17) as f32 * 0.1)
            .collect();
        let mut out = vec![0.0f32; hg.num_nodes()];

        group.throughput(Throughput::Elements(incidences(&hg) as u64));
        tune(&mut group, label);

        group.bench_function(BenchmarkId::new("serial", label), |b| {
            b.iter(|| {
                scatter_nodes_from_edges(
                    black_box(&hg),
                    black_box(&x_edges),
                    black_box(&mut out),
                    black_box(true),
                );
            });
        });
    }

    group.finish();
}

fn bench_clique_diag(c: &mut Criterion) {
    let mut group = c.benchmark_group("mp_clique_diag");

    for (label, spec) in scales() {
        let hg = spec.build();

        group.throughput(Throughput::Elements(incidences(&hg) as u64));
        tune(&mut group, label);

        group.bench_function(BenchmarkId::new("serial", label), |b| {
            b.iter(|| black_box(clique_diag(black_box(&hg), black_box(true))));
        });
    }

    group.finish();
}

fn bench_implicit_clique_step(c: &mut Criterion) {
    let mut group = c.benchmark_group("mp_implicit_clique_step");

    for (label, spec) in scales() {
        let hg = spec.build();
        let x = spec.node_vector();
        let cfg = CliqueStepCfg {
            use_abs: true,
            include_self: false,
        };
        let diag = clique_diag(&hg, cfg.use_abs);
        let mut y = vec![0.0f32; hg.num_nodes()];
        let mut buf = vec![0.0f32; hg.num_edges()];

        // gather + scatter each touch every incidence once
        group.throughput(Throughput::Elements((2 * incidences(&hg)) as u64));
        tune(&mut group, label);

        group.bench_function(BenchmarkId::new("serial", label), |b| {
            b.iter(|| {
                implicit_clique_step(
                    black_box(&hg),
                    black_box(&x),
                    black_box(&mut y),
                    black_box(&mut buf),
                    black_box(Some(&diag)),
                    black_box(cfg),
                );
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Convolutions
// ---------------------------------------------------------------------------

/// `(scale label, d_in, d_out)` cells.
///
/// The `d_out` sweep is what discriminates hypothesis H4 — if the forward pass
/// is bound by re-traversing the incidence structure once per output feature,
/// cost per incidence rises with `d_out`; if it is bound by the dense
/// `X · Theta` matmul, it rises with `d_in * d_out` instead. Both terms are
/// present, so the sweep separates them rather than assuming which dominates.
fn conv_cells() -> &'static [(&'static str, usize, usize)] {
    &[
        ("small", 32, 32),
        ("medium", 32, 8),
        ("medium", 32, 32),
        ("medium", 32, 128),
        ("large", 32, 32),
    ]
}

fn spec_for(label: &str) -> HyperSpec {
    match label {
        "small" => HyperSpec::small(),
        "medium" => HyperSpec::medium(),
        _ => HyperSpec::large(),
    }
}

fn bench_hgnn_forward(c: &mut Criterion) {
    let mut group = c.benchmark_group("conv_hgnn_forward");

    for &(label, d_in, d_out) in conv_cells() {
        let spec = spec_for(label);
        let hg = spec.build();
        let layer = HgnnLayer::<f32>::from_view(&hg, d_in, d_out);
        let x_in = spec.node_block(d_in);
        let mut x_out = vec![0.0f32; hg.num_nodes() * d_out];

        group.throughput(Throughput::Elements((incidences(&hg) * d_out) as u64));
        tune(&mut group, label);

        group.bench_function(
            BenchmarkId::new("serial", format!("{label}/{d_in}x{d_out}")),
            |b| {
                b.iter(|| {
                    layer.forward_pass(black_box(&hg), black_box(&x_in), black_box(&mut x_out));
                });
            },
        );
    }

    group.finish();
}

fn bench_signed_hgnn_forward(c: &mut Criterion) {
    let mut group = c.benchmark_group("conv_signed_hgnn_forward");

    for &(label, d_in, d_out) in conv_cells() {
        let spec = spec_for(label);
        let hg = spec.build();
        let layer = SignedHgnnLayer::<f32>::from_view(&hg, d_in, d_out);
        let x_in = spec.node_block(d_in);
        let mut x_out = vec![0.0f32; hg.num_nodes() * d_out];

        group.throughput(Throughput::Elements((incidences(&hg) * d_out) as u64));
        tune(&mut group, label);

        group.bench_function(
            BenchmarkId::new("serial", format!("{label}/{d_in}x{d_out}")),
            |b| {
                b.iter(|| {
                    layer.forward_pass(black_box(&hg), black_box(&x_in), black_box(&mut x_out));
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// View construction (allocation IS the subject here)
// ---------------------------------------------------------------------------

fn bench_view_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("view_construction");

    for (label, spec) in scales() {
        // The index-level design is built once; only the adaptation into the
        // two incidence CSRs is timed.
        let design = spec.design();
        let total: usize = design.edges.iter().map(Vec::len).sum();

        group.throughput(Throughput::Elements(total as u64));
        tune(&mut group, label);

        group.bench_function(BenchmarkId::new("from_design", label), |b| {
            b.iter(|| {
                black_box(View::from_design(
                    black_box(&design),
                    black_box(spec.signs),
                    black_box(spec.seed),
                ))
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Path comparison: explicit serial vs explicit parallel.
//
// This is what fixes `PAR_INCIDENCE_THRESHOLD`. The constant claims to be the
// measured crossover, so it is measured here rather than assumed: the sweep
// walks the incidence count across three orders of magnitude around the
// candidate value and reports both paths at each point.
// ---------------------------------------------------------------------------

/// Incidence counts bracketing the candidate threshold (16_384).
const INCIDENCE_SWEEP: [usize; 7] = [512, 2_048, 8_192, 16_384, 65_536, 262_144, 1_048_576];

/// Hypergraph with approximately `incidences` node-edge pairs at fixed
/// cardinality, so the sweep varies one quantity only.
fn spec_for_incidences(incidences: usize) -> HyperSpec {
    const CARD: usize = 8;
    let edges = (incidences / CARD).max(1);
    HyperSpec {
        nodes: (edges * 2).max(2),
        edges,
        avg_cardinality: CARD,
        shape: DegreeShape::Uniform,
        signs: SignPattern::Mixed,
        seed: 0x11AB_7777,
    }
}

fn bench_gather_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("mp_gather_paths");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(5));

    for work in INCIDENCE_SWEEP {
        let spec = spec_for_incidences(work);
        let hg = spec.build();
        let x = spec.node_vector();
        let mut out = vec![0.0f32; hg.num_edges()];

        group.throughput(Throughput::Elements(incidences(&hg) as u64));

        group.bench_function(BenchmarkId::new("serial", work), |b| {
            b.iter(|| {
                gather_edges_from_nodes_serial(
                    black_box(&hg),
                    black_box(&x),
                    black_box(&mut out),
                    true,
                )
            });
        });
        group.bench_function(BenchmarkId::new("parallel", work), |b| {
            b.iter(|| {
                gather_edges_from_nodes_parallel(
                    black_box(&hg),
                    black_box(&x),
                    black_box(&mut out),
                    true,
                )
            });
        });
    }

    group.finish();
}

fn bench_scatter_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("mp_scatter_paths");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(5));

    for work in INCIDENCE_SWEEP {
        let spec = spec_for_incidences(work);
        let hg = spec.build();
        let x_edges: Vec<f32> = (0..hg.num_edges())
            .map(|e| 0.5 + (e % 17) as f32 * 0.1)
            .collect();
        let mut out = vec![0.0f32; hg.num_nodes()];

        group.throughput(Throughput::Elements(incidences(&hg) as u64));

        group.bench_function(BenchmarkId::new("serial", work), |b| {
            b.iter(|| {
                scatter_nodes_from_edges_serial(
                    black_box(&hg),
                    black_box(&x_edges),
                    black_box(&mut out),
                    true,
                )
            });
        });
        group.bench_function(BenchmarkId::new("parallel", work), |b| {
            b.iter(|| {
                scatter_nodes_from_edges_parallel(
                    black_box(&hg),
                    black_box(&x_edges),
                    black_box(&mut out),
                    true,
                )
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Production construction path.
//
// `from_design` (benched above) is the harness path. `from_ir` is what the
// compiler actually calls, and it does strictly more work: collect triples,
// sort by (edge, node), reduce duplicates, build the edge-first CSR, re-sort by
// (node, edge), build the node-first CSR. Benchmarking construction without
// this cell would measure the wrong function -- so both are reported.
// ---------------------------------------------------------------------------

fn bench_view_from_ir(c: &mut Criterion) {
    let mut group = c.benchmark_group("view_construction_from_ir");

    for (label, spec) in scales() {
        let ir = spec.ir();
        let cfg = AggCfg {
            weight: WeightAgg::Sum,
            sign: SignAgg::PreferNonNeutral,
            clamp01: false,
        };
        let ex = ScalarWeightExtractor;
        let total: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();

        group.throughput(Throughput::Elements(total as u64));
        tune(&mut group, label);

        group.bench_function(BenchmarkId::new("from_ir", label), |b| {
            b.iter(|| {
                black_box(View::from_ir(
                    black_box(&ir),
                    black_box(&cfg),
                    black_box(&ex),
                ))
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Discriminating test for the from_ir bottleneck.
//
// `extract_ref_weight_scalar` returns `F::one()` immediately when a reference
// carries no weights, but allocates a `Vec<F>` when it does -- one heap
// allocation per incidence. The two cells below build structurally identical
// IRs differing only in that field, so the gap between them is that allocation
// and nothing else.
// ---------------------------------------------------------------------------

fn bench_from_ir_weight_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("view_from_ir_weight_path");

    for (label, spec) in scales() {
        let weighted = spec.ir();
        let unweighted = spec.ir_unweighted();
        let cfg = AggCfg {
            weight: WeightAgg::Sum,
            sign: SignAgg::PreferNonNeutral,
            clamp01: false,
        };
        let ex = ScalarWeightExtractor;
        let total: usize = weighted.arcs.iter().map(|a| a.refs.len()).sum();

        group.throughput(Throughput::Elements(total as u64));
        tune(&mut group, label);

        group.bench_function(BenchmarkId::new("weights_some", label), |b| {
            b.iter(|| {
                black_box(View::from_ir(
                    black_box(&weighted),
                    black_box(&cfg),
                    black_box(&ex),
                ))
            });
        });
        group.bench_function(BenchmarkId::new("weights_none", label), |b| {
            b.iter(|| {
                black_box(View::from_ir(
                    black_box(&unweighted),
                    black_box(&cfg),
                    black_box(&ex),
                ))
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Phase 9 Part E — representation conversion, traversal and remaining models
// ---------------------------------------------------------------------------
//
// These complete the operation inventory the Phase 9 brief asked for. They sit
// beside the existing groups rather than in a second file, so one harness, one
// workload generator and one sample-size policy cover every graph operation.
//
// Cost categories are kept apart deliberately (brief, Part E): construction is
// its own group, representation conversion is its own group, and the kernels
// below time only the operation on already-resident CPU data. Nothing here is
// compared against a GPU resident kernel or a GPU end-to-end path.

/// Expansions are the one family whose OUTPUT size is not known from the input:
/// star expansion is linear in incidences, clique expansion is superlinear in
/// hyperedge cardinality (an edge of size k contributes k*(k-1) off-diagonal
/// entries). Throughput is therefore declared in output nonzeros, and the sizes
/// are reported so a reader can see the growth rather than infer it.
fn bench_expansions(c: &mut Criterion) {
    let mut group = c.benchmark_group("representation_expansion");
    for (label, spec) in scales() {
        let hg = spec.build();
        let inc = incidences(&hg);

        // Measure the output size once, outside the timed region, both to
        // declare throughput and to report the expansion factor.
        let star_nnz = star_expansion_csr(&hg).nnz();
        let clique_nnz = clique_expansion_csr(&hg).nnz();
        eprintln!(
            "[expansion {label}] incidences={inc} star_nnz={star_nnz} \
             clique_nnz={clique_nnz} clique/star={:.2}x",
            clique_nnz as f64 / star_nnz.max(1) as f64
        );

        tune(&mut group, label);

        group.throughput(Throughput::Elements(star_nnz as u64));
        group.bench_with_input(BenchmarkId::new("star", label), &hg, |b, hg| {
            b.iter(|| black_box(star_expansion_csr(black_box(hg))));
        });

        group.throughput(Throughput::Elements(clique_nnz as u64));
        group.bench_with_input(BenchmarkId::new("clique", label), &hg, |b, hg| {
            b.iter(|| black_box(clique_expansion_csr(black_box(hg))));
        });
    }
    group.finish();
}

/// Clique expansion under deliberately high hyperedge cardinality.
///
/// The quadratic term only shows up when edges are large, which the standard
/// specs do not exercise. Sizes are chosen so the largest case stays well under
/// the 16 GB cap: the estimate is computed first and the cell is SKIPPED with a
/// printed projection rather than run into an allocation failure.
fn bench_clique_expansion_growth(c: &mut Criterion) {
    let mut group = c.benchmark_group("representation_clique_growth");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(8));

    // A few high-cardinality shapes: fewer, much larger edges.
    for &(nodes, edges, card) in &[
        (20_000usize, 400usize, 64usize),
        (20_000, 400, 128),
        (20_000, 200, 256),
        (20_000, 100, 512),
    ] {
        let spec = HyperSpec {
            nodes,
            edges,
            avg_cardinality: card,
            shape: DegreeShape::Uniform,
            signs: SignPattern::AllPlus,
            seed: 0x9E37_79B9_7F4A_7C15,
        };
        let hg = spec.build();

        // Upper bound on off-diagonal output entries, before building anything.
        let estimated_nnz: u128 = hg
            .edge_offsets
            .windows(2)
            .map(|w| {
                let k = (w[1] - w[0]) as u128;
                k * k
            })
            .sum();
        let estimated_bytes = estimated_nnz * 8; // u32 col + f32 val
        const BUDGET_BYTES: u128 = 2 << 30; // 2 GiB, far under the 16 GB cap
        if estimated_bytes > BUDGET_BYTES {
            eprintln!(
                "[clique_growth card={card}] SKIPPED - estimated {estimated_nnz} nnz \
                 (~{:.1} GiB) exceeds the {:.0} GiB budget; reported, not forced",
                estimated_bytes as f64 / (1u128 << 30) as f64,
                BUDGET_BYTES as f64 / (1u128 << 30) as f64
            );
            continue;
        }

        let actual_nnz = clique_expansion_csr(&hg).nnz();
        eprintln!(
            "[clique_growth card={card}] edges={edges} estimated<={estimated_nnz} \
             actual_nnz={actual_nnz}"
        );
        group.throughput(Throughput::Elements(actual_nnz as u64));
        group.bench_with_input(
            BenchmarkId::new("clique", format!("card{card}")),
            &hg,
            |b, hg| b.iter(|| black_box(clique_expansion_csr(black_box(hg)))),
        );
    }
    group.finish();
}

/// Explicit clique adjacency (COO) and bipartite degrees.
///
/// `build_explicit_a` is what `GcnCliqueLayer` pays once at construction, so it
/// is timed on its own rather than hidden inside the layer benchmark.
fn bench_explicit_clique_and_degrees(c: &mut Criterion) {
    let mut group = c.benchmark_group("representation_explicit");
    let cfg = CliqueStepCfg {
        use_abs: true,
        include_self: true,
    };
    for (label, spec) in scales() {
        let hg = spec.build();
        let inc = incidences(&hg);
        tune(&mut group, label);

        group.throughput(Throughput::Elements(inc as u64));
        group.bench_with_input(BenchmarkId::new("build_explicit_a", label), &hg, |b, hg| {
            b.iter(|| black_box(build_explicit_a(black_box(hg), cfg)));
        });

        group.bench_with_input(
            BenchmarkId::new("bipartite_degrees", label),
            &hg,
            |b, hg| {
                b.iter(|| black_box(compute_bipartite_degrees(black_box(hg), true)));
            },
        );
    }
    group.finish();
}

/// Traversal over the Berge (Levi) bipartite expansion.
///
/// Throughput is declared in visited states rather than incidences: a traversal
/// from one start vertex reaches only its connected component, so incidences/s
/// would silently mix components of different sizes across scales.
fn bench_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("traversal");
    for (label, spec) in scales() {
        let hg = spec.build();
        let view = BergeView { hg: &hg };
        let start = BergeState::Node(NodeId::new(0));

        let reached = bfs(&view, start).len();
        eprintln!("[traversal {label}] reachable states from node 0: {reached}");
        tune(&mut group, label);
        group.throughput(Throughput::Elements(reached as u64));

        group.bench_with_input(BenchmarkId::new("bfs", label), &view, |b, v| {
            b.iter(|| black_box(bfs(black_box(v), start)));
        });
        group.bench_with_input(BenchmarkId::new("dfs_preorder", label), &view, |b, v| {
            b.iter(|| black_box(dfs_preorder(black_box(v), start)));
        });
        group.bench_with_input(BenchmarkId::new("bfs_dense", label), &view, |b, v| {
            b.iter(|| black_box(bfs_dense(black_box(v), start)));
        });
        group.bench_with_input(
            BenchmarkId::new("dfs_preorder_dense", label),
            &view,
            |b, v| b.iter(|| black_box(dfs_preorder_dense(black_box(v), start))),
        );
    }
    group.finish();
}

/// `remove_self_effect` — the diagonal correction applied after an implicit
/// clique step. Cheap and elementwise, but on the hot path of every
/// clique-style layer, so it is measured rather than assumed negligible.
fn bench_remove_self_effect(c: &mut Criterion) {
    let mut group = c.benchmark_group("mp_remove_self_effect");
    let cfg = CliqueStepCfg {
        use_abs: true,
        include_self: true,
    };
    for (label, spec) in scales() {
        let hg = spec.build();
        let n = hg.num_nodes();
        let diag = clique_diag(&hg, cfg.use_abs);
        let x = spec.node_vector();
        let y0 = vec![1.0f32; n];
        tune(&mut group, label);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("nodes", label), &n, |b, _| {
            b.iter_batched_ref(
                || y0.clone(),
                |y| remove_self_effect(black_box(y), black_box(&diag), black_box(&x)),
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Clique-GCN forward, and the adjacency construction it depends on, timed
/// separately so the one-off cost is not amortised into the per-step cost.
fn bench_clique_gcn_forward(c: &mut Criterion) {
    let mut group = c.benchmark_group("conv_clique_gcn_forward");
    for (label, spec) in scales() {
        let hg = spec.build();
        let n = hg.num_nodes();
        let inc = incidences(&hg);
        tune(&mut group, label);

        for &(d_in, d_out) in &[(8usize, 8usize), (32, 32)] {
            let layer = GcnCliqueLayer::<f32>::from_view(&hg, d_in, d_out);
            let x_in = spec.node_block(d_in);
            let mut x_out = vec![0.0f32; n * d_out];
            group.throughput(Throughput::Elements((inc * d_out) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{label}/d{d_in}x{d_out}"), inc),
                &x_in,
                |b, x| {
                    b.iter(|| {
                        layer.forward_relu(black_box(x), black_box(&mut x_out));
                    });
                },
            );
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Fixture construction vs. view construction, separated.
//
// Making `HyperSpec` produce a *coherent* declaration program (unified-ir-
// hyperspec-coherence) added real work to the fixture: one `DeclNode` per node,
// edge and arc, an interned name for each, a path `Index` over the named ones,
// and the arc-to-edge child links. None of that is on `try_from_ir`'s path, but
// all of it lands in a naive "build an IR and view it" measurement.
//
// So the two costs are reported apart, and the sum is reported too, because a
// caller that builds one view per IR pays the sum while a caller that keeps a
// resident IR pays only the view. Folding the fixture's cost into the view's
// regression number would attribute the wrong term.
// ---------------------------------------------------------------------------

fn bench_workload_ir_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("workload_ir_construction");

    for (label, spec) in scales() {
        let resident = spec.ir();
        let total: usize = resident.arcs.iter().map(|a| a.refs.len()).sum();
        let cfg = AggCfg {
            weight: WeightAgg::Sum,
            sign: SignAgg::PreferNonNeutral,
            clamp01: false,
        };
        let ex = ScalarWeightExtractor;

        group.throughput(Throughput::Elements(total as u64));
        tune(&mut group, label);

        // 1. Fixture construction alone.
        group.bench_function(BenchmarkId::new("spec_ir", label), |b| {
            b.iter(|| black_box(black_box(&spec).ir()));
        });

        // 2. Checked view construction over a resident IR.
        group.bench_function(BenchmarkId::new("try_from_ir_resident", label), |b| {
            b.iter(|| {
                black_box(
                    View::try_from_ir(black_box(&resident), black_box(&cfg), black_box(&ex))
                        .expect("workload IRs are coherent"),
                )
            });
        });

        // 3. Combined one-shot: build the IR and immediately view it.
        group.bench_function(BenchmarkId::new("combined_one_shot", label), |b| {
            b.iter(|| {
                let ir = black_box(&spec).ir();
                black_box(
                    View::try_from_ir(black_box(&ir), black_box(&cfg), black_box(&ex))
                        .expect("workload IRs are coherent"),
                )
            });
        });

        // 4. Compatibility view over the same resident IR — the difference
        //    against cell 2 is the `Result` wrapper and nothing else.
        group.bench_function(BenchmarkId::new("from_ir_resident", label), |b| {
            b.iter(|| {
                black_box(View::from_ir(
                    black_box(&resident),
                    black_box(&cfg),
                    black_box(&ex),
                ))
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_gather,
    bench_scatter,
    bench_clique_diag,
    bench_implicit_clique_step,
    bench_gather_paths,
    bench_scatter_paths,
    bench_hgnn_forward,
    bench_signed_hgnn_forward,
    bench_view_construction,
    bench_view_from_ir,
    bench_from_ir_weight_extraction,
    bench_workload_ir_construction,
    bench_expansions,
    bench_clique_expansion_growth,
    bench_explicit_clique_and_degrees,
    bench_traversal,
    bench_remove_self_effect,
    bench_clique_gcn_forward
);
criterion_main!(benches);
