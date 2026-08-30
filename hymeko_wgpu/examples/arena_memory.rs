//! Phase 4 §8: memory of an 8-layer resident chain, four ways.
//!
//! Criterion measures time, not allocation, so the memory comparison lives here
//! rather than in a benchmark. Every figure printed is **counted**, not
//! estimated: `ArenaStats` records exactly what the arena created, cached,
//! retired and evicted.
//!
//! | variant | what it does |
//! |---|---|
//! | A `naive` | one fresh intermediate per layer — 8 live buffers |
//! | B `pingpong` | two manually managed buffers, alternating |
//! | C `arena` | `TensorArena`, leases acquired and released per layer |
//! | D `arena_repeat` | C, run several times on one arena |
//!
//! B is the target: it is the minimum a careful human achieves by hand. C is
//! worth having only if it approaches B while removing the manual lifetime
//! reasoning. That is the comparison this program exists to make.
//!
//! Run with:
//! ```text
//! cargo run -p hymeko_wgpu --release --example arena_memory
//! ```

use hymeko_wgpu::testing::SparseSpec;
use hymeko_wgpu::testing::{Fixture, encode_chain};
use hymeko_wgpu::{ActivationKind, TensorArenaConfig, WgpuBackend};

const LAYERS: usize = 8;
const REPEATS: usize = 4;

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

struct Case {
    label: &'static str,
    spec: SparseSpec,
    fd: usize,
}

fn main() {
    let backend = match WgpuBackend::builder().label("arena-memory").build() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no GPU adapter, skipping: {e}");
            return;
        }
    };
    println!("{}", backend.capabilities().summary());
    println!();

    let cases = [
        Case {
            label: "small",
            spec: SparseSpec::small(),
            fd: 8,
        },
        Case {
            label: "small",
            spec: SparseSpec::small(),
            fd: 32,
        },
        Case {
            label: "medium",
            spec: SparseSpec::medium(),
            fd: 32,
        },
        Case {
            label: "medium",
            spec: SparseSpec::medium(),
            fd: 128,
        },
        Case {
            label: "large",
            spec: SparseSpec::large(),
            fd: 32,
        },
        Case {
            label: "large",
            spec: SparseSpec::large(),
            fd: 128,
        },
    ];

    println!(
        "{:<8} {:>4} {:>10} {:>9} {:>10} {:>10} {:>10} {:>10} {:>7} {:>7} {:>6}",
        "case",
        "fd",
        "variant",
        "buffers",
        "device MiB",
        "peak MiB",
        "cached MiB",
        "logical MiB",
        "hits",
        "misses",
        "evict"
    );
    println!("{}", "-".repeat(112));

    for case in &cases {
        let fd = case.fd;
        let csr = case.spec.build();
        let x = case.spec.dense_block(fd);
        // The spec-to-CSR step stays here: `SparseSpec` lives in `hymeko_hnn`,
        // which the library must not depend on.
        let fx = Fixture::build(&backend, &csr, &x, fd);
        let (matrix, input, out_shape) = (&fx.matrix, &fx.input, fx.out_shape);
        let res = backend.resources();

        // One intermediate, in bytes. `max(1)` mirrors the allocator's padding.
        let one = (out_shape.elements().max(1) * std::mem::size_of::<f32>()) as u64;
        let logical = one * LAYERS as u64;

        // --- A: naive, one buffer per layer --------------------------------
        {
            let buffers: Vec<_> = (0..LAYERS)
                .map(|i| res.zeroed_tensor(&format!("h{i}"), out_shape).unwrap())
                .collect();
            let mut enc = backend.encoder(Some("naive"));
            let ops: Vec<_> = (0..LAYERS)
                .map(|i| {
                    let src = if i == 0 { input } else { &buffers[i - 1] };
                    let spmm = backend
                        .operations()
                        .spmm()
                        .matrix(matrix)
                        .input(src)
                        .output(&buffers[i])
                        .prepare()
                        .unwrap();
                    let act = backend
                        .operations()
                        .activation()
                        .kind(ActivationKind::Relu)
                        .input(&buffers[i])
                        .in_place(&buffers[i])
                        .prepare()
                        .unwrap();
                    (spmm, act)
                })
                .collect();
            for (s, a) in &ops {
                s.encode(&mut enc).unwrap();
                a.encode(&mut enc).unwrap();
            }
            backend.wait(backend.submit(enc)).unwrap();
            let device = one * LAYERS as u64;
            row(
                case.label,
                fd,
                "A naive",
                LAYERS as u64,
                device,
                device,
                0,
                logical,
                0,
                LAYERS as u64,
                0,
            );
        }

        // --- B: manual ping-pong -------------------------------------------
        {
            let a = res.zeroed_tensor("a", out_shape).unwrap();
            let b = res.zeroed_tensor("b", out_shape).unwrap();
            let mut enc = backend.encoder(Some("pingpong"));
            let ops: Vec<_> = (0..LAYERS)
                .map(|i| {
                    let (src, dst) = match i {
                        0 => (input, &a),
                        _ if i % 2 == 1 => (&a, &b),
                        _ => (&b, &a),
                    };
                    let spmm = backend
                        .operations()
                        .spmm()
                        .matrix(matrix)
                        .input(src)
                        .output(dst)
                        .prepare()
                        .unwrap();
                    let act = backend
                        .operations()
                        .activation()
                        .kind(ActivationKind::Relu)
                        .input(dst)
                        .in_place(dst)
                        .prepare()
                        .unwrap();
                    (spmm, act)
                })
                .collect();
            for (s, a) in &ops {
                s.encode(&mut enc).unwrap();
                a.encode(&mut enc).unwrap();
            }
            backend.wait(backend.submit(enc)).unwrap();
            let device = one * 2;
            row(
                case.label,
                fd,
                "B pingpong",
                2,
                device,
                device,
                0,
                logical,
                0,
                2,
                0,
            );
        }

        // --- C: arena --------------------------------------------------------
        {
            let arena = backend.tensor_arena(TensorArenaConfig::default());
            run_arena_chain(&backend, &arena, &fx, 1);
            let s = arena.stats();
            row(
                case.label,
                fd,
                "C arena",
                s.buffers_created,
                s.device_bytes(),
                s.peak_arena_bytes,
                s.cached_bytes,
                logical,
                s.reuse_hits,
                s.reuse_misses,
                s.evictions,
            );
        }

        // --- D: arena, repeated inferences ------------------------------------
        {
            let arena = backend.tensor_arena(TensorArenaConfig::default());
            run_arena_chain(&backend, &arena, &fx, REPEATS);
            let s = arena.stats();
            row(
                case.label,
                fd,
                "D arena xN",
                s.buffers_created,
                s.device_bytes(),
                s.peak_arena_bytes,
                s.cached_bytes,
                logical,
                s.reuse_hits,
                s.reuse_misses,
                s.evictions,
            );
        }
        println!();
    }

    println!("D runs the same chain {REPEATS}x on one arena.");
    println!("'logical MiB' is what {LAYERS} independent intermediates would occupy.");
}

/// The arena variant: leases are acquired and released per layer, so the pool —
/// not the caller — decides that two buffers suffice.
fn run_arena_chain(
    backend: &WgpuBackend,
    arena: &hymeko_wgpu::TensorArena<'_>,
    fx: &Fixture,
    repeats: usize,
) {
    for _ in 0..repeats {
        let a = arena.acquire(fx.out_shape).unwrap();
        let b = arena.acquire(fx.out_shape).unwrap();
        let mut session = arena.session(Some("arena-chain"));
        encode_chain(
            backend,
            fx,
            &a,
            &b,
            LAYERS,
            ActivationKind::Relu,
            &mut session,
        );
        session.submit_and_wait().unwrap();
        arena.reclaim_all().unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
fn row(
    case: &str,
    fd: usize,
    variant: &str,
    buffers: u64,
    device: u64,
    peak: u64,
    cached: u64,
    logical: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
) {
    println!(
        "{case:<8} {fd:>4} {variant:>10} {buffers:>9} {:>10.1} {:>10.1} {:>10.1} {:>10.1} {hits:>7} {misses:>7} {evictions:>6}",
        mib(device),
        mib(peak),
        mib(cached),
        mib(logical),
    );
}
