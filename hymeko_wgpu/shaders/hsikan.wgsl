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

fn cr(control_base: u32, raw: f32) -> f32 {
    let xc = clamp(raw, -1.0, 1.0);
    let span = 0.5 * f32(params.grid - 1u);
    let u = (xc + 1.0) * span;
    let i = min(u32(floor(u)), params.grid - 2u);
    let t = u - f32(i);
    let t2 = t * t;
    let t3 = t2 * t;
    let w0 = 0.5 * (-t3 + 2.0 * t2 - t);
    let w1 = 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0);
    let w2 = 0.5 * (-3.0 * t3 + 4.0 * t2 + t);
    let w3 = 0.5 * (t3 - t2);
    let c0 = max(i, 1u) - 1u;
    let c1 = i;
    let c2 = min(i + 1u, params.grid - 1u);
    let c3 = min(i + 2u, params.grid - 1u);
    return w0 * inner_control[control_base + c0]
         + w1 * inner_control[control_base + c1]
         + w2 * inner_control[control_base + c2]
         + w3 * inner_control[control_base + c3];
}

fn cr_outer(control_base: u32, raw: f32) -> f32 {
    let xc = clamp(raw, -1.0, 1.0);
    let span = 0.5 * f32(params.grid - 1u);
    let u = (xc + 1.0) * span;
    let i = min(u32(floor(u)), params.grid - 2u);
    let t = u - f32(i);
    let t2 = t * t;
    let t3 = t2 * t;
    let w0 = 0.5 * (-t3 + 2.0 * t2 - t);
    let w1 = 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0);
    let w2 = 0.5 * (-3.0 * t3 + 4.0 * t2 + t);
    let w3 = 0.5 * (t3 - t2);
    let c0 = max(i, 1u) - 1u;
    let c1 = i;
    let c2 = min(i + 1u, params.grid - 1u);
    let c3 = min(i + 2u, params.grid - 1u);
    return w0 * outer_control[control_base + c0]
         + w1 * outer_control[control_base + c1]
         + w2 * outer_control[control_base + c2]
         + w3 * outer_control[control_base + c3];
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
