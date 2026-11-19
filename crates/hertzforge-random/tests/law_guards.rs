//! The core's guards on the control law: the law proposes, the core disposes.
//!
//! Every test here injects a law through the service's law factory and closes the
//! loop through the real `RandomService` against the Sim plant. The misbehaving
//! laws wrap the shipped `HinvLaw` so a run seeds and engages normally, then
//! misbehave on every loop (`iterate`):
//!
//! - a law returning NaN,
//! - a law returning a ragged (wrong length) or negative drive,
//! - a runaway law (multiplies its drive by 10 every loop),
//! - an oscillating law (alternates x10 and /10 of its base drive),
//! - a slow law (advances the injected clock past its time budget),
//! - a panicking law.
//!
//! For each: the drive the core synthesized never exceeded the drive ceiling and
//! never held a non-finite value, no panic escaped `loop_tick`, and the run ended
//! in the expected way within a bounded number of loops. The first four misses
//! end in the law-fault controlled stop; the runaway and oscillating laws end in
//! the core's divergence abort.
//!
//! Two more pin the per-loop change cap on the shipped law: it never binds on a
//! full ladder (soft start, ramps, rungs, a controlled stop), and a commanded
//! level step larger than the cap passes through whole (the cap's reference is
//! rescaled by the command, not clamped against it).
//!
//! No test here rests on the wall clock: the slow law advances an injected clock.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_random::control::{
    ControlError, ControlLaw, HinvLaw, LawFault, LevelCommand, LoopInputs, LoopOutputs,
};
use hertzforge_random::service::config::{LevelSchedule, RandomTestConfig};
use hertzforge_random::service::report::RunState;
use hertzforge_random::service::RandomService;
use hertzforge_random::{LawClock, LawFactory, LawFaultReason, LawSetup, LoopConfig, TripKind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ===========================================================================
// Shared builders (the acceptance suite's demo plant)
// ===========================================================================

/// The Sim plant the acceptance suite runs on: two in-band modes, healthy
/// feedthrough.
fn backend(cfg: &LoopConfig) -> Box<dyn hertzforge_hal::Backend> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(cfg.fs_hz)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .noise_level(2e-4)
            .feedthrough(0.35)
            .shared_clock(true)
            .seed(0x5A5A_1234)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
    )
}

/// Control on channel 1 (accel g), drive on channel 2.
fn siso_table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    ctrl.abort.accel_g = Some(500.0);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

/// The decoded demo profile (20/0.01, 80/0.04, 350/0.04, 2000/0.007).
fn demo_profile() -> BreakpointTable {
    let mut bt = BreakpointTable::new("demo-profile");
    bt.push(BreakpointRow::psd(20.0, 0.01));
    bt.push(BreakpointRow::psd(80.0, 0.04));
    bt.push(BreakpointRow::psd(350.0, 0.04));
    bt.push(BreakpointRow::psd(2000.0, 0.007));
    bt
}

const CEILING_VRMS: f64 = 10.0;

/// A MANUAL run with the envelope out of the way (the guards, not the envelope,
/// are under test), a shutdown ramp so a controlled stop really ramps, and
/// generous everything else.
fn manual_cfg() -> RandomTestConfig {
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = CEILING_VRMS;
    cfg.alarm_db = (48.0, 48.0);
    cfg.abort_db = (60.0, 60.0);
    cfg.shutdown_ramp_s = 2.0;
    cfg
}

fn service(cfg: RandomTestConfig, factory: Arc<dyn LawFactory>) -> RandomService {
    let loop_cfg = LoopConfig::demo_default();
    let b = backend(&loop_cfg);
    let mut svc = RandomService::new(
        cfg,
        siso_table(),
        loop_cfg,
        b,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(std::env::temp_dir().join("hertzforge-law-guards")),
    )
    .expect("service builds")
    .with_law_factory(factory);
    svc.set_review_after_sysid(false);
    svc.set_soft_start_db(0.0);
    svc
}

/// Feasibility, arm, identification and the gate: the run is engaged at its
/// start level when this returns.
fn engage(svc: &mut RandomService) {
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"), "the gate passes");
    assert_eq!(svc.state(), RunState::Engaged);
}

/// What one guarded run did.
struct Ran {
    /// Ticks taken before the run ended (or the cap was reached).
    ticks: usize,
    /// Every drive PSD the core synthesized, one per tick.
    drives: Vec<Vec<f64>>,
}

/// Tick the run until it ends or `max_ticks` is reached, checking after every
/// tick that the drive the core synthesized is finite, non-negative and at or
/// below the drive ceiling. A panic out of `loop_tick` fails the test (it is not
/// caught here: "no panic escaped" is that the test completes).
fn run(svc: &mut RandomService, max_ticks: usize) -> Ran {
    let df = svc.realized_df_hz();
    let mut drives = Vec::new();
    let mut ticks = 0;
    while ticks < max_ticks {
        let more = svc.loop_tick().expect("loop_tick returns Ok");
        ticks += 1;
        if let Some(d) = svc.last_drive_psd() {
            assert!(
                d.iter().all(|p| p.is_finite() && *p >= 0.0),
                "tick {ticks}: the synthesized drive holds a non-finite or negative line"
            );
            let vrms = (d.iter().sum::<f64>() * df).sqrt();
            assert!(
                vrms <= CEILING_VRMS * (1.0 + 1e-9),
                "tick {ticks}: drive {vrms} V rms over the {CEILING_VRMS} V rms ceiling"
            );
            drives.push(d.to_vec());
        }
        if !more {
            break;
        }
    }
    Ran { ticks, drives }
}

/// The run ended in the law-fault controlled stop: `Stopped` (not `Aborted`), a
/// `LawFault` trip with `reason` after the configured misses in a row, the stop
/// went the ramped way, and the drive in force was held (unchanged) through
/// every miss.
fn assert_law_fault_stop(svc: &RandomService, ran: &Ran, reason: LawFaultReason) {
    println!(
        "ended {:?} after {} ticks, trip {:?}, stats {:?}",
        svc.state(),
        ran.ticks,
        svc.trip(),
        svc.law_guard_stats()
    );
    assert_eq!(
        svc.state(),
        RunState::Stopped,
        "a law fault takes the controlled stop, not an abort"
    );
    let trip = svc.trip().expect("the law fault is recorded");
    assert_eq!(
        trip.kind,
        TripKind::LawFault {
            reason,
            consecutive: 3
        }
    );
    assert!(
        svc.stop_requested_db().is_some(),
        "the stop took the shutdown ramp, as an operator stop does"
    );
    // Two settle loops, three misses, then the ramp-down (2 s at 1.5 s a loop).
    assert!(ran.ticks <= 10, "ended within 10 loops, took {}", ran.ticks);
    // The drive in force was held through every miss: until the ramp-down
    // moved the level, every loop synthesized the same drive.
    let first = &ran.drives[0];
    for (i, d) in ran.drives.iter().take(5).enumerate() {
        assert_eq!(d, first, "tick {}: the drive in force changed", i + 1);
    }
}

// ===========================================================================
// The adversarial laws
// ===========================================================================

/// How a [`Wrapped`] law misbehaves on `iterate` (call `n`, from 1).
type Twist = Box<dyn FnMut(u64, LoopOutputs) -> Result<LoopOutputs, LawFault> + Send>;

/// The shipped law with a twist on every loop. Seeding and level changes are the
/// shipped law's own, so the run engages normally.
struct Wrapped {
    inner: HinvLaw,
    calls: u64,
    twist: Twist,
}

impl ControlLaw for Wrapped {
    fn init(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        self.inner.init(inputs)
    }
    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let out = self.inner.iterate(inputs)?;
        self.calls += 1;
        (self.twist)(self.calls, out)
    }
    fn retarget_level(&mut self, cmd: LevelCommand) -> Result<LoopOutputs, LawFault> {
        self.inner.retarget_level(cmd)
    }
    fn name(&self) -> &str {
        "wrapped"
    }
}

/// A factory for the shipped law with `make()`'s twist.
fn wrapped<M>(make: M) -> Arc<dyn LawFactory>
where
    M: Fn() -> Twist + Send + Sync + 'static,
{
    Arc::new(
        move |s: &LawSetup<'_>| -> Result<Box<dyn ControlLaw + Send>, ControlError> {
            let inner = HinvLaw::new(s.hinv.clone(), s.target_psd, s.band, s.df_hz)?;
            Ok(Box::new(Wrapped {
                inner,
                calls: 0,
                twist: make(),
            }))
        },
    )
}

/// A law that owns its drive outright: it takes the shipped law's seed and level
/// changes as its BASE, and on every loop proposes `gain(n) x` something.
struct Owned {
    inner: HinvLaw,
    band: (usize, usize),
    base: Vec<f64>,
    last: Vec<f64>,
    calls: u64,
    /// `true`: scale the last proposal (compounding); `false`: scale the base.
    compound: bool,
    gain: fn(u64) -> f64,
}

impl Owned {
    fn take_base(&mut self, out: LoopOutputs) -> LoopOutputs {
        self.base = out.drive_psd.clone();
        self.last = out.drive_psd.clone();
        out
    }
}

impl ControlLaw for Owned {
    fn init(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let out = self.inner.init(inputs)?;
        Ok(self.take_base(out))
    }
    fn iterate(&mut self, _inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        self.calls += 1;
        let g = (self.gain)(self.calls);
        let from = if self.compound {
            &self.last
        } else {
            &self.base
        };
        let (lo, hi) = self.band;
        let drive: Vec<f64> = from
            .iter()
            .enumerate()
            .map(|(k, &p)| if k >= lo && k < hi { p * g } else { p })
            .collect();
        self.last = drive.clone();
        Ok(LoopOutputs {
            drive_psd: drive,
            level_cmd_db: 0.0,
            diagnostics: Default::default(),
        })
    }
    fn retarget_level(&mut self, cmd: LevelCommand) -> Result<LoopOutputs, LawFault> {
        let out = self.inner.retarget_level(cmd)?;
        Ok(self.take_base(out))
    }
    fn name(&self) -> &str {
        "owned"
    }
}

fn owned(compound: bool, gain: fn(u64) -> f64) -> Arc<dyn LawFactory> {
    Arc::new(
        move |s: &LawSetup<'_>| -> Result<Box<dyn ControlLaw + Send>, ControlError> {
            let inner = HinvLaw::new(s.hinv.clone(), s.target_psd, s.band, s.df_hz)?;
            Ok(Box::new(Owned {
                inner,
                band: s.band,
                base: Vec::new(),
                last: Vec::new(),
                calls: 0,
                compound,
                gain,
            }))
        },
    )
}

/// A clock the test owns: it only moves when something advances it.
#[derive(Default)]
struct TestClock {
    nanos: AtomicU64,
}

impl TestClock {
    fn advance(&self, s: f64) {
        self.nanos.fetch_add((s * 1e9) as u64, Ordering::SeqCst);
    }
}

impl LawClock for TestClock {
    fn now_s(&self) -> f64 {
        self.nanos.load(Ordering::SeqCst) as f64 * 1e-9
    }
}

// ===========================================================================
// Malformed output, panics and slow calls: held, counted, controlled stop
// ===========================================================================

#[test]
fn a_law_returning_nan_is_held_and_the_run_stops_under_control() {
    let factory = wrapped(|| {
        Box::new(|_n, mut out: LoopOutputs| {
            for p in out.drive_psd.iter_mut().skip(20).take(5) {
                *p = f64::NAN;
            }
            Ok(out)
        })
    });
    let mut svc = service(manual_cfg(), factory);
    engage(&mut svc);
    let ran = run(&mut svc, 40);
    assert_law_fault_stop(&svc, &ran, LawFaultReason::NonFinite);
}

#[test]
fn a_law_returning_a_ragged_or_negative_drive_is_held_and_the_run_stops() {
    // Alternates: a drive one line short, then a drive with a negative line.
    let factory = wrapped(|| {
        Box::new(|n, mut out: LoopOutputs| {
            if n % 2 == 1 {
                out.drive_psd.pop();
            } else {
                out.drive_psd[30] = -out.drive_psd[30].abs() - 1e-6;
            }
            Ok(out)
        })
    });
    let mut svc = service(manual_cfg(), factory);
    engage(&mut svc);
    let ran = run(&mut svc, 40);
    // Misses 1 and 3 are ragged, miss 2 negative: the record names the last.
    assert_law_fault_stop(&svc, &ran, LawFaultReason::WrongLength);
    let log = format!("{:?}", svc.log().events());
    assert!(
        log.contains("negative value"),
        "the negative miss was logged too"
    );
}

#[test]
fn a_panicking_law_is_contained_and_the_run_stops_under_control() {
    let factory = wrapped(|| Box::new(|_n, _out| panic!("adversarial law: deliberate panic")));
    let mut svc = service(manual_cfg(), factory);
    engage(&mut svc);
    let ran = run(&mut svc, 40);
    assert_law_fault_stop(&svc, &ran, LawFaultReason::Panicked);
}

#[test]
fn a_slow_law_misses_its_budget_is_held_and_the_run_stops_under_control() {
    // The law's output is fine; it just takes ten seconds of the injected clock
    // on every loop, against a budget of a quarter of a 1.5 s acquisition.
    let clock = Arc::new(TestClock::default());
    let law_clock = clock.clone();
    let factory = wrapped(move || {
        let c = law_clock.clone();
        Box::new(move |_n, out| {
            c.advance(10.0);
            Ok(out)
        })
    });
    let mut svc = service(manual_cfg(), factory).with_law_clock(clock.clone());
    engage(&mut svc);
    let ran = run(&mut svc, 40);
    assert_law_fault_stop(&svc, &ran, LawFaultReason::OverBudget);
}

// ===========================================================================
// A law that moves the drive too far: the change cap and the core's divergence
// ===========================================================================

/// The run ended in the core's divergence abort, with the drive ceiling held
/// throughout (checked per tick by `run`).
fn assert_core_divergence(svc: &RandomService, ran: &Ran) {
    println!(
        "ended {:?} after {} ticks, trip {:?}, stats {:?}",
        svc.state(),
        ran.ticks,
        svc.trip(),
        svc.law_guard_stats()
    );
    assert_eq!(svc.state(), RunState::Aborted);
    let trip = svc.trip().expect("trip recorded");
    assert!(
        matches!(trip.kind, TripKind::Divergence { .. }),
        "expected the divergence abort, got {:?}",
        trip.kind
    );
    // Two settle loops, `divergence_n` (10) capped loops to call a line
    // divergent, then `divergence_loops` (5) loops of divergent lines.
    assert!(ran.ticks <= 20, "ended within 20 loops, took {}", ran.ticks);
    assert!(
        svc.law_guard_stats().capped_lines > 0,
        "the cap held lines back"
    );
}

#[test]
fn a_runaway_law_is_capped_and_aborted_for_divergence() {
    // Multiplies its drive by 10 every loop, and reports no divergence itself.
    let mut svc = service(manual_cfg(), owned(true, |_| 10.0));
    engage(&mut svc);
    let ran = run(&mut svc, 60);
    assert_core_divergence(&svc, &ran);
    // The core let the drive in force rise by at most the cap per loop.
    let cap = 10f64.powf(svc.config().law_change_cap_db() / 10.0);
    for w in ran.drives.windows(2) {
        let (a, b): (f64, f64) = (w[0].iter().sum(), w[1].iter().sum());
        assert!(
            b <= a * cap * (1.0 + 1e-9),
            "drive rose {} dB in a loop",
            10.0 * (b / a).log10()
        );
    }
}

#[test]
fn an_oscillating_law_is_capped_and_aborted_for_divergence() {
    // Alternates x10 and /10 of its base drive: 20 dB peak to peak every loop.
    let mut svc = service(
        manual_cfg(),
        owned(false, |n| if n % 2 == 1 { 10.0 } else { 0.1 }),
    );
    engage(&mut svc);
    let ran = run(&mut svc, 60);
    assert_core_divergence(&svc, &ran);
}

// ===========================================================================
// The cap on the shipped law
// ===========================================================================

#[test]
fn the_change_cap_never_binds_on_the_shipped_law_through_a_full_ladder() {
    // AUTO ladder with the soft start, level ramps and a ramped controlled stop at
    // the end: every rung, every ramp step and every retarget goes through the
    // cap. The shipped law must never be held back by it.
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::auto_default();
    cfg.schedule.dwell_s = vec![3.0];
    cfg.full_level_duration_s = 6.0;
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.alarm_db = (6.0, 6.0);
    cfg.abort_db = (12.0, 12.0);
    cfg.level_ramp_s = 2.0;
    cfg.shutdown_ramp_s = 2.0;
    let loop_cfg = LoopConfig::demo_default();
    let b = backend(&loop_cfg);
    let mut svc = RandomService::new(
        cfg,
        siso_table(),
        loop_cfg,
        b,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(std::env::temp_dir().join("hertzforge-law-guards-ladder")),
    )
    .expect("service builds");
    svc.set_review_after_sysid(false);
    // The soft start stays ON (default 9 dB below the first rung).
    engage(&mut svc);
    let mut ticks = 0;
    while ticks < 200 && svc.loop_tick().expect("tick") {
        ticks += 1;
    }
    let stats = svc.law_guard_stats();
    let cap = svc.config().law_change_cap_db();
    println!(
        "shipped law over {ticks} loops: {stats:?}, cap {cap} dB, state {:?}, trip {:?}",
        svc.state(),
        svc.trip()
    );
    assert_eq!(svc.state(), RunState::Stopped, "the ladder completes");
    assert!(svc.trip().is_none(), "no trip");
    assert_eq!(stats.misses, 0);
    assert_eq!(stats.capped_lines, 0, "the cap held the shipped law back");
    assert!(
        stats.peak_change_db > 0.0 && stats.peak_change_db < cap - 3.0,
        "the shipped law's largest per-loop change {} dB is not 3 dB clear of the {cap} dB cap",
        stats.peak_change_db
    );
}

#[test]
fn a_commanded_step_larger_than_the_cap_passes_through_whole() {
    // Engaged at -12 dB, a jump to 0 dB is a 12 dB commanded step, twice the
    // cap. The core rescales its reference by the command, so the drive steps the
    // full 12 dB on the next loop instead of being clamped to the cap.
    let mut cfg = manual_cfg();
    cfg.drive_ceiling_vrms = 1_000.0;
    let factory: Arc<dyn LawFactory> = Arc::new(hertzforge_random::HinvLawFactory);
    let mut svc = service(cfg, factory);
    engage(&mut svc);
    for _ in 0..4 {
        assert!(svc.loop_tick().expect("tick"));
    }
    let before: f64 = svc.last_drive_psd().expect("drive").iter().sum();
    svc.jump_to_table_level(4).expect("jump to full");
    assert!(svc.loop_tick().expect("tick"));
    let after: f64 = svc.last_drive_psd().expect("drive").iter().sum();
    let step_db = 10.0 * (after / before).log10();
    let cap = svc.config().law_change_cap_db();
    println!(
        "commanded +12 dB: the drive moved {step_db:.2} dB (cap {cap} dB); {:?}",
        svc.law_guard_stats()
    );
    assert!(
        (step_db - 12.0).abs() < 1.5,
        "the drive moved {step_db:.2} dB on a +12 dB command"
    );
    assert_eq!(svc.law_guard_stats().capped_lines, 0);
}

// ===========================================================================
// A law that fails a level change
// ===========================================================================

/// The shipped law, except that its level change panics once.
struct BadRetarget {
    inner: HinvLaw,
    panicked: bool,
}

impl ControlLaw for BadRetarget {
    fn init(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        self.inner.init(inputs)
    }
    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        self.inner.iterate(inputs)
    }
    fn retarget_level(&mut self, cmd: LevelCommand) -> Result<LoopOutputs, LawFault> {
        // The engage at the start level goes through; the operator's jump does not.
        if cmd.level_db > -1.0 && !self.panicked {
            self.panicked = true;
            panic!("adversarial law: deliberate panic on a level change");
        }
        self.inner.retarget_level(cmd)
    }
    fn name(&self) -> &str {
        "bad-retarget"
    }
}

#[test]
fn a_law_that_fails_a_level_change_still_gets_the_commanded_level() {
    // The commanded step is the schedule's, not the law's: when the law cannot
    // answer it, the core carries the drive in force to the new level itself, the
    // miss is counted, and one good loop afterwards clears the count.
    let factory: Arc<dyn LawFactory> = Arc::new(
        |s: &LawSetup<'_>| -> Result<Box<dyn ControlLaw + Send>, ControlError> {
            let inner = HinvLaw::new(s.hinv.clone(), s.target_psd, s.band, s.df_hz)?;
            Ok(Box::new(BadRetarget {
                inner,
                panicked: false,
            }))
        },
    );
    let mut cfg = manual_cfg();
    cfg.drive_ceiling_vrms = 1_000.0;
    let mut svc = service(cfg, factory);
    engage(&mut svc);
    for _ in 0..4 {
        assert!(svc.loop_tick().expect("tick"));
    }
    let before: f64 = svc.last_drive_psd().expect("drive").iter().sum();
    svc.jump_to_table_level(4).expect("jump to full");
    assert_eq!(
        svc.law_guard_stats().misses,
        1,
        "the failed level change is a miss"
    );
    assert!(svc.loop_tick().expect("tick"));
    let after: f64 = svc.last_drive_psd().expect("drive").iter().sum();
    // `before` is what the last loop synthesized; that loop's own law step (well
    // under 0.5 dB) moved the drive in force once more before the jump. Held
    // unscaled, the drive would not have moved at all.
    let step_db = 10.0 * (after / before).log10();
    assert!(
        (step_db - 12.0).abs() < 0.5,
        "the core carried the drive in force the commanded 12 dB, moved {step_db:.3} dB"
    );
    for _ in 0..4 {
        assert!(svc.loop_tick().expect("tick"));
    }
    assert_eq!(svc.state(), RunState::FullLevel);
    assert!(svc.trip().is_none(), "one miss does not stop the run");
    assert_eq!(svc.law_guard_stats().misses, 1);
}

// ===========================================================================
// A law that cannot seed
// ===========================================================================

/// A law whose seed is not a number.
struct NoSeed;

impl ControlLaw for NoSeed {
    fn init(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        Ok(LoopOutputs {
            drive_psd: vec![f64::NAN; inputs.nbins()],
            level_cmd_db: 0.0,
            diagnostics: Default::default(),
        })
    }
    fn iterate(&mut self, _inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        Err(LawFault::Failed("never reached".into()))
    }
    fn retarget_level(&mut self, _cmd: LevelCommand) -> Result<LoopOutputs, LawFault> {
        Err(LawFault::Failed("never reached".into()))
    }
    fn name(&self) -> &str {
        "no-seed"
    }
}

#[test]
fn a_law_that_cannot_seed_is_refused_before_anything_engages() {
    let factory: Arc<dyn LawFactory> = Arc::new(
        |_s: &LawSetup<'_>| -> Result<Box<dyn ControlLaw + Send>, ControlError> {
            Ok(Box::new(NoSeed))
        },
    );
    let mut svc = service(manual_cfg(), factory);
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    let err = svc
        .run_sysid(3)
        .expect_err("a law that cannot seed is refused");
    let words = err.to_string();
    assert!(
        words.contains("could not be seeded") && words.contains("not a number"),
        "{words}"
    );
    assert_eq!(svc.state(), RunState::SysId, "nothing engaged");
    assert_eq!(svc.law_guard_stats().misses, 1);
}
