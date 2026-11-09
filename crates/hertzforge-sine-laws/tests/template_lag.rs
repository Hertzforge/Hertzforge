//! The Sine template's law file on a station shaped as a hardware station is:
//! a flat wire (1 g per volt) at 12 800 Hz behind 41 samples of converter
//! delay, fed through an output queue kept two 256-frame blocks (512 frames)
//! ahead of the input, as an NI chassis is driven at that rate. The engage's
//! lead-in fills that queue, so the first drive is read at once, and every
//! later decision reaches the measurement 553 frames (43.2 ms) after it is
//! made. On that station the controller's tracked amplitude rises from the
//! lead-in at the engage, then stalls and dips for several ticks before it
//! reads the drive in force; at 20 Hz the tracker's corner is 2 Hz and the
//! whole rise takes about a third of a second. A law that reads the error in
//! that window acts on a target it has not yet met and climbs blind. The
//! simulator's own station, which queues four frames, shows none of it.
//!
//! Three runs, as an operator runs them on such a station: a 100 Hz dwell at
//! -6 dB with the law's first drive 4 dB high; the default start at 20 Hz at
//! full level ahead of a sweep; and the default start on a 20 to 500 Hz
//! up-down sweep through -12, -6 and 0 dB. The control channel's raw-sample
//! watchdog sits where a hardware station arms it (the simulator's own headroom
//! is twice that), so a climb the hardware would trip on trips here too. The
//! law is timed on a clock that does not move, with the whole tick as its
//! budget: these runs are about what the law does, not how long its calls take
//! (`law_call_timing.rs` is that).

mod common;

use common::law_file;
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::SimConfig;
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_module_abi::StationClass;
use hertzforge_sine::law::{LawClock, LawGuards};
use hertzforge_sine::service::engine::TickObservation;
use hertzforge_sine::service::watchdog_accel_headroom;
use hertzforge_sine::{
    BidirSweepConfig, CompressionPreset, DwellFor, DwellPlan, ProfileRow, RunState, RungFor,
    SineMode, SineProfile, SineService, SweepDirection, SweepRate, SweepTarget,
};
use hertzforge_sine_laws::{
    sine_check_config, trip_name, SineCheckCase, WasmSineLawFactory, SINE_CHECK_NOISE_FLOOR_G,
    SINE_CHECK_SEED,
};
use std::sync::{Arc, Mutex};

/// The station's rate, Hz, and its tick, frames (10 ms).
const FS_HZ: f64 = 12_800.0;
const TICK_FRAMES: usize = 128;
/// The converters' delay, samples at [`FS_HZ`].
const CONVERTER_DELAY: usize = 41;
/// How far ahead of the input the output queue is kept, frames: two blocks.
const LOOKAHEAD_FRAMES: u32 = 512;
/// The full-level target, g pk, and the abort band, dB either way.
const FULL_G: f64 = 0.2;
const ABORT_DB: f64 = 3.0;

/// The simulator, stating an output lookahead of [`LOOKAHEAD_FRAMES`]: the
/// controller loads that much lead-in before it starts the engaged run, and
/// keeps the queue that deep.
struct DeepQueue {
    inner: Box<dyn Backend>,
}

impl Backend for DeepQueue {
    fn caps(&self) -> Caps {
        let mut caps = self.inner.caps();
        if let Some(out) = caps.output_buffer.as_mut() {
            out.min_lookahead_frames = LOOKAHEAD_FRAMES;
        }
        caps
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(table, acq)
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
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

/// A flat wire, 1 g per volt, behind the converters, through the deep queue.
fn station() -> Box<dyn Backend> {
    let wire = SimConfig::new()
        .sample_rate_hz(FS_HZ)
        .shared_clock(true)
        .max_output_channels(1)
        .seed(0x51E5_5EED)
        .resonances(Vec::new())
        .feedthrough(1.0)
        .noise_level(SINE_CHECK_NOISE_FLOOR_G)
        .transport_delay_samples(CONVERTER_DELAY)
        .build();
    Box::new(DeepQueue {
        inner: Box::new(wire),
    })
}

/// Where a hardware station arms the control channel's raw-sample watchdog:
/// the profile's abort edge at the highest level the run reaches, times the
/// hardware headroom.
fn hardware_watchdog_g(max_level_db: f64) -> f64 {
    FULL_G
        * 10f64.powf((ABORT_DB + max_level_db) / 20.0)
        * watchdog_accel_headroom(StationClass::NiReal)
}

fn table(watchdog_g: f64) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut drive = Channel::new(1);
    drive.role = ChannelRole::Drive;
    drive.units = Units::VoltageV;
    t.channels.push(drive);
    let mut control = Channel::new(2);
    control.role = ChannelRole::Control;
    control.units = Units::AccelG;
    control.abort.accel_g = Some(watchdog_g);
    t.channels.push(control);
    t
}

/// A flat profile over `band` Hz.
fn profile(band: (f64, f64)) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(band.0, FULL_G));
    p.push(ProfileRow::accel(band.1, FULL_G));
    p.global_alarm_db = (1.5, 1.5);
    p.global_abort_db = (ABORT_DB, ABORT_DB);
    p
}

struct Frozen;

impl LawClock for Frozen {
    fn now_s(&self) -> f64 {
        0.0
    }
}

/// How a run ended, and every tick it ran control on.
struct Run {
    end: String,
    ticks: Vec<TickObservation>,
}

/// The run: `band` the profile's, `mode` and the ladder of `levels`, the law
/// file with `params`, slow compression (a law file runs under the
/// configuration's), identification at -12 dB, the drive ceiling 0.5 V pk and
/// its abort 0.8 V pk.
fn run(
    band: (f64, f64),
    mode: SineMode,
    levels: &[f64],
    rungs: (RungFor, RungFor),
    params: &[Param],
) -> Run {
    let (per_rung, full_for) = rungs;
    let mut cfg = sine_check_config(SineCheckCase::Delay, FS_HZ);
    cfg.profile = profile(band);
    cfg.mode = mode;
    cfg.compression = CompressionPreset::Slow;
    cfg.ladder.sysid_level_db = -12.0;
    cfg.sysid_level_db = -12.0;
    cfg.ladder.start_db = levels[0];
    cfg.ladder.table_db = levels.to_vec();
    cfg.ladder.per_rung = per_rung;
    cfg.ladder.full_for = full_for;
    cfg.drive_vpk_ceiling = 0.5;
    cfg.drive_vpk_abort = 0.8;
    cfg.control_tick_frames = TICK_FRAMES;
    cfg.control_tick_s = Some(TICK_FRAMES as f64 / FS_HZ);
    cfg.law_guards = Some(LawGuards {
        budget_fraction: 1.0,
        ..LawGuards::default()
    });
    let max_level = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_sine(&std::fs::read(law_file("law_fixture_sine_template")).unwrap())
        .unwrap();
    let resolved = law.resolve_params(params, 1).unwrap();
    let factory = Arc::new(WasmSineLawFactory::new(law, resolved));

    let seen = Arc::new(Mutex::new(Vec::with_capacity(40_000)));
    let sink = seen.clone();
    let mut svc = SineService::new(
        cfg,
        table(hardware_watchdog_g(max_level)),
        station(),
        StationClass::HertzforgeSim,
        SINE_CHECK_SEED,
        Some(std::env::temp_dir().join("hertzforge-sine-template-lag")),
    )
    .unwrap()
    .with_law_factory(factory)
    .with_law_clock(Arc::new(Frozen))
    .with_tick_observer(Box::new(move |o: &TickObservation| {
        if let Ok(mut v) = sink.lock() {
            v.push(*o);
        }
    }));
    svc.assess_feasibility().unwrap();
    svc.arm().unwrap();
    svc.run_sysid(0).unwrap();
    svc.coherence_gate().unwrap();
    let mut chunks = 0;
    while matches!(svc.state(), RunState::Engaged | RunState::FullLevel) && chunks < 4_000 {
        if !svc.run_chunk().unwrap() {
            break;
        }
        chunks += 1;
    }
    let end = match (svc.state(), svc.trip()) {
        (RunState::Aborted, Some(t)) => format!("aborted ({})", trip_name(&t.kind)),
        (RunState::Aborted, None) => format!("aborted ({})", svc.end_reason().unwrap_or("")),
        _ => "completed".to_owned(),
    };
    let ticks = seen.lock().unwrap().clone();
    Run { end, ticks }
}

fn error_db(o: &TickObservation) -> f64 {
    20.0 * (o.measured.max(1e-12) / o.target).log10()
}

/// At full level after the settle window: the ticks, the share within
/// 0.25 dB, and the worst error, dB.
fn full_level(r: &Run) -> (usize, f64, f64) {
    let full: Vec<f64> = r
        .ticks
        .iter()
        .filter(|o| o.level_db.abs() < 1e-9 && !o.settling)
        .map(error_db)
        .collect();
    let within = full.iter().filter(|e| e.abs() <= 0.25).count() as f64 / full.len().max(1) as f64;
    let worst = full
        .iter()
        .copied()
        .fold(0.0, |a: f64, e| if e.abs() > a.abs() { e } else { a });
    println!(
        "full level: {} ticks, {:.2}% within 0.25 dB, worst {worst:+.3} dB",
        full.len(),
        100.0 * within
    );
    (full.len(), within, worst)
}

fn print_first(r: &Run, n: usize) {
    for o in r.ticks.iter().take(n) {
        println!(
            "tick {:>3} {:>7.2} Hz {:>+5.1} dB: measured {:.4} g, target {:.4} g ({:>+7.2} dB), \
             drive {:.4} V",
            o.tick,
            o.freq_hz,
            o.level_db,
            o.measured,
            o.target,
            error_db(o),
            o.drive
        );
    }
}

/// A 100 Hz dwell at -6 dB, the law's first drive 4 dB high: the first drive
/// is 4 dB over the target's, the run completes, and from 2 s after the
/// engage every tick is within 0.5 dB of the target.
#[test]
fn a_first_drive_four_db_high_comes_back_without_a_trip() {
    let r = run(
        (10.0, 1000.0),
        SineMode::Dwell(DwellPlan {
            freq_hz: 100.0,
            for_: DwellFor::Seconds(6.0),
        }),
        &[-6.0],
        (RungFor::Seconds(6.0), RungFor::Seconds(6.0)),
        &[Param::new("start_offset_db", ParamValue::Number(4.0))],
    );
    print_first(&r, 30);
    let peak = r.ticks.iter().map(|o| o.drive).fold(0.0, f64::max);
    println!(
        "end: {}; {} ticks; drive peak {peak:.4} V",
        r.end,
        r.ticks.len()
    );
    assert_eq!(r.end, "completed");
    // On a wire of 1 g per volt the target's drive is the target.
    let first = r.ticks.first().expect("a tick ran");
    let start_db = 20.0 * (first.drive / first.target).log10();
    assert!(
        (start_db - 4.0).abs() <= 0.5,
        "first drive {start_db:+.2} dB"
    );
    let tick_s = TICK_FRAMES as f64 / FS_HZ;
    let late: Vec<f64> = r
        .ticks
        .iter()
        .enumerate()
        .filter(|(k, _)| (*k + 1) as f64 * tick_s > 2.0 + 1e-9)
        .map(|(_, o)| error_db(o))
        .collect();
    let worst = late
        .iter()
        .copied()
        .fold(0.0, |a: f64, e| if e.abs() > a.abs() { e } else { a });
    println!("from 2 s: {} ticks, worst {worst:+.3} dB", late.len());
    assert!(late.len() >= 300, "{} ticks judged", late.len());
    assert!(
        worst.abs() <= 0.5,
        "worst {worst:+.3} dB from 2 s after the engage"
    );
}

/// The default start at 20 Hz at full level, ahead of a 20 to 500 Hz sweep at
/// 4 oct/min (the profile runs from 10 Hz, so 20 Hz is inside the band the
/// identification trusts and the first drive is the target's): the drive
/// never goes more than 1 dB over the target's, the run completes, and at
/// full level after the settle window the error is within 0.25 dB on 99% of
/// ticks and 0.5 dB on every tick.
#[test]
fn the_default_start_at_20_hz_at_full_level_holds_until_the_measurement_comes_up() {
    let r = run(
        (10.0, 1000.0),
        SineMode::Sweep(BidirSweepConfig {
            band: (20.0, 500.0),
            rate: SweepRate::LogOctPerMin(4.0),
            start_hz: 20.0,
            initial_direction: SweepDirection::Up,
            target: SweepTarget::Sweeps(1),
        }),
        &[0.0],
        (RungFor::Sweeps(1), RungFor::Sweeps(1)),
        &[],
    );
    print_first(&r, 40);
    // On a wire of 1 g per volt the target's drive is the target.
    let over = r
        .ticks
        .iter()
        .map(|o| 20.0 * (o.drive / o.target).log10())
        .fold(f64::NEG_INFINITY, f64::max);
    println!(
        "end: {}; {} ticks; the drive at most {over:+.2} dB over the target's",
        r.end,
        r.ticks.len()
    );
    assert_eq!(r.end, "completed");
    let first = r.ticks.first().expect("a tick ran");
    let start_db = 20.0 * (first.drive / first.target).log10();
    assert!(start_db.abs() <= 0.5, "first drive {start_db:+.2} dB");
    assert!(
        over <= 1.0,
        "the drive went {over:+.2} dB over the target's"
    );
    let (n, within, worst) = full_level(&r);
    assert!(n > 5_000, "{n} full-level ticks");
    assert!(within >= 0.99, "{:.2}% within 0.25 dB", 100.0 * within);
    assert!(worst.abs() <= 0.5, "worst {worst:+.3} dB");
}

/// The default start on a 20 to 500 Hz up-down sweep at 4 oct/min through
/// -12, -6 and 0 dB, one sweep each and two at full level: the run completes,
/// and at full level after the settle window the error is within 0.25 dB on
/// 99% of ticks and 0.5 dB on every tick.
#[test]
fn the_default_start_on_a_ladder_sweep_completes_within_its_band() {
    let r = run(
        (20.0, 500.0),
        SineMode::Sweep(BidirSweepConfig {
            band: (20.0, 500.0),
            rate: SweepRate::LogOctPerMin(4.0),
            start_hz: 20.0,
            initial_direction: SweepDirection::Up,
            target: SweepTarget::Sweeps(4),
        }),
        &[-12.0, -6.0, 0.0],
        (RungFor::Sweeps(1), RungFor::Sweeps(2)),
        &[],
    );
    print_first(&r, 30);
    let peak = r.ticks.iter().map(|o| o.drive).fold(0.0, f64::max);
    println!(
        "end: {}; {} ticks; drive peak {peak:.4} V",
        r.end,
        r.ticks.len()
    );
    assert_eq!(r.end, "completed");
    let (n, within, worst) = full_level(&r);
    assert!(n > 10_000, "{n} full-level ticks");
    assert!(within >= 0.99, "{:.2}% within 0.25 dB", 100.0 * within);
    assert!(worst.abs() <= 0.5, "worst {worst:+.3} dB");
}
