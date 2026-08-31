//! Generational-arena representation prototype (Stage 1.3, decision crux).
//!
//! Measures four representations of a canonical entity table against the
//! CLAUDE.md gates. All isolated from production `hymeko_core`.
//!
//!   BASE  today's form: dense `Vec<Rec>`, id = plain index, no generation.
//!         The regression baseline; the current append-only Ir.
//!   A     tombstone slot arena: `Vec<Slot{gen:u32, val:Option<Rec>}>` + free list.
//!   B     SoA: parallel `Vec<u32> gens` (parity = liveness) + `Vec<Rec> vals` + free list.
//!   C     dense storage + stable slot indirection (order-preserving removal;
//!         swap-remove is disallowed by the brief, so removal shifts the dense
//!         array — O(n) — which this harness measures rather than hides).
//!
//! Metrics: append-only construction, random valid lookup, sequential iteration,
//! insert/remove/reuse, tombstone density 0/1/10/30/50 %, clone, bytes/live,
//! bytes/allocated-slot. Median of N after warm-up. Instant-based, so DIAGNOSTIC
//! (CLAUDE.md 10 reserves "benchmark" for criterion); the reportable A/B runs on
//! real code.
//!
//! Build: rustc -O proto_arena.rs -o proto_arena && ./proto_arena

use std::hint::black_box;
use std::mem::size_of;
use std::time::Instant;

/// Representative entity: a decl back-pointer + a small fixed payload (~ a POD
/// NodeRec without its `Vec`; the `Vec` cost is identical across reps and is
/// discussed separately).
#[derive(Clone, Copy)]
struct Rec {
    decl: u64,
    payload: [u64; 3],
}
impl Rec {
    fn new(i: usize) -> Self {
        Rec { decl: i as u64, payload: [i as u64, (i as u64) ^ 0x5a5a, (i as u64).wrapping_mul(2654435761)] }
    }
    #[inline]
    fn sum(&self) -> u64 {
        self.decl ^ self.payload[0] ^ self.payload[1] ^ self.payload[2]
    }
}

/// (slot, generation) handle. In production this packs into a NonZeroU64 (8 B,
/// Option niche); here the pair suffices to measure the lookup path.
#[derive(Clone, Copy)]
struct Handle {
    slot: u32,
    gen: u32,
}

// ---------------------------------------------------------------- BASE
struct Base {
    v: Vec<Rec>,
}
impl Base {
    fn with_capacity(n: usize) -> Self {
        Base { v: Vec::with_capacity(n) }
    }
    #[inline]
    fn insert(&mut self, r: Rec) -> u32 {
        let i = self.v.len() as u32;
        self.v.push(r);
        i
    }
    #[inline]
    fn get(&self, i: u32) -> Option<&Rec> {
        self.v.get(i as usize)
    }
    fn bytes(&self) -> usize {
        self.v.capacity() * size_of::<Rec>()
    }
}

// ---------------------------------------------------------------- A: tombstone
struct SlotA {
    gen: u32,
    val: Option<Rec>,
}
struct ArenaA {
    slots: Vec<SlotA>,
    free: Vec<u32>,
    live: usize,
}
impl ArenaA {
    fn with_capacity(n: usize) -> Self {
        ArenaA { slots: Vec::with_capacity(n), free: Vec::new(), live: 0 }
    }
    #[inline]
    fn insert(&mut self, r: Rec) -> Handle {
        self.live += 1;
        if let Some(slot) = self.free.pop() {
            let s = &mut self.slots[slot as usize];
            s.val = Some(r);
            Handle { slot, gen: s.gen }
        } else {
            let slot = self.slots.len() as u32;
            self.slots.push(SlotA { gen: 1, val: Some(r) });
            Handle { slot, gen: 1 }
        }
    }
    #[inline]
    fn get(&self, h: Handle) -> Option<&Rec> {
        let s = self.slots.get(h.slot as usize)?;
        if s.gen == h.gen { s.val.as_ref() } else { None }
    }
    #[inline]
    fn remove(&mut self, h: Handle) {
        if let Some(s) = self.slots.get_mut(h.slot as usize) {
            if s.gen == h.gen && s.val.is_some() {
                s.val = None;
                s.gen = s.gen.wrapping_add(1);
                if s.gen == u32::MAX {
                    // retire: never reuse (wrap must not revive a stale id)
                } else {
                    self.free.push(h.slot);
                }
                self.live -= 1;
            }
        }
    }
    fn iter_sum(&self) -> u64 {
        let mut acc = 0u64;
        for s in &self.slots {
            if let Some(v) = &s.val {
                acc ^= v.sum();
            }
        }
        acc
    }
    fn bytes(&self) -> usize {
        self.slots.capacity() * size_of::<SlotA>() + self.free.capacity() * 4
    }
}

// ---------------------------------------------------------------- B: SoA parity
struct ArenaB {
    gens: Vec<u32>, // odd = live, even = dead/free
    vals: Vec<Rec>,
    free: Vec<u32>,
    live: usize,
}
impl ArenaB {
    fn with_capacity(n: usize) -> Self {
        ArenaB { gens: Vec::with_capacity(n), vals: Vec::with_capacity(n), free: Vec::new(), live: 0 }
    }
    #[inline]
    fn insert(&mut self, r: Rec) -> Handle {
        self.live += 1;
        if let Some(slot) = self.free.pop() {
            self.vals[slot as usize] = r;
            let g = &mut self.gens[slot as usize];
            *g |= 1; // make odd (live); gen was even after remove
            Handle { slot, gen: *g }
        } else {
            let slot = self.gens.len() as u32;
            self.gens.push(1);
            self.vals.push(r);
            Handle { slot, gen: 1 }
        }
    }
    #[inline]
    fn get(&self, h: Handle) -> Option<&Rec> {
        let g = *self.gens.get(h.slot as usize)?;
        if g == h.gen { Some(&self.vals[h.slot as usize]) } else { None }
    }
    #[inline]
    fn remove(&mut self, h: Handle) {
        if let Some(g) = self.gens.get_mut(h.slot as usize) {
            if *g == h.gen {
                let ng = g.wrapping_add(1); // now even (dead)
                if ng == u32::MAX - 1 {
                    *g = ng; // retire (do not push to free)
                } else {
                    *g = ng;
                    self.free.push(h.slot);
                }
                self.live -= 1;
            }
        }
    }
    fn iter_sum(&self) -> u64 {
        let mut acc = 0u64;
        for (i, &g) in self.gens.iter().enumerate() {
            if g & 1 == 1 {
                acc ^= self.vals[i].sum();
            }
        }
        acc
    }
    fn bytes(&self) -> usize {
        self.gens.capacity() * 4 + self.vals.capacity() * size_of::<Rec>() + self.free.capacity() * 4
    }
}

// ---------------------------------------------------------------- C: dense + indirection
const NIL: u32 = u32::MAX;
struct ArenaC {
    dense: Vec<Rec>,
    dense_slot: Vec<u32>, // dense idx -> slot
    slot_dense: Vec<u32>, // slot -> dense idx or NIL
    gens: Vec<u32>,
    free: Vec<u32>,
}
impl ArenaC {
    fn with_capacity(n: usize) -> Self {
        ArenaC {
            dense: Vec::with_capacity(n),
            dense_slot: Vec::with_capacity(n),
            slot_dense: Vec::with_capacity(n),
            gens: Vec::with_capacity(n),
            free: Vec::new(),
        }
    }
    #[inline]
    fn insert(&mut self, r: Rec) -> Handle {
        let d = self.dense.len() as u32;
        self.dense.push(r);
        let slot = if let Some(slot) = self.free.pop() {
            self.slot_dense[slot as usize] = d;
            self.gens[slot as usize] |= 1;
            slot
        } else {
            let slot = self.slot_dense.len() as u32;
            self.slot_dense.push(d);
            self.gens.push(1);
            slot
        };
        self.dense_slot.push(slot);
        Handle { slot, gen: self.gens[slot as usize] }
    }
    #[inline]
    fn get(&self, h: Handle) -> Option<&Rec> {
        let g = *self.gens.get(h.slot as usize)?;
        if g != h.gen {
            return None;
        }
        let d = self.slot_dense[h.slot as usize];
        if d == NIL { None } else { Some(&self.dense[d as usize]) }
    }
    /// Order-preserving removal (swap-remove disallowed): shift the dense tail.
    #[inline]
    fn remove(&mut self, h: Handle) {
        let g = match self.gens.get(h.slot as usize) {
            Some(&g) if g == h.gen => g,
            _ => return,
        };
        let d = self.slot_dense[h.slot as usize];
        if d == NIL {
            return;
        }
        let di = d as usize;
        self.dense.remove(di); // O(n) shift — the price of order preservation
        self.dense_slot.remove(di);
        for s in self.dense_slot[di..].iter() {
            self.slot_dense[*s as usize] -= 1;
        }
        self.slot_dense[h.slot as usize] = NIL;
        let ng = g.wrapping_add(1);
        self.gens[h.slot as usize] = ng;
        if ng != u32::MAX - 1 {
            self.free.push(h.slot);
        }
    }
    fn iter_sum(&self) -> u64 {
        let mut acc = 0u64;
        for r in &self.dense {
            acc ^= r.sum();
        }
        acc
    }
    fn bytes(&self) -> usize {
        self.dense.capacity() * size_of::<Rec>()
            + self.dense_slot.capacity() * 4
            + self.slot_dense.capacity() * 4
            + self.gens.capacity() * 4
            + self.free.capacity() * 4
    }
}

// ---------------------------------------------------------------- timing
fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}
fn timed<F: FnMut() -> u64>(reps: usize, mut f: F) -> f64 {
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
fn shuffle(n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..n).collect();
    let mut s = 0x2545_f491_4f6c_dd1du64;
    for i in (1..n).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let j = (s as usize) % (i + 1);
        idx.swap(i, j);
    }
    idx
}

fn main() {
    println!("size_of: Rec={}  SlotA={}  Handle={}  Option<Rec>={}",
        size_of::<Rec>(), size_of::<SlotA>(), size_of::<Handle>(), size_of::<Option<Rec>>());
    for &n in &[100_000usize, 1_000_000] {
        println!("\n================ N = {} live entities ================", n);
        // ---- append-only construction ----
        let cons_base = timed(9, || { let mut a = Base::with_capacity(n); for i in 0..n { a.insert(Rec::new(i)); } a.v.len() as u64 });
        let cons_a = timed(9, || { let mut a = ArenaA::with_capacity(n); for i in 0..n { a.insert(Rec::new(i)); } a.live as u64 });
        let cons_b = timed(9, || { let mut a = ArenaB::with_capacity(n); for i in 0..n { a.insert(Rec::new(i)); } a.live as u64 });
        let cons_c = timed(9, || { let mut a = ArenaC::with_capacity(n); for i in 0..n { a.insert(Rec::new(i)); } a.dense.len() as u64 });
        println!("append-only build ns:  BASE={:>11.0}  A={:>11.0} ({:+.1}%)  B={:>11.0} ({:+.1}%)  C={:>11.0} ({:+.1}%)",
            cons_base, cons_a, (cons_a-cons_base)/cons_base*100.0, cons_b,(cons_b-cons_base)/cons_base*100.0, cons_c,(cons_c-cons_base)/cons_base*100.0);

        // build populated arenas for lookup/iter
        let mut base = Base::with_capacity(n);
        let mut aa = ArenaA::with_capacity(n);
        let mut ab = ArenaB::with_capacity(n);
        let mut ac = ArenaC::with_capacity(n);
        let mut hb: Vec<u32> = Vec::with_capacity(n);
        let mut ha: Vec<Handle> = Vec::with_capacity(n);
        let mut hbb: Vec<Handle> = Vec::with_capacity(n);
        let mut hc: Vec<Handle> = Vec::with_capacity(n);
        for i in 0..n {
            hb.push(base.insert(Rec::new(i)));
            ha.push(aa.insert(Rec::new(i)));
            hbb.push(ab.insert(Rec::new(i)));
            hc.push(ac.insert(Rec::new(i)));
        }
        let ord = shuffle(n);

        // ---- random valid lookup ----
        let lk_base = timed(9, || { let mut acc=0u64; for &i in &ord { if let Some(r)=base.get(hb[i]){acc^=r.sum();} } acc });
        let lk_a = timed(9, || { let mut acc=0u64; for &i in &ord { if let Some(r)=aa.get(ha[i]){acc^=r.sum();} } acc });
        let lk_b = timed(9, || { let mut acc=0u64; for &i in &ord { if let Some(r)=ab.get(hbb[i]){acc^=r.sum();} } acc });
        let lk_c = timed(9, || { let mut acc=0u64; for &i in &ord { if let Some(r)=ac.get(hc[i]){acc^=r.sum();} } acc });
        println!("random valid lookup ns:BASE={:>11.0}  A={:>11.0} ({:+.1}%)  B={:>11.0} ({:+.1}%)  C={:>11.0} ({:+.1}%)",
            lk_base, lk_a,(lk_a-lk_base)/lk_base*100.0, lk_b,(lk_b-lk_base)/lk_base*100.0, lk_c,(lk_c-lk_base)/lk_base*100.0);

        // ---- sequential iteration ----
        let it_base = timed(9, || { let mut acc=0u64; for r in &base.v { acc^=r.sum(); } acc });
        let it_a = timed(9, || aa.iter_sum());
        let it_b = timed(9, || ab.iter_sum());
        let it_c = timed(9, || ac.iter_sum());
        println!("iteration ns:          BASE={:>11.0}  A={:>11.0} ({:+.1}%)  B={:>11.0} ({:+.1}%)  C={:>11.0} ({:+.1}%)",
            it_base, it_a,(it_a-it_base)/it_base*100.0, it_b,(it_b-it_base)/it_base*100.0, it_c,(it_c-it_base)/it_base*100.0);

        // ---- stale lookup O(1) sanity (remove one, lookup old handle) ----
        // (correctness only — not timed)
        {
            let mut a2 = ArenaB::with_capacity(4);
            let h = a2.insert(Rec::new(1));
            a2.remove(h);
            assert!(a2.get(h).is_none(), "stale must be rejected");
            let h2 = a2.insert(Rec::new(2)); // reuses slot 0, gen bumped
            assert!(a2.get(h).is_none(), "old handle still stale after reuse");
            assert!(a2.get(h2).is_some());
        }

        // ---- bytes accounting ----
        println!("bytes/live:            BASE={:>6.1}  A={:>6.1}  B={:>6.1}  C={:>6.1}",
            base.bytes() as f64/n as f64, aa.bytes() as f64/n as f64, ab.bytes() as f64/n as f64, ac.bytes() as f64/n as f64);

        // ---- clone ----
        let cl_base = timed(9, || { let c = base.v.clone(); c.len() as u64 });
        let cl_a = timed(9, || { let c: Vec<(u32,Option<Rec>)> = aa.slots.iter().map(|s|(s.gen,s.val)).collect(); c.len() as u64 });
        let cl_b = timed(9, || { let g=ab.gens.clone(); let v=ab.vals.clone(); (g.len()+v.len()) as u64 });
        let cl_c = timed(9, || { let d=ac.dense.clone(); d.len() as u64 });
        println!("clone ns:              BASE={:>11.0}  A={:>11.0} ({:+.1}%)  B={:>11.0} ({:+.1}%)  C={:>11.0} ({:+.1}%)",
            cl_base, cl_a,(cl_a-cl_base)/cl_base*100.0, cl_b,(cl_b-cl_base)/cl_base*100.0, cl_c,(cl_c-cl_base)/cl_base*100.0);
    }

    // ---- tombstone-density lookup + iteration (B, the leading rep) ----
    println!("\n================ tombstone density (N=1,000,000, rep B) ================");
    let n = 1_000_000usize;
    for &pct in &[0u32, 1, 10, 30, 50] {
        let mut ab = ArenaB::with_capacity(n);
        let mut hs: Vec<Handle> = (0..n).map(|i| ab.insert(Rec::new(i))).collect();
        // remove `pct`% (deterministic stride), leaving tombstones (no reuse)
        let rm = (n as u64 * pct as u64 / 100) as usize;
        let ord = shuffle(n);
        for k in 0..rm { ab.remove(hs[ord[k]]); }
        // live handles for lookup
        let live: Vec<Handle> = (rm..n).map(|k| hs[ord[k]]).collect();
        let lord = shuffle(live.len());
        let lk = timed(7, || { let mut acc=0u64; for &j in &lord { if let Some(r)=ab.get(live[j]){acc^=r.sum();} } acc });
        let it = timed(7, || ab.iter_sum());
        println!("tomb {:>2}%: live={:>9}  lookup ns={:>11.0}  iter ns={:>11.0}  bytes/live={:>6.1}",
            pct, ab.live, lk, it, ab.bytes() as f64/ab.live.max(1) as f64);
        black_box(&mut hs);
    }

    // ---- insert/remove/reuse churn (B) ----
    println!("\n================ insert/remove/reuse churn (rep B) ================");
    let churn = timed(9, || {
        let mut ab = ArenaB::with_capacity(100_000);
        let mut hs: Vec<Handle> = (0..100_000).map(|i| ab.insert(Rec::new(i))).collect();
        let ord = shuffle(100_000);
        for round in 0..3 {
            for k in 0..50_000 { ab.remove(hs[ord[k]]); }
            for k in 0..50_000 { hs[ord[k]] = ab.insert(Rec::new(round*1_000_000 + k)); }
        }
        ab.live as u64
    });
    println!("100k x 3 half-churn ns = {:.0}  (allocations amortized via free-list reuse)", churn);
}
