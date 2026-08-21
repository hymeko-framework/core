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
fn compute_subtree_hashes_with_kinds(
    cfg: CanonHashCfg,
    nodes: &[DeclNode], // Changed from kinds to nodes for clarity
    roots: &[DeclId],
    children: &[Vec<DeclId>],
) -> Vec<HashId> {
    let mut memo: Vec<Option<HashId>> = vec![None; children.len()];

    fn node_domain(kind: DeclKind) -> [u8; 8] {
        match kind {
            DeclKind::Node => *b"SKNOD\0\0\0",
            DeclKind::Edge => *b"SKEDG\0\0\0",
            DeclKind::HyperArc => *b"SKARC\0\0\0",
        }
    }

    fn rec(
        cfg: CanonHashCfg,
        v: DeclId,
        nodes: &[DeclNode], // Match the outer type exactly
        children: &[Vec<DeclId>],
        memo: &mut [Option<HashId>],
    ) -> HashId {
        if let Some(h) = memo[v.0 as usize] {
            return h;
        }

        let mut ch: Vec<[u8; 32]> = children[v.0 as usize]
            .iter()
            .map(|&c| rec(cfg, c, nodes, children, memo).0)
            .collect();

        ch.sort(); // lexicographic on [u8;32]

        let mut h = blake3::Hasher::new();
        // Extract the .kind field from the node struct here
        write_header(&mut h, cfg, node_domain(nodes[v.0 as usize].kind));
        for x in ch {
            h.update(&x);
        }
        let out = HashId(*h.finalize().as_bytes());
        memo[v.0 as usize] = Some(out);
        out
    }

    // compute for all reachable nodes
    for &r in roots {
        let _ = rec(cfg, r, nodes, children, &mut memo);
    }

    // fill missing (shouldn't happen) with zero
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
