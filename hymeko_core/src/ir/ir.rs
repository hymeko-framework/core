use crate::common::ids::{DeclId, EdgeId, HyperArcId, NodeId, SymId};
use crate::ir::hash::HashId;
use crate::ir::meta::Meta;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclKind {
    Node,
    Edge,
    HyperArc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclNode {
    pub kind: DeclKind,
    pub name: SymId,
    pub parent: DeclId,
    pub first_child: DeclId,
    pub last_child: DeclId,
    pub next_sibling: DeclId,
    pub anno: AnnoR,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Ir {
    pub meta: Option<Meta>,
    pub doc_hash: Option<HashId>,
    pub decl_hash: Vec<Option<HashId>>,
    pub arc_hash: Vec<Option<HashId>>,
    // Unified Arena
    pub decl_nodes: Vec<DeclNode>,

    // “konkrét” táblák
    pub nodes: Vec<NodeRec>, // NodeId indexel
    pub edges: Vec<EdgeRec>, // EdgeId indexel
    pub arcs: Vec<ArcRec>,   // ArcId indexel

    // DeclId -> NodeId/EdgeId leképzés (gyors jump)
    pub decl_to_node: Vec<Option<NodeId>>,
    pub decl_to_edge: Vec<Option<EdgeId>>,
    pub decl_to_arc: Vec<Option<HyperArcId>>,
}

impl Ir {
    pub fn new(meta: Meta) -> Self {
        Self {
            meta: Some(meta),
            ..Default::default()
        }
    }

    #[inline]
    pub fn decl_node(&self, id: DeclId) -> Option<&DeclNode> {
        if id.is_none() {
            return None;
        }
        self.decl_nodes.get(id.0)
    }

    #[inline]
    pub fn decl_node_unchecked(&self, id: DeclId) -> &DeclNode {
        assert!(id.is_some(), "attempted to dereference DeclId::NONE");
        &self.decl_nodes[id.0]
    }

    pub fn decl_children(&self, parent: DeclId) -> DeclChildren<'_> {
        let next = if parent.is_none() {
            DeclId::NONE
        } else {
            self.decl_nodes[parent.0].first_child
        };
        DeclChildren { ir: self, next }
    }

    pub fn decl_kind(&self, did: DeclId) -> DeclKind {
        self.decl_nodes[did.0].kind
    }

    #[inline]
    pub fn first_child(&self, item: DeclId) -> DeclId {
        if item.is_none() {
            return DeclId::NONE;
        }
        self.decl_nodes[item.0].first_child
    }

    #[inline]
    pub fn next_sibling(&self, item: DeclId) -> DeclId {
        if item.is_none() {
            return DeclId::NONE;
        }
        self.decl_nodes[item.0].next_sibling
    }

    #[inline]
    pub fn parent(&self, item: DeclId) -> DeclId {
        if item.is_none() {
            return DeclId::NONE;
        }
        self.decl_nodes[item.0].parent
    }

    #[inline]
    pub fn kind(&self, item: DeclId) -> DeclKind {
        self.decl_nodes[item.0].kind
    }

    /// Children iterator for *any* item (node/edge/arc).
    pub fn children(&self, parent: DeclId) -> DeclChildren<'_> {
        self.decl_children(parent)
    }

    pub fn as_node(&self, d: DeclId) -> Option<NodeId> {
        if d.is_none() {
            return None;
        }
        self.decl_to_node.get(d.0).copied().flatten()
    }
    pub fn as_edge(&self, d: DeclId) -> Option<EdgeId> {
        if d.is_none() {
            return None;
        }
        self.decl_to_edge.get(d.0).copied().flatten()
    }
    pub fn as_arc(&self, d: DeclId) -> Option<HyperArcId> {
        if d.is_none() {
            return None;
        }
        self.decl_to_arc.get(d.0).copied().flatten()
    }

    pub fn preallocate_from_index(&mut self, total_named_decls: usize) {
        let default_node = DeclNode {
            kind: DeclKind::Node,
            name: SymId::new(0),
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        };
        // Allocate exact capacity for Nodes and Edges
        self.decl_nodes.resize(total_named_decls, default_node);
        self.decl_to_node.resize(total_named_decls, None);
        self.decl_to_edge.resize(total_named_decls, None);
        self.decl_to_arc.resize(total_named_decls, None);
        self.decl_hash.resize(total_named_decls, None);
    }

    pub fn push_anonymous_arc(&mut self) -> DeclId {
        let id = DeclId::new(self.decl_nodes.len());
        self.decl_nodes.push(DeclNode {
            kind: DeclKind::HyperArc,
            name: SymId::new(0),
            parent: DeclId::NONE,
            first_child: DeclId::NONE,
            last_child: DeclId::NONE,
            next_sibling: DeclId::NONE,
            anno: AnnoR::default(),
        });
        self.decl_to_node.push(None);
        self.decl_to_edge.push(None);
        self.decl_to_arc.push(None);
        self.decl_hash.push(None);
        id
    }
}

pub struct DeclChildren<'a> {
    ir: &'a Ir,
    next: DeclId,
}

impl<'a> Iterator for DeclChildren<'a> {
    type Item = DeclId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next.is_none() {
            return None;
        }
        let cur = self.next;
        self.next = self.ir.next_sibling(cur);
        Some(cur)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRec {
    pub decl: DeclId,
    pub bases: Vec<SignedRefR>,
}
impl NodeRec {
    pub fn new(decl: DeclId, bases: Vec<SignedRefR>) -> Self {
        Self { decl, bases }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeRec {
    pub decl: DeclId,
    pub bases: Vec<SignedRefR>,
    pub arcs: Vec<HyperArcId>,
}
impl EdgeRec {
    pub fn new(decl: DeclId, bases: Vec<SignedRefR>) -> Self {
        Self {
            decl,
            bases,
            arcs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcRec {
    pub anno: AnnoR,
    pub in_edge: DeclId, // melyik edge scope-jában volt (általában EdgeDecl)
    pub refs: Vec<SignedRefR>, // DeclId-kra mutat (determinista)
                         // később: attribútumok/value külön, lásd lent
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefAtomR {
    pub target: DeclId,
    pub anno: AnnoR,
    pub weights: Option<Vec<ValueR>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SignedRefR {
    Plus(RefAtomR),
    Minus(RefAtomR),
    Neutral(RefAtomR),
}

impl SignedRefR {
    // Extract inner atom (RefAtomR) regardless of sign
    #[inline]
    pub fn atom(&self) -> &RefAtomR {
        match self {
            Self::Plus(r) | Self::Minus(r) | Self::Neutral(r) => r,
        }
    }

    // Target 'DeclId' of this reference
    #[inline]
    pub fn target(&self) -> DeclId {
        self.atom().target
    }

    // Sign as i8
    #[inline]
    pub fn sign(&self) -> i8 {
        match self {
            Self::Plus(_) => 1,
            Self::Minus(_) => -1,
            Self::Neutral(_) => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueR {
    Str(SymId),
    Num(f64),
    List(Vec<ValueR>),
    Ref(DeclId), // AST Ref(path) -> IR Ref(DeclId)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnnoR {
    pub tags: Vec<SymId>,
    pub value: Option<ValueR>,
}
