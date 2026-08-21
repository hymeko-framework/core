#[cfg(test)]
mod test_annotations {
    use crate::minimal_tests::constants::*;
    use crate::test_helpers::{
        find_decl, get_node, has_tag, load_and_lower, log_test_footer, log_test_header, weight0,
    };
    use hymeko::ir::ir::{DeclKind, SignedRefR};
    use log::info;
    use std::time::Instant;

    #[test]
    fn parses_minimal_tag_annotation_and_extracts_bases_and_arc_weights() {
        log_test_header(
            "parses_minimal_tag_annotation_and_extracts_bases_and_arc_weights",
            "Parses tag annotations and validates node bases along with arc weights.",
        );
        let start = Instant::now();
        let src = std::fs::read_to_string(TAG_ANNOTATION_PATH).unwrap();

        let ast = parser::parse_description(&src).expect("parse should succeed");

        use parser::ast::{HyperItem, SignedRef, Value};

        // 1) context NODE megkeresése (top-szinten)
        let context_node = ast
            .items
            .iter()
            .find_map(|it| match it {
                HyperItem::Node(n) if n.inner.name == CONTEXT_NODE_NAME => Some(n),
                _ => None,
            })
            .expect("context node should exist at top-level");

        let ctx_items = context_node
            .inner
            .body
            .as_ref()
            .expect("context should have a body");

        // 2) node10 és e0 keresése a context body-ban
        let node10 = ctx_items
            .iter()
            .find_map(|it| match it {
                HyperItem::Node(n) if n.inner.name == NODE10_NAME => Some(n),
                _ => None,
            })
            .expect("node10 should exist inside context");

        let edge_e0 = ctx_items
            .iter()
            .find_map(|it| match it {
                HyperItem::Edge(e) if e.inner.name == EDGE_E0_NAME => Some(e),
                _ => None,
            })
            .expect("e0 edge should exist inside context");

        // 3) node10 bases: + <isa> node0
        assert_eq!(node10.inner.bases.len(), 1);
        match &node10.inner.bases[0] {
            SignedRef::Plus(atom) => {
                assert_eq!(atom.target.path.as_slice(), [NODE0_NAME]);
                assert!(atom.anno.tags.contains(&TAG_ISA), "expected <isa> tag");
                assert!(atom.anno.value.is_none());
            }
            other => panic!("expected Plus base ref, got {:?}", other),
        }

        // 4) e0 arc: (- node0[0.85], + node10[0.9])
        let arc = edge_e0
            .inner
            .body
            .iter()
            .find_map(|it| {
                if let HyperItem::Arc(a) = it {
                    Some(a)
                } else {
                    None
                }
            })
            .expect("expected an arc inside e0");

        assert_eq!(arc.inner.refs.len(), 2);

        fn weight(r: &SignedRef<'_, &str>) -> f64 {
            let v = match r {
                SignedRef::Plus(a) | SignedRef::Minus(a) | SignedRef::Neutral(a) => {
                    a.anno.value.as_ref()
                }
            }
            .expect("expected weight value");

            match v {
                Value::List(xs) => match xs.as_slice() {
                    [Value::Num(n)] => *n,
                    _ => panic!("expected single numeric list weight, got {:?}", v),
                },
                Value::Num(n) => *n,
                _ => panic!("unexpected weight type: {:?}", v),
            }
        }

        // - node0[0.85]
        match &arc.inner.refs[0] {
            SignedRef::Minus(a) => {
                assert_eq!(a.target.path.as_slice(), [NODE0_NAME]);
                assert!((weight(&arc.inner.refs[0]) - 0.85).abs() < 1e-9);
            }
            other => panic!("expected first ref Minus(node0), got {:?}", other),
        }

        // + node10[0.9]
        match &arc.inner.refs[1] {
            SignedRef::Plus(a) => {
                assert_eq!(a.target.path.as_slice(), [NODE10_NAME]);
                assert!((weight(&arc.inner.refs[1]) - 0.9).abs() < 1e-9);
            }
            other => panic!("expected second ref Plus(node10), got {:?}", other),
        }
        info!("Validated annotation fixture {}", TAG_ANNOTATION_PATH);
        log_test_footer(
            "parses_minimal_tag_annotation_and_extracts_bases_and_arc_weights",
            Some(start.elapsed()),
            "Node bases and arc weights matched expectations.",
        );
    }

    #[test]
    fn lowers_node_bases_into_ir_with_isa_tag() {
        log_test_header(
            "lowers_node_bases_into_ir_with_isa_tag",
            "Checks that node bases carry isa tags through IR lowering.",
        );
        let start = Instant::now();
        let (store, compiled) = load_and_lower(TAG_ANNOTATION_PATH).unwrap();

        let ir = &compiled.ir;
        let it = &store.it;

        // 1) segéd: DeclId keresés név + kind alapján

        let node10 = find_decl(ir, it, NODE10_NAME, DeclKind::Node);
        let node0 = find_decl(ir, it, NODE0_NAME, DeclKind::Node);

        // 2) NodeRec lekérés
        let n10_id = ir.as_node(node10).expect("node10 should be a node");
        let n10 = &ir.nodes[n10_id.0];

        assert_eq!(n10.bases.len(), 1);

        // 3) base ref ellenőrzés
        let isa_sym = it.get_id(TAG_ISA).expect("isa should be interned");

        match &n10.bases[0] {
            SignedRefR::Plus(atom) => {
                assert_eq!(atom.target, node0);
                assert!(atom.anno.tags.contains(&isa_sym));
            }
            other => panic!("expected Plus base ref, got {:?}", other),
        }
        info!(
            "lowered node {} base count {}",
            NODE10_NAME,
            n10.bases.len()
        );
        log_test_footer(
            "lowers_node_bases_into_ir_with_isa_tag",
            Some(start.elapsed()),
            "IR retained isa tags on node10 -> node0 base.",
        );
    }

    #[test]
    fn lowers_multi_bases_into_ir_and_preserves_tags_and_default_direction() {
        log_test_header(
            "lowers_multi_bases_into_ir_and_preserves_tags_and_default_direction",
            "Ensures multi-base annotations keep their tags and signs in IR.",
        );
        let start = Instant::now();
        let (store, compiled) = load_and_lower(MULTI_TAG_ANNOTATION_PATH).unwrap();

        let ir = &compiled.ir;
        let it = &store.it;

        // --- decl ids ---
        let node0 = find_decl(ir, it, NODE0_NAME, DeclKind::Node);
        let node10 = find_decl(ir, it, NODE10_NAME, DeclKind::Node);
        let node11 = find_decl(ir, it, NODE11_NAME, DeclKind::Node);
        let node2 = find_decl(ir, it, NODE2_NAME, DeclKind::Node);

        // --- node10 bases ---
        let n10 = get_node(ir, node10);
        assert_eq!(n10.bases.len(), 1);
        match &n10.bases[0] {
            SignedRefR::Plus(a) => {
                assert_eq!(a.target, node0);
                assert!(has_tag(it, &a.anno.tags, TAG_ISA));
            }
            other => panic!("node10: expected Plus base, got {:?}", other),
        }

        // --- node11 bases: +isa node0, +impl node2, neutral isa node10 ---
        let n11 = get_node(ir, node11);
        assert_eq!(n11.bases.len(), 3);

        // 1) + <isa> node0
        match &n11.bases[0] {
            SignedRefR::Plus(a) => {
                assert_eq!(a.target, node0);
                assert!(has_tag(it, &a.anno.tags, TAG_ISA));
            }
            other => panic!("node11[0]: expected Plus(isa node0), got {:?}", other),
        }

        // 2) + <impl> node2
        match &n11.bases[1] {
            SignedRefR::Plus(a) => {
                assert_eq!(a.target, node2);
                assert!(has_tag(it, &a.anno.tags, TAG_IMPL));
            }
            other => panic!("node11[1]: expected Plus(impl node2), got {:?}", other),
        }

        // 3) <isa> node10  (no sign => Neutral default)
        match &n11.bases[2] {
            SignedRefR::Neutral(a) => {
                assert_eq!(a.target, node10);
                assert!(has_tag(it, &a.anno.tags, TAG_ISA));
            }
            other => panic!("node11[2]: expected Neutral(isa node10), got {:?}", other),
        }

        // --- node2 bases should be empty ---
        let n2 = get_node(ir, node2);
        assert!(n2.bases.is_empty());

        // --- optional: check e0 arc weights still parse/lower ---
        let e0 = find_decl(ir, it, EDGE_E0_NAME, DeclKind::Edge);
        let eid = ir.as_edge(e0).expect("e0 should be an edge");
        let e0rec = &ir.edges[eid.0];

        assert_eq!(e0rec.arcs.len(), 1);
        let arc = &ir.arcs[e0rec.arcs[0].0];

        assert_eq!(arc.refs.len(), 2);

        match &arc.refs[0] {
            SignedRefR::Minus(a) => {
                assert_eq!(a.target, node0);
                let w = weight0(&arc.refs[0]);
                assert!((w - 0.85).abs() < 1e-9);
            }
            other => panic!("arc[0]: expected Minus(node0), got {:?}", other),
        }

        match &arc.refs[1] {
            SignedRefR::Plus(a) => {
                assert_eq!(a.target, node10);
                let w = weight0(&arc.refs[1]);
                assert!((w - 0.9).abs() < 1e-9);
            }
            other => panic!("arc[1]: expected Plus(node10), got {:?}", other),
        }
        info!("Validated multi-base annotations across {} nodes", 4);
        log_test_footer(
            "lowers_multi_bases_into_ir_and_preserves_tags_and_default_direction",
            Some(start.elapsed()),
            "Multi-base annotations preserved isa/impl tags and edge weights.",
        );
    }
}
