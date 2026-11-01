//! A level change and the controlled stop move the drive smoothly, block by
//! block, within the ramp time (first Random bench run on NI, 2026-11-01,
//! finding 2).
//!
//! The bench saw a staircase: the stop went -6, -23.07, -40.13, -46 dB at
//! 1.28 s intervals (17 dB steps), with the whole first loop after the request
//! still at the old level, and a -1 dB manual step settled over three loops.
//! The level walked once per control loop and the step reached the drive one
//! loop late.
//!
//! This records every drive sample the station is handed (the Sim's block is
//! one frame) and reads its rms over consecutive quarter-second windows, the way
//! a digitizer on the drive would. Red before the fix: 6 dB (level change) and
//! 20 dB (stop) jumps between two windows at segment boundaries, and the first
//! second after either request at the old level.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_random::service::config::{LevelSchedule, RandomTestConfig, RunMode};
use hertzforge_random::service::report::RunState;
use hertzforge_random::service::RandomService;
use hertzforge_random::LoopConfig;
use std::sync::{Arc, Mutex};

/// A quarter second at the demo's 8192 Hz: long enough that a window's rms of
/// this band-limited drive scatters by about 0.3 dB, short against a 3 s ramp.
const WINDOW: usize = 2048;

/// The Sim, with every accepted drive sample recorded in order.
struct DriveTap {
    inner: SimBackend,
    drive: Arc<Mutex<Vec<f32>>>,
}

impl Backend for DriveTap {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
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
        let n = self.inner.write_drive(drive)?;
        self.drive.lock().unwrap().extend_from_slice(&drive[..n]);
        Ok(n)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
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

/// The drive's rms in dB over consecutive windows.
fn window_db(drive: &[f32]) -> Vec<f64> {
    drive
        .chunks_exact(WINDOW)
        .map(|w| {
            let ms = w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / WINDOW as f64;
            10.0 * ms.max(1e-30).log10()
        })
        .collect()
}

fn max_jump(db: &[f64]) -> (f64, usize) {
    db.windows(2)
        .enumerate()
        .map(|(i, w)| ((w[1] - w[0]).abs(), i))
        .fold((0.0, 0), |a, b| if b.0 > a.0 { b } else { a })
}

#[test]
fn a_level_change_and_the_controlled_stop_ramp_block_by_block() {
    let lc = LoopConfig::demo_default();
    let fs = lc.fs_hz;
    let seg_s = lc.segment_seconds();
    let drive = Arc::new(Mutex::new(Vec::new()));
    let backend = DriveTap {
        inner: SimConfig::new()
            .sample_rate_hz(fs)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .noise_level(2e-4)
            .feedthrough(0.35)
            .shared_clock(true)
            .seed(0x5A5A_1234)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
        drive: drive.clone(),
    };
    let mut cfg = RandomTestConfig::siso(profile(), 1);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Manual,
        start_db: -12.0,
        table_db: vec![-12.0, -6.0, 0.0],
        dwell_s: vec![600.0],
    };
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (24.0, 24.0);
    cfg.alarm_db = (24.0, 24.0);
    cfg.soft_start_db = 0.0;
    cfg.level_ramp_s = 3.0;
    cfg.shutdown_ramp_s = 3.0;
    let mut svc = RandomService::new(
        cfg,
        table(),
        lc,
        Box::new(backend),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(std::env::temp_dir().join("hertzforge-smooth-ramp")),
    )
    .expect("service builds");
    svc.set_review_after_sysid(false);
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));
    for _ in 0..6 {
        assert!(svc.loop_tick().expect("tick"));
    }

    // -12 to 0 dB, ramped over 3 s: two segments of the demo loop.
    let at_step = drive.lock().unwrap().len();
    svc.jump_to_table_level(2).expect("to full level");
    for _ in 0..5 {
        assert!(svc.loop_tick().expect("tick"));
    }
    assert!(svc.schedule_level_db().abs() < 1e-9);

    // The controlled stop: 40 dB over 3 s, then the run ends.
    let at_stop = drive.lock().unwrap().len();
    svc.request_stop().expect("stop");
    let mut ticks = 0;
    while svc.loop_tick().expect("tick") {
        ticks += 1;
        assert!(ticks < 10, "the stop never finished");
    }
    assert_eq!(svc.state(), RunState::Stopped);

    let d = drive.lock().unwrap().clone();
    let w0 = at_step / WINDOW + 1;
    let ws = at_stop / WINDOW + 1;
    let db = window_db(&d);
    let windows_per_s = fs / WINDOW as f64;

    // The level change: rising from its first second, smooth, and the full 12 dB.
    let step_part = &db[w0..ws - 1];
    let before = db[w0 - 5..w0 - 1].iter().sum::<f64>() / 4.0;
    let one_s = windows_per_s as usize;
    let first = step_part[..one_s].iter().sum::<f64>() / one_s as f64;
    println!(
        "level change: {:+.2} dB over its first second",
        first - before
    );
    assert!(
        first > before + 1.5,
        "the first second after the level change is still at the old level ({:+.2} dB)",
        first - before
    );
    let (jump, at) = max_jump(step_part);
    println!("level change: largest window-to-window step {jump:.2} dB at window {at}");
    assert!(
        jump < 3.5,
        "the level change jumped {jump:.2} dB between two windows"
    );
    let after = db[ws - 5..ws - 1].iter().sum::<f64>() / 4.0;
    assert!(
        (after - before - 12.0).abs() < 1.0,
        "+12 dB came out as {:+.2} dB",
        after - before
    );

    // The stop: falling from its first second, smooth (13.3 dB/s is 3.3 dB a
    // window), and 35 dB down within the ramp time (plus a window).
    let pre = after;
    let stop_part = &db[ws..];
    let first = stop_part[..one_s].iter().sum::<f64>() / one_s as f64;
    println!("stop: {:+.2} dB over its first second", first - pre);
    assert!(
        first < pre - 4.0,
        "the first second after the stop is still at the old level ({:+.2} dB)",
        first - pre
    );
    let (jump, at) = max_jump(stop_part);
    println!("stop: largest window-to-window step {jump:.2} dB at window {at}");
    assert!(
        jump < 6.0,
        "the stop jumped {jump:.2} dB between two windows"
    );
    let reach = stop_part.iter().position(|v| *v < pre - 35.0);
    let reach_s = reach.map(|i| i as f64 / windows_per_s);
    println!("stop: 35 dB down after {reach_s:?} s (segments of {seg_s:.2} s)");
    assert!(
        reach_s.is_some_and(|s| s <= 3.0 + 1.0 / windows_per_s),
        "35 dB down at {reach_s:?} s, not within the 3 s ramp"
    );
}
