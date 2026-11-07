//! The classical-shock control-law seam: what a law sees, what it returns, how
//! it is built, and the closed-loop settings the core runs it under.
//!
//! # Where a law runs
//!
//! Between pulses, on the shock run thread, never during a fire: once after
//! system identification ([`ClassicalLaw::seed`]) and once after each fired,
//! measured and judged pulse ([`ClassicalLaw::iterate`]). Each call is timed
//! against a budget (`guards.budget_fraction` times the rate model's minimum
//! pulse spacing) and contained: a late, failed, panicked or refused call is a
//! miss, and the next pulse refires the drive in force.
//!
//! # What a law decides, and what it does not
//!
//! A law returns the next FULL-LEVEL drive record in volts, and up to
//! [`hertzforge_shock_common::law_guard::MAX_LAW_VALUES`] declared values. It
//! does not decide the level, the ladder, when a pulse fires, the spacing, a
//! train, the polarity, the tolerance verdict or any abort. The core scales the
//! drive to the commanded level and refers the measured response back to full
//! level by the scale it actually commanded, so a law cannot raise the level
//! and never fights a limiter.
//!
//! # Conventions
//!
//! These types are the native side of the `shock-classical` world
//! (`wit/control-law/shock-classical.wit`), field for field, so a law file and
//! a native law see the same numbers:
//!
//! - every record has `record_len` samples at `fs_hz`, starting with the quiet
//!   lead-in;
//! - a spectrum is the one-sided transform under the NEGATIVE-exponent
//!   transform, `record_len / 2 + 1` bins. `hertzforge_dsp`'s FFT is
//!   positive-exponent, so the core conjugates its own plant estimate when it
//!   builds a [`ClassicalLawSetup`];
//! - reference and response in the control channel's units (EU), drive in
//!   volts, the plant in EU per volt.
//!
//! # State of this seam
//!
//! The traits, the inputs and outputs and [`ClassicalControl`] are the contract
//! the closed loop is built against. The service stores a factory
//! ([`crate::ClassicalShockService::with_law_factory`]) but does not call it
//! yet, and refuses a configuration that asks for closed-loop control until the
//! loop is built.

use crate::tolerance::ToleranceStandard;
use hertzforge_dsp::fft::Cplx;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawGuards, LawValue};
use hertzforge_twr::inverse::InverseConfig;
use hertzforge_twr::sysid::SysIdConfig;
use serde::{Deserialize, Serialize};

/// Default identification drive, volts rms (band-limited random).
pub const DEFAULT_SYSID_DRIVE_VRMS: f64 = 0.05;
/// Default identification length, seconds.
pub const DEFAULT_SYSID_DURATION_S: f64 = 4.0;
/// Default low edge of the identification and correction band, Hz.
pub const DEFAULT_BAND_LO_HZ: f64 = 5.0;
/// Default high edge of the identification and correction band, Hz, before the
/// limit of [`DEFAULT_BAND_HI_FRACTION_OF_FS`] times the rate.
pub const DEFAULT_BAND_HI_HZ: f64 = 5000.0;
/// The default band's high edge is at most this fraction of the sample rate.
pub const DEFAULT_BAND_HI_FRACTION_OF_FS: f64 = 0.4;
/// Default mean in-band coherence under which identification holds the run.
pub const DEFAULT_SYSID_MIN_MEAN_COHERENCE: f64 = 0.8;
/// Default reference dynamic-range floor, dB below the reference spectrum's
/// in-band peak: bins under it are not trusted.
pub const DEFAULT_REF_FLOOR_DB: f64 = 50.0;
/// Default fraction of the reference's energy the trusted bins must cover.
pub const DEFAULT_MIN_BAND_ENERGY: f64 = 0.9;
/// Default per-pulse cap on the rise of the drive's peak, dB, at full level.
pub const DEFAULT_MAX_PEAK_RISE_DB: f64 = 10.0;
/// Default per-pulse cap on the rise of any drive bin's magnitude, dB, at full
/// level.
pub const DEFAULT_MAX_BIN_RISE_DB: f64 = 10.0;
/// Default bound on the seed's predicted response peak, times the reference
/// peak.
pub const DEFAULT_FIRST_DRIVE_RATIO: f64 = 2.0;
/// Default number of pulses in a row, at one level, whose error rises by more
/// than [`DIVERGENCE_RISE_DB`] before the run ends `divergence`.
pub const DEFAULT_DIVERGENCE_PULSES: u32 = 3;
/// How much a pulse's error must rise over the last pulse's, dB, to count
/// towards divergence.
pub const DIVERGENCE_RISE_DB: f64 = 1.0;
/// The overshoot abort's default, as a multiple of the standard's amplitude
/// tolerance: 0.30 under MIL-STD-810 Method 516, 0.40 under IEC 60068-2-27.
pub const DEFAULT_OVERSHOOT_TOLERANCE_MULTIPLE: f64 = 2.0;

/// The core's plant estimate from system identification, on the grid's bins
/// (`record_len / 2 + 1` each), negative-exponent convention.
#[derive(Debug, Clone, Copy)]
pub struct ClassicalPlant<'a> {
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

/// What a classical law is built for: fixed for the run.
#[derive(Debug, Clone, Copy)]
pub struct ClassicalLawSetup<'a> {
    /// The rate the station runs at, Hz.
    pub fs_hz: f64,
    /// Samples in every record.
    pub record_len: usize,
    /// The correction band's low edge, Hz.
    pub band_lo_hz: f64,
    /// The correction band's high edge, Hz.
    pub band_hi_hz: f64,
    /// The full-level reference on the grid: the compensated pulse, EU.
    pub reference: &'a [f64],
    /// The main pulse's first sample within `reference`.
    pub main_start: usize,
    /// The main pulse's length in samples.
    pub main_len: usize,
    /// The identified plant.
    pub plant: ClassicalPlant<'a>,
    /// The drive ceiling referred to full level, volts peak.
    pub drive_ceiling_v: f64,
    /// The time one call may take, seconds. A law file's host deadline is the
    /// same number.
    pub call_budget_s: f64,
}

impl ClassicalLawSetup<'_> {
    /// Bins on the grid: `record_len / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.record_len / 2 + 1
    }
}

/// The core's judgement of the last pulse.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Inside the standard's tolerance at its commanded level.
    pub passed: bool,
    /// RMS error over the judged window, percent of the reference.
    pub error_pct: f64,
    /// Measured peak over commanded peak.
    pub peak_ratio: f64,
    /// Measured velocity change over the nominal.
    pub velocity_change_ratio: f64,
}

/// Everything a classical law sees after a pulse, or before the first.
#[derive(Debug, Clone, Copy)]
pub struct PulseInputs<'a> {
    /// Pulses fired so far: 0 at the seed.
    pub pulse_index: u64,
    /// The last pulse's response referred to full level, EU, `record_len`
    /// samples. Empty at the seed.
    pub response: &'a [f64],
    /// The drive the last pulse played referred to full level, volts, after the
    /// core's limits. Empty at the seed.
    pub drive: &'a [f64],
    /// `None` at the seed.
    pub judged: Option<Verdict>,
    /// The core scaled the last pulse down (drive ceiling or a monitor).
    pub limited: bool,
}

/// A classical law's output.
#[derive(Debug, Clone, PartialEq)]
pub struct PulseOutputs {
    /// The next full-level drive, volts: `record_len` finite samples.
    pub drive: Vec<f64>,
    /// At most `MAX_LAW_VALUES` values, each under a declared key.
    pub values: Vec<LawValue>,
}

/// A classical shock law. Its state lives in the value, built once per run
/// after system identification by a [`ClassicalLawFactory`].
pub trait ClassicalLaw {
    /// The law's name, for the log and the report.
    fn name(&self) -> &str;

    /// The first drive, before any pulse has fired. A miss here fires nothing.
    fn seed(&mut self, inputs: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault>;

    /// The next drive, after a pulse was fired, measured and judged.
    fn iterate(&mut self, inputs: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault>;
}

/// Builds the classical law for a run. Called once per run, after system
/// identification. The factory holds the law's parameter values; the core
/// holds the guards.
pub trait ClassicalLawFactory: Send + Sync {
    /// Build a law for `setup`, or say why it cannot be built (the run then
    /// ends `law_fault` before any pulse).
    fn build(
        &self,
        setup: &ClassicalLawSetup<'_>,
    ) -> Result<Box<dyn ClassicalLaw + Send>, LawFault>;

    /// The law as the run's records name it.
    fn describe(&self) -> LawDescriptor;
}

/// When a failed tolerance verdict aborts the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToleranceAbort {
    /// Failures below full level alarm; a failure at full level aborts. The
    /// closed loop's default: early pulses are how the law learns.
    FullLevel,
    /// Every failure aborts, at every level (the open-loop behaviour).
    EveryLevel,
}

/// The pulse's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// The pulse as specified.
    Positive,
    /// The pulse negated: the core negates the reference and judges the
    /// sign-corrected record.
    Negative,
}

/// The closed-loop settings of a classical run: identification, the plant
/// inverse and its trust, the guards on the law, the caps, the drive ceiling
/// and abort, the abort rules and the review hold.
///
/// Carried in [`crate::ClassicalTestConfig::control`]. `None` there is the open
/// loop (the reference in g written as volts), kept only for the old daemon
/// host and the recorded trace; the control service always sets it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassicalControl {
    /// System identification: band-limited random at `drive_rms` volts rms,
    /// its peak capped by the drive ceiling; holds the run when the in-band
    /// mean coherence is under `min_mean_coherence`.
    pub sysid: SysIdConfig,
    /// The correction band's low edge, Hz.
    pub band_lo_hz: f64,
    /// The correction band's high edge, Hz.
    pub band_hi_hz: f64,
    /// The inverse's Tikhonov floor and the trust mask's coherence floor.
    pub inverse: InverseConfig,
    /// Reference dynamic-range floor, dB below its in-band spectral peak.
    pub ref_floor_db: f64,
    /// Fraction of the reference's energy the trusted bins must cover, or the
    /// run ends before any pulse (`reference_out_of_band`).
    pub min_band_energy: f64,
    /// Hold for the operator to review the identification before any pulse.
    pub review_hold: bool,
    /// The guards on calling the law: budget, misses, the run's pulse cap.
    pub guards: LawGuards,
    /// Per-pulse cap on the rise of the drive's peak at full level, dB.
    pub max_peak_rise_db: f64,
    /// Per-pulse cap on the rise of any drive bin's magnitude at full level, dB.
    pub max_bin_rise_db: f64,
    /// Bound on the seed's predicted response peak, times the reference peak.
    pub first_drive_ratio: f64,
    /// Drive ceiling, volts peak: a commanded record over it is scaled down to
    /// it (a logged drive-limit action).
    pub drive_vpk_ceiling: f64,
    /// Drive abort, volts peak: a commanded record over it is refused and the
    /// run aborts. Above the ceiling.
    pub drive_vpk_abort: f64,
    /// Overshoot abort: a measured peak over `(1 + this)` times the commanded
    /// peak aborts at any level. `None`: [`DEFAULT_OVERSHOOT_TOLERANCE_MULTIPLE`]
    /// times the standard's amplitude tolerance.
    pub overshoot_abort: Option<f64>,
    /// When a failed tolerance verdict aborts.
    pub tolerance_abort: ToleranceAbort,
    /// Pulses in a row, at one level, whose error rises by more than
    /// [`DIVERGENCE_RISE_DB`] before the run ends `divergence`.
    pub divergence_pulses: u32,
    /// The pulse's direction.
    pub polarity: Polarity,
}

impl ClassicalControl {
    /// The defaults at `fs_hz`, with the drive ceiling and abort the run must
    /// state: identification 0.05 V rms for 4 s over 5 Hz to
    /// min(5 kHz, 0.4 fs), the correction band the same, the inverse's and the
    /// guards' defaults, review hold on, tolerance failures aborting at full
    /// level, positive polarity.
    pub fn new(fs_hz: f64, drive_vpk_ceiling: f64, drive_vpk_abort: f64) -> Self {
        let band_hi_hz = DEFAULT_BAND_HI_HZ.min(DEFAULT_BAND_HI_FRACTION_OF_FS * fs_hz);
        ClassicalControl {
            sysid: SysIdConfig {
                drive_rms: DEFAULT_SYSID_DRIVE_VRMS,
                duration_s: DEFAULT_SYSID_DURATION_S,
                f_lo_hz: DEFAULT_BAND_LO_HZ,
                f_hi_hz: band_hi_hz,
                min_mean_coherence: DEFAULT_SYSID_MIN_MEAN_COHERENCE,
                ..SysIdConfig::default()
            },
            band_lo_hz: DEFAULT_BAND_LO_HZ,
            band_hi_hz,
            inverse: InverseConfig::default(),
            ref_floor_db: DEFAULT_REF_FLOOR_DB,
            min_band_energy: DEFAULT_MIN_BAND_ENERGY,
            review_hold: true,
            guards: LawGuards::default(),
            max_peak_rise_db: DEFAULT_MAX_PEAK_RISE_DB,
            max_bin_rise_db: DEFAULT_MAX_BIN_RISE_DB,
            first_drive_ratio: DEFAULT_FIRST_DRIVE_RATIO,
            drive_vpk_ceiling,
            drive_vpk_abort,
            overshoot_abort: None,
            tolerance_abort: ToleranceAbort::FullLevel,
            divergence_pulses: DEFAULT_DIVERGENCE_PULSES,
            polarity: Polarity::Positive,
        }
    }

    /// The overshoot abort in force under `standard`: the configured value, or
    /// twice the standard's amplitude tolerance.
    pub fn overshoot_abort_frac(&self, standard: ToleranceStandard) -> f64 {
        self.overshoot_abort.unwrap_or_else(|| {
            DEFAULT_OVERSHOOT_TOLERANCE_MULTIPLE * standard.params().amp_tol_frac_of_peak
        })
    }

    /// Check every field at the run's rate. The error names the field by its
    /// path under `control`.
    pub fn validate(&self, fs_hz: f64) -> Result<(), String> {
        self.sysid
            .validate(fs_hz)
            .map_err(|e| format!("control.identification: {e}"))?;
        self.inverse
            .validate()
            .map_err(|e| format!("control.inverse: {e}"))?;
        if !(self.band_lo_hz.is_finite()
            && self.band_hi_hz.is_finite()
            && self.band_lo_hz > 0.0
            && self.band_hi_hz > self.band_lo_hz
            && self.band_hi_hz < fs_hz / 2.0)
        {
            return Err(format!(
                "control.band_hz: [{}, {}] Hz is not a band below half the rate ({} Hz)",
                self.band_lo_hz,
                self.band_hi_hz,
                fs_hz / 2.0
            ));
        }
        positive("control.ref_floor_db", self.ref_floor_db)?;
        if !(self.min_band_energy.is_finite()
            && self.min_band_energy > 0.0
            && self.min_band_energy < 1.0)
        {
            return Err(format!(
                "control.min_band_energy must be above 0 and below 1 (got {})",
                self.min_band_energy
            ));
        }
        self.guards.validate().map_err(|e| format!("control.{e}"))?;
        positive("control.max_peak_rise_db", self.max_peak_rise_db)?;
        positive("control.max_bin_rise_db", self.max_bin_rise_db)?;
        if !(self.first_drive_ratio.is_finite() && self.first_drive_ratio >= 1.0) {
            return Err(format!(
                "control.first_drive_ratio must be at least 1 (got {})",
                self.first_drive_ratio
            ));
        }
        positive("limits.drive_vpk_ceiling", self.drive_vpk_ceiling)?;
        if !(self.drive_vpk_abort.is_finite() && self.drive_vpk_abort > self.drive_vpk_ceiling) {
            return Err(format!(
                "limits.drive_vpk_abort must be above the drive ceiling of {} V (got {})",
                self.drive_vpk_ceiling, self.drive_vpk_abort
            ));
        }
        if let Some(o) = self.overshoot_abort {
            positive("standard.overshoot_abort", o)?;
        }
        if self.divergence_pulses == 0 {
            return Err("control.divergence_pulses must be at least 1".into());
        }
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
    fn the_defaults_validate_and_follow_the_rate() {
        let c = ClassicalControl::new(51_200.0, 0.5, 0.8);
        c.validate(51_200.0).unwrap();
        assert_eq!((c.band_lo_hz, c.band_hi_hz), (5.0, 5000.0));
        let c = ClassicalControl::new(5120.0, 0.5, 0.8);
        assert_eq!(c.band_hi_hz, 0.4 * 5120.0);
        assert_eq!(c.sysid.f_hi_hz, c.band_hi_hz);
        c.validate(5120.0).unwrap();
    }

    #[test]
    fn each_bad_field_is_named() {
        let fs = 51_200.0;
        let base = ClassicalControl::new(fs, 0.5, 0.8);
        let cases: Vec<(ClassicalControl, &str)> = vec![
            (
                ClassicalControl {
                    drive_vpk_ceiling: 0.0,
                    ..base.clone()
                },
                "drive_vpk_ceiling",
            ),
            (
                ClassicalControl {
                    drive_vpk_abort: 0.5,
                    ..base.clone()
                },
                "drive_vpk_abort",
            ),
            (
                ClassicalControl {
                    band_hi_hz: fs,
                    ..base.clone()
                },
                "band_hz",
            ),
            (
                ClassicalControl {
                    min_band_energy: 1.0,
                    ..base.clone()
                },
                "min_band_energy",
            ),
            (
                ClassicalControl {
                    first_drive_ratio: 0.5,
                    ..base.clone()
                },
                "first_drive_ratio",
            ),
            (
                ClassicalControl {
                    max_bin_rise_db: 0.0,
                    ..base.clone()
                },
                "max_bin_rise_db",
            ),
            (
                ClassicalControl {
                    divergence_pulses: 0,
                    ..base.clone()
                },
                "divergence_pulses",
            ),
            (
                ClassicalControl {
                    overshoot_abort: Some(-0.1),
                    ..base.clone()
                },
                "overshoot_abort",
            ),
        ];
        for (c, field) in cases {
            let e = c.validate(fs).unwrap_err();
            assert!(e.contains(field), "{field}: {e}");
        }
        let mut c = base.clone();
        c.guards.max_misses = 0;
        assert!(c.validate(fs).unwrap_err().contains("control.law_misses"));
        let mut c = base;
        c.sysid.drive_rms = 0.0;
        assert!(c
            .validate(fs)
            .unwrap_err()
            .contains("control.identification"));
    }

    #[test]
    fn the_overshoot_default_is_twice_the_standards_amplitude_tolerance() {
        let c = ClassicalControl::new(51_200.0, 0.5, 0.8);
        assert!((c.overshoot_abort_frac(ToleranceStandard::Mil810_516) - 0.30).abs() < 1e-12);
        assert!((c.overshoot_abort_frac(ToleranceStandard::Iec60068_2_27) - 0.40).abs() < 1e-12);
        let c = ClassicalControl {
            overshoot_abort: Some(0.25),
            ..c
        };
        assert_eq!(c.overshoot_abort_frac(ToleranceStandard::Mil810_516), 0.25);
    }

    #[test]
    fn control_round_trips_through_json() {
        let c = ClassicalControl::new(51_200.0, 0.5, 0.8);
        let back: ClassicalControl =
            serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
