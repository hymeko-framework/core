//! Explicitly aligned contractions of two borrowed canonical-IR spaces.
use super::{IncidenceProjection, IncidenceRowOperator, IncidenceRowVisitor};
use crate::common::ids::HyperArcId;
use hymeko_tensor::operator::{OrderedAccumulator, RowContraction, RowTransform};

/// An explicit alignment or vector does not fit its borrowed domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAlignmentError {
    /// There must be exactly one source row per destination row.
    Length { expected: usize, actual: usize },
    /// A source row does not exist in the source IR.
    Source {
        destination: usize,
        source: HyperArcId,
    },
    /// Input/output vectors do not cover their respective domains.
    Vectors,
}
impl std::fmt::Display for RowAlignmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "incidence alignment: {self:?}")
    }
}
impl std::error::Error for RowAlignmentError {}

/// `B_destination^T D P B_source`, with explicit row selection `P`.
///
/// Selection may repeat/omit source rows; it is not assumed to be a bijection.
/// IDs in different IRs acquire meaning only through this caller-supplied map.
/// Arbitrary many-to-many `P` is a separate composition, not silently inferred.
/// No IR, incidence table, product matrix or intermediate vector is copied.
pub struct IncidenceContraction<'a, 'ir> {
    source: &'a IncidenceProjection<'ir>,
    destination: &'a IncidenceProjection<'ir>,
    source_for_destination: &'a [HyperArcId],
}
impl<'a, 'ir> IncidenceContraction<'a, 'ir> {
    /// Validate the whole map before evaluation. Structural borrows prevent edits
    /// to either IR while the contraction lives.
    pub fn build(
        source: &'a IncidenceProjection<'ir>,
        destination: &'a IncidenceProjection<'ir>,
        source_for_destination: &'a [HyperArcId],
    ) -> Result<Self, RowAlignmentError> {
        if source_for_destination.len() != destination.arc_count() {
            return Err(RowAlignmentError::Length {
                expected: destination.arc_count(),
                actual: source_for_destination.len(),
            });
        }
        for (row, &arc) in source_for_destination.iter().enumerate() {
            if arc.raw() >= source.arc_count() {
                return Err(RowAlignmentError::Source {
                    destination: row,
                    source: arc,
                });
            }
        }
        Ok(Self {
            source,
            destination,
            source_for_destination,
        })
    }
    /// Apply in destination-row order, accumulating into the visitor's output.
    /// Vector extents are checked before the first write. Numeric policy failures
    /// retain the underlying ordered row contract.
    pub fn apply<
        A: OrderedAccumulator,
        S: Fn(usize) -> f64,
        D: Fn(usize) -> f64,
        T: RowTransform,
    >(
        &self,
        contraction: &mut RowContraction<'_, A, S, D, T>,
    ) -> Result<(), RowAlignmentError> {
        let (input, output) = contraction.dimensions();
        if input < self.source.domain_len() || output < self.destination.domain_len() {
            return Err(RowAlignmentError::Vectors);
        }
        self.destination.for_each_row(&mut MappedRows {
            alignment: self,
            contraction,
        });
        Ok(())
    }
}
struct MappedRows<'a, 'b, 'c, 'ir, A, S, D, T> {
    alignment: &'a IncidenceContraction<'b, 'ir>,
    contraction: &'a mut RowContraction<'c, A, S, D, T>,
}
impl<A: OrderedAccumulator, S: Fn(usize) -> f64, D: Fn(usize) -> f64, T: RowTransform>
    IncidenceRowVisitor for MappedRows<'_, '_, '_, '_, A, S, D, T>
{
    fn visit<R: IncidenceRowOperator>(&mut self, row: usize, destination: R) {
        let source = self
            .alignment
            .source
            .row(self.alignment.source_for_destination[row]);
        self.contraction.pair(row, &source, &destination);
    }
}
