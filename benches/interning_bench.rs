// Criterion harness entry; keep clippy permissive for micro-benchmark noise.
#![allow(warnings)]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hymeko::resolution::intern_pass::intern_ast;
use parser::lexer::simd::{Avx2Lexer, CoreLexer, ScalarLexer};
use parser::parse_description;
use std::hint::black_box;

fn generate_large_hypergraph(nodes: usize) -> String {
    let mut s = String::from("Description{} Benchmark_Graph {\n");
    for i in 0..nodes {
        s.push_str(&format!(
            "  node_{} {{\n    @edge_e{} {{ (+node_{} [0.5]); }}\n  }}\n",
            i, i, i
        ));
    }
    s.push_str("}");
    s
}

fn bench_interning_performance(c: &mut Criterion) {
    // Generate a large synthetic graph string (~1MB+)
    // to minimize measurement noise.
    let input = generate_large_hypergraph(50_000);
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

criterion_group!(benches, bench_interning_performance);
criterion_main!(benches);
