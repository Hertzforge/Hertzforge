//! The recorded trace of the built-in control: what the control-law seam must
//! not move.
//!
//! On six closed-loop runs on the simulator it records every control tick's
//! drive decision through the run's tick observer: the drive committed, the
//! amplitude servo's step and its own level and fault latch, what the tick
//! measured, its frequency and level, and whether the limit channels held the
//! drive. The six: (a) a fast sweep through a Q 50 resonance, (b) the slow
//! accuracy bar (1 oct/min through Q 50), (c) a dwell at a resonance, (d) a
//! -12/-6/0 dB ladder moved by hand with a live blend-weight change, (e) a
//! limit channel that holds the drive down at a dwell on a resonance, (f) a
//! resonance search and phase-tracked dwell.
//!
//! Per run the trace holds a hash of every tick's bits (FNV-1a over the f64
//! bits), the tick count, every 512th tick in full, and how the run ended.
//! Object keys are written sorted, so the file reads the same whether
//! `serde_json` keeps insertion order (the workspace build) or not (the
//! package alone).
//!
//! On the platform it was recorded on (macOS on aarch64) the trace must match
//! byte for byte. Elsewhere a last bit may round differently in the maths
//! library and a closed loop carries it on, so the sampled ticks are held to
//! relative 1e-6 and the hashes are not compared.
//!
//! Re-record (only when a change to the run is intended and said so):
//! `HERTZFORGE_RECORD_SINE_TRACE=1 cargo test -p hertzforge-sine --test recorded_trace`.
//!
//! Falsified once (session log 2026-11-08, Sine core): the servo's
//! proportional gain 0.3 changed to 0.31 in the service's servo build breaks
//! every run's hash.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, LimitProfile, SineMode, SineTestConfig,
};
use hertzforge_sine::service::engine::TickObservation;
use hertzforge_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig,
    LadderConfig, NotchGovernorConfig, ProfileRow, RstdConfig, RstdDwellMode, RunMode, RunState,
    RungFor, SineProfile, SineService, SweepDirection, SweepRate, SweepTarget,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const TRACE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/sine_servo_trace.txt"
);

/// The platform the trace was recorded on.
const RECORDING_PLATFORM: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

/// Every this-many ticks one is written out in full.
const SAMPLE_EVERY: u64 = 512;

fn flat(lo: f64, hi: f64, g: f64, alarm_db: f64, abort_db: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(lo, g));
    p.push(ProfileRow::accel(hi, g));
    p.global_alarm_db = (alarm_db, alarm_db);
    p.global_abort_db = (abort_db, abort_db);
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

fn base(band: (f64, f64), rate: SweepRate, profile: SineProfile) -> SineTestConfig {
    SineTestConfig {
        profile,
        mode: SineMode::Sweep(BidirSweepConfig {
            band,
            rate,
            start_hz: band.0,
            initial_direction: SweepDirection::Up,
            target: SweepTarget::Sweeps(1),
        }),
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 1.0,
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
            per_rung: RungFor::Sweeps(1),
            full_for: RungFor::Sweeps(1),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.5,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: 48,
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: None,
        control_tick_s: None,
    }
}

fn sim(f_n: f64, q: f64) -> SimConfig {
    SimConfig::new()
        .sample_rate_hz(16_384.0)
        .resonance(f_n, q)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(0x51E5_0001)
}

/// The fixtures, by name. Each is a configuration, a table, a plant and, for
/// the hand-moved ladder, the operator's moves (chunk index, move).
struct Fixture {
    name: &'static str,
    cfg: SineTestConfig,
    table: ChannelTable,
    sim: SimConfig,
    manual: bool,
    moves: Vec<(u64, Move)>,
    max_chunks: u64,
}

#[derive(Clone, Copy)]
enum Move {
    Level(usize),
    W(f64),
    Stop,
}

fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();

    // (a) The fast sweep through Q 50.
    let mut a = base(
        (80.0, 500.0),
        SweepRate::LogOctPerMin(6.0),
        flat(80.0, 500.0, 1.0, 2.5, 5.0),
    );
    a.control_tick_frames = 48;
    out.push(Fixture {
        name: "a_fast_sweep_q50",
        cfg: a,
        table: table(false),
        sim: sim(250.0, 50.0),
        manual: false,
        moves: vec![],
        max_chunks: 800,
    });

    // (b) The slow accuracy bar.
    let mut b = base(
        (100.0, 800.0),
        SweepRate::LogOctPerMin(1.0),
        flat(100.0, 800.0, 1.0, 1.5, 3.0),
    );
    b.compression = CompressionPreset::Slow;
    b.control_tick_frames = 32;
    out.push(Fixture {
        name: "b_slow_accuracy_bar",
        cfg: b,
        table: table(false),
        sim: sim(250.0, 50.0),
        manual: false,
        moves: vec![],
        max_chunks: 20_000,
    });

    // (c) A dwell at a Q 20 resonance.
    let mut c = base(
        (60.0, 500.0),
        SweepRate::LogOctPerMin(8.0),
        flat(60.0, 500.0, 1.0, 3.0, 6.0),
    );
    c.mode = SineMode::Dwell(DwellPlan {
        freq_hz: 250.0,
        for_: DwellFor::Seconds(4.0),
    });
    c.blend_w = 0.5;
    c.control_tick_frames = 164;
    out.push(Fixture {
        name: "c_dwell_at_resonance",
        cfg: c,
        table: table(false),
        sim: sim(250.0, 20.0),
        manual: false,
        moves: vec![],
        max_chunks: 40,
    });

    // (d) The ladder by hand: -12 dB, then -6, a blend-weight change, then 0,
    // then stop. A chunk is 0.25 s of station time.
    let mut d = base(
        (60.0, 500.0),
        SweepRate::LogOctPerMin(8.0),
        flat(60.0, 500.0, 1.0, 3.0, 6.0),
    );
    d.mode = SineMode::Dwell(DwellPlan {
        freq_hz: 180.0,
        for_: DwellFor::Seconds(600.0),
    });
    d.blend_w = 0.5;
    d.ladder = LadderConfig {
        sysid_level_db: -20.0,
        start_db: -12.0,
        table_db: vec![-12.0, -6.0, 0.0],
        per_rung: RungFor::Seconds(600.0),
        full_for: RungFor::Seconds(600.0),
    };
    d.control_tick_frames = 164;
    out.push(Fixture {
        name: "d_ladder_by_hand",
        cfg: d,
        table: table(false),
        sim: sim(250.0, 20.0),
        manual: true,
        moves: vec![
            (8, Move::Level(1)),
            (12, Move::W(0.2)),
            (16, Move::Level(2)),
            (24, Move::Stop),
        ],
        max_chunks: 40,
    });

    // (e) A limit channel whose bound sits under the control target at a dwell
    // on the resonance: the notch holds the drive down.
    let mut e = base(
        (60.0, 500.0),
        SweepRate::LogOctPerMin(8.0),
        flat(60.0, 500.0, 1.0, 3.0, 20.0),
    );
    e.mode = SineMode::Dwell(DwellPlan {
        freq_hz: 250.0,
        for_: DwellFor::Seconds(4.0),
    });
    e.blend_w = 0.5;
    e.control_tick_frames = 164;
    e.limit_profiles = vec![LimitProfile {
        channel_index: 3,
        profile: flat(60.0, 500.0, 0.6, 1.0, 2.0),
        shared_name: None,
    }];
    out.push(Fixture {
        name: "e_notch",
        cfg: e,
        table: table(true),
        sim: sim(250.0, 20.0),
        manual: false,
        moves: vec![],
        max_chunks: 40,
    });

    // (f) A resonance search, then the phase-tracked dwell.
    let mut f = base(
        (150.0, 400.0),
        SweepRate::LogOctPerMin(30.0),
        flat(150.0, 400.0, 0.2, 6.0, 12.0),
    );
    f.mode = SineMode::Rstd(RstdConfig {
        band: (150.0, 400.0),
        level_db: -12.0,
        rate: SweepRate::LogOctPerMin(60.0),
        tracking_bw_hz: 25.0,
        settle_k: 5.0,
        dwell_mode: RstdDwellMode::PhaseTracked,
        dwell_for: DwellFor::Seconds(0.2),
    });
    f.control_tick_frames = 32;
    f.coherence_floor = 0.0;
    out.push(Fixture {
        name: "f_rstd_phase_hold",
        cfg: f,
        table: table(false),
        sim: sim(250.0, 20.0),
        manual: false,
        moves: vec![],
        max_chunks: 400,
    });
    out
}

/// FNV-1a over a stream of 64-bit words.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn word(&mut self, w: u64) {
        for b in w.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

fn bits(v: f64) -> String {
    format!("{:016x}", v.to_bits())
}

/// One line of JSON with its keys sorted.
fn line(fields: BTreeMap<&'static str, String>) -> String {
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    format!("{{{}}}", body.join(","))
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// The hash, the ticks seen, the sampled ticks and the ticks the notch held.
type Seen = (Fnv, u64, Vec<TickObservation>, u64);

/// Run one fixture and write its part of the trace.
fn run(fx: Fixture) -> Vec<String> {
    let seen: Arc<Mutex<Seen>> = Arc::new(Mutex::new((Fnv::new(), 0, Vec::new(), 0)));
    let sink = seen.clone();
    let seed = fx.sim.seed;
    let mut svc = SineService::new(
        fx.cfg,
        fx.table,
        Box::new(fx.sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        seed,
        None,
    )
    .expect("the fixture builds")
    .with_tick_observer(Box::new(move |o: &TickObservation| {
        let mut g = sink.lock().unwrap();
        let (h, n, samples, notched) = &mut *g;
        *notched += o.notch_binding as u64;
        h.word(o.tick);
        for v in [
            o.freq_hz,
            o.level_db,
            o.measured,
            o.drive,
            o.servo_step_db,
            o.servo_level_db,
        ] {
            h.word(v.to_bits());
        }
        h.word(o.servo_faulted as u64 | (o.notch_binding as u64) << 1);
        if *n % SAMPLE_EVERY == 0 {
            samples.push(*o);
        }
        *n += 1;
    }));
    if fx.manual {
        svc.set_run_mode(RunMode::Manual).unwrap();
    }
    svc.assess_feasibility().unwrap();
    svc.arm().unwrap();
    svc.run_sysid(4096).unwrap();
    assert!(
        svc.coherence_gate().unwrap(),
        "{}: the gate passes",
        fx.name
    );
    let mut chunk = 0u64;
    while matches!(svc.state(), RunState::Engaged | RunState::FullLevel) && chunk < fx.max_chunks {
        for (_, m) in fx.moves.iter().filter(|(at, _)| *at == chunk) {
            match *m {
                Move::Level(i) => svc.jump_to_level(i).unwrap(),
                Move::W(w) => svc.set_w(w).unwrap(),
                Move::Stop => svc.request_stop().unwrap(),
            }
        }
        if !svc.run_chunk().unwrap_or(false) {
            break;
        }
        chunk += 1;
    }
    if matches!(svc.state(), RunState::Engaged | RunState::FullLevel) {
        svc.stop().unwrap();
    }

    let g = seen.lock().unwrap();
    let (h, n, samples, notched) = &*g;
    let mut out = Vec::new();
    for o in samples {
        let mut m = BTreeMap::new();
        m.insert("fixture", quoted(fx.name));
        m.insert("tick", o.tick.to_string());
        m.insert("freq_hz", quoted(&bits(o.freq_hz)));
        m.insert("level_db", quoted(&bits(o.level_db)));
        m.insert("measured", quoted(&bits(o.measured)));
        m.insert("drive", quoted(&bits(o.drive)));
        m.insert("servo_step_db", quoted(&bits(o.servo_step_db)));
        m.insert("servo_level_db", quoted(&bits(o.servo_level_db)));
        m.insert("servo_faulted", o.servo_faulted.to_string());
        m.insert("notch", o.notch_binding.to_string());
        out.push(line(m));
    }
    let mut m = BTreeMap::new();
    m.insert("fixture", quoted(fx.name));
    m.insert("end", quoted(&format!("{:?}", svc.state())));
    m.insert(
        "trip",
        quoted(&format!("{:?}", svc.trip().map(|t| t.kind.clone()))),
    );
    m.insert("ticks", n.to_string());
    m.insert("hash", quoted(&format!("{:016x}", h.0)));
    // Whether the fixture that is meant to bind did: part of the record.
    m.insert("notch_ticks", notched.to_string());
    out.push(line(m));
    out
}

fn record() -> String {
    let mut lines = Vec::new();
    for fx in fixtures() {
        lines.extend(run(fx));
    }
    lines.join("\n") + "\n"
}

/// Compare two traces off the recording platform: every line by its keys,
/// numbers held to relative 1e-6, hashes not compared.
fn close_enough(got: &str, want: &str) -> Result<(), String> {
    let g: Vec<&str> = got.lines().collect();
    let w: Vec<&str> = want.lines().collect();
    if g.len() != w.len() {
        return Err(format!("{} lines, the trace has {}", g.len(), w.len()));
    }
    for (i, (a, b)) in g.iter().zip(&w).enumerate() {
        let a: serde_json::Value = serde_json::from_str(a).unwrap();
        let b: serde_json::Value = serde_json::from_str(b).unwrap();
        for (k, bv) in b.as_object().unwrap() {
            let av = &a[k];
            if k == "hash" {
                continue;
            }
            let same = match (av.as_str(), bv.as_str()) {
                (Some(x), Some(y)) if x.len() == 16 && y.len() == 16 && k != "fixture" => {
                    let fx = f64::from_bits(u64::from_str_radix(x, 16).unwrap());
                    let fy = f64::from_bits(u64::from_str_radix(y, 16).unwrap());
                    (fx - fy).abs() <= 1e-6 * fy.abs().max(1e-12)
                }
                _ => av == bv,
            };
            if !same {
                return Err(format!("line {}: {k} is {av}, the trace has {bv}", i + 1));
            }
        }
    }
    Ok(())
}

#[test]
fn the_builtin_control_matches_its_recorded_trace() {
    let got = record();
    if std::env::var_os("HERTZFORGE_RECORD_SINE_TRACE").is_some() {
        std::fs::create_dir_all(std::path::Path::new(TRACE_PATH).parent().unwrap()).unwrap();
        std::fs::write(TRACE_PATH, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(TRACE_PATH).expect("the recorded trace");
    if RECORDING_PLATFORM {
        if got != want {
            let first = got
                .lines()
                .zip(want.lines())
                .position(|(a, b)| a != b)
                .map(|i| i + 1);
            panic!(
                "the run moved from its recorded trace (first differing line {first:?}):\n\
                 got  {:?}\nwant {:?}",
                first.and_then(|i| got.lines().nth(i - 1)),
                first.and_then(|i| want.lines().nth(i - 1))
            );
        }
    } else if let Err(e) = close_enough(&got, &want) {
        panic!("the run moved from its recorded trace: {e}");
    }
}
