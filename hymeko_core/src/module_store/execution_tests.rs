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
