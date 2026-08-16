use super::constants::*;
use super::helpers::assert_expected_fields;
use crate::test_helpers::{log_test_footer, log_test_header};
use hymeko::body;
use log::info;
use parser::ast::*;
use std::time::Instant;

#[test]
fn parses_legacy_context_with_refs() {
    run_field_ref_fixture(
        "parses_legacy_context_with_refs",
        MINIMAL_FIELDS_REF_PATH,
        "Validates reference nodes in the original minimal example.",
    );
}

#[test]
fn parses_legacy_context_with_refs_alternative() {
    run_field_ref_fixture(
        "parses_legacy_context_with_refs_alternative",
        MINIMAL_FIELDS_REF_ALT_PATH,
        "Validates reference nodes in the alternate minimal example.",
    );
}

fn run_field_ref_fixture(title: &str, path: &str, details: &str) {
    log_test_header(title, details);
    let start = Instant::now();
    let source_code = parser::read_source_file(path).expect("failed to read source file");
    let d = parser::parse_description(&source_code).unwrap();

    assert_eq!(d.name, DESC_MINIMAL_EXAMPLE_NAME);
    assert_eq!(d.items.len(), 1);
    let context = match &d.items[0] {
        HyperItem::Node(n) => n,
        _ => panic!("Expected Node(context)"),
    };
    assert_eq!(context.inner.name, CONTEXT_NODE_NAME);

    let ctx = body(context).unwrap();
    assert_expected_fields(ctx, FIELD_REF_EXPECTATIONS);
    info!(
        "Verified {} reference fields for {path}",
        FIELD_REF_EXPECTATIONS.len()
    );
    log_test_footer(
        title,
        Some(start.elapsed()),
        &format!("Parsed {path} and confirmed all reference lookups."),
    );
}
