//! Tensor decomposition for HyMeKo's 3D incidence tensors.
//!
//! Implements:
//!   - Mode-k unfolding (matricization)
//!   - Truncated SVD per mode
//!   - HOSVD (Higher-Order SVD / Tucker decomposition)
//!   - Structural entropy from singular value spectrum

use crate::tensor::common::Real;
use crate::tensor::representations::tensor_coo::TensorCoo;

/// Mode-k unfolding: reshape 3D tensor to 2D matrix.
/// Mode 0: rows = k (edges),    cols = i×J + j
/// Mode 1: rows = i (src nodes), cols = k×J + j
/// Mode 2: rows = j (dst nodes), cols = k×I + i
pub fn mode_k_unfold<F: Real>(tensor: &TensorCoo<F>, mode: usize) -> (Vec<F>, usize, usize) {
    let (K, I, J) = (tensor.num_slices, tensor.dim_i, tensor.dim_j);

    let (num_rows, num_cols) = match mode {
        0 => (K, I * J),
        1 => (I, K * J),
        2 => (J, K * I),
        _ => panic!("mode must be 0, 1, or 2"),
    };

    let mut mat = vec![F::zero(); num_rows * num_cols];

    for entry in tensor.iter() {
        let (row, col) = match mode {
            0 => (entry.k, entry.i * J + entry.j),
            1 => (entry.i, entry.k * J + entry.j),
            2 => (entry.j, entry.k * I + entry.i),
            _ => unreachable!(),
        };
        mat[row * num_cols + col] += entry.v;
    }

    (mat, num_rows, num_cols)
}

/// Truncated SVD via power iteration (Lanczos-like).
/// Returns (U, sigma, V_t) where U is m×r, sigma is r, V_t is r×n.
pub fn truncated_svd_power<F: Real>(
    mat: &[F],
    m: usize,
    n: usize,
    rank: usize,
    max_iter: usize,
) -> (Vec<F>, Vec<F>, Vec<F>) {
    let r = rank.min(m).min(n);
    let mut u_mat = vec![F::zero(); m * r];
    let mut sigma = vec![F::zero(); r];
    let mut vt_mat = vec![F::zero(); r * n];

    // Deflated power iteration
    let mut residual = mat.to_vec();

    for k in 0..r {
        // Random init for v (deterministic seed for reproducibility)
        let mut v = vec![F::zero(); n];
        for i in 0..n {
            v[i] = F::from_other(((i * 7 + k * 13 + 1) % 97) as f64 / 97.0);
        }
        normalize(&mut v);

        let mut u = vec![F::zero(); m];

        for _ in 0..max_iter {
            // u = A * v
            mat_vec(&residual, m, n, &v, &mut u);
            let s = norm(&u);
            if s > F::zero() {
                for x in u.iter_mut() {
                    *x = *x / s;
                }
            }

            // v = A^T * u
            mat_t_vec(&residual, m, n, &u, &mut v);
            let s2 = norm(&v);
            if s2 > F::zero() {
                for x in v.iter_mut() {
                    *x = *x / s2;
                }
            }
            sigma[k] = s;
        }

        // Store k-th singular triplet
        for i in 0..m {
            u_mat[i * r + k] = u[i];
        }
        for j in 0..n {
            vt_mat[k * n + j] = v[j];
        }

        // Deflate: residual -= sigma[k] * u * v^T
        for i in 0..m {
            for j in 0..n {
                residual[i * n + j] = residual[i * n + j] - sigma[k] * u[i] * v[j];
            }
        }
    }

    (u_mat, sigma, vt_mat)
}

/// HOSVD: Tucker decomposition of 3D tensor.
/// Returns (core_tensor, U0, U1, U2, sigmas per mode).
pub struct HosvdResult<F: Real> {
    /// Core tensor G: r0 × r1 × r2 (dense, flat)
    pub core: Vec<F>,
    pub r0: usize,
    pub r1: usize,
    pub r2: usize,
    /// Factor matrices: U_k is dim_k × r_k
    pub u0: Vec<F>,
    pub u1: Vec<F>,
    pub u2: Vec<F>,
    /// Singular values per mode
    pub sigma0: Vec<F>,
    pub sigma1: Vec<F>,
    pub sigma2: Vec<F>,
}

pub fn hosvd<F: Real>(
    tensor: &TensorCoo<F>,
    ranks: [usize; 3],
    svd_iters: usize,
) -> HosvdResult<F> {
    // Step 1: SVD of each mode-k unfolding
    let (mat0, m0, n0) = mode_k_unfold(tensor, 0);
    let (u0, s0, _) = truncated_svd_power(&mat0, m0, n0, ranks[0], svd_iters);

    let (mat1, m1, n1) = mode_k_unfold(tensor, 1);
    let (u1, s1, _) = truncated_svd_power(&mat1, m1, n1, ranks[1], svd_iters);

    let (mat2, m2, n2) = mode_k_unfold(tensor, 2);
    let (u2, s2, _) = truncated_svd_power(&mat2, m2, n2, ranks[2], svd_iters);

    // Step 2: Core tensor G = T ×₁ U₀ᵀ ×₂ U₁ᵀ ×₃ U₂ᵀ
    let r0 = ranks[0].min(m0);
    let r1 = ranks[1].min(m1);
    let r2 = ranks[2].min(m2);
    let mut core = vec![F::zero(); r0 * r1 * r2];

    for entry in tensor.iter() {
        for a in 0..r0 {
            for b in 0..r1 {
                for c in 0..r2 {
                    let u0_ka = u0[entry.k * r0 + a];
                    let u1_ib = u1[entry.i * r1 + b];
                    let u2_jc = u2[entry.j * r2 + c];
                    core[a * r1 * r2 + b * r2 + c] += entry.v * u0_ka * u1_ib * u2_jc;
                }
            }
        }
    }

    HosvdResult {
        core,
        r0,
        r1,
        r2,
        u0,
        u1,
        u2,
        sigma0: s0,
        sigma1: s1,
        sigma2: s2,
    }
}

/// Structural entropy from singular value spectrum.
/// H = -Σ p_i log(p_i), where p_i = σ_i² / Σ σ_j²
pub fn spectral_entropy<F: Real>(sigmas: &[F]) -> f64 {
    let total: f64 = sigmas
        .iter()
        .map(|s| {
            let sf = s.as_f64();
            sf * sf
        })
        .sum();

    if total < 1e-15 {
        return 0.0;
    }

    let mut entropy = 0.0;
    for s in sigmas {
        let p = s.as_f64() * s.as_f64() / total;
        if p > 1e-15 {
            entropy -= p * p.ln();
        }
    }
    entropy
}

/// Frobenius norm reconstruction error: ||T - G ×₁ U₀ ×₂ U₁ ×₃ U₂||_F
pub fn reconstruction_error<F: Real>(tensor: &TensorCoo<F>, result: &HosvdResult<F>) -> f64 {
    let mut err_sq = 0.0;

    for entry in tensor.iter() {
        // Reconstruct T[k,i,j] from HOSVD
        let mut reconstructed = F::zero();
        for a in 0..result.r0 {
            for b in 0..result.r1 {
                for c in 0..result.r2 {
                    let g = result.core[a * result.r1 * result.r2 + b * result.r2 + c];
                    let u0 = result.u0[entry.k * result.r0 + a];
                    let u1 = result.u1[entry.i * result.r1 + b];
                    let u2 = result.u2[entry.j * result.r2 + c];
                    reconstructed += g * u0 * u1 * u2;
                }
            }
        }
        let diff = (entry.v - reconstructed).as_f64();
        err_sq += diff * diff;
    }

    err_sq.sqrt()
}

// --- Utility functions ---

fn normalize<F: Real>(v: &mut [F]) {
    let n = norm(v);
    if n > F::zero() {
        for x in v.iter_mut() {
            *x = *x / n;
        }
    }
}

fn norm<F: Real>(v: &[F]) -> F {
    let mut s = F::zero();
    for &x in v {
        s += x * x;
    }
    s.sqrt()
}

fn mat_vec<F: Real>(a: &[F], m: usize, n: usize, x: &[F], y: &mut [F]) {
    for i in 0..m {
        let mut acc = F::zero();
        for j in 0..n {
            acc += a[i * n + j] * x[j];
        }
        y[i] = acc;
    }
}

fn mat_t_vec<F: Real>(a: &[F], m: usize, n: usize, x: &[F], y: &mut [F]) {
    for j in 0..n {
        y[j] = F::zero();
    }
    for i in 0..m {
        for j in 0..n {
            y[j] += a[i * n + j] * x[i];
        }
    }
}
