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
    // M2b: this resolves the record, so it is the checked-composite contract,
    // not a map-only read. `edge_for_decl` additionally refuses a record whose
    // back-link names a different declaration -- for a pretty-printer that is
    // strictly better than rendering some other edge under this name.
    let Some((_eid, edge)) = ir.edge_for_decl(edge_did) else {
        return " (edge_rec missing)".to_string();
    };
    let mut out = String::new();

    for (ai, arc_id) in edge.arcs.iter().enumerate() {
        // M2b: point lookup through the accessor. A dangling arc id renders a
        // marker instead of panicking — a debug printer must survive the
        // malformed input it exists to help diagnose.
        let Some(arc) = ir.arc(*arc_id) else {
            out.push_str(&format!("\n\t\t- arc#{ai}: (arc_rec missing)"));
            continue;
        };
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

    // U2.2: paths come from a view DERIVED from the Ir (links + namespace_roots),
    // not the resolution Index -- the Ir is the sole post-lowering authority.
    let path_index = match crate::ir::canonical_hash::build_path_index(&compiled.ir) {
        Ok(pi) => pi,
        Err(e) => {
            println!("\n(path index unavailable: {e})");
            return;
        }
    };
    println!("\nDerived path count: {}", path_index.len());
    println!("IR decls:   {}", compiled.ir.decl_count());
    println!("IR edges:   {}", compiled.ir.edge_count());
    println!("IR arcs:    {}", compiled.ir.arc_count());

    // Top-N decl list (rendezetten path szerint)
    println!("\nDecls (hierarchy):");

    let mut trie = TrieNode::default();

    // töltsük a trie-t a globál indexből
    for (k, did) in path_index.iter() {
        // ha a PathKey nálad tuple struct: PathKey(Vec<SymId>) → k.0
        insert_path(&mut trie, &k.0, did);
    }

    print_trie(&it, &compiled.ir, path_index.did_to_path(), &trie, 0);
}
