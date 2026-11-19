//! A Sine law file as the Sine core's law, and the factory the core builds a
//! run's law with.

use hertzforge_law_host::{
    LawError, LoadedSineLaw, Param, ParamKind, ParamValue, SineHold, SineIdentification,
    SineInputs, SineLawInstance, SineOutputRule as HostRule, SineSetup,
};
use hertzforge_sine::law::{
    Hold, LawDescriptor, LawFault, LawValues, LiveParam, ParamSetting, SineControlLaw,
    SineLawFactory, SineLawSetup, SineOutputRule, TickInputs, TickOutputs,
};
use std::time::Duration;

/// The host deadline a call is given when the core sets no budget (a run
/// without the guards): a second.
const NO_BUDGET: Duration = Duration::from_secs(1);

/// A law host error as the Sine core's [`LawFault`].
pub fn sine_law_fault(e: &LawError) -> LawFault {
    match e {
        LawError::Deadline { budget } => LawFault::OverBudget {
            took_s: budget.as_secs_f64(),
            budget_s: budget.as_secs_f64(),
        },
        LawError::Memory { .. } => LawFault::OverMemory,
        LawError::Trap(s) | LawError::Poisoned(s) => LawFault::Crashed(s.clone()),
        LawError::BadSineOutput { rule, .. } => LawFault::BadOutput(match rule {
            HostRule::NonFinite => SineOutputRule::NonFinite,
            HostRule::Negative => SineOutputRule::Negative,
            HostRule::TooManyValues => SineOutputRule::TooManyValues,
            HostRule::UndeclaredValue => SineOutputRule::UndeclaredValue,
        }),
        other => LawFault::Failed(other.to_string()),
    }
}

/// A parameter value in the run records' form.
pub fn setting(v: &ParamValue) -> Option<ParamSetting> {
    Some(match v {
        ParamValue::Number(x) => ParamSetting::Number(*x),
        ParamValue::Integer(i) => ParamSetting::Integer(*i),
        ParamValue::Toggle(b) => ParamSetting::Toggle(*b),
        ParamValue::Choice(c) => ParamSetting::Choice(c.clone()),
        ParamValue::Curve(_) | ParamValue::PerChannel(_) => return None,
    })
}

/// A Sine law file built for one run, as the core's [`SineControlLaw`].
///
/// Every call carries the run's call budget as the host's deadline, so the
/// host stops a call that will not return within about one epoch tick of the
/// time the core stops waiting for it. A call that traps, runs past its budget
/// or breaches the memory cap leaves the instance unusable; every later call
/// reports that same first fault, so the core's misses in a row run out and it
/// takes its controlled stop.
pub struct WasmSineLaw {
    instance: SineLawInstance,
    budget: Duration,
    first_fault: Option<LawFault>,
    keys: Vec<String>,
    name: String,
}

impl WasmSineLaw {
    /// The law `instance`, called with `budget` as every call's deadline.
    pub fn new(instance: SineLawInstance, budget: Duration) -> Self {
        let name = instance.identity().name.clone();
        let keys = instance
            .manifest()
            .diagnostics
            .iter()
            .map(|d| d.key.clone())
            .collect();
        WasmSineLaw {
            instance,
            budget,
            first_fault: None,
            keys,
            name,
        }
    }

    /// The contained instance (its guest output, its parameters).
    pub fn instance(&self) -> &SineLawInstance {
        &self.instance
    }

    fn call(
        &mut self,
        engage: bool,
        i: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let inputs = SineInputs {
            tick_index: i.tick_index,
            dt_s: i.dt_s,
            frequency_hz: i.frequency_hz,
            sweep_rate_hz_per_s: i.sweep_rate_hz_per_s,
            level_db: i.level_db,
            target: i.target,
            measured: i.measured,
            channel_amplitude: i.channel_amplitude,
            channel_phase_deg: i.channel_phase_deg,
            drive: i.drive,
            prediction_db: i.prediction_db,
            hold: match i.hold {
                Hold::Free => SineHold::Free,
                Hold::Notch => SineHold::Notch,
                Hold::Probe => SineHold::Probe,
            },
            notch_ceiling: i.notch_ceiling,
            settling: i.settling,
        };
        let r = if engage {
            self.instance.engage(&inputs, self.budget)
        } else {
            self.instance.tick(&inputs, self.budget)
        };
        match r {
            Ok(o) => {
                for v in &o.values {
                    let Some(ix) = self.keys.iter().position(|k| *k == v.key) else {
                        return Err(LawFault::BadOutput(SineOutputRule::UndeclaredValue));
                    };
                    values.set(ix, v.value).map_err(LawFault::BadOutput)?;
                }
                Ok(TickOutputs {
                    drive: o.drive,
                    faulted: o.faulted,
                })
            }
            Err(e) => {
                let f = sine_law_fault(&e);
                if e.poisons() {
                    self.first_fault = Some(f.clone());
                }
                Err(f)
            }
        }
    }
}

impl SineControlLaw for WasmSineLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn engage(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault> {
        self.call(true, inputs, values)
    }

    fn tick(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault> {
        self.call(false, inputs, values)
    }

    fn adjust(&mut self, key: &str, value: f64) -> Result<(), LawFault> {
        if let Some(f) = &self.first_fault {
            return Err(f.clone());
        }
        let r = self
            .instance
            .adjust(&Param::new(key, ParamValue::Number(value)), self.budget);
        r.map_err(|e| {
            let f = sine_law_fault(&e);
            if e.poisons() {
                self.first_fault = Some(f.clone());
            }
            f
        })
    }
}

/// Builds a Sine law file for each run, with its parameter values.
#[derive(Clone)]
pub struct WasmSineLawFactory {
    law: LoadedSineLaw,
    params: Vec<Param>,
}

impl WasmSineLawFactory {
    /// The law file `law` with `params` (already resolved against its
    /// manifest).
    pub fn new(law: LoadedSineLaw, params: Vec<Param>) -> Self {
        WasmSineLawFactory { law, params }
    }

    /// The law file.
    pub fn law(&self) -> &LoadedSineLaw {
        &self.law
    }
}

impl SineLawFactory for WasmSineLawFactory {
    fn build(&self, s: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        let setup = SineSetup {
            control_channels: s.control_channels,
            tick_s: s.tick_s,
            band_hz: s.band_hz,
            drive_ceiling: s.drive_ceiling,
            identification: SineIdentification {
                frequency_hz: s.identification.frequency_hz,
                gain: s.identification.gain,
                coherence: s.identification.coherence,
                trusted: s.identification.trusted,
                target: s.identification.target,
            },
        };
        // The host deadline is the core's budget for one call; the build itself
        // gets the host's describe budget.
        let budget = if s.call_budget_s.is_finite() && s.call_budget_s > 0.0 {
            Duration::from_secs_f64(s.call_budget_s)
        } else {
            NO_BUDGET
        };
        let instance = self
            .law
            .instantiate(&setup, &self.params, Duration::from_secs(1))
            .map_err(|e| sine_law_fault(&e))?;
        Ok(Box::new(WasmSineLaw::new(instance, budget)))
    }

    fn describe(&self) -> LawDescriptor {
        let m = self.law.manifest();
        let id = self.law.identity();
        LawDescriptor {
            id: id.sha256.clone(),
            name: id.name.clone(),
            version: id.version.clone(),
            sha256: Some(id.sha256.clone()),
            params: self
                .params
                .iter()
                .filter_map(|p| setting(&p.value).map(|v| (p.key.clone(), v)))
                .collect(),
            diagnostics: m.diagnostics.iter().map(|d| d.key.clone()).collect(),
            live: self
                .law
                .live()
                .iter()
                .filter_map(|key| {
                    m.params
                        .iter()
                        .find(|p| &p.key == key)
                        .and_then(|p| match &p.kind {
                            ParamKind::Number(n) => Some(LiveParam {
                                key: key.clone(),
                                min: n.min,
                                max: n.max,
                            }),
                            _ => None,
                        })
                })
                .collect(),
        }
    }
}
