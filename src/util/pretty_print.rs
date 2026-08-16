use crate::common::ids::{DeclId, SymId};
use crate::common::pathkey::PathKey;
use crate::ir::hash::HashId;
use crate::ir::ir::{DeclKind, Ir, SignedRefR};
use crate::module_store::module_store::CompiledProgram;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;
use std::collections::BTreeMap;

// ----------------------
// Pretty print helpers
// ----------------------
fn sym(it: &Interner, s: SymId) -> String {
    // Ha nálad más a metódus neve, írd át: resolve/str/get stb.
    it.resolve(s).to_string()
}

fn fmt_path(it: &Interner, k: &PathKey) -> String {
    let parts: Vec<String> = k.0.iter().map(|&sid| sym(it, sid)).collect();
    parts.join(".")
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

// Ha nálad HashId(pub [u8; 32]) jellegű:
fn fmt_hash_opt(h: Option<HashId>) -> String {
    match h {
        Some(h) => to_hex(&h.0),
        None => "None".to_string(),
    }
}

fn build_declid_to_path(idx: &Index, decl_count: usize) -> Vec<Option<PathKey>> {
    let mut v = vec![None; decl_count];
    for (k, &did) in &idx.by_path {
        let i = did.0 as usize;
        if i < v.len() {
            // ha több path is ugyanarra a DeclId-ra mutat, az elsőt hagyjuk (ok)
            if v[i].is_none() {
                v[i] = Some(k.clone());
            }
        }
    }
    v
}

#[derive(Default)]
struct TrieNode {
    // segment SymId -> child
    children: BTreeMap<SymId, TrieNode>,
    // ha egy teljes path ide ér, eltároljuk a DeclId-t
    did: Option<DeclId>,
}

fn insert_path(root: &mut TrieNode, path: &[SymId], did: DeclId) {
    let mut cur = root;
    for &seg in path {
        cur = cur.children.entry(seg).or_default();
    }
    cur.did = Some(did);
}

fn print_trie(
    it: &Interner,
    ir: &Ir,
    did_to_path: &[Option<PathKey>],
    node: &TrieNode,
    depth: usize,
) {
    // rendezzük a kulcsokat név szerint
    let mut keys: Vec<_> = node.children.keys().copied().collect();
    keys.sort_by(|a, b| it.resolve(*a).cmp(it.resolve(*b)));

    for seg in keys {
        let child = &node.children[&seg];
        let name = it.resolve(seg);

        if let Some(did) = child.did {
            let kind = ir.decl_nodes.get(did.0 as usize).map(|n| n.kind);
            let hash = ir.decl_hash.get(did.0 as usize).and_then(|x| *x);

            let indent = "\t".repeat(depth);
            let hash_s = match hash {
                Some(h) => to_hex(&h.0),
                None => "None".to_string(),
            };

            match kind {
                Some(DeclKind::Edge) => {
                    // ✅ Edge: arcok + referenciák
                    let details = fmt_edge_arcs(it, ir, did_to_path, did);
                    println!("{indent}{name}  did={did:?}  kind=Edge  hash={hash_s}{details}");
                }
                Some(k) => {
                    println!("{indent}{name}  did={did:?}  kind={k:?}  hash={hash_s}");
                }
                None => {
                    println!("{indent}{name}  did={did:?}  kind=?  hash={hash_s}");
                }
            }
        } else {
            let indent = "\t".repeat(depth);
            println!("{indent}{name}");
        }

        print_trie(it, ir, did_to_path, child, depth + 1);
    }
}

fn fmt_decl(it: &Interner, did_to_path: &[Option<PathKey>], did: DeclId) -> String {
    let i = did.0 as usize;
    if let Some(Some(pk)) = did_to_path.get(i) {
        // pk.0 = Vec<SymId> (ha nálad más, igazítsd)
        let parts: Vec<_> = pk.0.iter().map(|&sid| it.resolve(sid)).collect();
        parts.join(".")
    } else {
        format!("{did:?}")
    }
}

fn fmt_edge_arcs(
    it: &Interner,
    ir: &Ir,
    did_to_path: &[Option<PathKey>],
    edge_did: DeclId,
) -> String {
    let ei = edge_did.0 as usize;
    let Some(eid) = ir.decl_to_edge.get(ei).and_then(|x| *x) else {
        return " (edge_rec missing)".to_string();
    };

    let edge = &ir.edges[eid.0 as usize];
    let mut out = String::new();

    for (ai, arc_id) in edge.arcs.iter().enumerate() {
        let arc = &ir.arcs[arc_id.0 as usize];
        out.push_str(&format!("\n\t\t- arc#{ai}: "));

        for (ri, r) in arc.refs.iter().enumerate() {
            if ri > 0 {
                out.push_str(", ");
            }

            let (sgn, target) = match r {
                SignedRefR::Plus(a) => ("+", a.target),
                SignedRefR::Minus(a) => ("-", a.target),
                SignedRefR::Neutral(a) => ("0", a.target),
            };

            let t = fmt_decl(it, did_to_path, target);
            out.push_str(&format!("{sgn}{t}"));
        }
    }

    out
}

pub fn pretty_print_compiled(it: &Interner, compiled: &CompiledProgram) {
    println!("=== HyMeKo compile ===");
    println!("Canonical program hash: {}", to_hex(&compiled.canon_hash.0));
    println!("Root: {}", compiled.root.0.display());

    println!("\nImports (namespace -> file):");
    if compiled.imports.is_empty() {
        println!("  (none)");
    } else {
        for (ns, key) in &compiled.imports {
            println!("  {}  ->  {}", sym(it, *ns), key.0.display());
        }
    }

    println!("\nIndex size: {}", compiled.idx.by_path.len());
    println!("IR decls:   {}", compiled.ir.decl_nodes.len());
    println!("IR edges:   {}", compiled.ir.edges.len());
    println!("IR arcs:    {}", compiled.ir.arcs.len());

    // Top-N decl list (rendezetten path szerint)
    let mut keys: Vec<_> = compiled.idx.by_path.keys().cloned().collect();
    keys.sort_by(|a, b| fmt_path(it, a).cmp(&fmt_path(it, b)));

    println!("\nDecls (hierarchy):");

    let mut trie = TrieNode::default();

    // töltsük a trie-t a globál indexből
    for (k, &did) in &compiled.idx.by_path {
        // ha a PathKey nálad tuple struct: PathKey(Vec<SymId>) → k.0
        insert_path(&mut trie, &k.0, did);
    }

    // print
    let did_to_path = build_declid_to_path(&compiled.idx, compiled.ir.decl_nodes.len());
    print_trie(&it, &compiled.ir, &did_to_path, &trie, 0);
}
