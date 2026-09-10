// Identity-transport highway transition, per channel.
//
//   t = sigmoid(gate[c])
//   next[v, c] = t * step[v, c] + (1 - t) * carry[v, c]
//
// The algebra and the OPERATION ORDER are taken from the CPU owner
// (`hymeko_nagare::learning::signed_hsikan`, the transition block), not re-derived here:
//
//     let t = gate[i % d];
//     t * step[i] + (1.0 - t) * carry[i]
//
// so the two multiplies and the add happen in that sequence, and the gate is evaluated once per
// channel rather than once per element.
//
// The sigmoid is NOT redefined. This shader is composed with `activation_formulas.wgsl` -- the
// pattern `operations/activation.rs:31`, `operations/spmm.rs:40` -- and calls its `act_sigmoid`,
// whose body `1.0 / (1.0 + exp(-v))` is expression-identical to the CPU owner's
// `hymeko_nagare::stats::sigmoid`. `act_sigmoid` takes an unused `alpha`; 0.0 is passed.
//
// Identity transport only. A rotor-transported carry is a different operator and is deliberately
// absent: this slice covers `InterLayerUpdate::Highway(HighwayTransport::Identity)` alone.

struct Params {
    n_nodes: u32,
    hidden:  u32,
    count:   u32,   // n_nodes * hidden
    stride:  u32,
};

@group(0) @binding(0) var<storage, read>       step_state:  array<f32>;   // n_nodes * hidden
@group(0) @binding(1) var<storage, read>       carry_state: array<f32>;   // n_nodes * hidden
@group(0) @binding(2) var<storage, read>       gate:        array<f32>;   // hidden
@group(0) @binding(3) var<storage, read_write> next_state:  array<f32>;   // n_nodes * hidden
@group(0) @binding(4) var<uniform>             params:      Params;

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var i = gid.x; i < params.count; i += params.stride) {
        let channel = i % params.hidden;
        let t = act_sigmoid(gate[channel], 0.0);
        next_state[i] = t * step_state[i] + (1.0 - t) * carry_state[i];
    }
}
