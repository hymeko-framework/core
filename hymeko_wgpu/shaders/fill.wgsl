// Phase 1 skeleton validation kernel.
//
// NOT an operation. Its only job is to make the framework skeleton observable
// end-to-end: pipeline creation, workgroup-size specialisation, bind group
// layout, dispatch, submit, and readback. Phase 2 replaces it with CSR SpMM as
// the first real kernel.
//
// `{{WORKGROUP_X}}` / `{{WORKGROUP_Y}}` are substituted by PipelineCache before
// compilation -- WGSL has no runtime workgroup size, so each extent is a
// distinct module. This 1D kernel uses Y = 1.

struct Params {
    // Number of valid elements in `out`. The buffer may be padded (an empty
    // tensor is backed by a 1-element allocation), so the kernel must bound
    // itself by this, never by arrayLength.
    count: u32,
    // Value written to every element.
    value: f32,
};

@group(0) @binding(0) var<storage, read_write> out_data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;

    // Bounds check: the dispatch is rounded up to whole workgroups, so the
    // tail invocations must not write past `count`.
    if (index >= params.count) {
        return;
    }

    out_data[index] = params.value;
}
