//! Phase 2 correctness: WGPU CSR SpMM against the CPU reference.
//!
//! # Reference and tolerance
//!
//! The oracle is `TensorCsr::<f32>::spmm_into_serial` — the same function the
//! CPU backend's own bitwise tier is built on.
//!
//! **Bitwise equality between CPU and GPU is not promised and is not asserted.**
//! The kernel accumulates in exactly the CPU's order (see `spmm.wgsl`), so no
//! reordering occurs, but the GPU may contract `acc + v*x` into a fused
//! multiply-add and round once where the CPU rounds twice. Comparison is
//! therefore absolute **and** relative, with NaN and Inf handled explicitly
//! rather than swallowed by a tolerance.
//!
//! # Skip policy
//!
//! Only a missing adapter is a skip. A shader that fails to compile, a
//! validation error, a mapping failure or a numerical mismatch is a defect.

use hymeko_tensor::compute::{ComputeError, SparseCsrShape, TensorShape};
use hymeko_tensor::representations::tensor_csr::TensorCsr;
use hymeko_wgpu::testing::{DegreeShape, SparseSpec};
use hymeko_wgpu::testing::{backend_or_skip, shared_backend};
use hymeko_wgpu::{GpuCsrMatrix, GpuTensor, PreparedSpmm, WgpuBackend, WorkgroupShape};

const POISON: f32 = -12345.5;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// CPU oracle.
fn cpu_reference(csr: &TensorCsr<f32>, x: &[f32], fd: usize) -> Vec<f32> {
    let mut y = vec![POISON; csr.num_rows * fd];
    csr.spmm_into_serial(x, fd, &mut y);
    y
}

/// Compare GPU against CPU with absolute + relative tolerance.
///
/// NaN and Inf are compared structurally: a NaN must stay a NaN, an infinity
/// must keep its sign. Only finite values go through the tolerance test.
fn assert_close(gpu: &[f32], cpu: &[f32], ctx: &str) {
    const ABS: f32 = 1e-5;
    const REL: f32 = 1e-5;

    assert_eq!(gpu.len(), cpu.len(), "{ctx}: length mismatch");

    let mut worst_abs = 0.0f32;
    let mut worst_rel = 0.0f32;
    let mut worst_at = usize::MAX;

    for i in 0..cpu.len() {
        let (g, c) = (gpu[i], cpu[i]);

        if c.is_nan() || g.is_nan() {
            assert!(
                c.is_nan() && g.is_nan(),
                "{ctx}: NaN disagreement at {i}: gpu={g} cpu={c}"
            );
            continue;
        }
        if c.is_infinite() || g.is_infinite() {
            assert!(
                c.is_infinite() && g.is_infinite() && c.signum() == g.signum(),
                "{ctx}: infinity disagreement at {i}: gpu={g} cpu={c}"
            );
            continue;
        }

        let abs = (g - c).abs();
        let rel = if c.abs() > 0.0 { abs / c.abs() } else { abs };
        if abs > worst_abs {
            worst_abs = abs;
            worst_at = i;
        }
        worst_rel = worst_rel.max(rel);

        assert!(
            abs <= ABS || rel <= REL,
            "{ctx}: mismatch at {i}: gpu={g} cpu={c} abs={abs:e} rel={rel:e}"
        );
    }

    if worst_at != usize::MAX {
        eprintln!(
            "[{ctx}] worst abs={worst_abs:e} rel={worst_rel:e} (n={})",
            cpu.len()
        );
    }
}

/// Upload, prepare, dispatch, read back — the full explicit pipeline.
fn run_gpu(
    backend: &WgpuBackend,
    csr: &TensorCsr<f32>,
    x: &[f32],
    fd: usize,
    workgroup: WorkgroupShape,
) -> Vec<f32> {
    let res = backend.resources();
    let matrix = res.upload_csr_from("a", csr).expect("csr upload");
    let shape = matrix.shape();

    let input = res
        .upload_tensor("x", shape.input_shape(fd), x)
        .expect("x upload");
    // Poisoned on purpose: the kernel must fully overwrite Y, and `zeroed_tensor`
    // would hide a kernel that never writes.
    let poison = vec![POISON; shape.rows * fd];
    let output: GpuTensor<f32> = res
        .upload_tensor("y", shape.output_shape(fd), &poison)
        .expect("y alloc");

    let op = prepare_spmm(backend, &matrix, &input, &output, workgroup);

    let mut encoder = backend.encoder(Some("spmm"));
    op.encode(&mut encoder).expect("encode");
    let index = backend.submit(encoder);
    backend.wait(index).expect("submit");

    backend.read_tensor(&output).expect("readback")
}

fn prepare_spmm<'a>(
    backend: &'a WgpuBackend,
    matrix: &'a GpuCsrMatrix<f32>,
    input: &'a GpuTensor<f32>,
    output: &'a GpuTensor<f32>,
    workgroup: WorkgroupShape,
) -> PreparedSpmm<'a> {
    backend
        .operations()
        .spmm()
        .matrix(matrix)
        .input(input)
        .output(output)
        .workgroup(workgroup)
        .prepare()
        .expect("prepare")
}

/// Hand-built matrix: empty row (2), duplicate column in row 3, negatives.
fn handmade() -> TensorCsr<f32> {
    TensorCsr {
        num_rows: 4,
        num_cols: 5,
        //            r0    r1       r2(empty) r3
        row_ptr: vec![0, 2, 5, 5, 8],
        col_ind: vec![0, 3, 1, 2, 4, 4, 0, 4],
        val: vec![1.5, -2.0, 0.25, 3.0, 1.0, 2.0, -1.0, 0.5],
    }
}

// ---------------------------------------------------------------------------
// 1-9, 18: numerical agreement
// ---------------------------------------------------------------------------

#[test]
fn reusable_readback_waits_for_changing_resident_compute() {
    let Some(backend) = shared_backend("spmm_single_wait") else {
        return;
    };
    let csr = handmade();
    let fd = 8;
    let matrix = backend.resources().upload_csr_from("a", &csr).unwrap();
    let input = backend
        .resources()
        .zeroed_tensor("x", matrix.shape().input_shape(fd))
        .unwrap();
    let output = backend
        .resources()
        .zeroed_tensor("y", matrix.shape().output_shape(fd))
        .unwrap();
    let op = prepare_spmm(backend, &matrix, &input, &output, WorkgroupShape::default());
    let mut workspace = backend.create_readback_workspace();
    for round in 0..8 {
        let x: Vec<_> = (0..csr.num_cols * fd)
            .map(|i| (i as f32 - 9.0) * (round + 1) as f32)
            .collect();
        backend.resources().write_tensor(&input, &x).unwrap();
        let mut encoder = backend.encoder(None);
        op.encode(&mut encoder).unwrap();
        backend.submit(encoder);
        // No compute wait: readback completion must cover both ordered submissions.
        let got = backend.read_tensor_into(&output, &mut workspace).unwrap();
        assert_close(got, &cpu_reference(&csr, &x, fd), "single queue wait");
        assert_eq!(workspace.staging_allocations(), 1);
    }
}

#[test]
fn nonfinite_inputs_preserve_cpu_classification() {
    let Some(backend) = shared_backend("spmm_nonfinite") else {
        return;
    };
    let csr = handmade();
    for special in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut x = vec![1.0; csr.num_cols];
        x[0] = special;
        let got = run_gpu(backend, &csr, &x, 1, WorkgroupShape::default());
        assert_close(&got, &cpu_reference(&csr, &x, 1), "nonfinite input");
    }
}

#[test]
fn test_handmade_matrix_matches_cpu_across_feature_dims() {
    // Covers: empty CSR row (3), duplicate column index (5), negative values
    // (6), and feature_dim 1/8/32/128 (8).
    let Some(backend) = shared_backend("handmade") else {
        return;
    };
    let csr = handmade();

    for fd in [1usize, 8, 32, 128] {
        let x: Vec<f32> = (0..csr.num_cols * fd)
            .map(|t| 0.5 + (t % 13) as f32 * 0.125)
            .collect();
        let cpu = cpu_reference(&csr, &x, fd);
        let gpu = run_gpu(backend, &csr, &x, fd, WorkgroupShape::default());
        assert_close(&gpu, &cpu, &format!("handmade fd={fd}"));

        // Row 2 is structurally empty -> must be exactly zero, not poison.
        for f in 0..fd {
            assert_eq!(gpu[2 * fd + f], 0.0, "empty row must produce 0 at fd={fd}");
        }
    }
}

#[test]
fn test_empty_matrix_and_output() {
    // Covers: empty matrix/output (1) and the zero-extent dispatch rule.
    let Some(backend) = shared_backend("empty") else {
        return;
    };
    let res = backend.resources();

    let csr: TensorCsr<f32> = TensorCsr {
        num_rows: 0,
        num_cols: 0,
        row_ptr: vec![0],
        col_ind: Vec::new(),
        val: Vec::new(),
    };
    let matrix = res.upload_csr_from("empty", &csr).unwrap();
    let input = res.upload_tensor("x", TensorShape::new(0, 4), &[]).unwrap();
    let output = res.zeroed_tensor("y", TensorShape::new(0, 4)).unwrap();

    let op = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&output)
        .prepare()
        .expect("an empty operation must still prepare successfully");

    // No dispatch may be issued: a zero extent is invalid in WGPU.
    assert_eq!(
        op.dispatch_groups(),
        (0, 0),
        "empty operation must not dispatch"
    );

    let mut encoder = backend.encoder(Some("empty"));
    op.encode(&mut encoder).expect("encode must be a no-op");
    let index = backend.submit(encoder);
    backend.wait(index).unwrap();

    assert!(backend.read_tensor(&output).unwrap().is_empty());
}

#[test]
fn test_zero_feature_dim_is_a_valid_noop() {
    let Some(backend) = shared_backend("fd0") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let input = res
        .upload_tensor("x", TensorShape::new(csr.num_cols, 0), &[])
        .unwrap();
    let output = res
        .zeroed_tensor("y", TensorShape::new(csr.num_rows, 0))
        .unwrap();

    let op = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&output)
        .prepare()
        .expect("feature_dim = 0 must prepare");
    assert_eq!(op.dispatch_groups(), (0, 0));

    let mut encoder = backend.encoder(None);
    op.encode(&mut encoder).unwrap();
    backend.wait(backend.submit(encoder)).unwrap();
    assert!(backend.read_tensor(&output).unwrap().is_empty());
}

#[test]
fn test_singleton() {
    let Some(backend) = shared_backend("singleton") else {
        return;
    };
    let csr = TensorCsr {
        num_rows: 1,
        num_cols: 1,
        row_ptr: vec![0, 1],
        col_ind: vec![0],
        val: vec![2.5f32],
    };
    let x = vec![4.0f32];
    let cpu = cpu_reference(&csr, &x, 1);
    let gpu = run_gpu(backend, &csr, &x, 1, WorkgroupShape::default());
    assert_close(&gpu, &cpu, "singleton");
    assert_eq!(gpu[0], 10.0);
}

#[test]
fn test_signed_zero_is_preserved_structurally() {
    // Covers: +0.0 / -0.0 (7).
    //
    // The CPU starts each accumulator at +0.0 and adds, so `0.0 + (-0.0)` is
    // +0.0 there too. The GPU does the same. What is asserted is that both
    // agree, and that a genuinely negative result stays negative -- not that a
    // -0.0 input survives as -0.0, which the algorithm does not promise.
    let Some(backend) = shared_backend("signed_zero") else {
        return;
    };
    // X is [2 x 2] row-major: x[0,:] = [0.0, -0.0], x[1,:] = [5.0, -5.0].
    //   row 0 -> col 0, val -0.0  =>  -0.0 * 0.0  and  -0.0 * -0.0
    //   row 1 -> col 1, val  0.0  =>   0.0 * 5.0  and   0.0 * -5.0
    //   row 2 -> col 1, val -1.0  =>  -1.0 * 5.0 = -5.0  (a real negative)
    let csr = TensorCsr {
        num_rows: 3,
        num_cols: 2,
        row_ptr: vec![0, 1, 2, 3],
        col_ind: vec![0, 1, 1],
        val: vec![-0.0f32, 0.0, -1.0],
    };
    let x = vec![0.0f32, -0.0, 5.0, -5.0];
    let fd = 2;
    let cpu = cpu_reference(&csr, &x, fd);
    let gpu = run_gpu(backend, &csr, &x, fd, WorkgroupShape::default());

    assert_close(&gpu, &cpu, "signed zero");
    for i in 0..cpu.len() {
        assert_eq!(
            gpu[i].is_sign_negative() && gpu[i] == 0.0,
            cpu[i].is_sign_negative() && cpu[i] == 0.0,
            "signed-zero disagreement at {i}: gpu={} cpu={}",
            gpu[i],
            cpu[i]
        );
    }
    // Row 2 is -1.0 * 5.0 = -5.0: a real negative, not a signed zero.
    assert_eq!(gpu[2 * fd], -5.0);
}

#[test]
fn test_irregular_degree_and_non_multiple_shapes() {
    // Covers: irregular row degree (4) and shapes not divisible by the
    // workgroup extent (9). 1000 rows and fd=33 are coprime with 8/16.
    let Some(backend) = shared_backend("irregular") else {
        return;
    };
    let spec = SparseSpec {
        rows: 1000,
        cols: 997,
        avg_nnz_per_row: 8,
        shape: DegreeShape::Irregular,
        seed: 0xC0FFEE,
    };
    let csr = spec.build();

    for fd in [1usize, 33] {
        let x: Vec<f32> = (0..csr.num_cols * fd)
            .map(|t| 0.25 + (t % 29) as f32 * 0.0625)
            .collect();
        let cpu = cpu_reference(&csr, &x, fd);
        for wg in WorkgroupShape::sweep() {
            let gpu = run_gpu(backend, &csr, &x, fd, wg);
            assert_close(&gpu, &cpu, &format!("irregular fd={fd} wg={wg}"));
        }
    }
}

#[test]
fn test_deterministic_medium_and_large_workloads() {
    // Covers: deterministic medium and large workloads (18).
    let Some(backend) = shared_backend("scaled") else {
        return;
    };

    for (label, spec, fd) in [
        ("medium", SparseSpec::medium(), 8usize),
        ("large", SparseSpec::large(), 8usize),
    ] {
        let csr = spec.build();
        let x = spec.dense_block(fd);
        let cpu = cpu_reference(&csr, &x, fd);
        let gpu = run_gpu(backend, &csr, &x, fd, WorkgroupShape::default());
        assert_close(&gpu, &cpu, &format!("{label} fd={fd}"));
        eprintln!(
            "[{label}] rows={} nnz={} fd={fd} -> {} elements agreed",
            csr.num_rows,
            csr.nnz(),
            cpu.len()
        );
    }
}

// ---------------------------------------------------------------------------
// 10-12: validation
// ---------------------------------------------------------------------------

#[test]
fn test_shape_and_metadata_validation_at_prepare_time() {
    // Covers: input shape mismatch (10), output shape mismatch (11), bad CSR
    // metadata (12). All must be caught in `prepare`, never at dispatch.
    let Some(backend) = shared_backend("validation") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let fd = 4;

    let good_x = res
        .upload_tensor("x", TensorShape::new(5, fd), &[1.0; 20])
        .unwrap();
    let good_y = res.zeroed_tensor("y", TensorShape::new(4, fd)).unwrap();

    // (10) X rows must equal matrix columns.
    let bad_x = res
        .upload_tensor("bx", TensorShape::new(3, fd), &[1.0; 12])
        .unwrap();
    assert!(matches!(
        backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&bad_x)
            .output(&good_y)
            .prepare(),
        Err(ComputeError::ShapeMismatch {
            expected: 5,
            actual: 3,
            ..
        })
    ));

    // (11) Y rows must equal matrix rows.
    let bad_y = res.zeroed_tensor("by", TensorShape::new(7, fd)).unwrap();
    assert!(matches!(
        backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&good_x)
            .output(&bad_y)
            .prepare(),
        Err(ComputeError::ShapeMismatch {
            expected: 4,
            actual: 7,
            ..
        })
    ));

    // feature_dim must agree between X and Y.
    let mismatched_fd = res.zeroed_tensor("fy", TensorShape::new(4, 9)).unwrap();
    assert!(matches!(
        backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&good_x)
            .output(&mismatched_fd)
            .prepare(),
        Err(ComputeError::ShapeMismatch { .. })
    ));

    // Missing operands are rejected, not silently defaulted.
    assert!(
        backend
            .operations()
            .spmm()
            .input(&good_x)
            .output(&good_y)
            .prepare()
            .is_err()
    );
    assert!(
        backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .output(&good_y)
            .prepare()
            .is_err()
    );
    assert!(
        backend
            .operations()
            .spmm()
            .matrix(&matrix)
            .input(&good_x)
            .prepare()
            .is_err()
    );

    // Aliasing X and Y would be a read/read_write hazard on one buffer.
    //
    // This needs a SQUARE matrix: with the 4x5 `handmade` matrix, X must have 5
    // rows and Y must have 4, so one tensor can never legally be both and the
    // shape check would fire first. The aliasing rule is only reachable when the
    // shapes are otherwise valid.
    let square = TensorCsr {
        num_rows: 3,
        num_cols: 3,
        row_ptr: vec![0, 1, 2, 3],
        col_ind: vec![0, 1, 2],
        val: vec![1.0f32, 2.0, 3.0],
    };
    let square_gpu = res.upload_csr_from("sq", &square).unwrap();
    let shared = res
        .upload_tensor("shared", TensorShape::new(3, fd), &[1.0; 12])
        .unwrap();
    let err = backend
        .operations()
        .spmm()
        .matrix(&square_gpu)
        .input(&shared)
        .output(&shared)
        .prepare()
        .expect_err("aliasing input and output must be rejected");
    assert!(
        format!("{err}").contains("alias"),
        "expected an aliasing error, got: {err}"
    );

    // (12) Bad CSR metadata is rejected at upload, before any operation exists.
    assert!(matches!(
        res.upload_csr(
            "bad",
            SparseCsrShape::new(4, 5, 8),
            &[0, 2],
            &csr.col_ind,
            &csr.val
        ),
        Err(ComputeError::ShapeMismatch {
            operand: "csr row_ptr",
            ..
        })
    ));
    assert!(matches!(
        res.upload_csr(
            "bad",
            SparseCsrShape::new(4, 5, 99),
            &csr.row_ptr,
            &csr.col_ind,
            &csr.val
        ),
        Err(ComputeError::ShapeMismatch {
            operand: "csr col_ind",
            ..
        })
    ));
}

#[test]
fn test_oversized_workgroup_is_rejected() {
    let Some(backend) = shared_backend("workgroup_limit") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let x = res
        .upload_tensor("x", TensorShape::new(5, 2), &[1.0; 10])
        .unwrap();
    let y = res.zeroed_tensor("y", TensorShape::new(4, 2)).unwrap();

    let limit = backend.capabilities().max_compute_invocations_per_workgroup;
    let too_big = WorkgroupShape::new(limit, 2); // 2x the limit

    let err = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&x)
        .output(&y)
        .workgroup(too_big)
        .prepare()
        .unwrap_err();
    assert!(
        format!("{err}").contains("invocations"),
        "expected an invocation-limit error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 13-17: reuse, batching, cache
// ---------------------------------------------------------------------------

#[test]
fn test_repeated_dispatch_and_input_rewrite() {
    // Covers: multiple dispatch with one PreparedSpmm (13), input rewrite then
    // new dispatch (14), poisoned output overwrite (17).
    let Some(backend) = shared_backend("reuse") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let fd = 4;
    let matrix = res.upload_csr_from("a", &csr).unwrap();

    let x1: Vec<f32> = (0..csr.num_cols * fd)
        .map(|t| 1.0 + t as f32 * 0.5)
        .collect();
    let input = res
        .upload_tensor("x", TensorShape::new(csr.num_cols, fd), &x1)
        .unwrap();
    let poison = vec![POISON; csr.num_rows * fd];
    let output = res
        .upload_tensor("y", TensorShape::new(csr.num_rows, fd), &poison)
        .unwrap();

    let op = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&input)
        .output(&output)
        .prepare()
        .unwrap();

    // (17) first dispatch must overwrite every poisoned element.
    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    let first = backend.read_tensor(&output).unwrap();
    assert_close(&first, &cpu_reference(&csr, &x1, fd), "run 1");
    assert!(first.iter().all(|v| *v != POISON), "poison survived");

    // (13) re-dispatching the SAME prepared operation is idempotent.
    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    let second = backend.read_tensor(&output).unwrap();
    assert_eq!(first, second, "re-dispatch changed the result");

    // (14) rewrite X in place, then dispatch the same operation again.
    let x2: Vec<f32> = x1.iter().map(|v| v * -2.0).collect();
    res.write_tensor(&input, &x2).unwrap();
    let mut enc = backend.encoder(None);
    op.encode(&mut enc).unwrap();
    backend.wait(backend.submit(enc)).unwrap();
    let third = backend.read_tensor(&output).unwrap();
    assert_close(&third, &cpu_reference(&csr, &x2, fd), "after input rewrite");
    assert_ne!(first, third, "rewriting the input should change the result");
}

#[test]
fn test_two_operations_one_encoder_one_submit() {
    // Covers: two separate SpMM in one encoder and one submit (15).
    let Some(backend) = shared_backend("batched") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let fd = 4;
    let matrix = res.upload_csr_from("a", &csr).unwrap();

    let xa: Vec<f32> = (0..csr.num_cols * fd)
        .map(|t| 1.0 + t as f32 * 0.25)
        .collect();
    let xb: Vec<f32> = (0..csr.num_cols * fd)
        .map(|t| 3.0 - t as f32 * 0.125)
        .collect();

    let in_a = res
        .upload_tensor("xa", TensorShape::new(csr.num_cols, fd), &xa)
        .unwrap();
    let in_b = res
        .upload_tensor("xb", TensorShape::new(csr.num_cols, fd), &xb)
        .unwrap();
    let out_a = res
        .zeroed_tensor("ya", TensorShape::new(csr.num_rows, fd))
        .unwrap();
    let out_b = res
        .zeroed_tensor("yb", TensorShape::new(csr.num_rows, fd))
        .unwrap();

    let op_a = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&in_a)
        .output(&out_a)
        .prepare()
        .unwrap();
    let op_b = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&in_b)
        .output(&out_b)
        .prepare()
        .unwrap();

    // Both encode into ONE encoder; ONE submit executes both.
    let mut encoder = backend.encoder(Some("two-spmm"));
    op_a.encode(&mut encoder).unwrap();
    op_b.encode(&mut encoder).unwrap();
    let index = backend.submit(encoder);
    backend.wait(index).unwrap();

    assert_close(
        &backend.read_tensor(&out_a).unwrap(),
        &cpu_reference(&csr, &xa, fd),
        "batched a",
    );
    assert_close(
        &backend.read_tensor(&out_b).unwrap(),
        &cpu_reference(&csr, &xb, fd),
        "batched b",
    );
}

#[test]
fn test_pipeline_is_cached_across_prepares() {
    // Covers: pipeline-cache reuse (16).
    // FRESH device: this test asserts an EMPTY pipeline cache, which a shared
    // backend would not have. Sharing here would make it pass for the wrong reason.
    let Some(backend) = backend_or_skip("cache") else {
        return;
    };
    let res = backend.resources();
    let csr = handmade();
    let fd = 4;
    let matrix = res.upload_csr_from("a", &csr).unwrap();
    let x = res
        .upload_tensor("x", TensorShape::new(5, fd), &[1.0; 20])
        .unwrap();
    let y1 = res.zeroed_tensor("y1", TensorShape::new(4, fd)).unwrap();
    let y2 = res.zeroed_tensor("y2", TensorShape::new(4, fd)).unwrap();

    assert!(backend.pipelines().is_empty());

    let wg = WorkgroupShape::new(16, 8);
    let _a = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&x)
        .output(&y1)
        .workgroup(wg)
        .prepare()
        .unwrap();
    assert_eq!(backend.pipelines().len(), 1);

    // A second prepare with the same workgroup must reuse the pipeline, even
    // though the buffers differ -- buffer identity is not in the cache key.
    let _b = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&x)
        .output(&y2)
        .workgroup(wg)
        .prepare()
        .unwrap();
    assert_eq!(
        backend.pipelines().len(),
        1,
        "different buffers must not create a second pipeline"
    );

    // A different workgroup IS a different module and must add an entry.
    let _c = backend
        .operations()
        .spmm()
        .matrix(&matrix)
        .input(&x)
        .output(&y1)
        .workgroup(WorkgroupShape::new(8, 8))
        .prepare()
        .unwrap();
    assert_eq!(backend.pipelines().len(), 2);
}

#[test]
fn test_all_workgroup_variants_agree_with_cpu() {
    // The sweep configurations must be numerically interchangeable; only their
    // speed may differ.
    let Some(backend) = shared_backend("workgroup_sweep") else {
        return;
    };
    let spec = SparseSpec::small();
    let csr = spec.build();
    let fd = 32;
    let x = spec.dense_block(fd);
    let cpu = cpu_reference(&csr, &x, fd);

    for wg in WorkgroupShape::sweep() {
        let gpu = run_gpu(backend, &csr, &x, fd, wg);
        assert_close(&gpu, &cpu, &format!("workgroup {wg}"));
    }
}

#[test]
fn the_upload_signature_takes_one_bound_proof_and_not_a_tensor_receipt_pair() {
    // The API-level gate for the same property, checked against the adapter's source: a two-argument
    // form would re-open the replay even though the types above are sound. If someone adds
    // `tensor:` and `receipt:` parameters back, this fails.
    let source = include_str!("../src/resources.rs");
    let start = source
        .find("pub fn upload_validated_csr")
        .expect("upload_validated_csr exists");
    let signature = &source[start..start + source[start..].find(')').unwrap_or(0) + 1];
    assert!(
        signature.contains("validated: &ValidatedSparse<'_, TensorCsr<f32>>"),
        "upload_validated_csr no longer takes a single bound proof: `{signature}`"
    );
    assert!(
        !signature.contains("receipt:"),
        "a detachable receipt parameter reappeared: `{signature}`"
    );
}

#[test]
fn the_canonical_validator_is_the_only_structural_implementation() {
    // Source gate. The device adapter must translate Tensor's verdict, never re-derive it. If any of
    // these rule phrases reappears as a check in `resources.rs`, two authorities exist again.
    let adapter = include_str!("../src/resources.rs");
    assert!(
        adapter.contains("validate_csr_parts("),
        "the adapter stopped delegating to the canonical validator"
    );
    // The phrases may appear in the TRANSLATION table (mapping Tensor's verdict to operand strings),
    // but never beside a comparison operator that would make them a check.
    for forbidden in [
        "if row_ptr[0] != 0",
        "for pair in row_ptr.windows(2)",
        "row_ptr[shape.rows]",
        "col_ind.iter().find(",
    ] {
        assert!(
            !adapter.contains(forbidden),
            "the adapter re-implements the structural check `{forbidden}`"
        );
    }
}
