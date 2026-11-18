//! The five built-in laws, natively: thin adapters from this crate's law
//! traits ([`MimoSineLaw`], [`MimoSineLawFactory`]) to the law mathematics in
//! `hertzforge-mimo-sine-law-math`, the same code the law files are built
//! from.
//!
//! The adapters translate names only: the math crate's `f_hz`,
//! `settle_open`, `fault`, an infinite limit bound and a request of `None`
//! are this crate's `frequency_hz`, `settling`, `faulted`, a bound of `None`
//! and no request. A law is made by [`MimoSineLawFactory::build`], off the
//! real-time path: `matrix` fits its model there (bounded work, seconds).

use crate::law::{
    LawDescriptor, LawManifest, LawSetup, LawValues, MimoSineLaw, MimoSineLawFactory, ParamSetting,
};
use crate::setup::{TestType, Tracking};
use crate::tick::{Hold, LawFault, Request, TickInputs, TickOutputs};
use hertzforge_mimo_sine_law_math as lm;
use hertzforge_sdm::law::MimoShape;
use lm::law::SineLaw;
use serde_json::{Map, Value};
use std::sync::Arc;

/// The test types a math crate's constant names.
pub fn test_types(t: lm::TestTypes) -> Vec<TestType> {
    let mut v = Vec::new();
    if t.sweep {
        v.push(TestType::Sweep);
    }
    if t.dwell {
        v.push(TestType::Dwell);
    }
    if t.stepped {
        v.push(TestType::Stepped);
    }
    v
}

fn shape(s: MimoShape) -> lm::Shape {
    match s {
        MimoShape::Square => lm::Shape::Square,
        MimoShape::MoreControlsThanDrives => lm::Shape::Tall,
        MimoShape::FewerControlsThanDrives => lm::Shape::Wide,
    }
}

/// The tracking rule as the math crate takes it.
pub fn tracking(t: Tracking) -> lm::TrackingRule {
    match t {
        Tracking::Proportional { pct, max_hz } => lm::TrackingRule::Proportional { pct, max_hz },
        Tracking::Fixed { hz } => lm::TrackingRule::Fixed { hz },
    }
}

/// A law's setup as the math crate takes it. Each control's alarm and abort
/// bands become one number per control, the tightest over the grid; a limit
/// line without a bound is an infinite bound; an identification level that
/// could not be measured is read as full level.
pub fn math_setup(s: &LawSetup) -> lm::Setup {
    let lines = s.lines();
    let tight = |a: &[f64], b: &[f64], c: usize| {
        (0..lines)
            .map(|k| a[c * lines + k].min(b[c * lines + k]))
            .fold(f64::INFINITY, f64::min)
    };
    let t = &s.tolerance;
    lm::Setup {
        controls: s.controls,
        drives: s.drives,
        limits: s.limits,
        fs_hz: s.fs_hz,
        tick_s: s.tick_s,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        drive_ceiling_v: s.drive_ceiling_v.clone(),
        identification: lm::Identification {
            grid_hz: s.identification.frequency_hz.clone(),
            response: s.identification.response.clone(),
            limit_response: s.identification.limit_response.clone(),
            coherence: s.identification.coherence.clone(),
            limit_coherence: s.identification.limit_coherence.clone(),
            trusted: s.identification.trusted.clone(),
        },
        identification_level_db: s.identification_level_db.unwrap_or(0.0),
        reference: s.reference.clone(),
        phase_specified: s.phase_specified,
        limit_bounds: s
            .limit_bounds
            .iter()
            .map(|b| b.unwrap_or(f64::INFINITY))
            .collect(),
        alarm_db: (0..s.controls)
            .map(|c| tight(&t.alarm_low_db, &t.alarm_high_db, c))
            .collect(),
        abort_db: (0..s.controls)
            .map(|c| tight(&t.abort_low_db, &t.abort_high_db, c))
            .collect(),
        shape: shape(s.shape),
        tracking: tracking(s.tracking),
    }
}

/// One native built-in law: the math crate's law behind this crate's trait.
pub struct NativeLaw<L: SineLaw> {
    name: &'static str,
    law: L,
    out: lm::Outputs,
    bounds: Vec<f64>,
}

impl<L: SineLaw> NativeLaw<L> {
    fn new(name: &'static str, law: L, drives: usize, limits: usize) -> Self {
        let values = law.value_names().len();
        NativeLaw {
            name,
            law,
            out: lm::Outputs::new(drives, values),
            bounds: vec![f64::INFINITY; limits],
        }
    }

    fn call(
        &mut self,
        engage: bool,
        x: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault> {
        for (b, x) in self.bounds.iter_mut().zip(x.limit_bounds) {
            *b = x.unwrap_or(f64::INFINITY);
        }
        let inputs = lm::Inputs {
            index: x.index,
            dt_s: x.dt_s,
            f_hz: x.frequency_hz,
            sweep_hz_s: x.sweep_hz_per_s,
            sweep_oct_min: x.sweep_oct_per_min,
            level: x.level,
            reference: x.reference,
            controls: x.controls,
            limits: x.limits,
            limit_bounds: &self.bounds,
            drive_in_force: x.drive_in_force,
            hold: match x.hold {
                Hold::Free => lm::Hold::Free,
                Hold::Notch => lm::Hold::Notch,
                Hold::Held => lm::Hold::Held,
                Hold::Probe => lm::Hold::Probe,
                Hold::Engage => lm::Hold::Engage,
            },
            notch_scale: x.notch_scale,
            settle_open: x.settling,
        };
        self.out.fault = false;
        self.out.request = lm::Request::None;
        let r = if engage {
            self.law.engage(&inputs, &mut self.out)
        } else {
            self.law.tick(&inputs, &mut self.out)
        };
        r.map_err(LawFault::Failed)?;
        // A wrong count is passed on as it is, for the core to refuse by name.
        out.drive.clear();
        out.drive.extend_from_slice(&self.out.drive);
        out.faulted = self.out.fault;
        out.request = match self.out.request {
            lm::Request::None => None,
            lm::Request::Hold => Some(Request::Hold),
            lm::Request::Slow(f) => Some(Request::Slow(f)),
        };
        for (i, v) in self.out.values.iter().enumerate() {
            values
                .set(i, *v)
                .map_err(|_| LawFault::BadOutput(crate::tick::OutputRule::UndeclaredValue))?;
        }
        Ok(())
    }
}

impl<L: SineLaw + Send> MimoSineLaw for NativeLaw<L> {
    fn name(&self) -> &str {
        self.name
    }

    fn engage(
        &mut self,
        inputs: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.call(true, inputs, out, values)
    }

    fn tick(
        &mut self,
        inputs: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.call(false, inputs, out, values)
    }
}

/// A built-in law's parameters, read from the setup's values by key.
struct Params<'a> {
    map: &'a Map<String, Value>,
    used: Vec<&'static str>,
    out: Vec<(String, ParamSetting)>,
}

impl<'a> Params<'a> {
    fn new(map: &'a Map<String, Value>) -> Self {
        Params {
            map,
            used: Vec::new(),
            out: Vec::new(),
        }
    }

    fn number(&mut self, key: &'static str, default: f64, lo: f64, hi: f64) -> Result<f64, String> {
        self.used.push(key);
        let v = match self.map.get(key) {
            None => default,
            Some(v) => v
                .as_f64()
                .ok_or_else(|| format!("the parameter {key} is not a number"))?,
        };
        if !(v.is_finite() && v >= lo && v <= hi) {
            return Err(format!(
                "the parameter {key} is {v}; {lo} to {hi} is accepted"
            ));
        }
        self.out.push((key.into(), ParamSetting::Number(v)));
        Ok(v)
    }

    fn choice(&mut self, key: &'static str, options: &[&str]) -> Result<String, String> {
        self.used.push(key);
        let v = match self.map.get(key) {
            None => options[0].to_string(),
            Some(v) => v
                .as_str()
                .ok_or_else(|| format!("the parameter {key} is not a name"))?
                .to_string(),
        };
        if !options.contains(&v.as_str()) {
            return Err(format!(
                "the parameter {key} is {v}; one of {} is accepted",
                options.join(", ")
            ));
        }
        self.out.push((key.into(), ParamSetting::Choice(v.clone())));
        Ok(v)
    }

    fn toggle(&mut self, key: &'static str, default: bool) -> Result<bool, String> {
        self.used.push(key);
        let v = match self.map.get(key) {
            None => default,
            Some(v) => v
                .as_bool()
                .ok_or_else(|| format!("the parameter {key} is not on or off"))?,
        };
        self.out.push((key.into(), ParamSetting::Toggle(v)));
        Ok(v)
    }

    fn integer(&mut self, key: &'static str, lo: i64, hi: i64) -> Result<Option<i64>, String> {
        self.used.push(key);
        match self.map.get(key) {
            None => Ok(None),
            Some(v) => {
                let n = v
                    .as_i64()
                    .ok_or_else(|| format!("the parameter {key} is not a whole number"))?;
                if !(lo..=hi).contains(&n) {
                    return Err(format!(
                        "the parameter {key} is {n}; {lo} to {hi} is accepted"
                    ));
                }
                self.out.push((key.into(), ParamSetting::Integer(n)));
                Ok(Some(n))
            }
        }
    }

    /// The gain rule: the loop-speed rule (`g_max`, `beta`), or a fixed gain
    /// when `gain` is given.
    fn gain_rule(&mut self) -> Result<lm::speed::GainRule, String> {
        if self.map.contains_key("gain") {
            let g = self.number("gain", 0.5, 0.0, 2.0)?;
            self.used.push("g_max");
            self.used.push("beta");
            return Ok(lm::speed::GainRule::Fixed { g });
        }
        let g_max = self.number("g_max", 0.5, 0.0, 2.0)?;
        let beta = self.number("beta", 0.5, 0.0, 2.0)?;
        self.used.push("gain");
        Ok(lm::speed::GainRule::LoopSpeed { g_max, beta })
    }

    fn finish(self) -> Result<Vec<(String, ParamSetting)>, String> {
        if let Some(k) = self.map.keys().find(|k| !self.used.contains(&k.as_str())) {
            return Err(format!("the law has no parameter {k}"));
        }
        Ok(self.out)
    }
}

/// Which built-in law.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Matrix,
    Paired,
    SteepestDescent,
    Secant,
    Ratio,
}

impl Builtin {
    /// The built-in law of a catalog id.
    pub fn of(id: &str) -> Option<Builtin> {
        Some(match id {
            "matrix" => Builtin::Matrix,
            "paired" => Builtin::Paired,
            "steepest_descent" => Builtin::SteepestDescent,
            "secant" => Builtin::Secant,
            "ratio" => Builtin::Ratio,
            _ => return None,
        })
    }

    /// The catalog id.
    pub fn id(self) -> &'static str {
        match self {
            Builtin::Matrix => "matrix",
            Builtin::Paired => "paired",
            Builtin::SteepestDescent => "steepest_descent",
            Builtin::Secant => "secant",
            Builtin::Ratio => "ratio",
        }
    }

    fn types(self) -> lm::TestTypes {
        match self {
            Builtin::Matrix => lm::matrix::MATRIX_TEST_TYPES,
            Builtin::Paired => lm::paired::PAIRED_TEST_TYPES,
            Builtin::SteepestDescent => lm::steepest::STEEPEST_TEST_TYPES,
            Builtin::Secant => lm::secant::SECANT_TEST_TYPES,
            Builtin::Ratio => lm::ratio::RATIO_TEST_TYPES,
        }
    }

    fn values(self) -> &'static [&'static str] {
        match self {
            Builtin::Matrix => &lm::matrix::MATRIX_VALUES,
            Builtin::Paired => &lm::paired::PAIRED_VALUES,
            Builtin::SteepestDescent => &lm::steepest::STEEPEST_VALUES,
            Builtin::Secant => &lm::secant::SECANT_VALUES,
            Builtin::Ratio => &lm::ratio::RATIO_VALUES,
        }
    }
}

/// The parameters of a built-in law, checked.
#[derive(Debug, Clone)]
enum BuiltParams {
    Matrix(lm::matrix::MatrixParams),
    Paired(lm::speed::GainRule),
    Steepest(lm::steepest::SteepestParams),
    Secant(lm::secant::SecantParams),
    Ratio(lm::ratio::RatioParams),
}

/// The factory of a built-in law with its parameter values.
#[derive(Debug, Clone)]
pub struct BuiltinFactory {
    which: Builtin,
    params: BuiltParams,
    shown: Vec<(String, ParamSetting)>,
}

impl BuiltinFactory {
    /// The built-in law `id` with the parameter values `params` (defaults fill
    /// the rest); an unknown id, an unknown key or a value out of its range is
    /// refused with the words.
    pub fn new(id: &str, params: &Map<String, Value>) -> Result<BuiltinFactory, String> {
        let which = Builtin::of(id).ok_or_else(|| format!("{id} is not a built-in law"))?;
        let mut p = Params::new(params);
        let built = match which {
            Builtin::Matrix => {
                let d = lm::matrix::MatrixParams::default();
                let carry = p.number("carry", d.carry, 0.0, 1.0)?;
                let gain_rule = p.gain_rule()?;
                let eps = p.number("eps", d.eps, 0.0, 1.0)?;
                let lines = p.integer("model_lines", 0, 64)?.unwrap_or(8) as usize;
                let model = match p
                    .choice("model", &["modal", "smoothed", "identification"])?
                    .as_str()
                {
                    "modal" => lm::modal::ModelChoice::Modal { lines },
                    "smoothed" => lm::modal::ModelChoice::Smoothed { lines },
                    _ => lm::modal::ModelChoice::Identification,
                };
                let free_phase = match p.choice("free_phase", &["in_phase", "measured"])?.as_str() {
                    "in_phase" => lm::matrix::FreePhase::InPhase,
                    _ => lm::matrix::FreePhase::Measured,
                };
                let carry_outside_model = p.toggle("carry_outside_model", false)?;
                BuiltParams::Matrix(lm::matrix::MatrixParams {
                    carry,
                    gain_rule,
                    eps,
                    model,
                    free_phase,
                    carry_outside_model,
                    ..d
                })
            }
            Builtin::Paired => BuiltParams::Paired(p.gain_rule()?),
            Builtin::SteepestDescent => {
                let d = lm::steepest::SteepestParams::default();
                let gain_rule = p.gain_rule()?;
                let eps = p.number("eps", d.eps, 0.0, 1.0)?;
                BuiltParams::Steepest(lm::steepest::SteepestParams { gain_rule, eps })
            }
            Builtin::Secant => {
                let d = lm::secant::SecantParams::default();
                let carry = match p.choice("carry", &["factor", "plain"])?.as_str() {
                    "factor" => lm::secant::SecantCarry::Factor,
                    _ => lm::secant::SecantCarry::Plain,
                };
                let c0 = p.number("c0", d.c0, 0.0, 1.0)?;
                let c_max = p.number("c_max", d.c_max, 0.0, 10.0)?;
                let eps = p.number("eps", d.eps, 0.0, 1.0)?;
                let settle_factor = p.number("settle_factor", d.settle_factor, 0.5, 50.0)?;
                BuiltParams::Secant(lm::secant::SecantParams {
                    carry,
                    c0,
                    c_max,
                    eps,
                    settle_factor,
                    ..d
                })
            }
            Builtin::Ratio => {
                let d = lm::ratio::RatioParams::default();
                let k = p.number("k", d.k, 0.0, 10.0)?;
                let eps = p.number("eps", d.eps, 0.0, 1.0)?;
                let stride = p.integer("stride", 1, 1000)?.map(|n| n as usize);
                let pairing = match p.choice("pairing", &["reference", "drive"])?.as_str() {
                    "reference" => lm::ratio::Pairing::Reference,
                    _ => lm::ratio::Pairing::Drive,
                };
                BuiltParams::Ratio(lm::ratio::RatioParams {
                    k,
                    eps,
                    stride,
                    pairing,
                })
            }
        };
        let shown = p.finish()?;
        Ok(BuiltinFactory {
            which,
            params: built,
            shown,
        })
    }

    /// The built-in law `id` with its defaults.
    pub fn default_of(id: &str) -> Result<BuiltinFactory, String> {
        Self::new(id, &Map::new())
    }
}

/// The factory of a built-in law, shared.
pub fn builtin(
    id: &str,
    params: &Map<String, Value>,
) -> Result<Arc<dyn MimoSineLawFactory>, String> {
    Ok(Arc::new(BuiltinFactory::new(id, params)?))
}

impl MimoSineLawFactory for BuiltinFactory {
    fn build(&self, setup: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        let s = math_setup(setup);
        let (m, p) = (setup.drives, setup.limits);
        let fail = LawFault::Failed;
        Ok(match &self.params {
            BuiltParams::Matrix(q) => Box::new(NativeLaw::new(
                "matrix",
                lm::matrix::MatrixLaw::new(&s, *q).map_err(fail)?,
                m,
                p,
            )),
            BuiltParams::Paired(g) => {
                let mut q = lm::paired::PairedParams::diagonal(setup.controls, setup.drives);
                q.gain_rule = *g;
                Box::new(NativeLaw::new(
                    "paired",
                    lm::paired::PairedLaw::new(&s, q).map_err(fail)?,
                    m,
                    p,
                ))
            }
            BuiltParams::Steepest(q) => Box::new(NativeLaw::new(
                "steepest_descent",
                lm::steepest::SteepestLaw::new(&s, *q).map_err(fail)?,
                m,
                p,
            )),
            BuiltParams::Secant(q) => Box::new(NativeLaw::new(
                "secant",
                lm::secant::SecantLaw::new(&s, *q).map_err(fail)?,
                m,
                p,
            )),
            BuiltParams::Ratio(q) => Box::new(NativeLaw::new(
                "ratio",
                lm::ratio::RatioLaw::new(&s, *q).map_err(fail)?,
                m,
                p,
            )),
        })
    }

    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: self.which.id().into(),
            name: self.which.id().into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sha256: None,
            params: self.shown.clone(),
            diagnostics: self.which.values().iter().map(|s| s.to_string()).collect(),
            live: vec![],
        }
    }

    fn manifest(&self) -> LawManifest {
        let mut shapes = vec![
            MimoShape::Square,
            MimoShape::MoreControlsThanDrives,
            MimoShape::FewerControlsThanDrives,
        ];
        if let BuiltParams::Ratio(q) = &self.params {
            if q.pairing == lm::ratio::Pairing::Drive {
                shapes = vec![MimoShape::Square];
            }
        }
        LawManifest {
            shapes,
            test_types: test_types(self.which.types()),
            amplitude_only: true,
            values: self.which.values().iter().map(|s| s.to_string()).collect(),
        }
    }
}
