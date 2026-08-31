//! A/B/null size-sweep for the declaration-dispatch compaction (diagnostic).
//!
//! A = the pre-compaction representation: `as_node` reads a `Vec<Option<Id>>`
//! map (16 B/slot; the three-map total is 48 B/decl of storage).
//! B(=D) = the committed compact representation: `as_node` decodes a `Vec<u64>`
//! slot (8 B/decl) with exactly the shipped `dispatch::encode`/`decode_tag`
//! arithmetic.
//!
//! Reports mapping bytes/decl and `as_node` sequential + random point-lookup at
//! 1.5k / 15k / 100k / 1M declarations, alternating A/B with an A/A null control
//! (the second A run is the noise floor: any A-vs-A delta is measurement noise,
//! not representation). Median of 15 after warm-up. Instant-based, so it is a
//! DIAGNOSTIC signal (CLAUDE.md 10 reserves "benchmark" for criterion); the
//! reportable A/B is the criterion `m2a_map_lookup` run on both checkouts.
//!
//! Build: rustc -O bench_ab.rs -o bench_ab && ./bench_ab

use std::hint::black_box;
use std::time::Instant;

const NODE: u64 = 0;
const MASK: u64 = (1u64 << 62) - 1;
#[inline]
fn enc(kind: u64, idx: usize) -> u64 {
    (kind << 62) | ((idx as u64) + 1)
}
#[inline]
fn dec_node(slot: u64) -> Option<usize> {
    if slot != 0 && (slot >> 62) == NODE {
        Some(((slot & MASK) - 1) as usize)
    } else {
        None
    }
}
fn kind_of(i: usize) -> u64 {
    match i % 5 {
        0 | 1 => 0,
        2 | 3 => 1,
        _ => 2,
    }
}

struct RepA {
    d2n: Vec<Option<usize>>,
}
impl RepA {
    #[inline]
    fn as_node(&self, d: usize) -> Option<usize> {
        self.d2n.get(d).copied().flatten()
    }
}
struct RepD {
    d2r: Vec<u64>,
}
impl RepD {
    #[inline]
    fn as_node(&self, d: usize) -> Option<usize> {
        dec_node(self.d2r[d])
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}
fn timed<F: FnMut() -> usize>(reps: usize, mut f: F) -> f64 {
    for _ in 0..3 {
        black_box(f());
    }
    let mut s = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        let a = f();
        s.push(t.elapsed().as_secs_f64() * 1e9);
        black_box(a);
    }
    median(s)
}

fn build(n: usize) -> (RepA, RepD, Vec<usize>) {
    let mut a = RepA { d2n: vec![None; n] };
    let mut d = RepD { d2r: vec![0u64; n] };
    let mut nc = 0usize;
    for i in 0..n {
        match kind_of(i) {
            0 => {
                a.d2n[i] = Some(nc);
                d.d2r[i] = enc(0, nc);
                nc += 1;
            }
            1 => d.d2r[i] = enc(1, i),
            _ => d.d2r[i] = enc(2, i),
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    let mut s = 0x1234_5678u64;
    for i in (1..n).rev() {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (s >> 11) as usize % (i + 1);
        idx.swap(i, j);
    }
    (a, d, idx)
}

fn main() {
    println!(
        "mapping bytes/decl: A(3x Vec<Option<Id>>)=48  B(Vec<u64>)=8  reduction=6.00x\n"
    );
    println!(
        "{:>9} | {:>26} | {:>26}",
        "N", "SEQ as_node ns (A / B / d%)", "RND as_node ns (A / B / d%)"
    );
    for &n in &[1_500usize, 15_000, 100_000, 1_000_000] {
        let (a, d, idx) = build(n);
        let seq = |f: &dyn Fn(usize) -> Option<usize>| -> usize {
            (0..n).map(|x| f(x).unwrap_or(0)).sum()
        };
        let rnd = |f: &dyn Fn(usize) -> Option<usize>| -> usize {
            idx.iter().map(|&x| f(x).unwrap_or(0)).sum()
        };
        // A/B
        let sa = timed(15, || seq(&|x| a.as_node(x)));
        let sb = timed(15, || seq(&|x| d.as_node(x)));
        let ra = timed(15, || rnd(&|x| a.as_node(x)));
        let rb = timed(15, || rnd(&|x| d.as_node(x)));
        // A/A null (noise floor)
        let sa2 = timed(15, || seq(&|x| a.as_node(x)));
        let ra2 = timed(15, || rnd(&|x| a.as_node(x)));
        println!(
            "{:>9} | {:>7.0}/{:>7.0}/{:+6.1}% | {:>7.0}/{:>7.0}/{:+6.1}%   [null A/A seq {:+.1}% rnd {:+.1}%]",
            n,
            sa, sb, (sb - sa) / sa * 100.0,
            ra, rb, (rb - ra) / ra * 100.0,
            (sa2 - sa) / sa * 100.0,
            (ra2 - ra) / ra * 100.0
        );
    }
}
