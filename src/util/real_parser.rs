use crate::module_store::module_store::HymekoParser;
use parser::ast::AstStr;
use parser::parse_description;

// ----------------------
// Parser adapter (LALRPOP + Lexer)
// ----------------------
pub struct RealParser;

impl HymekoParser for RealParser {
    fn parse<'a>(&self, src: &'a str) -> Result<AstStr<'a>, String> {
        // Delegate to the hardware-optimized pipeline
        parse_description(src).map_err(|e| format!("{e:?}"))
    }
}
