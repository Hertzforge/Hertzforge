//! Spectrum averaging modes (linear, exponential, peak hold) for the analyzer
//! display pipeline.
//!
//! Operates on any per-bin nonnegative spectral quantity (raw periodograms,
//! PSDs, autopower). Averaging RAW periodograms and normalizing afterward is
//! equivalent to averaging normalized spectra (the normalizations are linear),
//! which is what makes the live pipeline's `periodogram -> SpectrumAverager ->
//! to_psd` path bit-comparable with `spectrum::psd_welch` - see the
//! `crate::spectrum_ext` equivalence golden.

/// How pushed spectra are combined.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AveragingMode {
    /// Uniform mean of the first `count` spectra; accumulation STOPS once
    /// `count` have been averaged (the standard "N averages then hold"
    /// analyzer behavior - later pushes are ignored).
    Linear { count: usize },
    /// Exponential (RC) averaging `y = (1 - alpha) * y + alpha * x`, starting
    /// from `y = 0`, so the step response after `k` pushes of a constant `x`
    /// is `x * (1 - (1 - alpha)^k)`.
    Exponential { alpha: f64 },
    /// Element-wise maximum over all pushes (initialized from the first push).
    PeakHold,
}

/// Streaming spectrum averager over a fixed bin count.
#[derive(Clone, Debug)]
pub struct SpectrumAverager {
    mode: AveragingMode,
    acc: Vec<f64>,
    n_pushed: usize,
}

impl SpectrumAverager {
    /// Build an averager for spectra of `n_bins` bins.
    ///
    /// Panics on invalid mode parameters: `Linear` needs `count >= 1`;
    /// `Exponential` needs `alpha` in `(0, 1]`.
    pub fn new(mode: AveragingMode, n_bins: usize) -> Self {
        match mode {
            AveragingMode::Linear { count } => {
                assert!(count >= 1, "SpectrumAverager: linear count must be >= 1");
            }
            AveragingMode::Exponential { alpha } => {
                assert!(
                    alpha > 0.0 && alpha <= 1.0,
                    "SpectrumAverager: exponential alpha must be in (0, 1]"
                );
            }
            AveragingMode::PeakHold => {}
        }
        Self {
            mode,
            acc: vec![0.0; n_bins],
            n_pushed: 0,
        }
    }

    /// The averaging mode.
    pub fn mode(&self) -> AveragingMode {
        self.mode
    }

    /// Number of bins per spectrum.
    pub fn n_bins(&self) -> usize {
        self.acc.len()
    }

    /// Push one spectrum. Panics if `spectrum.len() != n_bins`.
    pub fn push(&mut self, spectrum: &[f64]) {
        assert_eq!(
            spectrum.len(),
            self.acc.len(),
            "SpectrumAverager: spectrum length must match n_bins"
        );
        match self.mode {
            AveragingMode::Linear { count } => {
                if self.n_pushed < count {
                    // Numerically-stable running mean of the first `count`.
                    let k = (self.n_pushed + 1) as f64;
                    for (a, &x) in self.acc.iter_mut().zip(spectrum) {
                        *a += (x - *a) / k;
                    }
                }
                // Pushes beyond `count` are counted but do not accumulate.
            }
            AveragingMode::Exponential { alpha } => {
                for (a, &x) in self.acc.iter_mut().zip(spectrum) {
                    *a += alpha * (x - *a);
                }
            }
            AveragingMode::PeakHold => {
                if self.n_pushed == 0 {
                    self.acc.copy_from_slice(spectrum);
                } else {
                    for (a, &x) in self.acc.iter_mut().zip(spectrum) {
                        if x > *a {
                            *a = x;
                        }
                    }
                }
            }
        }
        self.n_pushed += 1;
    }

    /// The current average (zeros before the first push).
    pub fn average(&self) -> &[f64] {
        &self.acc
    }

    /// Total number of spectra pushed (including, for `Linear`, pushes past
    /// `count` that no longer accumulate).
    pub fn n_pushed(&self) -> usize {
        self.n_pushed
    }

    /// Whether the average is "complete": `Linear` after `count` pushes;
    /// `Exponential` and `PeakHold` after the first push.
    pub fn complete(&self) -> bool {
        match self.mode {
            AveragingMode::Linear { count } => self.n_pushed >= count,
            AveragingMode::Exponential { .. } | AveragingMode::PeakHold => self.n_pushed >= 1,
        }
    }

    /// Clear the accumulator and the push count.
    pub fn reset(&mut self) {
        self.acc.iter_mut().for_each(|a| *a = 0.0);
        self.n_pushed = 0;
    }
}
