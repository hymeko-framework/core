#[cfg(test)]
mod ir_blake3_hash_tests {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use blake3::Hasher;
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::hash::HashId;
    use hymeko::ir::hash_pass::compute_merkle_hashes;
    use hymeko::ir::ir::DeclKind;
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass::Interned;
    use hymeko::resolution::intern_pass::intern_ast;
    use hymeko::resolution::resolve::build_index_sym;
    use log::info;
    use parser::ast::AstStr;
    use parser::parse_description;
    use std::collections::HashMap;
    use std::time::Instant;

    const SINGLETON_SRC: &str = r#"
        HashDoc {}
        Root;
        "#;
    const ROOT_NAME: &str = "Root";
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

    #[test]
    fn singleton_node_hash_matches_blake3_preimage() {
        let timer = start(
            "singleton_node_hash_matches_blake3_preimage",
            "Verifies raw BLAKE3 digest for a single node matches compute_merkle_hashes output.",
        );
        let desc: AstStr = parse_description(SINGLETON_SRC).expect("parse failed");
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &mut interner).expect("index build failed");
        let mut ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        compute_merkle_hashes(&mut ir, &interner);

        let sid_root = interner.intern(ROOT_NAME);
        let did_root = *idx
            .by_path
            .get(&PathKey(vec![sid_root]))
            .expect("missing Root decl");

        let actual = ir.decl_hash[did_root.0 as usize].expect("Root hash not computed");

        // Manually reproduce the hashing preimage for this simple node.
        let mut hasher = Hasher::new();
        hasher.update(&[DeclKind::Node as u8]);
        hasher.update(ROOT_NAME.as_bytes());
        hasher.update(&(0u64).to_le_bytes()); // zero tags
        hasher.update(&[0]); // no value present
        let expected = HashId(*hasher.finalize().as_bytes());

        assert_eq!(
            actual.0, expected.0,
            "Root hash must match raw BLAKE3 digest"
        );
        info!("Singleton hash {:02x?}", &actual.0[..4]);
        finish(
            "singleton_node_hash_matches_blake3_preimage",
            timer,
            "Manual BLAKE3 preimage matched the IR hashing pipeline.",
        );
    }

    fn context_hash_from_file(path: &str) -> HashId {
        let source_code = parser::read_source_file(&path).expect("failed to read source file");

        // 2. Parse it, tying the AST lifetimes to the String
        let desc = parser::parse_description(&source_code).unwrap();
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let mut ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        compute_merkle_hashes(&mut ir, &interner);

        let sid_context = interner.intern(CONTEXT_NAME);
        let did_context = *idx
            .by_path
            .get(&PathKey(vec![sid_context]))
            .expect("context missing");

        ir.decl_hash[did_context.0 as usize].expect("context hash missing")
    }

    #[test]
    fn context_hash_differs_between_examples() {
        let timer = start(
            "context_hash_differs_between_examples",
            "Confirms different fixtures yield distinct context hashes.",
        );
        let edge_hash = context_hash_from_file(EDGE_FIXTURE);
        let hierarchy_hash = context_hash_from_file(HIERARCHY_FIXTURE);
        let edge_hash_again = context_hash_from_file(EDGE_FIXTURE);

        assert_eq!(
            edge_hash.0, edge_hash_again.0,
            "hashing should be deterministic for identical inputs"
        );
        assert_ne!(
            edge_hash.0, hierarchy_hash.0,
            "distinct descriptions must yield distinct context hashes"
        );
        info!(
            "edge={:02x?}, hierarchy={:02x?}",
            &edge_hash.0[..4],
            &hierarchy_hash.0[..4]
        );
        finish(
            "context_hash_differs_between_examples",
            timer,
            "Context hashes stayed deterministic and differed between fixtures.",
        );
    }

    fn hashes_for_paths<'a>(path: &str, specs: &[&[&'a str]]) -> HashMap<String, HashId> {
        let source_code = parser::read_source_file(&path).expect("failed to read source file");
        let desc = parser::parse_description(&source_code).unwrap();
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &interner).expect("index build failed");
        let mut ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower_to_ir failed");

        compute_merkle_hashes(&mut ir, &interner);

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
            let hash =
                ir.decl_hash[did.0 as usize].unwrap_or_else(|| panic!("missing hash for {key}"));
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
