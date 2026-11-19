//! Test helpers: a seeded Gaussian source (no dependency) and white noise of
//! a stated one-sided density.

/// A seeded Gaussian source (xorshift64* and Box-Muller).
pub(crate) struct Gauss {
    s: u64,
    spare: Option<f64>,
}

impl Gauss {
    pub(crate) fn new(seed: u64) -> Self {
        Gauss {
            s: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            spare: None,
        }
    }

    fn uniform(&mut self) -> f64 {
        self.s ^= self.s >> 12;
        self.s ^= self.s << 25;
        self.s ^= self.s >> 27;
        let x = self.s.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// One standard normal sample.
    pub(crate) fn next(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        let (u, v) = (self.uniform(), self.uniform());
        let r = (-2.0 * u.ln()).sqrt();
        let t = std::f64::consts::TAU * v;
        self.spare = Some(r * t.sin());
        r * t.cos()
    }

    /// One sample of white noise of one-sided density `g` at `fs`.
    pub(crate) fn white(&mut self, g: f64, fs: f64) -> f64 {
        (g * fs / 2.0).sqrt() * self.next()
    }
}
