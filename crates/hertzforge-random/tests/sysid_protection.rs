//! System identification runs under the control loop's protection, at a level
//! set by the reference (first Random bench run on NI, 2026-11-01, finding 1).
//!
//! On the bench, a 0.4 g accel abort sat under 48 consecutive DMM blocks
//! peaking up to 0.608 g for the whole 5.2 s identification and nothing
//! tripped: the raw-ring watchdog's trip only shortened a pretest segment, and
//! the kinematic monitor was never fed before the control loop. The
//! identification also ran at the test level it called "-20 dB", because its
//! level was 10^(-20/20) of a fixed 0.05 base, bounded only by the pretest
//! ceiling.
//!
//! - (a) A tight accel abort trips DURING identification: `run_sysid` returns
//!   with the run `Aborted` and a kinematic trip at loop 0, and the gate does
//!   not engage. Red before the fix: the run reached the coherence gate.
//! - (b) A drive abort below the identification drive trips before anything
//!   is driven.
//! - (c) The referenced identification puts the control response at the
//!   reference rms -20 dB, measured independently from the samples the engine
//!   acquired, and never drives above its ceiling.
//! - (d) A level the ceiling cannot reach is refused by name, with the facts.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_random::service::config::{LevelSchedule, RandomTestConfig};
use hertzforge_random::service::engine::BlockObserver;
use hertzforge_random::service::report::RunState;
use hertzforge_random::service::{RandomService, ServiceError};
use hertzforge_random::{LoopConfig, TripKind};
use std::sync::{Arc, Mutex};

fn loop_cfg() -> LoopConfig {
    LoopConfig::demo_default()
}

/// The demo Sim plant (two in-band modes), one input, one output.
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

fn table(control_abort_g: Option<f64>) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    ctrl.abort.accel_g = control_abort_g;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

fn flat(level: f64) -> BreakpointTable {
    let mut bt = BreakpointTable::new("flat");
    bt.push(BreakpointRow::psd(20.0, level));
    bt.push(BreakpointRow::psd(2000.0, level));
    bt
}

fn service(cfg: RandomTestConfig, t: ChannelTable) -> RandomService {
    let lc = loop_cfg();
    let b = backend(&lc);
    let mut s = RandomService::new(
        cfg,
        t,
        lc,
        b,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(std::env::temp_dir().join("hertzforge-sysid-protection")),
    )
    .expect("service builds");
    s.set_review_after_sysid(false);
    s
}

fn armed(mut s: RandomService) -> RandomService {
    s.assess_feasibility().expect("feasibility");
    s.arm().expect("arm");
    assert_eq!(s.state(), RunState::SysId);
    s
}

/// The legacy absolute identification (`sysid_level_db` of the 0.05 base, the
/// default 1 V ceiling) drives this plant well past a 0.05 g accel abort.
fn legacy_config() -> RandomTestConfig {
    let mut cfg = RandomTestConfig::siso(flat(1.0e-2), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.pretest_max_vrms = Some(1.0);
    cfg
}

#[test]
fn a_kinematic_abort_trips_during_identification() {
    let mut s = armed(service(legacy_config(), table(Some(0.05))));
    s.run_sysid(3)
        .expect("a protection trip is an outcome, not an error");
    assert_eq!(
        s.state(),
        RunState::Aborted,
        "the 0.05 g abort must end the identification (log: {:?})",
        s.log().events().iter().rev().take(4).collect::<Vec<_>>()
    );
    let trip = s.trip().expect("trip recorded");
    assert_eq!(trip.at_loop, 0, "it tripped before any control loop");
    match &trip.kind {
        TripKind::Kinematic {
            channel_index,
            axis,
            value,
            limit,
            ..
        } => {
            assert_eq!(*channel_index, 1);
            assert_eq!(axis, "Accel");
            assert!(value > limit, "{value} against {limit}");
        }
        other => panic!("expected a kinematic trip, got {other:?}"),
    }
    assert!(!s.coherence_gate().expect("the gate refuses nothing"));
    assert_eq!(s.state(), RunState::Aborted, "and nothing engages");
}

#[test]
fn run_auto_ends_aborted_when_identification_trips() {
    let mut s = service(legacy_config(), table(Some(0.05)));
    let end = s.run_auto(3, 50).expect("run_auto");
    assert_eq!(end, RunState::Aborted);
    assert!(matches!(
        s.trip().map(|t| &t.kind),
        Some(TripKind::Kinematic { .. })
    ));
}

#[test]
fn a_drive_abort_below_the_identification_drive_trips_before_driving() {
    let mut cfg = legacy_config();
    // The legacy level is 0.005 V rms per root line-width over the band's lines.
    cfg.drive_abort_vrms = Some(0.02);
    let mut s = armed(service(cfg, table(None)));
    s.run_sysid(3).expect("outcome, not error");
    assert_eq!(s.state(), RunState::Aborted);
    match s.trip().map(|t| &t.kind) {
        Some(TripKind::DriveVrms {
            realized_vrms,
            limit_vrms,
        }) => {
            let lc = loop_cfg();
            let (lo, hi) = lc.band_bins();
            let legacy = 0.005 * ((hi - lo) as f64 * lc.df()).sqrt();
            assert!(
                (realized_vrms - legacy).abs() < 1e-9,
                "{realized_vrms} vs {legacy}"
            );
            assert_eq!(*limit_vrms, 0.02);
        }
        other => panic!("expected a drive trip, got {other:?}"),
    }
}

/// The in-band rms of `samples` on the loop's grid (an estimate made here from
/// the acquired samples, not the controller's own).
fn inband_rms(samples: &[f64], lc: &LoopConfig) -> f64 {
    let (_f, psd) = hertzforge_dsp::spectrum::psd_welch(samples, lc.fs_hz, lc.nfft, lc.overlap);
    let (lo, hi) = lc.band_bins();
    (psd[lo..hi].iter().sum::<f64>() * lc.df()).sqrt()
}

#[test]
fn the_referenced_identification_drives_the_control_to_the_reference_less_20_db() {
    // A reference 20 dB under the demo plant's habit, so the legacy absolute
    // level (which lands near -20 dB on THIS plant at 1e-2 g^2/Hz by
    // coincidence) would sit at the reference itself, as it did on the bench.
    let mut cfg = RandomTestConfig::siso(flat(1.0e-4), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.pretest_max_vrms = Some(5.0);
    cfg.pretest_reference_db = Some(-20.0);
    cfg.pretest_start_vrms = Some(0.001);
    let segments = 3;
    let lc = loop_cfg();
    let mut s = armed(service(cfg, table(None)));
    // Every frame the engine acquired, through the per-tick display tap.
    let seen: Arc<Mutex<Vec<f64>>> = Arc::new(Mutex::new(Vec::new()));
    let tap = seen.clone();
    let obs: BlockObserver = Arc::new(move |input: &[f32], n: usize| {
        let mut v = tap.lock().unwrap();
        let ch = input.len().checked_div(n).unwrap_or(1).max(1);
        v.extend((0..n).map(|f| f64::from(input[f * ch])));
    });
    s.set_block_observer(Some(obs));
    let target = s.full_target_grms() * 0.1;
    s.run_sysid(segments).expect("sysid");
    assert_eq!(s.state(), RunState::CoherenceGate);

    // The identification record is the last `segments` segments (the level steps
    // came first); skip the first segment of it, which still carries the last
    // step's transient through the pipeline lag.
    let all = seen.lock().unwrap().clone();
    let w = lc.window_len();
    assert!(all.len() >= (segments + 1) * w, "{} frames seen", all.len());
    let record = &all[all.len() - (segments - 1) * w..];
    let measured = inband_rms(record, &lc);
    let err_db = 20.0 * (measured / target).log10();
    println!("target {target:.5} measured {measured:.5} ({err_db:+.2} dB)");
    assert!(
        err_db.abs() <= 1.5,
        "the identification response is {err_db:+.2} dB from reference -20 dB"
    );
    // The level steps are logged, and none drove above the 5 V rms ceiling.
    let steps: Vec<String> = s
        .log()
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            hertzforge_random::service::report::EventKind::Note { text }
                if text.starts_with("identification level step") =>
            {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    assert!(!steps.is_empty(), "the level steps are in the log");
    assert!(steps.len() <= 6, "it settled in a few steps: {steps:?}");
}

#[test]
fn a_level_the_ceiling_cannot_reach_is_refused_by_name() {
    let mut cfg = RandomTestConfig::siso(flat(1.0e-2), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.pretest_max_vrms = Some(0.002);
    cfg.pretest_reference_db = Some(-20.0);
    cfg.pretest_start_vrms = Some(0.001);
    let mut s = armed(service(cfg, table(None)));
    let target = s.full_target_grms() * 0.1;
    match s.run_sysid(3) {
        Err(ServiceError::SysIdLevelUnreachable {
            level_db,
            target_rms,
            reached_rms,
            drive_vrms,
            ceiling_vrms,
            ..
        }) => {
            assert_eq!(level_db, -20.0);
            assert!((target_rms - target).abs() < 1e-9);
            assert!(reached_rms < target_rms, "{reached_rms} vs {target_rms}");
            assert!((drive_vrms - 0.002).abs() < 1e-12, "{drive_vrms}");
            assert!((ceiling_vrms - 0.002).abs() < 1e-12);
        }
        other => panic!("expected the named refusal, got {other:?}"),
    }
    let e = ServiceError::SysIdLevelUnreachable {
        level_db: -20.0,
        target_rms: 0.5,
        reached_rms: 0.1,
        drive_vrms: 0.2,
        ceiling_vrms: 0.2,
        probes: 3,
    }
    .to_string();
    assert!(e.contains("could not reach its level"), "{e}");
}
