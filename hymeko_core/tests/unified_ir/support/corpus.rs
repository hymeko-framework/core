//! Shared corpus access for the unified-IR suite.
//!
//! APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers (2026-08-29).
//!
//! Consolidates four independent copies of the same `.hymeko` directory walk
//! that had accumulated across the M0.5, M0.6 and M2a phases (in `depth.rs`,
//! `coherence_tests.rs`, `frozen_v1.rs` and `accessors.rs`). They differed only
//! in variable names. `CLAUDE.md` §6.1: the same traversal in three or more
//! places is a refactor trigger, not a feature.

use std::path::{Path, PathBuf};

/// Every `.hymeko` fixture under `../data`, in a deterministic order.
///
/// Empty when the corpus is absent, so callers report a skip rather than fail —
/// the corpus is a working-tree artifact, not a build dependency.
#[must_use]
pub fn fixture_files() -> Vec<PathBuf> {
    let root = Path::new("../data");
    if !root.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
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

/// Number of fixtures found. Callers report a skip when this is zero.
#[must_use]
pub fn fixture_count() -> usize {
    fixture_files().len()
}
