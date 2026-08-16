use super::constants::*;
use crate::test_helpers::{log_test_footer, log_test_header};
use log::info;
use parser::ast::*;
use std::time::Instant;

#[test]
fn test_minimal_example() {
    log_test_header(
        "test_minimal_example",
        "Reads the minimal .hymeko example from disk and checks the top-level node.",
    );
    let start = Instant::now();
    let source_code =
        parser::read_source_file(MINIMAL_EXAMPLE_PATH).expect("failed to read source file");

    // 2. Parse it, tying the AST lifetimes to the String
    let d = parser::parse_description(&source_code).unwrap();

    assert_eq!(d.name, DESC_MINIMAL_EXAMPLE_NAME);
    /*
    assert_eq!(d.meta.inner.name, "Minimal_Example");
    assert!(d.meta.inner.body.is_some());

     */

    assert_eq!(d.items.len(), 1);
    match &d.items[0] {
        HyperItem::Node(n) => assert_eq!(n.inner.name, CONTEXT_NODE_NAME),
        _ => panic!("Expected Node(context)"),
    }
    info!(
        "File {} parsed with {} top-level items",
        MINIMAL_EXAMPLE_PATH,
        d.items.len()
    );
    log_test_footer(
        "test_minimal_example",
        Some(start.elapsed()),
        "Verified the context node in minimal_example.hymeko.",
    );
}

#[test]
fn test_minimal_example_base_elements() {
    log_test_header(
        "test_minimal_example_base_elements",
        "Reads the base elements example and validates nodes/arcs.",
    );
    let start = Instant::now();
    let source_code = parser::read_source_file(MINIMAL_EXAMPLE_BASE_ELEMENTS_PATH)
        .expect("failed to read source file");

    // 2. Parse it, tying the AST lifetimes to the String
    let d = parser::parse_description(&source_code).unwrap();

    assert_eq!(d.name, DESC_MY_DESC);

    assert_eq!(d.items.len(), BASE_ITEM_COUNT);

    // Hypernode A
    match &d.items[0] {
        HyperItem::Node(n) => assert_eq!(n.inner.name, NODE_A_NAME),
        _ => panic!("Expected Node(A)"),
    }

    // Hypernode B
    match &d.items[1] {
        HyperItem::Node(n) => assert_eq!(n.inner.name, NODE_B_NAME),
        _ => panic!("Expected Node(B)"),
    }

    // Edge E1: arcs már a body-ban vannak
    match &d.items[2] {
        HyperItem::Edge(e) => {
            assert_eq!(e.inner.name, EDGE_E1_NAME);

            // szedjük ki az arcokat a body-ból
            let arcs: Vec<&HyperArc<&str>> = e
                .inner
                .body
                .iter()
                .filter_map(|it| {
                    if let HyperItem::Arc(a) = it {
                        Some(a)
                    } else {
                        None
                    }
                })
                .collect();

            assert_eq!(arcs.len(), SINGLE_ARC_COUNT);
            assert_eq!(arcs[0].inner.refs.len(), ARC_REF_PAIR_COUNT);
            info!("Edge {} contained {} arc(s)", EDGE_E1_NAME, arcs.len());
        }
        _ => panic!("Expected Edge(E1)"),
    }
    log_test_footer(
        "test_minimal_example_base_elements",
        Some(start.elapsed()),
        "Verified nodes A/B and edge E1 with its arc references.",
    );
}
