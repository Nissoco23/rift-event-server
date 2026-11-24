//! A small random number generator, for shuffling the pool and picking a player: splitmix64, which
//! is fast, has no bad seeds and is plenty for choosing among a few hundred names. Nothing here is
//! secret, so nothing here needs more.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Seeded from the clock and the process, so a restarted server does not draw the same names.
    pub fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Self::new(nanos ^ (u64::from(std::process::id()) << 32))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from 0 up to but not including `n` (`n` above 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Fisher-Yates.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shuffle_keeps_everything_and_changes_with_the_seed() {
        let original: Vec<u32> = (0..50).collect();
        let mut a = original.clone();
        Rng::new(1).shuffle(&mut a);
        let mut b = original.clone();
        Rng::new(2).shuffle(&mut b);
        assert_ne!(a, original);
        assert_ne!(a, b);
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, original);
        // The same seed gives the same shuffle: a test can rely on it.
        let mut again = original.clone();
        Rng::new(1).shuffle(&mut again);
        assert_eq!(again, a);
    }

    #[test]
    fn below_stays_under_and_reaches_every_value() {
        let mut rng = Rng::new(7);
        let mut seen = [false; 5];
        for _ in 0..200 {
            let n = rng.below(5);
            assert!(n < 5);
            seen[n] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }
}
