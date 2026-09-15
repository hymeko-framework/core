//! Canonical CBOR snapshots through the public `ir_stream` wire validator and transaction assembler.
//!
//! APPROVED-CORE-EDIT: ir-stream-cbor-snapshot (2026-09-15, user chat). Every lowerable corpus program
//! is encoded, split into `CanonicalIrCbor` fragments, delivered message by message to an assembler
//! whose limits admit exactly that transaction, and reassembled by `decode_snapshot`. The result must
//! equal a direct decode of the same bytes.

use hymeko::ir_stream::{
    ApplyOutcome, AssemblerLimits, CommittedTransaction, DecodeLimits, SnapshotVersion,
    TransactionAssembler, validate_message,
};
use hymeko::writers::cbor_writer::{
    SnapshotMessageDraft, decode, decode_snapshot, encode_compiled, encode_snapshot_messages,
};
use std::path::PathBuf;

fn draft(max_fragment_bytes: usize) -> SnapshotMessageDraft {
    SnapshotMessageDraft {
        stream_id: 0x5eed,
        first_seq: 1,
        txn_id: 1,
        base_version: SnapshotVersion::ZERO,
        target_version: SnapshotVersion::new(1, 1, 1, 0),
        max_fragment_bytes,
    }
}

/// Delivers the messages to a fresh assembler whose limits admit exactly this transaction.
///
/// `max_chunks` bounds the staging messages of a transaction, and the `SnapshotBegin` stages too
/// (it may carry sections), so a document of `chunks` fragments needs `chunks + 1`.
fn deliver(document_bytes: usize, messages: &[Vec<u8>]) -> CommittedTransaction {
    let chunks = messages.len() - 2;
    let mut assembler = TransactionAssembler::new(AssemblerLimits {
        max_staged_bytes: document_bytes,
        max_staged_sections: chunks,
        max_chunks: u32::try_from(chunks + 1).expect("message count fits the assembler limit"),
        replay_capacity: 16,
    });
    let (last, earlier) = messages.split_last().expect("a transaction has messages");
    for bytes in earlier {
        let message =
            validate_message(bytes, &DecodeLimits::DEFAULT).expect("producer output validates");
        assert_eq!(assembler.apply(&message), ApplyOutcome::Accepted);
    }
    let message = validate_message(last, &DecodeLimits::DEFAULT).expect("commit validates");
    match assembler.apply(&message) {
        ApplyOutcome::Committed(transaction) => transaction,
        other => panic!("expected a commit, got {other:?}"),
    }
}

/// Sends `bytes` in fragments of `size` and compares the reassembled model with a direct decode.
fn assert_snapshot_matches_direct_decode(bytes: &[u8], size: usize, label: &str) {
    let direct = decode(bytes, &DecodeLimits::DEFAULT)
        .unwrap_or_else(|error| panic!("{label}: direct decode: {error}"));
    let messages = encode_snapshot_messages(bytes, &draft(size))
        .unwrap_or_else(|error| panic!("{label}: producer: {error}"));
    let decoded = decode_snapshot(&deliver(bytes.len(), &messages), &DecodeLimits::DEFAULT)
        .unwrap_or_else(|error| panic!("{label}: fragments of {size} bytes: {error}"));

    assert_eq!(
        decoded.canonical_hash(),
        direct.canonical_hash(),
        "{label}: canonical hash"
    );
    assert_eq!(
        decoded.payload().root_path,
        direct.payload().root_path,
        "{label}: root path"
    );
    assert_eq!(
        decoded.payload().imports,
        direct.payload().imports,
        "{label}: imports"
    );
    assert_eq!(
        decoded.payload().interned_strings,
        direct.payload().interned_strings,
        "{label}: interner"
    );
    assert_eq!(
        decoded.ir().decl_nodes.len(),
        direct.ir().decl_nodes.len(),
        "{label}: declarations"
    );
}

#[test]
fn every_lowerable_corpus_program_survives_fragmented_snapshot_transport() {
    let root = crate::unified_ir::support::corpus::corpus_root();
    let mut delivered = 0usize;
    let mut smallest: Option<(usize, PathBuf, Vec<u8>)> = None;
    for path in crate::unified_ir::support::corpus::require_fixtures() {
        let Ok((store, compiled)) = crate::test_helpers::load_and_lower(&path) else {
            continue;
        };
        let bytes = encode_compiled(&root, &compiled, &store.it)
            .unwrap_or_else(|error| panic!("{}: encode: {error}", path.display()));
        assert_snapshot_matches_direct_decode(&bytes, 4096, &path.display().to_string());
        delivered += 1;
        if smallest
            .as_ref()
            .is_none_or(|(length, _, _)| bytes.len() < *length)
        {
            smallest = Some((bytes.len(), path, bytes));
        }
    }
    assert!(delivered > 0, "the corpus must contain lowerable programs");

    // The smallest document again, one byte per chunk and at an odd fragment size.
    let (_, path, bytes) = smallest.expect("at least one document was delivered");
    for size in [1, 257] {
        assert_snapshot_matches_direct_decode(&bytes, size, &path.display().to_string());
    }
}
