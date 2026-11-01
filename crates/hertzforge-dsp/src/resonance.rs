//! Resonance search-and-track (RSTD) primitives: half-power Q estimation from
//! a swept amplitude curve, and a phase-locked resonance tracker for dwelling
//! on a moving natural frequency.
//!
//! # Phase convention (documented, load-bearing)
//!
//! Throughout this module phase means the response phase RELATIVE to the drive
//! as returned by [`crate::sine::TrackingFilter::update`]: a value in
//! `(-pi, pi]` that, for a base-excited single-degree-of-freedom (SDOF) system
//! (transmissibility `H(r) = (1 + i*2*zeta*r) / (1 - r^2 + i*2*zeta*r)`,
//! `r = f/f_n`, `zeta = 1/(2Q)`), starts near `0` below resonance, passes
//! `~ -pi/2` at `f_n`, and approaches `-pi` above. The phase-vs-frequency
//! slope at resonance is therefore NEGATIVE:
//!
//! ```text
//!   d(phi)/df |_(f = f_n)  ~=  -2 Q / f_n   [rad/Hz]
//! ```
//!
//! (the SDOF group-delay result `d(phi)/d(omega) = -2Q/omega_n` expressed in
//! Hz). That analytic slope is what [`PhaseTrackerConfig::slope_rad_per_hz`]
//! expects for an SDOF resonance under this convention.

use std::f64::consts::{PI, TAU};

/// Result of a half-power (-3 dB) resonance fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResonanceEstimate {
    /// Frequency of the amplitude peak (grid sample of the maximum).
    pub f_peak: f64,
    /// Quality factor `f_peak / (f_hi - f_lo)` from the interpolated
    /// half-power crossings.
    pub q: f64,
    /// Amplitude at the peak.
    pub amp_peak: f64,
}

/// Estimate a resonance from an amplitude-vs-frequency curve by the half-power
/// (-3 dB) bandwidth method.
///
/// Finds the global amplitude peak, then walks outward in each direction to
/// the first crossing of `amp_peak / sqrt(2)`, linearly interpolating the
/// crossing frequency between the bracketing samples. `Q = f_peak / (f_hi -
/// f_lo)`.
///
/// Returns `None` when the estimate is not well-posed: mismatched or too-short
/// inputs (< 3 points), a non-positive/NaN peak, a peak at either end of the
/// span (half-power point not bracketed), or a missing crossing on either
/// side. `freqs` must be monotonically increasing for the interpolation to be
/// meaningful; the function does not sort.
pub fn half_power(freqs: &[f64], amps: &[f64]) -> Option<ResonanceEstimate> {
    if freqs.len() != amps.len() || freqs.len() < 3 {
        return None;
    }
    // Global peak (NaN amplitudes lose every comparison and cannot win).
    let mut ipk = 0usize;
    let mut amp_peak = f64::NEG_INFINITY;
    for (i, &a) in amps.iter().enumerate() {
        if a > amp_peak {
            amp_peak = a;
            ipk = i;
        }
    }
    if amp_peak <= 0.0 || amp_peak.is_nan() {
        return None;
    }
    if ipk == 0 || ipk == amps.len() - 1 {
        return None; // peak on the boundary: half-power points not bracketed
    }
    let target = amp_peak / std::f64::consts::SQRT_2;

    // Walk left: first sample at/below the half-power level.
    let mut f_lo = None;
    for i in (0..ipk).rev() {
        if amps[i] <= target {
            let denom = amps[i + 1] - amps[i];
            if denom <= 0.0 || denom.is_nan() {
                return None;
            }
            let t = (target - amps[i]) / denom;
            f_lo = Some(freqs[i] + t * (freqs[i + 1] - freqs[i]));
            break;
        }
    }
    // Walk right.
    let mut f_hi = None;
    for i in (ipk + 1)..amps.len() {
        if amps[i] <= target {
            let denom = amps[i - 1] - amps[i];
            if denom <= 0.0 || denom.is_nan() {
                return None;
            }
            let t = (amps[i - 1] - target) / denom;
            f_hi = Some(freqs[i - 1] + t * (freqs[i] - freqs[i - 1]));
            break;
        }
    }
    let (f_lo, f_hi) = (f_lo?, f_hi?);
    let bw = f_hi - f_lo;
    if bw <= 0.0 || bw.is_nan() {
        return None;
    }
    Some(ResonanceEstimate {
        f_peak: freqs[ipk],
        q: freqs[ipk] / bw,
        amp_peak,
    })
}

/// Configuration for [`PhaseTracker`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseTrackerConfig {
    /// Phase setpoint to hold, radians (e.g. the phase captured at the
    /// detected resonance after a settle-and-hold).
    pub phase_setpoint_rad: f64,
    /// Local phase-vs-frequency slope `d(phi)/df` at the operating point,
    /// rad/Hz. For an SDOF resonance under this module's convention this is
    /// the ANALYTIC `-2*Q/f_n` (negative). Must be nonzero.
    pub slope_rad_per_hz: f64,
    /// Loop damping in `(0, 1]`; `1` is a full Newton step per update, `0.5`
    /// (the [`PhaseTrackerConfig::new`] default) halves it for robustness to
    /// slope-estimate error.
    pub gain: f64,
    /// Allowed frequency band `(lo_hz, hi_hz)`; the commanded frequency is
    /// clamped into this band on every update.
    pub band: (f64, f64),
    /// Per-update frequency-step magnitude limit, Hz.
    pub max_step_hz: f64,
    /// Number of CONSECUTIVE updates pinned at a band edge before
    /// [`PhaseTracker::band_exit`] latches.
    pub exit_ticks: u32,
}

impl PhaseTrackerConfig {
    /// Convenience constructor with the documented defaults: `gain = 0.5`,
    /// `max_step_hz = 1%` of the band width, `exit_ticks = 5`.
    pub fn new(phase_setpoint_rad: f64, slope_rad_per_hz: f64, band: (f64, f64)) -> Self {
        Self {
            phase_setpoint_rad,
            slope_rad_per_hz,
            gain: 0.5,
            band,
            max_step_hz: (band.1 - band.0) / 100.0,
            exit_ticks: 5,
        }
    }
}

/// Phase-locked resonance tracker (the "dwell on a drifting resonance" loop).
///
/// Each update takes the measured response phase (from the tracking filter)
/// and moves the commanded drive frequency toward the phase setpoint with a
/// damped Newton step on the local phase slope:
///
/// ```text
///   err  = wrap(phi_meas - phi_set)              (shortest angular distance)
///   f   <- clamp_band( f - clamp(gain * err / slope, +-max_step_hz) )
/// ```
///
/// The MINUS sign is the Newton correction: linearizing `phi(f) ~= phi_set +
/// slope * (f - f_star)` gives `f - f_star ~= err / slope`, so the corrective
/// move toward the resonance is `-gain * err / slope`. (A `+` here diverges
/// for either sign convention of the slope; an earlier `f += ...` form was a
/// sign slip.) With `slope = -2Q/f_n` and this module's phase
/// convention: response phase above setpoint means the drive is BELOW the
/// resonance, and the update moves the frequency UP.
///
/// If the resonance walks outside `band`, the commanded frequency pins at the
/// edge; after `exit_ticks` CONSECUTIVE pinned updates, [`PhaseTracker::
/// band_exit`] latches true (and stays latched until [`PhaseTracker::reset`])
/// so the caller can alarm / hold / re-search instead of silently dwelling on
/// a band edge.
#[derive(Clone, Debug)]
pub struct PhaseTracker {
    cfg: PhaseTrackerConfig,
    f_hz: f64,
    pinned_ticks: u32,
    band_exit: bool,
}

impl PhaseTracker {
    /// Build a tracker starting at `f0_hz` (clamped into the band).
    ///
    /// Panics unless: `band.0 > 0` and `band.1 > band.0`; `slope_rad_per_hz`
    /// is nonzero and finite; `gain` in `(0, 1]`; `max_step_hz > 0`;
    /// `exit_ticks >= 1`.
    pub fn new(cfg: PhaseTrackerConfig, f0_hz: f64) -> Self {
        assert!(
            cfg.band.0 > 0.0 && cfg.band.1 > cfg.band.0,
            "PhaseTracker: band must satisfy 0 < lo < hi"
        );
        assert!(
            cfg.slope_rad_per_hz.abs() > 0.0 && cfg.slope_rad_per_hz.is_finite(),
            "PhaseTracker: slope_rad_per_hz must be nonzero and finite"
        );
        assert!(
            cfg.gain > 0.0 && cfg.gain <= 1.0,
            "PhaseTracker: gain must be in (0, 1]"
        );
        assert!(
            cfg.max_step_hz > 0.0,
            "PhaseTracker: max_step_hz must be > 0"
        );
        assert!(cfg.exit_ticks >= 1, "PhaseTracker: exit_ticks must be >= 1");
        let f_hz = f0_hz.clamp(cfg.band.0, cfg.band.1);
        Self {
            cfg,
            f_hz,
            pinned_ticks: 0,
            band_exit: false,
        }
    }

    /// Current commanded frequency, Hz.
    pub fn freq_hz(&self) -> f64 {
        self.f_hz
    }

    /// Latched band-exit state (see the type-level docs).
    pub fn band_exit(&self) -> bool {
        self.band_exit
    }

    /// Clear the latch and the pinned counter, and restart at `f0_hz`
    /// (clamped into the band). Used after a re-search.
    pub fn reset(&mut self, f0_hz: f64) {
        self.f_hz = f0_hz.clamp(self.cfg.band.0, self.cfg.band.1);
        self.pinned_ticks = 0;
        self.band_exit = false;
    }

    /// Feed one measured phase (radians); returns the new commanded frequency.
    /// A NaN measurement is ignored (frequency held, no pinned-tick change).
    pub fn update(&mut self, phase_meas_rad: f64) -> f64 {
        if phase_meas_rad.is_nan() {
            return self.f_hz;
        }
        // Wrap-aware phase error: shortest angular distance, in (-pi, pi].
        let mut err = (phase_meas_rad - self.cfg.phase_setpoint_rad).rem_euclid(TAU);
        if err > PI {
            err -= TAU;
        }
        // Damped Newton step toward the setpoint (see type-level docs for the
        // sign derivation), magnitude-limited per update.
        let step = (-self.cfg.gain * err / self.cfg.slope_rad_per_hz)
            .clamp(-self.cfg.max_step_hz, self.cfg.max_step_hz);
        let unclamped = self.f_hz + step;
        self.f_hz = unclamped.clamp(self.cfg.band.0, self.cfg.band.1);

        let pinned = unclamped < self.cfg.band.0 || unclamped > self.cfg.band.1;
        if pinned {
            self.pinned_ticks = self.pinned_ticks.saturating_add(1);
            if self.pinned_ticks >= self.cfg.exit_ticks {
                self.band_exit = true;
            }
        } else {
            self.pinned_ticks = 0;
        }
        self.f_hz
    }
}
