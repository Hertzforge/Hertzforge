//! The law's first solve has a time budget of its own.
//!
//! The first solve (the law's seed) runs once, between identification and the
//! prediction, with the outputs stopped. It is held to a budget in seconds
//! (`MimoConfig::seed_budget_s`), and every call after it to a fraction of the
//! loop it serves. The law here is the default law taking a set time on an
//! injected clock: so much for its first solve, so much for every later call.
//!
//! Each test names the change to the source that makes it fail.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_law_host::{LawIdentity, Param};
use hertzforge_mimo_random::{
    MimoError, MimoEventKind, MimoFaultReason, MimoRandomService, MimoState,
};
use hertzforge_random::service::law::LawClock;
use hertzforge_random_laws::{ChoiceError, LawBudgets, MimoLawFactory};
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A clock the law advances.
#[derive(Default)]
struct TestClock(Mutex<f64>);

impl LawClock for TestClock {
    fn now_s(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

struct Timed {
    inner: Box<dyn MimoControlLaw + Send>,
    clock: Arc<TestClock>,
    seed_s: f64,
    call_s: f64,
}

impl MimoControlLaw for Timed {
    fn needs(&self) -> &[MimoInput] {
        self.inner.needs()
    }
    fn shapes(&self) -> &[MimoShape] {
        self.inner.shapes()
    }
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        *self.clock.0.lock().unwrap() += self.seed_s;
        self.inner.seed(inputs)
    }
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        *self.clock.0.lock().unwrap() += self.call_s;
        self.inner.iterate(inputs)
    }
}

struct TimedFactory {
    inner: Arc<dyn MimoLawFactory>,
    clock: Arc<TestClock>,
    seed_s: f64,
    call_s: f64,
    /// The budgets the run built its law with.
    built_with: Arc<Mutex<Option<LawBudgets>>>,
}

impl MimoLawFactory for TimedFactory {
    fn identity(&self) -> LawIdentity {
        self.inner.identity()
    }
    fn params(&self) -> &[Param] {
        self.inner.params()
    }
    fn build(
        &self,
        setup: &MimoSetup,
        budgets: LawBudgets,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        *self.built_with.lock().unwrap() = Some(budgets);
        Ok(Box::new(Timed {
            inner: self.inner.build(setup, budgets)?,
            clock: self.clock.clone(),
            seed_s: self.seed_s,
            call_s: self.call_s,
        }))
    }
}

/// A 4 by 4 run at the gate, with a law whose first solve takes `seed_s` and
/// whose later calls take `call_s`, and the first solve's budget set to
/// `seed_budget_s` when given.
fn at_the_gate(
    tag: &str,
    seed_s: f64,
    call_s: f64,
    seed_budget_s: Option<f64>,
) -> (MimoRandomService, Arc<Mutex<Option<LawBudgets>>>) {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    if let Some(s) = seed_budget_s {
        cfg.seed_budget_s = s;
    }
    let inner = hertzforge_mimo_random::law::builtin_factory(&cfg.law, 4).unwrap();
    let clock = Arc::new(TestClock::default());
    let built_with = Arc::new(Mutex::new(None));
    let factory = TimedFactory {
        inner,
        clock: clock.clone(),
        seed_s,
        call_s,
        built_with: built_with.clone(),
    };
    let mut svc = service(cfg, table, &p, &grid, tag)
        .with_law_factory(Arc::new(factory))
        .with_law_clock(clock);
    svc.start().expect("feasibility and arm");
    svc.run_noise_floor().expect("the noise floor");
    svc.run_identification().expect("identification");
    assert_eq!(svc.state(), MimoState::Gate, "{:?}", svc.trip());
    (svc, built_with)
}

fn misses(svc: &MimoRandomService) -> Vec<MimoFaultReason> {
    svc.log()
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            MimoEventKind::LawMiss { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect()
}

/// Mutation: time the first solve against the loop's budget (`call_law` with
/// `law_budget_s()` for every call). The 30 s solve is then over budget and the
/// run is refused at the gate.
#[test]
fn a_first_solve_far_longer_than_a_loop_call_is_used_inside_its_own_budget() {
    let (mut svc, built_with) = at_the_gate("seed-budget-inside", 30.0, 0.0, None);
    svc.proceed().expect("the first solve is used");
    assert_eq!(svc.state(), MimoState::Predict);
    // A law file is given both deadlines: the first solve's, and the loop's.
    let b = built_with.lock().unwrap().expect("the law was built");
    assert_eq!(b.seed, Duration::from_secs(120), "the default seed budget");
    assert!(
        b.call < Duration::from_secs(30) && b.call > Duration::ZERO,
        "a loop call's budget is a fraction of the loop: {:?}",
        b.call
    );
    svc.proceed().expect("engage");
    for _ in 0..3 {
        svc.loop_tick().expect("a loop");
    }
    assert!(svc.state().is_running(), "{:?}", svc.state());
    assert!(misses(&svc).is_empty(), "{:?}", misses(&svc));
}

/// Mutation: drop the over-budget arm in `predict` (or raise the budget in
/// `call_law`). The run then goes on to the prediction with a first solve that
/// took longer than its budget.
#[test]
fn a_first_solve_past_its_budget_is_refused_before_anything_drives() {
    let (mut svc, _) = at_the_gate("seed-budget-over", 6.0, 0.0, Some(5.0));
    let loops_before = svc.loop_count();
    let e = svc
        .proceed()
        .expect_err("the first solve is over its budget");
    let MimoError::Law(msg) = e else {
        panic!("a law error, not {e:?}");
    };
    assert!(
        msg.contains("first solve")
            && msg.contains("budget of 5 s")
            && msg.contains("nothing was driven"),
        "{msg}"
    );
    assert_eq!(svc.state(), MimoState::Gate, "the run goes no further");
    assert_eq!(svc.loop_count(), loops_before);
    assert!(svc.prediction().is_none(), "no prediction was formed");
}

/// Mutation: time every call against the first solve's budget. A 30 s loop
/// call is inside 120 s and would then be used.
#[test]
fn a_call_after_the_first_solve_is_still_held_to_the_loops_budget() {
    let (mut svc, _) = at_the_gate("seed-budget-loop", 0.0, 30.0, None);
    svc.proceed().expect("the first solve");
    svc.proceed().expect("engage");
    for _ in 0..12 {
        if !svc.state().is_running() {
            break;
        }
        svc.loop_tick().expect("a loop");
    }
    let m = misses(&svc);
    assert!(
        m.len() >= 3 && m.iter().all(|r| *r == MimoFaultReason::OverBudget),
        "{m:?}"
    );
    assert!(
        !svc.state().is_running(),
        "the misses in a row stop the run"
    );
}

/// Mutation: remove the check from `MimoConfig::validate`.
#[test]
fn a_seed_budget_that_is_not_a_time_is_refused_by_the_setup() {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let (mut cfg, _) = physical_config(&p, &grid, 0.5);
        cfg.seed_budget_s = bad;
        let e = cfg
            .validate(grid.lines(), grid.df())
            .expect_err("not a usable budget");
        assert!(format!("{e:?}").contains("first solve"), "{bad}: {e:?}");
    }
}
