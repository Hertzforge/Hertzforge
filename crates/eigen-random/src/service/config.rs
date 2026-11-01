//! The Random run configuration: [`RandomTestConfig`] and its parts (level schedule,
//! control-channel set, per-channel limit profiles, protection thresholds, capture
//! rules, drive ceiling/abort, metadata).
//!
//! # Scope-of-config vs identity (the one line to hold)
//!
//! The [`eigen_channel_table::ChannelTable`] carries channel IDENTITY (index, EU,
//! sensitivity, role, tri-field abort limits). The TEST DEFINITION - which control
//! channels compose, their weights + [`crate::control::CompositeMode`], the
//! response/force notch profiles, the alarm/abort envelope dB, the level schedule -
//! lives HERE (weights are test-definition, not the channel table). The tri-field
//! kinematic abort still reads the channel table's abort limits (that is per-channel
//! identity); everything else is per-test config.

use crate::control::{CompositeMode, HinvConfig};
use eigen_breakpoints::BreakpointTable;
use eigen_shaker_db::{ArticleGeometry, MovingMass, ShakerRecord};
use serde::{Deserialize, Serialize};

/// The default sysID level: -20 dB = 1/10 AMPLITUDE of full. Kept
/// conservative; a config knob. (Amplitude-vs-power reading is an open review item;
/// the default is the amplitude reading.)
pub const DEFAULT_SYSID_LEVEL_DB: f64 = -20.0;

/// The default START level once the loop engages: -12 dB.
pub const DEFAULT_START_LEVEL_DB: f64 = -12.0;

/// The default AUTO level table: 3 dB steps from START to full.
pub const DEFAULT_LEVEL_TABLE_DB: [f64; 5] = [-12.0, -9.0, -6.0, -3.0, 0.0];

/// The default per-channel overload proxy fraction: `|eu| >= 0.9 *
/// range_max` latches the overload indicator.
pub const DEFAULT_OVERLOAD_FRACTION: f64 = 0.9;

/// The default sysID band-coherence floor: below this the coherence gate
/// HOLDs and never engages.
pub const DEFAULT_COHERENCE_FLOOR: f64 = 0.5;

/// The default level-entry settling deadband for capture debounce: 2 s.
pub const DEFAULT_LEVEL_SETTLE_S: f64 = 2.0;

/// One user-defined metadata field. Seeded defaults
/// are Run Number / Test Article / SN / Operator; the set is expandable.
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

    /// The seeded default metadata set: Run Number, Test Article, SN,
    /// Operator, all empty. The UI adds/edits/removes fields freely.
    pub fn defaults() -> Vec<MetaField> {
        vec![
            MetaField::new("run_number", "Run Number", ""),
            MetaField::new("test_article", "Test Article", ""),
            MetaField::new("serial_number", "SN", ""),
            MetaField::new("operator", "Operator", ""),
        ]
    }
}

/// How the run advances through levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunMode {
    /// AUTO: walk the level table with per-level dwell, then hold full for the test
    /// duration, then a scheduled clean stop.
    Auto,
    /// MANUAL: operator steps levels with `+/- 1 dB` / jump-to-table-level; no
    /// automatic progression.
    Manual,
}

/// The level schedule. All levels are dB relative to full (0 dB = full
/// shaped reference). AUTO walks `table_db` with `dwell_s` per level; MANUAL uses only
/// `start_db` and steps from there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelSchedule {
    pub mode: RunMode,
    /// The engage/start level (default -12 dB).
    pub start_db: f64,
    /// The AUTO level table in dB (default `[-12,-9,-6,-3,0]`). Must be non-empty,
    /// non-decreasing, and end at the full level (0 dB) for AUTO.
    pub table_db: Vec<f64>,
    /// Per-level dwell in seconds for AUTO (one entry per table level, or a single
    /// value applied to every level if length 1).
    pub dwell_s: Vec<f64>,
}

impl LevelSchedule {
    /// The default AUTO schedule: `[-12,-9,-6,-3,0]` dB, 3 s dwell each.
    pub fn auto_default() -> Self {
        LevelSchedule {
            mode: RunMode::Auto,
            start_db: DEFAULT_START_LEVEL_DB,
            table_db: DEFAULT_LEVEL_TABLE_DB.to_vec(),
            dwell_s: vec![3.0],
        }
    }

    /// A MANUAL schedule starting at `start_db` (default -12 dB).
    pub fn manual_default() -> Self {
        LevelSchedule {
            mode: RunMode::Manual,
            start_db: DEFAULT_START_LEVEL_DB,
            table_db: DEFAULT_LEVEL_TABLE_DB.to_vec(),
            dwell_s: vec![3.0],
        }
    }

    /// The dwell (s) for table level `i` (single-entry `dwell_s` applies to all).
    pub fn dwell_for(&self, i: usize) -> f64 {
        if self.dwell_s.is_empty() {
            0.0
        } else if self.dwell_s.len() == 1 {
            self.dwell_s[0]
        } else {
            self.dwell_s
                .get(i)
                .copied()
                .unwrap_or(*self.dwell_s.last().unwrap())
        }
    }

    /// The full level (last table entry, or 0 dB if the table is empty).
    pub fn full_db(&self) -> f64 {
        self.table_db.last().copied().unwrap_or(0.0)
    }
}

/// A per-channel limit profile mapped through the plant into a drive-PSD ceiling
/// (the notch seam). A RESPONSE profile is a per-line ceiling in the channel's EU
/// (`g^2/Hz` for accel, a force spec for a force channel); the
/// [`crate::control::NotchSet`] divides it by `|H|^2` into a drive ceiling. Force
/// limiting is the SAME mechanism (PREDICTIVE / notch only, never a reactive
/// force-abort axis).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitProfile {
    /// The channel index this profile limits (must be a control/monitor/force channel).
    pub channel_index: u32,
    /// Whether this is a response (accel) or a force limit. Both notch identically; the
    /// kind is carried for the report / UI label.
    pub kind: LimitKind,
    /// Per-line RESPONSE ceiling in the channel's EU domain (`g^2/Hz` or force^2/Hz),
    /// on the SAME line grid as the control target. `f64::INFINITY` = no limit at a
    /// line.
    pub resp_ceiling: Vec<f64>,
}

/// What a [`LimitProfile`] protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LimitKind {
    /// A response (acceleration) limit on a control/monitor channel.
    Response,
    /// A force limit on a force channel (predictive notch only).
    Force,
}

/// One control channel's membership in the composite: its channel index
/// and relative weight. The [`CompositeMode`] applies across the whole set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ControlChannel {
    pub channel_index: u32,
    pub weight: f64,
}

/// The control-channel set: the members + how they compose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlSet {
    pub channels: Vec<ControlChannel>,
    pub mode: CompositeMode,
}

impl ControlSet {
    /// A single-channel SISO control set on `channel_index` (weight 1, WeightedAvg -
    /// degenerate for one channel).
    pub fn single(channel_index: u32) -> Self {
        ControlSet {
            channels: vec![ControlChannel {
                channel_index,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        }
    }

    /// The member channel indices, in order.
    pub fn indices(&self) -> Vec<u32> {
        self.channels.iter().map(|c| c.channel_index).collect()
    }

    /// The member weights, in order.
    pub fn weights(&self) -> Vec<f64> {
        self.channels.iter().map(|c| c.weight).collect()
    }
}

/// How time-history capture is triggered.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CaptureTrigger {
    /// Capture is off.
    Off,
    /// Stream the ENTIRE test to one capture file.
    EntireTest,
    /// Operator-toggled capture (start/stop from the UI). The service exposes
    /// `capture_start`/`capture_stop`; nothing automatic fires.
    Manual,
    /// AUTO: capture the first `n_s` seconds after EACH debounced level-entry. A new
    /// level entry inside an active window RESTARTS it.
    AutoFirstSeconds { n_s: f64 },
}

/// The capture rules for a run (capture trigger + periodic save).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRules {
    pub trigger: CaptureTrigger,
    /// Level-entry settling deadband (s) before the AUTO window opens (the debounce).
    pub level_settle_s: f64,
    /// Periodic snapshot auto-save interval (s). `None` = no periodic save.
    pub periodic_save_s: Option<f64>,
}

impl Default for CaptureRules {
    fn default() -> Self {
        CaptureRules {
            trigger: CaptureTrigger::Off,
            level_settle_s: DEFAULT_LEVEL_SETTLE_S,
            periodic_save_s: None,
        }
    }
}

/// The full Random test configuration. Everything
/// the service needs to run a SISO/multi-point random test on Path B.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RandomTestConfig {
    /// The reference breakpoint table. The service interpolates it onto the
    /// line grid.
    pub breakpoints: BreakpointTable,
    /// Line resolution: bin width (Hz). Together with the plant sample rate + nfft this
    /// pins the line grid; the service derives the grid from the engine's nfft/fs and
    /// interpolates the profile onto it. `df_hz` here is the REQUESTED resolution
    /// (recorded in the report); the realized `df` is `fs/nfft`.
    pub df_hz: f64,
    /// Requested number of control lines (recorded; realized is `nfft/2+1` in-band).
    pub lines: usize,
    /// The control-channel set.
    pub control: ControlSet,
    /// Per-channel response/force limit profiles (predictive notch).
    pub limit_profiles: Vec<LimitProfile>,
    /// Global alarm band `(minus_db, plus_db)` for the frequency-domain envelope.
    /// Per-segment overrides ride on the breakpoint table rows.
    pub alarm_db: (f64, f64),
    /// Global abort band `(minus_db, plus_db)` for the envelope.
    pub abort_db: (f64, f64),
    /// The level schedule.
    pub schedule: LevelSchedule,
    /// The capture rules.
    pub capture: CaptureRules,
    /// Drive ceiling in Vrms. A breach CLAMPS the synthesized drive; a hard breach
    /// beyond [`Self::drive_abort_vrms`] ABORTS (real on Sim / direct-DAC).
    pub drive_ceiling_vrms: f64,
    /// Drive-Vrms ABORT threshold. A realized drive Vrms at or above this stops the
    /// run (Aborted). `None` = clamp-only, no drive abort.
    pub drive_abort_vrms: Option<f64>,
    /// The control-law knobs (passed through to the law).
    pub hinv: HinvConfig,
    /// Consecutive loops with a non-empty divergent-bin set before the divergence abort
    /// fires (the caller side of the divergence contract). The service owns this
    /// count; the law owns per-bin divergence.
    pub divergence_loops: u32,
    /// Abort-envelope breaches TOLERATED per loop before the envelope abort fires
    /// (operator's "max lines out of tolerance", 2026-08-02 spec; default 10).
    ///
    /// Why not 1: a Welch line estimated from nAvg blocks carries chi-squared
    /// scatter of several dB at small nAvg. Measured on the jumpered cDAQ
    /// 2026-08-03 at nAvg 4: 1-3 of ~1000 in-band lines statistically breach a
    /// -6 dB abort floor on a PERFECT wire, killing every run at loop 4, while
    /// nAvg 12 (~sqrt(3) less scatter) never breached. One noisy line is
    /// estimator variance; max_lines_out of them at once is a plant fault.
    /// Breaches below the threshold still surface as the ALARM indicator.
    pub max_lines_out: usize,
    /// Per-channel overload proxy fraction: `|eu| >= fraction * range_max`
    /// latches the overload indicator. Default 0.9.
    pub overload_fraction: f64,
    /// sysID drive level in dB (default -20 dB).
    pub sysid_level_db: f64,
    /// Band coherence floor for the coherence gate (default 0.5).
    pub coherence_floor: f64,
    /// Test duration at full level (s) before the scheduled AUTO stop.
    pub full_level_duration_s: f64,
    /// Feasibility inputs: the selected shaker + the moving-mass stack + optional
    /// article geometry. `None` shaker => feasibility is skipped (Advisory note in
    /// the report).
    pub shaker: Option<ShakerRecord>,
    pub moving_mass: MovingMass,
    pub geometry: Option<ArticleGeometry>,
    /// Crest factor for the feasibility RMS->peak conversion.
    pub crest_factor: f64,
    /// Drive sigma clip (operator "max sigma"): synthesized drive samples are
    /// clamped to `+/- sigma x segment RMS`. `None` = no clip. Bounds the crest
    /// factor delivered to the amplifier at the cost of mild spectral
    /// distortion at the clip level - the standard random-controller knob.
    pub drive_clip_sigma: Option<f64>,
    /// Startup / level-change ramp, seconds per TRANSITION (operator knob).
    /// `0` = instantaneous steps (the core default; the UI sends its own).
    /// While ramping, the commanded level WALKS toward the target in
    /// per-segment steps - smooth by construction, never stepwise (a step
    /// into an armature is a broadband impulse; standing rule).
    pub level_ramp_s: f64,
    /// Controlled-stop ramp, seconds (operator knob; NOT the abort taper -
    /// aborts stay immediate). `0` = stop without a level ramp-down.
    pub shutdown_ramp_s: f64,
    /// Pretest drive ceiling, volts RMS (`None` = unbounded).
    pub pretest_max_vrms: Option<f64>,
    /// Pretest drive floor, volts RMS (`None` = unbounded).
    pub pretest_min_vrms: Option<f64>,
    /// Shape the pretest PSD like the reference instead of flat-in-band: a
    /// resonant article is excited where the test will actually drive it.
    pub shaped_pretest: bool,
    /// SOFT-START depth, dB below the lowest scheduled level (operator, 2026-08-03;
    /// default 9). The run engages this far down and ramps UP to the start
    /// level, so a big article - a slip table especially - sees the loop take
    /// hold gently instead of arriving at level. `0` disables the soft start.
    pub soft_start_db: f64,
    /// Envelope enforcement RELAXATION while below the start level, dB added to
    /// both the alarm and abort bands (operator, 2026-08-03; default 6).
    ///
    /// A big fixture genuinely takes time to settle at low level, and the
    /// envelope is authored for the TEST level: judging a still-settling
    /// soft-start climb against it aborts good runs. Relaxed, not disabled -
    /// a gross exceedance on the way up still trips, and every ABSOLUTE
    /// protection (kinematic g, drive Vrms, overload) is untouched throughout.
    pub soft_start_relax_db: f64,
    /// Statistical DEGREES OF FREEDOM the control estimate targets - how
    /// commercial random controllers state their smoothing (2026-08-03).
    ///
    /// A Welch line estimated from `N` averages scatters with
    /// `sigma_dB ~ 10*log10(1 + 1/sqrt(N))`, and `DOF = 2N`. At the shipped
    /// nAvg 4 the RAW per-loop line is +/-3.5 dB at 95% - the "wild variation"
    /// the operator reported on a perfect jumper, and NOT a control error.
    /// The service reaches this DOF by accumulating across loops, so a fast
    /// loop and a steady estimate are not in conflict.
    pub control_dof: f64,
    /// User-defined metadata.
    pub metadata: Vec<MetaField>,
}

impl RandomTestConfig {
    /// A single-control-channel config on `control_index` against `breakpoints`, with
    /// conservative defaults for everything else. The caller sets the shaker + mass for
    /// feasibility and the capture/schedule as needed.
    pub fn siso(breakpoints: BreakpointTable, control_index: u32) -> Self {
        RandomTestConfig {
            breakpoints,
            df_hz: 8.0,
            lines: 512,
            control: ControlSet::single(control_index),
            limit_profiles: Vec::new(),
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
            schedule: LevelSchedule::auto_default(),
            capture: CaptureRules::default(),
            drive_ceiling_vrms: 10.0,
            drive_abort_vrms: None,
            hinv: HinvConfig::default(),
            divergence_loops: 5,
            max_lines_out: 10,
            drive_clip_sigma: None,
            level_ramp_s: 0.0,
            shutdown_ramp_s: 0.0,
            pretest_max_vrms: None,
            pretest_min_vrms: None,
            shaped_pretest: false,
            soft_start_db: 9.0,
            soft_start_relax_db: 6.0,
            control_dof: 120.0,
            overload_fraction: DEFAULT_OVERLOAD_FRACTION,
            sysid_level_db: DEFAULT_SYSID_LEVEL_DB,
            coherence_floor: DEFAULT_COHERENCE_FLOOR,
            full_level_duration_s: 10.0,
            shaker: None,
            moving_mass: MovingMass::default(),
            geometry: None,
            crest_factor: eigen_shaker_db::DEFAULT_CREST_FACTOR,
            metadata: MetaField::defaults(),
        }
    }
}

/// Configuration-validation errors surfaced before a run starts (never mid-run).
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// A knob was out of range. Carries the knob name + note.
    BadConfig(String),
    /// The breakpoint table did not resolve. Carries the breakpoints error text.
    BadBreakpoints(String),
    /// The control-law config was invalid. Carries the control error text.
    BadControl(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::BadConfig(s) => write!(f, "random test config error: {s}"),
            ConfigError::BadBreakpoints(s) => write!(f, "breakpoint table error: {s}"),
            ConfigError::BadControl(s) => write!(f, "control law config error: {s}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl RandomTestConfig {
    /// Validate the config up front. Checks the control-law knobs, the breakpoint
    /// table, the level schedule invariants, and the protection thresholds. Returns the
    /// FIRST error (a run must not start on a bad config).
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Control law.
        self.hinv
            .validate()
            .map_err(|e| ConfigError::BadControl(e.to_string()))?;

        // Breakpoints resolve (so interpolation cannot fail mid-start).
        self.breakpoints
            .validate()
            .map_err(|e| ConfigError::BadBreakpoints(e.to_string()))?;

        if self.control.channels.is_empty() {
            return Err(ConfigError::BadConfig("no control channels".into()));
        }
        let wsum: f64 = self.control.weights().iter().sum();
        if wsum <= 0.0 || wsum.is_nan() {
            return Err(ConfigError::BadConfig(
                "control weights must sum to a positive value".into(),
            ));
        }

        // Level schedule.
        let sch = &self.schedule;
        if sch.table_db.is_empty() {
            return Err(ConfigError::BadConfig("level table is empty".into()));
        }
        if sch.table_db.iter().any(|v| v.is_nan()) {
            return Err(ConfigError::BadConfig("level table has NaN".into()));
        }
        for w in sch.table_db.windows(2) {
            if w[1] < w[0] {
                return Err(ConfigError::BadConfig(
                    "level table must be non-decreasing".into(),
                ));
            }
        }
        if matches!(sch.mode, RunMode::Auto) && sch.dwell_s.iter().any(|d| *d < 0.0 || d.is_nan()) {
            return Err(ConfigError::BadConfig("AUTO dwell must be >= 0".into()));
        }

        // Protection thresholds.
        if !(0.0..=1.0).contains(&self.overload_fraction) || self.overload_fraction.is_nan() {
            return Err(ConfigError::BadConfig(
                "overload_fraction must be in [0,1]".into(),
            ));
        }
        if self.coherence_floor.is_nan() || !(0.0..=1.0).contains(&self.coherence_floor) {
            return Err(ConfigError::BadConfig(
                "coherence_floor must be in [0,1]".into(),
            ));
        }
        if self.drive_ceiling_vrms <= 0.0 || self.drive_ceiling_vrms.is_nan() {
            return Err(ConfigError::BadConfig(
                "drive_ceiling_vrms must be > 0".into(),
            ));
        }
        if let Some(a) = self.drive_abort_vrms {
            if a <= 0.0 || a.is_nan() {
                return Err(ConfigError::BadConfig(
                    "drive_abort_vrms must be > 0 when set".into(),
                ));
            }
        }
        if self.divergence_loops == 0 {
            return Err(ConfigError::BadConfig(
                "divergence_loops must be >= 1".into(),
            ));
        }
        if self.full_level_duration_s < 0.0 || self.full_level_duration_s.is_nan() {
            return Err(ConfigError::BadConfig(
                "full_level_duration_s must be >= 0".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eigen_breakpoints::{BreakpointRow, BreakpointTable};

    fn demo_bt() -> BreakpointTable {
        let mut bt = BreakpointTable::new("t");
        bt.push(BreakpointRow::psd(20.0, 0.01));
        bt.push(BreakpointRow::psd(2000.0, 0.007));
        bt
    }

    #[test]
    fn siso_config_validates_by_default() {
        let cfg = RandomTestConfig::siso(demo_bt(), 1);
        assert!(cfg.validate().is_ok());
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
    fn auto_schedule_defaults_are_the_h1_ladder() {
        let s = LevelSchedule::auto_default();
        assert_eq!(s.mode, RunMode::Auto);
        assert_eq!(s.start_db, DEFAULT_START_LEVEL_DB);
        assert_eq!(s.table_db, DEFAULT_LEVEL_TABLE_DB.to_vec());
        assert_eq!(s.full_db(), 0.0);
        // A single-entry dwell applies to every rung.
        assert_eq!(s.dwell_for(0), 3.0);
        assert_eq!(s.dwell_for(4), 3.0);
    }

    #[test]
    fn validate_rejects_bad_knobs() {
        let mut cfg = RandomTestConfig::siso(demo_bt(), 1);
        cfg.overload_fraction = 1.5;
        assert!(matches!(cfg.validate(), Err(ConfigError::BadConfig(_))));

        let mut cfg = RandomTestConfig::siso(demo_bt(), 1);
        cfg.coherence_floor = -0.1;
        assert!(cfg.validate().is_err());

        let mut cfg = RandomTestConfig::siso(demo_bt(), 1);
        cfg.drive_ceiling_vrms = 0.0;
        assert!(cfg.validate().is_err());

        let mut cfg = RandomTestConfig::siso(demo_bt(), 1);
        cfg.divergence_loops = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_monotonic_level_table() {
        let mut cfg = RandomTestConfig::siso(demo_bt(), 1);
        cfg.schedule.table_db = vec![-12.0, -6.0, -9.0, 0.0]; // decreasing step
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn control_set_single_is_weighted_avg_of_one() {
        let cs = ControlSet::single(3);
        assert_eq!(cs.indices(), vec![3]);
        assert_eq!(cs.weights(), vec![1.0]);
        assert_eq!(cs.mode, CompositeMode::WeightedAvg);
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = RandomTestConfig::siso(demo_bt(), 1);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RandomTestConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.control.indices(), vec![1]);
        assert_eq!(back.metadata.len(), 4);
    }
}
