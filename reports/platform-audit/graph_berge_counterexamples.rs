//! External audit reproducer; link the unmodified Graph Berge implementation.
#[path = "../../../graph/hymeko_graph/tests/direct_ir_common/mod.rs"]
mod dc;
use hymeko::ir::incidence::build_incidence_index;
use hymeko_graph::direct_ir::{BudgetKind, EnumBudget, enumerate_berge_paths};

#[test]
fn zero_result_budget_emits_nothing() {
    let w = dc::compile("vv {}\ng { a {} b {} @ab { (+ a, + b); } }\n");
    let idx = build_incidence_index(w.ir()).unwrap();
    let out = enumerate_berge_paths(&idx, w.node("a"), Some(w.node("b")), EnumBudget::new(4, 0, 100)).unwrap();
    eprintln!("zero budget: items={}, bound={:?}", out.items.len(), out.bound);
    assert_eq!(out.items.len(), 0);
    assert_eq!(out.bound, Some(BudgetKind::Results));
}

#[test]
fn depth_truncation_is_reported() {
    let w = dc::compile("vv {}\ng { a {} b {} c {} @ab { (+ a, + b); } @bc { (+ b, + c); } }\n");
    let idx = build_incidence_index(w.ir()).unwrap();
    let out = enumerate_berge_paths(&idx, w.node("a"), Some(w.node("c")), EnumBudget::new(1, 100, 100)).unwrap();
    eprintln!("depth cap: items={}, bound={:?}", out.items.len(), out.bound);
    // The only a->c path has two hyperedges; depth one truncates its frontier.
    assert!(out.items.is_empty());
    assert_eq!(out.bound, Some(BudgetKind::Depth));
}
