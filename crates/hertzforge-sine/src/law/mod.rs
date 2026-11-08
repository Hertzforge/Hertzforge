//! The Sine control-law seam: what a law sees, what it returns, how it is
//! built, and the guards the core runs every law under.
//!
//! # Where a law runs
//!
//! Once per control tick, on the engine's real-time thread, inside the step's
//! `process()`: [`SineControlLaw::engage`] on the first tick after system
//! identification, [`SineControlLaw::tick`] on every tick after it. The law
//! takes the place of three things the step does today: the amplitude servo,
//! the feedforward blend and the drive decision. Everything before it (the
//! mailbox, the settle window, the composite, the limit channels' notch
//! observation) and after it (the notch ceiling, the drive guard, the scalar
//! alarm and abort, the limit-channel abort, the kinematic monitor, the servo
//! fault, the overload latch and the raw-ring watchdog) stays with the core.
//!
//! # What a law decides, and what it does not
//!
//! A law returns one number per tick, the tone's amplitude in drive units peak
//! (volts peak on hardware), with its own fault judgement and up to
//! [`MAX_LAW_VALUES`] declared values. It does not decide the frequency, the
//! sweep rate or direction, the phase (the oscillator keeps it continuous), the
//! level schedule, the second reference output or any abort.
//!
//! # The guards (the law proposes, the core disposes)
//!
//! With [`LawGuards`] set ([`crate::SineTestConfig::law_guards`]) the core
//! checks every output ([`SineOutputRule`]), times every call on an injectable
//! [`LawClock`] against `budget_fraction` of the tick, contains a panic, caps
//! the change per tick (`max_step_db`) and the rise per second
//! (`max_rise_db_per_s`), bounds the first drive ([`FIRST_DRIVE_MARGIN_DB`]
//! over the prediction, or [`FIRST_DRIVE_UNTRUSTED_FRACTION`] of the start
//! level without one), and trips `divergence` when the cap binds for
//! `divergence_s` running. A late, failed, panicked or refused call is a miss:
//! the drive in force is held, a usable call resets the count, and
//! `max_misses` in a row stop the run with the core's controlled stop (outcome
//! `law_fault`); the law is not called again.
//!
//! # Conventions
//!
//! These types are the native side of the `sine-single` world
//! (`wit/control-law/sine-single.wit`), field for field, so a law file and a
//! native law see the same numbers. The control channels' units are EU peak
//! (g pk for an accelerometer); the drive is in drive units peak; the
//! identified gain is EU pk per drive unit pk; `prediction_db` is dB relative
//! to one drive unit, at full level.
//!
//! No allocation per tick on the native path: inputs borrow the core's
//! buffers, the output is `Copy`, and diagnostics go into a [`LawValues`]
//! buffer the core owns.
//!
//! # Where it is built
//!
//! The service builds the run's law with its factory after identification
//! ([`crate::SineService::with_law_factory`]; without one, the built-in
//! [`servo_blend`]), engages it before the engage lead-in, and the step calls
//! it every tick under the guards the configuration sets
//! ([`crate::SineTestConfig::law_guards`]), timed on the service's clock
//! ([`crate::SineService::with_law_clock`]). Without guards the law runs with
//! none but its own: the shipped law's 1 dB clamp, the original arithmetic.

pub mod servo_blend;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Instant;

/// The most values a law may report on one call.
pub const MAX_LAW_VALUES: usize = 8;

/// The catalog id of the built-in Sine law, `servo_blend`: today's amplitude
/// servo and feedforward blend behind the seam, and the default.
pub const SERVO_BLEND_ID: &str = "servo_blend";

/// Default control tick, seconds (the NI arm gate's value).
pub const DEFAULT_TICK_S: f64 = 0.010;
/// Shortest control tick for a built-in law, seconds.
pub const MIN_TICK_S_BUILTIN: f64 = 0.002;
/// Shortest control tick for a law file, seconds: the law host enforces a
/// call's budget to within its 1 ms epoch, so a quarter of the tick must be
/// at least 2 ms.
pub const MIN_TICK_S_LAW_FILE: f64 = 0.008;

/// Default cap on the drive's change in one tick, dB either way. The shipped
/// law's own clamp is the same 1 dB, so the cap never binds on it.
pub const DEFAULT_MAX_STEP_DB: f64 = 1.0;
/// The range `max_step_db` may be set in, dB. Never tighter than the shipped
/// law's own clamp.
pub const MAX_STEP_DB_RANGE: (f64, f64) = (1.0, 3.0);
/// Default cap on the drive's rise, dB per second.
pub const DEFAULT_MAX_RISE_DB_PER_S: f64 = 240.0;
/// Default fraction of the control tick one law call may take.
pub const DEFAULT_LAW_BUDGET_FRACTION: f64 = 0.25;
/// Default unusable calls in a row that stop the run.
pub const DEFAULT_LAW_MISSES: u32 = 3;
/// Default time the change cap may bind without a break before the core trips
/// `divergence`, seconds.
pub const DEFAULT_DIVERGENCE_S: f64 = 1.0;

/// The first drive is at most the prediction at the start frequency and level
/// plus this, dB (and never over the drive ceiling).
pub const FIRST_DRIVE_MARGIN_DB: f64 = 6.0;
/// Without a trusted prediction at the start frequency, the first drive is at
/// most this times the start level's scale, drive units peak.
pub const FIRST_DRIVE_UNTRUSTED_FRACTION: f64 = 0.05;

/// Why the core holds the drive this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// Nothing holds it: the law's drive is used, within the core's caps.
    Free,
    /// A limit channel holds the control point down: the law must not raise
    /// the drive, and the core holds it at the notch ceiling.
    Notch,
    /// A resonance search maps the plant at a fixed drive: the law is called
    /// so its state keeps up, and its output is not used.
    Probe,
}

impl Hold {
    /// The stable wire and record name.
    pub fn as_str(self) -> &'static str {
        match self {
            Hold::Free => "free",
            Hold::Notch => "notch",
            Hold::Probe => "probe",
        }
    }
}

/// The system identification on its own frequency grid, as the law sees it.
/// Every slice has one value per grid frequency.
#[derive(Debug, Clone, Copy)]
pub struct Identification<'a> {
    /// The grid, Hz, ascending.
    pub frequency_hz: &'a [f64],
    /// `|H|`, EU pk per drive unit pk.
    pub gain: &'a [f64],
    /// Ordinary coherence, 0 to 1.
    pub coherence: &'a [f64],
    /// Whether the core trusts the estimate at this frequency (its
    /// `feedforward` coherence and gain floors).
    pub trusted: &'a [bool],
    /// The full-level target at each grid frequency, EU pk.
    pub target: &'a [f64],
}

impl Identification<'_> {
    /// Frequencies on the grid.
    pub fn len(&self) -> usize {
        self.frequency_hz.len()
    }

    /// Whether the grid is empty.
    pub fn is_empty(&self) -> bool {
        self.frequency_hz.is_empty()
    }

    /// Whether every slice has one value per grid frequency.
    pub fn is_consistent(&self) -> bool {
        let n = self.len();
        self.gain.len() == n
            && self.coherence.len() == n
            && self.trusted.len() == n
            && self.target.len() == n
    }
}

/// What a Sine law is built for: fixed for the run.
#[derive(Debug, Clone, Copy)]
pub struct SineLawSetup<'a> {
    /// Control channels the composite is built from.
    pub control_channels: usize,
    /// The control tick at the rate the run adopted, seconds.
    pub tick_s: f64,
    /// The band the run sweeps or dwells in, Hz.
    pub band_hz: (f64, f64),
    /// The drive ceiling, drive units peak.
    pub drive_ceiling: f64,
    /// The identification.
    pub identification: Identification<'a>,
    /// The time one call may take, seconds (`budget_fraction` times the
    /// tick). A law file's host deadline is the same number.
    pub call_budget_s: f64,
}

/// Everything a Sine law sees on one tick.
#[derive(Debug, Clone, Copy)]
pub struct TickInputs<'a> {
    /// Ticks since the law was built: 0 at `engage`.
    pub tick_index: u64,
    /// Seconds since the last call.
    pub dt_s: f64,
    /// The tone's frequency this tick, Hz.
    pub frequency_hz: f64,
    /// Sweep rate, Hz per second, signed: 0 at a dwell or while held.
    pub sweep_rate_hz_per_s: f64,
    /// The scheduled level, dB relative to the full-level profile.
    pub level_db: f64,
    /// The profile at this frequency at this level, EU pk.
    pub target: f64,
    /// The measured composite of the control channels, EU pk.
    pub measured: f64,
    /// Each control channel's tracked amplitude, EU pk, in control-set order.
    pub channel_amplitude: &'a [f64],
    /// Each control channel's tracked phase against the drive, degrees.
    pub channel_phase_deg: &'a [f64],
    /// The drive in force, as actually played, drive units peak.
    pub drive: f64,
    /// The core's plant-inverse drive at this frequency at full level, dB
    /// relative to one drive unit; `None` where the identification is not
    /// trusted.
    pub prediction_db: Option<f64>,
    /// Why the core holds the drive this tick.
    pub hold: Hold,
    /// The limit channels' ceiling on the drive, drive units peak; `None` when
    /// none binds.
    pub notch_ceiling: Option<f64>,
    /// Whether the core's settle window is open.
    pub settling: bool,
}

/// One tick's result. The values go into the [`LawValues`] the core hands
/// the call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickOutputs {
    /// The tone's amplitude, drive units peak: finite and at least 0.
    pub drive: f64,
    /// The law's own fault judgement. Shown and recorded, never relied on.
    pub faulted: bool,
}

/// The values a law reports on one call, by the index of their key in its
/// declared diagnostics. A fixed buffer the core owns and clears before each
/// call, so the native path allocates nothing per tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LawValues {
    declared: usize,
    values: [f64; MAX_LAW_VALUES],
    set: [bool; MAX_LAW_VALUES],
}

impl LawValues {
    /// An empty buffer for a law that declares `declared` diagnostics (at most
    /// [`MAX_LAW_VALUES`]; more are cut to it).
    pub fn new(declared: usize) -> Self {
        LawValues {
            declared: declared.min(MAX_LAW_VALUES),
            values: [0.0; MAX_LAW_VALUES],
            set: [false; MAX_LAW_VALUES],
        }
    }

    /// Diagnostics the law declares.
    pub fn declared(&self) -> usize {
        self.declared
    }

    /// Forget every value (the core calls this before each call).
    pub fn clear(&mut self) {
        self.set = [false; MAX_LAW_VALUES];
    }

    /// Report `value` under the declared diagnostic `index`. An index the law
    /// did not declare is refused ([`SineOutputRule::UndeclaredValue`]).
    pub fn set(&mut self, index: usize, value: f64) -> Result<(), SineOutputRule> {
        if index >= self.declared {
            return Err(SineOutputRule::UndeclaredValue);
        }
        self.values[index] = value;
        self.set[index] = true;
        Ok(())
    }

    /// The value reported under `index` this call, if any.
    pub fn get(&self, index: usize) -> Option<f64> {
        (index < self.declared && self.set[index]).then(|| self.values[index])
    }

    /// The values reported this call, as `(index, value)`.
    pub fn iter(&self) -> impl Iterator<Item = (usize, f64)> + '_ {
        (0..self.declared).filter_map(|i| self.get(i).map(|v| (i, v)))
    }
}

/// Which output rule a refused law output broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineOutputRule {
    /// The drive, or a reported value, was not a finite number.
    NonFinite,
    /// The drive was below zero.
    Negative,
    /// A value under a key the manifest does not declare.
    UndeclaredValue,
    /// More values than [`MAX_LAW_VALUES`].
    TooManyValues,
}

impl SineOutputRule {
    /// The stable wire and record name.
    pub fn as_str(self) -> &'static str {
        match self {
            SineOutputRule::NonFinite => "non_finite",
            SineOutputRule::Negative => "negative",
            SineOutputRule::UndeclaredValue => "undeclared_value",
            SineOutputRule::TooManyValues => "too_many_values",
        }
    }
}

impl fmt::Display for SineOutputRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SineOutputRule::NonFinite => "the output held a value that is not a finite number",
            SineOutputRule::Negative => "the drive was below zero",
            SineOutputRule::UndeclaredValue => {
                "the law reported a value its manifest does not declare"
            }
            SineOutputRule::TooManyValues => "the law reported more values than allowed",
        })
    }
}

/// Why a law call was not used. Every one is a miss.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawFault {
    /// The law reported that it could not compute an output (or could not be
    /// built).
    Failed(String),
    /// The law panicked (a native law) or trapped (a law file); the core caught
    /// it.
    Crashed(String),
    /// The call took longer than its budget.
    OverBudget {
        /// How long the call took, seconds.
        took_s: f64,
        /// The budget, seconds.
        budget_s: f64,
    },
    /// A law file asked for more memory than its cap.
    OverMemory,
    /// The core refused the output.
    BadOutput(SineOutputRule),
}

impl LawFault {
    /// The stable wire and record name: `error`, `panicked`, `over_budget`,
    /// `over_memory`, or the broken [`SineOutputRule`]'s name.
    pub fn reason(&self) -> &'static str {
        match self {
            LawFault::Failed(_) => "error",
            LawFault::Crashed(_) => "panicked",
            LawFault::OverBudget { .. } => "over_budget",
            LawFault::OverMemory => "over_memory",
            LawFault::BadOutput(rule) => rule.as_str(),
        }
    }
}

impl fmt::Display for LawFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LawFault::Failed(s) => write!(f, "the control law reported an error: {s}"),
            LawFault::Crashed(s) => write!(f, "the control law crashed: {s}"),
            LawFault::OverBudget { took_s, budget_s } => write!(
                f,
                "the control law took {:.3} ms, over its {:.3} ms budget",
                took_s * 1e3,
                budget_s * 1e3
            ),
            LawFault::OverMemory => {
                f.write_str("the control law asked for more memory than its cap")
            }
            LawFault::BadOutput(rule) => write!(f, "the control law's output was refused: {rule}"),
        }
    }
}

impl std::error::Error for LawFault {}

/// The core's guards on a Sine law, the same for every law, the shipped one
/// included. Carried in [`crate::SineTestConfig::law_guards`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LawGuards {
    /// Largest change of the drive in one tick, dB either way, measured from
    /// the drive in force. In [`MAX_STEP_DB_RANGE`].
    pub max_step_db: f64,
    /// Largest rise of the drive, dB per second.
    pub max_rise_db_per_s: f64,
    /// Fraction of the control tick one call may take before its output is not
    /// used. A law file's host deadline is the same number.
    pub budget_fraction: f64,
    /// Unusable calls in a row at which the run takes its controlled stop and
    /// ends `law_fault`.
    pub max_misses: u32,
    /// Seconds the change cap may bind without a break before the core trips
    /// `divergence`.
    pub divergence_s: f64,
}

impl Default for LawGuards {
    fn default() -> Self {
        LawGuards {
            max_step_db: DEFAULT_MAX_STEP_DB,
            max_rise_db_per_s: DEFAULT_MAX_RISE_DB_PER_S,
            budget_fraction: DEFAULT_LAW_BUDGET_FRACTION,
            max_misses: DEFAULT_LAW_MISSES,
            divergence_s: DEFAULT_DIVERGENCE_S,
        }
    }
}

impl LawGuards {
    /// The time one call may take at a control tick of `tick_s`, seconds.
    pub fn call_budget_s(&self, tick_s: f64) -> f64 {
        self.budget_fraction * tick_s
    }

    /// Check the guards' ranges. The error names the wire field.
    pub fn validate(&self) -> Result<(), String> {
        let (lo, hi) = MAX_STEP_DB_RANGE;
        if !(self.max_step_db.is_finite() && (lo..=hi).contains(&self.max_step_db)) {
            return Err(format!(
                "max_step_db must be {lo} to {hi} dB (got {})",
                self.max_step_db
            ));
        }
        if !(self.max_rise_db_per_s.is_finite()
            && self.max_rise_db_per_s > 0.0
            && self.max_rise_db_per_s <= 10_000.0)
        {
            return Err(format!(
                "max_rise_db_per_s must be above 0 and at most 10000 (got {})",
                self.max_rise_db_per_s
            ));
        }
        if !(self.budget_fraction.is_finite()
            && self.budget_fraction > 0.0
            && self.budget_fraction <= 1.0)
        {
            return Err(format!(
                "law_budget must be above 0 and at most 1 (got {})",
                self.budget_fraction
            ));
        }
        if !(1..=100).contains(&self.max_misses) {
            return Err(format!(
                "law_misses must be 1 to 100 (got {})",
                self.max_misses
            ));
        }
        if !(self.divergence_s.is_finite() && self.divergence_s > 0.0 && self.divergence_s <= 60.0)
        {
            return Err(format!(
                "divergence_s must be above 0 and at most 60 (got {})",
                self.divergence_s
            ));
        }
        Ok(())
    }
}

/// The clock a law call is timed on. Monotonic seconds from an arbitrary
/// origin. Injectable so a test makes a law "slow" without resting a pass or a
/// failure on the wall clock.
pub trait LawClock: Send + Sync {
    /// Seconds since this clock's origin. Never decreases.
    fn now_s(&self) -> f64;
}

/// The production clock: [`std::time::Instant`] from the moment it was built.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    /// A clock whose origin is now.
    pub fn new() -> Self {
        MonotonicClock {
            origin: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl LawClock for MonotonicClock {
    fn now_s(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
}

/// A parameter value as the run's records hold it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamSetting {
    /// A whole number.
    Integer(i64),
    /// A number.
    Number(f64),
    /// An on/off switch.
    Toggle(bool),
    /// One of a fixed set of named options.
    Choice(String),
}

/// A live parameter's declared range: the core checks an operator's change
/// against it before the law sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveParam {
    /// The parameter's key.
    pub key: String,
    /// Smallest accepted value.
    pub min: f64,
    /// Largest accepted value.
    pub max: f64,
}

/// A law as the run's records and the core name it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawDescriptor {
    /// The catalog id: the built-in's id, or a law file's SHA-256.
    pub id: String,
    /// The manifest's name.
    pub name: String,
    /// The manifest's version.
    pub version: String,
    /// A law file's SHA-256; `None` for a built-in law.
    pub sha256: Option<String>,
    /// The parameter values the run builds the law with, in manifest order.
    pub params: Vec<(String, ParamSetting)>,
    /// The declared diagnostics' keys, in order: a [`LawValues`] index is a
    /// position in this list.
    pub diagnostics: Vec<String>,
    /// The parameters the operator may change during the run.
    pub live: Vec<LiveParam>,
}

/// A Sine control law. Its state lives in the value, built once per run after
/// system identification by a [`SineLawFactory`].
pub trait SineControlLaw {
    /// The law's name, for the log and the report.
    fn name(&self) -> &str;

    /// The first tick after identification: the starting drive.
    fn engage(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault>;

    /// One control tick: the drive.
    fn tick(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault>;

    /// The operator changed the live parameter `key` to `value`. The core has
    /// checked that `key` is declared live and `value` is in its range; the
    /// value applies from the next tick. An error keeps the value in force and
    /// refuses the change; it is not a miss.
    fn adjust(&mut self, key: &str, value: f64) -> Result<(), LawFault>;
}

/// Builds the Sine law for a run. Called once per run, after system
/// identification. The factory holds the law's parameter values; the core
/// holds the guards.
pub trait SineLawFactory: Send + Sync {
    /// Build a law for `setup`, or say why it cannot be built (the run then
    /// ends `law_fault` before anything is driven at test level).
    fn build(&self, setup: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault>;

    /// The law as the run's records name it.
    fn describe(&self) -> LawDescriptor;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guards_default_to_the_plan_values_and_validate() {
        let g = LawGuards::default();
        assert_eq!(g.max_step_db, 1.0);
        assert_eq!(g.max_rise_db_per_s, 240.0);
        assert_eq!(g.budget_fraction, 0.25);
        assert_eq!(g.max_misses, 3);
        assert_eq!(g.divergence_s, 1.0);
        assert!(g.validate().is_ok());
        assert!((g.call_budget_s(DEFAULT_TICK_S) - 0.0025).abs() < 1e-15);
        // A law file's shortest tick still leaves the host a 2 ms budget.
        assert!(g.call_budget_s(MIN_TICK_S_LAW_FILE) >= 0.002 - 1e-15);

        for (bad, field) in [
            (
                LawGuards {
                    max_step_db: 0.5,
                    ..LawGuards::default()
                },
                "max_step_db",
            ),
            (
                LawGuards {
                    max_step_db: 3.5,
                    ..LawGuards::default()
                },
                "max_step_db",
            ),
            (
                LawGuards {
                    max_rise_db_per_s: f64::NAN,
                    ..LawGuards::default()
                },
                "max_rise_db_per_s",
            ),
            (
                LawGuards {
                    budget_fraction: 0.0,
                    ..LawGuards::default()
                },
                "law_budget",
            ),
            (
                LawGuards {
                    max_misses: 0,
                    ..LawGuards::default()
                },
                "law_misses",
            ),
            (
                LawGuards {
                    divergence_s: -1.0,
                    ..LawGuards::default()
                },
                "divergence_s",
            ),
        ] {
            let e = bad.validate().expect_err("refused");
            assert!(e.starts_with(field), "{e}");
        }
    }

    #[test]
    fn a_partial_guards_record_takes_the_defaults_for_the_rest() {
        let g: LawGuards = serde_json::from_str(r#"{"max_step_db":2}"#).unwrap();
        assert_eq!(
            g,
            LawGuards {
                max_step_db: 2.0,
                ..LawGuards::default()
            }
        );
    }

    #[test]
    fn values_are_held_by_declared_index_and_an_undeclared_one_is_refused() {
        let mut v = LawValues::new(3);
        assert_eq!(v.declared(), 3);
        v.set(0, 1.5).unwrap();
        v.set(2, -0.25).unwrap();
        assert_eq!(v.set(3, 1.0), Err(SineOutputRule::UndeclaredValue));
        assert_eq!(v.get(1), None);
        assert_eq!(v.iter().collect::<Vec<_>>(), vec![(0, 1.5), (2, -0.25)]);
        v.clear();
        assert_eq!(v.iter().count(), 0);
        assert_eq!(
            LawValues::new(MAX_LAW_VALUES + 4).declared(),
            MAX_LAW_VALUES
        );
    }

    #[test]
    fn every_fault_and_rule_has_its_stable_wire_name() {
        let names: Vec<&str> = [
            LawFault::Failed("x".into()),
            LawFault::Crashed("x".into()),
            LawFault::OverBudget {
                took_s: 0.003,
                budget_s: 0.0025,
            },
            LawFault::OverMemory,
            LawFault::BadOutput(SineOutputRule::NonFinite),
            LawFault::BadOutput(SineOutputRule::Negative),
            LawFault::BadOutput(SineOutputRule::UndeclaredValue),
            LawFault::BadOutput(SineOutputRule::TooManyValues),
        ]
        .iter()
        .map(LawFault::reason)
        .collect();
        assert_eq!(
            names,
            [
                "error",
                "panicked",
                "over_budget",
                "over_memory",
                "non_finite",
                "negative",
                "undeclared_value",
                "too_many_values"
            ]
        );
        assert_eq!(
            [Hold::Free, Hold::Notch, Hold::Probe].map(Hold::as_str),
            ["free", "notch", "probe"]
        );
        assert_eq!(serde_json::to_string(&Hold::Notch).unwrap(), "\"notch\"");
    }

    /// A law and a factory, enough to prove the traits are object safe and
    /// that a law runs on borrowed inputs with nothing allocated per tick.
    struct Echo;

    impl SineControlLaw for Echo {
        fn name(&self) -> &str {
            "echo"
        }

        fn engage(
            &mut self,
            inputs: &TickInputs<'_>,
            values: &mut LawValues,
        ) -> Result<TickOutputs, LawFault> {
            self.tick(inputs, values)
        }

        fn tick(
            &mut self,
            inputs: &TickInputs<'_>,
            values: &mut LawValues,
        ) -> Result<TickOutputs, LawFault> {
            values
                .set(0, inputs.measured)
                .map_err(LawFault::BadOutput)?;
            Ok(TickOutputs {
                drive: inputs.drive,
                faulted: false,
            })
        }

        fn adjust(&mut self, key: &str, _value: f64) -> Result<(), LawFault> {
            Err(LawFault::Failed(format!("no parameter {key}")))
        }
    }

    struct EchoFactory;

    impl SineLawFactory for EchoFactory {
        fn build(
            &self,
            setup: &SineLawSetup<'_>,
        ) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
            if !setup.identification.is_consistent() {
                return Err(LawFault::Failed("ragged identification".into()));
            }
            Ok(Box::new(Echo))
        }

        fn describe(&self) -> LawDescriptor {
            LawDescriptor {
                id: "echo".into(),
                name: "echo".into(),
                version: "0".into(),
                sha256: None,
                params: vec![("gain".into(), ParamSetting::Number(0.5))],
                diagnostics: vec!["measured".into()],
                live: vec![],
            }
        }
    }

    #[test]
    fn a_law_is_built_and_called_through_the_traits() {
        let f = [100.0, 200.0];
        let ident = Identification {
            frequency_hz: &f,
            gain: &[1.0, 2.0],
            coherence: &[0.99, 0.98],
            trusted: &[true, true],
            target: &[1.0, 1.0],
        };
        let setup = SineLawSetup {
            control_channels: 1,
            tick_s: DEFAULT_TICK_S,
            band_hz: (100.0, 200.0),
            drive_ceiling: 0.5,
            identification: ident,
            call_budget_s: LawGuards::default().call_budget_s(DEFAULT_TICK_S),
        };
        let factory: Box<dyn SineLawFactory> = Box::new(EchoFactory);
        let mut law = factory.build(&setup).expect("built");
        let d = factory.describe();
        let mut values = LawValues::new(d.diagnostics.len());
        let inputs = TickInputs {
            tick_index: 0,
            dt_s: DEFAULT_TICK_S,
            frequency_hz: 150.0,
            sweep_rate_hz_per_s: 0.0,
            level_db: -6.0,
            target: 0.5,
            measured: 0.49,
            channel_amplitude: &[0.49],
            channel_phase_deg: &[-3.0],
            drive: 0.1,
            prediction_db: Some(-20.0),
            hold: Hold::Free,
            notch_ceiling: None,
            settling: true,
        };
        let out = law.engage(&inputs, &mut values).expect("engaged");
        assert_eq!(out.drive, 0.1);
        assert_eq!(values.get(0), Some(0.49));
        assert!(law.adjust("gain", 1.0).is_err());

        let ragged = SineLawSetup {
            identification: Identification {
                gain: &[1.0],
                ..ident
            },
            ..setup
        };
        assert!(factory.build(&ragged).is_err());
    }
}
