// Signed HSiKAN Chebyshev--Catmull-Rom inference.
//
// Chebyshev coefficients are converted to control points by the canonical host
// spline owner. This kernel owns only device evaluation of the resulting CR
// splines, highway gate and signed branch means.

struct Params {
    n_nodes: u32,
    n_edges: u32,
    arity: u32,
    hidden: u32,
    branches: u32,
    grid: u32,
    count: u32,
    stride: u32,
};

@group(0) @binding(0) var<storage, read> x:             array<f32>;
@group(0) @binding(1) var<storage, read> vertices:      array<u32>;
@group(0) @binding(2) var<storage, read> signs:         array<i32>;
@group(0) @binding(3) var<storage, read> inner_control: array<f32>;
@group(0) @binding(4) var<storage, read> outer_control: array<f32>;
@group(0) @binding(5) var<storage, read> gate_w:        array<f32>;
@group(0) @binding(6) var<storage, read> gate_b:        array<f32>;
@group(0) @binding(7) var<storage, read_write> output:  array<f32>;
@group(0) @binding(8) var<uniform> params: Params;

// Both splines read the shared stencil (`catmull_rom_formulas.wgsl`, prepended) and form the same
// left-to-right sum the former inline bodies did.
fn cr(control_base: u32, raw: f32) -> f32 {
    let s = cr_stencil(raw, params.grid);
    return s.w.x * inner_control[control_base + s.idx.x]
         + s.w.y * inner_control[control_base + s.idx.y]
         + s.w.z * inner_control[control_base + s.idx.z]
         + s.w.w * inner_control[control_base + s.idx.w];
}

fn cr_outer(control_base: u32, raw: f32) -> f32 {
    let s = cr_stencil(raw, params.grid);
    return s.w.x * outer_control[control_base + s.idx.x]
         + s.w.y * outer_control[control_base + s.idx.y]
         + s.w.z * outer_control[control_base + s.idx.z]
         + s.w.w * outer_control[control_base + s.idx.w];
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var out_index = gid.x; out_index < params.count; out_index += params.stride) {
        let edge = out_index / params.hidden;
        let channel = out_index % params.hidden;
        var result = 0.0;

        for (var branch = 0u; branch < params.branches; branch += 1u) {
            let wanted = select(-1, 1, branch == 0u);
            var aggregate = 0.0;
            var count = 0.0;

            for (var member = 0u; member < params.arity; member += 1u) {
                let incidence = edge * params.arity + member;
                if (signs[incidence] == wanted) {
                    let vertex = vertices[incidence];
                    let x_base = vertex * params.hidden;
                    var z = gate_b[channel];
                    for (var input_channel = 0u; input_channel < params.hidden; input_channel += 1u) {
                        z += gate_w[channel * params.hidden + input_channel]
                           * x[x_base + input_channel];
                    }
                    let gate = 1.0 / (1.0 + exp(-z));
                    let value = x[x_base + channel];
                    let control_base = (branch * params.hidden + channel) * params.grid;
                    let inner = cr(control_base, value);
                    aggregate += gate * inner + (1.0 - gate) * value;
                    count += 1.0;
                }
            }

            aggregate /= max(count, 1.0);
            let outer_base = (branch * params.hidden + channel) * params.grid;
            result += cr_outer(outer_base, aggregate);
        }
        output[out_index] = result;
    }
}
