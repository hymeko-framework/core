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

use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use hymeko_tensor::compute::{ComputeError, TensorShape};
use hymeko_tensor::representations::tensor_csr::TensorCsr;

use crate::{ActivationKind, ArenaTensor, GpuCsrMatrix, GpuTensor, TrackedSession, WgpuBackend};

/// The default test backend's builder: every suite device that asks for no special feature.
fn default_backend() -> Result<WgpuBackend, ComputeError> {
    WgpuBackend::builder().label("hymeko-test").build()
}

/// A built backend, or `None` when the host has no usable adapter.
///
/// # Why availability is not a failure
///
/// A machine without a GPU should **skip** these tests, not fail them — otherwise CI on a headless
/// runner reports red for a condition the code cannot control. But every *other* initialisation
/// error is a real defect and panics, so a broken backend cannot masquerade as an absent one. This is
/// the one place that decides it, for [`BackendCell`] and [`GpuDiscipline`] alike.
fn classify(context: &str, built: Result<WgpuBackend, ComputeError>) -> Option<WgpuBackend> {
    match built {
        Ok(backend) => Some(backend),
        Err(ComputeError::BackendUnavailable { reason, .. }) => {
            eprintln!("[{context}] SKIPPED - no GPU adapter: {reason}");
            None
        }
        Err(other) => panic!("[{context}] backend init failed (not availability): {other}"),
    }
}

/// A benchmark's own backend for the whole run, or `None` without an adapter. The adapter is
/// printed, so a reported number names the device it was measured on.
#[must_use]
pub fn bench_backend(label: &str) -> Option<WgpuBackend> {
    let backend = classify(label, WgpuBackend::builder().label(label).build())?;
    eprintln!("GPU: {}", backend.capabilities().summary());
    Some(backend)
}

/// `n` values across `[-scale, scale]`, a fixed sequence of `seed`.
#[must_use]
pub fn field(n: usize, seed: u32, scale: f32) -> Vec<f32> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            scale * ((state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
        })
        .collect()
}

/// A suite-wide backend slot: built at most once, however many tests ask for it.
///
/// # Why a cell and not another function
///
/// A suite whose tests each build a device constructs several `WgpuBackend`s in rapid succession,
/// and this repository already records what that costs on Linux: "concurrent `WgpuBackend`
/// construction deadlocks in the Vulkan loader". Measured on 2026-09-08 it is worse than a deadlock —
/// a segmentation fault under the resulting device create/drop churn.
///
/// A `BackendCell` removes the churn instead of documenting a run-mode workaround: the `OnceLock`
/// guarantees one construction per process, and the value is shared read-only afterwards.
///
/// # Why it is a type rather than one function
///
/// Suites do not all want the *same* device. The profiling suite needs timestamp features; the rest
/// want the default. Both are `BackendCell`s — a suite declares its own `static` and supplies the
/// builder — so the skip/failure classification stays in **one** place instead of being hand-copied
/// per suite with its own subtly different error handling.
///
/// `WgpuBackend` is `Send + Sync`, so sharing one is sound. Nothing here locks or otherwise changes
/// production `WgpuBackend::build()`.
pub struct BackendCell {
    cell: OnceLock<Option<WgpuBackend>>,
    builds: AtomicUsize,
}

impl Default for BackendCell {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendCell {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cell: OnceLock::new(),
            builds: AtomicUsize::new(0),
        }
    }

    /// The suite's backend, building it on first use with `build`.
    ///
    /// # Returns
    /// `None` when the machine has no adapter — decided once, not per test. Any other
    /// initialisation error panics.
    pub fn get_or_init(
        &'static self,
        context: &str,
        build: impl FnOnce() -> Result<WgpuBackend, ComputeError>,
    ) -> Option<&'static WgpuBackend> {
        self.cell
            .get_or_init(|| {
                self.builds.fetch_add(1, Ordering::SeqCst);
                classify(context, build())
            })
            .as_ref()
    }

    /// How many times this cell actually invoked its builder.
    ///
    /// The observable receipt behind the one-device claim: a suite asserting `== 1` after all its
    /// tests have run is measuring the construction count, not asserting it in prose.
    #[must_use]
    pub fn builds(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }
}

/// How a test process uses the GPU: no Vulkan instance is created or destroyed while any other
/// Vulkan work runs in the process.
///
/// # Why
///
/// On Linux with several Vulkan drivers installed, creating or destroying an instance on one thread
/// while another thread uses the loader crashes inside `libvulkan.so`. Measured on 2026-09-27
/// (Nagare P43, loader 1.4.341, RADV + NVIDIA + lavapipe ICDs):
///
/// * `hymeko_wgpu --test arena`, which built a device per test, segfaulted in 13 of 20
///   default-threaded runs, and in 0 of 20 with the RADV driver alone;
/// * the faults were a null jump inside `vkCreateInstance` and a garbage load in a device call,
///   each while another test thread was constructing its backend;
/// * upstream records the same class: `gfx-rs/wgpu#5930`.
///
/// # What it guarantees
///
/// One `RwLock` per process, `LOADER`, models the process-global loader. Every discipline uses it,
/// however many exist, because the loader is one per process:
///
/// * [`Self::fresh`] builds, uses and drops a test's own device under the **write** guard, so
///   nothing else in the process touches Vulkan meanwhile;
/// * [`Self::shared`] uses the one shared device under a **read** guard, so shared tests run in
///   parallel with each other and never beside a fresh device's lifecycle;
/// * [`Self::shared_unguarded`] is the `'static` form for binaries that use nothing else. Mixing it
///   with fresh devices in one process would bring the race back, so the second of the two to be
///   used panics, in either order;
/// * a second guard asked for on a thread that already holds one panics instead of deadlocking
///   (the lock is not re-entrant). Two devices in one test are [`FreshGpu::sibling`].
pub struct GpuDiscipline {
    shared: BackendCell,
    unguarded_shared: AtomicBool,
    fresh_built: AtomicBool,
}

/// The process's Vulkan loader, as a lock: written while an instance is created or destroyed,
/// read while a shared device is used. See [`GpuDiscipline`].
static LOADER: RwLock<()> = RwLock::new(());

thread_local! {
    /// Whether this thread holds a guard of some `GpuDiscipline`; see [`GpuDiscipline`].
    static HOLDS_GPU_GUARD: Cell<bool> = const { Cell::new(false) };
}

/// Marks the current thread as holding a discipline guard for as long as it lives.
struct ThreadHold;

impl ThreadHold {
    fn take(context: &str) -> Self {
        HOLDS_GPU_GUARD.with(|held| {
            assert!(
                !held.replace(true),
                "[{context}] this thread already holds a GPU guard; the lock is not re-entrant and a \
                 second guard would deadlock. Take one per test; a second device is FreshGpu::sibling"
            );
        });
        Self
    }
}

impl Drop for ThreadHold {
    fn drop(&mut self) {
        HOLDS_GPU_GUARD.with(|held| held.set(false));
    }
}

impl Default for GpuDiscipline {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuDiscipline {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            shared: BackendCell::new(),
            unguarded_shared: AtomicBool::new(false),
            fresh_built: AtomicBool::new(false),
        }
    }

    /// A device of this test's own, built, used and dropped while nothing else in the process
    /// touches Vulkan. For a test that measures per-device state, such as an empty pipeline cache.
    ///
    /// # Panics
    /// When this process has used [`Self::shared_unguarded`], when this thread already holds a
    /// guard, or when initialisation fails for a reason other than a missing adapter.
    pub fn fresh(&'static self, context: &str) -> Option<FreshGpu> {
        let hold = ThreadHold::take(context);
        let guard = LOADER.write().unwrap_or_else(PoisonError::into_inner);
        assert!(
            !self.unguarded_shared.load(Ordering::SeqCst),
            "[{context}] this test process already uses shared_backend(), whose device is used \
             without a guard; a fresh device beside it would race the Vulkan loader. Use \
             shared_gpu() for the shared tests of a binary that also needs fresh_gpu()"
        );
        self.fresh_built.store(true, Ordering::SeqCst);
        let backend = classify(context, default_backend())?;
        Some(FreshGpu {
            backend,
            _guard: guard,
            _hold: hold,
        })
    }

    /// The process's shared device, for a test that does not care which device it runs on. Shared
    /// tests run in parallel with each other, never beside a fresh device's lifecycle.
    ///
    /// # Panics
    /// When this thread already holds a guard, or when initialisation fails for a reason other than
    /// a missing adapter.
    pub fn shared(&'static self, context: &str) -> Option<SharedGpu> {
        let hold = ThreadHold::take(context);
        let guard = LOADER.read().unwrap_or_else(PoisonError::into_inner);
        let backend = self.shared.get_or_init(context, default_backend)?;
        Some(SharedGpu {
            backend,
            _guard: guard,
            _hold: hold,
        })
    }

    /// The shared device as a plain `'static` reference, for a binary whose tests use nothing else.
    ///
    /// # Panics
    /// When this process has built a fresh device, or when initialisation fails for a reason other
    /// than a missing adapter.
    pub fn shared_unguarded(&'static self, context: &str) -> Option<&'static WgpuBackend> {
        let _construction = LOADER.read().unwrap_or_else(PoisonError::into_inner);
        assert!(
            !self.fresh_built.load(Ordering::SeqCst),
            "[{context}] this test process builds fresh devices; the unguarded shared device would \
             be used beside their lifecycle and race the Vulkan loader. Use shared_gpu()"
        );
        self.unguarded_shared.store(true, Ordering::SeqCst);
        self.shared.get_or_init(context, default_backend)
    }
}

/// A test's own device, alone in the process for its lifetime. Dereferences to the backend.
///
/// Fields drop in declaration order: the backend is destroyed while the write guard still holds.
pub struct FreshGpu {
    backend: WgpuBackend,
    _guard: RwLockWriteGuard<'static, ()>,
    _hold: ThreadHold,
}

impl FreshGpu {
    /// A second, distinct device under the same guard, for a test that needs two. It borrows this
    /// one, so it is dropped first, while the guard still holds.
    ///
    /// # Panics
    /// When initialisation fails for a reason other than a missing adapter.
    #[must_use]
    pub fn sibling(&self, context: &str) -> Option<SiblingGpu<'_>> {
        let backend = classify(context, default_backend())?;
        Some(SiblingGpu {
            backend,
            _within: PhantomData,
        })
    }
}

/// A second device built under a [`FreshGpu`]'s guard. Dereferences to the backend.
pub struct SiblingGpu<'g> {
    backend: WgpuBackend,
    _within: PhantomData<&'g FreshGpu>,
}

/// The process's shared device, held under a read guard. Dereferences to the backend.
pub struct SharedGpu {
    backend: &'static WgpuBackend,
    _guard: RwLockReadGuard<'static, ()>,
    _hold: ThreadHold,
}

impl Deref for FreshGpu {
    type Target = WgpuBackend;
    fn deref(&self) -> &WgpuBackend {
        &self.backend
    }
}

impl Deref for SiblingGpu<'_> {
    type Target = WgpuBackend;
    fn deref(&self) -> &WgpuBackend {
        &self.backend
    }
}

impl Deref for SharedGpu {
    type Target = WgpuBackend;
    fn deref(&self) -> &WgpuBackend {
        self.backend
    }
}

/// The test process's discipline. One per process, because the Vulkan loader it models is one per
/// process.
static PROCESS_GPU: GpuDiscipline = GpuDiscipline::new();

/// A fresh device for this test, alone in the process; see [`GpuDiscipline::fresh`].
#[must_use]
pub fn fresh_gpu(context: &str) -> Option<FreshGpu> {
    PROCESS_GPU.fresh(context)
}

/// The shared device under a read guard; see [`GpuDiscipline::shared`].
#[must_use]
pub fn shared_gpu(context: &str) -> Option<SharedGpu> {
    PROCESS_GPU.shared(context)
}

/// The default shared backend for a test binary — one device, whatever the test count — as a
/// `'static` reference, for a binary whose tests use nothing else; see
/// [`GpuDiscipline::shared_unguarded`].
///
/// # Returns
/// `None` when the machine has no adapter. Any other initialisation error still panics.
#[must_use]
pub fn shared_backend(context: &str) -> Option<&'static WgpuBackend> {
    PROCESS_GPU.shared_unguarded(context)
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

// === Backend-neutral sparse-CSR fixture ===
//
// Relocated to `hymeko_tensor::sparse_workload` (P1 benchmark-harness
// consolidation, `APPROVED-CORE-EDIT: benchmark-harness-consolidation`). It
// previously lived here byte-for-byte AND in `hymeko_core/benches/sparse_fixture.rs`;
// the single owner is now `hymeko_core` (the dependency edge is
// `hymeko_wgpu -> hymeko_core`, so core is the only crate both sides can reach).
// Re-exported here so every `hymeko_wgpu::testing::{Lcg, DegreeShape, SparseSpec}`
// path — in this crate's benches, examples, tests and `src/fanout_tests` — stays
// byte-identical. `hymeko_core`'s KAT (`tests/sparse_workload_kat.rs`) pins the
// emitted bytes, so the generated fixtures are unchanged.
#[doc(hidden)]
pub use hymeko_tensor::sparse_workload::{DegreeShape, Lcg, SparseSpec};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::TryLockError;

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

    /// Whether another thread could take `(the write guard, a read guard)` right now. A poisoned
    /// lock counts as free: the discipline recovers it.
    fn free_now() -> (bool, bool) {
        fn took<G>(attempt: std::sync::TryLockResult<G>) -> bool {
            !matches!(attempt, Err(TryLockError::WouldBlock))
        }
        std::thread::scope(|scope| {
            scope
                .spawn(|| (took(LOADER.try_write()), took(LOADER.try_read())))
                .join()
                .expect("the probe does not panic")
        })
    }

    /// Whether another thread obtains the write guard within a bound. Other tests in this binary
    /// share the lock and may hold it briefly, so "released" is checked by waiting, not by trying.
    fn released() -> bool {
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _write = LOADER.write().unwrap_or_else(PoisonError::into_inner);
            let _ = sent.send(());
        });
        received
            .recv_timeout(std::time::Duration::from_secs(60))
            .is_ok()
    }

    // Each test below owns a local `GpuDiscipline`, so its mixing flags are its own; the lock is the
    // process's, as it must be.

    #[test]
    fn a_fresh_device_excludes_every_other_guard_until_it_is_dropped() {
        static GPU: GpuDiscipline = GpuDiscipline::new();
        let Some(fresh) = GPU.fresh("exclusive") else {
            return;
        };
        assert_eq!(free_now(), (false, false));
        assert!(HOLDS_GPU_GUARD.with(Cell::get));
        drop(fresh);
        assert!(!HOLDS_GPU_GUARD.with(Cell::get));
        assert!(released());
    }

    /// The lock is the process's, not a discipline's: a fresh device of one discipline holds off
    /// every guard of another. With a lock per discipline (the first version of this module), the
    /// second discipline proceeded at once, and the lib test binary segfaulted in 5 of 5 runs.
    #[test]
    fn two_disciplines_share_the_one_loader_lock() {
        static FIRST: GpuDiscipline = GpuDiscipline::new();
        static SECOND: GpuDiscipline = GpuDiscipline::new();
        let Some(fresh) = FIRST.fresh("first") else {
            return;
        };
        let (sent, received) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let shared = SECOND.shared("second");
            let _ = sent.send(());
            drop(shared);
        });
        let blocked = received.recv_timeout(std::time::Duration::from_millis(300));
        assert!(
            blocked.is_err(),
            "the second discipline ran beside a fresh device"
        );
        drop(fresh);
        assert!(
            received
                .recv_timeout(std::time::Duration::from_secs(60))
                .is_ok()
        );
        waiter.join().expect("the waiter does not panic");
    }

    #[test]
    fn a_shared_device_excludes_a_fresh_devices_lifecycle() {
        static GPU: GpuDiscipline = GpuDiscipline::new();
        let Some(shared) = GPU.shared("parallel") else {
            return;
        };
        assert!(!free_now().0);
        drop(shared);
        assert!(released());
    }

    #[test]
    fn a_sibling_is_built_and_dropped_under_its_fresh_devices_guard() {
        static GPU: GpuDiscipline = GpuDiscipline::new();
        let Some(fresh) = GPU.fresh("pair") else {
            return;
        };
        let sibling = fresh
            .sibling("pair")
            .expect("the adapter that built one builds two");
        let arena = fresh.tensor_arena(crate::TensorArenaConfig::default());
        assert!(arena.owns_device(&fresh) && !arena.owns_device(&sibling));
        drop((arena, sibling));
        assert!(!free_now().0, "still under the fresh device's guard");
    }

    #[test]
    #[should_panic(expected = "already uses shared_backend()")]
    fn a_fresh_device_after_the_unguarded_shared_one_is_refused() {
        static GPU: GpuDiscipline = GpuDiscipline::new();
        let _shared = GPU.shared_unguarded("unguarded");
        let _fresh = GPU.fresh("fresh");
    }

    #[test]
    #[should_panic(expected = "builds fresh devices")]
    fn the_unguarded_shared_device_after_a_fresh_one_is_refused() {
        static GPU: GpuDiscipline = GpuDiscipline::new();
        drop(GPU.fresh("fresh"));
        let _shared = GPU.shared_unguarded("unguarded");
    }

    #[test]
    #[should_panic(expected = "already holds a GPU guard")]
    fn a_second_guard_on_one_thread_panics_instead_of_deadlocking() {
        let _first = ThreadHold::take("first");
        let _second = ThreadHold::take("second");
    }

    #[test]
    fn a_test_that_panics_under_the_write_guard_does_not_wedge_the_next() {
        static GPU: GpuDiscipline = GpuDiscipline::new();
        let failed = std::thread::spawn(|| {
            let _fresh = GPU.fresh("panicking");
            panic!("a failing test");
        })
        .join();
        assert!(failed.is_err());
        // With an adapter the panic poisoned the lock under the write guard. The next test must
        // neither panic on the poison nor wait forever.
        drop(GPU.fresh("after"));
        assert!(released());
    }
}
