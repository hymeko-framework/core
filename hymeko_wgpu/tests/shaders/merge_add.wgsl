// TEST-ONLY elementwise add:  out[i] = lhs[i] + rhs[i]
//
// Exists solely so the Phase 5 fan-out test has a real two-input operation. It
// is deliberately NOT part of the library: the phase brief forbids widening the
// scope into a general kernel framework, and a merge kernel with no measured
// consumer would be exactly that.
//
// Kept minimal on purpose -- no workgroup substitution, no variants, no
// parameterisation beyond the element count. Its only job is to consume two
// buffers so a diamond topology can exist.

struct Params {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<storage, read>       lhs:    array<f32>;
@group(0) @binding(1) var<storage, read>       rhs:    array<f32>;
@group(0) @binding(2) var<storage, read_write> result: array<f32>;
@group(0) @binding(3) var<uniform>             params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.count) { return; }
    result[i] = lhs[i] + rhs[i];
}
