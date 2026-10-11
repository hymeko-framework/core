//! Semantic identity of the canonical Core IR. Encoding versions are declared below.
//!
//! The structural hash remains unchanged. This module additionally covers
//! declaration annotations and every concrete node, edge and arc record,
//! including records that have no declaration dispatch binding. Document provenance
//! and derived indexes are not semantic payload.
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
//! # Preconditions
//! Symbol and concrete-record indices must be valid for the supplied IR and
//! interner. Canonical reindexing invariance additionally assumes a coherent
//! named hierarchy. This module is not a malformed-IR validator.
//! # Panics
//! Invalid symbol or bound-record indices may panic. Bounded path construction
//! does not make an arbitrary malformed IR valid.

use std::borrow::Cow;
use std::sync::Arc;

use crate::common::ids::{DeclId, HyperArcId};
use crate::common::pathkey::PathKey;
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

/// Structural identity: excludes annotation values and incidence weights.
///
/// This and [`SemanticModelDigest`] identify different aspects of the same model.
/// Encoding versions belong to the hash header, not the Rust type name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProgramStructureHash(pub HashId);

/// Semantic identity under the canonical/raw-ID domains documented by this module.
/// Includes annotation values and incidence weights omitted by [`ProgramStructureHash`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemanticModelDigest(pub HashId);

impl Canon<'_> {
    /// Return one contribution per declaration, in DeclId order.
    ///
    /// This diagnostic table includes each declaration's bound record, when present.
    /// Use [`Canon::hash`] for whole-model coverage of unbound records.
    /// # Preconditions
    /// The module's valid-index preconditions apply.
    /// # Postconditions
    /// Pure and deterministic; derived hashes are not serialized IR state.
    /// # Panics
    /// Invalid symbol or bound-record indices may panic.
    #[must_use]
    pub fn declaration_hashes(&self) -> Vec<HashId> {
        let layout = SemanticLayout::borrowed(self);
        let context = SemanticContext {
            ir: self.ir(),
            it: self.interner(),
            layout: &layout,
        };
        let records = context.record_hashes();
        context.declaration_hashes(&records)
    }

    /// Return the whole-model digest, with complete concrete-record coverage.
    ///
    /// # Preconditions
    /// The module's valid-index preconditions apply. Reindexing invariance is only
    /// promised in canonical mode; raw-ID fallback is explicitly domain-separated.
    /// # Postconditions
    /// Pure, deterministic and idempotent.
    /// Per-kind aggregation is unordered; incidence slots, arc slots, tags, bases,
    /// weights and value lists retain their order and multiplicity.
    /// # Panics
    /// Invalid symbol or bound-record indices may panic.
    #[must_use]
    pub fn hash(&self) -> SemanticModelDigest {
        let layout = SemanticLayout::borrowed(self);
        let context = SemanticContext {
            ir: self.ir(),
            it: self.interner(),
            layout: &layout,
        };
        let mut records = context.record_hashes();
        let mut declarations = context.declaration_hashes(&records);
        declarations.sort_unstable_by_key(|hash| hash.0);
        records.sort();
        model_digest(&context, &declarations, &records)
    }
}

/// Derived state owned exclusively by an immutable `CompiledModel`. Only the checked
/// value-update boundary may reuse it. Arbitrary IR edits must build a fresh state.
/// This is never serialized and does not add a second semantic encoding.
pub(crate) struct SemanticDigestState {
    layout: Arc<SemanticLayout<'static>>,
    records: Arc<RecordHashes>,
    sorted_records: Arc<RecordHashes>,
    declarations: Vec<HashId>,
    sorted_declarations: Vec<HashId>,
    digest: SemanticModelDigest,
}

impl SemanticDigestState {
    pub(crate) fn new(ir: &Ir, it: &Interner) -> Self {
        let layout = Arc::new(SemanticLayout::owned(&Canon::new(ir, it)));
        let context = SemanticContext {
            ir,
            it,
            layout: &layout,
        };
        let records = Arc::new(context.record_hashes());
        let declarations = context.declaration_hashes(&records);
        let mut sorted_declarations = declarations.clone();
        sorted_declarations.sort_unstable_by_key(|hash| hash.0);
        let mut sorted_records = (*records).clone();
        sorted_records.sort();
        let digest = model_digest(&context, &sorted_declarations, &sorted_records);
        Self {
            layout,
            records,
            sorted_records: Arc::new(sorted_records),
            declarations,
            sorted_declarations,
            digest,
        }
    }

    pub(crate) fn digest(&self) -> SemanticModelDigest {
        self.digest
    }

    /// The caller guarantees that only these declaration values changed, all IDs and
    /// existing symbol meanings are stable, and each declaration occurs at most once.
    /// A dense symbol remap rebuilds only the derived path layout, not record hashes.
    /// References encode target identities, not their values: changing a target's
    /// value does not dirty its referring records or its containment ancestors.
    pub(crate) fn with_values(
        &self,
        ir: &Ir,
        it: &Interner,
        changed: &[DeclId],
        symbols_renumbered: bool,
    ) -> Self {
        let layout = if symbols_renumbered {
            Arc::new(SemanticLayout::owned(&Canon::new(ir, it)))
        } else {
            Arc::clone(&self.layout)
        };
        let context = SemanticContext {
            ir,
            it,
            layout: &layout,
        };
        let mut declarations = self.declarations.clone();
        let mut removed = Vec::with_capacity(changed.len());
        let mut added = Vec::with_capacity(changed.len());
        for &id in changed {
            let hash = context.declaration_hash(id, &self.records);
            if hash != declarations[id.raw()] {
                removed.push(declarations[id.raw()]);
                added.push(hash);
                declarations[id.raw()] = hash;
            }
        }
        // Merge a sorted delta, retaining duplicate contributions. This avoids
        // re-sorting N entries or shifting the entire list once per edit.
        let digest_changed = !removed.is_empty();
        removed.sort_unstable_by_key(|hash| hash.0);
        added.sort_unstable_by_key(|hash| hash.0);
        let mut removed = removed.into_iter().peekable();
        let mut added = added.into_iter().peekable();
        let mut sorted_declarations = Vec::with_capacity(declarations.len());
        for &hash in &self.sorted_declarations {
            if removed.peek() == Some(&hash) {
                removed.next();
                continue;
            }
            while added.peek().is_some_and(|next| next.0 <= hash.0) {
                sorted_declarations.push(added.next().expect("peeked contribution"));
            }
            sorted_declarations.push(hash);
        }
        sorted_declarations.extend(added);
        let digest = if !digest_changed {
            self.digest
        } else {
            model_digest(&context, &sorted_declarations, &self.sorted_records)
        };
        Self {
            layout,
            records: Arc::clone(&self.records),
            sorted_records: Arc::clone(&self.sorted_records),
            declarations,
            sorted_declarations,
            digest,
        }
    }
}

/// Preserve the existing digest bytes: the final fold visits sorted 32-byte
/// contributions, while incremental updates avoid walking unchanged IR payloads.
fn model_digest(
    context: &SemanticContext<'_>,
    declarations: &[HashId],
    records: &RecordHashes,
) -> SemanticModelDigest {
    let mut h = context.header(DOMAIN_MODEL);
    for (domain, hashes) in [
        (DOMAIN_DECL, declarations),
        (DOMAIN_NODE, records.nodes.as_slice()),
        (DOMAIN_EDGE, records.edges.as_slice()),
        (DOMAIN_ARC, records.arcs.as_slice()),
    ] {
        h.update(domain);
        write_len(&mut h, hashes.len());
        for hash in hashes {
            h.update(&hash.0);
        }
    }
    SemanticModelDigest(HashId(*h.finalize().as_bytes()))
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

#[derive(Clone)]
struct RecordHashes {
    nodes: Vec<HashId>,
    edges: Vec<HashId>,
    arcs: Vec<HashId>,
}

impl RecordHashes {
    fn sort(&mut self) {
        for hashes in [&mut self.nodes, &mut self.edges, &mut self.arcs] {
            hashes.sort_unstable_by_key(|hash| hash.0);
        }
    }
}

/// Borrowed encoding context. Layout can be retained only across checked value edits.
struct SemanticContext<'a> {
    ir: &'a Ir,
    it: &'a Interner,
    layout: &'a SemanticLayout<'a>,
}

/// The derived lookup a digest encodes targets through: canonical paths, arc owner slots and the
/// target scheme. A one-shot digest borrows the canonical view's paths; retained state owns them.
struct SemanticLayout<'p> {
    paths: Cow<'p, [Option<PathKey>]>,
    first_owner_slots: Vec<Option<usize>>,
    scheme: TargetScheme,
}

impl<'p> SemanticLayout<'p> {
    fn borrowed(canon: &'p Canon<'_>) -> Self {
        Self::with_paths(canon, Cow::Borrowed(canon.paths()))
    }

    fn owned(canon: &Canon<'_>) -> SemanticLayout<'static> {
        SemanticLayout::with_paths(canon, Cow::Owned(canon.paths().to_vec()))
    }

    fn with_paths(canon: &Canon<'_>, paths: Cow<'p, [Option<PathKey>]>) -> Self {
        let ir = canon.ir();
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
            paths,
            first_owner_slots,
            scheme: if canonical {
                TargetScheme::Path
            } else {
                TargetScheme::DeclIdFallback
            },
        }
    }
}

impl SemanticContext<'_> {
    fn header(&self, domain: &[u8; 8]) -> DigestWriter {
        let mut h = DigestWriter::new();
        h.update(MAGIC);
        h.update(domain);
        h.update(&SEMANTIC_DIGEST_SCHEMA_VERSION.to_le_bytes());
        h.update(&SEMANTIC_DIGEST_ALGO_VERSION.to_le_bytes());
        h.update(&[self.layout.scheme.tag()]);
        h
    }

    fn record_hashes(&self) -> RecordHashes {
        #[cfg(test)]
        HASH_WORK.with(|work| {
            let mut counts = work.get();
            counts[1] += self.ir.nodes.len();
            counts[2] += self.ir.edges.len();
            counts[3] += self.ir.arcs.len();
            work.set(counts);
        });
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

    fn record_header(
        &self,
        domain: &[u8; 8],
        declaration: DeclId,
        bases: &[SignedRefR],
    ) -> DigestWriter {
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
            .map(|(di, _)| self.declaration_hash(DeclId::new(di), records))
            .collect()
    }

    fn declaration_hash(&self, d: DeclId, records: &RecordHashes) -> HashId {
        #[cfg(test)]
        HASH_WORK.with(|work| {
            let mut counts = work.get();
            counts[0] += 1;
            work.set(counts);
        });
        let declaration = &self.ir.decl_nodes[d.raw()];
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
    }

    fn write_target(&self, h: &mut DigestWriter, d: DeclId) {
        if d.is_none() {
            h.update(&[0]);
            return;
        }
        if self.layout.scheme == TargetScheme::Path {
            if let Some(path) = self.layout.paths.get(d.raw()).and_then(Option::as_ref) {
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

    fn write_arc_identity(&self, h: &mut DigestWriter, arc: HyperArcId) {
        if self.layout.scheme == TargetScheme::Path {
            h.update(&[0]);
            self.write_target(h, self.ir.arcs[arc.raw()].in_edge);
            // Construction selects Path only after every owner slot is Some.
            let position = self.layout.first_owner_slots[arc.raw()]
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
    fn write_model_facet(&self, h: &mut DigestWriter, facet: Option<&ModelFacet>) {
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

    fn write_anno(&self, h: &mut DigestWriter, anno: &AnnoR) {
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

    fn write_value(&self, h: &mut DigestWriter, value: &ValueR) {
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

    fn write_signed_refs(&self, h: &mut DigestWriter, refs: &[SignedRefR]) {
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

#[cfg(test)]
thread_local! { static HASH_WORK: std::cell::Cell<[usize; 4]> = const { std::cell::Cell::new([0; 4]) }; }

#[cfg(test)]
pub(crate) fn take_hash_work() -> [usize; 4] {
    HASH_WORK.with(|work| work.replace([0; 4]))
}

/// A blake3 hasher fed through a small stack buffer.
///
/// The digest writes many small fields -- tags, lengths, path segments -- and blake3's per-call
/// bookkeeping, not its compression, was most of their cost: 5.2 million updates for one
/// Bitcoin-OTC graph (APPROVED-CORE-EDIT: semantic-digest-path-cache, 2026-09-29). blake3 is a
/// streaming hash, so the digest does not depend on how its input is split into updates, and every
/// digest byte is unchanged.
struct DigestWriter {
    hasher: blake3::Hasher,
    buffer: [u8; DigestWriter::CAPACITY],
    len: usize,
}

impl DigestWriter {
    /// Bytes buffered before one update: a record's fields usually fit, and the writer stays cheap
    /// to move.
    const CAPACITY: usize = 256;

    fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
            buffer: [0; Self::CAPACITY],
            len: 0,
        }
    }

    /// Append `bytes`; returns the writer, as `blake3::Hasher::update` does.
    fn update(&mut self, bytes: &[u8]) -> &mut Self {
        if self.len + bytes.len() > Self::CAPACITY {
            self.flush();
        }
        if bytes.len() >= Self::CAPACITY {
            self.hasher.update(bytes);
        } else {
            self.buffer[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
        self
    }

    fn flush(&mut self) {
        self.hasher.update(&self.buffer[..self.len]);
        self.len = 0;
    }

    fn finalize(mut self) -> blake3::Hash {
        self.flush();
        self.hasher.finalize()
    }
}

fn write_len(h: &mut DigestWriter, len: usize) {
    h.update(&(len as u64).to_le_bytes());
}

fn write_str(h: &mut DigestWriter, value: &str) {
    write_len(h, value.len());
    h.update(value.as_bytes());
}

#[cfg(test)]
mod digest_writer_tests {
    use super::DigestWriter;

    /// The buffered writer is the blake3 stream it feeds, however the input is split: single
    /// bytes, fields that fill the buffer exactly, fields longer than the buffer, and a mix across
    /// every boundary. This is what keeps each semantic digest byte-identical.
    #[test]
    fn a_buffered_digest_is_the_stream_it_feeds() {
        let capacity = DigestWriter::CAPACITY;
        let input: Vec<u8> = (0..5 * capacity + 17)
            .map(|i| (i * 31 % 251) as u8)
            .collect();
        let reference = blake3::hash(&input);
        let splits: [&[usize]; 5] = [
            &[1],
            &[capacity],
            &[capacity + 1, 3],
            &[7, capacity - 7, 2 * capacity + 5, 1],
            &[3, 5, 8, 13, 21, 34, 55, 89, 144, 233],
        ];
        for sizes in splits {
            let mut writer = DigestWriter::new();
            let (mut at, mut next) = (0, 0);
            while at < input.len() {
                let size = sizes[next % sizes.len()].min(input.len() - at);
                writer.update(&input[at..at + size]);
                at += size;
                next += 1;
            }
            assert_eq!(writer.finalize(), reference, "splits {sizes:?}");
        }
        assert_eq!(DigestWriter::new().finalize(), blake3::hash(&[]));
    }
}
