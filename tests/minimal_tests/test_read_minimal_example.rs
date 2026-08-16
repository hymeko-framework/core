use super::constants::{CONTEXT_NODE_NAME, DESC_MINIMAL_EXAMPLE_NAME, MINIMAL_EXAMPLE_PATH};
use crate::test_helpers::{log_test_footer, log_test_header};
use log::info;
use parser::ast::HyperItem;
use std::time::Instant;

#[test]
fn test_minimal_example_from_file() {
    log_test_header(
        "test_minimal_example_from_file",
        "Loads the minimal example and ensures the context node is present.",
    );
    let start = Instant::now();
    let source_code = parser::read_source_file(MINIMAL_EXAMPLE_PATH)
        .expect("failed to read minimal example file");
    let desc =
        parser::parse_description(&source_code).expect("failed to parse minimal example file");

    assert_eq!(desc.name, DESC_MINIMAL_EXAMPLE_NAME);
    assert_eq!(desc.items.len(), 1, "expected a single top-level item");
    match &desc.items[0] {
        HyperItem::Node(node) => assert_eq!(node.inner.name, CONTEXT_NODE_NAME),
        other => panic!("expected context node, got {:?}", other),
    }
    info!(
        "Parsed {} with {} top-level items",
        MINIMAL_EXAMPLE_PATH,
        desc.items.len()
    );
    log_test_footer(
        "test_minimal_example_from_file",
        Some(start.elapsed()),
        "Successfully read and validated the minimal example.",
    );
}
