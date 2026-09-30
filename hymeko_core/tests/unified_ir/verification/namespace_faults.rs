//! U2.1 Part F — namespace-root fault injection.
//!
//! Every malformation of the namespace metadata is rejected with an exact typed
//! error, on both the serde-deserialize path (corrupt wire) and the
//! lowering-boundary validator (Index vs Ir disagreement). Each case mutates a
//! freshly-serialized valid imported Ir, so the baseline is restored per test.

use hymeko::common::ids::DeclId;
use hymeko::ir::canonical_hash::{NamespaceError, validate_namespace_roots};
use hymeko::ir::ir::Ir;
use hymeko::resolution::resolve::Index;
use serde_json::{Value, json};

/// The imported reference fixture (mini_literals): root DeclId 16 carries the
/// namespace prefix [SymId 20] (`meta_kinematics`).
fn imported() -> (Index, Ir, String) {
    let files = crate::unified_ir::support::corpus::require_fixtures();
    let f = files
        .iter()
        .find(|f| f.file_name().unwrap().to_string_lossy() == "mini_literals.hymeko")
        .expect("mini_literals present");
    let (_store, compiled) = crate::test_helpers::load_and_lower(f).expect("lowers");
    let json = serde_json::to_string(&compiled.ir).expect("serializes");
    (compiled.idx.clone(), compiled.ir.clone(), json)
}

fn wire(json: &str) -> Value {
    serde_json::from_str(json).unwrap()
}
fn decode_err(v: &Value) -> String {
    serde_json::from_str::<Ir>(&serde_json::to_string(v).unwrap())
        .expect_err("malformed namespace wire must be rejected")
        .to_string()
}

// ---- serde-path typed errors -----------------------------------------------

#[test]
fn unknown_schema_version_is_rejected() {
    let (_i, _ir, json) = imported();
    let mut v = wire(&json);
    v["namespace_roots"]["schema"] = json!(99);
    assert!(decode_err(&v).contains("unknown namespace metadata schema"));
}

#[test]
fn dangling_namespace_root_is_rejected() {
    let (_i, _ir, json) = imported();
    let mut v = wire(&json);
    v["namespace_roots"]["roots"] = json!([[999999, [20]]]);
    assert!(decode_err(&v).contains("out of range"));
}

#[test]
fn duplicate_namespace_root_is_rejected() {
    let (_i, _ir, json) = imported();
    let mut v = wire(&json);
    v["namespace_roots"]["roots"] = json!([[16, [20]], [16, [20]]]);
    assert!(decode_err(&v).contains("appears more than once"));
}

#[test]
fn empty_prefix_is_rejected() {
    let (_i, _ir, json) = imported();
    let mut v = wire(&json);
    v["namespace_roots"]["roots"] = json!([[16, []]]);
    assert!(decode_err(&v).contains("empty prefix"));
}

#[test]
fn unsorted_namespace_roots_are_rejected() {
    let (_i, _ir, json) = imported();
    let mut v = wire(&json);
    // descending DeclIds (18 before 16), both in range
    v["namespace_roots"]["roots"] = json!([[18, [1]], [16, [20]]]);
    assert!(decode_err(&v).contains("not sorted"));
}

// ---- lowering-boundary validator typed errors ------------------------------

#[test]
fn missing_namespace_metadata_is_detected() {
    let (idx, _ir, json) = imported();
    // strip the namespace metadata: the imported root then has an Index prefix
    // but no recorded namespace -> an incomplete Ir.
    let mut v = wire(&json);
    v.as_object_mut().unwrap().remove("namespace_roots");
    let stripped: Ir = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
    match validate_namespace_roots(&stripped, &idx) {
        Err(NamespaceError::MissingNamespaceMetadata { root }) => {
            assert_eq!(root, DeclId::new(16));
        }
        other => panic!("expected MissingNamespaceMetadata, got {other:?}"),
    }
}

#[test]
fn wrong_prefix_symbol_is_a_path_mismatch() {
    let (idx, _ir, json) = imported();
    let mut v = wire(&json);
    v["namespace_roots"]["roots"] = json!([[16, [1]]]); // wrong prefix symbol
    let corrupt: Ir = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
    match validate_namespace_roots(&corrupt, &idx) {
        Err(NamespaceError::PathMismatch { decl }) => assert_eq!(decl, DeclId::new(16)),
        other => panic!("expected PathMismatch, got {other:?}"),
    }
}

#[test]
fn a_correct_imported_ir_validates_clean() {
    let (idx, ir, _json) = imported();
    assert!(validate_namespace_roots(&ir, &idx).is_ok());
}
