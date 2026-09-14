#![cfg(test)]
mod mod_test_module_store {
    use crate::minimal_tests::TestParser;
    use crate::minimal_tests::constants::*;
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::canonical_hash::{
        CanonHashCfg, NamespaceError, canonical_program_hash, validate_namespace_roots,
    };
    use hymeko::module_store::module_store::ModuleStore;
    use hymeko::module_store::source_provider::MemProvider;
    use hymeko::resolution::resolve::{
        Index, build_index_sym_with_prefix, validate_all_refs_sym_with_prefix,
    };
    use log::info;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn course_aliases_resolve_for_every_entry() {
        for entry in ["course.hymeko", "lecture.hymeko", "index.hymeko"] {
            let provider = MemProvider::default()
                .with_file("concepts/production.hymeko",
                    "production_description {} production { production_concept {} }")
                .with_file("course.hymeko",
                    "course_description { @\"concepts/production.hymeko\"; using production as prod; } course { @topic_production { (+ prod.production_concept); } }")
                .with_file("lecture.hymeko", "lecture_description { @\"course.hymeko\"; } lecture {}")
                .with_file("index.hymeko", "index_description { @\"concepts/production.hymeko\"; @\"course.hymeko\"; } index {}");
            let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
            let compiled = store.compile(Path::new(entry)).unwrap_or_else(|e| panic!("{entry}: {e:?}"));
            let arcs: Vec<_> = compiled.ir.arcs.iter().filter(|arc|
                store.it.resolve(compiled.ir.decl_nodes[arc.in_edge.raw()].name) == "topic_production").collect();
            assert_eq!(arcs.len(), 1, "{entry}: course must be lowered exactly once");
            assert_eq!(arcs[0].refs.len(), 1);
            let target = arcs[0].refs[0].target();
            assert_eq!(store.it.resolve(compiled.ir.decl_nodes[target.raw()].name), "production_concept");
        }
    }

    #[test]
    fn course_parent_imports_use_canonical_provider_keys() {
        for import in ["../concepts/metrics.hymeko", "../concepts/./metrics.hymeko"] {
            let provider = MemProvider::default()
                .with_file("lectures/l01.hymeko", format!(
                    "lecture_description {{ @\"{import}\"; using metrics as met; }} lecture {{ @topic {{ (+ met.measure); }} }}"))
                .with_file("concepts/metrics.hymeko", "metrics_description {} metrics { measure {} }");
            let mut store = ModuleStore::new(provider.clone(), hymeko::util::real_parser::RealParser);
            let compiled = store.compile(Path::new("lectures/l01.hymeko")).unwrap();
            let target = compiled.ir.arcs[0].refs[0].target();
            assert_eq!(store.it.resolve(compiled.ir.decl_nodes[target.raw()].name), "measure");
            assert_eq!(provider.read_count("concepts/metrics.hymeko"), 1);
        }
    }

    #[test]
    fn course_virtual_paths_cannot_escape_root() {
        use hymeko::module_store::source_provider::SourceProvider;
        let provider = MemProvider::default();
        for path in ["../secret.hymeko", "lectures/../../secret.hymeko"] {
            assert!(provider.canonicalize(Path::new(path)).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn course_provider_normalization_covers_reads_versions_and_filesystem_paths() {
        use hymeko::module_store::source_provider::{SourceProvider, StdFsProvider};
        let mut memory = MemProvider::default().with_file("concepts/./subject.hymeko", "description{} subject{}");
        let first = Path::new("concepts/subject.hymeko");
        let equivalent = Path::new("lectures/../concepts/subject.hymeko");
        assert_eq!(memory.read(first).unwrap(), memory.read(equivalent).unwrap());
        assert_eq!(memory.version(first).unwrap(), memory.version(equivalent).unwrap());
        assert_eq!(memory.read_count(equivalent), 2);

        let fs = StdFsProvider::new();
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/minimal_examples");
        assert_eq!(fs.canonicalize(&base.join("minimal_example.hymeko")).unwrap(),
            fs.canonicalize(&base.join("../minimal_examples/./minimal_example.hymeko")).unwrap());
        let absolute = std::env::current_dir().unwrap();
        let root: std::path::PathBuf = absolute.components().take_while(|part|
            !matches!(part, std::path::Component::Normal(_))).collect();
        assert!(fs.canonicalize(&root.join("../not_read.hymeko")).unwrap_err().contains("escapes provider root"));
    }

    #[test]
    fn course_failed_load_can_be_retried_without_a_spurious_cycle() {
        let provider = MemProvider::default().with_file("lecture.hymeko",
            "lecture_description { @\"concepts.hymeko\"; } lecture {}");
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let error = store.compile(Path::new("lecture.hymeko")).err().unwrap();
        assert!(format!("{error:?}").contains("IoDiag"));
        store.provider_mut().insert_file("concepts.hymeko", "concepts_description{} concepts{}");
        assert!(store.compile(Path::new("lecture.hymeko")).is_ok());
    }

    #[test]
    fn course_source_metadata_rejects_corruption_and_marks_synthetic_declarations() {
        use hymeko::ir::coherence::{CoherenceLimits, Finding, validate_hierarchy_index_coherence};
        let mut store = ModuleStore::new(MemProvider::default().with_file("a.hymeko", "description{} a{}"),
            hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("a.hymeko")).unwrap();
        let mut ir = compiled.ir.clone();
        ir.decl_file[0] = 99;
        assert!(serde_json::to_value(&ir).is_err());
        let report = validate_hierarchy_index_coherence(&ir, &compiled.idx, &store.it, CoherenceLimits::default()).unwrap_err();
        assert!(report.findings.contains(&Finding::InvalidSourceProvenance));
        let mut ir = compiled.ir.clone();
        let appended = ir.push_decl(ir.decl_nodes[0].clone());
        assert_eq!(ir.decl_file[appended.raw()], u32::MAX);
        let arc = ir.push_anonymous_arc();
        assert_eq!(ir.decl_file[arc.raw()], u32::MAX);
        assert_eq!(ir.decl_file.len(), ir.decl_nodes.len());
    }

    #[test]
    fn course_provenance_covers_named_declarations_and_anonymous_arcs() {
        let provider = MemProvider::default()
            .with_file("lecture.hymeko", "lecture_description { @\"concepts.hymeko\"; } lecture { @weekly { (+ concepts_description.concepts.subject); } }")
            .with_file("concepts.hymeko", "concepts_description {} concepts { subject {} }");
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("lecture.hymeko")).unwrap();
        let wire = serde_json::to_value(&compiled.ir).unwrap();
        let files = wire["files"].as_array().expect("IR must expose source files");
        let decl_file = wire["decl_file"].as_array().expect("IR must expose declaration provenance");
        assert_eq!(files.len(), 2);
        assert_eq!(decl_file.len(), compiled.ir.decl_nodes.len());
        for (id, decl) in compiled.ir.decl_nodes.iter().enumerate() {
            let file = &files[decl_file[id].as_u64().unwrap() as usize];
            let name = store.it.resolve(decl.name);
            let expected = if ["concepts", "subject"].contains(&name) { "concepts.hymeko" } else { "lecture.hymeko" };
            assert_eq!(file, expected, "declaration {id}: {name}");
        }
        let restored: hymeko::ir::ir::Ir = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), wire);
    }

    #[test]
    fn course_references_keep_decl_ids_and_edge_membership() {
        let provider = MemProvider::default()
            .with_file("lecture.hymeko", "lecture_description { @\"concepts.hymeko\"; } lecture { @weekly { (+ concepts_description.concepts.subject, - concepts_description.concepts.topic); } }")
            .with_file("concepts.hymeko", "concepts_description {} concepts { @topic { (+ subject); } subject {} }");
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("lecture.hymeko")).unwrap();
        let refs = &compiled.ir.arcs[0].refs;
        let subject = refs[0].target();
        let topic = refs[1].target();
        assert_eq!(store.it.resolve(compiled.ir.decl_nodes[subject.raw()].name), "subject");
        assert_eq!(store.it.resolve(compiled.ir.decl_nodes[topic.raw()].name), "topic");
        assert_ne!(compiled.ir.as_node(subject).unwrap().raw(), subject.raw());
        assert!(compiled.ir.as_edge(topic).is_some());
        assert!(compiled.ir.as_node(topic).is_none());
    }

    #[test]
    fn compilation_preserves_normalized_ir_bytes() {
        // Frozen on unmodified Core f305ade before removing the AST clone.
        // The serialized IR includes declaration hashes and namespace metadata.
        for (name, root, dep, expected) in [
            (
                "mixed_values",
                "Description{} Example { node0{} @edge { (+node0 [node0, \"payload\", [node0, -0.0]], -node0 [], ~node0); } }",
                MODULE_STORE_SIMPLE_DEP_SRC,
                "363c33b5902fe43fe50262e7bb5fc3424eb047d5dad575c7644636e1c1953ffe",
            ),
            (
                "imported",
                MODULE_STORE_RESOLVE_ROOT_SRC,
                MODULE_STORE_RESOLVE_DEP_SRC,
                "9ba27ede1623362367c33939e1092af8fd380049ce9d1fdf41b09106fed76fd4",
            ),
            (
                "empty_import",
                MODULE_STORE_SIMPLE_ROOT_SRC,
                MODULE_STORE_SIMPLE_DEP_SRC,
                "81b7cb9a0de3c1dfe844115770e27856bfeb2ff938bb113cf396ba1bafff34e2",
            ),
        ] {
            let provider = MemProvider::default()
                .with_file(MODULE_STORE_ROOT_FILE, root)
                .with_file(MODULE_STORE_DEP_FILE, dep);
            let mut store = ModuleStore::new(provider, TestParser);
            let compiled = store.compile(Path::new(MODULE_STORE_ROOT_FILE)).unwrap();
            assert_eq!(
                compiled.canon_hash,
                canonical_program_hash(CanonHashCfg::default(), &compiled.ir, &store.it),
                "shared and standalone canonical paths: {name}"
            );
            assert_eq!(
                validate_namespace_roots(&compiled.ir, &compiled.idx),
                Ok(())
            );
            let mut ir = compiled.ir.clone();
            ir.meta.as_mut().unwrap().created_at_unix_ns = 0;
            // New optional source metadata has its own round-trip controls above.
            // This pre-optimization KAT continues to freeze every legacy IR field.
            ir.files.clear();
            ir.decl_file.clear();
            let digest = blake3::hash(&serde_json::to_vec(&ir).unwrap());
            println!("NORMALIZED_IR {name} {digest}");
            assert_eq!(digest.to_hex().as_str(), expected, "fixture {name}");

            // Corrupt the first indexed declaration without changing its path length.
            // Validation must still report the same first mismatch by declaration ID.
            if let Some(&did) = compiled.idx.by_path.values().min_by_key(|did| did.raw()) {
                ir.decl_nodes[did.raw()].name = store.it.intern("corrupted_name");
                assert_eq!(
                    validate_namespace_roots(&ir, &compiled.idx),
                    Err(NamespaceError::PathMismatch { decl: did }),
                    "fixture {name}"
                );
            } else {
                assert!(ir.decl_nodes.is_empty(), "fixture {name}");
            }
        }
    }

    #[test]
    fn course_alias_names_are_local_even_with_colliding_module_names() {
        let provider = MemProvider::default()
            .with_file("index.hymeko", "index_description { @\"a.hymeko\"; @\"b.hymeko\"; } index {}")
            .with_file("a.hymeko", "a_description { @\"a/concepts.hymeko\"; using concepts as c; } a { @topic_a { (+ c.subject_a); } }")
            .with_file("b.hymeko", "b_description { @\"b/concepts.hymeko\"; using concepts as c; } b { @topic_b { (+ c.subject_b); } }")
            .with_file("a/concepts.hymeko", "concepts_description {} concepts { subject_a {} }")
            .with_file("b/concepts.hymeko", "concepts_description {} concepts { subject_b {} }");
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("index.hymeko")).unwrap();
        for arc in &compiled.ir.arcs {
            let edge = store.it.resolve(compiled.ir.decl_nodes[arc.in_edge.raw()].name);
            let target = store.it.resolve(compiled.ir.decl_nodes[arc.refs[0].target().raw()].name);
            assert_eq!(target, if edge == "topic_a" { "subject_a" } else { "subject_b" });
        }
        assert_eq!(compiled.ir.arcs.len(), 2);
        assert_eq!(compiled.ir.files.len(), 5);
    }

    #[test]
    fn course_diamond_imports_link_each_file_once() {
        let provider = MemProvider::default()
            .with_file("index.hymeko", "index_description { @\"a.hymeko\"; @\"b.hymeko\"; } index {}")
            .with_file("a.hymeko", "a_description { @\"concepts.hymeko\"; using concepts as c; } a { @left { (+ c.subject); } }")
            .with_file("b.hymeko", "b_description { @\"./concepts.hymeko\"; using concepts as c; } b { @right { (+ c.subject); } }")
            .with_file("concepts.hymeko", "concepts_description {} concepts { subject {} }");
        let mut store = ModuleStore::new(provider.clone(), hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("index.hymeko")).unwrap();
        assert_eq!(compiled.ir.arcs[0].refs[0].target(), compiled.ir.arcs[1].refs[0].target());
        assert_eq!(compiled.ir.files.len(), 4);
        assert_eq!(provider.read_count("concepts.hymeko"), 1);
    }

    #[test]
    fn course_invalid_imported_using_is_not_silently_ignored() {
        let provider = MemProvider::default()
            .with_file("index.hymeko", "index_description { @\"course.hymeko\"; } index {}")
            .with_file("course.hymeko", "course_description { using missing as c; } course {}");
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let error = store.compile(Path::new("index.hymeko")).err().expect("invalid using must fail");
        let diagnostic = format!("{error:?}");
        assert!(diagnostic.contains("UnresolvedRef"));
        assert!(diagnostic.contains("<using>"));
        assert!(diagnostic.contains("missing"));
    }

    #[test]
    fn course_root_aliases_do_not_leak_into_imports() {
        let provider = MemProvider::default()
            .with_file("index.hymeko", "index_description { @\"course.hymeko\"; using index as c; } index { subject {} }")
            .with_file("course.hymeko", "course_description {} course { @topic { (+ c.subject); } }");
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let error = store.compile(Path::new("index.hymeko")).err().expect("entry alias leaked into dependency");
        let diagnostic = format!("{error:?}");
        assert!(diagnostic.contains("UnresolvedRef"));
        assert!(diagnostic.contains("course_description.course.topic"));
        assert!(diagnostic.contains("c.subject"));
    }

    #[test]
    fn course_source_metadata_is_optional_validated_and_hash_neutral() {
        use hymeko::ir::ir::Ir;
        use hymeko::ir::semantic_digest::semantic_model_digest_v2;
        use hymeko::writers::cbor_writer;
        let mut store = ModuleStore::new(MemProvider::default()
            .with_file("plain.hymeko", "description{} model{a \"árvíztűrő 東京\";}"),
            hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("plain.hymeko")).unwrap();
        assert_eq!(compiled.canon_hash.0.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "f004527060201656811f52eeaa7cbb2562961db352143971fcebc5e646ecc7b8");
        let structural = compiled.canon_hash;
        let semantic = semantic_model_digest_v2(&compiled.ir, &store.it);
        let mut ir = compiled.ir.clone();
        ir.files[0] = "moved/lecture.hymeko".into();
        assert_eq!(canonical_program_hash(CanonHashCfg::default(), &ir, &store.it), structural);
        assert_eq!(semantic_model_digest_v2(&ir, &store.it), semantic);
        let wire = serde_json::to_value(&ir).unwrap();
        for key in ["files", "decl_file"] {
            let mut invalid = wire.clone();
            invalid.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<Ir>(invalid).is_err(), "missing {key}");
        }
        for ids in [serde_json::json!([]), serde_json::json!([99, 0])] {
            let mut invalid = wire.clone();
            invalid["decl_file"] = ids;
            assert!(serde_json::from_value::<Ir>(invalid).is_err());
        }
        let mut legacy = wire.clone();
        legacy.as_object_mut().unwrap().remove("files");
        legacy.as_object_mut().unwrap().remove("decl_file");
        let decoded: Ir = serde_json::from_value(legacy).unwrap();
        assert!(decoded.files.is_empty() && decoded.decl_file.is_empty());
        let bytes = cbor_writer::encode_compiled(Path::new(""), &compiled, &store.it).unwrap();
        let decoded = cbor_writer::decode(&bytes, &Default::default()).unwrap();
        assert_eq!(decoded.ir().files, compiled.ir.files);
        assert_eq!(decoded.ir().decl_file, compiled.ir.decl_file);
    }

    #[test]
    fn module_store_loads_root_and_one_import() {
        log_test_header(
            "module_store_loads_root_and_one_import",
            "Ensures ModuleStore caches imported dependencies and avoids duplicate fs reads.",
        );
        let start = Instant::now();
        // root.hmk importálja a dep.hmk-t A alias alatt
        let fs = MemProvider::default()
            .with_file(
                MODULE_STORE_ROOT_FILE,
                Arc::<str>::from(MODULE_STORE_SIMPLE_ROOT_SRC),
            )
            .with_file(
                MODULE_STORE_DEP_FILE,
                Arc::<str>::from(MODULE_STORE_SIMPLE_DEP_SRC),
            );
        let fs_probe = fs.clone();
        // Ha nálad: ModuleStore::new(fs, parser)
        let mut ms = ModuleStore::new(fs, TestParser);

        let root_key = ms
            .load_recursive(Path::new(MODULE_STORE_ROOT_FILE))
            .unwrap();
        let root_entry = ms.get(&root_key).unwrap();

        assert_eq!(root_entry.deps.len(), 1);

        // ellenőrzés: dep is bent van a store-ban
        let dep_key = &root_entry.deps[0];
        assert!(ms.get(dep_key).is_some());

        assert_eq!(fs_probe.read_count(MODULE_STORE_ROOT_FILE), 1);
        assert_eq!(fs_probe.read_count(MODULE_STORE_DEP_FILE), 1);

        // ✅ cache ellenőrzés: újra betöltve nem olvasunk többet
        let _ = ms
            .load_recursive(Path::new(MODULE_STORE_ROOT_FILE))
            .unwrap();
        assert_eq!(fs_probe.read_count(MODULE_STORE_ROOT_FILE), 1);
        assert_eq!(fs_probe.read_count(MODULE_STORE_DEP_FILE), 1);
        info!(
            "ModuleStore cache read counts root={}, dep={}",
            fs_probe.read_count(MODULE_STORE_ROOT_FILE),
            fs_probe.read_count(MODULE_STORE_DEP_FILE)
        );
        log_test_footer(
            "module_store_loads_root_and_one_import",
            Some(start.elapsed()),
            "Verified caching behavior for root.hmk and dep.hmk.",
        );
    }

    #[test]
    fn module_store_allows_resolving_imported_refs() {
        log_test_header(
            "module_store_allows_resolving_imported_refs",
            "Confirms ModuleStore indexes imported modules with the correct alias prefix.",
        );
        let start = Instant::now();
        // root.hmk importálja a dep.hmk-t A alias alatt
        let fs = MemProvider::default()
            .with_file(MODULE_STORE_ROOT_FILE, MODULE_STORE_RESOLVE_ROOT_SRC)
            .with_file(MODULE_STORE_DEP_FILE, MODULE_STORE_RESOLVE_DEP_SRC);

        let mut ms = ModuleStore::new(fs, TestParser);

        // 1) load modules
        let root_key = ms
            .load_recursive(Path::new(MODULE_STORE_ROOT_FILE))
            .unwrap();

        // ✅ klónozzuk ki az AST-t, ne referencia legyen a store-ból
        let root_ast = ms.get(&root_key).unwrap().ast.clone();

        let dep_key = ms.get(&root_key).unwrap().deps[0].clone();
        let dep_ast = ms.get(&dep_key).unwrap().ast.clone();

        // most már oké mut interner
        let a = ms.it.intern(MODULE_STORE_ALIAS);

        let mut idx = Index::default();
        let mut next: usize = 0;

        build_index_sym_with_prefix(&root_ast, &[], &ms.it, &mut idx, &mut next).unwrap();
        build_index_sym_with_prefix(&dep_ast, &[a], &ms.it, &mut idx, &mut next).unwrap();

        validate_all_refs_sym_with_prefix(&root_ast, &[], &idx, &mut ms.it).unwrap();
        validate_all_refs_sym_with_prefix(&dep_ast, &[a], &idx, &mut ms.it).unwrap();

        let dep_sym = ms.it.intern(MODULE_STORE_DEP_NAMESPACE);
        let foo_sym = ms.it.intern(MODULE_STORE_DEP_NODE);
        let foo_path = PathKey(vec![a, dep_sym, foo_sym]);
        assert!(
            idx.by_path.contains_key(&foo_path),
            "Dep.Foo should be indexed under the A prefix"
        );
        info!("Indexed path {:?}", foo_path);
        // Compilation must read the cached ASTs without mutating them. The
        // result remains owned after its source store has been dropped.
        let root_before = format!("{:?}", ms.get(&root_key).unwrap().ast);
        let dep_before = format!("{:?}", ms.get(&dep_key).unwrap().ast);
        let compiled = ms.compile(Path::new(MODULE_STORE_ROOT_FILE)).unwrap();
        let cached = ms.compile(Path::new(MODULE_STORE_ROOT_FILE)).unwrap();
        assert!(Arc::ptr_eq(&compiled, &cached));
        assert_eq!(format!("{:?}", ms.get(&root_key).unwrap().ast), root_before);
        assert_eq!(format!("{:?}", ms.get(&dep_key).unwrap().ast), dep_before);
        drop(ms);
        assert_eq!(compiled.ir.arcs[0].refs[0].target(), idx.by_path[&foo_path]);
        assert!(compiled.ir.decl_hash.iter().all(Option::is_some));
        log_test_footer(
            "module_store_allows_resolving_imported_refs",
            Some(start.elapsed()),
            "Imported references resolved through alias A.",
        );
    }
}
