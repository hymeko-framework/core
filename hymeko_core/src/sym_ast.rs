use crate::common::ids::SymId;
use parser::ast::Description;

pub type AstSym<'a> = Description<'a, SymId>;
