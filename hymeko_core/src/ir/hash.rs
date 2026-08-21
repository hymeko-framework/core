use crate::common::ids::SymId;
use crate::common::pathkey::PathKey;
use crate::ir::ir::DeclKind;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct HashId(pub [u8; 32]);

fn path_bytes(path: &[SymId], it: &Interner) -> Vec<u8> {
    // pl: "fano.n0" UTF-8
    let s = path
        .iter()
        .map(|&x| it.resolve(x))
        .collect::<Vec<_>>()
        .join(".");
    s.into_bytes()
}

pub fn hash_doc(idx: &Index, it: &Interner) -> HashId {
    let mut h = blake3::Hasher::new();
    h.update(b"hymeko-doc-v1");

    // 1. Extract and sort by DeclId (Primitive integer comparison)
    // This perfectly restores AST chronological order and completely bypasses
    // the catastrophic pointer-chasing of sorting PathKey vectors.
    let mut entries: Vec<_> = idx.by_path.iter().collect();
    entries.sort_unstable_by_key(|(_, did)| did.0);

    // 2. One single contiguous memory arena for all text
    // Assuming roughly ~32 bytes per path to avoid resizing
    let mut arena = Vec::with_capacity(entries.len() * 32);

    for (k, _) in entries {
        for (i, &sym) in k.0.iter().enumerate() {
            if i > 0 {
                arena.push(b'.');
            }
            arena.extend_from_slice(it.resolve(sym).as_bytes());
        }
        arena.push(b'\n');
    }

    // 3. One single function call to Blake3
    h.update(&arena);
    HashId(*h.finalize().as_bytes())
}

pub fn hash_decl(doc: HashId, kind: DeclKind, path: &PathKey, it: &Interner) -> HashId {
    let mut h = blake3::Hasher::new();
    h.update(b"hymeko-decl-v1");
    h.update(&doc.0);
    h.update(match kind {
        DeclKind::Node => b"N",
        DeclKind::Edge => b"E",
        DeclKind::HyperArc => b"A",
    });
    let pb = path
        .0
        .iter()
        .map(|&x| it.resolve(x))
        .collect::<Vec<_>>()
        .join(".");
    h.update(pb.as_bytes());
    HashId(*h.finalize().as_bytes())
}
