//! An SRS law file as the SRS core's law, and the factory the core builds a
//! run's law with.

use crate::wasm::params_json;
use hertzforge_law_host::{
    LawError, LoadedSrsLaw, Param, SrsInputs as HostInputs, SrsLawInstance, SrsOutputRule, SrsSetup,
};
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawValue, OutputRule};
use hertzforge_srs::{SrsInputs, SrsLaw, SrsLawFactory, SrsLawSetup, SrsOutputs};
use std::time::Duration;

/// The longest host deadline a call is given when the core sets no budget (a
/// run without `control`): an hour.
const NO_BUDGET: Duration = Duration::from_secs(3600);

/// A law host error as the SRS core's [`LawFault`].
pub fn srs_law_fault(e: &LawError) -> LawFault {
    match e {
        LawError::Deadline { budget } => LawFault::OverBudget {
            took_s: budget.as_secs_f64(),
            budget_s: budget.as_secs_f64(),
        },
        LawError::Memory { .. } => LawFault::OverMemory,
        LawError::Trap(s) | LawError::Poisoned(s) => LawFault::Crashed(s.clone()),
        LawError::BadSrsOutput { rule, .. } => LawFault::BadOutput(match rule {
            SrsOutputRule::WrongLength => OutputRule::WrongLength,
            SrsOutputRule::NonFinite => OutputRule::NonFinite,
            SrsOutputRule::NotPositive => OutputRule::NotPositive,
            SrsOutputRule::TooManyValues => OutputRule::TooManyValues,
            SrsOutputRule::UndeclaredValue => OutputRule::UndeclaredValue,
        }),
        other => LawFault::Failed(other.to_string()),
    }
}

/// An SRS law file built for one run, as the core's [`SrsLaw`].
///
/// Every call carries the run's call budget as the host's deadline, so the host
/// stops a call that will not return when the core would stop waiting for it.
/// A call that traps, runs past its budget or breaches the memory cap leaves
/// the instance unusable; every later call reports that same first fault, so
/// the core's misses in a row run out and it takes its controlled stop.
pub struct WasmSrsLaw {
    instance: SrsLawInstance,
    budget: Duration,
    first_fault: Option<LawFault>,
    name: String,
}

impl WasmSrsLaw {
    /// The law `instance`, called with `budget` as every call's deadline.
    pub fn new(instance: SrsLawInstance, budget: Duration) -> Self {
        let name = instance.identity().name.clone();
        WasmSrsLaw {
            instance,
            budget,
            first_fault: None,
            name,
        }
    }

    /// The contained instance (its guest output, its parameters).
    pub fn instance(&self) -> &SrsLawInstance {
        &self.instance
    }

    fn call(&mut self, seed: bool, i: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let inputs = HostInputs {
            pulse_index: i.pulse_index,
            target: i.target,
            measured: i.measured,
            commanded_scale: i.commanded_scale,
        };
        let r = if seed {
            self.instance.seed(&inputs, self.budget)
        } else {
            self.instance.iterate(&inputs, self.budget)
        };
        match r {
            Ok(o) => Ok(SrsOutputs {
                gains: o.gains,
                values: o
                    .values
                    .into_iter()
                    .map(|v| LawValue::new(v.key, v.value))
                    .collect(),
            }),
            Err(e) => {
                let f = srs_law_fault(&e);
                if e.poisons() {
                    self.first_fault = Some(f.clone());
                }
                Err(f)
            }
        }
    }
}

impl SrsLaw for WasmSrsLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn seed(&mut self, inputs: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault> {
        self.call(true, inputs)
    }

    fn iterate(&mut self, inputs: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault> {
        self.call(false, inputs)
    }
}

/// Builds an SRS law file for each run, with its parameter values.
#[derive(Clone)]
pub struct WasmSrsLawFactory {
    law: LoadedSrsLaw,
    params: Vec<Param>,
}

impl WasmSrsLawFactory {
    /// The law file `law` with `params` (already resolved against its
    /// manifest).
    pub fn new(law: LoadedSrsLaw, params: Vec<Param>) -> Self {
        WasmSrsLawFactory { law, params }
    }
}

impl SrsLawFactory for WasmSrsLawFactory {
    fn build(&self, s: &SrsLawSetup<'_>) -> Result<Box<dyn SrsLaw + Send>, LawFault> {
        let setup = SrsSetup {
            band_hz: s.band_hz,
            component_hz: s.component_hz,
            target: s.target,
            q: s.q,
            component_gain: s.component_gain,
        };
        // The host deadline is the core's budget.
        let budget = Duration::try_from_secs_f64(s.call_budget_s.max(0.0)).unwrap_or(NO_BUDGET);
        let instance = self
            .law
            .instantiate(&setup, &self.params, budget)
            .map_err(|e| srs_law_fault(&e))?;
        Ok(Box::new(WasmSrsLaw::new(instance, budget)))
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
