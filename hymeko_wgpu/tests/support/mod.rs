//! Shared scaffolding for the sparse-adapter and parity suites.
//!
//! Three suites — `scatter_mean_csr`, `highway_identity`, `resident_hsikan_chain` — need the same
//! three things: an inverse incidence built the way `CycleIncidence` builds one, a route onto the
//! device through the **validated** core contract, and a parity assertion with a declared bound.
//! Written three times they would be three chances to drift, and the duplication gate sees them as
//! clones because they are.
//!
//! This owns no behaviour: it constructs inputs, calls production entry points, and compares. The
//! general builder-family refactor is deliberately NOT here — it is a separate follow-up before the
//! next WGPU primitive.

#![allow(dead_code)]

use hymeko_tensor::representations::sparse::SparseTensorContract;
use hymeko_tensor::representations::tensor_csr::TensorCsr;
use hymeko_wgpu::{ComputeError, GpuCsrMatrix, ResourceFactory, WgpuBackend};

/// The CPU/WGPU parity bound, **declared before any number was measured** and never relaxed.
pub const ABS_BOUND: f32 = 1.0e-6;

/// An inverse incidence: for each node, the hyperedges naming it.
///
/// Built exactly as the CPU owner builds one — a vertex listed twice in an edge appears twice,
/// because the mean's denominator counts incidences, not distinct edges.
pub struct Incidence {
    pub n_nodes: usize,
    pub n_edges: usize,
    pub row_ptr: Vec<usize>,
    pub col_ind: Vec<usize>,
    pub counts: Vec<f32>,
}

impl Incidence {
    /// From an edge -> member-vertex listing.
    pub fn from_edges(n_nodes: usize, edges: &[Vec<usize>]) -> Self {
        let mut rows: Vec<Vec<usize>> = vec![Vec::new(); n_nodes];
        for (edge, members) in edges.iter().enumerate() {
            for &v in members {
                rows[v].push(edge);
            }
        }
        let mut row_ptr = vec![0usize];
        let mut col_ind = Vec::new();
        for row in &rows {
            col_ind.extend_from_slice(row);
            row_ptr.push(col_ind.len());
        }
        let counts = rows.iter().map(|r| r.len() as f32).collect();
        Self {
            n_nodes,
            n_edges: edges.len(),
            row_ptr,
            col_ind,
            counts,
        }
    }

    /// The core-owned sparse tensor for this incidence: rows = nodes, cols = edges.
    ///
    /// Values are all `1.0` and are **not consumed** by the unweighted mean; they exist because the
    /// CSR layout carries a value per entry.
    pub fn tensor(&self) -> TensorCsr<f32> {
        TensorCsr {
            num_rows: self.n_nodes,
            num_cols: self.n_edges.max(1),
            row_ptr: self.row_ptr.clone(),
            col_ind: self.col_ind.clone(),
            val: vec![1.0; self.col_ind.len()],
        }
    }

    /// Validate through **core** and upload. The device adapter re-checks nothing structural.
    ///
    /// # Errors
    /// Core's [`SparseViolation`] surfaces as a panic here (a malformed fixture is a test bug);
    /// device errors propagate.
    pub fn upload(
        &self,
        resources: &ResourceFactory<'_>,
        label: &str,
    ) -> Result<GpuCsrMatrix<f32>, ComputeError> {
        // The tensor must outlive the proof that borrows it, which is exactly the binding this
        // fixture is exercising: `validated` cannot be handed a different tensor.
        let tensor = self.tensor();
        let validated = tensor
            .validate()
            .unwrap_or_else(|violation| panic!("{label}: the fixture is malformed: {violation}"));
        resources.upload_validated_csr(label, &validated)
    }

    /// The CPU owner's pooling, in its order: accumulate in CSR order, then multiply by the
    /// reciprocal of the incidence count. A division would be a different float operation.
    pub fn reference_mean(&self, edge_state: &[f32], hidden: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; self.n_nodes * hidden];
        for node in 0..self.n_nodes {
            let (first, last) = (self.row_ptr[node], self.row_ptr[node + 1]);
            let row = &mut out[node * hidden..(node + 1) * hidden];
            for &edge in &self.col_ind[first..last] {
                let base = edge * hidden;
                for (slot, &x) in row.iter_mut().zip(&edge_state[base..base + hidden]) {
                    *slot += x;
                }
            }
            if last > first {
                let inv = 1.0 / (last - first) as f32;
                for slot in row.iter_mut() {
                    *slot *= inv;
                }
            }
        }
        out
    }
}

/// Deterministic values, distinct per index so a permutation is visible.
pub fn series(len: usize, phase: usize, scale: f32) -> Vec<f32> {
    (0..len)
        .map(|i| (((i + phase) as f32) * 0.37).sin() * scale)
        .collect()
}

/// Assert CPU/WGPU parity, reporting the worst deviation even when it passes, and naming the FIRST
/// diverging coordinate with both values and the ULP distance when it does not.
pub fn assert_parity(label: &str, actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len(), "{label}: length");
    let worst = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    let worst_ulp = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (a.to_bits() as i64 - b.to_bits() as i64).abs())
        .max()
        .unwrap_or(0);
    eprintln!("PARITY {label}: worst |d| {worst:e}, worst ulp {worst_ulp}, bound {ABS_BOUND:e}");
    for (index, (&got, &want)) in actual.iter().zip(expected).enumerate() {
        assert!(got.is_finite(), "{label}[{index}]: {got} is not finite");
        let error = (got - want).abs();
        let bound = ABS_BOUND * 1.0f32.max(want.abs());
        assert!(
            error <= bound,
            "{label}: FIRST DIVERGENCE at coordinate {index} — gpu {got:e}, cpu {want:e}, \
             |d| {error:e}, bound {bound:e}, ulp {}",
            (got.to_bits() as i64 - want.to_bits() as i64).abs()
        );
    }
}

/// The suite's **one** backend, or `None` when no adapter exists.
///
/// Delegates to `hymeko_wgpu::testing::shared_backend`, so however many tests call this, the binary
/// constructs exactly one device. That is not a convenience: constructing several concurrently is
/// what this repository documents as deadlocking in the Vulkan loader, and what was measured here
/// segfaulting `highway_identity` in 2 of 3 default-threaded runs before the change.
///
/// The adapter identity is printed once, so a reported GPU result can never be a silent skip.
pub fn backend() -> Option<&'static WgpuBackend> {
    let backend = hymeko_wgpu::testing::shared_backend("hymeko-sparse-suite")?;
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();
    ANNOUNCED.call_once(|| eprintln!("adapter={}", backend.capabilities().summary()));
    Some(backend)
}

/// Catmull-Rom evaluation of one channel's `control` points at `raw`: the clamp, the segment, the four
/// weights and the endpoint-replicated indices of `hymeko_spline::catmull_rom`, in `f32`. Shared by
/// the HSiKAN suites and the membership suite, which transcribed it separately before.
pub fn cr(control: &[f32], grid: usize, raw: f32) -> f32 {
    let (weights, _, indices) = cr_stencil(grid, raw);
    weights
        .iter()
        .zip(indices)
        .map(|(weight, index)| weight * control[index])
        .sum()
}

/// The stencil of `raw` on a grid of `grid`: weights, their `t`-derivatives, control indices.
fn cr_stencil(grid: usize, raw: f32) -> ([f32; 4], [f32; 4], [usize; 4]) {
    let x = raw.clamp(-1.0, 1.0);
    let u = (x + 1.0) * (0.5 * (grid - 1) as f32);
    let i = (u.floor() as usize).min(grid - 2);
    let t = u - i as f32;
    let t2 = t * t;
    let t3 = t2 * t;
    (
        [
            0.5 * (-t3 + 2.0 * t2 - t),
            0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
            0.5 * (-3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 - t2),
        ],
        [
            0.5 * (-3.0 * t2 + 4.0 * t - 1.0),
            0.5 * (9.0 * t2 - 10.0 * t),
            0.5 * (-9.0 * t2 + 8.0 * t + 1.0),
            0.5 * (3.0 * t2 - 2.0 * t),
        ],
        [
            i.saturating_sub(1).min(grid - 1),
            i,
            (i + 1).min(grid - 1),
            (i + 2).min(grid - 1),
        ],
    )
}

/// `γ_k = k u / (1 - k u)` with `u = 2^-24`: the a-priori relative error of `k` rounded operations
/// (Higham 2002, §3.1).
pub fn gamma(k: usize) -> f64 {
    let unit = f64::from(f32::EPSILON) / 2.0;
    let ku = k as f64 * unit;
    ku / (1.0 - ku)
}

/// The polynomial masses `Σ_m |a_km| t^m` of the four weights and of their `t`-derivatives at `raw`,
/// with the control indices: the scales of the forward's and the input gradient's rounding (the
/// primer of P47 Amendment 3, Proposition 3).
pub fn cr_masses(grid: usize, raw: f32) -> ([f64; 4], [f64; 4], [usize; 4]) {
    let x = raw.clamp(-1.0, 1.0);
    let u = (x + 1.0) * (0.5 * (grid - 1) as f32);
    let i = (u.floor() as usize).min(grid - 2);
    let t = f64::from(u - i as f32);
    let (t2, t3) = (t * t, t * t * t);
    let (_, _, indices) = cr_stencil(grid, raw);
    (
        [
            0.5 * (t3 + 2.0 * t2 + t),
            0.5 * (3.0 * t3 + 5.0 * t2 + 2.0),
            0.5 * (3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 + t2),
        ],
        [
            0.5 * (3.0 * t2 + 4.0 * t + 1.0),
            0.5 * (9.0 * t2 + 10.0 * t),
            0.5 * (9.0 * t2 + 8.0 * t + 1.0),
            0.5 * (3.0 * t2 + 2.0 * t),
        ],
        indices,
    )
}

/// The forward's per-element bound between two correct `f32` evaluations: `2 γ_9 Σ_k w̃_k |c|`.
pub fn cr_forward_bound(control: &[f32], grid: usize, raw: f32) -> f64 {
    let (weights, _, indices) = cr_masses(grid, raw);
    2.0 * gamma(9)
        * weights
            .iter()
            .zip(indices)
            .map(|(m, j)| m * f64::from(control[j].abs()))
            .sum::<f64>()
}

/// The input gradient's per-element bound between two correct `f32` evaluations:
/// `2 γ_11 |g| s Σ_k d̃_k |c|`, `s = (grid - 1) / 2`.
pub fn cr_input_gradient_bound(control: &[f32], grid: usize, raw: f32, g: f32) -> f64 {
    let (_, derivatives, indices) = cr_masses(grid, raw);
    let span = 0.5 * (grid - 1) as f64;
    2.0 * gamma(11)
        * f64::from(g.abs())
        * span
        * derivatives
            .iter()
            .zip(indices)
            .map(|(m, j)| m * f64::from(control[j].abs()))
            .sum::<f64>()
}

/// The membership pullback of `hymeko_spline::catmull_rom_backward` over `x [rows, channels]`:
/// the input gradient in `f32` (zero outside `[-1, 1]`), and the control-point gradient summed in
/// `f64` beside `Σ|g w|` per control point, the scale of its recursive-summation error bound.
pub struct CrPullback {
    pub grad_x: Vec<f32>,
    pub grad_control: Vec<f64>,
    pub magnitude: Vec<f64>,
    /// Each control point's term count; zero on a point no element touches.
    pub terms_per_slot: Vec<usize>,
    /// The most terms any control point's sum has.
    pub terms: usize,
}

pub fn cr_pullback(
    x: &[f32],
    control: &[f32],
    channels: usize,
    grid: usize,
    grad_y: &[f32],
) -> CrPullback {
    let span = 0.5 * (grid - 1) as f32;
    let mut grad_x = vec![0.0f32; x.len()];
    let mut grad_control = vec![0.0f64; control.len()];
    let mut magnitude = vec![0.0f64; control.len()];
    let mut terms = vec![0usize; control.len()];
    for (e, (&raw, &g)) in x.iter().zip(grad_y).enumerate() {
        let base = (e % channels) * grid;
        let (weights, dweights, indices) = cr_stencil(grid, raw);
        let mut dy_dt = 0.0f32;
        for k in 0..4 {
            let slot = base + indices[k];
            grad_control[slot] += f64::from(g) * f64::from(weights[k]);
            magnitude[slot] += (f64::from(g) * f64::from(weights[k])).abs();
            terms[slot] += 1;
            dy_dt += dweights[k] * control[slot];
        }
        grad_x[e] = if (-1.0..=1.0).contains(&raw) {
            g * dy_dt * span
        } else {
            0.0
        };
    }
    CrPullback {
        grad_x,
        grad_control,
        magnitude,
        terms: terms.iter().copied().max().unwrap_or(0),
        terms_per_slot: terms,
    }
}
