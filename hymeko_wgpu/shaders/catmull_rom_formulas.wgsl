// Canonical Catmull-Rom stencil -- the single device definition of the clamp, the segment, the four
// weights, their t-derivatives and the endpoint-replicated control indices.
//
// The arithmetic is the CPU owner's, `hymeko_spline::catmull_rom` (`segment`, `cr_weights`,
// `cr_weight_derivatives`, `cr_control_indices`), expression for expression. Prepended to each
// module that evaluates a CR spline by `concat!(include_str!(..), include_str!(..))`, as
// `activation_formulas.wgsl` is: compile-time constant text, one place where the stencil is written.
//
// The caller reads its own control buffer at `idx` and forms `w.x c0 + w.y c1 + w.z c2 + w.w c3`
// left to right: storage buffers cannot be passed to a function, and the summation order stays the
// caller's to state.

struct CrStencil {
    w: vec4<f32>,
    dw: vec4<f32>,
    idx: vec4<u32>,
    // du/dx = (grid - 1) / 2, the segment parameter's slope.
    span: f32,
    // Whether the raw input lies in the closed interval [-1, 1], where the clamp's Jacobian is 1.
    interior: bool,
};

fn cr_stencil(raw: f32, grid: u32) -> CrStencil {
    let xc = clamp(raw, -1.0, 1.0);
    let span = 0.5 * f32(grid - 1u);
    let u = (xc + 1.0) * span;
    let i = min(u32(floor(u)), grid - 2u);
    let t = u - f32(i);
    let t2 = t * t;
    let t3 = t2 * t;
    var s: CrStencil;
    s.w = vec4<f32>(
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    );
    s.dw = vec4<f32>(
        0.5 * (-3.0 * t2 + 4.0 * t - 1.0),
        0.5 * (9.0 * t2 - 10.0 * t),
        0.5 * (-9.0 * t2 + 8.0 * t + 1.0),
        0.5 * (3.0 * t2 - 2.0 * t),
    );
    s.idx = vec4<u32>(max(i, 1u) - 1u, i, min(i + 1u, grid - 1u), min(i + 2u, grid - 1u));
    s.span = span;
    s.interior = raw >= -1.0 && raw <= 1.0;
    return s;
}
