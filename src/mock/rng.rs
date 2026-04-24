//! Tiny deterministic LCG, cloned from the design prototype so the output
//! matches value-for-value.

#[derive(Clone, Copy)]
pub struct Lcg {
    state: u32,
}

impl Lcg {
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    fn advance(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.state
    }

    /// Next u32 in the sequence.
    pub fn next_u32(&mut self) -> u32 {
        self.advance()
    }

    /// Uniform `[0.0, 1.0)` float.
    pub fn next_f32(&mut self) -> f32 {
        // Matches `x / 0xffffffff` from the JS version.
        self.advance() as f32 / u32::MAX as f32
    }

    /// `floor(rand() * bound)` — common JS pattern.
    pub fn gen_range(&mut self, bound: u32) -> u32 {
        (self.next_f32() * bound as f32) as u32
    }
}

/// FNV-1a over a byte slice — used to derive stable seeds from addresses.
pub fn hash_str(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16_777_619);
    }
    h
}
