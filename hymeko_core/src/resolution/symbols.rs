//! Model-local symbol lifetime. One typed field walk serves marking and rewriting.
//! Marking borrows shared IR tables; only dense rewriting may detach them.
use crate::ir::table::IrTable;
use crate::{
    common::ids::{DeclId, SymId},
    ir::ir::{AnnoR, Extent, Ir, ModelFacet, SignedRefR, ValueR},
    module_store::module_store::ModuleKey,
    resolution::{interner::Interner, resolve::Index},
};
use rustc_hash::FxHashMap;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

/// Groups share a mutation boundary, not an object lifetime. Names are separate
/// from annotations so value-only remaps cannot invalidate structural caches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SymbolRecord {
    DeclarationName(usize),
    DeclarationAnnotation(usize),
    Namespace(usize),
    NodeBases(usize),
    EdgeBases(usize),
    ArcAnnotation(usize),
    ArcReferences(usize),
    Facet(usize),
    Index,
    Imports,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SymbolUse {
    symbol: usize,
    record: SymbolRecord,
    count: usize,
}

impl SymbolUse {
    fn key(&self) -> (usize, SymbolRecord) {
        (self.symbol, self.record)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SymbolError {
    pub field: &'static str,
    pub symbol: usize,
}

trait SymbolReader {
    type Error: From<SymbolError>;
    fn record(&mut self, _record: SymbolRecord) {}
    fn payload(&mut self, _payload: ReferencePayload) -> bool {
        true
    }
    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), Self::Error>;
}

trait SymbolWriter {
    type Error: From<SymbolError>;
    fn record(&mut self, _record: SymbolRecord) {}
    fn payload(&mut self, payload: ReferencePayload) -> bool;
    fn symbol(&mut self, id: &mut SymId, field: &'static str) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ReferencePayload {
    Annotation(usize),
    Weights(usize),
}

trait SymbolMark {
    fn record(&mut self) -> Option<()>;
}

impl SymbolMark for bool {
    fn record(&mut self) -> Option<()> {
        *self = true;
        Some(())
    }
}

impl SymbolMark for usize {
    fn record(&mut self) -> Option<()> {
        *self = self.checked_add(1)?;
        Some(())
    }
}

/// Exact occurrences, including immutable topology, metadata and resolution paths.
/// Derived state belongs to one immutable program/interner pair, never the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SymbolUsage {
    counts: Arc<Vec<usize>>,
    uses: Arc<Vec<SymbolUse>>,
    unused: usize,
}

struct IndexedSymbols<'a> {
    counts: &'a mut [usize],
    uses: BTreeMap<(usize, SymbolRecord), usize>,
    record: SymbolRecord,
}

impl SymbolReader for IndexedSymbols<'_> {
    type Error = SymbolError;
    fn record(&mut self, record: SymbolRecord) {
        self.record = record;
    }
    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), SymbolError> {
        SymbolMarks(&mut *self.counts).symbol(id, field)?;
        // A per-record count cannot exceed the checked total for this symbol.
        *self.uses.entry((id.raw(), self.record)).or_default() += 1;
        Ok(())
    }
}

struct SymbolMarks<'a, T>(&'a mut [T]);
impl<T: SymbolMark> SymbolReader for SymbolMarks<'_, T> {
    type Error = SymbolError;
    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), SymbolError> {
        let error = SymbolError {
            field,
            symbol: id.raw(),
        };
        self.0.get_mut(id.raw()).ok_or(error)?.record().ok_or(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymbolUpdateError {
    Invalid,
    CapacityExceeded,
}

impl From<SymbolError> for SymbolUpdateError {
    fn from(_: SymbolError) -> Self {
        Self::Invalid
    }
}

/// Reusable model-independent occurrence storage; never borrows IR payloads.
#[derive(Debug)]
pub(crate) struct SymbolDelta {
    occurrences: Vec<(usize, SymbolRecord, bool)>,
    limit: usize,
    adding: bool,
    symbols: usize,
    record: SymbolRecord,
}

impl SymbolDelta {
    pub(crate) fn temporary() -> Self {
        Self {
            occurrences: Vec::new(),
            limit: usize::MAX,
            adding: false,
            symbols: 0,
            record: SymbolRecord::Index,
        }
    }

    pub(crate) fn with_capacity(limit: usize) -> Result<Self, std::collections::TryReserveError> {
        let mut delta = Self::temporary();
        delta.occurrences.try_reserve_exact(limit)?;
        delta.limit = limit;
        Ok(delta)
    }

    pub(crate) fn clear(&mut self) {
        self.occurrences.clear();
        self.adding = false;
        self.symbols = 0;
        self.record = SymbolRecord::Index;
    }

    #[cfg(test)]
    pub(crate) fn storage(&self) -> (usize, usize, usize) {
        (
            self.occurrences.as_ptr() as usize,
            self.occurrences.capacity(),
            self.occurrences.len(),
        )
    }

    /// Nonzero per-record changes, in the same order as the inverse index.
    fn changes(&self) -> impl Iterator<Item = (usize, SymbolRecord, usize, usize)> + '_ {
        let mut cursor = 0;
        std::iter::from_fn(move || {
            while cursor < self.occurrences.len() {
                let (symbol, record, _) = self.occurrences[cursor];
                let (mut added, mut removed) = (0, 0);
                while cursor < self.occurrences.len()
                    && (self.occurrences[cursor].0, self.occurrences[cursor].1) == (symbol, record)
                {
                    if self.occurrences[cursor].2 {
                        added += 1;
                    } else {
                        removed += 1;
                    }
                    cursor += 1;
                }
                if added != removed {
                    return Some((symbol, record, added, removed));
                }
            }
            None
        })
    }
}

impl SymbolReader for SymbolDelta {
    type Error = SymbolUpdateError;

    fn symbol(&mut self, id: &SymId, field: &'static str) -> Result<(), SymbolUpdateError> {
        if id.raw() >= self.symbols {
            return Err(SymbolError {
                field,
                symbol: id.raw(),
            }
            .into());
        }
        if self.occurrences.len() == self.limit {
            return Err(SymbolUpdateError::CapacityExceeded);
        }
        self.occurrences.push((id.raw(), self.record, self.adding));
        Ok(())
    }
}

impl SymbolUsage {
    pub(crate) fn new(
        ir: &Ir,
        index: &Index,
        imports: &[(SymId, ModuleKey)],
        source: &Interner,
    ) -> Result<Self, SymbolError> {
        let mut counts = vec![0; source.iter().count()];
        let mut indexed = IndexedSymbols {
            counts: &mut counts,
            uses: BTreeMap::new(),
            record: SymbolRecord::Index,
        };
        ProgramSymbols { ir, index, imports }.mark(&mut indexed)?;
        let uses = indexed
            .uses
            .into_iter()
            .map(|((symbol, record), count)| SymbolUse {
                symbol,
                record,
                count,
            })
            .collect();
        Ok(Self {
            unused: counts.iter().filter(|&&count| count == 0).count(),
            counts: Arc::new(counts),
            uses: Arc::new(uses),
        })
    }

    fn updated_uses(&self, delta: &SymbolDelta) -> Result<Arc<Vec<SymbolUse>>, SymbolUpdateError> {
        let mut changes = delta.changes().peekable();
        if changes.peek().is_none() {
            return Ok(Arc::clone(&self.uses));
        }
        let mut old = self.uses.iter().copied().peekable();
        let mut rows = Vec::with_capacity(self.uses.len());
        while old.peek().is_some() || changes.peek().is_some() {
            match (old.peek(), changes.peek()) {
                (Some(row), Some(&(symbol, record, _, _))) if row.key() < (symbol, record) => {
                    rows.push(old.next().expect("peeked use"));
                }
                (Some(_), None) => {
                    rows.extend(old);
                    break;
                }
                (_, Some(_)) => {
                    let (symbol, record, added, removed) = changes.next().expect("peeked change");
                    let count = if old.peek().is_some_and(|row| row.key() == (symbol, record)) {
                        old.next().expect("matched use").count
                    } else {
                        0
                    };
                    let count = count
                        .checked_sub(removed)
                        .and_then(|value| value.checked_add(added))
                        .ok_or(SymbolUpdateError::Invalid)?;
                    if count != 0 {
                        rows.push(SymbolUse {
                            symbol,
                            record,
                            count,
                        });
                    }
                }
                (None, None) => break,
            }
        }
        Ok(Arc::new(rows))
    }

    /// Change counts only for the supplied leaf values. Topology, annotations
    /// other than values, index and imports must still match the base snapshot.
    pub(crate) fn with_values(
        &self,
        ir: (&Ir, &mut Ir),
        index: &mut Arc<Index>,
        imports: &mut Arc<Vec<(SymId, ModuleKey)>>,
        source: &Interner,
        changed: &[DeclId],
        delta: &mut SymbolDelta,
    ) -> Result<(SymbolCompaction, Self), SymbolUpdateError> {
        let (before, after) = ir;
        delta.clear();
        let symbols = source.iter().count();
        if symbols < self.counts.len() {
            return Err(SymbolError {
                field: "symbol usage table",
                symbol: self.counts.len(),
            }
            .into());
        }
        delta.symbols = symbols;
        for &declaration in changed {
            delta.record = SymbolRecord::DeclarationAnnotation(declaration.raw());
            for (ir, adding) in [(before, false), (&*after, true)] {
                delta.adding = adding;
                if let Some(value) = &ir[ir.decl_nodes[declaration.raw()].anno].value {
                    ProgramSymbols::read_value(value, delta)?;
                }
            }
        }
        // Combine before mutating so swaps and equal symbol multisets neither
        // detach the counts nor transiently remove a still-referenced string.
        delta
            .occurrences
            .sort_unstable_by_key(|&(symbol, record, _)| (symbol, record));
        let mut next = self.clone();
        next.uses = self.updated_uses(delta)?;
        if symbols > next.counts.len() {
            next.unused += symbols - next.counts.len();
            Arc::make_mut(&mut next.counts).resize(symbols, 0);
        }
        let mut cursor = 0;
        while cursor < delta.occurrences.len() {
            let symbol = delta.occurrences[cursor].0;
            let mut added = 0;
            let mut removed = 0;
            while cursor < delta.occurrences.len() && delta.occurrences[cursor].0 == symbol {
                if delta.occurrences[cursor].2 {
                    added += 1;
                } else {
                    removed += 1;
                }
                cursor += 1;
            }
            let old = next.counts[symbol];
            let new = old
                .checked_sub(removed)
                .and_then(|n| n.checked_add(added))
                .ok_or(SymbolError {
                    field: "symbol usage count",
                    symbol,
                })?;
            if old != new {
                Arc::make_mut(&mut next.counts)[symbol] = new;
                if old == 0 {
                    next.unused -= 1;
                }
                if new == 0 {
                    next.unused += 1;
                }
            }
        }
        if next.unused == 0 {
            return Ok((
                SymbolCompaction {
                    interner: source.fork(),
                    renumbered: false,
                    path_symbols_changed: false,
                    structural_symbols_changed: false,
                },
                next,
            ));
        }
        let used: Vec<_> = next.counts.iter().map(|&count| count != 0).collect();
        let (mut path_symbols_changed, mut structural_symbols_changed) = (false, false);
        let mut compacted = SymbolCompaction::retain(source, &used, |mapping| {
            let mut records: Vec<_> = next
                .uses
                .iter()
                .filter(|row| mapping[row.symbol] != SymId::new(row.symbol))
                .map(|row| row.record)
                .collect();
            records.sort_unstable();
            records.dedup();
            let mut visitor = DenseSymbols::new(mapping);
            for record in records {
                path_symbols_changed |= matches!(
                    record,
                    SymbolRecord::DeclarationName(_) | SymbolRecord::Namespace(_)
                );
                structural_symbols_changed |= matches!(
                    record,
                    SymbolRecord::DeclarationName(_)
                        | SymbolRecord::Namespace(_)
                        | SymbolRecord::Facet(_)
                        | SymbolRecord::Index
                        | SymbolRecord::Imports
                );
                match record {
                    SymbolRecord::Index => {
                        ProgramSymbols::rewrite_index(Arc::make_mut(index), &mut visitor)?
                    }
                    SymbolRecord::Imports => ProgramSymbols::rewrite_imports(
                        Arc::make_mut(imports).as_mut_slice(),
                        &mut visitor,
                    )?,
                    _ => ProgramSymbols::write_record(after, record, &mut visitor)?,
                }
            }
            // Retention is monotone and injective on live IDs, so row order stays valid.
            for row in Arc::make_mut(&mut next.uses) {
                row.symbol = mapping[row.symbol].raw();
            }
            Ok(())
        })?;
        compacted.path_symbols_changed = path_symbols_changed;
        compacted.structural_symbols_changed = structural_symbols_changed;
        next.counts = Arc::new(
            next.counts
                .iter()
                .copied()
                .filter(|&count| count != 0)
                .collect(),
        );
        next.unused = 0;
        Ok((compacted, next))
    }
}

struct DenseSymbols<'a> {
    mapping: &'a [SymId],
    visited: HashSet<ReferencePayload>,
}
impl<'a> DenseSymbols<'a> {
    fn new(mapping: &'a [SymId]) -> Self {
        Self {
            mapping,
            visited: HashSet::new(),
        }
    }
}
impl SymbolWriter for DenseSymbols<'_> {
    type Error = SymbolError;
    fn payload(&mut self, payload: ReferencePayload) -> bool {
        self.visited.insert(payload)
    }
    fn symbol(&mut self, id: &mut SymId, field: &'static str) -> Result<(), SymbolError> {
        *id = self
            .mapping
            .get(id.raw())
            .copied()
            .filter(|s| s.is_some())
            .ok_or(SymbolError {
                field,
                symbol: id.raw(),
            })?;
        Ok(())
    }
}

pub(crate) struct SymbolCompaction {
    pub interner: Interner,
    pub renumbered: bool,
    pub path_symbols_changed: bool,
    pub structural_symbols_changed: bool,
}
impl SymbolCompaction {
    pub(crate) fn apply(
        ir: &mut Ir,
        index: &mut Index,
        imports: &mut [(SymId, ModuleKey)],
        source: &Interner,
    ) -> Result<Self, SymbolError> {
        let mut used = vec![false; source.iter().count()];
        let program = ProgramSymbols { ir, index, imports };
        program.mark(&mut SymbolMarks(&mut used))?;
        Self::retain(source, &used, |mapping| {
            ProgramSymbols::rewrite(ir, index, imports, &mut DenseSymbols::new(mapping))
        })
    }

    fn retain(
        source: &Interner,
        used: &[bool],
        rewrite: impl FnOnce(&[SymId]) -> Result<(), SymbolError>,
    ) -> Result<Self, SymbolError> {
        let (interner, mapping) = source.retained(used);
        let renumbered = mapping.is_some();
        if let Some(mapping) = mapping {
            rewrite(&mapping)?;
        }
        Ok(Self {
            interner,
            renumbered,
            path_symbols_changed: renumbered,
            structural_symbols_changed: renumbered,
        })
    }
}

struct ProgramSymbols<'a> {
    ir: &'a Ir,
    index: &'a Index,
    imports: &'a [(SymId, ModuleKey)],
}

// One field schema, instantiated with shared or mutable borrows. Merely asking
// for mutable iteration would detach a shared table, even when no ID changes.
// The visitor signatures enforce that the marking expansion cannot write.
macro_rules! symbol_walk {
    ($walk:ident, $record:ident, $path:ident, $anno:ident, $refs:ident, $value:ident,
       $annotation:ident, $bases:ident, $visitor:ident $(, $mutable:tt)?) => {
        impl ProgramSymbols<'_> {
            fn $walk<V: $visitor>(ir: & $($mutable)? Ir, visitor: &mut V) -> Result<(), V::Error> {
                for i in 0..ir.decl_nodes.len() {
                    Self::$record(ir, SymbolRecord::DeclarationName(i), visitor)?;
                    Self::$record(ir, SymbolRecord::DeclarationAnnotation(i), visitor)?;
                }
                for i in 0..ir.namespace_roots.len() { Self::$record(ir, SymbolRecord::Namespace(i), visitor)?; }
                for i in 0..ir.nodes.len() { Self::$record(ir, SymbolRecord::NodeBases(i), visitor)?; }
                for i in 0..ir.edges.len() { Self::$record(ir, SymbolRecord::EdgeBases(i), visitor)?; }
                for i in 0..ir.arcs.len() {
                    Self::$record(ir, SymbolRecord::ArcAnnotation(i), visitor)?;
                    Self::$record(ir, SymbolRecord::ArcReferences(i), visitor)?;
                }
                for i in 0..ir.decl_model.len() { Self::$record(ir, SymbolRecord::Facet(i), visitor)?; }
                Ok(())
            }

            fn $record<V: $visitor>(ir: & $($mutable)? Ir, record: SymbolRecord, visitor: &mut V) -> Result<(), V::Error> {
                visitor.record(record);
                match record {
                    SymbolRecord::DeclarationName(i) => visitor.symbol(& $($mutable)? ir.decl_nodes[i].name, "declaration name")?,
                    SymbolRecord::DeclarationAnnotation(i) => Self::$annotation(ir.decl_nodes[i].anno, & $($mutable)? ir.annotations, visitor)?,
                    SymbolRecord::Namespace(i) => Self::$path(& $($mutable)? ir.namespace_roots[i].1, "namespace prefix symbol", visitor)?,
                    SymbolRecord::NodeBases(i) => Self::$bases(ir.nodes[i].bases, &ir.base_lists, & $($mutable)? ir.annotations, & $($mutable)? ir.ref_weights, visitor)?,
                    SymbolRecord::EdgeBases(i) => Self::$bases(ir.edges[i].bases, &ir.base_lists, & $($mutable)? ir.annotations, & $($mutable)? ir.ref_weights, visitor)?,
                    SymbolRecord::ArcAnnotation(i) => Self::$annotation(ir.arcs[i].anno, & $($mutable)? ir.annotations, visitor)?,
                    SymbolRecord::ArcReferences(i) => Self::$refs(&ir.arcs[i].refs, & $($mutable)? ir.annotations, & $($mutable)? ir.ref_weights, visitor)?,
                    SymbolRecord::Facet(i) => if let Some(facet) = & $($mutable)? ir.decl_model[i] {
                        match facet {
                            ModelFacet::Tensor(tensor) => {
                                for dimension in & $($mutable)? tensor.shape {
                                    match dimension {
                                        Extent::Symbolic(id) => visitor.symbol(id, "tensor symbolic extent")?,
                                        Extent::Static(_) => {}
                                    }
                                }
                            }
                            ModelFacet::Operator(operator) => visitor.symbol(& $($mutable)? operator.identity, "operator identity")?,
                        }
                    },
                    SymbolRecord::Index | SymbolRecord::Imports => unreachable!("metadata has its own owner"),
                }
                Ok(())
            }

            fn $path<V: $visitor>(path: & $($mutable)? [SymId], field: &'static str,
                     visitor: &mut V) -> Result<(), V::Error> {
                for id in path { visitor.symbol(id, field)?; }
                Ok(())
            }

            fn $anno<V: $visitor>(anno: & $($mutable)? AnnoR, visitor: &mut V) -> Result<(), V::Error> {
                Self::$path(& $($mutable)? anno.tags, "annotation tag", visitor)?;
                if let Some(value) = & $($mutable)? anno.value { Self::$value(value, visitor)?; }
                Ok(())
            }

            fn $bases<V: $visitor>(id: crate::common::ids::BaseListId,
                lists: &IrTable<Box<[SignedRefR]>>, annotations: & $($mutable)? IrTable<AnnoR>,
                weights: & $($mutable)? IrTable<Vec<ValueR>>, visitor: &mut V) -> Result<(), V::Error> {
                let refs = if id.is_none() { &[][..] } else {
                    lists.get_record(id.raw()).ok_or(SymbolError { field: "base-list ID", symbol: id.raw() })?.as_ref()
                };
                Self::$refs(refs, annotations, weights, visitor)
            }

            fn $annotation<V: $visitor>(id: crate::common::ids::AnnotationId,
                annotations: & $($mutable)? IrTable<AnnoR>, visitor: &mut V) -> Result<(), V::Error> {
                if id.is_some() {
                    if id.raw() >= annotations.len() {
                        return Err(SymbolError { field: "annotation ID", symbol: id.raw() }.into());
                    }
                    Self::$anno(& $($mutable)? annotations[id.raw()], visitor)?;
                }
                Ok(())
            }

            fn $refs<V: $visitor>(refs: &[SignedRefR], annotations: & $($mutable)? IrTable<AnnoR>, weights: & $($mutable)? IrTable<Vec<ValueR>>, visitor: &mut V) -> Result<(), V::Error> {
                for reference in refs {
                    let id = reference.annotation_id();
                    if id.is_some() && visitor.payload(ReferencePayload::Annotation(id.raw())) {
                        Self::$annotation(id, annotations, visitor)?;
                    }
                    let vector = reference.weight_vector_id();
                    if vector.is_some() && visitor.payload(ReferencePayload::Weights(vector.raw())) {
                        if vector.raw() >= weights.len() {
                            return Err(SymbolError { field: "reference weight-vector ID", symbol: vector.raw() }.into());
                        }
                        for value in & $($mutable)? weights[vector.raw()] { Self::$value(value, visitor)?; }
                    }
                }
                Ok(())
            }

            fn $value<V: $visitor>(root: & $($mutable)? ValueR, visitor: &mut V) -> Result<(), V::Error> {
                match root {
                    ValueR::Num(_) | ValueR::Ref(_) => return Ok(()),
                    ValueR::Str(id) => return visitor.symbol(id, "string value"),
                    ValueR::List(_) => {}
                }
                let mut pending = vec![root];
                while let Some(value) = pending.pop() {
                    match value {
                        ValueR::Str(id) => visitor.symbol(id, "string value")?,
                        ValueR::List(values) => pending.extend(values),
                        ValueR::Num(_) | ValueR::Ref(_) => {}
                    }
                }
                Ok(())
            }
        }
    };
}

symbol_walk!(
    read_ir,
    read_record,
    read_path,
    read_anno,
    read_refs,
    read_value,
    read_annotation,
    read_bases,
    SymbolReader
);
symbol_walk!(
    write_ir,
    write_record,
    write_path,
    write_anno,
    write_refs,
    write_value,
    write_annotation,
    write_bases,
    SymbolWriter,
    mut
);

impl ProgramSymbols<'_> {
    fn mark<V: SymbolReader>(&self, visitor: &mut V) -> Result<(), V::Error> {
        Self::read_ir(self.ir, visitor)?;
        visitor.record(SymbolRecord::Index);
        for path in self.index.by_path.keys() {
            Self::read_path(&path.0, "path-index symbol", visitor)?;
        }
        for aliases in std::iter::once(&self.index.aliases)
            .chain(self.index.module_aliases.iter().map(|(_, aliases)| aliases))
        {
            for (alias, path) in aliases {
                visitor.symbol(alias, "alias")?;
                Self::read_path(path, "alias path symbol", visitor)?;
            }
        }
        visitor.record(SymbolRecord::Imports);
        for (namespace, _) in self.imports.iter() {
            visitor.symbol(namespace, "import namespace")?;
        }
        Ok(())
    }

    fn rewrite(
        ir: &mut Ir,
        index: &mut Index,
        imports: &mut [(SymId, ModuleKey)],
        visitor: &mut DenseSymbols<'_>,
    ) -> Result<(), SymbolError> {
        Self::write_ir(ir, visitor)?;
        Self::rewrite_index(index, visitor)?;
        Self::rewrite_imports(imports, visitor)
    }

    fn rewrite_index(index: &mut Index, visitor: &mut DenseSymbols<'_>) -> Result<(), SymbolError> {
        // A dense rewrite changes keys, so it must rebuild their maps.
        let paths = std::mem::take(&mut index.by_path);
        index.by_path.reserve(paths.len());
        for (mut path, declaration) in paths {
            Self::write_path(&mut path.0, "path-index symbol", visitor)?;
            index.by_path.insert(path, declaration);
        }
        Self::rewrite_aliases(&mut index.aliases, visitor)?;
        for (_, aliases) in &mut index.module_aliases {
            Self::rewrite_aliases(aliases, visitor)?;
        }
        Ok(())
    }

    fn rewrite_imports(
        imports: &mut [(SymId, ModuleKey)],
        visitor: &mut DenseSymbols<'_>,
    ) -> Result<(), SymbolError> {
        for (namespace, _) in imports {
            visitor.symbol(namespace, "import namespace")?;
        }
        Ok(())
    }

    fn rewrite_aliases(
        aliases: &mut FxHashMap<SymId, Vec<SymId>>,
        visitor: &mut DenseSymbols<'_>,
    ) -> Result<(), SymbolError> {
        let old = std::mem::take(aliases);
        aliases.reserve(old.len());
        for (mut alias, mut path) in old {
            visitor.symbol(&mut alias, "alias")?;
            Self::write_path(&mut path, "alias path symbol", visitor)?;
            aliases.insert(alias, path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ir::{
            canonical_hash::{Canon, CanonHashCfg, build_path_index},
            ir::{DType, OperatorContract, PortDirection, RefAtomR, TensorContract, TensorRole},
        },
        ir_stream::DecodeLimits,
        module_store::{execution::CompiledModel, source_provider::MemProvider},
        util::real_parser::RealParser,
        writers::cbor_writer,
    };
    use std::path::Path;

    struct Fixture {
        ir: Ir,
        index: Index,
        strings: Interner,
        imports: Vec<(SymId, ModuleKey)>,
    }
    impl Fixture {
        fn new() -> Self {
            let model = CompiledModel::compile(
                MemProvider::default().with_file(
                    "model.hymeko",
                    "description {} root { label \"active\"; target {} @edge { (+target); } }",
                ),
                RealParser,
                Path::new("model.hymeko"),
            )
            .unwrap();
            let mut ir = model.program().ir.clone();
            let mut strings = Interner::new();
            for (_, text) in model.interner().iter() {
                strings.intern(text);
            }
            let mut symbol = |text: &str| {
                strings.intern(&format!("unused-before-{text}"));
                strings.intern(text)
            };
            let [
                namespace,
                name,
                tag,
                scalar,
                nested,
                dimension,
                operator,
                alias,
                local,
            ] = [
                "namespace",
                "renamed-root",
                "tag",
                "scalar",
                "nested",
                "dimension",
                "operator",
                "alias",
                "local-alias",
            ]
            .map(&mut symbol);
            let root = ir.nodes[0].decl;
            let target = ir.nodes.last().unwrap().decl;
            ir.decl_nodes[root.raw()].name = name;
            ir.record_namespace_root(root, &[namespace]);
            for raw in 0..ir.decl_count() {
                ir.decl_annotation_mut(DeclId::new(raw))
                    .unwrap()
                    .tags
                    .push(tag);
            }
            let atom = || RefAtomR {
                target,
                direction: PortDirection::Out,
                anno: AnnoR {
                    tags: vec![tag],
                    value: Some(ValueR::Str(scalar)),
                },
                weights: Some(vec![ValueR::List(vec![
                    ValueR::Str(nested),
                    ValueR::Ref(root),
                ])]),
            };
            use crate::ir::ir::{IncidenceSign, ReferenceDraft};
            let plus = ir.store_reference(ReferenceDraft::new(IncidenceSign::Plus, atom()));
            let minus = ir.store_reference(ReferenceDraft::new(IncidenceSign::Minus, atom()));
            ir.nodes[0].bases = ir.store_bases(vec![plus]);
            ir.edges[0].bases = ir.store_bases(vec![minus]);
            ir.arcs[0].anno = ir.store_annotation(atom().anno);
            let mut input = atom();
            input.target = root;
            input.direction = PortDirection::In;
            let references = vec![
                ir.store_reference(ReferenceDraft::new(IncidenceSign::Plus, input)),
                ir.store_reference(ReferenceDraft::new(IncidenceSign::Minus, atom())),
                ir.store_reference(ReferenceDraft::new(IncidenceSign::Neutral, atom())),
            ];
            ir.arcs[0].refs = references;
            ir.set_model_facet(
                root,
                Some(ModelFacet::Tensor(TensorContract {
                    role: TensorRole::Input,
                    shape: vec![Extent::Symbolic(dimension)],
                    dtype: DType::F64,
                })),
            );
            ir.set_model_facet(
                ir.edges[0].decl,
                Some(ModelFacet::Operator(OperatorContract {
                    identity: operator,
                })),
            );
            ir.set_model_facet(
                target,
                Some(ModelFacet::Tensor(TensorContract {
                    role: TensorRole::Output,
                    shape: vec![Extent::Static(1)],
                    dtype: DType::F64,
                })),
            );
            crate::ir::coherence::validate_model_contract(&ir, &strings).unwrap();
            let mut index = Index::new();
            index.by_path = build_path_index(&ir)
                .unwrap()
                .iter()
                .map(|(p, d)| (p.clone(), d))
                .collect();
            index.aliases.insert(alias, vec![namespace, name]);
            index.module_aliases.push((
                0..ir.decl_count(),
                [(local, vec![namespace, name])].into_iter().collect(),
            ));
            Self {
                ir,
                index,
                strings,
                imports: vec![(namespace, ModuleKey("modules/library.hymeko".into()))],
            }
        }
    }

    #[test]
    fn symbol_marks_reject_range_and_count_overflow_without_changing_slots() {
        let mut counts = [usize::MAX];
        assert!(
            SymbolMarks(&mut counts)
                .symbol(&SymId::new(0), "overflow")
                .is_err()
        );
        assert_eq!(counts, [usize::MAX]);
        let mut live = [false];
        assert!(
            SymbolMarks(&mut live)
                .symbol(&SymId::new(1), "out of range")
                .is_err()
        );
        assert_eq!(live, [false]);
    }

    #[test]
    fn incremental_usage_matches_full_compaction_with_aliases_and_metadata() {
        let mut f = Fixture::new();
        let a = f.ir.nodes[1].decl;
        let b = f.ir.nodes[2].decl;
        let mut index = Arc::new(f.index);
        let mut imports = Arc::new(f.imports);
        let mut usage = SymbolUsage::new(&f.ir, &index, &imports, &f.strings).unwrap();
        for step in 0..24 {
            let mut source = f.strings.fork();
            let fresh = source.intern(&format!("new-value-{step}"));
            // A dead suffix must be removed even if no retained ID needs moving.
            source.intern("unused-suffix");
            let tag = source.get_id("tag").unwrap();
            let mut edited = f.ir.clone();
            edited.decl_annotation_mut(a).unwrap().value = Some(ValueR::List(vec![
                ValueR::Str(tag),
                ValueR::Str(fresh),
                ValueR::List(vec![ValueR::Str(fresh), ValueR::Num(step as f64)]),
            ]));
            edited.decl_annotation_mut(b).unwrap().value = if step % 2 == 0 {
                Some(ValueR::Str(fresh))
            } else {
                None
            };
            let mut reference_ir = edited.clone();
            let mut reference_index = index.as_ref().clone();
            let mut reference_imports = imports.as_ref().clone();
            let reference = SymbolCompaction::apply(
                &mut reference_ir,
                &mut reference_index,
                &mut reference_imports,
                &source,
            )
            .unwrap();
            let old_index = Arc::clone(&index);
            let old_imports = Arc::clone(&imports);
            let old_usage = usage.clone();
            let (actual, next) = usage
                .with_values(
                    (&f.ir, &mut edited),
                    &mut index,
                    &mut imports,
                    &source,
                    &[a, b],
                    &mut SymbolDelta::temporary(),
                )
                .unwrap();
            assert_eq!(
                serde_json::to_vec(&edited).unwrap(),
                serde_json::to_vec(&reference_ir).unwrap()
            );
            assert_eq!(index.by_path, reference_index.by_path);
            assert_eq!(index.aliases, reference_index.aliases);
            assert_eq!(index.module_aliases, reference_index.module_aliases);
            assert_eq!(imports.as_ref(), &reference_imports);
            assert_eq!(actual.renumbered, reference.renumbered);
            assert_eq!(
                actual.interner.iter().collect::<Vec<_>>(),
                reference.interner.iter().collect::<Vec<_>>()
            );
            assert_eq!(
                next,
                SymbolUsage::new(&edited, &index, &imports, &actual.interner).unwrap()
            );
            assert_eq!(usage, old_usage);
            assert_eq!(
                usage,
                SymbolUsage::new(&f.ir, &old_index, &old_imports, &f.strings).unwrap()
            );
            let index_changed = old_index.by_path != index.by_path
                || old_index.aliases != index.aliases
                || old_index.module_aliases != index.module_aliases;
            assert_eq!(Arc::ptr_eq(&old_index, &index), !index_changed);
            assert_eq!(Arc::ptr_eq(&old_imports, &imports), old_imports == imports);
            assert!(actual.interner.get_id("unused-suffix").is_none());
            f.ir = edited;
            f.strings = actual.interner;
            usage = next;
        }
    }

    #[test]
    fn swapped_values_update_inverse_rows_even_when_global_counts_stay_equal() {
        let mut f = Fixture::new();
        let a = f.ir.nodes[1].decl;
        let b = f.ir.nodes[2].decl;
        f.ir.decl_annotation_mut(a).unwrap().value =
            Some(ValueR::Str(f.strings.get_id("tag").unwrap()));
        f.ir.decl_annotation_mut(b).unwrap().value =
            Some(ValueR::Str(f.strings.get_id("nested").unwrap()));
        f.strings = SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &f.strings)
            .unwrap()
            .interner;
        let mut index = Arc::new(f.index);
        let mut imports = Arc::new(f.imports);
        let usage = SymbolUsage::new(&f.ir, &index, &imports, &f.strings).unwrap();
        let mut after = f.ir.clone();
        after.decl_annotation_mut(a).unwrap().value =
            f.ir.decl_annotation(b).unwrap().value.clone();
        after.decl_annotation_mut(b).unwrap().value =
            f.ir.decl_annotation(a).unwrap().value.clone();
        let (compacted, next) = usage
            .with_values(
                (&f.ir, &mut after),
                &mut index,
                &mut imports,
                &f.strings,
                &[a, b],
                &mut SymbolDelta::temporary(),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&usage.counts, &next.counts));
        assert!(!Arc::ptr_eq(&usage.uses, &next.uses));
        assert!(!compacted.renumbered);
        assert_eq!(
            next,
            SymbolUsage::new(&after, &index, &imports, &compacted.interner).unwrap()
        );
    }

    #[test]
    fn numeric_and_equal_symbol_multisets_keep_shared_counts() {
        let mut f = Fixture::new();
        let compact =
            SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &f.strings).unwrap();
        f.strings = compact.interner;
        let target = f.ir.nodes[2].decl;
        let tag = f.strings.get_id("tag").unwrap();
        for (before, after) in [
            (Some(ValueR::Num(0.0)), Some(ValueR::Num(1.0))),
            (
                Some(ValueR::List(vec![
                    ValueR::Str(tag),
                    ValueR::Num(0.0),
                    ValueR::Str(tag),
                ])),
                Some(ValueR::List(vec![
                    ValueR::Str(tag),
                    ValueR::List(vec![ValueR::Str(tag)]),
                    ValueR::Num(2.0),
                ])),
            ),
        ] {
            f.ir.decl_annotation_mut(target).unwrap().value = before;
            let mut edited = f.ir.clone();
            edited.decl_annotation_mut(target).unwrap().value = after;
            let mut index = Arc::new(f.index.clone());
            let mut imports = Arc::new(f.imports.clone());
            let old_index = Arc::clone(&index);
            let old_imports = Arc::clone(&imports);
            let usage = SymbolUsage::new(&f.ir, &index, &imports, &f.strings).unwrap();
            let (compacted, next) = usage
                .with_values(
                    (&f.ir, &mut edited),
                    &mut index,
                    &mut imports,
                    &f.strings,
                    &[target],
                    &mut SymbolDelta::temporary(),
                )
                .unwrap();
            assert!(Arc::ptr_eq(&usage.counts, &next.counts));
            assert!(Arc::ptr_eq(&usage.uses, &next.uses));
            assert!(Arc::ptr_eq(&old_index, &index));
            assert!(Arc::ptr_eq(&old_imports, &imports));
            assert!(!compacted.renumbered);
            assert_eq!(
                next,
                SymbolUsage::new(&edited, &index, &imports, &compacted.interner).unwrap()
            );
        }
    }

    #[test]
    fn usage_rejects_bad_symbols_and_count_underflow_without_changing_base() {
        let f = Fixture::new();
        let target = f.ir.nodes[1].decl;
        let usage = SymbolUsage::new(&f.ir, &f.index, &f.imports, &f.strings).unwrap();
        let base = usage.clone();
        let mut index = Arc::new(f.index);
        let mut imports = Arc::new(f.imports);
        let mut edited = f.ir.clone();
        edited.decl_annotation_mut(target).unwrap().value = Some(ValueR::Str(SymId::NONE));
        assert!(SymbolUsage::new(&edited, &index, &imports, &f.strings).is_err());
        assert!(
            usage
                .with_values(
                    (&f.ir, &mut edited),
                    &mut index,
                    &mut imports,
                    &f.strings,
                    &[target],
                    &mut SymbolDelta::temporary(),
                )
                .is_err()
        );
        edited.decl_annotation_mut(target).unwrap().value = None;
        // Internal misuse: removing the same single-use value twice must fail
        // closed instead of wrapping a counter and retaining a dead symbol.
        assert!(
            usage
                .with_values(
                    (&f.ir, &mut edited),
                    &mut index,
                    &mut imports,
                    &f.strings,
                    &[target, target],
                    &mut SymbolDelta::temporary(),
                )
                .is_err()
        );
        assert_eq!(usage, base);
        assert!(Arc::ptr_eq(&usage.counts, &base.counts));
    }

    #[test]
    fn marking_keeps_path_and_alias_maps_in_place() {
        let mut f = Fixture::new();
        f.ir = f.ir.into_shared();
        let retained = f.ir.clone();
        let symbol = SymId::NONE;
        let (alias, path) = f.index.aliases.iter().next().unwrap();
        let aliases_before = (*alias, path.clone(), alias as *const SymId as usize);
        let (path, declaration) = f.index.by_path.iter().next().unwrap();
        let path_before = (path.clone(), *declaration, path as *const _ as usize);
        let mut used = vec![false; f.strings.iter().count()];
        ProgramSymbols {
            ir: &mut f.ir,
            index: &mut f.index,
            imports: &mut f.imports,
        }
        .mark(&mut SymbolMarks(&mut used))
        .unwrap();
        assert_eq!(f.ir.decl_nodes.as_ptr(), retained.decl_nodes.as_ptr());
        assert_eq!(f.ir.nodes.as_ptr(), retained.nodes.as_ptr());
        assert_eq!(f.ir.edges.as_ptr(), retained.edges.as_ptr());
        assert_eq!(f.ir.arcs.as_ptr(), retained.arcs.as_ptr());
        let (path, declaration) = f.index.by_path.get_key_value(&path_before.0).unwrap();
        assert_eq!(*declaration, path_before.1);
        assert_eq!(
            path as *const _ as usize, path_before.2,
            "marking must not rebuild the path map"
        );
        let (alias, path) = f.index.aliases.get_key_value(&aliases_before.0).unwrap();
        assert_eq!(path, &aliases_before.1);
        assert_eq!(
            alias as *const SymId as usize, aliases_before.2,
            "marking must not rebuild the alias map"
        );
        assert!(
            SymbolMarks(&mut used)
                .symbol(&symbol, "invalid probe")
                .is_err()
        );
    }

    #[test]
    fn symbol_lifetime_covers_all_typed_symbol_positions_and_is_idempotent() {
        let mut f = Fixture::new();
        f.ir = f.ir.into_shared();
        let retained = f.ir.clone();
        let semantic = Canon::new(&f.ir, &f.strings).hash();
        let structural = Canon::new(&f.ir, &f.strings)
            .structure(CanonHashCfg::default())
            .0;
        let declaration_names =
            f.ir.decl_nodes
                .iter()
                .map(|d| f.strings.resolve(d.name).to_owned())
                .collect::<Vec<_>>();
        let compact =
            SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &f.strings).unwrap();
        assert!(compact.renumbered);
        assert_eq!(Canon::new(&retained, &f.strings).hash(), semantic);
        assert!(
            compact
                .interner
                .iter()
                .all(|(_, s)| !s.starts_with("unused-before-"))
        );
        assert_eq!(Canon::new(&f.ir, &compact.interner).hash(), semantic);
        assert_eq!(
            Canon::new(&f.ir, &compact.interner)
                .structure(CanonHashCfg::default())
                .0,
            structural
        );
        assert_eq!(
            f.ir.decl_nodes
                .iter()
                .map(|d| compact.interner.resolve(d.name).to_owned())
                .collect::<Vec<_>>(),
            declaration_names
        );
        let namespace = compact.interner.get_id("namespace").unwrap();
        let name = compact.interner.get_id("renamed-root").unwrap();
        assert_eq!(
            f.index.aliases[&compact.interner.get_id("alias").unwrap()],
            vec![namespace, name]
        );
        assert_eq!(
            f.index.module_aliases[0].1[&compact.interner.get_id("local-alias").unwrap()],
            vec![namespace, name]
        );
        assert_eq!(f.imports[0].0, namespace);
        let again =
            SymbolCompaction::apply(&mut f.ir, &mut f.index, &mut f.imports, &compact.interner)
                .unwrap();
        assert!(!again.renumbered);
        assert_eq!(
            again.interner.iter().collect::<Vec<_>>(),
            compact.interner.iter().collect::<Vec<_>>()
        );
        let bytes =
            cbor_writer::encode("model.hymeko", &f.ir, &f.index, &again.interner, &f.imports)
                .unwrap();
        let decoded = cbor_writer::decode(&bytes, &DecodeLimits::strict()).unwrap();
        assert_eq!(
            Canon::new(decoded.ir(), decoded.interner()).hash(),
            semantic
        );
        assert_eq!(decoded.canonical_hash(), structural);
        let base = CompiledModel::from_decoded_cbor(decoded);
        assert_eq!(base.semantic_digest(), semantic);
        let label = base
            .program()
            .ir
            .decls()
            .find(|(_, d)| base.interner().resolve(d.name) == "label")
            .unwrap()
            .0;
        let next = base
            .with_value_updates(&[crate::module_store::execution::ValueUpdate::Text {
                declaration: label,
                value: "new live value",
            }])
            .unwrap();
        assert_eq!(
            next.semantic_digest(),
            Canon::new(&next.program().ir, next.interner()).hash(),
            "record hashes remain valid while cached symbolic paths are remapped"
        );
        assert_eq!(base.semantic_digest(), semantic);
    }
}
