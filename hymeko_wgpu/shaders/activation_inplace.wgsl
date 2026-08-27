// Elementwise activation, in-place:  data[i] = f(data[i])
//
// ---------------------------------------------------------------------------
// Why this is a separate module rather than the out-of-place shader with the
// same buffer bound twice
// ---------------------------------------------------------------------------
// WebGPU's usage-scope rule forbids a buffer subresource being used as a
// writable storage binding while it is also used by any other binding in the
// same compute pass. Binding one buffer to both `in_data` (read) and
// `out_data` (read_write) of `activation.wgsl` is therefore a validation error,
// not a portability grey area.
//
// The legal way to do it is to stop having two bindings: this module declares
// ONE `read_write` binding and reads and writes the same element. No aliasing
// exists to violate, no unsafe, no backend-specific escape hatch.
//
// ---------------------------------------------------------------------------
// What Phase 9 changed, and what it deliberately did not
// ---------------------------------------------------------------------------
// The activation FORMULAS are no longer duplicated: they come from
// `activation_formulas.wgsl`, prepended at compile time, so this module and the
// out-of-place one share one definition of each kind.
//
// The two modules still exist separately, and that is a genuine WGSL resource-
// interface limit rather than laziness. A single module would have to declare
// binding 0 as `read_write` to serve the in-place entry points, which would
// force the out-of-place path to bind its INPUT as a writable storage buffer --
// losing the read-only guarantee at the API boundary and the driver's
// read-only optimisation, to save two declarations. The trade is not worth it.
// See the Phase 9 report, Part D.
//
// Numerical semantics are defined once in `activation_formulas.wgsl`.

struct Params {
    count: u32,
    alpha: f32,
    // Total invocations in the dispatch; see `activation.wgsl` for why the grid
    // may be smaller than `count` and each invocation strides through the rest.
    stride: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<storage, read_write> data:   array<f32>;
@group(0) @binding(1) var<uniform>             params: Params;

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_identity(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        // A genuine no-op on the values, but still a real dispatch: it keeps
        // the in-place path uniform across kinds and is what a caller
        // selecting Identity at runtime expects to cost.
        data[i] = act_identity(data[i], params.alpha);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        data[i] = act_relu(data[i], params.alpha);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_leaky_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        data[i] = act_leaky_relu(data[i], params.alpha);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_sigmoid(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        data[i] = act_sigmoid(data[i], params.alpha);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_tanh(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        data[i] = act_tanh(data[i], params.alpha);
    }
}
