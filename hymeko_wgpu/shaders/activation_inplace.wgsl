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
// The cost is that the activation expressions appear in two WGSL files. WGSL
// has no include mechanism and runtime shader-string assembly is explicitly out
// of scope, so the duplication is the honest price of a legal in-place path.
// It is bounded: each kind is one line, and both files are covered by the same
// CPU-reference tests, so a divergence between them fails a test rather than
// going unnoticed.
//
// Numerical semantics are identical to `activation.wgsl` -- see its header.

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
        data[i] = data[i];
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        let v = data[i];
        data[i] = select(0.0, v, v > 0.0);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_leaky_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        let v = data[i];
        data[i] = select(params.alpha * v, v, v > 0.0);
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_sigmoid(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        data[i] = 1.0 / (1.0 + exp(-data[i]));
    }
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_tanh(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i = i + params.stride) {
        data[i] = tanh(data[i]);
    }
}
