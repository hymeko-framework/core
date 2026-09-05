//! **APPROVED-CORE-EDIT: semantic-model-digest-v2** (2026-09-05, user chat).
//!
//! The complete semantic identity of an [`Ir`], as distinct from the narrow structural identity
//! that [`crate::ir::canonical_hash::canonical_program_hash`] has always computed.
//!
//! # Why this exists
//!
//! Two identities were already in the crate and they cover disjoint things:
//!
//! | field | `canonical_program_hash` v1 | [`crate::ir::hash_pass::compute_merkle_hashes`] |
//! |---|---|---|
//! | kind, name, path, hierarchy | yes | yes |
//! | incidence sign, slot, **target** | yes | **no** (measured) |
//! | annotations, values, weights | **no** (by design) | yes |
//!
//! Neither is a model identity. Worse, the Merkle pass carries three measured defects, recorded in
//! `nagare/reports/2026-09-05-hymeko-ir-execution-plan-audit.md` §14.3 and reproduced by the control
//! matrix in `tests/unified_ir/verification/semantic_digest.rs`:
//!
//! * **D1** it is not idempotent — it reads `Ir::decl_hash` while writing it, so the first call
//!   disagrees with the second and the value depends on the call count;
//! * **D2** an incidence target change is invisible to the first pass — `+a` and `+b` in the same
//!   slot with the same sign produce the same table, i.e. two different graphs, one digest;
//! * **D3** a reference to an **earlier-declared** entity folds `decl_hash[target] == None` and
//!   contributes nothing. That is the ordinary case: lowering numbers declarations in source order.
//!
//! # What this module does instead
//!
//! * **Pure.** It takes `&Ir` and returns a value. It reads no destination table and writes none, so
//!   a stale or pre-populated [`Ir::decl_hash`] cannot influence the result and repeated calls are
//!   identical by construction (D1).
//! * **Reference identity is the canonical path**, never the target's partially-computed digest
//!   (D2, D3). A path is stable, order-independent, and already the identity the v1 incidence record
//!   uses. Every declaration nevertheless contributes to the model digest, so the digest still moves
//!   when a referenced declaration's own content moves.
//! * **Domain-separated** per record kind, so a tag can never be confused with a value, a value with
//!   a weight, or a node's contribution with an edge's.
//! * **Versioned**: magic, domain, schema and algorithm version are hashed before any payload.
//! * **Total and terminating** on malformed input, including reference cycles and a corrupted child
//!   chain: every walk is bounded by `decl_nodes.len()`.
//!
//! # Relationship to v1
//!
//! v1 is untouched, byte-for-byte, and keeps its role as the structural identity;
//! [`program_structure_hash_v1`] is the name to use when that narrower contract is what is meant.
//! This module never renames, rewrites or re-versions it.

use blake3::Hasher;

use crate::common::ids::DeclId;
use crate::ir::canonical_hash::{CanonHashCfg, build_path_index, canonical_program_hash};
use crate::ir::hash::HashId;
use crate::ir::ir::{AnnoR, DeclKind, Ir, SignedRefR, ValueR};
use crate::resolution::interner::Interner;

/// Wire magic, shared with the v1 header so the family is greppable in a dump.
const MAGIC: &[u8; 8] = b"HYMEKOCN";
/// Domain for the whole-model digest.
const DOMAIN_MODEL: &[u8; 8] = b"SEMMODEL";
/// Domain for one declaration's contribution.
const DOMAIN_DECL: &[u8; 8] = b"SEMDECL\0";

/// Schema version of the semantic digest. Bumped when the *meaning* of the covered set changes.
pub const SEMANTIC_DIGEST_SCHEMA_VERSION: u16 = 2;
/// Algorithm version. Bumped when the byte layout changes without changing the covered set.
pub const SEMANTIC_DIGEST_ALGO_VERSION: u16 = 1;

/// The narrow, structural identity of a program: kind, name, path, hierarchy and incidence
/// (sign, slot, target). **Does not cover annotations, values or weights.**
///
/// A distinct type from [`SemanticModelDigestV2`] on purpose: the two must never be substituted for
/// one another, and a signature that says which one it means cannot be misread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProgramStructureHashV1(pub HashId);

/// The complete semantic identity of a model: everything v1 covers, plus annotations, annotation
/// values, incidence weights and reference-atom annotations.
///
/// This is the identity an `ExecutionPlan` binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemanticModelDigestV2(pub HashId);

/// The v1 structural hash under its narrow name.
///
/// Delegates to [`canonical_program_hash`] unchanged — same bytes, same value, same golden vectors.
/// The wrapper exists so a caller states which contract it wanted.
#[must_use]
pub fn program_structure_hash_v1(
    cfg: CanonHashCfg,
    ir: &Ir,
    it: &Interner,
) -> ProgramStructureHashV1 {
    ProgramStructureHashV1(canonical_program_hash(cfg, ir, it))
}

/// How a reference's target is identified inside the digest.
///
/// Recorded explicitly and domain-tagged, so a program hashed under one scheme can never collide
/// with a program hashed under the other.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetScheme {
    /// The canonical declaration path — the normal case.
    Path,
    /// The raw `DeclId`, used only when the path index cannot be built (duplicate paths, i.e. a
    /// malformed `Ir`). The digest stays total rather than refusing to produce a value.
    DeclIdFallback,
}

impl TargetScheme {
    const fn tag(self) -> u8 {
        match self {
            Self::Path => 0,
            Self::DeclIdFallback => 1,
        }
    }
}

/// Per-declaration semantic hashes, in `DeclId` order.
///
/// Exposed because a consumer that wants to know *which* declaration moved needs the table, not
/// only the folded scalar. It is a return value: nothing is written into the `Ir`.
#[must_use]
pub fn semantic_declaration_hashes(ir: &Ir, it: &Interner) -> Vec<HashId> {
    let n = ir.decl_nodes.len();
    let index = build_path_index(ir).ok();
    let scheme = if index.is_some() {
        TargetScheme::Path
    } else {
        TargetScheme::DeclIdFallback
    };
    let paths: Option<&[Option<crate::common::pathkey::PathKey>]> =
        index.as_ref().map(|i| i.did_to_path());

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let d = DeclId::new(i);
        let node = &ir.decl_nodes[i];
        let mut h = Hasher::new();
        write_header(&mut h, DOMAIN_DECL);
        h.update(&[scheme.tag()]);
        h.update(&[kind_tag(node.kind)]);
        write_str(&mut h, it.resolve(node.name));
        // The declaration's own position: its path under the chosen scheme. This is what makes a
        // hierarchy change visible without walking children, and it is order-independent.
        write_target(&mut h, paths, scheme, d, it);
        write_anno(&mut h, &node.anno, paths, scheme, it);

        // Records attached to this declaration.
        if let Some(nid) = ir.as_node(d) {
            h.update(b"NODE");
            write_signed_refs(&mut h, &ir.nodes[nid.raw()].bases, paths, scheme, it);
        }
        if let Some(eid) = ir.as_edge(d) {
            h.update(b"EDGE");
            let edge = &ir.edges[eid.raw()];
            write_signed_refs(&mut h, &edge.bases, paths, scheme, it);
            // Arc membership and its order are part of the edge's meaning.
            h.update(&(edge.arcs.len() as u64).to_le_bytes());
            for (position, arc_id) in edge.arcs.iter().enumerate() {
                h.update(&(position as u64).to_le_bytes());
                h.update(&(arc_id.raw() as u64).to_le_bytes());
            }
        }
        if let Some(aid) = ir.as_arc(d) {
            h.update(b"ARC0");
            let arc = &ir.arcs[aid.raw()];
            write_target(&mut h, paths, scheme, arc.in_edge, it);
            write_anno(&mut h, &arc.anno, paths, scheme, it);
            write_signed_refs(&mut h, &arc.refs, paths, scheme, it);
        }
        out.push(HashId(*h.finalize().as_bytes()));
    }
    out
}

/// The complete semantic model digest.
///
/// # Postconditions
/// Deterministic and idempotent: calling it any number of times on the same `(ir, it)` returns the
/// same value, and it neither reads nor writes [`Ir::decl_hash`].
#[must_use]
pub fn semantic_model_digest_v2(ir: &Ir, it: &Interner) -> SemanticModelDigestV2 {
    let per_decl = semantic_declaration_hashes(ir, it);
    let mut h = Hasher::new();
    write_header(&mut h, DOMAIN_MODEL);
    h.update(&(per_decl.len() as u64).to_le_bytes());
    for d in &per_decl {
        h.update(&d.0);
    }
    SemanticModelDigestV2(HashId(*h.finalize().as_bytes()))
}

// ---------------------------------------------------------------------------------------------
// Encoding helpers. Every one is length-prefixed or fixed-width, so two different structures can
// never produce the same byte stream by concatenation.
// ---------------------------------------------------------------------------------------------

fn write_header(h: &mut Hasher, domain: &[u8; 8]) {
    h.update(MAGIC);
    h.update(domain);
    h.update(&SEMANTIC_DIGEST_SCHEMA_VERSION.to_le_bytes());
    h.update(&SEMANTIC_DIGEST_ALGO_VERSION.to_le_bytes());
}

const fn kind_tag(k: DeclKind) -> u8 {
    match k {
        DeclKind::Node => 1,
        DeclKind::Edge => 2,
        DeclKind::HyperArc => 3,
    }
}

fn write_str(h: &mut Hasher, s: &str) {
    h.update(&(s.len() as u64).to_le_bytes());
    h.update(s.as_bytes());
}

/// A target's stable identity: its canonical path, or its `DeclId` when no path exists (arcs are
/// anonymous) or the index could not be built.
fn write_target(
    h: &mut Hasher,
    paths: Option<&[Option<crate::common::pathkey::PathKey>]>,
    scheme: TargetScheme,
    d: DeclId,
    it: &Interner,
) {
    if d.is_none() {
        h.update(&[0u8]);
        return;
    }
    let by_path = match (scheme, paths) {
        (TargetScheme::Path, Some(p)) => p.get(d.raw()).and_then(|x| x.as_ref()),
        _ => None,
    };
    match by_path {
        Some(pk) => {
            h.update(&[1u8]);
            h.update(&(pk.0.len() as u64).to_le_bytes());
            for sid in pk.iter() {
                write_str(h, it.resolve(sid));
            }
        }
        None => {
            // Anonymous (an arc) or fallback: the index itself is the identity.
            h.update(&[2u8]);
            h.update(&(d.raw() as u64).to_le_bytes());
        }
    }
}

fn write_anno(
    h: &mut Hasher,
    anno: &AnnoR,
    paths: Option<&[Option<crate::common::pathkey::PathKey>]>,
    scheme: TargetScheme,
    it: &Interner,
) {
    h.update(b"ANNO");
    h.update(&(anno.tags.len() as u64).to_le_bytes());
    for &t in &anno.tags {
        write_str(h, it.resolve(t));
    }
    match &anno.value {
        Some(v) => {
            h.update(&[1u8]);
            write_value(h, v, paths, scheme, it, 0);
        }
        None => {
            h.update(&[0u8]);
        }
    }
}

/// Maximum nesting depth of a `ValueR::List`. Beyond it the structure is recorded as truncated
/// rather than recursed into, so a pathological or cyclic construction terminates deterministically
/// instead of overflowing the stack.
const MAX_VALUE_DEPTH: u32 = 64;

fn write_value(
    h: &mut Hasher,
    v: &ValueR,
    paths: Option<&[Option<crate::common::pathkey::PathKey>]>,
    scheme: TargetScheme,
    it: &Interner,
    depth: u32,
) {
    if depth >= MAX_VALUE_DEPTH {
        h.update(b"TRUNC");
        return;
    }
    match v {
        ValueR::Str(sid) => {
            h.update(&[1u8]);
            write_str(h, it.resolve(*sid));
        }
        ValueR::Num(n) => {
            h.update(&[2u8]);
            // Bit pattern, not the value: -0.0 and 0.0 are different declarations, and NaN payloads
            // must not silently collapse.
            h.update(&n.to_bits().to_le_bytes());
        }
        ValueR::List(xs) => {
            h.update(&[3u8]);
            h.update(&(xs.len() as u64).to_le_bytes());
            for x in xs {
                write_value(h, x, paths, scheme, it, depth + 1);
            }
        }
        ValueR::Ref(target) => {
            h.update(&[4u8]);
            // The target's IDENTITY, never its digest: this is D2/D3.
            write_target(h, paths, scheme, *target, it);
        }
    }
}

fn write_signed_refs(
    h: &mut Hasher,
    refs: &[SignedRefR],
    paths: Option<&[Option<crate::common::pathkey::PathKey>]>,
    scheme: TargetScheme,
    it: &Interner,
) {
    h.update(&(refs.len() as u64).to_le_bytes());
    for (slot, r) in refs.iter().enumerate() {
        // Slot index: ordered ports are part of the meaning.
        h.update(&(slot as u64).to_le_bytes());
        let (sign, atom) = match r {
            SignedRefR::Plus(a) => (1u8, a),
            SignedRefR::Minus(a) => (2u8, a),
            SignedRefR::Neutral(a) => (3u8, a),
        };
        // Sign is the ALGEBRAIC axis. It is hashed as itself and is never read as a direction.
        h.update(&[sign]);
        write_target(h, paths, scheme, atom.target, it);
        write_anno(h, &atom.anno, paths, scheme, it);
        match &atom.weights {
            Some(ws) => {
                h.update(&[1u8]);
                h.update(&(ws.len() as u64).to_le_bytes());
                for w in ws {
                    write_value(h, w, paths, scheme, it, 0);
                }
            }
            None => {
                h.update(&[0u8]);
            }
        }
    }
}
