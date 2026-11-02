//! The host's own plain types: what a caller hands a law and gets back.
//!
//! They mirror the WIT types in `wit/control-law/types.wit`, with Rust index
//! types for line numbers. [`LoopInputs`] and [`LoopOutputs`] carry the
//! controller's loop boundary field for field, so an adapter can implement the
//! controller's law trait over a [`crate::LawInstance`] with plain copies.

/// A real number the operator enters in a box.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberSpec {
    /// Unit shown next to the box. May be empty.
    pub unit: String,
    /// Smallest accepted value.
    pub min: f64,
    /// Largest accepted value.
    pub max: f64,
    /// How far one step of the box moves the value (a hint for the form).
    pub step: f64,
    /// The value used when none is supplied.
    pub default: f64,
}

/// A whole number the operator enters in a box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegerSpec {
    /// Unit shown next to the box. May be empty.
    pub unit: String,
    /// Smallest accepted value.
    pub min: i64,
    /// Largest accepted value.
    pub max: i64,
    /// How far one step of the box moves the value (a hint for the form).
    pub step: i64,
    /// The value used when none is supplied.
    pub default: i64,
}

/// One of a fixed set of named options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceSpec {
    /// The options, in form order. Each is unique and not empty.
    pub options: Vec<String>,
    /// One of `options`.
    pub default: String,
}

/// One point of a curve: a value at a frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Breakpoint {
    /// Frequency, Hz.
    pub frequency_hz: f64,
    /// Value at that frequency, in the curve's unit.
    pub value: f64,
}

/// A value against frequency, as a breakpoint table.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveSpec {
    /// Unit of the value.
    pub unit: String,
    /// Smallest accepted value at any breakpoint.
    pub min: f64,
    /// Largest accepted value at any breakpoint.
    pub max: f64,
    /// The curve used when none is supplied. At least one point, frequencies
    /// strictly increasing.
    pub default: Vec<Breakpoint>,
}

/// What kind of input a parameter is.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    /// A real number.
    Number(NumberSpec),
    /// A whole number.
    Integer(IntegerSpec),
    /// An on/off switch, with its default state.
    Toggle(bool),
    /// One of a set of named options.
    Choice(ChoiceSpec),
    /// A value against frequency.
    Curve(CurveSpec),
    /// One number per control channel, each within the same limits.
    PerChannel(NumberSpec),
}

/// One parameter a law declares.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    /// Stable identifier: `[a-z][a-z0-9_]*`, at most 64 characters.
    pub key: String,
    /// Name shown on the form.
    pub label: String,
    /// Help text for the form.
    pub help: String,
    /// The kind, limits and default.
    pub kind: ParamKind,
}

/// One named number a law reports each loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticSpec {
    /// Same rules as a parameter key.
    pub key: String,
    /// Name shown on a plot or table.
    pub label: String,
    /// Unit. May be empty.
    pub unit: String,
    /// Help text.
    pub help: String,
}

/// What a law says about itself, read once when its file is loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Short name. Not empty.
    pub name: String,
    /// The law's own version. Not empty.
    pub version: String,
    /// Who wrote it. May be empty.
    pub author: String,
    /// What it does. May be empty.
    pub summary: String,
    /// Declared parameters, in form order.
    pub params: Vec<ParamSpec>,
    /// Declared per-loop diagnostics.
    pub diagnostics: Vec<DiagnosticSpec>,
}

/// A value for one parameter; its case must match the parameter's kind.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    /// For [`ParamKind::Number`].
    Number(f64),
    /// For [`ParamKind::Integer`].
    Integer(i64),
    /// For [`ParamKind::Toggle`].
    Toggle(bool),
    /// For [`ParamKind::Choice`]: one of the options, by name.
    Choice(String),
    /// For [`ParamKind::Curve`].
    Curve(Vec<Breakpoint>),
    /// For [`ParamKind::PerChannel`]: one value per control channel.
    PerChannel(Vec<f64>),
}

/// A parameter value with its key.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The declared key.
    pub key: String,
    /// The value.
    pub value: ParamValue,
}

impl Param {
    /// A parameter value for `key`.
    pub fn new(key: impl Into<String>, value: ParamValue) -> Self {
        Param {
            key: key.into(),
            value,
        }
    }
}

/// What a single-output Random law is built for; fixed for the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setup<'a> {
    /// The full-level target control PSD on the line grid. Zero out of band.
    pub target_psd: &'a [f64],
    /// Line spacing, Hz.
    pub df_hz: f64,
    /// The half-open in-band window `[lo, hi)` in lines.
    pub band: (usize, usize),
    /// Control channels the composite is built from (the length of every
    /// per-channel parameter).
    pub control_channels: usize,
}

/// Everything a law sees on one loop. The fields through `band` match the
/// controller's `LoopInputs` one for one; `dof` and `loop_index` are added.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopInputs<'a> {
    /// Full-level target PSD, `g^2/Hz`. Zero out of band.
    pub target_psd: &'a [f64],
    /// Measured composite control PSD at the running level, same grid.
    pub measured_psd: &'a [f64],
    /// Plant power gain `|H|^2`, drive to control response, per line.
    pub h_mag2: &'a [f64],
    /// Ordinary coherence per line, 0 to 1.
    pub coherence: &'a [f64],
    /// The scheduled level, dB (0 dB is the full target).
    pub current_level_db: f64,
    /// Line spacing, Hz.
    pub df_hz: f64,
    /// Lines the limiter is holding down this loop. Empty means none;
    /// otherwise one entry per line.
    pub notched_bins: &'a [bool],
    /// The half-open in-band window `[lo, hi)` in lines.
    pub band: (usize, usize),
    /// Degrees of freedom of the spectral estimate behind `measured_psd`.
    pub dof: f64,
    /// 0 for the seed, then 1, 2, ... for each iteration.
    pub loop_index: u64,
}

/// One named number a law reported for a loop.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticValue {
    /// A key the manifest declares.
    pub key: String,
    /// The value. Always finite.
    pub value: f64,
}

/// What the law reported about a loop besides its drive. Mirrors the
/// controller's `LoopDiagnostics` field for field, plus the law's own named
/// values. Shown and recorded; never relied on for protection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoopDiagnostics {
    /// Lines whose update was skipped as untrusted.
    pub untrusted_bins: Vec<usize>,
    /// Lines whose update hit the law's own step limit.
    pub clamped_bins: Vec<usize>,
    /// Lines the law judges divergent.
    pub divergent_bins: Vec<usize>,
    /// In-band error between measured and target, dB.
    pub inband_err_db: Option<f64>,
    /// In-band RMS of the measurement.
    pub inband_rms_g: Option<f64>,
    /// Whether the law's level loop is holding on a fault.
    pub servo_faulted: bool,
    /// The law's declared diagnostics, by key.
    pub values: Vec<DiagnosticValue>,
}

impl LoopDiagnostics {
    /// The value reported under `key`, if any.
    pub fn value(&self, key: &str) -> Option<f64> {
        self.values.iter().find(|v| v.key == key).map(|v| v.value)
    }
}

/// One loop's result, checked by the host: the drive has one finite,
/// non-negative value per line, the level command is finite, every reported
/// line is on the grid and every named value is declared and finite.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopOutputs {
    /// Drive PSD per line, before the controller's limits.
    pub drive_psd: Vec<f64>,
    /// The level the law commands, dB.
    pub level_cmd_db: f64,
    /// What the law reported.
    pub diagnostics: LoopDiagnostics,
}

/// Which law this is: its declared name, version and author, and the SHA-256
/// of the exact file it was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LawIdentity {
    /// From the manifest.
    pub name: String,
    /// From the manifest.
    pub version: String,
    /// From the manifest.
    pub author: String,
    /// SHA-256 of the law file, 64 lower-case hex digits.
    pub sha256: String,
}

impl LawIdentity {
    /// The first 12 hex digits of the hash, for a report line.
    pub fn short_hash(&self) -> &str {
        self.sha256.get(..12).unwrap_or(&self.sha256)
    }
}

/// What the law wrote to stdout and stderr, as far as the host kept it: the
/// last bytes of each, up to the configured capture size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestOutput {
    /// The kept tail of stdout, lossily decoded as UTF-8.
    pub stdout: String,
    /// The kept tail of stderr, lossily decoded as UTF-8.
    pub stderr: String,
    /// Bytes of stdout written but no longer kept.
    pub stdout_dropped: u64,
    /// Bytes of stderr written but no longer kept.
    pub stderr_dropped: u64,
}
