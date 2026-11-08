//! A law file run as a Random control law: [`WasmControlLaw`] implements the run
//! core's [`ControlLaw`] over a [`LawInstance`], and [`WasmLawFactory`] builds
//! one per run.
//!
//! The law host contains the law (a deadline on every call, a memory cap, no
//! system access) and checks what it returns; the run core then guards the
//! output exactly as it guards a native law's. This module only carries values
//! across and says how the host's errors read as the core's [`LawFault`]s.

use hertzforge_law_host::{
    LawError, LawInstance, LoadedLaw, LoopInputs as HostInputs, LoopOutputs as HostOutputs,
    OutputRule, Param, Setup,
};
use hertzforge_random::control::{
    ControlError, ControlLaw, LawFault, LevelCommand, LoopDiagnostics, LoopInputs, LoopOutputs,
    OutputFault,
};
use hertzforge_random::service::law::{LawFactory, LawSetup};
use std::time::Duration;

/// How a law host error reads as a run-core law fault.
///
/// A trap, a missed deadline and a breached memory cap become
/// [`LawFault::Crashed`], [`LawFault::OverBudget`] and [`LawFault::OverMemory`];
/// an output the host refused becomes [`LawFault::BadOutput`] naming the rule,
/// which the core records exactly as it records the same fault in a native
/// law's output. The law's own error is [`LawFault::Failed`] with its message;
/// inputs the host refused are [`LawFault::BadInputs`]. Anything else (the host
/// itself failing) is [`LawFault::Failed`].
///
/// [`LawError::Poisoned`] is mapped here to [`LawFault::Crashed`] for want of
/// the first fault; [`WasmControlLaw`] remembers the first fault and reports it
/// again instead, so a run stopped by a law that trapped is recorded as a trap.
pub fn law_fault(e: &LawError) -> LawFault {
    match e {
        LawError::Law(msg) => LawFault::Failed(msg.clone()),
        LawError::InputsInvalid(msg) => LawFault::BadInputs(msg.clone()),
        LawError::Trap(msg) => LawFault::Crashed(msg.clone()),
        LawError::Deadline { budget } => LawFault::OverBudget(format!(
            "stopped at its {:.1} ms budget",
            budget.as_secs_f64() * 1e3
        )),
        LawError::Memory {
            limit_bytes,
            requested_bytes,
        } => LawFault::OverMemory(format!(
            "asked for {requested_bytes} bytes against a cap of {limit_bytes}"
        )),
        LawError::BadOutput { rule, detail } => {
            LawFault::BadOutput(output_fault(*rule), detail.clone())
        }
        LawError::Poisoned(first) => LawFault::Crashed(first.clone()),
        // A refused output of another world's law: a single-output law never
        // returns one.
        LawError::BadMimoOutput { .. }
        | LawError::BadClassicalOutput { .. }
        | LawError::BadSrsOutput { .. }
        | LawError::BadSineOutput { .. }
        | LawError::Compile(_)
        | LawError::NotALaw(_)
        | LawError::ManifestInvalid(_)
        | LawError::ParamInvalid { .. }
        | LawError::SetupInvalid(_)
        | LawError::Host(_) => LawFault::Failed(e.to_string()),
    }
}

fn output_fault(rule: OutputRule) -> OutputFault {
    match rule {
        OutputRule::WrongLength => OutputFault::WrongLength,
        OutputRule::NonFiniteDrive => OutputFault::NonFinite,
        OutputRule::NegativeDrive => OutputFault::Negative,
        OutputRule::NonFiniteLevel => OutputFault::NonFiniteLevel,
        OutputRule::Diagnostics => OutputFault::Diagnostics,
    }
}

/// The diagnostics the run core reads. The law's declared named values are
/// shown and recorded by later work; the core does not use them.
fn core_outputs(o: HostOutputs) -> LoopOutputs {
    let d = o.diagnostics;
    LoopOutputs {
        drive_psd: o.drive_psd,
        level_cmd_db: o.level_cmd_db,
        diagnostics: LoopDiagnostics {
            untrusted_bins: d.untrusted_bins,
            clamped_bins: d.clamped_bins,
            divergent_bins: d.divergent_bins,
            inband_err_db: d.inband_err_db,
            inband_rms_g: d.inband_rms_g,
            servo_faulted: d.servo_faulted,
        },
    }
}

fn host_inputs<'a>(i: &LoopInputs<'a>, loop_index: u64) -> HostInputs<'a> {
    HostInputs {
        target_psd: i.target_psd,
        measured_psd: i.measured_psd,
        h_mag2: i.h_mag2,
        coherence: i.coherence,
        current_level_db: i.current_level_db,
        df_hz: i.df_hz,
        notched_bins: i.notched_bins,
        band: i.band,
        dof: i.dof,
        loop_index,
    }
}

/// A law file built for one run, as the run core's [`ControlLaw`].
///
/// Every call carries the run's call budget as its deadline: the same number
/// the core measures the call against, so the host stops a call that will not
/// return at the moment the core would stop waiting for it. A call that traps,
/// runs past its budget or breaches the memory cap leaves the instance
/// unusable; every later call fails with that same fault, so the core's misses
/// in a row run out and it takes its controlled stop.
pub struct WasmControlLaw {
    instance: LawInstance,
    budget: Duration,
    /// Iterations since the seed: what the law is told as its loop index.
    loop_index: u64,
    /// The fault that made the instance unusable.
    poisoned_by: Option<LawFault>,
}

impl WasmControlLaw {
    /// Run `instance` with `budget` as every call's deadline.
    pub fn new(instance: LawInstance, budget: Duration) -> Self {
        WasmControlLaw {
            instance,
            budget,
            loop_index: 0,
            poisoned_by: None,
        }
    }

    /// The law instance (its identity, parameters and kept output).
    pub fn instance(&self) -> &LawInstance {
        &self.instance
    }

    /// Each call's deadline.
    pub fn budget(&self) -> Duration {
        self.budget
    }

    fn fault(&mut self, e: LawError) -> LawFault {
        if let LawError::Poisoned(_) = e {
            if let Some(first) = &self.poisoned_by {
                return first.clone();
            }
        }
        let fault = law_fault(&e);
        if e.poisons() {
            self.poisoned_by = Some(fault.clone());
        }
        fault
    }
}

impl ControlLaw for WasmControlLaw {
    fn init(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        self.loop_index = 0;
        let r = self.instance.seed(&host_inputs(inputs, 0), self.budget);
        r.map(core_outputs).map_err(|e| self.fault(e))
    }

    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        self.loop_index += 1;
        let r = self
            .instance
            .iterate(&host_inputs(inputs, self.loop_index), self.budget);
        r.map(core_outputs).map_err(|e| self.fault(e))
    }

    fn retarget_level(&mut self, cmd: LevelCommand) -> Result<LoopOutputs, LawFault> {
        let r = self
            .instance
            .retarget_level(cmd.level_db, cmd.target_rms, self.budget);
        r.map(core_outputs).map_err(|e| self.fault(e))
    }

    fn name(&self) -> &str {
        &self.instance.identity().name
    }
}

/// The time one law call may take, from the run's setup.
fn call_budget(setup: &LawSetup<'_>) -> Result<Duration, ControlError> {
    Duration::try_from_secs_f64(setup.call_budget_s)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or_else(|| {
            ControlError::BadConfig(format!(
                "a law call budget of {} s is not a usable time",
                setup.call_budget_s
            ))
        })
}

/// Builds a law file's [`WasmControlLaw`] for each run, with the parameter
/// values it was chosen with (complete and checked: see
/// [`crate::LawCatalog::resolve`]). The law is constructed within the run's
/// call budget, like every call after it.
#[derive(Clone)]
pub struct WasmLawFactory {
    law: LoadedLaw,
    params: Vec<Param>,
}

impl WasmLawFactory {
    /// A factory for `law` with `params`.
    pub fn new(law: LoadedLaw, params: Vec<Param>) -> Self {
        WasmLawFactory { law, params }
    }

    /// The law this factory builds.
    pub fn law(&self) -> &LoadedLaw {
        &self.law
    }

    /// The parameter values each build is given.
    pub fn params(&self) -> &[Param] {
        &self.params
    }
}

impl LawFactory for WasmLawFactory {
    fn build(&self, setup: &LawSetup<'_>) -> Result<Box<dyn ControlLaw + Send>, ControlError> {
        let budget = call_budget(setup)?;
        let s = Setup {
            target_psd: setup.target_psd,
            df_hz: setup.df_hz,
            band: setup.band,
            control_channels: setup.control_channels,
        };
        let instance = self
            .law
            .instantiate(&s, &self.params, budget)
            .map_err(|e| ControlError::BadConfig(e.to_string()))?;
        Ok(Box::new(WasmControlLaw::new(instance, budget)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_host_error_reads_as_its_fault() {
        let budget = Duration::from_millis(375);
        let cases = [
            (LawError::Law("no".into()), LawFault::Failed("no".into())),
            (
                LawError::InputsInvalid("ragged".into()),
                LawFault::BadInputs("ragged".into()),
            ),
            (
                LawError::Trap("boom".into()),
                LawFault::Crashed("boom".into()),
            ),
            (
                LawError::Deadline { budget },
                LawFault::OverBudget("stopped at its 375.0 ms budget".into()),
            ),
            (
                LawError::Memory {
                    limit_bytes: 10,
                    requested_bytes: 20,
                },
                LawFault::OverMemory("asked for 20 bytes against a cap of 10".into()),
            ),
        ];
        for (e, want) in cases {
            assert_eq!(law_fault(&e), want, "{e:?}");
        }
        for (rule, want) in [
            (OutputRule::WrongLength, OutputFault::WrongLength),
            (OutputRule::NonFiniteDrive, OutputFault::NonFinite),
            (OutputRule::NegativeDrive, OutputFault::Negative),
            (OutputRule::NonFiniteLevel, OutputFault::NonFiniteLevel),
            (OutputRule::Diagnostics, OutputFault::Diagnostics),
        ] {
            let e = LawError::BadOutput {
                rule,
                detail: "why".into(),
            };
            assert_eq!(law_fault(&e), LawFault::BadOutput(want, "why".into()));
        }
    }
}
