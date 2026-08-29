//! Test-only IR builder with *controllable* dense-ID assignment.
//!
//! APPROVED-CORE-EDIT: unified-ir-m0-5-tests-and-benchmarks (2026-08-28).
//!
//! The point of this builder is that **path assignment and dense `DeclId`
//! assignment are independent**. Production lowering ties them together —
//! declarations get their `DeclId` in the order the parser meets them — so a
//! test that went through lowering could not separate "same meaning" from
//! "same layout". Here the caller chooses the insertion order, and the paths
//! are whatever the caller says, so two builders can produce IRs that agree on
//! every path and disagree on every index.
//!
//! Nothing here is production code and nothing here is a substitute for
//! lowering: it exercises the *representation*, not the language.

use hymeko::common::ids::{DeclId, EdgeId, HyperArcId, NodeId, SymId};
use hymeko::common::pathkey::PathKey;
use hymeko::ir::ir::{
    AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, Ir, NodeRec, RefAtomR, SignedRefR, ValueR,
};
use hymeko::resolution::interner::Interner;
use hymeko::resolution::resolve::Index;

/// Sign, mirrored locally so the tests do not depend on the editor crate.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum S {
    Plus,
    Minus,
    Neutral,
}

impl S {
    fn wrap(self, atom: RefAtomR) -> SignedRefR {
        match self {
            Self::Plus => SignedRefR::Plus(atom),
            Self::Minus => SignedRefR::Minus(atom),
            Self::Neutral => SignedRefR::Neutral(atom),
        }
    }
}

pub struct TestIr {
    pub ir: Ir,
    pub idx: Index,
    pub it: Interner,
}

impl TestIr {
    pub fn new() -> Self {
        Self {
            ir: Ir::default(),
            idx: Index::new(),
            it: Interner::new(),
        }
    }

    fn path_key(&mut self, path: &[&str]) -> PathKey {
        PathKey(path.iter().map(|s| self.it.intern(s)).collect())
    }

    /// Append a declaration and register its path.
    ///
    /// `parent_hint` populates `DeclNode.parent`. It is deliberately a
    /// parameter rather than derived from the path, because part of what M0.5
    /// establishes is whether the canonical hash reads the intrusive hierarchy
    /// at all — a test cannot answer that if the builder keeps the two in sync
    /// by construction.
    pub fn decl(&mut self, path: &[&str], kind: DeclKind, parent_hint: DeclId) -> DeclId {
        let pk = self.path_key(path);
        let name = *pk.0.last().expect("path must not be empty");
        let id = DeclId::new(self.ir.decl_nodes.len());
        self.ir.decl_nodes.push(DeclNode {
            kind,
            name,
            parent: parent_hint,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        self.ir.decl_hash.push(None);
        self.ir.decl_to_node.push(None);
        self.ir.decl_to_edge.push(None);
        self.ir.decl_to_arc.push(None);
        self.idx.by_path.insert(pk, id);
        id
    }

    pub fn node(&mut self, path: &[&str], parent_hint: DeclId) -> DeclId {
        let d = self.decl(path, DeclKind::Node, parent_hint);
        let nid = NodeId::new(self.ir.nodes.len());
        self.ir.nodes.push(NodeRec::new(d, Vec::new()));
        self.ir.decl_to_node[d.0] = Some(nid);
        d
    }

    pub fn edge(&mut self, path: &[&str], parent_hint: DeclId) -> DeclId {
        let d = self.decl(path, DeclKind::Edge, parent_hint);
        let eid = EdgeId::new(self.ir.edges.len());
        self.ir.edges.push(EdgeRec::new(d, Vec::new()));
        self.ir.decl_to_edge[d.0] = Some(eid);
        d
    }

    /// Attach an arc to `edge_decl` with the given signed references.
    pub fn arc(&mut self, edge_decl: DeclId, refs: &[(DeclId, S)]) -> HyperArcId {
        self.arc_weighted(
            edge_decl,
            &refs.iter().map(|&(t, s)| (t, s, None)).collect::<Vec<_>>(),
        )
    }

    pub fn arc_weighted(
        &mut self,
        edge_decl: DeclId,
        refs: &[(DeclId, S, Option<Vec<ValueR>>)],
    ) -> HyperArcId {
        let eid = self.ir.decl_to_edge[edge_decl.0].expect("edge_decl is not an edge");
        let aid = HyperArcId::new(self.ir.arcs.len());
        let built: Vec<SignedRefR> = refs
            .iter()
            .map(|(target, sign, weights)| {
                sign.wrap(RefAtomR {
                    target: *target,
                    anno: AnnoR::default(),
                    weights: weights.clone(),
                })
            })
            .collect();
        self.ir.arcs.push(ArcRec {
            anno: AnnoR::default(),
            in_edge: edge_decl,
            refs: built,
        });
        self.ir.arc_hash.push(None);
        self.ir.edges[eid.0].arcs.push(aid);
        aid
    }

    /// Attach annotation tags to a declaration.
    pub fn tag(&mut self, d: DeclId, tags: &[&str]) {
        let syms: Vec<SymId> = tags.iter().map(|t| self.it.intern(t)).collect();
        self.ir.decl_nodes[d.0].anno.tags = syms;
    }

    /// Attach inheritance bases to a node or edge.
    pub fn bases(&mut self, d: DeclId, bases: &[(DeclId, S)]) {
        let built: Vec<SignedRefR> = bases
            .iter()
            .map(|(t, s)| {
                s.wrap(RefAtomR {
                    target: *t,
                    anno: AnnoR::default(),
                    weights: None,
                })
            })
            .collect();
        if let Some(nid) = self.ir.decl_to_node[d.0] {
            self.ir.nodes[nid.0].bases = built;
        } else if let Some(eid) = self.ir.decl_to_edge[d.0] {
            self.ir.edges[eid.0].bases = built;
        } else {
            panic!("bases target is neither node nor edge");
        }
    }

    /// Resolve a `DeclId` back to its dotted path, for oracle output.
    pub fn path_of(&self, d: DeclId) -> String {
        for (pk, &did) in &self.idx.by_path {
            if did == d {
                return pk
                    .0
                    .iter()
                    .map(|&s| self.it.resolve(s))
                    .collect::<Vec<_>>()
                    .join(".");
            }
        }
        format!("<unindexed DeclId({})>", d.0)
    }
}

impl TestIr {
    /// Give every `ArcRec` a matching `HyperArc` **declaration** and register it
    /// in `decl_to_arc`.
    ///
    /// [`TestIr::arc`] creates records without declarations, mirroring what the
    /// editor bridge does. Anything that resolves an arc through `as_arc` /
    /// `arc_for_decl`, or that walks arcs via the merkle pass, needs the
    /// declaration side to exist as well. M0.6 learned this the hard way: a
    /// missing arc declaration made `compute_merkle_hashes` look as though it
    /// ignored incidence signs, when in fact the fixture never reached them.
    pub fn attach_arc_decls(&mut self) {
        for ai in 0..self.ir.arcs.len() {
            if self.ir.decl_to_arc.iter().flatten().any(|a| a.0 == ai) {
                continue; // already declared
            }
            let in_edge = self.ir.arcs[ai].in_edge;
            let name = self.it.intern(&format!("__arc{ai}"));
            let did = DeclId::new(self.ir.decl_nodes.len());
            self.ir.decl_nodes.push(DeclNode {
                kind: DeclKind::HyperArc,
                name,
                parent: in_edge,
                first_child: DeclId::NONE,
                last_child: DeclId::NONE,
                next_sibling: DeclId::NONE,
                anno: AnnoR::default(),
            });
            self.ir.decl_hash.push(None);
            self.ir.decl_to_node.push(None);
            self.ir.decl_to_edge.push(None);
            self.ir.decl_to_arc.push(None);
            self.ir.decl_to_arc[did.0] = Some(HyperArcId::new(ai));
        }
    }
}
