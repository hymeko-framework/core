//! WebXR semantic validation over the canonical `Ir`.
//!
//! This is the production validator the WebXR authoring path relies on. HyMeKo has no schema
//! validation — a green compile proves parse+resolve, not meaning — so this reads the canonical
//! [`crate::ir::ir::Ir`] back and enforces the WebXR spatial contract. It is **semantic only**: it
//! never materialises a scene DTO or a second scene graph, holds no runtime XR handle, and produces
//! no rendering state.
//!
//! # Contract
//! - Works over `&Ir + &Interner`, through the [`crate::ir::access`] accessor boundary (never raw
//!   `ir.nodes`/`ir.edges`/`ir.arcs` table indexing).
//! - Bounded: `O(declarations + incidences)`; every hierarchy walk is cycle-safe (a visited set or a
//!   decl-count budget), because the `Ir` does not *guarantee* an acyclic containment/relation graph.
//! - Errors are ids-only ([`DeclId`]/[`HyperArcId`]/[`SymId`]), one variant per validation family,
//!   deterministic; no panic/`unwrap` on the validation path.
//!
//! The canonical / runtime boundary is a documented contract: authoring pose, placement, affordance,
//! capability *intent* and asset references live in the `Ir`; `XRSession`/`XRFrame`/`XRSpace`/
//! `XRAnchor`, GL/DOM resources, controller button indices and per-frame poses do not. This module
//! enforces the "no runtime-handle field" half of that boundary (see [`SemanticError::RuntimeHandleField`]).

use std::collections::{HashMap, HashSet};

use crate::common::ids::{DeclId, HyperArcId, SymId};
use crate::ir::ir::{DeclKind, Ir, SignedRefR, ValueR};
use crate::resolution::interner::Interner;

/// The tolerance below which a quaternion's norm is treated as un-normalizable (zero quaternion).
const QUAT_NORM_EPSILON: f64 = 1.0e-6;

/// A WebXR semantic-contract violation. Ids-only, deterministic, one variant per family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticError {
    /// The webxr / meta_spatial vocabulary was not imported (kinds not resolvable).
    VocabularyMissing,
    /// The scene selects zero or more than one session mode.
    SessionModeCardinality { scene: DeclId, found: u32 },
    /// The scene selects zero or more than one primary reference space.
    ReferenceSpaceCardinality { scene: DeclId, found: u32 },
    /// A capability is requested both required and optional.
    CapabilityRequiredOptionalOverlap { capability: DeclId },
    /// A capability request names a decl that is not a known capability.
    UnknownCapability { request: DeclId, target: DeclId },
    /// A capability request's `requirement` attribute is missing or not `required`/`optional`.
    BadCapabilityRequirement { request: DeclId },
    /// A placement's frame participant is missing or is not a spatial frame.
    FrameMissingOrNotAFrame { placement: DeclId, frame: DeclId },
    /// The `@parent_frame` relation contains a cycle reached at `frame`.
    FrameCycle { frame: DeclId },
    /// A pose attribute has the wrong number of components.
    PoseBadLength {
        relation: DeclId,
        attr: SymId,
        expected: u32,
        found: u32,
    },
    /// A pose attribute component is non-finite (NaN/Inf) or not a number.
    PoseNonFinite {
        relation: DeclId,
        attr: SymId,
        index: u32,
    },
    /// A quaternion is zero / not normalizable within tolerance.
    QuaternionNotNormalizable { relation: DeclId },
    /// A scale component is not strictly positive.
    ScaleNonPositive { relation: DeclId, index: u32 },
    /// A relation's participant roles are the wrong cardinality/kind.
    RelationRole {
        relation: DeclId,
        archetype: SymId,
        reason: RoleReason,
    },
    /// An interaction names a decl that is not a known action.
    UnknownAction { interaction: DeclId, target: DeclId },
    /// A visual asset's `uri` attribute is missing or not a string.
    AssetUriNotString { asset: DeclId },
    /// A relation incidence targets a declaration that does not exist.
    DanglingReference { relation: DeclId, target: DeclId },
    /// A declaration carries an attribute whose name is reserved for a runtime WebXR handle.
    RuntimeHandleField { decl: DeclId, field: SymId },
}

/// Why a relation's roles are invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleReason {
    /// Expected exactly one subject (`+`) of the required kind; found a different count.
    SubjectCount { found: u32 },
    /// Expected exactly one object (`-`) of the required kind; found a different count.
    ObjectCount { found: u32 },
    /// A participant is not of the kind its role requires.
    WrongKind { participant: DeclId },
    /// Expected at least one target object; found none.
    NoTarget,
}

impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ids-only rendering; no attacker-controlled source text is embedded.
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SemanticError {}

/// Validate a compiled scene against the WebXR spatial contract. Returns every violation found
/// (empty == valid). Reads only `ir` and `it`, through the accessor boundary; bounded and cycle-safe.
#[must_use]
pub fn validate(ir: &Ir, it: &Interner) -> Vec<SemanticError> {
    let mut out = Vec::new();
    let reg = match Registry::resolve(ir, it) {
        Some(r) => r,
        None => {
            out.push(SemanticError::VocabularyMissing);
            return out;
        }
    };
    let scenes = reg.instances_of(ir, reg.scene);
    check_session(ir, &reg, &scenes, &mut out);
    check_capabilities(ir, it, &reg, &mut out);
    check_frames_and_placements(ir, it, &reg, &mut out);
    check_interactions_and_anchoring(ir, it, &reg, &mut out);
    check_assets(ir, it, &reg, &mut out);
    check_runtime_handle_fields(ir, it, &reg, &mut out);
    out
}

// ---------------------------------------------------------------------------------------------
// vocabulary registry (resolved once, via accessors)
// ---------------------------------------------------------------------------------------------

struct Registry {
    // meta_spatial kinds
    scene: DeclId,
    spatial_frame: DeclId,
    spatial_entity: DeclId,
    visual_asset: DeclId,
    semantic_anchor: DeclId,
    action: DeclId,
    input_role: DeclId,
    // relation archetypes
    placement: DeclId,
    parent_frame: DeclId,
    anchoring: DeclId,
    interaction: DeclId,
    appearance_binding: DeclId,
    effect: DeclId,
    // meta_webxr container children (kind sets)
    session_modes: Vec<DeclId>,
    reference_spaces: Vec<DeclId>,
    capabilities: Vec<DeclId>,
    actions: Vec<DeclId>,
    capability_request: DeclId,
    session_config: DeclId,
    // interned attribute symbols
    sym_position: Option<SymId>,
    sym_orientation: Option<SymId>,
    sym_scale: Option<SymId>,
    sym_requirement: Option<SymId>,
    sym_uri: Option<SymId>,
    runtime_handle_fields: Vec<SymId>,
}

impl Registry {
    fn resolve(ir: &Ir, it: &Interner) -> Option<Self> {
        let spatial = find_root(ir, it, "spatial")?;
        let kinds = child_named(ir, it, spatial, "kinds")?;
        let relations = child_named(ir, it, spatial, "relations")?;
        let webxr = find_root(ir, it, "webxr")?;
        let k = |n: &str| child_named(ir, it, kinds, n);
        let r = |n: &str| child_named(ir, it, relations, n);
        let container = |n: &str| child_named(ir, it, webxr, n);

        let session = child_named(ir, it, webxr, "session")?;
        let reg = Registry {
            scene: k("scene")?,
            spatial_frame: k("spatial_frame")?,
            spatial_entity: k("spatial_entity")?,
            visual_asset: k("visual_asset")?,
            semantic_anchor: k("semantic_anchor")?,
            action: k("action")?,
            input_role: k("input_role")?,
            placement: r("placement")?,
            parent_frame: r("parent_frame")?,
            anchoring: r("anchoring")?,
            interaction: r("interaction")?,
            appearance_binding: r("appearance_binding")?,
            effect: r("effect")?,
            session_modes: children(ir, container("session_modes")?),
            reference_spaces: children(ir, container("reference_spaces")?),
            capabilities: children(ir, container("capabilities")?),
            actions: children(ir, container("actions")?),
            capability_request: child_named(ir, it, session, "capability_request")?,
            session_config: child_named(ir, it, session, "session_config")?,
            sym_position: it.get_id("position"),
            sym_orientation: it.get_id("orientation"),
            sym_scale: it.get_id("scale"),
            sym_requirement: it.get_id("requirement"),
            sym_uri: it.get_id("uri"),
            runtime_handle_fields: RUNTIME_HANDLE_NAMES
                .iter()
                .filter_map(|n| it.get_id(n))
                .collect(),
        };
        Some(reg)
    }

    /// Every decl in the Ir that transitively `<isa>` `kind` (an instance of that kind).
    fn instances_of(&self, ir: &Ir, kind: DeclId) -> Vec<DeclId> {
        ir.decls()
            .filter(|(d, _)| *d != kind && isa_transitive(ir, *d, kind))
            .map(|(d, _)| d)
            .collect()
    }

    /// The relation archetype a given edge instance specialises, if any (by direct/transitive base).
    fn archetype_of(&self, ir: &Ir, edge: DeclId) -> Option<(DeclId, SymId)> {
        for (arch, name) in self.relation_archetypes(ir) {
            if isa_transitive(ir, edge, arch) {
                return Some((arch, name));
            }
        }
        None
    }

    fn relation_archetypes(&self, ir: &Ir) -> [(DeclId, SymId); 6] {
        let nm = |d: DeclId| ir.decl_node(d).map(|n| n.name).unwrap_or(SymId::NONE);
        [
            (self.placement, nm(self.placement)),
            (self.parent_frame, nm(self.parent_frame)),
            (self.anchoring, nm(self.anchoring)),
            (self.interaction, nm(self.interaction)),
            (self.appearance_binding, nm(self.appearance_binding)),
            (self.effect, nm(self.effect)),
        ]
    }

    fn is_kind_in(&self, ir: &Ir, d: DeclId, set: &[DeclId]) -> bool {
        set.iter().any(|&k| isa_transitive(ir, d, k))
    }
}

/// Attribute names reserved for runtime WebXR handles — a source scene must never carry them.
const RUNTIME_HANDLE_NAMES: &[&str] = &[
    "xrsession",
    "xrframe",
    "xrspace",
    "xranchor",
    "session_object",
    "gl_resource",
    "webgl_texture",
    "dom_node",
    "button_index",
    "viewer_pose",
    "permission_state",
    "tracking_confidence",
];

// ---------------------------------------------------------------------------------------------
// accessor helpers (never index ir.nodes/ir.edges/ir.arcs directly)
// ---------------------------------------------------------------------------------------------

fn find_root(ir: &Ir, it: &Interner, name: &str) -> Option<DeclId> {
    let sym = it.get_id(name)?;
    ir.decls()
        .find(|(d, node)| node.parent.is_none() && node.name == sym)
        .map(|(d, _)| d)
}

fn child_named(ir: &Ir, it: &Interner, parent: DeclId, name: &str) -> Option<DeclId> {
    let sym = it.get_id(name)?;
    ir.child_decls(parent)
        .find(|&c| ir.decl_node(c).map(|n| n.name == sym).unwrap_or(false))
}

fn children(ir: &Ir, parent: DeclId) -> Vec<DeclId> {
    ir.child_decls(parent).collect()
}

/// The inheritance bases of a decl (node or edge), via the accessor boundary.
fn base_targets(ir: &Ir, d: DeclId) -> Vec<DeclId> {
    if let Some((_, node)) = ir.node_for_decl(d) {
        return node.bases.iter().map(|b| b.target()).collect();
    }
    if let Some((_, edge)) = ir.edge_for_decl(d) {
        return edge.bases.iter().map(|b| b.target()).collect();
    }
    Vec::new()
}

/// Cycle-safe transitive `<isa>`: does `d` reach `target` through inheritance bases?
fn isa_transitive(ir: &Ir, d: DeclId, target: DeclId) -> bool {
    let mut seen: HashSet<DeclId> = HashSet::new();
    let mut stack = vec![d];
    while let Some(cur) = stack.pop() {
        if cur == target {
            return true;
        }
        if !seen.insert(cur) {
            continue; // visited-set makes an inheritance cycle terminate
        }
        stack.extend(base_targets(ir, cur));
    }
    false
}

/// The signed incidence participants `(sign, target)` of a relation edge instance (all arcs).
fn edge_participants(ir: &Ir, edge: DeclId) -> Vec<(i8, DeclId)> {
    let mut out = Vec::new();
    if let Some((_, rec)) = ir.edge_for_decl(edge) {
        for &aid in &rec.arcs {
            if let Some(arc) = ir.arc(aid) {
                for sref in &arc.refs {
                    out.push((sref.sign(), sref.target()));
                }
            }
        }
    }
    out
}

/// A decl's direct child attribute `attr` value, if present.
fn attr_value(ir: &Ir, d: DeclId, attr: SymId) -> Option<&ValueR> {
    ir.child_decls(d)
        .find(|&c| ir.decl_node(c).map(|n| n.name == attr).unwrap_or(false))
        .and_then(|c| ir.decl_node(c).and_then(|n| n.anno.value.as_ref()))
}

/// A numeric list attribute as `f64`s (with a `None` for any non-`Num` element, preserving index).
fn num_list(v: &ValueR) -> Option<Vec<Option<f64>>> {
    match v {
        ValueR::List(items) => Some(
            items
                .iter()
                .map(|e| match e {
                    ValueR::Num(n) => Some(*n),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// checks
// ---------------------------------------------------------------------------------------------

fn check_session(ir: &Ir, reg: &Registry, scenes: &[DeclId], out: &mut Vec<SemanticError>) {
    for &scene in scenes {
        let descendants = subtree(ir, scene);
        // The scene selects its (one) primary mode + (one) primary reference space through a single
        // @session_config relation; a fallback mode is the `~` participant and is not counted.
        let configs: Vec<DeclId> = descendants
            .iter()
            .copied()
            .filter(|&d| d != reg.session_config && isa_transitive(ir, d, reg.session_config))
            .collect();
        if configs.len() != 1 {
            out.push(SemanticError::SessionModeCardinality {
                scene,
                found: configs.len() as u32,
            });
            continue;
        }
        let parts = edge_participants(ir, configs[0]);
        let primary_modes: Vec<DeclId> = parts
            .iter()
            .filter(|(s, _)| *s > 0)
            .map(|(_, t)| *t)
            .collect();
        let primary_spaces: Vec<DeclId> = parts
            .iter()
            .filter(|(s, _)| *s < 0)
            .map(|(_, t)| *t)
            .collect();
        if primary_modes.len() != 1
            || !primary_modes
                .iter()
                .all(|&t| reg.is_kind_in(ir, t, &reg.session_modes))
        {
            out.push(SemanticError::SessionModeCardinality {
                scene,
                found: primary_modes.len() as u32,
            });
        }
        if primary_spaces.len() != 1
            || !primary_spaces
                .iter()
                .all(|&t| reg.is_kind_in(ir, t, &reg.reference_spaces))
        {
            out.push(SemanticError::ReferenceSpaceCardinality {
                scene,
                found: primary_spaces.len() as u32,
            });
        }
    }
}

fn check_capabilities(ir: &Ir, it: &Interner, reg: &Registry, out: &mut Vec<SemanticError>) {
    let mut required: HashSet<DeclId> = HashSet::new();
    let mut optional: HashSet<DeclId> = HashSet::new();
    for req in reg.instances_of(ir, reg.capability_request) {
        // capability = the `-` object; requirement = the attribute
        let parts = edge_participants(ir, req);
        for (sign, target) in &parts {
            if *sign < 0 {
                if !reg.is_kind_in(ir, *target, &reg.capabilities) {
                    out.push(SemanticError::UnknownCapability {
                        request: req,
                        target: *target,
                    });
                } else {
                    match requirement_of(ir, it, reg, req) {
                        Some(true) => {
                            required.insert(*target);
                        }
                        Some(false) => {
                            optional.insert(*target);
                        }
                        None => out.push(SemanticError::BadCapabilityRequirement { request: req }),
                    }
                }
            }
        }
    }
    for cap in required.intersection(&optional) {
        out.push(SemanticError::CapabilityRequiredOptionalOverlap { capability: *cap });
    }
}

/// `Some(true)` = required, `Some(false)` = optional, `None` = missing/invalid.
fn requirement_of(ir: &Ir, it: &Interner, reg: &Registry, req: DeclId) -> Option<bool> {
    let sym = reg.sym_requirement?;
    match attr_value(ir, req, sym)? {
        ValueR::Str(s) => match it.resolve(*s) {
            "required" => Some(true),
            "optional" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn check_frames_and_placements(
    ir: &Ir,
    it: &Interner,
    reg: &Registry,
    out: &mut Vec<SemanticError>,
) {
    // parent-frame relation, for the acyclicity check
    let mut parent: HashMap<DeclId, DeclId> = HashMap::new();
    for pf in reg.instances_of(ir, reg.parent_frame) {
        let parts = edge_participants(ir, pf);
        let child = parts.iter().find(|(s, _)| *s > 0).map(|(_, t)| *t);
        let par = parts.iter().find(|(s, _)| *s < 0).map(|(_, t)| *t);
        if let (Some(c), Some(p)) = (child, par) {
            parent.insert(c, p);
        }
    }
    for &start in parent.keys() {
        if frame_chain_has_cycle(&parent, start) {
            out.push(SemanticError::FrameCycle { frame: start });
        }
    }

    // placements: role cardinality, frame existence, pose validity
    for pl in reg.instances_of(ir, reg.placement) {
        let parts = edge_participants(ir, pl);
        check_dangling(ir, pl, &parts, out);
        let subjects: Vec<DeclId> = parts
            .iter()
            .filter(|(s, _)| *s > 0)
            .map(|(_, t)| *t)
            .collect();
        let objects: Vec<DeclId> = parts
            .iter()
            .filter(|(s, _)| *s < 0)
            .map(|(_, t)| *t)
            .collect();
        let arch = ir
            .decl_node(reg.placement)
            .map(|n| n.name)
            .unwrap_or(SymId::NONE);
        if subjects.len() != 1
            || !subjects
                .iter()
                .all(|&d| isa_transitive(ir, d, reg.spatial_entity))
        {
            out.push(SemanticError::RelationRole {
                relation: pl,
                archetype: arch,
                reason: RoleReason::SubjectCount {
                    found: subjects.len() as u32,
                },
            });
        }
        if objects.len() != 1 {
            out.push(SemanticError::RelationRole {
                relation: pl,
                archetype: arch,
                reason: RoleReason::ObjectCount {
                    found: objects.len() as u32,
                },
            });
        }
        for &frame in &objects {
            if !isa_transitive(ir, frame, reg.spatial_frame) {
                out.push(SemanticError::FrameMissingOrNotAFrame {
                    placement: pl,
                    frame,
                });
            }
        }
        check_pose(ir, reg, pl, out);
    }
}

fn frame_chain_has_cycle(parent: &HashMap<DeclId, DeclId>, start: DeclId) -> bool {
    let mut seen: HashSet<DeclId> = HashSet::new();
    let mut cur = start;
    while let Some(&p) = parent.get(&cur) {
        if !seen.insert(cur) {
            return true;
        }
        cur = p;
        if cur == start {
            return true;
        }
    }
    false
}

fn check_pose(ir: &Ir, reg: &Registry, relation: DeclId, out: &mut Vec<SemanticError>) {
    if let Some(sym) = reg.sym_position {
        check_vec(ir, relation, sym, 3, PosKind::Finite, out);
    }
    if let Some(sym) = reg.sym_orientation {
        check_vec(ir, relation, sym, 4, PosKind::Quaternion, out);
    }
    if let Some(sym) = reg.sym_scale {
        check_vec(ir, relation, sym, 3, PosKind::Positive, out);
    }
}

enum PosKind {
    Finite,
    Positive,
    Quaternion,
}

fn check_vec(
    ir: &Ir,
    relation: DeclId,
    attr: SymId,
    n: u32,
    kind: PosKind,
    out: &mut Vec<SemanticError>,
) {
    let value = match attr_value(ir, relation, attr) {
        Some(v) => v,
        None => return, // absent pose component is allowed (identity default); presence is validated
    };
    let list = match num_list(value) {
        Some(l) => l,
        None => {
            out.push(SemanticError::PoseBadLength {
                relation,
                attr,
                expected: n,
                found: 0,
            });
            return;
        }
    };
    if list.len() as u32 != n {
        out.push(SemanticError::PoseBadLength {
            relation,
            attr,
            expected: n,
            found: list.len() as u32,
        });
        return;
    }
    let mut norm2 = 0.0f64;
    for (i, comp) in list.iter().enumerate() {
        match comp {
            None => out.push(SemanticError::PoseNonFinite {
                relation,
                attr,
                index: i as u32,
            }),
            Some(x) if !x.is_finite() => out.push(SemanticError::PoseNonFinite {
                relation,
                attr,
                index: i as u32,
            }),
            Some(x) => {
                norm2 += x * x;
                if matches!(kind, PosKind::Positive) && *x <= 0.0 {
                    out.push(SemanticError::ScaleNonPositive {
                        relation,
                        index: i as u32,
                    });
                }
            }
        }
    }
    if matches!(kind, PosKind::Quaternion) && norm2 < QUAT_NORM_EPSILON {
        out.push(SemanticError::QuaternionNotNormalizable { relation });
    }
}

fn check_interactions_and_anchoring(
    ir: &Ir,
    it: &Interner,
    reg: &Registry,
    out: &mut Vec<SemanticError>,
) {
    let arch_i = ir
        .decl_node(reg.interaction)
        .map(|n| n.name)
        .unwrap_or(SymId::NONE);
    for ix in reg.instances_of(ir, reg.interaction) {
        let parts = edge_participants(ir, ix);
        check_dangling(ir, ix, &parts, out);
        // + input_role, ~ action, - target(s)
        let inputs = parts.iter().filter(|(s, _)| *s > 0).count() as u32;
        let actions: Vec<DeclId> = parts
            .iter()
            .filter(|(s, _)| *s == 0)
            .map(|(_, t)| *t)
            .collect();
        let targets = parts.iter().filter(|(s, _)| *s < 0).count() as u32;
        if inputs != 1 {
            out.push(SemanticError::RelationRole {
                relation: ix,
                archetype: arch_i,
                reason: RoleReason::SubjectCount { found: inputs },
            });
        }
        if targets == 0 {
            out.push(SemanticError::RelationRole {
                relation: ix,
                archetype: arch_i,
                reason: RoleReason::NoTarget,
            });
        }
        for &act in &actions {
            if !reg.is_kind_in(ir, act, &reg.actions) && !isa_transitive(ir, act, reg.action) {
                out.push(SemanticError::UnknownAction {
                    interaction: ix,
                    target: act,
                });
            }
        }
    }

    let arch_a = ir
        .decl_node(reg.anchoring)
        .map(|n| n.name)
        .unwrap_or(SymId::NONE);
    for an in reg.instances_of(ir, reg.anchoring) {
        let parts = edge_participants(ir, an);
        check_dangling(ir, an, &parts, out);
        let entity_ok = parts
            .iter()
            .filter(|(s, _)| *s > 0)
            .all(|(_, t)| isa_transitive(ir, *t, reg.spatial_entity));
        let frame_ok = parts
            .iter()
            .filter(|(s, _)| *s < 0)
            .all(|(_, t)| isa_transitive(ir, *t, reg.spatial_frame));
        let anchor_ok = parts
            .iter()
            .filter(|(s, _)| *s == 0)
            .any(|(_, t)| isa_transitive(ir, *t, reg.semantic_anchor));
        if !entity_ok {
            if let Some((_, t)) = parts
                .iter()
                .find(|(s, t)| *s > 0 && !isa_transitive(ir, *t, reg.spatial_entity))
            {
                out.push(SemanticError::RelationRole {
                    relation: an,
                    archetype: arch_a,
                    reason: RoleReason::WrongKind { participant: *t },
                });
            }
        }
        if !frame_ok || !anchor_ok {
            out.push(SemanticError::RelationRole {
                relation: an,
                archetype: arch_a,
                reason: RoleReason::NoTarget,
            });
        }
    }
}

fn check_assets(ir: &Ir, it: &Interner, reg: &Registry, out: &mut Vec<SemanticError>) {
    let uri = match reg.sym_uri {
        Some(s) => s,
        None => return,
    };
    for asset in reg.instances_of(ir, reg.visual_asset) {
        match attr_value(ir, asset, uri) {
            Some(ValueR::Str(_)) => {}
            _ => out.push(SemanticError::AssetUriNotString { asset }),
        }
    }
}

fn check_runtime_handle_fields(
    ir: &Ir,
    it: &Interner,
    reg: &Registry,
    out: &mut Vec<SemanticError>,
) {
    if reg.runtime_handle_fields.is_empty() {
        return;
    }
    for (d, node) in ir.decls() {
        if reg.runtime_handle_fields.contains(&node.name) {
            let parent = node.parent;
            out.push(SemanticError::RuntimeHandleField {
                decl: parent,
                field: node.name,
            });
        }
    }
}

fn check_dangling(ir: &Ir, relation: DeclId, parts: &[(i8, DeclId)], out: &mut Vec<SemanticError>) {
    for (_, target) in parts {
        if ir.decl_node(*target).is_none() {
            out.push(SemanticError::DanglingReference {
                relation,
                target: *target,
            });
        }
    }
}

/// Every decl in the containment subtree of `root` (inclusive), cycle-safe (visited set).
fn subtree(ir: &Ir, root: DeclId) -> Vec<DeclId> {
    let mut out = Vec::new();
    let mut seen: HashSet<DeclId> = HashSet::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        if !seen.insert(d) {
            continue;
        }
        out.push(d);
        stack.extend(ir.child_decls(d));
    }
    out
}
