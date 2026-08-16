#![cfg(test)]
mod mod_test_module_store {
    use crate::minimal_tests::TestParser;
    use crate::minimal_tests::constants::*;
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::pathkey::PathKey;
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
        log_test_footer(
            "module_store_allows_resolving_imported_refs",
            Some(start.elapsed()),
            "Imported references resolved through alias A.",
        );
    }
}
