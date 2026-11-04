// Catmull-Rom membership, pullback, second pass: the coefficient gradient. One workgroup per
// (channel, control) slot; lane `l` of the workgroup sums the chunks `l, l + LANES, ...` in order,
// then the lanes are added by a fixed binary tree in workgroup memory:
//
//   grad_control[c * grid + j] = sum_{chunk} partials[(chunk * channels + c) * grid + j]
//
// Every addition has a fixed place, so the result is deterministic whatever the scheduling; no
// atomics. The order differs from a serial sum, which the recursive-summation bound allows.

const LANES: u32 = 128u;

struct Params {
    channels: u32,
    grid:     u32,
    chunks:   u32,
    count:    u32,   // channels * grid
    stride:   u32,   // workgroups dispatched
    _pad0:    u32,
    _pad1:    u32,
    _pad2:    u32,
};

@group(0) @binding(0) var<storage, read>       partials:     array<f32>;
@group(0) @binding(1) var<storage, read_write> grad_control: array<f32>;
@group(0) @binding(2) var<uniform>             params:       Params;

var<workgroup> lane_sums: array<f32, LANES>;

@compute @workgroup_size(128, 1)
fn main(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
) {
    for (var slot = group.x; slot < params.count; slot += params.stride) {
        let channel = slot / params.grid;
        let j = slot % params.grid;
        var sum = 0.0;
        for (var chunk = local.x; chunk < params.chunks; chunk += LANES) {
            sum += partials[(chunk * params.channels + channel) * params.grid + j];
        }
        lane_sums[local.x] = sum;
        workgroupBarrier();
        for (var width = LANES / 2u; width > 0u; width = width / 2u) {
            if (local.x < width) {
                lane_sums[local.x] = lane_sums[local.x] + lane_sums[local.x + width];
            }
            workgroupBarrier();
        }
        if (local.x == 0u) {
            grad_control[slot] = lane_sums[0];
        }
        workgroupBarrier();
    }
}
