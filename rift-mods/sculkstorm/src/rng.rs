//! A small seeded generator for the crater's layout and the effects' scatter.
//!
//! It is not for anything that needs unpredictability. Its use is that the same seed makes the
//! same crater, which the tests rely on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// SplitMix64.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// A generator seeded from the clock and a counter, so two made in the same instant differ.
    pub fn from_entropy() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let n = COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        Self::new(nanos ^ n.rotate_left(17))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// In `[lo, hi)`.
    pub fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// In `0..n`; 0 for an `n` of 0.
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * u64::from(n)) >> 32) as u32
    }

    /// In `lo..hi`; `lo` when the range is empty.
    pub fn int_between(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + self.below((hi - lo) as u32) as i32
    }

    /// Whether a roll of 100 comes under `percent`.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }

    pub fn flip(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers_and_another_gives_others() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let mut c = Rng::new(8);
        let first: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        assert_eq!(first, (0..8).map(|_| b.next_u64()).collect::<Vec<_>>());
        assert_ne!(first, (0..8).map(|_| c.next_u64()).collect::<Vec<_>>());
    }

    #[test]
    fn draws_stay_inside_their_ranges() {
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            let x = r.between(-0.8, 0.8);
            assert!((-0.8..0.8).contains(&x));
            assert!(r.below(5) < 5);
            assert!((1..3).contains(&r.int_between(1, 3)));
        }
        assert_eq!(r.below(0), 0);
        assert_eq!(r.int_between(4, 4), 4);
    }

    #[test]
    fn a_percent_chance_comes_up_about_that_often() {
        let mut r = Rng::new(99);
        let hits = (0..20_000).filter(|_| r.chance(14)).count();
        // 14% of 20 000 is 2 800: far outside any sane spread of a fair generator.
        assert!((2_500..3_100).contains(&hits), "{hits}");
        assert!(!(0..1000).any(|_| r.chance(0)));
        assert!((0..1000).all(|_| r.chance(100)));
    }
}
