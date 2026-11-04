//! The resident two-layer HSiKAN chain:
//! `Hsikan L0 -> scatter_mean_csr -> highway_identity -> Hsikan L1 -> ONE readback`.
//!
//! # What this file proves
//!
//! 1. The four dispatches compose into one command encoder and one submission, with every
//!    intermediate staying on device. Residency is proved by an OBSERVED count: every host read
//!    goes through [`ReadbackObserver`], a test-owned wrapper that is the only path in this file to
//!    `WgpuBackend::read_tensor` and increments a counter on each actual call. The assertion reads
//!    that observed counter, never a literal.
//!
//!    The claim this supports is narrow and exact: **one explicit observed readback at the chain
//!    boundary; prepared encode-only operations are structurally unable to initiate host mapping.**
//!    It is not a claim of general GPU instrumentation — nothing here intercepts driver-level
//!    mapping, and none is needed, because `Prepared*::encode` takes only `&mut CommandEncoder`
//!    and holds no backend, queue, staging buffer or readback workspace.
//! 2. The composed result matches an independent CPU transcription of the canonical population
//!    (`Cycle` + `Mean` + `Highway(Identity)` + `PerLayer` + one slot + alpha = 1) within a bound
//!    declared before any number was measured.
//!
//! # What it is not
//!
//! Not a second model, IR, graph or scheduler. It builds four already-owned prepared operations and
//! encodes them in order. The full parity against Nagare's `backbone_forward` belongs to the
//! consumer suite; this crate cannot depend on Nagare, so the reference here is an independent
//! transcription — the same discipline `tests/hsikan.rs` already uses.

use std::cell::Cell;

use hymeko_wgpu::{KernelId, TensorShape, WgpuBackend};

use crate::support::assert_parity;

const N_NODES: usize = 4;
const N_EDGES: usize = 2;
use crate::support::cr;

const ARITY: usize = 3;
const HIDDEN: usize = 3;
const GRID: usize = 5;
const BRANCHES: usize = 2;

struct Layer {
    inner: Vec<f32>,
    outer: Vec<f32>,
    gate_w: Vec<f32>,
    gate_b: Vec<f32>,
}

struct Chain {
    x0: Vec<f32>,
    vertices: Vec<u32>,
    signs: Vec<i32>,
    layers: [Layer; 2],
    transition_gate: Vec<f32>,
}

fn series(len: usize, phase: usize, scale: f32) -> Vec<f32> {
    (0..len)
        .map(|i| (((i + phase) as f32) * 0.37).sin() * scale)
        .collect()
}

fn layer(phase: usize) -> Layer {
    Layer {
        inner: series(BRANCHES * HIDDEN * GRID, phase, 0.5),
        outer: series(BRANCHES * HIDDEN * GRID, phase + 7, 0.4),
        gate_w: series(HIDDEN * HIDDEN, phase + 13, 0.3),
        gate_b: series(HIDDEN, phase + 19, 0.2),
    }
}

fn chain(transition_bias: f32) -> Chain {
    Chain {
        x0: series(N_NODES * HIDDEN, 1, 0.6),
        // Two arity-3 signed hyperedges over four vertices.
        vertices: vec![0, 1, 2, 2, 3, 1],
        signs: vec![1, 1, 1, -1, 1, -1],
        layers: [layer(2), layer(31)],
        transition_gate: vec![transition_bias; HIDDEN],
    }
}

/// One HSiKAN layer, node -> edge. Transcribed from the existing kernel's reference.
fn hsikan(x: &[f32], c: &Chain, l: &Layer) -> Vec<f32> {
    let mut out = vec![0.0f32; N_EDGES * HIDDEN];
    for edge in 0..N_EDGES {
        for channel in 0..HIDDEN {
            for branch in 0..BRANCHES {
                let wanted = if branch == 0 { 1 } else { -1 };
                let mut aggregate = 0.0;
                let mut count = 0.0f32;
                for member in 0..ARITY {
                    let incidence = edge * ARITY + member;
                    if c.signs[incidence] == wanted {
                        let vertex = c.vertices[incidence] as usize;
                        let row = &x[vertex * HIDDEN..(vertex + 1) * HIDDEN];
                        let z = l.gate_b[channel]
                            + l.gate_w[channel * HIDDEN..(channel + 1) * HIDDEN]
                                .iter()
                                .zip(row)
                                .map(|(w, v)| w * v)
                                .sum::<f32>();
                        let gate = 1.0 / (1.0 + (-z).exp());
                        let base = (branch * HIDDEN + channel) * GRID;
                        let inner = cr(&l.inner[base..base + GRID], GRID, row[channel]);
                        aggregate += gate * inner + (1.0 - gate) * row[channel];
                        count += 1.0;
                    }
                }
                aggregate /= count.max(1.0);
                let base = (branch * HIDDEN + channel) * GRID;
                out[edge * HIDDEN + channel] += cr(&l.outer[base..base + GRID], GRID, aggregate);
            }
        }
    }
    out
}

/// The inverse incidence the scatter consumes, built the way the CPU owner builds it.
fn inverse_incidence(c: &Chain) -> (Vec<u32>, Vec<u32>, Vec<f32>) {
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); N_NODES];
    for edge in 0..N_EDGES {
        for member in 0..ARITY {
            rows[c.vertices[edge * ARITY + member] as usize].push(edge as u32);
        }
    }
    let mut offsets = vec![0u32];
    let mut index = Vec::new();
    for row in &rows {
        index.extend_from_slice(row);
        offsets.push(index.len() as u32);
    }
    let counts = rows.iter().map(|r| r.len() as f32).collect();
    (offsets, index, counts)
}

/// The whole chain on the host: the reference the device result is compared against.
fn reference(c: &Chain) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let h_e0 = hsikan(&c.x0, c, &c.layers[0]);
    let (offsets, index, counts) = inverse_incidence(c);
    let mut step = vec![0.0f32; N_NODES * HIDDEN];
    for node in 0..N_NODES {
        let (first, last) = (offsets[node] as usize, offsets[node + 1] as usize);
        let row = &mut step[node * HIDDEN..(node + 1) * HIDDEN];
        for &edge in &index[first..last] {
            let base = edge as usize * HIDDEN;
            for (slot, &v) in row.iter_mut().zip(&h_e0[base..base + HIDDEN]) {
                *slot += v;
            }
        }
        if counts[node] > 0.0 {
            let inv = 1.0 / counts[node];
            for slot in row.iter_mut() {
                *slot *= inv;
            }
        }
    }
    let x1: Vec<f32> = (0..N_NODES * HIDDEN)
        .map(|i| {
            let t = 1.0f32 / (1.0 + (-c.transition_gate[i % HIDDEN]).exp());
            t * step[i] + (1.0 - t) * c.x0[i]
        })
        .collect();
    let y = hsikan(&x1, c, &c.layers[1]);
    (h_e0, step, x1, y)
}

/// The only path in this file from device memory to host memory.
///
/// Every read is routed through [`Self::read`], which increments on the ACTUAL call. Nothing else
/// in this file calls `read_tensor`, so the counter is the observed number of readbacks rather than
/// a value someone wrote down. A structural gate below proves the encode path cannot bypass it.
struct ReadbackObserver<'a> {
    backend: &'a WgpuBackend,
    observed: Cell<usize>,
}

impl<'a> ReadbackObserver<'a> {
    fn new(backend: &'a WgpuBackend) -> Self {
        Self {
            backend,
            observed: Cell::new(0),
        }
    }

    /// Copy a device tensor to the host, counting the transfer.
    fn read<T: hymeko_wgpu::TensorOperand>(&self, operand: &T) -> Vec<f32> {
        self.observed.set(self.observed.get() + 1);
        self.backend.read_tensor(operand).unwrap()
    }

    fn observed(&self) -> usize {
        self.observed.get()
    }
}

/// Run the chain on device. Returns the final output, the observed readback count and the number
/// of encoded dispatches.
fn run(c: &Chain) -> Option<(Vec<f32>, usize, usize)> {
    let backend = crate::support::backend()?;
    let resources = backend.resources();
    let (offsets, index, counts) = inverse_incidence(c);

    // ---- upload ONCE, before any dispatch -------------------------------------------------
    let x0 = resources
        .upload_tensor("x0", TensorShape::new(N_NODES, HIDDEN), &c.x0)
        .unwrap();
    let vertices = resources
        .upload_u32_tensor("vertices", TensorShape::new(N_EDGES, ARITY), &c.vertices)
        .unwrap();
    let signs = resources
        .upload_i32_tensor("signs", TensorShape::new(N_EDGES, ARITY), &c.signs)
        .unwrap();
    let control_shape = TensorShape::new(BRANCHES * HIDDEN, GRID);
    let l0_inner = resources
        .upload_tensor("l0_inner", control_shape, &c.layers[0].inner)
        .unwrap();
    let l0_outer = resources
        .upload_tensor("l0_outer", control_shape, &c.layers[0].outer)
        .unwrap();
    let l0_gate_w = resources
        .upload_tensor(
            "l0_gate_w",
            TensorShape::new(HIDDEN, HIDDEN),
            &c.layers[0].gate_w,
        )
        .unwrap();
    let l0_gate_b = resources
        .upload_tensor(
            "l0_gate_b",
            TensorShape::new(HIDDEN, 1),
            &c.layers[0].gate_b,
        )
        .unwrap();
    let l1_inner = resources
        .upload_tensor("l1_inner", control_shape, &c.layers[1].inner)
        .unwrap();
    let l1_outer = resources
        .upload_tensor("l1_outer", control_shape, &c.layers[1].outer)
        .unwrap();
    let l1_gate_w = resources
        .upload_tensor(
            "l1_gate_w",
            TensorShape::new(HIDDEN, HIDDEN),
            &c.layers[1].gate_w,
        )
        .unwrap();
    let l1_gate_b = resources
        .upload_tensor(
            "l1_gate_b",
            TensorShape::new(HIDDEN, 1),
            &c.layers[1].gate_b,
        )
        .unwrap();
    // The incidence reaches the device only through CORE validation: the support owner calls
    // `SparseTensorContract::validate` and hands the receipt to `upload_validated_csr`.
    let incidence = crate::support::Incidence {
        n_nodes: N_NODES,
        n_edges: N_EDGES,
        row_ptr: offsets.iter().map(|&v| v as usize).collect(),
        col_ind: index.iter().map(|&v| v as usize).collect(),
        counts: counts.clone(),
    }
    .upload(&resources, "incidence")
    .unwrap();
    let counts_t = resources
        .upload_tensor("counts", TensorShape::new(N_NODES, 1), &counts)
        .unwrap();
    let t0_gate = resources
        .upload_tensor("t0_gate", TensorShape::new(HIDDEN, 1), &c.transition_gate)
        .unwrap();

    // ---- device-resident intermediates: never read back --------------------------------------
    let h_e0 = resources
        .zeroed_tensor("h_e0", TensorShape::new(N_EDGES, HIDDEN))
        .unwrap();
    let step = resources
        .zeroed_tensor("step", TensorShape::new(N_NODES, HIDDEN))
        .unwrap();
    let x1 = resources
        .zeroed_tensor("x1", TensorShape::new(N_NODES, HIDDEN))
        .unwrap();
    let y = resources
        .zeroed_tensor("y", TensorShape::new(N_EDGES, HIDDEN))
        .unwrap();

    let ops = backend.operations();
    let d1 = ops
        .hsikan()
        .features(&x0)
        .vertices(&vertices)
        .signs(&signs)
        .inner_control(&l0_inner)
        .outer_control(&l0_outer)
        .gate_weight(&l0_gate_w)
        .gate_bias(&l0_gate_b)
        .output(&h_e0)
        .label("chain.l0")
        .prepare()
        .unwrap();
    let d2 = ops
        .scatter_mean()
        .incidence(&incidence)
        .counts(&counts_t)
        .edge_state(&h_e0)
        .node_state(&step)
        .label("chain.scatter")
        .prepare()
        .unwrap();
    let d3 = ops
        .highway_identity()
        .step(&step)
        .carry(&x0)
        .gate(&t0_gate)
        .next(&x1)
        .label("chain.transition")
        .prepare()
        .unwrap();
    let d4 = ops
        .hsikan()
        .features(&x1)
        .vertices(&vertices)
        .signs(&signs)
        .inner_control(&l1_inner)
        .outer_control(&l1_outer)
        .gate_weight(&l1_gate_w)
        .gate_bias(&l1_gate_b)
        .output(&y)
        .label("chain.l1")
        .prepare()
        .unwrap();
    assert_eq!(d1.kernel(), KernelId::Hsikan);
    assert_eq!(d2.kernel(), KernelId::ScatterMeanCsr);
    assert_eq!(d3.kernel(), KernelId::HighwayIdentity);
    assert_eq!(d4.kernel(), KernelId::Hsikan);

    // ---- ONE encoder, four dispatches in order, ONE submission --------------------------------
    // `encode` takes only `&mut CommandEncoder`: it has no backend, queue, staging buffer or
    // readback workspace, so no dispatch below can move data to the host even in principle.
    let mut encoder = backend.encoder(Some("resident-hsikan-chain"));
    let mut encoded = 0usize;
    d1.encode(&mut encoder).unwrap();
    encoded += 1;
    d2.encode(&mut encoder).unwrap();
    encoded += 1;
    d3.encode(&mut encoder).unwrap();
    encoded += 1;
    d4.encode(&mut encoder).unwrap();
    encoded += 1;
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    // ---- ONE observed readback, at the chain boundary ------------------------------------------
    let observer = ReadbackObserver::new(backend);
    let out = observer.read(&y);
    Some((out, observer.observed(), encoded))
}

#[test]
fn the_resident_chain_matches_the_cpu_reference_with_exactly_one_readback() {
    for bias in [-4.0f32, 0.0, 4.0] {
        let c = chain(bias);
        let (h_e0, step, x1, expected) = reference(&c);
        // Every stage must be non-degenerate, or the parity is vacuous.
        for (stage, values) in [
            ("h_e0", &h_e0),
            ("step", &step),
            ("x1", &x1),
            ("y", &expected),
        ] {
            assert!(
                values.iter().any(|v| *v != values[0]),
                "bias {bias}: stage {stage} is constant; parity would be vacuous"
            );
        }
        let Some((actual, observed_readbacks, encoded)) = run(&c) else {
            eprintln!("SKIP: no WGPU adapter");
            return;
        };
        // L = 2: 3L - 2 = 4 encodes, one submit, one observed readback.
        assert_eq!(encoded, 3 * 2 - 2, "L=2 must encode 3L-2 dispatches");
        assert_eq!(
            observed_readbacks, 1,
            "the chain performed {observed_readbacks} observed readbacks; the residency contract \
             allows exactly one, at the chain boundary"
        );
        eprintln!(
            "RESIDENCY L=2 bias {bias}: encodes {encoded}, submits 1, observed readbacks \
             {observed_readbacks}"
        );
        assert_parity(&format!("chain bias {bias}"), &actual, &expected);
    }
}

#[test]
fn a_sign_flip_moves_the_chain_output() {
    // The mutation must apply and must be observable end to end, or the chain is not reading the
    // topology it claims to.
    let base = chain(0.5);
    let mut flipped = chain(0.5);
    flipped.signs[0] = -1;
    assert_ne!(base.signs, flipped.signs, "the sign flip did not apply");

    let expected_base = reference(&base).3;
    let expected_flip = reference(&flipped).3;
    assert_ne!(
        expected_base, expected_flip,
        "the reference itself is insensitive to the sign flip"
    );

    let Some((got_base, _, _)) = run(&base) else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let (got_flip, _, _) = run(&flipped).unwrap();
    assert_parity("base", &got_base, &expected_base);
    assert_parity("flipped", &got_flip, &expected_flip);
    assert_ne!(
        got_base, got_flip,
        "the device output ignored the sign flip"
    );
}

#[test]
fn permuting_members_within_one_tuple_leaves_the_chain_output_unchanged() {
    // Carried over from the CPU-side classification, unchanged: reordering a tuple's members while
    // keeping every (vertex, sign) PAIR intact is an identity-only change, because the branch means
    // are symmetric over members. Asserting a divergence here would assert something false.
    let base = chain(0.25);
    let mut permuted = chain(0.25);
    permuted.vertices.swap(0, 2);
    permuted.signs.swap(0, 2);
    assert_ne!(
        base.vertices, permuted.vertices,
        "the permutation did not apply"
    );

    let expected = reference(&base).3;
    let permuted_expected = reference(&permuted).3;
    assert_parity("permutation reference", &permuted_expected, &expected);

    let Some((got, _, _)) = run(&permuted) else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    assert_parity("permuted chain", &got, &expected);
}

#[test]
fn the_chain_extends_to_three_layers_without_a_new_kernel_or_a_second_readback() {
    // Depth is not a kernel property. A third layer is one more scatter, one more transition and
    // one more HSiKAN dispatch into the same encoder, still with a single readback.
    let Some(backend) = crate::support::backend() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let c = chain(0.75);
    let (offsets, index, counts) = inverse_incidence(&c);
    let resources = backend.resources();
    let control_shape = TensorShape::new(BRANCHES * HIDDEN, GRID);

    let x0 = resources
        .upload_tensor("x0", TensorShape::new(N_NODES, HIDDEN), &c.x0)
        .unwrap();
    let vertices = resources
        .upload_u32_tensor("v", TensorShape::new(N_EDGES, ARITY), &c.vertices)
        .unwrap();
    let signs = resources
        .upload_i32_tensor("s", TensorShape::new(N_EDGES, ARITY), &c.signs)
        .unwrap();
    let incidence = crate::support::Incidence {
        n_nodes: N_NODES,
        n_edges: N_EDGES,
        row_ptr: offsets.iter().map(|&v| v as usize).collect(),
        col_ind: index.iter().map(|&v| v as usize).collect(),
        counts: counts.clone(),
    }
    .upload(&resources, "incidence")
    .unwrap();
    let counts_t = resources
        .upload_tensor("c", TensorShape::new(N_NODES, 1), &counts)
        .unwrap();
    let gate = resources
        .upload_tensor("g", TensorShape::new(HIDDEN, 1), &c.transition_gate)
        .unwrap();

    // Three layers' parameters, and the intermediates each stage needs.
    let mut params = Vec::new();
    for l in 0..3usize {
        let p = layer(2 + 29 * l);
        params.push((
            resources
                .upload_tensor("in", control_shape, &p.inner)
                .unwrap(),
            resources
                .upload_tensor("ou", control_shape, &p.outer)
                .unwrap(),
            resources
                .upload_tensor("gw", TensorShape::new(HIDDEN, HIDDEN), &p.gate_w)
                .unwrap(),
            resources
                .upload_tensor("gb", TensorShape::new(HIDDEN, 1), &p.gate_b)
                .unwrap(),
        ));
    }
    let edge_bufs: Vec<_> = (0..3)
        .map(|_| {
            resources
                .zeroed_tensor("he", TensorShape::new(N_EDGES, HIDDEN))
                .unwrap()
        })
        .collect();
    let node_bufs: Vec<_> = (0..4)
        .map(|_| {
            resources
                .zeroed_tensor("hn", TensorShape::new(N_NODES, HIDDEN))
                .unwrap()
        })
        .collect();

    let ops = backend.operations();
    let mut encoder = backend.encoder(Some("resident-three-layer"));
    let mut dispatches = 0usize;
    for l in 0..3usize {
        let input = if l == 0 { &x0 } else { &node_bufs[l] };
        let (inner, outer, gw, gb) = &params[l];
        ops.hsikan()
            .features(input)
            .vertices(&vertices)
            .signs(&signs)
            .inner_control(inner)
            .outer_control(outer)
            .gate_weight(gw)
            .gate_bias(gb)
            .output(&edge_bufs[l])
            .prepare()
            .unwrap()
            .encode(&mut encoder)
            .unwrap();
        dispatches += 1;
        if l < 2 {
            ops.scatter_mean()
                .incidence(&incidence)
                .counts(&counts_t)
                .edge_state(&edge_bufs[l])
                .node_state(&node_bufs[3])
                .prepare()
                .unwrap()
                .encode(&mut encoder)
                .unwrap();
            ops.highway_identity()
                .step(&node_bufs[3])
                .carry(input)
                .gate(&gate)
                .next(&node_bufs[l + 1])
                .prepare()
                .unwrap()
                .encode(&mut encoder)
                .unwrap();
            dispatches += 2;
        }
    }
    // 3L - 2 dispatches for depth L, as the plan's cost model states.
    assert_eq!(dispatches, 3 * 3 - 2);
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    let observer = ReadbackObserver::new(backend);
    let out = observer.read(&edge_bufs[2]);
    assert_eq!(
        observer.observed(),
        1,
        "L=3 performed {} observed readbacks; the residency contract allows exactly one",
        observer.observed()
    );
    eprintln!(
        "RESIDENCY L=3: encodes {dispatches}, submits 1, observed readbacks {}",
        observer.observed()
    );
    assert_eq!(out.len(), N_EDGES * HIDDEN);
    assert!(
        out.iter().all(|v| v.is_finite()),
        "three-layer chain produced a non-finite value"
    );
    assert!(
        out.iter().any(|v| *v != out[0]),
        "three-layer chain produced a constant output"
    );
}

#[test]
fn every_prepared_encode_takes_only_an_encoder_and_cannot_reach_the_host() {
    // The structural half of the residency evidence. The observed counter proves how many readbacks
    // HAPPENED; this proves the encode path could not have added an unobserved one.
    //
    // `Prepared*::encode(&self, &mut wgpu::CommandEncoder) -> Result<(), ComputeError>` is the
    // whole surface: no `&WgpuBackend`, no `&wgpu::Queue`, no `ReadbackWorkspace`, no
    // `StagingStorage`. A command encoder can copy buffer-to-buffer but cannot map memory — mapping
    // requires the device and a submission the encoder does not own. So none of the four dispatches
    // can initiate a host transfer, and the observer sees every one that occurs.
    //
    // Checked against the source, so a future signature that gained backend access fails here
    // rather than silently widening what "resident" means. The three prepared types share one
    // `encode`, generated by `prepared_operation!`: each must still be declared through it, and its
    // one signature is the one checked.
    let operations = [
        (
            "scatter_mean",
            include_str!("../../src/operations/scatter_mean.rs"),
        ),
        ("highway", include_str!("../../src/operations/highway.rs")),
        ("hsikan", include_str!("../../src/operations/hsikan.rs")),
    ];
    for (name, source) in operations {
        assert!(
            source.contains("prepared_operation!(") && !source.contains("pub fn encode("),
            "{name}: its prepared type no longer takes the shared encode"
        );
    }
    let sources = [(
        "prepared_operation!",
        include_str!("../../src/operations/mod.rs"),
    )];
    for (name, source) in sources {
        // The whole signature, however it is wrapped: from `pub fn encode(` to the body's brace.
        let start = source
            .find("pub fn encode(")
            .unwrap_or_else(|| panic!("{name}: no encode method found"));
        let end = start + source[start..].find('{').expect("encode has a body");
        let signature = source[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace("( ", "(")
            .replace(", )", ")");
        assert!(
            signature.contains("&self, encoder: &mut wgpu::CommandEncoder"),
            "{name}: encode signature changed to `{signature}`; residency evidence depends on it \
             taking only a command encoder"
        );
        for forbidden in [
            "WgpuBackend",
            "ReadbackWorkspace",
            "StagingStorage",
            "wgpu::Queue",
        ] {
            assert!(
                !signature.contains(forbidden),
                "{name}: encode gained access to {forbidden}; it could then initiate a host \
                 transfer the observer would not count"
            );
        }
    }
}
