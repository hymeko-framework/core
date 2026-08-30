//! Shared scaffolding for this crate's tests, benchmarks and examples.
//!
//! # Why this module exists
//!
//! Benches, examples and integration tests are each **separate crates**, so they
//! cannot share a private helper. The result was five byte-identical copies of
//! `backend_or_skip` and four copies of the eight-layer chain builder — the
//! per-experiment scaffold duplication CLAUDE.md §6.5 #3 exists to prevent, and
//! which a duplication audit found in exactly the shape that rule predicts.
//!
//! # Why it is `pub` rather than feature-gated
//!
//! A `testing` cargo feature would keep this out of the public surface, but
//! benches and examples do not inherit features from a dev-dependency on their
//! own crate, so every invocation would need `--features testing` — including
//! the reproduction commands already published in the benchmark report.
//! Changing those to fix an internal tidiness problem is the wrong trade.
//!
//! It is therefore always compiled and `#[doc(hidden)]`: no documentation
//! surface, no feature matrix, no changed invocations. The cost is ~100 lines of
//! test scaffolding shipped in the library, which is accepted deliberately.
//!
//! # What deliberately stays duplicated
//!
//! * **CPU reference oracles** (`cpu_activation` in `tests/activation.rs`, the
//!   diamond oracle in `fanout_tests`) are written independently on purpose, so
//!   a shared bug cannot hide behind both sides of a differential comparison.
//!   Unifying them would weaken every test that uses them.
//! * **Activation expressions across the WGSL files** — WGSL has no include
//!   mechanism, runtime shader-string assembly is out of scope, and the copies
//!   are pinned to each other by `test_in_place_and_out_of_place_agree` and
//!   `test_fused_matches_unfused_chain`, so drift fails a test.

use hymeko::tensor::compute::{ComputeError, TensorShape};
use hymeko::tensor::representations::tensor_csr::TensorCsr;

use crate::{ActivationKind, ArenaTensor, GpuCsrMatrix, GpuTensor, TrackedSession, WgpuBackend};

/// Build a backend, or return `None` when the host has no usable adapter.
///
/// # Why availability is not a failure
///
/// A machine without a GPU should **skip** these tests, not fail them —
/// otherwise CI on a headless runner reports red for a condition the code
/// cannot control. But every *other* initialisation error is a real defect and
/// panics, so a broken backend cannot masquerade as an absent one.
#[must_use]
pub fn backend_or_skip(context: &str) -> Option<WgpuBackend> {
    match WgpuBackend::builder().label("hymeko-test").build() {
        Ok(b) => Some(b),
        Err(ComputeError::BackendUnavailable { reason, .. }) => {
            eprintln!("[{context}] SKIPPED - no GPU adapter: {reason}");
            None
        }
        Err(other) => panic!("[{context}] backend init failed (not availability): {other}"),
    }
}

/// A CSR matrix and dense input uploaded to the device, plus the shapes derived
/// from them.
///
/// Carries `csr_nnz` because throughput reporting needs it and recomputing it
/// would mean rebuilding the host-side matrix.
pub struct Fixture {
    pub csr_nnz: usize,
    pub num_rows: usize,
    pub matrix: GpuCsrMatrix<f32>,
    pub input: GpuTensor<f32>,
    pub out_shape: TensorShape,
}

impl Fixture {
    /// Upload a host-side CSR matrix and dense block to the device.
    ///
    /// # Why this takes a `TensorCsr` rather than a workload spec
    ///
    /// `SparseSpec` lives in `hymeko_hnn`, which is a **dev-dependency only**:
    /// the library must never depend on it, which is precisely why the shared
    /// vocabulary was placed in `hymeko_core`. Taking the spec here would drag
    /// `hymeko_hnn` into the library's real dependencies and break that
    /// layering. Callers keep the one-line `spec.build()` / `spec.dense_block()`
    /// step -- that is the parameterisation, not the scaffolding.
    ///
    /// # Panics
    ///
    /// On upload failure, which in a test or benchmark is a defect rather than a
    /// condition to handle.
    #[must_use]
    pub fn build(
        backend: &WgpuBackend,
        csr: &TensorCsr<f32>,
        x: &[f32],
        feature_dim: usize,
    ) -> Self {
        let res = backend.resources();
        let matrix = res.upload_csr_from("a", csr).expect("upload csr");
        let shape = matrix.shape();
        let input = res
            .upload_tensor("x", shape.input_shape(feature_dim), x)
            .expect("upload x");
        Self {
            csr_nnz: csr.nnz(),
            num_rows: csr.num_rows,
            matrix,
            input,
            out_shape: shape.output_shape(feature_dim),
        }
    }
}

/// Encode an alternating `SpMM -> activation` chain over two pooled buffers.
///
/// Layer 0 reads the fixture's input; every later layer reads the buffer the
/// previous layer wrote, alternating between `a` and `b`. With `layers` even the
/// final result is in `b`, with `layers` odd it is in `a` — see
/// [`chain_output`].
///
/// # Why the source is an `Option` rather than one variable
///
/// Layer 0's input is a caller-owned `GpuTensor` and every later layer's is a
/// pooled `ArenaTensor`. Those have different [`crate::TensorOperand::Scope`]s,
/// so they are different types and cannot share a binding. That is the scope
/// system making the provenance difference visible rather than letting it blur —
/// the awkwardness is the guarantee working.
///
/// # Panics
///
/// On `prepare` or `encode` failure, both of which are defects here.
pub fn encode_chain(
    backend: &WgpuBackend,
    fx: &Fixture,
    a: &ArenaTensor<'_>,
    b: &ArenaTensor<'_>,
    layers: usize,
    kind: ActivationKind,
    session: &mut TrackedSession<'_>,
) {
    // Prepared operations are built first and held, because each borrows its
    // operands: building and encoding in one pass would end each borrow early
    // and is not what a real caller does.
    let ops: Vec<_> = (0..layers)
        .map(|i| {
            let (src, dst) = match i {
                0 => (None, a),
                _ if i % 2 == 1 => (Some(a), b),
                _ => (Some(b), a),
            };
            let spmm = match src {
                None => backend
                    .operations()
                    .spmm()
                    .matrix(&fx.matrix)
                    .input(&fx.input)
                    .output(dst)
                    .prepare()
                    .expect("prepare spmm"),
                Some(s) => backend
                    .operations()
                    .spmm()
                    .matrix(&fx.matrix)
                    .input(s)
                    .output(dst)
                    .prepare()
                    .expect("prepare spmm"),
            };
            let act = backend
                .operations()
                .activation()
                .kind(kind)
                .in_place(dst)
                .prepare()
                .expect("prepare activation");
            (spmm, act)
        })
        .collect();

    for (spmm, act) in &ops {
        spmm.encode(session).expect("encode spmm");
        act.encode(session).expect("encode activation");
    }
}

/// Which of the two ping-pong buffers holds the result after `layers` layers.
///
/// Layer 0 writes `a`, then parity alternates, so an odd final layer index
/// (`layers` even) leaves the result in `b`. Getting this wrong reads a
/// one-layer-stale buffer and the numbers look *almost* right, which is why it
/// is a named function rather than an inline expression.
#[must_use]
pub fn chain_output<'x, T>(a: &'x T, b: &'x T, layers: usize) -> &'x T {
    if layers.is_multiple_of(2) { b } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parity rule, checked against the layer indices it describes.
    #[test]
    fn chain_output_follows_the_alternation() {
        let (a, b) = (1u8, 2u8);
        // 1 layer: layer 0 writes a.
        assert_eq!(*chain_output(&a, &b, 1), a);
        // 2 layers: layer 1 writes b.
        assert_eq!(*chain_output(&a, &b, 2), b);
        // 8 layers: layer 7 is odd, writes b.
        assert_eq!(*chain_output(&a, &b, 8), b);
        // 9 layers: layer 8 is even, writes a.
        assert_eq!(*chain_output(&a, &b, 9), a);
    }
}

// === Backend-neutral sparse-CSR fixture ===
// Replaces the former hymeko_hnn::workloads sparse part (hymeko_hnn moved to hymeko/nagare,
// 2026-08-31). Random-CSR generation ONLY: no hypergraph/HNN semantics, no Nagare dep; seeds
// match the retired generator so backend fixtures are byte-identical. Used by the backend tests,
// benches, examples and src/fanout_tests.

// === Backend-neutral sparse-CSR fixture ===
// Replaces the former `hymeko_hnn::workloads` sparse part (`hymeko_hnn` moved to
// `hymeko/nagare`, 2026-08-31). Random-CSR generation ONLY — no hypergraph / HNN
// semantics, no Nagare dependency; seeds match the retired generator so backend
// fixtures stay byte-identical. Used by the backend tests, benches, examples and
// `src/fanout_tests`. `TensorCsr` is already imported at the top of this module.

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
    #[inline]
    pub fn weight_f32(&mut self) -> f32 {
        0.5 + self.unit_f32()
    }
}

/// Row-degree distribution for the sparse fixture.
#[derive(Clone, Copy)]
pub enum DegreeShape {
    /// Every row has exactly the requested average degree.
    Uniform,
    /// Heavy-tailed: 5% at 8x avg, 15% at 2x avg, 60% at avg, 20% at avg/4.
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

    #[must_use]
    pub fn dense_vector(&self) -> Vec<f32> {
        let mut rng = Lcg::new(self.seed ^ 0xD0D0_D0D0);
        (0..self.cols).map(|_| rng.weight_f32()).collect()
    }

    #[must_use]
    pub fn dense_block(&self, feature_dim: usize) -> Vec<f32> {
        let mut rng = Lcg::new(self.seed ^ 0xB10C_B10C);
        (0..self.cols * feature_dim)
            .map(|_| rng.weight_f32())
            .collect()
    }
}
