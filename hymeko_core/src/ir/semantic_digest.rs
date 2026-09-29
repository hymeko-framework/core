//! Semantic identity of the canonical Core IR: schema 2, algorithm 2.
//!
//! The structural V1 hash remains unchanged. This module additionally covers
//! declaration annotations and every concrete node, edge and arc record,
//! including records that have no declaration dispatch binding. Cached hashes,
//! document provenance and derived indexes are not semantic payload.
//!
//! Named targets use resolved canonical paths. An anonymous arc uses its owner's
//! path and the first position of that arc in the owner's ordered arc list.
//! Repeated arc slots retain their identity, order and multiplicity. Anonymous
//! declaration names are lowerer/builder placeholders and are not semantic names.
//!
//! Canonical mode requires unique named paths, an owner slot for every arc, and
//! a valid, unique arc binding for every anonymous declaration. Otherwise the model
//! uses a separate raw-ID domain, preserving payload coverage and determinism
//! without promising invariance under storage reindexing. CBOR validity alone
//! does not imply these stronger canonical identity conditions.
//!
//! Algorithm 2 replaces algorithm 1's incomplete coverage, depth truncation and
//! storage-order dependence. Recompute cached semantic digests; the CBOR schema,
//! structural V1 values and `Ir::decl_hash` do not change.
//!
//! # Preconditions
//! Symbol and concrete-record indices must be valid for the supplied IR and
//! interner. Canonical reindexing invariance additionally assumes a coherent
//! named hierarchy. This module is not a malformed-IR validator.
//! # Panics
//! Invalid symbol or bound-record indices may panic. Bounded path construction
//! does not make an arbitrary malformed IR valid.

use blake3::Hasher;

use crate::common::ids::{DeclId, HyperArcId};
use crate::ir::canonical_hash::Canon;
use crate::ir::hash::HashId;
use crate::ir::ir::{
    AnnoR, DType, DeclKind, Extent, Ir, ModelFacet, PortDirection, SignedRefR, TensorRole, ValueR,
};
use crate::resolution::interner::Interner;

const MAGIC: &[u8; 8] = b"HYMEKOCN";
const DOMAIN_MODEL: &[u8; 8] = b"SEMMODEL";
const DOMAIN_DECL: &[u8; 8] = b"SEMDECL\0";
const DOMAIN_NODE: &[u8; 8] = b"SEMNODE\0";
const DOMAIN_EDGE: &[u8; 8] = b"SEMEDGE\0";
const DOMAIN_ARC: &[u8; 8] = b"SEMARC\0\0";

/// Version of the intended semantic field set.
pub const SEMANTIC_DIGEST_SCHEMA_VERSION: u16 = 2;
/// Encoding version. Algorithm 2 corrects coverage and canonicalization defects.
pub const SEMANTIC_DIGEST_ALGO_VERSION: u16 = 3;

/// V1 structural identity: excludes annotation values and incidence weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProgramStructureHashV1(pub HashId);

/// Semantic identity under the canonical/raw-ID domains documented by this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemanticModelDigestV2(pub HashId);

impl Canon<'_> {
    /// Return one contribution per declaration, in DeclId order.
    ///
    /// This diagnostic table includes each declaration's bound record, when present.
    /// Use [`Canon::hash`] for whole-model coverage of unbound records.
    /// # Preconditions
    /// The module's valid-index preconditions apply.
    /// # Postconditions
    /// Pure and deterministic; neither reads nor writes the IR's cached hash tables.
    /// # Panics
    /// Invalid symbol or bound-record indices may panic.
    #[must_use]
    pub fn declaration_hashes(&self) -> Vec<HashId> {
        let context = SemanticContext::new(self);
        let records = context.record_hashes();
        context.declaration_hashes(&records)
    }

    /// Return the whole-model digest, with complete concrete-record coverage.
    ///
    /// # Preconditions
    /// The module's valid-index preconditions apply. Reindexing invariance is only
    /// promised in canonical mode; raw-ID fallback is explicitly domain-separated.
    /// # Postconditions
    /// Pure, deterministic and idempotent, independent of cached hash tables.
    /// Per-kind aggregation is unordered; incidence slots, arc slots, tags, bases,
    /// weights and value lists retain their order and multiplicity.
    /// # Panics
    /// Invalid symbol or bound-record indices may panic.
    #[must_use]
    pub fn hash(&self) -> SemanticModelDigestV2 {
        let context = SemanticContext::new(self);
        let mut records = context.record_hashes();
        let mut declarations = context.declaration_hashes(&records);
        let mut h = context.header(DOMAIN_MODEL);
        for (domain, hashes) in [
            (DOMAIN_DECL, &mut declarations),
            (DOMAIN_NODE, &mut records.nodes),
            (DOMAIN_EDGE, &mut records.edges),
            (DOMAIN_ARC, &mut records.arcs),
        ] {
            h.update(domain);
            write_sorted_hashes(&mut h, hashes);
        }
        SemanticModelDigestV2(HashId(*h.finalize().as_bytes()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetScheme {
    Path,
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

struct RecordHashes {
    nodes: Vec<HashId>,
    edges: Vec<HashId>,
    arcs: Vec<HashId>,
}

/// One borrowed encoding context; it owns only reproducible derived lookup data.
struct SemanticContext<'a> {
    ir: &'a Ir,
    it: &'a Interner,
    /// The canonical view whose paths name every target.
    canon: &'a Canon<'a>,
    first_owner_slots: Vec<Option<usize>>,
    scheme: TargetScheme,
}

impl<'a> SemanticContext<'a> {
    fn new(canon: &'a Canon<'a>) -> Self {
        let (ir, it) = (canon.ir(), canon.interner());
        let mut first_owner_slots = vec![None; ir.arcs.len()];
        // Use the owner's actual dispatch binding, never an orphan EdgeRec that
        // happens to carry the same declaration backlink.
        for (ei, edge) in ir.edges.iter().enumerate() {
            if ir.as_edge(edge.decl).map(|id| id.raw()) != Some(ei) {
                continue;
            }
            for (position, aid) in edge.arcs.iter().enumerate() {
                if ir
                    .arcs
                    .get(aid.raw())
                    .is_some_and(|a| a.in_edge == edge.decl)
                {
                    first_owner_slots[aid.raw()].get_or_insert(position);
                }
            }
        }
        let mut canonical =
            canon.unique_paths().is_ok() && first_owner_slots.iter().all(Option::is_some);
        let mut declared = vec![false; ir.arcs.len()];
        for (di, declaration) in ir.decl_nodes.iter().enumerate() {
            if declaration.kind != DeclKind::HyperArc {
                continue;
            }
            match ir
                .as_arc(DeclId::new(di))
                .and_then(|a| declared.get_mut(a.raw()))
            {
                Some(seen) if !*seen => *seen = true,
                _ => canonical = false,
            }
        }
        Self {
            ir,
            it,
            canon,
            first_owner_slots,
            scheme: if canonical {
                TargetScheme::Path
            } else {
                TargetScheme::DeclIdFallback
            },
        }
    }

    fn header(&self, domain: &[u8; 8]) -> Hasher {
        let mut h = Hasher::new();
        h.update(MAGIC);
        h.update(domain);
        h.update(&SEMANTIC_DIGEST_SCHEMA_VERSION.to_le_bytes());
        h.update(&SEMANTIC_DIGEST_ALGO_VERSION.to_le_bytes());
        h.update(&[self.scheme.tag()]);
        h
    }

    fn record_hashes(&self) -> RecordHashes {
        let arcs = self
            .ir
            .arcs
            .iter()
            .enumerate()
            .map(|(ai, arc)| {
                let mut h = self.header(DOMAIN_ARC);
                self.write_arc_identity(&mut h, HyperArcId::new(ai));
                self.write_target(&mut h, arc.in_edge);
                self.write_anno(&mut h, &arc.anno);
                self.write_signed_refs(&mut h, &arc.refs);
                HashId(*h.finalize().as_bytes())
            })
            .collect::<Vec<_>>();
        let nodes = self
            .ir
            .nodes
            .iter()
            .map(|node| {
                let h = self.record_header(DOMAIN_NODE, node.decl, &node.bases);
                HashId(*h.finalize().as_bytes())
            })
            .collect();
        let edges = self
            .ir
            .edges
            .iter()
            .map(|edge| {
                let mut h = self.record_header(DOMAIN_EDGE, edge.decl, &edge.bases);
                write_len(&mut h, edge.arcs.len());
                for aid in &edge.arcs {
                    // Each arc hash includes its identity, so identical payloads do
                    // not erase sharing such as [A,A,B] versus [A,B,B].
                    h.update(&arcs[aid.raw()].0);
                }
                HashId(*h.finalize().as_bytes())
            })
            .collect();
        RecordHashes { nodes, edges, arcs }
    }

    fn record_header(&self, domain: &[u8; 8], declaration: DeclId, bases: &[SignedRefR]) -> Hasher {
        let mut h = self.header(domain);
        self.write_target(&mut h, declaration);
        self.write_signed_refs(&mut h, bases);
        h
    }

    fn declaration_hashes(&self, records: &RecordHashes) -> Vec<HashId> {
        self.ir
            .decl_nodes
            .iter()
            .enumerate()
            .map(|(di, declaration)| {
                let d = DeclId::new(di);
                let mut h = self.header(DOMAIN_DECL);
                h.update(&[kind_tag(declaration.kind)]);
                if declaration.kind != DeclKind::HyperArc {
                    write_str(&mut h, self.it.resolve(declaration.name));
                }
                self.write_target(&mut h, d);
                self.write_target(&mut h, declaration.parent);
                self.write_anno(&mut h, &declaration.anno);
                // APPROVED-CORE-EDIT: executable-model-contract (2026-09-06). The typed facet is
                // part of the meaning, so it is part of the identity. Absence is folded as an
                // explicit tag rather than as nothing, so "no facet" and "a facet that happens to
                // encode to no bytes" can never collide.
                self.write_model_facet(&mut h, self.ir.model_facet(d));
                let record = match declaration.kind {
                    DeclKind::Node => self.ir.as_node(d).map(|id| &records.nodes[id.raw()]),
                    DeclKind::Edge => self.ir.as_edge(d).map(|id| &records.edges[id.raw()]),
                    DeclKind::HyperArc => self.ir.as_arc(d).map(|id| &records.arcs[id.raw()]),
                };
                match record {
                    Some(hash) => {
                        h.update(&[1]);
                        h.update(&hash.0);
                    }
                    None => {
                        h.update(&[0]);
                    }
                }
                HashId(*h.finalize().as_bytes())
            })
            .collect()
    }

    fn write_target(&self, h: &mut Hasher, d: DeclId) {
        if d.is_none() {
            h.update(&[0]);
            return;
        }
        if self.scheme == TargetScheme::Path {
            if let Some(path) = self.canon.path_of(d) {
                h.update(&[1]);
                write_len(h, path.0.len());
                for symbol in path.iter() {
                    write_str(h, self.it.resolve(symbol));
                }
                return;
            }
            if let Some(arc) = self.ir.as_arc(d) {
                h.update(&[2]);
                self.write_arc_identity(h, arc);
                return;
            }
        }
        h.update(&[3]);
        h.update(&(d.raw() as u64).to_le_bytes());
    }

    fn write_arc_identity(&self, h: &mut Hasher, arc: HyperArcId) {
        if self.scheme == TargetScheme::Path {
            h.update(&[0]);
            self.write_target(h, self.ir.arcs[arc.raw()].in_edge);
            // Construction selects Path only after every owner slot is Some.
            let position = self.first_owner_slots[arc.raw()]
                .expect("canonical mode requires an owner slot for every arc");
            h.update(&(position as u64).to_le_bytes());
        } else {
            h.update(&[1]);
            h.update(&(arc.raw() as u64).to_le_bytes());
        }
    }

    /// The optional executable-model facet, domain-tagged so absence is explicit.
    ///
    /// Every field is fixed-width or length-prefixed, so two different facets cannot produce the
    /// same byte stream by concatenation. A `Symbolic` extent contributes the resolved symbol text,
    /// not its `SymId`, so the digest stays independent of interner order.
    fn write_model_facet(&self, h: &mut Hasher, facet: Option<&ModelFacet>) {
        h.update(b"MODEL");
        match facet {
            None => h.update(&[0u8]),
            Some(ModelFacet::Tensor(t)) => {
                h.update(&[1u8]);
                h.update(&[match t.role {
                    TensorRole::Input => 1u8,
                    TensorRole::Output => 2,
                    TensorRole::Parameter => 3,
                    TensorRole::State => 4,
                    TensorRole::Intermediate => 5,
                }]);
                h.update(&[match t.dtype {
                    DType::F32 => 1u8,
                    DType::F64 => 2,
                    DType::I32 => 3,
                    DType::I64 => 4,
                    DType::U8 => 5,
                    DType::Bool => 6,
                }]);
                write_len(h, t.shape.len());
                for extent in &t.shape {
                    match extent {
                        Extent::Static(v) => {
                            h.update(&[1u8]);
                            h.update(&v.to_le_bytes());
                        }
                        Extent::Symbolic(sym) => {
                            h.update(&[2u8]);
                            write_str(h, self.it.resolve(*sym));
                        }
                    }
                }
                h
            }
            Some(ModelFacet::Operator(o)) => {
                h.update(&[2u8]);
                write_str(h, self.it.resolve(o.identity));
                h
            }
        };
    }

    fn write_anno(&self, h: &mut Hasher, anno: &AnnoR) {
        h.update(b"ANNO");
        write_len(h, anno.tags.len());
        for &tag in &anno.tags {
            write_str(h, self.it.resolve(tag));
        }
        match &anno.value {
            Some(value) => {
                h.update(&[1]);
                self.write_value(h, value);
            }
            None => {
                h.update(&[0]);
            }
        }
    }

    fn write_value(&self, h: &mut Hasher, value: &ValueR) {
        // Iterator frames retain O(depth) state without recursive calls or
        // materializing a wide list's children. Every finite value is encoded.
        let mut frames = vec![std::slice::from_ref(value).iter()];
        while let Some(frame) = frames.last_mut() {
            let Some(value) = frame.next() else {
                frames.pop();
                continue;
            };
            match value {
                ValueR::Str(symbol) => {
                    h.update(&[1]);
                    write_str(h, self.it.resolve(*symbol));
                }
                ValueR::Num(number) => {
                    h.update(&[2]);
                    h.update(&number.to_bits().to_le_bytes());
                }
                ValueR::List(values) => {
                    h.update(&[3]);
                    write_len(h, values.len());
                    frames.push(values.iter());
                }
                ValueR::Ref(target) => {
                    h.update(&[4]);
                    self.write_target(h, *target);
                }
            }
        }
    }

    fn write_signed_refs(&self, h: &mut Hasher, refs: &[SignedRefR]) {
        write_len(h, refs.len());
        for (slot, reference) in refs.iter().enumerate() {
            h.update(&(slot as u64).to_le_bytes());
            let sign = match reference {
                SignedRefR::Plus(_) => 1,
                SignedRefR::Minus(_) => 2,
                SignedRefR::Neutral(_) => 3,
            };
            h.update(&[sign]);
            let atom = reference.atom();
            // APPROVED-CORE-EDIT: executable-model-contract (2026-09-06). Direction is folded under
            // its OWN tag, immediately after the sign and never merged with it, so a sign change
            // and a direction change are two distinct mutations of the digest.
            h.update(b"DIR");
            h.update(&[match atom.direction {
                PortDirection::Unspecified => 0u8,
                PortDirection::In => 1,
                PortDirection::Out => 2,
            }]);
            self.write_target(h, atom.target);
            self.write_anno(h, &atom.anno);
            match &atom.weights {
                Some(weights) => {
                    h.update(&[1]);
                    write_len(h, weights.len());
                    for weight in weights {
                        self.write_value(h, weight);
                    }
                }
                None => {
                    h.update(&[0]);
                }
            }
        }
    }
}

const fn kind_tag(kind: DeclKind) -> u8 {
    match kind {
        DeclKind::Node => 1,
        DeclKind::Edge => 2,
        DeclKind::HyperArc => 3,
    }
}

fn write_len(h: &mut Hasher, len: usize) {
    h.update(&(len as u64).to_le_bytes());
}

fn write_str(h: &mut Hasher, value: &str) {
    write_len(h, value.len());
    h.update(value.as_bytes());
}

fn write_sorted_hashes(h: &mut Hasher, hashes: &mut [HashId]) {
    hashes.sort_unstable_by_key(|hash| hash.0);
    write_len(h, hashes.len());
    for hash in hashes {
        h.update(&hash.0);
    }
}
