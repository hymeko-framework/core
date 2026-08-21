//! HyMeKo core: AST, IR, tensors, and module store.
//!
//! **APPROVED-CORE-EDIT: core-manifest-and-hymeko-core-clippy** (2026-05-11).
//! The lint allow-list below is deliberate staging debt so
//! `cargo clippy -p hymeko_core -- -D warnings` matches CI policy without a
//! single mega-refactor. Remove allows only with targeted fixes plus tests.

#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    non_snake_case,
    clippy::assign_op_pattern,
    clippy::collapsible_if,
    clippy::empty_line_after_doc_comments,
    clippy::extra_unused_lifetimes,
    clippy::io_other_error,
    clippy::manual_rotate,
    clippy::module_inception,
    clippy::multiple_bound_locations,
    clippy::needless_borrow,
    clippy::needless_range_loop,
    clippy::new_without_default,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_cast,
    clippy::unnecessary_sort_by,
    clippy::wrong_self_convention
)]

use crate::common::ids::SymId;
use parser::ast::{EdgeDecl, HyperItem, NodeDecl};

pub mod common;
pub mod generators;
pub mod ir;
pub mod module_store;
pub mod resolution;
pub mod sym_ast;
pub mod tensor;
pub mod util;
pub mod writers;

pub fn find_node<'ast, 'slice>(
    items: &'slice [HyperItem<'ast, &'ast str>],
    name: &str,
) -> Option<&'slice NodeDecl<'ast, &'ast str>> {
    items.iter().find_map(|it| match it {
        HyperItem::Node(n) if n.inner.name == name => Some(n),
        _ => None,
    })
}

pub fn as_node<'ast, 'slice, Id>(
    it: &'slice HyperItem<'ast, Id>,
) -> Option<&'slice NodeDecl<'ast, Id>> {
    match it {
        HyperItem::Node(n) => Some(n),
        _ => None,
    }
}

pub fn body<'ast, 'slice>(
    n: &'slice NodeDecl<'ast, &'ast str>,
) -> Option<&'slice [HyperItem<'ast, &'ast str>]> {
    n.inner.body.as_deref()
}

pub fn find_edge<'ast, 'slice>(
    items: &'slice [HyperItem<'ast, SymId>],
    name: SymId,
) -> Option<&'slice EdgeDecl<'ast, SymId>> {
    items.iter().find_map(|it| match it {
        HyperItem::Edge(e) if e.inner.name == name => Some(e),
        _ => None,
    })
}

pub fn find_node_id<'ast, 'slice>(
    items: &'slice [HyperItem<'ast, SymId>],
    name: SymId,
) -> Option<&'slice NodeDecl<'ast, SymId>> {
    items.iter().find_map(|it| match it {
        HyperItem::Node(n) if n.inner.name == name => Some(n),
        _ => None,
    })
}
