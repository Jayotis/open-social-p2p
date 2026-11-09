//! A small seeded random number generator (SplitMix64), so that a run of the
//! model can be repeated exactly, without a new dependency.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from 0 up to, not including, 1.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.f64()
    }

    /// A number from 0 up to, not including, `n`, which must be above 0.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.f64() * n as f64) as usize).min(n - 1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    pub fn shuffle<T>(&mut self, list: &mut [T]) {
        for i in (1..list.len()).rev() {
            let j = self.below(i + 1);
            list.swap(i, j);
        }
    }

    /// A random wait as the node varies one: from half to one and a half
    /// times `d` (`jitter` in `osp-node`).
    pub fn jitter(&mut self, d: f64) -> f64 {
        d * self.range(0.5, 1.5)
    }
}
