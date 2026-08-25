//! Encode scope — the type-level proof that arena resources reach a tracked
//! submission.
//!
//! # The hole this closes
//!
//! Phase 4 left one gap, and reported it rather than hiding it: the arena
//! tracked releases only for encoders *it* created. A caller could take an
//! `ArenaTensor`, build an operation on it, encode into a raw
//! `backend.encoder()`, release the lease, and let the pool hand the buffer to
//! someone else while the unsubmitted command buffer still referenced it. Wrong
//! numbers, no error.
//!
//! Phase 4 offered documentation and one test as the mitigation. That is the
//! weakest kind of guarantee: it holds until someone writes the natural thing.
//!
//! # Why documentation was not enough, and a runtime check would not be either
//!
//! The hazard is *provenance*: whether a buffer came from the pool. By the time
//! an operation holds `&GpuTensor<f32>` that information is gone, so no runtime
//! check inside `encode()` can recover it — the tensor looks identical either
//! way. The provenance has to survive into the type, or it is unavailable at the
//! moment the decision is made.
//!
//! # The construction
//!
//! Three pieces, none of them dynamic:
//!
//! 1. [`TensorOperand`] gives every tensor type an associated [`EncodeScope`]:
//!    a plain [`crate::GpuTensor`] is [`Untracked`], an
//!    [`crate::ArenaTensor`] is [`Tracked`].
//! 2. [`JoinScope`] combines operand scopes at the type level. It is a
//!    two-by-two lattice with `Tracked` absorbing: an operation touching *any*
//!    pooled buffer is `Tracked`.
//! 3. The prepared operation carries the joined scope, and `encode` is
//!    implemented **twice** — once for `Untracked` taking a raw
//!    `wgpu::CommandEncoder`, once for `Tracked` taking
//!    [`crate::TrackedSession`]. There is no third impl, so a `Tracked`
//!    operation has nowhere untracked to go.
//!
//! The mistake is now rejected at compile time with a type error, not caught by
//! review.
//!
//! # Cost
//!
//! Zero at runtime: these are zero-sized markers in `PhantomData`, resolved
//! entirely during monomorphisation. No branch, no vtable, no field.
//!
//! The real cost is that [`crate::ArenaTensor`] no longer implements `Deref` to
//! `GpuTensor`. It cannot: `&*lease` would produce a plain `&GpuTensor`, whose
//! scope is `Untracked`, laundering the provenance the design depends on. Users
//! call `lease.tensor()` where a raw tensor is genuinely wanted — which is now
//! an explicit, greppable act rather than an invisible coercion.

use crate::resources::GpuTensor;

pub(crate) mod sealed {
    use crate::resources::GpuTensor;

    /// Marker sealing [`super::EncodeScope`].
    pub trait Sealed {}
    impl Sealed for super::Untracked {}
    impl Sealed for super::Tracked {}

    /// Raw buffer access for operand types.
    ///
    /// This trait is the seal on [`super::TensorOperand`]. It lives in a
    /// `pub(crate)` module, so:
    ///
    /// * crate-internal code (the operation builders, readback, host writes) can
    ///   name it and reach the buffer;
    /// * an external crate can neither call `gpu_tensor` nor implement it, and
    ///   therefore cannot implement `TensorOperand` either.
    ///
    /// The second point is the one that matters: without it an external type
    /// could wrap a pooled buffer, declare `Scope = Untracked`, and hand the
    /// result to a raw encoder -- reopening the laundering path through a
    /// different door.
    pub trait TensorAccess {
        fn gpu_tensor(&self) -> &GpuTensor<f32>;
    }
}

/// Where a prepared operation is allowed to be encoded.
///
/// Sealed: the two scopes are exhaustive, and a third would have to define its
/// own safety story.
pub trait EncodeScope: sealed::Sealed {
    /// Human-readable name, for error messages and diagnostics.
    const NAME: &'static str;
}

/// The operation touches no pooled memory; any encoder will do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Untracked;

/// The operation touches at least one [`crate::ArenaTensor`], so it must be
/// encoded into a [`crate::TrackedSession`] whose submission the arena observes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tracked;

impl EncodeScope for Untracked {
    const NAME: &'static str = "untracked";
}
impl EncodeScope for Tracked {
    const NAME: &'static str = "tracked";
}

/// Type-level join over scopes, with [`Tracked`] absorbing.
///
/// | lhs \ rhs | `Untracked` | `Tracked` |
/// |---|---|---|
/// | **`Untracked`** | `Untracked` | `Tracked` |
/// | **`Tracked`** | `Tracked` | `Tracked` |
///
/// Absorbing is the safe direction: one pooled operand is enough to require a
/// tracked submission, and mixing pooled and plain tensors in one operation is
/// legitimate (an arena output fed from an uploaded input, for instance).
pub trait JoinScope<Rhs: EncodeScope>: EncodeScope {
    type Out: EncodeScope;
}

impl JoinScope<Untracked> for Untracked {
    type Out = Untracked;
}
impl JoinScope<Tracked> for Untracked {
    type Out = Tracked;
}
impl JoinScope<Untracked> for Tracked {
    type Out = Tracked;
}
impl JoinScope<Tracked> for Tracked {
    type Out = Tracked;
}

/// Shorthand for the scope produced by adding operand `T` to an operation
/// currently in scope `S`.
pub type Joined<S, T> = <S as JoinScope<<T as TensorOperand>::Scope>>::Out;

/// Anything an operation can take as a dense `f32` operand.
///
/// **Sealed.** Implemented for exactly two types, and an external crate cannot
/// add a third: the supertrait [`sealed::TensorAccess`] lives in a `pub(crate)`
/// module and is therefore unimplementable and uncallable from outside.
///
/// # Why sealing is load-bearing rather than tidy
///
/// Without it, an external crate could define `struct Sneaky<'a>(&'a
/// ArenaTensor<'a>)`, implement `TensorOperand` for it with `Scope =
/// Untracked`, and pass that to a builder -- producing an untracked operation on
/// a pooled buffer and encoding it into a raw encoder. The scope system would
/// report exactly what it was told, and the guarantee would be void.
///
/// The seal makes the two impls below the only ones that can exist.
pub trait TensorOperand: sealed::TensorAccess {
    /// [`Untracked`] for a caller-owned tensor, [`Tracked`] for a pooled lease.
    type Scope: EncodeScope;
}

impl sealed::TensorAccess for GpuTensor<f32> {
    fn gpu_tensor(&self) -> &GpuTensor<f32> {
        self
    }
}

impl TensorOperand for GpuTensor<f32> {
    type Scope = Untracked;
}

// ---------------------------------------------------------------------------
// Type-level guarantees, demonstrated rather than asserted
// ---------------------------------------------------------------------------
//
// Each `compile_fail` below is paired with a `no_run` twin that differs only in
// the forbidden step. The pair matters: a `compile_fail` alone passes even when
// the snippet fails for an unrelated reason (a typo, a missing import), which
// would make it a test of nothing. The twin proves the surrounding code is
// sound, so the only difference is the property under test.
//
// `no_run` because building a `WgpuBackend` needs a GPU; these are compilation
// tests, not execution tests.

/// Guarantee 1 — a **pooled operation cannot be encoded into a raw encoder**.
///
/// This is the Phase 4 bypass, now a type error.
///
/// ```compile_fail,E0599
/// use hymeko_wgpu::*;
/// fn hole(
///     backend: &WgpuBackend,
///     arena: &TensorArena<'_>,
///     m: &GpuCsrMatrix<f32>,
///     x: &GpuTensor<f32>,
///     shape: TensorShape,
/// ) {
///     let out = arena.acquire(shape).unwrap();
///     let op = backend.operations().spmm()
///         .matrix(m).input(x).output(&out)   // `out` is pooled -> Tracked
///         .prepare().unwrap();
///     let mut raw = backend.encoder(None);
///     op.encode(&mut raw).unwrap();          // no such method for Tracked
/// }
/// ```
///
/// The same code through a session compiles:
///
/// ```no_run
/// use hymeko_wgpu::*;
/// fn ok(
///     backend: &WgpuBackend,
///     arena: &TensorArena<'_>,
///     m: &GpuCsrMatrix<f32>,
///     x: &GpuTensor<f32>,
///     shape: TensorShape,
/// ) {
///     let out = arena.acquire(shape).unwrap();
///     let op = backend.operations().spmm()
///         .matrix(m).input(x).output(&out)
///         .prepare().unwrap();
///     let mut session = arena.session(None);
///     op.encode(&mut session).unwrap();
///     session.submit();
/// }
/// ```
pub struct PooledOperandNeedsSession;

/// Guarantee 2 — a **prepared operation cannot outlive its operands**.
///
/// Releasing a lease while an operation still dispatches into it would let the
/// pool hand that buffer to a different logical tensor.
///
/// ```compile_fail,E0505
/// use hymeko_wgpu::*;
/// fn hole(
///     backend: &WgpuBackend,
///     arena: &TensorArena<'_>,
///     m: &GpuCsrMatrix<f32>,
///     x: &GpuTensor<f32>,
///     shape: TensorShape,
/// ) {
///     let out = arena.acquire(shape).unwrap();
///     let op = backend.operations().spmm()
///         .matrix(m).input(x).output(&out)
///         .prepare().unwrap();
///     drop(out);                              // released while `op` is alive
///     let mut session = arena.session(None);
///     op.encode(&mut session).unwrap();
/// }
/// ```
///
/// Dropping the operation first compiles:
///
/// ```no_run
/// use hymeko_wgpu::*;
/// fn ok(
///     backend: &WgpuBackend,
///     arena: &TensorArena<'_>,
///     m: &GpuCsrMatrix<f32>,
///     x: &GpuTensor<f32>,
///     shape: TensorShape,
/// ) {
///     let out = arena.acquire(shape).unwrap();
///     {
///         let op = backend.operations().spmm()
///             .matrix(m).input(x).output(&out)
///             .prepare().unwrap();
///         let mut session = arena.session(None);
///         op.encode(&mut session).unwrap();
///         session.submit();
///     }
///     drop(out);                              // released after `op` is gone
/// }
/// ```
pub struct PreparedCannotOutliveOperands;

/// Guarantee 3 — a lease **cannot be released twice**, and there is no handle to
/// go stale.
///
/// `ArenaTensor` is neither `Clone` nor `Copy`, and `Drop` is the only release
/// path, so a second release would require using a moved value.
///
/// ```compile_fail,E0382
/// use hymeko_wgpu::*;
/// fn hole(arena: &TensorArena<'_>, shape: TensorShape) {
///     let t = arena.acquire(shape).unwrap();
///     drop(t);
///     drop(t);                                 // use of moved value
/// }
/// ```
///
/// ```compile_fail,E0277
/// use hymeko_wgpu::*;
/// fn hole(arena: &TensorArena<'_>, shape: TensorShape) {
///     let t = arena.acquire(shape).unwrap();
///     let _alias = t.clone();                  // ArenaTensor: !Clone
/// }
/// ```
///
/// A single release compiles:
///
/// ```no_run
/// use hymeko_wgpu::*;
/// fn ok(arena: &TensorArena<'_>, shape: TensorShape) {
///     let t = arena.acquire(shape).unwrap();
///     drop(t);
/// }
/// ```
pub struct LeaseReleasedExactlyOnce;

/// Guarantee 4 — a **session cannot be submitted twice**.
///
/// `submit` takes `self`, so the second call is a use-after-move rather than a
/// runtime check that might be forgotten.
///
/// ```compile_fail,E0382
/// use hymeko_wgpu::*;
/// fn hole(arena: &TensorArena<'_>) {
///     let session = arena.session(None);
///     let _first = session.submit();
///     let _second = session.submit();          // use of moved value
/// }
/// ```
///
/// ```no_run
/// use hymeko_wgpu::*;
/// fn ok(arena: &TensorArena<'_>) {
///     let session = arena.session(None);
///     let _epoch = session.submit();
/// }
/// ```
pub struct SessionSubmittedOnce;

/// Guarantee 5 — **no public raw-tensor accessor** on a pooled lease.
///
/// # Why this guarantee exists separately from Guarantee 1
///
/// Phase 5 removed `Deref` and declared the laundering path closed. An
/// independent API audit then showed the claim was false: `ArenaTensor::tensor()`
/// was public and returned `&GpuTensor<f32>`, whose scope is [`Untracked`]. The
/// implicit bypass had been removed; an explicit one remained, and a single
/// method call reopened it:
///
/// ```text
/// .output(pooled.tensor())   // -> PreparedSpmm<'_, Untracked>
/// op.encode(&mut raw_encoder)
/// ```
///
/// That path was verified to compile before this corrective, so this doctest is
/// a regression test for a demonstrated hole rather than a hypothetical one.
///
/// ```compile_fail,E0599
/// use hymeko_wgpu::*;
/// fn hole(
///     backend: &WgpuBackend,
///     arena: &TensorArena<'_>,
///     m: &GpuCsrMatrix<f32>,
///     x: &GpuTensor<f32>,
///     shape: TensorShape,
/// ) {
///     let pooled = arena.acquire(shape).unwrap();
///     let op = backend.operations().spmm()
///         .matrix(m).input(x)
///         .output(pooled.tensor())    // no such public method
///         .prepare().unwrap();
///     let mut raw = backend.encoder(None);
///     op.encode(&mut raw).unwrap();
/// }
/// ```
///
/// The sealed accessor is equally unreachable from outside the crate:
///
/// ```compile_fail,E0599
/// use hymeko_wgpu::*;
/// fn hole(arena: &TensorArena<'_>, shape: TensorShape) {
///     let pooled = arena.acquire(shape).unwrap();
///     let _raw = pooled.gpu_tensor();   // sealed supertrait is not nameable
/// }
/// ```
///
/// Legitimate readback and host writes still work, through an API that takes the
/// operand and returns data rather than a reference:
///
/// ```no_run
/// use hymeko_wgpu::*;
/// fn ok(backend: &WgpuBackend, arena: &TensorArena<'_>, shape: TensorShape) {
///     let pooled = arena.acquire(shape).unwrap();
///     backend.resources().write_tensor(&pooled, &[1.0, 2.0]).unwrap();
///     let _values: Vec<f32> = backend.read_tensor(&pooled).unwrap();
///     let _shape = pooled.shape();
///     let _bytes = pooled.allocation_bytes();
/// }
/// ```
pub struct NoRawTensorAccessor;

/// Guarantee 6 — an **external crate cannot implement [`TensorOperand`]**.
///
/// Without the seal, a foreign type could wrap a pooled lease, declare
/// `Scope = Untracked`, and hand it to a builder — producing an untracked
/// operation on pooled memory through a door the scope system does not watch.
///
/// ```compile_fail,E0277
/// use hymeko_wgpu::*;
/// struct Sneaky<'a>(&'a ArenaTensor<'a>);
///
/// // Fails: the supertrait `sealed::TensorAccess` is `pub(crate)` and cannot be
/// // implemented, so `TensorOperand` cannot be either.
/// impl TensorOperand for Sneaky<'_> {
///     type Scope = Untracked;
/// }
/// ```
///
/// The two sanctioned operand types work as before:
///
/// ```no_run
/// use hymeko_wgpu::*;
/// fn ok<T: TensorOperand>(_t: &T) {}
/// fn call(plain: &GpuTensor<f32>, pooled: &ArenaTensor<'_>) {
///     ok(plain);
///     ok(pooled);
/// }
/// ```
pub struct TensorOperandIsSealed;

/// Guarantee 7 — `ArenaTensor` does **not** dereference to `GpuTensor`.
///
/// The implicit half of the same hole: `&*lease` would yield a plain
/// `&GpuTensor` whose scope is [`Untracked`].
///
/// ```compile_fail,E0614
/// use hymeko_wgpu::*;
/// fn hole(arena: &TensorArena<'_>, shape: TensorShape) {
///     let pooled = arena.acquire(shape).unwrap();
///     let _raw: &GpuTensor<f32> = &*pooled;
/// }
/// ```
///
/// ```no_run
/// use hymeko_wgpu::*;
/// fn ok(arena: &TensorArena<'_>, shape: TensorShape) {
///     let pooled = arena.acquire(shape).unwrap();
///     let _shape = pooled.shape();     // metadata without a raw reference
/// }
/// ```
pub struct NoDerefToRawTensor;
#[cfg(test)]
mod tests {
    use super::*;

    /// The join table, checked by construction rather than by reading it.
    ///
    /// `assert_scope::<A, B, Expected>()` compiles only if the join of `A` and
    /// `B` really is `Expected`, so this test would fail to build if an impl
    /// were wrong — which is a stronger signal than a runtime assertion.
    fn assert_scope<A, B, Expected>()
    where
        A: JoinScope<B>,
        B: EncodeScope,
        A::Out: SameAs<Expected>,
        Expected: EncodeScope,
    {
    }

    /// Type equality witness, so a mismatch is a compile error.
    trait SameAs<T> {}
    impl SameAs<Untracked> for Untracked {}
    impl SameAs<Tracked> for Tracked {}

    #[test]
    fn join_table_is_tracked_absorbing() {
        assert_scope::<Untracked, Untracked, Untracked>();
        assert_scope::<Untracked, Tracked, Tracked>();
        assert_scope::<Tracked, Untracked, Tracked>();
        assert_scope::<Tracked, Tracked, Tracked>();
    }

    #[test]
    fn scopes_are_named() {
        assert_eq!(Untracked::NAME, "untracked");
        assert_eq!(Tracked::NAME, "tracked");
    }

    /// A plain tensor must be untracked, or the whole scheme inverts.
    #[test]
    fn plain_tensor_is_untracked() {
        fn check<T: TensorOperand<Scope = Untracked>>() {}
        check::<GpuTensor<f32>>();
    }
}
