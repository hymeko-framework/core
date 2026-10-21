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

mod fold;

use fold::{ModelHashInput, SemanticFold};
use std::sync::Arc;
use std::{borrow::Cow, ops::Range};

use crate::common::ids::{DeclId, HyperArcId};
use crate::common::pathkey::PathKey;
use crate::ir::canonical_hash::Canon;
use crate::ir::hash::HashId;
use crate::ir::ir::{
    AnnoR, DType, DeclKind, Extent, Ir, ModelFacet, PortDirection, SignedRefR, TensorRole, ValueR,
};
use crate::ir::table::IrTable;
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
    // Point edits detach only affected pages, using the canonical table owner.
    // Sorted aggregation stays contiguous; its virtual byte stream retains subtrees.
    declarations: IrTable<HashId>,
    sorted_declarations: Arc<Vec<HashId>>,
    fold: Arc<SemanticFold>,
}

/// Reusable hash-delta scratch. The returned digest state never borrows it.
#[derive(Debug, Default)]
pub(crate) struct SemanticDelta {
    removed: Vec<HashId>,
    added: Vec<HashId>,
}

impl SemanticDelta {
    pub(crate) fn with_capacity(
        capacity: usize,
    ) -> Result<Self, std::collections::TryReserveError> {
        let mut delta = Self::default();
        delta.removed.try_reserve_exact(capacity)?;
        delta.added.try_reserve_exact(capacity)?;
        Ok(delta)
    }

    pub(crate) fn clear(&mut self) {
        self.removed.clear();
        self.added.clear();
    }

    /// Merge the exact multiset delta and locate changed output ranks. Comparing
    /// while writing avoids a separate full scan solely for cache invalidation.
    fn merge_sorted(&mut self, base: &[HashId]) -> (Vec<HashId>, Option<Range<usize>>) {
        self.removed.sort_unstable_by_key(|hash| hash.0);
        self.added.sort_unstable_by_key(|hash| hash.0);
        let mut removed = self.removed.iter().copied().peekable();
        let mut added = self.added.iter().copied().peekable();
        let mut sorted = Vec::with_capacity(base.len());
        let mut dirty: Option<Range<usize>> = None;
        let mut push = |hash| {
            let index = sorted.len();
            if hash != base[index] {
                dirty.get_or_insert(index..index + 1).end = index + 1;
            }
            sorted.push(hash);
        };
        for &hash in base {
            if removed.peek() == Some(&hash) {
                removed.next();
                continue;
            }
            while added.peek().is_some_and(|next| next.0 <= hash.0) {
                push(added.next().expect("peeked contribution"));
            }
            push(hash);
        }
        for hash in added {
            push(hash);
        }
        debug_assert_eq!(sorted.len(), base.len());
        debug_assert!(removed.next().is_none());
        (sorted, dirty)
    }

    #[cfg(test)]
    pub(crate) fn storage(&self) -> [(*const HashId, usize, usize); 2] {
        [
            (
                self.removed.as_ptr(),
                self.removed.capacity(),
                self.removed.len(),
            ),
            (self.added.as_ptr(), self.added.capacity(), self.added.len()),
        ]
    }
}

impl SemanticDigestState {
    // 8 KiB of hash payload per page. This is a storage granularity, not a
    // measured optimum or a change to canonical identity.
    const HASHES_PER_PAGE: usize = 256;

    #[cfg(test)]
    pub(crate) fn shares_layout(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.layout, &other.layout)
    }

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
        let sorted_declarations = Arc::new(sorted_declarations);
        let fold = Arc::new(SemanticFold::new(
            layout.scheme,
            Arc::clone(&sorted_declarations),
            Arc::new(sorted_records),
        ));
        let mut declarations = IrTable::from(declarations);
        // The fixed page size is a nonzero power of two, so conversion cannot fail.
        declarations
            .share_pages(Self::HASHES_PER_PAGE)
            .expect("fixed hash page size is a nonzero power of two");
        Self {
            layout,
            records,
            declarations,
            sorted_declarations,
            fold,
        }
    }

    pub(crate) fn digest(&self) -> SemanticModelDigest {
        self.fold.digest()
    }

    /// The caller guarantees that only these declaration values changed, all IDs and
    /// existing symbol meanings are stable, and each declaration occurs at most once.
    /// A remap of path symbols rebuilds only the derived path layout, not record
    /// hashes. Annotation-only symbol remaps leave the path layout valid.
    /// References encode target identities, not their values: changing a target's
    /// value does not dirty its referring records or its containment ancestors.
    pub(crate) fn with_values(
        &self,
        ir: &Ir,
        it: &Interner,
        changed: &[DeclId],
        path_symbols_changed: bool,
        delta: &mut SemanticDelta,
    ) -> Self {
        let layout = if path_symbols_changed {
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
        delta.clear();
        // A bounded update workspace already has this capacity. The convenience
        // entry point reserves once here instead of growing on each hash push.
        delta.removed.reserve(changed.len());
        delta.added.reserve(changed.len());
        let removed = &mut delta.removed;
        let added = &mut delta.added;
        for &id in changed {
            let hash = context.declaration_hash(id, &self.records);
            if hash != declarations[id.raw()] {
                removed.push(declarations[id.raw()]);
                added.push(hash);
                declarations[id.raw()] = hash;
            }
        }
        let (sorted_declarations, fold) = if removed.is_empty() {
            // An unchanged value must not copy or scan either hash table.
            (
                Arc::clone(&self.sorted_declarations),
                Arc::clone(&self.fold),
            )
        } else {
            // Merge a sorted delta, retaining duplicate contributions. This avoids
            // re-sorting N entries or shifting the entire list once per edit.
            let (sorted, dirty) = delta.merge_sorted(&self.sorted_declarations);
            if let Some(dirty) = dirty {
                let sorted = Arc::new(sorted);
                let fold = Arc::new(self.fold.with_declarations(Arc::clone(&sorted), dirty));
                (sorted, fold)
            } else {
                (
                    Arc::clone(&self.sorted_declarations),
                    Arc::clone(&self.fold),
                )
            }
        };
        Self {
            layout,
            records: Arc::clone(&self.records),
            declarations,
            sorted_declarations,
            fold,
        }
    }
}

/// Full reference fold over the same virtual byte stream used by the cache.
/// Incremental states retain BLAKE3 subtrees; no alternate encoding is introduced.
fn model_digest(
    context: &SemanticContext<'_>,
    declarations: &[HashId],
    records: &RecordHashes,
) -> SemanticModelDigest {
    ModelHashInput::new(context.layout.scheme, declarations, records).digest()
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
                self.write_anno(&mut h, &self.ir[arc.anno]);
                self.write_signed_refs(&mut h, &arc.refs);
                HashId(*h.finalize().as_bytes())
            })
            .collect::<Vec<_>>();
        let nodes = self
            .ir
            .nodes
            .iter()
            .map(|node| {
                let h = self.record_header(DOMAIN_NODE, node.decl, &self.ir[node.bases]);
                HashId(*h.finalize().as_bytes())
            })
            .collect();
        let edges = self
            .ir
            .edges
            .iter()
            .map(|edge| {
                let mut h = self.record_header(DOMAIN_EDGE, edge.decl, &self.ir[edge.bases]);
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
        self.write_anno(&mut h, &self.ir[declaration.anno]);
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
            let sign = match reference.sign_kind() {
                crate::ir::ir::IncidenceSign::Plus => 1,
                crate::ir::ir::IncidenceSign::Minus => 2,
                crate::ir::ir::IncidenceSign::Neutral => 3,
            };
            h.update(&[sign]);
            let atom = self
                .ir
                .reference_atom(reference)
                .expect("semantic hash requires model-local reference payloads");
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
            self.write_anno(h, atom.anno);
            match atom.weights {
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
mod incremental_storage_tests {
    use super::*;
    use crate::ir::ir::{IrConstruction, LeafValueEdit};

    struct DigestFixture {
        ir: Ir,
        interner: Interner,
    }

    impl DigestFixture {
        fn new(declarations: usize) -> Self {
            let mut fixture = Self {
                ir: Ir::default(),
                interner: Interner::default(),
            };
            for index in 0..declarations {
                let name = fixture.interner.intern(&format!("value_{index}"));
                IrConstruction::new(&mut fixture.ir)
                    .append_node(
                        name,
                        AnnoR {
                            value: Some(ValueR::Num(0.0)),
                            ..AnnoR::default()
                        },
                        Vec::new(),
                    )
                    .unwrap();
            }
            fixture
        }

        fn update(
            &mut self,
            previous: &SemanticDigestState,
            values: &[(usize, f64)],
            delta: &mut SemanticDelta,
        ) -> SemanticDigestState {
            let mut edits = values
                .iter()
                .map(|&(id, value)| LeafValueEdit::Replace {
                    declaration: DeclId::new(id),
                    value: Some(ValueR::Num(value)),
                })
                .collect();
            self.ir.set_leaf_values(&mut edits).unwrap();
            let changed: Vec<_> = values.iter().map(|&(id, _)| DeclId::new(id)).collect();
            let next = previous.with_values(&self.ir, &self.interner, &changed, false, delta);
            assert_eq!(next.digest(), Canon::new(&self.ir, &self.interner).hash());
            next
        }
    }

    #[test]
    fn sorted_delta_preserves_multiplicity_and_reports_exact_changed_interval() {
        let hash = |value: u8| HashId([value; 32]);
        for base in [vec![], vec![1, 2, 2, 4], vec![1, 2, 3, 4]] {
            for mask in 0..1usize << base.len() {
                if mask.count_ones() > 2 {
                    continue;
                }
                let removed: Vec<_> = base
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(index, _)| mask & (1 << index) != 0)
                    .map(|(_, value)| value)
                    .collect();
                // Exhaust every replacement over a small alphabet, including
                // repeated contributions, cancellations and both end shifts.
                for code in 0..6usize.pow(removed.len() as u32) {
                    let added: Vec<_> = (0..removed.len())
                        .map(|position| ((code / 6usize.pow(position as u32)) % 6) as u8)
                        .collect();
                    let mut expected: Vec<_> = base
                        .iter()
                        .copied()
                        .enumerate()
                        .filter(|(index, _)| mask & (1 << index) == 0)
                        .map(|(_, value)| value)
                        .chain(added.iter().copied())
                        .collect();
                    expected.sort_unstable();
                    let mismatches: Vec<_> = base
                        .iter()
                        .zip(&expected)
                        .map(|(old, new)| old != new)
                        .collect();
                    let dirty = mismatches.iter().position(|&changed| changed).map(|first| {
                        first..mismatches.iter().rposition(|&changed| changed).unwrap() + 1
                    });
                    let mut delta = SemanticDelta::with_capacity(removed.len()).unwrap();
                    delta.removed.extend(removed.iter().copied().map(hash));
                    delta.added.extend(added.iter().copied().map(hash));
                    let hashes: Vec<_> = base.iter().copied().map(hash).collect();
                    let (actual, changed) = delta.merge_sorted(&hashes);
                    assert_eq!(actual, expected.into_iter().map(hash).collect::<Vec<_>>());
                    assert_eq!(changed, dirty);
                }
            }
        }
    }

    #[test]
    fn retained_states_defer_fold_and_keep_old_identity_after_pending_updates() {
        let mut fixture = DigestFixture::new(1025);
        let original = Canon::new(&fixture.ir, &fixture.interner).hash();
        let base = SemanticDigestState::new(&fixture.ir, &fixture.interner);
        let mut delta = SemanticDelta::with_capacity(1).unwrap();
        let mut edits = vec![LeafValueEdit::Replace {
            declaration: DeclId::new(512),
            value: Some(ValueR::Num(7.0)),
        }];
        fixture.ir.set_leaf_values(&mut edits).unwrap();
        let next = base.with_values(
            &fixture.ir,
            &fixture.interner,
            &[DeclId::new(512)],
            false,
            &mut delta,
        );
        assert!(!base.fold.is_materialized());
        assert!(!next.fold.is_materialized());
        assert_eq!(
            next.digest(),
            Canon::new(&fixture.ir, &fixture.interner).hash()
        );
        assert!(!base.fold.is_materialized());
        assert_eq!(base.digest(), original);
    }

    #[test]
    fn model_encoding_known_answers() {
        for (count, expected) in [
            (
                0,
                "c848e56f5f4cf6d10ecce85c967a79af8a0ca723712bb5ddd551b1b1bc4ffd4c",
            ),
            (
                1,
                "daa9c2f52d32dfe50a7563b8b321ed3f7db65b37d7b4e08a85850480fbf7999e",
            ),
            (
                15,
                "245ce7bcae47d186f9d8b87b2578fc84be9ad65e50f48dadd8d74a4f09edebab",
            ),
            (
                16,
                "88dc5896edc26abdbc6bf2682ffeff5810c8d09d51877ebda40222fdf25575a8",
            ),
            (
                255,
                "4ff97f932723faeba4c779d47ac4c13d8b1f1c34db95d4fe9ee19f52ad8cda93",
            ),
            (
                256,
                "1bc498d2191519b2f607ac7cbdd2e053f5ef70567d04b47640def7a327e511ec",
            ),
            (
                511,
                "32c8834821d158466627f8b38d18fdecc1e0e29b668995a38b540b661ebcfb75",
            ),
            (
                512,
                "f14fcf13761cbb229be754e3321eca7813735b398b641cbfab10005eb4c14dfc",
            ),
            (
                1025,
                "bd4115c3f4683cbc0b80f6b9aed685b5bca121195e8f109a3b07b6a64d51466e",
            ),
        ] {
            let fixture = DigestFixture::new(count);
            let hash = Canon::new(&fixture.ir, &fixture.interner).hash();
            let hex: String = hash.0.0.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(hex, expected, "model size {count}");
            let retained = SemanticDigestState::new(&fixture.ir, &fixture.interner);
            assert!(!retained.fold.is_materialized());
            assert_eq!(retained.digest(), hash);
            assert!(retained.fold.is_materialized());
        }
    }

    #[test]
    fn updates_share_untouched_hash_pages_and_preserve_old_snapshots() {
        let mut fixture = DigestFixture::new(769);
        let base = SemanticDigestState::new(&fixture.ir, &fixture.interner);
        let original_hashes = base.declarations.to_vec();
        let original_digest = base.digest();
        let mut delta = SemanticDelta::with_capacity(4).unwrap();
        let storage = delta.storage();
        let next = fixture.update(
            &base,
            &[(0, 1.0), (255, 2.0), (256, 3.0), (768, 4.0)],
            &mut delta,
        );
        for index in 0..769 {
            assert_eq!(
                std::ptr::eq(&base.declarations[index], &next.declarations[index]),
                (512..768).contains(&index),
                "only pages 0, 1 and 3 may detach; declaration {index}"
            );
        }
        let next_digest = next.digest();
        let branch = fixture.update(&next, &[(256, -0.0)], &mut delta);
        for index in 0..769 {
            assert_eq!(
                std::ptr::eq(&next.declarations[index], &branch.declarations[index]),
                !(256..512).contains(&index),
                "second snapshot detaches only page 1; declaration {index}"
            );
        }
        assert_eq!(base.digest(), original_digest);
        assert_eq!(base.declarations.to_vec(), original_hashes);
        assert_eq!(next.digest(), next_digest);
        delta.clear();
        assert_eq!(delta.storage(), storage);
    }

    #[test]
    fn unchanged_values_share_all_hash_storage_at_page_boundaries() {
        for count in [0, 1, 255, 256, 257, 769] {
            let mut fixture = DigestFixture::new(count);
            let base = SemanticDigestState::new(&fixture.ir, &fixture.interner);
            let edits: Vec<_> = (0..count).map(|index| (index, 0.0)).collect();
            let mut delta = SemanticDelta::with_capacity(count).unwrap();
            let next = fixture.update(&base, &edits, &mut delta);
            assert_eq!(base.digest(), next.digest());
            assert_eq!(
                base.sorted_declarations.as_ptr(),
                next.sorted_declarations.as_ptr(),
                "unchanged sorted contributions must be shared at size {count}"
            );
            for index in 0..count {
                assert!(std::ptr::eq(
                    &base.declarations[index],
                    &next.declarations[index]
                ));
            }
            let rebuilt_layout =
                next.with_values(&fixture.ir, &fixture.interner, &[], true, &mut delta);
            assert!(!base.shares_layout(&rebuilt_layout));
            assert_eq!(base.digest(), rebuilt_layout.digest());
            assert_eq!(
                base.sorted_declarations.as_ptr(),
                rebuilt_layout.sorted_declarations.as_ptr()
            );
        }
    }

    #[test]
    fn paged_hash_updates_match_full_digest_for_repeated_and_bulk_changes() {
        let mut fixture = DigestFixture::new(513);
        let mut state = SemanticDigestState::new(&fixture.ir, &fixture.interner);
        let mut delta = SemanticDelta::with_capacity(513).unwrap();
        let positive_zero = state.digest();
        state = fixture.update(&state, &[(256, -0.0)], &mut delta);
        assert_ne!(state.digest(), positive_zero);
        for step in 0..12 {
            // Dense and sparse edits, repeated values, and both sides of a page boundary.
            let edits: Vec<_> = (0..513)
                .filter(|index| step % 3 == 0 || index % 256 == step % 2)
                .map(|index| (index, (step / 2 + index % 7) as f64))
                .collect();
            state = fixture.update(&state, &edits, &mut delta);
        }
        let reset: Vec<_> = (0..513).map(|index| (index, 0.0)).collect();
        state = fixture.update(&state, &reset, &mut delta);
        assert_eq!(state.digest(), positive_zero);
    }
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
