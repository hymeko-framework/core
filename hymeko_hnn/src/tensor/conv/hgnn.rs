//! Level 2: HGNN — Hypergraph Neural Network (Feng et al. 2019)
//!
//! X' = σ(D_v⁻½ · B · W_e · D_e⁻¹ · Bᵀ · X · Θ)
//!
//! Uses the two-step gather/scatter from message_passing.rs
//! with degree normalization inserted between steps.
//!
//! # Feature blocking
//!
//! The forward pass used to run the whole gather → scale → scatter pipeline
//! once **per output feature**, allocating three temporaries (`z_col`,
//! `x_edges`, `y_col`) on each iteration. That is `3 · d_out` allocations and
//! `d_out` full traversals of the incidence structure, to produce a result
//! whose per-feature work is a single multiply-add.
//!
//! It now walks the incidence structure once per *tile* of features, keeping
//! the feature loop innermost over contiguous, equal-length slices — the shape
//! LLVM can vectorise. Allocation is `O(1)` in `d_out`: one `Z` buffer and one
//! edge scratch buffer, the latter capped by
//! [`crate::tensor::common::feature_tile`] so scratch does not grow without
//! bound on very large graphs.
//!
//! The scatter half additionally walks the **node-first** incidence CSR, so it
//! parallelises over nodes with no atomics. See `message_passing`'s module docs
//! for why the per-element summation order — and therefore the numerical
//! result, bitwise — is unchanged by any of this.

use rayon::prelude::*;

use hymeko::tensor::common::Real;
use hymeko::tensor::conv::traits::{DegreeMode, compute_degree_by_index, inv_sqrt_degree};
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};

use crate::tensor::common::{PAR_BLOCKED_WORK_THRESHOLD, PAR_INCIDENCE_THRESHOLD, feature_tile};
use crate::traversal::hypergraphview::HyperGraphView;

pub struct HgnnLayer<F: Real> {
    /// Per-edge learnable weights W_e: num_edges
    edge_weights: Vec<F>,
    /// Feature transform Θ: d_in × d_out
    theta: Vec<F>,
    d_in: usize,
    d_out: usize,
    /// Precomputed D_v^{-1/2}
    inv_sqrt_dv: Vec<F>,
    /// Precomputed D_e^{-1}
    inv_de: Vec<F>,
    num_nodes: usize,
    num_edges: usize,
}

impl<F: Real> HgnnLayer<F> {
    pub fn from_view<V, EW>(hg: &HyperGraphView<V, EW, F>, d_in: usize, d_out: usize) -> Self
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let nn = hg.num_nodes();
        let ne = hg.num_edges();

        let dv = compute_degree_by_index(
            &hg.edge_offsets,
            &hg.flat_edge_nodes,
            &hg.flat_edge_w,
            nn,
            ne,
            |d| d.as_usize(),
            DegreeMode::VertexSum,
        );
        let de = compute_degree_by_index(
            &hg.edge_offsets,
            &hg.flat_edge_nodes,
            &hg.flat_edge_w,
            nn,
            ne,
            |d| d.as_usize(),
            DegreeMode::EdgeSum,
        );

        Self {
            edge_weights: vec![F::one(); ne],
            theta: vec![F::from_other(1.0 / (d_in as f64).sqrt()); d_in * d_out],
            d_in,
            d_out,
            inv_sqrt_dv: inv_sqrt_degree(&dv),
            inv_de: de
                .iter()
                .map(|&d| {
                    if d > F::zero() {
                        F::one() / d
                    } else {
                        F::zero()
                    }
                })
                .collect(),
            num_nodes: nn,
            num_edges: ne,
        }
    }

    /// Forward pass: 5-step pipeline
    ///
    /// 1. Z = X · Θ                  (feature transform)
    /// 2. x_e = Bᵀ · Z              (gather: nodes → edges)
    /// 3. x_e *= D_e⁻¹ · W_e        (normalize + edge weight)
    /// 4. Y = B · x_e                (scatter: edges → nodes)
    /// 5. Y = D_v⁻½ · Y, then ReLU  (node normalization + activation)
    ///
    /// Steps 2–5 are executed per tile of output features rather than per
    /// feature, and steps 4 and 5 are fused so the normalisation and activation
    /// hit `x_out` while it is still hot.
    ///
    /// # Preconditions
    /// `x_in.len() == num_nodes * d_in` and `x_out.len() == num_nodes * d_out`.
    ///
    /// # Postconditions
    /// `x_out` fully overwritten. Bitwise identical to the pre-blocking
    /// implementation.
    pub fn forward_pass<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        x_in: &[F],      // n × d_in flat
        x_out: &mut [F], // n × d_out flat
    ) where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        debug_assert_eq!(x_in.len(), self.num_nodes * self.d_in);
        debug_assert_eq!(x_out.len(), self.num_nodes * self.d_out);
        if self.d_out == 0 || self.num_nodes == 0 {
            return;
        }

        // Step 1: Z = X · Θ  (n×d_in × d_in×d_out → n×d_out)
        let mut z = vec![F::zero(); self.num_nodes * self.d_out];
        self.dense_transform(x_in, &mut z);

        // Steps 2–5, feature-tiled.
        let m = self.num_edges;
        let tile = feature_tile(m, self.d_out);
        let mut edge_scratch = vec![F::zero(); m * tile];

        let mut f0 = 0;
        while f0 < self.d_out {
            let width = tile.min(self.d_out - f0);
            let scratch = &mut edge_scratch[..m * width];

            self.gather_tile(hg, &z, f0, width, scratch);
            self.scale_tile(scratch, width);
            self.scatter_tile(hg, scratch, f0, width, x_out);

            f0 += width;
        }
    }

    /// Step 1: `Z = X · Θ`, row-parallel.
    ///
    /// Loop order is `i → k → j` with the `j` (output-feature) loop innermost
    /// over a contiguous `Θ` row. The previous `i → j → k` order strode `Θ` by
    /// `d_out` on every step and could not vectorise. Each `Z[i][j]` still
    /// accumulates over `k` in `0..d_in` order, so the result is unchanged
    /// bitwise.
    fn dense_transform(&self, x_in: &[F], z: &mut [F]) {
        let work = self.num_nodes * self.d_in.max(1) * self.d_out;
        if work >= PAR_BLOCKED_WORK_THRESHOLD {
            z.par_chunks_mut(self.d_out)
                .enumerate()
                .for_each(|(i, z_row)| self.transform_row(x_in, i, z_row));
        } else {
            for (i, z_row) in z.chunks_mut(self.d_out).enumerate() {
                self.transform_row(x_in, i, z_row);
            }
        }
    }

    #[inline(always)]
    fn transform_row(&self, x_in: &[F], i: usize, z_row: &mut [F]) {
        for slot in z_row.iter_mut() {
            *slot = F::zero();
        }
        let x_row = &x_in[i * self.d_in..(i + 1) * self.d_in];
        for (k, &xik) in x_row.iter().enumerate() {
            let theta_row = &self.theta[k * self.d_out..(k + 1) * self.d_out];
            for (zj, &tj) in z_row.iter_mut().zip(theta_row.iter()) {
                *zj += xik * tj;
            }
        }
    }

    /// Step 2: `x_e[e][f] = Σ_v B[v,e] · Z[v][f0 + f]` for one feature tile.
    ///
    /// Edge-indexed output, so this walks the edge-first CSR and partitions
    /// over edges.
    fn gather_tile<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        z: &[F],
        f0: usize,
        width: usize,
        scratch: &mut [F],
    ) where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let d_out = self.d_out;
        let gather_row = |e: usize, xe_row: &mut [F]| {
            for slot in xe_row.iter_mut() {
                *slot = F::zero();
            }
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].as_usize();
                let w = hg.flat_edge_w[p].degree_mass();
                let z_row = &z[v * d_out + f0..v * d_out + f0 + width];
                for (acc, &zz) in xe_row.iter_mut().zip(z_row.iter()) {
                    *acc += w * zz;
                }
            }
        };

        if hg.flat_edge_nodes.len() * width >= PAR_INCIDENCE_THRESHOLD {
            scratch
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(e, xe_row)| gather_row(e, xe_row));
        } else {
            for (e, xe_row) in scratch.chunks_mut(width).enumerate() {
                gather_row(e, xe_row);
            }
        }
    }

    /// Step 3: `x_e *= D_e⁻¹ · W_e`.
    ///
    /// Kept as two separate multiplications in the original order —
    /// `(x · inv_de) · w_e`, not `x · (inv_de · w_e)` — because floating-point
    /// multiplication is not associative and precomputing the product would
    /// perturb the result.
    fn scale_tile(&self, scratch: &mut [F], width: usize) {
        for (e, xe_row) in scratch.chunks_mut(width).enumerate() {
            let inv_de = self.inv_de[e];
            let w_e = self.edge_weights[e];
            for slot in xe_row.iter_mut() {
                *slot = *slot * inv_de * w_e;
            }
        }
    }

    /// Steps 4 + 5 fused: `x_out[v][f0 + f] = ReLU(D_v⁻½ · Σ_e B[v,e] · x_e[e][f])`.
    ///
    /// Node-indexed output, so this walks the **node-first** CSR and partitions
    /// over nodes — no atomics, and each node's contributions still arrive in
    /// increasing edge order, matching the old edge-major accumulation exactly.
    fn scatter_tile<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        scratch: &[F],
        f0: usize,
        width: usize,
        x_out: &mut [F],
    ) where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let d_out = self.d_out;
        let inv_sqrt_dv = &self.inv_sqrt_dv;

        let scatter_row = |v: usize, out_row: &mut [F]| {
            let dst = &mut out_row[f0..f0 + width];
            for slot in dst.iter_mut() {
                *slot = F::zero();
            }
            let s = hg.node_offsets[v];
            let nend = hg.node_offsets[v + 1];
            for p in s..nend {
                let e = hg.flat_node_edges[p].as_usize();
                let w = hg.flat_node_w[p].degree_mass();
                let xe_row = &scratch[e * width..(e + 1) * width];
                for (acc, &xe) in dst.iter_mut().zip(xe_row.iter()) {
                    *acc += w * xe;
                }
            }
            // Step 5: D_v⁻½ normalization + ReLU, fused while `dst` is hot.
            let scale = inv_sqrt_dv[v];
            for slot in dst.iter_mut() {
                let val = scale * *slot;
                *slot = if val > F::zero() { val } else { F::zero() };
            }
        };

        if hg.flat_node_edges.len() * width >= PAR_INCIDENCE_THRESHOLD {
            x_out
                .par_chunks_mut(d_out)
                .enumerate()
                .for_each(|(v, out_row)| scatter_row(v, out_row));
        } else {
            for (v, out_row) in x_out.chunks_mut(d_out).enumerate() {
                scatter_row(v, out_row);
            }
        }
    }
}
