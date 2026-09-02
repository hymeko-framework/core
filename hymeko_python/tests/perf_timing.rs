//! Rust-side performance baseline (mission §10). Times the pipeline stages
//! (compile = parse+resolve+lower, coherence validation, canonical hash) and the
//! Rust-only arc-major SoA batch build, on deterministic windowed workloads at
//! small / medium / 1e5-incidence scale. Also serves as the production-scale (1e5)
//! smoke for the real pipeline. Run: `cargo test -p hymeko_python --test perf_timing
//! --release -- --nocapture`.

use std::time::Instant;

use hymeko::ir::canonical_hash::{CanonHashCfg, canonical_program_hash};
use hymeko::ir::coherence::{CoherenceLimits, validate_hierarchy_index_coherence};
use hymeko::ir::ir::Ir;
use hymeko::module_store::module_store::ModuleStore;
use hymeko::module_store::source_provider::MemProvider;
use hymeko::util::real_parser::RealParser;
use std::path::Path;

fn median_ms(iters: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..2 {
        f();
    }
    let mut s = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        s.push(t.elapsed().as_secs_f64() * 1e3);
    }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

/// Deterministic windowed `.hymeko` source (mirrors the Python `windowed_source`).
fn windowed_source(n_nodes: usize, arity: usize) -> String {
    let mut s = String::from("Scale{}\n");
    for i in 0..n_nodes {
        s.push_str(&format!("N{i};\n"));
    }
    s.push_str("@E {\n");
    for i in 0..(n_nodes - arity + 1) {
        let refs: Vec<String> = (i..i + arity)
            .map(|j| format!("{}N{j}", if j % 3 != 0 { "+" } else { "-" }))
            .collect();
        s.push_str(&format!("  ({});\n", refs.join(", ")));
    }
    s.push_str("}\n");
    s
}

fn compile(
    src: &str,
) -> (
    Ir,
    hymeko::resolution::resolve::Index,
    hymeko::resolution::interner::Interner,
) {
    let fs = MemProvider::default().with_file("root.hymeko", src.to_owned());
    let mut store = ModuleStore::new(fs, RealParser);
    let compiled = store.compile(Path::new("root.hymeko")).expect("compile");
    let it = std::mem::take(&mut store.it);
    (compiled.ir.clone(), compiled.idx.clone(), it)
}

/// Rust-only arc-major SoA batch build (the baseline the Python `incidence_arrays()`
/// bulk copy is measured against — same bytes, minus the `PyBytes` allocation).
fn rust_batch_bytes(ir: &Ir) -> usize {
    let nnz: usize = ir.arcs.iter().map(|a| a.refs.len()).sum();
    let mut offs: Vec<u8> = Vec::with_capacity((ir.arcs.len() + 1) * 4);
    let mut targets: Vec<u8> = Vec::with_capacity(nnz * 4);
    let mut signs: Vec<u8> = Vec::with_capacity(nnz);
    let mut slots: Vec<u8> = Vec::with_capacity(nnz * 4);
    let mut weights: Vec<u8> = Vec::with_capacity(nnz * 8);
    let mut acc: u32 = 0;
    offs.extend_from_slice(&acc.to_le_bytes());
    for arc in &ir.arcs {
        for (slot, sref) in arc.refs.iter().enumerate() {
            targets.extend_from_slice(&(sref.target().raw() as u32).to_le_bytes());
            signs.push(sref.sign() as u8);
            slots.extend_from_slice(&(slot as u32).to_le_bytes());
            let w: f64 = 1.0;
            weights.extend_from_slice(&w.to_le_bytes());
        }
        acc += arc.refs.len() as u32;
        offs.extend_from_slice(&acc.to_le_bytes());
    }
    offs.len() + targets.len() + signs.len() + slots.len() + weights.len()
}

#[test]
fn rust_stage_and_batch_timing() {
    println!("\n=== Rust-side pipeline + batch timing (release) ===");
    for (label, n, k) in [
        ("small", 204usize, 5usize),
        ("medium", 2004, 5),
        ("1e5", 100_004, 5),
    ] {
        let src = windowed_source(n, k);
        let nnz = (n - k + 1) * k;
        let compile_ms = median_ms(5, || {
            let _ = compile(&src);
        });
        let (ir, idx, it) = compile(&src);
        let coh_ms = median_ms(5, || {
            let _ = validate_hierarchy_index_coherence(&ir, &idx, &it, CoherenceLimits::default());
        });
        let hash_ms = median_ms(5, || {
            let _ = canonical_program_hash(CanonHashCfg::default(), &idx, &ir, &it);
        });
        let batch_ms = median_ms(5, || {
            let _ = rust_batch_bytes(&ir);
        });
        println!(
            "{label:6} nnz={nnz:>7}  compile={compile_ms:8.3}ms  coherence={coh_ms:7.3}ms  hash={hash_ms:7.3}ms  rust_batch={batch_ms:6.3}ms"
        );
        assert!(ir.arc_count() >= 1);
    }
}
