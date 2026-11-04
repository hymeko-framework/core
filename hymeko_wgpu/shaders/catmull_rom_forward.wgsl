// Catmull-Rom membership, forward: out[e] = S_c(x[e]) over x [rows, channels], channel c = e % channels,
// control points [channels, grid]. Composed with `catmull_rom_formulas.wgsl`.

struct Params {
    count:    u32,   // rows * channels
    channels: u32,
    grid:     u32,
    stride:   u32,
};

@group(0) @binding(0) var<storage, read>       x:       array<f32>;
@group(0) @binding(1) var<storage, read>       control: array<f32>;
@group(0) @binding(2) var<storage, read_write> output:  array<f32>;
@group(0) @binding(3) var<uniform>             params:  Params;

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var e = gid.x; e < params.count; e += params.stride) {
        let s = cr_stencil(x[e], params.grid);
        let base = (e % params.channels) * params.grid;
        output[e] = s.w.x * control[base + s.idx.x]
                  + s.w.y * control[base + s.idx.y]
                  + s.w.z * control[base + s.idx.z]
                  + s.w.w * control[base + s.idx.w];
    }
}
