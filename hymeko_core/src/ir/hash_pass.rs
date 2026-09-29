use crate::common::ids::DeclId;
use crate::ir::ir::{AnnoR, Ir, SignedRefR, ValueR};
use blake3::Hasher;

// Assuming HashId wraps a [u8; 32]
use crate::ir::hash::HashId;
use crate::resolution::interner::Interner;

/// Legacy per-declaration Merkle hashes, written into [`Ir::decl_hash`].
///
/// # Deprecated — three measured defects make it unsound as a model identity
///
/// Superseded by [`crate::ir::canonical_hash::Canon::hash`], which is pure, versioned
/// and domain-separated. Measured 2026-09-05, reproduced by
/// `tests/unified_ir/verification/semantic_digest.rs`:
///
/// * **D1** not idempotent — it reads `ir.decl_hash` while writing it, so the first call disagrees
///   with the second and the value depends on how many times it has run;
/// * **D2** an incidence target change is invisible to the first pass;
/// * **D3** a reference to an earlier-declared entity folds `None` and contributes nothing.
///
/// Retained, unchanged, for provenance and for consumers that have not migrated. It is **not** a
/// model identity and must not be bound to an `ExecutionPlan`.
#[deprecated(
    since = "0.1.0",
    note = "unsound as a model identity (D1 non-idempotent, D2 incidence target invisible, \
            D3 backward reference dropped); use ir::canonical_hash::Canon::hash"
)]
pub fn compute_merkle_hashes(ir: &mut Ir, interner: &Interner) {
    let num_decls = ir.decl_nodes.len();

    if ir.decl_hash.len() < num_decls {
        ir.decl_hash.resize(num_decls, None);
    }

    // Process backwards to ensure children are hashed before parents.
    for i in (0..num_decls).rev() {
        let did = DeclId::new(i);
        let mut hasher = Hasher::new();

        // Bind the unified node reference once for cache efficiency
        let node = &ir.decl_nodes[i];

        // 1. Hash Kind and Name
        hasher.update(&[node.kind as u8]);
        let name_str = interner.resolve(node.name);
        hasher.update(name_str.as_bytes());

        // 2. Hash Annotations
        hash_anno(&node.anno, &mut hasher, ir, interner);

        // 3. Hash Children (Merkle linkage)
        for child_did in ir.decl_children(did) {
            if let Some(h) = ir.decl_hash[child_did.0 as usize] {
                hasher.update(&h.0);
            }
        }

        // 4. Special handling for Arcs (they contain references/weights)
        if let Some(aid) = ir.as_arc(did) {
            let arc = &ir.arcs[aid.0 as usize];
            for sref in &arc.refs {
                hash_signed_ref(sref, &mut hasher, ir, interner);
            }
        }

        ir.decl_hash[i] = Some(HashId(hasher.finalize().into()));
    }
}

fn hash_anno(anno: &AnnoR, hasher: &mut Hasher, ir: &Ir, it: &Interner) {
    let num_tags = anno.tags.len() as u64;
    hasher.update(&num_tags.to_le_bytes());

    for &tag_sid in &anno.tags {
        let tag_str = it.resolve(tag_sid);

        // Mathematically isolate each string
        let len = tag_str.len() as u64;
        hasher.update(&len.to_le_bytes());
        hasher.update(tag_str.as_bytes());
    }

    match &anno.value {
        Some(v) => {
            hasher.update(&[1]);
            hash_value(v, hasher, ir, it);
        }
        None => {
            hasher.update(&[0]); // <-- THIS SEMICOLON IS CRITICAL
        }
    }
}

fn hash_value(v: &ValueR, hasher: &mut Hasher, ir: &Ir, it: &Interner) {
    match v {
        ValueR::Str(sid) => {
            hasher.update(&[0]); // Type tag
            hasher.update(it.resolve(*sid).as_bytes());
        }
        ValueR::Num(n) => {
            hasher.update(&[1]);
            hasher.update(&n.to_le_bytes());
        }
        ValueR::List(xs) => {
            hasher.update(&[2]);
            hasher.update(&(xs.len() as u64).to_le_bytes());
            for x in xs {
                hash_value(x, hasher, ir, it);
            }
        }
        ValueR::Ref(did) => {
            hasher.update(&[3]);
            // Use the HashId of the target to stay location-independent [cite: 2026-02-08].
            if let Some(h) = ir.decl_hash[did.0 as usize] {
                hasher.update(&h.0);
            }
        }
    }
}

fn hash_signed_ref(sref: &SignedRefR, hasher: &mut Hasher, ir: &Ir, it: &Interner) {
    // Hash the variant/sign
    let (tag, atom) = match sref {
        SignedRefR::Plus(a) => (1u8, a),
        SignedRefR::Minus(a) => (2u8, a),
        SignedRefR::Neutral(a) => (3u8, a),
    };
    hasher.update(&[tag]);

    // Hash the Target's HashId [cite: 2026-02-08]
    if let Some(h) = ir.decl_hash[atom.target.0 as usize] {
        hasher.update(&h.0);
    }

    // Hash Atom-specific annotations and weights
    hash_anno(&atom.anno, hasher, ir, it);
    if let Some(weights) = &atom.weights {
        hasher.update(&[1]);
        for w in weights {
            hash_value(w, hasher, ir, it);
        }
    } else {
        hasher.update(&[0]);
    }
}
