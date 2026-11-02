//! The accumulated control estimate is kept, scaled, across a level change, and
//! a real excursion at the new level still aborts (2026-11-01, finding 5).
//!
//! Every loop of a level ramp used to clear the accumulated estimate, so the
//! first loop judged after any ramp was one loop's Welch estimate. At 2 averages
//! per loop that is 4 degrees of freedom: about one line in eleven reads below a
//! -6 dB abort floor by chance, and the 10-line envelope abort tripped at the end
//! of a 3 s soft start on the simulator every time. At 8 averages it showed as
//! 10 to 12 lines in alarm for one loop after every level change on the bench.
//!
//! - (a) 2 averages, a 9 dB soft start over 3 s and two rungs: no envelope trip,
//!   and no lines in alarm on the first judged loop after each change.
//! - (b) The estimate is the scaled one: a level change does not reset its
//!   degrees of freedom.
//! - (c) A real excursion: the plant's response rises 10 dB the moment a level
//!   change arrives. The envelope abort still trips on the first judged loop at
//!   8 averages, as it did when the estimate was cleared.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_random::service::config::{LevelSchedule, RandomTestConfig, RunMode};
use hertzforge_random::service::report::{EventKind, RunState};
use hertzforge_random::service::RandomService;
use hertzforge_random::{LoopConfig, TripKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The Sim, whose response can be made 10 dB louder at a moment the test picks.
struct Excursion {
    inner: SimBackend,
    on: Arc<AtomicBool>,
}

impl Backend for Excursion {
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
        let r = self.inner.read_input(out)?;
        if self.on.load(Ordering::Relaxed) {
            let g = 10f32.powf(10.0 / 20.0);
            for x in out.iter_mut() {
                *x *= g;
            }
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
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
    ctrl.range_max = 1000.0;
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

fn service(averages: usize, mode: RunMode, on: Arc<AtomicBool>) -> RandomService {
    let mut lc = LoopConfig::demo_default();
    lc.blocks_per_window = averages;
    let backend = Excursion {
        inner: SimConfig::new()
            .sample_rate_hz(lc.fs_hz)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .noise_level(2e-4)
            .feedthrough(0.35)
            .shared_clock(true)
            .seed(0x5A5A_1234)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
        on,
    };
    let mut cfg = RandomTestConfig::siso(profile(), 1);
    cfg.schedule = LevelSchedule {
        mode,
        start_db: -12.0,
        table_db: vec![-12.0, -6.0, 0.0],
        dwell_s: vec![4.0, 4.0, 0.0],
    };
    cfg.full_level_duration_s = 4.0;
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.level_ramp_s = 3.0;
    cfg.shutdown_ramp_s = 3.0;
    cfg.soft_start_db = 9.0;
    cfg.divergence_loops = 1_000;
    let mut s = RandomService::new(
        cfg,
        table(),
        lc,
        Box::new(backend),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(std::env::temp_dir().join("hertzforge-estimate-across-levels")),
    )
    .expect("service builds");
    s.set_review_after_sysid(false);
    s.assess_feasibility().expect("feasibility");
    s.arm().expect("arm");
    s.run_sysid(3).expect("sysid");
    assert!(s.coherence_gate().expect("gate"));
    s
}

#[test]
fn at_two_averages_a_soft_start_and_two_rungs_do_not_trip_the_envelope() {
    let mut s = service(2, RunMode::Auto, Arc::new(AtomicBool::new(false)));
    let mut ticks = 0;
    while matches!(s.state(), RunState::Engaged | RunState::FullLevel) {
        if !s.loop_tick().expect("tick") {
            break;
        }
        ticks += 1;
        assert!(ticks < 200, "the schedule never finished");
    }
    let alarms: Vec<usize> = s
        .log()
        .events()
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::EnvelopeAlarm { lines, .. } => Some(lines),
            _ => None,
        })
        .collect();
    println!(
        "2 averages: {ticks} loops, end {:?}, trip {:?}, alarm loops {:?}",
        s.state(),
        s.trip(),
        alarms
    );
    assert_eq!(
        s.state(),
        RunState::Stopped,
        "the schedule must complete without an envelope trip: {:?}",
        s.trip()
    );
    // One loop's scatter after every change used to light the alarm on a dozen or
    // more of the 91 lines (17 to 27 here); the carried estimate does not.
    let worst = alarms.iter().copied().max().unwrap_or(0);
    assert!(worst < 5, "{worst} lines in alarm in one loop: {alarms:?}");
}

#[test]
fn a_ten_db_excursion_at_a_new_level_still_aborts_on_its_first_judged_loop() {
    let on = Arc::new(AtomicBool::new(false));
    let mut s = service(8, RunMode::Manual, on.clone());
    // Settle at the start level after the soft start.
    for _ in 0..12 {
        assert!(s.loop_tick().expect("tick"));
    }
    assert!((s.schedule_level_db() + 12.0).abs() < 1e-9);
    s.jump_to_table_level(1).expect("to -6 dB");
    // Ramp until the level arrives, then make the plant 10 dB louder.
    let mut guard = 0;
    while (s.schedule_level_db() + 6.0).abs() > 1e-9 {
        assert!(s.loop_tick().expect("tick"));
        guard += 1;
        assert!(guard < 10);
    }
    on.store(true, Ordering::Relaxed);
    let mut judged = 0;
    let mut loops = 0;
    while s.loop_tick().expect("tick") {
        loops += 1;
        judged += 1;
        assert!(loops < 10, "the excursion never aborted");
    }
    println!(
        "excursion: aborted after {} loop(s), trip {:?}",
        judged + 1,
        s.trip()
    );
    assert_eq!(s.state(), RunState::Aborted);
    assert!(
        matches!(s.trip().map(|t| &t.kind), Some(TripKind::Envelope { .. })),
        "{:?}",
        s.trip()
    );
    assert_eq!(
        judged,
        0,
        "the excursion took {} loops to abort; the first loop that measures it must catch \
         it, as when every level change restarted the estimate",
        judged + 1
    );
}
