//! [`WgpuBackend`] — device acquisition, capabilities, and the command
//! encode/submit boundary.
//!
//! # Design
//!
//! * **Builder** ([`WgpuBackendBuilder`]) because device acquisition is fallible
//!   and multi-step: instance → adapter → device, each of which can fail for a
//!   different reason the caller may want to distinguish.
//! * **Abstract Factory** ([`WgpuBackend::resources`]) because a GPU buffer
//!   cannot be created without a device, and the backend is the only thing that
//!   owns one.
//! * **Command** ([`WgpuBackend::encoder`] / [`WgpuBackend::submit`]) because
//!   several operations must share one command buffer and one submit. Nothing
//!   here submits implicitly.
//!
//! There is deliberately **no** automatic readback and **no** submit-per-call.

use hymeko::tensor::compute::{BackendKind, ComputeError};

use crate::block_on::block_on;
use crate::pipeline::PipelineCache;
use crate::resources::ResourceFactory;

/// What the acquired adapter and device actually support.
///
/// Recorded at build time so benchmarks and reports can state the device they
/// ran on, and so operations can validate a dispatch against real limits rather
/// than assumed ones.
#[derive(Clone, Debug)]
pub struct DeviceCapabilities {
    pub adapter_name: String,
    /// `"Vulkan"`, `"Dx12"`, … — which wgpu backend was selected.
    pub backend: String,
    /// `"DiscreteGpu"`, `"IntegratedGpu"`, …
    pub device_type: String,
    pub driver: String,
    pub driver_info: String,
    pub max_buffer_size: u64,
    pub max_storage_buffer_binding_size: u64,
    pub max_compute_workgroup_size_x: u32,
    pub max_compute_invocations_per_workgroup: u32,
    pub max_compute_workgroups_per_dimension: u32,
    pub max_storage_buffers_per_shader_stage: u32,
    /// Whether `Features::TIMESTAMP_QUERY_INSIDE_ENCODERS` was granted.
    ///
    /// Separate from [`Self::timestamp_query`] because timing a buffer *copy*
    /// needs `write_timestamp` between commands, which the base feature does
    /// not provide. An adapter can grant one without the other.
    pub timestamp_inside_encoders: bool,
    /// Whether `Features::TIMESTAMP_QUERY` was granted on this device.
    ///
    /// Gates kernel-only timing. When false, kernel time must be reported as
    /// unavailable rather than inferred by subtracting host-side measurements —
    /// that subtraction would fold in queue latency and be silently wrong.
    pub timestamp_query: bool,
    /// Nanoseconds per timestamp tick, valid only when `timestamp_query`.
    pub timestamp_period_ns: f32,
}

impl DeviceCapabilities {
    fn from_adapter(
        adapter: &wgpu::Adapter,
        limits: &wgpu::Limits,
        features: wgpu::Features,
        timestamp_period_ns: f32,
    ) -> Self {
        let info = adapter.get_info();
        Self {
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
            driver: info.driver,
            driver_info: info.driver_info,
            max_buffer_size: limits.max_buffer_size,
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_compute_workgroup_size_x: limits.max_compute_workgroup_size_x,
            max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            max_storage_buffers_per_shader_stage: limits.max_storage_buffers_per_shader_stage,
            timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
            timestamp_inside_encoders: features
                .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS),
            timestamp_period_ns,
        }
    }

    /// One-line summary for benchmark output and reports.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} [{}] {} (driver {} {})",
            self.adapter_name, self.backend, self.device_type, self.driver, self.driver_info
        )
    }
}

/// Configures and acquires a [`WgpuBackend`].
#[derive(Clone, Debug)]
pub struct WgpuBackendBuilder {
    backends: wgpu::Backends,
    power_preference: wgpu::PowerPreference,
    label: Option<String>,
    required_limits: Option<wgpu::Limits>,
    required_features: wgpu::Features,
}

impl Default for WgpuBackendBuilder {
    fn default() -> Self {
        Self {
            // Only the backends this crate's feature set actually compiles.
            backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            // Compute workloads want the discrete part, not the integrated one.
            // This host has both an Intel iGPU and an RTX 5080.
            power_preference: wgpu::PowerPreference::HighPerformance,
            label: None,
            required_limits: None,
            required_features: wgpu::Features::empty(),
        }
    }
}

impl WgpuBackendBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict which wgpu backends may be selected.
    #[must_use]
    pub fn backends(mut self, backends: wgpu::Backends) -> Self {
        self.backends = backends;
        self
    }

    /// Prefer a discrete or a low-power adapter.
    #[must_use]
    pub fn power_preference(mut self, preference: wgpu::PowerPreference) -> Self {
        self.power_preference = preference;
        self
    }

    /// Debug label applied to the device.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Request limits above the conservative defaults.
    ///
    /// Left unset, the adapter's own limits are requested, which is what a
    /// compute workload on a desktop GPU wants — the downlevel defaults cap
    /// storage buffers at 128 MiB.
    #[must_use]
    pub fn required_limits(mut self, limits: wgpu::Limits) -> Self {
        self.required_limits = Some(limits);
        self
    }

    /// Request optional device features, intersected with adapter support.
    ///
    /// Anything the adapter does not offer is silently dropped rather than
    /// failing the build: a benchmark asking for `TIMESTAMP_QUERY` should still
    /// run without kernel timing, and [`DeviceCapabilities::timestamp_query`]
    /// reports what was actually granted.
    #[must_use]
    pub fn features(mut self, features: wgpu::Features) -> Self {
        self.required_features = features;
        self
    }

    /// Acquire instance, adapter and device.
    ///
    /// Blocking: wgpu's init futures resolve without an event loop on native
    /// backends (see [`crate::block_on`]).
    ///
    /// # Errors
    /// [`ComputeError::BackendUnavailable`] when no adapter matches, or the
    /// device request is refused. That variant is distinct from
    /// [`ComputeError::Backend`] on purpose: *no adapter* is an environment
    /// fact a test may skip on, whereas a later failure is a defect.
    pub fn build(self) -> Result<WgpuBackend, ComputeError> {
        // `InstanceDescriptor` has no `Default` in wgpu 30 (the `display` field
        // is a boxed trait object), so it is built from the documented
        // constructor and then narrowed to the backends this crate compiles.
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = self.backends;
        let instance = wgpu::Instance::new(instance_desc);

        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: self.power_preference,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| ComputeError::BackendUnavailable {
            backend: BackendKind::Wgpu,
            reason: format!("no suitable adapter for {:?}: {e}", self.backends),
        })?;

        let limits = self
            .required_limits
            .clone()
            .unwrap_or_else(|| adapter.limits());

        // Only ask for what the adapter actually offers, so an unsupported
        // optional feature degrades the measurement instead of failing the build.
        let granted = self.required_features & adapter.features();

        let descriptor = wgpu::DeviceDescriptor {
            label: self.label.as_deref(),
            required_features: granted,
            required_limits: limits.clone(),
            ..Default::default()
        };

        let (device, queue) = block_on(adapter.request_device(&descriptor)).map_err(|e| {
            ComputeError::BackendUnavailable {
                backend: BackendKind::Wgpu,
                reason: format!("device request refused: {e}"),
            }
        })?;

        // `get_timestamp_period` is only meaningful when the feature was granted.
        let timestamp_period_ns = if granted.contains(wgpu::Features::TIMESTAMP_QUERY) {
            queue.get_timestamp_period()
        } else {
            0.0
        };
        let capabilities =
            DeviceCapabilities::from_adapter(&adapter, &limits, granted, timestamp_period_ns);

        Ok(WgpuBackend {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            capabilities,
            pipelines: PipelineCache::new(),
        })
    }
}

/// An acquired WGPU device, its capabilities, and its pipeline cache.
///
/// Owns every GPU resource handle. Core tensor types never do — an upload
/// adapter ([`WgpuBackend::resources`]) produces the device representation.
pub struct WgpuBackend {
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    capabilities: DeviceCapabilities,
    pipelines: PipelineCache,
}

impl WgpuBackend {
    #[must_use]
    pub fn builder() -> WgpuBackendBuilder {
        WgpuBackendBuilder::new()
    }

    /// Acquire with default settings.
    ///
    /// # Errors
    /// As [`WgpuBackendBuilder::build`].
    pub fn new() -> Result<Self, ComputeError> {
        Self::builder().build()
    }

    #[must_use]
    pub const fn kind(&self) -> BackendKind {
        BackendKind::Wgpu
    }

    #[must_use]
    pub const fn device(&self) -> &wgpu::Device {
        &self.device
    }

    #[must_use]
    pub const fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    #[must_use]
    pub const fn capabilities(&self) -> &DeviceCapabilities {
        &self.capabilities
    }

    #[must_use]
    pub const fn pipelines(&self) -> &PipelineCache {
        &self.pipelines
    }

    /// Resource factory bound to this device.
    #[must_use]
    pub const fn resources(&self) -> ResourceFactory<'_> {
        ResourceFactory::new(&self.device, &self.queue)
    }

    /// Operation factory bound to this device.
    ///
    /// Carries the device limits every operation validates against, so a
    /// builder never has to reach back for them.
    #[must_use]
    pub fn operations(&self) -> crate::OperationFactory<'_> {
        crate::OperationFactory::new(
            &self.device,
            &self.queue,
            &self.pipelines,
            self.capabilities.max_compute_workgroups_per_dimension,
            self.capabilities.max_compute_invocations_per_workgroup,
            self.capabilities.max_storage_buffers_per_shader_stage,
        )
    }

    /// Open a command encoder.
    ///
    /// Several prepared operations may encode into the same encoder before a
    /// single [`Self::submit`]. That is the whole point of exposing the encoder
    /// rather than hiding it behind a run-and-wait call.
    #[must_use]
    pub fn encoder(&self, label: Option<&str>) -> wgpu::CommandEncoder {
        self.device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label })
    }

    /// Submit a finished encoder. Does **not** wait.
    pub fn submit(&self, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        self.queue.submit(Some(encoder.finish()))
    }

    /// Copy a device tensor back to the host. **This is the explicit readback
    /// boundary** — nothing else in this crate moves data device→host.
    ///
    /// Costs a staging buffer, a copy, a submit and a device wait, which is
    /// exactly why the benchmark plan measures `gpu_resident` separately from
    /// `gpu_resident_readback`: calling this per operation defeats the point of
    /// keeping data resident.
    ///
    /// # Errors
    /// [`ComputeError::Backend`] if the copy, poll or buffer mapping fails.
    pub fn read_tensor<T>(&self, operand: &T) -> Result<Vec<f32>, ComputeError>
    where
        T: crate::scope::TensorOperand,
    {
        let tensor = operand.gpu_tensor();
        let elements = tensor
            .shape()
            .checked_elements()
            .ok_or(ComputeError::IndexOverflow {
                what: "tensor elements",
                value: usize::MAX,
            })?;
        if elements == 0 {
            return Ok(Vec::new());
        }

        let bytes = (elements * std::mem::size_of::<f32>()) as u64;
        let staging = self.resources().staging_buffer("readback", bytes);

        let mut encoder = self.encoder(Some("readback"));
        encoder.copy_buffer_to_buffer(tensor.buffer(), 0, &staging, 0, bytes);
        let index = self.submit(encoder);
        self.wait(index)?;

        let slice = staging.slice(..bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        // Drive the mapping callback. `PollType::Wait` blocks in the driver
        // rather than spinning.
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("poll during readback failed: {e}"),
            })?;

        rx.recv()
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("map callback never fired: {e}"),
            })?
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("buffer map failed: {e}"),
            })?;

        let view = slice
            .get_mapped_range()
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("mapped range unavailable: {e}"),
            })?;
        let out: Vec<f32> = bytemuck::cast_slice(&view).to_vec();
        drop(view);
        staging.unmap();
        Ok(out)
    }

    /// Block until a submission completes.
    ///
    /// Uses `PollType::Wait`, which blocks in the driver rather than spinning —
    /// the task's "no device busy-poll" rule.
    ///
    /// # Errors
    /// [`ComputeError::Backend`] if the device reports a poll failure.
    pub fn wait(&self, index: wgpu::SubmissionIndex) -> Result<(), ComputeError> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            })
            .map(|_| ())
            .map_err(|e| ComputeError::Backend {
                backend: BackendKind::Wgpu,
                message: format!("device poll failed: {e}"),
            })
    }
}

impl WgpuBackend {
    /// Create a [`crate::TensorArena`] that pools intermediate buffers for this
    /// backend.
    ///
    /// The arena borrows the backend for its whole life, which is what makes
    /// cross-device reuse structurally impossible (see
    /// [`crate::TensorArena::owns_device`]).
    ///
    /// There is deliberately no cached or global arena: a caller creates one per
    /// chain or per inference loop and drops it when done, so buffer lifetime
    /// stays visible at the call site.
    #[must_use]
    pub fn tensor_arena(&self, config: crate::TensorArenaConfig) -> crate::TensorArena<'_> {
        crate::TensorArena::new(self, config)
    }
}
