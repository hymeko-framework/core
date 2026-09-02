//! Rust-side canonical-hash oracle. Computes the single-model canonical hash for a
//! set of committed fixtures via the core pipeline (independently of the binding's
//! precomputed `canon_hash`) and asserts frozen hex values. The Python test suite
//! (`tests/python/test_hymeko.py::test_canonical_hash_parity_with_rust_oracle`)
//! asserts the SAME hex for the SAME fixtures loaded through `hymeko.load` — the two
//! together prove Python↔Rust hash parity (mission §8, §12.7).
//!
//! Regenerate the frozen values by running `cargo test -p hymeko_python
//! --test hash_oracle -- --nocapture` and copying the printed hex here and into the
//! Python oracle map.

use std::path::{Path, PathBuf};

use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::module_store::module_store::ModuleStore;
use hymeko::module_store::source_provider::StdFsProvider;
use hymeko::util::real_parser::RealParser;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("data")
        .join(rel)
}

fn hash_hex(rel: &str) -> String {
    let mut store = ModuleStore::new(StdFsProvider::new(), RealParser);
    let compiled = store
        .compile(&fixture(rel))
        .unwrap_or_else(|e| panic!("compile {rel} failed: {e:?}"));
    // Independent recompute must equal compile()'s precomputed canon_hash.
    let direct = canonical_program_hash(
        CanonHashCfg::default(),
        &compiled.idx,
        &compiled.ir,
        &store.it,
    );
    assert_eq!(
        direct, compiled.canon_hash,
        "direct canonical_program_hash != compile()'s canon_hash for {rel}"
    );
    direct.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// The frozen oracle: fixture → canonical hash. Shared with the Python suite.
const ORACLE: &[(&str, &str)] = &[
    (
        "minimal_examples/minimal_example.hymeko",
        "e7e1f1a305fb5e8754605252f6c351cd9af84211a27ee64fc27d09d0ab2ff738",
    ),
    (
        "minimal_examples/minimal_example_base_elements.hymeko",
        "ee3db3cb11ae66946dd21dbd8c7b6c6593c8ec0362f541ee12564003bde4609b",
    ),
    (
        "minimal_examples/minimal_example_basic_hierarchy.hymeko",
        "1f85625feaf93ec507363cc89bd3eb3f13504a90c408cc486db00c87c705195f",
    ),
    (
        "minimal_examples/tag_annotations/minimal_tag_annotation.hymeko",
        "90e3e0f1d071298dcf6ec9e45ed29ec84aa197715fc008e86887127f74a43a51",
    ),
    (
        "minimal_examples/minimal_example_fields_with_reference.hymeko",
        "93ac4fe4c8f7bcfd594656e25add2f1faea414948adf8905482c1e56f108475f",
    ),
    (
        "typical_graphs/fano_graph.hymeko",
        "b88715ed80e62f247ea71f57e0ffb4f450e86a799e630186ea5ec6a063c86dc5",
    ),
    (
        "typical_graphs/k4_3uniform.hymeko",
        "0a8a903e3f89660db98fb603d06d4993acea1b79e2bed04c8a7a5afe91294237",
    ),
    (
        "typical_graphs/sunflower_delta_system.hymeko",
        "194c71e4164a7aaef3897b59622767fe6533929dde231c2a5d707675dfd2914e",
    ),
    (
        "typical_graphs/generic_hypergraph.hymeko",
        "6a0c09d8cf24f1aa52223ba1378f900516b4fdb1d6495ab122bd8dee35242f89",
    ),
];

#[test]
fn print_and_check_oracle() {
    for (rel, frozen) in ORACLE {
        let hex = hash_hex(rel);
        println!("ORACLE  {rel}  =>  {hex}");
        if !frozen.starts_with("PLACEHOLDER") {
            assert_eq!(&hex, frozen, "frozen hash drift for {rel}");
        }
    }
}
