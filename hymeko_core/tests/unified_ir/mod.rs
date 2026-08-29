//! Unified IR verification and validation suite.
//!
//! APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers (2026-08-29).
//!
//! Organised by V&V role rather than by the phase that produced it.
//! Phase provenance is preserved in `docs/verification/unified-ir/README.md`
//! and in each module's own header, not in directory names — phase-numbered
//! directories stop being navigable as soon as the phase is over.
//!
//! * `support` — builders, semantic oracle, corpus access. No tests.
//! * `verification` — spec conformance on synthetic inputs with known answers.
//! * `validation` — behaviour on the real 96-fixture corpus and on real
//!   consumer code paths.

pub mod support;
pub mod validation;
pub mod verification;
