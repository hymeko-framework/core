// Fused CSR x dense SpMM + elementwise activation:  Y = f(A * X)    (f32)
//
// ---------------------------------------------------------------------------
// Why this file exists, and why it is hand-written
// ---------------------------------------------------------------------------
// Cell H of the Phase 3 benchmark: how much does fusing the activation into
// SpMM's store save, versus dispatching the two kernels back to back in one
// encoder (cell G)?
//
// The saving is bounded and predictable: one dispatch launch, plus one full
// read and one full write of Y through device memory. At 200k rows x 32
// features that is 25.6 MB read + 25.6 MB written that fusion never performs.
// Everything else -- the CSR traversal, the X gathers -- is unchanged.
//
// This is a HAND-WRITTEN fused variant, selected explicitly by the caller
// through `SpmmBuilder::fused_activation`. It is deliberately not produced by
// a runtime shader-string generator, and there is no automatic fusion pass:
// see `docs/compute-graph-decision.md`, which argues that fusion is a
// code-generation problem and that a graph which fuses automatically
// reintroduces codegen through the back door.
//
// ---------------------------------------------------------------------------
// Relationship to spmm.wgsl
// ---------------------------------------------------------------------------
// The body is spmm.wgsl's, with the activation applied to `acc` before the
// single store. Invocation mapping, accumulation order, bounds handling and
// the no-atomics property are identical and are documented there rather than
// repeated here.
//
// The duplication is real and bounded to the accumulation loop. It is the same
// trade already accepted for `activation_inplace.wgsl`: WGSL has no include
// mechanism, and the alternative -- assembling shader text at runtime -- is out
// of scope for this phase. `test_fused_matches_unfused_chain` compares this
// file's output against SpMM-then-activation on identical inputs, so a
// divergence fails a test rather than going unnoticed.
//
// Identity is intentionally absent: fusing a no-op is exactly the unfused
// kernel, and offering it would invite a caller to pay for a second pipeline
// to get spmm.wgsl's behaviour.
//
// Numerical semantics per kind are identical to activation.wgsl -- `select`
// rather than `max`, for NaN determinism. See that file's header.

struct Params {
    num_rows: u32,
    num_cols: u32,
    feature_dim: u32,
    // Occupies the tail-padding slot that `spmm.wgsl` declares as `_pad`, so
    // the Rust-side `SpmmParams` serves both shaders with one 16-byte layout.
    alpha: f32,
};

@group(0) @binding(0) var<storage, read>       row_ptr:  array<u32>;
@group(0) @binding(1) var<storage, read>       col_ind:  array<u32>;
@group(0) @binding(2) var<storage, read>       values:   array<f32>;
@group(0) @binding(3) var<storage, read>       x_data:   array<f32>;
@group(0) @binding(4) var<storage, read_write> y_data:   array<f32>;
@group(0) @binding(5) var<uniform>             params:   Params;

// The shared accumulation. Returns the raw dot product; each entry point
// applies its own activation to the result.
fn spmm_acc(row: u32, feature: u32) -> f32 {
    let start = row_ptr[row];
    let end   = row_ptr[row + 1u];

    var acc: f32 = 0.0;
    for (var k: u32 = start; k < end; k = k + 1u) {
        let col = col_ind[k];
        acc = acc + values[k] * x_data[col * params.feature_dim + feature];
    }
    return acc;
}

fn in_bounds(gid: vec3<u32>) -> bool {
    return gid.x < params.feature_dim && gid.y < params.num_rows;
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = select(0.0, acc, acc > 0.0);
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_leaky_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = select(params.alpha * acc, acc, acc > 0.0);
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_sigmoid(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = 1.0 / (1.0 + exp(-acc));
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_tanh(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = tanh(acc);
}
