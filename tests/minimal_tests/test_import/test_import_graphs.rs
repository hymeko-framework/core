#![cfg(test)]
mod test_import_graphs {
    use crate::minimal_tests::TestParser;
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::hash_pass::compute_merkle_hashes;
    use hymeko::ir::ir::{DeclKind, SignedRefR};
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::module_store::module_store::ModuleStore;
    use hymeko::module_store::source_provider::StdFsProvider;
    use hymeko::resolution::intern_pass::{Interned, intern_ast};
    use hymeko::resolution::resolve::build_index_sym;
    use log::info;
    use std::path::Path;
    use std::time::Instant;

    #[test]
    fn check_import_graph_library() {
        log_test_header(
            "check_import_graph_library",
            "Lowers the library import example and inspects operand/operator bindings.",
        );
        let start = Instant::now();
        let path = "../data/minimal_examples/import_examples/minimal_example_library.hymeko";
        let source_code = parser::read_source_file(&path).expect("failed to read source file");

        // 2. Parse it, tying the AST lifetimes to the String
        let desc = parser::parse_description(&source_code).unwrap();
        assert_eq!(desc.name, "basic_library");
        let Interned { ast, mut interner } = intern_ast(&desc);
        let idx = build_index_sym(&ast, &mut interner).expect("index build failed");
        let mut ir = lower_to_ir(&ast, &idx, &mut interner).expect("lower failed");
        compute_merkle_hashes(&mut ir, &interner);

        let sid_elements = interner.intern("elements");
        let sid_operand = interner.intern("operand");
        let sid_operand2 = interner.intern("operand2");
        let sid_operator = interner.intern("operator");

        let did_elements = *idx
            .by_path
            .get(&PathKey(vec![sid_elements]))
            .expect("elements missing");
        assert_eq!(ir.decl_nodes[did_elements.0].kind, DeclKind::Node);

        let did_operand = *idx
            .by_path
            .get(&PathKey(vec![sid_elements, sid_operand]))
            .expect("operand missing");
        let did_operand2 = *idx
            .by_path
            .get(&PathKey(vec![sid_elements, sid_operand2]))
            .expect("operand2 missing");
        assert_eq!(ir.decl_nodes[did_operand.0].kind, DeclKind::Node);
        assert_eq!(ir.decl_nodes[did_operand2.0].kind, DeclKind::Node);

        let did_operator = *idx
            .by_path
            .get(&PathKey(vec![sid_elements, sid_operator]))
            .expect("operator edge missing");
        assert_eq!(ir.decl_nodes[did_operator.0].kind, DeclKind::Edge);

        let edge_id = ir.decl_to_edge[did_operator.0].expect("operator not lowered as edge");
        let edge_rec = &ir.edges[edge_id.0];
        assert_eq!(
            edge_rec.arcs.len(),
            1,
            "operator edge should contain one arc"
        );

        let arc_id = edge_rec.arcs[0];
        let arc = &ir.arcs[arc_id.0];
        assert_eq!(arc.refs.len(), 2, "operator arc should connect two refs");

        match (&arc.refs[0], &arc.refs[1]) {
            (SignedRefR::Plus(lhs), SignedRefR::Minus(rhs)) => {
                assert_eq!(
                    lhs.target, did_operand,
                    "+ operand should target operand node"
                );
                assert_eq!(
                    rhs.target, did_operand,
                    "- operand should target operand node"
                );
                assert!(lhs.weights.as_ref().is_some(), "lhs weight missing");
                assert!(rhs.weights.as_ref().is_some(), "rhs weight missing");
            }
            other => panic!("unexpected arc refs ordering: {other:?}"),
        }
        info!(
            "{} lowered to IR with {} nodes and {} edges",
            path,
            ir.nodes.len(),
            ir.edges.len()
        );
        log_test_footer(
            "check_import_graph_library",
            Some(start.elapsed()),
            "Library operands/operators survived IR lowering with expected refs.",
        );
    }

    /// Cross-profile instance references (APPROVED-CORE-EDIT: xprofile-instance-refs, 2026-06-19).
    ///
    /// `xprofile_importer` imports `xprofile_shared` — a *profile* (not a bare vocab file): it has a
    /// `_description` wrapper with `using basic_library.elements as el`, and its `shared_thing` decl
    /// uses that alias as a base. Against the prior `compile()`, the dep's `using` was never applied
    /// (only the root's), so `el.operand` raised `UnresolvedRef` and compile failed. With the fix the
    /// shared profile lowers and its instance decl is referenceable cross-profile (`xs.shared_thing`).
    #[test]
    fn check_xprofile_instance_ref() {
        let root_path =
            Path::new("../data/minimal_examples/import_examples/xprofile_importer.hymeko");
        let mut ms = ModuleStore::new(StdFsProvider::new(), TestParser);
        let compiled = ms.compile(root_path).expect("cross-profile compile failed");

        // the shared profile's instance decl is indexed at [desc, content, shared_thing].
        let desc = ms.it.intern("xprofile_shared_description");
        let content = ms.it.intern("xprofile_shared");
        let thing = ms.it.intern("shared_thing");
        let did = *compiled
            .idx
            .by_path
            .get(&PathKey(vec![desc, content, thing]))
            .expect("shared_thing not indexed cross-profile");

        // the importer's E1 arc references it (proof the cross-profile ref resolved).
        let mut referenced = false;
        for arc in &compiled.ir.arcs {
            for r in &arc.refs {
                let tgt = match r {
                    SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => a.target,
                };
                if tgt == did {
                    referenced = true;
                }
            }
        }
        assert!(
            referenced,
            "importer arc should reference the shared profile's shared_thing (cross-profile)"
        );
    }

    #[test]
    fn check_import_graph_library_with_import() {
        log_test_header(
            "check_import_graph_library_with_import",
            "Compiles the importing root module and validates namespaced references.",
        );
        let start = Instant::now();
        // ugyanaz a root file, mint eddig
        let root_path =
            Path::new("../data/minimal_examples/import_examples/minimal_example_import.hymeko");

        // Parser adapter a LALRPOP-hoz (igazítsd a modulneveket, ha kell)

        let mut ms = ModuleStore::new(StdFsProvider::new(), TestParser);

        let compiled = ms.compile(&root_path).expect("compile failed");

        let ns = ms.it.intern("basic_library");
        let elements = ms.it.intern("elements");
        let operand = ms.it.intern("operand");
        assert!(
            compiled
                .idx
                .by_path
                .contains_key(&PathKey(vec![ns, elements, operand]))
        );

        assert!(
            compiled
                .imports
                .iter()
                .any(|(ns, _)| *ns == ms.it.intern("basic_library")),
            "expected imported namespace basic_library"
        );

        let did = *compiled
            .idx
            .by_path
            .get(&PathKey(vec![ns, elements, operand]))
            .expect("missing basic_library.elements.operand in global index");

        let kind = compiled.ir.decl_nodes[did.0].kind;
        assert!(matches!(
            kind,
            DeclKind::Node | DeclKind::Edge | DeclKind::HyperArc
        ));

        let mut referenced = false;
        for arc in &compiled.ir.arcs {
            for r in &arc.refs {
                let tgt = match r {
                    SignedRefR::Plus(a) | SignedRefR::Minus(a) | SignedRefR::Neutral(a) => a.target,
                };
                if tgt == did {
                    referenced = true;
                    break;
                }
            }
            if referenced {
                break;
            }
        }
        assert!(
            referenced,
            "expected at least one arc ref to target basic_library.elements.operand"
        );
        assert!(
            compiled.ir.decl_hash.get(did.0).and_then(|x| *x).is_some(),
            "expected decl hash for operand to be computed"
        );
        info!(
            "Import compilation pulled {} namespaces and produced {} decls",
            compiled.imports.len(),
            compiled.ir.decl_nodes.len()
        );
        log_test_footer(
            "check_import_graph_library_with_import",
            Some(start.elapsed()),
            "Imported namespace resolved operand references and hashes.",
        );
    }
}
