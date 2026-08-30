//! Minimal, test-support-only sparse-CSR fixture for the backend-local wgpu tests.
//!
//! Replaces the former `hymeko_hnn::workloads::{SparseSpec, DegreeShape}` dependency
//! (removed when `hymeko_hnn` moved to `hymeko/nagare`, 2026-08-31). This carries **only**
//! the backend-neutral random-CSR generation — presets, an LCG, a degree shape, and
//! `build`/`dense_*`. It has **no** hypergraph / HNN semantics (no `HyperGraphView`,
//! incidence, signs) and **no** Nagare dependency; it depends only on `hymeko` core's
//! `TensorCsr`. Seeds and constants match the retired generator so the generated matrices
//! are byte-identical and the backend tests behave exactly as before.

use hymeko::tensor::representations::tensor_csr::TensorCsr;

/// Deterministic MMIX LCG (fixture RNG). Any seed is valid.
pub struct Lcg(u64);

impl Lcg {
    #[inline]
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
    /// Uniform-ish index in `0..bound` (`bound` of 0 treated as 1). Uses the high bits.
    #[inline]
    pub fn below(&mut self, bound: usize) -> usize {
        let b = bound.max(1);
        ((self.next_u64() >> 11) as usize) % b
    }
    #[inline]
    pub fn unit_f32(&mut self) -> f32 {
        const SCALE: f32 = 1.0 / (1u32 << 24) as f32;
        ((self.next_u64() >> 40) as u32) as f32 * SCALE
    }
    /// Weight in `[0.5, 1.5)` — bounded away from zero, well conditioned for f32.
    #[inline]
    pub fn weight_f32(&mut self) -> f32 {
        0.5 + self.unit_f32()
    }
}

/// Row-degree distribution.
#[derive(Clone, Copy)]
pub enum DegreeShape {
    /// Every row has exactly the requested average degree.
    Uniform,
    /// Heavy-tailed: 5% at 8×avg, 15% at 2×avg, 60% at avg, 20% at avg/4.
    Irregular,
}

impl DegreeShape {
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

/// A random sparse-CSR workload specification (backend-neutral).
#[derive(Clone, Copy)]
pub struct SparseSpec {
    pub rows: usize,
    pub cols: usize,
    pub avg_nnz_per_row: usize,
    pub shape: DegreeShape,
    pub seed: u64,
}

impl SparseSpec {
    /// Latency regime: fits in L2.
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
    /// Throughput regime with irregular rows.
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

    /// Materialise the matrix (row_ptr non-decreasing; each row's columns strictly increasing).
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

    /// Dense input vector matching the column count.
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
