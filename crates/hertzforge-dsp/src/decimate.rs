//! Anti-aliased decimation by powers of two for the recorder's analysis-span
//! ladder (a naive stride decimator aliases - +24 dB PSD error at
//! large factors on white noise - so every rate reduction goes through a real
//! half-band low-pass first).
//!
//! # Filter design (pinned)
//!
//! Each [`HalfBandDecimator`] stage is a 63-tap Kaiser-windowed half-band FIR:
//!
//! - Ideal half-band prototype, cutoff at `fs/4` (center tap `0.5`, even
//!   offsets from center exactly zero, odd offsets `sin(pi*d/2)/(pi*d)`).
//! - Kaiser window with `beta = 7.857`, from Kaiser's stopband formula
//!   `beta = 0.1102 * (A - 8.7)` at `A = 80 dB`.
//! - Tap count from Kaiser's length estimate `N ~= (A - 8) / (2.285 *
//!   d_omega) + 1`: with `N = 63`, the total transition width is `d_omega ~=
//!   0.51 rad` centered on `pi/2`, i.e. passband to `~0.21 * fs_in` and
//!   stopband (>= 80 dB) from `~0.29 * fs_in`.
//!
//! In output-rate terms: the passband is flat to `~0.84 * f_nyq_out` and
//! every input component that would alias into `0..0.84 * f_nyq_out` is at
//! least 80 dB down. The conventional analysis span of `fs/2.56` (=
//! `0.78 * f_nyq_out`) therefore sits entirely inside the protected region.
//!
//! The taps are computed at construction from this recipe (dependency-free
//! Kaiser/Bessel evaluation below) rather than pasted as literals, so the
//! derivation IS the implementation; the alias-rejection, passband-flatness
//! and PSD-invariance goldens in `tests/` pin the resulting response.

/// Stage tap count (odd). See the module docs for the derivation.
const HALFBAND_TAPS: usize = 63;
/// Kaiser beta for >= 80 dB stopband: `0.1102 * (80 - 8.7)`.
const HALFBAND_KAISER_BETA: f64 = 7.857;

/// Zeroth-order modified Bessel function of the first kind, by the standard
/// power series (converges fast for the argument range Kaiser windows use).
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    let mut k = 1.0f64;
    loop {
        term *= (half / k) * (half / k);
        sum += term;
        if term < sum * 1e-18 {
            return sum;
        }
        k += 1.0;
    }
}

/// Kaiser-windowed ideal half-band low-pass, `n_taps` odd.
fn design_halfband_kaiser(n_taps: usize, beta: f64) -> Vec<f64> {
    assert!(
        n_taps >= 3 && n_taps % 2 == 1,
        "half-band FIR needs an odd tap count >= 3"
    );
    let m = ((n_taps - 1) / 2) as isize;
    let i0_beta = bessel_i0(beta);
    (0..n_taps as isize)
        .map(|n| {
            let d = n - m;
            let ideal = if d == 0 {
                0.5
            } else if d % 2 == 0 {
                0.0
            } else {
                let df = d as f64;
                (std::f64::consts::FRAC_PI_2 * df).sin() / (std::f64::consts::PI * df)
            };
            let r = d as f64 / m as f64;
            let w = bessel_i0(beta * (1.0 - r * r).sqrt()) / i0_beta;
            ideal * w
        })
        .collect()
}

/// One streaming decimate-by-2 stage (symmetric odd-length half-band FIR, see
/// the module docs for the pinned design). Keeps filter history across
/// [`HalfBandDecimator::process`] calls, so arbitrary block sizes stream
/// correctly; history before the first sample is zeros, so the stage delays
/// the signal by [`HalfBandDecimator::group_delay_samples`] input samples.
#[derive(Clone, Debug)]
pub struct HalfBandDecimator {
    taps: Vec<f64>,
    /// Delay line + unconsumed input. The next output's convolution window
    /// ends at `buf[next_center]`.
    buf: Vec<f64>,
    next_center: usize,
}

impl HalfBandDecimator {
    /// Build a stage with the pinned 63-tap / 80 dB Kaiser half-band design.
    pub fn new() -> Self {
        let taps = design_halfband_kaiser(HALFBAND_TAPS, HALFBAND_KAISER_BETA);
        let hist = taps.len() - 1;
        Self {
            taps,
            buf: vec![0.0; hist],
            next_center: hist,
        }
    }

    /// Filter + take every second sample. Output length depends on the phase
    /// carried across calls; over a long run it is `~input.len() / 2`.
    pub fn process(&mut self, input: &[f64]) -> Vec<f64> {
        self.buf.extend_from_slice(input);
        let t = self.taps.len();
        let mut out = Vec::with_capacity(input.len() / 2 + 1);
        while self.next_center < self.buf.len() {
            let start = self.next_center + 1 - t;
            let window = &self.buf[start..=self.next_center];
            // Symmetric taps: convolution == correlation, so a forward zip is
            // the full convolution sum y = sum_j h[j] * x[center - j].
            let y: f64 = self.taps.iter().zip(window).map(|(&h, &x)| h * x).sum();
            out.push(y);
            self.next_center += 2;
        }
        // Drop consumed samples, keeping the t-1 history the next call needs.
        let keep_from = (self.next_center + 1 - t).min(self.buf.len());
        self.buf.drain(..keep_from);
        self.next_center -= keep_from;
        out
    }

    /// Clear history (back to the zero-primed start state).
    pub fn reset(&mut self) {
        let hist = self.taps.len() - 1;
        self.buf.clear();
        self.buf.resize(hist, 0.0);
        self.next_center = hist;
    }

    /// Group delay of this stage in ITS OWN input-rate samples:
    /// `(n_taps - 1) / 2` (exact - the FIR is linear-phase symmetric).
    pub fn group_delay_samples(&self) -> f64 {
        (self.taps.len() - 1) as f64 / 2.0
    }
}

impl Default for HalfBandDecimator {
    fn default() -> Self {
        Self::new()
    }
}

/// Cascade of half-band stages implementing decimation by a power of two.
/// `factor = 1` is a pass-through (zero stages).
#[derive(Clone, Debug)]
pub struct DecimatorCascade {
    stages: Vec<HalfBandDecimator>,
    factor: u32,
}

impl DecimatorCascade {
    /// Build a cascade for the given decimation `factor`.
    ///
    /// Panics unless `factor` is a power of two (`1, 2, 4, 8, ...`): each
    /// stage halves the rate, so non-power-of-two factors are not
    /// representable by this cascade.
    pub fn new(factor: u32) -> Self {
        assert!(
            factor >= 1 && factor.is_power_of_two(),
            "DecimatorCascade: factor must be a power of two (1, 2, 4, ...), got {factor}"
        );
        let k = factor.trailing_zeros();
        Self {
            stages: (0..k).map(|_| HalfBandDecimator::new()).collect(),
            factor,
        }
    }

    /// The decimation factor.
    pub fn factor(&self) -> u32 {
        self.factor
    }

    /// Stream one block through every stage. Output length is
    /// `~input.len() / factor` (phase carried across calls).
    pub fn process(&mut self, input: &[f64]) -> Vec<f64> {
        let mut cur = input.to_vec();
        for stage in self.stages.iter_mut() {
            cur = stage.process(&cur);
        }
        cur
    }

    /// Reset every stage to the zero-primed start state.
    pub fn reset(&mut self) {
        for stage in self.stages.iter_mut() {
            stage.reset();
        }
    }

    /// Total group delay referred to the cascade INPUT rate, in input
    /// samples: stage `i` (0-based) runs at `fs_in / 2^i`, so its
    /// `(n_taps - 1) / 2` delay counts `2^i` input samples each.
    pub fn group_delay_samples(&self) -> f64 {
        self.stages
            .iter()
            .enumerate()
            .map(|(i, s)| s.group_delay_samples() * (1u64 << i) as f64)
            .sum()
    }
}
