//! A4: every guard falsified by an adversarial law that breaks it.
//!
//! Each law wraps the default law (so the seed and the prediction are sound) and
//! misbehaves from its first iteration on. Each test asserts the fault the core
//! records and that no drive ever went over its ceiling. The red-before evidence
//! (each test run with its guard removed from the source) is in the work
//! package's session log; the mutation each guard is falsified by is named on its
//! test.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_law_host::{LawIdentity, Param};
use hertzforge_mimo_random::guards::{drive_rms, OutputFault};
use hertzforge_mimo_random::{
    MimoEventKind, MimoFaultReason, MimoRandomService, MimoState, MimoTripKind,
};
use hertzforge_random::service::law::LawClock;
use hertzforge_random_laws::{ChoiceError, LawBudgets, MimoLawFactory};
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::{linalg, HermitianStack, C64};
use std::sync::{Arc, Mutex};

const CEILING: f64 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Nan,
    NonHermitian,
    WrongDims,
    OutOfBand,
    IndefiniteSmall,
    IndefiniteLarge,
    Runaway,
    Oscillating,
    Slow,
    Trapping,
}

/// A clock the slow law advances.
#[derive(Default)]
struct TestClock(Mutex<f64>);

impl LawClock for TestClock {
    fn now_s(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

struct Adversary {
    inner: Box<dyn MimoControlLaw + Send>,
    mode: Mode,
    clock: Arc<TestClock>,
    calls: u64,
}

fn rebuild(lines: usize, n: usize, data: Vec<C64>) -> HermitianStack {
    HermitianStack::from_packed(lines, n, data).unwrap()
}

impl Adversary {
    fn corrupt(&mut self, mut out: MimoLoopOutputs, band: (usize, usize)) -> MimoLoopOutputs {
        let (lines, n) = (out.drive.lines(), out.drive.n());
        let per = HermitianStack::packed_len(n);
        match self.mode {
            Mode::Nan => {
                let mut d = out.drive.data().to_vec();
                d[(band.0 + 3) * per] = C64::new(f64::NAN, 0.0);
                out.drive = rebuild(lines, n, d);
            }
            Mode::NonHermitian => {
                // A packed stack holds a real diagonal; a law that hands back one
                // built some other way (here through serde) can carry an imaginary
                // diagonal, which no Hermitian matrix has.
                let mut v = serde_json::to_value(&out.drive).unwrap();
                let k = (band.0 + 5) * per;
                let re = v["data"][k][0].as_f64().unwrap();
                v["data"][k] = serde_json::json!([re, 0.5 * re.abs() + 1e-3]);
                out.drive = serde_json::from_value(v).unwrap();
            }
            Mode::WrongDims => out.drive = HermitianStack::zeros(lines, n + 1),
            Mode::OutOfBand => {
                let mut d = out.drive.data().to_vec();
                d[(band.0 - 1) * per] = C64::new(1e-6, 0.0);
                out.drive = rebuild(lines, n, d);
            }
            Mode::IndefiniteSmall | Mode::IndefiniteLarge => {
                let rel = if self.mode == Mode::IndefiniteSmall {
                    3e-4
                } else {
                    0.1
                };
                let e = linalg::eigh(&out.drive).unwrap();
                let mut d = out.drive.data().to_vec();
                for line in band.0..band.1 {
                    let vals = &e.values[line * n..(line + 1) * n];
                    let (lmin, lmax) = (vals[0], vals[n - 1]);
                    let shift = lmin + rel * lmax;
                    for r in 0..n {
                        for c in r..n {
                            let u = e.vectors.get(line, r, 0) * e.vectors.get(line, c, 0).conj();
                            d[line * per + HermitianStack::packed_index(n, r, c)] -= u * shift;
                        }
                    }
                }
                out.drive = rebuild(lines, n, d);
            }
            Mode::Runaway => {
                let _ = out.drive.scale_lines(&vec![100.0; lines]);
            }
            Mode::Oscillating => {
                let f = if self.calls.is_multiple_of(2) {
                    2.5
                } else {
                    1.0 / 2.5
                };
                let _ = out.drive.scale_lines(&vec![f; lines]);
            }
            Mode::Slow => {
                *self.clock.0.lock().unwrap() += 1e6;
            }
            Mode::Trapping => panic!("the law trapped"),
        }
        out
    }
}

impl MimoControlLaw for Adversary {
    fn needs(&self) -> &[MimoInput] {
        self.inner.needs()
    }
    fn shapes(&self) -> &[MimoShape] {
        self.inner.shapes()
    }
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.inner.seed(inputs)
    }
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.calls += 1;
        let out = self.inner.iterate(inputs)?;
        Ok(self.corrupt(out, inputs.band))
    }
}

struct AdversaryFactory {
    inner: Arc<dyn MimoLawFactory>,
    mode: Mode,
    clock: Arc<TestClock>,
}

impl MimoLawFactory for AdversaryFactory {
    fn identity(&self) -> LawIdentity {
        LawIdentity {
            name: format!("adversary {:?}", self.mode),
            version: "0".into(),
            author: "test".into(),
            sha256: String::new(),
        }
    }
    fn params(&self) -> &[Param] {
        self.inner.params()
    }
    fn build(
        &self,
        setup: &MimoSetup,
        budgets: LawBudgets,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        Ok(Box::new(Adversary {
            inner: self.inner.build(setup, budgets)?,
            mode: self.mode,
            clock: self.clock.clone(),
            calls: 0,
        }))
    }
}

/// What a run with an adversary did.
struct Outcome {
    svc: MimoRandomService,
    /// The largest drive rms over the ceiling, per loop (SDM at the played level
    /// and the realized record), relative: 1.0 is at the ceiling.
    worst_sdm: f64,
    worst_realized: f64,
    misses: Vec<MimoFaultReason>,
}

fn run(mode: Mode, tag: &str, loops: usize) -> Outcome {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    for d in &mut cfg.drives {
        d.ceiling_vrms = CEILING;
        d.peak_v = 3.0 * CEILING;
        d.pretest_max_vrms = 0.5;
    }
    let inner = hertzforge_mimo_random::law::builtin_factory(&cfg.law, 4).unwrap();
    let clock = Arc::new(TestClock::default());
    let factory = AdversaryFactory {
        inner,
        mode,
        clock: clock.clone(),
    };
    let mut svc = service(cfg, table, &p, &grid, tag)
        .with_law_factory(Arc::new(factory))
        .with_law_clock(clock);
    let st = svc.run_auto(0).expect("the run engages");
    assert_eq!(st, MimoState::Engaged);
    let (mut worst_sdm, mut worst_realized) = (0.0f64, 0.0f64);
    let band = svc.band();
    for _ in 0..loops {
        if !svc.state().is_running() {
            break;
        }
        let level = 10f64.powf(svc.level_db() / 10.0);
        let mut g = svc.drive_full_level().clone();
        let _ = g.scale_lines(&vec![level; svc.lines()]);
        for r in drive_rms(&g, band, svc.df_hz()) {
            worst_sdm = worst_sdm.max(r / CEILING);
        }
        svc.loop_tick().expect("a loop");
        if std::env::var("ADV_DEBUG").is_ok() {
            eprintln!(
                "loop {} level {:.1} err {:?} guards {:?} misses {}",
                svc.loop_count(),
                svc.level_db(),
                svc.error_db(),
                svc.guard_summary(),
                svc.law_stats().misses
            );
        }
        for &r in svc.drive_rms() {
            worst_realized = worst_realized.max(r / CEILING);
        }
    }
    let misses = svc
        .log()
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            MimoEventKind::LawMiss { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect();
    Outcome {
        svc,
        worst_sdm,
        worst_realized,
        misses,
    }
}

fn assert_under_ceiling(o: &Outcome) {
    assert!(
        o.worst_sdm <= 1.0 + 1e-9,
        "the drive in force went {:.4} times its ceiling",
        o.worst_sdm
    );
    assert!(
        o.worst_realized <= 1.05,
        "a realized drive went {:.4} times its ceiling",
        o.worst_realized
    );
}

/// Three misses of `reason` in a row, then the controlled stop with the law fault
/// recorded.
fn assert_law_fault_stop(o: &Outcome, reason: MimoFaultReason) {
    assert!(o.misses.len() >= 3, "misses {:?}", o.misses);
    assert!(
        o.misses.iter().all(|m| *m == reason),
        "misses {:?}",
        o.misses
    );
    assert_eq!(o.svc.state(), MimoState::Stopped, "{:?}", o.svc.trip());
    match o.svc.trip().map(|t| &t.kind) {
        Some(MimoTripKind::LawFault {
            reason: r,
            consecutive,
        }) => {
            assert_eq!(*r, reason);
            assert!(*consecutive >= 3);
        }
        other => panic!("expected a law-fault stop, got {other:?}"),
    }
    assert_under_ceiling(o);
}

/// Mutation: remove the finiteness check in `guards::check_output`.
#[test]
fn a_nan_drive_is_refused_and_three_end_the_run() {
    let o = run(Mode::Nan, "adv-nan", 12);
    assert_law_fault_stop(&o, MimoFaultReason::BadOutput(OutputFault::NonFinite));
}

/// Mutation: remove the Hermitian check in `guards::check_output`.
#[test]
fn a_non_hermitian_drive_is_refused() {
    let o = run(Mode::NonHermitian, "adv-herm", 12);
    assert_law_fault_stop(&o, MimoFaultReason::BadOutput(OutputFault::NotHermitian));
}

/// Mutation: remove the dimension check in `guards::check_output`.
#[test]
fn a_drive_of_the_wrong_dimensions_is_refused() {
    let o = run(Mode::WrongDims, "adv-dims", 12);
    assert_law_fault_stop(&o, MimoFaultReason::BadOutput(OutputFault::Dimensions));
}

/// Mutation: remove the out-of-band check in `guards::check_output`.
#[test]
fn a_drive_outside_the_band_is_refused() {
    let o = run(Mode::OutOfBand, "adv-oob", 12);
    assert_law_fault_stop(&o, MimoFaultReason::BadOutput(OutputFault::OutOfBand));
}

/// Mutation: skip `psd_repair` in `guards::check_output`. Small negative
/// eigenvalues (3e-4 of the largest) are repaired, counted, and the run goes on.
#[test]
fn a_slightly_indefinite_drive_is_repaired_and_counted() {
    let o = run(Mode::IndefiniteSmall, "adv-indef-small", 4);
    assert!(o.misses.is_empty(), "{:?}", o.misses);
    assert!(o.svc.state().is_running(), "{:?}", o.svc.trip());
    let rep = &o.svc.guard_summary().repair;
    let (lo, hi) = o.svc.band();
    assert_eq!(rep.lines_repaired, hi - lo, "{rep:?}");
    assert!(rep.eigenvalues_clamped >= hi - lo);
    assert!(rep.worst_relative < -1e-4, "{rep:?}");
    // The drive in force is positive semi-definite.
    let e = linalg::eigh(o.svc.drive_full_level()).unwrap();
    let n = o.svc.drive_full_level().n();
    for line in lo..hi {
        let v = &e.values[line * n..(line + 1) * n];
        assert!(v[0] >= -1e-9 * v[n - 1].abs(), "line {line}: {v:?}");
    }
    assert!(o.svc.law_stats().repaired_lines > 0);
    assert_under_ceiling(&o);
}

/// Mutation: make the reject threshold out of reach in `guards::check_output`.
/// A negative eigenvalue of a tenth of the largest is refused.
#[test]
fn a_badly_indefinite_drive_is_refused() {
    let o = run(Mode::IndefiniteLarge, "adv-indef-large", 12);
    assert_law_fault_stop(
        &o,
        MimoFaultReason::BadOutput(OutputFault::NotPositiveSemiDefinite),
    );
}

/// Mutation: remove `cap_change` and `apply_ceilings` from `accept`. A law that
/// asks for 20 dB more every loop is held by the caps and the ceilings, and the
/// core's divergence judgement ends the run.
#[test]
fn a_runaway_law_is_held_by_the_caps_and_the_ceilings() {
    let o = run(Mode::Runaway, "adv-runaway", 20);
    assert!(o.svc.law_stats().capped_drive_lines > 0);
    assert_eq!(o.svc.state(), MimoState::Aborted);
    assert!(
        matches!(
            o.svc.trip().map(|t| &t.kind),
            Some(MimoTripKind::Divergence { .. })
                | Some(MimoTripKind::CapDivergence { .. })
                | Some(MimoTripKind::Envelope { .. })
        ),
        "{:?}",
        o.svc.trip()
    );
    assert_under_ceiling(&o);
}

/// Mutation: make `check_cap_divergence` return `true` at once. A law that swings
/// the drive up and down every loop, faster than the caps allow, is divergent by
/// the core's own count (the averaged estimate hides the swing), and the run ends.
#[test]
fn an_oscillating_law_trips_the_divergence_abort() {
    let o = run(Mode::Oscillating, "adv-osc", 20);
    assert_eq!(o.svc.state(), MimoState::Aborted, "{:?}", o.svc.trip());
    assert!(
        matches!(
            o.svc.trip().map(|t| &t.kind),
            Some(MimoTripKind::Divergence { .. }) | Some(MimoTripKind::CapDivergence { .. })
        ),
        "{:?}",
        o.svc.trip()
    );
    assert_under_ceiling(&o);
}

/// Mutation: remove the budget check in `law::guarded_call`. A late answer is a
/// miss even when it is a good one; three end the run with the drive in force
/// held, never above its ceiling.
#[test]
fn a_slow_law_misses_three_times_and_the_run_stops_with_the_drive_held() {
    let o = run(Mode::Slow, "adv-slow", 12);
    assert_law_fault_stop(&o, MimoFaultReason::OverBudget);
}

/// Mutation: remove `catch_unwind` from `law::guarded_call` (the test process
/// then dies). A law that traps (here: panics) is contained, counted, and three
/// end the run.
#[test]
fn a_trapping_law_is_contained() {
    let o = run(Mode::Trapping, "adv-trap", 12);
    assert_law_fault_stop(&o, MimoFaultReason::Panicked);
}
