#[cfg(test)]
mod test_hash_pass {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass::{Interned, intern_ast};
    use hymeko::resolution::resolve::build_index_sym;
    use log::info;
    use parser::ast::AstStr;
    use parser::parse_description;
    use std::time::Instant;

    // Adjust import path to wherever you placed compute_merkle_hashes
    use hymeko::ir::hash_pass::compute_merkle_hashes;

    fn start(name: &str, desc: &str) -> Instant {
        log_test_header(name, desc);
        Instant::now()
    }

    fn finish(name: &str, start: Instant, summary: &str) {
        log_test_footer(name, Some(start.elapsed()), summary);
    }

    #[test]
    fn deterministic_merkle_hashing() {
        let timer = start(
            "deterministic_merkle_hashing",
            "Ensures identical structures hash identically and parents differ from children.",
        );
        // We define two completely separate roots that contain an identical leaf
        let src = r#"
        hash_example {}
        Root1 { Leaf {} }
        Root2 { Leaf {} }
        "#;

        let desc: AstStr = parse_description(src).expect("parse failed");
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &mut interner).expect("index build failed");
        let mut ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower failed");

        // Execute the linear-time hashing pass [cite: 2026-02-08]
        compute_merkle_hashes(&mut ir, &interner);

        let sid_root1 = interner.intern("Root1");
        let sid_root2 = interner.intern("Root2");
        let sid_leaf = interner.intern("Leaf");

        let did_leaf1 = *idx
            .by_path
            .get(&PathKey(vec![sid_root1, sid_leaf]))
            .unwrap();
        let did_leaf2 = *idx
            .by_path
            .get(&PathKey(vec![sid_root2, sid_leaf]))
            .unwrap();
        let did_root1 = *idx.by_path.get(&PathKey(vec![sid_root1])).unwrap();

        // Extract the populated hashes
        // Assuming HashId(pub [u8; 32])
        let hash_leaf1 = ir.decl_hash[did_leaf1.0 as usize].expect("Leaf1 missing hash");
        let hash_leaf2 = ir.decl_hash[did_leaf2.0 as usize].expect("Leaf2 missing hash");
        let hash_root1 = ir.decl_hash[did_root1.0 as usize].expect("Root1 missing hash");

        // 1. Proof of Content Addressability:
        // Structurally identical nodes must compute the exact same HashId
        assert_eq!(
            hash_leaf1.0, hash_leaf2.0,
            "Identical structures must have identical hashes"
        );

        // 2. Proof of Avalanche Effect:
        // A parent's hash must be mathematically distinct from its child's hash
        assert_ne!(
            hash_root1.0, hash_leaf1.0,
            "Parent hash must differ from child hash"
        );
        info!(
            "Leaf hash {:?} == {:?}; root hash {:?}",
            hash_leaf1.0, hash_leaf2.0, hash_root1.0
        );
        finish(
            "deterministic_merkle_hashing",
            timer,
            "Merkle hashing stayed deterministic while preserving avalanche behavior.",
        );
    }
}
