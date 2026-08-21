use super::constants::{FieldExpectation, FieldValue};
use crate::test_asserts::test_helpers::{
    assert_list_nums, assert_no_value, assert_num_value, assert_str_value, assert_tags,
};
use hymeko::find_node;
use parser::ast::{HyperItem, Value};

pub fn assert_expected_fields<'a>(
    ctx_body: &'a [HyperItem<'a, &'a str>],
    expectations: &[FieldExpectation],
) {
    for expectation in expectations {
        let node = find_node(ctx_body, expectation.name)
            .unwrap_or_else(|| panic!("Missing field {}", expectation.name));
        assert_tags(node, expectation.tags);
        match (&node.anno.value, &expectation.value) {
            (_, FieldValue::Number(value)) => assert_num_value(node, *value),
            (_, FieldValue::Str(value)) => assert_str_value(node, value),
            (_, FieldValue::NumList(values)) => assert_list_nums(node, values),
            (_, FieldValue::None) => assert_no_value(node),
            (Some(Value::Ref(actual)), FieldValue::Ref(expected_path)) => {
                let expected: Vec<String> =
                    expected_path.iter().map(|seg| (*seg).to_string()).collect();
                assert_eq!(
                    actual.path, expected,
                    "Reference path mismatch for {}",
                    expectation.name
                );
            }
            (other_value, field_value) => {
                panic!(
                    "Unexpected combination for {}: {:?} vs {:?}",
                    expectation.name, other_value, field_value
                );
            }
        }
    }
}
