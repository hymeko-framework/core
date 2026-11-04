//! The Catmull-Rom membership on the device against the closed-form reference of
//! `hymeko_spline::catmull_rom` (plan 2026-09-28-p47-device-training-request, Amendment 3, K1).
//!
//! Bounds, a-priori forward-error bounds (the primer of P47 Amendment 3, Proposition 3):
//! - the forward, per element, `2 γ_9 Σ_k w̃_k |c|`; the input gradient `2 γ_11 |g| s Σ_k d̃_k |c|`,
//!   with `w̃_k`, `d̃_k` the polynomial masses of the weights and of their derivatives;
//! - the control-point gradient, a sum of `n_j` terms in another order than the CPU's,
//!   `γ_{n_j} Σ|g w|` against an `f64` reference, and exactly zero on a point no element touches;
//! - the device adjoint loop `Θ = |<W δc, g> - <δc, W^T g>|` under the sum of both sides' bounds.
//!
//! History: the first declaration (2026-10-11 13:01) bounded the forward and the input gradient by
//! `1e-6 · max(1, |ref|)`. Its first run missed the input-gradient bound by `1.0e-6` on one element at
//! grid 13: the declaration omitted the chain factor `|g| s` and the polynomial mass. The bounds above
//! replaced it from error analysis before the rerun (the plan, 14:04).

#[path = "support/mod.rs"]
mod support;

use hymeko_wgpu::testing::shared_backend;
use hymeko_wgpu::{KernelId, TensorShape, WgpuBackend};
use support::{cr, cr_forward_bound, cr_input_gradient_bound, cr_masses, cr_pullback, gamma};

/// `n` values across `[-scale, scale]`, a fixed sequence of `seed`.
fn field(n: usize, seed: u32, scale: f32) -> Vec<f32> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            scale * ((state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
        })
        .collect()
}

struct Case {
    rows: usize,
    channels: usize,
    grid: usize,
    x: Vec<f32>,
    control: Vec<f32>,
    grad_y: Vec<f32>,
}

impl Case {
    /// Rows that leave a ragged last chunk; inputs to `±1.3`, so some lie beyond the clamp.
    fn new(rows: usize, channels: usize, grid: usize) -> Self {
        Self {
            rows,
            channels,
            grid,
            x: field(rows * channels, 11, 1.3),
            control: field(channels * grid, 23, 1.0),
            grad_y: field(rows * channels, 37, 1.0),
        }
    }

    fn shape(&self) -> TensorShape {
        TensorShape::new(self.rows, self.channels)
    }

    fn control_shape(&self) -> TensorShape {
        TensorShape::new(self.channels, self.grid)
    }
}

/// The device forward of `case`.
fn forward(backend: &WgpuBackend, case: &Case) -> Vec<f32> {
    let resources = backend.resources();
    let x = resources.upload_tensor("x", case.shape(), &case.x).unwrap();
    let control = resources
        .upload_tensor("control", case.control_shape(), &case.control)
        .unwrap();
    let output = resources.zeroed_tensor("y", case.shape()).unwrap();
    let op = backend
        .operations()
        .catmull_rom_forward()
        .input(&x)
        .control(&control)
        .output(&output)
        .prepare()
        .unwrap();
    assert_eq!(op.kernel(), KernelId::CatmullRomForward);
    assert_eq!(op.elements(), case.x.len());
    let mut encoder = backend.encoder(Some("catmull-rom-forward"));
    op.encode(&mut encoder).unwrap();
    backend.wait(backend.submit(encoder)).unwrap();
    backend.read_tensor(&output).unwrap()
}

/// The device pullback of `case` at `chunk_rows`, encoded `repeats` times into one submission:
/// the input gradient and the control-point gradient.
fn pullback(
    backend: &WgpuBackend,
    case: &Case,
    chunk_rows: u32,
    repeats: usize,
) -> (Vec<f32>, Vec<f32>) {
    let resources = backend.resources();
    let x = resources.upload_tensor("x", case.shape(), &case.x).unwrap();
    let control = resources
        .upload_tensor("control", case.control_shape(), &case.control)
        .unwrap();
    let grad_y = resources
        .upload_tensor("grad_y", case.shape(), &case.grad_y)
        .unwrap();
    let grad_x = resources.zeroed_tensor("grad_x", case.shape()).unwrap();
    let grad_control = resources
        .zeroed_tensor("grad_control", case.control_shape())
        .unwrap();
    let op = backend
        .operations()
        .catmull_rom_pullback()
        .input(&x)
        .control(&control)
        .grad_output(&grad_y)
        .grad_input(&grad_x)
        .grad_control(&grad_control)
        .chunk_rows(chunk_rows)
        .prepare()
        .unwrap();
    assert_eq!(
        op.kernels(),
        [KernelId::CatmullRomPullback, KernelId::CatmullRomReduce]
    );
    assert_eq!(
        op.chunks() as usize,
        case.rows.div_ceil(chunk_rows as usize)
    );
    let mut encoder = backend.encoder(Some("catmull-rom-pullback"));
    for _ in 0..repeats {
        op.encode(&mut encoder).unwrap();
    }
    backend.wait(backend.submit(encoder)).unwrap();
    (
        backend.read_tensor(&grad_x).unwrap(),
        backend.read_tensor(&grad_control).unwrap(),
    )
}

/// The forward equals the reference per element at grids 3, 6 and 13, over inputs on both sides of
/// the clamp.
#[test]
fn the_forward_matches_the_reference() {
    let Some(backend) = shared_backend("suite") else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    for grid in [3, 6, 13] {
        let case = Case::new(333, 5, grid);
        let actual = forward(backend, &case);
        for (e, (&got, &raw)) in actual.iter().zip(&case.x).enumerate() {
            let channel = e % case.channels;
            let control = &case.control[channel * grid..(channel + 1) * grid];
            let want = cr(control, grid, raw);
            let bound = cr_forward_bound(control, grid, raw);
            assert!(
                f64::from((got - want).abs()) <= bound,
                "grid {grid}, element {e}: got {got}, want {want}, bound {bound}"
            );
        }
    }
}

/// The pullback equals the reference: the input gradient per element (zero beyond the clamp), the
/// control-point gradient under the recursive-summation bound, at grids 3, 6 and 13 and at chunks of
/// 64 rows (a ragged last one) and of 7. Encoded twice, the result is bit for bit the same: every sum
/// has one fixed order.
#[test]
fn the_pullback_matches_the_reference_and_is_deterministic() {
    let Some(backend) = shared_backend("suite") else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    for grid in [3, 6, 13] {
        let case = Case::new(333, 5, grid);
        let want = cr_pullback(&case.x, &case.control, case.channels, grid, &case.grad_y);
        for chunk_rows in [64, 7] {
            let (grad_x, grad_control) = pullback(backend, &case, chunk_rows, 1);
            for (e, (&got, &reference)) in grad_x.iter().zip(&want.grad_x).enumerate() {
                let channel = e % case.channels;
                let control = &case.control[channel * grid..(channel + 1) * grid];
                let bound = cr_input_gradient_bound(control, grid, case.x[e], case.grad_y[e]);
                assert!(
                    f64::from((got - reference).abs()) <= bound,
                    "grid {grid}, chunk {chunk_rows}, grad_x[{e}]: got {got}, want {reference}, \
                     bound {bound}"
                );
                if !(-1.0..=1.0).contains(&case.x[e]) {
                    assert_eq!(got, 0.0, "beyond the clamp the input gradient is zero");
                }
            }
            for (slot, ((&got, &reference), &scale)) in grad_control
                .iter()
                .zip(&want.grad_control)
                .zip(&want.magnitude)
                .enumerate()
            {
                let bound = gamma(want.terms_per_slot[slot]) * scale + f64::from(f32::MIN_POSITIVE);
                assert!(
                    (f64::from(got) - reference).abs() <= bound,
                    "grid {grid}, chunk {chunk_rows}, grad_control[{slot}]: got {got}, \
                     want {reference}, bound {bound}"
                );
            }
            let (again_x, again_control) = pullback(backend, &case, chunk_rows, 2);
            assert_eq!(
                again_x.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                grad_x.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
            assert_eq!(
                again_control
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                grad_control.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
    }
}

/// The forward/pullback pair closes its loop on the device: `<W δc, g>` (the forward run with the
/// control points `δc`) against `<δc, W^T g>` (the pullback's control-point gradient), both paired on
/// the host in `f64`, differ by no more than the sum of the two sides' a-priori bounds. A broken
/// pairing (the gradient read one control point off) exceeds that bound: the test has the power to
/// see a wrong pullback, with no CPU reference involved.
#[test]
fn the_forward_and_pullback_close_their_adjoint_loop() {
    let Some(backend) = shared_backend("suite") else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    for grid in [3, 6, 13] {
        let mut case = Case::new(301, 4, grid);
        let delta = field(case.channels * grid, 53, 1.0);
        let g = case.grad_y.clone();
        case.control = delta.clone();
        let forward_y = forward(backend, &case);
        let (_, w_t_g) = pullback(backend, &case, 64, 1);
        let left: f64 = forward_y
            .iter()
            .zip(&g)
            .map(|(&y, &gy)| f64::from(y) * f64::from(gy))
            .sum();
        let pair = |gradient: &[f32]| -> f64 {
            delta
                .iter()
                .zip(gradient)
                .map(|(&d, &q)| f64::from(d) * f64::from(q))
                .sum()
        };
        let right = pair(&w_t_g);
        let reference = cr_pullback(&case.x, &delta, case.channels, grid, &g);
        let forward_side: f64 = case
            .x
            .iter()
            .zip(&g)
            .enumerate()
            .map(|(e, (&raw, &gy))| {
                let channel = e % case.channels;
                let (weights, _, indices) = cr_masses(grid, raw);
                f64::from(gy.abs())
                    * gamma(9)
                    * weights
                        .iter()
                        .zip(indices)
                        .map(|(m, j)| m * f64::from(delta[channel * grid + j].abs()))
                        .sum::<f64>()
            })
            .sum();
        let pullback_side: f64 = delta
            .iter()
            .enumerate()
            .map(|(slot, &d)| {
                f64::from(d.abs())
                    * gamma(reference.terms_per_slot[slot])
                    * reference.magnitude[slot]
            })
            .sum();
        let bound = forward_side + pullback_side;
        let theta = (left - right).abs();
        assert!(
            theta <= bound,
            "grid {grid}: Θ {theta:e} above its bound {bound:e}"
        );
        let shifted: Vec<f32> = (0..w_t_g.len())
            .map(|slot| w_t_g[(slot + 1) % w_t_g.len()])
            .collect();
        let broken = (left - pair(&shifted)).abs();
        eprintln!("grid {grid}: Θ {theta:.3e}, bound {bound:.3e}, broken pairing {broken:.3e}");
        assert!(
            broken > bound,
            "grid {grid}: a broken pairing gives {broken:e}, not above the bound {bound:e}"
        );
    }
}

/// A control point no element touches has a gradient of exactly zero, bit for bit: its sum has no
/// terms. Inputs in `[-0.2, 0.2]` on a grid of 13 touch only the middle segments.
#[test]
fn untouched_control_points_have_an_exactly_zero_gradient() {
    let Some(backend) = shared_backend("suite") else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let mut case = Case::new(200, 3, 13);
    case.x = field(case.rows * case.channels, 71, 0.2);
    let reference = cr_pullback(&case.x, &case.control, case.channels, 13, &case.grad_y);
    let untouched: Vec<usize> = (0..reference.terms_per_slot.len())
        .filter(|&slot| reference.terms_per_slot[slot] == 0)
        .collect();
    assert!(
        untouched.len() >= 2 * case.channels,
        "the fixture leaves control points untouched: {untouched:?}"
    );
    let (_, grad_control) = pullback(backend, &case, 64, 1);
    for slot in untouched {
        assert_eq!(
            grad_control[slot].to_bits(),
            0.0f32.to_bits(),
            "untouched control point {slot}"
        );
    }
}

/// Operands that disagree, a grid outside 3..=32 and a chunk of zero rows are refused before any
/// dispatch.
#[test]
fn shapes_grids_and_chunks_are_refused_before_dispatch() {
    let Some(backend) = shared_backend("suite") else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let resources = backend.resources();
    let x = resources
        .upload_tensor("x", TensorShape::new(4, 2), &field(8, 1, 1.0))
        .unwrap();
    let wrong = resources
        .zeroed_tensor("wrong", TensorShape::new(3, 2))
        .unwrap();
    let output = resources
        .zeroed_tensor("y", TensorShape::new(4, 2))
        .unwrap();
    for (rows, grid) in [(2, 2), (2, 33), (3, 6)] {
        let control = resources
            .zeroed_tensor("control", TensorShape::new(rows, grid))
            .unwrap();
        let refused = backend
            .operations()
            .catmull_rom_forward()
            .input(&x)
            .control(&control)
            .output(&output)
            .prepare();
        assert!(refused.is_err(), "control [{rows}, {grid}] is refused");
    }
    let control = resources
        .zeroed_tensor("control", TensorShape::new(2, 6))
        .unwrap();
    assert!(
        backend
            .operations()
            .catmull_rom_forward()
            .input(&x)
            .control(&control)
            .output(&wrong)
            .prepare()
            .is_err()
    );
    let grad_control = resources
        .zeroed_tensor("grad_control", TensorShape::new(2, 6))
        .unwrap();
    let pullback = |chunk_rows: u32, grad_input| {
        backend
            .operations()
            .catmull_rom_pullback()
            .input(&x)
            .control(&control)
            .grad_output(&output)
            .grad_input(grad_input)
            .grad_control(&grad_control)
            .chunk_rows(chunk_rows)
            .prepare()
    };
    assert!(pullback(0, &output).is_err(), "a chunk of zero rows");
    assert!(
        pullback(4, &wrong).is_err(),
        "an input gradient of another shape"
    );
    assert!(pullback(4, &output).is_ok());
}
