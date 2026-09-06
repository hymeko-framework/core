use super::*;
use crate::common::ids::{DeclId, HyperArcId, NodeId, SymId};
use crate::common::pathkey::PathKey;
use crate::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use crate::ir::hash::HashId;
use crate::ir::ir::{AnnoR, ArcRec, DeclKind, DeclNode, Ir, NodeRec, RefAtomR, SignedRefR};
use crate::ir_stream::DecodeLimits;
use crate::module_store::module_store::{CompiledProgram, ModuleKey};
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;
use std::path::PathBuf;

fn fixture() -> (Ir, Index, Interner) {
    let mut interner = Interner::new();
    let charge = interner.intern("charge");

    let mut ir = Ir::default();
    let decl = ir.push_decl(DeclNode {
        kind: DeclKind::Node,
        name: charge,
        parent: DeclId::NONE,
        first_child: DeclId::NONE,
        last_child: DeclId::NONE,
        next_sibling: DeclId::NONE,
        anno: AnnoR::default(),
    });
    ir.decl_hash.push(None);
    ir.nodes.push(NodeRec::new(decl, Vec::new()));
    ir.bind_node_decl(decl, NodeId::new(0)).unwrap();

    let mut index = Index::new();
    index.by_path.insert(PathKey(vec![charge]), decl);
    (ir, index, interner)
}

fn encoded_fixture() -> Vec<u8> {
    let (ir, index, interner) = fixture();
    encode("models/charge.hymeko", &ir, &index, &interner, &[]).unwrap()
}

#[test]
fn facade_round_trips_usable_ir_interner_and_hash() {
    let bytes = encoded_fixture();
    let decoded = decode(&bytes, &DecodeLimits::strict()).unwrap();

    assert_eq!(decoded.payload().schema_version, CBOR_SCHEMA_VERSION);
    assert_eq!(
        decoded.payload().root_path,
        PathBuf::from("models/charge.hymeko")
    );
    assert_eq!(decoded.ir().decl_nodes.len(), 1);
    assert_eq!(decoded.index().by_path.len(), 1);
    assert_eq!(decoded.interner().resolve(SymId::new(0)), "charge");
    assert_eq!(
        decoded.canonical_hash(),
        canonical_program_hash(CanonHashCfg::default(), decoded.ir(), decoded.interner())
    );
}

#[test]
fn compiled_program_facade_makes_paths_workspace_relative() {
    let workspace = PathBuf::from("workspace");
    let (ir, index, interner) = fixture();
    let program = CompiledProgram {
        root: ModuleKey(workspace.join("models/charge.hymeko")),
        idx: index,
        ir,
        imports: vec![(
            SymId::new(0),
            ModuleKey(workspace.join("library/electrical.hymeko")),
        )],
        canon_hash: HashId([0; 32]),
    };

    let bytes = encode_compiled(&workspace, &program, &interner).unwrap();
    let decoded = decode(&bytes, &DecodeLimits::strict()).unwrap();

    assert_eq!(
        decoded.payload().root_path,
        PathBuf::from("models/charge.hymeko")
    );
    assert_eq!(
        decoded.payload().imports[0].1.0,
        PathBuf::from("library/electrical.hymeko")
    );
}

#[test]
fn decode_rejects_a_tampered_canonical_hash() {
    let bytes = encoded_fixture();
    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    payload.canon_hash = HashId([0x5a; 32]);
    let mut tampered = Vec::new();
    ciborium::ser::into_writer(&payload, &mut tampered).unwrap();

    assert!(matches!(
        decode(&tampered, &DecodeLimits::strict()),
        Err(CborError::CanonicalHashMismatch { .. })
    ));
}

#[test]
fn decode_requires_the_explicit_supported_schema() {
    let bytes = encoded_fixture();
    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    payload.schema_version = CBOR_SCHEMA_VERSION + 1;
    let mut unsupported = Vec::new();
    ciborium::ser::into_writer(&payload, &mut unsupported).unwrap();
    assert!(matches!(
        decode(&unsupported, &DecodeLimits::strict()),
        Err(CborError::UnsupportedSchema { .. })
    ));

    let mut value: ciborium::Value = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    let ciborium::Value::Map(entries) = &mut value else {
        panic!("payload must encode as a CBOR map");
    };
    entries.retain(|(key, _)| key.as_text() != Some("schema_version"));
    let mut missing = Vec::new();
    ciborium::ser::into_writer(&value, &mut missing).unwrap();
    assert!(matches!(
        decode(&missing, &DecodeLimits::strict()),
        Err(CborError::InvalidCbor(_))
    ));
}

#[test]
fn decode_rejects_a_tampered_index() {
    let bytes = encoded_fixture();
    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    payload.index.by_path.clear();
    let mut tampered = Vec::new();
    ciborium::ser::into_writer(&payload, &mut tampered).unwrap();

    assert!(matches!(
        decode(&tampered, &DecodeLimits::strict()),
        Err(CborError::IncoherentIr { .. } | CborError::IndexMismatch)
    ));
}

#[test]
fn decode_rejects_an_alias_to_a_missing_path() {
    let bytes = encoded_fixture();
    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    payload
        .index
        .aliases
        .insert(SymId::new(0), vec![SymId::new(0), SymId::new(0)]);
    let mut tampered = Vec::new();
    ciborium::ser::into_writer(&payload, &mut tampered).unwrap();

    assert!(matches!(
        decode(&tampered, &DecodeLimits::strict()),
        Err(CborError::IndexMismatch)
    ));
}

#[test]
fn decode_rejects_an_out_of_range_namespace_symbol_before_hashing() {
    let bytes = encoded_fixture();
    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    payload
        .ir
        .record_namespace_root(DeclId::new(0), &[SymId::new(99)]);
    let mut tampered = Vec::new();
    ciborium::ser::into_writer(&payload, &mut tampered).unwrap();

    assert!(matches!(
        decode(&tampered, &DecodeLimits::strict()),
        Err(CborError::SymbolOutOfRange {
            field: "namespace prefix symbol",
            ..
        })
    ));
}

#[test]
fn preflight_rejects_oversize_before_deserialization() {
    let limits = DecodeLimits {
        max_message_bytes: 16,
        max_payload_bytes: 16,
        max_section_count: 4,
        max_section_bytes: 8,
        max_elements_per_section: 4,
    };
    let claims_u64_max_length = [0x7b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

    assert!(matches!(
        decode(&claims_u64_max_length, &limits),
        Err(CborError::LimitExceeded {
            what: "byte/text string bytes",
            ..
        })
    ));
}

#[test]
fn codec_rejects_positions_that_do_not_fit_canonical_hash_v1() {
    let bytes = encoded_fixture();
    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    payload.ir.edges.push(crate::ir::ir::EdgeRec {
        decl: DeclId::new(0),
        bases: Vec::new(),
        arcs: vec![HyperArcId::new(0); usize::from(u16::MAX) + 2],
    });

    assert!(matches!(
        validate::payload(payload, false),
        Err(CborError::LimitExceeded {
            what: "arcs per edge in canonical hash v1",
            ..
        })
    ));

    let mut payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    let reference = SignedRefR::Neutral(RefAtomR {
        target: DeclId::new(0),
        weights: None,
        anno: AnnoR::default(),
    });
    payload.ir.arcs.push(ArcRec {
        anno: AnnoR::default(),
        in_edge: DeclId::new(0),
        refs: vec![reference; usize::from(u16::MAX) + 2],
    });
    assert!(matches!(
        validate::payload(payload, false),
        Err(CborError::LimitExceeded {
            what: "references per arc in canonical hash v1",
            ..
        })
    ));
}

#[test]
fn encode_rejects_absolute_and_parent_traversing_paths() {
    let (ir, index, interner) = fixture();
    let parent = encode("../escape.hymeko", &ir, &index, &interner, &[]);
    assert!(matches!(parent, Err(CborError::NonPortablePath { .. })));

    let absolute_path = std::env::current_dir().unwrap().join("model.hymeko");
    let absolute = encode(absolute_path, &ir, &index, &interner, &[]);
    assert!(matches!(absolute, Err(CborError::NonPortablePath { .. })));
}

#[test]
fn paths_have_one_platform_independent_wire_spelling() {
    let (ir, index, interner) = fixture();
    let native_path = PathBuf::from("models").join("charge.hymeko");
    let bytes = encode(native_path, &ir, &index, &interner, &[]).unwrap();
    let payload: CborPayload = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    assert_eq!(payload.root_path.to_str(), Some("models/charge.hymeko"));

    let mut non_portable = payload;
    non_portable.root_path = PathBuf::from(r"models\charge.hymeko");
    let mut tampered = Vec::new();
    ciborium::ser::into_writer(&non_portable, &mut tampered).unwrap();
    assert!(matches!(
        decode(&tampered, &DecodeLimits::strict()),
        Err(CborError::NonPortablePath { .. })
    ));
}

#[test]
fn paths_reject_drive_like_and_empty_segments() {
    let (ir, index, interner) = fixture();
    for path in ["C:/model.hymeko", "models//charge.hymeko"] {
        let result = encode(path, &ir, &index, &interner, &[]);
        assert!(matches!(result, Err(CborError::NonPortablePath { .. })));
    }
}

#[test]
fn every_truncated_prefix_is_rejected() {
    let bytes = encoded_fixture();
    for end in 0..bytes.len() {
        assert!(
            decode(&bytes[..end], &DecodeLimits::strict()).is_err(),
            "prefix of {end} bytes decoded successfully"
        );
    }
}

#[test]
fn preflight_rejects_indefinite_items_and_trailing_bytes() {
    assert!(matches!(
        decode(&[0x9f, 0xff], &DecodeLimits::strict()),
        Err(CborError::InvalidCbor(_))
    ));
    assert!(matches!(
        decode(&[0x00, 0x00], &DecodeLimits::strict()),
        Err(CborError::InvalidCbor(_))
    ));
}
