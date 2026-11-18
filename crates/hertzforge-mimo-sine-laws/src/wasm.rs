//! A MIMO Sine law file as the core's law: [`WasmSineMimoLaw`] implements
//! [`MimoSineLaw`] over the law host's contained instance, and
//! [`WasmSineMimoLawFactory`] builds one for each run.

use hertzforge_law_host::sine_mimo::{
    self as host, Complex, Identification, MatrixStack, ProportionalTracking, SineMimoInputs,
    SineMimoOutputs, SineMimoSetup, SineShape, Tolerance,
};
use hertzforge_law_host::{
    LawError, LoadedSineMimoLaw, Param, ParamValue, SineMimoLawInstance, SineMimoOutputRule,
};
use hertzforge_mimo_sine::law::{LawDescriptor, LawValues, ParamSetting};
use hertzforge_mimo_sine::{
    Hold, LawFault, LawManifest, LawSetup, MimoShape, MimoSineLaw, MimoSineLawFactory, OutputRule,
    Request, TestType, TickInputs, TickOutputs, Tracking,
};
use hertzforge_sdm::C64;
use std::time::Duration;

/// The default budget of a law file's constructor, seconds. A law may fit a
/// model of the structure when it is built (`matrix` does), and in a law file
/// that fit runs about nine times slower than natively: `matrix` on a four by
/// four structure with 1441 identification lines took 4.6 s natively and 42 s
/// as a law file (measured on an 18-core Mac, `tests/build_time.rs`). The
/// fit's work bounds allow about 7.5 s natively, about 70 s as a law file, so
/// the budget is twice MIMO Shock's 60 s for a law's first solve.
pub const DEFAULT_BUILD_BUDGET_S: f64 = 120.0;

/// A law host error as the core's [`LawFault`]. A refused output keeps the
/// host's rule, which has the core's name.
pub fn sine_mimo_law_fault(e: &LawError) -> LawFault {
    match e {
        LawError::Deadline { budget } => LawFault::OverBudget {
            took_s: budget.as_secs_f64(),
            budget_s: budget.as_secs_f64(),
        },
        LawError::Memory { .. } => LawFault::OverMemory,
        LawError::Trap(s) | LawError::Poisoned(s) => LawFault::Crashed(s.clone()),
        LawError::BadSineMimoOutput { rule, .. } => LawFault::BadOutput(output_rule(*rule)),
        other => LawFault::Failed(other.to_string()),
    }
}

/// The host's output rule as the core's (one for one, same names).
pub fn output_rule(r: SineMimoOutputRule) -> OutputRule {
    match r {
        SineMimoOutputRule::WrongDriveCount => OutputRule::WrongDriveCount,
        SineMimoOutputRule::NonFinite => OutputRule::NonFinite,
        SineMimoOutputRule::BadRequest => OutputRule::BadRequest,
        SineMimoOutputRule::WrongReferenceCount => OutputRule::WrongReferenceCount,
        SineMimoOutputRule::TooManyValues => OutputRule::TooManyValues,
        SineMimoOutputRule::UndeclaredValue => OutputRule::UndeclaredValue,
    }
}

/// The core's shape as the law world's.
pub fn host_shape(s: MimoShape) -> SineShape {
    match s {
        MimoShape::Square => SineShape::Square,
        MimoShape::MoreControlsThanDrives => SineShape::MoreControlsThanDrives,
        MimoShape::FewerControlsThanDrives => SineShape::FewerControlsThanDrives,
    }
}

/// The law world's shape as the core's.
pub fn core_shape(s: SineShape) -> MimoShape {
    match s {
        SineShape::Square => MimoShape::Square,
        SineShape::MoreControlsThanDrives => MimoShape::MoreControlsThanDrives,
        SineShape::FewerControlsThanDrives => MimoShape::FewerControlsThanDrives,
    }
}

/// The core's test type as the law world's.
pub fn host_test_type(t: TestType) -> host::TestType {
    match t {
        TestType::Sweep => host::TestType::Sweep,
        TestType::Dwell => host::TestType::Dwell,
        TestType::Stepped => host::TestType::Stepped,
    }
}

/// The law world's test type as the core's.
pub fn core_test_type(t: host::TestType) -> TestType {
    match t {
        host::TestType::Sweep => TestType::Sweep,
        host::TestType::Dwell => TestType::Dwell,
        host::TestType::Stepped => TestType::Stepped,
    }
}

fn c(v: &C64) -> Complex {
    Complex { re: v.re, im: v.im }
}

fn stack(m: &hertzforge_sdm::MatrixStack, rows: usize, drives: usize) -> MatrixStack {
    let (lines, r, cols) = m.dims();
    if r != rows {
        return MatrixStack {
            lines: lines as u32,
            rows: rows as u32,
            cols: drives as u32,
            data: Vec::new(),
        };
    }
    MatrixStack {
        lines: lines as u32,
        rows: r as u32,
        cols: cols as u32,
        data: m.data().iter().map(c).collect(),
    }
}

/// The core's law setup as the law world carries it. A proportional tracking
/// rule's percentage is a share of the frequency there; an identification
/// level that was not measured is not a number.
pub fn host_setup(s: &LawSetup) -> SineMimoSetup {
    let id = &s.identification;
    SineMimoSetup {
        controls: s.controls as u32,
        drives: s.drives as u32,
        limits: s.limits as u32,
        fs_hz: s.fs_hz,
        tick_s: s.tick_s,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        test_type: host_test_type(s.test_type),
        shape: host_shape(s.shape),
        drive_ceiling_v: s.drive_ceiling_v.clone(),
        identification: Identification {
            frequency_hz: id.frequency_hz.clone(),
            response: stack(&id.response, s.controls, s.drives),
            limit_response: stack(&id.limit_response, s.limits, s.drives),
            coherence: id.coherence.clone(),
            limit_coherence: id.limit_coherence.clone(),
            trusted: id.trusted.clone(),
        },
        identification_level_db: s.identification_level_db.unwrap_or(f64::NAN),
        reference: s.reference.iter().map(c).collect(),
        phase_specified: s.phase_specified,
        limit_bounds: s.limit_bounds.clone(),
        tolerance: Tolerance {
            alarm_low_db: s.tolerance.alarm_low_db.clone(),
            alarm_high_db: s.tolerance.alarm_high_db.clone(),
            abort_low_db: s.tolerance.abort_low_db.clone(),
            abort_high_db: s.tolerance.abort_high_db.clone(),
        },
        tracking: match s.tracking {
            Tracking::Proportional { pct, max_hz } => {
                host::Tracking::Proportional(ProportionalTracking {
                    share: pct / 100.0,
                    max_hz,
                })
            }
            Tracking::Fixed { hz } => host::Tracking::Fixed(hz),
        },
    }
}

/// One update's core inputs as the law world carries them.
pub fn host_inputs(i: &TickInputs<'_>) -> SineMimoInputs {
    SineMimoInputs {
        index: i.index,
        dt_s: i.dt_s,
        frequency_hz: i.frequency_hz,
        sweep_hz_per_s: i.sweep_hz_per_s,
        sweep_oct_per_min: i.sweep_oct_per_min,
        level: i.level,
        reference: i.reference.iter().map(c).collect(),
        controls: i.controls.iter().map(c).collect(),
        limits: i.limits.iter().map(c).collect(),
        limit_bounds: i.limit_bounds.to_vec(),
        drive_in_force: i.drive_in_force.iter().map(c).collect(),
        hold: match i.hold {
            Hold::Free => host::Hold::Free,
            Hold::Notch => host::Hold::Notch,
            Hold::Held => host::Hold::Held,
            Hold::Probe => host::Hold::Probe,
            Hold::Engage => host::Hold::Engage,
        },
        notch_scale: i.notch_scale,
        settling: i.settling,
    }
}

/// A host parameter value as it is recorded.
pub fn setting(v: &ParamValue) -> Option<ParamSetting> {
    Some(match v {
        ParamValue::Number(x) => ParamSetting::Number(*x),
        ParamValue::Integer(x) => ParamSetting::Integer(*x),
        ParamValue::Toggle(x) => ParamSetting::Toggle(*x),
        ParamValue::Choice(x) => ParamSetting::Choice(x.clone()),
        ParamValue::Curve(_) | ParamValue::PerChannel(_) => return None,
    })
}

/// A MIMO Sine law file built for one run, as the core's [`MimoSineLaw`].
///
/// Every call carries the run's call budget as the host's deadline. A call
/// that traps, runs past its budget or breaches the memory cap leaves the
/// instance unusable; every later call reports that same first fault, so the
/// core's misses in a row run out and it takes its controlled stop.
pub struct WasmSineMimoLaw {
    instance: SineMimoLawInstance,
    budget: Duration,
    first_fault: Option<LawFault>,
    name: String,
    keys: Vec<String>,
}

impl WasmSineMimoLaw {
    /// The law `instance`, called with `budget` as every update's deadline.
    pub fn new(instance: SineMimoLawInstance, budget: Duration) -> Self {
        let name = instance.identity().name.clone();
        let keys = instance
            .manifest()
            .law
            .diagnostics
            .iter()
            .map(|d| d.key.clone())
            .collect();
        WasmSineMimoLaw {
            instance,
            budget,
            first_fault: None,
            name,
            keys,
        }
    }

    /// The contained instance (its guest output, its parameters).
    pub fn instance(&self) -> &SineMimoLawInstance {
        &self.instance
    }

    fn answer(
        &mut self,
        r: Result<SineMimoOutputs, LawError>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault> {
        let o = match r {
            Ok(o) => o,
            Err(e) => {
                let f = sine_mimo_law_fault(&e);
                if e.poisons() {
                    self.first_fault = Some(f.clone());
                }
                return Err(f);
            }
        };
        if o.drive.len() != out.drive.len() {
            return Err(LawFault::BadOutput(OutputRule::WrongDriveCount));
        }
        for (d, v) in out.drive.iter_mut().zip(&o.drive) {
            *d = C64::new(v.re, v.im);
        }
        out.faulted = o.faulted;
        out.request = o.request.map(|r| match r {
            host::Request::Hold => Request::Hold,
            host::Request::Slow(f) => Request::Slow(f),
        });
        out.notched = false;
        if let Some(r) = &o.notched_reference {
            if r.len() != out.notched_reference.len() {
                return Err(LawFault::BadOutput(OutputRule::WrongReferenceCount));
            }
            out.notched = true;
            for (d, v) in out.notched_reference.iter_mut().zip(r) {
                *d = C64::new(v.re, v.im);
            }
        }
        for v in &o.values {
            let i = self
                .keys
                .iter()
                .position(|k| *k == v.key)
                .ok_or(LawFault::BadOutput(OutputRule::UndeclaredValue))?;
            values
                .set(i, v.value)
                .map_err(|_| LawFault::BadOutput(OutputRule::UndeclaredValue))?;
        }
        Ok(())
    }
}

impl MimoSineLaw for WasmSineMimoLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn engage(
        &mut self,
        inputs: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let r = self.instance.engage(&host_inputs(inputs), self.budget);
        self.answer(r, out, values)
    }

    fn tick(
        &mut self,
        inputs: &TickInputs<'_>,
        out: &mut TickOutputs,
        values: &mut LawValues,
    ) -> Result<(), LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let r = self.instance.tick(&host_inputs(inputs), self.budget);
        self.answer(r, out, values)
    }
}

/// Builds a MIMO Sine law file for each run, with its parameter values, the
/// budget of every update's call and the longer budget of its constructor.
#[derive(Clone)]
pub struct WasmSineMimoLawFactory {
    law: LoadedSineMimoLaw,
    params: Vec<Param>,
    budget: Duration,
    build_budget: Duration,
}

impl WasmSineMimoLawFactory {
    /// The law file `law` with `params` (checked against its manifest when a
    /// run is built) and `budget` as the deadline of every update's call; the
    /// constructor has [`DEFAULT_BUILD_BUDGET_S`], or `budget` when that is
    /// longer.
    pub fn new(law: LoadedSineMimoLaw, params: Vec<Param>, budget: Duration) -> Self {
        WasmSineMimoLawFactory {
            law,
            params,
            build_budget: budget.max(Duration::from_secs_f64(DEFAULT_BUILD_BUDGET_S)),
            budget,
        }
    }

    /// The deadline of the constructor.
    pub fn with_build_budget(mut self, build_budget: Duration) -> Self {
        self.build_budget = build_budget;
        self
    }

    /// The loaded law file.
    pub fn law(&self) -> &LoadedSineMimoLaw {
        &self.law
    }
}

impl MimoSineLawFactory for WasmSineMimoLawFactory {
    fn build(&self, setup: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        let instance = self
            .law
            .instantiate(&host_setup(setup), &self.params, self.build_budget)
            .map_err(|e| sine_mimo_law_fault(&e))?;
        Ok(Box::new(WasmSineMimoLaw::new(instance, self.budget)))
    }

    fn describe(&self) -> LawDescriptor {
        let id = self.law.identity();
        let m = self.law.manifest();
        // Every declared parameter, the defaults filled in. A per-channel
        // parameter is not recorded here: its count is the run's control
        // count, known only when the law is built.
        let params = self
            .law
            .resolve_params(&self.params, 1)
            .unwrap_or_else(|_| self.params.clone());
        LawDescriptor {
            id: id.sha256.clone(),
            name: id.name.clone(),
            version: id.version.clone(),
            sha256: Some(id.sha256.clone()),
            params: params
                .iter()
                .filter_map(|p| setting(&p.value).map(|s| (p.key.clone(), s)))
                .collect(),
            diagnostics: m.law.diagnostics.iter().map(|d| d.key.clone()).collect(),
            live: Vec::new(),
        }
    }

    fn manifest(&self) -> LawManifest {
        let m = self.law.manifest();
        LawManifest {
            shapes: m.shapes.iter().map(|s| core_shape(*s)).collect(),
            test_types: m.test_types.iter().map(|t| core_test_type(*t)).collect(),
            amplitude_only: m.amplitude_only,
            values: m.law.diagnostics.iter().map(|d| d.key.clone()).collect(),
        }
    }
}
