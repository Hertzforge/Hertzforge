//! Running two MIMO Shock laws side by side: [`compare_laws`] plays a scripted
//! run (a ladder of levels through a plant the caller supplies) and feeds both
//! laws the same inputs after every pulse, then reports how far apart their
//! drives were. It is how a law file is shown to run exactly as the same
//! mathematics runs natively ([`MathLaw`]), and how a native built-in is shown
//! to match its law file.

use hertzforge_law_sdk::shock_mimo::math::{self, LawSteps};
use hertzforge_mimo_shock::{
    ControlVerdict, MimoShockLaw, PulseInputs, PulseOutputs, Setup, Target,
};
use hertzforge_shock_common::law_guard::{LawFault, LawValue};

/// `n` rows of `len` from a flat channel-major list.
fn rows(flat: &[f64], len: usize) -> Vec<Vec<f64>> {
    flat.chunks(len.max(1)).map(<[f64]>::to_vec).collect()
}

/// The core's setup as the law mathematics takes it.
pub fn math_setup(s: &Setup) -> math::Setup {
    math::Setup {
        fs_hz: s.fs_hz,
        record_len: s.record_len,
        n_drives: s.n_drives,
        n_controls: s.n_controls,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        reference: rows(s.target.waveforms(), s.record_len),
        srs: match &s.target {
            Target::Waveform(_) => None,
            Target::Srs(t) => Some(math::SrsTarget {
                natural_hz: t.natural_hz.clone(),
                q: t.q,
                points: rows(&t.points, t.natural_hz.len()),
            }),
        },
        plant: math::PlantModel {
            frf: s.plant.frf.clone(),
            trusted: s.plant.trusted.clone(),
        },
    }
}

/// The core's pulse inputs as the law mathematics takes them (whose pulse
/// index counts the pulse just played from 0).
pub fn math_inputs(i: &PulseInputs, s: &Setup) -> math::PulseInputs {
    let len = s.record_len;
    math::PulseInputs {
        pulse_index: i.pulse_index.saturating_sub(1) as usize,
        level_db: i.level_db,
        next_level_db: i.next_level_db,
        drives: rows(&i.drives, len),
        responses: rows(&i.responses, len),
        srs: match (&i.srs, &s.target) {
            (Some(m), Target::Srs(t)) => Some(rows(m, t.natural_hz.len())),
            _ => None,
        },
    }
}

fn outputs(o: math::PulseOutputs) -> PulseOutputs {
    PulseOutputs {
        drives: o.drives.into_iter().flatten().collect(),
        delays_s: o.delays_s,
        values: o
            .values
            .into_iter()
            .map(|(k, v)| LawValue::new(k.to_string(), v))
            .collect(),
    }
}

/// A law of the shared mathematics ([`LawSteps`]) called natively through the
/// core's law trait. A reference for parity tests: the native built-in laws
/// are the engine's own adapters, and the law files are compared with both.
pub struct MathLaw {
    name: String,
    setup: Setup,
    steps: Box<dyn LawSteps + Send>,
}

impl MathLaw {
    /// `steps` under `name`, for the run `setup`.
    pub fn new(name: &str, setup: &Setup, steps: Box<dyn LawSteps + Send>) -> Self {
        MathLaw {
            name: name.into(),
            setup: setup.clone(),
            steps,
        }
    }

    /// `inverse` with `params`.
    pub fn inverse(setup: &Setup, params: math::inverse::InverseParams) -> Result<Self, String> {
        let law = math::inverse::InverseLaw::new(math_setup(setup), params)?;
        Ok(Self::new("inverse", setup, Box::new(law)))
    }

    /// `shape` with `params`.
    pub fn shape(setup: &Setup, params: math::shape::ShapeParams) -> Result<Self, String> {
        let law = math::shape::ShapeLaw::new(math_setup(setup), params)?;
        Ok(Self::new("shape", setup, Box::new(law)))
    }

    /// `paired` with `params`.
    pub fn paired(setup: &Setup, params: math::paired::PairedParams) -> Result<Self, String> {
        let law = math::paired::PairedLaw::new(math_setup(setup), params)?;
        Ok(Self::new("paired", setup, Box::new(law)))
    }
}

impl MimoShockLaw for MathLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, LawFault> {
        self.steps
            .seed(level_db)
            .map(outputs)
            .map_err(LawFault::Failed)
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, LawFault> {
        self.steps
            .iterate(&math_inputs(inputs, &self.setup))
            .map(outputs)
            .map_err(LawFault::Failed)
    }
}

/// How far apart two laws' drives were over a scripted run.
#[derive(Debug, Clone, PartialEq)]
pub struct ParityReport {
    /// Calls compared: the seed and every iteration.
    pub calls: usize,
    /// The largest difference of any drive sample on any call, relative to
    /// the largest drive sample of that call.
    pub worst_rel: f64,
    /// The call it happened on (0 is the seed).
    pub worst_call: usize,
    /// The largest difference of any reported value or delay, relative to its
    /// size (or absolute under 1).
    pub worst_value_rel: f64,
    /// Whether both reported the same value keys and delay presence on every
    /// call.
    pub same_keys: bool,
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / a.abs().max(1.0)
}

/// Each control's waveform error against its target at the played level, dB:
/// the deterministic stand-in verdict a scripted run hands both laws.
fn verdicts(setup: &Setup, responses: &[f64], level_db: f64) -> Vec<ControlVerdict> {
    let s = 10f64.powf(level_db / 20.0);
    let len = setup.record_len;
    let r = setup.target.waveforms();
    (0..setup.n_controls)
        .map(|c| {
            let (y, t) = (&responses[c * len..][..len], &r[c * len..][..len]);
            let e: f64 = y.iter().zip(t).map(|(y, t)| (y - s * t).powi(2)).sum();
            let n: f64 = t.iter().map(|t| (s * t).powi(2)).sum();
            let error_db = 10.0 * (e.max(1e-300) / n.max(1e-300)).log10();
            let peak = |x: &[f64]| x.iter().fold(0.0f64, |a, v| a.max(v.abs()));
            ControlVerdict {
                passed: error_db < -20.0,
                error_db,
                delay_s: 0.0,
                peak_ratio: peak(y) / (s * peak(t)).max(1e-300),
            }
        })
        .collect()
}

/// Play `levels` (one pulse per entry) through `plant` with law `a`'s drives,
/// hand both laws the same inputs after every pulse (`b`'s own drives are
/// compared, never played) and report how far apart the drives were.
///
/// `plant` maps the flat drives as played (drive after drive) and the level
/// they were played at to the flat responses (control after control). For an
/// SRS target the measured SRS is computed from the responses on the target's
/// frequencies; the verdicts are each control's waveform error.
pub fn compare_laws(
    a: &mut dyn MimoShockLaw,
    b: &mut dyn MimoShockLaw,
    setup: &Setup,
    levels: &[f64],
    plant: &mut dyn FnMut(&[f64], f64) -> Result<Vec<f64>, String>,
) -> Result<ParityReport, String> {
    let first = *levels.first().ok_or("the script has no pulse")?;
    let fault = |w: &str, f: LawFault| format!("law {w}: {} ({})", f.reason(), f_text(&f));
    let mut oa = a.seed(first).map_err(|f| fault("a", f))?;
    let mut ob = b.seed(first).map_err(|f| fault("b", f))?;
    let mut report = ParityReport {
        calls: 0,
        worst_rel: 0.0,
        worst_call: 0,
        worst_value_rel: 0.0,
        same_keys: true,
    };
    for k in 0..levels.len() {
        report.compare(k, &oa, &ob)?;
        if k + 1 == levels.len() {
            break;
        }
        let responses = plant(&oa.drives, levels[k])?;
        let srs = match &setup.target {
            Target::Waveform(_) => None,
            Target::Srs(t) => Some(
                math::srs::srs_rows(
                    &rows(&responses, setup.record_len),
                    setup.fs_hz,
                    &t.natural_hz,
                    t.q,
                    1.0,
                )
                .into_iter()
                .flatten()
                .collect(),
            ),
        };
        let inputs = PulseInputs {
            pulse_index: k as u64 + 1,
            level_db: levels[k],
            next_level_db: levels[k + 1],
            verdicts: verdicts(setup, &responses, levels[k]),
            drives: oa.drives.clone(),
            responses,
            srs,
            limited: false,
        };
        oa = a.iterate(&inputs).map_err(|f| fault("a", f))?;
        ob = b.iterate(&inputs).map_err(|f| fault("b", f))?;
    }
    Ok(report)
}

fn f_text(f: &LawFault) -> String {
    match f {
        LawFault::Failed(s) | LawFault::Crashed(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

impl ParityReport {
    fn compare(&mut self, call: usize, a: &PulseOutputs, b: &PulseOutputs) -> Result<(), String> {
        if a.drives.len() != b.drives.len() {
            return Err(format!(
                "call {call}: {} drive samples against {}",
                a.drives.len(),
                b.drives.len()
            ));
        }
        let peak = a.drives.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let diff = a
            .drives
            .iter()
            .zip(&b.drives)
            .fold(0.0f64, |m, (x, y)| m.max((x - y).abs()));
        let r = if peak > 0.0 { diff / peak } else { diff };
        if r > self.worst_rel || r.is_nan() {
            self.worst_rel = r;
            self.worst_call = call;
        }
        let keys = |o: &PulseOutputs| o.values.iter().map(|v| v.key.clone()).collect::<Vec<_>>();
        if keys(a) != keys(b) || a.delays_s.is_some() != b.delays_s.is_some() {
            self.same_keys = false;
        }
        for (x, y) in a.values.iter().zip(&b.values) {
            self.worst_value_rel = self.worst_value_rel.max(rel(x.value, y.value));
        }
        if let (Some(x), Some(y)) = (&a.delays_s, &b.delays_s) {
            for (x, y) in x.iter().zip(y) {
                self.worst_value_rel = self.worst_value_rel.max(rel(*x, *y));
            }
        }
        self.calls += 1;
        Ok(())
    }
}
