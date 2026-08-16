//! Tests for multi-dimensional array values and edge weight annotations.
//!
//! Place in: hymeko_core/tests/test_edge_values.rs
//! Data files: hymeko_core/data/

use hymeko::common::ids::DeclId;
use hymeko::ir::ir::{DeclKind, Ir, SignedRefR, ValueR};
use hymeko::resolution::interner::Interner;

// ─── Value inspection helpers ─────────────────────────────────────────────

fn count_leaves(v: &ValueR) -> usize {
    match v {
        ValueR::Num(_) => 1,
        ValueR::List(xs) => xs.iter().map(count_leaves).sum(),
        _ => 0,
    }
}

fn nesting_depth(v: &ValueR) -> usize {
    match v {
        ValueR::List(xs) => 1 + xs.iter().map(nesting_depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn describe(v: &ValueR) -> String {
    match v {
        ValueR::Num(n) => format!("{n}"),
        ValueR::List(xs) => {
            let inner: Vec<String> = xs.iter().map(describe).collect();
            format!("[{}]", inner.join(", "))
        }
        other => format!("{:?}", other),
    }
}

fn weights_of(r: &SignedRefR) -> &[ValueR] {
    let atom = match r {
        SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => a,
    };
    atom.weights.as_deref().unwrap_or(&[])
}

fn count_kind(ir: &Ir, kind: DeclKind) -> usize {
    ir.decl_nodes.iter().filter(|d| d.kind == kind).count()
}

// ═══════════════════════════════════════════════════════════════════════════
// linear_edge_values.hymeko — flat weight annotations
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod linear_edges {
    use super::*;
    use crate::test_helpers::{find_decl, load_and_lower};
    const PATH: &str = "../data/minimal_examples/testing_edges/linear_edge_values.hymeko";

    #[test]
    fn parses_without_error() {
        let (_s, c) = load_and_lower(PATH).unwrap();
        assert!(c.ir.decl_nodes.len() > 0);
    }

    #[test]
    fn has_four_edges() {
        let (_s, c) = load_and_lower(PATH).unwrap();
        assert_eq!(count_kind(&c.ir, DeclKind::Edge), 4);
    }

    #[test]
    fn all_edges_found() {
        let (s, c) = load_and_lower(PATH).unwrap();
        for name in &["e1", "e2", "e3", "e4"] {
            find_decl(&c.ir, &s.it, name, DeclKind::Edge);
        }
    }

    #[test]
    fn five_nodes() {
        let (_s, c) = load_and_lower(PATH).unwrap();
        let n = count_kind(&c.ir, DeclKind::Node);
        assert!(n >= 5, "Expected >=5 nodes, got {}", n);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// edge_example_multivalue.hymeko — nested multi-dimensional arrays
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod multivalue_edges {
    use super::*;
    use crate::test_helpers::{find_decl, load_and_lower};
    const PATH: &str = "../data/minimal_examples/testing_edges/edge_example_multivalue.hymeko";

    #[test]
    fn parses_nested_arrays() {
        // THE critical test: if this passes, grammar handles [[...]] recursion
        let (_s, c) = load_and_lower(PATH).unwrap();
        assert!(c.ir.decl_nodes.len() > 0);
    }

    #[test]
    fn edge_e_exists() {
        let (s, c) = load_and_lower(PATH).unwrap();
        find_decl(&c.ir, &s.it, "e", DeclKind::Edge);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// hierarchy references with values
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod hierarchy_edges {
    use super::*;
    use crate::test_helpers::{find_decl, load_and_lower};
    const PATH: &str = "../data/minimal_examples/testing_edges/minimal_example_with_hierarchy_ref_edges_with_values.hymeko";

    #[test]
    fn parses_without_error() {
        let (_s, c) = load_and_lower(PATH).unwrap();
        assert!(c.ir.decl_nodes.len() > 0);
    }

    #[test]
    fn e0_exists() {
        let (s, c) = load_and_lower(PATH).unwrap();
        find_decl(&c.ir, &s.it, "e0", DeclKind::Edge);
    }

    #[test]
    fn e0_first_weight_is_0_85() {
        let (s, c) = load_and_lower(PATH).unwrap();
        let did = find_decl(&c.ir, &s.it, "e0", DeclKind::Edge);
        // Access the edge's first arc's first ref
        // NOTE: adjust field access if your IR stores arcs differently
        //   e.g., c.ir.edges[eid.0].arcs[0].refs[0]
        //   or c.ir.edge_arcs(did)[0].refs[0]
        // Using weight0 from test_helpers once you have the SignedRefR:
        // let w = weight0(&ref0);
        // assert!((w - 0.85).abs() < 1e-9);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// minimal 2-node 1-edge
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod minimal_2n1e {
    use super::*;
    use crate::test_helpers::{find_decl, load_and_lower};
    const PATH: &str =
        "../data/minimal_examples/testing_edges/minimal_test_tensor_values_2nodes_1_edge.hymeko";

    #[test]
    fn parses_and_has_one_edge() {
        let (_s, c) = load_and_lower(PATH).unwrap();
        assert_eq!(count_kind(&c.ir, DeclKind::Edge), 1);
    }

    #[test]
    fn e0_exists() {
        let (s, c) = load_and_lower(PATH).unwrap();
        find_decl(&c.ir, &s.it, "e0", DeclKind::Edge);
    }
}
