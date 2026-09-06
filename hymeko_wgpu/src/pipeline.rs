//! Kernel identity, layouts, shader modules and compute pipelines.
//!
//! # Why this exists
//!
//! Pipeline creation compiles WGSL and is orders of magnitude more expensive
//! than a dispatch. Rebuilding one per call is the single most common way to
//! make a GPU backend slower than the CPU it replaced.
//!
//! # The invariant, and why the previous design did not hold it
//!
//! Phase 9 replaced a key of `(shader name, entry point, workgroup)` where the
//! WGSL **source** and the **pipeline layout** were passed in beside it. On a
//! cache hit both were ignored, so an equal key could return a pipeline built
//! from different source with a different layout. That was demonstrated, not
//! assumed: two modules under one key, the second silently discarded, the
//! first's pipeline returned and executed (Phase 9 report, Part A).
//!
//! The invariant is now structural rather than conventional:
//!
//! > **An equal cache key necessarily denotes source-compatible compiled code
//! > and a compatible layout.**
//!
//! It holds because [`KernelId`] is a closed enum that *owns* the mapping to
//! source text, entry point and layout. There is no way to name a kernel and
//! supply a different body for it, because the body is not a parameter.
//!
//! # What is, and is not, part of kernel identity
//!
//! Workgroup extent **is**: WGSL has no runtime workgroup size, so each extent
//! is a distinct compiled module. The LeakyReLU slope **is not**: `alpha` is a
//! uniform read at runtime, so every slope shares one pipeline. That is why
//! [`ActivationEntry`] carries the discriminant only — folding `alpha` into the
//! key would compile a pipeline per slope value.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hymeko::tensor::compute::{BackendKind, ComputeError};

/// Which activation formula a kernel compiles.
///
/// Discriminant only. [`crate::ActivationKind`] additionally carries the
/// LeakyReLU slope, which is a runtime uniform and must not multiply pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActivationEntry {
    Identity,
    Relu,
    LeakyRelu,
    Sigmoid,
    Tanh,
}

impl ActivationEntry {
    /// The WGSL entry point implementing this formula.
    ///
    /// The same names are used by the out-of-place, in-place and fused-SpMM
    /// modules, which is what lets one `ActivationEntry` address all three.
    #[must_use]
    pub const fn entry_point(self) -> &'static str {
        match self {
            Self::Identity => "main_identity",
            Self::Relu => "main_relu",
            Self::LeakyRelu => "main_leaky_relu",
            Self::Sigmoid => "main_sigmoid",
            Self::Tanh => "main_tanh",
        }
    }
}

/// The bind-group shape a kernel requires.
///
/// Separate from [`KernelId`] because several kernels share one shape: raw and
/// fused SpMM bind the same six resources, so they share one cached
/// `BindGroupLayout` and one `PipelineLayout` instead of rebuilding both on
/// every `prepare()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutId {
    /// CSR row_ptr/col_ind/values, X, Y, params.
    Spmm,
    /// input (read), output (read_write), params.
    ActivationOutOfPlace,
    /// data (read_write), params.
    ActivationInPlace,
    /// Features, topology, spline/gate parameters, output and uniform shape.
    Hsikan,
    /// Queries, keys, values, preferred rotor, CSR, output and uniform shape.
    RotorAttention,
    /// Node features, star centres, membership, output and uniform shape.
    StarRotorSpike,
    /// output (read_write), params.
    Fill,
}

impl LayoutId {
    fn label(self) -> &'static str {
        match self {
            Self::Spmm => "spmm.layout",
            Self::ActivationOutOfPlace => "activation.layout",
            Self::ActivationInPlace => "activation.layout.in_place",
            Self::Hsikan => "hsikan.layout",
            Self::RotorAttention => "rotor_attention.layout",
            Self::StarRotorSpike => "star_rotor_spike.layout",
            Self::Fill => "fill.layout",
        }
    }

    fn entries(self) -> Vec<wgpu::BindGroupLayoutEntry> {
        match self {
            Self::Spmm => vec![
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, false),
                uniform_entry(5),
            ],
            Self::ActivationOutOfPlace => vec![
                storage_entry(0, true),
                storage_entry(1, false),
                uniform_entry(2),
            ],
            Self::ActivationInPlace | Self::Fill => {
                vec![storage_entry(0, false), uniform_entry(1)]
            }
            Self::Hsikan => vec![
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, true),
                storage_entry(6, true),
                storage_entry(7, false),
                uniform_entry(8),
            ],
            Self::RotorAttention => vec![
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, true),
                storage_entry(6, false),
                uniform_entry(7),
            ],
            Self::StarRotorSpike => vec![
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, false),
                uniform_entry(4),
            ],
        }
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Every compute kernel this crate can build.
///
/// A closed enum on purpose. Because it maps to source, entry point and layout,
/// naming a kernel fully determines what will be compiled — which is what makes
/// the cache invariant structural instead of a convention callers must honour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KernelId {
    /// Raw `Y = A·X`, no activation.
    Spmm,
    /// Fused `Y = f(A·X)`.
    SpmmActivation(ActivationEntry),
    /// `out[i] = f(in[i])`.
    ActivationOutOfPlace(ActivationEntry),
    /// `data[i] = f(data[i])`.
    ActivationInPlace(ActivationEntry),
    /// Signed HSiKAN Chebyshev--Catmull-Rom inference.
    Hsikan,
    /// Rotation-invariant spatial covariance and eigen-entropy pooling.
    GlobalEntropyPool,
    /// Sparse, single-head, untransported quaternion rotor attention.
    RotorAttention,
    /// Star-centred rotor-spike orientation population code.
    StarRotorSpike,
    /// Skeleton validation kernel. Not an operation; retained because
    /// `tests/skeleton.rs` exercises the encode/submit/readback machinery with
    /// it in isolation from any real kernel.
    Fill,
}

impl KernelId {
    /// Logical module name. Two kernels sharing a name share a compiled module
    /// when their workgroup extent matches.
    #[must_use]
    pub const fn shader_name(self) -> &'static str {
        match self {
            // Raw SpMM is an entry point of the fused module: Phase 9 Part D
            // made `spmm_activation.wgsl` canonical so one `spmm_acc` serves
            // every variant.
            Self::Spmm | Self::SpmmActivation(_) => "spmm",
            Self::ActivationOutOfPlace(_) => "activation",
            Self::ActivationInPlace(_) => "activation_inplace",
            Self::Hsikan => "hsikan",
            Self::GlobalEntropyPool => "global_entropy_pool",
            Self::RotorAttention => "rotor_attention",
            Self::StarRotorSpike => "star_rotor_spike",
            Self::Fill => "fill",
        }
    }

    /// WGSL source. Not a parameter — this is what closes the aliasing hole.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::Spmm | Self::SpmmActivation(_) => crate::operations::SPMM_SHADER,
            Self::ActivationOutOfPlace(_) => crate::operations::ACTIVATION_SHADER,
            Self::ActivationInPlace(_) => crate::operations::ACTIVATION_INPLACE_SHADER,
            Self::Hsikan => crate::operations::HSIKAN_SHADER,
            Self::GlobalEntropyPool => crate::operations::GLOBAL_ENTROPY_POOL_SHADER,
            Self::RotorAttention => crate::operations::ROTOR_ATTENTION_SHADER,
            Self::StarRotorSpike => crate::operations::STAR_ROTOR_SPIKE_SHADER,
            Self::Fill => crate::FILL_SHADER,
        }
    }

    #[must_use]
    pub const fn entry_point(self) -> &'static str {
        match self {
            Self::Spmm => "main_raw",
            Self::SpmmActivation(a)
            | Self::ActivationOutOfPlace(a)
            | Self::ActivationInPlace(a) => a.entry_point(),
            Self::Fill => "main",
            Self::Hsikan => "main",
            Self::GlobalEntropyPool => "main",
            Self::RotorAttention => "main",
            Self::StarRotorSpike => "main",
        }
    }

    #[must_use]
    pub const fn layout(self) -> LayoutId {
        match self {
            Self::Spmm | Self::SpmmActivation(_) => LayoutId::Spmm,
            Self::ActivationOutOfPlace(_) => LayoutId::ActivationOutOfPlace,
            Self::ActivationInPlace(_) => LayoutId::ActivationInPlace,
            Self::Hsikan => LayoutId::Hsikan,
            Self::GlobalEntropyPool => LayoutId::ActivationOutOfPlace,
            Self::RotorAttention => LayoutId::RotorAttention,
            Self::StarRotorSpike => LayoutId::StarRotorSpike,
            Self::Fill => LayoutId::Fill,
        }
    }
}

/// Everything a prepared operation needs to encode a dispatch, built together
/// and cached together so they cannot be mismatched.
#[derive(Debug)]
pub struct PipelineBundle {
    bind_group_layout: Arc<wgpu::BindGroupLayout>,
    pipeline_layout: Arc<wgpu::PipelineLayout>,
    pipeline: Arc<wgpu::ComputePipeline>,
}

impl PipelineBundle {
    /// The layout a caller must use to build a compatible bind group.
    #[must_use]
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    #[must_use]
    pub fn pipeline_layout(&self) -> &wgpu::PipelineLayout {
        &self.pipeline_layout
    }

    #[must_use]
    pub fn pipeline(&self) -> &Arc<wgpu::ComputePipeline> {
        &self.pipeline
    }
}

/// Identifies a specialised shader module: `(module name, wg_x, wg_y)`.
///
/// Omits the entry point, because entry points in one source with one workgroup
/// extent share a compiled module and need only distinct pipelines.
type ModuleKey = (&'static str, u32, u32);

/// Identifies a pipeline: a kernel plus the workgroup extent baked into it.
type BundleKey = (KernelId, u32, u32);

/// Caches bind-group layouts, pipeline layouts, shader modules and pipelines.
///
/// Interior mutability so the backend can hand out `&self` and still populate
/// the cache. **Not** global state — owned by one [`crate::WgpuBackend`] and
/// dropped with it.
///
/// # Concurrency contract
///
/// No lock is held across `create_shader_module` or `create_compute_pipeline`;
/// driver compilation can take milliseconds and serialising every backend
/// thread behind it would be a worse trade than the race below.
///
/// The consequence is that two threads missing on the same key concurrently may
/// both compile. That is accepted and bounded: **insertion is idempotent**, so
/// the first value to land wins and every later caller — including the thread
/// that lost — receives that same `Arc`. A transiently duplicated compilation is
/// discarded; the cache never returns two different objects for one key
/// afterwards. `pipeline_cache_converges_under_concurrent_misses` tests it.
#[derive(Debug, Default)]
pub struct PipelineCache {
    bundles: Mutex<HashMap<BundleKey, Arc<PipelineBundle>>>,
    modules: Mutex<HashMap<ModuleKey, Arc<wgpu::ShaderModule>>>,
    bind_group_layouts: Mutex<HashMap<LayoutId, Arc<wgpu::BindGroupLayout>>>,
    pipeline_layouts: Mutex<HashMap<LayoutId, Arc<wgpu::PipelineLayout>>>,
}

impl PipelineCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cached pipelines. Used by tests to prove a pipeline is built
    /// once and reused, rather than asserting it indirectly through timing.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bundles.lock().map(|m| m.len()).unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of cached bind-group layouts, for the test that proves layouts
    /// are not rebuilt on every `prepare()`.
    #[must_use]
    pub fn layout_count(&self) -> usize {
        self.bind_group_layouts.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Fetch or build the bundle for `kernel` at this workgroup extent.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Backend`] if a cache mutex is poisoned. Module and
    /// pipeline creation itself is infallible in wgpu — a malformed shader
    /// surfaces through the device error scope, not here.
    pub fn bundle(
        &self,
        device: &wgpu::Device,
        kernel: KernelId,
        workgroup_x: u32,
        workgroup_y: u32,
    ) -> Result<Arc<PipelineBundle>, ComputeError> {
        let key = (kernel, workgroup_x, workgroup_y);
        if let Some(found) = self
            .bundles
            .lock()
            .map_err(Self::poisoned)?
            .get(&key)
            .cloned()
        {
            return Ok(found);
        }

        let layout_id = kernel.layout();
        let bind_group_layout = self.bind_group_layout(device, layout_id)?;
        let pipeline_layout = self.pipeline_layout(device, layout_id, &bind_group_layout)?;
        let module = self.module(
            device,
            kernel.shader_name(),
            workgroup_x,
            workgroup_y,
            kernel.source(),
        )?;

        let pipeline = Arc::new(
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(kernel.shader_name()),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(kernel.entry_point()),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            }),
        );

        let bundle = Arc::new(PipelineBundle {
            bind_group_layout,
            pipeline_layout,
            pipeline,
        });

        // Idempotent insert: a concurrent miss that lost the race gets the
        // winner's bundle, so one key never yields two objects.
        Ok(Arc::clone(
            self.bundles
                .lock()
                .map_err(Self::poisoned)?
                .entry(key)
                .or_insert(bundle),
        ))
    }

    fn bind_group_layout(
        &self,
        device: &wgpu::Device,
        id: LayoutId,
    ) -> Result<Arc<wgpu::BindGroupLayout>, ComputeError> {
        if let Some(found) = self
            .bind_group_layouts
            .lock()
            .map_err(Self::poisoned)?
            .get(&id)
            .cloned()
        {
            return Ok(found);
        }
        let layout = Arc::new(
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(id.label()),
                entries: &id.entries(),
            }),
        );
        Ok(Arc::clone(
            self.bind_group_layouts
                .lock()
                .map_err(Self::poisoned)?
                .entry(id)
                .or_insert(layout),
        ))
    }

    fn pipeline_layout(
        &self,
        device: &wgpu::Device,
        id: LayoutId,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Result<Arc<wgpu::PipelineLayout>, ComputeError> {
        if let Some(found) = self
            .pipeline_layouts
            .lock()
            .map_err(Self::poisoned)?
            .get(&id)
            .cloned()
        {
            return Ok(found);
        }
        let layout = Arc::new(
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(id.label()),
                bind_group_layouts: &[Some(bind_group_layout)],
                immediate_size: 0,
            }),
        );
        Ok(Arc::clone(
            self.pipeline_layouts
                .lock()
                .map_err(Self::poisoned)?
                .entry(id)
                .or_insert(layout),
        ))
    }

    fn module(
        &self,
        device: &wgpu::Device,
        shader: &'static str,
        workgroup_x: u32,
        workgroup_y: u32,
        source: &str,
    ) -> Result<Arc<wgpu::ShaderModule>, ComputeError> {
        let mkey = (shader, workgroup_x, workgroup_y);
        if let Some(found) = self
            .modules
            .lock()
            .map_err(Self::poisoned)?
            .get(&mkey)
            .cloned()
        {
            return Ok(found);
        }

        // WGSL has no runtime workgroup size, so the extent is substituted
        // before compilation and a different extent is a different module.
        let specialised = source
            .replace("{{WORKGROUP_X}}", &workgroup_x.to_string())
            .replace("{{WORKGROUP_Y}}", &workgroup_y.to_string());
        let module = Arc::new(device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(shader),
            source: wgpu::ShaderSource::Wgsl(specialised.into()),
        }));
        Ok(Arc::clone(
            self.modules
                .lock()
                .map_err(Self::poisoned)?
                .entry(mkey)
                .or_insert(module),
        ))
    }

    fn poisoned<T>(_: std::sync::PoisonError<T>) -> ComputeError {
        ComputeError::Backend {
            backend: BackendKind::Wgpu,
            message: "pipeline cache mutex poisoned".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_identity_determines_source_entry_and_layout() {
        // The invariant, stated as a test: nothing about a kernel's compiled
        // form is supplied from outside.
        assert_eq!(KernelId::Spmm.layout(), LayoutId::Spmm);
        assert_eq!(
            KernelId::SpmmActivation(ActivationEntry::Relu).layout(),
            LayoutId::Spmm
        );
        assert_eq!(KernelId::Spmm.shader_name(), "spmm");
        assert_eq!(KernelId::Spmm.entry_point(), "main_raw");
        assert_eq!(
            KernelId::SpmmActivation(ActivationEntry::Relu).entry_point(),
            "main_relu"
        );
        // Raw and fused SpMM share source and layout, differing only by entry.
        assert_eq!(
            KernelId::Spmm.source().as_ptr(),
            KernelId::SpmmActivation(ActivationEntry::Tanh)
                .source()
                .as_ptr(),
            "raw and fused SpMM must be the same module"
        );
    }

    #[test]
    fn distinct_kernels_cannot_alias() {
        // Every kernel identity must differ from every other, or two kernels
        // would share a cache slot. This is the property the old string-keyed
        // design could not guarantee.
        let all = [
            KernelId::Spmm,
            KernelId::SpmmActivation(ActivationEntry::Relu),
            KernelId::SpmmActivation(ActivationEntry::Tanh),
            KernelId::ActivationOutOfPlace(ActivationEntry::Relu),
            KernelId::ActivationInPlace(ActivationEntry::Relu),
            KernelId::ActivationOutOfPlace(ActivationEntry::Identity),
            KernelId::Hsikan,
            KernelId::GlobalEntropyPool,
            KernelId::RotorAttention,
            KernelId::StarRotorSpike,
            KernelId::Fill,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{a:?} and {b:?} share an identity");
                }
            }
        }
    }

    #[test]
    fn in_place_and_out_of_place_activation_do_not_share_a_layout() {
        // They bind different resources; sharing a layout would let a bind
        // group built for one be used with the other's pipeline.
        assert_ne!(
            KernelId::ActivationInPlace(ActivationEntry::Relu).layout(),
            KernelId::ActivationOutOfPlace(ActivationEntry::Relu).layout()
        );
    }

    #[test]
    fn layout_entries_match_the_declared_shader_interfaces() {
        assert_eq!(LayoutId::Spmm.entries().len(), 6);
        assert_eq!(LayoutId::ActivationOutOfPlace.entries().len(), 3);
        assert_eq!(LayoutId::ActivationInPlace.entries().len(), 2);
        assert_eq!(LayoutId::Hsikan.entries().len(), 9);
        assert_eq!(LayoutId::RotorAttention.entries().len(), 8);
        assert_eq!(LayoutId::StarRotorSpike.entries().len(), 5);
        assert_eq!(LayoutId::Fill.entries().len(), 2);
        // SpMM binds Y writable and everything else read-only.
        let spmm = LayoutId::Spmm.entries();
        for (i, e) in spmm.iter().enumerate().take(4) {
            assert!(
                matches!(
                    e.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        ..
                    }
                ),
                "SpMM binding {i} must be read-only"
            );
        }
    }

    #[test]
    fn global_entropy_pool_reuses_the_compatible_three_binding_layout() {
        assert_eq!(
            KernelId::GlobalEntropyPool.layout(),
            LayoutId::ActivationOutOfPlace
        );
        assert_eq!(KernelId::GlobalEntropyPool.layout().entries().len(), 3);
    }
}
