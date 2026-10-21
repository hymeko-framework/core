//! Bounded correctness tests; no timers, benchmark harness, or resource campaign.
use super::*;
use crate::ir::canonical_hash::Canon;
use crate::module_store::source_provider::MemProvider;
use crate::util::real_parser::RealParser as Parser;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Barrier;
use std::thread;

fn source(name: &str) -> MemProvider {
    MemProvider::default().with_file(
        "model.hymeko",
        format!("world {{}} network {{ {name} {{}} }}"),
    )
}

fn model(name: &str) -> CompiledModel {
    CompiledModel::compile(source(name), Parser, Path::new("model.hymeko")).unwrap()
}

fn prepare(state: &ExecutionState, name: &str) -> PreparedCompilation {
    state
        .prepare(source(name), Parser, Path::new("model.hymeko"))
        .unwrap()
}

fn names(snapshot: &ExecutionSnapshot) -> Vec<&str> {
    let model = snapshot.model();
    model
        .program()
        .ir
        .decl_nodes
        .iter()
        .map(|decl| model.interner().resolve(decl.name))
        .collect()
}

#[test]
fn retained_publication_reports_capacity_and_returns_the_candidate() {
    assert!(SnapshotRetirement::with_capacity(usize::MAX).is_err());
    let state = ExecutionState::new(model("old"));
    let original = state.snapshot().unwrap();
    let mut none = SnapshotRetirement::with_capacity(0).unwrap();
    assert_eq!(none.capacity(), 0);
    assert!(none.is_empty());
    let candidate = prepare(&state, "next");
    let candidate_program = Arc::downgrade(&candidate.model.program);
    let error = state.commit_retaining(candidate, &mut none).err().unwrap();
    assert!(matches!(
        &error,
        RetainedPublicationError::RetirementFull(_)
    ));
    assert!(error.to_string().contains("full"));
    assert!(std::error::Error::source(&error).is_none());
    assert!(format!("{error:?}").contains("Full"));
    assert!(candidate_program.upgrade().is_some());
    assert!(Arc::ptr_eq(&original, &state.snapshot().unwrap()));
    let mut retirement = SnapshotRetirement::with_capacity(1).unwrap();
    let next = state
        .commit_retaining(error.into_candidate(), &mut retirement)
        .unwrap();
    assert_eq!(next.revision(), 1);
    assert_eq!(retirement.len(), 1);
    assert_eq!(retirement.reclaim(0), 0);
    assert_eq!(
        retirement.reclaim(1),
        0,
        "the old reader still owns its root"
    );
    let blocked = state
        .commit_retaining(prepare(&state, "third"), &mut retirement)
        .err()
        .unwrap();
    assert!(matches!(
        blocked,
        RetainedPublicationError::RetirementFull(_)
    ));
    assert_eq!(state.snapshot().unwrap().revision(), 1);
    drop(original);
    assert_eq!(retirement.reclaim(1), 1);
    assert!(retirement.is_empty());
    assert_eq!(
        state
            .commit_retaining(blocked.into_candidate(), &mut retirement)
            .unwrap()
            .revision(),
        2
    );
}

#[test]
fn retirement_rotates_live_readers_and_destroys_unique_roots_on_the_worker() {
    let state = ExecutionState::new(model("a"));
    let a = state.snapshot().unwrap();
    let a_weak = Arc::downgrade(&a);
    let mut retirement = SnapshotRetirement::with_capacity(2).unwrap();
    state
        .commit_retaining(prepare(&state, "b"), &mut retirement)
        .unwrap();
    let b = state.snapshot().unwrap();
    let b_program = Arc::downgrade(&b.model.program);
    drop(b);
    state
        .commit_retaining(prepare(&state, "c"), &mut retirement)
        .unwrap();
    assert_eq!(retirement.reclaim(1), 0);
    assert_eq!(retirement.reclaim(1), 1, "blocked a must not starve b");
    assert!(b_program.upgrade().is_none());
    let upgraded_reader = a_weak.upgrade().unwrap();
    drop(a);
    assert_eq!(retirement.reclaim(1), 0, "a weak upgrade is a real reader");
    drop(upgraded_reader);
    let count = thread::spawn(move || retirement.reclaim(1)).join().unwrap();
    assert_eq!(count, 1);
    assert!(a_weak.upgrade().is_none());
}

#[test]
fn retained_rejections_preserve_queue_and_candidate_ownership() {
    let state = ExecutionState::new(model("a"));
    let other = ExecutionState::new(model("other"));
    let mut retirement = SnapshotRetirement::with_capacity(2).unwrap();
    let candidate = prepare(&state, "b");
    let error = other
        .commit_retaining(candidate, &mut retirement)
        .err()
        .unwrap();
    assert!(matches!(
        &error,
        RetainedPublicationError::Rejected {
            error: PublicationError::ForeignState,
            ..
        }
    ));
    assert!(error.to_string().contains("ForeignState"));
    assert!(std::error::Error::source(&error).is_some());
    assert!(format!("{error:?}").contains("ForeignState"));
    assert!(retirement.is_empty());
    let stale = prepare(&state, "stale");
    state
        .commit_retaining(error.into_candidate(), &mut retirement)
        .unwrap();
    let size = retirement.len();
    let error = state
        .commit_retaining(stale, &mut retirement)
        .err()
        .unwrap();
    assert!(matches!(
        &error,
        RetainedPublicationError::Rejected {
            error: PublicationError::StaleBase { .. },
            ..
        }
    ));
    assert_eq!(retirement.len(), size);
    let recovered = error.into_candidate();
    assert_eq!(recovered.base.revision(), 0);
    assert!(names(&state.snapshot().unwrap()).contains(&"b"));
}

#[test]
fn retained_poison_and_exhaustion_return_the_original_model() {
    for poison in [false, true] {
        let state = ExecutionState::new(model("old"));
        if !poison {
            let mut current = state.current.write().unwrap();
            Arc::get_mut(&mut current).unwrap().revision = u64::MAX;
        }
        let candidate = prepare(&state, "candidate");
        let original_model = Arc::downgrade(&candidate.model.program);
        if poison {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let _guard = state.current.write().unwrap();
                panic!("deliberate publication poison");
            }));
        }
        let mut retirement = SnapshotRetirement::with_capacity(1).unwrap();
        let error = state
            .commit_retaining(candidate, &mut retirement)
            .err()
            .unwrap();
        let RetainedPublicationError::Rejected { error, candidate } = error else {
            panic!("publication error");
        };
        assert_eq!(
            error,
            if poison {
                PublicationError::Poisoned
            } else {
                PublicationError::RevisionExhausted
            }
        );
        assert!(Arc::ptr_eq(
            &candidate.model.program,
            &original_model.upgrade().unwrap()
        ));
        assert!(retirement.is_empty());
    }
}

#[test]
fn replacement_handling_is_outside_the_publication_lock() {
    let state = ExecutionState::new(model("old"));
    let mut observed = false;
    let result = state.publish(prepare(&state, "new"), |previous| {
        assert!(state.current.try_write().is_ok());
        assert_eq!(previous.revision(), 0);
        observed = true;
    });
    assert!(result.is_ok());
    assert!(observed);
}

#[test]
fn isolated_compilation_matches_the_canonical_pipeline() {
    let input = MemProvider::default().with_file(
        "model.hymeko",
        r#"
        world {}
        network {
            alpha { mass 2.0; matrix [[1.0, 2.0], [3.0, 4.0]]; }
            beta {}
            @flow { (+alpha 0.5, -beta, ~alpha, +alpha 1.5); }
        }
    "#,
    );
    let mut oracle = ModuleStore::new(input.clone(), Parser);
    let expected = oracle.compile(Path::new("model.hymeko")).unwrap();
    let actual = CompiledModel::compile(input, Parser, Path::new("model.hymeko")).unwrap();
    assert_eq!(
        Canon::new(&actual.program().ir, actual.interner()).hash(),
        Canon::new(&expected.ir, &oracle.it).hash()
    );
    assert_eq!(actual.program().canon, expected.canon);
    assert_eq!(actual.program().ir.edges.len(), 1);
    assert_eq!(actual.program().ir.arcs.len(), 1);
    let state = ExecutionState::new(actual);
    let first = state.snapshot().unwrap();
    assert_eq!(first.revision(), 0);
    assert!(names(&first).contains(&"alpha"));
    assert!(Arc::ptr_eq(&first, &state.snapshot().unwrap()));
}

#[test]
fn source_updates_use_fresh_stores_and_old_readers_keep_their_interner() {
    let state = ExecutionState::new(model("old"));
    let retained = state.snapshot().unwrap();
    let stale = prepare(&state, "from_original_base");
    for (revision, name) in [(1, "beta"), (2, "gamma"), (3, "old")] {
        let published = state.commit(prepare(&state, name)).unwrap();
        assert_eq!(published.revision(), revision);
        assert!(names(&published).contains(&name));
        assert!(Arc::ptr_eq(&published, &state.snapshot().unwrap()));
        assert!(names(&retained).contains(&"old"));
        assert_eq!(retained.revision(), 0);
    }
    // Returning to the same content must not revive an old publication token (ABA).
    assert_eq!(
        state.commit(stale).err(),
        Some(PublicationError::StaleBase {
            expected: 0,
            actual: 3,
        })
    );
    drop(state);
    assert!(names(&retained).contains(&"old"));
}

#[test]
fn failed_compilation_and_abandoned_candidates_do_not_publish() {
    let state = ExecutionState::new(model("old"));
    let original = state.snapshot().unwrap();
    let result = state.prepare(
        MemProvider::default().with_file("model.hymeko", "world {"),
        Parser,
        Path::new("model.hymeko"),
    );
    let error = result.err().unwrap();
    assert!(matches!(error, PrepareError::Compile(_)));
    assert!(error.to_string().contains("preparation failed"));
    drop(prepare(&state, "discarded"));
    assert!(Arc::ptr_eq(&original, &state.snapshot().unwrap()));
}

#[test]
fn parser_panic_does_not_poison_publication() {
    let state = ExecutionState::new(model("initial"));
    let original = state.snapshot().unwrap();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = state.prepare(
            source("panicking"),
            CallbackParser(|| panic!("parser panic")),
            Path::new("model.hymeko"),
        );
    }));
    assert!(panic.is_err());
    assert!(Arc::ptr_eq(&original, &state.snapshot().unwrap()));
    let recovered = state.commit(prepare(&state, "recovered")).unwrap();
    assert_eq!(recovered.revision(), 1);
    assert!(names(&recovered).contains(&"recovered"));
}

#[test]
fn foreign_candidate_is_rejected_even_at_the_same_revision() {
    let first = ExecutionState::new(model("first"));
    let second = ExecutionState::new(model("second"));
    let original = second.snapshot().unwrap();
    assert_eq!(
        second.commit(prepare(&first, "foreign")).err(),
        Some(PublicationError::ForeignState)
    );
    assert!(Arc::ptr_eq(&original, &second.snapshot().unwrap()));
    assert_eq!(first.snapshot().unwrap().revision(), 0);
}

pub(super) struct CallbackParser<F>(pub(super) F);

impl<F: Fn()> HymekoParser for CallbackParser<F> {
    fn parse<'a>(&self, source: &'a str) -> Result<parser::ast::AstStr<'a>, String> {
        (self.0)();
        Parser.parse(source)
    }
}

#[test]
fn same_base_workers_compile_concurrently_and_exactly_one_publishes() {
    let state = ExecutionState::new(model("initial"));
    let compiling = Barrier::new(2);
    let committing = Barrier::new(2);
    let outcomes = thread::scope(|scope| {
        let workers: Vec<_> = ["left", "right"]
            .into_iter()
            .map(|name| {
                let state = &state;
                let compiling = &compiling;
                let committing = &committing;
                scope.spawn(move || {
                    let candidate = state
                        .prepare(
                            source(name),
                            CallbackParser(|| {
                                compiling.wait();
                            }),
                            Path::new("model.hymeko"),
                        )
                        .unwrap();
                    committing.wait();
                    state.commit(candidate).map(|snapshot| {
                        assert!(names(&snapshot).contains(&name));
                        snapshot.revision()
                    })
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        outcomes.iter().filter(|outcome| **outcome == Ok(1)).count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome
                == Err(PublicationError::StaleBase {
                    expected: 0,
                    actual: 1,
                }))
            .count(),
        1
    );
    assert_eq!(state.snapshot().unwrap().revision(), 1);
}

#[test]
fn concurrent_readers_keep_one_snapshot_across_publication() {
    let state = ExecutionState::new(model("initial"));
    let acquired = Barrier::new(3);
    let published = Barrier::new(3);
    thread::scope(|scope| {
        for _ in 0..2 {
            let state = &state;
            let acquired = &acquired;
            let published = &published;
            scope.spawn(move || {
                let snapshot = state.snapshot().unwrap();
                acquired.wait();
                published.wait();
                assert_eq!(snapshot.revision(), 0);
                assert!(names(&snapshot).contains(&"initial"));
                assert!(names(&state.snapshot().unwrap()).contains(&"replacement"));
            });
        }
        acquired.wait();
        state.commit(prepare(&state, "replacement")).unwrap();
        published.wait();
    });
}

#[test]
fn compiler_callbacks_run_outside_publication_lock() {
    let state = ExecutionState::new(model("initial"));
    let candidate = state
        .prepare(
            source("outer"),
            CallbackParser(|| {
                state.commit(prepare(&state, "interleaved")).unwrap();
            }),
            Path::new("model.hymeko"),
        )
        .unwrap();
    assert_eq!(
        state.commit(candidate).err(),
        Some(PublicationError::StaleBase {
            expected: 0,
            actual: 1,
        })
    );
    assert!(names(&state.snapshot().unwrap()).contains(&"interleaved"));
}

#[test]
fn revision_exhaustion_and_poison_are_explicit_and_do_not_wrap() {
    let state = ExecutionState::new(model("initial"));
    {
        let mut current = state.current.write().unwrap();
        Arc::get_mut(&mut current).unwrap().revision = u64::MAX;
    }
    let original = state.snapshot().unwrap();
    assert_eq!(
        state.commit(prepare(&state, "overflow")).err(),
        Some(PublicationError::RevisionExhausted)
    );
    assert!(Arc::ptr_eq(&original, &state.snapshot().unwrap()));
    let candidate = prepare(&state, "poison");
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _lock = state.current.write().unwrap();
            panic!("test-only writer panic");
        }))
        .is_err()
    );
    assert_eq!(state.snapshot().err(), Some(PublicationError::Poisoned));
    assert_eq!(
        state.commit(candidate).err(),
        Some(PublicationError::Poisoned)
    );
    let error = state
        .prepare(source("unused"), Parser, Path::new("model.hymeko"))
        .err()
        .unwrap();
    assert!(matches!(
        error,
        PrepareError::Publication(PublicationError::Poisoned)
    ));
    assert!(error.to_string().contains("Poisoned"));
    assert_eq!(original.revision(), u64::MAX);
    assert!(names(&original).contains(&"initial"));
}

fn incidence_model() -> CompiledModel {
    let input = MemProvider::default().with_file(
        "model.hymeko",
        "world {} network { a { w 1; } b {} c {} @e { (+ a, - b); } @f { (+ b, ~ c, - a); } }",
    );
    CompiledModel::compile(input, Parser, Path::new("model.hymeko")).unwrap()
}

/// The model lends the index of its own IR: equal to a fresh build, the same one on every call.
#[test]
fn model_incidences_are_derived_once_and_match_a_fresh_index() {
    let model = incidence_model();
    let ir = &model.program().ir;
    let incidences: usize = ir.arcs.iter().map(|arc| arc.refs.len()).sum();
    assert_eq!(
        (ir.arcs.len(), incidences),
        (2, 5),
        "two hyperedges, five incidences"
    );
    let view = model.incidences().unwrap();
    let fresh = crate::ir::incidence::PreparedIr::new(ir.clone()).unwrap();
    for decl in (0..ir.decl_nodes.len()).map(crate::common::ids::DeclId::new) {
        assert_eq!(
            view.incident_arcs(decl),
            fresh.incidences().incident_arcs(decl)
        );
        assert_eq!(
            view.incident_slots(decl),
            fresh.incidences().incident_slots(decl)
        );
    }
    assert!(std::ptr::eq(view.ir(), ir));
    assert!(std::ptr::eq(
        view.data(),
        model.incidences().unwrap().data()
    ));
}

/// Checked value updates keep every declaration, arc and slot, so the updated model shares a
/// built index, which stays exact for its IR. An unbuilt one is left to be built on demand.
#[test]
fn value_updated_models_share_the_incidence_index() {
    let base = incidence_model();
    let (w, _) = base
        .program()
        .ir
        .decls()
        .find(|(_, decl)| base.interner().resolve(decl.name) == "w")
        .unwrap();
    let update = [ValueUpdate::Set {
        declaration: w,
        value: crate::ir::ir::ValueR::Num(2.),
    }];
    assert!(
        base.with_value_updates(&update)
            .unwrap()
            .incidence
            .get()
            .is_none()
    );
    let built: *const _ = base.incidences().unwrap().data();
    let updated = base.with_value_updates(&update).unwrap();
    let view = updated.incidences().unwrap();
    assert!(std::ptr::eq(view.data(), built));
    assert!(view.data().validate(&updated.program().ir).is_ok());
}

/// A broken IR reports the error a direct build reports, and keeps it.
#[test]
fn a_broken_ir_reports_and_keeps_its_incidence_error() {
    let mut model = incidence_model();
    Arc::get_mut(&mut model.program)
        .unwrap()
        .ir
        .decl_nodes
        .truncate(1);
    let expected =
        crate::ir::incidence::IncidenceIndexData::build(&model.program().ir).unwrap_err();
    assert_eq!(model.incidences().err(), Some(expected));
    assert_eq!(model.incidences().err(), Some(expected));
}
