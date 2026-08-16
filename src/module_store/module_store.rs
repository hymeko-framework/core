use crate::common::ids::SymId;
use crate::ir::hash::HashId;
use crate::ir::hash_pass::compute_merkle_hashes;
use crate::ir::ir::Ir;
use crate::ir::lower::lower_program_to_ir;
use crate::module_store::source_provider::SourceProvider;
use crate::resolution::const_resolve::resolve_consts;
use crate::resolution::intern_pass::intern_ast_into_owned;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::{Index, apply_usings, build_index_sym_with_prefix};
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
    pub idx: Index,
    pub ir: Ir,
    // extra debug/teszt kényelmesség:
    pub imports: Vec<(SymId, ModuleKey)>, // (namespace, module key)
    pub canon_hash: HashId,
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

pub struct CompiledProgramCache {
    // később ide jöhet fingerprint/hash
    pub idx: Index,
    pub ir: Ir,
    pub imports: Vec<(SymId, ModuleKey)>,
}

pub struct ModuleStore<P: SourceProvider, R: HymekoParser> {
    pub it: Interner,
    fs: P,
    parser: R,
    entries: HashMap<ModuleKey, ModuleEntry>,
    in_progress: Vec<ModuleKey>,
    program_cache: HashMap<ModuleKey, CompiledProgramCache>,
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
            program_cache: HashMap::new(),
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
        self.ensure_loaded(&key)?;
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
        let mut ast_sym: AstSym<'static> = intern_ast_into_owned(&ast_str, &mut self.it);
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
            let resolved = resolve_import_path(base_dir, imp.path.as_ref());
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

        // 3) kivehetjük a root AST-t (clone, hogy ne legyen borrow fight)
        let root_ast = self
            .get(&root)
            .ok_or_else(|| ModuleLoadError::Parse("root missing after load".into()))?
            .ast
            .clone();

        let root_entry = self.get(&root).unwrap();
        let deps = root_entry.deps.clone();

        // 4) imported list (alias opcionális: ns = alias || dep_ast.name)
        let mut imported: Vec<(SymId, AstSym<'static>)> = Vec::new();
        let mut imports_map: Vec<(SymId, ModuleKey)> = Vec::new();

        for (imp, dep_key) in root_ast.imports.iter().zip(deps.iter()) {
            let dep_ast = self
                .get(dep_key)
                .ok_or_else(|| {
                    ModuleLoadError::Parse(format!("dep missing: {}", dep_key.0.display()))
                })?
                .ast
                .clone();

            let ns = imp.alias.unwrap_or(dep_ast.name);
            imports_map.push((ns, dep_key.clone()));
            imported.push((ns, dep_ast));
        }

        // 5) global index: root + deps namespace alatt
        let mut idx = Index::default();
        let mut next: usize = 0;

        build_index_sym_with_prefix(&root_ast, &[], &self.it, &mut idx, &mut next)
            .map_err(|e| ModuleLoadError::Parse(format!("index root failed: {e:?}")))?;

        for (ns, dep_ast) in imported.iter() {
            build_index_sym_with_prefix(dep_ast, &[*ns], &self.it, &mut idx, &mut next)
                .map_err(|e| ModuleLoadError::Parse(format!("index dep failed: {e:?}")))?;
        }

        // 6b) apply using aliases
        let import_ns: Vec<SymId> = imported.iter().map(|(ns, _)| *ns).collect();
        apply_usings(&mut idx, &root_ast.usings, &import_ns, &self.it)
            .map_err(|e| ModuleLoadError::Parse(format!("using alias failed: {e:?}")))?;

        // 6c) cross-profile instance references (APPROVED-CORE-EDIT: xprofile-instance-refs,
        // 2026-06-19). Also apply each imported *profile*'s own `using` aliases into the shared
        // index, so its arcs/bases lower and its instance decls become referenceable from the
        // importer (e.g. `using <desc>.<content> as arr; (+ arr.dist)`). Best-effort and
        // per-statement: an alias that does not resolve against the available import namespaces is
        // skipped (a genuinely-needed one surfaces later as a clear UnresolvedRef on the dep's arc),
        // so this is strictly additive — meta-only imports (no usings) are a no-op, and no existing
        // program's resolution or canonical hash changes.
        for (_ns, dep_ast) in imported.iter() {
            for u in &dep_ast.usings {
                let _ = apply_usings(&mut idx, std::slice::from_ref(u), &import_ns, &self.it);
            }
        }

        // 7) lower program IR (2A) + merkle
        let mut ir = lower_program_to_ir(&root_ast, &imported, &idx, &mut self.it)
            .map_err(|e| ModuleLoadError::Parse(format!("lower failed: {e:?}")))?;

        compute_merkle_hashes(&mut ir, &self.it);

        let cfg = crate::ir::canonical_hash::CanonHashCfg {
            schema_version: 1,
            algo_version: 1,
            flags: 0,
        };
        let canon_hash =
            crate::ir::canonical_hash::canonical_program_hash(cfg, &idx, &ir, &self.it);

        // 9) bundle + cache
        let compiled = Arc::new(CompiledProgram {
            root: root.clone(),
            idx,
            ir,
            imports: imports_map,
            canon_hash,
        });

        self.last_compiled = Some(compiled.clone());
        Ok(compiled)
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

fn resolve_import_path(base_dir: &Path, import_path: &str) -> PathBuf {
    let p = Path::new(import_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
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

fn make_build_id(created_at_unix_ns: i128) -> [u8; 16] {
    let mut h = blake3::Hasher::new();
    h.update(b"HYMEKO_BUILD_ID");
    h.update(&created_at_unix_ns.to_le_bytes());

    // kis entropy: address (nem kriptó, de session ID-nek oké)
    let addr = (&h as *const _ as usize).to_le_bytes();
    h.update(&addr);

    let out = h.finalize();
    let b = out.as_bytes();
    let mut id = [0u8; 16];
    id.copy_from_slice(&b[..16]);
    id
}
