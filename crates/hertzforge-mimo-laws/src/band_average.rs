//! Brandon Zwink's band-average law on the MIMO law boundary.
//!
//! The math is `hertzforge-law-band-average-math`, a port of his Python law, on
//! this crate's stacks and factorizations. This file carries the law's inputs
//! across the boundary, declares its parameters and reports its diagnostics. It is
//! one of the [`crate::BuiltinMimo`] laws, and `laws/mimo-band-average` builds the
//! same code as a law file.
//!
//! It controls each control's fractional-octave band averages, not its lines: on
//! a plant with resonances inside a band, a control's line-by-line error stays
//! above what a per-line inverse reaches (1.5 dB rms on the law check's 4 x 4
//! modal plant, where the per-line laws come within 1 dB), and the drive does
//! not chase identification noise line by line.
//!
//! What the core does is left to the core:
//!
//! - **Limit channels.** The Python law's response limits are not offered.
//! - **Drive ceilings.** The law's own RMS limit is off unless it is set. Set, it
//!   moves the solve along its error-energy front to stay under the limit, which
//!   costs less fidelity than the core's scaling at the ceiling.
//! - **The plant estimate.** With `use_tf_update` the law solves again from the FRF
//!   the core hands it, so the test's FRF mode must be update. The law averages
//!   accepted updates into its band sensitivity itself (that average is on the
//!   band cross-power, which is the method's, not on the FRF), and it reads the
//!   multiple coherence the core gives with the updated FRF.
//! - **Buzz cross terms** are the buzz-test law's ([`crate::buzz_reference`]): the
//!   reference's autospectra with the identification's coherence and phase, pairs
//!   under a coherence floor left independent. The law then fits that reference's
//!   cross terms band by band.
//!
//! What differs from the law's inputs under Rattlesnake:
//!
//! - `last_output_cpsd` is the drive in force as the core played it, where
//!   Rattlesnake measures it over the same frames as the response.
//! - An FRF update is accepted once the estimate holds `estimate_frames` frames,
//!   read from the core's degrees of freedom. The core's estimate is an exponential
//!   average, so there is no rolling buffer to wait on.

use crate::buzz::buzz_reference;
use crate::pinv::ALL_SHAPES;
use crate::spec::{DiagDecl, DiagKind, LawSpec, ParamDecl, ParamKindDecl, ParamVal};
use crate::support::{diag_correction, largest_change_db, workers};
use crate::BuiltinMimoLaw;
use hertzforge_law_band_average_math::{
    BandAverage, Frame, KappaRule, Params, Regularization, TfUpdateBlend,
};
use hertzforge_sdm::law::{
    DiagnosticData, DiagnosticValue, LawError, MimoControlLaw, MimoInput, MimoLoopInputs,
    MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::{HermitianStack, C64};

/// The drive in force, for the gain correction's prediction of the response.
const NEEDS: [MimoInput; 1] = [MimoInput::LastDrive];

/// Where the cross terms the law fits come from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CrossTerms {
    /// None: the autospectra alone.
    Off,
    /// The reference's own.
    Reference,
    /// The identification's coherence and phase, pairs under the floor left out.
    Buzz { floor: f64 },
}

/// The band-average law for one run.
pub struct BandAverageLaw {
    law: BandAverage,
    frequencies: Vec<f64>,
    lines: usize,
    controls: usize,
    drives: usize,
    band: (usize, usize),
    estimate_frames: usize,
    use_tf_update: bool,
    uses_coherence: bool,
    cross: CrossTerms,
    /// The drive last returned, for the largest change.
    last: Option<HermitianStack>,
}

/// Every line's full matrix, row-major, as the law's arithmetic reads a drive.
fn full(g: &HermitianStack) -> Vec<C64> {
    let n = g.n();
    let mut out = Vec::with_capacity(g.lines() * n * n);
    for line in 0..g.lines() {
        for r in 0..n {
            for c in 0..n {
                out.push(g.get(line, r, c));
            }
        }
    }
    out
}

fn autospectra(g: &HermitianStack) -> Vec<f64> {
    let mut out = Vec::with_capacity(g.lines() * g.n());
    for line in 0..g.lines() {
        out.extend(g.diagonal(line));
    }
    out
}

impl BandAverageLaw {
    /// The drive the law returned, on the band, as the core takes it, with the
    /// loop's diagnostics.
    fn outputs(&mut self, drive: &[C64]) -> MimoLoopOutputs {
        let d = self.drives;
        let mut out = HermitianStack::zeros(self.lines, d);
        for line in self.band.0..self.band.1 {
            let m = &drive[line * d * d..(line + 1) * d * d];
            for r in 0..d {
                for c in r..d {
                    let (a, b) = (m[r * d + c], m[c * d + r]);
                    out.set(
                        line,
                        r,
                        c,
                        C64::new(0.5 * (a.re + b.re), 0.5 * (a.im - b.im)),
                    );
                }
            }
        }
        let correction_db = self
            .last
            .as_ref()
            .map_or(0.0, |last| largest_change_db(last, &out, self.band));
        let diagnostics = self.diagnostics(correction_db);
        self.last = Some(out.clone());
        MimoLoopOutputs {
            drive: out,
            diagnostics,
        }
    }

    fn diagnostics(&self, correction_db: f64) -> Vec<DiagnosticValue> {
        let (lines, c) = (self.lines, self.controls);
        let mut threshold = vec![0.0; lines];
        let mut gain_db = vec![0.0; lines * c];
        let gain = self.law.band_log_gain();
        let channels = self.law.control_channels();
        for (b, sel) in self.law.band_lines().iter().enumerate() {
            let kappa = self.law.band_threshold(b).unwrap_or(0.0);
            for &k in sel
                .iter()
                .filter(|k| (self.band.0..self.band.1).contains(*k))
            {
                threshold[k] = kappa;
                if let Some(g) = gain {
                    for (p, &channel) in channels.iter().enumerate() {
                        gain_db[k * c + channel] =
                            g[b * channels.len() + p] * (10.0 / std::f64::consts::LN_10);
                    }
                }
            }
        }
        let value = |key: &str, data| DiagnosticValue {
            key: key.into(),
            data,
        };
        let count = |n: usize| DiagnosticData::Scalar(n as f64);
        vec![
            value("threshold", DiagnosticData::PerLine(threshold)),
            value("gain_db", DiagnosticData::PerControlLine(gain_db)),
            value("correction_db", DiagnosticData::Scalar(correction_db)),
            value("excluded", count(self.law.excluded_channels())),
            value("frf_updates", count(self.law.frf_updates())),
            value("held_bands", count(self.law.held_bands())),
            value("capped_bands", count(self.law.capped_bands())),
        ]
    }
}

impl MimoControlLaw for BandAverageLaw {
    fn needs(&self) -> &[MimoInput] {
        &NEEDS
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let buzzed;
        let specification = match self.cross {
            // At the seed the measured response is the identification's, at
            // whatever level: the buzz reference takes only its coherence and phase.
            CrossTerms::Buzz { floor } => {
                buzzed = buzz_reference(inputs.reference, inputs.response, self.band, floor)?;
                &buzzed
            }
            CrossTerms::Off | CrossTerms::Reference => inputs.reference,
        };
        let drive = self
            .law
            .control(&Frame {
                frequencies: &self.frequencies,
                specification: Some(specification),
                transfer_function: inputs.frf.data(),
                sysid_response_cpsd: None,
                multiple_coherence: None,
                frames: (inputs.response_dof / 2.0) as usize,
                total_frames: self.estimate_frames,
                last_response_asd: None,
                last_output_cpsd: None,
            })
            .map_err(LawError)?;
        Ok(self.outputs(&drive))
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let last = inputs
            .last_drive
            .ok_or_else(|| LawError("the core did not pass the last drive".into()))?;
        let frames = (inputs.response_dof / 2.0) as usize;
        let update = self.use_tf_update && frames >= self.estimate_frames;
        let asd = autospectra(inputs.response);
        let output = full(last);
        let drive = self
            .law
            .control(&Frame {
                frequencies: &self.frequencies,
                specification: None,
                transfer_function: if update { inputs.frf.data() } else { &[] },
                sysid_response_cpsd: None,
                multiple_coherence: (update && self.uses_coherence)
                    .then_some(inputs.multiple_coherence),
                frames,
                total_frames: self.estimate_frames,
                last_response_asd: Some(&asd),
                last_output_cpsd: Some(&output),
            })
            .map_err(LawError)?;
        Ok(self.outputs(&drive))
    }
}

fn number(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
    default: f64,
) -> ParamDecl {
    ParamDecl {
        key,
        label,
        help,
        kind: ParamKindDecl::Number {
            unit,
            min,
            max,
            step: 0.01,
            default,
        },
    }
}

fn toggle(key: &'static str, label: &'static str, help: &'static str, default: bool) -> ParamDecl {
    ParamDecl {
        key,
        label,
        help,
        kind: ParamKindDecl::Toggle(default),
    }
}

fn parameters() -> Vec<ParamDecl> {
    vec![
        ParamDecl {
            key: "oct_order",
            label: "Bands per octave",
            help: "The law solves one flat drive per band of this fractional octave.",
            kind: ParamKindDecl::Integer {
                unit: "",
                min: 1,
                max: 48,
                step: 1,
                default: 6,
            },
        },
        ParamDecl {
            key: "kappa_rule",
            label: "Threshold rule",
            help: "How the first solve's sweep picks each band's threshold: along the common \
                   error-energy front, each band's own knee, or each band's L-curve corner.",
            kind: ParamKindDecl::Choice {
                options: &["front", "knee", "lcurve"],
                default: "front",
            },
        },
        number(
            "drive_reduction",
            "Drive reduction",
            "The fraction of the selected solution's predicted drive energy to give up, moving \
             along the error-energy front. Check the error before using a large value.",
            "",
            0.0,
            0.99,
            0.0,
        ),
        ParamDecl {
            key: "regularization",
            label: "Regularization",
            help: "Automatic thresholds from the sweep, or the plain pseudoinverse in each band.",
            kind: ParamKindDecl::Choice {
                options: &["auto", "none"],
                default: "auto",
            },
        },
        number(
            "condition_number",
            "Fixed threshold",
            "A threshold to use in every band in place of the sweep. 0 runs the sweep.",
            "",
            0.0,
            1e12,
            0.0,
        ),
        toggle(
            "constrained_solve",
            "Constrained solve",
            "Fit again over positive semi-definite drives where dropping negative eigenvalues \
             moves a response by more than 0.1 dB.",
            true,
        ),
        toggle(
            "gain_correction",
            "Gain correction",
            "Correct each band and channel from the measured response.",
            true,
        ),
        number(
            "gain_correction_rate",
            "Gain correction rate",
            "The smoothing rate of the gain correction, per call.",
            "",
            0.0,
            1.0,
            0.25,
        ),
        number(
            "channel_check_db",
            "Channel check",
            "A control channel this far from the model's prediction over the band is left out, \
             as a failed gauge. 0 turns the check off.",
            "dB",
            0.0,
            200.0,
            20.0,
        ),
        number(
            "drive_rms_limit",
            "Drive RMS limit",
            "The law moves along the error-energy front to keep each drive at or under this RMS, \
             in place of the core's scaling at its ceiling, which still applies. Set it at or \
             under the drive ceilings; 0 leaves the ceilings to the core.",
            "V",
            0.0,
            1e9,
            0.0,
        ),
        toggle(
            "use_tf_update",
            "FRF update",
            "Solve again from the core's running FRF, with a drive conditioned so the estimate \
             sees every drive direction. Set the test's FRF mode to update; a blend of 1 hands \
             the law each loop's own estimate, which it averages itself.",
            false,
        ),
        ParamDecl {
            key: "estimate_frames",
            label: "Frames before an FRF update",
            help: "An FRF update is accepted once the estimate holds this many frames.",
            kind: ParamKindDecl::Integer {
                unit: "",
                min: 1,
                max: 100_000,
                step: 1,
                default: 20,
            },
        },
        number(
            "drive_condition_number",
            "Drive condition limit",
            "With the FRF update, the largest condition number of a band's drive matrix, so the \
             update can see every drive direction.",
            "",
            1.0,
            1e12,
            100.0,
        ),
        number(
            "min_drive_fraction",
            "Minimum drive fraction",
            "With the FRF update, each drive's smallest autopower as a fraction of the band's \
             mean.",
            "",
            0.0,
            1.0,
            0.02,
        ),
        toggle(
            "hold_capped_bands",
            "Hold capped bands",
            "Leave a band out of the FRF update when its conditioning load was capped.",
            false,
        ),
        number(
            "min_coherence",
            "Minimum coherence",
            "Leave a channel's line out of the FRF update below this multiple coherence. 0 turns \
             it off.",
            "",
            0.0,
            1.0,
            0.0,
        ),
        number(
            "tf_update_blend",
            "FRF update weight",
            "The weight of each accepted update under exponential forgetting. 0 weights each \
             update as one new frame.",
            "",
            0.0,
            1.0,
            0.0,
        ),
        number(
            "n_e_seed",
            "FRF update seed count",
            "The frames the identification counts for in the update's average. 0 uses the \
             identification's own count.",
            "",
            0.0,
            1e9,
            0.0,
        ),
        ParamDecl {
            key: "cross_terms",
            label: "Cross terms",
            help: "Fit the autospectra alone, the reference's cross terms as well, or the \
                   cross terms of the buzz test: the identification's coherence and phase on \
                   the reference's autospectra.",
            kind: ParamKindDecl::Choice {
                options: &["off", "reference", "buzz"],
                default: "off",
            },
        },
        number(
            "coherence_floor",
            "Coherence floor",
            "With buzz cross terms, pairs whose identification coherence is below this are \
             left independent.",
            "",
            0.0,
            1.0,
            0.1,
        ),
    ]
}

fn diagnostics() -> Vec<DiagDecl> {
    let scalar = |key, label, help| DiagDecl {
        key,
        label,
        unit: "",
        help,
        kind: DiagKind::Scalar,
    };
    vec![
        DiagDecl {
            key: "threshold",
            label: "Band threshold",
            unit: "",
            help: "The threshold of the band each line is in: the bound on the band \
                   sensitivity's condition number that its solve is regularized to. 0 out of \
                   band and under the plain pseudoinverse.",
            kind: DiagKind::PerLine,
        },
        DiagDecl {
            key: "gain_db",
            label: "Gain correction",
            unit: "dB",
            help: "The gain correction on each control's target in the band each line is in.",
            kind: DiagKind::PerControlLine,
        },
        diag_correction(),
        scalar(
            "excluded",
            "Channels left out",
            "Control channels the channel check is leaving out of the solve as failed gauges.",
        ),
        scalar(
            "frf_updates",
            "FRF updates",
            "FRF updates the law has accepted so far.",
        ),
        scalar(
            "held_bands",
            "Bands held",
            "Bands left out of the last FRF update because their conditioning load was capped.",
        ),
        scalar(
            "capped_bands",
            "Bands at the conditioning cap",
            "Bands whose drive could not be conditioned as far as the drive condition limit \
             asks without passing the reference.",
        ),
    ]
}

/// A parameter where 0 means off.
fn positive(v: f64) -> Option<f64> {
    (v > 0.0).then_some(v)
}

impl BuiltinMimoLaw for BandAverageLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "band average",
            version: "2026.11.10",
            author: "Brandon Zwink",
            summary: "Solves the inverse in fractional-octave bands: one flat correlated drive \
                      per band whose band-averaged response matches the band-averaged reference, \
                      through the band cross-power sensitivity. Averaging the FRF over each band \
                      before the inverse keeps identification noise from being amplified. A \
                      threshold per band is chosen by a sweep at the first solve and held; gain \
                      correction and optional FRF updates then run on the measured response.",
            params: parameters(),
            diagnostics: diagnostics(),
            shapes: ALL_SHAPES.to_vec(),
            needs: NEEDS.to_vec(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        setup.check()?;
        let mut p = Params {
            df: setup.df_hz,
            drive_rms_limit: None,
            ..Params::default()
        };
        let mut estimate_frames = 20;
        let mut cross = "off".to_string();
        let mut floor = 0.1;
        let choice =
            |key: &str, got: &str| LawError(format!("parameter `{key}` has no option `{got}`"));
        for (key, v) in params {
            match key.as_str() {
                "oct_order" => p.oct_order = v.integer(key)?.max(1) as usize,
                "kappa_rule" => {
                    p.kappa_rule = match v.choice(key)? {
                        "front" => KappaRule::Front,
                        "knee" => KappaRule::Knee,
                        "lcurve" => KappaRule::LCurve,
                        other => return Err(choice(key, other)),
                    }
                }
                "drive_reduction" => p.drive_reduction = v.number(key)?,
                "regularization" => {
                    p.regularization = match v.choice(key)? {
                        "auto" => Regularization::Auto,
                        "none" => Regularization::None,
                        other => return Err(choice(key, other)),
                    }
                }
                "condition_number" => p.condition_number = positive(v.number(key)?),
                "constrained_solve" => p.constrained_solve = v.toggle(key)?,
                "gain_correction" => p.gain_correction = v.toggle(key)?,
                "gain_correction_rate" => p.gain_correction_rate = v.number(key)?,
                "channel_check_db" => p.channel_check_db = positive(v.number(key)?),
                "drive_rms_limit" => {
                    p.drive_rms_limit = positive(v.number(key)?).map(|limit| vec![limit])
                }
                "use_tf_update" => p.use_tf_update = v.toggle(key)?,
                "estimate_frames" => estimate_frames = v.integer(key)?.max(1) as usize,
                "drive_condition_number" => p.drive_condition_number = v.number(key)?,
                "min_drive_fraction" => p.min_drive_fraction = v.number(key)?,
                "hold_capped_bands" => p.hold_capped_bands = v.toggle(key)?,
                "min_coherence" => p.min_coherence = positive(v.number(key)?),
                "tf_update_blend" => {
                    p.tf_update_blend = positive(v.number(key)?)
                        .map_or(TfUpdateBlend::Cumulative, TfUpdateBlend::Beta)
                }
                "n_e_seed" => p.n_e_seed = positive(v.number(key)?),
                "cross_terms" => cross = v.choice(key)?.to_string(),
                "coherence_floor" => floor = v.number(key)?,
                other => return Err(LawError(format!("parameter `{other}` is not declared"))),
            }
        }
        let cross = match cross.as_str() {
            "off" => CrossTerms::Off,
            "reference" => CrossTerms::Reference,
            "buzz" => CrossTerms::Buzz { floor },
            other => return Err(choice("cross_terms", other)),
        };
        p.control_to_cross_terms = cross != CrossTerms::Off;
        let (use_tf_update, uses_coherence) = (p.use_tf_update, p.min_coherence.is_some());
        let mut law = BandAverage::new(p, setup.lines, setup.controls, setup.drives);
        law.set_threads(workers());
        Ok(BandAverageLaw {
            use_tf_update,
            uses_coherence,
            law,
            frequencies: (0..setup.lines).map(|k| k as f64 * setup.df_hz).collect(),
            lines: setup.lines,
            controls: setup.controls,
            drives: setup.drives,
            band: setup.band,
            estimate_frames,
            cross,
            last: None,
        })
    }
}
