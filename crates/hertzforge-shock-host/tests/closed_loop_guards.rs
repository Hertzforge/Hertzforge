//! The core's guards on a classical control law, each held by an adversarial
//! native law that breaks one rule.
//!
//! Every law here is the built-in `fd_correct` with one thing wrong: an output
//! that is not a number, a short record, a bin that jumps 20 dB, a peak that
//! runs away, a first drive three times the reference, a drive that does not
//! end at rest, a call that is slow on the test clock, a panic, an overshoot of
//! 50%, a law that never corrects, one that diverges, and a drive that grows
//! past the drive abort. The core must catch each one, the way the plan says,
//! and nothing it refuses may play. The drive ceiling, the predicted drive at
//! proceed, a refused level move and the predicted kinematics are held here
//! too.
//!
//! The plant is the simulator's: a wire of 40 g per volt, so the 20 g half-sine
//! needs 0.5 V at full level.

use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_shaker_db::{RatingValue, ShakerRecord};
use hertzforge_shock_common::law_guard::{LawClock, LawDescriptor, LawFault, OutputRule};
use hertzforge_shock_common::{LadderAdvance, ShockLevelSchedule, ShockRunMode};
use hertzforge_shock_host::service::pacer::VirtualPacer;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalControl, ClassicalLaw, ClassicalLawFactory, ClassicalLawSetup,
    ClassicalPulseSpec, ClassicalShape, ClassicalShockService, ClassicalTestConfig,
    FdCorrectFactory, PulseInputs, PulseOutputs, RunState, ServiceError, TripKind,
};
use std::sync::{Arc, Mutex};

const FS: f64 = 5120.0;
const GAIN: f64 = 40.0;

type Svc = ClassicalShockService<BackendPulseFirer<SimBackend>>;

fn spec() -> ClassicalPulseSpec {
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    spec
}

fn config(ceiling: f64, abort: f64) -> ClassicalTestConfig {
    let mut cfg = ClassicalTestConfig::new(spec());
    let mut control = ClassicalControl::new(FS, ceiling, abort);
    control.sysid.duration_s = 2.0;
    control.review_hold = false;
    cfg.control = Some(control);
    cfg.schedule = ShockLevelSchedule {
        pulses_per_level: vec![1, 1, 1, 1, 3],
        advance: LadderAdvance::OnPass,
        ..ShockLevelSchedule::auto_default()
    };
    cfg
}

fn service(cfg: ClassicalTestConfig, tag: &str) -> Svc {
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    let backend = SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .noise_level(0.0)
        .feedthrough(GAIN)
        .shared_clock(true)
        .build();
    let firer = BackendPulseFirer::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut s = ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(std::env::temp_dir().join(format!(
            "hertzforge-shock-guards-{tag}-{}",
            std::process::id()
        ))),
    )
    .unwrap();
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));
    s
}

/// What one adversarial law gets wrong.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wrong {
    /// Every correction holds a value that is not a number.
    Nan,
    /// Every correction is one sample short.
    Short,
    /// Every correction adds a tone 20 dB over a quiet bin.
    BinJump,
    /// Every correction is four times the last drive (12 dB).
    PeakRunaway,
    /// The seed is three times the built-in's.
    FirstDriveTriple,
    /// Every drive ends on a step of a tenth of its peak.
    NotAtRest,
    /// Every correction takes 0.3 s on the test clock (the budget is 0.25 s).
    Slow,
    /// Every correction panics.
    Panic,
    /// The seed is 1.5 times the built-in's.
    Overshoot,
    /// The seed is half the built-in's, and it never changes.
    NeverCorrects,
    /// Every correction is 0.7 times the last drive.
    Diverges,
    /// The second drive is 1.8 times the first (5.1 dB, inside the caps).
    PastTheAbort,
}

/// A clock the slow law moves forward.
#[derive(Default)]
struct TestClock(Mutex<f64>);
impl LawClock for TestClock {
    fn now_s(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

struct Adversary {
    inner: Box<dyn ClassicalLaw + Send>,
    wrong: Wrong,
    clock: Arc<TestClock>,
    last: Vec<f64>,
    tone: Vec<f64>,
    calls: u32,
}

impl ClassicalLaw for Adversary {
    fn name(&self) -> &str {
        "adversary"
    }

    fn seed(&mut self, inputs: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault> {
        let mut out = self.inner.seed(inputs)?;
        let k = match self.wrong {
            Wrong::FirstDriveTriple => 3.0,
            Wrong::Overshoot => 1.5,
            Wrong::NeverCorrects => 0.5,
            _ => 1.0,
        };
        for v in out.drive.iter_mut() {
            *v *= k;
        }
        if self.wrong == Wrong::NotAtRest {
            let p = out.drive.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            let n = out.drive.len();
            for v in out.drive[n - 40..].iter_mut() {
                *v = 0.1 * p;
            }
            out.drive[0] = 0.1 * p;
        }
        self.last = out.drive.clone();
        Ok(out)
    }

    fn iterate(&mut self, inputs: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault> {
        self.calls += 1;
        let scaled = |last: &[f64], k: f64| PulseOutputs {
            drive: last.iter().map(|v| v * k).collect(),
            values: Vec::new(),
        };
        let out = match self.wrong {
            Wrong::Nan => {
                let mut o = self.inner.iterate(inputs)?;
                o.drive[100] = f64::NAN;
                o
            }
            Wrong::Short => {
                let mut o = self.inner.iterate(inputs)?;
                o.drive.pop();
                o
            }
            Wrong::BinJump => {
                let mut o = self.inner.iterate(inputs)?;
                for (v, t) in o.drive.iter_mut().zip(&self.tone) {
                    *v += t;
                }
                o
            }
            Wrong::PeakRunaway => scaled(&self.last, 4.0),
            Wrong::Slow => {
                *self.clock.0.lock().unwrap() += 0.3;
                self.inner.iterate(inputs)?
            }
            Wrong::Panic => panic!("the adversary panics on purpose"),
            Wrong::NeverCorrects => scaled(&self.last, 1.0),
            Wrong::Diverges => scaled(&self.last, 0.7),
            Wrong::PastTheAbort if self.calls == 1 => scaled(&self.last, 1.8),
            _ => {
                let mut o = self.inner.iterate(inputs)?;
                if self.wrong == Wrong::NotAtRest {
                    let p = o.drive.iter().fold(0.0f64, |m, v| m.max(v.abs()));
                    let n = o.drive.len();
                    for v in o.drive[n - 40..].iter_mut() {
                        *v = 0.1 * p;
                    }
                }
                o
            }
        };
        if out.drive.len() == self.last.len() && out.drive.iter().all(|v| v.is_finite()) {
            self.last = out.drive.clone();
        }
        Ok(out)
    }
}

struct AdversaryFactory {
    wrong: Wrong,
    clock: Arc<TestClock>,
}

impl ClassicalLawFactory for AdversaryFactory {
    fn build(
        &self,
        setup: &ClassicalLawSetup<'_>,
    ) -> Result<Box<dyn ClassicalLaw + Send>, LawFault> {
        // A tone at the bin where the reference has least content in the
        // correction band, 0.05 of the reference's peak over the drive: tiny
        // in the time record, a jump of far more than 20 dB in its bin.
        let n = setup.record_len;
        let df = setup.fs_hz / n as f64;
        let bins = n / 2 + 1;
        let mut best = (f64::INFINITY, 1usize);
        for k in 1..bins - 1 {
            let f = k as f64 * df;
            if f < setup.band_lo_hz || f > setup.band_hi_hz || !setup.plant.trusted[k] {
                continue;
            }
            // Reference magnitude at k by direct sum (n is small).
            let (mut re, mut im) = (0.0, 0.0);
            for (i, r) in setup.reference.iter().enumerate() {
                let w = std::f64::consts::TAU * (k * i) as f64 / n as f64;
                re += r * w.cos();
                im -= r * w.sin();
            }
            let m = (re * re + im * im).sqrt();
            if m < best.0 {
                best = (m, k);
            }
        }
        let amp = 0.05 * 20.0 / GAIN;
        let tone = (0..n)
            .map(|i| amp * (std::f64::consts::TAU * (best.1 * i) as f64 / n as f64).sin())
            .collect();
        Ok(Box::new(Adversary {
            inner: FdCorrectFactory::default().build(setup)?,
            wrong: self.wrong,
            clock: Arc::clone(&self.clock),
            last: Vec::new(),
            tone,
            calls: 0,
        }))
    }

    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: format!("adversary-{:?}", self.wrong),
            name: "adversary".into(),
            version: "0".into(),
            sha256: None,
            params: serde_json::json!({}),
        }
    }
}

/// Run the AUTO schedule with the law that gets `wrong` wrong.
fn run(wrong: Wrong, cfg: ClassicalTestConfig) -> Svc {
    let clock = Arc::new(TestClock::default());
    let mut s = service(cfg, &format!("{wrong:?}"))
        .with_law_factory(Arc::new(AdversaryFactory {
            wrong,
            clock: Arc::clone(&clock),
        }))
        .with_law_clock(clock);
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    let st = s.identify().unwrap();
    if st == RunState::Firing {
        s.run_auto(100).unwrap();
    }
    s
}

/// Run the AUTO schedule with the built-in law.
fn run_builtin(cfg: ClassicalTestConfig, tag: &str) -> Svc {
    let mut s = service(cfg, tag);
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    if s.identify().unwrap() == RunState::Firing {
        s.run_auto(100).unwrap();
    }
    s
}

fn law_fault(s: &Svc) -> Option<(LawFault, u32)> {
    match &s.trip()?.kind {
        TripKind::LawFault { fault, consecutive } => Some((fault.clone(), *consecutive)),
        _ => None,
    }
}

/// Every pulse that played had a finite drive under the abort.
fn nothing_bad_played(s: &Svc, abort: f64) {
    for row in s.report_data().pulses {
        let v = row.drive_vpk.expect("a closed-loop row has its drive peak");
        assert!(
            v.is_finite() && v <= abort,
            "pulse {} played {v} V",
            row.pulse
        );
    }
}

fn misses_end_the_run(wrong: Wrong, reason: &str) {
    let s = run(wrong, config(0.6, 0.8));
    let (fault, n) = law_fault(&s).unwrap_or_else(|| panic!("{wrong:?}: {:?}", s.trip()));
    assert_eq!((fault.reason(), n), (reason, 3), "{wrong:?}");
    assert_eq!(s.state(), RunState::Aborted, "{wrong:?}");
    // The seed's drive and two refires of it: the third miss ends the run.
    assert_eq!(s.report_data().pulses.len(), 3, "{wrong:?}");
    let d0 = s.pulse_view(0).unwrap().drive;
    for p in 1..3 {
        let row = &s.report_data().pulses[p];
        let call = row.law.as_ref().expect("the row says which call it played");
        assert!(
            !call.used,
            "{wrong:?}: pulse {p} refired the drive in force"
        );
        assert_eq!(call.miss.as_ref().map(|f| f.reason()), Some(reason));
        // In force: the same full-level drive, at this pulse's level.
        let d = s.pulse_view(p as u64).unwrap().drive;
        let k = d.iter().fold(0.0f64, |m, v| m.max(v.abs()))
            / d0.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        for (a, b) in d.iter().zip(&d0) {
            assert!(
                (a - k * b).abs() < 1e-9,
                "{wrong:?}: pulse {p} is not the drive in force"
            );
        }
    }
    nothing_bad_played(&s, 0.8);
    let law = s.report_data().law.unwrap();
    assert_eq!((law.misses, law.calls), (3, 4), "{wrong:?}");
}

#[test]
fn a_drive_that_is_not_a_number_is_refused_and_never_plays() {
    misses_end_the_run(Wrong::Nan, "non_finite");
}

#[test]
fn a_short_record_is_refused() {
    misses_end_the_run(Wrong::Short, "wrong_length");
}

#[test]
fn a_bin_that_jumps_twenty_db_is_refused() {
    misses_end_the_run(Wrong::BinJump, "bin_rise");
    let s = run(Wrong::BinJump, config(0.6, 0.8));
    assert!(s.report_data().law.unwrap().capped_bins >= 3);
}

#[test]
fn a_peak_that_runs_away_is_refused() {
    misses_end_the_run(Wrong::PeakRunaway, "peak_rise");
}

#[test]
fn a_slow_law_on_the_test_clock_is_over_budget() {
    misses_end_the_run(Wrong::Slow, "over_budget");
    let s = run(Wrong::Slow, config(0.6, 0.8));
    match law_fault(&s).unwrap().0 {
        LawFault::OverBudget { took_s, budget_s } => {
            assert!((took_s - 0.3).abs() < 1e-9 && (budget_s - 0.25).abs() < 1e-12)
        }
        f => panic!("{f:?}"),
    }
}

#[test]
fn a_panic_is_contained_and_counted() {
    misses_end_the_run(Wrong::Panic, "panicked");
}

#[test]
fn a_first_drive_three_times_the_reference_fires_nothing() {
    let s = run(Wrong::FirstDriveTriple, config(0.6, 0.8));
    let (fault, n) = law_fault(&s).expect("a law fault");
    assert_eq!(fault, LawFault::BadOutput(OutputRule::FirstDrive));
    assert_eq!(n, 1);
    assert!(s.report_data().pulses.is_empty(), "nothing fired");
}

#[test]
fn every_drive_starts_and_ends_at_rest_whatever_the_law_returns() {
    let s = run(Wrong::NotAtRest, config(0.6, 0.8));
    let pulses = s.report_data().pulses.len();
    assert!(
        pulses >= 7,
        "the run went on: {pulses} pulses, {:?}",
        s.trip()
    );
    for p in 0..pulses as u64 {
        let d = s.pulse_view(p).unwrap().drive;
        assert_eq!((d[0], d[d.len() - 1]), (0.0, 0.0), "pulse {p}");
    }
}

#[test]
fn an_overshoot_of_half_aborts_at_the_first_pulse() {
    let s = run(Wrong::Overshoot, config(0.6, 0.8));
    match &s.trip().expect("a trip").kind {
        TripKind::Overshoot {
            pulse,
            peak_ratio,
            limit_ratio,
        } => {
            assert_eq!(*pulse, 0);
            assert!((peak_ratio - 1.5).abs() < 0.02, "{peak_ratio}");
            assert!((limit_ratio - 1.3).abs() < 1e-12);
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(s.report_data().pulses.len(), 1, "no second pulse");
}

#[test]
fn a_law_that_never_corrects_ends_not_converged_at_the_first_rung() {
    let s = run(Wrong::NeverCorrects, config(0.6, 0.8));
    assert_eq!(s.state(), RunState::NotConverged);
    match &s.trip().expect("a trip").kind {
        TripKind::NotConverged { pulses, .. } => assert_eq!(*pulses, 5),
        k => panic!("{k:?}"),
    }
    let rows = s.report_data().pulses;
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|r| r.scheduled_level_db == -12.0));
    assert!(rows
        .iter()
        .all(|r| (r.peak_ratio.unwrap() - 0.5).abs() < 0.02));
}

#[test]
fn a_law_that_diverges_trips_divergence() {
    let s = run(Wrong::Diverges, config(0.6, 0.8));
    match &s.trip().expect("a trip").kind {
        TripKind::Divergence { pulses, .. } => assert_eq!(*pulses, 3),
        k => panic!("{k:?}"),
    }
    assert_eq!(s.state(), RunState::Aborted);
}

#[test]
fn a_drive_past_the_abort_is_refused_before_it_plays() {
    let mut cfg = config(0.6, 0.8);
    cfg.schedule = ShockLevelSchedule {
        start_db: 0.0,
        ..ShockLevelSchedule::manual_default()
    };
    let clock = Arc::new(TestClock::default());
    let mut s = service(cfg, "abort")
        .with_law_factory(Arc::new(AdversaryFactory {
            wrong: Wrong::PastTheAbort,
            clock: Arc::clone(&clock),
        }))
        .with_law_clock(clock);
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.identify().unwrap(), RunState::Firing);
    assert!(s.fire_next_pulse().unwrap(), "0.5 V plays");
    assert!(!s.fire_next_pulse().unwrap(), "0.9 V does not");
    match &s.trip().expect("a trip").kind {
        TripKind::DriveVpk {
            vpk,
            abort_vpk,
            predicted,
            ..
        } => {
            assert!(
                (vpk - 0.9).abs() < 0.02 && *abort_vpk == 0.8 && !predicted,
                "{vpk}"
            );
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(s.report_data().pulses.len(), 1);
    nothing_bad_played(&s, 0.8);
}

#[test]
fn a_predicted_drive_over_the_abort_ends_the_run_before_any_pulse() {
    let s = run_builtin(config(0.4, 0.45), "predicted-abort");
    match &s.trip().expect("a trip").kind {
        TripKind::DriveVpk {
            pulse,
            vpk,
            abort_vpk,
            predicted,
        } => {
            assert_eq!(*pulse, 0);
            assert!(*predicted);
            assert!((vpk - 0.5).abs() < 0.01 && *abort_vpk == 0.45, "{vpk}");
        }
        k => panic!("{k:?}"),
    }
    assert!(s.report_data().pulses.is_empty());
}

#[test]
fn a_drive_over_the_ceiling_is_scaled_to_it_and_judged_as_commanded() {
    let s = run_builtin(config(0.475, 0.6), "ceiling");
    assert_eq!(s.state(), RunState::Stopped, "{:?}", s.trip());
    let report = s.report_data();
    let full: Vec<_> = report
        .pulses
        .iter()
        .filter(|r| r.scheduled_level_db == 0.0)
        .collect();
    assert_eq!(full.len(), 3);
    for r in full {
        assert!((r.drive_vpk.unwrap() - 0.475).abs() < 1e-9, "{r:?}");
        assert!(r.drive_limit_scale < 1.0);
        assert_eq!(r.tolerance_overall, Some(true), "5% low passes: {r:?}");
    }
    assert!(report
        .drive_limit_actions
        .iter()
        .any(|a| a.phase == "drive-ceiling"));
}

#[test]
fn a_level_move_past_the_abort_is_refused_with_both_numbers() {
    let mut cfg = config(0.4, 0.45);
    cfg.schedule = ShockLevelSchedule {
        start_db: -3.0,
        mode: ShockRunMode::Manual,
        ..ShockLevelSchedule::manual_default()
    };
    let mut s = service(cfg, "level");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.identify().unwrap(), RunState::Firing);
    assert!(s.fire_next_pulse().unwrap());
    match s.jump_to_level(4) {
        Err(ServiceError::DriveCeiling {
            level_db,
            vpk,
            abort_vpk,
        }) => {
            assert_eq!(level_db, 0.0);
            assert!((vpk - 0.5).abs() < 0.01 && abort_vpk == 0.45, "{vpk}");
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(s.current_level_db(), -3.0, "the level did not move");
    s.step_down().unwrap();
    s.stop().unwrap();
}

#[test]
fn predicted_kinematics_over_the_shaker_stop_the_first_full_level_pulse() {
    // X0: the full-level predicted pk-pk displacement at proceed, from a run
    // with no shaker (the number stated before a bench run); then a shaker
    // rated at 0.9 X0.
    let mut s = service(config(0.6, 0.8), "x0");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.identify().unwrap(), RunState::Firing);
    let x0 = s.status().prediction.unwrap().displacement_pk_pk_in;
    s.stop().unwrap();
    let mut cfg = config(0.6, 0.8);
    let mut shaker = ShakerRecord::new("short stroke");
    shaker.force_shock = Some(RatingValue::peak(1e6));
    shaker.max_velocity_ips = Some(RatingValue::peak(1e3));
    shaker.max_displacement_in_pp = Some(RatingValue::peak(0.9 * x0));
    cfg.shaker = Some(shaker);
    let mut s = service(cfg, "kinematic");
    let feas = s.assess_feasibility().unwrap();
    if feas.exceeded() {
        s.arm_with_override("predicted kinematics test").unwrap();
    } else {
        s.arm().unwrap();
    }
    assert_eq!(s.identify().unwrap(), RunState::Firing);
    s.run_auto(100).unwrap();
    match &s.trip().expect("a trip").kind {
        TripKind::Kinematic {
            quantity,
            value,
            predicted,
            ..
        } => {
            assert!(predicted);
            assert!(quantity.contains("displacement"));
            assert!((value / x0 - 1.0).abs() < 0.03, "{value} against {x0}");
        }
        k => panic!("{k:?}"),
    }
    let rows = s.report_data().pulses;
    assert_eq!(
        rows.last().unwrap().scheduled_level_db,
        -3.0,
        "the last pulse fired"
    );
}
