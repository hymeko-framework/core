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
    AnnoR, ArcRec, DeclKind, DeclNode, EdgeRec, IncidenceSign, Ir, NodeRec, PortDirection,
    RefAtomR, ReferenceDraft, SignedRefR, ValueR,
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
    pub fn kind(self) -> IncidenceSign {
        match self {
            Self::Plus => IncidenceSign::Plus,
            Self::Minus => IncidenceSign::Minus,
            Self::Neutral => IncidenceSign::Neutral,
        }
    }
    fn wrap(self, atom: RefAtomR) -> ReferenceDraft {
        ReferenceDraft::new(self.kind(), atom)
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
        let id = self.ir.push_decl(DeclNode {
            kind,
            name,
            parent: parent_hint,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        self.idx.by_path.insert(pk, id);
        id
    }

    pub fn node(&mut self, path: &[&str], parent_hint: DeclId) -> DeclId {
        let d = self.decl(path, DeclKind::Node, parent_hint);
        let nid = NodeId::new(self.ir.nodes.len());
        self.ir
            .nodes
            .push(NodeRec::new(d, hymeko::common::ids::BaseListId::NONE));
        self.ir.bind_node_decl(d, nid).unwrap();
        d
    }

    pub fn edge(&mut self, path: &[&str], parent_hint: DeclId) -> DeclId {
        let d = self.decl(path, DeclKind::Edge, parent_hint);
        let eid = EdgeId::new(self.ir.edges.len());
        self.ir
            .edges
            .push(EdgeRec::new(d, hymeko::common::ids::BaseListId::NONE));
        self.ir.bind_edge_decl(d, eid).unwrap();
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
        let eid = self
            .ir
            .as_edge(edge_decl)
            .expect("edge_decl is not an edge");
        let aid = HyperArcId::new(self.ir.arcs.len());
        let built: Vec<SignedRefR> = refs
            .iter()
            .map(|(target, sign, weights)| {
                self.ir.store_reference(
                    sign.wrap(RefAtomR::plain(*target).with_weights(weights.clone())),
                )
            })
            .collect();
        self.ir.arcs.push(ArcRec {
            anno: hymeko::common::ids::AnnotationId::NONE,
            in_edge: edge_decl,
            refs: built,
        });
        self.ir.edges[eid.raw()].arcs.push(aid);
        aid
    }

    /// Attach annotation tags to a declaration.
    pub fn tag(&mut self, d: DeclId, tags: &[&str]) {
        let syms: Vec<SymId> = tags.iter().map(|t| self.it.intern(t)).collect();
        self.ir.decl_annotation_mut(d).unwrap().tags = syms;
    }

    /// Attach inheritance bases to a node or edge.
    pub fn bases(&mut self, d: DeclId, bases: &[(DeclId, S)]) {
        let built: Vec<SignedRefR> = bases
            .iter()
            .map(|(t, s)| self.ir.store_reference(s.wrap(RefAtomR::plain(*t))))
            .collect();
        let built = self.ir.store_bases(built);
        if let Some(nid) = self.ir.as_node(d) {
            self.ir.nodes[nid.raw()].bases = built;
        } else if let Some(eid) = self.ir.as_edge(d) {
            self.ir.edges[eid.raw()].bases = built;
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
        format!("<unindexed DeclId({})>", d.raw())
    }

    /// Deliberate payload mutation for hash counterexamples, through the IR owner.
    pub fn edit_reference(&mut self, arc: usize, slot: usize, mutate: impl FnOnce(&mut RefAtomR)) {
        let reference = self.ir.arcs[arc].refs[slot];
        let mut draft = self
            .ir
            .reference_draft(&reference)
            .expect("fixture payload");
        mutate(&mut draft.atom);
        let updated = self.ir.store_reference(draft);
        self.ir.arcs[arc].refs[slot] = updated;
    }
}

impl TestIr {
    /// Give every `ArcRec` a matching `HyperArc` **declaration** and register it
    /// in `decl_to_arc`.
    ///
    /// Bind record-only arcs to declarations for dispatch and hierarchy checks.
    pub fn attach_arc_decls(&mut self) {
        for ai in 0..self.ir.arcs.len() {
            let target = HyperArcId::new(ai);
            if (0..self.ir.decl_nodes.len()).any(|d| self.ir.as_arc(DeclId::new(d)) == Some(target))
            {
                continue; // already declared
            }
            let in_edge = self.ir.arcs[ai].in_edge;
            let name = self.it.intern(&format!("__arc{ai}"));
            let did = self.ir.push_decl(DeclNode {
                kind: DeclKind::HyperArc,
                name,
                parent: in_edge,
                first_child: DeclId::NONE,
                last_child: DeclId::NONE,
                next_sibling: DeclId::NONE,
                anno: AnnoR::default(),
            });
            self.ir.bind_arc_decl(did, target).unwrap();
        }
    }
}
