//! The station's own overload report feeds the run's per-channel overload flag
//! (2026-11-01, finding 6).
//!
//! On the NI 9234 the driver's overloaded-channels query is unsupported
//! (-200452), so every backend now checks its inputs in software against each
//! channel's configured range (`Backend::input_overloads`, 98 %). Here a station
//! reports an overload on its second input while every sample stays far below
//! the controller's own 90 % proxy: the run latches that channel, and only it.
//!
//! RED-MUTATION: drop the station half of `RandomService::check_overloads` and
//! the latch stays clear.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_random::service::config::{LevelSchedule, RandomTestConfig};
use hertzforge_random::service::report::EventKind;
use hertzforge_random::service::RandomService;
use hertzforge_random::LoopConfig;

/// The Sim, reporting its second input overloaded once it has been read.
struct Overloaded {
    inner: SimBackend,
    reads: u64,
}

impl Backend for Overloaded {
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
        self.reads += 1;
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        vec![false, self.reads > 0]
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

#[test]
fn a_station_overload_report_latches_that_channel() {
    let lc = LoopConfig::demo_default();
    let backend = Overloaded {
        inner: SimConfig::new()
            .sample_rate_hz(lc.fs_hz)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .noise_level(2e-4)
            .feedthrough(0.35)
            .shared_clock(true)
            .seed(0x5A5A_1234)
            .max_input_channels(2)
            .max_output_channels(1)
            .build(),
        reads: 0,
    };
    let mut t = ChannelTable::new();
    for (i, role) in [(1, ChannelRole::Control), (2, ChannelRole::Monitor)] {
        let mut c = Channel::new(i);
        c.role = role;
        c.units = Units::AccelG;
        c.range_max = 1e6; // the 90 % proxy is never reached
        t.channels.push(c);
    }
    let mut d = Channel::new(3);
    d.role = ChannelRole::Drive;
    t.channels.push(d);
    let mut bt = BreakpointTable::new("flat");
    bt.push(BreakpointRow::psd(20.0, 0.01));
    bt.push(BreakpointRow::psd(2000.0, 0.01));
    let mut cfg = RandomTestConfig::siso(bt, 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    let mut svc = RandomService::new(
        cfg,
        t,
        lc,
        Box::new(backend),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(std::env::temp_dir().join("hertzforge-station-overload")),
    )
    .expect("service builds");
    svc.set_review_after_sysid(false);
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert_eq!(
        svc.status().overload_latches,
        vec![(1, false), (2, true)],
        "identification already reads the station's report"
    );
    let latched: Vec<u32> = svc
        .log()
        .events()
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::OverloadLatch { channel_index } => Some(channel_index),
            _ => None,
        })
        .collect();
    assert_eq!(latched, [2], "logged once, for the monitor only");
}
