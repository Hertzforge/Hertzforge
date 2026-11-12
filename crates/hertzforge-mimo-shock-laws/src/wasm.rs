//! A MIMO Shock law file as the core's law: [`WasmMimoShockLaw`] implements
//! [`MimoShockLaw`] over the law host's contained instance, and
//! [`WasmMimoShockLawFactory`] builds one for each run.

use hertzforge_law_host::shock_mimo::{
    self as host, ComponentShape, ControlVerdict, HoldTiming, MimoShockInputs, MimoShockSetup,
    SrsComponent, SrsTarget, Target, TargetKind as HostKind, Timing, WaveformTarget,
};
use hertzforge_law_host::{
    LawError, LoadedShockMimoLaw, Param, ParamValue, ShockMimoLawInstance, ShockMimoOutputRule,
};
use hertzforge_mimo_shock::{
    self as core, LawManifest, MimoShockLaw, MimoShockLawFactory, PulseInputs, PulseOutputs, Setup,
    TargetKind,
};
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawValue, OutputRule};
use std::time::Duration;

/// A law host error as the core's [`LawFault`]. A refused output names its
/// rule; the shared fault has no drive-count or delays rule, so a wrong drive
/// count and delays that are not one per control are its `wrong_length` (the
/// host's own rule, `wrong_drive_count` or `delays_shape`, is in the error's
/// text).
pub fn shock_mimo_law_fault(e: &LawError) -> LawFault {
    match e {
        LawError::Deadline { budget } => LawFault::OverBudget {
            took_s: budget.as_secs_f64(),
            budget_s: budget.as_secs_f64(),
        },
        LawError::Memory { .. } => LawFault::OverMemory,
        LawError::Trap(s) | LawError::Poisoned(s) => LawFault::Crashed(s.clone()),
        LawError::BadShockMimoOutput { rule, .. } => LawFault::BadOutput(match rule {
            ShockMimoOutputRule::WrongDriveCount
            | ShockMimoOutputRule::WrongLength
            | ShockMimoOutputRule::DelaysShape => OutputRule::WrongLength,
            ShockMimoOutputRule::NonFinite => OutputRule::NonFinite,
            ShockMimoOutputRule::TooManyValues => OutputRule::TooManyValues,
            ShockMimoOutputRule::UndeclaredValue => OutputRule::UndeclaredValue,
        }),
        other => LawFault::Failed(other.to_string()),
    }
}

/// The host's target kind as the core's.
pub fn core_kind(k: HostKind) -> TargetKind {
    match k {
        HostKind::Waveform => TargetKind::Waveform,
        HostKind::Srs => TargetKind::Srs,
    }
}

/// The core's setup as the law world carries it.
pub fn host_setup(s: &Setup) -> MimoShockSetup {
    let f = &s.plant.frf;
    MimoShockSetup {
        fs_hz: s.fs_hz,
        record_len: s.record_len as u32,
        drives: s.n_drives as u32,
        controls: s.n_controls as u32,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        target: match &s.target {
            core::Target::Waveform(w) => Target::Waveform(WaveformTarget {
                records: w.records.clone(),
            }),
            core::Target::Srs(t) => Target::Srs(SrsTarget {
                natural_hz: t.natural_hz.clone(),
                q: t.q,
                points: t.points.clone(),
                tol_lo_db: t.tol_lo_db,
                tol_hi_db: t.tol_hi_db,
                waveforms: t.waveforms.clone(),
                components: t
                    .components
                    .iter()
                    .map(|cs| {
                        cs.iter()
                            .map(|c| SrsComponent {
                                hz: c.hz,
                                amplitude: c.amplitude,
                                delay_s: c.delay_s,
                                shape: match c.shape {
                                    core::ComponentShape::Decay(z) => ComponentShape::Decay(z),
                                    core::ComponentShape::HalfCycles(n) => {
                                        ComponentShape::HalfCycles(n)
                                    }
                                },
                            })
                            .collect()
                    })
                    .collect(),
            }),
        },
        timing: match &s.timing {
            core::Timing::Free { window_s } => Timing::Free(*window_s),
            core::Timing::Hold(h) => Timing::Hold(HoldTiming {
                delays_s: h.delays_s.clone(),
                window_s: h.window_s,
            }),
        },
        plant: host::Plant {
            frf: host::MatrixStack {
                lines: f.lines() as u32,
                rows: f.rows() as u32,
                cols: f.cols() as u32,
                data: f
                    .data()
                    .iter()
                    .map(|c| host::Complex { re: c.re, im: c.im })
                    .collect(),
            },
            coherence: s.plant.coherence.clone(),
            trusted: s.plant.trusted.clone(),
        },
        drive_ceiling_v: s.drive_ceiling_v.clone(),
        ladder_db: s.ladder_db.clone(),
    }
}

/// The core's pulse inputs as the law world carries them.
pub fn host_inputs(i: &PulseInputs) -> MimoShockInputs {
    MimoShockInputs {
        pulse_index: i.pulse_index,
        level_db: i.level_db,
        next_level_db: i.next_level_db,
        drives: i.drives.clone(),
        responses: i.responses.clone(),
        srs: i.srs.clone(),
        verdicts: i
            .verdicts
            .iter()
            .map(|v| ControlVerdict {
                passed: v.passed,
                error_db: v.error_db,
                delay_s: v.delay_s,
                peak_ratio: v.peak_ratio,
            })
            .collect(),
        limited: i.limited,
    }
}

/// A MIMO Shock law file built for one run, as the core's [`MimoShockLaw`].
///
/// Every call carries the run's call budget as the host's deadline. A call
/// that traps, runs past its budget or breaches the memory cap leaves the
/// instance unusable; every later call reports that same first fault, so the
/// core's misses in a row run out and it takes its controlled stop.
pub struct WasmMimoShockLaw {
    instance: ShockMimoLawInstance,
    budget: Duration,
    first_fault: Option<LawFault>,
    name: String,
}

impl WasmMimoShockLaw {
    /// The law `instance`, called with `budget` as every call's deadline.
    pub fn new(instance: ShockMimoLawInstance, budget: Duration) -> Self {
        let name = instance.identity().name.clone();
        WasmMimoShockLaw {
            instance,
            budget,
            first_fault: None,
            name,
        }
    }

    /// The contained instance (its guest output, its parameters).
    pub fn instance(&self) -> &ShockMimoLawInstance {
        &self.instance
    }

    fn answer(
        &mut self,
        r: Result<hertzforge_law_host::shock_mimo::MimoShockOutputs, LawError>,
    ) -> Result<PulseOutputs, LawFault> {
        match r {
            Ok(o) => Ok(PulseOutputs {
                drives: o.drives,
                delays_s: o.delays_s,
                values: o
                    .values
                    .into_iter()
                    .map(|v| LawValue::new(v.key, v.value))
                    .collect(),
            }),
            Err(e) => {
                let f = shock_mimo_law_fault(&e);
                if e.poisons() {
                    self.first_fault = Some(f.clone());
                }
                Err(f)
            }
        }
    }
}

impl MimoShockLaw for WasmMimoShockLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let r = self.instance.seed(level_db, self.budget);
        self.answer(r)
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let r = self.instance.iterate(&host_inputs(inputs), self.budget);
        self.answer(r)
    }
}

/// Builds a MIMO Shock law file for each run, with its parameter values and
/// the call budget every call is held to.
#[derive(Clone)]
pub struct WasmMimoShockLawFactory {
    law: LoadedShockMimoLaw,
    params: Vec<Param>,
    budget: Duration,
}

impl WasmMimoShockLawFactory {
    /// The law file `law` with `params` (checked against its manifest when a
    /// run is built) and `budget` as the deadline of every call.
    pub fn new(law: LoadedShockMimoLaw, params: Vec<Param>, budget: Duration) -> Self {
        WasmMimoShockLawFactory {
            law,
            params,
            budget,
        }
    }

    /// The loaded law file.
    pub fn law(&self) -> &LoadedShockMimoLaw {
        &self.law
    }
}

/// Parameter values as a JSON object by key, for the run record.
pub fn params_json(params: &[Param]) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for p in params {
        let v = match &p.value {
            ParamValue::Number(x) => serde_json::json!(x),
            ParamValue::Integer(x) => serde_json::json!(x),
            ParamValue::Toggle(x) => serde_json::json!(x),
            ParamValue::Choice(x) => serde_json::json!(x),
            ParamValue::PerChannel(x) => serde_json::json!(x),
            ParamValue::Curve(c) => serde_json::json!(c
                .iter()
                .map(|b| [b.frequency_hz, b.value])
                .collect::<Vec<_>>()),
        };
        m.insert(p.key.clone(), v);
    }
    serde_json::Value::Object(m)
}

impl MimoShockLawFactory for WasmMimoShockLawFactory {
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        let instance = self
            .law
            .instantiate(&host_setup(setup), &self.params, self.budget)
            .map_err(|e| shock_mimo_law_fault(&e))?;
        Ok(Box::new(WasmMimoShockLaw::new(instance, self.budget)))
    }

    fn describe(&self) -> LawDescriptor {
        let id = self.law.identity();
        LawDescriptor {
            id: id.sha256.clone(),
            name: id.name.clone(),
            version: id.version.clone(),
            sha256: Some(id.sha256.clone()),
            params: params_json(&self.params),
        }
    }

    fn manifest(&self) -> LawManifest {
        let m = self.law.manifest();
        LawManifest {
            targets: m.targets.iter().map(|k| core_kind(*k)).collect(),
            values: m.law.diagnostics.iter().map(|d| d.key.clone()).collect(),
        }
    }
}
