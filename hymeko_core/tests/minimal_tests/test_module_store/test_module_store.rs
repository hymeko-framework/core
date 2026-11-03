#![cfg(test)]
mod mod_test_module_store {
    use crate::minimal_tests::TestParser;
    use crate::minimal_tests::constants::*;
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::canonical_hash::{
        Canon, CanonHashCfg, CanonicalIdentity, NamespaceError, validate_namespace_roots,
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
    fn compiler_and_validator_share_module_aliases_in_headers_and_values() {
        let provider = MemProvider::default()
            .with_file(
                "root.hymeko",
                r#"description {
                @"dep.hymeko" -> dep;
                using dep.vocab as el;
                const K = 3.0;
                header el.base;
            }
            root: el.base { n (K * 2.0); @edge el.base { (+root [el.base]); } }
            "#,
            )
            .with_file(
                "dep.hymeko",
                "dependency { const D = 4.0; header (D / 2.0); } vocab { base {} }",
            );
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("root.hymeko")).unwrap();
        for (prefix, key) in std::iter::once((Vec::new(), &compiled.root))
            .chain(compiled.imports.iter().map(|(ns, key)| (vec![*ns], key)))
        {
            let ast = store.get(key).unwrap().ast.clone();
            validate_all_refs_sym_with_prefix(&ast, &prefix, &compiled.idx, &mut store.it).unwrap();
        }
        let header = compiled
            .ir
            .decls()
            .find(|(_, node)| store.it.resolve(node.name) == "header")
            .unwrap()
            .1;
        let Some(hymeko::ir::ir::ValueR::Ref(target)) = compiled.ir[header.anno].value else {
            panic!("header reference")
        };
        assert_eq!(
            store
                .it
                .resolve(compiled.ir.decl_node(target).unwrap().name),
            "base"
        );
        let n = compiled
            .ir
            .decls()
            .find(|(_, node)| store.it.resolve(node.name) == "n")
            .unwrap()
            .1;
        assert_eq!(
            compiled.ir[n.anno].value,
            Some(hymeko::ir::ir::ValueR::Num(6.0))
        );
    }

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
            let compiled = store
                .compile(Path::new(entry))
                .unwrap_or_else(|e| panic!("{entry}: {e:?}"));
            let arcs: Vec<_> = compiled
                .ir
                .arcs
                .iter()
                .filter(|arc| {
                    store
                        .it
                        .resolve(compiled.ir.decl_nodes[arc.in_edge.raw()].name)
                        == "topic_production"
                })
                .collect();
            assert_eq!(
                arcs.len(),
                1,
                "{entry}: course must be lowered exactly once"
            );
            assert_eq!(arcs[0].refs.len(), 1);
            let target = arcs[0].refs[0].target();
            assert_eq!(
                store.it.resolve(compiled.ir.decl_nodes[target.raw()].name),
                "production_concept"
            );
        }
    }

    #[test]
    fn course_parent_imports_use_canonical_provider_keys() {
        for import in ["../concepts/metrics.hymeko", "../concepts/./metrics.hymeko"] {
            let provider = MemProvider::default()
                .with_file("lectures/l01.hymeko", format!(
                    "lecture_description {{ @\"{import}\"; using metrics as met; }} lecture {{ @topic {{ (+ met.measure); }} }}"))
                .with_file("concepts/metrics.hymeko", "metrics_description {} metrics { measure {} }");
            let mut store =
                ModuleStore::new(provider.clone(), hymeko::util::real_parser::RealParser);
            let compiled = store.compile(Path::new("lectures/l01.hymeko")).unwrap();
            let target = compiled.ir.arcs[0].refs[0].target();
            assert_eq!(
                store.it.resolve(compiled.ir.decl_nodes[target.raw()].name),
                "measure"
            );
            assert_eq!(provider.read_count("concepts/metrics.hymeko"), 1);
        }
    }

    #[test]
    fn course_virtual_paths_cannot_escape_root() {
        use hymeko::module_store::source_provider::SourceProvider;
        let provider = MemProvider::default();
        for path in ["../secret.hymeko", "lectures/../../secret.hymeko"] {
            assert!(
                provider.canonicalize(Path::new(path)).is_err(),
                "accepted {path}"
            );
        }
    }

    #[test]
    fn course_provider_normalization_covers_reads_versions_and_filesystem_paths() {
        use hymeko::module_store::source_provider::{SourceProvider, StdFsProvider};
        let mut memory = MemProvider::default()
            .with_file("concepts/./subject.hymeko", "description{} subject{}");
        let first = Path::new("concepts/subject.hymeko");
        let equivalent = Path::new("lectures/../concepts/subject.hymeko");
        assert_eq!(
            memory.read(first).unwrap(),
            memory.read(equivalent).unwrap()
        );
        assert_eq!(
            memory.version(first).unwrap(),
            memory.version(equivalent).unwrap()
        );
        assert_eq!(memory.read_count(equivalent), 2);

        let fs = StdFsProvider::new();
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/minimal_examples");
        assert_eq!(
            fs.canonicalize(&base.join("minimal_example.hymeko"))
                .unwrap(),
            fs.canonicalize(&base.join("../minimal_examples/./minimal_example.hymeko"))
                .unwrap()
        );
        let absolute = std::env::current_dir().unwrap();
        let root: std::path::PathBuf = absolute
            .components()
            .take_while(|part| !matches!(part, std::path::Component::Normal(_)))
            .collect();
        assert!(
            fs.canonicalize(&root.join("../not_read.hymeko"))
                .unwrap_err()
                .contains("escapes provider root")
        );
    }

    #[test]
    fn memory_provider_spells_a_path_one_way_on_every_target() {
        use hymeko::module_store::source_provider::SourceProvider;
        let memory = MemProvider::default()
            .with_file("concepts\\subject.hymeko", "description{} subject{}")
            .with_file("/models/./room.hymeko", "description{} room{}");
        let canonical = |p: &str| memory.canonicalize(Path::new(p)).unwrap();
        assert_eq!(
            canonical("lectures\\..\\concepts/subject.hymeko").to_str(),
            Some("concepts/subject.hymeko")
        );
        assert_eq!(
            canonical("/models/room.hymeko").to_str(),
            Some("/models/room.hymeko")
        );
        assert_eq!(
            memory.paths(),
            ["/models/room.hymeko", "concepts/subject.hymeko"]
        );
        assert!(
            memory
                .canonicalize(Path::new("lectures\\..\\..\\secret.hymeko"))
                .is_err()
        );

        // A relative import across directories reaches the key inserted with backslashes.
        let provider = MemProvider::default()
            .with_file(
                "dynamics\\model.hymeko",
                "model_description { @\"../optimization/vocab.hymeko\"; } model {}",
            )
            .with_file("optimization/vocab.hymeko", "vocab_description {} vocab {}");
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let compiled = store.compile(Path::new("dynamics/model.hymeko")).unwrap();
        for file in ["dynamics/model.hymeko", "optimization/vocab.hymeko"] {
            assert!(compiled.ir.files.iter().any(|f| f == file), "{file}");
        }
    }

    #[test]
    fn memory_provider_names_the_provided_paths_when_a_source_is_missing() {
        use hymeko::module_store::source_provider::SourceProvider;
        let mut memory = MemProvider::default()
            .with_file("b/x.hymeko", "description{} x{}")
            .with_file("a.hymeko", "description{} a{}");
        let error = memory.read(Path::new("c.hymeko")).unwrap_err();
        assert!(error.contains("file not found: c.hymeko"), "{error}");
        assert!(error.contains("provided: a.hymeko, b/x.hymeko"), "{error}");
        let error = MemProvider::default()
            .read(Path::new("c.hymeko"))
            .unwrap_err();
        assert!(error.contains("no sources were provided"), "{error}");
    }

    #[test]
    fn memory_provider_versions_follow_the_content() {
        use hymeko::module_store::source_provider::SourceProvider;
        let path = Path::new("a.hymeko");
        let mut memory = MemProvider::default().with_file(path, "description{} a{}");
        let first = memory.version(path).unwrap();
        memory.insert_file(path, "description{} a{}");
        assert_eq!(memory.version(path).unwrap(), first);
        memory.insert_file(path, "description{} b{}");
        assert_ne!(memory.version(path).unwrap(), first);
    }

    #[test]
    fn course_failed_load_can_be_retried_without_a_spurious_cycle() {
        let provider = MemProvider::default().with_file(
            "lecture.hymeko",
            "lecture_description { @\"concepts.hymeko\"; } lecture {}",
        );
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let error = store.compile(Path::new("lecture.hymeko")).err().unwrap();
        assert!(format!("{error:?}").contains("IoDiag"));
        store
            .provider_mut()
            .insert_file("concepts.hymeko", "concepts_description{} concepts{}");
        assert!(store.compile(Path::new("lecture.hymeko")).is_ok());
    }

    #[test]
    fn course_source_metadata_rejects_corruption_and_marks_synthetic_declarations() {
        use hymeko::ir::coherence::{CoherenceLimits, Finding, validate_hierarchy_index_coherence};
        let mut store = ModuleStore::new(
            MemProvider::default().with_file("a.hymeko", "description{} a{}"),
            hymeko::util::real_parser::RealParser,
        );
        let compiled = store.compile(Path::new("a.hymeko")).unwrap();
        let mut ir = compiled.ir.clone();
        ir.decl_file[0] = 99;
        assert!(serde_json::to_value(&ir).is_err());
        let report = validate_hierarchy_index_coherence(
            &ir,
            &compiled.idx,
            &store.it,
            CoherenceLimits::default(),
        )
        .unwrap_err();
        assert!(report.findings.contains(&Finding::InvalidSourceProvenance));
        let mut ir = compiled.ir.clone();
        let appended = ir.push_decl(ir.decl_draft(hymeko::common::ids::DeclId::new(0)).unwrap());
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
        let files = wire["files"]
            .as_array()
            .expect("IR must expose source files");
        let decl_file = wire["decl_file"]
            .as_array()
            .expect("IR must expose declaration provenance");
        assert_eq!(files.len(), 2);
        assert_eq!(decl_file.len(), compiled.ir.decl_nodes.len());
        for (id, decl) in compiled.ir.decl_nodes.iter().enumerate() {
            let file = &files[decl_file[id].as_u64().unwrap() as usize];
            let name = store.it.resolve(decl.name);
            let expected = if ["concepts", "subject"].contains(&name) {
                "concepts.hymeko"
            } else {
                "lecture.hymeko"
            };
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
        assert_eq!(
            store.it.resolve(compiled.ir.decl_nodes[subject.raw()].name),
            "subject"
        );
        assert_eq!(
            store.it.resolve(compiled.ir.decl_nodes[topic.raw()].name),
            "topic"
        );
        assert_ne!(compiled.ir.as_node(subject).unwrap().raw(), subject.raw());
        assert!(compiled.ir.as_edge(topic).is_some());
        assert!(compiled.ir.as_node(topic).is_none());
    }

    #[test]
    fn compiled_ir_roundtrips_without_hash_caches() {
        for (name, root, dep) in [
            (
                "mixed_values",
                "Description{} Example { node0{} @edge { (+node0 [node0, \"payload\", [node0, -0.0]], -node0 [], ~node0); } }",
                MODULE_STORE_SIMPLE_DEP_SRC,
            ),
            (
                "imported",
                MODULE_STORE_RESOLVE_ROOT_SRC,
                MODULE_STORE_RESOLVE_DEP_SRC,
            ),
            (
                "empty_import",
                MODULE_STORE_SIMPLE_ROOT_SRC,
                MODULE_STORE_SIMPLE_DEP_SRC,
            ),
        ] {
            let provider = MemProvider::default()
                .with_file(MODULE_STORE_ROOT_FILE, root)
                .with_file(MODULE_STORE_DEP_FILE, dep);
            let mut store = ModuleStore::new(provider, TestParser);
            let compiled = store.compile(Path::new(MODULE_STORE_ROOT_FILE)).unwrap();
            assert_eq!(
                compiled.canon,
                CanonicalIdentity::of(
                    &Canon::new(&compiled.ir, &store.it),
                    CanonHashCfg::default()
                ),
                "the compile's identity is its IR's canonical identity: {name}"
            );
            assert_eq!(
                validate_namespace_roots(&compiled.ir, &compiled.idx),
                Ok(())
            );
            let mut ir = compiled.ir.clone();
            ir.meta.as_mut().unwrap().created_at_unix_ns = 0;
            let encoded = serde_json::to_value(&ir).unwrap();
            for removed in ["doc_hash", "decl_hash", "arc_hash"] {
                assert!(encoded.get(removed).is_none(), "obsolete field: {removed}");
            }
            let decoded: hymeko::ir::ir::Ir = serde_json::from_value(encoded).unwrap();
            assert_eq!(
                Canon::new(&ir, &store.it).hash(),
                Canon::new(&decoded, &store.it).hash(),
                "fixture {name}"
            );

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
            let edge = store
                .it
                .resolve(compiled.ir.decl_nodes[arc.in_edge.raw()].name);
            let target = store
                .it
                .resolve(compiled.ir.decl_nodes[arc.refs[0].target().raw()].name);
            assert_eq!(
                target,
                if edge == "topic_a" {
                    "subject_a"
                } else {
                    "subject_b"
                }
            );
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
        assert_eq!(
            compiled.ir.arcs[0].refs[0].target(),
            compiled.ir.arcs[1].refs[0].target()
        );
        assert_eq!(compiled.ir.files.len(), 4);
        assert_eq!(provider.read_count("concepts.hymeko"), 1);
    }

    #[test]
    fn course_invalid_imported_using_is_not_silently_ignored() {
        let provider = MemProvider::default()
            .with_file(
                "index.hymeko",
                "index_description { @\"course.hymeko\"; } index {}",
            )
            .with_file(
                "course.hymeko",
                "course_description { using missing as c; } course {}",
            );
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let error = store
            .compile(Path::new("index.hymeko"))
            .err()
            .expect("invalid using must fail");
        let diagnostic = format!("{error:?}");
        assert!(diagnostic.contains("UnresolvedRef"));
        assert!(diagnostic.contains("<using>"));
        assert!(diagnostic.contains("missing"));
    }

    #[test]
    fn course_root_aliases_do_not_leak_into_imports() {
        let provider = MemProvider::default()
            .with_file(
                "index.hymeko",
                "index_description { @\"course.hymeko\"; using index as c; } index { subject {} }",
            )
            .with_file(
                "course.hymeko",
                "course_description {} course { @topic { (+ c.subject); } }",
            );
        let mut store = ModuleStore::new(provider, hymeko::util::real_parser::RealParser);
        let error = store
            .compile(Path::new("index.hymeko"))
            .err()
            .expect("entry alias leaked into dependency");
        let diagnostic = format!("{error:?}");
        assert!(diagnostic.contains("UnresolvedRef"));
        assert!(diagnostic.contains("course_description.course.topic"));
        assert!(diagnostic.contains("c.subject"));
    }

    /// A compiled program's IR holds no spare capacity: lowering appends the anonymous arcs after
    /// sizing the declaration tables for the named ones, and then shrinks every table in place
    /// (ir-shrink-after-lowering, 2026-09-29), so a consumer that keeps the IR keeps its size.
    #[test]
    fn a_compiled_ir_holds_no_spare_capacity() {
        let mut graph = String::from("g\n{}\ngraph\n{\n");
        for v in 0..40 {
            graph += &format!("    v{v:02} <vertex> {{ vertex_index {v}; }}\n");
        }
        for e in 0..60 {
            let (a, b) = (e % 40, (e * 7 + 3) % 40);
            graph += &format!(
                "    @e{e:02} <hyperedge> {{ slot \"s\"; ( + <in> v{a:02}, - <out> v{b:02} ); }}\n"
            );
        }
        graph += "}\n";
        let mut store = ModuleStore::new(
            MemProvider::default().with_file("g.hymeko", graph),
            hymeko::util::real_parser::RealParser,
        );
        let program = store.compile(Path::new("g.hymeko")).unwrap();
        let ir = &program.ir;
        assert_eq!(ir.arcs.len(), 60);
        for (name, capacity, len) in [
            ("decl_nodes", ir.decl_nodes.capacity(), ir.decl_nodes.len()),
            ("nodes", ir.nodes.capacity(), ir.nodes.len()),
            ("edges", ir.edges.capacity(), ir.edges.len()),
            ("arcs", ir.arcs.capacity(), ir.arcs.len()),
        ] {
            assert_eq!(capacity, len, "{name}");
        }
    }

    /// A compile keeps both identities of what it compiled (canonical-identity, 2026-09-29): the
    /// structural hash is blind to annotation values, the semantic digest is not. Two graphs that
    /// differ only in a vertex index, or only in a hyperedge's slot, share `canon.structure()` and
    /// differ in `canon.hash()`; a changed arc sign moves both.
    #[test]
    fn a_compile_keeps_the_semantic_identity_the_structure_hash_cannot_see() {
        let compile = |text: &str| {
            let mut store = ModuleStore::new(
                MemProvider::default().with_file("g.hymeko", text),
                hymeko::util::real_parser::RealParser,
            );
            store.compile(Path::new("g.hymeko")).unwrap().canon
        };
        let graph = "g\n{}\ngraph\n{\n    v0 <vertex> { vertex_index 0; }\n    \
                     v1 <vertex> { vertex_index 1; }\n    \
                     @e0 <hyperedge> { slot \"s\"; ( + <in> v0, - <out> v1 ); }\n}\n";
        let base = compile(graph);
        for (change, value_only) in [
            (graph.replace("vertex_index 1;", "vertex_index 7;"), true),
            (graph.replace("slot \"s\"", "slot \"t\""), true),
            (graph.replace("( + <in> v0", "( - <in> v0"), false),
        ] {
            let other = compile(&change);
            assert_ne!(other.hash(), base.hash(), "{change}");
            assert_eq!(
                other.structure() == base.structure(),
                value_only,
                "{change}"
            );
        }
        assert_eq!(
            compile(graph),
            base,
            "an identity is a function of the source"
        );
    }

    #[test]
    fn course_source_metadata_is_optional_validated_and_hash_neutral() {
        use hymeko::ir::ir::Ir;
        use hymeko::writers::cbor_writer;
        let mut store = ModuleStore::new(
            MemProvider::default()
                .with_file("plain.hymeko", "description{} model{a \"árvíztűrő 東京\";}"),
            hymeko::util::real_parser::RealParser,
        );
        let compiled = store.compile(Path::new("plain.hymeko")).unwrap();
        assert_eq!(
            compiled
                .canon
                .structure()
                .0
                .0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "f004527060201656811f52eeaa7cbb2562961db352143971fcebc5e646ecc7b8"
        );
        let structural = compiled.canon.structure().0;
        let semantic = compiled.canon.hash();
        let mut ir = compiled.ir.clone();
        ir.files[0] = "moved/lecture.hymeko".into();
        assert_eq!(
            Canon::new(&ir, &store.it)
                .structure(CanonHashCfg::default())
                .0,
            structural
        );
        assert_eq!(Canon::new(&ir, &store.it).hash(), semantic);
        let wire = serde_json::to_value(&ir).unwrap();
        for key in ["files", "decl_file"] {
            let mut invalid = wire.clone();
            invalid.as_object_mut().unwrap().remove(key);
            assert!(
                serde_json::from_value::<Ir>(invalid).is_err(),
                "missing {key}"
            );
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
        log_test_footer(
            "module_store_allows_resolving_imported_refs",
            Some(start.elapsed()),
            "Imported references resolved through alias A.",
        );
    }
}
