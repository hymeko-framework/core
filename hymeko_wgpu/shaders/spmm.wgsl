// CSR x dense SpMM:  Y = A * X, optionally fused with an activation.  (f32)
//
// A : CSR [num_rows x num_cols], nnz stored values
// X : dense row-major [num_cols x feature_dim]
// Y : dense row-major [num_rows x feature_dim]
//
// ---------------------------------------------------------------------------
// One module, one accumulation
// ---------------------------------------------------------------------------
// Phase 9 merged the former `spmm.wgsl` and `spmm_activation.wgsl`. They had
// carried two byte-identical copies of the accumulation loop, and a change to
// one would silently not reach the other.
//
// Now `spmm_acc` is written once and every entry point calls it:
//
//   main_raw          Y = A*X            (what the old spmm.wgsl `main` did)
//   main_relu         Y = relu(A*X)
//   main_leaky_relu   Y = leaky_relu(A*X)
//   main_sigmoid      Y = sigmoid(A*X)
//   main_tanh         Y = tanh(A*X)
//
// The entry point is selected statically when the pipeline is built, from
// `KernelId`. There is NO runtime activation-kind branch: a `kind: u32` in the
// params plus a switch would put a branch in every invocation for a value that
// is uniform across the whole dispatch, and would make the cheap kinds pay the
// transcendental kinds' register budget.
//
// The activation formulas themselves come from `activation_formulas.wgsl`,
// prepended at compile time, so `relu` means one thing across the fused,
// out-of-place and in-place kernels.
//
// Fusion is selected explicitly by the caller through
// `SpmmBuilder::fused_activation`. There is no automatic fusion pass: see
// `docs/compute-graph-decision.md`, which argues that fusion is a
// code-generation problem and a graph that fuses automatically reintroduces
// codegen through the back door.
//
// ---------------------------------------------------------------------------
// Invocation mapping
// ---------------------------------------------------------------------------
// One invocation computes exactly one Y element, i.e. one (row, feature) pair.
//
//   gid.x -> feature index   (fastest-varying)
//   gid.y -> row index
//
// The feature index is mapped to X so that adjacent invocations in a workgroup
// read adjacent f32s of X and write adjacent f32s of Y: both are row-major with
// `feature_dim` as the minor axis. Mapping rows to gid.x instead would make
// every lane in a wave touch a different CSR row and stride Y by feature_dim,
// which is the standard way to lose coalescing on this kernel.
//
// Because every invocation owns its own Y element exclusively, there is no
// sharing between invocations and therefore NO float atomics -- the same
// disjoint-output-partition property that let the CPU backend parallelise
// without atomics.
//
// ---------------------------------------------------------------------------
// Accumulation order
// ---------------------------------------------------------------------------
// The inner loop walks k = row_ptr[row] .. row_ptr[row+1] in increasing order
// and accumulates into a single scalar, exactly matching the CPU reference
// `TensorCsr::row_axpy`:
//
//     acc = 0; for k in start..end { acc += values[k] * x[col_ind[k]*fd + f] }
//
// Consequences that are deliberate, not accidental:
//   * a duplicate column index appears as two separate k iterations and is
//     therefore summed as two separate terms, matching the CPU;
//   * an empty CSR row (start == end) leaves acc at 0 and writes 0 (or f(0)
//     for a fused kind);
//   * no reassociation across invocations occurs, because the whole row is
//     summed by one invocation.
//
// Bitwise equality with the CPU is still NOT promised: the GPU may contract
// `acc + values[k] * xv` into a single fused multiply-add, which rounds once
// instead of twice. That is a per-term rounding difference, not a reordering.
//
// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------
// The dispatch is rounded up to whole workgroups, so tail invocations in both
// dimensions must be discarded. Bounds are taken from `params`, never from
// arrayLength(): buffers are padded to a minimum of one element so that a
// zero-sized tensor still has a valid allocation, and arrayLength would report
// that padding as real data.
//
// `{{WORKGROUP_X}}` / `{{WORKGROUP_Y}}` are substituted by PipelineCache.

struct Params {
    num_rows: u32,
    num_cols: u32,
    feature_dim: u32,
    // Occupies the 16-byte tail-padding slot a uniform-address-space struct
    // requires. Unfused entry points ignore it; the Rust `SpmmParams` mirrors
    // this field so the two layouts cannot drift apart silently.
    alpha: f32,
};

@group(0) @binding(0) var<storage, read>       row_ptr:  array<u32>;
@group(0) @binding(1) var<storage, read>       col_ind:  array<u32>;
@group(0) @binding(2) var<storage, read>       values:   array<f32>;
@group(0) @binding(3) var<storage, read>       x_data:   array<f32>;
@group(0) @binding(4) var<storage, read_write> y_data:   array<f32>;
@group(0) @binding(5) var<uniform>             params:   Params;

// The one accumulation. Returns the raw dot product for one (row, feature).
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
fn main_raw(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    y_data[gid.y * params.feature_dim + gid.x] = spmm_acc(gid.y, gid.x);
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = act_relu(acc, params.alpha);
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_leaky_relu(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = act_leaky_relu(acc, params.alpha);
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_sigmoid(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = act_sigmoid(acc, params.alpha);
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main_tanh(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_bounds(gid)) { return; }
    let acc = spmm_acc(gid.y, gid.x);
    y_data[gid.y * params.feature_dim + gid.x] = act_tanh(acc, params.alpha);
}
