//! Criterion cells for the public canonical-CBOR facade on real corpus workloads.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hymeko::ir_stream::DecodeLimits;
use hymeko::module_store::module_store::{
    CompiledProgram, HymekoParser, ModuleLoadError, ModuleStore,
};
use hymeko::module_store::source_provider::StdFsProvider;
use hymeko::writers::cbor_writer::{decode, encode_compiled};
use parser::ast::AstStr;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

struct LalrpopParser;

impl HymekoParser for LalrpopParser {
    fn parse<'a>(&self, source: &'a str) -> Result<AstStr<'a>, String> {
        parser::parse_description(source).map_err(|error| format!("{error:?}"))
    }
}

type Loaded = (
    ModuleStore<StdFsProvider, LalrpopParser>,
    Arc<CompiledProgram>,
);

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest directory has a parent")
        .join("data")
}

fn load(relative: &str) -> Result<Loaded, ModuleLoadError> {
    let root = corpus_root().join(relative);
    let mut store = ModuleStore::new(StdFsProvider::new(), LalrpopParser);
    let compiled = store.compile(&root)?;
    Ok((store, compiled))
}

fn bench_canonical_cbor(c: &mut Criterion) {
    const WORKLOADS: [(&str, &str); 3] = [
        ("small_d1", "minimal_examples/minimal_example.hymeko"),
        ("medium_d45", "robotics/galambos/galambos_env.hymeko"),
        ("large_d409", "robotics/robots/quadruped.hymeko"),
    ];
    let root = corpus_root();

    let mut encode_group = c.benchmark_group("canonical_cbor_encode");
    encode_group
        .sample_size(30)
        .measurement_time(Duration::from_secs(5));
    for (label, relative) in WORKLOADS {
        let (store, compiled) =
            load(relative).unwrap_or_else(|error| panic!("{relative} did not lower: {error:?}"));
        let encoded = encode_compiled(&root, &compiled, &store.it)
            .unwrap_or_else(|error| panic!("{relative} did not encode: {error}"));
        eprintln!(
            "[canonical_cbor {label}] decls={} arcs={} bytes={}",
            compiled.ir.decl_nodes.len(),
            compiled.ir.arcs.len(),
            encoded.len()
        );
        encode_group.throughput(Throughput::Bytes(encoded.len() as u64));
        encode_group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                black_box(
                    encode_compiled(black_box(&root), black_box(&compiled), black_box(&store.it))
                        .expect("validated fixture encodes"),
                )
            });
        });
    }
    encode_group.finish();

    let mut decode_group = c.benchmark_group("canonical_cbor_decode");
    decode_group
        .sample_size(30)
        .measurement_time(Duration::from_secs(5));
    for (label, relative) in WORKLOADS {
        let (store, compiled) =
            load(relative).unwrap_or_else(|error| panic!("{relative} did not lower: {error:?}"));
        let encoded = encode_compiled(&root, &compiled, &store.it)
            .unwrap_or_else(|error| panic!("{relative} did not encode: {error}"));
        decode_group.throughput(Throughput::Bytes(encoded.len() as u64));
        decode_group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                black_box(
                    decode(black_box(&encoded), black_box(&DecodeLimits::DEFAULT))
                        .expect("validated bytes decode"),
                )
            });
        });
    }
    decode_group.finish();
}

criterion_group!(benches, bench_canonical_cbor);
criterion_main!(benches);
