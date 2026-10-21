//! P2.1 V&V for the canonical incidence backbone (APPROVED-CORE-EDIT:
//! canonical-incidence-backbone). Proves the owned [`PreparedIr`] bundle + borrowed
//! [`IncidenceView`] / [`StarOperator`] are an exact, byte-identical replacement for
//! the borrowed [`IncidenceIndex`] path, that the derived index is *rebuilt* from the
//! canonical `Ir` (never carried in the wire), and that view creation is `O(1)` /
//! zero-allocation. Wired into `tests/mod.rs` (autotests=false).
//!
//! Coverage map (mission §7): old-vs-new API differential (B / Bᵀ / BᵀB, resolve,
//! incident_arcs/slots, degree) · owned-bundle lifecycle (new / ir / index / into_ir)
//! · star-operator handle parity · prepared-plan parity via the view · index-rebuild
//! parity from a cloned `Ir` (deserialize stand-in) · derived-index-not-in-wire
//! (structural) · zero-alloc view creation (structural: `Copy`, references-only) ·
//! wrong-owner fault mutant (bijection rejects a foreign `Ir`) · 0/1/2/63/64/65 and
//! large boundary sizes.

use std::mem::{size_of, size_of_val};

use hymeko::common::ids::DeclId;
use hymeko::ir::incidence::{
    IncidenceAccumulator, IncidenceDomain, IncidenceIndex, IncidenceIndexData, IncidenceProjection,
    IncidenceProjectionError, IncidenceRowOperator, IncidenceRowVisitor, IncidenceView, PreparedIr,
    StarOperator, build_incidence_index,
};
use hymeko::ir::ir::Ir;

use super::incidence_support::{arc, bits_eq, median_ms, node, r, windowed};

#[test]
fn partitioned_projection_and_generic_transpose_preserve_global_slots() {
    use hymeko::common::ids::HyperArcId;
    use hymeko::tensor::operator::RowGather;
    for rows in [
        vec![vec![0, 1], vec![1, 1], vec![2, 3]],
        vec![vec![], vec![3, 0, 3], vec![2]],
        (0..3).map(|start| (start..start + 32).collect()).collect(),
    ] {
        let mut ir = Ir::default();
        for _ in 0..40 {
            node(&mut ir);
        }
        for row in &rows {
            arc(&mut ir, row.iter().map(|&n| r(n, 1, None)).collect());
        }
        let projection = IncidenceProjection::build(&ir, IncidenceDomain::Nodes).unwrap();
        let input: Vec<_> = (1..=40).map(|n| n as f64).collect();
        let mut actual = vec![0.0; rows.len()];
        for span in [0..1, 1..1, 1..rows.len()] {
            let first = span.start;
            let mut gather = RowGather::new(
                &input,
                &mut actual[span.clone()],
                |slot| (slot + 1) as f64,
                |_, value| value,
            )
            .starting_at(first);
            projection.for_each_row_range(span, &mut gather);
        }
        let mut offset = 0;
        let expected: Vec<f64> = rows
            .iter()
            .map(|row| {
                let sum = row
                    .iter()
                    .enumerate()
                    .map(|(slot, &node)| input[node] * (offset + slot + 1) as f64)
                    .sum();
                offset += row.len();
                sum
            })
            .collect();
        assert_eq!(actual, expected);
        let index = build_incidence_index(&ir).unwrap();
        for node in 0..40 {
            let declaration = ir.nodes[node].decl;
            let slots = index.incident_slots(declaration);
            let value = index.transpose_row::<f64>(declaration, &actual, |ordinal, arc| {
                (projection.row(arc).offset() + slots[ordinal] as usize + 1) as f64
            });
            let mut oracle = 0.0;
            let mut offset = 0;
            for (row, members) in rows.iter().enumerate() {
                for (slot, &target) in members.iter().enumerate() {
                    if target == node {
                        oracle += actual[row] * (offset + slot + 1) as f64;
                    }
                }
                offset += members.len();
            }
            assert_eq!(value, oracle);
        }
        let mut output = [0.0];
        let mut gather = RowGather::new(&input, &mut output, |_| 1.0, |_, value| value);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || projection.for_each_row_range(rows.len()..rows.len() + 1, &mut gather)
            ))
            .is_err()
        );
        assert_eq!(projection.row(HyperArcId::new(0)).len(), rows[0].len());
    }
}

#[test]
fn execution_layout_preserves_incidence_slots_and_relabeling() {
    use hymeko::ir::incidence::IncidenceOrder;
    use hymeko::tensor::operator::RowContraction;
    let logical_rows = [vec![5, 1, 5, 3], vec![], vec![3, 0, 1]];
    let mut execution_rows = Vec::new();
    for permutation in [[0, 1, 2, 3, 4, 5, 6], [3, 6, 0, 5, 2, 1, 4]] {
        let mut ir = Ir::default();
        for _ in 0..7 {
            node(&mut ir);
        }
        for row in &logical_rows {
            arc(
                &mut ir,
                row.iter().map(|&n| r(permutation[n], 1, None)).collect(),
            );
        }
        let projection = IncidenceProjection::build(&ir, IncidenceDomain::Nodes).unwrap();
        let identity = projection.layout(IncidenceOrder::Canonical);
        assert_eq!(identity.order(), IncidenceOrder::Canonical);
        assert_eq!(identity.additional_bytes(), 0);
        assert!(identity.canonical_indices().is_none());
        let layout = projection.layout(IncidenceOrder::FirstEncounter);
        assert_eq!(layout.order(), IncidenceOrder::FirstEncounter);
        assert!(layout.additional_bytes() > 0);
        assert_eq!(layout.projection().incidence_count(), 7);
        assert_eq!(layout.range_count(), 7); // Short rows retain indexed storage.
        let mut mapping = layout.canonical_indices().unwrap().to_vec();
        mapping.sort_unstable();
        assert_eq!(mapping, (0..7).collect::<Vec<_>>());
        for (original, (unchanged, mapped)) in
            projection.rows().zip(identity.rows().zip(layout.rows()))
        {
            assert_eq!(original.targets(), unchanged.targets());
            assert_eq!(original.offset(), mapped.offset());
            assert_eq!(
                original.iter_targets().collect::<Vec<_>>(),
                mapped
                    .iter_targets()
                    .map(|n| layout.canonical_index(n))
                    .collect::<Vec<_>>()
            );
        }
        execution_rows.push(
            layout
                .rows()
                .map(|row| row.iter_targets().collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        );
        let input: Vec<_> = (0..7).map(|n| (n + 1) as f64).collect();
        let ordered_input: Vec<_> = (0..7).map(|n| input[layout.canonical_index(n)]).collect();
        let mut canonical_output = vec![0.0; 7];
        let mut ordered_output = vec![0.0; 7];
        projection.for_each_row(&mut RowContraction::new(
            &input,
            &mut canonical_output,
            |slot| (slot + 1) as f64,
            |_| 1.0,
            |_, value| value,
        ));
        layout.for_each_row(&mut RowContraction::new(
            &ordered_input,
            &mut ordered_output,
            |slot| (slot + 1) as f64,
            |_| 1.0,
            |_, value| value,
        ));
        for (coordinate, &value) in ordered_output.iter().enumerate() {
            assert_eq!(value, canonical_output[layout.canonical_index(coordinate)]);
        }
    }
    assert_eq!(execution_rows[0], execution_rows[1]);
    let empty = Ir::default();
    let layout = IncidenceProjection::build(&empty, IncidenceDomain::Nodes)
        .unwrap()
        .layout(IncidenceOrder::FirstEncounter);
    assert_eq!(layout.rows().len(), 0);
    assert_eq!(layout.canonical_indices(), Some([].as_slice()));
}

#[test]
#[should_panic]
fn execution_layout_rejects_out_of_domain_coordinate() {
    use hymeko::ir::incidence::IncidenceOrder;
    let ir = Ir::default();
    IncidenceProjection::build(&ir, IncidenceDomain::Nodes)
        .unwrap()
        .layout(IncidenceOrder::Canonical)
        .canonical_index(0);
}

#[test]
fn execution_layout_recovers_virtual_ranges_without_replacing_canonical_coordinates() {
    use hymeko::ir::incidence::IncidenceOrder;
    let mut ir = Ir::default();
    for _ in 0..128 {
        node(&mut ir);
    }
    for start in 0..16 {
        arc(
            &mut ir,
            (start..start + 32)
                .map(|v| r((v * 37) % 128, 1, None))
                .collect(),
        );
    }
    let canonical = IncidenceProjection::build(&ir, IncidenceDomain::Nodes).unwrap();
    let original_bytes = canonical.retained_bytes();
    let layout = canonical.layout(IncidenceOrder::FirstEncounter);
    assert_eq!(layout.range_count(), 16);
    assert!(layout.range_count() < canonical.range_count());
    for (ordinal, row) in layout.rows().enumerate() {
        let mut ranges = Vec::new();
        row.for_each_range(|range| ranges.push(range));
        assert_eq!(ranges, [ordinal..ordinal + 32]);
    }
    assert_eq!(canonical.retained_bytes(), original_bytes);
    assert_eq!(
        canonical
            .row(hymeko::common::ids::HyperArcId::new(0))
            .iter_targets()
            .nth(1),
        Some(37)
    );
}

#[test]
fn two_ir_contraction_obeys_explicit_alignment_and_signed_multiplicity() {
    use hymeko::common::ids::HyperArcId;
    use hymeko::ir::incidence::IncidenceContraction;
    use hymeko::tensor::operator::RowContraction;
    let mut source = Ir::default();
    for _ in 0..3 {
        node(&mut source);
    }
    arc(
        &mut source,
        vec![r(0, 1, None), r(1, -1, None), r(1, 1, None)],
    );
    arc(&mut source, vec![r(2, -1, None)]);
    let mut destination = Ir::default();
    for _ in 0..2 {
        node(&mut destination);
    }
    arc(
        &mut destination,
        vec![r(0, 1, None), r(1, -1, None), r(1, 1, None)],
    );
    arc(&mut destination, vec![r(0, 1, None)]);
    let a = IncidenceProjection::build(&source, IncidenceDomain::Nodes).unwrap();
    let b = IncidenceProjection::build(&destination, IncidenceDomain::Nodes).unwrap();
    let map = [HyperArcId::new(1), HyperArcId::new(0)];
    let operator = IncidenceContraction::build(&a, &b, &map).unwrap();
    let mut output = [0.0, 0.0];
    let mut evaluation = RowContraction::new(
        &[2.0, 3.0, 5.0],
        &mut output,
        |i| [1.0, -1.0, 2.0, -3.0][i],
        |i| [2.0, -1.0, 1.0, 4.0][i],
        |row, value| value * [0.5, 2.0][row],
    );
    operator.apply(&mut evaluation).unwrap();
    // Source gathers [5,-15], map swaps them; signed destination duplicate
    // cancels, and the second destination row contributes 5*2*4 to node zero.
    assert_eq!(output, [25.0, 0.0]);
    let mut repeated = [1.0, 1.0];
    operator
        .apply(&mut RowContraction::new(
            &[2.0, 3.0, 5.0],
            &mut repeated,
            |i| [1.0, -1.0, 2.0, -3.0][i],
            |i| [2.0, -1.0, 1.0, 4.0][i],
            |row, value| value * [0.5, 2.0][row],
        ))
        .unwrap();
    assert_eq!(
        repeated,
        [26.0, 1.0],
        "contraction accumulates rather than clears"
    );
}

#[test]
fn two_ir_alignment_rejects_invalid_maps_and_short_vectors_before_writes() {
    use hymeko::common::ids::HyperArcId;
    use hymeko::ir::incidence::{IncidenceContraction, RowAlignmentError};
    use hymeko::tensor::operator::RowContraction;
    let ir = windowed(4, 2);
    let projection = IncidenceProjection::build(&ir, IncidenceDomain::Nodes).unwrap();
    assert!(matches!(
        IncidenceContraction::build(&projection, &projection, &[]),
        Err(RowAlignmentError::Length { .. })
    ));
    assert!(matches!(
        IncidenceContraction::build(
            &projection,
            &projection,
            &[HyperArcId::new(0), HyperArcId::new(0), HyperArcId::new(99)]
        ),
        Err(RowAlignmentError::Source { destination: 2, .. })
    ));
    let map = [HyperArcId::new(0); 3];
    let aligned = IncidenceContraction::build(&projection, &projection, &map).unwrap();
    let mut output = [9.0; 4];
    assert_eq!(
        aligned.apply(&mut RowContraction::new(
            &[],
            &mut output,
            |_| 1.0,
            |_| 1.0,
            |_, v| v
        )),
        Err(RowAlignmentError::Vectors)
    );
    assert_eq!(output, [9.0; 4]);
    let empty = Ir::default();
    let e = IncidenceProjection::build(&empty, IncidenceDomain::Nodes).unwrap();
    IncidenceContraction::build(&e, &e, &[])
        .unwrap()
        .apply(&mut RowContraction::<f64, _, _, _>::new(
            &[],
            &mut [],
            |_| 1.0,
            |_| 1.0,
            |_, v| v,
        ))
        .unwrap();
}

#[test]
fn ordered_reduction_seeds_once_and_preserves_empty_zero_and_cancellation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEEDS: AtomicUsize = AtomicUsize::new(0);
    #[derive(Default)]
    struct Probe(f64);
    impl IncidenceAccumulator for Probe {
        fn from_first(value: f64) -> Self {
            SEEDS.fetch_add(1, Ordering::Relaxed);
            Self(0.0 + value)
        }
        fn add(&mut self, value: f64) {
            self.0 += value;
        }
        fn total(&self) -> f64 {
            self.0
        }
    }
    let cases: &[&[f64]] = &[&[], &[-0.0], &[1.0], &[1e16, 1.0, -1e16], &[2.0, 3.0]];
    for &values in cases {
        let expected = values.iter().fold(0.0, |sum, &v| sum + v);
        assert_eq!(
            Probe::sum(values.iter().copied()).to_bits(),
            expected.to_bits()
        );
        assert_eq!(
            f64::sum(values.iter().copied()).to_bits(),
            expected.to_bits()
        );
        assert_eq!(
            f64::sum_chunks(values.chunks(2).map(|chunk| chunk.iter().copied())).to_bits(),
            expected.to_bits()
        );
    }
    assert_eq!(SEEDS.load(Ordering::Relaxed), cases.len() - 1);
}

struct ProjectionCase {
    ir: Ir,
    owner: DeclId,
}

struct RowVisitCheck<'a> {
    expected: &'a [Vec<usize>],
    visits: usize,
    offset: usize,
}
impl IncidenceRowVisitor for RowVisitCheck<'_> {
    fn visit<R: IncidenceRowOperator>(&mut self, index: usize, row: R) {
        assert_eq!(index, self.visits);
        assert_eq!(row.offset(), self.offset);
        assert_eq!(row.len(), self.expected[index].len());
        assert_eq!(row.is_empty(), self.expected[index].is_empty());
        assert_eq!(row.iter_targets().collect::<Vec<_>>(), self.expected[index]);
        self.offset += row.len();
        self.visits += 1;
    }
}

#[test]
fn pair_gather_honors_overridden_sum_and_lazy_coefficient_order() {
    use hymeko::common::ids::HyperArcId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static STAGE: AtomicUsize = AtomicUsize::new(0);
    #[derive(Default)]
    struct Override;
    impl IncidenceAccumulator for Override {
        fn add(&mut self, _: f64) {
            panic!("the sum override must own the reduction");
        }
        fn total(&self) -> f64 {
            panic!("the sum override must own the reduction");
        }
        fn sum(terms: impl IntoIterator<Item = f64>) -> f64 {
            assert_eq!(STAGE.swap(1, Ordering::Relaxed), 0);
            terms.into_iter().sum()
        }
    }
    let case = ProjectionCase::pair();
    let projection = IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes).unwrap();
    let result = projection
        .row(HyperArcId::new(1))
        .gather::<Override>(&[3.0, 5.0], |slot| {
            assert_eq!(STAGE.swap(slot - 1, Ordering::Relaxed), slot - 2);
            [2.0, -4.0][slot - 3]
        });
    assert_eq!(result, -14.0);
    assert_eq!(STAGE.load(Ordering::Relaxed), 3);
}

#[test]
fn pair_scatter_keeps_duplicate_order_and_partial_writes() {
    use hymeko::common::ids::HyperArcId;
    let mut case = ProjectionCase::pair();
    case.ir.arcs[1].refs[1] = case.ir.arcs[1].refs[1].with_target(case.ir.arcs[1].refs[0].target());
    let projection = IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes).unwrap();
    let row = projection.row(HyperArcId::new(1));
    let mut output = [1e16];
    row.scatter(1.0, &mut output, |slot| [-1e16, 1.0][slot - 3]);
    assert_eq!(output, [1.0], "duplicate writes must remain sequential");

    let case = ProjectionCase::pair();
    let projection = IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes).unwrap();
    let row = projection.row(HyperArcId::new(1));
    let calls = std::cell::RefCell::new(Vec::new());
    let mut output = [0.0];
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        row.scatter(2.0, &mut output, |slot| {
            calls.borrow_mut().push(slot);
            1.0
        });
    }));
    assert!(failure.is_err());
    assert_eq!(output, [2.0]);
    assert_eq!(*calls.borrow(), [3]);
}
impl ProjectionCase {
    fn with_rows(nodes: usize, rows: &[Vec<usize>]) -> Self {
        use hymeko::common::ids::SymId;
        use hymeko::ir::ir::{AnnoR, ArcPlacement, IrConstruction};
        let mut ir = Ir::default();
        let mut builder = IrConstruction::new(&mut ir);
        for _ in 0..nodes {
            builder
                .append_node(SymId::new(0), AnnoR::default(), vec![])
                .unwrap();
        }
        let owner = builder
            .append_edge(SymId::new(0), AnnoR::default(), vec![])
            .unwrap();
        for row in rows {
            builder
                .append_arc(
                    ArcPlacement::under_owner(owner),
                    AnnoR::default(),
                    row.iter().map(|&target| r(target, 1, None)).collect(),
                )
                .unwrap();
        }
        Self { ir, owner }
    }
    fn pair() -> Self {
        let mut case = Self::new();
        case.ir.arcs[1].refs = case.ir.arcs[0].refs[..2].to_vec();
        case
    }
    fn new() -> Self {
        use hymeko::common::ids::SymId;
        use hymeko::ir::ir::{AnnoR, ArcPlacement, IrConstruction};
        let mut ir = Ir::default();
        let mut builder = IrConstruction::new(&mut ir);
        let first = builder
            .append_node(SymId::new(0), AnnoR::default(), vec![])
            .unwrap();
        let owner = builder
            .append_edge(SymId::new(1), AnnoR::default(), vec![])
            .unwrap();
        let second = builder
            .append_node(SymId::new(2), AnnoR::default(), vec![])
            .unwrap();
        builder
            .append_arc(
                ArcPlacement::under_owner(owner),
                AnnoR::default(),
                vec![
                    r(first.raw(), 1, None),
                    r(second.raw(), -1, Some(vec![2.0])),
                    r(first.raw(), 1, Some(vec![3.0])),
                ],
            )
            .unwrap();
        builder
            .append_arc(ArcPlacement::under_owner(owner), AnnoR::default(), vec![])
            .unwrap();
        Self { ir, owner }
    }
}

#[test]
fn virtual_rows_preserve_ordered_operations_and_expand_only_on_slice_request() {
    let mut rows = vec![(0..64).collect::<Vec<_>>(); 8];
    rows.extend([
        (0..64).rev().collect(),
        vec![0, 126],
        vec![3; 64],
        std::iter::once(127).chain(0..64).collect(),
        (64..128).chain(0..64).collect(),
        vec![],
        vec![7],
        vec![1, 1],
        vec![0, 3, 8, 5, 32, 2, 63, 1],
    ]);
    let case = ProjectionCase::with_rows(128, &rows);
    let projection = IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes).unwrap();
    let count = rows.iter().map(Vec::len).sum::<usize>();
    let compressed_bytes = projection.retained_bytes();
    assert!(compressed_bytes < (rows.len() + 1 + count) * size_of::<usize>());
    assert_eq!(projection.incidence_count(), count);
    let input: Vec<_> = (0..128)
        .map(|i| [1e16, 1.0, -1e16, -0.0, f64::MIN_POSITIVE][i % 5])
        .collect();
    let coefficient = |slot: usize| [1.0, -2.0, 0.5][slot % 3];
    let mut output = vec![0.0; 128];
    let mut expected_output = output.clone();
    let mut offset = 0;
    for (row, expected) in projection.rows().zip(&rows) {
        assert_eq!(row.len(), expected.len());
        assert_eq!(row.is_empty(), expected.is_empty());
        assert_eq!(row.offset(), offset);
        assert_eq!(row.iter_targets().collect::<Vec<_>>(), *expected);
        let wanted = expected.iter().enumerate().fold(0.0, |sum, (slot, &node)| {
            sum + input[node] * coefficient(offset + slot)
        });
        assert_eq!(
            row.gather::<f64>(&input, coefficient).to_bits(),
            wanted.to_bits()
        );
        row.scatter(2.0, &mut output, coefficient);
        for (slot, &node) in expected.iter().enumerate() {
            expected_output[node] += 2.0 * coefficient(offset + slot);
        }
        offset += expected.len();
    }
    assert!(bits_eq(&output, &expected_output));
    let mut visitor = RowVisitCheck {
        expected: &rows,
        visits: 0,
        offset: 0,
    };
    projection.for_each_row(&mut visitor);
    assert_eq!(visitor.visits, rows.len());
    // Exercise Tensor's interval contraction through the real compressed row
    // adapter, including reversed runs, duplicates, singletons and empty rows.
    // Tensor's literal-row tests cover the shared indexed/singleton adapter.
    {
        use hymeko::tensor::operator::{RangePartition, RangeWorkspace};
        let partition = RangePartition::new(128, std::iter::once(0..128).collect()).unwrap();
        let input: Vec<_> = (0..128).map(|i| (i % 7) as f64 - 3.0).collect();
        let mut workspace = RangeWorkspace::<f64>::default();
        workspace.prepare(&input, &partition);
        projection.for_each_row(&mut workspace.contraction(&partition, |_, value| value));
        let mut actual = vec![1.0; 128];
        workspace.accumulate_into(&partition, &mut actual);
        let mut expected = vec![1.0; 128];
        for row in &rows {
            let sum: f64 = row.iter().map(|&target| input[target]).sum();
            for &target in row {
                expected[target] += sum;
            }
        }
        assert_eq!(actual, expected);
    }
    assert_eq!(
        projection.retained_bytes(),
        compressed_bytes,
        "operators must stay virtual"
    );
    let cloned = projection.clone();
    for ((row, other), expected) in projection.rows().zip(cloned.rows()).zip(&rows) {
        assert_eq!(row.targets(), expected);
        assert_eq!(row.targets().as_ptr(), other.targets().as_ptr());
    }
    assert_eq!(
        projection.retained_bytes(),
        compressed_bytes + count * size_of::<usize>()
    );
}

#[test]
fn virtual_rows_keep_lazy_bounds_checks_and_partial_writes() {
    use hymeko::common::ids::HyperArcId;
    let rows = vec![(0..64).collect::<Vec<_>>(); 8];
    let case = ProjectionCase::with_rows(64, &rows);
    let projection = IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes).unwrap();
    let row = projection.row(HyperArcId::new(0));
    let calls = std::cell::Cell::new(0);
    let coefficient = |_| {
        calls.set(calls.get() + 1);
        1.0
    };
    let mut output = [0.0; 16];
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            row.scatter(2.0, &mut output, coefficient);
        }))
        .is_err()
    );
    assert_eq!(output, [2.0; 16]);
    assert_eq!(calls.replace(0), 16);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            row.gather::<f64>(&output, coefficient);
        }))
        .is_err()
    );
    assert_eq!(calls.get(), 16);
    #[derive(Default)]
    struct Ignore;
    impl IncidenceAccumulator for Ignore {
        fn add(&mut self, _: f64) {
            unreachable!()
        }
        fn total(&self) -> f64 {
            unreachable!()
        }
        fn sum(_: impl IntoIterator<Item = f64>) -> f64 {
            42.0
        }
    }
    assert_eq!(
        row.gather::<Ignore>(&[], |_| panic!("must remain lazy")),
        42.0
    );
}

#[test]
fn irregular_and_pair_rows_keep_the_original_index_storage() {
    let irregular = vec![0, 3, 8, 5, 32, 2, 63, 1];
    let mut misleading_prefix = vec![(0..64).collect::<Vec<_>>(); 8];
    misleading_prefix.extend(vec![irregular.clone(); 1000]);
    for rows in [
        vec![vec![0, 1]; 64],
        vec![(0..64).step_by(2).collect(); 64],
        vec![irregular; 64],
        misleading_prefix,
    ] {
        let case = ProjectionCase::with_rows(64, &rows);
        let projection = IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes).unwrap();
        let count = rows.iter().map(Vec::len).sum::<usize>();
        assert_eq!(
            projection.retained_bytes(),
            (rows.len() + 1 + count) * size_of::<usize>()
        );
        let mut visitor = RowVisitCheck {
            expected: &rows,
            visits: 0,
            offset: 0,
        };
        projection.for_each_row(&mut visitor);
        assert_eq!(visitor.visits, rows.len());
    }
}

#[test]
fn projection_domains_preserve_empty_rows_duplicates_and_cached_storage() {
    use hymeko::common::ids::HyperArcId;
    let case = ProjectionCase::new();
    let prepared = PreparedIr::new(case.ir).unwrap();
    let view = prepared.incidences();
    let before = view.retained_bytes();
    let nodes = view.prepare_projection(IncidenceDomain::Nodes).unwrap();
    let declarations = view
        .prepare_projection(IncidenceDomain::Declarations)
        .unwrap();
    assert!(std::ptr::eq(nodes.ir(), prepared.ir()));
    assert_eq!(nodes.domain(), IncidenceDomain::Nodes);
    assert_eq!(nodes.domain_len(), 2);
    assert_eq!(declarations.domain_len(), 5);
    assert_eq!(nodes.arc_count(), 2);
    assert_eq!(nodes.incidence_count(), 3);
    assert_eq!(nodes.row(HyperArcId::new(0)).targets(), [0, 1, 0]);
    assert_eq!(declarations.row(HyperArcId::new(0)).targets(), [0, 2, 0]);
    assert_eq!(nodes.row(HyperArcId::new(1)).offset(), 3);
    assert!(nodes.row(HyperArcId::new(1)).targets().is_empty());
    assert_eq!(nodes.retained_bytes(), 6 * size_of::<usize>());
    assert_eq!(
        view.retained_bytes(),
        before + nodes.retained_bytes() + declarations.retained_bytes()
    );
    let again = view.prepare_projection(IncidenceDomain::Nodes).unwrap();
    let cloned = nodes.clone();
    for projection in [&again, &cloned] {
        assert_eq!(
            nodes.row(HyperArcId::new(0)).targets().as_ptr(),
            projection.row(HyperArcId::new(0)).targets().as_ptr()
        );
    }
    let independent = IncidenceProjection::build(prepared.ir(), IncidenceDomain::Nodes).unwrap();
    assert_eq!(
        nodes.rows().map(|r| r.targets()).collect::<Vec<_>>(),
        independent.rows().map(|r| r.targets()).collect::<Vec<_>>()
    );
    let survives_index = {
        let temporary = build_incidence_index(prepared.ir()).unwrap();
        temporary
            .prepare_projection(IncidenceDomain::Nodes)
            .unwrap()
    };
    assert_eq!(survives_index.row(HyperArcId::new(0)).targets(), [0, 1, 0]);
}

#[test]
fn shared_row_operators_match_closed_form_and_adjoint_without_expansion() {
    let case = ProjectionCase::new();
    let projection = IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes).unwrap();
    let coefficients = [1.0, -2.0, 3.0];
    let input = [2.0, 5.0];
    let row_values = [7.0, -9.0];
    let mut gathered = Vec::new();
    let mut scattered = [0.0; 2];
    for (row, value) in projection.rows().zip(row_values) {
        gathered.push(row.gather::<f64>(&input, |slot| coefficients[slot]));
        row.scatter(value, &mut scattered, |slot| coefficients[slot]);
    }
    assert!(bits_eq(&gathered, &[-2.0, 0.0]));
    assert!(bits_eq(&scattered, &[28.0, -14.0]));
    assert_eq!(
        gathered
            .iter()
            .zip(row_values)
            .map(|(a, b)| a * b)
            .sum::<f64>(),
        input.iter().zip(scattered).map(|(a, b)| a * b).sum::<f64>()
    );
}

#[test]
fn projection_rejects_wrong_domains_and_accepts_empty_ir() {
    use hymeko::common::ids::NodeId;
    let mut case = ProjectionCase::new();
    case.ir.arcs[0].refs[0] = case.ir.arcs[0].refs[0].with_target(case.owner);
    assert!(matches!(
        IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes),
        Err(IncidenceProjectionError::InvalidTarget { .. })
    ));
    assert!(IncidenceProjection::build(&case.ir, IncidenceDomain::Declarations).is_ok());
    let index = build_incidence_index(&case.ir).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            index.prepare_projection(IncidenceDomain::Nodes),
            Err(IncidenceProjectionError::InvalidTarget { .. })
        ));
    }
    case.ir.arcs[0].refs[0] = case.ir.arcs[0].refs[0].with_target(DeclId::NONE);
    assert!(matches!(
        IncidenceProjection::build(&case.ir, IncidenceDomain::Declarations),
        Err(IncidenceProjectionError::InvalidTarget { .. })
    ));
    case.ir.nodes[0].decl = DeclId::NONE;
    assert!(
        matches!(IncidenceProjection::build(&case.ir, IncidenceDomain::Nodes),
                     Err(IncidenceProjectionError::InvalidNode(node)) if node == NodeId::new(0))
    );
    let empty = Ir::default();
    for domain in [IncidenceDomain::Nodes, IncidenceDomain::Declarations] {
        let projection = IncidenceProjection::build(&empty, domain).unwrap();
        assert_eq!(projection.domain_len(), 0);
        assert_eq!(projection.arc_count(), 0);
        assert_eq!(projection.incidence_count(), 0);
        assert_eq!(projection.rows().len(), 0);
    }
}

#[test]
fn prepared_forward_and_factorized_star_reuse_the_shared_projection() {
    let case = ProjectionCase::new();
    let index = build_incidence_index(&case.ir).unwrap();
    let plan = index.prepare_operator().unwrap();
    let initial_plan = plan.retained_bytes();
    let initial_index = index.retained_bytes();
    let input = [2.0, 0.0, 5.0, 0.0, 0.0];
    let mut direct = [0.0; 2];
    index.node_to_arc(&input, &mut direct);
    let mut projected = [0.0; 2];
    plan.node_to_arc(&input, &mut projected);
    assert!(bits_eq(&direct, &projected));
    assert!(bits_eq(&projected, &[-2.0, 0.0]));
    assert_eq!(plan.retained_bytes(), initial_plan + 3 * size_of::<f64>());
    let projection = index
        .prepare_projection(IncidenceDomain::Declarations)
        .unwrap();
    assert_eq!(
        index.retained_bytes(),
        initial_index + projection.retained_bytes()
    );
    let mut live = [0.0; 5];
    let mut cached = live;
    index.star_operator(&input, &mut live, &mut direct);
    for _ in 0..2 {
        plan.star_operator(&input, &mut cached, &mut projected);
        assert!(bits_eq(&live, &cached));
        assert!(bits_eq(&cached, &[-8.0, 0.0, 4.0, 0.0, 0.0]));
        assert_eq!(plan.retained_bytes(), initial_plan + 3 * size_of::<f64>());
    }
}

// ---- workloads ------------------------------------------------------------

/// A mixed workload: 4 nodes, 3 arcs mixing +/−/~ signs, none/single/multi weights,
/// a duplicate incidence and a `-0.0` weight — the same shape the P2 parity test uses.
fn mixed() -> Ir {
    let mut ir = Ir::default();
    for _ in 0..4 {
        node(&mut ir);
    }
    arc(
        &mut ir,
        vec![
            r(0, 1, None),
            r(1, -1, Some(vec![2.0])),
            r(0, 1, Some(vec![3.0])),
        ],
    );
    arc(
        &mut ir,
        vec![r(1, 0, Some(vec![5.0])), r(2, 1, Some(vec![2.0, 0.5]))],
    );
    arc(
        &mut ir,
        vec![
            r(3, -1, Some(vec![-0.0])),
            r(2, 1, None),
            r(0, -1, Some(vec![4.0])),
        ],
    );
    ir
}

/// Deterministic pseudo-random dense vectors for the operator inputs.
fn xvec(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 1.0 + (i % 13) as f64 * 0.1 - (i % 5) as f64 * 0.07)
        .collect()
}
fn yvec(n_arcs: usize) -> Vec<f64> {
    (0..n_arcs).map(|i| 0.5 + (i % 11) as f64 * 0.13).collect()
}

fn assert_node_incidence(index: &IncidenceIndex<'_>, view: &IncidenceView<'_, '_>, node: DeclId) {
    let d = node.raw();
    assert_eq!(
        view.incident_arcs(node),
        index.incident_arcs(node),
        "arcs@{d}"
    );
    assert_eq!(
        view.incident_slots(node),
        index.incident_slots(node),
        "slots@{d}"
    );
    assert_eq!(view.degree(node), index.degree(node), "deg@{d}");

    let a: Vec<_> = index
        .incidences(node)
        .map(|i| (i.arc, i.slot, i.target, i.sign, i.edge))
        .collect();
    let b: Vec<_> = view
        .incidences(node)
        .map(|i| (i.arc, i.slot, i.target, i.sign, i.edge))
        .collect();
    assert_eq!(a, b, "resolved incidences@{d}");
    // resolve() coordinate-for-coordinate (incl. weights)
    for (&arc_id, &slot) in index
        .incident_arcs(node)
        .iter()
        .zip(index.incident_slots(node))
    {
        let ri = index.resolve(arc_id, slot);
        let rv = view.resolve(arc_id, slot);
        assert_eq!(
            (ri.arc, ri.slot, ri.target, ri.sign, ri.edge),
            (rv.arc, rv.slot, rv.target, rv.sign, rv.edge)
        );
        assert_eq!(ri.weights, rv.weights, "weights@({},{slot})", arc_id.raw());
    }
}

/// The heart of the differential: every observable of the borrowed `IncidenceIndex`
/// (old) must equal the owned `PreparedIr` view (new) for the *same* `Ir` — coordinate
/// resolution, adjacency, degree and all three operators, bit-for-bit.
fn assert_view_matches_index(ir: Ir) {
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();

    let index: IncidenceIndex<'_> = build_incidence_index(&ir).expect("index builds");
    index.validate().expect("index is a bijection");

    let prepared = PreparedIr::new(ir.clone()).expect("prepared builds");
    let view: IncidenceView<'_, '_> = prepared.incidences();
    view.validate().expect("view is a bijection");

    let (data, arcs, slots, incidences, plan) = {
        let short_lived = prepared.incidences();
        (
            short_lived.data(),
            short_lived.incident_arcs(DeclId::new(0)),
            short_lived.incident_slots(DeclId::new(0)),
            short_lived.incidences(DeclId::new(0)),
            short_lived
                .prepare_operator()
                .expect("short-lived view plan"),
        )
    };
    assert!(std::ptr::eq(data, prepared.index()));
    assert_eq!(arcs, index.incident_arcs(DeclId::new(0)));
    assert_eq!(slots, index.incident_slots(DeclId::new(0)));
    assert_eq!(incidences.count(), index.degree(DeclId::new(0)));
    assert_eq!(
        plan.retained_bytes(),
        index.prepare_operator().unwrap().retained_bytes()
    );

    // structural / accounting invariants
    assert_eq!(view.incidence_count(), index.incidence_count());
    assert_eq!(view.retained_bytes(), index.retained_bytes());
    assert_eq!(prepared.index().incidence_count(), index.incidence_count());

    // per-node adjacency + resolution
    for d in 0..n {
        let node = DeclId::new(d);
        assert_node_incidence(&index, &view, node);
    }

    // operators B / Bᵀ / BᵀB — bit-identical
    let x = xvec(n);
    let y = yvec(n_arcs);

    let (mut yi, mut yv) = (vec![0.0; n_arcs], vec![0.0; n_arcs]);
    index.node_to_arc(&x, &mut yi);
    view.node_to_arc(&x, &mut yv);
    assert!(bits_eq(&yi, &yv), "Bx (node_to_arc) differs");

    let (mut zi, mut zv) = (vec![0.0; n], vec![0.0; n]);
    index.arc_to_node(&y, &mut zi);
    view.arc_to_node(&y, &mut zv);
    assert!(bits_eq(&zi, &zv), "Bᵀy (arc_to_node) differs");

    let (mut si, mut sv) = (vec![0.0; n], vec![0.0; n]);
    let (mut sci, mut scv) = (vec![0.0; n_arcs], vec![0.0; n_arcs]);
    index.star_operator(&x, &mut si, &mut sci);
    view.star_operator(&x, &mut sv, &mut scv);
    assert!(bits_eq(&si, &sv), "BᵀBx (star_operator) differs");

    // star-operator handle == the view's own star_operator
    let star: StarOperator<'_, '_> = prepared.star_operator();
    let mut sh = vec![0.0; n];
    let mut shc = vec![0.0; n_arcs];
    star.apply(&x, &mut sh, &mut shc);
    assert!(
        bits_eq(&si, &sh),
        "StarOperator::apply differs from star_operator"
    );
    let mut yh = vec![0.0; n_arcs];
    star.node_to_arc(&x, &mut yh);
    assert!(bits_eq(&yi, &yh), "StarOperator::node_to_arc differs");
    let mut zh = vec![0.0; n];
    star.arc_to_node(&y, &mut zh);
    assert!(bits_eq(&zi, &zh), "StarOperator::arc_to_node differs");

    // prepared plan via the view == prepared plan via the index (both bit-identical to legacy Bᵀ)
    let plan_i = index.prepare_operator().expect("index plan");
    let plan_v = view.prepare_operator().expect("view plan");
    assert_eq!(plan_i.retained_bytes(), plan_v.retained_bytes());
    let (mut pi, mut pv) = (vec![0.0; n], vec![0.0; n]);
    plan_i.arc_to_node(&y, &mut pi);
    plan_v.arc_to_node(&y, &mut pv);
    assert!(bits_eq(&pi, &pv), "prepared Bᵀy view vs index differs");
    assert!(bits_eq(&zi, &pv), "prepared Bᵀy differs from legacy Bᵀy");
    let mut par = vec![0.0; n];
    plan_v.par_arc_to_node(&y, &mut par);
    assert!(bits_eq(&pv, &par), "prepared serial vs Rayon differs");
}

// ---- tests ----------------------------------------------------------------

#[test]
fn prepared_view_matches_index_bitwise_mixed() {
    assert_view_matches_index(mixed());
}

#[test]
fn prepared_view_matches_index_across_boundary_sizes() {
    // Empty, single, two, and the 63/64/65 CSR/word boundaries, plus a wider window.
    assert_view_matches_index(Ir::default());
    assert_view_matches_index(windowed(1, 1));
    assert_view_matches_index(windowed(2, 1));
    assert_view_matches_index(windowed(2, 2));
    for m in [63usize, 64, 65] {
        assert_view_matches_index(windowed(m, 1));
        assert_view_matches_index(windowed(m, 3));
    }
}

#[test]
fn prepared_view_matches_index_large() {
    assert_view_matches_index(windowed(50_000, 5));
}

#[test]
fn prepared_ir_lifecycle_new_ir_index_into_ir() {
    let ir = mixed();
    let arcs_before = ir.arcs.len();
    let prepared = PreparedIr::new(ir).expect("prepared builds");

    // ir() borrows the owned Ir; index() the derived payload; both consistent.
    assert_eq!(prepared.ir().arcs.len(), arcs_before);
    let nnz: usize = prepared.ir().arcs.iter().map(|a| a.refs.len()).sum();
    assert_eq!(prepared.index().incidence_count(), nnz);
    prepared.incidences().validate().expect("bijection");

    // into_ir returns the owned Ir unchanged (the derived index is dropped, not serialized).
    let back = prepared.into_ir();
    assert_eq!(back.arcs.len(), arcs_before);
}

#[test]
fn index_rebuilt_from_cloned_ir_is_bit_identical() {
    // A clone stands in for serialize → deserialize: the wire carries only the
    // canonical `Ir`; the derived index is rebuilt on load (never carried). The
    // rebuilt operators must be bit-identical to the original.
    let ir = mixed();
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();
    let index = build_incidence_index(&ir).unwrap();

    let reloaded = PreparedIr::new(ir.clone()).unwrap();
    let view = reloaded.incidences();

    let y = yvec(n_arcs);
    let (mut z0, mut z1) = (vec![0.0; n], vec![0.0; n]);
    index.arc_to_node(&y, &mut z0);
    view.arc_to_node(&y, &mut z1);
    assert!(bits_eq(&z0, &z1), "rebuilt Bᵀy differs from original");

    // The data alone rebuilds identically too (data-level constructor).
    let data = IncidenceIndexData::build(reloaded.ir()).unwrap();
    let (mut z2, mut z3) = (vec![0.0; n], vec![0.0; n]);
    data.arc_to_node(reloaded.ir(), &y, &mut z2);
    reloaded.index().arc_to_node(reloaded.ir(), &y, &mut z3);
    assert!(
        bits_eq(&z0, &z2) && bits_eq(&z0, &z3),
        "data-level rebuild differs"
    );
}

#[test]
fn view_and_star_creation_is_zero_alloc_structural() {
    // `IncidenceView` / `StarOperator` are `Copy` and hold only references — creating
    // one is a register move of two (resp. two) pointers, provably heap-free. The
    // owned index memory lives in the `PreparedIr`; the view borrows it, copies nothing.
    let prepared = PreparedIr::new(mixed()).unwrap();

    let view = prepared.incidences();
    assert_eq!(
        size_of_val(&view),
        2 * size_of::<usize>(),
        "view is two thin references"
    );
    let star = prepared.star_operator();
    assert_eq!(
        size_of_val(&star),
        2 * size_of::<usize>(),
        "star wraps the two-ref view"
    );

    // Compile-time proof of `Copy` (fails to compile if either loses `Copy`).
    fn assert_copy<T: Copy>(_: &T) {}
    assert_copy(&view);
    assert_copy(&star);

    // Handing out a million views allocates nothing structurally (references only).
    let mut acc = 0usize;
    for _ in 0..1_000_000 {
        let v: IncidenceView<'_, '_> = prepared.incidences();
        acc = acc.wrapping_add(v.incidence_count());
    }
    assert_eq!(
        acc,
        prepared.index().incidence_count().wrapping_mul(1_000_000)
    );
}

#[test]
fn wrong_owner_index_is_rejected_by_bijection() {
    // Fault mutant: an `IncidenceIndexData` built from one `Ir` validated against a
    // *different* `Ir` must fail the bijection — the runtime guard for the raw-data
    // path (the `IncidenceIndex` / `PreparedIr` wrappers make the wrong pairing a
    // compile error by bundling the `Ir` with its data).
    let ir_a = windowed(8, 3);
    let ir_b = windowed(5, 2); // different node/arc counts
    let data = IncidenceIndexData::build(&ir_a).unwrap();
    assert!(data.validate(&ir_a).is_ok(), "matches its own Ir");
    assert!(data.validate(&ir_b).is_err(), "must reject a foreign Ir");
}

#[test]
#[ignore = "performance benchmark; opt in with --ignored --exact"]
fn bench_representation_view_vs_index_and_o1_creation() {
    // §8 decision gate. Representations compared on the SAME data + math:
    //   A = borrowed `IncidenceIndex` (row/arcs/slots SoA borrowing the `Ir`);
    //   B = owned `PreparedIr` + borrowed `IncidenceView` (the chosen model).
    // A and B share the operator code (the logic lives on `IncidenceIndexData`), so B
    // must be a no-regression drop-in AND its view creation must be O(1) / zero-alloc.
    // (The flat arc-major SoA "C" — a second (arc,slot)/coefficient array — was measured
    // in P2's bx_profile: ~2× Bx but +8 B/incidence with no consumer, so it is deferred,
    // not forced; see the report.)
    let (m, k) = (200_001usize, 5usize);
    let ir = windowed(m, k);
    let n = ir.decl_nodes.len();
    let n_arcs = ir.arcs.len();
    let nnz: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();

    let index = build_incidence_index(&ir).unwrap();
    let prepared = PreparedIr::new(ir.clone()).unwrap();
    let view = prepared.incidences();

    // retained memory is identical (B owns the same SoA arrays A borrows).
    assert_eq!(view.retained_bytes(), index.retained_bytes());

    let x = xvec(n);
    let y = yvec(n_arcs);
    let (mut za, mut zb) = (vec![0.0; n], vec![0.0; n]);

    // O(1) view creation: median over 1e6 creations, black-boxed against elision.
    let create = median_ms(9, || {
        let mut acc = 0usize;
        for _ in 0..1_000_000 {
            acc = acc.wrapping_add(prepared.incidences().incidence_count());
        }
        assert_eq!(acc, nnz.wrapping_mul(1_000_000));
    });
    let per_create_ns = create * 1e6 / 1_000_000.0;

    let bt_a = median_ms(9, || index.arc_to_node(&y, &mut za));
    let bt_b = median_ms(9, || view.arc_to_node(&y, &mut zb));
    assert!(bits_eq(&za, &zb), "Bᵀy A vs B must be bit-identical");

    let (mut sa, mut sb) = (vec![0.0; n], vec![0.0; n]);
    let mut scr = vec![0.0; n_arcs];
    let star = prepared.star_operator();
    let st_a = median_ms(9, || index.star_operator(&x, &mut sa, &mut scr));
    let st_b = median_ms(9, || star.apply(&x, &mut sb, &mut scr));
    assert!(bits_eq(&sa, &sb), "BᵀBx A vs B must be bit-identical");

    println!("\n=== P2.1 §8 representation gate — {nnz} incidences ===");
    println!(
        "retained bytes A==B      : {} B ({:.1} B/incidence)",
        view.retained_bytes(),
        view.retained_bytes() as f64 / nnz as f64
    );
    println!("view creation (O(1))     : {per_create_ns:.2} ns/create");
    println!("Bᵀy   A (index)          : {bt_a:.3} ms");
    println!(
        "Bᵀy   B (view)           : {bt_b:.3} ms   ({:.2}x)",
        bt_a / bt_b.max(1e-9)
    );
    println!("BᵀBx  A (index)          : {st_a:.3} ms");
    println!(
        "BᵀBx  B (star handle)    : {st_b:.3} ms   ({:.2}x)",
        st_a / st_b.max(1e-9)
    );

    // O(1) guard: a 16-byte reference-pair copy is sub-100 ns even on a loaded host.
    assert!(
        per_create_ns < 100.0,
        "view creation must be O(1) (was {per_create_ns:.1} ns)"
    );
    // No-regression guard: same code path, so B must stay within a generous band of A.
    assert!(bt_b <= bt_a * 1.5 + 0.5, "view Bᵀy regressed vs index");
    assert!(st_b <= st_a * 1.5 + 0.5, "star BᵀBx regressed vs index");
}
