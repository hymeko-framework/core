use super::constants::*;
use super::helpers::assert_expected_fields;
use crate::test_helpers::{log_test_footer, log_test_header};
use log::info;
use parser::{ast::HyperItem, parse_description};
use std::time::Instant;

#[test]
fn parses_minimal_example_context_fields_with_comments() {
    run_comment_fixture(
        "parses_minimal_example_context_fields_with_comments",
        MINIMAL_COMMENTS_WITH_FIELDS_PATH,
        "Parses block comments interleaved with field definitions.",
    );
}

#[test]
fn parses_minimal_example_context_fields_with_line_comments() {
    run_comment_fixture(
        "parses_minimal_example_context_fields_with_line_comments",
        MINIMAL_COMMENTS_WITH_LINE_PATH,
        "Parses line comments interleaved with field definitions.",
    );
}

#[test]
fn parses_minimal_example_context_fields_with_header_comment() {
    run_comment_fixture(
        "parses_minimal_example_context_fields_with_header_comment",
        MINIMAL_COMMENTS_WITH_HEADER_PATH,
        "Parses header comments preceding the minimal example.",
    );
}

#[test]
fn parses_minimal_example_context_fields_with_bad_comments() {
    log_test_header(
        "parses_minimal_example_context_fields_with_bad_comments",
        "Ensures malformed block comments trip the parser.",
    );
    let start = Instant::now();
    let path = MINIMAL_COMMENTS_WITH_BAD_PATH;
    // This should throw an error
    let source_code = parser::read_source_file(&path).expect("failed to read source file");

    // 2. Parse it, tying the AST lifetimes to the String
    let res = parser::parse_description(&source_code);
    // Expect error
    assert!(res.is_err());
    log_test_footer(
        "parses_minimal_example_context_fields_with_bad_comments",
        Some(start.elapsed()),
        "Malformed file correctly failed to parse.",
    );
}

#[test]
fn parses_minimal_example_context_fields_with_bad_comments_inline() {
    log_test_header(
        "parses_minimal_example_context_fields_with_bad_comments_inline",
        "Ensures unterminated inline comments return an error.",
    );
    let start = Instant::now();
    let res = parse_description(BAD_INLINE_COMMENT_SRC);
    // Expect error
    assert!(res.is_err());
    log_test_footer(
        "parses_minimal_example_context_fields_with_bad_comments_inline",
        Some(start.elapsed()),
        "Inline parse correctly errored on bad comments.",
    );
}

fn run_comment_fixture(title: &str, path: &str, details: &str) {
    log_test_header(title, details);
    let start = Instant::now();
    let source_code = parser::read_source_file(path).expect("failed to read source file");
    let d = parser::parse_description(&source_code).unwrap();

    assert_eq!(d.name, DESC_MINIMAL_EXAMPLE_NAME);
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
    assert_expected_fields(
        ctx_body.as_slice(),
        MINIMAL_WITH_FIELDS_COMMENTS_EXPECTATIONS,
    );
    info!("Validated comment fixture {}", path);
    log_test_footer(
        title,
        Some(start.elapsed()),
        &format!("Parsed {path} and confirmed all comment-related fields."),
    );
}
