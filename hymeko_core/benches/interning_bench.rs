// Criterion harness entry; keep clippy permissive for micro-benchmark noise.
#![allow(warnings)]

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use hymeko::{
    ir::{
        canonical_hash::{CanonHashCfg, CanonicalPaths, NamespaceValidator},
        lower::lower_to_ir,
    },
    module_store::{module_store::ModuleStore, source_provider::MemProvider},
    resolution::{
        const_resolve::resolve_consts,
        intern_pass::{intern_ast, intern_ast_into},
        interner::Interner,
        resolve::build_index_sym,
    },
    util::real_parser::RealParser,
};
use parser::lexer::simd::{Avx2Lexer, CoreLexer, ScalarLexer};
use parser::parse_description;
use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn generate_large_hypergraph(nodes: usize, annotation: &str) -> String {
    let mut s = String::from("Description{} Benchmark_Graph {\n");
    for i in 0..nodes {
        s.push_str(&format!(
            "  node_{} {{\n    @edge_e{} {{ (+node_{} {annotation}); }}\n  }}\n",
            i, i, i
        ));
    }
    s.push_str("}");
    s
}

fn bench_interning_performance(c: &mut Criterion) {
    // Generate a large synthetic graph string (~1MB+)
    // to minimize measurement noise.
    let input = generate_large_hypergraph(50_000, "[0.5]");
    let mut group = c.benchmark_group("hymeko_frontend");

    group.throughput(Throughput::Bytes(input.len() as u64));

    // Test 1: Raw SIMD Lexing (Zero-Copy)
    group.bench_function("lexer_dispatch_and_scan", |b| {
        b.iter(|| {
            let core = CoreLexer::new(black_box(&input));

            #[cfg(target_arch = "x86_64")]
            {
                if std::is_x86_feature_detected!("avx2") {
                    let lex = Avx2Lexer(core);
                    for tok in lex {
                        let _ = black_box(tok);
                    }
                    return;
                }
            }
            let lex = ScalarLexer(core);
            for tok in lex {
                let _ = black_box(tok);
            }
        })
    });

    // Test 2: Full Interning Pass
    group.bench_function("full_frontend_pipeline", |b| {
        b.iter(|| {
            let ast = parse_description(black_box(&input)).unwrap();
            let _ = black_box(intern_ast(black_box(&ast)));
        })
    });

    group.finish();
}

// A phase decomposition, not additive end-to-end numbers: each phase receives
// prepared input outside the timer, and PerIteration excludes final destruction.
// Reuses the frontend fixture above, so this remains one benchmark owner.
fn bench_compiler_phases(c: &mut Criterion) {
    // Explicit harness input; larger scales retain distinct Criterion identities.
    let nodes = std::env::var("HYMEKO_BENCH_NODES")
        .map(|value| {
            value
                .parse::<usize>()
                .expect("HYMEKO_BENCH_NODES must be an integer")
        })
        .unwrap_or(50_000);
    assert!(
        nodes > 0 && nodes <= 500_000,
        "supported fixture size: 1..=500000"
    );
    let payload = std::env::var("HYMEKO_BENCH_PAYLOAD").unwrap_or_else(|_| "numeric".into());
    let annotation = match payload.as_str() {
        "numeric" => "[0.5]",
        "mixed" => "[node_0, \"payload\", [node_0, -0.0, node_0]]",
        _ => panic!("HYMEKO_BENCH_PAYLOAD must be numeric or mixed"),
    };
    let input: Arc<str> = generate_large_hypergraph(nodes, annotation).into();
    let parsed = parse_description(&input).unwrap();
    let intern = || {
        let mut it = Interner::new();
        let ast = intern_ast_into(&parsed, &mut it);
        (ast, it)
    };
    let (mut ast, mut it) = intern();
    resolve_consts(&mut ast, &it).unwrap();
    let idx = build_index_sym(&ast, &it).unwrap();
    let ir = lower_to_ir(&ast, &idx, &mut it).unwrap();
    let path = Path::new("benchmark.hymeko");
    let make_store = || {
        ModuleStore::new(
            MemProvider::default().with_file(path, input.clone()),
            RealParser,
        )
    };
    let mut cached_store = make_store();
    let expected = cached_store.compile(path).unwrap();
    eprintln!(
        "COMPILER_FIXTURE bytes={} blake3={} nodes={} edges={} arcs={} decls={} canonical_hash={:?}",
        input.len(),
        blake3::hash(input.as_bytes()),
        expected.ir.nodes.len(),
        expected.ir.edges.len(),
        expected.ir.arcs.len(),
        expected.ir.decl_nodes.len(),
        expected.canon_hash,
    );
    assert_eq!(expected.ir.decl_nodes.len(), ir.decl_nodes.len());
    assert_eq!(
        expected.canon_hash,
        CanonicalPaths::new(&ir).hash(CanonHashCfg::default(), &it)
    );
    let scale = if nodes % 1000 == 0 {
        format!("{}k", nodes / 1000)
    } else {
        nodes.to_string()
    };
    let suffix = if payload == "numeric" {
        String::new()
    } else {
        format!("_{payload}")
    };
    let mut group = c.benchmark_group(format!("compiler_phases_{scale}{suffix}"));
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_function("parse", |b| {
        b.iter_batched(
            || (),
            |()| parse_description(black_box(&input)).unwrap(),
            BatchSize::PerIteration,
        )
    });
    group.bench_function("intern", |b| {
        b.iter_batched(|| (), |()| intern(), BatchSize::PerIteration)
    });
    group.bench_function("resolve_consts", |b| {
        b.iter_batched_ref(
            || ast.clone(),
            |value| resolve_consts(value, &it).unwrap(),
            BatchSize::PerIteration,
        )
    });
    group.bench_function("owned_ast_clone", |b| {
        b.iter_batched(|| (), |()| black_box(&ast).clone(), BatchSize::PerIteration)
    });
    group.bench_function("index", |b| {
        b.iter_batched(
            || (),
            |()| build_index_sym(black_box(&ast), &it).unwrap(),
            BatchSize::PerIteration,
        )
    });
    group.bench_function("lower", |b| {
        b.iter_batched(
            || (),
            |()| lower_to_ir(black_box(&ast), &idx, &mut it).unwrap(),
            BatchSize::PerIteration,
        )
    });
    group.bench_function("namespace_validation", |b| {
        b.iter(|| {
            NamespaceValidator::new(&CanonicalPaths::new(black_box(&ir)))
                .validate(&idx)
                .unwrap()
        })
    });
    group.bench_function("canonical_hash", |b| {
        b.iter(|| CanonicalPaths::new(black_box(&ir)).hash(CanonHashCfg::default(), &it))
    });
    group.bench_function("compile_cold", |b| {
        b.iter_batched(
            make_store,
            |mut store| {
                let result = store.compile(path).unwrap();
                (store, result)
            },
            BatchSize::PerIteration,
        )
    });
    // A cache hit processes no source bytes; report lookups, not fictitious GB/s.
    group.throughput(Throughput::Elements(1));
    group.bench_function("compile_cached", |b| {
        b.iter(|| {
            let result = cached_store.compile(path).unwrap();
            assert!(Arc::ptr_eq(&expected, &result));
            black_box(result)
        })
    });
    group.finish();
}

// Characterize the public digest/update paths separately: the update includes
// model copying and validation, so it is not an isolated hash microbenchmark.
fn bench_semantic_digest(c: &mut Criterion) {
    use hymeko::common::ids::DeclId;
    use hymeko::ir::ir::ValueR;
    use hymeko::ir::semantic_digest::semantic_model_digest;
    use hymeko::module_store::execution::{CompiledModel, ValueUpdate};

    let input: Arc<str> = generate_large_hypergraph(1_000, "[0.5]")
        .replacen(
            "Benchmark_Graph {",
            "Benchmark_Graph { benchmark_value 0;",
            1,
        )
        .into();
    let path = Path::new("digest-benchmark.hymeko");
    let model = CompiledModel::compile(
        MemProvider::default().with_file(path, input.clone()),
        RealParser,
        path,
    )
    .unwrap();
    let declaration = model
        .program()
        .ir
        .decl_nodes
        .iter()
        .enumerate()
        .find(|(_, node)| model.interner().resolve(node.name) == "benchmark_value")
        .map(|(index, _)| DeclId::new(index))
        .unwrap();
    let updates = [ValueUpdate::Set {
        declaration,
        value: ValueR::Num(1.0),
    }];
    let before = model.semantic_digest();
    assert_eq!(
        before,
        semantic_model_digest(&model.program().ir, model.interner())
    );
    let changed = model.with_value_updates(&updates).unwrap();
    assert_ne!(before, changed.semantic_digest());
    assert_eq!(
        changed.semantic_digest(),
        semantic_model_digest(&changed.program().ir, changed.interner())
    );
    eprintln!(
        "DIGEST_FIXTURE nodes=1000 bytes={} blake3={} decls={}",
        input.len(),
        blake3::hash(input.as_bytes()),
        model.program().ir.decl_nodes.len()
    );

    let mut group = c.benchmark_group("semantic_digest_1k");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    group.bench_function("full", |b| {
        b.iter(|| {
            semantic_model_digest(black_box(&model.program().ir), black_box(model.interner()))
        })
    });
    group.bench_function("cached", |b| b.iter(|| black_box(&model).semantic_digest()));
    group.bench_function("checked_leaf_update", |b| {
        b.iter_batched(
            || (),
            |()| {
                let next = black_box(&model)
                    .with_value_updates(black_box(&updates))
                    .unwrap();
                black_box(next.semantic_digest());
                next
            },
            BatchSize::PerIteration,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_interning_performance,
    bench_compiler_phases,
    bench_semantic_digest
);
criterion_main!(benches);
