use super::{CBOR_SCHEMA_VERSION, CborError, CborPayload, DecodedCborPayload};
use crate::common::ids::{DeclId, SymId};
use crate::ir::canonical_hash::{
    CanonHashCfg, build_path_index, canonical_program_hash, validate_namespace_roots,
};
use crate::ir::coherence::{CoherenceLimits, validate_hierarchy_index_coherence};
use crate::ir::ir::{AnnoR, SignedRefR, ValueR};
use crate::ir_stream::DecodeLimits;
use crate::resolution::interner::Interner;
use crate::resolution::resolve::Index;
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
    validate_symbols_and_ids(&payload, payload.interned_strings.len())?;

    validate_namespace_roots(&payload.ir, &payload.index)
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
        let expected = canonical_program_hash(CanonHashCfg::default(), &payload.ir, &interner);
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
        ("declaration hashes", payload.ir.decl_hash.len()),
        ("arc hashes", payload.ir.arc_hash.len()),
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

fn validate_symbols_and_ids(payload: &CborPayload, symbol_count: usize) -> Result<(), CborError> {
    let ir = &payload.ir;
    let decl_count = ir.decl_nodes.len();

    for decl in &ir.decl_nodes {
        symbol(decl.name, symbol_count, "declaration name")?;
        optional_decl(decl.parent, decl_count, "declaration parent")?;
        optional_decl(decl.first_child, decl_count, "declaration first_child")?;
        optional_decl(decl.last_child, decl_count, "declaration last_child")?;
        optional_decl(decl.next_sibling, decl_count, "declaration next_sibling")?;
        anno(&decl.anno, symbol_count, decl_count)?;
    }
    for (root, prefix) in &ir.namespace_roots {
        required_decl(*root, decl_count, "namespace root")?;
        for part in prefix {
            symbol(*part, symbol_count, "namespace prefix symbol")?;
        }
    }
    for node in &ir.nodes {
        required_decl(node.decl, decl_count, "node declaration")?;
        refs(&node.bases, symbol_count, decl_count)?;
    }
    for edge in &ir.edges {
        required_decl(edge.decl, decl_count, "edge declaration")?;
        refs(&edge.bases, symbol_count, decl_count)?;
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
        anno(&arc.anno, symbol_count, decl_count)?;
        refs(&arc.refs, symbol_count, decl_count)?;
    }
    for (path, decl) in &payload.index.by_path {
        required_decl(*decl, decl_count, "path-index declaration")?;
        for part in &path.0 {
            symbol(*part, symbol_count, "path-index symbol")?;
        }
    }
    for (alias, path) in &payload.index.aliases {
        symbol(*alias, symbol_count, "alias")?;
        for part in path {
            symbol(*part, symbol_count, "alias path symbol")?;
        }
    }
    for (namespace, _) in &payload.imports {
        symbol(*namespace, symbol_count, "import namespace")?;
    }
    Ok(())
}

fn refs(refs: &[SignedRefR], symbol_count: usize, decl_count: usize) -> Result<(), CborError> {
    for reference in refs {
        let atom = reference.atom();
        required_decl(atom.target, decl_count, "reference target")?;
        anno(&atom.anno, symbol_count, decl_count)?;
        if let Some(weights) = &atom.weights {
            for weight in weights {
                value(weight, symbol_count, decl_count)?;
            }
        }
    }
    Ok(())
}

fn anno(anno: &AnnoR, symbol_count: usize, decl_count: usize) -> Result<(), CborError> {
    for tag in &anno.tags {
        symbol(*tag, symbol_count, "annotation tag")?;
    }
    if let Some(value_ref) = &anno.value {
        value(value_ref, symbol_count, decl_count)?;
    }
    Ok(())
}

fn value(root: &ValueR, symbol_count: usize, decl_count: usize) -> Result<(), CborError> {
    let mut pending = vec![root];
    while let Some(value_ref) = pending.pop() {
        match value_ref {
            ValueR::Str(symbol_id) => symbol(*symbol_id, symbol_count, "string value")?,
            ValueR::Ref(decl) => required_decl(*decl, decl_count, "value reference")?,
            ValueR::List(values) => pending.extend(values),
            ValueR::Num(_) => {}
        }
    }
    Ok(())
}

fn symbol(id: SymId, count: usize, field: &'static str) -> Result<(), CborError> {
    if id.is_none() || id.raw() >= count {
        return Err(CborError::SymbolOutOfRange {
            field,
            symbol: id.raw(),
        });
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
