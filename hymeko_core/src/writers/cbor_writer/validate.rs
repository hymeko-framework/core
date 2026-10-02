use super::{CBOR_SCHEMA_VERSION, CborError, CborPayload, DecodedCborPayload};
use crate::common::ids::DeclId;
use crate::ir::canonical_hash::{Canon, CanonHashCfg, build_path_index};
use crate::ir::coherence::{CoherenceLimits, validate_hierarchy_index_coherence};
use crate::ir::ir::{AnnoR, SignedRefR, ValueR};
use crate::ir_stream::DecodeLimits;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;
use crate::resolution::symbols::SymbolCompaction;
use std::path::{Component, Path, PathBuf};

pub(super) fn payload(
    mut payload: CborPayload,
    verify_hash: bool,
) -> Result<DecodedCborPayload, CborError> {
    if payload.schema_version != CBOR_SCHEMA_VERSION {
        return Err(CborError::UnsupportedSchema {
            got: payload.schema_version,
            expected: CBOR_SCHEMA_VERSION,
        });
    }
    canonical_position_counts(&payload)?;
    portable_path(&payload.root_path, "root_path")?;
    for (_, module) in &payload.imports {
        portable_path(&module.0, "import path")?;
    }

    let mut interner = Interner::new();
    for (index, value) in payload.interned_strings.iter().enumerate() {
        if interner.intern(value).raw() != index {
            return Err(CborError::DuplicateInternedString { index });
        }
    }
    // Module-local parser environments were never serialized. The shared
    // typed symbol visitor validates every live symbol before dense rewriting.
    payload.index.module_aliases.clear();
    let compacted = SymbolCompaction::apply(
        &mut payload.ir,
        &mut payload.index,
        &mut payload.imports,
        &interner,
    )
    .map_err(|error| CborError::SymbolOutOfRange {
        field: error.field,
        symbol: error.symbol,
    })?;
    payload.interned_strings = compacted
        .interner
        .iter()
        .map(|(_, text)| text.to_owned())
        .collect();
    let interner = compacted.interner;
    validate_declaration_ids(&payload)?;

    // One canonical view serves the namespace check and the hash check.
    let canon = Canon::new(&payload.ir, &interner);
    canon
        .validate(&payload.index)
        .map_err(|_| CborError::NamespaceMismatch)?;
    let max_walk = payload.ir.decl_nodes.len().saturating_add(1);
    validate_hierarchy_index_coherence(
        &payload.ir,
        &payload.index,
        &interner,
        CoherenceLimits {
            max_findings: 64,
            max_walk,
        },
    )
    .map_err(|report| CborError::IncoherentIr {
        findings: report.total,
    })?;

    let derived = build_path_index(&payload.ir).map_err(|_| CborError::IndexMismatch)?;
    let mut rebuilt = Index::new();
    for (path, decl) in derived.iter() {
        rebuilt.by_path.insert(path.clone(), decl);
    }
    if rebuilt.by_path != payload.index.by_path {
        return Err(CborError::IndexMismatch);
    }
    for target in payload.index.aliases.values() {
        if target.is_empty() || !rebuilt.by_path.contains_key(target.as_slice()) {
            return Err(CborError::IndexMismatch);
        }
    }
    rebuilt.aliases = payload.index.aliases.clone();
    payload.index = rebuilt;

    if verify_hash {
        let expected = canon.structure(CanonHashCfg::default()).0;
        if expected != payload.canon_hash {
            return Err(CborError::CanonicalHashMismatch {
                expected,
                got: payload.canon_hash,
            });
        }
    }

    Ok(DecodedCborPayload { payload, interner })
}

pub(super) fn limits(payload: &CborPayload, limits: &DecodeLimits) -> Result<(), CborError> {
    let max = limits.max_elements_per_section;
    for (what, length) in [
        ("declarations", payload.ir.decl_nodes.len()),
        ("nodes", payload.ir.nodes.len()),
        ("edges", payload.ir.edges.len()),
        ("arcs", payload.ir.arcs.len()),
        ("path-index entries", payload.index.by_path.len()),
        ("aliases", payload.index.aliases.len()),
        ("interned strings", payload.interned_strings.len()),
        ("imports", payload.imports.len()),
    ] {
        let got = u64::try_from(length).unwrap_or(u64::MAX);
        if got > max {
            return Err(CborError::LimitExceeded { what, got, max });
        }
    }
    Ok(())
}

fn canonical_position_counts(payload: &CborPayload) -> Result<(), CborError> {
    let position_count_max = u64::from(u16::MAX) + 1;
    for edge in &payload.ir.edges {
        let got = u64::try_from(edge.arcs.len()).unwrap_or(u64::MAX);
        if got > position_count_max {
            return Err(CborError::LimitExceeded {
                what: "arcs per edge in canonical hash v1",
                got,
                max: position_count_max,
            });
        }
    }
    for arc in &payload.ir.arcs {
        let got = u64::try_from(arc.refs.len()).unwrap_or(u64::MAX);
        if got > position_count_max {
            return Err(CborError::LimitExceeded {
                what: "references per arc in canonical hash v1",
                got,
                max: position_count_max,
            });
        }
    }
    Ok(())
}

pub(super) fn portable_path(path: &Path, what: &'static str) -> Result<(), CborError> {
    let Some(raw) = path.to_str() else {
        return Err(CborError::NonPortablePath { what });
    };
    if raw.is_empty()
        || raw.starts_with('/')
        || raw.contains(['\\', '\0', ':'])
        || raw
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(CborError::NonPortablePath { what });
    }
    Ok(())
}

/// Convert a native relative path into the single portable wire spelling.
pub(super) fn normalize_portable_path(
    path: &Path,
    what: &'static str,
) -> Result<PathBuf, CborError> {
    let Some(raw) = path.to_str() else {
        return Err(CborError::NonPortablePath { what });
    };
    #[cfg(windows)]
    let invalid_component = raw
        .split(['/', '\\'])
        .any(|part| part.is_empty() || matches!(part, "." | ".."));
    #[cfg(not(windows))]
    let invalid_component = raw
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."));
    if raw.is_empty() || raw.contains(['\0', ':']) || path.is_absolute() || invalid_component {
        return Err(CborError::NonPortablePath { what });
    }
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            return Err(CborError::NonPortablePath { what });
        };
        let Some(part) = part.to_str() else {
            return Err(CborError::NonPortablePath { what });
        };
        if part.is_empty() || matches!(part, "." | "..") || part.contains(['/', '\\', '\0', ':']) {
            return Err(CborError::NonPortablePath { what });
        }
        parts.push(part);
    }
    let normalized = PathBuf::from(parts.join("/"));
    portable_path(&normalized, what)?;
    Ok(normalized)
}

fn validate_declaration_ids(payload: &CborPayload) -> Result<(), CborError> {
    let ir = &payload.ir;
    let decl_count = ir.decl_nodes.len();

    for decl in &ir.decl_nodes {
        optional_decl(decl.parent, decl_count, "declaration parent")?;
        optional_decl(decl.first_child, decl_count, "declaration first_child")?;
        optional_decl(decl.last_child, decl_count, "declaration last_child")?;
        optional_decl(decl.next_sibling, decl_count, "declaration next_sibling")?;
        anno(&decl.anno, decl_count)?;
    }
    for (root, _) in &ir.namespace_roots {
        required_decl(*root, decl_count, "namespace root")?;
    }
    for node in &ir.nodes {
        required_decl(node.decl, decl_count, "node declaration")?;
        refs(&node.bases, decl_count)?;
    }
    for edge in &ir.edges {
        required_decl(edge.decl, decl_count, "edge declaration")?;
        refs(&edge.bases, decl_count)?;
        for arc in &edge.arcs {
            if arc.is_none() || arc.raw() >= ir.arcs.len() {
                return Err(CborError::IdOutOfRange {
                    field: "edge arc",
                    id: arc.raw(),
                });
            }
        }
    }
    for arc in &ir.arcs {
        required_decl(arc.in_edge, decl_count, "arc owner")?;
        anno(&arc.anno, decl_count)?;
        refs(&arc.refs, decl_count)?;
    }
    for decl in payload.index.by_path.values() {
        required_decl(*decl, decl_count, "path-index declaration")?;
    }
    Ok(())
}

fn refs(refs: &[SignedRefR], decl_count: usize) -> Result<(), CborError> {
    for reference in refs {
        let atom = reference.atom();
        required_decl(atom.target, decl_count, "reference target")?;
        anno(&atom.anno, decl_count)?;
        if let Some(weights) = &atom.weights {
            for weight in weights {
                value(weight, decl_count)?;
            }
        }
    }
    Ok(())
}

fn anno(anno: &AnnoR, decl_count: usize) -> Result<(), CborError> {
    if let Some(value_ref) = &anno.value {
        value(value_ref, decl_count)?;
    }
    Ok(())
}

fn value(root: &ValueR, decl_count: usize) -> Result<(), CborError> {
    let mut pending = vec![root];
    while let Some(value_ref) = pending.pop() {
        match value_ref {
            ValueR::Ref(decl) => required_decl(*decl, decl_count, "value reference")?,
            ValueR::List(values) => pending.extend(values),
            ValueR::Num(_) | ValueR::Str(_) => {}
        }
    }
    Ok(())
}

fn required_decl(id: DeclId, count: usize, field: &'static str) -> Result<(), CborError> {
    if id.is_none() || id.raw() >= count {
        return Err(CborError::IdOutOfRange {
            field,
            id: id.raw(),
        });
    }
    Ok(())
}

fn optional_decl(id: DeclId, count: usize, field: &'static str) -> Result<(), CborError> {
    if id.is_some() && id.raw() >= count {
        return Err(CborError::IdOutOfRange {
            field,
            id: id.raw(),
        });
    }
    Ok(())
}
