//! Part B — a semantic oracle that does **not** call the canonical hash.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-5-tests-and-benchmarks (2026-08-28).
//!
//! If equivalence were defined by `CanonicalPaths::hash`, then "equivalent
//! histories hash identically" would be a tautology and the M0.5 test would
//! prove nothing. So this module derives meaning independently, by walking the
//! `Ir` and the `Index` directly, and emits a normalised textual form. Two IRs
//! are semantically equal iff their oracle forms are string-equal.
//!
//! # Which ordering is semantic, and which is representational
//!
//! This is the question M0.5 was asked to answer, and the honest answer is that
//! **the current contracts do not fully settle it.** What can be established by
//! reading the code:
//!
//! | Ordering | Status | Evidence |
//! |---|---|---|
//! | Dense `DeclId`/`NodeId`/`EdgeId` assignment | **representational** | `canonical_hash` keys everything on `PathKey`; `verts`/`children`/`roots` are sorted before use |
//! | Sibling order among declarations | **representational** | `SkeletonForest::subtree_hashes` sorts child hashes (`ch.sort()`) before folding |
//! | `Vec` insertion order of `Ir.nodes` / `Ir.edges` | **representational** | never read positionally by the hash; only `.decl` is followed |
//! | **Arc order within an edge** (`EdgeRec.arcs` position) | **SEMANTIC — by the hash** | `canonical_incidence_bytes` writes `ai as u16` and sorts on it |
//! | **Reference slot within an arc** (`ArcRec.refs` position) | **SEMANTIC — by the hash** | same function writes `slot as u16` and sorts on it |
//! | `DeclNode.parent` / `first_child` / `next_sibling` | **NOT READ AT ALL** | the skeleton forest is built from `Index` path prefixes, never from these fields |
//!
//! **The ambiguity, stated rather than resolved.** The last three rows are not
//! documented anywhere as language decisions; they are consequences of one
//! implementation. Specifically:
//!
//! 1. Nothing in the HyMeKo language reference or in `ir.rs` says whether
//!    reordering two arcs of the same hyperedge, or two references within one
//!    arc, changes program meaning. The hash says it does. If that is
//!    unintended, the hash over-discriminates and every editor round trip that
//!    reorders arcs produces a spurious "changed" verdict.
//! 2. `DeclNode`'s intrusive hierarchy being invisible to the hash means an
//!    `Ir` whose `parent` links contradict its `Index` paths hashes as though
//!    the paths were authoritative. Two structures, one of them silently
//!    ignored.
//!
//! **This phase does not choose.** The oracle therefore reports *both*
//! readings — `Ordered` (arc/slot position is meaning) and `Multiset` (it is
//! not) — so a reader can see which cases are sensitive to the open question
//! and which are not.

use std::collections::BTreeSet;

use hymeko::common::ids::DeclId;
use hymeko::ir::ir::{DeclKind, Ir, SignedRefR, ValueR};

use crate::unified_ir::support::builder::TestIr;

/// How to treat positional ordering of arcs and of references within an arc.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Ordering {
    /// Arc index and reference slot carry meaning — the canonical hash's reading.
    Ordered,
    /// Arc index and reference slot are representational — sorted away.
    Multiset,
}

fn kind_str(k: DeclKind) -> &'static str {
    match k {
        DeclKind::Node => "node",
        DeclKind::Edge => "edge",
        DeclKind::HyperArc => "arc",
    }
}

fn sign_str(r: &SignedRefR) -> &'static str {
    match r {
        SignedRefR::Plus(_) => "+",
        SignedRefR::Minus(_) => "-",
        SignedRefR::Neutral(_) => "0",
    }
}

fn value_str(v: &ValueR, t: &TestIr) -> String {
    match v {
        ValueR::Str(s) => format!("s:{}", t.it.resolve(*s)),
        ValueR::Num(n) => format!("n:{n}"),
        ValueR::List(xs) => format!(
            "[{}]",
            xs.iter()
                .map(|x| value_str(x, t))
                .collect::<Vec<_>>()
                .join(",")
        ),
        ValueR::Ref(d) => format!("r:{}", t.path_of(*d)),
    }
}

fn anno_str(d: DeclId, t: &TestIr) -> String {
    let a = &t.ir.decl_nodes[d.raw()].anno;
    // Tags are sorted: a declaration's tag set is a set, not a sequence.
    let mut tags: Vec<&str> = a.tags.iter().map(|&s| t.it.resolve(s)).collect();
    tags.sort_unstable();
    let val = a
        .value
        .as_ref()
        .map(|v| value_str(v, t))
        .unwrap_or_else(|| "-".into());
    format!("tags[{}] val[{val}]", tags.join(","))
}

fn ref_str(r: &SignedRefR, t: &TestIr) -> String {
    let atom = r.atom();
    let w = atom
        .weights
        .as_ref()
        .map(|ws| {
            ws.iter()
                .map(|v| value_str(v, t))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "-".into());
    format!("{}{} w[{w}]", sign_str(r), t.path_of(atom.target))
}

fn bases_str(d: DeclId, t: &TestIr) -> String {
    let ir: &Ir = &t.ir;
    let bases = if let Some(nid) = ir.as_node(d) {
        &ir.nodes[nid.raw()].bases
    } else if let Some(eid) = ir.as_edge(d) {
        &ir.edges[eid.raw()].bases
    } else {
        return String::from("-");
    };
    // Inheritance is a set of base declarations; order is not meaning.
    let mut v: Vec<String> = bases.iter().map(|b| ref_str(b, t)).collect();
    v.sort();
    v.join(",")
}

/// The independent semantic form. String equality on this is the oracle.
pub fn semantic_form(t: &TestIr, ord: Ordering) -> String {
    let mut out = String::new();

    // ---- declarations, keyed by path, sorted; dense IDs never appear ----
    let mut decls: BTreeSet<String> = BTreeSet::new();
    for (pk, &did) in &t.idx.by_path {
        let path =
            pk.0.iter()
                .map(|&s| t.it.resolve(s))
                .collect::<Vec<_>>()
                .join(".");
        decls.insert(format!(
            "  {} {} anno[{}] bases[{}]",
            kind_str(t.ir.decl_nodes[did.raw()].kind),
            path,
            anno_str(did, t),
            bases_str(did, t)
        ));
    }
    out.push_str("DECLS\n");
    for d in &decls {
        out.push_str(d);
        out.push('\n');
    }

    // ---- hierarchy, derived from paths (the reading the hash uses) ----
    // Recorded explicitly so a divergence between path-parenthood and the
    // intrusive `DeclNode.parent` links is visible rather than assumed away.
    let mut hier: BTreeSet<String> = BTreeSet::new();
    for (pk, _) in &t.idx.by_path {
        if pk.0.len() >= 2 {
            let child =
                pk.0.iter()
                    .map(|&s| t.it.resolve(s))
                    .collect::<Vec<_>>()
                    .join(".");
            let parent = pk.0[..pk.0.len() - 1]
                .iter()
                .map(|&s| t.it.resolve(s))
                .collect::<Vec<_>>()
                .join(".");
            hier.insert(format!("  {parent} -> {child}"));
        }
    }
    out.push_str("HIERARCHY(by path)\n");
    for h in &hier {
        out.push_str(h);
        out.push('\n');
    }

    // ---- incidences, grouped by edge path ----
    let mut edges: Vec<(String, Vec<Vec<String>>)> = Vec::new();
    for e in &t.ir.edges {
        let epath = t.path_of(e.decl);
        let arcs: Vec<Vec<String>> = e
            .arcs
            .iter()
            .map(|&aid| {
                let arc = &t.ir.arcs[aid.raw()];
                let mut refs: Vec<String> = arc.refs.iter().map(|r| ref_str(r, t)).collect();
                if ord == Ordering::Multiset {
                    refs.sort();
                }
                refs
            })
            .collect();
        edges.push((epath, arcs));
    }
    edges.sort_by(|a, b| a.0.cmp(&b.0));

    out.push_str(&format!("INCIDENCES({ord:?})\n"));
    for (epath, mut arcs) in edges {
        if ord == Ordering::Multiset {
            // An edge's arcs are a bag of arcs, not a sequence.
            let mut keyed: Vec<String> = arcs.iter().map(|a| a.join("|")).collect();
            keyed.sort();
            out.push_str(&format!("  {epath}: {}\n", keyed.join(" ;; ")));
        } else {
            let rendered: Vec<String> = arcs.drain(..).map(|a| a.join("|")).collect();
            out.push_str(&format!("  {epath}: {}\n", rendered.join(" ;; ")));
        }
    }

    out
}

/// The intrusive hierarchy, reported separately — the hash never reads it.
pub fn intrusive_hierarchy_form(t: &TestIr) -> String {
    let mut v: Vec<String> = Vec::new();
    for (i, d) in t.ir.decl_nodes.iter().enumerate() {
        let me = t.path_of(DeclId::new(i));
        let parent = if d.parent.is_none() {
            "<root>".to_string()
        } else {
            t.path_of(d.parent)
        };
        v.push(format!("  {parent} -> {me}"));
    }
    v.sort();
    format!("HIERARCHY(intrusive)\n{}\n", v.join("\n"))
}

/// Dense-ID assignment, for reporting how far apart two layouts are.
pub fn dense_id_form(t: &TestIr) -> String {
    let mut v: Vec<String> = t
        .idx
        .by_path
        .iter()
        .map(|(pk, &did)| {
            let p =
                pk.0.iter()
                    .map(|&s| t.it.resolve(s))
                    .collect::<Vec<_>>()
                    .join(".");
            format!("{p}=#{}", did.raw())
        })
        .collect();
    v.sort();
    v.join(" ")
}
