//! Shared numeric coordinates and matrix-free row operations over canonical IR.
use hymeko_tensor::operator::IndexedRow;
use std::sync::{Arc, OnceLock};

mod layout;
mod targets;
pub use layout::{IncidenceLayout, IncidenceOrder};
use targets::{ProjectionTargets, TargetRun};

use super::IncidenceIndexData;
use crate::{
    common::ids::{DeclId, HyperArcId, NodeId},
    ir::ir::{DeclKind, Ir},
};

/// Which existing canonical ID indexes an operator's input/output vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncidenceDomain {
    /// Every declaration may be a target; indices are `DeclId::raw()`.
    Declarations,
    /// All targets must be bound nodes; indices are `NodeId::raw()`.
    Nodes,
}
impl IncidenceDomain {
    pub(super) fn cache_slot(self) -> usize {
        match self {
            Self::Declarations => 0,
            Self::Nodes => 1,
        }
    }
}

/// Canonical bindings did not support the selected numeric domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncidenceProjectionError {
    /// A node's declaration kind or reverse binding is inconsistent.
    InvalidNode(NodeId),
    /// An incidence is outside the selected domain, or its backlink is invalid.
    InvalidTarget {
        /// Canonical arc containing the invalid reference.
        arc: HyperArcId,
        /// Position inside that arc, including duplicate slots.
        slot: usize,
        /// Canonical declaration referenced by the slot.
        target: DeclId,
    },
}
impl std::fmt::Display for IncidenceProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "incidence projection: {self:?}")
    }
}
impl std::error::Error for IncidenceProjectionError {}

pub use hymeko_tensor::operator::{
    OrderedAccumulator as IncidenceAccumulator, RowVisitor as IncidenceRowVisitor,
    SparseRow as IncidenceRowOperator,
};

#[derive(Debug)]
pub(super) struct ProjectionData {
    domain: IncidenceDomain,
    domain_len: usize,
    offsets: Box<[usize]>,
    uniform_arity: Option<usize>,
    targets: ProjectionTargets,
}
pub(super) type ProjectionCache = OnceLock<Result<Arc<ProjectionData>, IncidenceProjectionError>>;

impl ProjectionData {
    /// Ordered rows. Weights are supplied separately by the operator's policy.
    #[inline]
    fn rows(&self) -> impl ExactSizeIterator<Item = IncidenceRow<'_>> {
        self.offsets
            .windows(2)
            .enumerate()
            .map(|(row, range)| IncidenceRow {
                offset: range[0],
                end: range[1],
                source: self.row_source(row, range[0], range[1]),
                data: self,
            })
    }
    /// Visit every row once, in canonical order, without allocating or expanding.
    /// Selects the coordinate representation before entering the row loop.
    #[inline(always)] // Keep the storage choice outside the consumer's hot row loop.
    fn for_each_row(&self, visitor: &mut impl IncidenceRowVisitor) {
        self.for_each_row_range(0..self.offsets.len() - 1, visitor);
    }
    #[inline(always)]
    fn for_each_row_range(
        &self,
        span: std::ops::Range<usize>,
        visitor: &mut impl IncidenceRowVisitor,
    ) {
        assert!(span.start <= span.end && span.end < self.offsets.len());
        let first = span.start;
        match &self.targets {
            ProjectionTargets::Indexed(targets) => {
                if self.uniform_arity == Some(2) {
                    // The immutable row-width proof makes both bounds and pair
                    // reductions static in the consumer's loop. No ID reordering.
                    for (relative, pair) in targets
                        [self.offsets[span.start]..self.offsets[span.end]]
                        .chunks_exact(2)
                        .enumerate()
                    {
                        let row = first + relative;
                        visitor.visit(row, IndexedRow::new(row * 2, pair));
                    }
                } else {
                    for (relative, range) in
                        self.offsets[span.start..=span.end].windows(2).enumerate()
                    {
                        let row = first + relative;
                        visitor.visit(row, IndexedRow::new(range[0], &targets[range[0]..range[1]]));
                    }
                }
            }
            ProjectionTargets::Runs { rows, runs, .. } => {
                for (relative, range) in self.offsets[span.start..=span.end].windows(2).enumerate()
                {
                    let row = first + relative;
                    visitor.visit(
                        row,
                        RunRow {
                            offset: range[0],
                            len: range[1] - range[0],
                            runs: &runs[rows[row]..rows[row + 1]],
                        },
                    );
                }
            }
        }
    }
    #[inline]
    fn row_source(&self, row: usize, first: usize, end: usize) -> RowSource<'_> {
        match &self.targets {
            ProjectionTargets::Indexed(targets) => RowSource::Indexed(&targets[first..end]),
            ProjectionTargets::Runs { rows, runs, .. } => {
                RowSource::Runs(&runs[rows[row]..rows[row + 1]])
            }
        }
    }
    /// Borrow one row by canonical hyperarc ID.
    /// # Panics
    /// If `arc` is outside this model's arc domain.
    fn row(&self, arc: HyperArcId) -> IncidenceRow<'_> {
        let offset = self.offsets[arc.raw()];
        IncidenceRow {
            offset,
            end: self.offsets[arc.raw() + 1],
            source: self.row_source(arc.raw(), offset, self.offsets[arc.raw() + 1]),
            data: self,
        }
    }
    fn build(ir: &Ir, domain: IncidenceDomain) -> Result<Self, IncidenceProjectionError> {
        if domain == IncidenceDomain::Nodes {
            for (raw, node) in ir.nodes.iter().enumerate() {
                if ir.kind_of(node.decl) != Some(DeclKind::Node)
                    || ir.as_node(node.decl) != Some(NodeId::new(raw))
                {
                    return Err(IncidenceProjectionError::InvalidNode(NodeId::new(raw)));
                }
            }
        }
        let domain_len = match domain {
            IncidenceDomain::Declarations => ir.decl_nodes.len(),
            IncidenceDomain::Nodes => ir.nodes.len(),
        };
        let count = ir.arcs.iter().map(|arc| arc.refs.len()).sum();
        let mut targets = Vec::with_capacity(count);
        let mut offsets = Vec::with_capacity(ir.arcs.len() + 1);
        offsets.push(0);
        for (raw, arc) in ir.arcs.iter().enumerate() {
            for (slot, reference) in arc.refs.iter().enumerate() {
                let target = reference.target();
                let projected = match domain {
                    IncidenceDomain::Declarations => {
                        (target.raw() < domain_len).then_some(target.raw())
                    }
                    IncidenceDomain::Nodes => ir.as_node(target).and_then(|id| {
                        ir.nodes
                            .get(id.raw())
                            .filter(|node| node.decl == target)
                            .map(|_| id.raw())
                    }),
                };
                targets.push(projected.ok_or(IncidenceProjectionError::InvalidTarget {
                    arc: HyperArcId::new(raw),
                    slot,
                    target,
                })?);
            }
            offsets.push(targets.len());
        }
        let uniform_arity = offsets
            .windows(2)
            .next()
            .map(|row| row[1] - row[0])
            .filter(|&width| offsets.windows(2).all(|row| row[1] - row[0] == width));
        let targets = ProjectionTargets::build(&offsets, targets);
        Ok(Self {
            domain,
            domain_len,
            offsets: offsets.into_boxed_slice(),
            uniform_arity,
            targets,
        })
    }
    pub(super) fn retained_bytes(&self) -> usize {
        size_of_val(self.offsets.as_ref()) + self.targets.retained_bytes()
    }
}

/// Reusable numeric incidence coordinates, tied to their immutable canonical IR.
///
/// Order and duplicates are preserved. Native indices do not narrow the ID domain.
/// Only offsets and resolved target coordinates/runs are retained; no annotations, weights, records,
/// adjacency or operator product is materialized. Clones share the same allocation.
/// The immutable borrow prevents changing the model while any projection is used:
/// ```compile_fail
/// use hymeko::ir::{ir::Ir, incidence::{IncidenceDomain, IncidenceProjection}};
/// let mut ir = Ir::default();
/// let projection = IncidenceProjection::build(&ir, IncidenceDomain::Nodes).unwrap();
/// ir.arcs.clear();
/// assert_eq!(projection.arc_count(), 0);
/// ```
#[derive(Clone)]
pub struct IncidenceProjection<'ir> {
    ir: &'ir Ir,
    data: Arc<ProjectionData>,
}
impl<'ir> IncidenceProjection<'ir> {
    /// Prepare coordinates without forcing construction of a reverse index.
    /// # Errors
    /// Rejects out-of-domain targets and, for the node domain, invalid bindings.
    pub fn build(ir: &'ir Ir, domain: IncidenceDomain) -> Result<Self, IncidenceProjectionError> {
        Ok(Self {
            ir,
            data: Arc::new(ProjectionData::build(ir, domain)?),
        })
    }
    pub(super) fn cached(
        ir: &'ir Ir,
        index: &IncidenceIndexData,
        domain: IncidenceDomain,
    ) -> Result<Self, IncidenceProjectionError> {
        let data = index.projections[domain.cache_slot()]
            .get_or_init(|| ProjectionData::build(ir, domain).map(Arc::new));
        Ok(Self {
            ir,
            data: data.as_ref().map(Arc::clone).map_err(|error| *error)?,
        })
    }
    /// The exact immutable model whose bindings were resolved.
    pub fn ir(&self) -> &'ir Ir {
        self.ir
    }
    /// Canonical ID domain used by the numeric vectors.
    pub fn domain(&self) -> IncidenceDomain {
        self.data.domain
    }
    /// Required vector length, including isolated domain elements.
    pub fn domain_len(&self) -> usize {
        self.data.domain_len
    }
    /// Number of hyperarc rows, including empty rows.
    pub fn arc_count(&self) -> usize {
        self.data.offsets.len() - 1
    }
    /// Number of incidence slots, including repeated targets.
    pub fn incidence_count(&self) -> usize {
        self.data.offsets.last().copied().unwrap_or(0)
    }
    /// Number of lossless contiguous ranges in the stored coordinate view.
    /// Indexed slots are singleton ranges. This counts metadata without expansion.
    pub fn range_count(&self) -> usize {
        self.data.targets.range_count()
    }
    /// Retained coordinate-buffer bytes; shared clones do not allocate them again.
    pub fn retained_bytes(&self) -> usize {
        self.data.retained_bytes()
    }
    /// Build a numeric execution view while preserving canonical identifiers.
    /// The view retains the immutable IR borrow:
    /// ```compile_fail
    /// use hymeko::ir::{ir::Ir, incidence::{IncidenceDomain, IncidenceOrder, IncidenceProjection}};
    /// let mut ir = Ir::default();
    /// let layout = IncidenceProjection::build(&ir, IncidenceDomain::Nodes).unwrap()
    ///     .layout(IncidenceOrder::FirstEncounter);
    /// ir.nodes.clear();
    /// assert_eq!(layout.projection().domain_len(), 0);
    /// ```
    pub fn layout(&self, order: IncidenceOrder) -> IncidenceLayout<'ir> {
        IncidenceLayout::new(self.clone(), order)
    }
    /// Ordered rows in canonical coordinates.
    #[inline]
    pub fn rows(&self) -> impl ExactSizeIterator<Item = IncidenceRow<'_>> {
        self.data.rows()
    }
    /// Visit rows through the shared coordinate representation dispatch.
    #[inline(always)]
    pub fn for_each_row(&self, visitor: &mut impl IncidenceRowVisitor) {
        self.data.for_each_row(visitor);
    }
    /// Visit a bounded row interval with global row and coefficient ordinals.
    /// Shares the full traversal's dispatch and pair specialization.
    /// # Panics
    /// If the interval is reversed or extends beyond the arc count.
    #[inline(always)]
    pub fn for_each_row_range(
        &self,
        span: std::ops::Range<usize>,
        visitor: &mut impl IncidenceRowVisitor,
    ) {
        self.data.for_each_row_range(span, visitor);
    }
    /// Borrow one row by canonical hyperarc ID.
    /// # Panics
    /// If the arc is outside the model.
    pub fn row(&self, arc: HyperArcId) -> IncidenceRow<'_> {
        self.data.row(arc)
    }
}

/// One borrowed sparse row. Coefficient positions are global arc-major slots,
/// shared by both directions; duplicate slots are never collapsed.
#[derive(Clone, Copy)]
pub struct IncidenceRow<'a> {
    offset: usize,
    end: usize,
    source: RowSource<'a>,
    data: &'a ProjectionData,
}
#[derive(Clone, Copy)]
enum RowSource<'a> {
    Indexed(&'a [usize]),
    Runs(&'a [TargetRun]),
}
impl<'a> IncidenceRow<'a> {
    /// Beginning of this row in an arc-major coefficient array.
    pub fn offset(self) -> usize {
        self.offset
    }
    /// Numeric targets in the parent projection or execution layout's coordinate space.
    /// A compressed projection expands its shared slice on the first explicit
    /// slice request. Use `iter_targets`, `gather` or `scatter` to keep it virtual.
    #[inline]
    pub fn targets(self) -> &'a [usize] {
        match self.source {
            RowSource::Indexed(targets) => targets,
            RowSource::Runs(_) => &self.data.targets.as_slice()[self.offset..self.end],
        }
    }
    /// Number of ordered incidence slots, including repeated targets.
    #[inline]
    pub fn len(self) -> usize {
        self.end - self.offset
    }
    /// Whether the row has no incidence slots.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.offset == self.end
    }
    #[inline]
    fn runs(self) -> Option<&'a [TargetRun]> {
        match self.source {
            RowSource::Indexed(_) => None,
            RowSource::Runs(runs) => Some(runs),
        }
    }
    /// Traverse the parent view's coordinates in incidence order without expansion.
    /// Duplicates remain distinct slots; no allocation occurs.
    #[inline]
    pub fn iter_targets(self) -> impl Iterator<Item = usize> + 'a {
        let indexed = match self.source {
            RowSource::Indexed(data) => Some(data),
            RowSource::Runs(_) => None,
        };
        indexed.into_iter().flatten().copied().chain(
            self.runs()
                .into_iter()
                .flatten()
                .flat_map(|run| run.indices()),
        )
    }
    /// Apply this sparse row to `input`, in declaration order, with no allocation.
    /// # Panics
    /// If an input target or a coefficient requested from the policy is missing.
    #[inline(always)] // The caller's row representation must specialize this branch.
    pub fn gather<A: IncidenceAccumulator>(
        self,
        input: &[f64],
        coefficient: impl Fn(usize) -> f64,
    ) -> f64 {
        match self.source {
            RowSource::Indexed(targets) => {
                IndexedRow::new(self.offset, targets).gather::<A>(input, coefficient)
            }
            RowSource::Runs(runs) => RunRow {
                offset: self.offset,
                len: self.len(),
                runs,
            }
            .gather::<A>(input, coefficient),
        }
    }
    /// Apply the transpose of this row, adding into caller-owned accumulators.
    /// This does not clear `output`; callers can fuse a factorized operator row
    /// by row, keeping only one intermediate scalar instead of a full arc vector.
    /// # Panics
    /// If an output target or a coefficient requested from the policy is missing.
    #[inline(always)]
    pub fn scatter<A: IncidenceAccumulator>(
        self,
        value: f64,
        output: &mut [A],
        coefficient: impl Fn(usize) -> f64,
    ) {
        match self.source {
            RowSource::Indexed(targets) => {
                IndexedRow::new(self.offset, targets).scatter(value, output, coefficient)
            }
            RowSource::Runs(runs) => RunRow {
                offset: self.offset,
                len: self.len(),
                runs,
            }
            .scatter(value, output, coefficient),
        }
    }
}

impl IncidenceRowOperator for IncidenceRow<'_> {
    fn for_each_range(&self, visit: impl FnMut(std::ops::Range<usize>)) {
        match self.source {
            RowSource::Indexed(targets) => {
                IndexedRow::new(self.offset, targets).for_each_range(visit);
            }
            RowSource::Runs(runs) => RunRow {
                offset: self.offset,
                len: self.len(),
                runs,
            }
            .for_each_range(visit),
        }
    }
    fn offset(&self) -> usize {
        (*self).offset()
    }
    fn len(&self) -> usize {
        (*self).len()
    }
    fn iter_targets(&self) -> impl Iterator<Item = usize> {
        (*self).iter_targets()
    }
    #[inline(always)]
    fn gather<A: IncidenceAccumulator>(
        &self,
        input: &[f64],
        coefficient: impl Fn(usize) -> f64,
    ) -> f64 {
        (*self).gather::<A>(input, coefficient)
    }
    #[inline(always)]
    fn scatter<A: IncidenceAccumulator>(
        &self,
        value: f64,
        output: &mut [A],
        coefficient: impl Fn(usize) -> f64,
    ) {
        (*self).scatter(value, output, coefficient);
    }
}

struct RunRow<'a> {
    offset: usize,
    len: usize,
    runs: &'a [TargetRun],
}
impl IncidenceRowOperator for RunRow<'_> {
    fn for_each_range(&self, mut visit: impl FnMut(std::ops::Range<usize>)) {
        for run in self.runs {
            run.for_each_range(&mut visit);
        }
    }
    fn offset(&self) -> usize {
        self.offset
    }
    fn len(&self) -> usize {
        self.len
    }
    fn iter_targets(&self) -> impl Iterator<Item = usize> {
        self.runs.iter().flat_map(|run| run.indices())
    }
    #[inline(always)]
    fn gather<A: IncidenceAccumulator>(
        &self,
        input: &[f64],
        coefficient: impl Fn(usize) -> f64,
    ) -> f64 {
        if self.len == 2 {
            let mut targets = self.iter_targets();
            // Validated row length is exactly two, including duplicate slots.
            let [Some(first), Some(second)] = [targets.next(), targets.next()] else {
                unreachable!("two-slot projection row");
            };
            return A::sum_pair(
                || input[first] * coefficient(self.offset),
                || input[second] * coefficient(self.offset + 1),
            );
        }
        A::sum_chunks(self.runs.iter().scan(self.offset, |offset, run| {
            let first = *offset;
            *offset += run.len;
            let coefficient = &coefficient;
            Some(
                run.values(input)
                    .enumerate()
                    .map(move |(slot, value)| value * coefficient(first + slot)),
            )
        }))
    }
    #[inline(always)]
    fn scatter<A: IncidenceAccumulator>(
        &self,
        value: f64,
        output: &mut [A],
        coefficient: impl Fn(usize) -> f64,
    ) {
        let mut slot = self.offset;
        for run in self.runs {
            let end = run
                .first
                .checked_add(run.len)
                .filter(|&end| run.step == 1 && end <= output.len());
            if let Some(end) = end {
                for accumulator in &mut output[run.first..end] {
                    accumulator.add(value * coefficient(slot));
                    slot += 1;
                }
            } else {
                for target in run.indices() {
                    output[target].add(value * coefficient(slot));
                    slot += 1;
                }
            }
        }
    }
}

pub(super) struct ForwardOperator<'ir> {
    pub(super) projection: IncidenceProjection<'ir>,
    pub(super) coefficients: Box<[f64]>,
}

impl ForwardOperator<'_> {
    pub(super) fn apply(&self, input: &[f64], output: &mut [f64]) {
        self.projection
            .for_each_row(&mut hymeko_tensor::operator::RowGather::new(
                input,
                output,
                |slot| self.coefficients[slot],
                |_, value| value,
            ));
    }
}
