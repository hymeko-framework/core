use super::constants::*;
use crate::test_helpers::{log_test_footer, log_test_header};
use log::info;
use parser::ast::*;
use parser::parse_description;
use std::time::Instant;

fn must_parse<'a>(input: &'a str) -> AstStr<'a> {
    parse_description(input).unwrap()
}

// EdgeArcs now filters based on the reference lifetime
fn edge_arcs<'ast, 'slice>(
    e: &'slice EdgeDecl<'ast, &'ast str>,
) -> Vec<&'slice HyperArc<'ast, &'ast str>> {
    e.inner
        .body
        .iter()
        .filter_map(|it| match it {
            HyperItem::Arc(a) => Some(a),
            _ => None,
        })
        .collect()
}

#[test]
fn parses_minimal_description() {
    log_test_header(
        "parses_minimal_description",
        "Confirms the base minimal example parses and contains the expected items.",
    );
    let start = Instant::now();
    let d = must_parse(PARSE_DESC_SRC);

    assert_eq!(d.name, DESC_MY_DESC);
    assert_eq!(d.items.len(), BASE_ITEM_COUNT);

    match &d.items[0] {
        HyperItem::Node(n) => assert_eq!(n.inner.name, NODE_A_NAME),
        _ => panic!("Expected Node(A)"),
    }
    info!("Parsed minimal description with {} items", d.items.len());
    log_test_footer(
        "parses_minimal_description",
        Some(start.elapsed()),
        "Validated minimal description top-level elements.",
    );
}

#[test]
fn parses_multiple_arcs_in_one_edge() {
    log_test_header(
        "parses_multiple_arcs_in_one_edge",
        "Ensures arcs can repeat within a single edge declaration.",
    );
    let start = Instant::now();
    let d = must_parse(MULTI_ARC_DESC_SRC);

    // items: A, B, C, E1
    let e1 = match &d.items[MULTI_ARC_EDGE_INDEX] {
        HyperItem::Edge(e) => e,
        _ => panic!("Expected Edge at items[3]"),
    };

    let arcs = edge_arcs(e1);
    assert_eq!(arcs.len(), MULTI_ARC_COUNT);
    info!("Edge E1 contained {} arcs", arcs.len());
    log_test_footer(
        "parses_multiple_arcs_in_one_edge",
        Some(start.elapsed()),
        "Confirmed arc count for E1.",
    );
}

#[test]
fn fails_if_arc_missing_semicolon() {
    log_test_header(
        "fails_if_arc_missing_semicolon",
        "Verifies the parser rejects arcs missing trailing semicolons.",
    );
    let start = Instant::now();
    let err = parse_description(MISSING_SEMI_DESC_SRC).unwrap_err();

    let _ = err;
    log_test_footer(
        "fails_if_arc_missing_semicolon",
        Some(start.elapsed()),
        "Parser returned an error for the malformed arc as expected.",
    );
}
