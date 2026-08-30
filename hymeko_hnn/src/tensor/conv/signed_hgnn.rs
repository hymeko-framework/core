//! Level 3: Signed-HGNN — Separate positive/negative incidence channels.
//!
//! X' = σ(D⁻½ B₊ W₊ B₊ᵀ X Θ₊  +  D⁻½ B₋ W₋ B₋ᵀ X Θ₋)
//!
//! This is NOVEL — no prior art on signed-incidence-aware hypergraph
//! convolution with separate learnable transforms per sign channel.
//! The signed semantics from HyMeKo's IR (Plus/Minus/Neutral incidence)
//! map directly to excitatory/inhibitory message passing channels.
//!
//! # Feature blocking
//!
//! Structurally identical to the treatment in [`crate::tensor::conv::hgnn`],
//! and for the same reason: the forward pass used to run the whole dual-channel
//! pipeline once per output feature, allocating **five** temporaries each time
//! (`zp_col`, `zm_col`, `xe_plus`, `xe_minus`, `y`) — `5 · d_out` allocations
//! and `d_out` full traversals of the incidence structure.
//!
//! It now traverses once per *tile* of features, with the feature loop
//! innermost over contiguous slices, and allocation `O(1)` in `d_out`. The
//! scatter half walks the node-first incidence CSR so it parallelises over
//! nodes without atomics. Per-element summation order is unchanged throughout,
//! so results are bitwise identical to the pre-blocking implementation — see
//! `message_passing`'s module docs for the ordering argument.

use rayon::prelude::*;

use hymeko::tensor::common::Real;
use hymeko::tensor::conv::traits::inv_sqrt_degree;
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};

use crate::tensor::common::{PAR_BLOCKED_WORK_THRESHOLD, PAR_INCIDENCE_THRESHOLD, feature_tile};
use crate::traversal::hypergraphview::HyperGraphView;

pub struct SignedHgnnLayer<F: Real> {
    /// Θ₊: d_in × d_out (positive channel transform)
    theta_plus: Vec<F>,
    /// Θ₋: d_in × d_out (negative channel transform)
    theta_minus: Vec<F>,
    /// Per-edge learnable W₊, W₋
    w_plus: Vec<F>,
    w_minus: Vec<F>,
    /// Precomputed inverse sqrt degree for positive/negative subgraphs
    inv_sqrt_dv_plus: Vec<F>,
    inv_sqrt_dv_minus: Vec<F>,
    inv_de_plus: Vec<F>,
    inv_de_minus: Vec<F>,
    d_in: usize,
    d_out: usize,
    num_nodes: usize,
    num_edges: usize,
}

impl<F: Real> SignedHgnnLayer<F> {
    pub fn from_view<V, EW>(hg: &HyperGraphView<V, EW, F>, d_in: usize, d_out: usize) -> Self
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let nn = hg.num_nodes();
        let ne = hg.num_edges();

        // Compute separate degree vectors for +/- channels
        let mut dv_plus = vec![F::zero(); nn];
        let mut dv_minus = vec![F::zero(); nn];
        let mut de_plus = vec![F::zero(); ne];
        let mut de_minus = vec![F::zero(); ne];

        for e in 0..ne {
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].as_usize();
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
                        // Neutral: contribute to both
                        dv_plus[v] += w;
                        dv_minus[v] += w;
                        de_plus[e] += w;
                        de_minus[e] += w;
                    }
                }
            }
        }

        let inv_de = |d: &[F]| -> Vec<F> {
            d.iter()
                .map(|&x| {
                    if x > F::zero() {
                        F::one() / x
                    } else {
                        F::zero()
                    }
                })
                .collect()
        };

        let scale = F::from_other(1.0 / (d_in as f64).sqrt());

        Self {
            theta_plus: vec![scale; d_in * d_out],
            theta_minus: vec![scale; d_in * d_out],
            w_plus: vec![F::one(); ne],
            w_minus: vec![F::one(); ne],
            inv_sqrt_dv_plus: inv_sqrt_degree(&dv_plus),
            inv_sqrt_dv_minus: inv_sqrt_degree(&dv_minus),
            inv_de_plus: inv_de(&de_plus),
            inv_de_minus: inv_de(&de_minus),
            d_in,
            d_out,
            num_nodes: nn,
            num_edges: ne,
        }
    }

    /// Forward: dual-channel signed message passing.
    ///
    /// # Preconditions
    /// `x_in.len() == num_nodes * d_in` and `x_out.len() == num_nodes * d_out`.
    ///
    /// # Postconditions
    /// `x_out` fully overwritten. Bitwise identical to the pre-blocking
    /// implementation.
    pub fn forward_pass<V, EW>(&self, hg: &HyperGraphView<V, EW, F>, x_in: &[F], x_out: &mut [F])
    where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        debug_assert_eq!(x_in.len(), self.num_nodes * self.d_in);
        debug_assert_eq!(x_out.len(), self.num_nodes * self.d_out);
        if self.d_out == 0 || self.num_nodes == 0 {
            return;
        }

        // Z₊ = X · Θ₊,  Z₋ = X · Θ₋  (one pass over X, both channels)
        let mut z_plus = vec![F::zero(); self.num_nodes * self.d_out];
        let mut z_minus = vec![F::zero(); self.num_nodes * self.d_out];
        self.dense_transform_dual(x_in, &mut z_plus, &mut z_minus);

        let m = self.num_edges;
        let tile = feature_tile(m, self.d_out);
        let mut xe_plus = vec![F::zero(); m * tile];
        let mut xe_minus = vec![F::zero(); m * tile];

        let mut f0 = 0;
        while f0 < self.d_out {
            let width = tile.min(self.d_out - f0);
            let (sp, sm) = (&mut xe_plus[..m * width], &mut xe_minus[..m * width]);

            self.gather_tile(hg, &z_plus, &z_minus, f0, width, sp, sm);
            self.scale_tile(sp, sm, width);
            self.scatter_tile(hg, sp, sm, f0, width, x_out);

            f0 += width;
        }
    }

    /// `Z₊ = X · Θ₊` and `Z₋ = X · Θ₋`, row-parallel, one pass over `X`.
    ///
    /// Loop order `i → k → j` with `j` innermost over contiguous `Θ` rows.
    /// Each output element still accumulates over `k` in `0..d_in` order, so
    /// the result is unchanged bitwise.
    fn dense_transform_dual(&self, x_in: &[F], z_plus: &mut [F], z_minus: &mut [F]) {
        let d_out = self.d_out;
        let work = self.num_nodes * self.d_in.max(1) * d_out;

        let transform_row = |i: usize, zp_row: &mut [F], zm_row: &mut [F]| {
            for slot in zp_row.iter_mut() {
                *slot = F::zero();
            }
            for slot in zm_row.iter_mut() {
                *slot = F::zero();
            }
            let x_row = &x_in[i * self.d_in..(i + 1) * self.d_in];
            for (k, &xik) in x_row.iter().enumerate() {
                let tp = &self.theta_plus[k * d_out..(k + 1) * d_out];
                let tm = &self.theta_minus[k * d_out..(k + 1) * d_out];
                for ((zp, zm), (&p, &n)) in zp_row
                    .iter_mut()
                    .zip(zm_row.iter_mut())
                    .zip(tp.iter().zip(tm.iter()))
                {
                    *zp += xik * p;
                    *zm += xik * n;
                }
            }
        };

        if work >= PAR_BLOCKED_WORK_THRESHOLD {
            z_plus
                .par_chunks_mut(d_out)
                .zip(z_minus.par_chunks_mut(d_out))
                .enumerate()
                .for_each(|(i, (zp_row, zm_row))| transform_row(i, zp_row, zm_row));
        } else {
            for (i, (zp_row, zm_row)) in z_plus
                .chunks_mut(d_out)
                .zip(z_minus.chunks_mut(d_out))
                .enumerate()
            {
                transform_row(i, zp_row, zm_row);
            }
        }
    }

    /// Signed gather for one feature tile: only matching-sign incidences
    /// accumulate into a channel; neutral incidences feed both.
    #[allow(clippy::too_many_arguments)] // two output channels + tile window;
    // a config struct here would be threaded through the hot loop for no gain.
    fn gather_tile<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        z_plus: &[F],
        z_minus: &[F],
        f0: usize,
        width: usize,
        sp: &mut [F],
        sm: &mut [F],
    ) where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let d_out = self.d_out;

        let gather_row = |e: usize, xp_row: &mut [F], xm_row: &mut [F]| {
            for slot in xp_row.iter_mut() {
                *slot = F::zero();
            }
            for slot in xm_row.iter_mut() {
                *slot = F::zero();
            }
            let s = hg.edge_offsets[e];
            let eend = hg.edge_offsets[e + 1];
            for p in s..eend {
                let v = hg.flat_edge_nodes[p].as_usize();
                let w = hg.flat_edge_w[p].degree_mass();
                let base = v * d_out + f0;
                match hg.flat_edge_sign[p] {
                    1 => {
                        let zp = &z_plus[base..base + width];
                        for (acc, &z) in xp_row.iter_mut().zip(zp.iter()) {
                            *acc += w * z;
                        }
                    }
                    -1 => {
                        let zm = &z_minus[base..base + width];
                        for (acc, &z) in xm_row.iter_mut().zip(zm.iter()) {
                            *acc += w * z;
                        }
                    }
                    _ => {
                        let zp = &z_plus[base..base + width];
                        let zm = &z_minus[base..base + width];
                        for ((ap, am), (&p, &n)) in xp_row
                            .iter_mut()
                            .zip(xm_row.iter_mut())
                            .zip(zp.iter().zip(zm.iter()))
                        {
                            *ap += w * p;
                            *am += w * n;
                        }
                    }
                }
            }
        };

        if hg.flat_edge_nodes.len() * width >= PAR_INCIDENCE_THRESHOLD {
            sp.par_chunks_mut(width)
                .zip(sm.par_chunks_mut(width))
                .enumerate()
                .for_each(|(e, (xp_row, xm_row))| gather_row(e, xp_row, xm_row));
        } else {
            for (e, (xp_row, xm_row)) in sp.chunks_mut(width).zip(sm.chunks_mut(width)).enumerate()
            {
                gather_row(e, xp_row, xm_row);
            }
        }
    }

    /// Edge-level normalisation and per-edge weights, both channels.
    ///
    /// Kept as `(x · inv_de) · w`, matching the original association — folding
    /// `inv_de · w` first would perturb the result, floating-point
    /// multiplication not being associative.
    fn scale_tile(&self, sp: &mut [F], sm: &mut [F], width: usize) {
        for (e, (xp_row, xm_row)) in sp.chunks_mut(width).zip(sm.chunks_mut(width)).enumerate() {
            let (idp, wp) = (self.inv_de_plus[e], self.w_plus[e]);
            let (idm, wm) = (self.inv_de_minus[e], self.w_minus[e]);
            for slot in xp_row.iter_mut() {
                *slot = *slot * idp * wp;
            }
            for slot in xm_row.iter_mut() {
                *slot = *slot * idm * wm;
            }
        }
    }

    /// Signed scatter + combine + node normalisation + LeakyReLU, fused.
    ///
    /// Node-indexed output, so this walks the **node-first** CSR and
    /// parallelises over nodes. Contributions still arrive in increasing edge
    /// order per node, matching the old edge-major accumulation exactly.
    #[allow(clippy::too_many_arguments)] // see gather_tile
    fn scatter_tile<V, EW>(
        &self,
        hg: &HyperGraphView<V, EW, F>,
        sp: &[F],
        sm: &[F],
        f0: usize,
        width: usize,
        x_out: &mut [F],
    ) where
        V: IncVal<F>,
        EW: EdgeWeight<V, F>,
    {
        let d_out = self.d_out;
        let leak = F::from_other(0.01);

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
                let base = e * width;
                match hg.flat_node_sign[p] {
                    1 => {
                        let xp = &sp[base..base + width];
                        for (acc, &x) in dst.iter_mut().zip(xp.iter()) {
                            *acc += w * x;
                        }
                    }
                    -1 => {
                        let xm = &sm[base..base + width];
                        for (acc, &x) in dst.iter_mut().zip(xm.iter()) {
                            *acc += w * x;
                        }
                    }
                    _ => {
                        let xp = &sp[base..base + width];
                        let xm = &sm[base..base + width];
                        for (acc, (&a, &b)) in dst.iter_mut().zip(xp.iter().zip(xm.iter())) {
                            *acc += w * (a + b);
                        }
                    }
                }
            }

            // Node normalization + activation (LeakyReLU), fused while hot.
            let (pp, mm) = (self.inv_sqrt_dv_plus[v], self.inv_sqrt_dv_minus[v]);
            for slot in dst.iter_mut() {
                let y = *slot;
                let combined = pp * y + mm * y;
                *slot = if combined > F::zero() {
                    combined
                } else {
                    combined * leak
                };
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
