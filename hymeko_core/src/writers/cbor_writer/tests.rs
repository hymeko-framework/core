use super::*;
use crate::common::ids::{DeclId, HyperArcId, NodeId, SymId};
use crate::common::pathkey::PathKey;
use crate::ir::canonical_hash::{Canon, CanonHashCfg, CanonicalIdentity};
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
    let decl = ir.push_decl(DeclNode::root(DeclKind::Node, charge, AnnoR::default()));
    ir.nodes
        .push(NodeRec::new(decl, crate::common::ids::BaseListId::NONE));
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
fn symbol_lifetime_cbor_omits_unreferenced_history_before_wire_limits() {
    let (ir, index, mut interner) = fixture();
    for i in 0..65536 {
        interner.intern(&format!("unused-timestamp-{i}"));
    }
    let bytes = encode("models/charge.hymeko", &ir, &index, &interner, &[]).unwrap();
    let decoded = decode(&bytes, &DecodeLimits::strict())
        .expect("unused symbol history must not exhaust a valid snapshot's array limit");
    assert_eq!(decoded.payload().interned_strings, vec!["charge"]);
    assert_eq!(
        interner.iter().count(),
        65537,
        "export must not mutate its source interner"
    );
}

#[test]
fn symbol_lifetime_admitted_old_payload_discards_unused_strings_but_still_checks_duplicates() {
    let mut payload: CborPayload = ciborium::de::from_reader(encoded_fixture().as_slice()).unwrap();
    let hash = payload.canon_hash;
    payload
        .interned_strings
        .push("historical-unused-value".into());
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&payload, &mut bytes).unwrap();
    let decoded = decode(&bytes, &DecodeLimits::strict()).unwrap();
    assert_eq!(decoded.payload().interned_strings, vec!["charge"]);
    assert_eq!(decoded.canonical_hash(), hash);
    payload.interned_strings.push("charge".into());
    bytes.clear();
    ciborium::ser::into_writer(&payload, &mut bytes).unwrap();
    assert!(matches!(
        decode(&bytes, &DecodeLimits::strict()),
        Err(CborError::DuplicateInternedString { .. })
    ));
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
        Canon::new(decoded.ir(), decoded.interner())
            .structure(CanonHashCfg::default())
            .0
    );
}

#[test]
fn compiled_program_facade_makes_paths_workspace_relative() {
    let workspace = PathBuf::from("workspace");
    let (ir, index, interner) = fixture();
    let canon = CanonicalIdentity::of(&Canon::new(&ir, &interner), CanonHashCfg::default());
    let program = CompiledProgram {
        root: ModuleKey(workspace.join("models/charge.hymeko")),
        idx: index.into(),
        ir,
        imports: vec![(
            SymId::new(0),
            ModuleKey(workspace.join("library/electrical.hymeko")),
        )]
        .into(),
        canon,
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
        bases: crate::common::ids::BaseListId::NONE,
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
    let reference =
        crate::ir::ir::SignedRefR::plain(DeclId::new(0), crate::ir::ir::IncidenceSign::Neutral);
    payload.ir.arcs.push(ArcRec {
        anno: crate::common::ids::AnnotationId::NONE,
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

// APPROVED-CORE-EDIT: ir-stream-cbor-snapshot (2026-09-15, user chat).
mod snapshot_stream {
    use super::*;
    use crate::ir_stream::{
        ApplyOutcome, AssemblerLimits, CommittedTransaction, ElementKind, EnvelopeDraft,
        FLAG_LAST_CHUNK, MessageType, ResyncReason, SectionKind, SnapshotVersion,
        TransactionAssembler, TransactionKind, encode_message, validate_message,
    };
    use crate::writers::cbor_writer::stream::single_section_payload;

    const TARGET: SnapshotVersion = SnapshotVersion::new(1, 1, 1, 0);

    fn draft(max_fragment_bytes: usize) -> SnapshotMessageDraft {
        SnapshotMessageDraft {
            stream_id: 7,
            first_seq: 1,
            txn_id: 1,
            base_version: SnapshotVersion::ZERO,
            target_version: TARGET,
            max_fragment_bytes,
        }
    }

    /// Drives the messages through the real validator and a fresh assembler: every message
    /// before the last must be accepted, and the last must commit.
    fn commit(messages: &[Vec<u8>]) -> CommittedTransaction {
        let mut assembler = TransactionAssembler::new(AssemblerLimits::DEFAULT);
        let (last, earlier) = messages.split_last().expect("a transaction has messages");
        for bytes in earlier {
            let message = validate_message(bytes, &DecodeLimits::DEFAULT).unwrap();
            assert_eq!(assembler.apply(&message), ApplyOutcome::Accepted);
        }
        let message = validate_message(last, &DecodeLimits::DEFAULT).unwrap();
        match assembler.apply(&message) {
            ApplyOutcome::Committed(transaction) => transaction,
            other => panic!("expected a commit, got {other:?}"),
        }
    }

    /// A committed transaction of `kind` whose chunks carry exactly `chunk_payloads`.
    fn transaction(kind: TransactionKind, chunk_payloads: &[Vec<u8>]) -> CommittedTransaction {
        let (begin, chunk, target) = match kind {
            TransactionKind::Snapshot => (
                MessageType::SnapshotBegin,
                MessageType::SnapshotChunk,
                TARGET,
            ),
            TransactionKind::Delta => (
                MessageType::DeltaBegin,
                MessageType::DeltaChunk,
                SnapshotVersion::new(0, 1, 0, 0),
            ),
        };
        let mut seq = 0;
        let mut message = |msg_type: MessageType, payload: &[u8]| {
            seq += 1;
            let envelope = EnvelopeDraft {
                stream_id: 7,
                seq,
                txn_id: 1,
                target_version: target,
                ..EnvelopeDraft::new(msg_type)
            };
            encode_message(&envelope, payload)
        };
        let mut messages = vec![message(begin, &[0; 8])];
        for payload in chunk_payloads {
            messages.push(message(chunk, payload));
        }
        messages.push(message(MessageType::Commit, &[]));
        commit(&messages)
    }

    #[test]
    fn a_fragmented_snapshot_reassembles_to_the_directly_decoded_model() {
        let bytes = encoded_fixture();
        let direct = decode(&bytes, &DecodeLimits::strict()).unwrap();
        for size in [1, 3, bytes.len(), bytes.len() + 1] {
            let messages = encode_snapshot_messages(&bytes, &draft(size)).unwrap();
            assert_eq!(
                messages.len(),
                bytes.len().div_ceil(size) + 2,
                "size {size}"
            );
            let last = messages.len() - 1;
            for (position, raw) in messages.iter().enumerate() {
                let message = validate_message(raw, &DecodeLimits::strict()).unwrap();
                let envelope = message.envelope();
                assert_eq!(envelope.seq(), 1 + position as u64);
                let expected = match position {
                    0 => MessageType::SnapshotBegin,
                    p if p == last => MessageType::Commit,
                    _ => MessageType::SnapshotChunk,
                };
                assert_eq!(envelope.msg_type(), expected);
                assert_eq!(
                    envelope.flags() & FLAG_LAST_CHUNK != 0,
                    position + 1 == last,
                    "only the last chunk is flagged"
                );
                if expected == MessageType::SnapshotChunk {
                    let [section] = message.sections() else {
                        panic!("a chunk carries exactly one section");
                    };
                    assert_eq!(section.kind(), SectionKind::CanonicalIrCbor);
                    assert_eq!(section.element_kind(), ElementKind::Raw8);
                    assert!(section.bytes().len() <= size);
                } else {
                    assert!(message.sections().is_empty());
                }
            }
            let decoded = decode_snapshot(&commit(&messages), &DecodeLimits::strict()).unwrap();
            assert_eq!(
                decoded.canonical_hash(),
                direct.canonical_hash(),
                "size {size}"
            );
            assert_eq!(decoded.payload().root_path, direct.payload().root_path);
        }
    }

    #[test]
    fn the_producer_rejects_empty_documents_empty_fragments_and_sequence_overflow() {
        let bytes = encoded_fixture();
        assert!(matches!(
            encode_snapshot_messages(&[], &draft(8)),
            Err(CborStreamError::EmptyDocument)
        ));
        assert!(matches!(
            encode_snapshot_messages(&bytes, &draft(0)),
            Err(CborStreamError::ZeroFragmentSize)
        ));
        // The Commit takes first_seq + chunks + 1: the largest first_seq that fits, then one more.
        let chunks = bytes.len().div_ceil(8) as u64;
        let fits = SnapshotMessageDraft {
            first_seq: u64::MAX - chunks - 1,
            ..draft(8)
        };
        assert!(encode_snapshot_messages(&bytes, &fits).is_ok());
        let overflows = SnapshotMessageDraft {
            first_seq: u64::MAX - chunks,
            ..draft(8)
        };
        assert!(matches!(
            encode_snapshot_messages(&bytes, &overflows),
            Err(CborStreamError::SequenceOverflow)
        ));
    }

    #[test]
    fn a_topology_bump_without_weights_and_features_is_refused_by_the_assembler() {
        let bytes = encoded_fixture();
        let incoherent = SnapshotMessageDraft {
            target_version: SnapshotVersion::new(1, 0, 0, 0),
            ..draft(64)
        };
        let messages = encode_snapshot_messages(&bytes, &incoherent).unwrap();
        let begin = validate_message(&messages[0], &DecodeLimits::strict()).unwrap();
        assert!(matches!(
            TransactionAssembler::new(AssemblerLimits::DEFAULT).apply(&begin),
            ApplyOutcome::ResyncRequired(ResyncReason::IncoherentVersionBump { .. })
        ));
    }

    #[test]
    fn decode_snapshot_accepts_only_snapshot_transactions_of_raw_cbor_fragments() {
        let bytes = encoded_fixture();
        let limits = DecodeLimits::strict();
        let fragment =
            single_section_payload(SectionKind::CanonicalIrCbor, ElementKind::Raw8, &bytes);

        let accepted = transaction(TransactionKind::Snapshot, std::slice::from_ref(&fragment));
        assert!(decode_snapshot(&accepted, &limits).is_ok());

        let delta = transaction(TransactionKind::Delta, std::slice::from_ref(&fragment));
        assert!(matches!(
            decode_snapshot(&delta, &limits),
            Err(CborStreamError::NotASnapshot {
                kind: TransactionKind::Delta
            })
        ));

        let empty = transaction(TransactionKind::Snapshot, &[]);
        assert!(matches!(
            decode_snapshot(&empty, &limits),
            Err(CborStreamError::NoFragments)
        ));

        let numeric = single_section_payload(
            SectionKind::WeightPatchRange,
            ElementKind::F32,
            &1.0f32.to_le_bytes(),
        );
        let mixed = transaction(TransactionKind::Snapshot, &[fragment, numeric]);
        assert!(matches!(
            decode_snapshot(&mixed, &limits),
            Err(CborStreamError::UnexpectedSection {
                index: 1,
                kind: SectionKind::WeightPatchRange
            })
        ));

        let mistyped =
            single_section_payload(SectionKind::CanonicalIrCbor, ElementKind::U32, &[0; 4]);
        let mistyped = transaction(TransactionKind::Snapshot, &[mistyped]);
        assert!(matches!(
            decode_snapshot(&mistyped, &limits),
            Err(CborStreamError::WrongElementKind {
                index: 0,
                got: ElementKind::U32
            })
        ));
    }

    #[test]
    fn the_reassembled_length_is_limited_before_it_is_allocated() {
        let bytes = encoded_fixture();
        let messages = encode_snapshot_messages(&bytes, &draft(16)).unwrap();
        let length = bytes.len() as u64;
        let limits = DecodeLimits {
            max_message_bytes: length - 1,
            ..DecodeLimits::strict()
        };
        assert!(matches!(
            decode_snapshot(&commit(&messages), &limits),
            Err(CborStreamError::TooLarge { got, max }) if got == length && max == length - 1
        ));
    }

    #[test]
    fn missing_or_reordered_fragments_fail_the_cbor_boundary() {
        let bytes = encoded_fixture();
        // A fragment boundary near the middle, so 3-byte fragments line up with the edits.
        let middle = bytes.len() / 2 / 3 * 3;

        let mut missing = bytes.clone();
        missing.drain(middle..middle + 3);
        let mut reordered = bytes.clone();
        reordered[middle..middle + 6].rotate_left(3);
        assert_ne!(reordered, bytes, "the swapped fragments must differ");

        for document in [missing, reordered] {
            let messages = encode_snapshot_messages(&document, &draft(3)).unwrap();
            assert!(matches!(
                decode_snapshot(&commit(&messages), &DecodeLimits::strict()),
                Err(CborStreamError::Cbor(_))
            ));
        }
    }

    #[test]
    fn stream_errors_name_the_rule_and_keep_the_cbor_cause() {
        assert!(
            CborStreamError::NoFragments
                .to_string()
                .contains("CanonicalIrCbor")
        );
        let wrapped: CborStreamError = CborError::IndexMismatch.into();
        assert!(matches!(
            wrapped,
            CborStreamError::Cbor(CborError::IndexMismatch)
        ));
        assert!(std::error::Error::source(&wrapped).is_some());
        assert!(std::error::Error::source(&CborStreamError::EmptyDocument).is_none());
    }
}
