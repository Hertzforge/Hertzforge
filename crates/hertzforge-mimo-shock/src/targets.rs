//! Building the targets: what each control is to follow, at the rate the run
//! adopted.
//!
//! A test is specified per control ([`TargetSpec`]) and synthesized here at the
//! run's rate and record length ([`TargetSpec::build`]):
//!
//! - **Classical**: a compensated pulse per control from the classical pulse
//!   library and its compensation (zero net velocity and displacement), each
//!   with its own shape, amplitude, duration and polarity.
//! - **SRS**: per control, the existing synthesis (wavelets or decaying
//!   sinusoids) iterated to that control's SRS, giving the waveform and the
//!   component table the law world carries.
//!
//! Every waveform starts `lead_s` into the record and must end by three
//! quarters of it, so the structure's ring-down stays inside the record.

use crate::setup::{
    ComponentShape, SetupError, SetupRule, SrsComponent, SrsTarget, Target, WaveformTarget,
};
use hertzforge_shock_host::spec::ClassicalPulseSpec;
use hertzforge_shock_host::tolerance::ToleranceStandard;
use hertzforge_srs::iterate::{synthesize_to_target, SynthesisEngine, SynthesisOpts};
use serde::{Deserialize, Serialize};

/// One control's classical pulse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassicalControl {
    /// Shape, amplitude (g, positive), duration and compensation.
    pub pulse: ClassicalPulseSpec,
    /// The pulse is played upside down.
    pub inverted: bool,
}

/// One SRS test: the spectrum each control is judged against and the synthesis
/// that gives each control's waveform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrsSpec {
    /// The SRS natural frequencies, Hz, ascending.
    pub natural_hz: Vec<f64>,
    /// The SRS quality factor.
    pub q: f64,
    /// The full-level target SRS of each control, one row per control.
    pub points: Vec<Vec<f64>>,
    /// The tolerance below and above the target, dB.
    pub tol_lo_db: f64,
    pub tol_hi_db: f64,
    /// The starting synthesis table of each control (its amplitudes are
    /// iterated to the control's SRS).
    pub tables: Vec<SynthesisEngine>,
    /// The synthesis iteration's knobs (its `q` is replaced by [`Self::q`]).
    pub synthesis: SynthesisOpts,
}

/// What a test asks of its controls, before it is synthesized at a rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetSpec {
    /// A classical pulse per control, judged by `standard`.
    Classical {
        controls: Vec<ClassicalControl>,
        standard: ToleranceStandard,
        /// Where every waveform starts, s into the record.
        lead_s: f64,
    },
    /// An SRS per control.
    Srs {
        spec: SrsSpec,
        /// Where every waveform starts, s into the record.
        lead_s: f64,
    },
}

/// How a control is judged, fixed when the target is built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Judging {
    /// A classical pulse per control, judged by its standard after its own
    /// delay is removed.
    Classical {
        controls: Vec<ClassicalControl>,
        standard: ToleranceStandard,
    },
    /// A waveform given sample by sample: every sample within
    /// `tol_frac_of_peak` of the target's peak after the delay is removed.
    Envelope { tol_frac_of_peak: f64 },
    /// The SRS of each control against its target ([`Target::Srs`]).
    Srs,
}

/// The default envelope of a waveform given sample by sample: the classical
/// standard's amplitude tolerance (15 % of the peak).
pub const DEFAULT_ENVELOPE_TOL: f64 = 0.15;

/// A target synthesized at a rate.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltTarget {
    pub target: Target,
    pub judging: Judging,
}

fn shape_err(message: String) -> SetupError {
    SetupError::new(SetupRule::TargetShape, message)
}

/// Lay `wave` into a record of `n` samples starting at `lead`.
fn place(wave: &[f64], lead: usize, n: usize, what: &str) -> Result<Vec<f64>, SetupError> {
    let end = lead + wave.len();
    if wave.is_empty() || end > 3 * n / 4 {
        return Err(shape_err(format!(
            "the {what} runs {} samples from sample {lead}; it must end by sample {} \
             (three quarters of the {n}-sample record) so the response can ring down",
            wave.len(),
            3 * n / 4
        )));
    }
    let mut out = vec![0.0; n];
    out[lead..end].copy_from_slice(wave);
    Ok(out)
}

impl TargetSpec {
    /// The number of controls the spec describes.
    pub fn n_controls(&self) -> usize {
        match self {
            TargetSpec::Classical { controls, .. } => controls.len(),
            TargetSpec::Srs { spec, .. } => spec.points.len(),
        }
    }

    /// Synthesize every control's target at `fs_hz` over `record_len` samples.
    pub fn build(&self, fs_hz: f64, record_len: usize) -> Result<BuiltTarget, SetupError> {
        let lead_of = |lead_s: f64| -> Result<usize, SetupError> {
            if !(lead_s.is_finite() && lead_s >= 0.0) {
                return Err(shape_err(format!("a lead of {lead_s} s")));
            }
            Ok((lead_s * fs_hz).round() as usize)
        };
        match self {
            TargetSpec::Classical {
                controls,
                standard,
                lead_s,
            } => {
                let lead = lead_of(*lead_s)?;
                let mut records = Vec::with_capacity(controls.len() * record_len);
                for (c, ctl) in controls.iter().enumerate() {
                    ctl.pulse.validate().map_err(|e| {
                        shape_err(format!("the pulse of control {c} is not usable: {e:?}"))
                    })?;
                    let sign = if ctl.inverted { -1.0 } else { 1.0 };
                    let wave: Vec<f64> = ctl
                        .pulse
                        .synthesize(fs_hz)
                        .waveform_g
                        .iter()
                        .map(|v| sign * v)
                        .collect();
                    records.extend(place(&wave, lead, record_len, "pulse")?);
                }
                Ok(BuiltTarget {
                    target: Target::Waveform(WaveformTarget { records }),
                    judging: Judging::Classical {
                        controls: controls.clone(),
                        standard: *standard,
                    },
                })
            }
            TargetSpec::Srs { spec, lead_s } => {
                let lead = lead_of(*lead_s)?;
                let nc = spec.points.len();
                if spec.tables.len() != nc {
                    return Err(shape_err(format!(
                        "{} synthesis tables for {nc} controls",
                        spec.tables.len()
                    )));
                }
                let opts = SynthesisOpts {
                    q: spec.q,
                    ..spec.synthesis
                };
                let lead_t = lead as f64 / fs_hz;
                let mut waveforms = Vec::with_capacity(nc * record_len);
                let mut components = Vec::with_capacity(nc);
                for (c, (points, table)) in spec.points.iter().zip(&spec.tables).enumerate() {
                    if points.len() != spec.natural_hz.len() {
                        return Err(shape_err(format!(
                            "control {c} has {} SRS points for {} frequencies",
                            points.len(),
                            spec.natural_hz.len()
                        )));
                    }
                    let r =
                        synthesize_to_target(points, &spec.natural_hz, fs_hz, table.clone(), opts);
                    waveforms.extend(place(
                        &r.waveform,
                        lead,
                        record_len,
                        "synthesized waveform",
                    )?);
                    components.push(component_table(&r.components, lead_t));
                }
                Ok(BuiltTarget {
                    target: Target::Srs(SrsTarget {
                        natural_hz: spec.natural_hz.clone(),
                        q: spec.q,
                        points: spec.points.concat(),
                        tol_lo_db: spec.tol_lo_db,
                        tol_hi_db: spec.tol_hi_db,
                        waveforms,
                        components,
                    }),
                    judging: Judging::Srs,
                })
            }
        }
    }
}

/// The law world's component table of a synthesis, its delays moved by the
/// record's lead.
fn component_table(engine: &SynthesisEngine, lead_s: f64) -> Vec<SrsComponent> {
    match engine {
        SynthesisEngine::Wavsyn(t) => t
            .components
            .iter()
            .map(|k| SrsComponent {
                hz: k.freq_hz,
                amplitude: k.amplitude_g,
                delay_s: k.delay_s + lead_s,
                shape: ComponentShape::HalfCycles(k.half_cycles),
            })
            .collect(),
        SynthesisEngine::Seds(t) => t
            .components
            .iter()
            .map(|k| SrsComponent {
                hz: k.freq_hz,
                amplitude: k.amplitude_g * k.polarity.sign(),
                delay_s: k.delay_s + lead_s,
                shape: ComponentShape::Decay(k.zeta),
            })
            .collect(),
    }
}
