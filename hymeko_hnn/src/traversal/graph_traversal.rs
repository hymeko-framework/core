use std::collections::{HashSet, VecDeque};
use hymeko::tensor::common::Real;
use hymeko::tensor::tensor_val::{EdgeWeight, IncVal};
use crate::traversal::graphview::GraphView;
use crate::traversal::hypergraphview::{BergeState, BergeView};

struct VisitedSet {
    nodes: Vec<bool>,
    edges: Vec<bool>,
}

impl VisitedSet {
    fn new(num_nodes: usize, num_edges: usize) -> Self {
        Self {
            nodes: vec![false; num_nodes],
            edges: vec![false; num_edges],
        }
    }

    #[inline(always)]
    fn mark_and_check(&mut self, s: BergeState) -> bool {
        match s {
            BergeState::Node(n) => {
                let v = self.nodes[n.0];
                self.nodes[n.0] = true;
                v // returns true if it WAS already visited
            }
            BergeState::Edge(e) => {
                let v = self.edges[e.0];
                self.edges[e.0] = true;
                v
            }
        }
    }
}

pub fn bfs<V: GraphView>(view: &V, start: V::Node) -> Vec<V::Node> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut q = VecDeque::new();

    seen.insert(start);
    q.push_back(start);

    while let Some(u) = q.pop_front() {
        out.push(u);
        for v in view.neighbors(u) {
            if seen.insert(v) {
                q.push_back(v);
            }
        }
    }
    out
}

pub fn dfs_preorder<V: GraphView>(view: &V, start: V::Node) -> Vec<V::Node> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut st = vec![start];

    while let Some(u) = st.pop() {
        if !seen.insert(u) { continue; }
        out.push(u);

        for v in view.neighbors(u) {
            st.push(v);
        }
    }
    out
}

pub fn bfs_dense<'a, V, EW, F>(view: &BergeView<'a, V, EW, F>, start: BergeState) -> Vec<BergeState>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real,
{
    let mut out = Vec::new();
    let mut q = VecDeque::new();

    let num_nodes = view.hg.node_offsets.len() - 1;
    let num_edges = view.hg.edge_offsets.len() - 1;
    let mut seen = VisitedSet::new(num_nodes, num_edges);

    seen.mark_and_check(start);
    q.push_back(start);

    while let Some(u) = q.pop_front() {
        out.push(u);
        for v in view.neighbors(u) {
            if !seen.mark_and_check(v) {
                q.push_back(v);
            }
        }
    }
    out
}

pub fn dfs_preorder_dense<'a, V, EW, F>(view: &BergeView<'a, V, EW, F>, start: BergeState) -> Vec<BergeState>
where
    V: IncVal<F>,
    EW: EdgeWeight<V, F>,
    F: Real
{
    let mut out = Vec::new();
    let mut st = vec![start];

    let num_nodes = view.hg.node_offsets.len() - 1;
    let num_edges = view.hg.edge_offsets.len() - 1;
    let mut seen = VisitedSet::new(num_nodes, num_edges);

    while let Some(u) = st.pop() {
        if seen.mark_and_check(u) { continue; }
        out.push(u);

        for v in view.neighbors(u) {
            st.push(v);
        }
    }
    out
}