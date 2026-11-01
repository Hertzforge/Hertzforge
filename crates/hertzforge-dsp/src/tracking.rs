//! Tracking-filter bandwidth modes, an adaptive (frequency-following) wrapper
//! over [`crate::sine::TrackingFilter`], and a harmonic lock-in bank for THD.
//!
//! The base `TrackingFilter` is fixed-bandwidth. Sine control wants the corner
//! to follow the sweep: proportional bandwidth (a percentage of the current
//! drive frequency, capped) is the standard controller behavior, and BOTH modes
//! must respect the image-rejection ceiling described on
//! [`BandwidthMode::effective_bw`].

use crate::sine::TrackingFilter;
use std::f64::consts::TAU;

/// How the tracking-filter low-pass corner is derived from the instantaneous
/// drive frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BandwidthMode {
    /// Fixed corner in Hz (subject to the image-rejection clamp).
    FixedHz(f64),
    /// Proportional corner: `pct` percent of the drive frequency, capped at
    /// `max_hz` (then subject to the image-rejection clamp).
    Proportional { pct: f64, max_hz: f64 },
}

impl BandwidthMode {
    /// Effective low-pass corner at drive frequency `f_hz`.
    ///
    /// The mode rule (fixed value, or `pct% * f` capped at `max_hz`) is then
    /// CLAMPED to `<= 0.5 * f_hz` in BOTH modes. This is the image-rejection
    /// ceiling: the lock-in demodulation leaves an image at `2f` in baseband,
    /// and the low-pass corner must stay well below `2f` for the two-pole
    /// cascade to remove it. Capping the corner at `f/2` guarantees the image
    /// sits at least 4 corner-widths out (>= ~24 dB of two-pole rejection) no
    /// matter how low the sweep goes; without the clamp a fixed 10 Hz corner
    /// at a 5 Hz drive would pass its own `2f = 10 Hz` image straight through
    /// and bias the amplitude estimate.
    ///
    /// Panics unless `f_hz > 0`.
    pub fn effective_bw(&self, f_hz: f64) -> f64 {
        assert!(f_hz > 0.0, "BandwidthMode: f_hz must be positive");
        let rule = match *self {
            BandwidthMode::FixedHz(bw) => bw,
            BandwidthMode::Proportional { pct, max_hz } => (f_hz * pct / 100.0).min(max_hz),
        };
        rule.min(0.5 * f_hz)
    }
}

/// [`crate::sine::TrackingFilter`] wrapper that re-derives the low-pass corner
/// from the instantaneous frequency on every update, via the state-preserving
/// [`TrackingFilter::set_bandwidth`] (no dropout at a retune).
///
/// Retunes are applied when the effective bandwidth moves by more than 0.1%
/// relative, so the per-sample cost in a slow sweep is one comparison, not one
/// `exp()`.
#[derive(Clone, Debug)]
pub struct AdaptiveTrackingFilter {
    filter: TrackingFilter,
    mode: BandwidthMode,
    /// Bandwidth currently programmed into `filter` (NaN until first update).
    current_bw: f64,
}

impl AdaptiveTrackingFilter {
    /// Build at sample rate `fs_hz` with the given bandwidth mode. Mode
    /// parameters are validated here: `FixedHz(bw)` needs `bw > 0`;
    /// `Proportional` needs `pct > 0` and `max_hz > 0`.
    pub fn new(fs_hz: f64, mode: BandwidthMode) -> Self {
        match mode {
            BandwidthMode::FixedHz(bw) => {
                assert!(bw > 0.0, "AdaptiveTrackingFilter: FixedHz bw must be > 0");
            }
            BandwidthMode::Proportional { pct, max_hz } => {
                assert!(
                    pct > 0.0,
                    "AdaptiveTrackingFilter: Proportional pct must be > 0"
                );
                assert!(
                    max_hz > 0.0,
                    "AdaptiveTrackingFilter: Proportional max_hz must be > 0"
                );
            }
        }
        // Placeholder corner; the first update() always retunes (current_bw is
        // NaN) before any sample is demodulated with this value's speed.
        let filter = TrackingFilter::new(fs_hz, fs_hz / 8.0);
        Self {
            filter,
            mode,
            current_bw: f64::NAN,
        }
    }

    /// The bandwidth mode this wrapper was built with.
    pub fn mode(&self) -> BandwidthMode {
        self.mode
    }

    /// Effective bandwidth the filter would use at `f_hz` (see
    /// [`BandwidthMode::effective_bw`]).
    pub fn effective_bw(&self, f_hz: f64) -> f64 {
        self.mode.effective_bw(f_hz)
    }

    /// First-order settling-time budget at `f_hz`: `1 / effective_bw`.
    pub fn settling_s(&self, f_hz: f64) -> f64 {
        1.0 / self.mode.effective_bw(f_hz)
    }

    /// Feed one sample at instantaneous drive frequency `inst_freq_hz`;
    /// returns `(amplitude_peak, phase_rad)` exactly like
    /// [`TrackingFilter::update`]. The low-pass corner is re-derived from
    /// `inst_freq_hz` first (state-preserving retune).
    pub fn update(&mut self, sample: f64, inst_freq_hz: f64) -> (f64, f64) {
        let bw = self.mode.effective_bw(inst_freq_hz);
        if self.current_bw.is_nan() || (bw - self.current_bw).abs() > 1e-3 * self.current_bw {
            self.filter.set_bandwidth(bw);
            self.current_bw = bw;
        }
        self.filter.update(sample, inst_freq_hz)
    }
}

/// Highest harmonic order tracked by [`ThdBank`] (2..=10 inclusive).
const THD_MAX_HARMONIC: usize = 10;
/// Number of harmonic lock-in stages (orders 2..=10).
const THD_STAGES: usize = THD_MAX_HARMONIC - 1;

/// One two-pole lock-in accumulator pair (same topology as the baseband
/// low-pass inside [`crate::sine::TrackingFilter`]).
#[derive(Clone, Copy, Debug, Default)]
struct LockinStage {
    i_lp1: f64,
    i_lp2: f64,
    q_lp1: f64,
    q_lp2: f64,
}

impl LockinStage {
    fn zero(&mut self) {
        *self = LockinStage::default();
    }

    fn update(&mut self, i: f64, q: f64, alpha: f64) {
        self.i_lp1 += alpha * (i - self.i_lp1);
        self.i_lp2 += alpha * (self.i_lp1 - self.i_lp2);
        self.q_lp1 += alpha * (q - self.q_lp1);
        self.q_lp2 += alpha * (self.q_lp1 - self.q_lp2);
    }

    fn amplitude(&self) -> f64 {
        2.0 * (self.i_lp2 * self.i_lp2 + self.q_lp2 * self.q_lp2).sqrt()
    }
}

/// Bank of lock-in amplifiers at harmonics `2..=10` of the fundamental, for
/// live THD readout during a sine sweep.
///
/// The harmonic quadrature references are generated by the Chebyshev
/// recurrence on the fundamental's `(cos, sin)` pair,
///
/// ```text
///   cos((k+1)t) = 2 cos(t) cos(kt) - cos((k-1)t)
///   sin((k+1)t) = 2 cos(t) sin(kt) - sin((k-1)t)
/// ```
///
/// so ONE `cos`/`sin` evaluation per sample serves all nine stages - no
/// per-harmonic trig. The phase fed in must be the SAME reference phase the
/// drive NCO used for the sample (see
/// [`crate::sweep::SweepOscillator::next`]), which keeps every harmonic
/// lock-in exactly phase-locked to the drive as it sweeps.
///
/// Harmonics whose frequency `k * f` would sit at or above Nyquist (`fs/2`)
/// are skipped AND their accumulated state is zeroed, so a harmonic that
/// sweeps out of band cannot leave a stale amplitude in the THD sum (it
/// re-acquires from zero if the sweep brings it back in band).
#[derive(Clone, Debug)]
pub struct ThdBank {
    fs_hz: f64,
    /// Per-pole low-pass coefficient, same convention as the tracking filter.
    alpha: f64,
    stages: [LockinStage; THD_STAGES],
}

impl ThdBank {
    /// Build a bank at sample rate `fs_hz` with lock-in low-pass corner
    /// `bandwidth_hz` (same domain rules as
    /// [`crate::sine::TrackingFilter::new`]: `0 < bandwidth < fs/2`).
    pub fn new(fs_hz: f64, bandwidth_hz: f64) -> Self {
        assert!(fs_hz > 0.0, "ThdBank: fs_hz must be positive");
        assert!(
            bandwidth_hz > 0.0 && bandwidth_hz < fs_hz / 2.0,
            "ThdBank: bandwidth must be in (0, fs/2)"
        );
        let alpha = 1.0 - (-TAU * bandwidth_hz / fs_hz).exp();
        Self {
            fs_hz,
            alpha,
            stages: [LockinStage::default(); THD_STAGES],
        }
    }

    /// Reset all lock-in state to zero.
    pub fn reset(&mut self) {
        for s in self.stages.iter_mut() {
            s.zero();
        }
    }

    /// Feed one sample. `phase_rad` is the fundamental reference phase for
    /// THIS sample (from the shared NCO); `inst_freq_hz` is the fundamental's
    /// instantaneous frequency, used only to skip out-of-band harmonics.
    pub fn update(&mut self, sample: f64, inst_freq_hz: f64, phase_rad: f64) {
        let c1 = phase_rad.cos();
        let s1 = phase_rad.sin();
        let nyq = self.fs_hz / 2.0;
        // Chebyshev recurrence state: (cos, sin) at harmonic k-1 and k.
        let (mut c_prev, mut s_prev) = (1.0, 0.0); // k = 0
        let (mut c_k, mut s_k) = (c1, s1); // k = 1
        for k in 2..=THD_MAX_HARMONIC {
            let c_next = 2.0 * c1 * c_k - c_prev;
            let s_next = 2.0 * c1 * s_k - s_prev;
            c_prev = c_k;
            s_prev = s_k;
            c_k = c_next;
            s_k = s_next;

            let stage = &mut self.stages[k - 2];
            if k as f64 * inst_freq_hz >= nyq {
                // Out of band: skip AND clear so no stale amplitude lingers.
                stage.zero();
            } else {
                stage.update(sample * s_k, sample * c_k, self.alpha);
            }
        }
    }

    /// Peak amplitude estimates for harmonics 2..=10 (index 0 = 2nd harmonic).
    pub fn harmonic_amplitudes(&self) -> [f64; THD_STAGES] {
        let mut out = [0.0; THD_STAGES];
        for (o, s) in out.iter_mut().zip(self.stages.iter()) {
            *o = s.amplitude();
        }
        out
    }

    /// Total harmonic distortion: `sqrt(sum A_k^2) / A_1` for `k = 2..=10`,
    /// with `A_1 = fundamental_amp` supplied by the caller (typically the
    /// tracking filter's amplitude readout). Returns NaN when
    /// `fundamental_amp` is not a positive number - THD is undefined without
    /// a fundamental.
    pub fn thd(&self, fundamental_amp: f64) -> f64 {
        if fundamental_amp <= 0.0 || fundamental_amp.is_nan() {
            return f64::NAN;
        }
        let sum_sq: f64 = self
            .stages
            .iter()
            .map(|s| {
                let a = s.amplitude();
                a * a
            })
            .sum();
        sum_sq.sqrt() / fundamental_amp
    }
}
