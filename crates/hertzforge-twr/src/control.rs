//! The TWR control-law seam: what a law sees, what it returns, how it is built,
//! and the closed-loop settings the core runs it under.
//!
//! # Where a law runs
//!
//! Between plays, on the run thread, never during a play: once at proceed,
//! after the identification and its review ([`TwrLaw::seed`]), and after each
//! judged play the core wants corrected ([`TwrLaw::iterate`]): an iteration
//! play that did not gate while adaptation is not frozen, and a playback play
//! when adaptation during playback is on. Each call is timed against a budget
//! ([`TwrControl::call_budget_s`]) and contained: a late, failed, panicked or
//! refused call is a miss, and the next play replays the drive in force.
//!
//! # What a law decides, and what it does not
//!
//! A law returns the next FULL-LEVEL drive record in volts, and up to
//! [`hertzforge_shock_common::law_guard::MAX_LAW_VALUES`] declared values. It
//! does not decide the level, the ladder, the gate, when a play starts, the
//! playback schedule, adaptation, the trust mask or any abort. The core scales
//! the drive to the commanded level and refers the measured response back to
//! full level by the scale it actually commanded, so a law cannot raise the
//! level.
//!
//! # Conventions
//!
//! These types are the native side of the `twr-single` world
//! (`wit/control-law/twr-single.wit`), field for field, so a law file and a
//! native law see the same numbers:
//!
//! - every record has `record_len` samples at `fs_hz`: a quiet lead-in, the
//!   replicated section, a quiet lead-out, padded to a power of two;
//! - a spectrum is the one-sided transform under the NEGATIVE-exponent
//!   transform, `record_len / 2 + 1` bins. `hertzforge_dsp`'s FFT is
//!   positive-exponent, so the core conjugates its own spectra when it builds a
//!   [`TwrLawSetup`];
//! - reference and response in the control channel's units (EU), drive in
//!   volts, the plant in EU per volt.
//!
//! # State of the seam
//!
//! These are the types and the settings only. [`crate::TwrConfig::control`]
//! `None` is the iteration the crate has always run, unchanged; a
//! configuration that sets it is refused by [`crate::TwrService::new`] until
//! the core runs a law under these guards.

use hertzforge_dsp::fft::Cplx;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawGuards, LawValue};
use serde::{Deserialize, Serialize};

use crate::limits::TwrLimits;
use crate::TwrError;

/// The catalog id of the built-in TWR law, `twr_correct`. Built-in ids are
/// global across every world's catalog, so it is its own.
pub const TWR_CORRECT_ID: &str = "twr_correct";

/// Default per-call cap on the rise of the drive's peak, dB, at full level,
/// against the last drive played.
pub const DEFAULT_MAX_PEAK_RISE_DB: f64 = 10.0;
/// Default per-call cap on the rise of any drive bin's magnitude, dB, at full
/// level, against the last drive played.
pub const DEFAULT_MAX_BIN_RISE_DB: f64 = 10.0;
/// Bins more than this many dB under the largest bin of the last drive are
/// held to that floor before the bin-rise cap compares them, so a bin rising
/// out of a spectral null is not refused for its ratio alone.
pub const BIN_RISE_FLOOR_DB: f64 = 40.0;
/// The per-call caps are never tighter than the built-in law's own growth
/// clamp plus this margin, dB, so the core never refuses the built-in's own
/// corrections.
pub const CAP_MARGIN_OVER_CLAMP_DB: f64 = 1.0;
/// Default bound on the seed's predicted response peak, times the level
/// target's peak.
pub const DEFAULT_FIRST_DRIVE_RATIO: f64 = 2.0;
/// Default largest drift, frames, of a play's fitted lag from the
/// identification's before the run ends `alignment`.
pub const DEFAULT_ALIGNMENT_FRAMES: f64 = 1.0;
/// The shortest time a law call's budget is a fraction of, seconds: the
/// budget is `law_budget` times the longer of the record and this.
pub const MIN_BUDGET_BASIS_S: f64 = 4.0;
/// The longest record, samples on the record grid, a closed-loop run takes.
pub const MAX_RECORD_LEN: usize = 1 << 20;

/// The core's plant estimate from the identification, on the record grid's
/// bins (`record_len / 2 + 1` each), negative-exponent convention.
#[derive(Debug, Clone, Copy)]
pub struct TwrPlant<'a> {
    /// `H[k]`, control response per drive volt.
    pub frf: &'a [Cplx],
    /// Ordinary coherence of the identification, 0 to 1.
    pub coherence: &'a [f64],
    /// The core's regularized inverse, `conj(H) / (|H|^2 + epsilon)`.
    pub inverse: &'a [Cplx],
    /// Bins the core trusts: in the correction band, coherence at or over its
    /// floor, reference within its dynamic-range floor.
    pub trusted: &'a [bool],
    /// The Tikhonov epsilon behind `inverse`, (EU/V)^2.
    pub epsilon: f64,
}

/// What a TWR law is built for: fixed for the run.
#[derive(Debug, Clone, Copy)]
pub struct TwrLawSetup<'a> {
    /// The rate the station runs at, Hz.
    pub fs_hz: f64,
    /// Samples in every record.
    pub record_len: usize,
    /// The correction band's low edge, Hz.
    pub band_lo_hz: f64,
    /// The correction band's high edge, Hz.
    pub band_hi_hz: f64,
    /// The full-level reference on the record grid: the prepared section
    /// between its quiet pads, EU.
    pub reference: &'a [f64],
    /// The replicated section's first sample within `reference`.
    pub section_start: usize,
    /// The replicated section's length, samples.
    pub section_len: usize,
    /// The identified plant.
    pub plant: TwrPlant<'a>,
    /// The drive ceiling at full level, volts peak.
    pub drive_ceiling_v: f64,
    /// The time one call may take, seconds. A law file's host deadline is the
    /// same number.
    pub call_budget_s: f64,
}

impl TwrLawSetup<'_> {
    /// Bins on the grid: `record_len / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.record_len / 2 + 1
    }
}

/// The core's judgement of the last play.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// The play met the convergence gate at its level.
    pub gated: bool,
    /// `100 ||y - r|| / ||r||` over the record.
    pub rms_error_pct: f64,
    /// `100 max|y - r| / max|r|`.
    pub peak_error_pct: f64,
    /// Mean `|20 log10(|Y| / |R|)|` over the trusted bins, dB.
    pub spectral_error_db: f64,
}

/// What the last play was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayKind {
    /// A correction play on the ladder. The seed's inputs say `Iteration`: the
    /// play it seeds is one.
    Iteration,
    /// A scheduled playback play at full level.
    Playback,
}

impl PlayKind {
    /// The stable wire and record name.
    pub fn as_str(self) -> &'static str {
        match self {
            PlayKind::Iteration => "iteration",
            PlayKind::Playback => "playback",
        }
    }
}

/// Everything a TWR law sees after a play, or before the first.
#[derive(Debug, Clone, Copy)]
pub struct PlayInputs<'a> {
    /// Plays judged so far, the identification not counted: 0 at the seed.
    pub play_index: u64,
    /// What the last play was ([`PlayKind::Iteration`] at the seed).
    pub kind: PlayKind,
    /// The last play's response referred to full level, EU, `record_len`
    /// samples. Empty at the seed.
    pub response: &'a [f64],
    /// The drive the last play played, referred to full level, volts, after the
    /// core's clip and taper. Empty at the seed.
    pub drive: &'a [f64],
    /// `None` at the seed.
    pub judged: Option<Verdict>,
    /// The core clipped the last drive.
    pub limited: bool,
}

/// A TWR law's output.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayOutputs {
    /// The next full-level drive, volts: `record_len` finite samples.
    pub drive: Vec<f64>,
    /// At most `MAX_LAW_VALUES` values, each under a declared key.
    pub values: Vec<LawValue>,
}

/// A TWR law. Its state lives in the value, built once per run at proceed by a
/// [`TwrLawFactory`].
pub trait TwrLaw {
    /// The law's name, for the log and the report.
    fn name(&self) -> &str;

    /// The first drive, before any play. A miss here plays nothing.
    fn seed(&mut self, inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault>;

    /// The next drive, after a play was played, measured and judged.
    fn iterate(&mut self, inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault>;
}

/// Builds the TWR law for a run. Called once per run, at proceed. The factory
/// holds the law's parameter values; the core holds the guards.
pub trait TwrLawFactory: Send + Sync {
    /// Build a law for `setup`, or say why it cannot be built (the run then
    /// ends `law_fault` before any play).
    fn build(&self, setup: &TwrLawSetup<'_>) -> Result<Box<dyn TwrLaw + Send>, LawFault>;

    /// The law as the run's records name it.
    fn describe(&self) -> LawDescriptor;
}

/// The closed-loop settings of a TWR run: the guards on the law, the per-call
/// caps, the first-drive bound, the alignment check and the at-rest taper.
///
/// Carried in [`crate::TwrConfig::control`]. `None` there is the iteration the
/// crate has always run, kept for the old daemon host, the module sweep and the
/// recorded trace; the control service always sets it.
///
/// The predicted-peak bound on every drive is not a field: it is
/// `1 + envelope_abort_frac` of the run's limits
/// ([`TwrControl::predicted_peak_ratio`]), so the prediction and the envelope
/// abort cannot disagree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrControl {
    /// The guards on calling the law: the budget fraction and the misses in a
    /// row. A TWR run's plays are bounded by its own schedule (iterations per
    /// rung, then the playback schedule), so `max_pulses` is not read.
    pub guards: LawGuards,
    /// Per-call cap on the rise of the drive's peak at full level, dB.
    pub max_peak_rise_db: f64,
    /// Per-call cap on the rise of any drive bin's magnitude at full level, dB
    /// (bins under [`BIN_RISE_FLOOR_DB`] of the largest held to that floor).
    pub max_bin_rise_db: f64,
    /// Bound on the seed's predicted response peak, times the level target's
    /// peak.
    pub first_drive_ratio: f64,
    /// The largest drift, frames, of a play's fitted lag from the
    /// identification's; more ends the run `alignment`.
    pub alignment_frames: f64,
    /// Taper the outer half of the leading and trailing pads of every drive to
    /// zero (raised cosine), so every play starts and ends at 0 V.
    pub rest_taper: bool,
}

impl Default for TwrControl {
    /// The defaults: the shared guards (a quarter of the budget basis, three
    /// misses), 10 dB caps, a first drive at most twice the target's peak, one
    /// frame of alignment drift, and the at-rest taper on.
    fn default() -> Self {
        TwrControl {
            guards: LawGuards::default(),
            max_peak_rise_db: DEFAULT_MAX_PEAK_RISE_DB,
            max_bin_rise_db: DEFAULT_MAX_BIN_RISE_DB,
            first_drive_ratio: DEFAULT_FIRST_DRIVE_RATIO,
            alignment_frames: DEFAULT_ALIGNMENT_FRAMES,
            rest_taper: true,
        }
    }
}

impl TwrControl {
    /// The time one law call may take, seconds, for a record of `record_s`
    /// seconds: `law_budget` times the longer of the record and
    /// [`MIN_BUDGET_BASIS_S`].
    pub fn call_budget_s(&self, record_s: f64) -> f64 {
        self.guards.budget_fraction * record_s.max(MIN_BUDGET_BASIS_S)
    }

    /// The bound on every drive's predicted response peak, times the level
    /// target's peak: `1 + envelope_abort_frac`.
    pub fn predicted_peak_ratio(limits: &TwrLimits) -> f64 {
        1.0 + limits.envelope_abort_frac
    }

    /// Check the caps against a law's own growth clamp, dB: each cap at least
    /// the clamp plus [`CAP_MARGIN_OVER_CLAMP_DB`]. The error names the field.
    pub fn check_caps_against_clamp(&self, clamp_db: f64) -> Result<(), TwrError> {
        let floor = clamp_db + CAP_MARGIN_OVER_CLAMP_DB;
        for (field, cap) in [
            ("control.max_peak_rise_db", self.max_peak_rise_db),
            ("control.max_bin_rise_db", self.max_bin_rise_db),
        ] {
            if cap < floor {
                return Err(TwrError::Config(format!(
                    "{field} must be at least the law's own clamp of {clamp_db} dB plus \
                     {CAP_MARGIN_OVER_CLAMP_DB} dB (got {cap})"
                )));
            }
        }
        Ok(())
    }

    /// Check every field. The error names the field by its path under
    /// `control`.
    pub fn validate(&self) -> Result<(), TwrError> {
        self.guards
            .validate()
            .map_err(|e| TwrError::Config(format!("control.{e}")))?;
        positive("control.max_peak_rise_db", self.max_peak_rise_db)?;
        positive("control.max_bin_rise_db", self.max_bin_rise_db)?;
        if !(self.first_drive_ratio.is_finite() && self.first_drive_ratio >= 1.0) {
            return Err(TwrError::Config(format!(
                "control.first_drive_ratio must be at least 1 (got {})",
                self.first_drive_ratio
            )));
        }
        positive("control.alignment_frames", self.alignment_frames)?;
        Ok(())
    }
}

fn positive(field: &str, v: f64) -> Result<(), TwrError> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(TwrError::Config(format!(
            "{field} must be above 0 (got {v})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_plan_numbers_and_validate() {
        let c = TwrControl::default();
        c.validate().unwrap();
        assert_eq!(c.guards.budget_fraction, 0.25);
        assert_eq!(c.guards.max_misses, 3);
        assert_eq!((c.max_peak_rise_db, c.max_bin_rise_db), (10.0, 10.0));
        assert_eq!(c.first_drive_ratio, 2.0);
        assert_eq!(c.alignment_frames, 1.0);
        assert!(c.rest_taper);
        assert_eq!(TWR_CORRECT_ID, "twr_correct");
        assert_eq!(MAX_RECORD_LEN, 1_048_576);
    }

    #[test]
    fn the_budget_is_a_fraction_of_the_record_or_four_seconds() {
        let c = TwrControl::default();
        // The bench's record: 65 536 samples at 12 800 Hz is 5.12 s.
        assert!((c.call_budget_s(65_536.0 / 12_800.0) - 1.28).abs() < 1e-12);
        // A short record is held to the 4 s basis.
        assert!((c.call_budget_s(2.0) - 1.0).abs() < 1e-12);
        // The record cap at 12 800 Hz: 81.92 s, 20.48 s a call.
        assert!((c.call_budget_s(MAX_RECORD_LEN as f64 / 12_800.0) - 20.48).abs() < 1e-9);
    }

    #[test]
    fn the_predicted_peak_bound_follows_the_envelope_abort() {
        let mut l = TwrLimits::default();
        assert!((TwrControl::predicted_peak_ratio(&l) - 1.5).abs() < 1e-15);
        l.envelope_abort_frac = 0.3;
        assert!((TwrControl::predicted_peak_ratio(&l) - 1.3).abs() < 1e-15);
    }

    #[test]
    fn the_caps_are_never_tighter_than_the_laws_clamp_plus_a_decibel() {
        let c = TwrControl::default();
        c.check_caps_against_clamp(6.0).unwrap();
        c.check_caps_against_clamp(9.0).unwrap();
        let e = c.check_caps_against_clamp(9.5).unwrap_err().to_string();
        assert!(e.contains("control.max_peak_rise_db"), "{e}");
        let tight = TwrControl {
            max_bin_rise_db: 6.5,
            ..TwrControl::default()
        };
        let e = tight.check_caps_against_clamp(6.0).unwrap_err().to_string();
        assert!(e.contains("control.max_bin_rise_db"), "{e}");
    }

    #[test]
    fn each_bad_field_is_named() {
        let base = TwrControl::default();
        let cases: Vec<(TwrControl, &str)> = vec![
            (
                TwrControl {
                    max_peak_rise_db: 0.0,
                    ..base.clone()
                },
                "control.max_peak_rise_db",
            ),
            (
                TwrControl {
                    max_bin_rise_db: f64::NAN,
                    ..base.clone()
                },
                "control.max_bin_rise_db",
            ),
            (
                TwrControl {
                    first_drive_ratio: 0.5,
                    ..base.clone()
                },
                "control.first_drive_ratio",
            ),
            (
                TwrControl {
                    alignment_frames: 0.0,
                    ..base.clone()
                },
                "control.alignment_frames",
            ),
        ];
        for (c, field) in cases {
            let e = c.validate().unwrap_err().to_string();
            assert!(e.contains(field), "{field}: {e}");
        }
        let mut c = base.clone();
        c.guards.max_misses = 0;
        assert!(c
            .validate()
            .unwrap_err()
            .to_string()
            .contains("control.law_misses"));
        let mut c = base;
        c.guards.budget_fraction = 1.5;
        assert!(c
            .validate()
            .unwrap_err()
            .to_string()
            .contains("control.law_budget"));
    }

    #[test]
    fn control_round_trips_through_json_and_play_kinds_carry_their_wire_names() {
        let c = TwrControl::default();
        let back: TwrControl = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        for k in [PlayKind::Iteration, PlayKind::Playback] {
            assert_eq!(
                serde_json::to_value(k).unwrap(),
                serde_json::Value::String(k.as_str().into())
            );
        }
    }
}
