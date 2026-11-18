//! MIMO Sine laws: the `sine-mimo` world (several drives, several control
//! channels, one tone at a time, one complex amplitude per drive each control
//! update).
//!
//! A MIMO Sine law is called once per control update on the controller's
//! real-time thread: [`SineMimoLaw::engage`] for the first drive after
//! identification, [`SineMimoLaw::tick`] for every update after it. It returns
//! one complex amplitude per drive, in volts peak at the scheduled level, its
//! own fault judgement, up to [`MAX_VALUES`] values its manifest declares, and
//! optionally a request to hold or slow the sweep and a notched reference.
//!
//! The SDK hands the law plain Rust ([`Setup`], [`Inputs`]) and takes plain
//! Rust back ([`Outputs`]). The identification is a [`MatrixStack`] of
//! `controls x drives` per line; complex values are [`C64`]. The same values
//! convert to the arguments of the built-in laws' mathematics
//! ([`Setup::to_math`], [`Inputs::to_math`], [`Outputs::from_math`]), so a law
//! built on [`math`] is a few lines here (see [`builtin`]).
//!
//! The controller checks what a law returns before it plays it: one finite
//! drive per drive, a slow request's factor, a notched reference's length and
//! the declared values. It caps each drive's change and rise, scales the whole
//! drive vector for the limit channels and the drive ceilings, and rebases the
//! law on the drive it played. A law needs none of that for safety; its answer
//! is a request.
//!
//! ```ignore
//! use hertzforge_law_sdk::sine_mimo::*;
//! use hertzforge_law_sdk::{export_sine_mimo_law, Params};
//!
//! struct Keep { drives: usize }
//!
//! impl SineMimoLaw for Keep {
//!     fn describe() -> SineMimoManifest {
//!         ManifestBuilder::new("keep", "0.1.0")
//!             .summary("Keeps the drive in force.")
//!             .shapes(&ALL_SHAPES)
//!             .test_types(&TestType::ALL)
//!             .amplitude_only(true)
//!             .build()
//!     }
//!     fn new(setup: Setup, _params: Params) -> Result<Self, String> {
//!         Ok(Keep { drives: setup.drives })
//!     }
//!     fn engage(&mut self, _inputs: &Inputs) -> Result<Outputs, String> {
//!         Ok(Outputs::drive(vec![C64::new(0.0, 0.0); self.drives]))
//!     }
//!     fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
//!         Ok(Outputs::drive(inputs.drive_in_force.clone()))
//!     }
//! }
//!
//! export_sine_mimo_law!(Keep);
//! ```

use crate::__bindings::hertzforge::control_law::types::{
    ChoiceSpec, DiagnosticSpec, IntegerSpec, Manifest, NumberSpec, ParamKind, ParamSpec,
};
use crate::__sine_mimo_bindings::exports::hertzforge::control_law::sine_mimo_law as wit_law;
use crate::{DiagnosticValue, Param, Params};
use core::cell::RefCell;

/// The world's records, as the interface generates them: the setup, the
/// inputs and outputs of one update, the request, the hold, the shapes and
/// the test types, and the manifest. A law written with this module does not
/// need them; [`Setup`], [`Inputs`] and [`Outputs`] are their plain forms.
pub mod wit {
    pub use crate::__sine_mimo_bindings::hertzforge::control_law::mimo_sine_types::{
        Complex, Hold, Identification, MatrixStack, ProportionalTracking, Request, SineMimoInputs,
        SineMimoOutputs, SineMimoSetup, SineShape, TestType, Tolerance, Tracking,
    };
}

pub use crate::__sine_mimo_bindings::hertzforge::control_law::mimo_sine_types::SineMimoManifest;

/// The MIMO Sine laws' mathematics: the built-in laws (`matrix`, `paired`,
/// `steepest`, `secant`, `ratio`), the loop-speed rule, the model the carry
/// runs through, the regularized inverse and the readings for several
/// shakers. Plain math that builds for `wasm32-wasip2`.
pub use hertzforge_mimo_sine_law_math as math;
pub use hertzforge_mimo_sine_law_math::{
    Hold, LoopDelay, MatrixStack, Request, Shape, TrackingRule, C64,
};

pub mod builtin;

/// The most values a MIMO Sine law may report on one call.
pub const MAX_VALUES: usize = 8;

// ---------------------------------------------------------------------------
// Plain values

/// The kind of test a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestType {
    /// The tone sweeps the band.
    Sweep,
    /// The tone stays at one frequency.
    Dwell,
    /// The tone steps through a list of frequencies.
    Stepped,
}

impl TestType {
    /// Every test type.
    pub const ALL: [TestType; 3] = [TestType::Sweep, TestType::Dwell, TestType::Stepped];

    /// The stable name: `sweep`, `dwell` or `stepped`.
    pub fn as_str(self) -> &'static str {
        match self {
            TestType::Sweep => "sweep",
            TestType::Dwell => "dwell",
            TestType::Stepped => "stepped",
        }
    }

    /// The test types a built-in law's mathematics accepts, in this order.
    pub fn of(t: math::TestTypes) -> Vec<TestType> {
        [
            (t.sweep, TestType::Sweep),
            (t.dwell, TestType::Dwell),
            (t.stepped, TestType::Stepped),
        ]
        .into_iter()
        .filter_map(|(on, k)| on.then_some(k))
        .collect()
    }

    fn to_wit(self) -> wit::TestType {
        match self {
            TestType::Sweep => wit::TestType::Sweep,
            TestType::Dwell => wit::TestType::Dwell,
            TestType::Stepped => wit::TestType::Stepped,
        }
    }

    fn from_wit(t: wit::TestType) -> TestType {
        match t {
            wit::TestType::Sweep => TestType::Sweep,
            wit::TestType::Dwell => TestType::Dwell,
            wit::TestType::Stepped => TestType::Stepped,
        }
    }
}

/// Every shape, in the order the world lists them.
pub const ALL_SHAPES: [Shape; 3] = [Shape::Square, Shape::Tall, Shape::Wide];

fn shape_to_wit(s: Shape) -> wit::SineShape {
    match s {
        Shape::Square => wit::SineShape::Square,
        Shape::Tall => wit::SineShape::MoreControlsThanDrives,
        Shape::Wide => wit::SineShape::FewerControlsThanDrives,
    }
}

fn shape_from_wit(s: wit::SineShape) -> Shape {
    match s {
        wit::SineShape::Square => Shape::Square,
        wit::SineShape::MoreControlsThanDrives => Shape::Tall,
        wit::SineShape::FewerControlsThanDrives => Shape::Wide,
    }
}

/// The stable name of a shape: `square`, `more_controls_than_drives` or
/// `fewer_controls_than_drives`.
pub fn shape_name(s: Shape) -> &'static str {
    match s {
        Shape::Square => "square",
        Shape::Tall => "more_controls_than_drives",
        Shape::Wide => "fewer_controls_than_drives",
    }
}

fn c64(c: &wit::Complex) -> C64 {
    C64::new(c.re, c.im)
}

fn wit_c(c: &C64) -> wit::Complex {
    wit::Complex { re: c.re, im: c.im }
}

/// The identification taken before the run.
#[derive(Debug, Clone, PartialEq)]
pub struct Identification {
    /// The grid, Hz, ascending.
    pub frequency_hz: Vec<f64>,
    /// Every control's response to every drive, EU pk per volt pk:
    /// `controls x drives` on every line.
    pub response: MatrixStack,
    /// Every limit channel's response to every drive: `limits x drives` on
    /// every line.
    pub limit_response: MatrixStack,
    /// Each control's multiple coherence, `controls x lines`, control after
    /// control.
    pub coherence: Vec<f64>,
    /// Each limit channel's multiple coherence, `limits x lines`.
    pub limit_coherence: Vec<f64>,
    /// Whether the controller trusts each line.
    pub trusted: Vec<bool>,
}

/// Each control's alarm and abort bands on the identification's grid, in
/// positive dB below and above the reference, `controls x lines` each.
#[derive(Debug, Clone, PartialEq)]
pub struct Bands {
    /// Alarm band below.
    pub alarm_low_db: Vec<f64>,
    /// Alarm band above.
    pub alarm_high_db: Vec<f64>,
    /// Abort band below.
    pub abort_low_db: Vec<f64>,
    /// Abort band above.
    pub abort_high_db: Vec<f64>,
}

/// What a MIMO Sine law is built for: fixed for the run.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    /// Control channels.
    pub controls: usize,
    /// Drives.
    pub drives: usize,
    /// Limit channels.
    pub limits: usize,
    /// The station's sample rate, Hz.
    pub fs_hz: f64,
    /// The control update interval, s.
    pub tick_s: f64,
    /// The band the run moves in, Hz.
    pub band_lo_hz: f64,
    /// The band the run moves in, Hz.
    pub band_hi_hz: f64,
    /// The kind of test.
    pub test_type: TestType,
    /// The count of controls against the count of drives.
    pub shape: Shape,
    /// Each drive's ceiling, volts peak.
    pub drive_ceiling_v: Vec<f64>,
    /// The identification.
    pub identification: Identification,
    /// The level the identification was taken at, dB against full level; not
    /// finite when the controller could not measure it.
    pub identification_level_db: f64,
    /// The full-level reference of every control on the identification's
    /// grid, `controls x lines`, control after control, EU pk.
    pub reference: Vec<C64>,
    /// Whether the reference's phases are part of the test.
    pub phase_specified: bool,
    /// Each limit channel's bound on the grid, `limits x lines`; infinite
    /// where a channel has none.
    pub limit_bounds: Vec<f64>,
    /// Each control's alarm and abort bands.
    pub bands: Bands,
    /// The tracking filter's bandwidth rule.
    pub tracking: TrackingRule,
    /// The tracker's delay factor and the transport delay the identification
    /// does not show.
    pub loop_delay: LoopDelay,
}

/// Each control's tightest band over its lines, below and above.
fn tightest(low: &[f64], high: &[f64], controls: usize, lines: usize) -> Vec<f64> {
    (0..controls)
        .map(|c| {
            let r = c * lines..(c + 1) * lines;
            low.get(r.clone())
                .unwrap_or(&[])
                .iter()
                .chain(high.get(r).unwrap_or(&[]))
                .copied()
                .fold(f64::INFINITY, f64::min)
        })
        .collect()
}

impl Setup {
    /// Lines on the identification's grid.
    pub fn lines(&self) -> usize {
        self.identification.frequency_hz.len()
    }

    /// The setup as the built-in laws' mathematics takes it. Each control's
    /// alarm and abort band there is one number: the tightest of its bands,
    /// below and above, over the lines.
    pub fn to_math(&self) -> math::Setup {
        let id = &self.identification;
        let (nc, lines) = (self.controls, self.lines());
        let b = &self.bands;
        math::Setup {
            controls: self.controls,
            drives: self.drives,
            limits: self.limits,
            fs_hz: self.fs_hz,
            tick_s: self.tick_s,
            band_lo_hz: self.band_lo_hz,
            band_hi_hz: self.band_hi_hz,
            drive_ceiling_v: self.drive_ceiling_v.clone(),
            identification: math::Identification {
                grid_hz: id.frequency_hz.clone(),
                response: id.response.clone(),
                limit_response: id.limit_response.clone(),
                coherence: id.coherence.clone(),
                limit_coherence: id.limit_coherence.clone(),
                trusted: id.trusted.clone(),
            },
            identification_level_db: self.identification_level_db,
            reference: self.reference.clone(),
            phase_specified: self.phase_specified,
            limit_bounds: self.limit_bounds.clone(),
            alarm_db: tightest(&b.alarm_low_db, &b.alarm_high_db, nc, lines),
            abort_db: tightest(&b.abort_low_db, &b.abort_high_db, nc, lines),
            shape: self.shape,
            tracking: self.tracking,
            loop_delay: self.loop_delay,
        }
    }
}

/// Everything a MIMO Sine law sees on one update.
#[derive(Debug, Clone, PartialEq)]
pub struct Inputs {
    /// Updates since the law was built: 0 at `engage`.
    pub index: u64,
    /// Seconds since the last call.
    pub dt_s: f64,
    /// The tone's frequency this update, Hz.
    pub frequency_hz: f64,
    /// The sweep rate, Hz per second, signed (0 at a dwell, a step or a hold).
    pub sweep_hz_per_s: f64,
    /// The same rate in octaves per minute, signed.
    pub sweep_oct_per_min: f64,
    /// The scheduled level as a fraction of full level.
    pub level: f64,
    /// Every control's reference at this frequency and level, EU pk.
    pub reference: Vec<C64>,
    /// Every control's tracked amplitude, EU pk.
    pub controls: Vec<C64>,
    /// Every limit channel's tracked amplitude, in its units, peak.
    pub limits: Vec<C64>,
    /// Every limit channel's bound at this frequency; infinite where it has
    /// none.
    pub limit_bounds: Vec<f64>,
    /// The drive in force, volts peak, as played after every guard. Zero at
    /// `engage`.
    pub drive_in_force: Vec<C64>,
    /// Why the controller holds the drive this update.
    pub hold: Hold,
    /// The scale the limit channels hold the whole drive vector to, 0 to 1.
    pub notch_scale: f64,
    /// Whether the controller's settle window is open.
    pub settling: bool,
}

impl Inputs {
    /// The inputs as the built-in laws' mathematics takes them.
    pub fn to_math(&self) -> math::Inputs<'_> {
        math::Inputs {
            index: self.index,
            dt_s: self.dt_s,
            f_hz: self.frequency_hz,
            sweep_hz_s: self.sweep_hz_per_s,
            sweep_oct_min: self.sweep_oct_per_min,
            level: self.level,
            reference: &self.reference,
            controls: &self.controls,
            limits: &self.limits,
            limit_bounds: &self.limit_bounds,
            drive_in_force: &self.drive_in_force,
            hold: self.hold,
            notch_scale: self.notch_scale,
            settle_open: self.settling,
        }
    }
}

/// One update's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Outputs {
    /// The drive, volts peak: one complex amplitude per drive.
    pub drive: Vec<C64>,
    /// The law's own judgement that it is faulted (shown, never relied on).
    pub faulted: bool,
    /// Values under keys the manifest declares, at most [`MAX_VALUES`].
    pub values: Vec<(String, f64)>,
    /// A request to hold or slow the sweep.
    pub request: Request,
    /// A lowered reference, one per control, or none.
    pub notched_reference: Option<Vec<C64>>,
}

impl Outputs {
    /// A drive and nothing else.
    pub fn drive(drive: Vec<C64>) -> Self {
        Outputs {
            drive,
            faulted: false,
            values: Vec::new(),
            request: Request::None,
            notched_reference: None,
        }
    }

    /// A built-in law's answer, its values named in declaration order. A value
    /// that is not a finite number is left out (the controller refuses a whole
    /// answer for one).
    pub fn from_math(o: &math::Outputs, names: &[&str]) -> Self {
        Outputs {
            drive: o.drive.clone(),
            faulted: o.fault,
            values: names
                .iter()
                .zip(&o.values)
                .filter(|(_, v)| v.is_finite())
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect(),
            request: o.request,
            notched_reference: None,
        }
    }
}

// ---------------------------------------------------------------------------
// The interface's flat records to plain values and back

fn stack(m: &wit::MatrixStack) -> Result<MatrixStack, String> {
    MatrixStack::from_vec(
        m.lines as usize,
        m.rows as usize,
        m.cols as usize,
        m.data.iter().map(c64).collect(),
    )
    .map_err(|e| e.0)
}

fn hold_from_wit(h: wit::Hold) -> Hold {
    match h {
        wit::Hold::Free => Hold::Free,
        wit::Hold::Notch => Hold::Notch,
        wit::Hold::Held => Hold::Held,
        wit::Hold::Probe => Hold::Probe,
        wit::Hold::Engage => Hold::Engage,
    }
}

fn bound(b: &Option<f64>) -> f64 {
    b.unwrap_or(f64::INFINITY)
}

/// The interface's setup as plain values. A proportional tracking share
/// (0 to 1) becomes the mathematics' percentage.
pub fn setup_from_wit(s: wit::SineMimoSetup) -> Result<Setup, String> {
    let id = &s.identification;
    let limits = s.limits as usize;
    let lines = id.frequency_hz.len();
    let limit_response = if limits == 0 {
        MatrixStack::zeros(lines, 0, s.drives as usize)
    } else {
        stack(&id.limit_response)?
    };
    Ok(Setup {
        controls: s.controls as usize,
        drives: s.drives as usize,
        limits,
        fs_hz: s.fs_hz,
        tick_s: s.tick_s,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        test_type: TestType::from_wit(s.test_type),
        shape: shape_from_wit(s.shape),
        drive_ceiling_v: s.drive_ceiling_v,
        identification: Identification {
            frequency_hz: id.frequency_hz.clone(),
            response: stack(&id.response)?,
            limit_response,
            coherence: id.coherence.clone(),
            limit_coherence: id.limit_coherence.clone(),
            trusted: id.trusted.clone(),
        },
        identification_level_db: s.identification_level_db,
        reference: s.reference.iter().map(c64).collect(),
        phase_specified: s.phase_specified,
        limit_bounds: s.limit_bounds.iter().map(bound).collect(),
        bands: Bands {
            alarm_low_db: s.tolerance.alarm_low_db,
            alarm_high_db: s.tolerance.alarm_high_db,
            abort_low_db: s.tolerance.abort_low_db,
            abort_high_db: s.tolerance.abort_high_db,
        },
        tracking: match s.tracking {
            wit::Tracking::Proportional(p) => TrackingRule::Proportional {
                pct: p.share * 100.0,
                max_hz: p.max_hz,
            },
            wit::Tracking::Fixed(hz) => TrackingRule::Fixed { hz },
        },
        loop_delay: LoopDelay {
            tracker_delay_factor: s.tracker_delay_factor,
            transport_delay_s: s.transport_delay_s,
        },
    })
}

/// One update's interface inputs as plain values.
pub fn inputs_from_wit(i: wit::SineMimoInputs) -> Inputs {
    Inputs {
        index: i.index,
        dt_s: i.dt_s,
        frequency_hz: i.frequency_hz,
        sweep_hz_per_s: i.sweep_hz_per_s,
        sweep_oct_per_min: i.sweep_oct_per_min,
        level: i.level,
        reference: i.reference.iter().map(c64).collect(),
        controls: i.controls.iter().map(c64).collect(),
        limits: i.limits.iter().map(c64).collect(),
        limit_bounds: i.limit_bounds.iter().map(bound).collect(),
        drive_in_force: i.drive_in_force.iter().map(c64).collect(),
        hold: hold_from_wit(i.hold),
        notch_scale: i.notch_scale,
        settling: i.settling,
    }
}

/// Plain outputs as the interface carries them.
pub fn outputs_to_wit(o: Outputs) -> wit::SineMimoOutputs {
    wit::SineMimoOutputs {
        drive: o.drive.iter().map(wit_c).collect(),
        faulted: o.faulted,
        values: o
            .values
            .into_iter()
            .map(|(key, value)| DiagnosticValue { key, value })
            .collect(),
        request: match o.request {
            Request::None => None,
            Request::Hold => Some(wit::Request::Hold),
            Request::Slow(f) => Some(wit::Request::Slow(f)),
        },
        notched_reference: o.notched_reference.map(|r| r.iter().map(wit_c).collect()),
    }
}

// ---------------------------------------------------------------------------
// Manifest

/// Builds a [`SineMimoManifest`]: the name, version, parameters and values
/// every law declares, and the shapes, test types and references this one
/// accepts.
#[derive(Debug, Clone)]
pub struct ManifestBuilder {
    law: Manifest,
    shapes: Vec<wit::SineShape>,
    test_types: Vec<wit::TestType>,
    amplitude_only: bool,
}

impl ManifestBuilder {
    /// A manifest named `name` at `version`, accepting no shape and no test
    /// type yet, and only references with phases.
    pub fn new(name: &str, version: &str) -> Self {
        ManifestBuilder {
            law: Manifest {
                name: name.into(),
                version: version.into(),
                author: String::new(),
                summary: String::new(),
                params: Vec::new(),
                diagnostics: Vec::new(),
            },
            shapes: Vec::new(),
            test_types: Vec::new(),
            amplitude_only: false,
        }
    }

    /// Who wrote the law.
    pub fn author(mut self, author: &str) -> Self {
        self.law.author = author.into();
        self
    }

    /// One paragraph on what the law does.
    pub fn summary(mut self, summary: &str) -> Self {
        self.law.summary = summary.into();
        self
    }

    /// The shapes the law accepts (at least one).
    pub fn shapes(mut self, shapes: &[Shape]) -> Self {
        self.shapes = shapes.iter().map(|s| shape_to_wit(*s)).collect();
        self
    }

    /// The test types the law accepts (at least one).
    pub fn test_types(mut self, types: &[TestType]) -> Self {
        self.test_types = types.iter().map(|t| t.to_wit()).collect();
        self
    }

    /// Whether the law accepts an amplitude-only reference.
    pub fn amplitude_only(mut self, accepts: bool) -> Self {
        self.amplitude_only = accepts;
        self
    }

    /// Any parameter.
    pub fn param(mut self, spec: ParamSpec) -> Self {
        self.law.params.push(spec);
        self
    }

    /// A number parameter.
    #[allow(clippy::too_many_arguments)]
    pub fn number(
        self,
        key: &str,
        label: &str,
        unit: &str,
        help: &str,
        min: f64,
        max: f64,
        step: f64,
        default: f64,
    ) -> Self {
        self.param(ParamSpec {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ParamKind::Number(NumberSpec {
                unit: unit.into(),
                min,
                max,
                step,
                default,
            }),
        })
    }

    /// An integer parameter.
    #[allow(clippy::too_many_arguments)]
    pub fn integer(
        self,
        key: &str,
        label: &str,
        unit: &str,
        help: &str,
        min: i64,
        max: i64,
        default: i64,
    ) -> Self {
        self.param(ParamSpec {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ParamKind::Integer(IntegerSpec {
                unit: unit.into(),
                min,
                max,
                step: 1,
                default,
            }),
        })
    }

    /// An on or off parameter.
    pub fn toggle(self, key: &str, label: &str, help: &str, default: bool) -> Self {
        self.param(ParamSpec {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ParamKind::Toggle(default),
        })
    }

    /// A choice among named options.
    pub fn choice(
        self,
        key: &str,
        label: &str,
        help: &str,
        options: &[&str],
        default: &str,
    ) -> Self {
        self.param(ParamSpec {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ParamKind::Choice(ChoiceSpec {
                options: options.iter().map(|o| (*o).to_string()).collect(),
                default: default.into(),
            }),
        })
    }

    /// A value the law may report on a call.
    pub fn value(mut self, key: &str, label: &str, unit: &str, help: &str) -> Self {
        self.law.diagnostics.push(DiagnosticSpec {
            key: key.into(),
            label: label.into(),
            unit: unit.into(),
            help: help.into(),
        });
        self
    }

    /// The manifest.
    pub fn build(self) -> SineMimoManifest {
        SineMimoManifest {
            law: self.law,
            shapes: self.shapes,
            test_types: self.test_types,
            amplitude_only: self.amplitude_only,
        }
    }
}

/// The shapes a manifest accepts, in plain form.
pub fn manifest_shapes(m: &SineMimoManifest) -> Vec<Shape> {
    m.shapes.iter().map(|s| shape_from_wit(*s)).collect()
}

/// The test types a manifest accepts, in plain form.
pub fn manifest_test_types(m: &SineMimoManifest) -> Vec<TestType> {
    m.test_types
        .iter()
        .map(|t| TestType::from_wit(*t))
        .collect()
}

// ---------------------------------------------------------------------------
// The law

/// A MIMO Sine control law.
///
/// The host builds one value per run with [`Self::new`], after
/// identification, calls [`Self::engage`] once for the first drive, then
/// [`Self::tick`] every control update. Calls never overlap.
pub trait SineMimoLaw: Sized + 'static {
    /// The law's name, version, parameters, values, shapes, test types and
    /// whether it accepts an amplitude-only reference. Read once, when the
    /// file is loaded; it must not depend on anything but the code. Build it
    /// with [`ManifestBuilder`].
    fn describe() -> SineMimoManifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest and completed with defaults. An
    /// error refuses the run with its text as the reason. Work done here
    /// (fitting a model) has a longer budget than an update.
    fn new(setup: Setup, params: Params) -> Result<Self, String>;

    /// The first update after identification: the first drive.
    /// `inputs.drive_in_force` is zero on every drive.
    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String>;

    /// One control update: the drive.
    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String>;
}

/// The resource the interface exports: a [`SineMimoLaw`] behind a
/// `RefCell`.
#[doc(hidden)]
pub struct SineMimoLawResource<L: SineMimoLaw> {
    law: RefCell<L>,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: SineMimoLaw> wit_law::GuestLaw for SineMimoLawResource<L> {
    fn new(setup: wit::SineMimoSetup, params: Vec<Param>) -> Result<Self, String> {
        let setup = setup_from_wit(setup)?;
        L::new(setup, Params(params)).map(|law| SineMimoLawResource {
            law: RefCell::new(law),
        })
    }

    fn engage(&self, inputs: wit::SineMimoInputs) -> Result<wit::SineMimoOutputs, String> {
        let inputs = inputs_from_wit(inputs);
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .engage(&inputs)
            .map(outputs_to_wit)
    }

    fn tick(&self, inputs: wit::SineMimoInputs) -> Result<wit::SineMimoOutputs, String> {
        let inputs = inputs_from_wit(inputs);
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .tick(&inputs)
            .map(outputs_to_wit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_the_builder_states_shapes_test_types_and_references() {
        let m = ManifestBuilder::new("t", "1")
            .shapes(&[Shape::Square, Shape::Wide])
            .test_types(&[TestType::Dwell])
            .amplitude_only(true)
            .value("gain", "Gain", "", "")
            .build();
        assert_eq!(manifest_shapes(&m), [Shape::Square, Shape::Wide]);
        assert_eq!(manifest_test_types(&m), [TestType::Dwell]);
        assert!(m.amplitude_only);
        assert_eq!(m.law.diagnostics[0].key, "gain");
        let none = ManifestBuilder::new("t", "1").build();
        assert!(!none.amplitude_only && none.shapes.is_empty());
        assert_eq!(
            TestType::of(math::TestTypes {
                sweep: false,
                dwell: true,
                stepped: true
            }),
            [TestType::Dwell, TestType::Stepped]
        );
    }

    #[test]
    fn mimo_sine_a_proportional_share_is_the_maths_percentage() {
        let z = wit::Complex { re: 0.0, im: 0.0 };
        let one = |n: usize| wit::MatrixStack {
            lines: 2,
            rows: n as u32,
            cols: 1,
            data: vec![z; 2 * n],
        };
        let s = wit::SineMimoSetup {
            controls: 1,
            drives: 1,
            limits: 1,
            fs_hz: 1000.0,
            tick_s: 0.01,
            band_lo_hz: 10.0,
            band_hi_hz: 20.0,
            test_type: wit::TestType::Sweep,
            shape: wit::SineShape::Square,
            drive_ceiling_v: vec![1.0],
            identification: wit::Identification {
                frequency_hz: vec![10.0, 20.0],
                response: one(1),
                limit_response: one(1),
                coherence: vec![1.0; 2],
                limit_coherence: vec![1.0; 2],
                trusted: vec![true; 2],
            },
            identification_level_db: -20.0,
            reference: vec![z; 2],
            phase_specified: false,
            limit_bounds: vec![Some(2.0), None],
            tolerance: wit::Tolerance {
                alarm_low_db: vec![3.0, 2.0],
                alarm_high_db: vec![4.0, 2.5],
                abort_low_db: vec![6.0, 6.0],
                abort_high_db: vec![9.0, 5.0],
            },
            tracking: wit::Tracking::Proportional(wit::ProportionalTracking {
                share: 0.1,
                max_hz: 20.0,
            }),
            tracker_delay_factor: 1.0 / std::f64::consts::PI,
            transport_delay_s: 0.0,
        };
        let p = setup_from_wit(s).unwrap();
        assert_eq!(
            p.tracking,
            TrackingRule::Proportional {
                pct: 10.0,
                max_hz: 20.0
            }
        );
        assert_eq!(p.limit_bounds, [2.0, f64::INFINITY]);
        let m = p.to_math();
        assert_eq!(m.alarm_db, [2.0]);
        assert_eq!(m.abort_db, [5.0]);
        assert_eq!(m.shape, Shape::Square);
    }

    #[test]
    fn mimo_sine_a_value_that_is_not_finite_is_not_reported() {
        let mut o = math::Outputs::new(1, 3);
        o.values = vec![1.0, f64::INFINITY, 2.0];
        o.request = Request::Slow(0.5);
        let p = Outputs::from_math(&o, &["a", "b", "c"]);
        assert_eq!(p.values, [("a".to_string(), 1.0), ("c".to_string(), 2.0)]);
        let w = outputs_to_wit(p);
        assert_eq!(w.request, Some(wit::Request::Slow(0.5)));
        assert_eq!(w.values.len(), 2);
    }
}
