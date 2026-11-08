//! The SRS control-law seam: what a law sees, what it returns, how it is
//! built, and the closed-loop settings the core runs it under.
//!
//! # Where a law runs
//!
//! Between pulses, on the shock run thread, never during a fire: once after
//! system identification and before the arm-time limit notch is planned
//! ([`SrsLaw::seed`]), and once after each fired, measured and judged pulse
//! ([`SrsLaw::iterate`]). Each call is timed against a budget
//! (`guards.budget_fraction` times the rate model's minimum pulse spacing) and
//! contained: a late, failed, panicked or refused call is a miss, and the next
//! pulse refires the drive in force.
//!
//! # What a law decides, and what it does not
//!
//! A law returns one multiplicative gain per synthesis component (finite, above
//! zero) and up to [`hertzforge_shock_common::law_guard::MAX_LAW_VALUES`]
//! declared values. At the seed the gains are the starting gains relative to
//! the model table. The core synthesizes every pulse from the table, so the
//! wavelets' self-nulling and the damped sines' compensation hold by
//! construction whatever a law returns.
//!
//! After each call the core caps the gains: by its own per-pulse rise limit
//! ([`SrsControl::max_gain_rise_db`]), and by the predictive limit cap, the
//! tightest allowed-over-predicted ratio over the limit channels and the bands
//! near each component, taken as `min(law gain, cap)`, so a falling gain
//! always wins over a rising one. It then synthesizes the drive in volts and
//! checks it against the drive ceiling and abort, the predicted response peak
//! and the predicted kinematics before it plays.
//!
//! A law does not decide the level, the ladder, when a pulse fires, the
//! spacing, the envelope verdict or any abort.
//!
//! # Conventions
//!
//! These types are the native side of the `shock-srs` world
//! (`wit/control-law/shock-srs.wit`), field for field, so a law file and a
//! native law see the same numbers:
//!
//! - every spectrum is a shock response spectrum on the band grid `band_hz` at
//!   quality factor `q`, in the control channel's units (EU);
//! - `component_hz` is the synthesis grid, one frequency per component in table
//!   order, distinct from the band grid;
//! - the identified gain at each component is in EU per volt.
//!
//! # State of this seam
//!
//! The traits, the inputs and outputs and [`SrsControl`] are the contract the
//! closed loop is built against. The service stores a factory
//! ([`crate::SrsShockService::with_law_factory`]) but does not call it yet, and
//! refuses a configuration that asks for closed-loop control until the loop is
//! built.

use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawGuards, LawValue};
use hertzforge_shock_common::{DrivePath, RateModel};
use serde::{Deserialize, Serialize};

/// The identification burst's default peak, as a fraction of the drive
/// ceiling.
pub const DEFAULT_SYSID_VPK_FRACTION: f64 = 0.1;
/// Default mean in-band coherence under which identification holds the run.
pub const DEFAULT_SYSID_MIN_MEAN_COHERENCE: f64 = 0.8;
/// Default per-pulse cap on the rise of any component's gain, dB. It sits
/// above the built-in law's own largest change, so it never binds on it.
pub const DEFAULT_MAX_GAIN_RISE_DB: f64 = 6.0;

/// What an SRS law is built for: fixed for the run.
#[derive(Debug, Clone, Copy)]
pub struct SrsLawSetup<'a> {
    /// The analysis band centres, Hz, ascending.
    pub band_hz: &'a [f64],
    /// The synthesis components' frequencies, Hz, in table order.
    pub component_hz: &'a [f64],
    /// The full-level target SRS on the band grid, EU.
    pub target: &'a [f64],
    /// The quality factor every SRS is computed at.
    pub q: f64,
    /// `|H|` at each component, EU per volt, floored by the core. `None` when
    /// the run identified nothing.
    pub component_gain: Option<&'a [f64]>,
    /// The time one call may take, seconds. A law file's host deadline is the
    /// same number.
    pub call_budget_s: f64,
}

impl SrsLawSetup<'_> {
    /// Bands on the analysis grid.
    pub fn bands(&self) -> usize {
        self.band_hz.len()
    }

    /// Components in the table.
    pub fn components(&self) -> usize {
        self.component_hz.len()
    }
}

/// Everything an SRS law sees after a pulse, or before the first.
#[derive(Debug, Clone, Copy)]
pub struct SrsInputs<'a> {
    /// Pulses fired so far: 0 at the seed.
    pub pulse_index: u64,
    /// The target at the scale the last pulse was commanded at (level times the
    /// core's safety trims); at the seed, at the starting level. EU, on the
    /// band grid.
    pub target: &'a [f64],
    /// The last pulse's measured control SRS, EU, on the band grid; zeros at
    /// the seed.
    pub measured: &'a [f64],
    /// The scale the last pulse was commanded at, linear; at the seed, the
    /// starting level's.
    pub commanded_scale: f64,
}

/// An SRS law's output.
#[derive(Debug, Clone, PartialEq)]
pub struct SrsOutputs {
    /// One multiplicative update per component, in table order: finite, above
    /// zero.
    pub gains: Vec<f64>,
    /// At most `MAX_LAW_VALUES` values, each under a declared key.
    pub values: Vec<LawValue>,
}

/// An SRS shock law. Its state lives in the value, built once per run after
/// system identification by an [`SrsLawFactory`].
pub trait SrsLaw {
    /// The law's name, for the log and the report.
    fn name(&self) -> &str;

    /// The starting gains, before any pulse has fired. A miss here fires
    /// nothing.
    fn seed(&mut self, inputs: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault>;

    /// The next gains, after a pulse was fired, measured and judged.
    fn iterate(&mut self, inputs: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault>;
}

/// Builds the SRS law for a run. Called once per run, after system
/// identification. The factory holds the law's parameter values; the core
/// holds the guards.
pub trait SrsLawFactory: Send + Sync {
    /// Build a law for `setup`, or say why it cannot be built (the run then
    /// ends `law_fault` before any pulse).
    fn build(&self, setup: &SrsLawSetup<'_>) -> Result<Box<dyn SrsLaw + Send>, LawFault>;

    /// The law as the run's records name it.
    fn describe(&self) -> LawDescriptor;
}

/// The closed-loop settings of an SRS run: the drive ceiling and abort in
/// volts, the identification burst's cap and its coherence hold, the pacing
/// between pulses, the guards on the law and the core's per-pulse gain cap.
///
/// Carried in [`crate::SrsTestConfig::control`]. `None` there is the run as it
/// was before the drive was in volts (the synthesis in g written as volts, the
/// pulses back to back), kept only for the old daemon host and the recorded
/// trace; the control service always sets it, and always identifies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrsControl {
    /// Drive ceiling, volts peak: a commanded drive over it is scaled down to
    /// it (a logged drive-limit action).
    pub drive_vpk_ceiling: f64,
    /// Drive abort, volts peak: a commanded drive over it is refused and the
    /// run aborts. Above the ceiling.
    pub drive_vpk_abort: f64,
    /// The identification burst's peak, volts, at most this. `None`:
    /// [`DEFAULT_SYSID_VPK_FRACTION`] of the drive ceiling.
    pub sysid_max_vpk: Option<f64>,
    /// Mean in-band coherence of the identification under which the run is
    /// held before any pulse.
    pub min_mean_coherence: f64,
    /// The floors between pulse starts. The time between pulses is the largest
    /// of the floors that apply on the host path.
    pub rate_model: RateModel,
    /// The guards on calling the law: budget, misses, the run's pulse cap.
    pub guards: LawGuards,
    /// The most any component's gain may rise in one pulse, dB. A law output
    /// over it is refused (a miss).
    pub max_gain_rise_db: f64,
}

impl SrsControl {
    /// The defaults, with the drive ceiling and abort the run must state: the
    /// identification burst capped at a tenth of the ceiling, a coherence hold
    /// at 0.8, the rate model's default floors, the guards' defaults and a 6 dB
    /// per-pulse gain cap.
    pub fn new(drive_vpk_ceiling: f64, drive_vpk_abort: f64) -> Self {
        SrsControl {
            drive_vpk_ceiling,
            drive_vpk_abort,
            sysid_max_vpk: None,
            min_mean_coherence: DEFAULT_SYSID_MIN_MEAN_COHERENCE,
            rate_model: RateModel::conservative_default(),
            guards: LawGuards::default(),
            max_gain_rise_db: DEFAULT_MAX_GAIN_RISE_DB,
        }
    }

    /// The identification burst's peak cap in force, volts.
    pub fn identification_vpk_cap(&self) -> f64 {
        self.sysid_max_vpk
            .unwrap_or(DEFAULT_SYSID_VPK_FRACTION * self.drive_vpk_ceiling)
    }

    /// The time between pulse starts, seconds: the largest of the floors that
    /// apply on the host path.
    pub fn min_spacing_s(&self) -> f64 {
        self.rate_model.effective_min_spacing(DrivePath::HostDac)
    }

    /// Check every field. The error names the field by its wire path.
    pub fn validate(&self) -> Result<(), String> {
        positive("limits.drive_vpk_ceiling", self.drive_vpk_ceiling)?;
        if !(self.drive_vpk_abort.is_finite() && self.drive_vpk_abort > self.drive_vpk_ceiling) {
            return Err(format!(
                "limits.drive_vpk_abort must be above the drive ceiling of {} V (got {})",
                self.drive_vpk_ceiling, self.drive_vpk_abort
            ));
        }
        if let Some(v) = self.sysid_max_vpk {
            if !(v.is_finite() && v > 0.0 && v <= self.drive_vpk_ceiling) {
                return Err(format!(
                    "identification.max_vpk must be above 0 and at most the drive ceiling of \
                     {} V (got {v})",
                    self.drive_vpk_ceiling
                ));
            }
        }
        if !(self.min_mean_coherence.is_finite() && (0.0..1.0).contains(&self.min_mean_coherence)) {
            return Err(format!(
                "identification.min_mean_coherence must be at least 0 and below 1 (got {})",
                self.min_mean_coherence
            ));
        }
        let spacing = self.min_spacing_s();
        if !(spacing.is_finite() && spacing > 0.0) {
            return Err(format!(
                "spacing: the time between pulses must be above 0 s and finite (got {spacing})"
            ));
        }
        self.guards.validate().map_err(|e| format!("control.{e}"))?;
        positive("control.max_gain_rise_db", self.max_gain_rise_db)?;
        Ok(())
    }
}

fn positive(field: &str, v: f64) -> Result<(), String> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(format!("{field} must be above 0 (got {v})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_validate_and_cap_identification_at_a_tenth_of_the_ceiling() {
        let c = SrsControl::new(0.5, 0.8);
        c.validate().unwrap();
        assert!((c.identification_vpk_cap() - 0.05).abs() < 1e-15);
        assert_eq!(c.min_spacing_s(), 1.0, "the cooling floor binds by default");
        assert_eq!(c.max_gain_rise_db, 6.0);
        let c = SrsControl {
            sysid_max_vpk: Some(0.02),
            ..c
        };
        assert_eq!(c.identification_vpk_cap(), 0.02);
    }

    #[test]
    fn each_bad_field_is_named() {
        let base = SrsControl::new(0.5, 0.8);
        let cases: Vec<(SrsControl, &str)> = vec![
            (
                SrsControl {
                    drive_vpk_ceiling: 0.0,
                    ..base.clone()
                },
                "drive_vpk_ceiling",
            ),
            (
                SrsControl {
                    drive_vpk_abort: 0.5,
                    ..base.clone()
                },
                "drive_vpk_abort",
            ),
            (
                SrsControl {
                    sysid_max_vpk: Some(0.6),
                    ..base.clone()
                },
                "identification.max_vpk",
            ),
            (
                SrsControl {
                    min_mean_coherence: 1.0,
                    ..base.clone()
                },
                "min_mean_coherence",
            ),
            (
                SrsControl {
                    max_gain_rise_db: f64::NAN,
                    ..base.clone()
                },
                "max_gain_rise_db",
            ),
        ];
        for (c, field) in cases {
            let e = c.validate().unwrap_err();
            assert!(e.contains(field), "{field}: {e}");
        }
        let mut c = base.clone();
        c.guards.max_misses = 0;
        assert!(c.validate().unwrap_err().contains("control.law_misses"));
        let mut c = base;
        c.rate_model.read_cadence_ok = false;
        assert!(c.validate().unwrap_err().contains("spacing"));
    }

    #[test]
    fn control_round_trips_through_json() {
        let c = SrsControl {
            sysid_max_vpk: Some(0.04),
            ..SrsControl::new(0.5, 0.8)
        };
        let back: SrsControl = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
