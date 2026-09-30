//! GPU-resident tensors and CSR matrices, and the factory that creates them.
//!
//! # Adapter, not replacement
//!
//! `hymeko_core::TensorCsr<f32>` and the dense host slices stay exactly as they
//! are — no device handle is ever stored in a core type. [`ResourceFactory`]
//! *adapts* them into device representations. That is the Adapter pattern and
//! the task's explicit constraint.
//!
//! # Index width
//!
//! `TensorCsr` stores indices as `usize`; WGSL has no 64-bit integer indexing,
//! so every index narrows to `u32` through
//! [`hymeko_tensor::compute::try_u32`] — a **checked** conversion that reports
//! [`ComputeError::IndexOverflow`] instead of silently wrapping into a kernel
//! that reads the wrong memory.

use std::marker::PhantomData;

use hymeko_tensor::compute::{ComputeError, SparseCsrShape, TensorShape, try_u32};
use hymeko_tensor::representations::sparse::{
    SparseLayout, SparseViolation, ValidatedSparse, validate_csr_parts,
};
use hymeko_tensor::representations::tensor_csr::TensorCsr;
use wgpu::util::DeviceExt;

/// A dense, row-major GPU tensor.
///
/// The element type is a real type parameter rather than a runtime dtype tag:
/// round one is explicitly f32, and a tag would have no consumer.
#[derive(Debug)]
pub struct GpuTensor<T> {
    buffer: wgpu::Buffer,
    shape: TensorShape,
    _marker: PhantomData<T>,
}

impl<T> GpuTensor<T> {
    pub(crate) fn binding(&self, binding: u32) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding,
            resource: self.buffer.as_entire_binding(),
        }
    }

    /// Build a tensor from a buffer the caller already owns.
    ///
    /// Exists for [`crate::TensorArena`], which hands out recycled buffers and
    /// must wrap them in a tensor without going through `create_buffer`.
    ///
    /// # Preconditions
    ///
    /// `buffer` must be at least `shape.elements() * size_of::<T>()` bytes and
    /// have been created with usage flags covering every intended binding. The
    /// arena guarantees both by construction; there is no way to check the second
    /// after the fact, because wgpu does not expose a buffer's usage.
    #[must_use]
    pub const fn from_parts(buffer: wgpu::Buffer, shape: TensorShape) -> Self {
        Self {
            buffer,
            shape,
            _marker: PhantomData,
        }
    }

    /// Consume the tensor and return its buffer, for recycling.
    #[must_use]
    pub fn into_buffer(self) -> wgpu::Buffer {
        self.buffer
    }

    #[must_use]
    pub const fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    #[must_use]
    pub const fn shape(&self) -> TensorShape {
        self.shape
    }

    /// Size in bytes of the backing buffer.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.buffer.size()
    }
}

/// A GPU-resident CSR matrix: three buffers plus the shape.
///
/// Uploaded once and reused across dispatches — re-uploading a static matrix
/// per call is the failure mode the benchmark plan separates `gpu_oneshot` from
/// `gpu_resident` to expose.
#[derive(Debug)]
pub struct GpuCsrMatrix<T> {
    row_ptr: wgpu::Buffer,
    col_ind: wgpu::Buffer,
    values: wgpu::Buffer,
    shape: SparseCsrShape,
    _marker: PhantomData<T>,
}

impl<T> GpuCsrMatrix<T> {
    #[must_use]
    pub const fn row_ptr(&self) -> &wgpu::Buffer {
        &self.row_ptr
    }

    #[must_use]
    pub const fn col_ind(&self) -> &wgpu::Buffer {
        &self.col_ind
    }

    #[must_use]
    pub const fn values(&self) -> &wgpu::Buffer {
        &self.values
    }

    #[must_use]
    pub const fn shape(&self) -> SparseCsrShape {
        self.shape
    }

    /// Total device bytes held by the three buffers.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.row_ptr.size() + self.col_ind.size() + self.values.size()
    }
}

/// Creates device resources. Borrowed from a [`crate::WgpuBackend`].
///
/// Abstract Factory: only the thing that owns a device can make device
/// resources, so resource construction lives here rather than in free functions.
#[derive(Clone, Copy)]
pub struct ResourceFactory<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    /// The device allocation ceiling, so every size check names the real limit.
    max_buffer_size: u64,
}

/// Translate core's typed structural verdict into this backend's error type.
///
/// A **translation**, not a check. The rules live in
/// [`hymeko_tensor::representations::sparse::validate_csr_parts`]; this only carries the verdict
/// across an API boundary, preserving the operand wording the existing consumers already assert on
/// (`tests/skeleton.rs`) so the migration to the core owner changes no caller's contract.
fn structural(violation: SparseViolation) -> ComputeError {
    match violation {
        SparseViolation::RowPointerLength { expected, actual } => ComputeError::ShapeMismatch {
            operand: "csr row_ptr",
            expected,
            actual,
        },
        SparseViolation::FirstOffsetNotZero { actual } => ComputeError::ShapeMismatch {
            operand: "csr row_ptr must start at 0",
            expected: 0,
            actual,
        },
        SparseViolation::OffsetNotMonotone {
            previous, actual, ..
        } => ComputeError::ShapeMismatch {
            operand: "csr row_ptr must be monotonically non-decreasing",
            expected: previous,
            actual,
        },
        SparseViolation::OffsetAboveNnz { nnz, actual, .. } => ComputeError::ShapeMismatch {
            operand: "csr row_ptr entry exceeds nnz",
            expected: nnz,
            actual,
        },
        SparseViolation::TerminalOffsetNotNnz { nnz, actual } => ComputeError::ShapeMismatch {
            operand: "csr row_ptr must end at nnz",
            expected: nnz,
            actual,
        },
        SparseViolation::EntryArrayLength {
            expected, actual, ..
        } => ComputeError::ShapeMismatch {
            operand: "csr values",
            expected,
            actual,
        },
        SparseViolation::CoordinateOutOfRange { extent, actual, .. } => {
            ComputeError::ShapeMismatch {
                operand: "csr col_ind entry is not a valid column",
                expected: extent,
                actual,
            }
        }
        SparseViolation::MetadataInconsistent { .. } => ComputeError::ShapeMismatch {
            operand: "csr metadata is inconsistent with its entries",
            expected: 0,
            actual: 0,
        },
        SparseViolation::ExtentOverflow { value, .. } => ComputeError::IndexOverflow {
            what: "csr row count + 1",
            value,
        },
    }
}

impl<'a> ResourceFactory<'a> {
    #[must_use]
    pub const fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        max_buffer_size: u64,
    ) -> Self {
        Self {
            device,
            queue,
            max_buffer_size,
        }
    }

    /// Standard usage for a compute input/output: storage, plus copy in/out.
    const STORAGE_RW: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE
        .union(wgpu::BufferUsages::COPY_DST)
        .union(wgpu::BufferUsages::COPY_SRC);

    /// Upload a dense host slice as a GPU tensor.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] if `data.len() != shape.elements()`.
    pub fn upload_tensor(
        &self,
        label: &str,
        shape: TensorShape,
        data: &[f32],
    ) -> Result<GpuTensor<f32>, ComputeError> {
        self.upload_pod_tensor(label, shape, data)
    }

    /// Upload an unsigned-index tensor for a typed compute operation.
    ///
    /// This is deliberately separate from [`Self::upload_tensor`]: an index
    /// buffer cannot accidentally be bound where a shader expects `f32`.
    pub fn upload_u32_tensor(
        &self,
        label: &str,
        shape: TensorShape,
        data: &[u32],
    ) -> Result<GpuTensor<u32>, ComputeError> {
        self.upload_pod_tensor(label, shape, data)
    }

    /// Upload a signed-integer tensor for a typed compute operation.
    pub fn upload_i32_tensor(
        &self,
        label: &str,
        shape: TensorShape,
        data: &[i32],
    ) -> Result<GpuTensor<i32>, ComputeError> {
        self.upload_pod_tensor(label, shape, data)
    }

    /// Allocate a zeroed GPU tensor of the given shape (a kernel output).
    ///
    /// # Errors
    /// [`ComputeError::IndexOverflow`] if the element count overflows.
    pub fn zeroed_tensor(
        &self,
        label: &str,
        shape: TensorShape,
    ) -> Result<GpuTensor<f32>, ComputeError> {
        // One checked computation, shared with the arena and the readback path.
        let extent = crate::sizing::f32_storage_extent(shape, self.max_buffer_size)?;
        let size = extent.padded_bytes;

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: Self::STORAGE_RW,
            mapped_at_creation: false,
        });

        Ok(GpuTensor {
            buffer,
            shape,
            _marker: PhantomData,
        })
    }

    /// Upload a CSR matrix from host arrays.
    ///
    /// Takes the raw arrays rather than `&TensorCsr<f32>` so this crate does not
    /// need to know that type's layout — and so a `HyperGraphView` incidence
    /// CSR can use the same path later without a second function.
    ///
    /// # Preconditions
    /// `row_ptr.len() == shape.rows + 1`, `col_ind.len() == values.len() == shape.nnz`.
    ///
    /// # Preconditions
    ///
    /// Beyond the array lengths, the CSR must be structurally valid:
    /// `row_ptr[0] == 0`, `row_ptr` monotonically non-decreasing, every entry
    /// `<= nnz`, `row_ptr[rows] == nnz`, and every `col_ind` entry `< cols`.
    /// All are checked here rather than left to the shader.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] on a length disagreement or a structural
    /// violation, naming which one; [`ComputeError::IndexOverflow`] if any index
    /// exceeds `u32::MAX`.
    pub fn upload_csr(
        &self,
        label: &str,
        shape: SparseCsrShape,
        row_ptr: &[usize],
        col_ind: &[usize],
        values: &[f32],
    ) -> Result<GpuCsrMatrix<f32>, ComputeError> {
        if row_ptr.len() != shape.row_ptr_len() {
            return Err(ComputeError::ShapeMismatch {
                operand: "csr row_ptr",
                expected: shape.row_ptr_len(),
                actual: row_ptr.len(),
            });
        }
        if col_ind.len() != shape.nnz {
            return Err(ComputeError::ShapeMismatch {
                operand: "csr col_ind",
                expected: shape.nnz,
                actual: col_ind.len(),
            });
        }
        if values.len() != shape.nnz {
            return Err(ComputeError::ShapeMismatch {
                operand: "csr values",
                expected: shape.nnz,
                actual: values.len(),
            });
        }

        // --- structural validation: NOT ours -------------------------------------
        // These rules are facts about the tensor, so `hymeko_core` owns them. Calling the canonical
        // validator here — rather than repeating it — is what keeps this crate from being a second
        // structural authority whose copy could drift from core's. It borrows the arrays already in
        // hand: nothing is copied to check them.
        validate_csr_parts(shape.rows, shape.cols, row_ptr, col_ind, values.len())
            .map_err(structural)?;

        // Checked narrowing. One transient Vec per array, paid once at upload;
        // the steady-state dispatch path allocates nothing.
        let row_ptr_u32 = Self::narrow(row_ptr, "csr row_ptr entry")?;
        let col_ind_u32 = Self::narrow(col_ind, "csr col_ind entry")?;

        let row_ptr_buf = self.init_buffer(&format!("{label}.row_ptr"), &row_ptr_u32);
        let col_ind_buf = self.init_buffer(&format!("{label}.col_ind"), &col_ind_u32);

        let value_bytes: &[u8] = if values.is_empty() {
            bytemuck::cast_slice(&[0.0f32][..])
        } else {
            bytemuck::cast_slice(values)
        };
        let values_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label}.values")),
                contents: value_bytes,
                usage: Self::STORAGE_RW,
            });

        Ok(GpuCsrMatrix {
            row_ptr: row_ptr_buf,
            col_ind: col_ind_buf,
            values: values_buf,
            shape,
            _marker: PhantomData,
        })
    }

    /// Upload a CSR that core has already validated, from its **bound** proof.
    ///
    /// # Why one argument and not `(tensor, receipt)`
    ///
    /// A detachable receipt carrying only metadata is unsound: two tensors sharing a shape and
    /// differing in structure produce interchangeable receipts, so `(B, receipt_of_A)` would pass
    /// any metadata check. [`ValidatedSparse`] holds an immutable borrow of the tensor it
    /// validated, so **there is no pair to mismatch** — the proof and the data are one value, and
    /// the type system rejects the replay rather than a runtime check catching it.
    ///
    /// This function performs **only device work**: checked `usize -> u32` narrowing, buffer
    /// allocation, and the device shape it hands the kernels. It re-checks nothing structural; the
    /// source gate in `tests/scatter_mean_csr.rs` fails if it ever starts.
    ///
    /// # Errors
    /// [`ComputeError::IndexOverflow`] if an index exceeds `u32::MAX`.
    pub fn upload_validated_csr(
        &self,
        label: &str,
        validated: &ValidatedSparse<'_, TensorCsr<f32>>,
    ) -> Result<GpuCsrMatrix<f32>, ComputeError> {
        debug_assert_eq!(
            validated.layout(),
            SparseLayout::Csr,
            "a CSR layout type cannot yield a non-CSR proof"
        );
        // The tensor is reached THROUGH the proof; no second reference can be substituted.
        let csr = validated.tensor();
        let shape = SparseCsrShape::new(csr.num_rows, csr.num_cols, csr.col_ind.len());
        let row_ptr_u32 = Self::narrow(&csr.row_ptr, "csr row_ptr entry")?;
        let col_ind_u32 = Self::narrow(&csr.col_ind, "csr col_ind entry")?;
        let row_ptr_buf = self.init_buffer(&format!("{label}.row_ptr"), &row_ptr_u32);
        let col_ind_buf = self.init_buffer(&format!("{label}.col_ind"), &col_ind_u32);
        let value_bytes: &[u8] = if csr.val.is_empty() {
            bytemuck::cast_slice(&[0.0f32][..])
        } else {
            bytemuck::cast_slice(&csr.val)
        };
        let values_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label}.values")),
                contents: value_bytes,
                usage: Self::STORAGE_RW,
            });
        Ok(GpuCsrMatrix {
            row_ptr: row_ptr_buf,
            col_ind: col_ind_buf,
            values: values_buf,
            shape,
            _marker: PhantomData,
        })
    }

    /// Upload a `hymeko_core` CSR matrix directly.
    ///
    /// The **Adapter** in its most literal form: `TensorCsr<f32>` is unchanged
    /// and gains no device handle; this reads its three arrays and produces the
    /// device representation. Shape is derived rather than supplied, so it
    /// cannot disagree with the data.
    ///
    /// # Errors
    /// As [`Self::upload_csr`].
    pub fn upload_csr_from(
        &self,
        label: &str,
        csr: &hymeko_tensor::representations::tensor_csr::TensorCsr<f32>,
    ) -> Result<GpuCsrMatrix<f32>, ComputeError> {
        let shape = SparseCsrShape::new(csr.num_rows, csr.num_cols, csr.nnz());
        self.upload_csr(label, shape, &csr.row_ptr, &csr.col_ind, &csr.val)
    }

    /// Create a buffer that can be mapped for readback.
    #[must_use]
    pub fn staging_buffer(&self, label: &str, size: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Overwrite a tensor's contents without reallocating.
    ///
    /// This is what lets a prepared operation be re-dispatched against new input
    /// without rebuilding its bind group.
    ///
    /// # Errors
    /// [`ComputeError::ShapeMismatch`] if `data` does not match the shape.
    pub fn write_tensor<T>(&self, operand: &T, data: &[f32]) -> Result<(), ComputeError>
    where
        T: crate::scope::TensorOperand,
    {
        let tensor = operand.gpu_tensor();
        tensor.shape.validate_len("tensor data", data.len())?;
        if !data.is_empty() {
            self.queue
                .write_buffer(&tensor.buffer, 0, bytemuck::cast_slice(data));
        }
        Ok(())
    }

    fn narrow(src: &[usize], what: &'static str) -> Result<Vec<u32>, ComputeError> {
        src.iter().map(|&v| try_u32(what, v)).collect()
    }

    fn upload_pod_tensor<T>(
        &self,
        label: &str,
        shape: TensorShape,
        data: &[T],
    ) -> Result<GpuTensor<T>, ComputeError>
    where
        T: bytemuck::Pod + bytemuck::Zeroable,
    {
        shape.validate_len("tensor data", data.len())?;

        // An empty buffer is not a valid wgpu allocation; model an empty tensor
        // as a 1-element placeholder so shape stays authoritative and callers
        // need no special case. Kernels bound loops by shape, never buffer size.
        let zero = T::zeroed();
        let bytes: &[u8] = if data.is_empty() {
            bytemuck::bytes_of(&zero)
        } else {
            bytemuck::cast_slice(data)
        };
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: Self::STORAGE_RW,
            });
        Ok(GpuTensor {
            buffer,
            shape,
            _marker: PhantomData,
        })
    }

    fn init_buffer(&self, label: &str, data: &[u32]) -> wgpu::Buffer {
        let bytes: &[u8] = if data.is_empty() {
            bytemuck::cast_slice(&[0u32][..])
        } else {
            bytemuck::cast_slice(data)
        };
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: Self::STORAGE_RW,
            })
    }
}
