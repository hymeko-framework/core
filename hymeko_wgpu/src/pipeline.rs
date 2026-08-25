//! Shader module and compute pipeline cache.
//!
//! # Why this exists
//!
//! Pipeline creation compiles WGSL and is orders of magnitude more expensive
//! than a dispatch. Rebuilding one per call is the single most common way to
//! make a GPU backend slower than the CPU it replaced, and the task names it
//! explicitly: *"Pipeline ne épüljön újra dispatchenként."*
//!
//! # Key design (Flyweight)
//!
//! [`PipelineKey`] contains **only** properties that genuinely require a
//! distinct pipeline: which shader source, which entry point, and the workgroup
//! size (which is baked into the WGSL at specialisation time, so two sizes are
//! two pipelines). Buffer identity, tensor shape and dispatch extent are *not*
//! in the key — they vary per call and do not change the pipeline.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hymeko::tensor::compute::{BackendKind, ComputeError};

/// Identifies a compute pipeline.
///
/// # Invariants
/// Two calls with an equal key must be able to share one pipeline object. If a
/// property would change the compiled code, it belongs here; if it only changes
/// the arguments, it does not.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    /// Logical shader name, e.g. `"spmm"`. Not the source text.
    pub shader: &'static str,
    /// WGSL entry point.
    pub entry_point: &'static str,
    /// Workgroup extent along X, baked into the module via specialisation.
    pub workgroup_x: u32,
    /// Workgroup extent along Y, baked into the module via specialisation.
    ///
    /// Phase 1 carried a single scalar size, which was sufficient for the
    /// 1D validation kernel. SpMM maps one invocation to one `(row, feature)`
    /// element and therefore needs a 2D workgroup, and the phase brief calls
    /// for measuring 8x8 / 16x8 / 16x16. Widening the key to 2D is the minimal
    /// change that supports that — a separate 2D cache beside the 1D one would
    /// have been duplicate infrastructure. 1D kernels use `workgroup_y = 1`.
    pub workgroup_y: u32,
}

impl PipelineKey {
    /// 2D workgroup key.
    #[must_use]
    pub const fn new_2d(
        shader: &'static str,
        entry_point: &'static str,
        workgroup_x: u32,
        workgroup_y: u32,
    ) -> Self {
        Self {
            shader,
            entry_point,
            workgroup_x,
            workgroup_y,
        }
    }

    /// 1D workgroup key — shorthand for `new_2d(.., x, 1)`.
    #[must_use]
    pub const fn new(shader: &'static str, entry_point: &'static str, workgroup_size: u32) -> Self {
        Self::new_2d(shader, entry_point, workgroup_size, 1)
    }

    /// Invocations per workgroup, for validation against
    /// `max_compute_invocations_per_workgroup`.
    #[must_use]
    pub const fn invocations(&self) -> u32 {
        self.workgroup_x * self.workgroup_y
    }
}

impl core::fmt::Display for PipelineKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{}::{}@{}x{}",
            self.shader, self.entry_point, self.workgroup_x, self.workgroup_y
        )
    }
}

/// Identifies a *specialised shader module*: `(shader name, wg_x, wg_y)`.
///
/// Coarser than [`PipelineKey`] — it omits the entry point, because two entry
/// points in the same source with the same workgroup extent share one compiled
/// module and only need distinct pipelines.
type ModuleKey = (&'static str, u32, u32);

/// Caches shader modules and compute pipelines by [`PipelineKey`].
///
/// Interior mutability so the backend can hand out `&self` and still populate
/// the cache. This is **not** global state — the cache is owned by one
/// [`crate::WgpuBackend`] and dies with it.
#[derive(Debug, Default)]
pub struct PipelineCache {
    pipelines: Mutex<HashMap<PipelineKey, Arc<wgpu::ComputePipeline>>>,
    modules: Mutex<HashMap<ModuleKey, Arc<wgpu::ShaderModule>>>,
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
        self.pipelines.lock().map(|m| m.len()).unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fetch or build the pipeline for `key`.
    ///
    /// `source` is the WGSL text, supplied by the caller so the cache does not
    /// need to know about any particular operation. `{{WORKGROUP_X}}` and
    /// `{{WORKGROUP_Y}}` in the source are substituted from the key — WGSL has
    /// no runtime workgroup size, so a different extent is a different module.
    ///
    /// # Errors
    /// [`ComputeError::Backend`] if the device rejects the module or pipeline.
    pub fn get_or_create(
        &self,
        device: &wgpu::Device,
        key: PipelineKey,
        source: &str,
        layout: Option<&wgpu::PipelineLayout>,
    ) -> Result<Arc<wgpu::ComputePipeline>, ComputeError> {
        if let Some(found) = self
            .pipelines
            .lock()
            .map_err(Self::poisoned)?
            .get(&key)
            .cloned()
        {
            return Ok(found);
        }

        let module =
            self.module_for(device, key.shader, key.workgroup_x, key.workgroup_y, source)?;

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(key.shader),
            layout,
            module: &module,
            entry_point: Some(key.entry_point),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let pipeline = Arc::new(pipeline);
        self.pipelines
            .lock()
            .map_err(Self::poisoned)?
            .insert(key, Arc::clone(&pipeline));
        Ok(pipeline)
    }

    fn module_for(
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

        let specialised = source
            .replace("{{WORKGROUP_X}}", &workgroup_x.to_string())
            .replace("{{WORKGROUP_Y}}", &workgroup_y.to_string());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(shader),
            source: wgpu::ShaderSource::Wgsl(specialised.into()),
        });

        let module = Arc::new(module);
        self.modules
            .lock()
            .map_err(Self::poisoned)?
            .insert(mkey, Arc::clone(&module));
        Ok(module)
    }

    fn poisoned<T>(_: std::sync::PoisonError<T>) -> ComputeError {
        ComputeError::Backend {
            backend: BackendKind::Wgpu,
            message: "pipeline cache mutex poisoned".to_owned(),
        }
    }
}
