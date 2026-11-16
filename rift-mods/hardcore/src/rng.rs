//! A small random source for `@r`, with a seed a test can set.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9, |d| d.as_nanos() as u64);
        Self::new(nanos)
    }

    /// A number from 0 up to, but not including, 1.
    pub fn unit(&mut self) -> f64 {
        const STEPS: u32 = 1 << 24;
        f64::from(self.below(STEPS)) / f64::from(STEPS)
    }

    /// A number in `0..n` (0 for `n` of 0).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) % u64::from(n)) as u32
    }
}
