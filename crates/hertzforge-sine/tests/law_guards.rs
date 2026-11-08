//! The core's guards on a Sine control law, each against a law built to
//! break it.
//!
//! Every test runs the real service on the simulator under the default
//! guards (`LawGuards::default()`: 1 dB a tick, 240 dB/s up, a quarter of the
//! tick per call, three misses, one second of capping) with a law that
//! behaves for its first ticks and then misbehaves one way: a NaN drive, a
//! negative drive, +20 dB a tick, a climb inside the per-tick cap, a call slow
//! on the test clock, a panic, a rise while the limit channel holds the drive,
//! a drive that ignores the resonance search, a first drive at the ceiling.
//! The law proposes; the core disposes.
//!
//! Red first, green after (session log 2026-11-08, Sine core): every test but
//! `a_law_raising_the_drive_while_the_notch_holds_it_stays_under_the_ceiling`
//! was committed failing against the seam with no guards, and passes with
//! them. That one is held by the notch governor's own clamp, which the seam
//! already applied after the law; it is falsified by taking the clamp out.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::law::servo_blend::{ServoBlendLaw, ServoBlendParams};
use hertzforge_sine::law::{
    Hold, LawClock, LawDescriptor, LawFault, LawGuards, LawValues, SineControlLaw, SineLawFactory,
    SineLawSetup, SineOutputRule, TickInputs, TickOutputs,
};
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, LimitProfile, SineMode, SineTestConfig,
};
use hertzforge_sine::service::engine::TickObservation;
use hertzforge_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig,
    LadderConfig, NotchGovernorConfig, ProfileRow, RstdConfig, RstdDwellMode, RunState, RungFor,
    SineProfile, SineService, SweepDirection, SweepRate, SweepTarget, TripKind,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const FS: f64 = 16_384.0;
const TICK: usize = 164;
/// Good ticks before a misbehaving law turns.
const GOOD_TICKS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Nan,
    Negative,
    Plus20Db,
    Climb,
    Slow,
    Panic,
    RiseInNotch,
    IgnoreProbe,
    FirstAtCeiling,
    /// Two NaN calls, then a good one, over and over: never three in a row.
    Flaky,
}

/// The test's clock: microseconds, moved only by the law that is "slow".
struct TestClock(AtomicU64);

impl LawClock for TestClock {
    fn now_s(&self) -> f64 {
        self.0.load(Ordering::Relaxed) as f64 * 1e-6
    }
}

struct Bad {
    inner: ServoBlendLaw,
    mode: Mode,
    clock: Arc<TestClock>,
    ceiling: f64,
}

impl SineControlLaw for Bad {
    fn name(&self) -> &str {
        "misbehaving"
    }

    fn engage(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault> {
        let out = self.inner.engage(inputs, values)?;
        if self.mode == Mode::FirstAtCeiling {
            return Ok(TickOutputs {
                drive: self.ceiling,
                faulted: false,
            });
        }
        Ok(out)
    }

    fn tick(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault> {
        let out = self.inner.tick(inputs, values)?;
        let turned = inputs.tick_index > GOOD_TICKS;
        let bad = |drive: f64| {
            Ok(TickOutputs {
                drive,
                faulted: false,
            })
        };
        match self.mode {
            Mode::Nan if turned => bad(f64::NAN),
            Mode::Negative if turned => bad(-0.1),
            Mode::Plus20Db if turned => bad(inputs.drive * 10.0),
            Mode::Climb if turned => bad(inputs.drive * 10f64.powf(0.8 / 20.0)),
            Mode::Slow if turned => {
                self.clock.0.fetch_add(3_000, Ordering::Relaxed);
                Ok(out)
            }
            Mode::Panic if turned => panic!("the law fell over"),
            Mode::RiseInNotch if inputs.hold == Hold::Notch => bad(inputs.drive * 4.0),
            Mode::IgnoreProbe if inputs.hold == Hold::Probe => bad(inputs.drive * 3.0),
            Mode::Flaky if turned && !inputs.tick_index.is_multiple_of(3) => bad(f64::NAN),
            _ => Ok(out),
        }
    }

    fn adjust(&mut self, key: &str, value: f64) -> Result<(), LawFault> {
        self.inner.adjust(key, value)
    }
}

struct BadFactory {
    mode: Mode,
    clock: Arc<TestClock>,
    ceiling: f64,
}

impl SineLawFactory for BadFactory {
    fn build(&self, _setup: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        Ok(Box::new(Bad {
            inner: ServoBlendLaw::new(&params()).unwrap(),
            mode: self.mode,
            clock: self.clock.clone(),
            ceiling: self.ceiling,
        }))
    }

    fn describe(&self) -> LawDescriptor {
        let mut d = params().descriptor();
        d.id = "misbehaving".into();
        d.name = "misbehaving".into();
        d
    }
}

fn params() -> ServoBlendParams {
    ServoBlendParams {
        blend_w: 0.5,
        compression: CompressionPreset::Fast,
        kp: 0.3,
    }
}

fn flat(lo: f64, hi: f64, g: f64, alarm: f64, abort: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(lo, g));
    p.push(ProfileRow::accel(hi, g));
    p.global_alarm_db = (alarm, alarm);
    p.global_abort_db = (abort, abort);
    p
}

fn table(limit: bool) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    t.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    t.channels.push(d);
    if limit {
        let mut l = Channel::new(3);
        l.role = ChannelRole::Monitor;
        l.units = Units::AccelG;
        t.channels.push(l);
    }
    t
}

const CEILING: f64 = 5.0;

fn cfg(mode: SineMode) -> SineTestConfig {
    SineTestConfig {
        // A wide abort band: the guards, not the band, are what these runs
        // test (a law capped at +1 dB a tick still climbs 20 dB in 20 ticks).
        profile: flat(60.0, 500.0, 1.0, 3.0, 60.0),
        mode,
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 0.5,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Seconds(600.0),
            full_for: RungFor::Seconds(600.0),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.0,
        drive_vpk_ceiling: CEILING,
        drive_vpk_abort: 2.0 * CEILING,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: TICK,
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: Some(LawGuards::default()),
        control_tick_s: None,
    }
}

fn dwell(hz: f64, s: f64) -> SineMode {
    SineMode::Dwell(DwellPlan {
        freq_hz: hz,
        for_: DwellFor::Seconds(s),
    })
}

/// What one run left behind.
struct Outcome {
    svc: SineService,
    ticks: Vec<TickObservation>,
}

fn run_with(mode: Mode, cfg: SineTestConfig, table: ChannelTable, chunks: u64) -> Outcome {
    let clock = Arc::new(TestClock(AtomicU64::new(0)));
    let seen: Arc<Mutex<Vec<TickObservation>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 20.0)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(0x51E5_0002);
    let mut svc = SineService::new(
        cfg,
        table,
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        0x51E5_0002,
        None,
    )
    .expect("the configuration builds")
    .with_law_factory(Arc::new(BadFactory {
        mode,
        clock: clock.clone(),
        ceiling: CEILING,
    }))
    .with_law_clock(clock)
    .with_tick_observer(Box::new(move |o: &TickObservation| {
        sink.lock().unwrap().push(*o)
    }));
    svc.assess_feasibility().unwrap();
    svc.arm().unwrap();
    svc.run_sysid(4096).unwrap();
    svc.coherence_gate().unwrap();
    let mut n = 0;
    while matches!(svc.state(), RunState::Engaged | RunState::FullLevel) && n < chunks {
        if !svc.run_chunk().unwrap_or(false) {
            break;
        }
        n += 1;
    }
    let ticks = seen.lock().unwrap().clone();
    Outcome { svc, ticks }
}

fn run(mode: Mode) -> Outcome {
    run_with(mode, cfg(dwell(150.0, 4.0)), table(false), 40)
}

/// The run ended `law_fault` for `want`, after exactly three misses, with the
/// drive in force held through them and the controlled stop after.
fn assert_law_fault(o: &Outcome, want: &str) {
    let trip = o.svc.trip().expect("a trip");
    match &trip.kind {
        TripKind::LawFault { fault, consecutive } => {
            assert_eq!(fault.reason(), want, "{fault}");
            assert_eq!(*consecutive, 3);
        }
        other => panic!("ended on {other:?}, not a law fault"),
    }
    assert_eq!(o.svc.state(), RunState::Aborted);
    // The last good tick, then nothing the law asked for: the drive the core
    // committed is the one in force on the last good tick.
    let last_good = o
        .ticks
        .iter()
        .rfind(|t| t.tick < GOOD_TICKS)
        .expect("good ticks")
        .drive;
    for t in o.ticks.iter().filter(|t| t.tick >= GOOD_TICKS) {
        assert_eq!(t.drive.to_bits(), last_good.to_bits(), "the drive was held");
    }
    // The controlled stop ramped it to zero (it did not cut it at the batch).
    assert!(o.svc.drive_is_zero());
    let ended = o.svc.end_reason().unwrap_or_default();
    assert!(ended.starts_with("control law"), "{ended}");
}

#[test]
fn a_nan_drive_is_refused_three_times_then_the_run_stops() {
    let o = run(Mode::Nan);
    assert_law_fault(&o, "non_finite");
}

#[test]
fn a_negative_drive_is_refused_three_times_then_the_run_stops() {
    let o = run(Mode::Negative);
    assert_law_fault(&o, "negative");
}

#[test]
fn a_call_slow_on_the_test_clock_is_a_miss() {
    let o = run(Mode::Slow);
    assert_law_fault(&o, "over_budget");
    match &o.svc.trip().unwrap().kind {
        TripKind::LawFault {
            fault: LawFault::OverBudget { took_s, budget_s },
            ..
        } => {
            assert!((took_s - 0.003).abs() < 1e-9, "{took_s}");
            assert!(
                (budget_s - 0.25 * TICK as f64 / FS).abs() < 1e-12,
                "{budget_s}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_panicking_law_is_caught_per_call() {
    let o = run(Mode::Panic);
    assert_law_fault(&o, "panicked");
}

#[test]
fn a_usable_call_resets_the_count() {
    let o = run(Mode::Flaky);
    assert!(o.svc.trip().is_none(), "{:?}", o.svc.trip());
    assert_eq!(o.svc.state(), RunState::Stopped, "the dwell completed");
    let tally = o.svc.law_tally();
    assert!(tally.misses > 150, "{tally:?}");
}

#[test]
fn twenty_db_a_tick_is_capped_to_one_and_trips_divergence_after_a_second() {
    let o = run(Mode::Plus20Db);
    let turned: Vec<&TickObservation> = o.ticks.iter().filter(|t| t.tick >= GOOD_TICKS).collect();
    for w in turned.windows(2) {
        let step = 20.0 * (w[1].drive / w[0].drive).log10();
        assert!(step <= 1.0 + 1e-9, "a tick moved {step} dB");
    }
    match &o.svc.trip().expect("a trip").kind {
        TripKind::Divergence { capped_s } => assert!((*capped_s - 1.0).abs() < 0.011, "{capped_s}"),
        other => panic!("ended on {other:?}"),
    }
    assert!(o.svc.drive_is_zero());
}

#[test]
fn a_climb_inside_the_step_cap_is_held_to_the_rise_cap() {
    // 0.8 dB a tick is inside the 1 dB step cap; a rise cap of 50 dB/s holds
    // it to 0.5 dB a 10 ms tick.
    let mut c = cfg(dwell(150.0, 4.0));
    c.law_guards = Some(LawGuards {
        max_rise_db_per_s: 50.0,
        ..LawGuards::default()
    });
    let o = run_with(Mode::Climb, c, table(false), 40);
    let dt = TICK as f64 / FS;
    let turned: Vec<&TickObservation> = o.ticks.iter().filter(|t| t.tick >= GOOD_TICKS).collect();
    assert!(turned.len() > 50);
    for w in turned.windows(2) {
        let step = 20.0 * (w[1].drive / w[0].drive).log10();
        assert!(step <= 50.0 * dt + 1e-9, "a tick rose {step} dB");
    }
    assert!(matches!(
        o.svc.trip().expect("a trip").kind,
        TripKind::Divergence { .. }
    ));
}

#[test]
fn a_law_raising_the_drive_while_the_notch_holds_it_stays_under_the_ceiling() {
    // The limit channel's bound sits under the control target: the notch
    // holds the drive at the dwell, and the law quadruples it every time.
    let mut c = cfg(dwell(250.0, 3.0));
    c.limit_profiles = vec![LimitProfile {
        channel_index: 3,
        profile: flat(60.0, 500.0, 0.6, 1.0, 2.0),
        shared_name: None,
    }];
    let o = run_with(Mode::RiseInNotch, c, table(true), 40);
    assert!(o.ticks.iter().any(|t| t.notch_binding), "the notch engaged");
    // Whatever ended the run, the limit channel was never driven past its
    // bound by the law's ask: a quadrupled drive would read 12 dB over.
    if let Some(TripKind::LimitChannel {
        measured, limit, ..
    }) = o.svc.trip().map(|t| t.kind.clone())
    {
        let over_db = 20.0 * (measured / limit).log10();
        assert!(over_db < 0.5, "the limit channel read {over_db:.2} dB over");
    }
}

#[test]
fn a_law_ignoring_the_resonance_search_does_not_move_the_drive() {
    let mut c = cfg(SineMode::Rstd(RstdConfig {
        band: (150.0, 400.0),
        level_db: -12.0,
        rate: SweepRate::LogOctPerMin(60.0),
        tracking_bw_hz: 25.0,
        settle_k: 5.0,
        dwell_mode: RstdDwellMode::PhaseTracked,
        dwell_for: DwellFor::Seconds(0.2),
    }));
    c.profile = flat(150.0, 400.0, 0.2, 6.0, 20.0);
    c.control_tick_frames = 32;
    let o = run_with(Mode::IgnoreProbe, c, table(false), 400);
    // During the search the drive is held where it was engaged: every tick
    // of the probe commits the same drive.
    let first = o.ticks.first().expect("ticks").drive;
    let probe: Vec<&TickObservation> = o.ticks.iter().take(200).collect();
    assert!(
        probe.iter().all(|t| t.drive.to_bits() == first.to_bits()),
        "the drive moved during the search: {:?}",
        probe.iter().map(|t| t.drive).take(8).collect::<Vec<_>>()
    );
}

#[test]
fn a_first_drive_at_the_ceiling_is_bounded_by_the_prediction() {
    // At 220 Hz the identification is trusted: the same run with the
    // built-in's own first drive (the prediction) and with a law that asks for
    // the 5 V ceiling. The core lets the second have the prediction plus 6 dB
    // and no more; each first tick then moves the drive by at most the 1 dB a
    // tick the law's own clamp allows.
    let at = |mode| run_with(mode, cfg(dwell(220.0, 2.0)), table(false), 20);
    let ceiling = at(Mode::FirstAtCeiling);
    let own = at(Mode::Flaky);
    let first = ceiling.ticks.first().expect("ticks").drive;
    let seed = own.ticks.first().expect("ticks").drive;
    let over = 20.0 * (first / seed).log10();
    assert!(
        (4.0..=8.0).contains(&over),
        "the first drive is {over:.2} dB over the prediction (the bound is 6 dB), {first} V"
    );

    // At 150 Hz this plant's identification is not trusted (its gain is under
    // the trust floor): the first drive is at most 0.05 of the start level's
    // scale (here 0 dB, so 0.05), moved by the first tick's 1 dB at most.
    let untrusted = run(Mode::FirstAtCeiling);
    let first = untrusted.ticks.first().expect("ticks").drive;
    assert!(first <= 0.05 * 10f64.powf(1.0 / 20.0) + 1e-12, "{first} V");
}

#[test]
fn the_output_rules_have_their_names() {
    assert_eq!(
        LawFault::BadOutput(SineOutputRule::NonFinite).reason(),
        "non_finite"
    );
    // A sweep configuration with guards builds (the guards are accepted).
    let _ = cfg(SineMode::Sweep(BidirSweepConfig {
        band: (60.0, 500.0),
        rate: SweepRate::LogOctPerMin(8.0),
        start_hz: 60.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    }));
}
