//! Stage 3 reference benchmark (direct-Ir vs materialization).
//!
//! Models the canonical Ir incidence exactly: edges hold arcs, arcs hold signed
//! refs to node targets (edge -> arc -> ref -> target). Measures Berge-reachability
//! (two nodes adjacent iff they co-occur in a hyperarc) three ways, so the FULL
//! cost of each path is visible (build + search + memory), not just resident
//! kernel time:
//!
//!   SCAN  direct edge-major traversal, NO node index: each BFS step rescans all
//!         arcs for the frontier node -> O(V*E). The naive "no index" baseline.
//!   REV   direct-Ir with a compact CSR reverse-incidence index (node -> arc ids),
//!         built once. Lossless: keeps every original incidence, sign, duplicate.
//!   DENSE materialized star expansion: every pair of refs inside an arc becomes a
//!         pairwise edge in a CSR adjacency (the "view" control). Lossy (pairwise
//!         flattening) and quadratic in arc arity.
//!
//! Build: rustc -O proto_incidence_bench.rs -o pb && ./pb

use std::hint::black_box;
use std::time::Instant;

// ---- xorshift (no Date/rand) ----
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn upto(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Canonical-shaped incidence: arcs each carry a list of node targets. (Sign
/// omitted from the reachability math but the layout/costs are identical.)
struct Hyper {
    n_nodes: usize,
    arcs: Vec<Vec<u32>>, // arc -> node targets (refs)
}

/// `n` nodes, `~incidences` total (node,arc) pairs, `arity` refs per arc.
/// `hub`: if true, targets are drawn with a skew toward low-id hub nodes.
fn gen(n: usize, incidences: usize, arity: usize, hub: bool, seed: u64) -> Hyper {
    let mut r = Rng(seed | 1);
    let n_arcs = incidences / arity;
    let mut arcs = Vec::with_capacity(n_arcs);
    for _ in 0..n_arcs {
        let mut refs = Vec::with_capacity(arity);
        for _ in 0..arity {
            let t = if hub && r.upto(2) == 0 {
                r.upto(n / 50 + 1) // hub bias: 2% of nodes get ~half the mass
            } else {
                r.upto(n)
            };
            refs.push(t as u32);
        }
        arcs.push(refs);
    }
    Hyper { n_nodes: n, arcs }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}
fn timed<F: FnMut() -> u64>(reps: usize, mut f: F) -> f64 {
    for _ in 0..2 {
        black_box(f());
    }
    let mut s = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        let a = f();
        s.push(t.elapsed().as_secs_f64() * 1e6);
        black_box(a);
    }
    median(s)
}

/// CSR reverse-incidence index: node -> incident arc ids. One pass to count, one
/// to fill. This is the candidate core Ir index.
struct Rev {
    row: Vec<u32>, // len n+1
    col: Vec<u32>, // node -> arc ids, grouped
}
fn build_rev(h: &Hyper) -> Rev {
    let n = h.n_nodes;
    let mut row = vec![0u32; n + 1];
    for a in &h.arcs {
        for &t in a {
            row[t as usize + 1] += 1;
        }
    }
    for i in 0..n {
        row[i + 1] += row[i];
    }
    let total = row[n] as usize;
    let mut col = vec![0u32; total];
    let mut cur = row.clone();
    for (ai, a) in h.arcs.iter().enumerate() {
        for &t in a {
            let p = &mut cur[t as usize];
            col[*p as usize] = ai as u32;
            *p += 1;
        }
    }
    Rev { row, col }
}

/// BFS reachability from `start` via the reverse index: node -> incident arcs ->
/// co-incident nodes. Caller-owned scratch (visited + queue), no per-step alloc.
fn bfs_rev(h: &Hyper, rev: &Rev, start: u32, visited: &mut [u32], gen: u32, q: &mut Vec<u32>) -> u64 {
    q.clear();
    q.push(start);
    visited[start as usize] = gen;
    let mut count = 0u64;
    let mut head = 0;
    while head < q.len() {
        let node = q[head];
        head += 1;
        count += 1;
        let (lo, hi) = (rev.row[node as usize] as usize, rev.row[node as usize + 1] as usize);
        for &ai in &rev.col[lo..hi] {
            for &t in &h.arcs[ai as usize] {
                if visited[t as usize] != gen {
                    visited[t as usize] = gen;
                    q.push(t);
                }
            }
        }
    }
    count
}

/// BFS reachability with NO node index: each step rescans every arc. O(V*E).
fn bfs_scan(h: &Hyper, start: u32, visited: &mut [u32], gen: u32, q: &mut Vec<u32>) -> u64 {
    q.clear();
    q.push(start);
    visited[start as usize] = gen;
    let mut count = 0u64;
    let mut head = 0;
    while head < q.len() {
        let node = q[head];
        head += 1;
        count += 1;
        for a in &h.arcs {
            if a.iter().any(|&t| t == node) {
                for &t in a {
                    if visited[t as usize] != gen {
                        visited[t as usize] = gen;
                        q.push(t);
                    }
                }
            }
        }
    }
    count
}

/// Dense star expansion: CSR pairwise adjacency (every pair of refs in an arc).
struct Csr {
    row: Vec<u32>,
    col: Vec<u32>,
}
fn build_dense(h: &Hyper) -> Csr {
    let n = h.n_nodes;
    // count degrees (with multiplicity, as a star expansion does)
    let mut deg = vec![0u32; n];
    for a in &h.arcs {
        let k = a.len();
        for &u in a {
            deg[u as usize] += (k as u32).saturating_sub(1);
        }
    }
    let mut row = vec![0u32; n + 1];
    for i in 0..n {
        row[i + 1] = row[i] + deg[i];
    }
    let total = row[n] as usize;
    let mut col = vec![0u32; total];
    let mut cur = row.clone();
    for a in &h.arcs {
        for (i, &u) in a.iter().enumerate() {
            for (j, &v) in a.iter().enumerate() {
                if i != j {
                    let p = &mut cur[u as usize];
                    col[*p as usize] = v;
                    *p += 1;
                }
            }
        }
    }
    Csr { row, col }
}
fn bfs_csr(csr: &Csr, start: u32, visited: &mut [u32], gen: u32, q: &mut Vec<u32>) -> u64 {
    q.clear();
    q.push(start);
    visited[start as usize] = gen;
    let mut count = 0u64;
    let mut head = 0;
    while head < q.len() {
        let node = q[head];
        head += 1;
        count += 1;
        let (lo, hi) = (csr.row[node as usize] as usize, csr.row[node as usize + 1] as usize);
        for &t in &csr.col[lo..hi] {
            if visited[t as usize] != gen {
                visited[t as usize] = gen;
                q.push(t);
            }
        }
    }
    count
}

fn run(label: &str, n: usize, incidences: usize, arity: usize, hub: bool) {
    let h = gen(n, incidences, arity, hub, 0x9E3779B97F4A7C15);
    let n_arcs = h.arcs.len();
    let mut visited = vec![0u32; n];
    let mut q: Vec<u32> = Vec::with_capacity(n);
    let mut g = 1u32;

    // build costs
    let t_rev = timed(7, || {
        let r = build_rev(&h);
        (r.col.len() as u64) ^ (r.row.len() as u64)
    });
    let t_dense = timed(5, || {
        let c = build_dense(&h);
        c.col.len() as u64
    });
    let rev = build_rev(&h);
    let dense = build_dense(&h);

    // memory bytes/incidence
    let rev_bytes = rev.row.len() * 4 + rev.col.len() * 4;
    let dense_bytes = dense.row.len() * 4 + dense.col.len() * 4;

    // search costs (first-search resident; scratch reused)
    let s_rev = timed(9, || { g += 1; bfs_rev(&h, &rev, 0, &mut visited, g, &mut q) });
    let s_dense = timed(9, || { g += 1; bfs_csr(&dense, 0, &mut visited, g, &mut q) });
    // scan only at small size (O(V*E) is unusable at scale)
    let s_scan = if n_arcs <= 20_000 {
        timed(3, || { g += 1; bfs_scan(&h, 0, &mut visited, g, &mut q) })
    } else {
        f64::NAN
    };

    println!("\n== {label}: n={n} incidences={incidences} arity={arity} arcs={n_arcs} hub={hub} ==");
    println!("  build ns:   REV={:>10.0}  DENSE={:>12.0} ({:.1}x)", t_rev, t_dense, t_dense / t_rev.max(1.0));
    println!("  bytes/inc:  REV={:>6.1}  DENSE={:>8.1} ({:.1}x)", rev_bytes as f64 / incidences as f64, dense_bytes as f64 / incidences as f64, dense_bytes as f64 / rev_bytes as f64);
    println!("  search ns:  REV={:>10.0}  DENSE={:>10.0} ({:+.1}% vs REV)  SCAN={:>12.0}", s_rev, s_dense, (s_dense - s_rev) / s_rev * 100.0, s_scan);
    println!("  end-to-end (build+search) ns:  REV={:>10.0}  DENSE={:>12.0} ({:.1}x)", t_rev + s_rev, t_dense + s_dense, (t_dense + s_dense) / (t_rev + s_rev));
}

fn main() {
    println!("Direct-Ir reverse-incidence index (REV) vs materialized star-expansion CSR (DENSE)");
    run("uniform-small", 20_000, 100_000, 5, false);
    run("uniform-1e5", 200_000, 100_000, 5, false);
    run("uniform-1e6", 2_000_000, 1_000_000, 5, false);
    run("hub-skew-1e6", 2_000_000, 1_000_000, 5, true);
    run("high-arity-1e6", 500_000, 1_000_000, 20, false);
}
