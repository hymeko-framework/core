//! Product-CLI conformance for the single user-facing `hymeko` binary.
//!
//! The framework's architectural invariant is *one* user-facing CLI. This binary
//! (package `hymeko_core`, the `hymeko` command) is that entrypoint; these tests
//! exercise its three externally-observable outcomes — success, usage error, and
//! missing-file error — through the **already-built** executable named by
//! `CARGO_BIN_EXE_hymeko_core`. They do **not** invoke Cargo recursively.
//!
//! Scope note: this is a conformance gate on the *current* minimal CLI (compile +
//! pretty-print, no subcommands). The multi-CLI consolidation (folding `pgraph`
//! and `hymeko_pgraph_dump` in as subcommands) is a later production change; see
//! `docs/unified-ir-single-cli.md`.

use std::path::PathBuf;
use std::process::Command;

/// Path to the pre-built `hymeko` binary (Cargo sets this for integration tests
/// of a crate that has a `[[bin]]`/`src/main.rs`). Using it avoids a recursive
/// `cargo run` inside the test.
fn hymeko_bin() -> &'static str {
    env!("CARGO_BIN_EXE_hymeko_core")
}

/// A unique temp path for this test process; no fixture under the locked
/// `data/**` is written.
fn temp_hymeko(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hymeko_cli_{}_{}.hymeko", tag, std::process::id()))
}

const MINIMAL_SOURCE: &str = "cli_probe\n{}\ncontext\n{\n    node0{}\n    node1{}\n    @e0 { ( - node0[0.85], + context.node1[0.9]); }\n}\n";

#[test]
fn product_cli_compiles_a_valid_source_and_succeeds() {
    let path = temp_hymeko("ok");
    std::fs::write(&path, MINIMAL_SOURCE).expect("write temp source");

    let out = Command::new(hymeko_bin())
        .arg(&path)
        .output()
        .expect("spawn hymeko");

    let _ = std::fs::remove_file(&path);

    assert!(
        out.status.success(),
        "valid source must compile (exit 0); got {:?}, stderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.stdout.is_empty(),
        "a successful compile pretty-prints the module to stdout"
    );
}

#[test]
fn product_cli_reports_usage_without_arguments() {
    let out = Command::new(hymeko_bin()).output().expect("spawn hymeko");

    assert_eq!(
        out.status.code(),
        Some(2),
        "no argument is a usage error (exit 2)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Usage"),
        "usage error must print a usage line; got:\n{stderr}"
    );
}

#[test]
fn product_cli_reports_a_missing_input_file() {
    let missing = temp_hymeko("missing_never_created");
    // Guard: ensure it truly does not exist.
    let _ = std::fs::remove_file(&missing);

    let out = Command::new(hymeko_bin())
        .arg(&missing)
        .output()
        .expect("spawn hymeko");

    assert_eq!(
        out.status.code(),
        Some(1),
        "a missing input file is a compile failure (exit 1)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("compile failed"),
        "the failure must be reported on stderr; got:\n{stderr}"
    );
}
