#[cfg(test)]
mod ir_blake3_hash_tests {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::canonical_hash::Canon;
    use hymeko::ir::hash::HashId;
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass::Interned;
    use hymeko::resolution::intern_pass::intern_ast;
    use hymeko::resolution::resolve::build_index_sym;
    use log::info;
    use parser::ast::AstStr;
    use parser::parse_description;
    use std::collections::HashMap;
    use std::time::Instant;

    const CONTEXT_NAME: &str = "context";
    const EDGE_FIXTURE: &str = "../data/minimal_examples/testing_edges/minimal_example_with_hierarchy_ref_edges_with_values.hymeko";
    const HIERARCHY_FIXTURE: &str =
        "../data/minimal_examples/minimal_example_basic_hierarchy.hymeko";
    type PathSpec = &'static [&'static str];
    const EDGE_HASH_SPECS: [PathSpec; 6] = [
        &[CONTEXT_NAME],
        &[CONTEXT_NAME, "node_lev_0"],
        &[CONTEXT_NAME, "node_lev_1"],
        &[CONTEXT_NAME, "node_lev_1", "e0"],
        &[CONTEXT_NAME, "node_lev_0", "node0"],
        &[CONTEXT_NAME, "node_lev_0", "node1"],
    ];
    const HIERARCHY_HASH_SPECS: [PathSpec; 6] = [
        &[CONTEXT_NAME],
        &[CONTEXT_NAME, "node_lev_0"],
        &[CONTEXT_NAME, "node_lev_0", "node0"],
        &[CONTEXT_NAME, "node_lev_0", "node0", "node0"],
        &[CONTEXT_NAME, "node_lev_0", "node1"],
        &[CONTEXT_NAME, "node_lev_1"],
    ];

    fn start(name: &str, desc: &str) -> Instant {
        log_test_header(name, desc);
        Instant::now()
    }

    fn finish(name: &str, start: Instant, summary: &str) {
        log_test_footer(name, Some(start.elapsed()), summary);
    }

    fn model_hash_from_file(path: &str) -> HashId {
        let source_code = parser::read_source_file(&path).expect("failed to read source file");

        // 2. Parse it, tying the AST lifetimes to the String
        let desc = parser::parse_description(&source_code).unwrap();
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        Canon::new(&ir, &interner).hash().0
    }

    #[test]
    fn model_hash_differs_between_examples() {
        let timer = start(
            "model_hash_differs_between_examples",
            "Confirms different fixtures yield distinct model digests.",
        );
        let edge_hash = model_hash_from_file(EDGE_FIXTURE);
        let hierarchy_hash = model_hash_from_file(HIERARCHY_FIXTURE);
        let edge_hash_again = model_hash_from_file(EDGE_FIXTURE);

        assert_eq!(
            edge_hash.0, edge_hash_again.0,
            "hashing should be deterministic for identical inputs"
        );
        assert_ne!(
            edge_hash.0, hierarchy_hash.0,
            "distinct descriptions must yield distinct model digests"
        );
        info!(
            "edge={:02x?}, hierarchy={:02x?}",
            &edge_hash.0[..4],
            &hierarchy_hash.0[..4]
        );
        finish(
            "model_hash_differs_between_examples",
            timer,
            "Model digests stayed deterministic and differed between fixtures.",
        );
    }

    fn hashes_for_paths<'a>(path: &str, specs: &[&[&'a str]]) -> HashMap<String, HashId> {
        let source_code = parser::read_source_file(&path).expect("failed to read source file");
        let desc = parser::parse_description(&source_code).unwrap();
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        let hashes = Canon::new(&ir, &interner).declaration_hashes();

        let mut out = HashMap::new();
        for spec in specs {
            let mut path_syms = Vec::with_capacity(spec.len());
            for segment in *spec {
                path_syms.push(interner.intern(segment));
            }
            let key = spec.join(".");
            let did = *idx
                .by_path
                .get(&PathKey(path_syms.clone()))
                .unwrap_or_else(|| panic!("missing path {key}"));
            let hash = hashes[did.raw()];
            out.insert(key, hash);
        }
        out
    }

    #[test]
    fn edge_fixture_hashes_cover_subelements() {
        let timer = start(
            "edge_fixture_hashes_cover_subelements",
            "Ensures deterministic hashes across the edge-heavy robotics fixture.",
        );
        let first = hashes_for_paths(EDGE_FIXTURE, &EDGE_HASH_SPECS);
        let second = hashes_for_paths(EDGE_FIXTURE, &EDGE_HASH_SPECS);
        assert_eq!(
            first, second,
            "hashing must be deterministic for edge fixture"
        );

        let context = first[CONTEXT_NAME];
        let level0 = first["context.node_lev_0"];
        let level1 = first["context.node_lev_1"];
        let edge_e0 = first["context.node_lev_1.e0"];
        assert_ne!(context.0, level0.0, "parent and child hashes must differ");
        assert_ne!(
            level1.0, edge_e0.0,
            "edge should hash differently than containing node"
        );

        let node0 = first["context.node_lev_0.node0"];
        let node1 = first["context.node_lev_0.node1"];
        assert_ne!(
            node0.0, node1.0,
            "distinct siblings must yield distinct hashes"
        );
        info!("Edge fixture tracked {} hashes", first.len());
        finish(
            "edge_fixture_hashes_cover_subelements",
            timer,
            "Edge fixture hashes were deterministic and distinct across hierarchy levels.",
        );
    }

    #[test]
    fn hierarchy_fixture_hashes_cover_subelements() {
        let timer = start(
            "hierarchy_fixture_hashes_cover_subelements",
            "Double-checks deterministic hashing for the hierarchy fixture.",
        );
        let first = hashes_for_paths(HIERARCHY_FIXTURE, &HIERARCHY_HASH_SPECS);
        let second = hashes_for_paths(HIERARCHY_FIXTURE, &HIERARCHY_HASH_SPECS);
        assert_eq!(
            first, second,
            "hashing must be deterministic for hierarchy fixture"
        );

        let context = first[CONTEXT_NAME];
        let level0 = first["context.node_lev_0"];
        let level1 = first["context.node_lev_1"];
        assert_ne!(
            context.0, level0.0,
            "context hash should differ from node_lev_0"
        );
        assert_ne!(
            context.0, level1.0,
            "context hash should differ from node_lev_1"
        );

        let inner_node0 = first["context.node_lev_0.node0"];
        let inner_node0_child = first["context.node_lev_0.node0.node0"];
        assert_ne!(
            inner_node0.0, inner_node0_child.0,
            "nested child should impact hash"
        );
        assert_ne!(
            first["context.node_lev_0.node1"].0, inner_node0.0,
            "sibling hashes must differ"
        );
        info!("Hierarchy fixture tracked {} hashes", first.len());
        finish(
            "hierarchy_fixture_hashes_cover_subelements",
            timer,
            "Hierarchy fixture hashing remained deterministic and sensitive to structure.",
        );
    }
}
