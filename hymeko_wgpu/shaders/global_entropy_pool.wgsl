struct Params {
    channels: u32,
    height: u32,
    width: u32,
    area: u32,
    stride: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<storage, read> response: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

const EPS: f32 = 1.0e-6;

fn norm_coord(index: u32, extent: u32) -> f32 {
    if (extent <= 1u) {
        return 0.0;
    }
    return f32(index) / f32(extent - 1u);
}

fn entropy_term(value: f32) -> f32 {
    if (value > EPS) {
        return -value * log(value);
    }
    return 0.0;
}

@compute @workgroup_size({{WORKGROUP_X}}, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    var channel = gid.x;
    let area = params.area;
    while (channel < params.channels) {
        var mass = 0.0;
        var sx = 0.0;
        var sy = 0.0;
        var sxx = 0.0;
        var syy = 0.0;
        var sxy = 0.0;
        var row = 0u;
        loop {
            if (row >= params.height) { break; }
            var col = 0u;
            loop {
                if (col >= params.width) { break; }
                let value = response[channel * area + row * params.width + col];
                let weight = value * value;
                let x = norm_coord(col, params.width);
                let y = norm_coord(row, params.height);
                mass += weight;
                sx += weight * x;
                sy += weight * y;
                sxx += weight * x * x;
                syy += weight * y * y;
                sxy += weight * x * y;
                col += 1u;
            }
            row += 1u;
        }

        let out = channel * 3u;
        if (mass < EPS) {
            output[out] = 0.0;
            output[out + 1u] = 0.0;
            output[out + 2u] = 0.0;
        } else {
            let cx = sx / mass;
            let cy = sy / mass;
            let a = sxx / mass - cx * cx;
            let d = syy / mass - cy * cy;
            let b = sxy / mass - cx * cy;
            let trace = max(a + d, EPS);
            let determinant = max(a * d - b * b, 0.0);
            let q = clamp(determinant / (trace * trace), 0.0, 0.25);
            let disc = sqrt(max(1.0 - 4.0 * q, 0.0));
            let e1 = 0.5 * (1.0 + disc);
            let e2 = 0.5 * (1.0 - disc);
            output[out] = mass / f32(area);
            output[out + 1u] = trace;
            output[out + 2u] = entropy_term(e1) + entropy_term(e2);
        }
        channel += params.stride;
    }
}
