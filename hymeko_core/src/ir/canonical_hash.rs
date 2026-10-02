use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::common::ids::DeclId;
use crate::common::pathkey::PathKey;
use crate::ir::hash::HashId;
use crate::ir::ir::{DeclKind, DeclNode, EdgeRec, Ir, SignedRefR};
use crate::ir::semantic_digest::{ProgramStructureHashV1, SemanticModelDigestV2};
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;

/// Canonical-hash konfiguráció (timestamp NEM ide)
#[derive(Copy, Clone, Debug)]
pub struct CanonHashCfg {
    pub schema_version: u16,
    pub algo_version: u16,
    pub flags: u32,
}

impl Default for CanonHashCfg {
    fn default() -> Self {
        Self {
            schema_version: 1,
            algo_version: 1,
            flags: 0,
        }
    }
}

impl CanonHashCfg {
    fn hasher(self, domain: [u8; 8]) -> blake3::Hasher {
        let mut header = [0; 24];
        header[..8].copy_from_slice(b"HYMEKOCN");
        header[8..16].copy_from_slice(&domain);
        header[16..18].copy_from_slice(&self.schema_version.to_le_bytes());
        header[18..20].copy_from_slice(&self.algo_version.to_le_bytes());
        header[20..].copy_from_slice(&self.flags.to_le_bytes());
        let mut hash = blake3::Hasher::new();
        hash.update(&header);
        hash
    }

    fn hash_chunk(self, domain: [u8; 8], payload: &[u8]) -> HashId {
        let mut hash = self.hasher(domain);
        hash.update(payload);
        HashId(*hash.finalize().as_bytes())
    }
}

/// The canonical view of one [`Ir`]: the IR, its interner, and every named declaration's canonical
/// path, derived once. Every identity of the IR is a method of this view --
/// [`Self::structure`] (V1), `Self::hash` (the semantic V2 digest, in `ir::semantic_digest`) and
/// `Self::declaration_hashes` -- so the paths are derived once however many are asked for.
///
/// Borrowing the IR prevents changes that would invalidate the derived paths.
///
/// # Invariants
/// The path table has one entry per declaration of the borrowed IR, `None` for an anonymous arc.
pub struct Canon<'ir> {
    paths: CanonicalPaths<'ir>,
    it: &'ir Interner,
    /// The first two declarations that share a path, in path order; `None` when every path is
    /// unique, which is what the semantic digest's canonical scheme requires.
    duplicate: Option<PathIndexError>,
}

impl<'ir> Canon<'ir> {
    /// The canonical view of `ir`, whose symbols resolve through `it`.
    ///
    /// # Preconditions
    /// None: total over any `Ir`. A malformed hierarchy (cycle, dangling parent) terminates
    /// deterministically (bounded walks), and a duplicate path is recorded, not refused.
    #[must_use]
    pub fn new(ir: &'ir Ir, it: &'ir Interner) -> Self {
        let paths = CanonicalPaths::new(ir);
        let duplicate = sorted_paths(&paths.did_to_path).err();
        Self { paths, it, duplicate }
    }

    /// The IR this view reads.
    #[must_use]
    pub fn ir(&self) -> &'ir Ir {
        self.paths.ir
    }

    /// The interner its symbols resolve through.
    #[must_use]
    pub fn interner(&self) -> &'ir Interner {
        self.it
    }

    /// The canonical path of a declaration, or `None` for an anonymous arc or an out-of-range id.
    #[must_use]
    pub fn path_of(&self, d: DeclId) -> Option<&PathKey> {
        self.paths.did_to_path.get(d.raw())?.as_ref()
    }

    /// Whether every named declaration has its own path.
    ///
    /// # Errors
    /// [`PathIndexError::DuplicatePath`] naming the first two declarations, in path order, that
    /// share one.
    pub fn unique_paths(&self) -> Result<(), PathIndexError> {
        self.duplicate.map_or(Ok(()), Err)
    }

    /// Canonical program hash V1: `H( header || H(skeleton_prufer) || H(incidence_bytes) )`.
    /// Structural only: annotation values and incidence weights are not covered; `Self::hash` is
    /// the identity that covers them. See [`CanonicalPaths::hash`] for its preconditions.
    ///
    /// # Postconditions
    /// Deterministic; independent of dense `DeclId` assignment (name/path-based).
    #[must_use]
    pub fn structure(&self, cfg: CanonHashCfg) -> ProgramStructureHashV1 {
        ProgramStructureHashV1(self.paths.hash(cfg, self.it))
    }

    /// The lowering-boundary check: the IR's links reproduce the resolution `Index` hierarchy.
    pub(crate) fn validate(&self, idx: &Index) -> Result<(), NamespaceError> {
        NamespaceValidator::new(&self.paths).validate(idx)
    }

    /// The per-declaration path table, for the semantic digest's layout.
    pub(crate) fn paths(&self) -> &[Option<PathKey>] {
        &self.paths.did_to_path
    }
}

/// The identity a compile keeps of the program it compiled: its semantic digest and its structural
/// hash, computed once, through one [`Canon`], where the IR is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CanonicalIdentity {
    hash: SemanticModelDigestV2,
    structure: ProgramStructureHashV1,
}

impl CanonicalIdentity {
    /// Both identities of `canon`'s IR, the structural one under `cfg`.
    #[must_use]
    pub fn of(canon: &Canon<'_>, cfg: CanonHashCfg) -> Self {
        Self {
            hash: canon.hash(),
            structure: canon.structure(cfg),
        }
    }

    /// An identity whose parts were computed elsewhere: a checked value update keeps the
    /// structural hash (values are not structure) and supplies its incremental semantic digest.
    pub(crate) const fn from_parts(
        hash: SemanticModelDigestV2,
        structure: ProgramStructureHashV1,
    ) -> Self {
        Self { hash, structure }
    }

    /// The semantic identity: every declaration, annotation value, record and arc.
    #[must_use]
    pub const fn hash(&self) -> SemanticModelDigestV2 {
        self.hash
    }

    /// The structural identity, blind to annotation values and incidence weights.
    #[must_use]
    pub const fn structure(&self) -> ProgramStructureHashV1 {
        self.structure
    }
}

/// Transient canonical paths shared by lowering-boundary validation and hashing.
/// Borrowing the IR prevents changes that would invalidate these derived paths.
pub(crate) struct CanonicalPaths<'ir> {
    ir: &'ir Ir,
    did_to_path: Vec<Option<PathKey>>,
}

impl<'ir> CanonicalPaths<'ir> {
    /// Derive named declaration paths from IR links and namespace metadata.
    /// Parent walks are bounded even for malformed cycles; arcs have no path.
    #[must_use]
    pub(crate) fn new(ir: &'ir Ir) -> Self {
        let n = ir.decl_nodes.len();
        let mut v: Vec<Option<PathKey>> = vec![None; n];
        let mut buf: Vec<crate::common::ids::SymId> = Vec::new();
        for i in 0..n {
            match ir.decl_nodes[i].kind {
                DeclKind::Node | DeclKind::Edge => {
                    buf.clear();
                    let mut cur = DeclId::new(i);
                    let mut root = cur;
                    let mut guard = 0usize;
                    while cur.is_some() && (cur.0) < n {
                        buf.push(ir.decl_nodes[cur.0].name);
                        root = cur;
                        cur = ir.decl_nodes[cur.0].parent;
                        guard += 1;
                        if guard > n {
                            break; // malformed cycle: stop deterministically
                        }
                    }
                    buf.reverse();
                    // U2.1: prepend the root's import namespace prefix so imported
                    // declarations reconstruct their full canonical path.
                    let full: Vec<crate::common::ids::SymId> = match ir.namespace_prefix(root) {
                        Some(prefix) => prefix.iter().copied().chain(buf.iter().copied()).collect(),
                        None => buf.clone(),
                    };
                    v[i] = Some(PathKey(full));
                }
                DeclKind::HyperArc => {}
            }
        }
        Self { ir, did_to_path: v }
    }

    /// Canonical program hash v1: `H( header || H(skeleton_prufer) || H(incidence_bytes) )`.
    ///
    /// Reads the declaration hierarchy exclusively from the `Ir` â the intrusive
    /// links (`DeclNode.parent`, `name`, `kind`) plus `Ir.namespace_roots` for the
    /// import prefix â with **no** `resolution::Index`. The post-lowering `Ir` is the
    /// single authority (U2.2). `algo_version = 1` and every hash byte are unchanged
    /// from the former index-based form: proven bit-identical on the 96-fixture
    /// corpus and the frozen v1 known-answer vectors.
    ///
    /// # Preconditions
    /// Named declarations form an acyclic hierarchy, and referenced declaration,
    /// arc and symbol indices are valid. Path reconstruction bounds parent walks;
    /// structural coherence is a separate diagnostic (`coherence`, [`validate_namespace_roots`]).
    ///
    /// # Postconditions
    /// Deterministic; independent of dense `DeclId` assignment (name/path-based).
    /// # Panics
    /// Invalid indices or a cyclic skeleton may panic; this is not an IR validator.
    #[must_use]
    pub(crate) fn hash(&self, cfg: CanonHashCfg, it: &Interner) -> HashId {
        let sk = CanonicalPrufer::from_paths(cfg, self, it).hash();
        let inc = CanonicalIncidence::from_paths(cfg, self, it).hash();
        let mut h = cfg.hasher(*b"PROGRAM\0");
        h.update(&sk.0);
        h.update(&inc.0);
        HashId(*h.finalize().as_bytes())
    }

    fn strings(&self, it: &Interner) -> Vec<String> {
        self.did_to_path
            .iter()
            .enumerate()
            .map(|(index, path)| match path {
                Some(path) => path
                    .0
                    .iter()
                    .map(|symbol| it.resolve(*symbol))
                    .collect::<Vec<_>>()
                    .join("."),
                None => format!("DeclId({index})"),
            })
            .collect()
    }
}

/// Named declaration topology, shared by labeling and leaf elimination.
struct SkeletonForest<'ir> {
    nodes: &'ir [DeclNode],
    vertices: Vec<DeclId>,
    parent: Vec<Option<DeclId>>,
    children: Vec<Vec<DeclId>>,
    roots: Vec<DeclId>,
}

impl<'ir> SkeletonForest<'ir> {
    fn node_domain(kind: DeclKind) -> [u8; 8] {
        match kind {
            DeclKind::Node => *b"SKNOD\0\0\0",
            DeclKind::Edge => *b"SKEDG\0\0\0",
            DeclKind::HyperArc => *b"SKARC\0\0\0",
        }
    }

    fn new(ir: &'ir Ir) -> Self {
        let n = ir.decl_nodes.len();
        let vertices: Vec<_> = ir
            .decl_nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.kind, DeclKind::Node | DeclKind::Edge))
            .map(|(i, _)| DeclId::new(i))
            .collect();
        let mut forest = Self {
            nodes: &ir.decl_nodes,
            vertices,
            parent: vec![None; n],
            children: vec![Vec::new(); n],
            roots: Vec::new(),
        };
        for &did in &forest.vertices {
            let parent = forest.nodes[did.raw()].parent;
            let named_parent = parent.is_some()
                && parent.raw() < n
                && matches!(
                    forest.nodes[parent.raw()].kind,
                    DeclKind::Node | DeclKind::Edge
                );
            if named_parent {
                forest.parent[did.raw()] = Some(parent);
                forest.children[parent.raw()].push(did);
            } else {
                forest.roots.push(did);
            }
        }
        forest
    }

    // Each named vertex has at most one parent, so reversing root-first order
    // places every reachable child before its parent. A cycle cannot be reached
    // from a root in this representation; disconnected components remain zero.
    fn subtree_hashes(&self, cfg: CanonHashCfg) -> Vec<HashId> {
        let mut order = Vec::with_capacity(self.vertices.len());
        order.extend_from_slice(&self.roots);
        let mut cursor = 0;
        let mut max_fanout = 0;
        while cursor < order.len() {
            let children = &self.children[order[cursor].raw()];
            max_fanout = max_fanout.max(children.len());
            order.extend_from_slice(children);
            cursor += 1;
        }

        let mut hashes = vec![HashId([0; 32]); self.nodes.len()];
        let mut leaf_hashes: [Option<HashId>; 2] = [None; 2];
        // No sorting storage is needed for a forest of chains or isolated roots.
        let mut sorted = Vec::with_capacity(if max_fanout > 1 { max_fanout } else { 0 });
        for vertex in order.into_iter().rev() {
            let index = vertex.raw();
            let children = &self.children[index];
            let kind = self.nodes[index].kind;
            if children.is_empty() {
                // SkeletonForest contains only Node/Edge vertices. Equal-kind
                // leaves have identical configured hashes, independent of paths.
                let slot = usize::from(matches!(kind, DeclKind::Edge));
                hashes[index] = *leaf_hashes[slot]
                    .get_or_insert_with(|| cfg.hash_chunk(Self::node_domain(kind), b""));
                continue;
            }

            let mut hash = cfg.hasher(Self::node_domain(kind));
            if let [child] = children.as_slice() {
                hash.update(&hashes[child.raw()].0);
            } else {
                sorted.clear();
                sorted.extend(children.iter().map(|child| hashes[child.raw()].0));
                // Equal keys are identical bytes: stability cannot affect the
                // digest, and unstable sorting needs no auxiliary allocation.
                sorted.sort_unstable();
                hash.update(sorted.as_flattened());
            }
            hashes[index] = HashId(*hash.finalize().as_bytes());
        }
        hashes
    }

    fn tie_breaks(
        &self,
        cfg: CanonHashCfg,
        did_to_path: &[Option<PathKey>],
        it: &Interner,
    ) -> Vec<u64> {
        let mut tb = vec![0u64; did_to_path.len()];
        for &v in &self.vertices {
            let i = v.0 as usize;
            if let Some(pk) = &did_to_path[i] {
                let mut h = cfg.hasher(*b"TIEBRK\0\0");
                // stable: stringify path
                for (j, seg) in pk.0.iter().enumerate() {
                    if j > 0 {
                        h.update(b".");
                    }
                    h.update(it.resolve(*seg).as_bytes());
                }
                let bytes = h.finalize();
                let b = bytes.as_bytes();
                tb[i] = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            }
        }
        tb
    }
}

/// Prepared canonical Pruefer encoding of the named declaration forest.
///
/// Subtree hashes and path tie-breaks label vertices; a synthetic root joins
/// the forest. Byte and hash outputs consume the same leaf-elimination iterator.
/// The borrowed IR keeps the prepared topology and labels coherent.
pub struct CanonicalPrufer<'ir> {
    cfg: CanonHashCfg,
    forest: SkeletonForest<'ir>,
    labels: Vec<PruferLeaf>,
}

impl<'ir> CanonicalPrufer<'ir> {
    /// Prepare the forest and its canonical labels for repeated output.
    ///
    /// # Preconditions
    /// Named declarations form an acyclic hierarchy; symbol indices are valid.
    /// # Postconditions
    /// Dense anonymous declarations are omitted from the skeleton. The input
    /// IR remains unchanged, and no encoded token buffer is retained.
    /// # Panics
    /// Invalid symbol indices may panic. Cyclic topology is not validated here;
    /// attempting to encode it may panic when no removable leaf remains.
    #[must_use]
    pub fn new(cfg: CanonHashCfg, ir: &'ir Ir, it: &Interner) -> Self {
        Self::from_paths(cfg, &CanonicalPaths::new(ir), it)
    }

    fn from_paths(cfg: CanonHashCfg, paths: &CanonicalPaths<'ir>, it: &Interner) -> Self {
        let forest = SkeletonForest::new(paths.ir);
        let hashes = forest.subtree_hashes(cfg);
        let ties = forest.tie_breaks(cfg, &paths.did_to_path, it);
        let mut labels: Vec<_> = forest
            .vertices
            .iter()
            .enumerate()
            .map(|(v, did)| PruferLeaf {
                key_hash: hashes[did.raw()].0,
                key_tie: ties[did.raw()],
                v,
            })
            .collect();
        labels.push(PruferLeaf {
            key_hash: cfg.hash_chunk(*b"ROOTNODE", b"ROOT").0,
            key_tie: 0,
            v: labels.len(),
        });
        Self {
            cfg,
            forest,
            labels,
        }
    }

    fn encode(&self, sink: &mut impl CanonicalSink) {
        for token in PruferTraversal::new(self) {
            sink.write(&token.0);
        }
    }

    /// Return the ordered 32-byte tokens, without the SKELETON hash header.
    ///
    /// # Postconditions
    /// An N-vertex forest emits max(N - 1, 0) tokens, including the synthetic
    /// root in elimination. Each call starts an independent traversal.
    /// # Panics
    /// A cyclic named hierarchy may exhaust removable leaves before completion.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.forest.vertices.len().saturating_sub(1) * 32);
        self.encode(&mut bytes);
        bytes
    }

    /// Hash the configured SKELETON header and the same tokens as `bytes()`.
    ///
    /// # Postconditions
    /// Deterministic; streams tokens through a fixed buffer without allocating
    /// the complete encoded byte stream.
    /// # Panics
    /// A cyclic named hierarchy may exhaust removable leaves before completion.
    #[must_use]
    pub fn hash(&self) -> HashId {
        let mut hash = CanonicalHasher::new(self.cfg, *b"SKELETON");
        self.encode(&mut hash);
        hash.finish()
    }
}

/// One traversal owns all mutable elimination state; labels remain immutable.
struct PruferTraversal<'a> {
    labels: &'a [PruferLeaf],
    degree: Vec<usize>,
    neighbor_xor: Vec<usize>,
    alive: Vec<bool>,
    heap: BinaryHeap<PruferLeaf>,
    remaining: usize,
}

impl<'a> PruferTraversal<'a> {
    fn new(prufer: &'a CanonicalPrufer<'_>) -> Self {
        let forest = &prufer.forest;
        let total = prufer.labels.len();
        let mut traversal = Self {
            labels: &prufer.labels,
            degree: vec![0; total],
            neighbor_xor: vec![0; total],
            alive: vec![true; total],
            heap: BinaryHeap::new(),
            remaining: total.saturating_sub(2),
        };
        let mut index = vec![usize::MAX; forest.parent.len()];
        for (i, id) in forest.vertices.iter().enumerate() {
            index[id.raw()] = i;
        }
        for (a, id) in forest.vertices.iter().enumerate() {
            if let Some(parent) = forest.parent[id.raw()] {
                traversal.connect(a, index[parent.raw()]);
            }
        }
        for root in &forest.roots {
            traversal.connect(index[root.raw()], total - 1);
        }
        for &leaf in traversal.labels {
            if traversal.degree[leaf.v] == 1 {
                traversal.heap.push(leaf);
            }
        }
        traversal
    }

    fn connect(&mut self, a: usize, b: usize) {
        self.neighbor_xor[a] ^= b;
        self.neighbor_xor[b] ^= a;
        self.degree[a] += 1;
        self.degree[b] += 1;
    }
}

impl Iterator for PruferTraversal<'_> {
    type Item = HashId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let leaf = loop {
            // An acyclic skeleton joined to the synthetic root is a tree:
            // while tokens remain, at least one live degree-one vertex exists.
            // Retain the existing panic on a violated topology precondition.
            let leaf = self
                .heap
                .pop()
                .expect("Pruefer traversal requires an acyclic skeleton");
            if self.alive[leaf.v] && self.degree[leaf.v] == 1 {
                break leaf;
            }
        };
        let v = leaf.v;
        let u = self.neighbor_xor[v];
        debug_assert!(
            u < self.labels.len() && self.alive[u],
            "leaf had no neighbor"
        );
        let token = HashId(self.labels[u].key_hash);
        self.alive[v] = false;
        self.degree[v] = 0;
        self.degree[u] -= 1;
        self.neighbor_xor[u] ^= v;
        if self.degree[u] == 1 {
            self.heap.push(self.labels[u]);
        }
        self.remaining -= 1;
        Some(token)
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct PruferLeaf {
    key_hash: [u8; 32],
    key_tie: u64,
    v: usize,
}

impl Ord for PruferLeaf {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse the full key so BinaryHeap removes the smallest leaf first.
        other
            .key_hash
            .cmp(&self.key_hash)
            .then_with(|| other.key_tie.cmp(&self.key_tie))
            .then_with(|| other.v.cmp(&self.v))
    }
}

impl PartialOrd for PruferLeaf {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Prepared canonical incidence encoding shared by byte and digest consumers.
///
/// Borrows the IR, so its record order and resolved paths cannot become stale.
/// Annotation values and weights are excluded, as in the program structure hash.
/// Arc and slot positions retain the existing wrapping `u16` representation;
/// path strings retain the existing 65535-byte prefix limit.
pub struct CanonicalIncidence<'ir> {
    cfg: CanonHashCfg,
    ir: &'ir Ir,
    paths: Vec<String>,
    edges: Vec<&'ir EdgeRec>,
    ordered_records: Option<Vec<IncidenceRecord<'ir>>>,
}

impl<'ir> CanonicalIncidence<'ir> {
    /// Prepare resolved paths and canonical record order once for either output.
    ///
    /// # Preconditions
    /// Referenced declaration, arc and symbol indices must be valid.
    /// # Postconditions
    /// Duplicate edge paths and wrapped positions use the same record ordering
    /// as the existing structural encoding. The borrowed IR is unchanged.
    /// # Panics
    /// Invalid declaration, arc or symbol indices may panic.
    #[must_use]
    pub fn new(cfg: CanonHashCfg, ir: &'ir Ir, it: &Interner) -> Self {
        Self::from_paths(cfg, &CanonicalPaths::new(ir), it)
    }

    fn from_paths(cfg: CanonHashCfg, paths: &CanonicalPaths<'ir>, it: &Interner) -> Self {
        let ir = paths.ir;
        let paths = paths.strings(it);
        let mut edges: Vec<_> = ir.edges.iter().collect();
        edges.sort_unstable_by(|a, b| paths[a.decl.raw()].cmp(&paths[b.decl.raw()]));
        let needs_record_sort = edges
            .windows(2)
            .any(|pair| paths[pair[0].decl.raw()] == paths[pair[1].decl.raw()])
            || edges.iter().any(|edge| {
                edge.arcs.len() > usize::from(u16::MAX) + 1
                    || edge
                        .arcs
                        .iter()
                        .any(|arc| ir.arcs[arc.raw()].refs.len() > usize::from(u16::MAX) + 1)
            });
        let mut incidence = Self {
            cfg,
            ir,
            paths,
            edges,
            ordered_records: None,
        };
        if needs_record_sort {
            // Start in IR order to preserve the ordering of equal keys under
            // the existing unstable sort. Only descriptors are materialised.
            let mut records: Vec<_> = incidence.records(ir.edges.iter()).collect();
            records.sort_unstable_by(|a, b| {
                incidence.paths[a.edge.raw()]
                    .cmp(&incidence.paths[b.edge.raw()])
                    .then_with(|| a.arc.cmp(&b.arc))
                    .then_with(|| a.slot.cmp(&b.slot))
            });
            incidence.ordered_records = Some(records);
        }
        incidence
    }

    fn records<'a>(
        &'a self,
        edges: impl Iterator<Item = &'ir EdgeRec> + 'a,
    ) -> impl Iterator<Item = IncidenceRecord<'ir>> + 'a {
        edges.flat_map(move |edge| {
            edge.arcs.iter().enumerate().flat_map(move |(arc, id)| {
                self.ir.arcs[id.raw()]
                    .refs
                    .iter()
                    .enumerate()
                    .map(move |(slot, reference)| IncidenceRecord {
                        edge: edge.decl,
                        arc: arc as u16,
                        slot: slot as u16,
                        reference,
                    })
            })
        })
    }

    fn encode(&self, sink: &mut impl CanonicalSink) {
        let encode = |record: IncidenceRecord<'_>| record.encode(&self.paths, sink);
        match &self.ordered_records {
            Some(records) => records.iter().copied().for_each(encode),
            None => self.records(self.edges.iter().copied()).for_each(encode),
        }
    }

    /// Return the ordered incidence payload without a hash-domain header.
    ///
    /// # Postconditions
    /// Each ordered incidence is encoded exactly once; multiplicity is retained.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.encode(&mut bytes);
        bytes
    }

    /// Hash the configured INCIDENC header and the same payload as `bytes()`.
    ///
    /// # Postconditions
    /// Deterministic; uses a fixed buffer, not a model-sized encoded byte stream.
    #[must_use]
    pub fn hash(&self) -> HashId {
        let mut hash = CanonicalHasher::new(self.cfg, *b"INCIDENC");
        self.encode(&mut hash);
        hash.finish()
    }
}

#[derive(Clone, Copy)]
struct IncidenceRecord<'ir> {
    edge: DeclId,
    arc: u16,
    slot: u16,
    reference: &'ir SignedRefR,
}

impl IncidenceRecord<'_> {
    fn encode(self, paths: &[String], sink: &mut impl CanonicalSink) {
        let (sign, target) = match self.reference {
            SignedRefR::Plus(atom) => (1i8, atom.target),
            SignedRefR::Minus(atom) => (-1i8, atom.target),
            SignedRefR::Neutral(atom) => (0i8, atom.target),
        };
        sink.string(&paths[self.edge.raw()]);
        sink.write(&self.arc.to_le_bytes());
        sink.write(&self.slot.to_le_bytes());
        sink.write(&[sign as u8]);
        sink.string(&paths[target.raw()]);
    }
}

/// Output strategy only: field representation belongs to the shared encoder.
trait CanonicalSink {
    fn write(&mut self, bytes: &[u8]);

    fn string(&mut self, value: &str) {
        let bytes = value.as_bytes();
        let len = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
        self.write(&len.to_le_bytes());
        self.write(&bytes[..usize::from(len)]);
    }
}

impl CanonicalSink for Vec<u8> {
    fn write(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// Bounded output storage batches small fields into multi-chunk BLAKE3 updates.
/// Canonical field encoding stays in the same encoder used by the byte sink.
struct CanonicalHasher {
    hash: blake3::Hasher,
    buffer: [u8; 16 * 1024],
    used: usize,
}

impl CanonicalHasher {
    fn new(cfg: CanonHashCfg, domain: [u8; 8]) -> Self {
        Self {
            hash: cfg.hasher(domain),
            buffer: [0; 16 * 1024],
            used: 0,
        }
    }

    fn finish(mut self) -> HashId {
        self.hash.update(&self.buffer[..self.used]);
        HashId(*self.hash.finalize().as_bytes())
    }
}

impl CanonicalSink for CanonicalHasher {
    fn write(&mut self, mut bytes: &[u8]) {
        let available = self.buffer.len() - self.used;
        if bytes.len() > available {
            self.buffer[self.used..].copy_from_slice(&bytes[..available]);
            self.hash.update(&self.buffer);
            self.used = 0;
            bytes = &bytes[available..];
            // Long fields already provide bulk input: avoid copying them again.
            if bytes.len() >= self.buffer.len() {
                self.hash.update(bytes);
                return;
            }
        }
        self.buffer[self.used..self.used + bytes.len()].copy_from_slice(bytes);
        self.used += bytes.len();
    }
}

// -----------------------------
//  U2.1 lowering-boundary namespace validator
// -----------------------------

/// Checks the lowering boundary against the canonical paths of one borrowed IR.
/// Path preparation is shared with hashing; validation neither rebuilds paths
/// nor mutates the IR or the resolution index.
pub(crate) struct NamespaceValidator<'paths, 'ir> {
    paths: &'paths CanonicalPaths<'ir>,
}

impl<'paths, 'ir> NamespaceValidator<'paths, 'ir> {
    /// Borrow an existing path view for namespace diagnostics.
    #[must_use]
    pub(crate) fn new(paths: &'paths CanonicalPaths<'ir>) -> Self {
        Self { paths }
    }

    /// Lowering-boundary check (U2.1, Part C): prove the `Ir` reproduces the
    /// resolution `Index` hierarchy from `(namespace prefix + intrusive links)` for
    /// every declaration. This is the diagnostic that authorises treating the `Ir` as
    /// the post-lowering authority; it is **not** a hot-path tax — call it once at the
    /// lowering boundary, not per hash.
    ///
    /// # Errors
    /// [`NamespaceError::MissingNamespaceMetadata`] when an imported root lacks its
    /// prefix (an incomplete `Ir`); [`NamespaceError::PathMismatch`] on any other
    /// disagreement (corrupt metadata, malformed links).
    pub(crate) fn validate(&self, idx: &Index) -> Result<(), NamespaceError> {
        let ir = self.paths.ir;
        let n = ir.decl_nodes.len();
        let idx_paths = self.resolution_paths(idx);
        let link_paths = &self.paths.did_to_path;
        for i in 0..n {
            if let (Some(ip), Some(lp)) = (&idx_paths[i], &link_paths[i]) {
                if ip.0 != lp.0 {
                    let did = DeclId::new(i);
                    let p = ir.decl_nodes[i].parent;
                    let is_root = p.is_none()
                        || (p.0 < n
                            && !matches!(ir.decl_nodes[p.0].kind, DeclKind::Node | DeclKind::Edge));
                    if is_root && ir.namespace_prefix(did).is_none() && ip.0.len() > lp.0.len() {
                        return Err(NamespaceError::MissingNamespaceMetadata { root: did });
                    }
                    return Err(NamespaceError::PathMismatch { decl: did });
                }
            }
        }
        Ok(())
    }

    fn resolution_paths<'index>(&self, idx: &'index Index) -> Vec<Option<&'index PathKey>> {
        let mut v = vec![None; self.paths.did_to_path.len()];
        for (k, &did) in &idx.by_path {
            let i = did.0 as usize;
            if i < v.len() && v[i].is_none() {
                v[i] = Some(k);
            }
        }
        v
    }
}

/// A disagreement between the resolution `Index` and the `Ir` (namespace + links)
/// declaration hierarchy. Ids-only, deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceError {
    /// A declaration's `(namespace prefix + intrusive-link path)` does not equal
    /// its resolution-`Index` path.
    PathMismatch { decl: DeclId },
    /// An imported root's `Index` path carries a prefix but no namespace metadata
    /// was recorded for it — e.g. an `Ir` deserialized from a pre-U2.1 wire. Such
    /// an `Ir` must not be treated as a complete canonical authority.
    MissingNamespaceMetadata { root: DeclId },
}

impl core::fmt::Display for NamespaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PathMismatch { decl } => write!(
                f,
                "decl {} namespace+link path disagrees with the Index path",
                decl.raw()
            ),
            Self::MissingNamespaceMetadata { root } => write!(
                f,
                "imported root {} has an Index prefix but no namespace metadata",
                root.raw()
            ),
        }
    }
}
impl std::error::Error for NamespaceError {}

/// Lowering-boundary check (U2.1, Part C) over a fresh canonical path view of `ir`; see
/// [`Canon`] for a view that is also hashed.
///
/// # Errors
/// [`NamespaceError::MissingNamespaceMetadata`] when an imported root lacks its
/// prefix (an incomplete `Ir`); [`NamespaceError::PathMismatch`] on any other
/// disagreement (corrupt metadata, malformed links).
pub fn validate_namespace_roots(ir: &Ir, idx: &Index) -> Result<(), NamespaceError> {
    NamespaceValidator::new(&CanonicalPaths::new(ir)).validate(idx)
}

// -----------------------------
//  U2.2 derived path index (Ir -> path view)
// -----------------------------

/// A path lookup structure **derived** from the `Ir` (links + `namespace_roots`),
/// for post-lowering consumers (pretty-print, queries) that need paths. It is a
/// derived view, not serialized canonical state: build it explicitly, own it at
/// the call site, and rebuild on `Ir` mutation (a future mutable IR must
/// document its invalidation point). There is no hidden cache, lock, or
/// automatic per-lookup rebuild.
pub struct DerivedPathIndex {
    by_path: Vec<(PathKey, DeclId)>, // sorted by path, then DeclId
    did_to_path: Vec<Option<PathKey>>,
}

/// Building a [`DerivedPathIndex`] failed. Ids-only, deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathIndexError {
    /// Two declarations resolve to the same canonical path.
    DuplicatePath { first: DeclId, second: DeclId },
}

impl core::fmt::Display for PathIndexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicatePath { first, second } => write!(
                f,
                "decls {} and {} share a canonical path",
                first.raw(),
                second.raw()
            ),
        }
    }
}
impl std::error::Error for PathIndexError {}

impl DerivedPathIndex {
    /// Number of named (Node/Edge) declarations with a path.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_path.len()
    }
    /// True when no named declaration has a path.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
    /// The canonical path of a declaration, or `None` for an arc / out-of-range id.
    #[must_use]
    pub fn path_of(&self, d: DeclId) -> Option<&PathKey> {
        self.did_to_path.get(d.raw())?.as_ref()
    }
    /// Iterate `(path, DeclId)` in deterministic path order.
    pub fn iter(&self) -> impl Iterator<Item = (&PathKey, DeclId)> {
        self.by_path.iter().map(|(p, d)| (p, *d))
    }
    /// The declaration at an exact path, if any (binary search).
    #[must_use]
    pub fn decl_of(&self, path: &[crate::common::ids::SymId]) -> Option<DeclId> {
        self.by_path
            .binary_search_by(|(p, _)| p.0.as_slice().cmp(path))
            .ok()
            .map(|i| self.by_path[i].1)
    }
    /// The per-declaration path table (DeclId-indexed), for consumers that want it.
    #[must_use]
    pub fn did_to_path(&self) -> &[Option<PathKey>] {
        &self.did_to_path
    }
}

/// Build a [`DerivedPathIndex`] from the `Ir` — the U2.2 replacement for reading
/// paths off the (now non-authoritative) resolution `Index`.
///
/// # Errors
/// [`PathIndexError::DuplicatePath`] if two declarations resolve to the same
/// canonical path (a malformed hierarchy).
pub fn build_path_index(ir: &Ir) -> Result<DerivedPathIndex, PathIndexError> {
    let did_to_path = CanonicalPaths::new(ir).did_to_path;
    let by_path = sorted_paths(&did_to_path)?
        .into_iter()
        .map(|(path, d)| (path.clone(), d))
        .collect();
    Ok(DerivedPathIndex {
        by_path,
        did_to_path,
    })
}

/// The named declarations with their paths, sorted by path, then by id: the one order
/// [`build_path_index`] keeps and [`Canon`] checks.
///
/// # Errors
/// [`PathIndexError::DuplicatePath`] for the first two adjacent declarations, in that order, that
/// share a path.
fn sorted_paths(
    did_to_path: &[Option<PathKey>],
) -> Result<Vec<(&PathKey, DeclId)>, PathIndexError> {
    let mut named: Vec<(&PathKey, DeclId)> = did_to_path
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|path| (path, DeclId::new(i))))
        .collect();
    named.sort_by(|a, b| a.0.0.cmp(&b.0.0).then_with(|| a.1.raw().cmp(&b.1.raw())));
    match named.windows(2).find(|w| w[0].0.0 == w[1].0.0) {
        Some(w) => Err(PathIndexError::DuplicatePath {
            first: w[0].1,
            second: w[1].1,
        }),
        None => Ok(named),
    }
}
