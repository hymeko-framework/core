//! Shared corpus access for the unified-IR suite.
//!
//! APPROVED-CORE-EDIT: unified-ir-m2b-vv-corrective (2026-08-29).
//!
//! Consolidates four independent copies of the same `.hymeko` directory walk
//! that had accumulated across the M0.5, M0.6 and M2a phases. They differed
//! only in variable names. `CLAUDE.md` §6.1: the same traversal in three or
//! more places is a refactor trigger, not a feature.
//!
//! # Fail-closed
//!
//! The corpus is **required**, not optional. Two things changed in the
//! corrective phase:
//!
//! 1. The root is resolved from `CARGO_MANIFEST_DIR`, not from a relative path
//!    against the process working directory. A relative `../data` happens to
//!    work under `cargo test` and silently resolves elsewhere under any other
//!    runner, which would have turned every corpus test into a no-op.
//! 2. [`require_fixtures`] **panics** when the corpus is missing, empty or
//!    unreadable. The previous form printed `skipped` and returned, so a
//!    validation suite that validated nothing still reported green — the
//!    failure mode a validation suite exists to prevent.
//!
//! A genuinely optional diagnostic probe may still call [`fixture_files`] and
//! decide for itself, but it must then be labelled diagnostic and excluded from
//! the normative validation rate.

use std::path::{Path, PathBuf};

/// The tracked fixture root, resolved against this crate's manifest.
///
/// `CARGO_MANIFEST_DIR` is `…/core/hymeko_core`, and the corpus is the sibling
/// `…/core/data`.
#[must_use]
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR always has a parent")
        .join("data")
}

/// Every `.hymeko` fixture under the corpus root, in a deterministic order.
///
/// Returns an empty vector when the root does not exist. Callers that require
/// the corpus should use [`require_fixtures`] instead.
#[must_use]
pub fn fixture_files() -> Vec<PathBuf> {
    let root = corpus_root();
    if !root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // An unreadable directory is not "no fixtures" — it is a broken
            // corpus, and `require_fixtures` turns it into a failure via the
            // count check rather than pretending the tree is smaller.
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "hymeko") {
                out.push(p);
            }
        }
    }
    // Sorting is what makes every corpus-derived digest reproducible.
    out.sort();
    out
}

/// The corpus, or a failure.
///
/// # Panics
///
/// Panics when the corpus root is absent, is not a directory, is unreadable, or
/// contains no `.hymeko` fixture. Every one of those means the validation about
/// to run would validate nothing, and reporting that as a pass is worse than
/// reporting it as a failure.
#[must_use]
pub fn require_fixtures() -> Vec<PathBuf> {
    let root = corpus_root();
    assert!(
        root.exists(),
        "fixture corpus root does not exist: {}",
        root.display()
    );
    assert!(
        root.is_dir(),
        "fixture corpus root is not a directory: {}",
        root.display()
    );
    std::fs::read_dir(&root).unwrap_or_else(|e| {
        panic!(
            "fixture corpus root is unreadable: {} ({e})",
            root.display()
        )
    });

    let files = fixture_files();
    assert!(
        !files.is_empty(),
        "fixture corpus at {} contains no .hymeko files; validation would assert nothing",
        root.display()
    );
    files
}

/// Number of fixtures found. For diagnostic probes only.
#[must_use]
pub fn fixture_count() -> usize {
    fixture_files().len()
}
