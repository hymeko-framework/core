//! Payload-density inputs for the existing canonical construction benchmark.
use super::{
    AnnoR, DeclId, IncidenceSign, Ir, IrBuilder, ReferenceDraft, SymId, SyntheticIrBuilder,
};
use criterion::{BatchSize, Criterion, Throughput};
use hymeko::ir::ir::ValueR;
use std::{hint::black_box, time::Duration};

#[derive(Clone, Copy)]
pub(super) enum MetadataDensity {
    Empty,
    Sparse,
    Dense,
}

impl MetadataDensity {
    pub(super) fn from_name(name: &str) -> Self {
        match name {
            "empty" => Self::Empty,
            "sparse" => Self::Sparse,
            "dense" => Self::Dense,
            _ => panic!("metadata density must be empty, sparse or dense"),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Sparse => "sparse",
            Self::Dense => "dense",
        }
    }
    fn stride(self) -> Option<usize> {
        match self {
            Self::Empty => None,
            Self::Sparse => Some(100),
            Self::Dense => Some(1),
        }
    }
    pub(super) fn payload(self, ordinal: usize, root: DeclId) -> (AnnoR, Vec<ReferenceDraft>) {
        if self
            .stride()
            .is_some_and(|step| ordinal.is_multiple_of(step))
        {
            (
                AnnoR {
                    tags: vec![SymId::new(0)],
                    value: Some(ValueR::Num((ordinal + 1) as f64)),
                },
                vec![ReferenceDraft::plain(root, IncidenceSign::Plus)],
            )
        } else {
            (AnnoR::default(), Vec::new())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    annotations: usize,
    bases: usize,
    checksum: u64,
}
impl Observation {
    fn read(ir: &Ir) -> Self {
        let mut result = Self {
            annotations: 0,
            bases: 0,
            checksum: 0,
        };
        for node in &ir.nodes {
            let decl = &ir.decl_nodes[node.decl.raw()];
            let anno = ir.annotation(decl.anno).expect("fixture annotation");
            result.annotations += usize::from(!anno.tags.is_empty() || anno.value.is_some());
            result.checksum += anno.tags.len() as u64;
            for tag in &anno.tags {
                result.checksum += tag.raw() as u64;
            }
            if let Some(value) = &anno.value {
                let ValueR::Num(number) = value else {
                    panic!("fixture value must be numeric")
                };
                result.checksum += *number as u64;
            }
            let bases = ir.bases(node.bases).expect("fixture bases");
            result.bases += bases.len();
            for base in bases {
                result.checksum +=
                    (base.target().raw() as u64 + 1) * (base.sign() as i64 + 2) as u64;
            }
        }
        result
    }
}

pub(super) struct MetadataCase {
    nodes: usize,
    edges: usize,
    arity: usize,
    density: MetadataDensity,
}
impl MetadataCase {
    pub(super) fn benchmark(density: MetadataDensity) -> Self {
        Self {
            nodes: 1_000_000,
            edges: 1000,
            arity: 1000,
            density,
        }
    }
    fn build(&self) -> Ir {
        SyntheticIrBuilder::new(self.nodes, self.edges, self.arity)
            .with_metadata(self.density)
            .build()
            .expect("canonical metadata fixture")
            .ir
    }
    fn expected(&self) -> Observation {
        let (count, sum) = self.density.stride().map_or((0, 0), |step| {
            let count = self.nodes.div_ceil(step);
            let sum = (step as u64 * count as u64 * count.saturating_sub(1) as u64) / 2;
            (count, sum + 5 * count as u64)
        });
        Observation {
            annotations: count,
            bases: count,
            checksum: sum,
        }
    }
    pub(super) fn verify_contracts() {
        for nodes in [0, 1, 17, 101] {
            for density in [
                MetadataDensity::Empty,
                MetadataDensity::Sparse,
                MetadataDensity::Dense,
            ] {
                let case = Self {
                    nodes,
                    edges: usize::from(nodes != 0) * 3,
                    arity: 5,
                    density,
                };
                let built = case.build();
                assert_eq!(built.nodes.len(), nodes + 1);
                assert_eq!(built.decl_count(), 1 + nodes + 2 * case.edges);
                assert_eq!(Observation::read(&built), case.expected());
                let encoded = serde_json::to_vec(&built).expect("logical wire");
                let decoded: Ir = serde_json::from_slice(&encoded).expect("logical decode");
                assert_eq!(Observation::read(&decoded), case.expected());
            }
        }
    }
    pub(super) fn run(&self, c: &mut Criterion, corrupt: bool) {
        let probe = self.build();
        let mut expected = self.expected();
        if corrupt {
            expected.checksum ^= 1;
        }
        assert_eq!(
            Observation::read(&probe),
            expected,
            "independent metadata oracle"
        );
        eprintln!(
            "METADATA_FIXTURE density={} nodes={} edges={} arity={} annotations={} bases={} checksum={}",
            self.density.name(),
            self.nodes,
            self.edges,
            self.arity,
            expected.annotations,
            expected.bases,
            expected.checksum
        );
        drop(probe);
        let mut group = c.benchmark_group(format!("metadata_{}", self.density.name()));
        group
            .sample_size(20)
            .warm_up_time(Duration::from_secs(1))
            .measurement_time(Duration::from_secs(3));
        group.throughput(Throughput::Elements(self.nodes as u64));
        group.bench_function("build", |b| {
            b.iter_batched(|| (), |_| black_box(self.build()), BatchSize::PerIteration)
        });
        group.bench_function("build_drop", |b| b.iter(|| drop(black_box(self.build()))));
        let graph = self.build();
        assert_eq!(Observation::read(&graph), self.expected());
        group.bench_function("read", |b| {
            b.iter(|| black_box(Observation::read(black_box(&graph))))
        });
        group.finish();
    }
}
