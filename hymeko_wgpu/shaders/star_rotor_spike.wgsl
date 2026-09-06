// Star-centred static rotor-spike orientation population code.

struct Params {
    n_nodes: u32,
    n_edges: u32,
    arity: u32,
    hidden: u32,
    bins: u32,
    projection_x: u32,
    projection_y: u32,
    count: u32,
    kappa: f32,
    stride: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<storage, read> features: array<f32>;
@group(0) @binding(1) var<storage, read> centres: array<f32>;
@group(0) @binding(2) var<storage, read> vertices: array<u32>;
@group(0) @binding(3) var<storage, read_write> output: array<f32>;
@group(0) @binding(4) var<uniform> params: Params;

const PI: f32 = 3.14159265358979323846;

fn energy(edge: u32, bin: u32) -> f32 {
    let mu = PI * f32(bin) / f32(params.bins);
    var result = 0.0;
    for (var member = 0u; member < params.arity; member += 1u) {
        let vertex = vertices[edge * params.arity + member];
        let gx = features[vertex * params.hidden + params.projection_x]
            - centres[edge * params.hidden + params.projection_x];
        let gy = features[vertex * params.hidden + params.projection_y]
            - centres[edge * params.hidden + params.projection_y];
        let magnitude = sqrt(gx*gx + gy*gy);
        if (magnitude >= 1.0e-6) {
            let theta = atan2(gy, gx);
            let phi = 2.0*theta - 2.0*mu;
            result += magnitude * exp(params.kappa * (cos(phi) - 1.0));
        }
    }
    return result;
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var edge = gid.x; edge < params.count; edge += params.stride) {
        var z = 1.0e-6;
        for (var bin = 0u; bin < params.bins; bin += 1u) {
            z += energy(edge, bin);
        }
        for (var bin = 0u; bin < params.bins; bin += 1u) {
            output[edge * params.bins + bin] = energy(edge, bin) / z;
        }
    }
}
