//! Isolated prototype for the declaration-dispatch compaction decision (Part B).
//! Compares three representations of decl -> concrete-record mapping WITHOUT touching
//! production core. Measures mapping-storage bytes/decl and as_node point-lookup
//! (sequential + random), at 1M decls with a realistic kind mix (40% node / 40% edge
//! / 20% arc). Not a criterion bench; a release-optimized signal to decide feasibility.
//!
//! Build: rustc -O decl_dispatch_proto.rs -o proto && ./proto

use std::hint::black_box;
use std::time::Instant;

const N: usize = 1_000_000;
const SENTINEL: usize = usize::MAX;

// --- kind, stored per-decl in the arena (already exists as DeclNode.kind) ---
#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
enum Kind { Node, Edge, Arc }

// A stand-in for DeclNode: kind + several DeclId/SymId fields (~56 bytes like the real one).
#[derive(Clone, Copy)]
struct DeclNodeLike { kind: Kind, _f: [usize; 6] }

// ---- Rep A: three parallel Vec<Option<usize>> (current; Option<usize> = 16 bytes) ----
struct RepA { d2n: Vec<Option<usize>>, d2e: Vec<Option<usize>>, d2a: Vec<Option<usize>> }
impl RepA {
    #[inline] fn as_node(&self, d: usize) -> Option<usize> { self.d2n.get(d).copied().flatten() }
}

// ---- Rep B: single tagged enum (16 bytes: discriminant + usize) ----
#[derive(Clone, Copy)]
enum Rec { None, Node(usize), Edge(usize), Arc(usize) }
struct RepB { d2r: Vec<Rec> }
impl RepB {
    #[inline] fn as_node(&self, d: usize) -> Option<usize> {
        match self.d2r.get(d) { Some(Rec::Node(n)) => Some(*n), _ => None }
    }
}

// ---- Rep C: single sentinel usize index, kind read from the decl arena (8 bytes/decl) ----
struct RepC<'a> { d2r: Vec<usize>, decls: &'a [DeclNodeLike] }
impl<'a> RepC<'a> {
    #[inline] fn as_node(&self, d: usize) -> Option<usize> {
        if self.decls.get(d).map(|x| x.kind) == Some(Kind::Node) {
            let r = self.d2r[d];
            if r != SENTINEL { return Some(r); }
        }
        None
    }
}

fn kind_of(i: usize) -> Kind {
    match i % 5 { 0 | 1 => Kind::Node, 2 | 3 => Kind::Edge, _ => Kind::Arc }
}

fn median(mut v: Vec<f64>) -> f64 { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] }

fn time_it<F: FnMut() -> usize>(reps: usize, mut f: F) -> f64 {
    for _ in 0..3 { black_box(f()); } // warm-up
    let mut samples = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        let acc = f();
        samples.push(t.elapsed().as_secs_f64() * 1e9);
        black_box(acc);
    }
    median(samples)
}

fn main() {
    // build the shared decl arena + per-rep mappings with a consistent record layout
    let decls: Vec<DeclNodeLike> = (0..N).map(|i| DeclNodeLike { kind: kind_of(i), _f: [0; 6] }).collect();
    let mut a = RepA { d2n: vec![None; N], d2e: vec![None; N], d2a: vec![None; N] };
    let mut b = RepB { d2r: vec![Rec::None; N] };
    let mut c_d2r = vec![SENTINEL; N];
    // assign each decl a record index into its kind's table (index = running per-kind counter)
    let (mut nc, mut ec, mut ac) = (0usize, 0usize, 0usize);
    for i in 0..N {
        match kind_of(i) {
            Kind::Node => { a.d2n[i] = Some(nc); b.d2r[i] = Rec::Node(nc); c_d2r[i] = nc; nc += 1; }
            Kind::Edge => { a.d2e[i] = Some(ec); b.d2r[i] = Rec::Edge(ec); c_d2r[i] = ec; ec += 1; }
            Kind::Arc  => { a.d2a[i] = Some(ac); b.d2r[i] = Rec::Arc(ac);  c_d2r[i] = ac; ac += 1; }
        }
    }
    let c = RepC { d2r: c_d2r, decls: &decls };

    // memory (mapping storage only), bytes/decl
    let bytes_a = a.d2n.capacity() * 16 + a.d2e.capacity() * 16 + a.d2a.capacity() * 16;
    let bytes_b = b.d2r.capacity() * std::mem::size_of::<Rec>();
    let bytes_c = c.d2r.capacity() * 8; // kind reuses the existing DeclNode.kind (no new storage)
    println!("size_of: Option<usize>={} Rec(enum)={} usize={}",
        std::mem::size_of::<Option<usize>>(), std::mem::size_of::<Rec>(), std::mem::size_of::<usize>());
    println!("mapping bytes/decl:  A={:.1}  B={:.1}  C={:.1}",
        bytes_a as f64 / N as f64, bytes_b as f64 / N as f64, bytes_c as f64 / N as f64);
    println!("reduction vs A:      A=1.0x  B={:.2}x  C={:.2}x",
        bytes_a as f64 / bytes_b as f64, bytes_a as f64 / bytes_c as f64);

    // sequential scan: sum all as_node results (None counted as 0)
    let seq = |f: &dyn Fn(usize) -> Option<usize>| -> usize { (0..N).map(|d| f(d).unwrap_or(0)).sum() };
    let sa = time_it(15, || seq(&|d| a.as_node(d)));
    let sb = time_it(15, || seq(&|d| b.as_node(d)));
    let sc = time_it(15, || seq(&|d| c.as_node(d)));
    println!("\nsequential as_node (ns median, {} decls):  A={:.0}  B={:.0} ({:+.1}%)  C={:.0} ({:+.1}%)",
        N, sa, sb, (sb - sa) / sa * 100.0, sc, (sc - sa) / sa * 100.0);

    // random point-lookup: LCG-shuffled indices
    let mut idx: Vec<usize> = (0..N).collect();
    let mut s = 0x1234_5678u64;
    for i in (1..N).rev() { s = s.wrapping_mul(6364136223846793005).wrapping_add(1); let j = (s >> 11) as usize % (i + 1); idx.swap(i, j); }
    let rnd = |f: &dyn Fn(usize) -> Option<usize>| -> usize { idx.iter().map(|&d| f(d).unwrap_or(0)).sum() };
    let ra = time_it(15, || rnd(&|d| a.as_node(d)));
    let rb = time_it(15, || rnd(&|d| b.as_node(d)));
    let rc = time_it(15, || rnd(&|d| c.as_node(d)));
    println!("random as_node     (ns median):  A={:.0}  B={:.0} ({:+.1}%)  C={:.0} ({:+.1}%)",
        ra, rb, (rb - ra) / ra * 100.0, rc, (rc - ra) / ra * 100.0);
}
