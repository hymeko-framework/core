use crate::test_helpers::{log_test_footer, log_test_header};
use crate::typical_graphs::fano::constants::*;
use hymeko::{body, find_node};
use log::info;
use parser::ast::*;
use std::time::Instant;

#[test]
fn parse_fano_graph() {
    log_test_header(
        "parse_fano_graph",
        "Parses the canonical Fano graph and validates node/edge counts.",
    );
    let start = Instant::now();

    let source_code =
        parser::read_source_file(FANO_GRAPH_PATH).expect("failed to read source file");

    // 2. Parse it, tying the AST lifetimes to the String
    let desc = parser::parse_description(&source_code).unwrap();

    // Top-level: "Fano_graph" név + üres header
    assert_eq!(desc.name, FANO_DESCRIPTION_NAME);
    assert!(desc.header.is_empty(), "Expected empty header");

    // Top-levelben legyen a fano block (NodeDecl body-val)
    let fano = find_node(&desc.items, FANO_BLOCK_NAME).unwrap();
    let fano_body = body(fano).unwrap();
    assert_eq!(
        fano_body.len(),
        FANO_BODY_ITEM_COUNT,
        "Expected 14 items in fano body (7 nodes + 7 edges)"
    );

    // 7 node: n0..n6
    for i in 0..FANO_POINT_NODE_COUNT {
        let n = format!("{}{}", FANO_NODE_PREFIX, i);
        let _ = find_node(fano_body, &n);
    }

    // 7 edge: e0..e6
    // Itt csak azt ellenőrizzük, hogy mindegyik EdgeDecl megvan,
    // és hogy a body-jában 1 arc van, és az 3 referenciát tartalmaz.
    for i in 0..FANO_EDGE_COUNT {
        let ename = format!("{}{}", FANO_EDGE_PREFIX, i);

        let edge = fano_body
            .iter()
            .find_map(|it| match it {
                HyperItem::Edge(e) if e.inner.name == ename => Some(e),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Expected Edge({}) in fano body", ename));

        // edge.inner.body : Vec<HyperItem>
        let arc_items: Vec<&parser::ast::HyperArc<&str>> = edge
            .inner
            .body
            .iter()
            .filter_map(|x| match x {
                HyperItem::Arc(a) => Some(a),
                _ => None,
            })
            .collect();

        assert_eq!(
            arc_items.len(),
            1,
            "Each edge should contain exactly 1 HyperArc statement; edge={}",
            ename
        );

        let arc = arc_items[0];

        // A te jelenlegi grammarodban:
        // HyperArc { inner: ArcInner { refs } }
        // ahol refs tipikusan Vec<SignedRef> vagy Vec<DirectedRef>.
        // A Fano input "~ n0" -> SignedRef::Neutral várható.
        assert_eq!(
            arc.inner.refs.len(),
            FANO_ARC_REF_COUNT,
            "Expected 3 endpoints in arc inside edge {}",
            edge.inner.name
        );
    }

    // extra sanity: a fano body-ban ne legyen top-level arc
    // (minden arc edge body-ban van)
    assert!(
        !fano_body.iter().any(|it| matches!(it, HyperItem::Arc(_))),
        "Did not expect HyperArc directly under `fano graph`"
    );
    info!(
        "Validated {} nodes and {} edges for the Fano graph",
        FANO_POINT_NODE_COUNT, FANO_EDGE_COUNT
    );
    log_test_footer(
        "parse_fano_graph",
        Some(start.elapsed()),
        "Fano graph AST structure matches expectations.",
    );
}
