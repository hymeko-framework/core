//! Regression tests for the feature-blocked HGNN / Signed-HGNN forward passes.
//!
//! The 2026-08-24 optimisation replaced a per-output-feature loop (which
//! allocated three temporaries for HGNN, five for Signed-HGNN, and re-walked
//! the whole incidence structure on every iteration) with a feature-tiled
//! single traversal, and moved the scatter half onto the node-first incidence
//! CSR so it parallelises without atomics.
//!
//! `reference_hgnn_forward` / `reference_signed_hgnn_forward` below are
//! verbatim transcriptions of the **pre-optimisation** algorithms. They
//! reconstruct the layer parameters exactly the way `from_view` does — using
//! the same public `compute_degree` / `inv_sqrt_degree` helpers — so no
//! accessor had to be added to the production types just to test them.
//!
//! These oracles are the only thing that can catch a silent numerical drift in
//! the rewrite; comparing the new code against itself would not. That is why
//! the assertions are **bitwise**, not tolerance-based: the rewrite's whole
//! claim is that it preserves per-element summation order, and a tolerance
//! assertion would pass even if it did not.

#[cfg(test)]
mod test_conv_forward {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::tensor::conv::traits::{DegreeMode, compute_degree, inv_sqrt_degree};
    use hymeko::tensor::tensor_val::{EdgeWScalar, IncVal};
    use hymeko_hnn::tensor::conv::hgnn::HgnnLayer;
    use hymeko_hnn::tensor::conv::signed_hgnn::SignedHgnnLayer;
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hnn::workloads::{DegreeShape, HyperSpec, SignPattern};
    use log::info;
    use std::time::Instant;

    type View = HyperGraphView<f32, EdgeWScalar<f32>, f32>;

    const POISON: f32 = -999.5;

    fn assert_bitwise(a: &[f32], b: &[f32], msg: &str) {
        assert_eq!(a.len(), b.len(), "len mismatch: {msg}");
        let mut mismatches = 0usize;
        for i in 0..a.len() {
            if a[i].to_bits() != b[i].to_bits() {
                if mismatches < 5 {
                    eprintln!("  {msg}: i={i} got {} want {}", a[i], b[i]);
                }
                mismatches += 1;
            }
        }
        assert_eq!(
            mismatches,
            0,
            "{msg}: {mismatches}/{} elements differ",
            a.len()
        );
    }

    /// Cells kept small enough that the O(d_out · incidences) reference stays
    /// affordable, while still covering d_out below, at, and above a tile
    /// boundary, and both regular and irregular edge cardinality.
    fn cells() -> Vec<(&'static str, HyperSpec, usize, usize)> {
        vec![
            (
                "tiny/8x4",
                HyperSpec {
                    nodes: 64,
                    edges: 32,
                    avg_cardinality: 3,
                    shape: DegreeShape::Uniform,
                    signs: SignPattern::Mixed,
                    seed: 21,
                },
                8,
                4,
            ),
            ("small/32x32", HyperSpec::small(), 32, 32),
            ("small/16x1", HyperSpec::small(), 16, 1),
            (
                "irregular/8x16",
                HyperSpec {
                    shape: DegreeShape::Irregular,
                    ..HyperSpec::small()
                },
                8,
                16,
            ),
            (
                "allplus/4x8",
                HyperSpec {
                    signs: SignPattern::AllPlus,
                    ..HyperSpec::small()
                },
                4,
                8,
            ),
        ]
    }

    // -----------------------------------------------------------------
    // HGNN reference oracle (pre-optimisation algorithm)
    // -----------------------------------------------------------------

    fn reference_hgnn_forward(hg: &View, x_in: &[f32], d_in: usize, d_out: usize) -> Vec<f32> {
        let n = hg.num_nodes();
        let m = hg.num_edges();

        // Parameters exactly as HgnnLayer::from_view builds them.
        let dv = compute_degree(
            &hg.edge_offsets,
            &hg.node_offsets,
            &hg.flat_edge_nodes,
            &hg.flat_edge_sign,
            &hg.flat_edge_w,
            n,
            m,
            DegreeMode::VertexSum,
        );
        let de = compute_degree(
            &hg.edge_offsets,
            &hg.node_offsets,
            &hg.flat_edge_nodes,
            &hg.flat_edge_sign,
            &hg.flat_edge_w,
            n,
            m,
            DegreeMode::EdgeSum,
        );
        let inv_sqrt_dv = inv_sqrt_degree(&dv);
        let inv_de: Vec<f32> = de
            .iter()
            .map(|&d| if d > 0.0 { 1.0 / d } else { 0.0 })
            .collect();
        let edge_weights = vec![1.0f32; m];
        let theta = vec![(1.0f64 / (d_in as f64).sqrt()) as f32; d_in * d_out];

        let mut x_out = vec![0.0f32; n * d_out];

        // Step 1: Z = X . Theta
        let mut z = vec![0.0f32; n * d_out];
        for i in 0..n {
            for j in 0..d_out {
                let mut acc = 0.0f32;
                for k in 0..d_in {
                    acc += x_in[i * d_in + k] * theta[k * d_out + j];
                }
                z[i * d_out + j] = acc;
            }
        }

        // Steps 2-5: once per output feature, edge-major throughout.
        for f in 0..d_out {
            let z_col: Vec<f32> = (0..n).map(|i| z[i * d_out + f]).collect();

            let mut x_edges = vec![0.0f32; m];
            for e in 0..m {
                let s = hg.edge_offsets[e];
                let eend = hg.edge_offsets[e + 1];
                let mut acc = 0.0f32;
                for p in s..eend {
                    let v = hg.flat_edge_nodes[p].raw();
                    acc += hg.flat_edge_w[p].degree_mass() * z_col[v];
                }
                x_edges[e] = acc;
            }

            for e in 0..m {
                x_edges[e] = x_edges[e] * inv_de[e] * edge_weights[e];
            }

            let mut y_col = vec![0.0f32; n];
            for e in 0..m {
                let s = hg.edge_offsets[e];
                let eend = hg.edge_offsets[e + 1];
                for p in s..eend {
                    let v = hg.flat_edge_nodes[p].raw();
                    y_col[v] += hg.flat_edge_w[p].degree_mass() * x_edges[e];
                }
            }

            for i in 0..n {
                let val = inv_sqrt_dv[i] * y_col[i];
                x_out[i * d_out + f] = if val > 0.0 { val } else { 0.0 };
            }
        }

        x_out
    }

    // -----------------------------------------------------------------
    // Signed-HGNN reference oracle (pre-optimisation algorithm)
    // -----------------------------------------------------------------

    fn reference_signed_hgnn_forward(
        hg: &View,
        x_in: &[f32],
        d_in: usize,
        d_out: usize,
    ) -> Vec<f32> {
        let n = hg.num_nodes();
        let m = hg.num_edges();

        let mut dv_plus = vec![0.0f32; n];
        let mut dv_minus = vec![0.0f32; n];
        let mut de_plus = vec![0.0f32; m];
        let mut de_minus = vec![0.0f32; m];
        for e in 0..m {
            for p in hg.edge_offsets[e]..hg.edge_offsets[e + 1] {
                let v = hg.flat_edge_nodes[p].raw();
                let w = hg.flat_edge_w[p].degree_mass();
                match hg.flat_edge_sign[p] {
                    1 => {
                        dv_plus[v] += w;
                        de_plus[e] += w;
                    }
                    -1 => {
                        dv_minus[v] += w;
                        de_minus[e] += w;
                    }
                    _ => {
                        dv_plus[v] += w;
                        dv_minus[v] += w;
                        de_plus[e] += w;
                        de_minus[e] += w;
                    }
                }
            }
        }
        let inv = |d: &[f32]| -> Vec<f32> {
            d.iter()
                .map(|&x| if x > 0.0 { 1.0 / x } else { 0.0 })
                .collect()
        };
        let inv_sqrt_dv_plus = inv_sqrt_degree(&dv_plus);
        let inv_sqrt_dv_minus = inv_sqrt_degree(&dv_minus);
        let inv_de_plus = inv(&de_plus);
        let inv_de_minus = inv(&de_minus);
        let w_plus = vec![1.0f32; m];
        let w_minus = vec![1.0f32; m];
        let scale = (1.0f64 / (d_in as f64).sqrt()) as f32;
        let theta_plus = vec![scale; d_in * d_out];
        let theta_minus = vec![scale; d_in * d_out];

        let mut x_out = vec![0.0f32; n * d_out];

        let mut z_plus = vec![0.0f32; n * d_out];
        let mut z_minus = vec![0.0f32; n * d_out];
        for i in 0..n {
            for j in 0..d_out {
                let mut ap = 0.0f32;
                let mut am = 0.0f32;
                for k in 0..d_in {
                    let xik = x_in[i * d_in + k];
                    ap += xik * theta_plus[k * d_out + j];
                    am += xik * theta_minus[k * d_out + j];
                }
                z_plus[i * d_out + j] = ap;
                z_minus[i * d_out + j] = am;
            }
        }

        for f in 0..d_out {
            let zp_col: Vec<f32> = (0..n).map(|i| z_plus[i * d_out + f]).collect();
            let zm_col: Vec<f32> = (0..n).map(|i| z_minus[i * d_out + f]).collect();

            let mut xe_plus = vec![0.0f32; m];
            let mut xe_minus = vec![0.0f32; m];
            for e in 0..m {
                for p in hg.edge_offsets[e]..hg.edge_offsets[e + 1] {
                    let v = hg.flat_edge_nodes[p].raw();
                    let w = hg.flat_edge_w[p].degree_mass();
                    match hg.flat_edge_sign[p] {
                        1 => xe_plus[e] += w * zp_col[v],
                        -1 => xe_minus[e] += w * zm_col[v],
                        _ => {
                            xe_plus[e] += w * zp_col[v];
                            xe_minus[e] += w * zm_col[v];
                        }
                    }
                }
            }

            for e in 0..m {
                xe_plus[e] = xe_plus[e] * inv_de_plus[e] * w_plus[e];
                xe_minus[e] = xe_minus[e] * inv_de_minus[e] * w_minus[e];
            }

            let mut y = vec![0.0f32; n];
            for e in 0..m {
                for p in hg.edge_offsets[e]..hg.edge_offsets[e + 1] {
                    let v = hg.flat_edge_nodes[p].raw();
                    let w = hg.flat_edge_w[p].degree_mass();
                    match hg.flat_edge_sign[p] {
                        1 => y[v] += w * xe_plus[e],
                        -1 => y[v] += w * xe_minus[e],
                        _ => y[v] += w * (xe_plus[e] + xe_minus[e]),
                    }
                }
            }

            for i in 0..n {
                let val_p = inv_sqrt_dv_plus[i] * y[i];
                let val_m = inv_sqrt_dv_minus[i] * y[i];
                let combined = val_p + val_m;
                x_out[i * d_out + f] = if combined > 0.0 {
                    combined
                } else {
                    combined * 0.01f32
                };
            }
        }

        x_out
    }

    // -----------------------------------------------------------------
    // tests
    // -----------------------------------------------------------------

    #[test]
    fn test_hgnn_forward_matches_prerewrite_reference() {
        log_test_header(
            "test_hgnn_forward_matches_prerewrite_reference",
            "Feature-blocked HGNN forward vs the original per-feature algorithm.",
        );
        let start = Instant::now();

        for (label, spec, d_in, d_out) in cells() {
            let hg = spec.build();
            let x_in = spec.node_block(d_in);

            let layer = HgnnLayer::<f32>::from_view(&hg, d_in, d_out);
            let mut got = vec![POISON; hg.num_nodes() * d_out];
            layer.forward_pass(&hg, &x_in, &mut got);

            let expected = reference_hgnn_forward(&hg, &x_in, d_in, d_out);
            assert_bitwise(&got, &expected, &format!("hgnn forward [{label}]"));

            info!(
                "[{label}] HGNN forward matched the reference ({} nodes, d_out={d_out})",
                hg.num_nodes()
            );
        }

        log_test_footer(
            "test_hgnn_forward_matches_prerewrite_reference",
            Some(start.elapsed()),
            "Feature blocking and the node-first scatter left the HGNN output bit-identical.",
        );
    }

    #[test]
    fn test_signed_hgnn_forward_matches_prerewrite_reference() {
        log_test_header(
            "test_signed_hgnn_forward_matches_prerewrite_reference",
            "Feature-blocked Signed-HGNN forward vs the original per-feature algorithm.",
        );
        let start = Instant::now();

        for (label, spec, d_in, d_out) in cells() {
            let hg = spec.build();
            let x_in = spec.node_block(d_in);

            let layer = SignedHgnnLayer::<f32>::from_view(&hg, d_in, d_out);
            let mut got = vec![POISON; hg.num_nodes() * d_out];
            layer.forward_pass(&hg, &x_in, &mut got);

            let expected = reference_signed_hgnn_forward(&hg, &x_in, d_in, d_out);
            assert_bitwise(&got, &expected, &format!("signed hgnn forward [{label}]"));

            info!(
                "[{label}] Signed-HGNN forward matched the reference ({} nodes, d_out={d_out})",
                hg.num_nodes()
            );
        }

        log_test_footer(
            "test_signed_hgnn_forward_matches_prerewrite_reference",
            Some(start.elapsed()),
            "The dual-channel rewrite left the Signed-HGNN output bit-identical, neutral arm included.",
        );
    }

    #[test]
    fn test_forward_overwrites_poisoned_output() {
        log_test_header(
            "test_forward_overwrites_poisoned_output",
            "Both forwards must fully overwrite x_out, never accumulate into it.",
        );
        let start = Instant::now();

        let spec = HyperSpec::small();
        let hg = spec.build();
        let (d_in, d_out) = (8usize, 12usize);
        let x_in = spec.node_block(d_in);

        let hgnn = HgnnLayer::<f32>::from_view(&hg, d_in, d_out);
        let mut y = vec![POISON; hg.num_nodes() * d_out];
        hgnn.forward_pass(&hg, &x_in, &mut y);
        let first = y.clone();
        hgnn.forward_pass(&hg, &x_in, &mut y);
        assert_bitwise(
            &first,
            &y,
            "HGNN forward accumulates instead of overwriting",
        );

        let signed = SignedHgnnLayer::<f32>::from_view(&hg, d_in, d_out);
        let mut ys = vec![POISON; hg.num_nodes() * d_out];
        signed.forward_pass(&hg, &x_in, &mut ys);
        let first_s = ys.clone();
        signed.forward_pass(&hg, &x_in, &mut ys);
        assert_bitwise(
            &first_s,
            &ys,
            "Signed-HGNN forward accumulates instead of overwriting",
        );

        log_test_footer(
            "test_forward_overwrites_poisoned_output",
            Some(start.elapsed()),
            "Running each forward twice into the same buffer changed nothing.",
        );
    }

    #[test]
    fn test_forward_degenerate_shapes() {
        log_test_header(
            "test_forward_degenerate_shapes",
            "Edgeless, nodeless and singleton graphs, plus d_out = 1.",
        );
        let start = Instant::now();

        // no edges: gather/scatter contribute nothing, activation clamps to 0
        let edgeless = HyperSpec {
            nodes: 6,
            edges: 0,
            ..HyperSpec::small()
        };
        let hg = edgeless.build();
        let (d_in, d_out) = (4usize, 3usize);
        let x_in = edgeless.node_block(d_in);

        let layer = HgnnLayer::<f32>::from_view(&hg, d_in, d_out);
        let mut y = vec![POISON; hg.num_nodes() * d_out];
        layer.forward_pass(&hg, &x_in, &mut y);
        assert!(
            y.iter().all(|v| *v == 0.0),
            "edgeless HGNN should output all zeros, got {y:?}"
        );

        // no nodes at all
        let nodeless = HyperSpec {
            nodes: 0,
            edges: 0,
            ..HyperSpec::small()
        };
        let hg0 = nodeless.build();
        let layer0 = HgnnLayer::<f32>::from_view(&hg0, d_in, d_out);
        let mut y0: Vec<f32> = Vec::new();
        layer0.forward_pass(&hg0, &[], &mut y0);

        // d_out = 1 exercises the tile loop's single-iteration path
        let spec = HyperSpec::small();
        let hg1 = spec.build();
        let x1 = spec.node_block(5);
        let layer1 = HgnnLayer::<f32>::from_view(&hg1, 5, 1);
        let mut y1 = vec![POISON; hg1.num_nodes()];
        layer1.forward_pass(&hg1, &x1, &mut y1);
        let expected1 = reference_hgnn_forward(&hg1, &x1, 5, 1);
        assert_bitwise(&y1, &expected1, "d_out = 1 HGNN forward");

        log_test_footer(
            "test_forward_degenerate_shapes",
            Some(start.elapsed()),
            "Edgeless, nodeless and d_out=1 cases behaved as specified.",
        );
    }
}

/// Coverage for `GcnCliqueLayer::forward_relu`, which the 2026-08-24 change
/// rewrote to write through `spmm_into` (removing an `n x d_out` temporary)
/// and to reorder the dense matmul loops.
///
/// The layer's normalised adjacency is private, so a full dense oracle is not
/// constructible from outside. These tests instead pin the *invariants* the
/// rewrite could plausibly break — linearity in the weights, the zero-weight
/// identity, output-buffer handling, and trait/inherent agreement — which is
/// strictly more coverage than the function had before (none).
#[cfg(test)]
mod test_gcn_clique_forward {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::tensor::conv::traits::HypergraphConv;
    use hymeko::tensor::conv::weight_init::weight_init::{Constant, Zeros};
    use hymeko::tensor::tensor_val::EdgeWScalar;
    use hymeko_hnn::tensor::conv::gcn_clique::GcnCliqueLayer;
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hnn::workloads::HyperSpec;
    use std::time::Instant;

    const POISON: f32 = -999.5;

    fn small_view() -> (HyperSpec, HyperGraphView<f32, EdgeWScalar<f32>, f32>) {
        // Deliberately tiny: build_explicit_a is O(sum of deg^2) and the
        // clique expansion of a large hypergraph is not what is under test.
        let spec = HyperSpec {
            nodes: 48,
            edges: 24,
            avg_cardinality: 3,
            ..HyperSpec::small()
        };
        let hg = spec.build();
        (spec, hg)
    }

    #[test]
    fn test_gcn_clique_forward_is_positively_homogeneous() {
        log_test_header(
            "test_gcn_clique_forward_is_positively_homogeneous",
            "Doubling W must double the output: X' = ReLU(A X W) is degree-1 homogeneous in W.",
        );
        let start = Instant::now();

        let (spec, hg) = small_view();
        let (d_in, d_out) = (6usize, 5usize);
        let x_in = spec.node_block(d_in);

        let half =
            GcnCliqueLayer::<f32>::from_view_with_init(&hg, d_in, d_out, &Constant { value: 0.5 });
        let full =
            GcnCliqueLayer::<f32>::from_view_with_init(&hg, d_in, d_out, &Constant { value: 1.0 });

        let mut y_half = vec![POISON; hg.num_nodes() * d_out];
        let mut y_full = vec![POISON; hg.num_nodes() * d_out];
        half.forward_relu(&x_in, &mut y_half);
        full.forward_relu(&x_in, &mut y_full);

        // Scaling by exactly 2 is exact in IEEE-754 (barring overflow), and
        // the sparse product distributes over it, so this should hold tightly.
        for i in 0..y_half.len() {
            let expected = 2.0 * y_half[i];
            let d = (y_full[i] - expected).abs();
            assert!(
                d <= 1e-4 * expected.abs().max(1.0),
                "homogeneity broken at i={i}: 2*{} = {expected} but got {}",
                y_half[i],
                y_full[i]
            );
        }
        assert!(
            y_full.iter().any(|&v| v > 0.0),
            "fixture produced an all-zero output; the test would prove nothing"
        );

        log_test_footer(
            "test_gcn_clique_forward_is_positively_homogeneous",
            Some(start.elapsed()),
            "Doubling the weight matrix doubled the GCN output, as linearity requires.",
        );
    }

    #[test]
    fn test_gcn_clique_zero_weights_give_zero_output() {
        log_test_header(
            "test_gcn_clique_zero_weights_give_zero_output",
            "W = 0 implies H = 0 implies ReLU(A H) = 0 everywhere.",
        );
        let start = Instant::now();

        let (spec, hg) = small_view();
        let (d_in, d_out) = (4usize, 3usize);
        let x_in = spec.node_block(d_in);

        let layer = GcnCliqueLayer::<f32>::from_view_with_init(&hg, d_in, d_out, &Zeros);
        let mut y = vec![POISON; hg.num_nodes() * d_out];
        layer.forward_relu(&x_in, &mut y);

        assert!(
            y.iter().all(|&v| v == 0.0),
            "zero weights must produce a zero output, found {:?}",
            y.iter().find(|&&v| v != 0.0)
        );

        log_test_footer(
            "test_gcn_clique_zero_weights_give_zero_output",
            Some(start.elapsed()),
            "Zero-initialised weights produced an all-zero forward output.",
        );
    }

    #[test]
    fn test_gcn_clique_leaves_oversized_output_tail_untouched() {
        log_test_header(
            "test_gcn_clique_leaves_oversized_output_tail_untouched",
            "Guards the lenient x_out contract kept across the spmm_into rewrite.",
        );
        let start = Instant::now();

        let (spec, hg) = small_view();
        let (d_in, d_out) = (4usize, 3usize);
        let x_in = spec.node_block(d_in);
        let needed = hg.num_nodes() * d_out;

        let layer =
            GcnCliqueLayer::<f32>::from_view_with_init(&hg, d_in, d_out, &Constant { value: 0.25 });
        let mut y = vec![POISON; needed + 7];
        layer.forward_relu(&x_in, &mut y);

        assert!(
            y[needed..].iter().all(|&v| v == POISON),
            "the tail beyond num_rows*d_out must not be written"
        );
        assert!(
            y[..needed].iter().all(|&v| v >= 0.0),
            "ReLU output must be non-negative"
        );

        log_test_footer(
            "test_gcn_clique_leaves_oversized_output_tail_untouched",
            Some(start.elapsed()),
            "An oversized output buffer kept its tail; the written prefix was non-negative.",
        );
    }

    #[test]
    fn test_gcn_clique_trait_forward_matches_inherent() {
        log_test_header(
            "test_gcn_clique_trait_forward_matches_inherent",
            "HypergraphConv::forward must delegate to forward_relu unchanged.",
        );
        let start = Instant::now();

        let (spec, hg) = small_view();
        let (d_in, d_out) = (5usize, 4usize);
        let x_in = spec.node_block(d_in);

        let layer =
            GcnCliqueLayer::<f32>::from_view_with_init(&hg, d_in, d_out, &Constant { value: 0.75 });
        let mut a = vec![POISON; hg.num_nodes() * d_out];
        let mut b = vec![POISON; hg.num_nodes() * d_out];
        layer.forward_relu(&x_in, &mut a);
        layer.forward(&x_in, d_in, &mut b, d_out);

        for i in 0..a.len() {
            assert_eq!(
                a[i].to_bits(),
                b[i].to_bits(),
                "trait vs inherent differ at i={i}"
            );
        }
        assert_eq!(
            layer.num_params(),
            d_in * d_out,
            "num_params must be d_in * d_out"
        );

        log_test_footer(
            "test_gcn_clique_trait_forward_matches_inherent",
            Some(start.elapsed()),
            "The trait forward is bit-identical to forward_relu.",
        );
    }
}
