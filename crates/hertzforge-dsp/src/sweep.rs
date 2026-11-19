//! Phase-continuous numerically-controlled oscillator (NCO) + sweep laws.
//!
//! This is the host-side sine generator for closed-loop sine control (the
//! delegated path owns its own oscillator on-device; the host path built
//! here needs one of its own).
//!
//! The load-bearing property is PHASE CONTINUITY: the oscillator's phase
//! accumulator is only ever ADVANCED, never reset or recomputed from a
//! frequency law, so any frequency move (sweep step, direction reversal at a
//! band edge, operator jog, resonance-dwell PLL correction) changes the phase
//! *increment* from the next sample on but never the phase itself. The output
//! waveform therefore has no discontinuity - only its instantaneous frequency
//! changes.
//!
//! Sweep *laws* (log-octave and linear) are pure functions of elapsed time via
//! [`SweepRate::freq_at`]; the caller evaluates the law per control tick (or
//! per sample) and feeds the frequency into [`SweepOscillator::set_frequency`].
//! Keeping law and oscillator separate is what makes bidirectional sweeps and
//! mid-run rate changes trivial: the law may jump arbitrarily, the phase
//! accumulator cannot.

use std::f64::consts::TAU;

/// Sweep-rate law: how the commanded frequency evolves from a start frequency
/// over elapsed time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SweepRate {
    /// Logarithmic sweep in octaves per minute (the standard sine-test law).
    /// Negative values sweep downward.
    LogOctPerMin(f64),
    /// Linear sweep in Hz per second. Negative values sweep downward.
    LinearHzPerSec(f64),
}

impl SweepRate {
    /// Frequency after `t_s` seconds of sweeping from `f0` under this law.
    ///
    /// - Log law: `f(t) = f0 * 2^(R * t / 60)` with `R` in oct/min.
    /// - Linear law: `f(t) = f0 + rate * t` with `rate` in Hz/s.
    ///
    /// The linear law can go non-positive for a long enough downward sweep;
    /// the caller owns band clamping (this is the raw law).
    pub fn freq_at(&self, f0: f64, t_s: f64) -> f64 {
        match *self {
            SweepRate::LogOctPerMin(r) => f0 * (r * t_s / 60.0).exp2(),
            SweepRate::LinearHzPerSec(rate) => f0 + rate * t_s,
        }
    }
}

/// Signed octave distance from `f1` to `f2`: `log2(f2 / f1)`.
///
/// Positive when `f2 > f1`. Both frequencies must be positive; NaN is returned
/// for a non-positive input (log of a non-positive ratio), which callers should
/// treat as a config error upstream.
pub fn octaves_between(f1: f64, f2: f64) -> f64 {
    (f2 / f1).log2()
}

/// Time in seconds to traverse from `f1` to `f2` at `rate` (using the
/// magnitudes of the distance and the rate; direction is the caller's concern).
///
/// Returns `f64::INFINITY` when the rate is zero (or NaN rate/frequencies
/// propagate as NaN).
pub fn sweep_seconds(f1: f64, f2: f64, rate: SweepRate) -> f64 {
    match rate {
        SweepRate::LogOctPerMin(r) => {
            if r == 0.0 {
                return f64::INFINITY;
            }
            60.0 * octaves_between(f1, f2).abs() / r.abs()
        }
        SweepRate::LinearHzPerSec(rate_hz_s) => {
            if rate_hz_s == 0.0 {
                return f64::INFINITY;
            }
            (f2 - f1).abs() / rate_hz_s.abs()
        }
    }
}

/// Why a frequency was refused by [`SweepOscillator::try_set_frequency`].
///
/// The oscillator's domain rule is `0 <= f < fs/2`: 0 Hz is a parked oscillator and
/// Nyquist and above is not representable at this rate. Outside that, there is no
/// meaningful sample to generate.
///
/// ## Why this type exists
///
/// [`SweepOscillator::set_frequency`] ASSERTS the domain rule, i.e. it panics. That is
/// correct for a caller that derived the value from a validated config and would rather
/// crash than emit a fold-back tone. It is the wrong shape for a value that arrived from
/// an operator over a wire: a panic on the RT tick takes the drive path down with the
/// task still clocked, and guarding every caller is exactly the open-ended arrangement
/// that is better replaced with an unreachable hazardous path. Wire-fed callers use
/// [`SweepOscillator::try_set_frequency`] and never name the panicking form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrequencyOutOfRange {
    /// The frequency that was refused, Hz.
    pub requested_hz: f64,
    /// The oscillator's sample rate, Hz. The admissible range is `[0, fs/2)`.
    pub fs_hz: f64,
}

impl std::fmt::Display for FrequencyOutOfRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frequency {} Hz is outside the oscillator's [0, {}) Hz range at fs = {} Hz",
            self.requested_hz,
            self.fs_hz / 2.0,
            self.fs_hz
        )
    }
}

impl std::error::Error for FrequencyOutOfRange {}

/// Phase-continuous sine NCO.
///
/// Phase accumulates by `2*pi*f/fs` per sample and wraps into `[0, 2*pi)` via
/// a continuous modulo (`rem_euclid`), which subtracts an exact multiple of
/// `2*pi` and therefore introduces no waveform discontinuity.
/// [`SweepOscillator::set_frequency`] NEVER resets the phase - that is the
/// contract that keeps sweep reversals and rate changes click-free.
///
/// State-accessor convention: [`SweepOscillator::phase`] and
/// [`SweepOscillator::cos_sin`] report the phase that the NEXT
/// [`SweepOscillator::next`] call will emit (i.e. the pending sample's phase).
/// `next()` itself returns the phase it actually used, which is what
/// demodulators (`crate::tracking::ThdBank`, lock-ins) should consume so
/// generator and demodulator share the exact same reference phase.
#[derive(Clone, Debug)]
pub struct SweepOscillator {
    fs_hz: f64,
    f_hz: f64,
    /// Phase of the next sample to be generated, in `[0, 2*pi)`.
    phase: f64,
}

impl SweepOscillator {
    /// Build an oscillator at sample rate `fs_hz` starting at `f0_hz`, phase 0.
    ///
    /// Panics unless `fs_hz > 0` and `f0_hz` is in `[0, fs/2)` (0 Hz is allowed
    /// as a "parked" oscillator; Nyquist and above is not representable).
    pub fn new(fs_hz: f64, f0_hz: f64) -> Self {
        assert!(
            fs_hz > 0.0 && !fs_hz.is_nan(),
            "SweepOscillator: fs_hz must be positive"
        );
        assert!(
            f0_hz >= 0.0 && f0_hz < fs_hz / 2.0,
            "SweepOscillator: f0_hz must be in [0, fs/2)"
        );
        Self {
            fs_hz,
            f_hz: f0_hz,
            phase: 0.0,
        }
    }

    /// Change the instantaneous frequency. PHASE-CONTINUOUS: the phase
    /// accumulator is untouched; only the per-sample increment changes from
    /// the next [`SweepOscillator::next`] call on.
    ///
    /// Panics on the same domain rule as [`SweepOscillator::new`]. **A caller whose
    /// value came from an operator, a wire, or any other open-ended source must use
    /// [`SweepOscillator::try_set_frequency`] instead** - see [`FrequencyOutOfRange`]
    /// for why the panicking form is not merely inconvenient there.
    pub fn set_frequency(&mut self, f_hz: f64) {
        assert!(
            self.try_set_frequency(f_hz).is_ok(),
            "SweepOscillator: f_hz must be in [0, fs/2)"
        );
    }

    /// Change the instantaneous frequency, REFUSING an out-of-domain value instead of
    /// panicking. Same phase-continuity contract as [`SweepOscillator::set_frequency`];
    /// on a refusal the oscillator is left exactly as it was, still generating its
    /// previous frequency, so a bad intent degrades to "the tone did not move" rather
    /// than to an unwound RT thread with the drive task still clocked.
    ///
    /// A NaN is refused (it fails both comparisons), which is the point: `set_frequency`
    /// would panic on it and a bare assignment would poison the phase accumulator for
    /// the rest of the run.
    pub fn try_set_frequency(&mut self, f_hz: f64) -> Result<(), FrequencyOutOfRange> {
        if f_hz >= 0.0 && f_hz < self.fs_hz / 2.0 {
            self.f_hz = f_hz;
            Ok(())
        } else {
            Err(FrequencyOutOfRange {
                requested_hz: f_hz,
                fs_hz: self.fs_hz,
            })
        }
    }

    /// The sample rate this oscillator generates at.
    pub fn sample_rate_hz(&self) -> f64 {
        self.fs_hz
    }

    /// Current instantaneous frequency in Hz.
    pub fn frequency(&self) -> f64 {
        self.f_hz
    }

    /// Phase (radians, `[0, 2*pi)`) that the next sample will use.
    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// `(cos(theta), sin(theta))` at the phase the next sample will use -
    /// the quadrature reference pair for demodulators that want to peek at the
    /// oscillator state without advancing it.
    pub fn cos_sin(&self) -> (f64, f64) {
        (self.phase.cos(), self.phase.sin())
    }

    /// Generate one sample. Returns `(sin(theta), inst_freq_hz, theta)` where
    /// `theta` is the phase THIS sample was generated at; the accumulator then
    /// advances by `2*pi*f/fs` and wraps continuously into `[0, 2*pi)`.
    ///
    /// (Deliberately named `next`; this is a sample generator, not an
    /// `Iterator` - it never terminates and returns a tuple, so the trait would
    /// fit poorly.)
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> (f64, f64, f64) {
        let theta = self.phase;
        let sample = theta.sin();
        self.phase = (self.phase + TAU * self.f_hz / self.fs_hz).rem_euclid(TAU);
        (sample, self.f_hz, theta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_set_frequency_accepts_the_whole_legal_domain() {
        let mut osc = SweepOscillator::new(1000.0, 100.0);
        for f in [0.0, 1e-9, 100.0, 499.0, 499.999_999] {
            assert!(osc.try_set_frequency(f).is_ok(), "{f} Hz should be legal");
            assert_eq!(osc.frequency(), f);
        }
    }

    /// The refusal LEAVES THE OSCILLATOR ALONE. That is the property that makes the
    /// non-panicking form safe on the RT tick: a bad wire intent degrades to "the tone
    /// did not move", never to a poisoned phase accumulator.
    #[test]
    fn try_set_frequency_refuses_out_of_domain_and_holds_the_previous_tone() {
        let mut osc = SweepOscillator::new(1000.0, 100.0);
        for f in [-1.0, 500.0, 501.0, f64::NAN, f64::INFINITY] {
            let err = osc
                .try_set_frequency(f)
                .expect_err("{f} Hz must be refused");
            assert_eq!(err.fs_hz, 1000.0);
            assert_eq!(osc.frequency(), 100.0, "refusal must not move the tone");
        }
    }

    /// The panicking form is the same rule, so the two cannot drift apart.
    #[test]
    #[should_panic(expected = "f_hz must be in [0, fs/2)")]
    fn set_frequency_still_panics_out_of_domain() {
        let mut osc = SweepOscillator::new(1000.0, 100.0);
        osc.set_frequency(600.0);
    }

    /// Phase continuity across a retune, stated as the property the host control core
    /// depends on: the phase the FIRST sample after a retune is generated at differs
    /// from the phase the sample before it was generated at by exactly one step of the
    /// OLD frequency - the retune itself contributes nothing.
    #[test]
    fn retune_is_phase_continuous_to_one_old_frequency_step() {
        let fs = 8000.0;
        let f_old = 100.0;
        let mut osc = SweepOscillator::new(fs, f_old);
        for _ in 0..37 {
            osc.next();
        }
        let (_, _, theta_before) = osc.next();
        osc.try_set_frequency(400.0).unwrap();
        let (_, _, theta_after) = osc.next();
        let step = (theta_after - theta_before).rem_euclid(TAU);
        let expected = TAU * f_old / fs;
        assert!(
            (step - expected).abs() < 1e-12,
            "phase step across a retune was {step}, expected the OLD frequency's {expected}"
        );
    }
}
