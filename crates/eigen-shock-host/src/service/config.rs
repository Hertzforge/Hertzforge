//! The classical-shock test configuration: [`ClassicalTestConfig`] and its parts
//! (pulse spec, level schedule, tolerance standard, feasibility inputs, drive
//! limits, pulse-train / gunfire settings, SRS analysis grid, capture rules,
//! metadata) - mirroring the eigen-random service config conventions.
//!
//! # Scope-of-config vs identity (the one line to hold)
//!
//! The `eigen_channel_table::ChannelTable` carries channel IDENTITY (index, EU,
//! sensitivity, role). The TEST DEFINITION - the pulse spec, the level schedule,
//! the tolerance standard, the drive-limit CEILINGS (ceilings are
//! test scope, not channel identity), the train/gunfire settings - lives HERE.

use crate::spec::ClassicalPulseSpec;
use crate::tolerance::ToleranceStandard;
use eigen_shaker_db::{MovingMass, ShakerRecord};
use eigen_shock_common::{
    DriveLimit, FeasibilityTolerances, RateModel, ShockLevelSchedule, TrainSpec,
};
use serde::{Deserialize, Serialize};

/// The default sample rate the reference is synthesized (and the Sim plant run)
/// at: the decoded Shock config rate, 5120 Hz (cap_shock f309 "SR 5120").
pub const DEFAULT_FS_HZ: f64 = 5120.0;

/// Default SRS analysis grid low edge, Hz (every pulse's SRS).
pub const DEFAULT_SRS_F_LO_HZ: f64 = 20.0;

/// Default SRS analysis grid high edge, Hz (below Nyquist at the default rate).
pub const DEFAULT_SRS_F_HI_HZ: f64 = 2000.0;

/// Default SRS band density: `N` in the geometric ratio `2^(1/N)` (third-octave,
/// the classic SRS default - `eigen_dsp::srs::OctaveFraction::Third`).
pub const DEFAULT_SRS_BANDS_PER_OCTAVE: f64 = 3.0;

/// Default per-band SRS fidelity tolerance, dB (see
/// [`ClassicalTestConfig::srs_band_tol_db`]): the measured pulse SRS vs the
/// COMMANDED drive SRS. Informational - the time-domain tolerance checks are the
/// classical acceptance; this is the "SRS pass/fail per band" report column.
pub const DEFAULT_SRS_BAND_TOL_DB: f64 = 3.0;

/// One user-defined metadata field (mirroring eigen-random's
/// `MetaField` shape). Seeded defaults are Run Number / Test Article / SN /
/// Operator; the set is expandable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaField {
    /// Stable key (machine name), e.g. `run_number`.
    pub key: String,
    /// Human label, e.g. `Run Number`.
    pub label: String,
    /// The value (freeform text; the UI supplies typed editors).
    pub value: String,
}

impl MetaField {
    pub fn new(key: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> Self {
        MetaField {
            key: key.into(),
            label: label.into(),
            value: value.into(),
        }
    }

    /// The seeded default metadata set: Run Number, Test Article, SN, Operator,
    /// all empty (same seed as the Random service, common infra).
    pub fn defaults() -> Vec<MetaField> {
        vec![
            MetaField::new("run_number", "Run Number", ""),
            MetaField::new("test_article", "Test Article", ""),
            MetaField::new("serial_number", "SN", ""),
            MetaField::new("operator", "Operator", ""),
        ]
    }
}

/// Gunfire settings (MIL-STD-810 Method 519.8 Procedure IV): a
/// repeated-pulse burst at a rounds-per-minute rate. The inter-pulse spacing is
/// `60 / rounds_per_min` seconds via `eigen_shock_common::gunfire_spacing`; the
/// rate-model floor still applies (a rate below the floors is REFUSED).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GunfireSpec {
    /// Firing rate, rounds per minute (> 0).
    pub rounds_per_min: f64,
    /// Rounds per burst (>= 1).
    pub rounds: u32,
}

/// How the service captures fired pulses. The per-pulse time history
/// is ALWAYS ingested into the in-memory `PulseStore`; these rules govern the
/// durable eigen-testkit capture sink.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRules {
    /// Auto-start the capture sink at arm (each measured pulse's control-channel
    /// record is streamed to `telemetry.f32`).
    pub auto_start: bool,
    /// The capture run name (folder under the data root).
    pub run_name: String,
}

impl Default for CaptureRules {
    fn default() -> Self {
        CaptureRules {
            auto_start: false,
            run_name: "classical_shock".into(),
        }
    }
}

/// The full classical-shock test configuration.
/// Everything [`crate::service::ClassicalShockService`] needs to run a classical
/// pulse test. Fully serde-serializable: this struct IS the report's settings
/// record and the capture sidecar's settings JSON (no digest mirror to drift).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassicalTestConfig {
    /// The classical pulse: shape + amplitude + duration + Path-B compensation
    /// config (compensation is host-side on Path B ONLY; the
    /// delegated path downloads the uncompensated main pulse).
    pub spec: ClassicalPulseSpec,
    /// Synthesis / plant sample rate, Hz (default [`DEFAULT_FS_HZ`]).
    pub fs_hz: f64,
    /// Which standard's tolerance parameters the per-pulse tolerance checks apply.
    pub standard: ToleranceStandard,
    /// The level schedule: default AUTO `[-12, -9, -6, -3, 0]` dB,
    /// N pulses per level; MANUAL steps +/- 1 dB / jumps table levels.
    pub schedule: ShockLevelSchedule,
    /// The pulse-train spec: `count` pulses at a fixed delay or max-rate. Fired
    /// via `fire_train`; the hardware-blocked marker travels on the plan.
    pub train: TrainSpec,
    /// Gunfire settings (Procedure IV), fired via `fire_gunfire_burst`. `None` =
    /// no gunfire configured.
    pub gunfire: Option<GunfireSpec>,
    /// The rate model whose floors set the minimum legal inter-pulse spacing.
    /// The conservative default's floors are PLACEHOLDERS pending a hardware
    /// characterization.
    pub rate_model: RateModel,
    /// Drive peak limiting: the predictive peak-accel
    /// ceiling + between-pulse monitor channels. Ceilings live in TEST config
    /// (identity-vs-test scope line).
    pub drive_limit: DriveLimit,
    /// The shaker to assess feasibility against. `None` = feasibility
    /// NOT evaluated (logged; arming allowed without a shaker-limit block).
    pub shaker: Option<ShakerRecord>,
    /// The moving-mass stack for the feasibility force deration.
    pub moving_mass: MovingMass,
    /// Compensation-quality residual-velocity tolerance, in/s (the compensation gate).
    pub residual_velocity_tol_ips: f64,
    /// Compensation-quality residual-displacement tolerance, in.
    pub residual_disp_tol_in: f64,
    /// SRS analysis grid low edge, Hz.
    pub srs_f_lo_hz: f64,
    /// SRS analysis grid high edge, Hz.
    pub srs_f_hi_hz: f64,
    /// SRS band density: `N` of the geometric ratio `2^(1/N)` (3 = third-octave).
    pub srs_bands_per_octave: f64,
    /// Per-band SRS fidelity tolerance, dB: measured pulse SRS vs the COMMANDED
    /// drive SRS within +/- this. INFORMATIONAL (the report's per-band pass/fail
    /// column); the time-domain tolerance checks are the classical acceptance.
    pub srs_band_tol_db: f64,
    /// Also compute the pseudo-velocity SRS per pulse.
    pub compute_pvss: bool,
    /// Abort the run on a failed tolerance report (the tolerance abort). When
    /// `false` a breach raises the alarm indicator + an event and firing
    /// continues (alarm-only posture).
    pub abort_on_tolerance_failure: bool,
    /// Durable capture rules.
    pub capture: CaptureRules,
    /// User-defined metadata.
    pub metadata: Vec<MetaField>,
}

impl ClassicalTestConfig {
    /// A config for `spec` with the defaults: 5120 Hz, MIL-STD-810 516
    /// tolerance, the default AUTO ladder, a single-pulse "train", no gunfire,
    /// the conservative rate model, no drive ceilings, no shaker (feasibility
    /// not evaluated), third-octave SRS 20-2000 Hz, abort-on-tolerance.
    pub fn new(spec: ClassicalPulseSpec) -> Self {
        let tol = FeasibilityTolerances::default();
        ClassicalTestConfig {
            spec,
            fs_hz: DEFAULT_FS_HZ,
            standard: ToleranceStandard::Mil810_516,
            schedule: ShockLevelSchedule::auto_default(),
            train: TrainSpec::single(),
            gunfire: None,
            rate_model: RateModel::conservative_default(),
            drive_limit: DriveLimit {
                peak_accel_g: None,
                monitors: Vec::new(),
            },
            shaker: None,
            moving_mass: MovingMass::default(),
            residual_velocity_tol_ips: tol.residual_velocity_ips,
            residual_disp_tol_in: tol.residual_disp_in,
            srs_f_lo_hz: DEFAULT_SRS_F_LO_HZ,
            srs_f_hi_hz: DEFAULT_SRS_F_HI_HZ,
            srs_bands_per_octave: DEFAULT_SRS_BANDS_PER_OCTAVE,
            srs_band_tol_db: DEFAULT_SRS_BAND_TOL_DB,
            compute_pvss: false,
            abort_on_tolerance_failure: true,
            capture: CaptureRules::default(),
            metadata: MetaField::defaults(),
        }
    }

    /// The [`FeasibilityTolerances`] built from the config's residual fields
    /// (the eigen-shock-common struct is not serde, so the config carries the
    /// two scalars and rebuilds it here).
    pub fn feasibility_tolerances(&self) -> FeasibilityTolerances {
        FeasibilityTolerances {
            residual_velocity_ips: self.residual_velocity_tol_ips,
            residual_disp_in: self.residual_disp_tol_in,
        }
    }

    /// Validate the config up front (a run must not start on a bad config).
    /// Returns the FIRST error.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.spec
            .validate()
            .map_err(|e| ConfigError::BadSpec(e.to_string()))?;
        if !self.fs_hz.is_finite() || self.fs_hz <= 0.0 {
            return Err(ConfigError::BadConfig(format!(
                "fs_hz must be finite and > 0 (got {})",
                self.fs_hz
            )));
        }
        self.schedule
            .validate()
            .map_err(|e| ConfigError::BadSchedule(e.to_string()))?;
        self.drive_limit
            .validate()
            .map_err(|e| ConfigError::BadDriveLimit(e.to_string()))?;
        if self.train.count == 0 {
            return Err(ConfigError::BadConfig(
                "train.count must be >= 1 (an empty train cannot fire)".into(),
            ));
        }
        if let Some(g) = self.gunfire {
            if !g.rounds_per_min.is_finite() || g.rounds_per_min <= 0.0 {
                return Err(ConfigError::BadConfig(format!(
                    "gunfire.rounds_per_min must be finite and > 0 (got {})",
                    g.rounds_per_min
                )));
            }
            if g.rounds == 0 {
                return Err(ConfigError::BadConfig("gunfire.rounds must be >= 1".into()));
            }
        }
        if !self.srs_f_lo_hz.is_finite() || self.srs_f_lo_hz <= 0.0 {
            return Err(ConfigError::BadConfig(format!(
                "srs_f_lo_hz must be finite and > 0 (got {})",
                self.srs_f_lo_hz
            )));
        }
        if !self.srs_f_hi_hz.is_finite() || self.srs_f_hi_hz < self.srs_f_lo_hz {
            return Err(ConfigError::BadConfig(format!(
                "srs_f_hi_hz must be finite and >= srs_f_lo_hz (got {})",
                self.srs_f_hi_hz
            )));
        }
        if !self.srs_bands_per_octave.is_finite() || self.srs_bands_per_octave <= 0.0 {
            return Err(ConfigError::BadConfig(format!(
                "srs_bands_per_octave must be finite and > 0 (got {})",
                self.srs_bands_per_octave
            )));
        }
        if !self.srs_band_tol_db.is_finite() || self.srs_band_tol_db <= 0.0 {
            return Err(ConfigError::BadConfig(format!(
                "srs_band_tol_db must be finite and > 0 (got {})",
                self.srs_band_tol_db
            )));
        }
        if !self.residual_velocity_tol_ips.is_finite() || self.residual_velocity_tol_ips <= 0.0 {
            return Err(ConfigError::BadConfig(format!(
                "residual_velocity_tol_ips must be finite and > 0 (got {})",
                self.residual_velocity_tol_ips
            )));
        }
        if !self.residual_disp_tol_in.is_finite() || self.residual_disp_tol_in <= 0.0 {
            return Err(ConfigError::BadConfig(format!(
                "residual_disp_tol_in must be finite and > 0 (got {})",
                self.residual_disp_tol_in
            )));
        }
        Ok(())
    }
}

/// Configuration-validation errors surfaced before a run starts (never mid-run).
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// The pulse spec was invalid. Carries the spec error text.
    BadSpec(String),
    /// The level schedule was invalid. Carries the ladder error text.
    BadSchedule(String),
    /// The drive-limit configuration was invalid.
    BadDriveLimit(String),
    /// A knob was out of range. Carries the knob name + note.
    BadConfig(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::BadSpec(s) => write!(f, "classical test config: pulse spec: {s}"),
            ConfigError::BadSchedule(s) => write!(f, "classical test config: schedule: {s}"),
            ConfigError::BadDriveLimit(s) => write!(f, "classical test config: drive limit: {s}"),
            ConfigError::BadConfig(s) => write!(f, "classical test config: {s}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ClassicalShape;

    fn cfg() -> ClassicalTestConfig {
        ClassicalTestConfig::new(ClassicalPulseSpec::new(
            ClassicalShape::HalfSine,
            20.0,
            11.0,
        ))
    }

    #[test]
    fn default_config_validates() {
        cfg().validate().unwrap();
    }

    #[test]
    fn validate_rejects_bad_knobs() {
        let mut c = cfg();
        c.fs_hz = 0.0;
        assert!(matches!(c.validate(), Err(ConfigError::BadConfig(_))));

        let mut c = cfg();
        c.spec.amplitude_g = -1.0;
        assert!(matches!(c.validate(), Err(ConfigError::BadSpec(_))));

        let mut c = cfg();
        c.schedule.table_db = Vec::new();
        assert!(matches!(c.validate(), Err(ConfigError::BadSchedule(_))));

        let mut c = cfg();
        c.drive_limit.peak_accel_g = Some(0.0);
        assert!(matches!(c.validate(), Err(ConfigError::BadDriveLimit(_))));

        let mut c = cfg();
        c.train.count = 0;
        assert!(c.validate().is_err());

        let mut c = cfg();
        c.gunfire = Some(GunfireSpec {
            rounds_per_min: 0.0,
            rounds: 3,
        });
        assert!(c.validate().is_err());

        let mut c = cfg();
        c.srs_f_hi_hz = 1.0; // below f_lo
        assert!(c.validate().is_err());

        let mut c = cfg();
        c.residual_velocity_tol_ips = 0.0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_round_trips_through_json() {
        let c = cfg();
        let json = serde_json::to_string(&c).unwrap();
        let back: ClassicalTestConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn metadata_defaults_seed_the_expected_fields() {
        let m = MetaField::defaults();
        let keys: Vec<&str> = m.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            ["run_number", "test_article", "serial_number", "operator"]
        );
    }

    #[test]
    fn feasibility_tolerances_rebuild_from_the_scalars() {
        let mut c = cfg();
        c.residual_velocity_tol_ips = 0.2;
        c.residual_disp_tol_in = 0.005;
        let t = c.feasibility_tolerances();
        assert_eq!(t.residual_velocity_ips, 0.2);
        assert_eq!(t.residual_disp_in, 0.005);
    }
}
