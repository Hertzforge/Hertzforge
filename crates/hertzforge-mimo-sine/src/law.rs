//! The native MIMO Sine law seam: what a law is built with ([`LawSetup`]),
//! what it says about itself ([`LawManifest`]), the law ([`MimoSineLaw`]) and
//! what builds it ([`MimoSineLawFactory`]).
//!
//! A law is built once per run, after identification, by a factory that holds
//! its parameter values; `engage` gives the first drive and `tick` every drive
//! after it, once per control update, on the controller's real-time thread. A
//! law file of the `sine-mimo` world is adapted to the same traits. The guards
//! (budget, misses, answer checks, the change, rise and rotation caps, the
//! backstop notch, the drive ceilings) are the core's and apply to every law.
//!
//! Mirrors `sine-mimo-setup` and `sine-mimo-manifest` of
//! `wit/control-law/sine-mimo.wit`.

use crate::limits::{MAX_CONTROLS, MAX_DRIVES, MAX_LIMITS};
use crate::setup::{shape_name, SetupError, SetupRule, TestType, Tracking};
use crate::tick::{LawFault, TickInputs, TickOutputs};
use hertzforge_sdm::law::MimoShape;
use hertzforge_sdm::{MatrixStack, C64};
use serde::{Deserialize, Serialize};
use std::fmt;

pub use hertzforge_sine::law::{LawDescriptor, LawValues, ParamSetting};

/// The catalog id of the default law, `matrix`: the correction through the
/// identification's inverse.
pub const DEFAULT_LAW_ID: &str = "matrix";

/// The catalog ids of the built-in laws, the default first.
pub const BUILTIN_LAW_IDS: [&str; 5] = ["matrix", "paired", "steepest_descent", "secant", "ratio"];

/// The identification on its own frequency grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawIdentification {
    /// The grid, Hz, ascending.
    pub frequency_hz: Vec<f64>,
    /// Every control's response to every drive, EU pk per volt pk: one
    /// matrix of controls by drives per line.
    pub response: MatrixStack,
    /// Every limit channel's response to every drive: one matrix of limit
    /// channels by drives per line.
    pub limit_response: MatrixStack,
    /// Each control's multiple coherence, `controls * lines` values, control
    /// after control.
    pub coherence: Vec<f64>,
    /// Each limit channel's multiple coherence, `limits * lines` values.
    pub limit_coherence: Vec<f64>,
    /// Whether the core trusts each line.
    pub trusted: Vec<bool>,
}

/// Each control's alarm and abort bands on the identification's grid,
/// positive dB below and above the reference, `controls * lines` values in
/// each, control after control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawTolerance {
    pub alarm_low_db: Vec<f64>,
    pub alarm_high_db: Vec<f64>,
    pub abort_low_db: Vec<f64>,
    pub abort_high_db: Vec<f64>,
}

/// What a MIMO Sine law is built for: fixed for the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawSetup {
    /// Control channels, 1 to [`MAX_CONTROLS`].
    pub controls: usize,
    /// Drives, 1 to [`MAX_DRIVES`].
    pub drives: usize,
    /// Limit channels, 0 to [`MAX_LIMITS`].
    pub limits: usize,
    /// The station's sample rate, Hz.
    pub fs_hz: f64,
    /// The control update interval, s.
    pub tick_s: f64,
    /// The band the run moves in, Hz.
    pub band_lo_hz: f64,
    pub band_hi_hz: f64,
    pub test_type: TestType,
    pub shape: MimoShape,
    /// Each drive's ceiling, volts peak.
    pub drive_ceiling_v: Vec<f64>,
    pub identification: LawIdentification,
    /// The level the identification was taken at, dB against full level;
    /// `None` when it could not be measured.
    pub identification_level_db: Option<f64>,
    /// The full-level reference on the identification's grid,
    /// `controls * lines` values, control after control, EU pk.
    pub reference: Vec<C64>,
    /// Whether the reference's phases are part of the test.
    pub phase_specified: bool,
    /// Each limit channel's bound on the grid, `limits * lines` values;
    /// `None` on a line without one.
    pub limit_bounds: Vec<Option<f64>>,
    pub tolerance: LawTolerance,
    pub tracking: Tracking,
    /// The tracker's group delay times its bandwidth: a tracked amplitude
    /// lags its response by `tracker_delay_factor / B`.
    #[serde(default = "default_tracker_delay_factor")]
    pub tracker_delay_factor: f64,
    /// The delay between a drive and its tracked reading that the
    /// identification does not show, s (the identification has the lag taken
    /// out). The loop takes it on every update.
    #[serde(default)]
    pub transport_delay_s: f64,
}

/// The delay factor a setup that does not state one is read with: two
/// one-pole stages, `1 / pi`.
pub fn default_tracker_delay_factor() -> f64 {
    1.0 / std::f64::consts::PI
}

/// The rule a [`LawSetup`] broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawSetupRule {
    /// A count is 0 or over its limit (limit channels may be 0).
    SizeOutOfRange,
    /// The rate, the tick or the band is not usable.
    GridInvalid,
    /// The shape is not the shape of the counts.
    ShapeMismatch,
    /// A list or a matrix is not the length its counts ask for.
    WrongLength,
    /// A number that must be finite, or positive, is not.
    ValueInvalid,
    /// The grid is not ascending.
    GridNotAscending,
}

impl LawSetupRule {
    /// The stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            LawSetupRule::SizeOutOfRange => "size_out_of_range",
            LawSetupRule::GridInvalid => "grid_invalid",
            LawSetupRule::ShapeMismatch => "shape_mismatch",
            LawSetupRule::WrongLength => "wrong_length",
            LawSetupRule::ValueInvalid => "value_invalid",
            LawSetupRule::GridNotAscending => "grid_not_ascending",
        }
    }
}

/// Why a [`LawSetup`] was refused.
#[derive(Debug, Clone, PartialEq)]
pub struct LawSetupError {
    pub rule: LawSetupRule,
    pub message: String,
}

impl fmt::Display for LawSetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule.as_str(), self.message)
    }
}

impl std::error::Error for LawSetupError {}

fn law_err(rule: LawSetupRule, message: impl Into<String>) -> LawSetupError {
    LawSetupError {
        rule,
        message: message.into(),
    }
}

fn want(name: &str, got: usize, want: usize) -> Result<(), LawSetupError> {
    if got == want {
        Ok(())
    } else {
        Err(law_err(
            LawSetupRule::WrongLength,
            format!("{name} has {got} values; {want} are needed"),
        ))
    }
}

impl LawSetup {
    /// Lines on the identification's grid.
    pub fn lines(&self) -> usize {
        self.identification.frequency_hz.len()
    }

    /// Check the setup's sizes, lengths and values; the first rule broken is
    /// named.
    pub fn validate(&self) -> Result<(), LawSetupError> {
        let (nc, nd, nl) = (self.controls, self.drives, self.limits);
        if nc == 0 || nc > MAX_CONTROLS || nd == 0 || nd > MAX_DRIVES || nl > MAX_LIMITS {
            return Err(law_err(
                LawSetupRule::SizeOutOfRange,
                format!(
                    "{nc} controls, {nd} drives and {nl} limit channels; 1 to {MAX_CONTROLS}, \
                     1 to {MAX_DRIVES} and 0 to {MAX_LIMITS} are accepted"
                ),
            ));
        }
        if !(self.fs_hz.is_finite()
            && self.fs_hz > 0.0
            && self.tick_s.is_finite()
            && self.tick_s > 0.0
            && self.band_lo_hz.is_finite()
            && self.band_hi_hz.is_finite()
            && 0.0 < self.band_lo_hz
            && self.band_lo_hz <= self.band_hi_hz
            && self.band_hi_hz < self.fs_hz / 2.0)
        {
            return Err(law_err(
                LawSetupRule::GridInvalid,
                "the rate, the tick or the band is not usable",
            ));
        }
        if self.shape != MimoShape::of(nc, nd) {
            return Err(law_err(
                LawSetupRule::ShapeMismatch,
                format!(
                    "the shape is {}; {nc} controls and {nd} drives are {}",
                    shape_name(self.shape),
                    shape_name(MimoShape::of(nc, nd))
                ),
            ));
        }
        want("the drive ceilings", self.drive_ceiling_v.len(), nd)?;
        if !self
            .drive_ceiling_v
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
        {
            return Err(law_err(
                LawSetupRule::ValueInvalid,
                "a drive ceiling is not a number above 0",
            ));
        }
        let id = &self.identification;
        let lines = self.lines();
        if lines == 0 {
            return Err(law_err(
                LawSetupRule::WrongLength,
                "the identification has no line",
            ));
        }
        if !id.frequency_hz.iter().all(|f| f.is_finite() && *f >= 0.0)
            || id.frequency_hz.windows(2).any(|w| w[1] <= w[0])
        {
            return Err(law_err(
                LawSetupRule::GridNotAscending,
                "the identification's grid is not ascending",
            ));
        }
        let (r, lr) = (&id.response, &id.limit_response);
        if (r.lines(), r.rows(), r.cols()) != (lines, nc, nd) {
            return Err(law_err(
                LawSetupRule::WrongLength,
                format!(
                    "the response is {} lines of {} x {}; {lines} lines of {nc} x {nd} are needed",
                    r.lines(),
                    r.rows(),
                    r.cols()
                ),
            ));
        }
        if nl > 0 && (lr.lines(), lr.rows(), lr.cols()) != (lines, nl, nd) {
            return Err(law_err(
                LawSetupRule::WrongLength,
                format!(
                    "the limit response is {} lines of {} x {}; {lines} lines of {nl} x {nd} are \
                     needed",
                    lr.lines(),
                    lr.rows(),
                    lr.cols()
                ),
            ));
        }
        if !r.data().iter().chain(lr.data()).all(|c| c.is_finite()) {
            return Err(law_err(
                LawSetupRule::ValueInvalid,
                "the identification holds a value that is not finite",
            ));
        }
        want("the coherence", id.coherence.len(), nc * lines)?;
        want("the limit coherence", id.limit_coherence.len(), nl * lines)?;
        if !id
            .coherence
            .iter()
            .chain(&id.limit_coherence)
            .all(|c| (0.0..=1.0).contains(c))
        {
            return Err(law_err(
                LawSetupRule::ValueInvalid,
                "a coherence is not between 0 and 1",
            ));
        }
        want("the trusted flags", id.trusted.len(), lines)?;
        want("the reference", self.reference.len(), nc * lines)?;
        if !self.reference.iter().all(|c| c.is_finite()) {
            return Err(law_err(
                LawSetupRule::ValueInvalid,
                "the reference holds a value that is not finite",
            ));
        }
        want("the limit bounds", self.limit_bounds.len(), nl * lines)?;
        if !self
            .limit_bounds
            .iter()
            .flatten()
            .all(|b| b.is_finite() && *b > 0.0)
        {
            return Err(law_err(
                LawSetupRule::ValueInvalid,
                "a limit bound is not a number above 0",
            ));
        }
        let t = &self.tolerance;
        for (name, v) in [
            ("the alarm band below", &t.alarm_low_db),
            ("the alarm band above", &t.alarm_high_db),
            ("the abort band below", &t.abort_low_db),
            ("the abort band above", &t.abort_high_db),
        ] {
            want(name, v.len(), nc * lines)?;
            if !v.iter().all(|x| x.is_finite() && *x >= 0.0) {
                return Err(law_err(
                    LawSetupRule::ValueInvalid,
                    format!("{name} holds a value that is not a positive number of dB"),
                ));
            }
        }
        if let Some(l) = self.identification_level_db {
            if !l.is_finite() {
                return Err(law_err(
                    LawSetupRule::ValueInvalid,
                    "the identification's level is not finite",
                ));
            }
        }
        if !(self.tracker_delay_factor.is_finite() && self.tracker_delay_factor > 0.0) {
            return Err(law_err(
                LawSetupRule::ValueInvalid,
                "the tracker's delay factor is not a number above 0",
            ));
        }
        if !(self.transport_delay_s.is_finite() && self.transport_delay_s >= 0.0) {
            return Err(law_err(
                LawSetupRule::ValueInvalid,
                "the transport delay is not a time of 0 or more",
            ));
        }
        Ok(())
    }
}

/// What a law says about itself besides its descriptor. Mirrors
/// `sine-mimo-manifest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawManifest {
    /// The shapes the law accepts: at least one, each at most once.
    pub shapes: Vec<MimoShape>,
    /// The test types the law accepts: at least one, each at most once.
    pub test_types: Vec<TestType>,
    /// Whether the law accepts an amplitude-only reference.
    pub amplitude_only: bool,
    /// The keys of the values the law may report.
    pub values: Vec<String>,
}

impl LawManifest {
    /// Whether the law accepts a test of `shape`.
    pub fn accepts_shape(&self, shape: MimoShape) -> bool {
        self.shapes.contains(&shape)
    }

    /// Whether the law accepts a test of `test_type`.
    pub fn accepts_test_type(&self, test_type: TestType) -> bool {
        self.test_types.contains(&test_type)
    }
}

/// Refuse a run the law does not accept, before anything is built: its shape
/// ([`SetupRule::LawShapeNotAccepted`]), its test type
/// ([`SetupRule::LawTestTypeNotAccepted`]) or an amplitude-only reference
/// ([`SetupRule::LawAmplitudeOnlyNotAccepted`]).
pub fn check_law_accepts(
    manifest: &LawManifest,
    shape: MimoShape,
    test_type: TestType,
    phase_specified: bool,
) -> Result<(), SetupError> {
    if !manifest.accepts_shape(shape) {
        return Err(SetupError::new(
            SetupRule::LawShapeNotAccepted,
            "law.id",
            format!(
                "the law accepts {}, not {}",
                manifest
                    .shapes
                    .iter()
                    .map(|s| shape_name(*s))
                    .collect::<Vec<_>>()
                    .join(" and "),
                shape_name(shape)
            ),
        ));
    }
    if !manifest.accepts_test_type(test_type) {
        return Err(SetupError::new(
            SetupRule::LawTestTypeNotAccepted,
            "law.id",
            format!(
                "the law accepts {}, not {}",
                manifest
                    .test_types
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" and "),
                test_type.as_str()
            ),
        ));
    }
    if !phase_specified && !manifest.amplitude_only {
        return Err(SetupError::new(
            SetupRule::LawAmplitudeOnlyNotAccepted,
            "law.id",
            "the law needs the references' phases; this run gives amplitudes only",
        ));
    }
    Ok(())
}

/// A MIMO Sine law. Its state lives in the value and lasts the whole run.
///
/// Each call writes its answer into `out`, which the core has reset and sized
/// (one drive per drive, one notched reference value per control), and its
/// values into `values`. A law rebases on `inputs.drive_in_force` every
/// update: the drive it asked for is not always the drive that was played.
pub trait MimoSineLaw {
    /// The law's name, for the log and the report.
    fn name(&self) -> &str;

    /// The first update after identification: the first drive.
    fn engage(
        &mut self,
        inputs: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault>;

    /// One control update: the drive.
    fn tick(
        &mut self,
        inputs: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault>;
}

/// Builds the law for a run. Called once per run, after identification.
pub trait MimoSineLawFactory: Send + Sync {
    /// Build a law for `setup`, or say why it cannot be built (the run then
    /// ends `law_fault` before anything is driven at test level).
    fn build(&self, setup: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault>;

    /// The law as the run's records name it.
    fn describe(&self) -> LawDescriptor;

    /// The shapes, test types and references it accepts, and the values it
    /// may report.
    fn manifest(&self) -> LawManifest;
}
