//! Compact-declaration-dispatch serde boundary — known-answer wire and
//! fail-closed decoding.
//!
//! APPROVED-CORE-EDIT: compact-declaration-dispatch (2026-08-31).
//!
//! The in-memory `Ir` stores one compact `decl_to_record: Vec<u64>` slot per
//! declaration, but the serde wire keeps the legacy three parallel maps
//! (`decl_to_node`/`decl_to_edge`/`decl_to_arc`) byte-for-byte. These tests pin
//! that wire against a frozen fixture (independently of the decoder) and prove
//! every structural inconsistency is rejected with a typed, deterministic error
//! rather than silently reinterpreted.

use hymeko::common::ids::DeclId;
use hymeko::ir::ir::Ir;
use serde_json::{Value, json};

use crate::unified_ir::support::builder::{S, TestIr};

/// A small, deterministic fixture: the builder assigns dense ids in insertion
/// order (nodes `x`,`y` = decls 0,1; edge `e` = decl 2; its arc = decl 3), so
/// the wire is fixed.
fn fixture() -> TestIr {
    let mut t = TestIr::new();
    let x = t.node(&["m", "x"], DeclId::NONE);
    let y = t.node(&["m", "y"], DeclId::NONE);
    let e = t.edge(&["m", "e"], DeclId::NONE);
    t.arc(e, &[(x, S::Plus), (y, S::Minus)]);
    t.attach_arc_decls();
    t
}

fn wire(t: &TestIr) -> Value {
    serde_json::from_str(&serde_json::to_string(&t.ir).expect("Ir serializes")).unwrap()
}

/// Deserialize a (possibly corrupted) wire and return the error text, asserting
/// it is rejected rather than accepted.
fn decode_err(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap();
    serde_json::from_str::<Ir>(&s)
        .expect_err("malformed wire must be rejected")
        .to_string()
}

// --------------------------------------------------------------------------
// Known-answer: the encoder reproduces the legacy three-map layout exactly.
// Compared against frozen literals *without* invoking the decoder.
// --------------------------------------------------------------------------

#[test]
fn wire_is_byte_frozen_and_carries_the_legacy_three_maps() {
    let v = wire(&fixture());
    assert_eq!(v["decl_to_node"], json!([0, 1, null, null]));
    assert_eq!(v["decl_to_edge"], json!([null, null, 0, null]));
    assert_eq!(v["decl_to_arc"], json!([null, null, null, 0]));
    // Field is present under its legacy name; the compact slot never leaks.
    assert!(
        v.get("decl_to_record").is_none(),
        "compact slot must not reach the wire"
    );
}

#[test]
fn round_trip_reconstructs_the_compact_slots() {
    let t = fixture();
    let s = serde_json::to_string(&t.ir).unwrap();
    let ir: Ir = serde_json::from_str(&s).expect("Ir deserializes");
    use hymeko::common::ids::{EdgeId, HyperArcId, NodeId};
    assert_eq!(ir.as_node(DeclId::new(0)), Some(NodeId::new(0)));
    assert_eq!(ir.as_node(DeclId::new(1)), Some(NodeId::new(1)));
    assert_eq!(ir.as_edge(DeclId::new(2)), Some(EdgeId::new(0)));
    assert_eq!(ir.as_arc(DeclId::new(3)), Some(HyperArcId::new(0)));
    // Cross-tag queries stay absent.
    assert_eq!(ir.as_edge(DeclId::new(0)), None);
    assert_eq!(ir.as_node(DeclId::new(2)), None);
}

// --------------------------------------------------------------------------
// Fail-closed: each structural inconsistency in the legacy wire is a typed,
// deterministic rejection. Corruptions are surgical mutations of a valid wire.
// --------------------------------------------------------------------------

#[test]
fn invalid_map_length_is_rejected() {
    let mut v = wire(&fixture());
    v["decl_to_node"] = json!([0, 1, null]); // 3 != 4 declarations
    assert!(decode_err(&v).contains("legacy map length mismatch"));
}

#[test]
fn a_declaration_bound_in_two_maps_is_rejected() {
    let mut v = wire(&fixture());
    v["decl_to_edge"] = json!([0, null, 0, null]); // decl 0 in node AND edge maps
    assert!(decode_err(&v).contains("bound in more than one legacy map"));
}

#[test]
fn a_binding_disagreeing_with_the_declaration_kind_is_rejected() {
    let mut v = wire(&fixture());
    v["decl_to_node"] = json!([null, 1, null, null]); // decl 0 (a Node) ...
    v["decl_to_edge"] = json!([0, null, 0, null]); // ... bound only as an Edge
    assert!(decode_err(&v).contains("disagrees with kind"));
}

#[test]
fn an_out_of_range_record_index_is_rejected() {
    let mut v = wire(&fixture());
    v["decl_to_node"] = json!([999, 1, null, null]); // node table has length 2
    assert!(decode_err(&v).contains("out-of-range record"));
}

#[test]
fn a_wrong_backlink_is_rejected() {
    let mut v = wire(&fixture());
    v["decl_to_node"] = json!([1, 1, null, null]); // decl 0 -> node 1 (belongs to decl 1)
    assert!(decode_err(&v).contains("back-pointer does not match"));
}

/// A two-edge fixture, for the edge-side wrong-backlink branch that needs a
/// second edge record to point at.
fn fixture_two_edges() -> TestIr {
    let mut t = TestIr::new();
    let _x = t.node(&["m", "x"], DeclId::NONE); // decl 0, node 0
    let _e = t.edge(&["m", "e"], DeclId::NONE); // decl 1, edge 0
    let _f = t.edge(&["m", "f"], DeclId::NONE); // decl 2, edge 1
    t
}

#[test]
fn an_out_of_range_edge_record_is_rejected() {
    let mut v = wire(&fixture());
    v["decl_to_edge"] = json!([null, null, 999, null]); // decl 2 (edge) past the table
    assert!(decode_err(&v).contains("out-of-range record"));
}

#[test]
fn an_out_of_range_arc_record_is_rejected() {
    let mut v = wire(&fixture());
    v["decl_to_arc"] = json!([null, null, null, 999]); // decl 3 (arc) past the table
    assert!(decode_err(&v).contains("out-of-range record"));
}

#[test]
fn an_edge_bound_to_a_foreign_record_is_rejected() {
    let mut v = wire(&fixture_two_edges());
    // decl 1 (edge, record 0) rebound to edge record 1, whose backlink is decl 2.
    v["decl_to_edge"] = json!([null, 1, 1]);
    assert!(decode_err(&v).contains("back-pointer does not match"));
}

#[test]
fn a_declaration_bound_in_no_map_decodes_to_an_absent_slot() {
    use hymeko::common::ids::NodeId;
    let mut v = wire(&fixture());
    v["decl_to_node"] = json!([null, 1, null, null]); // decl 0 unbound in every map
    let s = serde_json::to_string(&v).unwrap();
    let ir: Ir = serde_json::from_str(&s).expect("an unbound declaration is a valid partial state");
    assert_eq!(ir.as_node(DeclId::new(0)), None);
    assert_eq!(ir.as_node(DeclId::new(1)), Some(NodeId::new(1)));
}
