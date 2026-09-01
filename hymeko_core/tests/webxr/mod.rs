//! Strict semantic validator for the WebXR vocabulary + demo scene.
//!
//! PoW-0, semantic only: no JS/WASM/WebXR-session/renderer. The canonical truth is the compiled
//! `hymeko_core::ir::Ir`. A green CLI compile proves parse+resolve, NOT meaning (HyMeKo has no
//! schema validation — a misspelled attribute compiles), so this reads the `Ir` back and enforces
//! the vocabulary's invariants. `validate_scene` is a reusable function: the demo must pass it, and
//! a deliberately-malformed in-memory scene must fail it (proving the validator is strict, not
//! vacuous).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hymeko::common::ids::DeclId;
use hymeko::ir::ir::{DeclKind, Ir, ValueR};
use hymeko::module_store::module_store::{CompiledProgram, ModuleStore};
use hymeko::module_store::source_provider::{MemProvider, StdFsProvider};
use hymeko::resolution::interner::Interner;
use hymeko::util::real_parser::RealParser;

// ---------------------------------------------------------------------------------------------
// Ir read-back helpers (public API only)
// ---------------------------------------------------------------------------------------------

fn name_of<'a>(ir: &Ir, it: &'a Interner, d: DeclId) -> &'a str {
    ir.decl_node(d)
        .map(|n| it.resolve(n.name))
        .unwrap_or("<unknown>")
}

fn children(ir: &Ir, parent: DeclId) -> Vec<DeclId> {
    ir.decl_children(parent).collect()
}

/// All root decls (parent == NONE).
fn roots(ir: &Ir) -> Vec<DeclId> {
    (0..ir.decl_nodes.len())
        .map(DeclId::new)
        .filter(|&d| ir.decl_node(d).map(|n| n.parent.is_none()).unwrap_or(false))
        .collect()
}

/// Walk a name path from the roots (e.g. `["xr", "elements"]` for the imported vocabulary root).
fn find_path(ir: &Ir, it: &Interner, path: &[&str]) -> Option<DeclId> {
    let (&first, rest) = path.split_first()?;
    let mut cur = roots(ir)
        .into_iter()
        .find(|&d| name_of(ir, it, d) == first)?;
    for seg in rest {
        cur = children(ir, cur)
            .into_iter()
            .find(|&c| name_of(ir, it, c) == *seg)?;
    }
    Some(cur)
}

/// The inheritance bases of a decl (node or edge), as target DeclIds.
fn base_targets(ir: &Ir, d: DeclId) -> Vec<DeclId> {
    if let Some(nid) = ir.as_node(d) {
        return ir.nodes[nid.raw()]
            .bases
            .iter()
            .map(|b| b.target())
            .collect();
    }
    if let Some(eid) = ir.as_edge(d) {
        return ir.edges[eid.raw()]
            .bases
            .iter()
            .map(|b| b.target())
            .collect();
    }
    Vec::new()
}

/// A decl's direct string attribute `attr`, if present with a `Str` value.
fn str_attr<'a>(ir: &Ir, it: &'a Interner, parent: DeclId, attr: &str) -> Option<&'a str> {
    children(ir, parent).into_iter().find_map(|c| {
        if name_of(ir, it, c) != attr {
            return None;
        }
        match ir.decl_node(c)?.anno.value.as_ref()? {
            ValueR::Str(sym) => Some(it.resolve(*sym)),
            _ => None,
        }
    })
}

/// Every decl in the subtree rooted at `root` (inclusive), preorder.
fn subtree(ir: &Ir, root: DeclId) -> Vec<DeclId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        out.push(d);
        stack.extend(children(ir, d));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// the validator
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Violation {
    /// A scene element inherits a base that is not a known vocabulary kind.
    UnknownKind { element: String, base: String },
    /// A scene element under `scene` inherits nothing from the vocabulary (a bare node).
    NotAnInstance { element: String },
    /// A required attribute is missing or not a string.
    MissingAttribute {
        element: String,
        kind: String,
        attr: String,
    },
    /// An attribute value is outside its allowed set.
    BadAttributeValue {
        element: String,
        attr: String,
        value: String,
        allowed: String,
    },
    /// An `anchor` is not spatially contained within a `reference_space`.
    AnchorNotInReferenceSpace { element: String },
    /// The vocabulary's own two-layer <isa> structure is broken.
    VocabularyLayer { kind: String, reason: String },
}

/// Validate a compiled WebXR scene against the imported webxr vocabulary. Returns every violation
/// found (empty == well-formed). Reads only the canonical `Ir`.
pub fn validate_scene(ir: &Ir, it: &Interner) -> Vec<Violation> {
    let mut v = Vec::new();

    // 1. locate the vocabulary kinds: <vocab>.xr.elements.* and .xr.meta.*
    let elements = match find_path(ir, it, &["xr", "elements"]) {
        Some(e) => e,
        None => {
            v.push(Violation::VocabularyLayer {
                kind: "xr.elements".into(),
                reason: "vocabulary not imported / not found".into(),
            });
            return v;
        }
    };
    let meta = find_path(ir, it, &["xr", "meta"]);

    // name -> DeclId for element kinds and meta kinds
    let kinds: BTreeMap<String, DeclId> = children(ir, elements)
        .into_iter()
        .map(|d| (name_of(ir, it, d).to_string(), d))
        .collect();
    let meta_kinds: BTreeMap<String, DeclId> = meta
        .map(|m| {
            children(ir, m)
                .into_iter()
                .map(|d| (name_of(ir, it, d).to_string(), d))
                .collect()
        })
        .unwrap_or_default();
    let kind_ids: std::collections::BTreeSet<DeclId> = kinds.values().copied().collect();

    // 2. the vocabulary's own two-layer structure: every element kind <isa> a meta kind;
    //    every meta kind (except the root) <isa> cognitive_entity.
    let meta_ids: std::collections::BTreeSet<DeclId> = meta_kinds.values().copied().collect();
    for (kname, &kid) in &kinds {
        let bases = base_targets(ir, kid);
        if !bases.iter().any(|b| meta_ids.contains(b)) {
            v.push(Violation::VocabularyLayer {
                kind: kname.clone(),
                reason: "element kind does not <isa> a meta kind".into(),
            });
        }
    }
    if let Some(&root) = meta_kinds.get("cognitive_entity") {
        for (mname, &mid) in &meta_kinds {
            if mname == "cognitive_entity" {
                continue;
            }
            if !base_targets(ir, mid).contains(&root) {
                v.push(Violation::VocabularyLayer {
                    kind: mname.clone(),
                    reason: "meta kind does not <isa> cognitive_entity".into(),
                });
            }
        }
    }

    // 3. the scene: every element instance inherits a known kind; required attrs; structural.
    let scene = match find_path(ir, it, &["scene"]) {
        Some(s) => s,
        None => return v, // no scene root: vocabulary-only file, nothing more to check
    };

    // map instance DeclId -> its kind name (first base that is a known kind)
    let mut kind_of: BTreeMap<DeclId, String> = BTreeMap::new();
    for d in subtree(ir, scene) {
        if d == scene {
            continue;
        }
        if ir.decl_kind(d) != DeclKind::Node {
            continue;
        }
        let bases = base_targets(ir, d);
        if bases.is_empty() {
            // No inheritance. A *valued* leaf (type "x", color "…") is a legitimate attribute;
            // a bare node with neither a base nor a value instantiates nothing and is flagged.
            let has_value = ir
                .decl_node(d)
                .map(|n| n.anno.value.is_some())
                .unwrap_or(false);
            if !has_value {
                v.push(Violation::NotAnInstance {
                    element: name_of(ir, it, d).to_string(),
                });
            }
            continue;
        }
        // every base must be a known kind (no dangling / foreign inheritance)
        let mut matched = None;
        for b in &bases {
            if kind_ids.contains(b) {
                matched.get_or_insert_with(|| name_of(ir, it, *b).to_string());
            } else {
                v.push(Violation::UnknownKind {
                    element: name_of(ir, it, d).to_string(),
                    base: name_of(ir, it, *b).to_string(),
                });
            }
        }
        if let Some(k) = matched {
            kind_of.insert(d, k);
        } else {
            v.push(Violation::NotAnInstance {
                element: name_of(ir, it, d).to_string(),
            });
        }
    }

    // 4. required attributes per kind + value constraints
    for (&d, k) in &kind_of {
        let ename = name_of(ir, it, d).to_string();
        let need_str = |v: &mut Vec<Violation>, attr: &str| {
            if str_attr(ir, it, d, attr).is_none() {
                v.push(Violation::MissingAttribute {
                    element: ename.clone(),
                    kind: k.clone(),
                    attr: attr.into(),
                });
            }
        };
        match k.as_str() {
            "reference_space" => need_str(&mut v, "type"),
            "geometry" => need_str(&mut v, "primitive"),
            "light" => need_str(&mut v, "type"),
            "input_source" => match str_attr(ir, it, d, "handedness") {
                None => v.push(Violation::MissingAttribute {
                    element: ename.clone(),
                    kind: k.clone(),
                    attr: "handedness".into(),
                }),
                Some(h) if !matches!(h, "left" | "right" | "none") => {
                    v.push(Violation::BadAttributeValue {
                        element: ename.clone(),
                        attr: "handedness".into(),
                        value: h.to_string(),
                        allowed: "left|right|none".into(),
                    })
                }
                Some(_) => {}
            },
            _ => {}
        }
    }

    // 5. structural: each anchor is contained (via parent chain) within a reference_space instance
    for (&d, k) in &kind_of {
        if k != "anchor" {
            continue;
        }
        let mut p = ir.decl_node(d).map(|n| n.parent).unwrap_or(DeclId::NONE);
        let mut ok = false;
        while p.is_some() {
            if kind_of
                .get(&p)
                .map(|s| s == "reference_space")
                .unwrap_or(false)
            {
                ok = true;
                break;
            }
            p = ir.decl_node(p).map(|n| n.parent).unwrap_or(DeclId::NONE);
        }
        if !ok {
            v.push(Violation::AnchorNotInReferenceSpace {
                element: name_of(ir, it, d).to_string(),
            });
        }
    }

    v
}

// ---------------------------------------------------------------------------------------------
// compile harnesses
// ---------------------------------------------------------------------------------------------

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("data")
        .join("webxr")
}

/// A compiled program plus its interner, both kept alive so the `Ir` can be read back.
struct Compiled<P: hymeko::module_store::source_provider::SourceProvider> {
    store: ModuleStore<P, RealParser>,
    prog: Arc<CompiledProgram>,
}

impl<P: hymeko::module_store::source_provider::SourceProvider> Compiled<P> {
    fn ir(&self) -> &Ir {
        &self.prog.ir
    }
    fn it(&self) -> &Interner {
        &self.store.it
    }
}

fn compile_demo() -> Compiled<StdFsProvider> {
    let path = data_dir().join("webxr_scene_demo.hymeko");
    let mut store = ModuleStore::new(StdFsProvider::new(), RealParser);
    let prog = store
        .compile(&path)
        .unwrap_or_else(|e| panic!("demo must compile: {e:?}"));
    Compiled { store, prog }
}

/// Compile an in-memory scene that imports the real (on-disk) vocabulary.
fn compile_scene_src(scene_src: &str) -> Compiled<MemProvider> {
    let vocab = std::fs::read_to_string(data_dir().join("webxr_vocabulary.hymeko"))
        .expect("read vocabulary");
    let provider = MemProvider::default()
        .with_file("/webxr_vocabulary.hymeko", vocab)
        .with_file("/scene.hymeko", scene_src.to_string());
    let mut store = ModuleStore::new(provider, RealParser);
    let prog = store
        .compile(Path::new("/scene.hymeko"))
        .unwrap_or_else(|e| panic!("scene must compile: {e:?}"));
    Compiled { store, prog }
}

// ---------------------------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------------------------

#[test]
fn demo_scene_is_valid() {
    let c = compile_demo();
    let violations = validate_scene(c.ir(), c.it());
    assert!(
        violations.is_empty(),
        "demo must be valid, got: {violations:#?}"
    );
}

#[test]
fn vocabulary_two_layer_isa_structure_holds() {
    // The valid demo exercises the vocabulary; if any element kind failed to <isa> a meta kind,
    // or a meta kind failed to <isa> cognitive_entity, validate_scene would report it.
    let c = compile_demo();
    let layer: Vec<_> = validate_scene(c.ir(), c.it())
        .into_iter()
        .filter(|v| matches!(v, Violation::VocabularyLayer { .. }))
        .collect();
    assert!(
        layer.is_empty(),
        "two-layer <isa> structure broken: {layer:#?}"
    );
}

#[test]
fn unknown_kind_is_rejected() {
    // 'blob' inherits nothing from the vocabulary.
    let src = r#"
bad_scene {
    @"webxr_vocabulary.hymeko";
    using webxr_vocabulary.xr.elements as e;
}
scene {
    stage: e.reference_space { type "local-floor";
        blob {}
    }
}
"#;
    let c = compile_scene_src(src);
    let v = validate_scene(c.ir(), c.it());
    assert!(
        v.iter()
            .any(|x| matches!(x, Violation::NotAnInstance { element } if element == "blob")),
        "a bare node must be flagged, got: {v:#?}"
    );
}

#[test]
fn bad_handedness_is_rejected() {
    let src = r#"
bad_scene {
    @"webxr_vocabulary.hymeko";
    using webxr_vocabulary.xr.elements as e;
}
scene {
    stage: e.reference_space { type "local-floor";
        hand: e.input_source { handedness "sideways"; }
    }
}
"#;
    let c = compile_scene_src(src);
    let v = validate_scene(c.ir(), c.it());
    assert!(
        v.iter().any(
            |x| matches!(x, Violation::BadAttributeValue { attr, value, .. }
            if attr == "handedness" && value == "sideways")
        ),
        "an out-of-set handedness must be flagged, got: {v:#?}"
    );
}

#[test]
fn missing_required_attribute_is_rejected() {
    // a reference_space with no `type`
    let src = r#"
bad_scene {
    @"webxr_vocabulary.hymeko";
    using webxr_vocabulary.xr.elements as e;
}
scene {
    stage: e.reference_space {}
}
"#;
    let c = compile_scene_src(src);
    let v = validate_scene(c.ir(), c.it());
    assert!(
        v.iter().any(
            |x| matches!(x, Violation::MissingAttribute { kind, attr, .. }
            if kind == "reference_space" && attr == "type")
        ),
        "a reference_space without `type` must be flagged, got: {v:#?}"
    );
}

#[test]
fn anchor_outside_reference_space_is_rejected() {
    // an anchor directly under scene, not inside a reference_space
    let src = r#"
bad_scene {
    @"webxr_vocabulary.hymeko";
    using webxr_vocabulary.xr.elements as e;
}
scene {
    stray: e.anchor {}
}
"#;
    let c = compile_scene_src(src);
    let v = validate_scene(c.ir(), c.it());
    assert!(
        v.iter().any(
            |x| matches!(x, Violation::AnchorNotInReferenceSpace { element } if element == "stray")
        ),
        "an anchor outside a reference_space must be flagged, got: {v:#?}"
    );
}
