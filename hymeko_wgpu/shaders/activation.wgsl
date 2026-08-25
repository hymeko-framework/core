// Elementwise activation, out-of-place:  out[i] = f(in[i])
//
// ---------------------------------------------------------------------------
// Variant strategy
// ---------------------------------------------------------------------------
// One module, one entry point per activation kind. `PipelineCache`'s module key
// is (shader, workgroup_x, workgroup_y) and deliberately omits the entry point,
// so all kinds share ONE compiled module and differ only by pipeline. That is
// what "separate pipeline variant" costs here: a pipeline object, not a
// recompile.
//
// The alternative -- a `kind: u32` in the params and a switch in the body --
// was rejected: it puts a branch in every invocation for a value that is
// uniform across the whole dispatch, and it makes the transcendental kinds pay
// for the cheap ones' register budget.
//
// No runtime shader-string generation anywhere; `{{WORKGROUP_X}}` /
// `{{WORKGROUP_Y}}` are the same two substitutions every kernel in this crate
// uses, applied by PipelineCache before compilation.
//
// ---------------------------------------------------------------------------
// Numerical semantics (deliberate, and matched to the CPU convention)
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
// The resulting behaviour is exactly what the CPU layers already do --
// `if val > F::zero() { val } else { F::zero() }` in `conv/hgnn.rs`, and
// `if c > 0 { c } else { c * 0.01 }` in `conv/signed_hgnn.rs`.

struct Params {
    // Number of valid elements. Buffers are padded to a 1-element minimum for
    // empty tensors, so the kernel must bound itself by this and never by
    // arrayLength().
    count: u32,
    // LeakyReLU slope for the negative half. Ignored by other kinds; carried
    // for all of them so one Params layout serves every pipeline variant.
    alpha: f32,
    // Total invocations in the dispatch = groups * workgroup_size.
    //
    // When `count` exceeds what one dispatch can address (the device caps
    // workgroups per dimension at 65535, so 256-wide groups reach only 16.7M
    // elements), the dispatch is clamped and each invocation strides through
    // the remainder. Striding by the whole grid -- rather than giving each
    // invocation a contiguous block -- keeps adjacent lanes on adjacent
    // elements, so coalescing survives.
    //
    // When the grid already covers `count`, the loop body runs exactly once.
    stride: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<storage, read>       in_data:  array<f32>;
@group(0) @binding(1) var<storage, read_write> out_data: array<f32>;
@group(0) @binding(2) var<uniform>             params:   Params;

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_identity(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        out_data[i] = in_data[i];
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        let v = in_data[i];
        out_data[i] = select(0.0, v, v > 0.0);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_leaky_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        let v = in_data[i];
        out_data[i] = select(params.alpha * v, v, v > 0.0);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_sigmoid(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        out_data[i] = 1.0 / (1.0 + exp(-in_data[i]));
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_tanh(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        out_data[i] = tanh(in_data[i]);
    }
}
