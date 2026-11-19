//! A law that cannot fit its time is named before the run, not in the middle of
//! one: the seed (build and first answer together) against the seed budget,
//! and one rehearsed iteration against the call budget.

mod support;

use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_mimo_shock::builtin::InverseFactory;
use hertzforge_mimo_shock::{
    EndOutcome, LawManifest, MimoShockEngine, MimoShockLaw, MimoShockLawFactory, PulseInputs,
    PulseOutputs, Setup,
};
use hertzforge_module_abi::StationClass;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault};
use std::sync::Arc;
use support::*;

/// `inverse`, with its build and its iterations taking the times given on the
/// run's own clock.
struct Slow {
    clock: Arc<ManualClock>,
    build_s: f64,
    iterate_s: f64,
}

struct SlowLaw {
    inner: Box<dyn MimoShockLaw + Send>,
    clock: Arc<ManualClock>,
    iterate_s: f64,
}

impl MimoShockLaw for SlowLaw {
    fn name(&self) -> &str {
        "slow"
    }
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, LawFault> {
        self.inner.seed(level_db)
    }
    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, LawFault> {
        self.clock.advance(self.iterate_s);
        self.inner.iterate(inputs)
    }
}

impl MimoShockLawFactory for Slow {
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        self.clock.advance(self.build_s);
        Ok(Box::new(SlowLaw {
            inner: InverseFactory::default().build(setup)?,
            clock: self.clock.clone(),
            iterate_s: self.iterate_s,
        }))
    }
    fn describe(&self) -> LawDescriptor {
        InverseFactory::default().describe()
    }
    fn manifest(&self) -> LawManifest {
        InverseFactory::default().manifest()
    }
}

/// Arm, identify and proceed on the linear plant with a classical target;
/// the engine after `proceed`.
fn proceeded(
    build_s: f64,
    iterate_s: f64,
    edit: impl FnOnce(&mut hertzforge_mimo_shock::MimoShockConfig),
) -> MimoShockEngine {
    let clock = Arc::new(ManualClock::default());
    let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
    edit(&mut cfg);
    let law = Slow {
        clock: clock.clone(),
        build_s,
        iterate_s,
    };
    let mut e = MimoShockEngine::new(cfg, Arc::new(law))
        .unwrap()
        .with_target_spec(classical(&[20.0, 18.0, 20.0, 18.0]))
        .with_clock(clock)
        .with_pacing(false);
    e.arm(
        station(ShockPlant::Linear4x4, FS, 11),
        table(4, 0, 4),
        StationClass::HertzforgeSim,
    )
    .unwrap();
    e.identify().unwrap();
    e.proceed().unwrap();
    e
}

#[test]
fn mimo_shock_x_a_seed_over_its_budget_is_named_before_the_first_pulse() {
    let e = proceeded(61.0, 0.0, |_| {});
    let end = e.end().expect("ended before the first pulse");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::LawFault, "seed_over_budget"),
        "{end:?}"
    );
    assert!(e.pulses().is_empty());
    // Inside its budget, the same law runs.
    let e = proceeded(59.0, 0.0, |_| {});
    assert!(e.end().is_none(), "{:?}", e.end());
}

#[test]
fn mimo_shock_x_a_law_too_slow_for_a_pulse_is_named_before_the_first_pulse() {
    // The call budget is a quarter of the 1 s spacing.
    let mut e = proceeded(0.0, 0.3, |_| {});
    let end = e.end().expect("ended before the first pulse").clone();
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::LawFault, "law_too_slow"),
        "{end:?}"
    );
    assert!(e.pulses().is_empty());
    assert!(e.run_to_end(None).is_err() || e.pulses().is_empty());
    // With the rehearsal off it is found in the middle of the run.
    let mut e = proceeded(0.0, 0.3, |c| c.guards.rehearse_law = false);
    assert!(e.end().is_none());
    let end = e.run_to_end(None).unwrap();
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::LawFault, "over_budget")
    );
    assert!(!e.pulses().is_empty());
    // A law inside its budget rehearses and runs.
    let e = proceeded(0.0, 0.2, |_| {});
    assert!(e.end().is_none(), "{:?}", e.end());
}

#[test]
fn mimo_shock_a_law_too_slow_names_a_budget_under_a_tenth_of_a_millisecond_as_it_is() {
    // The NI bench, 2026-11-11, row M9: a budget of 1 us was stated as "its
    // 0.0 ms budget". Here the budget is a millionth of the 1 s spacing.
    let e = proceeded(0.0, 0.3, |c| c.guards.law.budget_fraction = 1e-6);
    let end = e.end().expect("ended before the first pulse");
    assert_eq!(end.reason, "law_too_slow", "{end:?}");
    assert!(!end.detail.contains("0.0 ms budget"), "{}", end.detail);
    assert!(end.detail.contains("0.001 ms budget"), "{}", end.detail);
    assert!(end.detail.contains("took 300 ms"), "{}", end.detail);
}
