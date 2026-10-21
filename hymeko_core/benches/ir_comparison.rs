//! Explicitly opted-in native comparison; no external runtime dependency in Core.
use criterion::{BatchSize, Criterion, Throughput};
use hymeko::ir::ir::{Ir, IrBuilder};
use std::{ffi::c_void, hint::black_box, os::windows::ffi::OsStrExt, ptr::NonNull, time::Duration};

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0100_0000_01b3;

#[derive(Clone, Copy, Debug)]
struct Shape {
    edges: u32,
    arity: u32,
}

impl Shape {
    fn vertices(self) -> usize {
        self.edges as usize * self.arity as usize
    }
    fn checksum(self) -> u64 {
        let mut sum = OFFSET;
        for edge in 0..self.edges {
            sum = (sum ^ u64::from(edge)).wrapping_mul(PRIME);
            sum = (sum ^ u64::from(self.arity)).wrapping_mul(PRIME);
            for member in 0..self.arity {
                sum = (sum ^ u64::from(edge * self.arity + member)).wrapping_mul(PRIME);
            }
        }
        sum
    }
    fn stats(self) -> [u64; 4] {
        [
            self.vertices() as u64,
            self.edges as u64,
            self.vertices() as u64,
            self.checksum(),
        ]
    }
}

trait Construction {
    type Graph<'a>
    where
        Self: 'a;
    fn name(&self) -> &'static str;
    fn create(&self, shape: Shape) -> Self::Graph<'_>;
    fn verify(&self, graph: &Self::Graph<'_>, shape: Shape) -> [u64; 4];
}

struct Core;
impl Construction for Core {
    type Graph<'a> = Ir;
    fn name(&self) -> &'static str {
        "hymeko"
    }
    fn create(&self, shape: Shape) -> Ir {
        super::SyntheticIrBuilder::new(shape.vertices(), shape.edges as usize, shape.arity as usize)
            .build()
            .expect("canonical fixture construction")
            .ir
    }
    fn verify(&self, graph: &Ir, shape: Shape) -> [u64; 4] {
        // One bound administrative root is outside the normalized vertex domain.
        assert_eq!(graph.nodes.len(), shape.vertices() + 1);
        assert_eq!(graph.edges.len(), shape.edges as usize);
        assert_eq!(graph.arcs.len(), shape.edges as usize);
        let mut checksum = OFFSET;
        let mut count = 0;
        for (ordinal, edge) in graph.edges.iter().enumerate() {
            assert_eq!(edge.arcs.len(), 1);
            let members = &graph.arcs[edge.arcs[0].raw()].refs;
            assert_eq!(members.len(), shape.arity as usize);
            checksum = (checksum ^ ordinal as u64).wrapping_mul(PRIME);
            checksum = (checksum ^ members.len() as u64).wrapping_mul(PRIME);
            for (slot, member) in members.iter().enumerate() {
                let (id, _) = graph.node_for_decl(member.target()).expect("bound vertex");
                let vertex = id
                    .raw()
                    .checked_sub(1)
                    .expect("reference cannot target the administrative root");
                assert_eq!(vertex, ordinal * shape.arity as usize + slot);
                checksum = (checksum ^ vertex as u64).wrapping_mul(PRIME);
                count += 1;
            }
        }
        [
            (graph.nodes.len() - 1) as u64,
            graph.edges.len() as u64,
            count,
            checksum,
        ]
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(path: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}

type Create = unsafe extern "C" fn(u32, u32) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Verify = unsafe extern "C" fn(*mut c_void, u32, u32, *mut u64) -> u32;

struct Library(NonNull<c_void>);
impl Library {
    fn symbol(&self, name: &std::ffi::CStr) -> *mut c_void {
        // SAFETY: self owns a live module; name is NUL-terminated.
        let result = unsafe { GetProcAddress(self.0.as_ptr(), name.as_ptr().cast()) };
        assert!(!result.is_null(), "missing adapter symbol {name:?}");
        result
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: the module was loaded exactly once and graph borrows have ended.
        assert_ne!(unsafe { FreeLibrary(self.0.as_ptr()) }, 0);
    }
}

struct Zig {
    create: Create,
    destroy: Destroy,
    verify: Verify,
    _library: Library,
}
impl Zig {
    fn load() -> Self {
        let path = std::env::var_os("HYM_HGZ_LIBRARY").expect("absolute adapter DLL path required");
        assert!(std::path::Path::new(&path).is_absolute());
        let wide: Vec<u16> = path.encode_wide().chain(Some(0)).collect();
        // SAFETY: wide is a live NUL-terminated UTF-16 path. This opt-in loads only
        // the explicitly selected benchmark adapter, never an implicit dependency.
        let raw = unsafe { LoadLibraryW(wide.as_ptr()) };
        let library = Library(NonNull::new(raw).expect("load external benchmark adapter"));
        // SAFETY: these names and exact C signatures are owned by adapter.zig,
        // archived with the benchmark. Library stays live for all function calls.
        unsafe {
            Self {
                create: std::mem::transmute::<*mut c_void, Create>(library.symbol(c"hgz_create")),
                destroy: std::mem::transmute::<*mut c_void, Destroy>(
                    library.symbol(c"hgz_destroy"),
                ),
                verify: std::mem::transmute::<*mut c_void, Verify>(library.symbol(c"hgz_verify")),
                _library: library,
            }
        }
    }
}
struct ZigGraph<'a> {
    raw: NonNull<c_void>,
    owner: &'a Zig,
}
impl Drop for ZigGraph<'_> {
    fn drop(&mut self) {
        // SAFETY: unique handle from the same live adapter; destroyed exactly once.
        unsafe { (self.owner.destroy)(self.raw.as_ptr()) };
    }
}
impl Construction for Zig {
    type Graph<'a> = ZigGraph<'a>;
    fn name(&self) -> &'static str {
        "hypergraphz"
    }
    fn create(&self, shape: Shape) -> ZigGraph<'_> {
        // SAFETY: scalar inputs; adapter checks limits and reports failure as null.
        let raw = unsafe { (self.create)(shape.edges, shape.arity) };
        ZigGraph {
            raw: NonNull::new(raw).expect("external graph construction failed"),
            owner: self,
        }
    }
    fn verify(&self, graph: &ZigGraph<'_>, shape: Shape) -> [u64; 4] {
        let mut stats = [0; 4];
        // SAFETY: live adapter-owned graph and a writable four-u64 output buffer.
        assert_eq!(
            unsafe {
                (self.verify)(
                    graph.raw.as_ptr(),
                    shape.edges,
                    shape.arity,
                    stats.as_mut_ptr(),
                )
            },
            1
        );
        stats
    }
}

fn check(backend: &impl Construction, shape: Shape) {
    let graph = backend.create(shape);
    let mut expected = shape.stats();
    if std::env::var_os("HYM_COMPARISON_CORRUPT_EXPECTATION").is_some() {
        expected[3] ^= 1;
    }
    assert_eq!(
        backend.verify(&graph, shape),
        expected,
        "{} {shape:?}",
        backend.name()
    );
}

fn measure(c: &mut Criterion, backend: &impl Construction, shape: Shape) {
    let mut group = c.benchmark_group("disjoint_construction_1m");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(shape.vertices() as u64));
    group.bench_function(format!("{}/build", backend.name()), |b| {
        b.iter_batched(
            || (),
            |_| black_box(backend.create(black_box(shape))),
            BatchSize::PerIteration,
        );
    });
    group.bench_function(format!("{}/build_drop", backend.name()), |b| {
        b.iter(|| drop(black_box(backend.create(black_box(shape)))));
    });
    group.finish();
}

pub(super) fn run(c: &mut Criterion) {
    let zig = Zig::load();
    let full = Shape {
        edges: 1000,
        arity: 1000,
    };
    for shape in [
        Shape { edges: 0, arity: 0 },
        Shape { edges: 3, arity: 0 },
        Shape { edges: 1, arity: 1 },
        Shape { edges: 3, arity: 7 },
    ] {
        check(&Core, shape);
        check(&zig, shape);
        eprintln!("PARITY {shape:?}: {:?}", shape.stats());
    }
    // Verify the adapter's failure and shape guards without risking a large allocation.
    // SAFETY: invalid scalar input is explicitly rejected by this adapter.
    assert!(unsafe { (zig.create)(1001, 1) }.is_null());
    let tiny = zig.create(Shape { edges: 1, arity: 1 });
    let mut rejected = [0; 4];
    // SAFETY: valid graph/output; intentionally wrong expected dimensions.
    assert_eq!(
        unsafe { (zig.verify)(tiny.raw.as_ptr(), 2, 1, rejected.as_mut_ptr()) },
        0
    );
    drop(tiny);
    match std::env::var("HYM_COMPARISON_BACKEND").as_deref() {
        Ok("hymeko") => {
            check(&Core, full);
            measure(c, &Core, full);
        }
        Ok("hypergraphz") => {
            check(&zig, full);
            measure(c, &zig, full);
        }
        Err(_) => {
            check(&Core, full);
            check(&zig, full);
            eprintln!("FULL PARITY {full:?}: {:?}", full.stats());
            measure(c, &Core, full);
            measure(c, &zig, full);
        }
        Ok(other) => panic!("unknown comparison backend {other}"),
    }
}
