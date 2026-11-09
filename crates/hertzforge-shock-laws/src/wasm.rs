//! A classical law file as the classical core's law, and the factories the
//! core builds a run's law with.

use hertzforge_law_host::{
    ClassicalInputs, ClassicalLawInstance, ClassicalOutputRule, ClassicalPlant as HostPlant,
    ClassicalSetup, ClassicalVerdict, LawError, LoadedClassicalLaw, Param, ParamValue,
};
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawValue, OutputRule};
use hertzforge_shock_host::{
    ClassicalLaw, ClassicalLawFactory, ClassicalLawSetup, PulseInputs, PulseOutputs,
};
use std::time::Duration;

/// A law host error as the classical core's [`LawFault`].
pub fn classical_law_fault(e: &LawError) -> LawFault {
    match e {
        LawError::Deadline { budget } => LawFault::OverBudget {
            took_s: budget.as_secs_f64(),
            budget_s: budget.as_secs_f64(),
        },
        LawError::Memory { .. } => LawFault::OverMemory,
        LawError::Trap(s) | LawError::Poisoned(s) => LawFault::Crashed(s.clone()),
        LawError::BadClassicalOutput { rule, .. } => LawFault::BadOutput(match rule {
            ClassicalOutputRule::WrongLength => OutputRule::WrongLength,
            ClassicalOutputRule::NonFinite => OutputRule::NonFinite,
            ClassicalOutputRule::TooManyValues => OutputRule::TooManyValues,
            ClassicalOutputRule::UndeclaredValue => OutputRule::UndeclaredValue,
        }),
        other => LawFault::Failed(other.to_string()),
    }
}

/// A classical law file built for one run, as the core's [`ClassicalLaw`].
///
/// Every call carries the run's call budget as the host's deadline, so the host
/// stops a call that will not return when the core would stop waiting for it.
/// A call that traps, runs past its budget or breaches the memory cap leaves
/// the instance unusable; every later call reports that same first fault, so
/// the core's misses in a row run out and it takes its controlled stop.
pub struct WasmClassicalLaw {
    instance: ClassicalLawInstance,
    budget: Duration,
    first_fault: Option<LawFault>,
    name: String,
}

impl WasmClassicalLaw {
    /// The law `instance`, called with `budget` as every call's deadline.
    pub fn new(instance: ClassicalLawInstance, budget: Duration) -> Self {
        let name = instance.identity().name.clone();
        WasmClassicalLaw {
            instance,
            budget,
            first_fault: None,
            name,
        }
    }

    /// The contained instance (its guest output, its parameters).
    pub fn instance(&self) -> &ClassicalLawInstance {
        &self.instance
    }

    fn call(&mut self, seed: bool, i: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let inputs = ClassicalInputs {
            pulse_index: i.pulse_index,
            response: i.response,
            drive: i.drive,
            judged: i.judged.map(|v| ClassicalVerdict {
                passed: v.passed,
                error_pct: v.error_pct,
                peak_ratio: v.peak_ratio,
                velocity_change_ratio: v.velocity_change_ratio,
            }),
            limited: i.limited,
        };
        let r = if seed {
            self.instance.seed(&inputs, self.budget)
        } else {
            self.instance.iterate(&inputs, self.budget)
        };
        match r {
            Ok(o) => Ok(PulseOutputs {
                drive: o.drive,
                values: o
                    .values
                    .into_iter()
                    .map(|v| LawValue::new(v.key, v.value))
                    .collect(),
            }),
            Err(e) => {
                let f = classical_law_fault(&e);
                if e.poisons() {
                    self.first_fault = Some(f.clone());
                }
                Err(f)
            }
        }
    }
}

impl ClassicalLaw for WasmClassicalLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn seed(&mut self, inputs: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault> {
        self.call(true, inputs)
    }

    fn iterate(&mut self, inputs: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault> {
        self.call(false, inputs)
    }
}

/// Builds a classical law file for each run, with its parameter values.
#[derive(Clone)]
pub struct WasmClassicalLawFactory {
    law: LoadedClassicalLaw,
    params: Vec<Param>,
}

impl WasmClassicalLawFactory {
    /// The law file `law` with `params` (already resolved against its
    /// manifest).
    pub fn new(law: LoadedClassicalLaw, params: Vec<Param>) -> Self {
        WasmClassicalLawFactory { law, params }
    }
}

pub(crate) fn params_json(params: &[Param]) -> serde_json::Value {
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

impl ClassicalLawFactory for WasmClassicalLawFactory {
    fn build(&self, s: &ClassicalLawSetup<'_>) -> Result<Box<dyn ClassicalLaw + Send>, LawFault> {
        let setup = ClassicalSetup {
            fs_hz: s.fs_hz,
            record_len: s.record_len,
            band_hz: (s.band_lo_hz, s.band_hi_hz),
            reference: s.reference,
            main_start: s.main_start,
            main_len: s.main_len,
            plant: HostPlant {
                frf: s.plant.frf,
                coherence: s.plant.coherence,
                inverse: s.plant.inverse,
                trusted: s.plant.trusted,
                epsilon: s.plant.epsilon,
            },
            drive_ceiling: s.drive_ceiling_v,
        };
        let budget = Duration::from_secs_f64(s.call_budget_s.max(0.0));
        let instance = self
            .law
            .instantiate(&setup, &self.params, budget)
            .map_err(|e| classical_law_fault(&e))?;
        Ok(Box::new(WasmClassicalLaw::new(instance, budget)))
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
}
