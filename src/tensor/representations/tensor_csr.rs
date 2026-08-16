use crate::tensor::common::Real;

pub struct TensorCsrBuilder<F> {
    pub dim_i: usize,
    pub dim_j: usize,
    pub uncoalesced_row_ptr: Vec<usize>, // Length: dim_i
    pub cols: Vec<usize>,                // Length: nnz (number of non-zeros)
    pub vals: Vec<F>,                    // Length: nnz
}

#[derive(Debug, Clone)]
pub struct TensorCsr<F> {
    pub num_rows: usize,
    pub num_cols: usize,
    pub row_ptr: Vec<usize>, // Length: num_rows + 1
    pub col_ind: Vec<usize>, // Length: nnz (number of non-zeros)
    pub val: Vec<F>,         // Length: nnz
}

impl<F: Real> TensorCsr<F> {
    /// Pre-allocates the exact memory required for the CSR structure.
    pub fn with_capacity(num_rows: usize, num_cols: usize, nnz: usize) -> Self {
        let row_ptr = vec![0; num_rows + 1];
        Self {
            num_rows,
            num_cols,
            row_ptr,
            col_ind: vec![0; nnz],
            val: vec![F::zero(); nnz],
        }
    }

    /// Executes y = A * x (Sparse Matrix-Vector Multiplication)
    /// Complexity: O(NNZ)
    pub fn spmv(&self, x: &[F]) -> Vec<F> {
        assert_eq!(
            self.num_cols,
            x.len(),
            "Dimension mismatch: CSR has {} columns, but vector x has length {}",
            self.num_cols,
            x.len()
        );

        let mut y = vec![F::zero(); self.num_rows];

        // Because row_ptr explicitly partitions the arrays, we iterate
        // row by row. This is perfectly cache-friendly.
        for i in 0..self.num_rows {
            let start = self.row_ptr[i];
            let end = self.row_ptr[i + 1];

            // Local accumulator avoids repeated heap writes
            let mut dot_product = F::zero();

            for k in start..end {
                let col = self.col_ind[k];
                let val = self.val[k];
                dot_product += val * x[col];
            }

            y[i] = dot_product;
        }

        y
    }

    /// Executes Y = A * X (Sparse Matrix-Dense Matrix Multiplication)
    /// X is a flattened 2D array of shape (num_cols, feature_dim)
    /// Returns a flattened 2D array of shape (num_rows, feature_dim)
    pub fn spmm(&self, x_flat: &[F], feature_dim: usize) -> Vec<F> {
        assert_eq!(
            self.num_cols * feature_dim,
            x_flat.len(),
            "Dimension mismatch: X must be exactly num_cols * feature_dim"
        );

        let mut y_flat = vec![F::zero(); self.num_rows * feature_dim];

        for i in 0..self.num_rows {
            let start = self.row_ptr[i];
            let end = self.row_ptr[i + 1];

            // We process the entire feature dimension for row i
            let y_row_offset = i * feature_dim;

            for k in start..end {
                let col = self.col_ind[k];
                let val = self.val[k];
                let x_row_offset = col * feature_dim;

                // Inner loop over features.
                // A smart compiler will unroll and auto-vectorize this loop using SIMD.
                for f in 0..feature_dim {
                    y_flat[y_row_offset + f] += val * x_flat[x_row_offset + f];
                }
            }
        }

        y_flat
    }
}

// ==========================================
// UTILITY: Universal Prefix Sum
// ==========================================
#[inline]
pub fn build_row_ptr(counts: &[usize]) -> (Vec<usize>, usize) {
    let dim = counts.len();
    let mut row_ptr = vec![0; dim + 1];
    let mut total = 0;
    for i in 0..dim {
        row_ptr[i] = total;
        total += counts[i];
    }
    row_ptr[dim] = total;
    (row_ptr, total)
}
