//! Every guard on the station, each with a law that provokes it: answer
//! checks and the miss rule, the time budget, the change and rise caps and
//! divergence, the rotation cap, the first-drive bound, the drive ceiling,
//! the control abort, the kinematic check, a run-thread failure, the stop
//! ramp, and a law refused for a test type it does not accept.

mod common;

use common::*;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::core::{CoreState, ShakerRating};
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup, LawValues};
use hertzforge_mimo_sine::records::{EventKind, Outcome};
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService, ServiceError, ServiceState};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::law::LawClock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// How the misbehaving law misbehaves, from its `from`-th tick on.
#[derive(Clone, Copy, Debug)]
enum Mode {
    WrongCount,
    NonFinite,
    Panic,
    /// Ask this many times the drive in force every tick.
    Grow(f64),
    /// Turn every drive's phase by this many degrees every tick.
    Turn(f64),
    /// Engage at this many times `matrix`'s first drive.
    BigFirst(f64),
    /// Hold the drive at this share of `matrix`'s.
    Low(f64),
    /// Sleep past the budget (with the stepping clock).
    Slow,
    /// Take this many seconds on the stepping clock every tick.
    SlowBy(f64),
}

struct Bad {
    inner: Arc<dyn MimoSineLawFactory>,
    mode: Mode,
    from: u64,
    clock: Option<Arc<StepClock>>,
}

struct BadLaw {
    inner: Box<dyn MimoSineLaw + Send>,
    mode: Mode,
    from: u64,
    clock: Option<Arc<StepClock>>,
}

impl MimoSineLaw for BadLaw {
    fn name(&self) -> &str {
        "bad"
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.inner.engage(x, o, v)?;
        if let Mode::BigFirst(k) = self.mode {
            o.drive.iter_mut().for_each(|d| *d *= k);
        }
        Ok(())
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.inner.tick(x, o, v)?;
        if x.index < self.from {
            return Ok(());
        }
        match self.mode {
            Mode::WrongCount => {
                o.drive.pop();
            }
            Mode::NonFinite => o.drive[1] = C64::new(f64::NAN, 0.0),
            Mode::Panic => panic!("a law that panics"),
            Mode::Grow(k) => {
                for (d, f) in o.drive.iter_mut().zip(x.drive_in_force) {
                    *d = f * k;
                }
            }
            Mode::Turn(deg) => {
                for (d, f) in o.drive.iter_mut().zip(x.drive_in_force) {
                    *d = f * C64::from_polar(1.0, deg.to_radians());
                }
            }
            Mode::Low(k) => o.drive.iter_mut().for_each(|d| *d *= k),
            Mode::Slow => {
                if let Some(c) = &self.clock {
                    c.advance(1.0);
                }
            }
            Mode::SlowBy(s) => {
                if let Some(c) = &self.clock {
                    c.advance(s);
                }
            }
            Mode::BigFirst(_) => {}
        }
        Ok(())
    }
}

impl MimoSineLawFactory for Bad {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(BadLaw {
            inner: self.inner.build(s)?,
            mode: self.mode,
            from: self.from,
            clock: self.clock.clone(),
        }))
    }
    fn describe(&self) -> LawDescriptor {
        self.inner.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.inner.manifest()
    }
}

/// A clock a law can push forward, to be late without sleeping.
#[derive(Default)]
struct StepClock(Mutex<f64>);

impl StepClock {
    fn advance(&self, s: f64) {
        *self.0.lock().unwrap() += s;
    }
}

impl LawClock for StepClock {
    fn now_s(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

fn bad(mode: Mode, from: u64) -> Arc<dyn MimoSineLawFactory> {
    Arc::new(Bad {
        inner: law("matrix"),
        mode,
        from,
        clock: None,
    })
}

fn run_bad(mode: Mode, from: u64, tweak: impl FnOnce(&mut MimoSineSetup)) -> Run {
    let s = SineStructure::Linear4x4;
    let mut st = setup(s, dwell(96.0, 8.0), "matrix");
    tweak(&mut st);
    run(engaged(s, st, bad(mode, from), NOISE), 12.0)
}

fn misses(r: &Run) -> Vec<&'static str> {
    r.ticks.iter().filter_map(|t| t.miss).collect()
}

/// A refused answer is a miss with the drive in force held; three in a row
/// take the controlled stop.
fn three_misses_stop(mode: Mode, reason: &str) {
    let r = run_bad(mode, 150, |_| {});
    assert_eq!(r.outcome, Some(Outcome::LawFault));
    assert_eq!(misses(&r), vec![reason; 3]);
    let held: Vec<_> = r.ticks.iter().filter(|t| t.miss.is_some()).collect();
    for t in held {
        assert_eq!(
            t.drives(),
            &t.drive_in_force[..4],
            "the drive in force is held"
        );
    }
    assert!(r.events.iter().any(|e| matches!(
        e.kind,
        EventKind::Stopping {
            outcome: Outcome::LawFault
        }
    )));
    assert!(r.service.core().unwrap().drive_is_zero());
}

#[test]
fn mimo_sine_a_wrong_drive_count_is_refused_three_times_then_the_run_stops() {
    three_misses_stop(Mode::WrongCount, "wrong_drive_count");
}

#[test]
fn mimo_sine_a_non_finite_drive_is_refused_three_times_then_the_run_stops() {
    three_misses_stop(Mode::NonFinite, "non_finite");
}

#[test]
fn mimo_sine_a_panicking_law_is_caught_per_call_and_the_run_stops() {
    three_misses_stop(Mode::Panic, "panicked");
}

#[test]
fn mimo_sine_a_law_over_its_budget_three_times_ends_the_run() {
    let s = SineStructure::Linear4x4;
    let clock = Arc::new(StepClock::default());
    let f = Arc::new(Bad {
        inner: law("matrix"),
        mode: Mode::Slow,
        from: 150,
        clock: Some(clock.clone()),
    });
    let mut svc = MimoSineService::new(
        setup(s, dwell(96.0, 8.0), "matrix"),
        table(s),
        backend(s, NOISE, 7),
        f,
        EngineOptions {
            clock: Some(clock),
            ..EngineOptions::new(StationClass::HertzforgeSim)
        },
    )
    .unwrap();
    svc.arm().unwrap();
    svc.identify().unwrap();
    svc.engage().unwrap();
    let r = run(svc, 12.0);
    assert_eq!(r.outcome, Some(Outcome::LawFault));
    assert_eq!(misses(&r), vec!["over_budget"; 3]);
}

/// A run of a law 0.1 s late on every update from the 150th, on `class`
/// with the law budget `relaxed` (or the setup's).
fn late_run(class: StationClass, relaxed: Option<std::time::Duration>) -> (Run, f64, f64) {
    let s = SineStructure::Linear4x4;
    let clock = Arc::new(StepClock::default());
    let f = Arc::new(Bad {
        inner: law("matrix"),
        mode: Mode::SlowBy(0.1),
        from: 150,
        clock: Some(clock.clone()),
    });
    let mut svc = MimoSineService::new(
        setup(s, dwell(96.0, 4.0), "matrix"),
        table(s),
        backend(s, NOISE, 7),
        f,
        EngineOptions {
            clock: Some(clock),
            law_budget: relaxed,
            ..EngineOptions::new(class)
        },
    )
    .unwrap();
    svc.arm().unwrap();
    svc.identify().unwrap();
    svc.engage().unwrap();
    let core = svc.core().unwrap();
    let budgets = (core.law_budget_s(), core.setup_law_budget_s());
    (run(svc, 8.0), budgets.0, budgets.1)
}

#[test]
fn mimo_sine_a_relaxed_law_budget_on_the_simulator_takes_a_late_answer() {
    // The harness's budget, by name: a law 0.1 s late every update (40 times
    // the setup's 2.5 ms) is answered on time and the run completes.
    let (r, in_force, setup) = late_run(
        StationClass::HertzforgeSim,
        Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
    );
    assert_eq!(in_force, 1.0);
    assert!((setup - 0.25 * 0.01).abs() < 1e-4, "{setup}");
    assert!(misses(&r).is_empty(), "{:?}", misses(&r));
    assert_eq!(r.outcome, Some(Outcome::Completed));
    assert!(r.ticks.iter().any(|t| t.law_s >= 0.1));
    // Without it, the setup's budget refuses the same answers.
    let (r, in_force, _) = late_run(StationClass::HertzforgeSim, None);
    assert_eq!(in_force, setup);
    assert_eq!(r.outcome, Some(Outcome::LawFault));
    assert_eq!(misses(&r), vec!["over_budget"; 3]);
}

#[test]
fn mimo_sine_a_hardware_station_class_keeps_the_setup_law_budget() {
    use hertzforge_mimo_sine::service::law_budget_s;
    let relaxed = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);
    assert_eq!(
        law_budget_s(StationClass::HertzforgeSim, relaxed),
        Some(1.0)
    );
    assert_eq!(law_budget_s(StationClass::HertzforgeSim, None), None);
    for class in [StationClass::NiSimulated, StationClass::NiReal] {
        assert_eq!(law_budget_s(class, relaxed), None, "{class:?}");
    }
}

#[test]
fn mimo_sine_a_drive_growing_past_the_caps_is_capped_and_trips_divergence() {
    // +20 dB asked every update: each drive moves 1 dB an update, and the cap
    // binding for a second without a break stops the run.
    let r = run_bad(Mode::Grow(10.0), 150, |st| {
        for c in &mut st.controls {
            c.profile = flat(1.0, 1.5, 40.0);
        }
    });
    assert_eq!(r.outcome, Some(Outcome::Divergence));
    let capped: Vec<_> = r.ticks.iter().filter(|t| t.cap_bound).collect();
    assert!(
        capped.len() >= 99 && capped.len() <= 102,
        "{}",
        capped.len()
    );
    for t in &capped {
        for (d, f) in t.drives().iter().zip(&t.drive_in_force) {
            let step = 20.0 * (d.norm() / f.norm()).log10();
            assert!(step <= 1.0 + 1e-6, "{step}");
        }
    }
}

#[test]
fn mimo_sine_a_drive_over_its_ceiling_scales_the_whole_vector() {
    // +2.9 dB an update, under the 3 dB cap: the drive reaches its ceiling and
    // the whole vector is held there, direction kept, every sample under it.
    let r = run_bad(Mode::Grow(1.4), 150, |st| {
        st.guards.max_step_db = 3.0;
        st.guards.max_rise_db_per_s = 10000.0;
        st.guards.abort_ticks = 100;
        for c in &mut st.controls {
            c.profile = flat(1.0, 1.5, 60.0);
        }
        for d in &mut st.drives {
            d.ceiling_vpk = 0.5;
            d.abort_vpk = 4.0;
        }
    });
    let over: Vec<_> = r.ticks.iter().filter(|t| t.ceiling_scale < 1.0).collect();
    assert!(!over.is_empty());
    for t in &over {
        let largest = t.drives().iter().map(|d| d.norm()).fold(0.0, f64::max);
        assert!((largest - 0.5).abs() < 1e-9, "{largest}");
        // Direction kept: the drive played is the asked drive scaled.
        let asked = &t.drive_asked[..4];
        let k = t.drives()[0] / asked[0];
        for (d, a) in t.drives().iter().zip(asked) {
            assert!((d - a * k).norm() < 1e-9);
        }
    }
    assert!(r.service.core().unwrap().largest_sample_v() <= 0.5 + 1e-6);
    assert!(r.service.core().unwrap().over_ceiling_count() > 0);
    assert!(r
        .events
        .iter()
        .any(|e| matches!(e.kind, EventKind::OverCeiling { .. })));
}

#[test]
fn mimo_sine_the_rotation_cap_is_off_by_default_records_the_turn_and_binds_when_set() {
    let r = run_bad(Mode::Turn(30.0), 150, |st| {
        st.guards.abort_ticks = 100;
        for c in &mut st.controls {
            c.profile = flat(1.0, 1.5, 60.0);
        }
    });
    let t = r.ticks.iter().find(|t| t.index == 160).unwrap();
    assert!((t.largest_rotation_deg - 30.0).abs() < 1e-6);
    assert!(!t.rotation_bound);
    let r = run_bad(Mode::Turn(30.0), 150, |st| {
        st.guards.rotation_cap_deg = Some(5.0);
        st.guards.abort_ticks = 100;
        for c in &mut st.controls {
            c.profile = flat(1.0, 1.5, 60.0);
        }
    });
    let t = r.ticks.iter().find(|t| t.index == 160).unwrap();
    assert!(t.rotation_bound);
    let turn = (t.drives()[0] / t.drive_in_force[0]).arg().to_degrees();
    assert!((turn - 5.0).abs() < 1e-6, "{turn}");
    // Bound every update: divergence after a second.
    assert_eq!(r.outcome, Some(Outcome::Divergence));
}

#[test]
fn mimo_sine_a_first_drive_over_its_bound_is_scaled_to_it() {
    let r = run_bad(Mode::BigFirst(100.0), u64::MAX, |_| {});
    let first = &r.ticks[0];
    assert!(first.engage);
    // The bound is the inverse drive plus 6 dB; the law asked 40 dB over it.
    assert!(
        (first.first_drive_scale - 0.02).abs() < 1e-3,
        "{}",
        first.first_drive_scale
    );
    let asked: f64 = first.drive_asked[..4]
        .iter()
        .map(|d| d.norm_sqr())
        .sum::<f64>()
        .sqrt();
    let played: f64 = first
        .drives()
        .iter()
        .map(|d| d.norm_sqr())
        .sum::<f64>()
        .sqrt();
    assert!((played / asked - first.first_drive_scale).abs() < 1e-12);
}

#[test]
fn mimo_sine_a_control_outside_its_abort_band_ends_the_run_and_ramps_out() {
    // From the 150th update the law holds the drive 10 dB low.
    let r = run_bad(Mode::Low(0.0), 150, |_| {});
    match r.outcome {
        Some(Outcome::ControlAbort { .. }) => {}
        o => panic!("{o:?}"),
    }
    assert!(r.service.core().unwrap().drive_is_zero());
    assert_eq!(r.service.state(), ServiceState::Ended(r.outcome.unwrap()));
}

#[test]
fn mimo_sine_a_control_past_its_shakers_rating_ends_the_run() {
    let s = SineStructure::Linear4x4;
    let st = setup(s, dwell(96.0, 8.0), "matrix");
    let mut opts = EngineOptions::new(StationClass::HertzforgeSim);
    // 1 g at 96 Hz is 16.3 mm/s: rate the shaker under control 2 at 10.
    opts.ratings = vec![ShakerRating {
        control: 2,
        max_displacement_mm_pk: None,
        max_velocity_m_s_pk: Some(0.010),
        max_acceleration_g_pk: None,
    }];
    let mut svc =
        MimoSineService::new(st, table(s), backend(s, NOISE, 7), law("matrix"), opts).unwrap();
    svc.arm().unwrap();
    svc.identify().unwrap();
    svc.engage().unwrap();
    let r = run(svc, 12.0);
    assert_eq!(r.outcome, Some(Outcome::Kinematic { control: 2 }));
}

#[test]
fn mimo_sine_a_law_is_refused_for_a_test_type_its_manifest_does_not_accept() {
    let s = SineStructure::Linear4x4;
    let st = setup(s, common_sweep(), "paired");
    let e = MimoSineService::new(
        st,
        table(s),
        backend(s, 0.0, 7),
        law("paired"),
        EngineOptions::new(StationClass::HertzforgeSim),
    );
    match e {
        Err(ServiceError::Setup(e)) => assert_eq!(e.rule, SetupRule::LawTestTypeNotAccepted),
        _ => panic!("a sweep under paired was not refused"),
    }
}

fn common_sweep() -> TestPlan {
    TestPlan::Sweep(hertzforge_sine::scheduler::BidirSweepConfig {
        band: (40.0, 400.0),
        rate: hertzforge_sine::SweepRate::LogOctPerMin(1.0),
        start_hz: 40.0,
        initial_direction: hertzforge_sine::scheduler::SweepDirection::Up,
        target: hertzforge_sine::scheduler::SweepTarget::Sweeps(1),
    })
}

/// A station that fails on its `after`-th read, and remembers an emergency
/// stop.
struct Failing {
    inner: Box<dyn Backend>,
    reads: Arc<AtomicU32>,
    after: u32,
    estop: Arc<AtomicBool>,
}

impl Backend for Failing {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        t: &hertzforge_channel_table::ChannelTable,
        a: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(t, a)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == self.after {
            panic!("the run thread fails");
        }
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(d)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.estop.store(true, Ordering::SeqCst);
        self.inner.emergency_stop()
    }
}

#[test]
fn mimo_sine_a_run_thread_failure_stops_the_station_at_once() {
    let s = SineStructure::Linear4x4;
    let reads = Arc::new(AtomicU32::new(0));
    let estop = Arc::new(AtomicBool::new(false));
    let b = Failing {
        inner: backend(s, NOISE, 7),
        reads: reads.clone(),
        after: u32::MAX,
        estop: estop.clone(),
    };
    let mut svc = MimoSineService::new(
        setup(s, dwell(96.0, 8.0), "matrix"),
        table(s),
        Box::new(b),
        law("matrix"),
        EngineOptions::new(StationClass::HertzforgeSim),
    )
    .unwrap();
    svc.arm().unwrap();
    svc.identify().unwrap();
    svc.engage().unwrap();
    for _ in 0..50 {
        svc.run_batches(1).unwrap();
    }
    assert!(!estop.load(Ordering::SeqCst));
    // The next read panics: the batch is caught, the station stopped at once,
    // and the run is over with nothing to resume.
    let n = reads.load(Ordering::SeqCst);
    let _ = n;
    // Swap in the failure by asking for it on the read after this one.
    // (`Failing::after` is fixed at build; a second service shows the path.)
    let reads2 = Arc::new(AtomicU32::new(0));
    let estop2 = Arc::new(AtomicBool::new(false));
    let b2 = Failing {
        inner: backend(s, NOISE, 7),
        reads: reads2.clone(),
        after: n,
        estop: estop2.clone(),
    };
    let mut svc2 = MimoSineService::new(
        setup(s, dwell(96.0, 8.0), "matrix"),
        table(s),
        Box::new(b2),
        law("matrix"),
        EngineOptions::new(StationClass::HertzforgeSim),
    )
    .unwrap();
    svc2.arm().unwrap();
    svc2.identify().unwrap();
    svc2.engage().unwrap();
    let mut failed = None;
    for _ in 0..200 {
        match svc2.run_batches(1) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    assert!(failed.is_some(), "the failure surfaced");
    assert!(
        estop2.load(Ordering::SeqCst),
        "the station was stopped at once"
    );
    assert_eq!(svc2.state(), ServiceState::Ended(Outcome::RunThreadFailure));
    assert!(!svc2.run_batches(1).unwrap());
}

#[test]
fn mimo_sine_an_operator_stop_walks_every_drive_down_and_stops_the_station() {
    let s = SineStructure::Linear4x4;
    let mut svc = engaged(
        s,
        setup(s, dwell(96.0, 30.0), "matrix"),
        law("matrix"),
        NOISE,
    );
    for _ in 0..(3.0 * FS / 512.0) as usize {
        svc.run_batches(1).unwrap();
    }
    let before = svc.core().unwrap().drive_in_force().to_vec();
    assert!(before.iter().all(|d| d.norm() > 0.0));
    svc.request_stop();
    let mut k = 0;
    while svc.run_batches(1).unwrap() {
        k += 1;
        assert!(k < 1000);
    }
    assert_eq!(svc.state(), ServiceState::Ended(Outcome::Stopped));
    assert!(matches!(
        svc.core().unwrap().state(),
        CoreState::Ended(Outcome::Stopped)
    ));
    // 120 dB at 120 dB a second: the ramp lasts about a second, never a cliff.
    let ramp_s = k as f64 * 512.0 / FS;
    assert!(ramp_s > 0.9, "{ramp_s}");
}
