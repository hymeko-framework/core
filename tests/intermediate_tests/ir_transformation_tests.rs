#[cfg(test)]
mod basic_transformation_tests {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::ids::{DeclId, SymId};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::common::{ref_sign, ref_target};
    use hymeko::ir::ir::DeclKind;
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass::{Interned, intern_ast};
    use hymeko::resolution::resolve::build_index_sym;
    use log::info;
    use parser::parse_description;
    use std::time::Instant;

    const SIMPLE_CHAIN_SRC: &str = r#"
        test{}
        D {
            Root {
                A;
                B;
                C;
            }
        }
        "#;
    const DUP_NAMES_SRC: &str = r#"
        test{}
        D {
            Root {
                A {
                  A;
                  B;
                  C;
                }
                B;
                C;
            }
        }
        "#;
    const EDGE_WITH_ARC_SRC: &str = r#"
        edge_example{}
        D {
            Root0 {}
            Root {
                @E { (+Root, -Root0); }
            }
        }
        "#;

    const SYM_TEST: &str = "test";
    const SYM_D: &str = "D";
    const SYM_ROOT: &str = "Root";
    const SYM_ROOT0: &str = "Root0";
    const SYM_A: &str = "A";
    const SYM_B: &str = "B";
    const SYM_C: &str = "C";
    const SYM_E: &str = "E";

    const SID_TEST: usize = 0;
    const SID_D: usize = 1;
    const SID_ROOT: usize = 2;
    const SID_A: usize = 3;
    const SID_B: usize = 4;
    const SID_C: usize = 5;

    const SIMPLE_INDEX_LEN: usize = 5;
    const DUP_INDEX_LEN: usize = 8;

    fn start(name: &str, desc: &str) -> Instant {
        log_test_header(name, desc);
        Instant::now()
    }

    fn finish(name: &str, start: Instant, summary: &str) {
        log_test_footer(name, Some(start.elapsed()), summary);
    }

    #[test]
    fn node_child_chain_order_is_body_order() {
        let timer = start(
            "node_child_chain_order_is_body_order",
            "Validates sibling ordering for a simple Root.{A,B,C} chain.",
        );
        let ast_str = parse_description(SIMPLE_CHAIN_SRC).expect("parse failed");
        let Interned { ast, mut interner } = intern_ast(&ast_str);
        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        let sid_root = interner.intern(SYM_ROOT);
        let sid_a = interner.intern(SYM_A);
        let sid_b = interner.intern(SYM_B);
        let sid_c = interner.intern(SYM_C);

        assert_eq!(interner.resolve(SymId::new(SID_TEST)), SYM_TEST);
        assert_eq!(interner.resolve(SymId::new(SID_D)), SYM_D);
        assert_eq!(interner.resolve(SymId::new(SID_ROOT)), SYM_ROOT);
        assert_eq!(interner.resolve(SymId::new(SID_A)), SYM_A);
        assert_eq!(interner.resolve(SymId::new(SID_B)), SYM_B);
        assert_eq!(interner.resolve(SymId::new(SID_C)), SYM_C);

        assert_eq!(sid_root.0, SID_ROOT);
        assert_eq!(sid_a.0, SID_A);
        assert_eq!(sid_b.0, SID_B);
        assert_eq!(sid_c.0, SID_C);

        let sid_d = interner.intern(SYM_D);
        assert_eq!(sid_d.0, SID_D, "D SymId should match expectation");

        let did_d = *idx.by_path.get(&PathKey(vec![sid_d])).expect("missing D");
        let did_root = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root]))
            .expect("missing D.Root");
        let did_a = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_a]))
            .expect("missing D.Root.A");
        let did_b = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_b]))
            .expect("missing D.Root.B");
        let did_c = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_c]))
            .expect("missing D.Root.C");

        // Assert Index paths match expected DeclIds
        assert_eq!(did_d.0, 0, "D DeclId should be 0");
        assert_eq!(did_root.0, 1, "D.Root DeclId should be 1");
        assert_eq!(did_a.0, 2, "D.Root.A DeclId should be 2");
        assert_eq!(did_b.0, 3, "D.Root.B DeclId should be 3");
        assert_eq!(did_c.0, 4, "D.Root.C DeclId should be 4");
        assert_eq!(
            idx.by_path.len(),
            SIMPLE_INDEX_LEN,
            "Index should contain expected number of paths"
        );

        // Root must be a node in IR
        let _root_nid = ir.decl_to_node[did_root.0].expect("Root not lowered as node");

        // first_child points to A, and A->B->C via next_sibling
        assert_eq!(
            ir.first_child(did_root),
            did_a,
            "Root.first_child should be Root.A"
        );

        assert_eq!(ir.next_sibling(did_a), did_b, "A.next_sibling should be B");
        assert_eq!(ir.next_sibling(did_b), did_c, "B.next_sibling should be C");
        assert_eq!(
            ir.next_sibling(did_c),
            DeclId::NONE,
            "C.next_sibling should be None"
        );
        let children: Vec<_> = ir.children(did_root).collect();
        info!(
            "Root children sequence = {:?}",
            children.iter().map(|d| d.0).collect::<Vec<_>>()
        );
        finish(
            "node_child_chain_order_is_body_order",
            timer,
            "Root preserved the body order for all immediate children.",
        );
    }

    #[test]
    fn node_child_chain_order_is_body_order_same_names() {
        let timer = start(
            "node_child_chain_order_is_body_order_same_names",
            "Ensures siblings keep order even when names repeat.",
        );
        let ast_str = parse_description(DUP_NAMES_SRC).expect("parse failed");
        let Interned { ast, mut interner } = intern_ast(&ast_str);
        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        let sid_root = interner.intern(SYM_ROOT);
        let sid_a = interner.intern(SYM_A);
        let sid_b = interner.intern(SYM_B);
        let sid_c = interner.intern(SYM_C);

        assert_eq!(interner.resolve(SymId::new(SID_TEST)), SYM_TEST);
        assert_eq!(interner.resolve(SymId::new(SID_D)), SYM_D);
        assert_eq!(interner.resolve(SymId::new(SID_ROOT)), SYM_ROOT);
        assert_eq!(interner.resolve(SymId::new(SID_A)), SYM_A);
        assert_eq!(interner.resolve(SymId::new(SID_B)), SYM_B);
        assert_eq!(interner.resolve(SymId::new(SID_C)), SYM_C);

        assert_eq!(sid_root.0, SID_ROOT);
        assert_eq!(sid_a.0, SID_A);
        assert_eq!(sid_b.0, SID_B);
        assert_eq!(sid_c.0, SID_C);

        let sid_d = interner.intern(SYM_D);
        assert_eq!(sid_d.0, SID_D, "D SymId should match expectation");

        let did_d = *idx.by_path.get(&PathKey(vec![sid_d])).expect("missing D");
        let did_root = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root]))
            .expect("missing D.Root");
        let did_root_a = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_a]))
            .expect("missing D.Root.A");
        let did_root_a_a = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_a, sid_a]))
            .expect("missing D.Root.A.A");
        let did_root_a_b = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_a, sid_b]))
            .expect("missing D.Root.A.B");
        let did_root_a_c = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_a, sid_c]))
            .expect("missing D.Root.A.C");
        let did_root_b = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_b]))
            .expect("missing D.Root.B");
        let did_root_c = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_c]))
            .expect("missing D.Root.C");

        assert_eq!(did_d.0, 0, "D DeclId should be 0");
        assert_eq!(did_root.0, 1, "D.Root DeclId should be 1");
        assert_eq!(did_root_a.0, 2, "D.Root.A DeclId should be 2");
        assert_eq!(did_root_a_a.0, 3, "D.Root.A.A DeclId should be 3");
        assert_eq!(did_root_a_b.0, 4, "D.Root.A.B DeclId should be 4");
        assert_eq!(did_root_a_c.0, 5, "D.Root.A.C DeclId should be 5");
        assert_eq!(did_root_b.0, 6, "D.Root.B DeclId should be 6");
        assert_eq!(did_root_c.0, 7, "D.Root.C DeclId should be 7");
        assert_eq!(
            idx.by_path.len(),
            DUP_INDEX_LEN,
            "Index should contain expected number of paths"
        );

        // Root must be a node in IR
        let _root_nid = ir.decl_to_node[did_root.0].expect("Root not lowered as node");

        // first_child points to Root.A, and Root.A->Root.B->Root.C via next_sibling
        assert_eq!(
            ir.first_child(did_root),
            did_root_a,
            "Root.first_child should be Root.A"
        );

        let root_a_nid = ir.decl_to_node[did_root_a.0].expect("Root.A not lowered as node");
        let _root_b_nid = ir.decl_to_node[did_root_b.0].expect("Root.B not lowered as node");
        let _root_c_nid = ir.decl_to_node[did_root_c.0].expect("Root.C not lowered as node");

        assert_eq!(
            ir.next_sibling(did_root_a),
            did_root_b,
            "Root.A.next_sibling should be Root.B"
        );
        assert_eq!(
            ir.next_sibling(did_root_b),
            did_root_c,
            "Root.B.next_sibling should be Root.C"
        );
        assert_eq!(
            ir.next_sibling(did_root_c),
            DeclId::NONE,
            "Root.C.next_sibling should be None"
        );

        // Root.A must also have children: Root.A.A->Root.A.B->Root.A.C
        let _root_a_rec = &ir.nodes[root_a_nid.0];
        assert_eq!(
            ir.first_child(did_root_a),
            did_root_a_a,
            "Root.A.first_child should be Root.A.A"
        );

        let root_a_a_nid = ir.decl_to_node[did_root_a_a.0].expect("Root.A.A not lowered as node");
        let root_a_b_nid = ir.decl_to_node[did_root_a_b.0].expect("Root.A.B not lowered as node");
        let root_a_c_nid = ir.decl_to_node[did_root_a_c.0].expect("Root.A.C not lowered as node");
        assert_ne!(root_a_a_nid, root_a_b_nid);
        assert_ne!(root_a_b_nid, root_a_c_nid);

        assert_eq!(
            ir.next_sibling(did_root_a_a),
            did_root_a_b,
            "Root.A.A.next_sibling should be Root.A.B"
        );
        assert_eq!(
            ir.next_sibling(did_root_a_b),
            did_root_a_c,
            "Root.A.B.next_sibling should be Root.A.C"
        );
        assert_eq!(
            ir.next_sibling(did_root_a_c),
            DeclId::NONE,
            "Root.A.C.next_sibling should be None"
        );
        let kids_root: Vec<_> = ir.children(did_root).collect();
        assert_eq!(kids_root, vec![did_root_a, did_root_b, did_root_c]);

        let kids_a: Vec<_> = ir.children(did_root_a).collect();
        assert_eq!(kids_a, vec![did_root_a_a, did_root_a_b, did_root_a_c]);
        info!(
            "Root kids {:?}, nested Root.A kids {:?}",
            kids_root.iter().map(|d| d.0).collect::<Vec<_>>(),
            kids_a.iter().map(|d| d.0).collect::<Vec<_>>()
        );
        finish(
            "node_child_chain_order_is_body_order_same_names",
            timer,
            "Duplicate-named children retained deterministic ordering.",
        );
    }

    #[test]
    fn edge_children_include_arcs_as_decls() {
        let timer = start(
            "edge_children_include_arcs_as_decls",
            "Checks that arc decls stay attached to their parent edge.",
        );
        let ast_str = parse_description(EDGE_WITH_ARC_SRC).expect("parse failed");
        let Interned { ast, mut interner } = intern_ast(&ast_str);
        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        let sid_d = interner.intern(SYM_D);
        let sid_root = interner.intern(SYM_ROOT);
        let sid_root0 = interner.intern(SYM_ROOT0);
        let sid_e = interner.intern(SYM_E);

        let did_root = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root]))
            .expect("missing D.Root");
        let did_root0 = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root0]))
            .expect("missing D.Root0");
        let did_edge = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_e]))
            .unwrap();

        // 1) Edge children (decl-level) contains exactly one Arc decl
        let kids: Vec<DeclId> = ir.decl_children(did_edge).collect();
        assert_eq!(
            kids.len(),
            1,
            "edge should have exactly one decl-child (the arc)"
        );

        let arc_decl = kids[0];
        assert_eq!(
            ir.decl_nodes[arc_decl.0].kind,
            DeclKind::HyperArc,
            "child should be Arc"
        );

        // 2) Downcast: Arc decl -> ArcId -> ArcRec
        let arc_id =
            ir.decl_to_arc[arc_decl.0].expect("arc decl should map to ArcId via decl_to_arc");
        let arc = &ir.arcs[arc_id.0];

        // Arc decl konzisztencia (ArcRec-ben nincs `decl`, ezért mappinget tesztelünk)
        assert_eq!(
            ir.decl_nodes[arc_decl.0].kind,
            DeclKind::HyperArc,
            "child decl should be Arc"
        );
        assert_eq!(
            ir.decl_nodes[arc_decl.0].parent, did_edge,
            "Arc decl parent should be the edge"
        );

        assert_eq!(
            arc.in_edge, did_edge,
            "ArcRec.in_edge should be the edge decl"
        );

        // Refs: (+Root, -Root0) sorrendben
        assert_eq!(arc.refs.len(), 2, "arc should have 2 refs");

        assert_eq!(ref_sign(&arc.refs[0]), 1, "first ref should be +");
        assert_eq!(
            ref_target(&arc.refs[0]),
            did_root,
            "first ref should target Root"
        );

        assert_eq!(ref_sign(&arc.refs[1]), -1, "second ref should be -");
        assert_eq!(
            ref_target(&arc.refs[1]),
            did_root0,
            "second ref should target Root0"
        );
        info!(
            "Arc decl {:?} resolved to refs {:?}",
            arc_decl.0,
            arc.refs.len()
        );
        finish(
            "edge_children_include_arcs_as_decls",
            timer,
            "Edge children enumeration exposed the arc declaration and references.",
        );
    }
}
