//! The declarations of the built-in MIMO Sine laws (`matrix`, `paired`,
//! `steepest_descent`, `secant`, `ratio`, `followed_poles`): their names, parameters, values,
//! shapes, test types and whether they take an amplitude-only reference, as
//! plain data, and the one constructor ([`config`], [`Config::build`]) that
//! the law files and the controller's native laws both call, so a parameter
//! means the same mathematics on either side. A law of your own does not need
//! this module.

use super::{math, ManifestBuilder, Outputs, Setup, Shape, SineMimoManifest, TestType, ALL_SHAPES};
use crate::Params;
use math::followed::{FollowedLaw, FollowedParams, FOLLOWED_TEST_TYPES, FOLLOWED_VALUES};
use math::law::SineLaw;
use math::matrix::{FreePhase, MatrixLaw, MatrixParams, MATRIX_TEST_TYPES, MATRIX_VALUES};
use math::modal::ModelChoice;
use math::paired::{PairedLaw, PairedParams, PAIRED_TEST_TYPES, PAIRED_VALUES};
use math::ratio::{Pairing, RatioLaw, RatioParams, RATIO_TEST_TYPES, RATIO_VALUES};
use math::secant::{SecantCarry, SecantLaw, SecantParams, SECANT_TEST_TYPES, SECANT_VALUES};
use math::setup::DelayEstimate;
use math::speed::GainRule;
use math::steepest::{SteepestLaw, SteepestParams, STEEPEST_TEST_TYPES, STEEPEST_VALUES};

pub use crate::shock_mimo::builtin::{Knob, ParamDecl, ValueDecl};

/// Who the built-in laws are credited to in their manifests.
pub const AUTHOR: &str = "Hertzforge contributors";

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
    /// The shapes it accepts.
    pub shapes: &'static [Shape],
    /// The test types it accepts: the mathematics' own constant.
    pub test_types: math::TestTypes,
    /// Whether it accepts an amplitude-only reference.
    pub amplitude_only: bool,
    /// Its parameters.
    pub params: &'static [ParamDecl],
    /// The values it reports, in the order the mathematics fills them.
    pub values: &'static [ValueDecl],
}

impl LawDecl {
    /// The test types it accepts, in plain form.
    pub fn test_types(&self) -> Vec<TestType> {
        TestType::of(self.test_types)
    }

    /// The keys of the values it reports, in order.
    pub fn value_keys(&self) -> Vec<&'static str> {
        self.values.iter().map(|v| v.key).collect()
    }

    /// The law's manifest.
    pub fn manifest(&self) -> SineMimoManifest {
        let mut b = ManifestBuilder::new(self.name, self.version)
            .author(AUTHOR)
            .summary(self.summary)
            .shapes(self.shapes)
            .test_types(&self.test_types())
            .amplitude_only(self.amplitude_only);
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

const GAIN_RULE: ParamDecl = ParamDecl {
    key: "gain_rule",
    label: "Gain rule",
    help: "`loop_speed` sets the gain every update from the tracking filter's delay and the \
           identification's group delay near the tone; `fixed` uses the fixed gain every update.",
    knob: Knob::Choice {
        options: &["loop_speed", "fixed"],
        default: "loop_speed",
    },
};
const FIXED_GAIN: ParamDecl = ParamDecl {
    key: "fixed_gain",
    label: "Fixed gain",
    help: "The gain every update when the gain rule is `fixed`. 0.5 is the published value; on \
           a lightly damped structure it can lose the loop.",
    knob: number(0.01, 1.0, 0.01, 0.5, ""),
};
const DELAY_ESTIMATE: ParamDecl = ParamDecl {
    key: "delay_estimate",
    label: "Structure delay",
    help: "How the loop-speed rule reads the identification's group delay near the tone: \
           `largest` takes the largest phase slope over the response's elements (an element \
           with little response and some noise can set it, and the loop then runs slower than \
           it needs to between resonances); `principal` takes the phase slope along the \
           response's largest singular direction, which at a resonance is the mode's. \
           `principal` runs the loop faster between resonances; on a structure whose \
           resonances move with level that can lose a resonance `largest` holds.",
    knob: Knob::Choice {
        options: &DelayEstimate::NAMES,
        default: "largest",
    },
};
const REGULARIZATION: ParamDecl = ParamDecl {
    key: "regularization",
    label: "Regularization",
    help: "Regularization of the identification's inverse at each frequency, relative to its \
           largest singular value. Larger is safer and less exact.",
    knob: number(1.0e-6, 1.0, 1.0e-6, 1.0e-3, ""),
};
const GAIN: ValueDecl = ValueDecl {
    key: "gain",
    label: "Gain",
    unit: "",
    help: "The correction gain this update.",
};
const ERROR_DB: ValueDecl = ValueDecl {
    key: "error_db",
    label: "Worst error",
    unit: "dB",
    help: "The largest control error against the reference, either way.",
};

/// `matrix`: correction through the identification's regularized inverse at
/// the loop-speed rule's gain, carried along the model in a sweep. The
/// default.
pub const MATRIX: LawDecl = LawDecl {
    id: "matrix",
    name: "matrix",
    version: "0.1.0",
    summary: "Corrects the drive every update through the regularized inverse of the \
              identification (least squares with more controls than drives, least norm with \
              more drives), at a gain set from the loop's delays. In a sweep it also carries the \
              drive along a model of the structure, by the share the carry weight sets. It \
              rebases on the drive in force every update and removes the part of the drive the \
              controls cannot see. Its limit: a resonance that has moved more than a quarter of \
              its half-power bandwidth from where it was identified is not controlled as on a \
              linear structure.",
    shapes: &ALL_SHAPES,
    test_types: MATRIX_TEST_TYPES,
    amplitude_only: true,
    params: &[
        ParamDecl {
            key: "carry",
            label: "Carry weight",
            help: "How much of the drive change the model predicts for the next frequency is \
                   added in a sweep, 0 to 1. Zero is the correction alone.",
            knob: number(0.0, 1.0, 0.01, 0.67, ""),
        },
        GAIN_RULE,
        FIXED_GAIN,
        DELAY_ESTIMATE,
        REGULARIZATION,
        ParamDecl {
            key: "model",
            label: "Model",
            help: "The model the carry and the correction read: `modal` (the identification's \
                   modes exact, the rest smoothed), `smoothed` (the whole identification \
                   smoothed) or `identification` (as measured).",
            knob: Knob::Choice {
                options: &["modal", "smoothed", "identification"],
                default: "modal",
            },
        },
        ParamDecl {
            key: "smoothing_lines",
            label: "Smoothing",
            help: "Half-width of the model's smoothing, in identification lines.",
            knob: Knob::Integer {
                unit: "lines",
                min: 1,
                max: 64,
                default: 8,
            },
        },
        ParamDecl {
            key: "free_phase",
            label: "Free phases",
            help: "With an amplitude-only reference: `in_phase` controls every control at phase \
                   zero; `measured` takes each control's phase from its measurement.",
            knob: Knob::Choice {
                options: &["in_phase", "measured"],
                default: "in_phase",
            },
        },
        ParamDecl {
            key: "carry_outside_model",
            label: "Carry outside the model",
            help: "Carry also where the modal fit left a group of resonances out. Off by \
                   default.",
            knob: Knob::Toggle(false),
        },
    ],
    values: &[
        GAIN,
        ValueDecl {
            key: "group_delay_s",
            label: "Structure delay",
            unit: "s",
            help: "The identification's group delay near the tone that the gain was set from.",
        },
        ValueDecl {
            key: "condition",
            label: "Condition",
            unit: "",
            help: "The condition number of the identification at this frequency.",
        },
        ValueDecl {
            key: "carried_share",
            label: "Carried share",
            unit: "",
            help: "The carry's share of this update's drive change, 0 to 1.",
        },
        ValueDecl {
            key: "rotation_deg",
            label: "Rotation",
            unit: "deg",
            help: "The largest change of phase of any drive this update.",
        },
        ValueDecl {
            key: "unseen_share",
            label: "Unseen share",
            unit: "",
            help: "The share of the drive the controls cannot see that was removed.",
        },
        ValueDecl {
            key: "modes_kept",
            label: "Modes in the model",
            unit: "",
            help: "Resonances the modal fit kept.",
        },
        ValueDecl {
            key: "candidates_left_out",
            label: "Resonances left out",
            unit: "",
            help: "Candidate resonances the fit left in the smoothed part of the model.",
        },
    ],
};

/// `paired`: each control paired with one drive, that drive's amplitude
/// following its control's ratio of wanted to measured. The template a
/// contributor copies.
pub const PAIRED: LawDecl = LawDecl {
    id: "paired",
    name: "paired",
    version: "0.1.0",
    summary: "Pairs control i with drive i and, every update, moves that drive's amplitude by \
              the ratio of wanted to measured amplitude at its control, raised to the gain; \
              phase is left to the pair. It has no model of the coupling between pairs, so on a \
              coupled structure the pairs work against each other.",
    shapes: &ALL_SHAPES,
    test_types: PAIRED_TEST_TYPES,
    amplitude_only: true,
    params: &[GAIN_RULE, FIXED_GAIN, DELAY_ESTIMATE],
    values: &[GAIN, ERROR_DB],
};

/// `steepest_descent`: the drive moves down the gradient of the squared
/// control error through the identification.
pub const STEEPEST_DESCENT: LawDecl = LawDecl {
    id: "steepest_descent",
    name: "steepest_descent",
    version: "0.1.0",
    summary: "Starts from the identification's inverse drive and then moves the drive every \
              update down the gradient of the squared control error through the \
              identification, scaled by its largest singular value. No inverse is formed after \
              the first drive.",
    shapes: &ALL_SHAPES,
    test_types: STEEPEST_TEST_TYPES,
    amplitude_only: false,
    params: &[GAIN_RULE, FIXED_GAIN, DELAY_ESTIMATE, REGULARIZATION],
    values: &[GAIN, ERROR_DB],
};

/// `secant`: a complex gain found from two successive drives and their
/// responses.
pub const SECANT: LawDecl = LawDecl {
    id: "secant",
    name: "secant",
    version: "0.1.0",
    summary: "Works in two-step cycles: a probe along the inverse correction, then a step with \
              the complex gain found from the probe's measured response, and a rank-one update \
              of its inverse model. Each step waits for the response to settle, so its pace is \
              the settle wait.",
    shapes: &ALL_SHAPES,
    test_types: SECANT_TEST_TYPES,
    amplitude_only: false,
    params: &[
        ParamDecl {
            key: "carry",
            label: "Carry along frequency",
            help: "How the inverse model moves to a new frequency: `factor` keeps the learned \
                   correction as a factor on the identification's inverse there; `plain` keeps \
                   the inverse learned at the start.",
            knob: Knob::Choice {
                options: &["factor", "plain"],
                default: "factor",
            },
        },
        ParamDecl {
            key: "first_probe",
            label: "First probe weight",
            help: "The probe's weight at the start of the run.",
            knob: number(0.01, 1.0, 0.01, 0.1, ""),
        },
        ParamDecl {
            key: "largest_probe",
            label: "Largest probe weight",
            help: "The largest the probe's weight may grow.",
            knob: number(0.01, 1.0, 0.01, 1.0, ""),
        },
        ParamDecl {
            key: "settle_factor",
            label: "Settle wait",
            help: "How long each step waits for the response, in multiples of the tracking \
                   filter's and the structure's delays.",
            knob: number(1.0, 20.0, 0.5, 5.0, ""),
        },
        REGULARIZATION,
    ],
    values: &[
        ValueDecl {
            key: "a_re",
            label: "Gain error, real",
            unit: "",
            help: "The real part of the last measured gain against the probe.",
        },
        ValueDecl {
            key: "a_im",
            label: "Gain error, imaginary",
            unit: "",
            help: "The imaginary part of the last measured gain against the probe.",
        },
        ValueDecl {
            key: "c",
            label: "Probe weight",
            unit: "",
            help: "The probe's weight now.",
        },
        ValueDecl {
            key: "phase",
            label: "Step",
            unit: "",
            help: "0 waiting for the base reading, 1 waiting for the probe's.",
        },
        ERROR_DB,
        ValueDecl {
            key: "secant_skipped",
            label: "Updates skipped",
            unit: "",
            help: "Model updates skipped because the response did not change enough.",
        },
    ],
};

/// `ratio`: each control's amplitude ratio applied to its reference through
/// the identification's inverse.
pub const RATIO: LawDecl = LawDecl {
    id: "ratio",
    name: "ratio",
    version: "0.1.0",
    summary: "Plays the identification's inverse of a reference scaled by one factor per \
              control, and updates each factor from a compressed ratio of wanted to measured \
              amplitude once the reading has settled. With `drive` pairing the factors scale \
              the drives instead (square setups only).",
    shapes: &ALL_SHAPES,
    test_types: RATIO_TEST_TYPES,
    amplitude_only: true,
    params: &[
        ParamDecl {
            key: "pairing",
            label: "Pairing",
            help: "`reference` applies control i's ratio to its own reference (any shape); \
                   `drive` applies it to drive i (square setups only).",
            knob: Knob::Choice {
                options: &["reference", "drive"],
                default: "reference",
            },
        },
        ParamDecl {
            key: "compression",
            label: "Compression",
            help: "The constant K of the ratio update: larger is gentler.",
            knob: number(0.1, 10.0, 0.1, 1.0, ""),
        },
        REGULARIZATION,
    ],
    values: &[
        ValueDecl {
            key: "sigma_min",
            label: "Least factor",
            unit: "",
            help: "The smallest correction factor.",
        },
        ValueDecl {
            key: "sigma_max",
            label: "Largest factor",
            unit: "",
            help: "The largest correction factor.",
        },
        ERROR_DB,
    ],
};

/// The key of `followed_poles`' station delay.
pub const STATION_DELAY_KEY: &str = "station_delay_s";

/// `followed_poles`: the carried inverse on a modal model whose poles follow
/// the structure through a sweep, refitted to the readings.
pub const FOLLOWED_POLES: LawDecl = LawDecl {
    id: "followed_poles",
    name: "followed_poles",
    version: "0.1.0",
    summary: "Carries the drive along a modal model of the structure through a sweep and \
              corrects it through the model's regularized inverse, as `matrix` does, but \
              follows the model's resonances: every second update the frequency and damping of \
              the modes near the tone are refitted to the last 40 readings, against the model \
              run in time with the tracking filter's lag, held to the identification's by a \
              prior and moved only where the readings show it. When the followed model predicts \
              the readings worse than the identification's own, the identification's inverse is \
              used until it predicts better. The model is fitted with the station's delay taken \
              out, so the law needs it set. Sweeps only.",
    shapes: &ALL_SHAPES,
    test_types: FOLLOWED_TEST_TYPES,
    amplitude_only: false,
    params: &[
        ParamDecl {
            key: STATION_DELAY_KEY,
            label: "Station delay",
            help: "The delay from the controller's writing a drive sample to its reading the \
                   response to it, the structure's own response left out: the controller's \
                   output queue (the identification's frames ahead plus one block), the output \
                   and acquisition chain and any transport delay. The modal fit takes it out of \
                   the identification, less the lag the controller already took out, and puts \
                   it back on the modes. Zero is not set: the law is refused.",
            knob: number(0.0, 1.0, 1.0e-6, 0.0, "s"),
        },
        GAIN_RULE,
        FIXED_GAIN,
        DELAY_ESTIMATE,
        REGULARIZATION,
        ParamDecl {
            key: "smoothing_lines",
            label: "Smoothing",
            help: "Half-width of the smoothing of the model's remainder, in identification \
                   lines.",
            knob: Knob::Integer {
                unit: "lines",
                min: 1,
                max: 64,
                default: 8,
            },
        },
        ParamDecl {
            key: "window",
            label: "Fit window",
            help: "The updates the poles are refitted over.",
            knob: Knob::Integer {
                unit: "updates",
                min: 4,
                max: 400,
                default: 40,
            },
        },
        ParamDecl {
            key: "gate",
            label: "Prediction gate",
            help: "Use the identification's inverse while the followed model predicts the \
                   readings worse than the identification's own model.",
            knob: Knob::Toggle(true),
        },
        ParamDecl {
            key: "gate_ratio",
            label: "Gate ratio",
            help: "How much worse the followed model's prediction must be for the gate to \
                   switch to the identification's inverse; it switches back when the followed \
                   model is better by as much.",
            knob: number(1.05, 100.0, 0.05, 2.0, ""),
        },
    ],
    values: &[
        GAIN,
        ERROR_DB,
        ValueDecl {
            key: "gated",
            label: "Gated",
            unit: "",
            help: "1 while the identification's inverse is used, 0 while the followed model's.",
        },
        ValueDecl {
            key: "near_modes",
            label: "Modes near",
            unit: "",
            help: "Modes within four bandwidths of the tone.",
        },
        ValueDecl {
            key: "shift_bw",
            label: "Shift",
            unit: "bw",
            help: "The largest move of a mode near the tone from where it was identified, in its \
                   half-power bandwidths.",
        },
        ValueDecl {
            key: "damping_change",
            label: "Damping change",
            unit: "",
            help: "The largest change of a near mode's damping, as the size of the logarithm of \
                   its ratio to the identification's.",
        },
        ValueDecl {
            key: "prediction_ratio",
            label: "Prediction ratio",
            unit: "",
            help: "The followed model's prediction error over the window against the \
                   identification's model's; -1 until the window is full.",
        },
        ValueDecl {
            key: "modes_kept",
            label: "Modes in the model",
            unit: "",
            help: "Resonances the modal fit kept.",
        },
    ],
};

/// Every built-in law, the default (`matrix`) first.
pub const ALL: [LawDecl; 6] = [
    MATRIX,
    PAIRED,
    STEEPEST_DESCENT,
    SECANT,
    RATIO,
    FOLLOWED_POLES,
];

/// The declaration with catalog id `id`.
pub fn decl(id: &str) -> Option<&'static LawDecl> {
    ALL.iter().find(|d| d.id == id)
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

/// A built-in law's parameters, read from checked values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Config {
    /// `matrix`.
    Matrix(MatrixParams),
    /// `paired`: control i with drive i, at this gain rule.
    Paired(GainRule),
    /// `steepest_descent`.
    Steepest(SteepestParams),
    /// `secant`.
    Secant(SecantParams),
    /// `ratio`.
    Ratio(RatioParams),
    /// `followed_poles`.
    Followed(FollowedParams),
}

fn gain_rule(p: &Params) -> Result<GainRule, String> {
    Ok(match p.choice("gain_rule")? {
        "fixed" => GainRule::Fixed {
            g: p.number("fixed_gain")?,
        },
        _ => {
            let name = p.choice("delay_estimate")?;
            let delay = DelayEstimate::from_name(name)
                .ok_or_else(|| format!("no structure delay estimate is named `{name}`"))?;
            match GainRule::default() {
                GainRule::LoopSpeed { g_max, beta, .. } => {
                    GainRule::LoopSpeed { g_max, beta, delay }
                }
                fixed => fixed,
            }
        }
    })
}

/// The parameters of the built-in law `id` from `p`, every declared key
/// present (checked by the host, or [`defaults`] with changes).
pub fn config(id: &str, p: &Params) -> Result<Config, String> {
    Ok(match id {
        "matrix" => {
            let lines = usize::try_from(p.integer("smoothing_lines")?)
                .map_err(|_| "the smoothing must be at least one line".to_string())?;
            Config::Matrix(MatrixParams {
                carry: p.number("carry")?,
                gain_rule: gain_rule(p)?,
                eps: p.number("regularization")?,
                model: match p.choice("model")? {
                    "smoothed" => ModelChoice::Smoothed { lines },
                    "identification" => ModelChoice::Identification,
                    _ => ModelChoice::Modal { lines },
                },
                free_phase: match p.choice("free_phase")? {
                    "measured" => FreePhase::Measured,
                    _ => FreePhase::InPhase,
                },
                carry_outside_model: p.toggle("carry_outside_model")?,
                ..MatrixParams::default()
            })
        }
        "paired" => Config::Paired(gain_rule(p)?),
        "steepest_descent" => Config::Steepest(SteepestParams {
            gain_rule: gain_rule(p)?,
            eps: p.number("regularization")?,
        }),
        "secant" => Config::Secant(SecantParams {
            carry: match p.choice("carry")? {
                "plain" => SecantCarry::Plain,
                _ => SecantCarry::Factor,
            },
            c0: p.number("first_probe")?,
            c_max: p.number("largest_probe")?,
            settle_factor: p.number("settle_factor")?,
            eps: p.number("regularization")?,
            ..SecantParams::default()
        }),
        "ratio" => Config::Ratio(RatioParams {
            pairing: match p.choice("pairing")? {
                "drive" => Pairing::Drive,
                _ => Pairing::Reference,
            },
            k: p.number("compression")?,
            eps: p.number("regularization")?,
            ..RatioParams::default()
        }),
        "followed_poles" => {
            let lines = usize::try_from(p.integer("smoothing_lines")?)
                .map_err(|_| "the smoothing must be at least one line".to_string())?;
            let window = usize::try_from(p.integer("window")?)
                .map_err(|_| "the fit window must be a count of updates".to_string())?;
            Config::Followed(FollowedParams {
                station_delay_s: p.number(STATION_DELAY_KEY)?,
                gain_rule: gain_rule(p)?,
                eps: p.number("regularization")?,
                smoothing_lines: lines,
                window,
                gate: p.toggle("gate")?,
                gate_ratio: p.number("gate_ratio")?,
                ..FollowedParams::default()
            })
        }
        other => return Err(format!("no built-in MIMO Sine law has the id `{other}`")),
    })
}

impl Config {
    /// The declaration of the law these parameters are for.
    pub fn decl(&self) -> &'static LawDecl {
        match self {
            Config::Matrix(_) => &MATRIX,
            Config::Paired(_) => &PAIRED,
            Config::Steepest(_) => &STEEPEST_DESCENT,
            Config::Secant(_) => &SECANT,
            Config::Ratio(_) => &RATIO,
            Config::Followed(_) => &FOLLOWED_POLES,
        }
    }

    /// Build the law for a run. `matrix` and `followed_poles` fit their
    /// models here.
    pub fn build(&self, setup: &math::Setup) -> Result<BuiltLaw, String> {
        Ok(match *self {
            Config::Matrix(p) => BuiltLaw::Matrix(Box::new(MatrixLaw::new(setup, p)?)),
            Config::Paired(rule) => {
                let mut p = PairedParams::diagonal(setup.controls, setup.drives);
                p.gain_rule = rule;
                BuiltLaw::Paired(Box::new(PairedLaw::new(setup, p)?))
            }
            Config::Steepest(p) => BuiltLaw::Steepest(Box::new(SteepestLaw::new(setup, p)?)),
            Config::Secant(p) => BuiltLaw::Secant(Box::new(SecantLaw::new(setup, p)?)),
            Config::Ratio(p) => BuiltLaw::Ratio(Box::new(RatioLaw::new(setup, p)?)),
            Config::Followed(p) => BuiltLaw::Followed(Box::new(FollowedLaw::new(setup, p)?)),
        })
    }
}

/// A built-in law built for a run.
#[derive(Debug, Clone)]
pub enum BuiltLaw {
    /// `matrix`.
    Matrix(Box<MatrixLaw>),
    /// `paired`.
    Paired(Box<PairedLaw>),
    /// `steepest_descent`.
    Steepest(Box<SteepestLaw>),
    /// `secant`.
    Secant(Box<SecantLaw>),
    /// `ratio`.
    Ratio(Box<RatioLaw>),
    /// `followed_poles`.
    Followed(Box<FollowedLaw>),
}

impl BuiltLaw {
    fn law(&mut self) -> &mut dyn SineLaw {
        match self {
            BuiltLaw::Matrix(l) => l.as_mut(),
            BuiltLaw::Paired(l) => l.as_mut(),
            BuiltLaw::Steepest(l) => l.as_mut(),
            BuiltLaw::Secant(l) => l.as_mut(),
            BuiltLaw::Ratio(l) => l.as_mut(),
            BuiltLaw::Followed(l) => l.as_mut(),
        }
    }
}

impl SineLaw for BuiltLaw {
    fn engage(&mut self, x: &math::Inputs, out: &mut math::Outputs) -> Result<(), String> {
        self.law().engage(x, out)
    }

    fn tick(&mut self, x: &math::Inputs, out: &mut math::Outputs) -> Result<(), String> {
        self.law().tick(x, out)
    }

    fn value_names(&self) -> &'static [&'static str] {
        match self {
            BuiltLaw::Matrix(_) => &MATRIX_VALUES,
            BuiltLaw::Paired(_) => &PAIRED_VALUES,
            BuiltLaw::Steepest(_) => &STEEPEST_VALUES,
            BuiltLaw::Secant(_) => &SECANT_VALUES,
            BuiltLaw::Ratio(_) => &RATIO_VALUES,
            BuiltLaw::Followed(_) => &FOLLOWED_VALUES,
        }
    }
}

/// A built-in law behind the SDK's plain types: what each built-in law file
/// is. Its answer buffer is kept for the run.
#[derive(Debug, Clone)]
pub struct BuiltinFile {
    law: BuiltLaw,
    out: math::Outputs,
}

impl BuiltinFile {
    /// Build the built-in law `decl` for `setup` with checked `params`.
    pub fn new(decl: &LawDecl, setup: &Setup, params: &Params) -> Result<Self, String> {
        let law = config(decl.id, params)?.build(&setup.to_math())?;
        let names = law.value_names().len();
        Ok(BuiltinFile {
            law,
            out: math::Outputs::new(setup.drives, names),
        })
    }

    fn answer(&self) -> Outputs {
        Outputs::from_math(&self.out, self.law.value_names())
    }

    /// The first drive.
    pub fn engage(&mut self, inputs: &super::Inputs) -> Result<Outputs, String> {
        self.out.request = math::Request::None;
        self.law.engage(&inputs.to_math(), &mut self.out)?;
        Ok(self.answer())
    }

    /// Every drive after it.
    pub fn tick(&mut self, inputs: &super::Inputs) -> Result<Outputs, String> {
        self.out.request = math::Request::None;
        self.law.tick(&inputs.to_math(), &mut self.out)?;
        Ok(self.answer())
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest_shapes;
    use super::*;

    #[test]
    fn mimo_sine_each_declaration_names_the_maths_values_and_test_types() {
        let maths: [(&str, &[&str], math::TestTypes); 6] = [
            ("matrix", &MATRIX_VALUES, MATRIX_TEST_TYPES),
            ("paired", &PAIRED_VALUES, PAIRED_TEST_TYPES),
            ("steepest_descent", &STEEPEST_VALUES, STEEPEST_TEST_TYPES),
            ("secant", &SECANT_VALUES, SECANT_TEST_TYPES),
            ("ratio", &RATIO_VALUES, RATIO_TEST_TYPES),
            ("followed_poles", &FOLLOWED_VALUES, FOLLOWED_TEST_TYPES),
        ];
        for (id, values, types) in maths {
            let d = decl(id).unwrap();
            assert_eq!(d.value_keys(), values, "{id}");
            assert_eq!(d.test_types, types, "{id}");
            let m = d.manifest();
            assert_eq!(m.law.name, id);
            assert_eq!(manifest_shapes(&m), ALL_SHAPES, "{id}");
            assert!(m.law.diagnostics.len() <= super::super::MAX_VALUES);
            // Every declared parameter is read by the constructor at its default.
            let c = config(id, &defaults(d)).unwrap();
            assert_eq!(c.decl().id, id);
        }
        assert_eq!(ALL[0].id, "matrix");
        assert!(config("inverse", &Params(Vec::new())).is_err());
    }

    #[test]
    fn mimo_sine_the_declared_defaults_are_the_maths_defaults() {
        match config("matrix", &defaults(&MATRIX)).unwrap() {
            Config::Matrix(p) => assert_eq!(p, MatrixParams::default()),
            c => panic!("{c:?}"),
        }
        match config("steepest_descent", &defaults(&STEEPEST_DESCENT)).unwrap() {
            Config::Steepest(p) => assert_eq!(p, SteepestParams::default()),
            c => panic!("{c:?}"),
        }
        match config("secant", &defaults(&SECANT)).unwrap() {
            Config::Secant(p) => assert_eq!(p, SecantParams::default()),
            c => panic!("{c:?}"),
        }
        match config("ratio", &defaults(&RATIO)).unwrap() {
            Config::Ratio(p) => assert_eq!(p, RatioParams::default()),
            c => panic!("{c:?}"),
        }
        match config("paired", &defaults(&PAIRED)).unwrap() {
            Config::Paired(r) => assert_eq!(r, GainRule::default()),
            c => panic!("{c:?}"),
        }
        match config("followed_poles", &defaults(&FOLLOWED_POLES)).unwrap() {
            Config::Followed(p) => assert_eq!(p, FollowedParams::default()),
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn mimo_sine_the_structure_delay_choice_reaches_the_gain_rule() {
        use crate::ParamValue;
        for d in [&MATRIX, &PAIRED, &STEEPEST_DESCENT, &FOLLOWED_POLES] {
            let mut p = defaults(d);
            for x in p.0.iter_mut() {
                if x.key == "delay_estimate" {
                    x.value = ParamValue::Choice("principal".into());
                }
            }
            let rule = match config(d.id, &p).unwrap() {
                Config::Matrix(m) => m.gain_rule,
                Config::Paired(r) => r,
                Config::Steepest(s) => s.gain_rule,
                Config::Followed(f) => f.gain_rule,
                c => panic!("{c:?}"),
            };
            assert_eq!(rule.delay_estimate(), DelayEstimate::Principal, "{}", d.id);
        }
    }
}
