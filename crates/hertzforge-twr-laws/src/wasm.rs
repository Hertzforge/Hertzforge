//! A TWR law file as the TWR core's law, and the factory the core builds a
//! run's law with.

use hertzforge_law_host::{
    ClassicalPlant as HostPlant, LawError, LoadedTwrLaw, Param, ParamValue, TwrInputs,
    TwrLawInstance, TwrOutputRule, TwrPlayKind, TwrSetup, TwrVerdict,
};
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawValue, OutputRule};
use hertzforge_twr::{PlayInputs, PlayKind, PlayOutputs, TwrLaw, TwrLawFactory, TwrLawSetup};
use std::time::Duration;

/// A law host error as the TWR core's [`LawFault`].
pub fn twr_law_fault(e: &LawError) -> LawFault {
    match e {
        LawError::Deadline { budget } => LawFault::OverBudget {
            took_s: budget.as_secs_f64(),
            budget_s: budget.as_secs_f64(),
        },
        LawError::Memory { .. } => LawFault::OverMemory,
        LawError::Trap(s) | LawError::Poisoned(s) => LawFault::Crashed(s.clone()),
        LawError::BadTwrOutput { rule, .. } => LawFault::BadOutput(match rule {
            TwrOutputRule::WrongLength => OutputRule::WrongLength,
            TwrOutputRule::NonFinite => OutputRule::NonFinite,
            TwrOutputRule::TooManyValues => OutputRule::TooManyValues,
            TwrOutputRule::UndeclaredValue => OutputRule::UndeclaredValue,
        }),
        other => LawFault::Failed(other.to_string()),
    }
}

/// A TWR law file built for one run, as the core's [`TwrLaw`].
///
/// Every call carries the run's call budget as the host's deadline, so the host
/// stops a call that will not return when the core would stop waiting for it.
/// A call that traps, runs past its budget or breaches the memory cap leaves
/// the instance unusable; every later call reports that same first fault, so
/// the core's misses in a row run out and it takes its controlled stop.
pub struct WasmTwrLaw {
    instance: TwrLawInstance,
    budget: Duration,
    first_fault: Option<LawFault>,
    name: String,
}

impl WasmTwrLaw {
    /// The law `instance`, called with `budget` as every call's deadline.
    pub fn new(instance: TwrLawInstance, budget: Duration) -> Self {
        let name = instance.identity().name.clone();
        WasmTwrLaw {
            instance,
            budget,
            first_fault: None,
            name,
        }
    }

    /// The contained instance (its guest output, its parameters).
    pub fn instance(&self) -> &TwrLawInstance {
        &self.instance
    }

    fn call(&mut self, seed: bool, i: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let inputs = TwrInputs {
            play_index: i.play_index,
            kind: match i.kind {
                PlayKind::Iteration => TwrPlayKind::Iteration,
                PlayKind::Playback => TwrPlayKind::Playback,
            },
            response: i.response,
            drive: i.drive,
            judged: i.judged.map(|v| TwrVerdict {
                gated: v.gated,
                rms_error_pct: v.rms_error_pct,
                peak_error_pct: v.peak_error_pct,
                spectral_error_db: v.spectral_error_db,
            }),
            limited: i.limited,
        };
        let r = if seed {
            self.instance.seed(&inputs, self.budget)
        } else {
            self.instance.iterate(&inputs, self.budget)
        };
        match r {
            Ok(o) => Ok(PlayOutputs {
                drive: o.drive,
                values: o
                    .values
                    .into_iter()
                    .map(|v| LawValue::new(v.key, v.value))
                    .collect(),
            }),
            Err(e) => {
                let f = twr_law_fault(&e);
                if e.poisons() {
                    self.first_fault = Some(f.clone());
                }
                Err(f)
            }
        }
    }
}

impl TwrLaw for WasmTwrLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn seed(&mut self, inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault> {
        self.call(true, inputs)
    }

    fn iterate(&mut self, inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault> {
        self.call(false, inputs)
    }
}

/// Builds a TWR law file for each run, with its parameter values.
#[derive(Clone)]
pub struct WasmTwrLawFactory {
    law: LoadedTwrLaw,
    params: Vec<Param>,
}

impl WasmTwrLawFactory {
    /// The law file `law` with `params` (already resolved against its
    /// manifest).
    pub fn new(law: LoadedTwrLaw, params: Vec<Param>) -> Self {
        WasmTwrLawFactory { law, params }
    }
}

/// Parameter values as the run record carries them.
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

impl TwrLawFactory for WasmTwrLawFactory {
    fn build(&self, s: &TwrLawSetup<'_>) -> Result<Box<dyn TwrLaw + Send>, LawFault> {
        let setup = TwrSetup {
            fs_hz: s.fs_hz,
            record_len: s.record_len,
            band_hz: (s.band_lo_hz, s.band_hi_hz),
            reference: s.reference,
            section_start: s.section_start,
            section_len: s.section_len,
            plant: HostPlant {
                frf: s.plant.frf,
                coherence: s.plant.coherence,
                inverse: s.plant.inverse,
                trusted: s.plant.trusted,
                epsilon: s.plant.epsilon,
            },
            drive_ceiling: s.drive_ceiling_v,
        };
        // The host's deadline is the core's budget.
        let budget = Duration::from_secs_f64(s.call_budget_s.max(0.0));
        let instance = self
            .law
            .instantiate(&setup, &self.params, budget)
            .map_err(|e| twr_law_fault(&e))?;
        Ok(Box::new(WasmTwrLaw::new(instance, budget)))
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
