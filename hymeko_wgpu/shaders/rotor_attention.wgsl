// Sparse single-head untransported quaternion rotor attention.

struct Params {
    tokens: u32,
    edge_count: u32,
    count: u32,
    stride: u32,
    kappa: f32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(0) @binding(0) var<storage, read> queries: array<f32>;
@group(0) @binding(1) var<storage, read> keys: array<f32>;
@group(0) @binding(2) var<storage, read> values: array<f32>;
@group(0) @binding(3) var<storage, read> preferred: array<f32>;
@group(0) @binding(4) var<storage, read> offsets: array<u32>;
@group(0) @binding(5) var<storage, read> sources: array<u32>;
@group(0) @binding(6) var<storage, read_write> output: array<f32>;
@group(0) @binding(7) var<uniform> params: Params;

fn identity() -> vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 0.0); }

fn normalise(q: vec4<f32>) -> vec4<f32> {
    let n = sqrt(dot(q, q));
    if (n < 1.0e-6) { return identity(); }
    return q / n;
}

fn quat_conj(q: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(q.x, -q.y, -q.z, -q.w);
}

fn quat_mul(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        a.x*b.x - a.y*b.y - a.z*b.z - a.w*b.w,
        a.x*b.y + a.y*b.x + a.z*b.w - a.w*b.z,
        a.x*b.z - a.y*b.w + a.z*b.x + a.w*b.y,
        a.x*b.w + a.y*b.z - a.z*b.y + a.w*b.x
    );
}

fn load_query(row: u32) -> vec4<f32> {
    let base = row * 4u;
    return vec4<f32>(queries[base], queries[base+1u], queries[base+2u], queries[base+3u]);
}

fn load_key(row: u32) -> vec4<f32> {
    let base = row * 4u;
    return vec4<f32>(keys[base], keys[base+1u], keys[base+2u], keys[base+3u]);
}

fn load_value(row: u32) -> vec4<f32> {
    let base = row * 4u;
    return vec4<f32>(values[base], values[base+1u], values[base+2u], values[base+3u]);
}

fn load_preferred() -> vec4<f32> {
    return vec4<f32>(preferred[0], preferred[1], preferred[2], preferred[3]);
}

fn score(qc: vec4<f32>, p: vec4<f32>, source: u32) -> f32 {
    let r = quat_mul(qc, normalise(load_key(source)));
    let c = dot(p, r);
    return params.kappa * (c*c - 1.0);
}

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var token = gid.x; token < params.count; token += params.stride) {
        let base = token * 4u;
        output[base] = 0.0;
        output[base+1u] = 0.0;
        output[base+2u] = 0.0;
        output[base+3u] = 0.0;
        let lo = offsets[token];
        let hi = offsets[token+1u];
        if (lo == hi) { continue; }
        let qc = quat_conj(normalise(load_query(token)));
        let p = normalise(load_preferred());
        var maximum = -3.402823466e+38;
        for (var edge = lo; edge < hi; edge += 1u) {
            maximum = max(maximum, score(qc, p, sources[edge]));
        }
        var denominator = 0.0;
        for (var edge = lo; edge < hi; edge += 1u) {
            denominator += exp(score(qc, p, sources[edge]) - maximum);
        }
        let inv = 1.0 / max(denominator, 1.0e-30);
        var accumulated = vec4<f32>(0.0);
        for (var edge = lo; edge < hi; edge += 1u) {
            let source = sources[edge];
            let weight = exp(score(qc, p, source) - maximum) * inv;
            accumulated += weight * load_value(source);
        }
        output[base] = accumulated.x;
        output[base+1u] = accumulated.y;
        output[base+2u] = accumulated.z;
        output[base+3u] = accumulated.w;
    }
}
