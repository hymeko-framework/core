// Catmull-Rom membership, pullback, first pass. One invocation per (chunk of rows, channel):
//
//   grad_x[e] = interior(x[e]) ? grad_y[e] * (sum_k dw_k c_k) * span : 0
//   partials[(chunk * channels + c) * grid + j] = sum over the chunk's rows of grad_y * w_k, idx_k = j
//
// as `hymeko_spline::catmull_rom_backward` defines them (the input gradient zero outside the clamp
// interval, the coefficient gradient accumulated everywhere). The coefficient sums are finished by
// `catmull_rom_reduce.wgsl` in chunk order. WGSL has no float atomics, and none are needed: every
// partial has one writer, and every sum a fixed order. Composed with `catmull_rom_formulas.wgsl`.

// The largest grid the private accumulator holds; `MAX_CATMULL_ROM_GRID` on the host.
const MAX_GRID: u32 = 32u;

struct Params {
    rows:       u32,
    channels:   u32,
    grid:       u32,
    chunk_rows: u32,
    count:      u32,   // chunks * channels
    stride:     u32,
    _pad0:      u32,
    _pad1:      u32,
};

@group(0) @binding(0) var<storage, read>       x:        array<f32>;
@group(0) @binding(1) var<storage, read>       control:  array<f32>;
@group(0) @binding(2) var<storage, read>       grad_y:   array<f32>;
@group(0) @binding(3) var<storage, read_write> grad_x:   array<f32>;
@group(0) @binding(4) var<storage, read_write> partials: array<f32>;
@group(0) @binding(5) var<uniform>             params:   Params;

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var unit = gid.x; unit < params.count; unit += params.stride) {
        let chunk = unit / params.channels;
        let channel = unit % params.channels;
        let base = channel * params.grid;
        var acc: array<f32, MAX_GRID>;
        for (var j = 0u; j < params.grid; j += 1u) {
            acc[j] = 0.0;
        }
        let first = chunk * params.chunk_rows;
        let last = min(first + params.chunk_rows, params.rows);
        for (var row = first; row < last; row += 1u) {
            let e = row * params.channels + channel;
            let s = cr_stencil(x[e], params.grid);
            let g = grad_y[e];
            acc[s.idx.x] += g * s.w.x;
            acc[s.idx.y] += g * s.w.y;
            acc[s.idx.z] += g * s.w.z;
            acc[s.idx.w] += g * s.w.w;
            var dy_dt = 0.0;
            dy_dt += s.dw.x * control[base + s.idx.x];
            dy_dt += s.dw.y * control[base + s.idx.y];
            dy_dt += s.dw.z * control[base + s.idx.z];
            dy_dt += s.dw.w * control[base + s.idx.w];
            grad_x[e] = select(0.0, g * dy_dt * s.span, s.interior);
        }
        let dst = unit * params.grid;
        for (var j = 0u; j < params.grid; j += 1u) {
            partials[dst + j] = acc[j];
        }
    }
}
