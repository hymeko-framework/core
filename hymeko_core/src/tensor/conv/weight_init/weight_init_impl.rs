use crate::tensor::common::Real;
use crate::tensor::conv::weight_init::weight_init::{
    Constant, Kaiming, KaimingRandom, LeCun, Ones, WeightInit, Xavier, XavierRandom, Zeros,
};

// ─── Deterministic initializers (no RNG needed) ───────────────────────────

impl<F: Real> WeightInit<F> for Xavier {
    fn init(&self, fan_in: usize, fan_out: usize, out: &mut [F]) {
        let scale = F::from_other((6.0 / (fan_in + fan_out) as f64).sqrt());
        // Deterministic low-discrepancy fill using van der Corput sequence
        for (i, w) in out.iter_mut().enumerate() {
            let vdc = van_der_corput(i as u32);
            // Map [0, 1) → [-scale, +scale)
            *w = F::from_other(vdc * 2.0 - 1.0) * scale;
        }
    }
}

impl<F: Real> WeightInit<F> for Kaiming {
    fn init(&self, fan_in: usize, _fan_out: usize, out: &mut [F]) {
        let scale = F::from_other((6.0 / fan_in as f64).sqrt());
        for (i, w) in out.iter_mut().enumerate() {
            let vdc = van_der_corput(i as u32);
            *w = F::from_other(vdc * 2.0 - 1.0) * scale;
        }
    }
}

impl<F: Real> WeightInit<F> for LeCun {
    fn init(&self, fan_in: usize, _fan_out: usize, out: &mut [F]) {
        let scale = F::from_other((3.0 / fan_in as f64).sqrt());
        for (i, w) in out.iter_mut().enumerate() {
            let vdc = van_der_corput(i as u32);
            *w = F::from_other(vdc * 2.0 - 1.0) * scale;
        }
    }
}

impl Constant {
    pub fn xavier_scale(fan_in: usize) -> Self {
        Self {
            value: 1.0 / (fan_in as f64).sqrt(),
        }
    }
}

impl<F: Real> WeightInit<F> for Constant {
    fn init(&self, _fan_in: usize, _fan_out: usize, out: &mut [F]) {
        let v = F::from_other(self.value);
        out.fill(v);
    }
}

impl<F: Real> WeightInit<F> for Zeros {
    fn init(&self, _fan_in: usize, _fan_out: usize, out: &mut [F]) {
        out.fill(F::zero());
    }
}

impl<F: Real> WeightInit<F> for Ones {
    fn init(&self, _fan_in: usize, _fan_out: usize, out: &mut [F]) {
        out.fill(F::one());
    }
}

impl<F: Real> WeightInit<F> for XavierRandom {
    fn init(&self, fan_in: usize, fan_out: usize, out: &mut [F]) {
        let scale = (6.0 / (fan_in + fan_out) as f64).sqrt();
        let mut rng = Xoshiro256ss::new(self.seed);
        for w in out.iter_mut() {
            let u = rng.next_f64(); // [0, 1)
            *w = F::from_other((u * 2.0 - 1.0) * scale);
        }
    }
}

// ─── Minimal embedded PRNG (no external dependency) ───────────────────────

/// xoshiro256** — fast, high-quality PRNG. 256-bit state, 64-bit output.
/// Zero dependencies. Seedable. Deterministic across platforms.
struct Xoshiro256ss {
    s: [u64; 4],
}

impl Xoshiro256ss {
    fn new(seed: u64) -> Self {
        // SplitMix64 seeding (recommended by xoshiro authors)
        let mut sm = seed;
        let mut s = [0u64; 4];
        for slot in &mut s {
            sm = sm.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            *slot = z ^ (z >> 31);
        }
        Self { s }
    }

    fn next_u64(&mut self) -> u64 {
        let result = (self.s[1].wrapping_mul(5)).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    fn next_f64(&mut self) -> f64 {
        // Upper 53 bits → [0, 1) with full mantissa precision
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}
impl<F: Real> WeightInit<F> for KaimingRandom {
    fn init(&self, fan_in: usize, _fan_out: usize, out: &mut [F]) {
        let scale = (6.0 / fan_in as f64).sqrt();
        let mut rng = Xoshiro256ss::new(self.seed);
        for w in out.iter_mut() {
            let u = rng.next_f64();
            *w = F::from_other((u * 2.0 - 1.0) * scale);
        }
    }
}

// ─── Van der Corput sequence (deterministic quasi-random) ─────────────────

/// Radical-inverse base 2. Maps integers to [0, 1) with low discrepancy.
/// Better than `i as f64 / n` for weight initialization because it doesn't
/// cluster values — each new sample fills the largest gap.
pub fn van_der_corput(mut n: u32) -> f64 {
    n = (n << 16) | (n >> 16);
    n = ((n & 0x55555555) << 1) | ((n & 0xAAAAAAAA) >> 1);
    n = ((n & 0x33333333) << 2) | ((n & 0xCCCCCCCC) >> 2);
    n = ((n & 0x0F0F0F0F) << 4) | ((n & 0xF0F0F0F0) >> 4);
    n = ((n & 0x00FF00FF) << 8) | ((n & 0xFF00FF00) >> 8);
    n as f64 / (1u64 << 32) as f64
}
