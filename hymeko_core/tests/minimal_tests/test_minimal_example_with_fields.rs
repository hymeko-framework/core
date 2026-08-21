use super::constants::{
    CONTEXT_NODE_NAME, DESC_MINIMAL_EXAMPLE_NAME, MINIMAL_WITH_FIELDS_EXPECTATIONS,
    MINIMAL_WITH_FIELDS_PATH,
};
use super::helpers::assert_expected_fields;
use crate::test_helpers::{log_test_footer, log_test_header};
use log::info;
use parser::ast::*;
use std::time::Instant;

#[test]
fn parses_minimal_example_context_fields() {
    log_test_header(
        "parses_minimal_example_context_fields",
        "Parses the minimal example with fields and validates every annotated node.",
    );
    let start = Instant::now();
    let source_code =
        parser::read_source_file(MINIMAL_WITH_FIELDS_PATH).expect("failed to read source file");

    // 2. Parse it, tying the AST lifetimes to the String
    let d = parser::parse_description(&source_code).unwrap();
    // description name
    assert_eq!(d.name, DESC_MINIMAL_EXAMPLE_NAME);
    // top-level: context node
    assert_eq!(d.items.len(), 1);
    let context = match &d.items[0] {
        HyperItem::Node(n) => n,
        _ => panic!("Expected top-level Node(context)"),
    };
    assert_eq!(context.inner.name, CONTEXT_NODE_NAME);

    let ctx_body = context
        .inner
        .body
        .as_ref()
        .expect("context must have a body");
    assert_expected_fields(ctx_body.as_slice(), MINIMAL_WITH_FIELDS_EXPECTATIONS);
    info!(
        "Validated {} field expectations",
        MINIMAL_WITH_FIELDS_EXPECTATIONS.len()
    );
    log_test_footer(
        "parses_minimal_example_context_fields",
        Some(start.elapsed()),
        "Parsed source and confirmed all expected field annotations.",
    );
}
