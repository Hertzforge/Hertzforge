//! The native built-in laws: `inverse`, `shape` and `paired`, each a thin
//! adapter from the contract types ([`crate::Setup`], [`crate::PulseInputs`],
//! [`crate::PulseOutputs`]) to the law mathematics in
//! `hertzforge-mimo-shock-law-math`. No mathematics of their own: the adapters
//! only reshape records (flat, channel after channel, to rows and back), turn
//! the iteration count into the math's pulse index, and name the values.
//!
//! [`builtin_factory`] gives each law's factory by name, at its default
//! parameters; each factory also reads its parameters from a JSON object
//! ([`InverseFactory::from_params`] and the like), keyed by the names
//! [`MimoShockLawFactory::describe`] reports.

use crate::law::{LawManifest, MimoShockLaw, MimoShockLawFactory};
use crate::pulse::{PulseInputs, PulseOutputs};
use crate::setup::{Setup, Target, TargetKind};
use hertzforge_mimo_shock_law_math as math;
use hertzforge_mimo_shock_law_math::delay::LeastEffort;
use hertzforge_mimo_shock_law_math::inverse::{InverseLaw, InverseParams};
use hertzforge_mimo_shock_law_math::paired::{PairedLaw, PairedParams};
use hertzforge_mimo_shock_law_math::shape::{Anchor, SeedTiming, ShapeLaw, ShapeParams};
use hertzforge_mimo_shock_law_math::LawSteps;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawValue};
use serde_json::{json, Map, Value};

/// The built-in laws' names.
pub const BUILTIN_NAMES: [&str; 3] = ["inverse", "shape", "paired"];

/// The factory of the built-in law `name` at its default parameters, `None`
/// for any other name.
pub fn builtin_factory(name: &str) -> Option<Box<dyn MimoShockLawFactory>> {
    match name {
        "inverse" => Some(Box::new(InverseFactory::default())),
        "shape" => Some(Box::new(ShapeFactory::default())),
        "paired" => Some(Box::new(PairedFactory::default())),
        _ => None,
    }
}

/// The math crate's setup from the contract's.
pub fn math_setup(setup: &Setup) -> math::Setup {
    let n = setup.record_len;
    let rows = |flat: &[f64]| -> Vec<Vec<f64>> { flat.chunks(n).map(|c| c.to_vec()).collect() };
    let srs = match &setup.target {
        Target::Waveform(_) => None,
        Target::Srs(t) => Some(math::SrsTarget {
            natural_hz: t.natural_hz.clone(),
            q: t.q,
            points: t
                .points
                .chunks(t.natural_hz.len())
                .map(|c| c.to_vec())
                .collect(),
        }),
    };
    math::Setup {
        fs_hz: setup.fs_hz,
        record_len: n,
        n_drives: setup.n_drives,
        n_controls: setup.n_controls,
        band_lo_hz: setup.band_lo_hz,
        band_hi_hz: setup.band_hi_hz,
        reference: rows(setup.target.waveforms()),
        srs,
        plant: math::PlantModel {
            frf: setup.plant.frf.clone(),
            trusted: setup.plant.trusted.clone(),
        },
    }
}

fn math_inputs(record_len: usize, nf: usize, inputs: &PulseInputs) -> math::PulseInputs {
    let rows = |flat: &[f64], n: usize| -> Vec<Vec<f64>> {
        flat.chunks(n.max(1)).map(|c| c.to_vec()).collect()
    };
    math::PulseInputs {
        pulse_index: inputs.pulse_index.saturating_sub(1) as usize,
        level_db: inputs.level_db,
        next_level_db: inputs.next_level_db,
        drives: rows(&inputs.drives, record_len),
        responses: rows(&inputs.responses, record_len),
        srs: inputs.srs.as_ref().map(|s| rows(s, nf)),
    }
}

fn contract_outputs(out: math::PulseOutputs) -> PulseOutputs {
    PulseOutputs {
        drives: out.drives.concat(),
        delays_s: out.delays_s,
        values: out
            .values
            .into_iter()
            .map(|(k, v)| LawValue::new(k, v))
            .collect(),
    }
}

/// A built-in law: the math law and the sizes the adapter needs.
struct Adapter<L: LawSteps> {
    name: &'static str,
    law: L,
    record_len: usize,
    srs_freqs: usize,
}

impl<L: LawSteps> MimoShockLaw for Adapter<L> {
    fn name(&self) -> &str {
        self.name
    }

    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, LawFault> {
        self.law
            .seed(level_db)
            .map(contract_outputs)
            .map_err(LawFault::Failed)
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, LawFault> {
        self.law
            .iterate(&math_inputs(self.record_len, self.srs_freqs, inputs))
            .map(contract_outputs)
            .map_err(LawFault::Failed)
    }
}

fn adapter<L: LawSteps + Send + 'static>(
    name: &'static str,
    setup: &Setup,
    law: L,
) -> Box<dyn MimoShockLaw + Send> {
    let srs_freqs = match &setup.target {
        Target::Srs(t) => t.natural_hz.len(),
        Target::Waveform(_) => 0,
    };
    Box::new(Adapter {
        name,
        law,
        record_len: setup.record_len,
        srs_freqs,
    })
}

fn descriptor(id: &str, params: Value) -> LawDescriptor {
    LawDescriptor {
        id: id.into(),
        name: id.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        sha256: None,
        params,
    }
}

fn keys(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

/// Read the keys of `params` into `set`, refusing an unknown key or a value of
/// the wrong type.
fn read_params(
    params: &Value,
    mut set: impl FnMut(&str, &Value) -> Option<Result<(), String>>,
) -> Result<(), String> {
    let obj: &Map<String, Value> = params
        .as_object()
        .ok_or("the parameters are not a JSON object")?;
    for (k, v) in obj {
        match set(k, v) {
            Some(r) => r.map_err(|e| format!("parameter `{k}`: {e}"))?,
            None => return Err(format!("no parameter is named `{k}`")),
        }
    }
    Ok(())
}

fn num(v: &Value) -> Result<f64, String> {
    v.as_f64().ok_or_else(|| "not a number".to_string())
}

fn flag(v: &Value) -> Result<bool, String> {
    v.as_bool().ok_or_else(|| "not true or false".to_string())
}

fn count(v: &Value) -> Result<usize, String> {
    v.as_u64()
        .map(|u| u as usize)
        .ok_or_else(|| "not a whole number".to_string())
}

/// `inverse`: the regularized inverse of the plant per line, corrected between
/// pulses with a gain per line (accepts waveform and SRS targets; an SRS target
/// is followed through the waveform synthesized for it).
#[derive(Debug, Clone, Default)]
pub struct InverseFactory {
    pub params: InverseParams,
}

impl InverseFactory {
    /// The factory at the defaults with the keys of `params` set.
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let mut p = InverseParams::default();
        read_params(params, |k, v| {
            Some(match k {
                "lambda_rel" => num(v).map(|x| p.lambda_rel = x),
                "gain" => num(v).map(|x| p.gain = x),
                "watchdog" => flag(v).map(|x| p.watchdog = x),
                "watchdog_lines" => count(v).map(|x| p.watchdog_lines = x),
                "watchdog_grow" => num(v).map(|x| p.watchdog_grow = x),
                "watchdog_cut" => num(v).map(|x| p.watchdog_cut = x),
                "watchdog_restore" => num(v).map(|x| p.watchdog_restore = x),
                "refresh" => flag(v).map(|x| p.refresh = x),
                "refresh_mu" => num(v).map(|x| p.refresh_mu = x),
                "refresh_floor_rel" => num(v).map(|x| p.refresh_floor_rel = x),
                "align" => flag(v).map(|x| p.align = x),
                "align_max_s" => num(v).map(|x| p.align_max_s = x),
                _ => return None,
            })
        })?;
        Ok(InverseFactory { params: p })
    }
}

impl MimoShockLawFactory for InverseFactory {
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        let law =
            InverseLaw::new(math_setup(setup), self.params.clone()).map_err(LawFault::Failed)?;
        Ok(adapter("inverse", setup, law))
    }

    fn describe(&self) -> LawDescriptor {
        let p = &self.params;
        descriptor(
            "inverse",
            json!({
                "lambda_rel": p.lambda_rel, "gain": p.gain, "watchdog": p.watchdog,
                "watchdog_lines": p.watchdog_lines, "watchdog_grow": p.watchdog_grow,
                "watchdog_cut": p.watchdog_cut, "watchdog_restore": p.watchdog_restore,
                "refresh": p.refresh, "refresh_mu": p.refresh_mu,
                "refresh_floor_rel": p.refresh_floor_rel, "align": p.align,
                "align_max_s": p.align_max_s,
            }),
        )
    }

    fn manifest(&self) -> LawManifest {
        LawManifest {
            targets: TargetKind::ALL.to_vec(),
            values: keys(&[
                "drive_rms_full_level",
                "waveform_error",
                "least_line_gain",
                "lines_cut",
                "lines_refreshed",
            ]),
        }
    }
}

/// `shape`: each control's target shaped by a real gain from its SRS ratio,
/// the drives the plant's inverse applied to the shaped targets (SRS targets
/// only).
#[derive(Debug, Clone, Default)]
pub struct ShapeFactory {
    pub params: ShapeParams,
}

impl ShapeFactory {
    /// The factory at the defaults with the keys of `params` set. `weights` is
    /// a list of `[level_db, weight]` pairs; `anchor` is `target` or
    /// `measured`; `seed_timing` is `zero` or `least_effort` (at the search's
    /// default settings).
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let mut p = ShapeParams::default();
        read_params(params, |k, v| {
            Some(match k {
                "lambda_rel" => num(v).map(|x| p.lambda_rel = x),
                "max_step_db" => num(v).map(|x| p.max_step_db = x),
                "delay_relax" => num(v).map(|x| p.delay_relax = x),
                "delay_bound_s" => num(v).map(|x| p.delay_bound_s = x),
                "refresh" => flag(v).map(|x| p.refresh = x),
                "refresh_mu" => num(v).map(|x| p.refresh_mu = x),
                "refresh_floor_rel" => num(v).map(|x| p.refresh_floor_rel = x),
                "anchor" => match v.as_str() {
                    Some("target") => Ok(p.anchor = Anchor::Target),
                    Some("measured") => Ok(p.anchor = Anchor::Measured),
                    _ => Err("`target` or `measured`".into()),
                },
                "seed_timing" => match v.as_str() {
                    Some("zero") => Ok(p.seed_timing = SeedTiming::Zero),
                    Some("least_effort") => {
                        Ok(p.seed_timing = SeedTiming::LeastEffort(LeastEffort::default()))
                    }
                    _ => Err("`zero` or `least_effort`".into()),
                },
                "weights" => v
                    .as_array()
                    .and_then(|rows| {
                        rows.iter()
                            .map(|r| {
                                let r = r.as_array()?;
                                Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?))
                            })
                            .collect::<Option<Vec<_>>>()
                    })
                    .map(|w| p.weights = w)
                    .ok_or_else(|| "a list of [level_db, weight] pairs".into()),
                _ => return None,
            })
        })?;
        Ok(ShapeFactory { params: p })
    }
}

impl MimoShockLawFactory for ShapeFactory {
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        let law =
            ShapeLaw::new(math_setup(setup), self.params.clone()).map_err(LawFault::Failed)?;
        Ok(adapter("shape", setup, law))
    }

    fn describe(&self) -> LawDescriptor {
        let p = &self.params;
        descriptor(
            "shape",
            json!({
                "lambda_rel": p.lambda_rel,
                "weights": p.weights.iter().map(|(l, w)| json!([l, w])).collect::<Vec<_>>(),
                "max_step_db": p.max_step_db, "delay_relax": p.delay_relax,
                "delay_bound_s": p.delay_bound_s,
                "anchor": match p.anchor { Anchor::Target => "target", Anchor::Measured => "measured" },
                "refresh": p.refresh, "refresh_mu": p.refresh_mu,
                "refresh_floor_rel": p.refresh_floor_rel,
                "seed_timing": match p.seed_timing {
                    SeedTiming::Zero => "zero",
                    SeedTiming::LeastEffort(_) => "least_effort",
                },
            }),
        )
    }

    fn manifest(&self) -> LawManifest {
        LawManifest {
            targets: vec![TargetKind::Srs],
            values: keys(&[
                "effort_cost_zero_delay",
                "effort_cost_seed",
                "drive_rms_full_level",
                "worst_srs_error_db",
                "weight",
                "lines_refreshed",
            ]),
        }
    }
}

/// `paired`: each control paired with one drive, that drive's spectrum scaled
/// by the ratio of wanted to measured magnitude (waveform and SRS targets).
#[derive(Debug, Clone)]
pub struct PairedFactory {
    /// The drive paired with each control; `None` pairs control `c` with drive
    /// `c` for the first `min(controls, drives)` controls.
    pub drive_of: Option<Vec<usize>>,
    pub smooth_lines: usize,
    pub weight: f64,
    pub max_step_db: f64,
}

impl Default for PairedFactory {
    fn default() -> Self {
        let d = PairedParams::diagonal(1, 1);
        PairedFactory {
            drive_of: None,
            smooth_lines: d.smooth_lines,
            weight: d.weight,
            max_step_db: d.max_step_db,
        }
    }
}

impl PairedFactory {
    /// The factory at the defaults with the keys of `params` set.
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let mut p = PairedFactory::default();
        read_params(params, |k, v| {
            Some(match k {
                "smooth_lines" => count(v).map(|x| p.smooth_lines = x),
                "weight" => num(v).map(|x| p.weight = x),
                "max_step_db" => num(v).map(|x| p.max_step_db = x),
                "drive_of" => v
                    .as_array()
                    .and_then(|a| {
                        a.iter()
                            .map(|x| x.as_u64().map(|u| u as usize))
                            .collect::<Option<Vec<_>>>()
                    })
                    .map(|d| p.drive_of = Some(d))
                    .ok_or_else(|| "a list of drive numbers".into()),
                _ => return None,
            })
        })?;
        Ok(p)
    }
}

impl MimoShockLawFactory for PairedFactory {
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        let mut params = PairedParams::diagonal(setup.n_controls, setup.n_drives);
        if let Some(d) = &self.drive_of {
            params.drive_of = d.clone();
        }
        params.smooth_lines = self.smooth_lines;
        params.weight = self.weight;
        params.max_step_db = self.max_step_db;
        let law = PairedLaw::new(math_setup(setup), params).map_err(LawFault::Failed)?;
        Ok(adapter("paired", setup, law))
    }

    fn describe(&self) -> LawDescriptor {
        descriptor(
            "paired",
            json!({
                "drive_of": self.drive_of, "smooth_lines": self.smooth_lines,
                "weight": self.weight, "max_step_db": self.max_step_db,
            }),
        )
    }

    fn manifest(&self) -> LawManifest {
        LawManifest {
            targets: TargetKind::ALL.to_vec(),
            values: keys(&["drive_rms_full_level"]),
        }
    }
}
