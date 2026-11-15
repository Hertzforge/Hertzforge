//! The jointed modal plant: a cross-coupled modal structure whose resonances move and
//! whose damping rises with amplitude, seen through a delay per output. A mode may
//! soften (its frequency falls with amplitude, as a bolted joint does), harden (its
//! frequency rises, as a stiffening spring does), or both.
//!
//! # The model
//!
//! Each mode `m` is a unit-mass oscillator in its modal displacement `q_m`, forced by
//! the inner product of its input shape with the drive frame `d` (after the common
//! transport delay):
//!
//! ```text
//!   q_m'' + 2 zeta_m w_m q_m' + w_m^2 q_m = u_m = input_shape_m . d
//!   a_m   = q_m''                                   the mode's acceleration
//!   y_i   = sum_m output_shape_m[i] a_m + sum_j residual[i][j] d_j + feedthrough sum_j d_j
//! ```
//!
//! and output `i` is `y_i` delayed by `output_delays_s[i]` (whole samples plus a
//! fraction, by a four-tap Lagrange interpolator, exact at whole samples and most
//! accurate well below a quarter of the rate: the tests measure each standard plant's
//! delays within a tenth of a sample over the shock band, to 2 kHz at 12 800 Hz), then
//! noise is added. A mode's
//! acceleration response is `s^2 / (s^2 + 2 zeta w s + w^2)` (the
//! [`crate::ModeResponse::Acceleration`] shape); the `residual` is the structure's mass
//! line below its first mode.
//!
//! # The envelope
//!
//! A level-dependent mode keeps a running amplitude envelope `A_m`: a peak follower,
//! decaying by `exp(-1)` in `decay_cycles` periods of the mode's small-signal
//! frequency, of the mode's amplitude `sqrt(q_m^2 + (q_m' / w_m)^2)`. That amplitude is
//! the peak displacement of the mode's present motion (its energy over its stiffness),
//! so it is steady through a cycle of steady motion: a follower of `|q_m|` alone dips
//! between peaks, and the stiffness it then modulates twice a cycle pumps the mode,
//! taking away much of its damping. In a free decay the amplitude falls slower than
//! the follower can, so the envelope tracks it and the frequency glides back as the
//! motion dies. `decay_cycles` is structure-wide: the [`JointLaw`]'s when the plant has
//! one, else three cycles.
//!
//! # The level rule
//!
//! With `r = A_m / reference_amplitude_m`, a mode's frequency and damping each sample
//! follow its own [`LevelRule`] when it has one (`JointedMode::level`):
//!
//! ```text
//!   f_m    = f0_m (1 - drop min(1, r)^drop_exponent + rise min(rise_cap, r)^rise_exponent)
//!   zeta_m = zeta0_m (1 + damping_gain min(damping_cap, r)^damping_exponent)
//!                                                                (at most MAX_DAMPING)
//! ```
//!
//! At the reference amplitude the frequency factor is `1 - drop + rise` (with a
//! `rise_cap` of at least 1; a cap under 1, allowed, stops the rise below it). The drop stops
//! growing at the reference amplitude, as the joint law's does;
//! the rise stops at `rise_cap` times it; the damping stops at `damping_cap` times it
//! (and never passes [`MAX_DAMPING`]). Without that cap a mode driven several times past
//! its reference would take many times its intended damping. A `rise_exponent` of 2 is a cubic stiffness: a Duffing spring's
//! backbone frequency rises with the square of the amplitude while the rise is small.
//!
//! A mode with no rule of its own follows the structure-wide [`JointLaw`]
//!
//! ```text
//!   f_m    = f0_m (1 - delta min(1, r)^freq_exponent)
//!   zeta_m = zeta0_m (1 + damping_gain r^damping_exponent)       (at most MAX_DAMPING)
//! ```
//!
//! exactly as before modes could carry their own (the same arithmetic, so the same
//! samples to the bit), and a mode with neither is linear.
//!
//! # The highest frequency
//!
//! A mode can reach at most `f0 (1 + rise rise_cap^rise_exponent)`
//! ([`LevelRule::max_frequency_factor`], [`JointedPlant::highest_frequencies_hz`]). The
//! step below is exact at any frequency, so it cannot go unstable however high the
//! frequency goes; but a mode at or above the Nyquist frequency is sampled as its alias
//! and is no longer the mode it was stated to be. So [`JointedPlant::validate`] refuses
//! a plant one of whose modes can reach the Nyquist frequency, `fs / 2`, and says what
//! the bound is ([`JointedPlant::frequency_bound_hz`]).
//!
//! # The step, and why it is stable at any amplitude
//!
//! Each sample advances every mode by the exact solution of its equation over one
//! sample period with that sample's frequency and damping held and the modal force
//! varying linearly between the previous sample and this one (a triangle hold, which
//! adds no phase). With the parameters held, that step is the true motion of a damped
//! oscillator over one period, which never gains energy, for any frequency and any
//! damping up to [`MAX_DAMPING`]; nothing in it can overflow the way an explicit
//! integrator does past its step limit. The parameters move only through the envelope.
//! A softening mode's frequency falls as the envelope rises (which takes energy out) and
//! rises back no faster than the envelope's decay, several periods. A hardening mode's
//! frequency rises with the envelope, which puts energy in, but only while the envelope
//! climbs toward its cap: the mode's energy at a given amplitude grows at most by the
//! square of `max_frequency_factor`, and the frequency falls back, taking energy out,
//! no faster than the envelope decays. Neither slow modulation can pump the mode: the
//! envelope is steady through a cycle of steady motion. The tests drive every standard
//! plant on every drive with random, repeated pulses, and sines at its first mode's shifted resonance and at twice it, at
//! the full drive range (10 V) and at a hundred thousand times it, and check the
//! response against the bound of its small-signal modes.
//!
//! # The force between samples, and the equalized step
//!
//! The step above is exact for a modal force that goes linearly from one sample to the
//! next. A drive given as samples of a band-limited signal is not that force: the
//! linear interpolant of the samples has the samples' spectrum times
//! `sinc^2(w dt / 2)`, so with [`JointedStep::TriangleHold`] (the default, and the only
//! step before [`JointedStep::Equalized`] existed) every mode is driven a little less
//! than the continuous model says, more so toward Nyquist: at 8192 Hz, about 0.4 dB at
//! 1 kHz and 1.6 dB at 1.9 kHz on a mode's resonance. The direct part of a mode's
//! acceleration (`u` in `q'' = u - 2 zeta w q' - w^2 q`) is not held, so the departure
//! is that factor on the restoring part alone.
//!
//! [`JointedStep::Equalized`] puts a short linear-phase FIR on each drive ahead of the
//! modal force, fitted to `1 / sinc^2` up to `band_hz` (and held so the low-frequency
//! cancellation of a mode's acceleration below its resonance is kept), and delays the
//! direct part, the residual and the feedthrough by the FIR's half length to match. The
//! small-signal response is then the continuous model's times a pure delay of
//! `half_taps` samples on every path ([`JointedPlant::step_delay_samples`]), within
//! about 0.1 dB to `band_hz`; what is left is the images the hold folds back, which an
//! equalizer cannot remove. The level rule, the envelope and the step itself are
//! unchanged, and the FRFs below include the FIR and its delay exactly.
//!
//! # What its FRF means
//!
//! A level-dependent plant has no single FRF. [`crate::PlantModel::analytic_frf`] of a
//! jointed plant is its **small-signal** FRF (every envelope zero, every mode at its
//! small-signal frequency and damping): what an identification at a low level should
//! find. [`JointedPlant::frf_at_envelope`] gives the FRF with every mode's frequency and
//! damping frozen at a stated envelope, the plant an identification at that level is
//! compared with. Both are exact for the discrete plant the backend steps (a frozen
//! step is linear and time-invariant), the output delays included.

use crate::plant::{fft_exponent_sign, inverse_dft};
use crate::{DelayLine, Xorshift64};
use hertzforge_dsp::fft::{self, Cplx};
use hertzforge_sdm::{MatrixStack, C64};
use std::f64::consts::TAU;

/// The largest damping ratio a jointed mode takes, however far past its reference
/// amplitude it is driven. It keeps every mode underdamped.
pub const MAX_DAMPING: f64 = 0.9;

/// The longest output delay a jointed plant takes, seconds.
pub const MAX_OUTPUT_DELAY_S: f64 = 0.1;

/// How a jointed plant's modes change with amplitude (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointLaw {
    /// The fractional frequency drop at and above the reference amplitude, `0 <= delta < 1`.
    pub delta: f64,
    /// The damping at the reference amplitude is `1 + damping_gain` times the
    /// small-signal damping (1 doubles it, 0 leaves it).
    pub damping_gain: f64,
    /// The exponent on the amplitude ratio in the frequency drop.
    pub freq_exponent: f64,
    /// The exponent on the amplitude ratio in the damping rise.
    pub damping_exponent: f64,
    /// The envelope falls by `exp(-1)` in this many periods of the mode's
    /// small-signal frequency.
    pub decay_cycles: f64,
}

impl JointLaw {
    /// A law with both exponents 0.5 (a joint's dissipation per cycle growing as the
    /// force amplitude to a power between 2 and 3) and a three-cycle envelope.
    pub fn new(delta: f64, damping_gain: f64) -> Self {
        JointLaw {
            delta,
            damping_gain,
            freq_exponent: 0.5,
            damping_exponent: 0.5,
            decay_cycles: 3.0,
        }
    }

    /// The frequency over its small-signal value at amplitude ratio `r`.
    pub fn frequency_factor(&self, r: f64) -> f64 {
        if r <= 0.0 {
            1.0
        } else {
            1.0 - self.delta * r.min(1.0).powf(self.freq_exponent)
        }
    }

    /// The damping ratio at amplitude ratio `r` of a mode whose small-signal damping
    /// is `zeta0`.
    pub fn damping(&self, zeta0: f64, r: f64) -> f64 {
        if r <= 0.0 {
            zeta0
        } else {
            (zeta0 * (1.0 + self.damping_gain * r.powf(self.damping_exponent))).min(MAX_DAMPING)
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !(self.delta.is_finite() && (0.0..1.0).contains(&self.delta)) {
            return Err(format!(
                "a joint's frequency drop {} is not in [0, 1)",
                self.delta
            ));
        }
        if !(self.damping_gain.is_finite() && self.damping_gain >= 0.0) {
            return Err(format!(
                "a joint's damping gain {} is negative or not finite",
                self.damping_gain
            ));
        }
        for (what, v) in [
            ("frequency exponent", self.freq_exponent),
            ("damping exponent", self.damping_exponent),
            ("envelope decay", self.decay_cycles),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("a joint's {what} {v} is not positive"));
            }
        }
        Ok(())
    }
}

/// How one mode changes with its amplitude: a drop term (softening) and a rise term
/// (hardening) on its frequency, and a damping rise (see the module docs). The field
/// names are the level rule's contract with the case and structure files.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelRule {
    /// The fractional frequency drop at and above the reference amplitude, `0 <= drop < 1`.
    pub drop: f64,
    /// The exponent on the amplitude ratio in the drop, above zero.
    pub drop_exponent: f64,
    /// The fractional frequency rise at the reference amplitude, at least zero.
    pub rise: f64,
    /// The exponent on the amplitude ratio in the rise, above zero (2 for a cubic
    /// stiffness).
    pub rise_exponent: f64,
    /// The amplitude ratio above which the rise stops growing, above zero and finite. It
    /// may be under 1: the reference amplitude is shared with the drop, and full level
    /// can sit below it.
    pub rise_cap: f64,
    /// The damping at the reference amplitude is `1 + damping_gain` times the
    /// small-signal damping, at least zero.
    pub damping_gain: f64,
    /// The exponent on the amplitude ratio in the damping rise, above zero.
    pub damping_exponent: f64,
    /// The amplitude ratio above which the damping stops growing, above zero and
    /// finite (it may be under 1). The structure-wide [`JointLaw`] has no such cap.
    pub damping_cap: f64,
}

impl LevelRule {
    /// A rule with no drop, no rise and no damping change: the mode is linear. Set the
    /// terms wanted on top of it.
    pub fn linear() -> Self {
        LevelRule {
            drop: 0.0,
            drop_exponent: 1.0,
            rise: 0.0,
            rise_exponent: 2.0,
            rise_cap: 1.0,
            damping_gain: 0.0,
            damping_exponent: 1.0,
            damping_cap: 1.0,
        }
    }

    /// A hardening rule: frequency up by `rise` at the reference amplitude as its
    /// square (a cubic stiffness), growing to `rise_cap` times the reference amplitude,
    /// no drop and no damping change.
    pub fn hardening(rise: f64, rise_cap: f64) -> Self {
        LevelRule {
            rise,
            rise_cap,
            ..LevelRule::linear()
        }
    }

    /// The rule a [`JointLaw`] gives a mode: its drop and damping, no rise, and a
    /// damping cap of `f64::MAX` (the law has none, and `min(f64::MAX, r)` is `r` for every
    /// finite `r`, so the numbers are the law's to the bit). The plant steps a mode that
    /// follows the law by the law itself; this is for comparison.
    pub fn from_joint(j: &JointLaw) -> Self {
        LevelRule {
            drop: j.delta,
            drop_exponent: j.freq_exponent,
            damping_gain: j.damping_gain,
            damping_exponent: j.damping_exponent,
            damping_cap: f64::MAX,
            ..LevelRule::linear()
        }
    }

    /// The frequency over its small-signal value at amplitude ratio `r`.
    pub fn frequency_factor(&self, r: f64) -> f64 {
        if r <= 0.0 {
            1.0
        } else {
            1.0 - self.drop * r.min(1.0).powf(self.drop_exponent)
                + self.rise * r.min(self.rise_cap).powf(self.rise_exponent)
        }
    }

    /// The damping ratio at amplitude ratio `r` of a mode whose small-signal damping
    /// is `zeta0`: `zeta0 (1 + damping_gain min(damping_cap, r)^damping_exponent)`, at
    /// most [`MAX_DAMPING`]. A rule with no damping gain leaves it at `zeta0` at any
    /// amplitude.
    pub fn damping(&self, zeta0: f64, r: f64) -> f64 {
        if r <= 0.0 || self.damping_gain == 0.0 {
            zeta0
        } else {
            (zeta0
                * (1.0 + self.damping_gain * r.min(self.damping_cap).powf(self.damping_exponent)))
            .min(MAX_DAMPING)
        }
    }

    /// The largest frequency factor the rule can give, at any amplitude:
    /// `1 + rise rise_cap^rise_exponent` (the drop only lowers it).
    pub fn max_frequency_factor(&self) -> f64 {
        1.0 + self.rise * self.rise_cap.powf(self.rise_exponent)
    }

    /// Check every field; the message names the field and its allowed range.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.drop.is_finite() && (0.0..1.0).contains(&self.drop)) {
            return Err(format!("drop {} is not in [0, 1)", self.drop));
        }
        if !(self.rise.is_finite() && self.rise >= 0.0) {
            return Err(format!("rise {} is negative or not finite", self.rise));
        }
        if !(self.rise_cap.is_finite() && self.rise_cap > 0.0) {
            return Err(format!(
                "rise_cap {} is not positive and finite",
                self.rise_cap
            ));
        }
        if !(self.damping_gain.is_finite() && self.damping_gain >= 0.0) {
            return Err(format!(
                "damping_gain {} is negative or not finite",
                self.damping_gain
            ));
        }
        if !(self.damping_cap.is_finite() && self.damping_cap > 0.0) {
            return Err(format!(
                "damping_cap {} is not positive and finite",
                self.damping_cap
            ));
        }
        for (what, v) in [
            ("drop_exponent", self.drop_exponent),
            ("rise_exponent", self.rise_exponent),
            ("damping_exponent", self.damping_exponent),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{what} {v} is not positive and finite"));
            }
        }
        if !self.max_frequency_factor().is_finite() {
            return Err(format!(
                "rise {} with rise_cap {} and rise_exponent {} gives no finite highest frequency",
                self.rise, self.rise_cap, self.rise_exponent
            ));
        }
        Ok(())
    }
}

/// How a jointed plant turns the sampled drive into its modes' force (see the module
/// docs).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum JointedStep {
    /// The force goes linearly between samples, as before the equalized step existed.
    /// Earlier results were made with this; it stays the default.
    #[default]
    TriangleHold,
    /// The force is the drive through a `2 half_taps + 1` tap FIR fitted to undo the
    /// hold up to `band_hz`; every path is delayed `half_taps` samples.
    Equalized {
        /// The top of the band the FIR is fitted over, Hz (under the Nyquist frequency).
        band_hz: f64,
        /// Half the FIR's length less one, 1 to 16 (2 holds 0.1 dB to 2 kHz at 8192 Hz).
        half_taps: usize,
    },
}

impl JointedStep {
    /// The equalized step a benchmark structure uses: fitted to `band_hz`, five taps.
    pub fn equalized(band_hz: f64) -> Self {
        JointedStep::Equalized {
            band_hz,
            half_taps: 2,
        }
    }

    fn validate(&self, fs_hz: f64) -> Result<(), String> {
        match *self {
            JointedStep::TriangleHold => Ok(()),
            JointedStep::Equalized { band_hz, half_taps } => {
                if !(band_hz.is_finite() && band_hz > 0.0 && band_hz < 0.5 * fs_hz) {
                    return Err(format!(
                        "the equalized step's band {band_hz} Hz is not between 0 and the \
                         Nyquist frequency of {fs_hz} Hz"
                    ));
                }
                if !(1..=16).contains(&half_taps) {
                    return Err(format!(
                        "the equalized step's half_taps {half_taps} is not between 1 and 16"
                    ));
                }
                Ok(())
            }
        }
    }

    /// The FIR taps (`2 half_taps + 1`, symmetric) at `fs_hz`, or `None` for the hold.
    fn taps(&self, fs_hz: f64) -> Option<Vec<f64>> {
        match *self {
            JointedStep::TriangleHold => None,
            JointedStep::Equalized { band_hz, half_taps } => {
                Some(equalizer_taps(band_hz / fs_hz, half_taps))
            }
        }
    }
}

/// The symmetric FIR `c[0..=2D]` (centred at `D`) whose zero-phase response
/// `G(w) = h0 + 2 sum h_k cos(k w)` makes `G(w) sinc^2(w / 2)` one: least squares, in
/// relative error weighted by `1 / w^2` up to `band` (cycles per sample) so the
/// cancellation of a mode's acceleration below its resonance is held, and by 0.01 above.
/// Solved by Householder QR (the columns are nearly parallel at low frequency).
#[allow(clippy::needless_range_loop)] // Householder QR reads as the textbook does.
fn equalizer_taps(band: f64, d: usize) -> Vec<f64> {
    let n_cols = d + 1;
    let wb = TAU * band;
    let (n_in, n_out) = (2000usize, 400usize);
    let mut a: Vec<Vec<f64>> = Vec::with_capacity(n_in + n_out);
    let mut b: Vec<f64> = Vec::with_capacity(n_in + n_out);
    for k in 0..n_in + n_out {
        let (w, wt) = if k < n_in {
            let w = wb * (k + 1) as f64 / n_in as f64;
            (w, 1.0 / (w * w))
        } else {
            let w = wb + (std::f64::consts::PI - wb) * (k - n_in + 1) as f64 / n_out as f64;
            (w, 0.01)
        };
        let x = 0.5 * w;
        let s = (x.sin() / x).powi(2);
        let mut row = vec![s * wt];
        row.extend((1..=d).map(|j| 2.0 * (j as f64 * w).cos() * s * wt));
        a.push(row);
        b.push(wt);
    }
    // Householder QR of a (rows x n_cols), applied to b as well.
    let rows = a.len();
    for c in 0..n_cols {
        let norm = (c..rows).map(|r| a[r][c] * a[r][c]).sum::<f64>().sqrt();
        let alpha = if a[c][c] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (c..rows).map(|r| a[r][c]).collect();
        v[0] -= alpha;
        let vn = v.iter().map(|x| x * x).sum::<f64>();
        if vn == 0.0 {
            continue;
        }
        for cc in c..n_cols {
            let dot: f64 = (c..rows).zip(&v).map(|(r, vi)| a[r][cc] * vi).sum();
            let f = 2.0 * dot / vn;
            for (r, vi) in (c..rows).zip(&v) {
                a[r][cc] -= f * vi;
            }
        }
        let dot: f64 = (c..rows).zip(&v).map(|(r, vi)| b[r] * vi).sum();
        let f = 2.0 * dot / vn;
        for (r, vi) in (c..rows).zip(&v) {
            b[r] -= f * vi;
        }
    }
    let mut h = vec![0.0; n_cols];
    for c in (0..n_cols).rev() {
        let s: f64 = (c + 1..n_cols).map(|k| a[c][k] * h[k]).sum();
        h[c] = (b[c] - s) / a[c][c];
    }
    // Taps k = 0..=2d, centred at d.
    (0..=2 * d).map(|k| h[k.abs_diff(d)]).collect()
}

/// The rule a mode follows: its own, the structure's joint law, or none.
#[derive(Debug, Clone, Copy)]
enum ModeRule {
    Linear,
    Joint(JointLaw),
    Level(LevelRule),
}

impl ModeRule {
    fn of(m: &JointedMode, joint: Option<JointLaw>) -> Self {
        match (m.level, joint) {
            (Some(l), _) => ModeRule::Level(l),
            (None, Some(j)) => ModeRule::Joint(j),
            (None, None) => ModeRule::Linear,
        }
    }

    /// Angular frequency and damping at amplitude ratio `r` of a mode at `w0`, `zeta0`.
    #[inline]
    fn params(&self, w0: f64, zeta0: f64, r: f64) -> (f64, f64) {
        match self {
            ModeRule::Linear => (w0, zeta0),
            ModeRule::Joint(j) => (w0 * j.frequency_factor(r), j.damping(zeta0, r)),
            ModeRule::Level(l) => (w0 * l.frequency_factor(r), l.damping(zeta0, r)),
        }
    }
}

/// One mode of a [`JointedPlant`].
#[derive(Debug, Clone, PartialEq)]
pub struct JointedMode {
    /// Small-signal natural frequency, Hz (below the Nyquist frequency).
    pub f_hz: f64,
    /// Small-signal damping ratio, above zero and under [`MAX_DAMPING`].
    pub zeta: f64,
    /// Participation of each drive, `n_drives` long.
    pub input_shape: Vec<f64>,
    /// How each output sees the mode's acceleration, `n_inputs` long.
    pub output_shape: Vec<f64>,
    /// The modal displacement amplitude at which the joint law reaches its full drop
    /// (and a level rule its stated drop and rise).
    pub reference_amplitude: f64,
    /// The mode's own level rule, or `None` to follow the plant's joint law (linear
    /// when the plant has none).
    pub level: Option<LevelRule>,
}

/// A cross-coupled modal plant with level-dependent modes and a delay per output.
/// See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct JointedPlant {
    /// Outputs (measurement inputs; rows of the FRF).
    pub n_inputs: usize,
    /// Drives (columns of the FRF).
    pub n_drives: usize,
    pub modes: Vec<JointedMode>,
    /// The structure-wide amplitude law for every mode without a level rule of its
    /// own, or `None` (those modes are then linear). Its `decay_cycles` sets every
    /// mode's envelope.
    pub joint: Option<JointLaw>,
    /// Added to every FRF element.
    pub feedthrough: f64,
    /// Row-major `n_inputs x n_drives`, or empty for none: the mass line.
    pub residual: Vec<f64>,
    /// Whole samples on every drive before the structure, common to every path.
    pub transport_delay_samples: usize,
    /// Each output's own delay, seconds (whole samples plus a fraction at the
    /// configured rate), or empty for none. At most [`MAX_OUTPUT_DELAY_S`].
    pub output_delays_s: Vec<f64>,
    /// How the sampled drive becomes the modes' force: the triangle hold (default) or
    /// the equalized step, which matches the continuous model at small signal.
    pub step: JointedStep,
}

/// What [`JointedPlant::simulate`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Simulated {
    /// `n_inputs` records, each as long as the drive records.
    pub outputs: Vec<Vec<f64>>,
    /// Each mode's largest envelope over the run.
    pub peak_envelope: Vec<f64>,
}

impl Simulated {
    /// Each mode's largest envelope over its reference amplitude.
    pub fn peak_ratio(&self, plant: &JointedPlant) -> Vec<f64> {
        self.peak_envelope
            .iter()
            .zip(&plant.modes)
            .map(|(a, m)| a / m.reference_amplitude)
            .collect()
    }
}

impl JointedPlant {
    /// A linear plant with no residual, no feedthrough and no delays.
    pub fn new(n_inputs: usize, n_drives: usize, modes: Vec<JointedMode>) -> Self {
        JointedPlant {
            n_inputs,
            n_drives,
            modes,
            joint: None,
            feedthrough: 0.0,
            residual: Vec::new(),
            transport_delay_samples: 0,
            output_delays_s: Vec::new(),
            step: JointedStep::TriangleHold,
        }
    }

    /// The same plant with no joint law and no level rules: what it is at a vanishing
    /// level.
    pub fn linear(&self) -> JointedPlant {
        let mut p = JointedPlant {
            joint: None,
            ..self.clone()
        };
        p.modes.iter_mut().for_each(|m| m.level = None);
        p
    }

    /// Whether any mode changes with amplitude.
    pub fn is_level_dependent(&self) -> bool {
        self.joint.is_some() || self.modes.iter().any(|m| m.level.is_some())
    }

    /// The highest frequency each mode can reach at any amplitude, Hz: `f0 (1 + rise
    /// rise_cap^rise_exponent)` under its own rule, `f0` under the joint law or none.
    pub fn highest_frequencies_hz(&self) -> Vec<f64> {
        self.modes
            .iter()
            .map(|m| m.f_hz * m.level.map_or(1.0, |l| l.max_frequency_factor()))
            .collect()
    }

    /// The whole samples the step adds to every path (beyond the transport delay): 0 for
    /// the triangle hold, `half_taps` for the equalized step.
    pub fn step_delay_samples(&self) -> usize {
        match self.step {
            JointedStep::TriangleHold => 0,
            JointedStep::Equalized { half_taps, .. } => half_taps,
        }
    }

    /// The bound every mode's highest frequency must stay under at `fs_hz`: the Nyquist
    /// frequency (see the module docs).
    pub fn frequency_bound_hz(fs_hz: f64) -> f64 {
        0.5 * fs_hz
    }

    /// Mode `k` alone, one drive to one output with unit shapes, its own reference
    /// amplitude, its own level rule and this plant's law, no residual and no delays: the mode as a
    /// single-degree-of-freedom article, for measuring what the law does to it.
    pub fn single_mode(&self, k: usize) -> JointedPlant {
        let m = &self.modes[k];
        JointedPlant {
            joint: self.joint,
            ..JointedPlant::new(
                1,
                1,
                vec![JointedMode {
                    input_shape: vec![1.0],
                    output_shape: vec![1.0],
                    ..m.clone()
                }],
            )
        }
    }

    /// Check the plant can run at `fs_hz`; the message says what is wrong.
    pub fn validate(&self, fs_hz: f64) -> Result<(), String> {
        if self.n_inputs == 0 || self.n_drives == 0 {
            return Err(format!(
                "a jointed plant needs at least one output and one drive, got {} x {}",
                self.n_inputs, self.n_drives
            ));
        }
        if !(self.residual.is_empty() || self.residual.len() == self.n_inputs * self.n_drives) {
            return Err(format!(
                "the residual holds {} values; a {} x {} plant needs {} or none",
                self.residual.len(),
                self.n_inputs,
                self.n_drives,
                self.n_inputs * self.n_drives
            ));
        }
        if !self.feedthrough.is_finite() || self.residual.iter().any(|v| !v.is_finite()) {
            return Err("a jointed plant's feedthrough or residual is not finite".into());
        }
        for (k, m) in self.modes.iter().enumerate() {
            if m.input_shape.len() != self.n_drives || m.output_shape.len() != self.n_inputs {
                return Err(format!(
                    "mode {k} has shapes {} (drives) and {} (outputs); the plant is {} x {}",
                    m.input_shape.len(),
                    m.output_shape.len(),
                    self.n_inputs,
                    self.n_drives
                ));
            }
            if !(m.f_hz.is_finite() && m.f_hz > 0.0 && m.f_hz < 0.5 * fs_hz) {
                return Err(format!(
                    "mode {k} at {} Hz is not between 0 and the Nyquist frequency of {fs_hz} Hz",
                    m.f_hz
                ));
            }
            if !(m.zeta.is_finite() && m.zeta > 0.0 && m.zeta < MAX_DAMPING) {
                return Err(format!(
                    "mode {k} has damping {}; it must be above 0 and under {MAX_DAMPING}",
                    m.zeta
                ));
            }
            if !(m.reference_amplitude.is_finite() && m.reference_amplitude > 0.0) {
                return Err(format!(
                    "mode {k}'s reference amplitude {} is not positive",
                    m.reference_amplitude
                ));
            }
            if m.input_shape
                .iter()
                .chain(&m.output_shape)
                .any(|v| !v.is_finite())
            {
                return Err(format!("mode {k} has a non-finite shape value"));
            }
        }
        if let Some(j) = &self.joint {
            j.validate()?;
        }
        self.step.validate(fs_hz)?;
        let bound = Self::frequency_bound_hz(fs_hz);
        for (k, (m, top)) in self
            .modes
            .iter()
            .zip(self.highest_frequencies_hz())
            .enumerate()
        {
            let Some(l) = &m.level else { continue };
            l.validate()
                .map_err(|e| format!("mode {k}'s level rule: {e}"))?;
            if top >= bound {
                return Err(format!(
                    "mode {k} at {} Hz can rise to {top} Hz (f0 (1 + rise rise_cap^rise_exponent)), \
                     at or above the bound of {bound} Hz (the Nyquist frequency at {fs_hz} Hz); \
                     lower its rise or rise_cap, or raise the sample rate",
                    m.f_hz
                ));
            }
        }
        if !(self.output_delays_s.is_empty() || self.output_delays_s.len() == self.n_inputs) {
            return Err(format!(
                "{} output delays for {} outputs",
                self.output_delays_s.len(),
                self.n_inputs
            ));
        }
        if let Some(t) = self
            .output_delays_s
            .iter()
            .find(|t| !(t.is_finite() && **t >= 0.0 && **t <= MAX_OUTPUT_DELAY_S))
        {
            return Err(format!(
                "an output delay of {t} s is not between 0 and {MAX_OUTPUT_DELAY_S} s"
            ));
        }
        Ok(())
    }

    /// Output `i`'s delay in samples at `fs_hz`.
    pub fn output_delay_samples(&self, i: usize, fs_hz: f64) -> f64 {
        self.output_delays_s.get(i).copied().unwrap_or(0.0) * fs_hz
    }

    /// The FRF with every mode's frequency and damping frozen at envelope
    /// `envelope[m]` (modal displacement), on `freqs_hz`. An all-zero envelope is the
    /// small-signal FRF. Panics when the plant does not validate at `fs_hz` or the
    /// envelope is not one value per mode.
    pub fn frf_at_envelope(&self, freqs_hz: &[f64], fs_hz: f64, envelope: &[f64]) -> MatrixStack {
        if let Err(e) = self.validate(fs_hz) {
            panic!("frf_at_envelope on an invalid jointed plant: {e}");
        }
        assert_eq!(envelope.len(), self.modes.len(), "one envelope per mode");
        let steps: Vec<ModeStep> = self
            .modes
            .iter()
            .zip(envelope)
            .map(|(m, &a)| {
                let (w, z) = self.params(m, a);
                ModeStep::new(w, z, 1.0 / fs_hz)
            })
            .collect();
        let odelays: Vec<FracDelay> = (0..self.n_inputs)
            .map(|i| FracDelay::new(self.output_delay_samples(i, fs_hz)))
            .collect();
        let (n_in, n_d) = (self.n_inputs, self.n_drives);
        let mut out = MatrixStack::zeros(freqs_hz.len(), n_in, n_d);
        let mut rm = vec![C64::new(0.0, 0.0); self.modes.len()];
        let taps = self.step.taps(fs_hz);
        let sd = self.step_delay_samples() as f64;
        for (k, &f) in freqs_hz.iter().enumerate() {
            let om = TAU * f / fs_hz;
            let z = C64::new(om.cos(), om.sin());
            for (r, s) in rm.iter_mut().zip(&steps) {
                *r = s.response(z);
            }
            let mut ph = -om * self.transport_delay_samples as f64;
            if let Some(c) = &taps {
                // The FIR on the modal force, the step delay on the direct part:
                // r' e^{-j w D} = e^{-j w D} - FIR(w) (1 - r).
                let g = c
                    .iter()
                    .enumerate()
                    .fold(C64::new(0.0, 0.0), |acc, (n, &cn)| {
                        let p = -om * n as f64;
                        acc + C64::new(cn * p.cos(), cn * p.sin())
                    });
                // G e^{j w D} is the FIR's zero-phase response; the delay e^{-j w D} on
                // every path goes with the transport delay.
                let g0 = g * C64::new((om * sd).cos(), (om * sd).sin());
                for r in rm.iter_mut() {
                    *r = C64::new(1.0, 0.0) - g0 * (C64::new(1.0, 0.0) - *r);
                }
                ph -= om * sd;
            }
            let dl = C64::new(ph.cos(), ph.sin());
            let line = out.line_mut(k);
            for i in 0..n_in {
                let li = odelays[i].response(om) * dl;
                for j in 0..n_d {
                    let mut h = C64::new(self.feedthrough, 0.0);
                    if !self.residual.is_empty() {
                        h.re += self.residual[i * n_d + j];
                    }
                    for (mode, r) in self.modes.iter().zip(&rm) {
                        h += r * (mode.output_shape[i] * mode.input_shape[j]);
                    }
                    line[i * n_d + j] = h * li;
                }
            }
        }
        out
    }

    /// The FRF with every mode frozen at `ratio` times its reference amplitude. At the
    /// standard plants' calibration, the reference pulse at a level `L` dB below full
    /// brings the dominant mode to about `ratio = 10^(L / 20)`.
    pub fn frf_at_reference_ratio(&self, freqs_hz: &[f64], fs_hz: f64, ratio: f64) -> MatrixStack {
        let env: Vec<f64> = self
            .modes
            .iter()
            .map(|m| ratio * m.reference_amplitude)
            .collect();
        self.frf_at_envelope(freqs_hz, fs_hz, &env)
    }

    /// Each mode's `(frequency Hz, damping ratio)` at envelope `envelope[m]`.
    pub fn modes_at_envelope(&self, envelope: &[f64]) -> Vec<(f64, f64)> {
        self.modes
            .iter()
            .zip(envelope)
            .map(|(m, &a)| {
                let (w, z) = self.params(m, a);
                (w / TAU, z)
            })
            .collect()
    }

    /// The plant at rest, to be stepped one frame at a time with no noise, by the same
    /// step the backend runs.
    pub fn stepper(&self, fs_hz: f64) -> Result<Stepper, String> {
        self.validate(fs_hz)?;
        Ok(Stepper {
            run: JointedRun::new(self, fs_hz, 0.0, 1),
        })
    }

    /// Play `drives` (`n_drives` records of one length) through the plant from rest,
    /// with no noise, by the same step the backend runs: each output's record and each
    /// mode's largest envelope.
    pub fn simulate(&self, fs_hz: f64, drives: &[Vec<f64>]) -> Result<Simulated, String> {
        self.validate(fs_hz)?;
        if drives.len() != self.n_drives {
            return Err(format!(
                "{} drive records for a plant with {} drives",
                drives.len(),
                self.n_drives
            ));
        }
        let n = drives.first().map_or(0, Vec::len);
        if drives.iter().any(|d| d.len() != n) {
            return Err("the drive records differ in length".into());
        }
        let mut run = JointedRun::new(self, fs_hz, 0.0, 1);
        let mut outputs = vec![Vec::with_capacity(n); self.n_inputs];
        let mut frame = vec![0.0; self.n_drives];
        let mut out = vec![0.0; self.n_inputs];
        for t in 0..n {
            for (f, d) in frame.iter_mut().zip(drives) {
                *f = d[t];
            }
            run.step(&frame, &mut out);
            for (o, &v) in outputs.iter_mut().zip(&out) {
                o.push(v);
            }
        }
        Ok(Simulated {
            outputs,
            peak_envelope: run.modes.iter().map(|m| m.peak).collect(),
        })
    }

    /// The drives of least energy that make output `output` of the small-signal plant
    /// play `target` (one period of a periodic record whose length is a power of two),
    /// line by line: `d(f) = h(f)^H r(f) / |h(f)|^2`, `h` that output's row of the
    /// small-signal FRF. The other outputs go where they go. A line where the row
    /// vanishes gets no drive.
    pub fn least_drive_for(
        &self,
        output: usize,
        target: &[f64],
        fs_hz: f64,
    ) -> Result<Vec<Vec<f64>>, String> {
        self.validate(fs_hz)?;
        let n = target.len();
        if n < 2 || !n.is_power_of_two() {
            return Err(format!("a target of {n} samples is not a power of two"));
        }
        if output >= self.n_inputs {
            return Err(format!("output {output} of a plant with {}", self.n_inputs));
        }
        let lines = n / 2 + 1;
        let df = fs_hz / n as f64;
        let freqs: Vec<f64> = (0..lines).map(|k| k as f64 * df).collect();
        let h = self
            .linear()
            .frf_at_envelope(&freqs, fs_hz, &vec![0.0; self.modes.len()]);
        let conj = fft_exponent_sign() > 0.0;
        let spec = fft::fft(&target.iter().map(|&v| (v, 0.0)).collect::<Vec<Cplx>>());
        let mut d: Vec<Vec<Cplx>> = vec![vec![(0.0, 0.0); n]; self.n_drives];
        for k in 0..lines {
            let r = C64::new(spec[k].0, if conj { -spec[k].1 } else { spec[k].1 });
            let row: Vec<C64> = (0..self.n_drives).map(|j| h.get(k, output, j)).collect();
            let norm: f64 = row.iter().map(|v| v.norm_sqr()).sum();
            if norm <= 0.0 {
                continue;
            }
            for (j, hj) in row.iter().enumerate() {
                let mut v = hj.conj() * r / norm;
                if k == 0 || 2 * k == n {
                    v.im = 0.0;
                }
                d[j][k] = (v.re, v.im);
                if k > 0 && 2 * k < n {
                    d[j][n - k] = (v.re, -v.im);
                }
            }
        }
        Ok(d.iter()
            .map(|s| inverse_dft(s).into_iter().map(|c| c.0).collect())
            .collect())
    }

    /// A mode's angular frequency and damping at envelope `a`.
    fn params(&self, m: &JointedMode, a: f64) -> (f64, f64) {
        let w0 = TAU * m.f_hz;
        match ModeRule::of(m, self.joint) {
            ModeRule::Linear => (w0, m.zeta),
            rule => rule.params(w0, m.zeta, a / m.reference_amplitude),
        }
    }
}

/// A jointed plant being stepped by its caller ([`JointedPlant::stepper`]).
#[derive(Debug, Clone)]
pub struct Stepper {
    run: JointedRun,
}

impl Stepper {
    /// Advance one frame: `drive` holds one value per drive, `out` receives one per
    /// output.
    pub fn step(&mut self, drive: &[f64], out: &mut [f64]) {
        self.run.step(drive, out);
    }

    /// Each mode's present envelope.
    pub fn envelope(&self) -> Vec<f64> {
        self.run.modes.iter().map(|m| m.envelope).collect()
    }

    /// Back to rest.
    pub fn reset(&mut self) {
        self.run.reset();
    }
}

// ---------------------------------------------------------------------------------------
// The step.
// ---------------------------------------------------------------------------------------

/// One mode's step over one sample period at a held frequency and damping: the exact
/// solution under a modal force varying linearly across the period.
#[derive(Debug, Clone, Copy)]
struct ModeStep {
    w: f64,
    zeta: f64,
    dt: f64,
    /// The state transition `exp(A dt)`, row-major.
    p: [f64; 4],
}

impl ModeStep {
    fn new(w: f64, zeta: f64, dt: f64) -> Self {
        let sigma = zeta * w;
        let wd = w * (1.0 - zeta * zeta).sqrt();
        let e = (-sigma * dt).exp();
        let (s, c) = (wd * dt).sin_cos();
        ModeStep {
            w,
            zeta,
            dt,
            p: [
                e * (c + sigma / wd * s),
                e * s / wd,
                -e * w * w / wd * s,
                e * (c - sigma / wd * s),
            ],
        }
    }

    /// Advance `(q, v)` from the previous sample to this one under the force going
    /// linearly from `u0` to `u1`; return the acceleration at this sample.
    #[inline]
    fn advance(&self, q: &mut f64, v: &mut f64, u0: f64, u1: f64) -> f64 {
        let w2 = self.w * self.w;
        let k = (u1 - u0) / self.dt;
        // The particular solution under the ramp, at the start of the period.
        let pq = (u0 - 2.0 * self.zeta * k / self.w) / w2;
        let pv = k / w2;
        let (dq, dv) = (*q - pq, *v - pv);
        *q = pq + k * self.dt / w2 + self.p[0] * dq + self.p[1] * dv;
        *v = pv + self.p[2] * dq + self.p[3] * dv;
        u1 - 2.0 * self.zeta * self.w * *v - w2 * *q
    }

    /// The step's acceleration response at `z = exp(j w dt)`, held parameters.
    fn response(&self, z: C64) -> C64 {
        let w2 = self.w * self.w;
        let c = 2.0 * self.zeta / (self.w * self.dt);
        let tw = 1.0 / (self.dt * w2);
        // x[n] = P x[n-1] + G0 u[n-1] + G1 u[n], from the ramp's particular solution.
        let (a0, a1) = ([c / w2, -tw], [(1.0 - c) / w2, tw]);
        let (b0, b1) = ([(1.0 + c) / w2, -tw], [-c / w2, tw]);
        let p = &self.p;
        let mul = |b: [f64; 2]| [p[0] * b[0] + p[1] * b[1], p[2] * b[0] + p[3] * b[1]];
        let (pb0, pb1) = (mul(b0), mul(b1));
        let g0 = [a0[0] - pb0[0], a0[1] - pb0[1]];
        let g1 = [a1[0] - pb1[0], a1[1] - pb1[1]];
        let g = [z * g1[0] + g0[0], z * g1[1] + g0[1]];
        let m11 = z - p[0];
        let m22 = z - p[3];
        let det = m11 * m22 - p[1] * p[2];
        let xq = (m22 * g[0] + g[1] * p[1]) / det;
        let xv = (g[0] * p[2] + m11 * g[1]) / det;
        C64::new(1.0, 0.0) - xq * w2 - xv * (2.0 * self.zeta * self.w)
    }
}

/// A delay of whole samples plus a fraction: `n0` whole samples, then a four-tap
/// Lagrange interpolator at fraction `d` (in `[1, 2)` for a delay of a sample or more,
/// where the interpolator is most accurate, else the delay itself).
#[derive(Debug, Clone)]
struct FracDelay {
    buf: Vec<f64>,
    pos: usize,
    n0: usize,
    h: [f64; 4],
}

impl FracDelay {
    fn new(samples: f64) -> Self {
        let (n0, d) = if samples < 1.0 {
            (0usize, samples)
        } else {
            let n0 = samples.floor() as usize - 1;
            (n0, samples - n0 as f64)
        };
        let mut h = [0.0; 4];
        for (k, hk) in h.iter_mut().enumerate() {
            let mut v = 1.0;
            for m in 0..4 {
                if m != k {
                    v *= (d - m as f64) / (k as f64 - m as f64);
                }
            }
            *hk = v;
        }
        FracDelay {
            buf: vec![0.0; n0 + 4],
            pos: 0,
            n0,
            h,
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let len = self.buf.len();
        self.buf[self.pos] = x;
        let mut y = 0.0;
        for (k, &hk) in self.h.iter().enumerate() {
            y += hk * self.buf[(self.pos + len - self.n0 - k) % len];
        }
        self.pos = (self.pos + 1) % len;
        y
    }

    fn response(&self, om: f64) -> C64 {
        let mut acc = C64::new(0.0, 0.0);
        for (k, &hk) in self.h.iter().enumerate() {
            let ph = -om * (self.n0 + k) as f64;
            acc += C64::new(hk * ph.cos(), hk * ph.sin());
        }
        acc
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

#[derive(Debug, Clone)]
struct Equalizer {
    taps: Vec<f64>,
    d: usize,
    /// `n_drive` rings of `taps.len()` samples.
    hist: Vec<f64>,
    pos: usize,
    /// The drive through the FIR, per drive.
    forced: Vec<f64>,
    /// The drive delayed `d` samples, per drive.
    direct: Vec<f64>,
}

impl Equalizer {
    fn new(taps: Vec<f64>, n_drive: usize) -> Self {
        let n = taps.len();
        Equalizer {
            d: n / 2,
            hist: vec![0.0; n * n_drive],
            pos: 0,
            forced: vec![0.0; n_drive],
            direct: vec![0.0; n_drive],
            taps,
        }
    }

    #[inline]
    fn push(&mut self, delayed: &[f64]) {
        let n = self.taps.len();
        self.pos = (self.pos + n - 1) % n;
        for (j, &x) in delayed.iter().enumerate() {
            let ring = &mut self.hist[j * n..(j + 1) * n];
            ring[self.pos] = x;
            let mut acc = 0.0;
            for (k, &c) in self.taps.iter().enumerate() {
                acc += c * ring[(self.pos + k) % n];
            }
            self.forced[j] = acc;
            self.direct[j] = ring[(self.pos + self.d) % n];
        }
    }

    fn reset(&mut self) {
        self.hist.fill(0.0);
        self.forced.fill(0.0);
        self.direct.fill(0.0);
        self.pos = 0;
    }
}

#[derive(Debug, Clone)]
struct ModeState {
    w0: f64,
    zeta0: f64,
    a_ref: f64,
    q: f64,
    v: f64,
    u_prev: f64,
    envelope: f64,
    /// The envelope's decay per sample.
    decay: f64,
    /// The envelope the current step was made for.
    stepped_at: f64,
    step: ModeStep,
    peak: f64,
    rule: ModeRule,
}

/// A running jointed plant (crate-private; built by the backend at `configure`).
#[derive(Debug, Clone)]
pub(crate) struct JointedRun {
    pub(crate) n_out: usize,
    n_drive: usize,
    /// `n_modes x n_drive`, row-major.
    phi: Vec<f64>,
    /// `n_modes x n_out`, row-major.
    psi: Vec<f64>,
    modes: Vec<ModeState>,
    residual: Vec<(usize, usize, f64)>,
    feedthrough: f64,
    delays: Vec<DelayLine>,
    delayed: Vec<f64>,
    /// The equalized step: taps, each drive's history (newest first is `hist_pos`), and
    /// this sample's equalized force and delayed direct drive per drive.
    eq: Option<Equalizer>,
    clean: Vec<f64>,
    odelays: Vec<FracDelay>,
    rng: Xorshift64,
    noise_level: f64,
    dt: f64,
}

impl JointedRun {
    pub(crate) fn new(p: &JointedPlant, fs_hz: f64, noise_level: f64, seed: u64) -> Self {
        let dt = 1.0 / fs_hz;
        let mut phi = Vec::with_capacity(p.modes.len() * p.n_drives);
        let mut psi = Vec::with_capacity(p.modes.len() * p.n_inputs);
        for m in &p.modes {
            phi.extend_from_slice(&m.input_shape);
            psi.extend_from_slice(&m.output_shape);
        }
        let decay_cycles = p.joint.map_or(3.0, |j| j.decay_cycles);
        let modes = p
            .modes
            .iter()
            .map(|m| {
                let w0 = TAU * m.f_hz;
                ModeState {
                    w0,
                    zeta0: m.zeta,
                    a_ref: m.reference_amplitude,
                    q: 0.0,
                    v: 0.0,
                    u_prev: 0.0,
                    envelope: 0.0,
                    decay: (-m.f_hz * dt / decay_cycles).exp(),
                    stepped_at: 0.0,
                    step: ModeStep::new(w0, m.zeta, dt),
                    peak: 0.0,
                    rule: ModeRule::of(m, p.joint),
                }
            })
            .collect();
        let residual = if p.residual.is_empty() {
            Vec::new()
        } else {
            (0..p.n_inputs)
                .flat_map(|i| (0..p.n_drives).map(move |j| (i, j)))
                .filter_map(|(i, j)| {
                    let v = p.residual[i * p.n_drives + j];
                    (v != 0.0).then_some((i, j, v))
                })
                .collect()
        };
        JointedRun {
            n_out: p.n_inputs,
            n_drive: p.n_drives,
            phi,
            psi,
            modes,
            residual,
            feedthrough: p.feedthrough,
            delays: (0..p.n_drives)
                .map(|_| DelayLine::new(p.transport_delay_samples))
                .collect(),
            delayed: vec![0.0; p.n_drives],
            eq: p.step.taps(fs_hz).map(|t| Equalizer::new(t, p.n_drives)),
            clean: vec![0.0; p.n_inputs],
            odelays: (0..p.n_inputs)
                .map(|i| FracDelay::new(p.output_delay_samples(i, fs_hz)))
                .collect(),
            rng: Xorshift64::new(seed),
            noise_level,
            dt,
        }
    }

    #[inline]
    pub(crate) fn step(&mut self, drive: &[f64], out: &mut [f64]) {
        let mut sum = 0.0;
        for ((d, line), &x) in self.delayed.iter_mut().zip(&mut self.delays).zip(drive) {
            *d = line.process(x);
            sum += *d;
        }
        if let Some(eq) = &mut self.eq {
            eq.push(&self.delayed);
            sum = eq.direct.iter().sum();
        }
        let direct = self.eq.as_ref().map_or(&self.delayed, |e| &e.direct);
        let forced = self.eq.as_ref().map_or(&self.delayed, |e| &e.forced);
        self.clean.fill(self.feedthrough * sum);
        for &(i, j, v) in &self.residual {
            self.clean[i] += v * direct[j];
        }
        let equalized = self.eq.is_some();
        let (nd, no) = (self.n_drive, self.n_out);
        for (m, st) in self.modes.iter_mut().enumerate() {
            if !matches!(st.rule, ModeRule::Linear) && st.envelope != st.stepped_at {
                let (w, z) = st.rule.params(st.w0, st.zeta0, st.envelope / st.a_ref);
                st.step = ModeStep::new(w, z, self.dt);
                st.stepped_at = st.envelope;
            }
            let shape = &self.phi[m * nd..(m + 1) * nd];
            let u: f64 = shape.iter().zip(forced).map(|(a, b)| a * b).sum();
            let mut a = st.step.advance(&mut st.q, &mut st.v, st.u_prev, u);
            if equalized {
                // The direct part of the acceleration is the delayed drive, not the
                // equalized force.
                let u_dir: f64 = shape.iter().zip(direct).map(|(a, b)| a * b).sum();
                a += u_dir - u;
            }
            st.u_prev = u;
            let amplitude = st.q.hypot(st.v / st.step.w);
            st.envelope = amplitude.max(st.envelope * st.decay);
            st.peak = st.peak.max(st.envelope);
            for (c, &s) in self.clean.iter_mut().zip(&self.psi[m * no..(m + 1) * no]) {
                *c += s * a;
            }
        }
        let out = &mut out[..self.n_out];
        for ((o, line), &c) in out.iter_mut().zip(&mut self.odelays).zip(&self.clean) {
            *o = line.process(c);
        }
        crate::plant::add_noise(&mut self.rng, self.noise_level, out);
    }

    pub(crate) fn reset(&mut self) {
        let dt = self.dt;
        for st in &mut self.modes {
            st.q = 0.0;
            st.v = 0.0;
            st.u_prev = 0.0;
            st.envelope = 0.0;
            st.stepped_at = 0.0;
            st.peak = 0.0;
            st.step = ModeStep::new(st.w0, st.zeta0, dt);
        }
        self.delays.iter_mut().for_each(DelayLine::reset);
        self.delayed.fill(0.0);
        if let Some(eq) = &mut self.eq {
            eq.reset();
        }
        self.odelays.iter_mut().for_each(FracDelay::reset);
    }
}
