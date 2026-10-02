use crate::common::ids::SymId;
use crate::ir::canonical_hash::{Canon, CanonicalIdentity};
use crate::ir::ir::Ir;
use crate::ir::lower::lower_program_to_ir;
use crate::module_store::source_provider::SourceProvider;
use crate::resolution::const_resolve::resolve_consts;
use crate::resolution::intern_pass::intern_ast_into;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::{Index, build_index_sym_with_prefix, module_aliases};
use parser::ast::AstStr;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::sym_ast::AstSym;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModuleKey(pub PathBuf);

#[derive(Debug)]
pub struct ModuleEntry {
    pub key: ModuleKey,
    pub ast: AstSym<'static>,
    pub deps: Vec<ModuleKey>,
}

pub struct CompiledProgram {
    pub root: ModuleKey,
    /// The resolution `Index`. **Non-authoritative after U2.2**: the canonical
    /// identity and all post-lowering path consumers read the `Ir`
    /// (`canonical_hash::Canon`, `canonical_hash::build_path_index`).
    /// This is a resolution-time artifact retained for external/test consumers;
    /// its removal is planned for a breaking release once they migrate to the
    /// derived path index. It is NOT re-validated against the `Ir` after the
    /// one-time lowering-boundary check in `compile`.
    pub idx: Index,
    pub ir: Ir,
    // extra debug/teszt kényelmesség:
    pub imports: Vec<(SymId, ModuleKey)>, // (namespace, module key)
    /// The program's canonical identity, computed once here: `canon.hash()` is its semantic
    /// digest, `canon.structure()` its structural hash.
    pub canon: CanonicalIdentity,
}

#[derive(Debug)]
pub enum ModuleLoadError {
    Io(IoDiag),
    Parse(String),
    Cycle { stack: Vec<PathBuf> },
}

#[derive(Debug)]
pub struct IoDiag {
    pub op: &'static str,
    pub path: PathBuf,
    pub cwd: Option<PathBuf>,
    pub err: std::io::Error,
}

/// A parse-t is kiszervezzük, mert nincs még "main"/egységes entrypoint.
/// Tesztben adsz neki egy implementációt, ami a LALRPOP parsereidet hívja.
pub trait HymekoParser {
    fn parse<'a>(&self, src: &'a str) -> Result<AstStr<'a>, String>;
}

pub struct ModuleStore<P: SourceProvider, R: HymekoParser> {
    pub it: Interner,
    fs: P,
    parser: R,
    entries: HashMap<ModuleKey, ModuleEntry>,
    in_progress: Vec<ModuleKey>,
    last_compiled: Option<Arc<CompiledProgram>>,
}

impl<'a, P: SourceProvider, R: HymekoParser> ModuleStore<P, R> {
    pub fn new(fs: P, parser: R) -> Self {
        Self {
            it: Interner::new(),
            fs,
            parser,
            entries: HashMap::new(),
            in_progress: Vec::new(),
            last_compiled: None,
        }
    }

    pub fn provider_mut(&mut self) -> &mut P {
        &mut self.fs
    }

    pub fn get(&self, k: &ModuleKey) -> Option<&ModuleEntry> {
        self.entries.get(k)
    }

    pub fn load_recursive(&mut self, root: &Path) -> Result<ModuleKey, ModuleLoadError> {
        let cwd = std::env::current_dir().ok();

        let root_can = self.fs.canonicalize(root).map_err(|e| {
            ModuleLoadError::Io(IoDiag {
                op: "canonicalize",
                path: root.to_path_buf(),
                cwd: cwd.clone(),
                err: std::io::Error::new(std::io::ErrorKind::Other, e),
            })
        })?;

        let key = ModuleKey(root_can);
        let result = self.ensure_loaded(&key);
        self.in_progress.clear();
        result?;
        Ok(key)
    }

    pub fn canonicalize_path(&self, p: &Path) -> Result<PathBuf, String> {
        self.fs.canonicalize(p)
    }

    fn ensure_loaded(&mut self, key: &ModuleKey) -> Result<(), ModuleLoadError> {
        if self.entries.contains_key(key) {
            return Ok(());
        }
        if self.in_progress.contains(key) {
            return Err(ModuleLoadError::Cycle {
                stack: self.in_progress.iter().map(|k| k.0.clone()).collect(),
            });
        }

        self.in_progress.push(key.clone());

        // 1) read (provider cache-elhet)
        let src = self
            .fs
            .read(&key.0)
            .map_err(|e| io_diag("read", &key.0, e))?;

        // 2) parse
        let ast_str = self
            .parser
            .parse(src.as_ref())
            .map_err(ModuleLoadError::Parse)?;
        // 3) intern (közös internerbe!)
        let mut ast_sym: AstSym<'static> = intern_ast_into(&ast_str, &mut self.it);
        // 3b) Tier B: resolve top-level `const` decls into numeric
        // literals before downstream passes (resolve / lower) see the
        // AST. Any `Value::Expr` or `Value::Ref(<single-segment-const>)`
        // in value positions is replaced with `Value::Num(eval'd)`.
        resolve_consts(&mut ast_sym, &self.it)
            .map_err(|e| ModuleLoadError::Parse(format!("const: {e}")))?;
        // 4) deps from imports
        let base_dir = key.0.parent().unwrap_or(Path::new("../../hymeko/parser"));
        let mut deps = Vec::new();

        for imp in &ast_sym.imports {
            let resolved = resolve_import_path(base_dir, imp.path.as_ref())
                .map_err(|e| io_diag("canonicalize", Path::new(imp.path.as_ref()), e))?;
            let canon = self
                .fs
                .canonicalize(&resolved)
                .map_err(|e| io_diag("canonicalize", &resolved, e))?;
            deps.push(ModuleKey(canon));
        }

        // 5) recursively load deps
        for d in &deps {
            self.ensure_loaded(d)?;
        }

        // 6) commit
        let entry = ModuleEntry {
            key: key.clone(),
            ast: ast_sym,
            deps,
        };
        self.entries.insert(key.clone(), entry);

        self.in_progress.pop();
        Ok(())
    }

    pub fn compile(&mut self, root_path: &Path) -> Result<Arc<CompiledProgram>, ModuleLoadError> {
        // 1) load modules
        let root = self.load_recursive(root_path)?;

        // 2) cache (ultra-minimál): ha ugyanaz a root, add vissza
        if let Some(c) = &self.last_compiled {
            if c.root == root {
                return Ok(c.clone());
            }
        }

        let imports_map = self.linked_imports(&root);

        // Borrow cached syntax independently of the mutable interner: the entries and the
        // interner are disjoint fields, so every module's AST is read in place. Compilation
        // only reads them, and a dependency is not copied (a 424k-hyperedge graph module's
        // AST was 364 MiB of the compile's peak; module-store-borrowed-asts, 2026-09-29).
        let root_entry = self
            .entries
            .get(&root)
            .ok_or_else(|| ModuleLoadError::Parse("root missing after load".into()))?;
        let root_ast = &root_entry.ast;

        // 4) imported list (alias opcionális: ns = alias || dep_ast.name)
        let mut imported: Vec<(SymId, &AstSym<'static>)> = Vec::with_capacity(imports_map.len());
        for &(ns, ref dep_key) in &imports_map {
            let dep_ast = &self
                .entries
                .get(dep_key)
                .ok_or_else(|| {
                    ModuleLoadError::Parse(format!("dep missing: {}", dep_key.0.display()))
                })?
                .ast;

            imported.push((ns, dep_ast));
        }

        // 5) global index: root + deps namespace alatt
        let mut idx = Index::default();
        let mut next: usize = 0;

        build_index_sym_with_prefix(root_ast, &[], &self.it, &mut idx, &mut next)
            .map_err(|e| ModuleLoadError::Parse(format!("index root failed: {e:?}")))?;

        let mut ranges = Vec::with_capacity(imported.len() + 1);
        ranges.push(0..next);

        for (ns, dep_ast) in imported.iter() {
            let start = next;
            build_index_sym_with_prefix(dep_ast, &[*ns], &self.it, &mut idx, &mut next)
                .map_err(|e| ModuleLoadError::Parse(format!("index dep failed: {e:?}")))?;
            ranges.push(start..next);
        }

        // Each source owns its import bindings and using statements. Keep environments
        // on named declaration ranges; lowering anonymous arcs resolves in their owner edge.
        let modules: Vec<_> = std::iter::once((&root, Vec::new()))
            .chain(imports_map.iter().map(|(ns, key)| (key, vec![*ns])))
            .collect();
        let linked_names: HashMap<_, _> = imports_map.iter().map(|(ns, key)| (key, *ns)).collect();
        for ((key, prefix), range) in modules.iter().zip(&ranges) {
            let entry = &self.entries[*key];
            let bindings: Vec<_> = entry
                .ast
                .imports
                .iter()
                .zip(&entry.deps)
                .map(|(imp, dep)| {
                    let local = imp.alias.unwrap_or(self.entries[dep].ast.name);
                    let linked = linked_names[dep];
                    (local, linked)
                })
                .collect();
            let aliases = module_aliases(&idx, &entry.ast.usings, prefix, &bindings, &self.it)
                .map_err(|e| ModuleLoadError::Parse(format!("using alias failed: {e:?}")))?;
            idx.module_aliases.push((range.clone(), aliases));
        }
        idx.aliases = root_ast
            .usings
            .iter()
            .map(|u| (u.alias, idx.module_aliases[0].1[&u.alias].clone()))
            .collect();

        // 7) Lower the canonical program IR.
        let mut ir = lower_program_to_ir(root_ast, &imported, &idx, &mut self.it)
            .map_err(|e| ModuleLoadError::Parse(format!("lower failed: {e:?}")))?;

        ir.files = modules
            .iter()
            .map(|(key, _)| key.0.to_string_lossy().replace('\\', "/"))
            .collect();
        ir.decl_file = vec![u32::MAX; ir.decl_nodes.len()];
        for (file, range) in ranges.iter().enumerate() {
            ir.decl_file[range.clone()].fill(file as u32);
        }
        // Anonymous declarations are appended after named declarations. Their edge
        // parents have already received ownership, including arcs inside nested edges.
        for id in next..ir.decl_nodes.len() {
            let parent = ir.decl_nodes[id].parent;
            ir.decl_file[id] = ir.decl_file[parent.raw()];
        }

        let cfg = crate::ir::canonical_hash::CanonHashCfg {
            schema_version: 1,
            algo_version: 1,
            flags: 0,
        };
        // U2.2 lowering boundary: prove the Ir (namespace_roots + intrusive links)
        // reproduces the resolution Index hierarchy, ONCE, before the Index is
        // discarded. After this the Ir is the sole post-lowering authority and the
        // canonical identity reads no Index. One canonical view serves the check and both
        // identities, so the paths are derived once.
        let canon = {
            let canon = Canon::new(&ir, &self.it);
            canon.validate(&idx).map_err(|e| {
                ModuleLoadError::Parse(format!("namespace boundary validation failed: {e}"))
            })?;
            CanonicalIdentity::of(&canon, cfg)
        };

        // 9) bundle + cache
        let compiled = Arc::new(CompiledProgram {
            root: root.clone(),
            idx,
            ir,
            imports: imports_map,
            canon,
        });

        self.last_compiled = Some(compiled.clone());
        Ok(compiled)
    }

    fn linked_imports(&mut self, root: &ModuleKey) -> Vec<(SymId, ModuleKey)> {
        // Breadth first preserves direct-import order. Each canonical file is linked
        // once, so shared and diamond imports cannot multiply its declarations.
        let mut modules = vec![root.clone()];
        let mut seen = std::collections::HashSet::from([root.clone()]);
        let mut namespaces = std::collections::HashSet::new();
        let mut linked = Vec::new();
        let mut cursor = 0;
        while cursor < modules.len() {
            let entry = &self.entries[&modules[cursor]];
            for (import, key) in entry.ast.imports.iter().zip(&entry.deps) {
                if !seen.insert(key.clone()) {
                    continue;
                }
                let mut ns = import.alias.unwrap_or(self.entries[key].ast.name);
                if !namespaces.insert(ns) {
                    let mut suffix = linked.len();
                    loop {
                        ns = self.it.intern(&format!("__hymeko_module_{suffix}"));
                        if namespaces.insert(ns) {
                            break;
                        }
                        suffix += 1;
                    }
                }
                linked.push((ns, key.clone()));
                modules.push(key.clone());
            }
            cursor += 1;
        }
        linked
    }

    /// Consumes the store (or extracts the specific module) to return the owned Ir.
    /// This avoids requiring #[derive(Clone)] on the massive Ir tree.
    pub fn take_last_ir(mut self) -> Result<Ir, String> {
        // Take the Arc out of the Option
        let arc_prog = self
            .last_compiled
            .take()
            .ok_or("No program was compiled.")?;

        // Attempt to unwrap the Arc to get ownership of the CompiledProgram
        match Arc::try_unwrap(arc_prog) {
            Ok(prog) => Ok(prog.ir),
            Err(_) => Err("Cannot extract Ir: Arc has multiple strong references.".to_string()),
        }
    }
}

fn resolve_import_path(base_dir: &Path, import_path: &str) -> Result<PathBuf, String> {
    let p = Path::new(import_path);
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        // Validate before PathBuf::join: Windows verbatim paths can collapse parent
        // components during joining, hiding an attempt to move above the root.
        crate::module_store::source_provider::normalize_components(
            base_dir.components().chain(p.components()),
        )
        .map_err(|e| format!("{e}: {import_path}"))?;
        Ok(base_dir.join(p))
    }
}

fn io_diag(op: &'static str, path: &Path, e: String) -> ModuleLoadError {
    ModuleLoadError::Io(IoDiag {
        op,
        path: path.to_path_buf(),
        cwd: std::env::current_dir().ok(),
        err: std::io::Error::new(std::io::ErrorKind::Other, e),
    })
}
