//! The five built-in laws, natively: [`BuiltinFactory`] implements this
//! crate's [`MimoSineLawFactory`] over the SDK's built-in constructor
//! (`sine_mimo::builtin`), so a built-in law runs natively the same
//! mathematics, with the same declared parameters, keys, ranges and defaults,
//! as its law file. This is the one native form: the laws crate resolves a
//! built-in through it and its parity tests compare law files with it.
//!
//! The setup and the inputs reach the mathematics through the same
//! conversions a law file's do: a limit channel without a bound is an
//! infinite bound, an identification level that was not measured is not a
//! number, the tracking share goes through the law interface's fraction
//! (`pct / 100 * 100`), and each control's alarm and abort bands are the
//! tightest over its lines. A value that is not a finite number is not
//! reported, as the SDK does not report it from a law file. A law is made by
//! [`MimoSineLawFactory::build`], off the real-time path: `matrix` fits its
//! model there (bounded work, seconds).

use crate::law::{
    LawDescriptor, LawManifest, LawSetup, LawValues, MimoSineLaw, MimoSineLawFactory, ParamSetting,
};
use crate::setup::{TestType, Tracking};
use crate::tick::{Hold, LawFault, OutputRule, Request, TickInputs, TickOutputs};
use hertzforge_law_sdk::sine_mimo::builtin::{self as decl, BuiltLaw, Config, LawDecl};
use hertzforge_law_sdk::sine_mimo::math::law::SineLaw;
use hertzforge_law_sdk::sine_mimo::{self as sdk, math};
use hertzforge_law_sdk::Params;
use hertzforge_sdm::law::MimoShape;
use std::sync::Arc;

/// The core's shape as the SDK's.
pub fn sdk_shape(s: MimoShape) -> sdk::Shape {
    match s {
        MimoShape::Square => sdk::Shape::Square,
        MimoShape::MoreControlsThanDrives => sdk::Shape::Tall,
        MimoShape::FewerControlsThanDrives => sdk::Shape::Wide,
    }
}

/// The SDK's shape as the core's.
pub fn core_shape(s: sdk::Shape) -> MimoShape {
    match s {
        sdk::Shape::Square => MimoShape::Square,
        sdk::Shape::Tall => MimoShape::MoreControlsThanDrives,
        sdk::Shape::Wide => MimoShape::FewerControlsThanDrives,
    }
}

/// The SDK's test type as the core's.
pub fn core_test_type(t: sdk::TestType) -> TestType {
    match t {
        sdk::TestType::Sweep => TestType::Sweep,
        sdk::TestType::Dwell => TestType::Dwell,
        sdk::TestType::Stepped => TestType::Stepped,
    }
}

/// The core's test type as the SDK's.
pub fn sdk_test_type(t: TestType) -> sdk::TestType {
    match t {
        TestType::Sweep => sdk::TestType::Sweep,
        TestType::Dwell => sdk::TestType::Dwell,
        TestType::Stepped => sdk::TestType::Stepped,
    }
}

/// The tracking rule as a law reads it: a proportional rule's percentage
/// passes through the law interface's share of the frequency.
pub fn tracking_rule(t: Tracking) -> sdk::TrackingRule {
    match t {
        Tracking::Proportional { pct, max_hz } => sdk::TrackingRule::Proportional {
            pct: (pct / 100.0) * 100.0,
            max_hz,
        },
        Tracking::Fixed { hz } => sdk::TrackingRule::Fixed { hz },
    }
}

/// The core's law setup as the SDK's plain setup.
pub fn sdk_setup(s: &LawSetup) -> sdk::Setup {
    let id = &s.identification;
    sdk::Setup {
        controls: s.controls,
        drives: s.drives,
        limits: s.limits,
        fs_hz: s.fs_hz,
        tick_s: s.tick_s,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        test_type: sdk_test_type(s.test_type),
        shape: sdk_shape(s.shape),
        drive_ceiling_v: s.drive_ceiling_v.clone(),
        identification: sdk::Identification {
            frequency_hz: id.frequency_hz.clone(),
            response: id.response.clone(),
            limit_response: if s.limits == 0 {
                sdk::MatrixStack::zeros(id.frequency_hz.len(), 0, s.drives)
            } else {
                id.limit_response.clone()
            },
            coherence: id.coherence.clone(),
            limit_coherence: id.limit_coherence.clone(),
            trusted: id.trusted.clone(),
        },
        identification_level_db: s.identification_level_db.unwrap_or(f64::NAN),
        reference: s.reference.clone(),
        phase_specified: s.phase_specified,
        limit_bounds: s
            .limit_bounds
            .iter()
            .map(|b| b.unwrap_or(f64::INFINITY))
            .collect(),
        bands: sdk::Bands {
            alarm_low_db: s.tolerance.alarm_low_db.clone(),
            alarm_high_db: s.tolerance.alarm_high_db.clone(),
            abort_low_db: s.tolerance.abort_low_db.clone(),
            abort_high_db: s.tolerance.abort_high_db.clone(),
        },
        tracking: tracking_rule(s.tracking),
        loop_delay: sdk::LoopDelay {
            tracker_delay_factor: s.tracker_delay_factor,
            transport_delay_s: s.transport_delay_s,
        },
    }
}

/// A host or SDK parameter value as it is recorded.
pub fn setting(v: &hertzforge_law_sdk::ParamValue) -> Option<ParamSetting> {
    use hertzforge_law_sdk::ParamValue as V;
    Some(match v {
        V::Number(x) => ParamSetting::Number(*x),
        V::Integer(x) => ParamSetting::Integer(*x),
        V::Toggle(x) => ParamSetting::Toggle(*x),
        V::Choice(x) => ParamSetting::Choice(x.clone()),
        V::Curve(_) | V::PerChannel(_) => return None,
    })
}

/// The core's manifest of a built-in declaration.
pub fn decl_manifest(d: &LawDecl) -> LawManifest {
    LawManifest {
        shapes: d.shapes.iter().map(|s| core_shape(*s)).collect(),
        test_types: d.test_types().into_iter().map(core_test_type).collect(),
        amplitude_only: d.amplitude_only,
        values: d.values.iter().map(|v| v.key.to_string()).collect(),
    }
}

/// Builds a built-in law for each run from its declaration and checked
/// parameter values.
#[derive(Debug, Clone)]
pub struct BuiltinFactory {
    decl: &'static LawDecl,
    params: Params,
    config: Config,
}

impl BuiltinFactory {
    /// The built-in law `decl` with `params` (every declared key, checked;
    /// [`decl::defaults`] with changes).
    pub fn new(decl: &'static LawDecl, params: Params) -> Result<Self, String> {
        let config = decl::config(decl.id, &params)?;
        Ok(BuiltinFactory {
            decl,
            params,
            config,
        })
    }

    /// The declaration.
    pub fn decl(&self) -> &'static LawDecl {
        self.decl
    }

    /// The parameter values.
    pub fn params(&self) -> &Params {
        &self.params
    }
}

impl MimoSineLawFactory for BuiltinFactory {
    fn build(&self, setup: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        let law = self
            .config
            .build(&sdk_setup(setup).to_math())
            .map_err(LawFault::Failed)?;
        let names = law.value_names().len();
        Ok(Box::new(NativeLaw {
            name: self.decl.name,
            law,
            out: math::Outputs::new(setup.drives, names),
            bounds: vec![f64::INFINITY; setup.limits],
        }))
    }

    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: self.decl.id.into(),
            name: self.decl.name.into(),
            version: self.decl.version.into(),
            sha256: None,
            params: self
                .params
                .0
                .iter()
                .filter_map(|p| setting(&p.value).map(|s| (p.key.clone(), s)))
                .collect(),
            diagnostics: self.decl.values.iter().map(|v| v.key.into()).collect(),
            live: Vec::new(),
        }
    }

    fn manifest(&self) -> LawManifest {
        decl_manifest(self.decl)
    }
}

/// A built-in law built for one run, as the core's [`MimoSineLaw`].
pub struct NativeLaw {
    name: &'static str,
    law: BuiltLaw,
    out: math::Outputs,
    bounds: Vec<f64>,
}

/// The core's inputs as the mathematics takes them; `bounds` holds the limit
/// bounds, infinite where there is none.
pub fn math_inputs<'a>(x: &TickInputs<'a>, bounds: &'a mut Vec<f64>) -> math::Inputs<'a> {
    bounds.clear();
    bounds.extend(x.limit_bounds.iter().map(|b| b.unwrap_or(f64::INFINITY)));
    math::Inputs {
        index: x.index,
        dt_s: x.dt_s,
        f_hz: x.frequency_hz,
        sweep_hz_s: x.sweep_hz_per_s,
        sweep_oct_min: x.sweep_oct_per_min,
        level: x.level,
        reference: x.reference,
        controls: x.controls,
        limits: x.limits,
        limit_bounds: bounds,
        drive_in_force: x.drive_in_force,
        hold: match x.hold {
            Hold::Free => math::Hold::Free,
            Hold::Notch => math::Hold::Notch,
            Hold::Held => math::Hold::Held,
            Hold::Probe => math::Hold::Probe,
            Hold::Engage => math::Hold::Engage,
        },
        notch_scale: x.notch_scale,
        settle_open: x.settling,
    }
}

impl NativeLaw {
    fn call(
        &mut self,
        engage: bool,
        x: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault> {
        let mut bounds = std::mem::take(&mut self.bounds);
        self.out.request = math::Request::None;
        let r = {
            let mx = math_inputs(x, &mut bounds);
            if engage {
                self.law.engage(&mx, &mut self.out)
            } else {
                self.law.tick(&mx, &mut self.out)
            }
        };
        self.bounds = bounds;
        r.map_err(LawFault::Failed)?;
        if self.out.drive.len() != out.drive.len() {
            return Err(LawFault::BadOutput(OutputRule::WrongDriveCount));
        }
        out.drive.copy_from_slice(&self.out.drive);
        out.faulted = self.out.fault;
        out.request = match self.out.request {
            math::Request::None => None,
            math::Request::Hold => Some(Request::Hold),
            math::Request::Slow(f) => Some(Request::Slow(f)),
        };
        out.notched = false;
        for (i, v) in self.out.values.iter().enumerate() {
            if v.is_finite() {
                values
                    .set(i, *v)
                    .map_err(|_| LawFault::BadOutput(OutputRule::UndeclaredValue))?;
            }
        }
        Ok(())
    }
}

impl MimoSineLaw for NativeLaw {
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

/// The built-in law `id` at its declared defaults, shared; `None` for an id
/// that is not a built-in. Parameters set by a run go through the laws
/// crate, which checks them as the law host checks a law file's.
pub fn builtin(id: &str) -> Option<Arc<dyn MimoSineLawFactory>> {
    let d = decl::decl(id)?;
    BuiltinFactory::new(d, decl::defaults(d))
        .ok()
        .map(|f| Arc::new(f) as Arc<dyn MimoSineLawFactory>)
}
