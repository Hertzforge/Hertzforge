//! A drive row listed BEFORE the inputs does not shift them (found by the control
//! service; worked around there by handing the controller its table with drive
//! rows last).
//!
//! The acquired frame holds the enabled INPUT rows only. `slot_of` and the
//! kinematic monitor counted an enabled drive row as a place in it, so with a
//! table of [drive, control, reference] the control loop read the reference's
//! samples and the monitor walked the frame with a stride of three. The engine's
//! watchdog had the same defect and was fixed on 2026-07-16; this is the
//! service's half.
//!
//! The Sim makes the two inputs tell apart: a control row reads the plant's
//! response, a reference row reads the commanded drive.
//!
//! - The loop controls the CONTROL row: its own samples, measured here from the
//!   engine's acquired frames, sit on the reference spectrum. Red before the fix:
//!   the loop drove the drive's own spectrum onto the reference and the response
//!   sat off it by the plant's gain.
//! - A kinematic limit on the reference row judges the reference row: set between
//!   the drive's peaks and the response's, it does not trip. Red before the fix:
//!   the monitor read response samples into the reference's limiter and tripped.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_random::service::config::{LevelSchedule, RandomTestConfig, RunMode};
use hertzforge_random::service::engine::BlockObserver;
use hertzforge_random::service::report::RunState;
use hertzforge_random::service::RandomService;
use hertzforge_random::LoopConfig;
use std::sync::{Arc, Mutex};

fn table(reference_abort_g: Option<f64>) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut drv = Channel::new(1);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    let mut ctrl = Channel::new(2);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 1000.0;
    let mut reference = Channel::new(3);
    reference.role = ChannelRole::Reference;
    reference.units = Units::AccelG;
    reference.range_max = 1000.0;
    reference.abort.accel_g = reference_abort_g;
    // The drive row FIRST.
    t.channels.push(drv);
    t.channels.push(ctrl);
    t.channels.push(reference);
    t
}

fn profile() -> BreakpointTable {
    let mut bt = BreakpointTable::new("demo-profile");
    bt.push(BreakpointRow::psd(20.0, 0.01));
    bt.push(BreakpointRow::psd(80.0, 0.04));
    bt.push(BreakpointRow::psd(350.0, 0.04));
    bt.push(BreakpointRow::psd(2000.0, 0.007));
    bt
}

struct Run {
    svc: RandomService,
    /// Every acquired frame, interleaved [control, reference].
    seen: Arc<Mutex<Vec<f32>>>,
}

fn run(reference_abort_g: Option<f64>) -> Run {
    let lc = LoopConfig::demo_default();
    let backend = hertzforge_hal_sim::SimConfig::new()
        .sample_rate_hz(lc.fs_hz)
        .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
        .noise_level(2e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .seed(0x5A5A_1234)
        .max_input_channels(2)
        .max_output_channels(1)
        .build();
    let mut cfg = RandomTestConfig::siso(profile(), 2);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Manual,
        start_db: -6.0,
        table_db: vec![-6.0, 0.0],
        dwell_s: vec![600.0],
    };
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (48.0, 48.0);
    cfg.alarm_db = (48.0, 48.0);
    cfg.soft_start_db = 0.0;
    let mut svc = RandomService::new(
        cfg,
        table(reference_abort_g),
        lc,
        Box::new(backend),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(std::env::temp_dir().join("hertzforge-drive-row-first")),
    )
    .expect("service builds");
    svc.set_review_after_sysid(false);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tap = seen.clone();
    let obs: BlockObserver = Arc::new(move |input: &[f32], _n: usize| {
        tap.lock().unwrap().extend_from_slice(input);
    });
    svc.set_block_observer(Some(obs));
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    Run { svc, seen }
}

/// Mean |dB| between a channel's spectrum (from the acquired frames) and the
/// reference at the run's level, over the control band.
fn control_error_db(r: &Run, slot: usize, frames: usize) -> f64 {
    let lc = LoopConfig::demo_default();
    let all = r.seen.lock().unwrap().clone();
    let x: Vec<f64> = all
        .chunks_exact(2)
        .rev()
        .take(frames)
        .map(|f| f64::from(f[slot]))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let (_f, psd) = hertzforge_dsp::spectrum::psd_welch(&x, lc.fs_hz, lc.nfft, lc.overlap);
    let rep = r.svc.report_data();
    let scale = 10f64.powf(r.svc.schedule_level_db() / 10.0);
    let (lo, hi) = lc.band_bins();
    let errs: Vec<f64> = (lo + 2..hi - 2)
        .map(|k| 10.0 * (psd[k] / (rep.target_psd.psd[k] * scale)).log10())
        .collect();
    errs.iter().map(|e| e.abs()).sum::<f64>() / errs.len() as f64
}

#[test]
fn the_loop_controls_the_control_row_when_the_drive_row_comes_first() {
    let mut r = run(None);
    assert!(r.svc.coherence_gate().expect("gate"));
    for _ in 0..25 {
        assert!(r.svc.loop_tick().expect("tick"), "{:?}", r.svc.trip());
    }
    let lc = LoopConfig::demo_default();
    let frames = 4 * lc.window_len();
    let control = control_error_db(&r, 0, frames);
    let reference = control_error_db(&r, 1, frames);
    println!("mean |error| control row {control:.2} dB, reference row {reference:.2} dB");
    assert!(
        control < 1.5,
        "the control row sits {control:.2} dB (mean) off the reference spectrum"
    );
}

#[test]
fn a_limit_on_the_row_after_the_inputs_judges_that_row() {
    // At -6 dB on this seeded plant the drive (the reference row's samples)
    // peaks at 7.3 and the response at 10.4: a limit of 9 on the reference row
    // is never reached, and the test checks both peaks.
    let mut r = run(Some(9.0));
    assert!(r.svc.coherence_gate().expect("gate"));
    for _ in 0..12 {
        if !r.svc.loop_tick().expect("tick") {
            break;
        }
    }
    let all = r.seen.lock().unwrap().clone();
    let peak = |slot: usize| {
        all.chunks_exact(2)
            .map(|f| f[slot].abs())
            .fold(0.0f32, f32::max)
    };
    println!(
        "peaks: control {:.2}, reference {:.2}; state {:?} trip {:?}",
        peak(0),
        peak(1),
        r.svc.state(),
        r.svc.trip()
    );
    assert!(
        peak(1) < 9.0 && peak(0) > 9.0,
        "the limit must sit between the two"
    );
    assert_eq!(
        r.svc.state(),
        RunState::Engaged,
        "the reference row never reached its limit, yet the run ended: {:?}",
        r.svc.trip()
    );
}
