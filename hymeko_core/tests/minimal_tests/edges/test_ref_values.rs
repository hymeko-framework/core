#![cfg(test)]
mod test_ref_values {
    use hymeko::common::ids::DeclId;
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::ir::{RefAtomR, SignedRefR, ValueR};
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass::{Interned, intern_ast};
    use hymeko::resolution::interner::Interner;
    use hymeko::resolution::resolve::{Index, build_index_sym};

    use crate::minimal_tests::constants::*;
    use crate::test_helpers::{log_test_footer, log_test_header};
    use log::info;
    use parser::ast::*;
    use std::time::Instant;

    fn find_node<'a>(items: &'a [HyperItem<'a, &'a str>], name: &str) -> &'a NodeDecl<'a, &'a str> {
        items
            .iter()
            .find_map(|item| match item {
                HyperItem::Node(n) if n.inner.name == name => Some(n),
                _ => None,
            })
            .expect("expected node to be present")
    }

    fn find_edge<'a>(items: &'a [HyperItem<'a, &'a str>], name: &str) -> &'a EdgeDecl<'a, &'a str> {
        items
            .iter()
            .find_map(|item| match item {
                HyperItem::Edge(e) if e.inner.name == name => Some(e),
                _ => None,
            })
            .expect("expected edge to be present")
    }

    fn edge_arcs<'a>(edge: &'a EdgeDecl<'a, &'a str>) -> Vec<&'a HyperArc<'a, &'a str>> {
        edge.inner
            .body
            .iter()
            .filter_map(|item| match item {
                HyperItem::Arc(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    fn dir_and_atom<'a>(
        signed: &'a SignedRef<'a, &'a str>,
    ) -> (&'static str, &'a RefAtom<'a, &'a str>) {
        match signed {
            SignedRef::Plus(atom) => ("+", atom),
            SignedRef::Minus(atom) => ("-", atom),
            SignedRef::Neutral(atom) => ("~", atom),
        }
    }

    fn assert_weights(atom: &RefAtom<'_, &'_ str>, expected: &[f64]) -> Vec<f64> {
        // 1. Extract the unified value field
        let actual_val = atom.anno.value.as_ref().expect("weights must be present");

        // 2. Safely map it to a slice, whether it's a list or a single number
        let actual_slice = match actual_val {
            Value::List(xs) => xs.as_slice(),
            Value::Num(_) => std::slice::from_ref(actual_val), // Zero-cost abstraction
            other => panic!("expected numeric weight or list, got {:?}", other),
        };

        // 3. Verify arity and precision
        assert_eq!(
            actual_slice.len(),
            expected.len(),
            "weight arity mismatch for {:?}",
            atom.target.path
        );

        let mut collected = Vec::with_capacity(actual_slice.len());
        for (value, exp) in actual_slice.iter().zip(expected.iter()) {
            let num = match value {
                Value::Num(n) => *n,
                other => panic!("expected numeric weight, got {:?}", other),
            };
            assert!(
                (num - exp).abs() < 1e-9,
                "weight mismatch: got {num}, expected {exp}"
            );
            collected.push(num);
        }
        collected
    }

    fn dir_and_atom_ir<'a>(signed: &'a SignedRefR) -> (&'static str, &'a RefAtomR) {
        match signed {
            SignedRefR::Plus(atom) => ("+", atom),
            SignedRefR::Minus(atom) => ("-", atom),
            SignedRefR::Neutral(atom) => ("~", atom),
        }
    }

    fn assert_ir_weights(weights: &Option<Vec<ValueR>>, expected: &[f64]) -> Vec<f64> {
        let actual = weights.as_ref().expect("weights must be present");
        assert_eq!(actual.len(), expected.len(), "weight arity mismatch");
        let mut collected = Vec::with_capacity(actual.len());
        for (value, exp) in actual.iter().zip(expected.iter()) {
            let num = match value {
                ValueR::Num(n) => *n,
                other => panic!("expected numeric weight, got {:?}", other),
            };
            assert!(
                (num - exp).abs() < 1e-9,
                "weight mismatch: got {num}, expected {exp}"
            );
            collected.push(num);
        }
        collected
    }

    #[test]
    fn edges_with_ref_values() {
        log_test_header(
            "edges_with_ref_values",
            "Parses the minimal edge fixture and validates AST-level references.",
        );
        let start = Instant::now();
        let source_code =
            parser::read_source_file(EDGE_REF_VALUES_PATH).expect("failed to read source file");

        // 2. Parse it, tying the AST lifetimes to the String
        let d = parser::parse_description(&source_code).unwrap();
        assert_eq!(d.name, DESC_MINIMAL_EXAMPLE_NAME);

        let context = find_node(&d.items, CONTEXT_NODE_NAME);
        let ctx_body = context
            .inner
            .body
            .as_ref()
            .expect("context should contain nested items");
        let node_lev_1 = find_node(ctx_body, NODE_LEVEL1_NAME);
        let lev1_body = node_lev_1
            .inner
            .body
            .as_ref()
            .expect("node_lev_1 should contain nested items");

        let edge = find_edge(lev1_body, EDGE_E0_NAME);
        let arcs = edge_arcs(edge);
        assert_eq!(arcs.len(), 1, "edge e0 should have a single arc");

        let refs = &arcs[0].inner.refs;
        assert_eq!(refs.len(), 4, "arc should reference four nodes");

        let mut flattened_weights = Vec::new();
        for (signed, expectation) in refs.iter().zip(EDGE_REF_EXPECTATIONS.iter()) {
            let (actual_dir, atom) = dir_and_atom(signed);
            assert_eq!(
                actual_dir, expectation.dir,
                "unexpected reference direction"
            );
            assert_eq!(
                atom.target.path.as_slice(),
                expectation.path,
                "reference path mismatch"
            );
            let nums = assert_weights(atom, expectation.weights);
            flattened_weights.extend(nums);
        }

        assert_eq!(
            flattened_weights.len(),
            EDGE_REF_FLAT_WEIGHTS.len(),
            "flattened weights length mismatch (AST)"
        );
        for (actual, expected) in flattened_weights.iter().zip(EDGE_REF_FLAT_WEIGHTS.iter()) {
            assert!(
                (*actual - *expected).abs() < 1e-9,
                "AST flattened weight mismatch: got {actual}, expected {expected}"
            );
        }
        info!("Validated AST refs for edge {}", EDGE_E0_NAME);
        log_test_footer(
            "edges_with_ref_values",
            Some(start.elapsed()),
            "All AST references carried the expected weights and paths.",
        );
    }

    #[test]
    fn edges_with_ref_values_ir() {
        log_test_header(
            "edges_with_ref_values_ir",
            "Lowers the fixture to IR and checks reference targets/weights.",
        );
        let start = Instant::now();
        let source_code =
            parser::read_source_file(EDGE_REF_VALUES_PATH).expect("failed to read source file");

        // 2. Parse it, tying the AST lifetimes to the String
        let desc = parser::parse_description(&source_code).unwrap();
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &interner).unwrap();
        let ir = lower_to_ir(&ast, &idx, &mut interner).unwrap();

        let did_context = decl_id_for_path(&mut interner, &idx, &[CONTEXT_NODE_NAME]);
        let did_node_lev_0 =
            decl_id_for_path(&mut interner, &idx, &[CONTEXT_NODE_NAME, NODE_LEVEL0_NAME]);
        let did_node_lev_1 =
            decl_id_for_path(&mut interner, &idx, &[CONTEXT_NODE_NAME, NODE_LEVEL1_NAME]);
        let did_node_lev1_node0 = decl_id_for_path(
            &mut interner,
            &idx,
            &[CONTEXT_NODE_NAME, NODE_LEVEL1_NAME, NODE0_NAME],
        );
        let did_node_lev0_node0 = decl_id_for_path(
            &mut interner,
            &idx,
            &[CONTEXT_NODE_NAME, NODE_LEVEL0_NAME, NODE0_NAME],
        );
        let did_node_lev0_node1 = decl_id_for_path(
            &mut interner,
            &idx,
            &[CONTEXT_NODE_NAME, NODE_LEVEL0_NAME, NODE1_NAME],
        );
        let did_node_lev0_node2 = decl_id_for_path(
            &mut interner,
            &idx,
            &[CONTEXT_NODE_NAME, NODE_LEVEL0_NAME, NODE2_NAME],
        );
        let did_e0 = decl_id_for_path(
            &mut interner,
            &idx,
            &[CONTEXT_NODE_NAME, NODE_LEVEL1_NAME, EDGE_E0_NAME],
        );

        let edge_id = ir.decl_to_edge[did_e0.0].expect("e0 not lowered as edge");
        let edge_rec = &ir.edges[edge_id.0];
        assert_eq!(edge_rec.arcs.len(), 1, "expected a single arc in e0");
        let arc_id = edge_rec.arcs[0];
        let arc = &ir.arcs[arc_id.0];
        assert_eq!(arc.refs.len(), 4, "arc should reference four nodes");

        let expectations = [
            ("-", did_node_lev1_node0, EDGE_REF_EXPECTATIONS[0].weights),
            ("+", did_node_lev0_node1, EDGE_REF_EXPECTATIONS[1].weights),
            ("-", did_node_lev0_node2, EDGE_REF_EXPECTATIONS[2].weights),
            ("-", did_node_lev0_node0, EDGE_REF_EXPECTATIONS[3].weights),
        ];
        let mut flattened_weights = Vec::new();

        for (signed, (dir, target, weights)) in arc.refs.iter().zip(expectations.into_iter()) {
            let (actual_dir, atom) = dir_and_atom_ir(signed);
            assert_eq!(actual_dir, dir, "unexpected reference direction");
            assert_eq!(atom.target, target, "reference target mismatch");
            let nums = assert_ir_weights(&atom.weights, &weights);
            flattened_weights.extend(nums);
        }

        assert_eq!(
            flattened_weights.len(),
            EDGE_REF_FLAT_WEIGHTS.len(),
            "flattened weights length mismatch"
        );
        for (actual, expected) in flattened_weights.iter().zip(EDGE_REF_FLAT_WEIGHTS.iter()) {
            assert!(
                (*actual - *expected).abs() < 1e-9,
                "flattened weight mismatch: got {actual}, expected {expected}"
            );
        }

        // Ensure the context node still maps to a DeclId to avoid unused warnings
        let _ = did_context;
        let _ = did_node_lev_0;
        let _ = did_node_lev_1;
        info!("Validated IR refs for edge {}", EDGE_E0_NAME);
        log_test_footer(
            "edges_with_ref_values_ir",
            Some(start.elapsed()),
            "IR references matched expected targets and weights.",
        );
    }

    fn decl_id_for_path(interner: &mut Interner, idx: &Index, segments: &[&str]) -> DeclId {
        let path_syms: Vec<_> = segments.iter().map(|seg| interner.intern(seg)).collect();
        *idx.by_path
            .get(&PathKey(path_syms))
            .expect("missing declaration for path")
    }
}
