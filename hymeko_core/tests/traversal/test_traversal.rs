#![cfg(test)]
mod test_traversal {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use crate::traversal::constants::*;
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass::{Interned, intern_ast};
    use hymeko::resolution::resolve::build_index_sym;
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko_hnn::traversal::graph_traversal::dfs_preorder;
    use hymeko_hnn::traversal::hypergraphview::{BergeState, BergeView, HyperGraphView};
    use log::info;
    use parser::ast::AstStr;
    use parser::parse_description;
    use std::collections::HashSet;
    use std::time::Instant;

    fn start(name: &str, desc: &str) -> Instant {
        log_test_header(name, desc);
        Instant::now()
    }

    fn finish(name: &str, start: Instant, summary: &str) {
        log_test_footer(name, Some(start.elapsed()), summary);
    }

    #[test]
    fn berge_dfs_reaches_other_node() {
        let timer = start(
            "berge_dfs_reaches_other_node",
            "Ensures Berge DFS from A visits edge E and node B.",
        );
        let desc: AstStr = parse_description(BERGE_SRC).expect("parse failed");
        let Interned { ast, mut interner } = intern_ast(&desc);

        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        // --- DeclId-k kikeresése path alapján
        let sid_d = interner.intern(NAMESPACE_D);
        let sid_a = interner.intern(NODE_A);
        let sid_b = interner.intern(NODE_B);
        let sid_root = interner.intern(ROOT_NAME);
        let sid_e = interner.intern(EDGE_E);

        let did_a = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_a]))
            .expect("D.A missing");
        let did_b = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_b]))
            .expect("D.B missing");
        let did_e = *idx
            .by_path
            .get(&PathKey(vec![sid_d, sid_root, sid_e]))
            .expect("D.Root.E missing");

        // --- Decl -> NodeId / EdgeId
        let nid_a = ir.decl_to_node[did_a.0 as usize].expect("A not lowered as node");
        let nid_b = ir.decl_to_node[did_b.0 as usize].expect("B not lowered as node");
        let eid_e = ir.decl_to_edge[did_e.0 as usize].expect("E not lowered as edge");

        // --- HyperGraphView felépítése
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&ir, &AGG_CFG, &ex);

        // 1) incidenciák ellenőrzése (set-alapon)
        let (a_start, a_end) = hg.node_span(nid_a);
        let a_edges = &hg.flat_node_edges[a_start..a_end];
        assert!(a_edges.contains(&eid_e), "A should be incident to E");

        let (b_start, b_end) = hg.node_span(nid_b);
        let b_edges = &hg.flat_node_edges[b_start..b_end];
        assert!(b_edges.contains(&eid_e), "B should be incident to E");

        let (e_start, e_end) = hg.edge_span(eid_e);
        let e_nodes = &hg.flat_edge_nodes[e_start..e_end];
        assert!(e_nodes.contains(&nid_a), "E should connect to A");
        assert!(e_nodes.contains(&nid_b), "E should connect to B");

        // 2) Berge-DFS: A-ból indulva elérjük (Edge E)-t és B-t
        let bv = BergeView { hg: &hg };
        let got = dfs_preorder(&bv, BergeState::Node(nid_a));

        let got_set: HashSet<BergeState> = got.into_iter().collect();

        assert!(
            got_set.contains(&BergeState::Node(nid_a)),
            "DFS should include start node A"
        );
        assert!(
            got_set.contains(&BergeState::Edge(eid_e)),
            "DFS should include edge E"
        );
        assert!(
            got_set.contains(&BergeState::Node(nid_b)),
            "DFS should reach node B"
        );
        info!(
            "Berge DFS visited {} states (nodes={}, edges={})",
            got_set.len(),
            got_set
                .iter()
                .filter(|s| matches!(s, BergeState::Node(_)))
                .count(),
            got_set
                .iter()
                .filter(|s| matches!(s, BergeState::Edge(_)))
                .count()
        );
        finish(
            "berge_dfs_reaches_other_node",
            timer,
            "DFS traversal covered the expected node/edge frontier.",
        );
    }
}
