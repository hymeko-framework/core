use super::*;
use crate::{
    common::{ids::SymId, pathkey::PathKey},
    ir::{
        canonical_hash::Canon,
        ir::{ArcPlacement, IrConstruction},
    },
    resolution::{
        interner::Interner,
        resolve::Index,
        symbols::{SymbolCompaction, SymbolDelta, SymbolUsage},
    },
};
use std::sync::Arc;

struct Fixture {
    ir: Ir,
    target: DeclId,
    owner: DeclId,
}

#[test]
fn repeated_bases_share_storage_without_sharing_editable_annotations() {
    let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
    let draft = ReferenceDraft::new(
        IncidenceSign::Minus,
        RefAtomR {
            target: fixture.target,
            anno: AnnoR {
                tags: vec![SymId::new(2)],
                value: Some(ValueR::Num(-0.0)),
            },
            weights: Some(vec![ValueR::List(vec![ValueR::Num(3.0)])]),
            direction: PortDirection::In,
        },
    );
    let mut declarations = Vec::new();
    for ordinal in 0..128 {
        let id = IrConstruction::new(&mut fixture.ir)
            .append_node(
                SymId::new(3),
                draft.atom.anno.clone(),
                vec![draft.clone(); 2],
            )
            .unwrap();
        declarations.push(id);
        let record = fixture.ir.node_for_decl(id).unwrap().1;
        assert_eq!(record.bases, BaseListId::new(0));
        WireOracle::check(
            &wire::Record {
                ir: &fixture.ir,
                value: record,
            },
            &InlineNode {
                decl: id,
                bases: vec![InlineReference::from(&draft), InlineReference::from(&draft)],
            },
        );
        // Empty inputs preserve the last nonempty candidate and allocate no slot.
        assert!(fixture.ir.store_base_drafts(Vec::new()).is_none());
        assert_eq!(fixture.ir.annotations.len(), ordinal + 3);
        assert_eq!(fixture.ir.ref_weights.len(), 2);
        assert_eq!(fixture.ir.base_lists.len(), 1);
    }
    for cbor in [false, true] {
        let decoded: Ir = if cbor {
            let mut bytes = Vec::new();
            ciborium::ser::into_writer(&fixture.ir, &mut bytes).unwrap();
            ciborium::de::from_reader(bytes.as_slice()).unwrap()
        } else {
            serde_json::from_slice(&serde_json::to_vec(&fixture.ir).unwrap()).unwrap()
        };
        assert_eq!(decoded.base_lists.len(), 1);
        assert_eq!(decoded.annotations.len(), fixture.ir.annotations.len());
        assert_eq!(decoded.ref_weights.len(), 2);
        WireOracle::check(&decoded, &fixture.ir);
    }
    let old = fixture.ir.into_shared();
    let mut edited = old.clone();
    edited
        .decl_annotation_mut(declarations[0])
        .unwrap()
        .tags
        .clear();
    assert_eq!(old.decl_annotation(declarations[0]).unwrap().tags.len(), 1);
    assert_eq!(
        edited.decl_annotation(declarations[1]).unwrap().tags.len(),
        1
    );
    assert_eq!(
        edited
            .reference_atom(&edited[BaseListId::new(0)][0])
            .unwrap()
            .anno
            .tags
            .len(),
        1
    );
    assert_eq!(
        edited.store_base_drafts(vec![draft.clone(); 2]),
        BaseListId::new(0)
    );
    assert!(std::ptr::eq(
        old[BaseListId::new(0)].as_ptr(),
        edited[BaseListId::new(0)].as_ptr()
    ));
    let mut changed = draft;
    changed.sign = IncidenceSign::Plus;
    let different = edited.store_base_drafts(vec![changed]);
    assert_eq!(edited[different].len(), 1);
    assert_eq!(old.base_lists.len(), 1);
    assert_eq!(old[BaseListId::new(0)].len(), 2);
}

#[test]
fn base_reuse_preserves_every_reference_distinction_and_float_bit_pattern() {
    let plain = ReferenceDraft::plain(DeclId::new(0), IncidenceSign::Plus);
    let mut variants = vec![vec![plain.clone()], vec![plain.clone(); 2]];
    for sign in [IncidenceSign::Minus, IncidenceSign::Neutral] {
        let mut draft = plain.clone();
        draft.sign = sign;
        variants.push(vec![draft]);
    }
    for direction in [PortDirection::In, PortDirection::Out] {
        let mut draft = plain.clone();
        draft.atom.direction = direction;
        variants.push(vec![draft]);
    }
    let mut other = plain.clone();
    other.atom.target = DeclId::new(1);
    variants.extend([
        vec![other.clone()],
        vec![other.clone(), plain.clone()],
        vec![plain.clone(), other],
    ]);
    for tags in [vec![0], vec![0, 0], vec![0, 1], vec![1, 0]] {
        let mut draft = plain.clone();
        draft.atom.anno.tags = tags.into_iter().map(SymId::new).collect();
        variants.push(vec![draft]);
    }
    for bits in [
        0,
        1,
        (-0.0_f64).to_bits(),
        0x7ff8_0000_0000_0042,
        0x7ff8_0000_0000_0043,
    ] {
        for weights in [false, true] {
            let mut draft = plain.clone();
            let value = ValueR::List(vec![ValueR::Num(f64::from_bits(bits))]);
            if weights {
                draft.atom.weights = Some(vec![value]);
            } else {
                draft.atom.anno.value = Some(value);
            }
            variants.push(vec![draft]);
        }
    }
    let mut empty_weights = plain;
    empty_weights.atom.weights = Some(vec![]);
    variants.push(vec![empty_weights]);
    for (i, first) in variants.iter().enumerate() {
        for (j, second) in variants.iter().enumerate() {
            let mut ir = Ir::default();
            let a = ir.store_base_drafts(first.clone());
            let b = ir.store_base_drafts(second.clone());
            assert_eq!(a == b, i == j, "logical variants {i}, {j}");
            let counts = (
                ir.base_lists.len(),
                ir.annotations.len(),
                ir.ref_weights.len(),
            );
            assert_eq!(ir.store_base_drafts(second.clone()), b);
            assert_eq!(
                counts,
                (
                    ir.base_lists.len(),
                    ir.annotations.len(),
                    ir.ref_weights.len()
                )
            );
        }
    }
}

#[test]
fn base_reuse_is_bounded_and_checks_invalid_payloads_without_panicking() {
    let mut ir = Ir::default();
    let first = SignedRefR::plain(DeclId::new(0), IncidenceSign::Plus);
    let second = first.with_target(DeclId::new(1));
    let a = ir.store_bases(vec![first]);
    assert_eq!(ir.store_bases(vec![first]), a);
    let b = ir.store_bases(vec![second]);
    assert_ne!(a, b);
    let c = ir.store_bases(vec![first]);
    assert_ne!(a, c); // One candidate; no history scan or global intern table.
    assert_eq!(ir[a], ir[c]);
    for weights in [false, true] {
        let mut invalid = first;
        if weights {
            invalid.weights = WeightVectorId::new(99);
        } else {
            invalid.annotation = AnnotationId::new(99);
        }
        let broken = ir.store_bases(vec![invalid]);
        let repaired = ir.store_base_drafts(vec![ReferenceDraft::plain(
            DeclId::new(0),
            IncidenceSign::Plus,
        )]);
        assert_ne!(broken, repaired);
        assert!(ir.reference_atom(&ir[repaired][0]).is_ok());
    }
    ir.base_lists.share_pages(2).unwrap();
    let last = ir.store_base_drafts(vec![ReferenceDraft::plain(
        DeclId::new(0),
        IncidenceSign::Plus,
    )]);
    assert_eq!(ir[last], [first]);
}

#[test]
fn canonical_reference_vectors_release_draft_capacity_without_cloning_the_ir() {
    for count in [0, 2, 32, 1000] {
        let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
        let drafts = (0..count)
            .map(|slot| {
                let atom = RefAtomR::plain(fixture.target)
                    .with_weights((slot % 2 == 0).then(|| vec![ValueR::Num(slot as f64)]));
                ReferenceDraft::new(IncidenceSign::Minus, atom)
            })
            .collect();
        let arc = IrConstruction::new(&mut fixture.ir)
            .append_arc(
                ArcPlacement::under_owner(fixture.owner),
                AnnoR::default(),
                drafts,
            )
            .unwrap();
        // Inspect retained storage directly: Vec::clone would hide the old slack.
        let refs = &fixture.ir.arc_for_decl(arc).unwrap().1.refs;
        assert_eq!(refs.len(), count);
        assert_eq!(refs.capacity(), count);
        assert_eq!(
            refs.capacity() * size_of::<SignedRefR>(),
            count * size_of::<SignedRefR>()
        );
        for (slot, reference) in refs.iter().enumerate() {
            assert_eq!(reference.target(), fixture.target);
            assert_eq!(reference.sign(), -1);
            let expected = (slot % 2 == 0).then(|| vec![ValueR::Num(slot as f64)]);
            assert_eq!(
                fixture.ir.reference_atom(reference).unwrap().weights,
                expected.as_deref()
            );
        }
    }
}
impl Fixture {
    fn new(node_name: SymId, edge_name: SymId) -> Self {
        let mut ir = Ir::default();
        let mut construction = IrConstruction::new(&mut ir);
        let target = construction
            .append_node(node_name, AnnoR::default(), vec![])
            .unwrap();
        let owner = construction
            .append_edge(edge_name, AnnoR::default(), vec![])
            .unwrap();
        Self { ir, target, owner }
    }
    fn append(&mut self, drafts: Vec<ReferenceDraft>) -> Vec<SignedRefR> {
        let id = IrConstruction::new(&mut self.ir)
            .append_arc(
                ArcPlacement::under_owner(self.owner),
                AnnoR::default(),
                drafts,
            )
            .unwrap();
        self.ir.arc_for_decl(id).unwrap().1.refs.clone()
    }
}

#[test]
fn layout_has_byte_tags_and_plain_references_allocate_no_payload_storage() {
    use crate::ir::ir::{ArcRec, DeclNode, EdgeRec, NodeRec};
    if size_of::<usize>() == 8 {
        assert_eq!(
            [
                size_of::<DeclNode>(),
                size_of::<NodeRec>(),
                size_of::<EdgeRec>(),
                size_of::<ArcRec>()
            ],
            [56, 16, 40, 40]
        );
    }
    assert_eq!(
        size_of::<BaseListId>() + size_of::<Box<[SignedRefR]>>(),
        size_of::<Vec<SignedRefR>>()
    );
    assert_eq!(std::mem::size_of::<IncidenceSign>(), 1);
    assert_eq!(std::mem::size_of::<PortDirection>(), 1);
    assert_eq!(
        std::mem::size_of::<SignedRefR>(),
        4 * std::mem::size_of::<usize>()
    );
    assert_eq!(
        std::mem::offset_of!(SignedRefR, direction),
        std::mem::offset_of!(SignedRefR, sign) + 1
    );
    let mut ir = Ir::default();
    let reference = ir.store_reference(ReferenceDraft::plain(DeclId::new(2), IncidenceSign::Plus));
    assert_eq!(ir.reference_atom(&reference).unwrap().weights, None);
    let ir = ir.into_shared();
    for model in [&ir, &ir.clone()] {
        assert_eq!(model.annotations.capacity(), 0);
        assert_eq!(model.base_lists.capacity(), 0);
        assert_eq!(model.ref_weights.capacity(), 0);
        assert!(reference.annotation_id().is_none());
        assert!(reference.weight_vector_id().is_none());
    }
    let edited = reference
        .with_target(DeclId::new(3))
        .with_sign(IncidenceSign::Minus)
        .with_direction(PortDirection::Out);
    assert_eq!(
        (edited.target(), edited.sign(), edited.direction()),
        (DeclId::new(3), -1, PortDirection::Out)
    );
    assert_eq!(
        (reference.target(), reference.sign(), reference.direction()),
        (DeclId::new(2), 1, PortDirection::Unspecified)
    );
}

#[test]
fn compact_annotation_updates_reuse_slots_and_preserve_old_paged_readers() {
    let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
    assert!(fixture.ir.annotations.is_empty());
    assert!(fixture.ir.base_lists.is_empty());
    fixture
        .ir
        .set_decl_annotation(fixture.target, AnnoR::default())
        .unwrap();
    assert!(fixture.ir.annotations.is_empty());
    for bits in [0x8000_0000_0000_0000, 0x7ff8_0000_0000_0042] {
        fixture
            .ir
            .set_leaf_values(&mut vec![crate::ir::ir::LeafValueEdit::Replace {
                declaration: fixture.target,
                value: Some(ValueR::Num(f64::from_bits(bits))),
            }])
            .unwrap();
        let Some(ValueR::Num(value)) = fixture.ir.decl_annotation(fixture.target).unwrap().value
        else {
            panic!("numeric payload lost")
        };
        assert_eq!(value.to_bits(), bits);
        assert_eq!(fixture.ir.annotations.len(), 1);
    }
    fixture
        .ir
        .set_leaf_values(&mut vec![crate::ir::ir::LeafValueEdit::Replace {
            declaration: fixture.target,
            value: None,
        }])
        .unwrap();
    assert!(
        fixture
            .ir
            .decl_annotation(fixture.target)
            .unwrap()
            .value
            .is_none()
    );
    fixture
        .ir
        .set_decl_annotation(
            fixture.target,
            AnnoR {
                tags: vec![SymId::new(2)],
                value: Some(ValueR::Num(-0.0)),
            },
        )
        .unwrap();
    for page_size in [None, Some(1)] {
        let mut base = fixture.ir.clone().into_shared();
        if let Some(size) = page_size {
            base.annotations.share_pages(size).unwrap();
        }
        let original = base.decl_annotation(fixture.target).unwrap();
        let slot = base.decl_node(fixture.target).unwrap().anno;
        let mut next = base.clone();
        for iteration in 0..32 {
            let value = (iteration % 2 == 0).then_some(ValueR::Num(iteration as f64));
            next.set_leaf_values(&mut vec![crate::ir::ir::LeafValueEdit::Replace {
                declaration: fixture.target,
                value: value.clone(),
            }])
            .unwrap();
            assert_eq!(next.decl_annotation(fixture.target).unwrap().value, value);
            assert_eq!(next.annotations.len(), 1);
            assert_eq!(next.decl_node(fixture.target).unwrap().anno, slot);
            assert_eq!(original.tags, vec![SymId::new(2)]);
            let Some(ValueR::Num(number)) = original.value else {
                panic!("old payload lost")
            };
            assert_eq!(number.to_bits(), (-0.0_f64).to_bits());
        }
        let draft = next.decl_draft(fixture.target).unwrap();
        assert_eq!(draft.anno, *next.decl_annotation(fixture.target).unwrap());
        next.decl_annotation_mut(fixture.owner)
            .unwrap()
            .tags
            .push(SymId::new(3));
        assert_eq!(next.annotations.len(), 2);
        assert!(base.decl_annotation(fixture.owner).unwrap().tags.is_empty());
        assert!(next.decl_draft(DeclId::NONE).is_none());
        let before = format!("{next:?}");
        assert!(
            next.set_decl_annotation(DeclId::NONE, AnnoR::default())
                .is_err()
        );
        assert!(next.decl_annotation_mut(DeclId::NONE).is_err());
        assert_eq!(format!("{next:?}"), before);
    }
}

// Frozen logical record shapes from compact/initial/core/.../ir.rs and
// reference/wire.rs. These serializers are independent of the compact wire view.
#[derive(Serialize)]
struct InlineNode<'a> {
    decl: DeclId,
    bases: Vec<InlineReference<'a>>,
}
#[derive(Serialize)]
struct InlineArc<'a> {
    anno: &'a AnnoR,
    in_edge: DeclId,
    refs: Vec<InlineReference<'a>>,
}

#[derive(Serialize)]
struct InlineDeclaration<'a> {
    kind: crate::ir::ir::DeclKind,
    name: SymId,
    parent: DeclId,
    first_child: DeclId,
    last_child: DeclId,
    next_sibling: DeclId,
    anno: &'a AnnoR,
}

struct WireOracle;
impl WireOracle {
    fn check(actual: &impl Serialize, expected: &impl Serialize) {
        assert_eq!(
            serde_json::to_vec(actual).unwrap(),
            serde_json::to_vec(expected).unwrap()
        );
        let (mut left, mut right) = (Vec::new(), Vec::new());
        ciborium::ser::into_writer(actual, &mut left).unwrap();
        ciborium::ser::into_writer(expected, &mut right).unwrap();
        assert_eq!(left, right);
    }
}

#[test]
fn compact_record_wire_matches_inline_oracle_and_preserves_base_multiplicity() {
    use crate::ir::ir::{DeclKind, DeclNode};
    let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
    let anno = AnnoR {
        tags: vec![SymId::new(2)],
        value: Some(ValueR::Num(-0.0)),
    };
    let draft = DeclNode::root(DeclKind::Node, SymId::new(3), anno.clone());
    let reference = ReferenceDraft::new(
        IncidenceSign::Minus,
        RefAtomR {
            target: fixture.target,
            anno: anno.clone(),
            weights: Some(vec![]),
            direction: PortDirection::In,
        },
    );
    let target = IrConstruction::new(&mut fixture.ir)
        .append_named(
            draft.clone(),
            vec![reference.clone(), reference.clone()],
            &[],
        )
        .unwrap();
    let node = fixture.ir.node_for_decl(target).unwrap().1;
    assert_eq!(fixture.ir[node.bases].len(), 2);
    let expected = InlineNode {
        decl: target,
        bases: vec![
            InlineReference::from(&reference),
            InlineReference::from(&reference),
        ],
    };
    WireOracle::check(
        &wire::Record {
            ir: &fixture.ir,
            value: node,
        },
        &expected,
    );
    WireOracle::check(
        &wire::Record {
            ir: &fixture.ir,
            value: fixture.ir.decl_node(target).unwrap(),
        },
        &InlineDeclaration {
            kind: draft.kind,
            name: draft.name,
            parent: draft.parent,
            first_child: draft.first_child,
            last_child: draft.last_child,
            next_sibling: draft.next_sibling,
            anno: &draft.anno,
        },
    );
    IrConstruction::new(&mut fixture.ir)
        .append_arc(
            ArcPlacement::under_owner(fixture.owner),
            anno.clone(),
            vec![reference.clone()],
        )
        .unwrap();
    WireOracle::check(
        &wire::Record {
            ir: &fixture.ir,
            value: &fixture.ir.arcs[0],
        },
        &InlineArc {
            anno: &anno,
            in_edge: fixture.owner,
            refs: vec![InlineReference::from(&reference)],
        },
    );
    let encoded = serde_json::to_vec(&fixture.ir).unwrap();
    let decoded: Ir = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
    assert_eq!(decoded[decoded.nodes[1].bases].len(), 2);
    assert_eq!(
        fixture.ir[fixture.ir.nodes[1].bases]
            .iter()
            .map(|r| r.target())
            .collect::<Vec<_>>(),
        vec![fixture.target; 2]
    );
}

#[test]
fn invalid_and_aliased_compact_payloads_fail_admission() {
    use crate::ir::coherence::{CoherenceLimits, Finding, validate_hierarchy_index_coherence};
    let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
    let mut interner = Interner::new();
    interner.intern("node");
    interner.intern("edge");
    let id = fixture.ir.store_annotation(AnnoR {
        tags: vec![],
        value: Some(ValueR::Num(1.0)),
    });
    fixture.ir.decl_nodes[0].anno = id;
    fixture.ir.decl_nodes[1].anno = id;
    let report = validate_hierarchy_index_coherence(
        &fixture.ir,
        &Index::new(),
        &Interner::new(),
        CoherenceLimits::default(),
    )
    .unwrap_err();
    assert!(report.findings.iter().any(|f| matches!(
        f,
        Finding::InvalidPayload {
            field: "exclusive annotation",
            ..
        }
    )));
    for annotation in [true, false] {
        let mut invalid = fixture.ir.clone();
        if annotation {
            invalid.decl_nodes[0].anno = AnnotationId::new(99);
        } else {
            invalid.nodes[0].bases = BaseListId::new(99);
        }
        assert!(serde_json::to_vec(&invalid).is_err());
        let error = SymbolCompaction::apply(&mut invalid, &mut Index::new(), &mut [], &interner)
            .err()
            .unwrap();
        assert_eq!(
            error.field,
            if annotation {
                "annotation ID"
            } else {
                "base-list ID"
            }
        );
        assert!(invalid.annotation(AnnotationId::new(99)).is_none());
        assert!(invalid.bases(BaseListId::new(99)).is_none());
        assert!(invalid.bases(BaseListId::NONE).unwrap().is_empty());
        if annotation {
            assert!(invalid.decl_draft(fixture.target).is_none());
            let before = format!("{invalid:?}");
            assert!(
                invalid
                    .set_decl_annotation(fixture.target, AnnoR::default())
                    .is_err()
            );
            assert!(invalid.decl_annotation_mut(fixture.target).is_err());
            assert_eq!(format!("{invalid:?}"), before);
        }
    }
}

// Frozen inline reference shape from the pre-compaction ir.rs, archived under
// reports/2026-10-17-core-construction-references/before/. Test-only oracle;
// production serializes the compact record through its owner-aware wire view.
#[derive(Serialize)]
enum InlineReference<'a> {
    Plus(&'a RefAtomR),
    Minus(&'a RefAtomR),
    Neutral(&'a RefAtomR),
}
impl<'a> From<&'a ReferenceDraft> for InlineReference<'a> {
    fn from(draft: &'a ReferenceDraft) -> Self {
        match draft.sign {
            IncidenceSign::Plus => Self::Plus(&draft.atom),
            IncidenceSign::Minus => Self::Minus(&draft.atom),
            IncidenceSign::Neutral => Self::Neutral(&draft.atom),
        }
    }
}

#[test]
fn inline_wire_bytes_survive_all_sign_direction_and_vector_presence_cases() {
    let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
    let mut drafts = Vec::new();
    for sign in [
        IncidenceSign::Plus,
        IncidenceSign::Minus,
        IncidenceSign::Neutral,
    ] {
        for direction in [
            PortDirection::Unspecified,
            PortDirection::In,
            PortDirection::Out,
        ] {
            for weights in [
                None,
                Some(vec![]),
                Some(vec![
                    ValueR::Num(-0.0),
                    ValueR::List(vec![ValueR::Num(2.0)]),
                ]),
            ] {
                drafts.push(ReferenceDraft::new(
                    sign,
                    RefAtomR {
                        target: fixture.target,
                        anno: AnnoR {
                            tags: vec![SymId::new(2)],
                            value: Some(ValueR::Num(4.0)),
                        },
                        weights,
                        direction,
                    },
                ));
            }
        }
    }
    let expected: Vec<_> = drafts.iter().map(InlineReference::from).collect();
    let references = fixture.append(drafts.clone());
    let actual = wire::References {
        ir: &fixture.ir,
        values: &references,
    };
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    let (mut actual_bytes, mut expected_bytes) = (Vec::new(), Vec::new());
    ciborium::ser::into_writer(&actual, &mut actual_bytes).unwrap();
    ciborium::ser::into_writer(&expected, &mut expected_bytes).unwrap();
    assert_eq!(actual_bytes, expected_bytes);
    let encoded = serde_json::to_vec(&fixture.ir).unwrap();
    let decoded: Ir = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
    assert_eq!(decoded.arcs[0].refs.capacity(), decoded.arcs[0].refs.len());
    for (reference, expected) in decoded.arcs[0].refs.iter().zip(&drafts) {
        let restored = decoded.reference_draft(reference).unwrap();
        assert_eq!(restored, *expected);
        if let Some(values) = &restored.atom.weights {
            if let Some(ValueR::Num(value)) = values.first() {
                assert_eq!(value.to_bits(), (-0.0f64).to_bits());
            }
        }
    }
}

#[test]
fn replacing_payloads_keeps_retained_snapshot_readers_independent() {
    let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
    let original = ReferenceDraft::new(
        IncidenceSign::Plus,
        RefAtomR {
            target: fixture.target,
            anno: AnnoR {
                tags: vec![SymId::new(2)],
                value: Some(ValueR::Num(7.0)),
            },
            weights: Some(vec![ValueR::Num(-0.0)]),
            direction: PortDirection::In,
        },
    );
    let reference = fixture.append(vec![original.clone()])[0];
    let old = fixture.ir.into_shared();
    let mut next = old.clone();
    assert!(std::ptr::eq(&old.annotations[0], &next.annotations[0]));
    let mut draft = next.reference_draft(&reference).unwrap();
    draft.atom.anno.value = Some(ValueR::Num(8.0));
    draft.atom.weights = Some(vec![ValueR::Num(3.0)]);
    let changed = next.store_reference(draft.clone());
    next.arcs[0].refs[0] = changed;
    assert_eq!(old.reference_draft(&reference).unwrap(), original);
    assert_eq!(next.reference_draft(&changed).unwrap(), draft);
    assert_eq!(old.annotations.len(), 1);
    assert_eq!(next.annotations.len(), 2);
}

#[test]
fn invalid_payload_ids_fail_lookup_wire_and_symbol_admission() {
    let mut fixture = Fixture::new(SymId::new(0), SymId::new(1));
    fixture.append(vec![ReferenceDraft::plain(
        fixture.target,
        IncidenceSign::Plus,
    )]);
    let mut interner = Interner::new();
    interner.intern("node");
    interner.intern("edge");
    for weights in [false, true] {
        let mut invalid = fixture.ir.clone();
        let reference = &mut invalid.arcs[0].refs[0];
        let expected = if weights {
            reference.weights = WeightVectorId::new(17);
            ReferenceError::Weights(reference.weights)
        } else {
            reference.annotation = AnnotationId::new(17);
            ReferenceError::Annotation(reference.annotation)
        };
        let reference = *reference;
        assert_eq!(invalid.reference_atom(&reference).unwrap_err(), expected);
        assert!(serde_json::to_vec(&invalid).is_err());
        assert!(
            SymbolCompaction::apply(&mut invalid, &mut Index::new(), &mut [], &interner).is_err()
        );
    }
}

#[test]
fn shared_payloads_are_remapped_once_in_full_and_incremental_symbol_compaction() {
    let mut interner = Interner::new();
    interner.intern("placeholder");
    interner.intern("unused");
    let node = interner.intern("node");
    let edge = interner.intern("edge");
    let tag = interner.intern("tag");
    let text = interner.intern("text");
    let mut fixture = Fixture::new(node, edge);
    let reference = fixture.append(vec![ReferenceDraft::new(
        IncidenceSign::Plus,
        RefAtomR {
            target: fixture.target,
            anno: AnnoR {
                tags: vec![tag],
                value: Some(ValueR::Str(text)),
            },
            weights: Some(vec![ValueR::Str(text)]),
            direction: PortDirection::Out,
        },
    )])[0];
    fixture.ir.nodes[0].bases = fixture.ir.store_bases(vec![reference]);
    fixture.ir.edges[0].bases = fixture.ir.store_bases(vec![reference]);
    assert_eq!(fixture.ir.nodes[0].bases, fixture.ir.edges[0].bases);
    let base = fixture.ir.into_shared();
    let mut index = Index::new();
    index.by_path.insert(PathKey(vec![node]), fixture.target);
    index.by_path.insert(PathKey(vec![edge]), fixture.owner);
    let digest = Canon::new(&base, &interner).hash();
    for incremental in [false, true] {
        let mut next = base.clone();
        let mut index = Arc::new(index.clone());
        let mut imports = Arc::new(Vec::new());
        let compacted = if incremental {
            SymbolUsage::new(&base, &index, &imports, &interner)
                .unwrap()
                .with_values(
                    (&base, &mut next),
                    &mut index,
                    &mut imports,
                    &interner,
                    &[],
                    &mut SymbolDelta::with_capacity(0).unwrap(),
                )
                .unwrap()
                .0
        } else {
            SymbolCompaction::apply(
                &mut next,
                Arc::make_mut(&mut index),
                Arc::make_mut(&mut imports).as_mut_slice(),
                &interner,
            )
            .unwrap()
        };
        let atom = next.reference_atom(&reference).unwrap();
        assert_eq!(compacted.interner.resolve(atom.anno.tags[0]), "tag");
        let Some(ValueR::Str(symbol)) = &atom.anno.value else {
            panic!("annotation value lost")
        };
        assert_eq!(compacted.interner.resolve(*symbol), "text");
        let Some([ValueR::Str(symbol)]) = atom.weights else {
            panic!("vector value lost")
        };
        assert_eq!(compacted.interner.resolve(*symbol), "text");
        assert!(compacted.interner.get_id("unused").is_none());
        assert_eq!(Canon::new(&next, &compacted.interner).hash(), digest);
        let draft = next.reference_draft(&reference).unwrap();
        let shared = next.nodes[0].bases;
        assert_eq!(next.store_base_drafts(vec![draft]), shared);
    }
    assert_eq!(
        base.reference_atom(&reference).unwrap().anno.tags,
        vec![tag]
    );
}
