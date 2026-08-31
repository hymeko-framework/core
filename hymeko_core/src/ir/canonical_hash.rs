use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::common::ids::DeclId;
use crate::common::pathkey::PathKey;
use crate::ir::hash::HashId;
use crate::ir::ir::{DeclKind, DeclNode, Ir, SignedRefR};
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

/// Top-level: H( header || H(skeleton_prufer) || H(incidence_bytes) )
pub fn canonical_program_hash(cfg: CanonHashCfg, idx: &Index, ir: &Ir, it: &Interner) -> HashId {
    // 1) reverse map: DeclId -> PathKey (első találat elég)
    let did_to_path = build_did_to_path(idx, ir.decl_nodes.len());

    // 2) skeleton forest (csak Node+Edge) -> roots + children
    let (verts, parent, children, roots) = build_skeleton_forest(idx, ir, &did_to_path);

    // 3) skeleton subtree hashes (name-invariant)
    let vhash = compute_subtree_hashes_with_kinds(cfg, &ir.decl_nodes, &roots, &children);

    // 4) tie-break: path hash (csak ütközés ellen)
    let vtie = compute_tiebreak(cfg, &verts, &did_to_path, it);

    // 5) forest -> tree (super-root) + Prüfer encode with hash-labels
    let prufer_bytes = prufer_from_forest(cfg, &verts, &parent, &roots, &vhash, &vtie);

    // 6) incidence bytes (port/slot megőrzésével)
    let incidence_bytes = canonical_incidence_bytes(cfg, ir, &did_to_path, it);

    // 7) program hash
    let sk = canonical_chunk_hash(cfg, *b"SKELETON", &prufer_bytes);
    let inc = canonical_chunk_hash(cfg, *b"INCIDENC", &incidence_bytes);

    let mut h = blake3::Hasher::new();
    write_header(&mut h, cfg, *b"PROGRAM\0");
    h.update(&sk.0);
    h.update(&inc.0);
    HashId(*h.finalize().as_bytes())
}

/// Authority-flip form (U2): the canonical hash of `ir` with **no** `Index` —
/// the declaration hierarchy read exclusively from the `Ir` intrusive links
/// (`DeclNode.parent`, `name`, `kind`).
///
/// **DEFERRED — not a drop-in replacement for [`canonical_program_hash`].** For
/// single-module programs the two are bit-identical (lowering builds the links
/// from the index, arcs never parent a Node/Edge). But for **multi-module
/// programs the results DIVERGE**: the v1 hash embeds the resolution-`Index`
/// import namespace prefix (e.g. `meta_kinematics`) in every imported
/// declaration's path, and that prefix is stored *only* in the `Index` — the
/// `Ir` has no field for it and an imported module's root has `parent == NONE`,
/// so a pure-links path cannot recover it. Measured on the corpus: 35 fixtures
/// agree, 61 (all import-bearing) diverge. Making this the production hash would
/// change the digest for every multi-module program, i.e. an `algo_version`
/// bump, which the U2 phase forbade — so the flip is HARD-STOPPED pending the
/// `Ir` gaining explicit namespace state. See
/// `reports/2026-08-31-hierarchy-authority-decision.md`.
///
/// Kept as the study artifact and the differential oracle that pins the exact
/// divergence; **do not** call it as the canonical hash.
///
/// # Preconditions
/// None — total over any `Ir`; malformed hierarchies terminate deterministically
/// (bounded walks).
pub fn canonical_program_hash_ir(cfg: CanonHashCfg, ir: &Ir, it: &Interner) -> HashId {
    let did_to_path = build_did_to_path_from_ir(ir);
    let (verts, parent, children, roots) = build_skeleton_forest_from_ir(ir);
    let vhash = compute_subtree_hashes_with_kinds(cfg, &ir.decl_nodes, &roots, &children);
    let vtie = compute_tiebreak(cfg, &verts, &did_to_path, it);
    let prufer_bytes = prufer_from_forest(cfg, &verts, &parent, &roots, &vhash, &vtie);
    let incidence_bytes = canonical_incidence_bytes(cfg, ir, &did_to_path, it);
    let sk = canonical_chunk_hash(cfg, *b"SKELETON", &prufer_bytes);
    let inc = canonical_chunk_hash(cfg, *b"INCIDENC", &incidence_bytes);
    let mut h = blake3::Hasher::new();
    write_header(&mut h, cfg, *b"PROGRAM\0");
    h.update(&sk.0);
    h.update(&inc.0);
    HashId(*h.finalize().as_bytes())
}

// -----------------------------
//  DeclId -> PathKey from the Ir links (parent chain of `name`s)
// -----------------------------
//
// Each named (Node/Edge) declaration's path is the sequence of `name`s from the
// root down to it, read by walking `parent`. Anonymous arcs have no path (they
// are never used as a path in the incidence, which keys on the edge and target
// declarations). The parent walk is bounded by `decl_nodes.len()` so a malformed
// cycle terminates rather than looping.
fn build_did_to_path_from_ir(ir: &Ir) -> Vec<Option<PathKey>> {
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
    v
}

// -----------------------------
//  Skeleton forest from the Ir links
//   - vertices = Node/Edge declarations, in DeclId order
//   - parent   = DeclNode.parent (a Node/Edge; arcs never parent a Node/Edge),
//                which equals the nearest Node/Edge path-prefix on well-formed IR
//   - children = inversion of parent in DeclId order (matches the DeclId sort
//                the index-based forest applied)
// -----------------------------
fn build_skeleton_forest_from_ir(
    ir: &Ir,
) -> (
    Vec<DeclId>,
    Vec<Option<DeclId>>,
    Vec<Vec<DeclId>>,
    Vec<DeclId>,
) {
    let n = ir.decl_nodes.len();
    let mut verts = Vec::new();
    for i in 0..n {
        match ir.decl_nodes[i].kind {
            DeclKind::Node | DeclKind::Edge => verts.push(DeclId::new(i)),
            DeclKind::HyperArc => {}
        }
    }
    let mut parent: Vec<Option<DeclId>> = vec![None; n];
    let mut children: Vec<Vec<DeclId>> = vec![Vec::new(); n];
    let mut roots = Vec::new();
    for &did in &verts {
        let p = ir.decl_nodes[did.0].parent;
        let skeleton_parent = if p.is_some() && p.0 < n {
            matches!(ir.decl_nodes[p.0].kind, DeclKind::Node | DeclKind::Edge).then_some(p)
        } else {
            None
        };
        match skeleton_parent {
            Some(p) => {
                parent[did.0] = Some(p);
                children[p.0].push(did); // verts in DeclId order => children sorted
            }
            None => roots.push(did),
        }
    }
    (verts, parent, children, roots)
}

// -----------------------------
//  Header + chunk hashing
// -----------------------------
fn write_header(h: &mut blake3::Hasher, cfg: CanonHashCfg, domain: [u8; 8]) {
    h.update(b"HYMEKOCN"); // 8B magic
    h.update(&domain); // 8B domain
    h.update(&cfg.schema_version.to_le_bytes());
    h.update(&cfg.algo_version.to_le_bytes());
    h.update(&cfg.flags.to_le_bytes());
}

fn canonical_chunk_hash(cfg: CanonHashCfg, domain: [u8; 8], payload: &[u8]) -> HashId {
    let mut h = blake3::Hasher::new();
    write_header(&mut h, cfg, domain);
    h.update(payload);
    HashId(*h.finalize().as_bytes())
}

// -----------------------------
//  DeclId -> PathKey reverse map
// -----------------------------
fn build_did_to_path(idx: &Index, decl_count: usize) -> Vec<Option<PathKey>> {
    let mut v = vec![None; decl_count];
    for (k, &did) in &idx.by_path {
        let i = did.0 as usize;
        if i < v.len() && v[i].is_none() {
            v[i] = Some(k.clone());
        }
    }
    v
}

// -----------------------------
//  Skeleton forest from hierarchy paths
//   - vertices = DeclId where kind is Node/Edge
//   - parent is nearest prefix path with Node/Edge
//   - roots: those without parent
// -----------------------------
fn build_skeleton_forest(
    idx: &Index,
    ir: &Ir,
    did_to_path: &[Option<PathKey>],
) -> (
    Vec<DeclId>,
    Vec<Option<DeclId>>,
    Vec<Vec<DeclId>>,
    Vec<DeclId>,
) {
    // collect skeleton vertices
    let mut verts = Vec::new();
    for &did in idx.by_path.values() {
        match ir.decl_nodes[did.0 as usize].kind {
            DeclKind::Node | DeclKind::Edge => verts.push(did),
            DeclKind::HyperArc => {}
        }
    }
    // stable order
    verts.sort_by_key(|d| d.0);

    // quick lookup: path -> did (only skeleton)
    let mut path_to_did: HashMap<PathKey, DeclId> = HashMap::new();
    for &did in &verts {
        if let Some(pk) = &did_to_path[did.0 as usize] {
            path_to_did.insert(pk.clone(), did);
        }
    }

    // compute parent by nearest prefix
    let mut parent: Vec<Option<DeclId>> = vec![None; ir.decl_nodes.len()];
    for &did in &verts {
        let Some(pk) = &did_to_path[did.0 as usize] else {
            continue;
        };
        // walk prefixes: [a,b,c] -> [a,b] -> [a] -> []
        for cut in (0..pk.0.len()).rev() {
            let pref = PathKey(pk.0[..cut].to_vec());
            if let Some(&pdid) = path_to_did.get(&pref) {
                parent[did.0 as usize] = Some(pdid);
                break;
            }
        }
    }

    // children lists
    let mut children: Vec<Vec<DeclId>> = vec![Vec::new(); ir.decl_nodes.len()];
    let mut roots = Vec::new();
    for &did in &verts {
        match parent[did.0 as usize] {
            Some(p) => children[p.0 as usize].push(did),
            None => roots.push(did),
        }
    }

    // sort children by DeclId for determinism (hash-invariant will still be ok)
    for v in &mut children {
        v.sort_by_key(|d| d.0);
    }
    roots.sort_by_key(|d| d.0);

    (verts, parent, children, roots)
}

/// Proper subtree hash computation with decl kinds supplied.
///
/// Iterative post-order fold. The traversal was recursive until 2026-08-28
/// (M0.6); a chain of nested declarations then consumed one stack frame per
/// level and aborted the process at ~215 levels in debug and ~458 in release
/// (measured, M0.5 Part E). A stack overflow is an abort, not a catchable
/// panic, so the depth ceiling could not be contained by the caller.
///
/// # Output contract
///
/// Bit-for-bit identical to the previous recursive form. Specifically:
/// child hashes are collected in `children` order and then sorted
/// lexicographically, so traversal order cannot influence the result; the
/// per-node header and domain are unchanged; nodes not reachable from `roots`
/// remain unhashed and fold to `HashId([0; 32])` exactly as before. Frozen
/// known-answer vectors and a 96-fixture corpus digest guard this
/// (`tests/unified_ir_m06/frozen_v1.rs`).
///
/// # Invariants
///
/// `children` is derived from strictly shortening path prefixes, so the
/// relation is a forest and every node is entered at most once. `on_stack`
/// nevertheless guards the traversal: a malformed `children` containing a
/// cycle terminates deterministically — the back edge contributes the same
/// zero hash an unreachable node would — instead of looping forever. The
/// recursive form would have overflowed the stack on such an input.
fn compute_subtree_hashes_with_kinds(
    cfg: CanonHashCfg,
    nodes: &[DeclNode],
    roots: &[DeclId],
    children: &[Vec<DeclId>],
) -> Vec<HashId> {
    let n = children.len();
    let mut memo: Vec<Option<HashId>> = vec![None; n];

    fn node_domain(kind: DeclKind) -> [u8; 8] {
        match kind {
            DeclKind::Node => *b"SKNOD\0\0\0",
            DeclKind::Edge => *b"SKEDG\0\0\0",
            DeclKind::HyperArc => *b"SKARC\0\0\0",
        }
    }

    /// `Enter` schedules a node's children; `Exit` folds their finished hashes.
    enum Frame {
        Enter(DeclId),
        Exit(DeclId),
    }

    // Depth now lives on the heap. Both are O(n) alongside the existing memo.
    let mut stack: Vec<Frame> = Vec::new();
    let mut on_stack: Vec<bool> = vec![false; n];

    for &r in roots {
        let ri = r.0;
        if ri >= n || memo[ri].is_some() {
            continue;
        }
        stack.push(Frame::Enter(r));

        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Enter(v) => {
                    let vi = v.0;
                    // Checked: a malformed id is skipped rather than panicking.
                    if vi >= n || memo[vi].is_some() || on_stack[vi] {
                        continue;
                    }
                    on_stack[vi] = true;
                    // Exit is pushed first so it pops after every descendant.
                    stack.push(Frame::Exit(v));
                    for &c in &children[vi] {
                        let ci = c.0;
                        if ci < n && memo[ci].is_none() && !on_stack[ci] {
                            stack.push(Frame::Enter(c));
                        }
                    }
                }
                Frame::Exit(v) => {
                    let vi = v.0;
                    if vi >= n {
                        continue;
                    }
                    on_stack[vi] = false;
                    if memo[vi].is_some() {
                        continue;
                    }

                    // Same collection and ordering as the recursive form: read
                    // children in `children` order, then sort lexicographically.
                    let mut ch: Vec<[u8; 32]> = children[vi]
                        .iter()
                        .map(|&c| {
                            memo.get(c.0)
                                .copied()
                                .flatten()
                                .unwrap_or(HashId([0; 32]))
                                .0
                        })
                        .collect();

                    ch.sort(); // lexicographic on [u8;32]

                    let mut h = blake3::Hasher::new();
                    write_header(&mut h, cfg, node_domain(nodes[vi].kind));
                    for x in ch {
                        h.update(&x);
                    }
                    memo[vi] = Some(HashId(*h.finalize().as_bytes()));
                }
            }
        }
    }

    // fill missing (unreachable from any root) with zero
    memo.into_iter()
        .map(|x| x.unwrap_or(HashId([0; 32])))
        .collect()
}

// -----------------------------
//  Tie-break (path hash) – only for collision resolution
// -----------------------------
fn compute_tiebreak(
    cfg: CanonHashCfg,
    verts: &[DeclId],
    did_to_path: &[Option<PathKey>],
    it: &Interner,
) -> Vec<u64> {
    let mut tb = vec![0u64; did_to_path.len()];
    for &v in verts {
        let i = v.0 as usize;
        if let Some(pk) = &did_to_path[i] {
            let mut h = blake3::Hasher::new();
            write_header(&mut h, cfg, *b"TIEBRK\0\0");
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

fn prufer_from_forest(
    cfg: CanonHashCfg,
    verts: &[DeclId],
    parent: &[Option<DeclId>],
    roots: &[DeclId],
    vhash: &[HashId], // indexed by DeclId.0 as usize
    vtie: &[u64],     // indexed by DeclId.0 as usize
) -> Vec<u8> {
    let n = verts.len();
    let mut vidx: HashMap<usize, usize> = HashMap::new();
    for (i, d) in verts.iter().enumerate() {
        vidx.insert(d.0, i);
    }
    let super_idx = n;

    // adjacency (undirected) for the super-rooted tree
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n + 1];

    for &v_did in verts {
        if let Some(p_did) = parent[v_did.0 as usize] {
            let a = *vidx.get(&v_did.0).unwrap();
            let b = *vidx.get(&p_did.0).unwrap();
            adj[a].push(b);
            adj[b].push(a);
        }
    }
    for &r_did in roots {
        let a = *vidx.get(&r_did.0).unwrap();
        adj[a].push(super_idx);
        adj[super_idx].push(a);
    }

    let mut deg: Vec<usize> = adj.iter().map(|xs| xs.len()).collect();

    #[derive(Copy, Clone)]
    struct Leaf {
        key_hash: [u8; 32],
        key_tie: u64,
        v: usize,
    }
    impl Eq for Leaf {}
    impl PartialEq for Leaf {
        fn eq(&self, other: &Self) -> bool {
            self.key_hash == other.key_hash && self.key_tie == other.key_tie && self.v == other.v
        }
    }
    impl Ord for Leaf {
        fn cmp(&self, other: &Self) -> Ordering {
            // reverse => BinaryHeap becomes min-heap by (hash, tie, v)
            match other.key_hash.cmp(&self.key_hash) {
                Ordering::Equal => match other.key_tie.cmp(&self.key_tie) {
                    Ordering::Equal => other.v.cmp(&self.v),
                    x => x,
                },
                x => x,
            }
        }
    }
    impl PartialOrd for Leaf {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    let super_hash = canonical_chunk_hash(cfg, *b"ROOTNODE", b"ROOT").0;

    // One single source of truth for labels:
    let label = |v: usize| -> ([u8; 32], u64) {
        if v == super_idx {
            (super_hash, 0)
        } else {
            let did = verts[v].0 as usize; // v is local index -> verts[v] -> DeclId -> global index
            (vhash[did].0, vtie[did])
        }
    };

    let mut heap = BinaryHeap::new();
    for v in 0..=super_idx {
        if deg[v] == 1 {
            let (h, t) = label(v);
            heap.push(Leaf {
                key_hash: h,
                key_tie: t,
                v,
            });
        }
    }

    let total = n + 1;
    let mut out = Vec::with_capacity(total.saturating_sub(2) * 32);
    let mut alive = vec![true; total];

    for _ in 0..total.saturating_sub(2) {
        let leaf = loop {
            let x = heap.pop().expect("heap empty");
            if alive[x.v] && deg[x.v] == 1 {
                break x;
            }
        };

        let v = leaf.v;
        let u = adj[v]
            .iter()
            .copied()
            .find(|&u| alive[u])
            .expect("leaf had no neighbor");

        // token = neighbor hash
        let (tok, _) = label(u);
        out.extend_from_slice(&tok);

        alive[v] = false;
        deg[v] = 0;
        deg[u] -= 1;

        if deg[u] == 1 {
            let (h, t) = label(u);
            heap.push(Leaf {
                key_hash: h,
                key_tie: t,
                v: u,
            });
        }
    }

    out
}

// -----------------------------
//  Incidence canonical bytes
//   record: edge_path | arc_idx | slot | sign | target_path | weights?
//   (slot = ref index -> permutation preserved)
// -----------------------------
fn canonical_incidence_bytes(
    cfg: CanonHashCfg,
    ir: &Ir,
    did_to_path: &[Option<PathKey>],
    it: &Interner,
) -> Vec<u8> {
    let _ = cfg; // flags later (weights/tags inclusion)

    // 1. Precompute path strings ONCE. No cloning in the loop.
    let mut did_path_str: Vec<String> = vec![String::new(); did_to_path.len()];
    for (i, pk) in did_to_path.iter().enumerate() {
        if let Some(pk) = pk {
            did_path_str[i] =
                pk.0.iter()
                    .map(|&sid| it.resolve(sid))
                    .collect::<Vec<_>>()
                    .join(".");
        } else {
            did_path_str[i] = format!("DeclId({})", i);
        }
    }

    // 2. The Arena: One single contiguous memory block.
    let mut arena = Vec::with_capacity(ir.edges.len() * 128);

    // 3. Lightweight struct storing references and offsets, NOT allocations.
    struct Rec<'a> {
        edge_path: &'a str,
        arc_idx: u16,
        slot: u16,
        start: usize,
        end: usize,
    }

    // Guessing capacity to avoid vector resizing
    let mut recs = Vec::with_capacity(ir.edges.len() * 4);

    for e in &ir.edges {
        let edge_path: &str = &did_path_str[e.decl.0 as usize];

        for (ai, &arc_id) in e.arcs.iter().enumerate() {
            let arc = &ir.arcs[arc_id.0 as usize];

            for (slot, sref) in arc.refs.iter().enumerate() {
                let (sign, target) = match sref {
                    SignedRefR::Plus(a) => (1i8, a.target),
                    SignedRefR::Minus(a) => (-1i8, a.target),
                    SignedRefR::Neutral(a) => (0i8, a.target),
                };

                let target_path: &str = &did_path_str[target.0 as usize];

                let start = arena.len();

                // Write directly into the arena
                push_str(&mut arena, edge_path);
                arena.extend_from_slice(&(ai as u16).to_le_bytes());
                arena.extend_from_slice(&(slot as u16).to_le_bytes());
                arena.push(sign as u8);
                push_str(&mut arena, target_path);

                let end = arena.len();

                recs.push(Rec {
                    edge_path,
                    arc_idx: ai as u16,
                    slot: slot as u16,
                    start,
                    end,
                });
            }
        }
    }

    // 4. sort_unstable_by is faster than sort_by, and safe here because keys are unique
    recs.sort_unstable_by(|a, b| {
        a.edge_path
            .cmp(b.edge_path)
            .then_with(|| a.arc_idx.cmp(&b.arc_idx))
            .then_with(|| a.slot.cmp(&b.slot))
    });

    // 5. Build final output by reading from the sorted arena offsets
    let mut out = Vec::with_capacity(arena.len());
    for r in recs {
        out.extend_from_slice(&arena[r.start..r.end]);
    }
    out
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let n16: u16 = n.try_into().unwrap_or(u16::MAX);
    out.extend_from_slice(&n16.to_le_bytes());
    out.extend_from_slice(&bytes[..(n16 as usize)]);
}

// -----------------------------
//  U2.1 lowering-boundary namespace validator
// -----------------------------

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
pub fn validate_namespace_roots(ir: &Ir, idx: &Index) -> Result<(), NamespaceError> {
    let n = ir.decl_nodes.len();
    let idx_paths = build_did_to_path(idx, n);
    let link_paths = build_did_to_path_from_ir(ir);
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
