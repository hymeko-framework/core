use hymeko::module_store::module_store::HymekoParser;
use parser::ast::AstStr;
use parser::parse_description;

pub mod constants;
pub mod helpers;
pub mod test_minimal_example;
pub mod test_minimal_example_with_fields;

pub mod test_minimal_example_fileread;

mod edges;
mod test_minimal_example_basic_hierarchy;
mod test_minimal_example_comments;
mod test_minimal_example_field_refs;
pub mod test_read_minimal_example;
mod test_smoke_test;

mod annotations;
mod test_import;
mod test_module_store;

struct TestParser;

impl HymekoParser for TestParser {
    fn parse<'a>(&self, src: &'a str) -> Result<AstStr<'a>, String> {
        parse_description(src).map_err(|e| format!("{e:?}"))
    }
}
