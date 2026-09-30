//! Gates for the edge-to-node mean scatter.
//!
//! The reference is a transcription of the CPU owner's arithmetic — accumulate in CSR order, then
//! multiply by the reciprocal `1.0 / count` — not a re-derivation of what a mean "should" be. The
//! reciprocal multiply is deliberate: a division here would be a different float operation.

use hymeko_wgpu::{KernelId, TensorShape};

use hymeko_tensor::representations::sparse::SparseTensorContract;
use hymeko_tensor::representations::tensor_csr::TensorCsr;

use crate::support::{assert_parity, series as raw_series};

/// This suite's value series, at the scale its fixtures were written for.
fn series(len: usize, phase: usize) -> Vec<f32> {
    raw_series(len, phase, 0.6)
}

/// One inverse-incidence CSR plus the states it pools.
struct Csr {
    n_nodes: usize,
    n_edges: usize,
    hidden: usize,
    row_offsets: Vec<u32>,
    edge_index: Vec<u32>,
    counts: Vec<f32>,
    edge_state: Vec<f32>,
}

impl Csr {
    /// Build the inverse incidence from an edge -> member-vertex listing, the way the CPU owner
    /// does: a vertex listed twice in one edge contributes twice, because the denominator counts
    /// incidences and not distinct edges.
    fn from_edges(
        n_nodes: usize,
        hidden: usize,
        edges: &[Vec<usize>],
        edge_state: Vec<f32>,
    ) -> Self {
        let mut rows: Vec<Vec<u32>> = vec![Vec::new(); n_nodes];
        for (edge, members) in edges.iter().enumerate() {
            for &v in members {
                rows[v].push(edge as u32);
            }
        }
        let mut row_offsets = Vec::with_capacity(n_nodes + 1);
        let mut edge_index = Vec::new();
        row_offsets.push(0u32);
        for row in &rows {
            edge_index.extend_from_slice(row);
            row_offsets.push(edge_index.len() as u32);
        }
        let counts = rows.iter().map(|r| r.len() as f32).collect();
        Self {
            n_nodes,
            n_edges: edges.len(),
            hidden,
            row_offsets,
            edge_index,
            counts,
            edge_state,
        }
    }

    /// The same incidence, as the shared support owner models it.
    fn incidence(&self) -> crate::support::Incidence {
        crate::support::Incidence {
            n_nodes: self.n_nodes,
            n_edges: self.n_edges,
            row_ptr: self.row_offsets.iter().map(|&v| v as usize).collect(),
            col_ind: self.edge_index.iter().map(|&v| v as usize).collect(),
            counts: self.counts.clone(),
        }
    }

    /// The CPU owner's arithmetic, in its order.
    fn reference(&self) -> Vec<f32> {
        let mut out = vec![0.0f32; self.n_nodes * self.hidden];
        for node in 0..self.n_nodes {
            let first = self.row_offsets[node] as usize;
            let last = self.row_offsets[node + 1] as usize;
            let row = &mut out[node * self.hidden..(node + 1) * self.hidden];
            for &edge in &self.edge_index[first..last] {
                let base = edge as usize * self.hidden;
                for (slot, &x) in row
                    .iter_mut()
                    .zip(&self.edge_state[base..base + self.hidden])
                {
                    *slot += x;
                }
            }
            if last > first {
                let inv = 1.0 / (last - first) as f32;
                for slot in row.iter_mut() {
                    *slot *= inv;
                }
            }
        }
        out
    }
}

/// Run one incidence on the device and return the node state, or `None` when no adapter exists.
///
/// The incidence reaches the device only through core validation: `crate::support::Incidence::upload`
/// calls `SparseTensorContract::validate` and then `upload_validated_csr`, which re-checks nothing
/// structural. Nothing in this file validates a CSR.
fn run(csr: &Csr) -> Option<Vec<f32>> {
    let backend = crate::support::backend()?;
    let resources = backend.resources();
    let incidence = csr.incidence().upload(&resources, "incidence").unwrap();
    let counts = resources
        .upload_tensor("counts", TensorShape::new(csr.n_nodes, 1), &csr.counts)
        .unwrap();
    let edge_state = resources
        .upload_tensor(
            "edge_state",
            TensorShape::new(csr.n_edges.max(1), csr.hidden),
            &if csr.edge_state.is_empty() {
                vec![0.0f32; csr.hidden]
            } else {
                csr.edge_state.clone()
            },
        )
        .unwrap();
    let node_state = resources
        .zeroed_tensor("node_state", TensorShape::new(csr.n_nodes, csr.hidden))
        .unwrap();

    let op = backend
        .operations()
        .scatter_mean()
        .incidence(&incidence)
        .counts(&counts)
        .edge_state(&edge_state)
        .node_state(&node_state)
        .prepare()
        .unwrap();
    assert_eq!(op.kernel(), KernelId::ScatterMeanCsr);
    assert_eq!(op.elements(), csr.n_nodes * csr.hidden);
    let mut encoder = backend.encoder(Some("scatter-mean-test"));
    op.encode(&mut encoder).unwrap();
    let submission = backend.submit(encoder);
    backend.wait(submission).unwrap();
    Some(backend.read_tensor(&node_state).unwrap())
}

#[test]
fn one_edge_and_many_edges_match_the_cpu_owner() {
    for (label, n_nodes, hidden, edges) in [
        ("one edge", 3usize, 2usize, vec![vec![0usize, 1, 2]]),
        (
            "many edges",
            4,
            3,
            vec![vec![0, 1, 2], vec![2, 3, 1], vec![0, 3, 3]],
        ),
    ] {
        let csr = Csr::from_edges(n_nodes, hidden, &edges, series(edges.len() * hidden, 1));
        let expected = csr.reference();
        let Some(actual) = run(&csr) else {
            eprintln!("SKIP: no WGPU adapter");
            return;
        };
        assert!(
            expected.iter().any(|v| *v != expected[0]),
            "{label}: the reference is constant, parity would be vacuous"
        );
        assert_parity(label, &actual, &expected);
    }
}

#[test]
fn a_node_with_no_incidence_yields_the_cpu_owners_value_not_a_nan() {
    // Node 2 is isolated: its CSR row is empty and its count is 0.
    let csr = Csr::from_edges(3, 2, &[vec![0, 1], vec![1, 0]], series(4, 2));
    assert_eq!(
        csr.counts[2], 0.0,
        "node 2 must be isolated for this control"
    );
    let expected = csr.reference();
    let Some(actual) = run(&csr) else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    assert_eq!(
        &expected[4..6],
        &[0.0, 0.0],
        "the CPU owner yields zero, not NaN"
    );
    assert_parity("isolated node", &actual, &expected);
}

#[test]
fn a_graph_with_zero_edges_yields_an_all_zero_node_state() {
    let csr = Csr::from_edges(3, 2, &[], Vec::new());
    let expected = csr.reference();
    let Some(actual) = run(&csr) else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    assert!(expected.iter().all(|v| *v == 0.0));
    assert_parity("zero edges", &actual, &expected);
}

#[test]
fn non_square_shapes_and_several_hidden_widths_match() {
    // n_nodes != n_edges in both directions, and three widths.
    for (n_nodes, edges) in [
        (5usize, vec![vec![0usize, 1, 2]]),
        (2, vec![vec![0, 1], vec![1, 0], vec![0, 0], vec![1, 1]]),
    ] {
        for hidden in [1usize, 3, 8] {
            let csr = Csr::from_edges(n_nodes, hidden, &edges, series(edges.len() * hidden, 5));
            assert_ne!(csr.n_nodes, csr.n_edges, "the control must be non-square");
            let expected = csr.reference();
            let Some(actual) = run(&csr) else {
                eprintln!("SKIP: no WGPU adapter");
                return;
            };
            assert_parity(
                &format!("n={n_nodes} m={} d={hidden}", csr.n_edges),
                &actual,
                &expected,
            );
        }
    }
}

#[test]
fn a_repeated_member_contributes_twice_as_the_cpu_owner_counts_incidences() {
    // Edge 0 lists vertex 0 twice. The denominator counts incidences, not distinct edges — the
    // invariant `hymeko_graph::build_csr` would violate by deduplicating.
    let csr = Csr::from_edges(2, 2, &[vec![0, 0, 1]], series(2, 3));
    assert_eq!(csr.counts[0], 2.0, "vertex 0 must have two incidences");
    let expected = csr.reference();
    let Some(actual) = run(&csr) else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    assert_parity("repeated member", &actual, &expected);
}

#[test]
fn a_malformed_incidence_cannot_reach_the_device_because_it_cannot_obtain_a_receipt() {
    // The authority split, from the device side. Structural rules are core's now — they are tested
    // exhaustively in `hymeko_core`'s `test_sparse_authority`. What this suite must show is that the
    // device path has no way around them: `upload_validated_csr` takes a receipt, and a malformed
    // tensor cannot produce one.
    let Some(backend) = crate::support::backend() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let resources = backend.resources();
    let good = crate::support::Incidence::from_edges(3, &[vec![0, 1], vec![1, 2]]);
    assert!(
        good.upload(&resources, "good").is_ok(),
        "the well-formed incidence must upload, or the refusals below are vacuous"
    );

    // Six faults, each proved to have applied, then shown to be un-receiptable.
    let base = good.tensor();
    let faults: [(&str, TensorCsr<f32>); 6] = [
        ("first offset not zero", {
            let mut t = base.clone();
            t.row_ptr[0] = 1;
            t
        }),
        ("decreasing offset", {
            let mut t = base.clone();
            t.row_ptr = vec![0, 3, 1, 4];
            t
        }),
        ("offset above nnz", {
            let mut t = base.clone();
            t.row_ptr = vec![0, 1, 9, 4];
            t
        }),
        // Monotone and all offsets <= nnz, but the last one is not nnz — so this control
        // exercises the terminal rule and not, accidentally, the monotonicity rule before it.
        ("terminal offset not nnz", {
            let mut t = base.clone();
            let last = t.row_ptr.len() - 1;
            t.row_ptr[last] -= 1;
            t
        }),
        ("column out of range", {
            let mut t = base.clone();
            t.col_ind[0] = 99;
            t
        }),
        ("short row pointer", {
            let mut t = base.clone();
            t.row_ptr.pop();
            t
        }),
    ];
    for (label, faulty) in faults {
        assert!(
            faulty.row_ptr != base.row_ptr || faulty.col_ind != base.col_ind,
            "{label}: the mutation did not apply"
        );
        let violation = faulty.validate().expect_err(&format!(
            "{label}: core issued a receipt for a malformed incidence"
        ));
        // And with no receipt there is simply no argument to give the device path.
        eprintln!("REFUSED {label}: {violation}");
    }
}

#[test]
fn the_adapter_holds_no_second_structural_validator() {
    // Checked against the source. If `upload_validated_csr` ever grew its own monotonicity or
    // range check, the two authorities could drift and the receipt would stop meaning anything.
    // The device-specific checks it DOES own — narrowing, buffer sizes, layout — stay.
    let source = include_str!("../../src/resources.rs");
    let start = source
        .find("pub fn upload_validated_csr")
        .expect("upload_validated_csr exists");
    let end = source[start..]
        .find("\n    /// Upload a `hymeko_core` CSR matrix directly.")
        .map_or(source.len(), |offset| start + offset);
    let body = &source[start..end];
    for structural in [
        "must start at 0",
        "monotonically non-decreasing",
        "exceeds nnz",
        "must end at nnz",
        "is not a valid column",
    ] {
        assert!(
            !body.contains(structural),
            "upload_validated_csr re-implements the structural check `{structural}`; core owns it"
        );
    }
    // It still narrows, which is its own device concern.
    assert!(
        body.contains("Self::narrow("),
        "the adapter stopped narrowing indices"
    );
}

#[test]
fn the_builder_checks_the_receipt_describes_this_pooling_and_nothing_structural() {
    let Some(backend) = crate::support::backend() else {
        eprintln!("SKIP: no WGPU adapter");
        return;
    };
    let resources = backend.resources();
    let good = crate::support::Incidence::from_edges(3, &[vec![0, 1], vec![1, 2]]);
    let incidence = good.upload(&resources, "incidence").unwrap();
    // An incidence over a different node count: a wiring mistake, not a structural one.
    let other = crate::support::Incidence::from_edges(5, &[vec![0, 1], vec![1, 2]]);
    let mismatched = other.upload(&resources, "mismatched").unwrap();

    let counts = resources
        .upload_tensor("counts", TensorShape::new(3, 1), &good.counts)
        .unwrap();
    let short_counts = resources
        .upload_tensor("short_counts", TensorShape::new(2, 1), &good.counts[..2])
        .unwrap();
    let edge_state = resources
        .upload_tensor("edge_state", TensorShape::new(2, 2), &series(4, 1))
        .unwrap();
    let wrong_width = resources
        .upload_tensor("wrong_width", TensorShape::new(2, 3), &[0.0; 6])
        .unwrap();
    let node_state = resources
        .zeroed_tensor("node_state", TensorShape::new(3, 2))
        .unwrap();

    let cases: [(&str, Option<&hymeko_wgpu::GpuCsrMatrix<f32>>, _, _); 4] = [
        (
            "incidence over the wrong node count",
            Some(&mismatched),
            &counts,
            &edge_state,
        ),
        (
            "short count vector",
            Some(&incidence),
            &short_counts,
            &edge_state,
        ),
        (
            "edge state of the wrong width",
            Some(&incidence),
            &counts,
            &wrong_width,
        ),
        ("missing incidence", None, &counts, &edge_state),
    ];
    for (label, csr, counts_operand, edge_operand) in cases {
        let mut builder = backend
            .operations()
            .scatter_mean()
            .counts(counts_operand)
            .edge_state(edge_operand)
            .node_state(&node_state);
        if let Some(csr) = csr {
            builder = builder.incidence(csr);
        }
        assert!(builder.prepare().is_err(), "{label} was accepted");
    }
    assert!(
        backend
            .operations()
            .scatter_mean()
            .incidence(&incidence)
            .counts(&counts)
            .edge_state(&edge_state)
            .node_state(&node_state)
            .prepare()
            .is_ok(),
        "the well-formed combination must still prepare"
    );
}

#[test]
fn the_shader_contains_no_atomic_and_the_source_says_why() {
    // The structural gate: an atomics-based push scatter would pass every numerical test above on
    // a quiet machine and diverge under contention. Forbid it in the source, not just in prose.
    let shader = hymeko_wgpu::SCATTER_MEAN_CSR_SHADER;
    // Strip comments first: the prose deliberately NAMES the rejected alternative, so a substring
    // search over the whole file would be satisfied by the very sentence explaining the ban.
    let code: String = shader
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for construct in [
        "atomic<",
        "atomicAdd",
        "atomicStore",
        "atomicCompareExchangeWeak",
    ] {
        assert!(
            !code.contains(construct),
            "the scatter kernel gained {construct}; accumulation order would stop being \
             reproducible and CPU parity would have to be weakened to a tolerance"
        );
    }
    assert!(
        shader.contains("no atomic"),
        "the shader must record why the atomics alternative is rejected"
    );
    // And the reciprocal multiply, not a division, is what matches the CPU owner.
    assert!(
        shader.contains("let inv = 1.0 / denom;") && shader.contains("value = acc * inv;"),
        "the shader must reproduce the CPU owner's reciprocal multiply"
    );
}
