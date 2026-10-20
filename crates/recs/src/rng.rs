//! A small seeded generator: every pick of the engine is a function of the
//! caller's seed (and `now`), so a mix is reproducible and tests are stable.

/// SplitMix64.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// An independent stream for one purpose of one seed.
    pub fn derive(seed: u64, salt: u64) -> Self {
        let mut r = Self::new(seed ^ salt.wrapping_mul(0xD1B5_4A32_D192_ED03));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn f32(&mut self) -> f32 {
        self.f64() as f32
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.f64() * n as f64) as usize % n.max(1)
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }

    /// Up to `k` of `items` without replacement, each drawn with probability
    /// proportional to its weight (Efraimidis–Spirakis); items with a weight
    /// that is not positive are never drawn. The first drawn comes first.
    pub fn weighted_sample<T: Copy>(&mut self, items: &[(T, f64)], k: usize) -> Vec<T> {
        let mut keyed: Vec<(f64, usize)> = items
            .iter()
            .enumerate()
            .filter(|(_, (_, w))| *w > 0.0 && w.is_finite())
            .map(|(i, (_, w))| {
                // ln(u) / w: the larger, the earlier (u^(1/w) without the pow).
                let u = self.f64().max(f64::MIN_POSITIVE);
                (u.ln() / w, i)
            })
            .collect();
        keyed.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        keyed.into_iter().take(k).map(|(_, i)| items[i].0).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_and_weighted() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        assert_eq!(a.next_u64(), b.next_u64());
        assert_ne!(Rng::derive(7, 1).next_u64(), Rng::derive(7, 2).next_u64());
        let items = [(0u32, 1.0), (1, 100.0), (2, 0.0), (3, -1.0)];
        let mut first_is_heavy = 0;
        for s in 0..200 {
            let got = Rng::new(s).weighted_sample(&items, 4);
            assert_eq!(got.len(), 2, "weights that are not positive never come");
            first_is_heavy += usize::from(got[0] == 1);
        }
        assert!(first_is_heavy > 180, "{first_is_heavy}");
        let mut v: Vec<u32> = (0..50).collect();
        Rng::new(1).shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(v, sorted);
    }
}
