//! MIMO Shock laws: the `shock-mimo` world (several drives, several control
//! channels, one pulse at a time, a classical pulse or a shock response
//! spectrum (SRS) at every control).
//!
//! A MIMO Shock law is called between pulses, never while one plays: once
//! after system identification ([`ShockMimoLaw::seed`], for the first level of
//! the ladder) and once after each fired, measured and judged pulse
//! ([`ShockMimoLaw::iterate`], after the controller has scheduled the next
//! level). It returns the whole drive record of every drive for the next pulse,
//! in volts at the next level, and up to eight values its manifest declares.
//!
//! The SDK hands the law plain Rust ([`Setup`], [`Inputs`]) and takes plain
//! Rust back ([`Outputs`]): records as one row per channel, the plant as a
//! [`MatrixStack`] of `controls x drives` per line. The interface's flat
//! layout never reaches the law. The same values convert to the arguments of
//! the shared law mathematics ([`Setup::to_math`], [`Inputs::to_math`],
//! [`Outputs::from_math`]), so a law built on [`math`] is a few lines here.
//!
//! The controller checks what a law returns before it plays: the drive count
//! and record length, finite values, the declared values, each drive's
//! ceiling, the growth of each drive from one pulse to the next and the
//! predicted response against the abort lines. A law needs none of those for
//! safety; its answer is a request.
//!
//! ```ignore
//! use hertzforge_law_sdk::shock_mimo::*;
//! use hertzforge_law_sdk::{export_shock_mimo_law, Params};
//!
//! struct Silence { drives: usize, len: usize }
//!
//! impl ShockMimoLaw for Silence {
//!     fn describe() -> ShockMimoManifest {
//!         ManifestBuilder::new("silence", "0.1.0")
//!             .summary("Plays nothing.")
//!             .targets(&[TargetKind::Waveform, TargetKind::Srs])
//!             .build()
//!     }
//!     fn new(setup: Setup, _params: Params) -> Result<Self, String> {
//!         Ok(Silence { drives: setup.n_drives, len: setup.record_len })
//!     }
//!     fn seed(&mut self, _level_db: f64) -> Result<Outputs, String> {
//!         Ok(Outputs::drives_only(vec![vec![0.0; self.len]; self.drives]))
//!     }
//!     fn iterate(&mut self, _inputs: &Inputs) -> Result<Outputs, String> {
//!         Ok(Outputs::drives_only(vec![vec![0.0; self.len]; self.drives]))
//!     }
//! }
//!
//! export_shock_mimo_law!(Silence);
//! ```

use crate::__bindings::hertzforge::control_law::types::{
    ChoiceSpec, DiagnosticSpec, IntegerSpec, Manifest, NumberSpec, ParamKind, ParamSpec,
};
use crate::__shock_mimo_bindings::exports::hertzforge::control_law::shock_mimo_law as wit_law;
use crate::__shock_mimo_bindings::hertzforge::control_law::mimo_shock_types as wit;
use crate::{DiagnosticValue, Param, Params};
use core::cell::RefCell;

pub use crate::__shock_mimo_bindings::hertzforge::control_law::mimo_shock_types::MimoShockManifest as ShockMimoManifest;
pub use hertzforge_mimo_shock_law_math as math;
/// Transforms on the pulse grid in the world's convention: `rfft`, `irfft`,
/// their row forms with a scale, `apply` (a matrix stack times spectra),
/// `smooth`, `rms_all`.
pub use hertzforge_mimo_shock_law_math::spectrum;
pub use hertzforge_mimo_shock_law_math::{level_scale, LawSteps, MatrixStack, C64};

/// What a test asks of its control channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// A time history at every control: a compensated classical pulse.
    Waveform,
    /// A shock response spectrum at every control.
    Srs,
}

impl TargetKind {
    /// The stable name: `waveform` or `srs`.
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Waveform => "waveform",
            TargetKind::Srs => "srs",
        }
    }

    fn to_wit(self) -> wit::TargetKind {
        match self {
            TargetKind::Waveform => wit::TargetKind::Waveform,
            TargetKind::Srs => wit::TargetKind::Srs,
        }
    }
}

/// The decay of one synthesis component.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComponentShape {
    /// A damped sinusoid with this damping ratio.
    Decay(f64),
    /// A windowed sine of this many half-cycles.
    HalfCycles(u32),
}

/// One component of a control's SRS synthesis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SrsComponent {
    /// Frequency, Hz.
    pub hz: f64,
    /// Peak amplitude at full level, EU; negative for an inverted component.
    pub amplitude: f64,
    /// Start after the record's start, s.
    pub delay_s: f64,
    /// Its decay.
    pub shape: ComponentShape,
}

/// An SRS target. The waveform the controller synthesized for each control is
/// [`Setup::reference`].
#[derive(Debug, Clone, PartialEq)]
pub struct SrsTarget {
    /// The SRS natural frequencies, Hz, ascending.
    pub natural_hz: Vec<f64>,
    /// The SRS quality factor (10 for 5 % damping).
    pub q: f64,
    /// The full-level target SRS, one row per control on `natural_hz`, EU peak.
    pub points: Vec<Vec<f64>>,
    /// The tolerance below the target, dB (negative).
    pub tol_lo_db: f64,
    /// The tolerance above the target, dB.
    pub tol_hi_db: f64,
    /// The synthesis table behind each control's waveform, one per control.
    pub components: Vec<Vec<SrsComponent>>,
}

/// The target of a test.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A waveform at every control: [`Setup::reference`].
    Waveform,
    /// An SRS at every control, with the waveform synthesized for it in
    /// [`Setup::reference`].
    Srs(SrsTarget),
}

impl Target {
    /// The target's kind.
    pub fn kind(&self) -> TargetKind {
        match self {
            Target::Waveform => TargetKind::Waveform,
            Target::Srs(_) => TargetKind::Srs,
        }
    }
}

/// How the arrival times of the controls are treated.
#[derive(Debug, Clone, PartialEq)]
pub enum Timing {
    /// Each control is judged on its own; its delay is searched over
    /// `window_s` and reported.
    Free {
        /// The search window, s.
        window_s: f64,
    },
    /// Each control is held to a set delay inside a window.
    Hold {
        /// The delay each control is held to, s, one per control.
        delays_s: Vec<f64>,
        /// The window a measured delay may move within, s.
        window_s: f64,
    },
}

/// The controller's plant estimate on the pulse grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Plant {
    /// `H`, `record_len / 2 + 1` lines of `controls x drives`, EU per volt.
    /// The path delays are in it.
    pub frf: MatrixStack,
    /// Multiple coherence of each control on all drives, one row per control,
    /// one value per line, 0 to 1.
    pub coherence: Vec<Vec<f64>>,
    /// The lines the controller trusts: in the band, coherent enough.
    pub trusted: Vec<bool>,
}

/// What a law is built for: fixed for the run.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    /// Sample rate, Hz.
    pub fs_hz: f64,
    /// Samples in every record, a power of two.
    pub record_len: usize,
    /// Drives the law commands.
    pub n_drives: usize,
    /// Control channels.
    pub n_controls: usize,
    /// The band the law corrects, Hz.
    pub band_lo_hz: f64,
    /// Top of the band, Hz.
    pub band_hi_hz: f64,
    /// The full-level waveform every control is to follow, one row per
    /// control: the target itself, or the waveform synthesized for an SRS.
    pub reference: Vec<Vec<f64>>,
    /// The kind of target, and for an SRS its spectrum and tolerance.
    pub target: Target,
    /// The timing mode.
    pub timing: Timing,
    /// The identified plant.
    pub plant: Plant,
    /// Each drive's ceiling, volts peak. The controller scales a drive over it
    /// down.
    pub drive_ceiling_v: Vec<f64>,
    /// The level ladder, dB, ascending, ending at 0.
    pub ladder_db: Vec<f64>,
}

/// The controller's judgement of one control on the last pulse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    /// Inside the control's tolerance at its level.
    pub passed: bool,
    /// The worst deviation from the target, dB.
    pub error_db: f64,
    /// The control's measured delay against its target, s.
    pub delay_s: f64,
    /// Measured peak over the target's peak at the played level.
    pub peak_ratio: f64,
}

/// What a law sees after a pulse.
#[derive(Debug, Clone, PartialEq)]
pub struct Inputs {
    /// Pulses fired so far: 1 for the first iteration.
    pub pulse_index: u64,
    /// The level the last pulse was played at, dB.
    pub level_db: f64,
    /// The level the controller has scheduled for the next pulse, dB.
    pub next_level_db: f64,
    /// The drives as played, one row per drive, volts.
    pub drives: Vec<Vec<f64>>,
    /// The measured responses, one row per control, EU.
    pub responses: Vec<Vec<f64>>,
    /// The measured SRS of every control on the target's natural frequencies,
    /// at the played level. Present for an SRS target only.
    pub srs: Option<Vec<Vec<f64>>>,
    /// The controller's judgement of each control.
    pub verdicts: Vec<Verdict>,
    /// The controller scaled the last pulse down.
    pub limited: bool,
}

/// What a law returns for the next pulse.
#[derive(Debug, Clone, PartialEq)]
pub struct Outputs {
    /// The next drives at the next level, one row of `record_len` samples per
    /// drive, volts.
    pub drives: Vec<Vec<f64>>,
    /// The delay the law expects at each control, s, when it has one.
    pub delays_s: Option<Vec<f64>>,
    /// At most eight values, each under a key the manifest declares.
    pub values: Vec<(String, f64)>,
}

impl Outputs {
    /// Drives with no delays and no values.
    pub fn drives_only(drives: Vec<Vec<f64>>) -> Self {
        Outputs {
            drives,
            delays_s: None,
            values: Vec::new(),
        }
    }

    /// The outputs of a law step of [`math`].
    pub fn from_math(o: math::PulseOutputs) -> Self {
        Outputs {
            drives: o.drives,
            delays_s: o.delays_s,
            values: o
                .values
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }
}

impl Setup {
    /// The setup as the law steps of [`math`] take it.
    pub fn to_math(&self) -> math::Setup {
        math::Setup {
            fs_hz: self.fs_hz,
            record_len: self.record_len,
            n_drives: self.n_drives,
            n_controls: self.n_controls,
            band_lo_hz: self.band_lo_hz,
            band_hi_hz: self.band_hi_hz,
            reference: self.reference.clone(),
            srs: match &self.target {
                Target::Waveform => None,
                Target::Srs(t) => Some(math::SrsTarget {
                    natural_hz: t.natural_hz.clone(),
                    q: t.q,
                    points: t.points.clone(),
                }),
            },
            plant: math::PlantModel {
                frf: self.plant.frf.clone(),
                trusted: self.plant.trusted.clone(),
            },
        }
    }
}

impl Inputs {
    /// The inputs as the law steps of [`math`] take them (whose pulse index
    /// counts from 0).
    pub fn to_math(&self) -> math::PulseInputs {
        math::PulseInputs {
            pulse_index: self.pulse_index.saturating_sub(1) as usize,
            level_db: self.level_db,
            next_level_db: self.next_level_db,
            drives: self.drives.clone(),
            responses: self.responses.clone(),
            srs: self.srs.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// The interface's flat records to and from the plain types.

/// `n` rows of `len` from a flat channel-major list.
fn rows(name: &str, flat: Vec<f64>, n: usize, len: usize) -> Result<Vec<Vec<f64>>, String> {
    if flat.len() != n * len {
        return Err(format!(
            "{name} holds {} values; {n} rows of {len} were expected",
            flat.len()
        ));
    }
    Ok(if len == 0 {
        vec![Vec::new(); n]
    } else {
        flat.chunks(len).map(<[f64]>::to_vec).collect()
    })
}

/// The interface's setup as plain Rust. Fails only on a setup whose lengths
/// disagree, which the host refuses before a law sees it.
pub fn setup_from_wit(s: wit::MimoShockSetup) -> Result<Setup, String> {
    let (nc, nd, len) = (
        s.controls as usize,
        s.drives as usize,
        s.record_len as usize,
    );
    let f = &s.plant.frf;
    let frf = MatrixStack::from_vec(
        f.lines as usize,
        f.rows as usize,
        f.cols as usize,
        f.data.iter().map(|c| C64::new(c.re, c.im)).collect(),
    )
    .map_err(|e| e.to_string())?;
    let lines = frf.lines();
    let (reference, target) = match s.target {
        wit::Target::Waveform(w) => (rows("the target", w.records, nc, len)?, Target::Waveform),
        wit::Target::Srs(t) => {
            let nf = t.natural_hz.len();
            let components = t
                .components
                .into_iter()
                .map(|cs| {
                    cs.into_iter()
                        .map(|c| SrsComponent {
                            hz: c.hz,
                            amplitude: c.amplitude,
                            delay_s: c.delay_s,
                            shape: match c.shape {
                                wit::ComponentShape::Decay(z) => ComponentShape::Decay(z),
                                wit::ComponentShape::HalfCycles(n) => ComponentShape::HalfCycles(n),
                            },
                        })
                        .collect()
                })
                .collect();
            (
                rows("the synthesized waveforms", t.waveforms, nc, len)?,
                Target::Srs(SrsTarget {
                    points: rows("the target SRS", t.points, nc, nf)?,
                    natural_hz: t.natural_hz,
                    q: t.q,
                    tol_lo_db: t.tol_lo_db,
                    tol_hi_db: t.tol_hi_db,
                    components,
                }),
            )
        }
    };
    Ok(Setup {
        fs_hz: s.fs_hz,
        record_len: len,
        n_drives: nd,
        n_controls: nc,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        reference,
        target,
        timing: match s.timing {
            wit::Timing::Free(w) => Timing::Free { window_s: w },
            wit::Timing::Hold(h) => Timing::Hold {
                delays_s: h.delays_s,
                window_s: h.window_s,
            },
        },
        plant: Plant {
            coherence: rows("the coherence", s.plant.coherence, nc, lines)?,
            trusted: s.plant.trusted,
            frf,
        },
        drive_ceiling_v: s.drive_ceiling_v,
        ladder_db: s.ladder_db,
    })
}

/// The interface's pulse inputs as plain Rust, for a run of `drives` drives,
/// `controls` controls and records of `len`.
pub fn inputs_from_wit(
    i: wit::MimoShockInputs,
    drives: usize,
    controls: usize,
    len: usize,
) -> Result<Inputs, String> {
    let srs = match i.srs {
        Some(s) => {
            let nf = if controls == 0 { 0 } else { s.len() / controls };
            Some(rows("the measured SRS", s, controls, nf)?)
        }
        None => None,
    };
    Ok(Inputs {
        pulse_index: i.pulse_index,
        level_db: i.level_db,
        next_level_db: i.next_level_db,
        drives: rows("the drives", i.drives, drives, len)?,
        responses: rows("the responses", i.responses, controls, len)?,
        srs,
        verdicts: i
            .verdicts
            .into_iter()
            .map(|v| Verdict {
                passed: v.passed,
                error_db: v.error_db,
                delay_s: v.delay_s,
                peak_ratio: v.peak_ratio,
            })
            .collect(),
        limited: i.limited,
    })
}

/// A law's outputs as the interface carries them: the rows flattened drive
/// after drive. The host checks the count and the lengths.
pub fn outputs_to_wit(o: Outputs) -> wit::MimoShockOutputs {
    wit::MimoShockOutputs {
        drives: o.drives.into_iter().flatten().collect(),
        delays_s: o.delays_s,
        values: o
            .values
            .into_iter()
            .map(|(key, value)| DiagnosticValue { key, value })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Manifest

/// Builds a [`ShockMimoManifest`]: the name, version, parameters and values
/// every law declares, and the target kinds this one accepts.
#[derive(Debug, Clone)]
pub struct ManifestBuilder {
    law: Manifest,
    targets: Vec<wit::TargetKind>,
}

impl ManifestBuilder {
    /// A manifest named `name` at `version`, accepting no target kind yet.
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
            targets: Vec::new(),
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

    /// The target kinds the law accepts (at least one).
    pub fn targets(mut self, kinds: &[TargetKind]) -> Self {
        self.targets = kinds.iter().map(|k| k.to_wit()).collect();
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

    /// A parameter with one number per control channel.
    #[allow(clippy::too_many_arguments)]
    pub fn per_control(
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
            kind: ParamKind::PerChannel(NumberSpec {
                unit: unit.into(),
                min,
                max,
                step,
                default,
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
    pub fn build(self) -> ShockMimoManifest {
        ShockMimoManifest {
            law: self.law,
            targets: self.targets,
        }
    }
}

/// The target kinds a manifest accepts, in plain form.
pub fn manifest_targets(m: &ShockMimoManifest) -> Vec<TargetKind> {
    m.targets
        .iter()
        .map(|k| match k {
            wit::TargetKind::Waveform => TargetKind::Waveform,
            wit::TargetKind::Srs => TargetKind::Srs,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The law

/// A MIMO Shock control law.
///
/// The host builds one value per run with [`Self::new`], after system
/// identification, calls [`Self::seed`] once for the first pulse, then
/// [`Self::iterate`] after each pulse. Calls never overlap.
pub trait ShockMimoLaw: Sized + 'static {
    /// The law's name, version, parameters, values and target kinds. Read
    /// once, when the file is loaded; it must not depend on anything but the
    /// code. Build it with [`ManifestBuilder`].
    fn describe() -> ShockMimoManifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest and completed with defaults (a
    /// per-channel parameter has one value per control). An error refuses the
    /// run with its text as the reason.
    fn new(setup: Setup, params: Params) -> Result<Self, String>;

    /// The drives of the first pulse, played at `level_db` (the ladder's
    /// first level).
    fn seed(&mut self, level_db: f64) -> Result<Outputs, String>;

    /// The drives of the next pulse, at `inputs.next_level_db`, from the
    /// measurements of the last.
    fn iterate(&mut self, inputs: &Inputs) -> Result<Outputs, String>;
}

/// The resource the interface exports: a [`ShockMimoLaw`] behind a
/// `RefCell`, with the run's shape kept to unpack each call's inputs.
#[doc(hidden)]
pub struct ShockMimoLawResource<L: ShockMimoLaw> {
    law: RefCell<L>,
    drives: usize,
    controls: usize,
    len: usize,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: ShockMimoLaw> wit_law::GuestLaw for ShockMimoLawResource<L> {
    fn new(setup: wit::MimoShockSetup, params: Vec<Param>) -> Result<Self, String> {
        let setup = setup_from_wit(setup)?;
        let (drives, controls, len) = (setup.n_drives, setup.n_controls, setup.record_len);
        L::new(setup, Params(params)).map(|law| ShockMimoLawResource {
            law: RefCell::new(law),
            drives,
            controls,
            len,
        })
    }

    fn seed(&self, level_db: f64) -> Result<wit::MimoShockOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .seed(level_db)
            .map(outputs_to_wit)
    }

    fn iterate(&self, inputs: wit::MimoShockInputs) -> Result<wit::MimoShockOutputs, String> {
        let inputs = inputs_from_wit(inputs, self.drives, self.controls, self.len)?;
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .iterate(&inputs)
            .map(outputs_to_wit)
    }
}

/// The declarations of the built-in MIMO Shock laws (`inverse`, `shape`,
/// `paired`): their names, parameters, values and target kinds as plain data,
/// so the law files, the controller's catalog and the native laws name the
/// same parameters. A law of your own does not need this module.
pub mod builtin {
    use super::{math, ManifestBuilder, ShockMimoManifest, TargetKind};
    use crate::Params;

    /// Who the built-in laws are credited to in their manifests.
    pub const AUTHOR: &str = "Hertzforge contributors";

    /// The kind of a declared parameter, with its range and default.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Knob {
        /// A number in `min..=max` on a grid of `step`.
        Number {
            /// Unit shown beside it.
            unit: &'static str,
            /// Least value.
            min: f64,
            /// Greatest value.
            max: f64,
            /// Grid.
            step: f64,
            /// Default.
            default: f64,
        },
        /// A whole number in `min..=max`.
        Integer {
            /// Unit shown beside it.
            unit: &'static str,
            /// Least value.
            min: i64,
            /// Greatest value.
            max: i64,
            /// Default.
            default: i64,
        },
        /// On or off.
        Toggle(bool),
        /// One of named options.
        Choice {
            /// The options.
            options: &'static [&'static str],
            /// Default.
            default: &'static str,
        },
    }

    /// One declared parameter.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ParamDecl {
        /// Key.
        pub key: &'static str,
        /// Label.
        pub label: &'static str,
        /// One sentence of help.
        pub help: &'static str,
        /// Kind, range and default.
        pub knob: Knob,
    }

    /// One declared value.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ValueDecl {
        /// Key.
        pub key: &'static str,
        /// Label.
        pub label: &'static str,
        /// Unit.
        pub unit: &'static str,
        /// One sentence of help.
        pub help: &'static str,
    }

    /// A built-in law's declaration.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct LawDecl {
        /// The catalog id.
        pub id: &'static str,
        /// The manifest name.
        pub name: &'static str,
        /// The law's version.
        pub version: &'static str,
        /// One paragraph on what it does.
        pub summary: &'static str,
        /// The target kinds it accepts.
        pub targets: &'static [TargetKind],
        /// Its parameters.
        pub params: &'static [ParamDecl],
        /// The values it reports.
        pub values: &'static [ValueDecl],
    }

    impl LawDecl {
        /// The law's manifest.
        pub fn manifest(&self) -> ShockMimoManifest {
            let mut b = ManifestBuilder::new(self.name, self.version)
                .author(AUTHOR)
                .summary(self.summary)
                .targets(self.targets);
            for p in self.params {
                b = match p.knob {
                    Knob::Number {
                        unit,
                        min,
                        max,
                        step,
                        default,
                    } => b.number(p.key, p.label, unit, p.help, min, max, step, default),
                    Knob::Integer {
                        unit,
                        min,
                        max,
                        default,
                    } => b.integer(p.key, p.label, unit, p.help, min, max, default),
                    Knob::Toggle(d) => b.toggle(p.key, p.label, p.help, d),
                    Knob::Choice { options, default } => {
                        b.choice(p.key, p.label, p.help, options, default)
                    }
                };
            }
            for v in self.values {
                b = b.value(v.key, v.label, v.unit, v.help);
            }
            b.build()
        }
    }

    const fn number(min: f64, max: f64, step: f64, default: f64, unit: &'static str) -> Knob {
        Knob::Number {
            unit,
            min,
            max,
            step,
            default,
        }
    }

    const LAMBDA: ParamDecl = ParamDecl {
        key: "lambda_rel",
        label: "Regularization",
        help: "Tikhonov regularization of the plant inverse on each line, relative to that \
               line's largest singular value. Larger is safer and less exact.",
        knob: number(1.0e-6, 1.0, 1.0e-6, 1.0e-3, ""),
    };
    const REFRESH: ParamDecl = ParamDecl {
        key: "refresh",
        label: "Refresh the model",
        help: "After every pulse, move the plant model the least amount that makes it \
               reproduce that pulse, blended by the refresh weight.",
        knob: Knob::Toggle(false),
    };
    const DRIVE_RMS: ValueDecl = ValueDecl {
        key: "drive_rms_full_level",
        label: "Drive rms at full level",
        unit: "V",
        help: "Rms of every drive together, referred to full level.",
    };
    const LINES_REFRESHED: ValueDecl = ValueDecl {
        key: "lines_refreshed",
        label: "Lines refreshed",
        unit: "",
        help: "Lines whose model the refresh moved on this pulse.",
    };

    /// `inverse`: the regularized inverse of the plant per line, corrected
    /// between pulses. The default for a waveform target.
    pub const INVERSE: LawDecl = LawDecl {
        id: "inverse",
        name: "inverse",
        version: "0.1.0",
        summary: "Drives every control's waveform through the regularized inverse of the \
                  identified plant on each spectral line, then corrects the drives after every \
                  pulse from the waveform error, with a gain per line that is cut on a line \
                  whose error grew. Options refresh the model after each pulse and remove each \
                  control's best-fit delay before the error is formed.",
        targets: &[TargetKind::Waveform, TargetKind::Srs],
        params: &[
            LAMBDA,
            ParamDecl {
                key: "gain",
                label: "Correction gain",
                help: "The fraction of each pulse's waveform error corrected on the next pulse.",
                knob: number(0.05, 1.0, 0.05, 1.0, ""),
            },
            ParamDecl {
                key: "watchdog",
                label: "Per-line watchdog",
                help: "Halve a line's gain when its smoothed error grew, and restore it slowly.",
                knob: Knob::Toggle(true),
            },
            REFRESH,
            ParamDecl {
                key: "refresh_mu",
                label: "Refresh weight",
                help: "How far the refresh moves the model toward the last pulse, 0 to 1.",
                knob: number(0.05, 1.0, 0.05, 0.5, ""),
            },
            ParamDecl {
                key: "align",
                label: "Remove each control's delay",
                help: "Remove each control's best-fit delay from its response before the error \
                       is formed.",
                knob: Knob::Toggle(false),
            },
            ParamDecl {
                key: "align_max_ms",
                label: "Largest delay removed",
                help: "The largest delay the alignment removes.",
                knob: number(0.0, 20.0, 0.1, 2.0, "ms"),
            },
        ],
        values: &[
            ValueDecl {
                key: "waveform_error",
                label: "Waveform error",
                unit: "",
                help: "Norm of the response error over the norm of the target, at full level.",
            },
            DRIVE_RMS,
            ValueDecl {
                key: "least_line_gain",
                label: "Least line gain",
                unit: "",
                help: "The smallest correction gain on any driven line.",
            },
            ValueDecl {
                key: "lines_cut",
                label: "Lines cut",
                unit: "",
                help: "Lines whose gain the watchdog cut on this pulse.",
            },
            LINES_REFRESHED,
        ],
    };

    /// `shape`: magnitude correction of each control's target from its SRS
    /// ratio, model phase untouched. The default for an SRS target.
    pub const SHAPE: LawDecl = LawDecl {
        id: "shape",
        name: "shape",
        version: "0.1.0",
        summary: "Keeps a smooth real gain per control and shapes each control's target by its \
                  SRS ratio after every pulse, with a weight scheduled by level; the drives are \
                  the regularized plant inverse applied to the shaped targets, so no waveform \
                  error is formed. Each control's delay is re-estimated from its response.",
        targets: &[TargetKind::Srs],
        params: &[
            LAMBDA,
            ParamDecl {
                key: "max_step_db",
                label: "Largest step",
                help: "The largest change of a control's gain in one pulse.",
                knob: number(0.5, 12.0, 0.5, 6.0, "dB"),
            },
            ParamDecl {
                key: "delay_relax",
                label: "Delay relaxation",
                help: "How far each control's delay moves toward its measured delay per pulse; \
                       zero holds the delays.",
                knob: number(0.0, 1.0, 0.05, 0.5, ""),
            },
            ParamDecl {
                key: "delay_bound_ms",
                label: "Delay bound",
                help: "Each control's delay stays this close to the first control's.",
                knob: number(0.0, 10.0, 0.1, 2.0, "ms"),
            },
            ParamDecl {
                key: "anchor",
                label: "Anchor",
                help: "Shape the target (default), or the measured response with its own \
                       phase (needs the refresh).",
                knob: Knob::Choice {
                    options: &["target", "measured"],
                    default: "target",
                },
            },
            REFRESH,
            ParamDecl {
                key: "refresh_mu",
                label: "Refresh weight",
                help: "How far the refresh moves the model toward the last pulse, 0 to 1.",
                knob: number(0.05, 1.0, 0.05, 1.0, ""),
            },
            ParamDecl {
                key: "seed_timing",
                label: "Seed timing",
                help: "Start every control at zero delay, or at the delays that need the least \
                       unreachable energy and drive.",
                knob: Knob::Choice {
                    options: &["zero", "least_effort"],
                    default: "zero",
                },
            },
        ],
        values: &[
            ValueDecl {
                key: "worst_srs_error_db",
                label: "Worst SRS error",
                unit: "dB",
                help: "The largest SRS deviation over every control and frequency.",
            },
            ValueDecl {
                key: "weight",
                label: "Weight",
                unit: "",
                help: "The weight on the SRS ratio at the level just played.",
            },
            DRIVE_RMS,
            LINES_REFRESHED,
            ValueDecl {
                key: "effort_cost_zero_delay",
                label: "Effort at zero delay",
                unit: "",
                help: "The seed timing's cost with every delay at zero.",
            },
            ValueDecl {
                key: "effort_cost_seed",
                label: "Effort at the seed delays",
                unit: "",
                help: "The seed timing's cost at the delays chosen.",
            },
        ],
    };

    /// `paired`: each control paired with one drive, that drive scaled by its
    /// control's magnitude ratio. The template a contributor copies.
    pub const PAIRED: LawDecl = LawDecl {
        id: "paired",
        name: "paired",
        version: "0.1.0",
        summary: "Pairs each control with one drive and, after every pulse, scales that drive's \
                  spectrum by the smoothed ratio of wanted to measured magnitude at its control. \
                  It uses no cross path of the plant, so on a coupled structure the pairs work \
                  against each other.",
        targets: &[TargetKind::Waveform, TargetKind::Srs],
        params: &[
            ParamDecl {
                key: "weight",
                label: "Weight",
                help: "The power the magnitude ratio is raised to, 0 to 1.",
                knob: number(0.0, 1.0, 0.05, 0.8, ""),
            },
            ParamDecl {
                key: "smooth_lines",
                label: "Smoothing",
                help: "Lines the magnitudes are smoothed over (odd).",
                knob: Knob::Integer {
                    unit: "lines",
                    min: 1,
                    max: 99,
                    default: 15,
                },
            },
            ParamDecl {
                key: "max_step_db",
                label: "Largest step",
                help: "The largest change of a line in one pulse.",
                knob: number(0.5, 12.0, 0.5, 6.0, "dB"),
            },
        ],
        values: &[DRIVE_RMS],
    };

    /// Every built-in law, the default for a waveform target first.
    pub const ALL: [LawDecl; 3] = [INVERSE, SHAPE, PAIRED];

    /// The `inverse` parameters from checked values.
    pub fn inverse_params(p: &Params) -> Result<math::inverse::InverseParams, String> {
        Ok(math::inverse::InverseParams {
            lambda_rel: p.number("lambda_rel")?,
            gain: p.number("gain")?,
            watchdog: p.toggle("watchdog")?,
            refresh: p.toggle("refresh")?,
            refresh_mu: p.number("refresh_mu")?,
            align: p.toggle("align")?,
            align_max_s: p.number("align_max_ms")? * 1e-3,
            ..Default::default()
        })
    }

    /// The `shape` parameters from checked values.
    pub fn shape_params(p: &Params) -> Result<math::shape::ShapeParams, String> {
        Ok(math::shape::ShapeParams {
            lambda_rel: p.number("lambda_rel")?,
            max_step_db: p.number("max_step_db")?,
            delay_relax: p.number("delay_relax")?,
            delay_bound_s: p.number("delay_bound_ms")? * 1e-3,
            anchor: match p.choice("anchor")? {
                "measured" => math::shape::Anchor::Measured,
                _ => math::shape::Anchor::Target,
            },
            refresh: p.toggle("refresh")?,
            refresh_mu: p.number("refresh_mu")?,
            seed_timing: match p.choice("seed_timing")? {
                "least_effort" => math::shape::SeedTiming::LeastEffort(Default::default()),
                _ => math::shape::SeedTiming::Zero,
            },
            ..Default::default()
        })
    }

    /// The default value of every parameter of `decl`, as a law receives it.
    pub fn defaults(decl: &LawDecl) -> Params {
        use crate::{Param, ParamValue};
        Params(
            decl.params
                .iter()
                .map(|p| Param {
                    key: p.key.into(),
                    value: match p.knob {
                        Knob::Number { default, .. } => ParamValue::Number(default),
                        Knob::Integer { default, .. } => ParamValue::Integer(default),
                        Knob::Toggle(d) => ParamValue::Toggle(d),
                        Knob::Choice { default, .. } => ParamValue::Choice(default.into()),
                    },
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wit_setup(kind: TargetKind) -> wit::MimoShockSetup {
        let (nc, nd, len) = (2u32, 3u32, 8u32);
        let lines = len / 2 + 1;
        let n = (nc * len) as usize;
        let ramp: Vec<f64> = (0..n).map(|i| i as f64).collect();
        wit::MimoShockSetup {
            fs_hz: 1000.0,
            record_len: len,
            drives: nd,
            controls: nc,
            band_lo_hz: 10.0,
            band_hi_hz: 400.0,
            target: match kind {
                TargetKind::Waveform => {
                    wit::Target::Waveform(wit::WaveformTarget { records: ramp })
                }
                TargetKind::Srs => wit::Target::Srs(wit::SrsTarget {
                    natural_hz: vec![100.0, 200.0, 300.0],
                    q: 10.0,
                    points: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
                    tol_lo_db: -1.5,
                    tol_hi_db: 3.0,
                    waveforms: ramp,
                    components: vec![
                        vec![wit::SrsComponent {
                            hz: 100.0,
                            amplitude: -1.0,
                            delay_s: 0.001,
                            shape: wit::ComponentShape::HalfCycles(5),
                        }],
                        vec![wit::SrsComponent {
                            hz: 200.0,
                            amplitude: 2.0,
                            delay_s: 0.0,
                            shape: wit::ComponentShape::Decay(0.03),
                        }],
                    ],
                }),
            },
            timing: wit::Timing::Hold(wit::HoldTiming {
                delays_s: vec![0.0, 0.001],
                window_s: 0.002,
            }),
            plant: wit::Plant {
                frf: wit::MatrixStack {
                    lines,
                    rows: nc,
                    cols: nd,
                    data: (0..lines * nc * nd)
                        .map(|i| wit::Complex {
                            re: i as f64,
                            im: -(i as f64),
                        })
                        .collect(),
                },
                coherence: (0..nc * lines).map(|i| i as f64 / 10.0).collect(),
                trusted: vec![true; lines as usize],
            },
            drive_ceiling_v: vec![1.0, 2.0, 3.0],
            ladder_db: vec![-6.0, 0.0],
        }
    }

    #[test]
    fn mimo_shock_sdk_unpacks_the_setup_channel_after_channel() {
        let s = setup_from_wit(wit_setup(TargetKind::Srs)).unwrap();
        assert_eq!((s.n_controls, s.n_drives, s.record_len), (2, 3, 8));
        assert_eq!(s.reference[1][0], 8.0);
        assert_eq!(s.reference[0][7], 7.0);
        // Element (r, c) of line l is data[(l * rows + r) * cols + c].
        assert_eq!(s.plant.frf.get(1, 1, 2), C64::new(11.0, -11.0));
        assert_eq!(s.plant.coherence[1][0], 0.5);
        match &s.target {
            Target::Srs(t) => {
                assert_eq!(t.points[1], vec![4.0, 5.0, 6.0]);
                assert_eq!(t.components[0][0].shape, ComponentShape::HalfCycles(5));
                assert_eq!(t.tol_lo_db, -1.5);
            }
            Target::Waveform => panic!("an SRS target was read as a waveform"),
        }
        assert_eq!(
            s.timing,
            Timing::Hold {
                delays_s: vec![0.0, 0.001],
                window_s: 0.002
            }
        );
        let m = s.to_math();
        assert_eq!(
            m.srs.as_ref().unwrap().points,
            vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]
        );
        assert_eq!(m.reference, s.reference);
        let w = setup_from_wit(wit_setup(TargetKind::Waveform)).unwrap();
        assert_eq!(w.target, Target::Waveform);
        assert!(w.to_math().srs.is_none());
    }

    #[test]
    fn mimo_shock_sdk_refuses_records_of_the_wrong_length() {
        let mut s = wit_setup(TargetKind::Waveform);
        if let wit::Target::Waveform(w) = &mut s.target {
            w.records.pop();
        }
        assert!(setup_from_wit(s).unwrap_err().contains("the target"));
        let i = wit::MimoShockInputs {
            pulse_index: 1,
            level_db: -6.0,
            next_level_db: 0.0,
            drives: vec![0.0; 3 * 8 - 1],
            responses: vec![0.0; 16],
            srs: None,
            verdicts: Vec::new(),
            limited: false,
        };
        assert!(inputs_from_wit(i, 3, 2, 8).unwrap_err().contains("drives"));
    }

    #[test]
    fn mimo_shock_sdk_inputs_and_outputs_round_trip() {
        let i = wit::MimoShockInputs {
            pulse_index: 3,
            level_db: -6.0,
            next_level_db: -3.0,
            drives: (0..24).map(f64::from).collect(),
            responses: (0..16).map(f64::from).collect(),
            srs: Some(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            verdicts: vec![
                wit::ControlVerdict {
                    passed: true,
                    error_db: 0.5,
                    delay_s: 0.0,
                    peak_ratio: 1.0,
                };
                2
            ],
            limited: true,
        };
        let p = inputs_from_wit(i, 3, 2, 8).unwrap();
        assert_eq!(p.drives[2][0], 16.0);
        assert_eq!(p.srs.as_ref().unwrap()[1], vec![4.0, 5.0, 6.0]);
        assert!(p.limited && p.verdicts[1].passed);
        let m = p.to_math();
        assert_eq!(
            m.pulse_index, 2,
            "the math counts the pulse just played from 0"
        );
        let o = Outputs {
            drives: p.drives.clone(),
            delays_s: Some(vec![0.0, 1e-3]),
            values: vec![("weight".into(), 0.5)],
        };
        let w = outputs_to_wit(o);
        assert_eq!(w.drives, (0..24).map(f64::from).collect::<Vec<_>>());
        assert_eq!(w.values[0].key, "weight");
    }

    #[test]
    fn mimo_shock_sdk_manifest_builder_declares_targets_params_and_values() {
        let m = ManifestBuilder::new("n", "1.0.0")
            .summary("s")
            .targets(&[TargetKind::Srs])
            .number("gain", "Gain", "", "h", 0.0, 1.0, 0.05, 0.8)
            .toggle("refresh", "Refresh", "h", false)
            .choice("anchor", "Anchor", "h", &["target", "measured"], "target")
            .per_control("drive", "Drive", "", "h", 0.0, 8.0, 1.0, 0.0)
            .integer("lines", "Lines", "", "h", 1, 99, 15)
            .value("weight", "Weight", "", "h")
            .build();
        assert_eq!(manifest_targets(&m), vec![TargetKind::Srs]);
        assert_eq!(m.law.params.len(), 5);
        assert_eq!(m.law.diagnostics[0].key, "weight");
    }

    #[test]
    fn mimo_shock_sdk_builtin_defaults_are_the_math_defaults() {
        use builtin::*;
        let i = inverse_params(&defaults(&INVERSE)).unwrap();
        assert_eq!(i, math::inverse::InverseParams::default());
        let s = shape_params(&defaults(&SHAPE)).unwrap();
        assert_eq!(s, math::shape::ShapeParams::default());
        let p = math::paired::PairedParams::diagonal(1, 1);
        let d = defaults(&PAIRED);
        assert_eq!(d.number("weight").unwrap(), p.weight);
        assert_eq!(d.integer("smooth_lines").unwrap() as usize, p.smooth_lines);
        assert_eq!(d.number("max_step_db").unwrap(), p.max_step_db);
        for decl in ALL {
            let m = decl.manifest();
            assert_eq!(m.law.name, decl.id);
            assert!(m.law.diagnostics.len() <= 8, "{}", decl.id);
            assert!(!m.targets.is_empty());
        }
    }
}
