// Canonical activation formulas — the single definition of each kind.
//
// ---------------------------------------------------------------------------
// How this is shared
// ---------------------------------------------------------------------------
// WGSL has no include mechanism, and assembling shader text at runtime is out
// of scope for this crate (see docs/compute-graph-decision.md). Before Phase 9
// the consequence was that each formula appeared in three files:
// activation.wgsl, activation_inplace.wgsl and spmm_activation.wgsl.
//
// This file is prepended to each of those modules by `concat!(include_str!(..),
// include_str!(..))` in Rust. That is **compile-time** constant concatenation:
// the shader text is fixed in the binary, there is no runtime generator, and
// the substitution PipelineCache performs is still only the workgroup extent.
//
// The result is one place where "what does ReLU mean" is written down. A change
// here reaches the out-of-place, in-place and fused-SpMM kernels together, so
// they cannot drift apart.
//
// ---------------------------------------------------------------------------
// Numerical semantics (deliberate, matched to the CPU convention)
// ---------------------------------------------------------------------------
// identity     v            -- bit-preserving: NaN payload and -0.0 survive
// relu         v > 0 ? v : +0.0
//                           -- NaN -> +0.0  (NaN > 0 is false)
//                           -- -0.0 -> +0.0, +0.0 -> +0.0
//                           -- -Inf -> +0.0, +Inf -> +Inf
// leaky_relu   v > 0 ? v : alpha * v
//                           -- NaN -> NaN   (alpha * NaN)
//                           -- -0.0 -> -0.0 for alpha > 0 (sign preserved)
//                           -- -Inf -> -Inf for alpha > 0
// sigmoid      1 / (1 + exp(-v))
//                           -- NaN -> NaN, +Inf -> 1, -Inf -> 0
// tanh         tanh(v)      -- NaN -> NaN, +Inf -> 1, -Inf -> -1
//
// `relu` and `leaky_relu` use `select` rather than `max(v, 0.0)` on purpose:
// WGSL leaves `max` with a NaN operand implementation-defined, whereas the
// comparison in `select` is well-defined (any comparison with NaN is false).
// This matches what the CPU layers already do -- `if val > F::zero() { val }
// else { F::zero() }` in `conv/hgnn.rs`, and `if c > 0 { c } else { c * 0.01 }`
// in `conv/signed_hgnn.rs`.
//
// Every formula takes `alpha` so the call sites are uniform; kinds that do not
// use it ignore it. That costs nothing -- the parameter is a compile-time
// constant fold away in every backend compiler -- and it keeps the entry-point
// bodies below identical in shape.

fn act_identity(v: f32, alpha: f32) -> f32 {
    return v;
}

fn act_relu(v: f32, alpha: f32) -> f32 {
    return select(0.0, v, v > 0.0);
}

fn act_leaky_relu(v: f32, alpha: f32) -> f32 {
    return select(alpha * v, v, v > 0.0);
}

fn act_sigmoid(v: f32, alpha: f32) -> f32 {
    return 1.0 / (1.0 + exp(-v));
}

fn act_tanh(v: f32, alpha: f32) -> f32 {
    return tanh(v);
}
