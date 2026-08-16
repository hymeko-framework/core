use super::constants::{
    NODE_A_NAME, NODE_B_NAME, SMOKE_ARC_WEIGHT, SMOKE_EDGE_NAME, SMOKE_NODE_NAME,
};
use crate::test_helpers::{log_test_footer, log_test_header};
use log::info;
use parser::ast::*;
use std::time::Instant;

fn ra(name: &str) -> RefAtom<'static, String> {
    RefAtom {
        target: Ref {
            path: vec![name.to_string()],
        },
        anno: Anno {
            tags: vec![],
            value: None,
        },
    }
}

#[test]
fn ast_variants_are_constructible() {
    log_test_header(
        "ast_variants_are_constructible",
        "Constructs node/edge/arc HyperItems to ensure smoke-level API stability.",
    );
    let start = Instant::now();
    let node_item = HyperItem::Node(HyperAnnotatedElement {
        anno: Anno {
            tags: Vec::new(),
            value: None,
        },
        inner: NodeInner {
            name: SMOKE_NODE_NAME.to_string(),
            bases: vec![],
            body: None,
        },
    });
    match node_item {
        HyperItem::Node(node) => {
            assert_eq!(node.inner.name, SMOKE_NODE_NAME);
            assert!(node.inner.bases.is_empty());
            assert!(node.inner.body.is_none());
        }
        _ => panic!("expected HyperItem::Node"),
    }

    let edge_item = HyperItem::Edge(HyperAnnotatedElement {
        anno: Anno {
            tags: Vec::new(),
            value: None,
        },
        inner: EdgeInner {
            name: SMOKE_EDGE_NAME.to_string(),
            bases: vec![],
            body: Vec::new(),
        },
    });
    match edge_item {
        HyperItem::Edge(edge) => {
            assert_eq!(edge.inner.name, SMOKE_EDGE_NAME);
            assert!(edge.inner.body.is_empty());
        }
        _ => panic!("expected HyperItem::Edge"),
    }

    let arc_item = HyperItem::Arc(HyperAnnotatedElement {
        anno: Anno {
            tags: Vec::new(),
            value: Some(Value::Num(SMOKE_ARC_WEIGHT)),
        },
        inner: ArcInner {
            refs: vec![
                SignedRef::Plus(ra(NODE_A_NAME)),
                SignedRef::Minus(ra(NODE_B_NAME)),
            ],
        },
    });
    match arc_item {
        HyperItem::Arc(arc) => {
            assert_eq!(arc.inner.refs.len(), 2);
            assert!(
                matches!(arc.anno.value, Some(Value::Num(w)) if (w - SMOKE_ARC_WEIGHT).abs() < 1e-9)
            );

            match &arc.inner.refs[0] {
                SignedRef::Plus(atom) => {
                    assert_eq!(atom.target.path, vec![NODE_A_NAME.to_string()])
                }
                other => panic!("expected plus ref to {}, got {:?}", NODE_A_NAME, other),
            }
            match &arc.inner.refs[1] {
                SignedRef::Minus(atom) => {
                    assert_eq!(atom.target.path, vec![NODE_B_NAME.to_string()])
                }
                other => panic!("expected minus ref to {}, got {:?}", NODE_B_NAME, other),
            }
        }
        _ => panic!("expected HyperItem::Arc"),
    }
    info!(
        "Constructed node '{}', edge '{}', and arc with weight {:.2}",
        SMOKE_NODE_NAME, SMOKE_EDGE_NAME, SMOKE_ARC_WEIGHT
    );
    log_test_footer(
        "ast_variants_are_constructible",
        Some(start.elapsed()),
        "Verified HyperItem variants remain constructible in tests.",
    );
}
