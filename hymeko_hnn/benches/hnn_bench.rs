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

use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
use hymeko_hnn::tensor::conv::hgnn::HgnnLayer;
use hymeko_hnn::tensor::conv::signed_hgnn::SignedHgnnLayer;
use hymeko_hnn::tensor::message_passing::{
    CliqueStepCfg, clique_diag, gather_edges_from_nodes, gather_edges_from_nodes_parallel,
    gather_edges_from_nodes_serial, implicit_clique_step, scatter_nodes_from_edges,
    scatter_nodes_from_edges_parallel, scatter_nodes_from_edges_serial,
};
use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
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
    bench_from_ir_weight_extraction
);
criterion_main!(benches);
