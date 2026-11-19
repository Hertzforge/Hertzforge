//! What a hand-steered sine must never do, each against the run that would do it:
//! nothing raises or moves the drive once a stop has begun (G1); a sweep never runs ahead
//! of the tone, and a hold leaves the tone where it is (G2); no generated sample past
//! the ceiling reaches the station (G3); and a controlled stop says why it began (G4).

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_engine::{AbortAxis, AbortTrip};
use hertzforge_explorer_host::service::{
    DriveTrip, ExplorerHostConfig, ExplorerHostService, RunState, ServiceError,
};
use hertzforge_explorer_host::slew::SlewLimits;
use hertzforge_explorer_host::step::AmplitudeRamp;
use hertzforge_explorer_host::{FrequencyPart, LevelPart, StopCause, SweepDirection};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_module_abi::StationClass;
use std::sync::{Arc, Mutex};

const FS: f64 = 8_192.0;
const BLOCK: usize = 128;

/// A simulated station that records every drive sample it accepts.
struct Recording {
    inner: Box<dyn Backend>,
    drive: Arc<Mutex<Vec<f32>>>,
}

impl Backend for Recording {
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
        t: &ChannelTable,
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
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(d)?;
        self.drive
            .lock()
            .unwrap()
            .extend_from_slice(&d[..n.min(d.len())]);
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
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    c.range_max = 10.0;
    t.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    t.channels.push(d);
    t
}

struct Rig {
    svc: ExplorerHostService,
    drive: Arc<Mutex<Vec<f32>>>,
}

fn rig(freq_oct_per_s: f64, drive_db_per_s: f64) -> Rig {
    let drive = Arc::new(Mutex::new(Vec::new()));
    let plant = hertzforge_hal_sim::SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .feedthrough(1.0)
        .noise_level(0.0)
        .transport_delay_samples(0)
        .build();
    let cfg = ExplorerHostConfig {
        fs_hz: FS,
        band_hz: (20.0, 2_000.0),
        start_hz: 100.0,
        max_drive_vpk: 0.5,
        slew: SlewLimits {
            freq_oct_per_s,
            drive_db_per_s,
        },
        ramp: AmplitudeRamp::default(),
        display_nfft: 1_024,
        ring_blocks: 16,
    };
    let mut svc = ExplorerHostService::new(
        cfg,
        table(),
        Box::new(Recording {
            inner: Box::new(plant),
            drive: drive.clone(),
        }),
        StationClass::HertzforgeSim,
    )
    .unwrap()
    .with_block_frames(BLOCK);
    svc.arm().unwrap();
    svc.start().unwrap();
    Rig { svc, drive }
}

impl Rig {
    /// Run passes until `done` or the budget runs out; the last status.
    fn until(
        &mut self,
        max: usize,
        mut done: impl FnMut(&hertzforge_explorer_host::ExplorerStatus) -> bool,
    ) -> hertzforge_explorer_host::ExplorerStatus {
        let mut st = self.svc.service_analysis();
        for _ in 0..max {
            let alive = self.svc.run_batches(1).unwrap_or(false);
            st = self.svc.service_analysis();
            if done(&st) || !alive {
                break;
            }
        }
        st
    }

    fn passes(&mut self, n: usize) -> hertzforge_explorer_host::ExplorerStatus {
        self.until(n, |_| false)
    }

    fn at_level(&mut self, vpk: f64) {
        self.svc.set_amplitude_vpk(vpk).unwrap();
        let st = self.until(2_000, |st| st.commanded_vpk == vpk);
        assert_eq!(st.commanded_vpk, vpk, "the level settles");
    }

    fn drive(&self) -> Vec<f32> {
        self.drive.lock().unwrap().clone()
    }
}

/// Frames a batch generates: two ticks of one block.
const BATCH: usize = 2 * BLOCK;

#[test]
fn explorer_intents_are_refused_while_the_stop_ramps() {
    let mut r = rig(1.0, 60.0);
    r.at_level(0.2);
    r.passes(4);
    r.svc.request_stop();
    let from = r.svc.status_cell().read().unwrap().frames_generated as usize;
    r.passes(10);
    // Nothing raises or moves the drive once the stop has begun.
    let refused = |e: Result<_, ServiceError>| match e {
        Err(ServiceError::BadState(m)) => assert!(m.contains("stopping"), "{m}"),
        other => panic!("taken while the stop ramps: {other:?}"),
    };
    refused(r.svc.set_amplitude_vpk(0.3).map(|_| ()));
    refused(r.svc.set_frequency(400.0).map(|_| ()));
    refused(r.svc.steer(None, Some(LevelPart::StepDb(6.0))).map(|_| ()));
    refused(r.svc.sweep(SweepDirection::Up, 60.0).map(|_| ()));
    refused(r.svc.hold().map(|_| ()));
    // ... and the ramp ends at 0 V on its slew: 40 dB at 60 dB/s.
    let st = r.until(2_000, |st| st.state.is_terminal());
    assert_eq!(st.state, RunState::Stopped);
    let drive = r.drive();
    let last = drive.iter().rposition(|v| *v != 0.0).unwrap();
    let ramp_s = (last - from) as f64 / FS;
    assert!(
        (ramp_s - 40.0 / 60.0).abs() < 2.0 * BATCH as f64 / FS,
        "the stop took {ramp_s} s"
    );
    let peak = drive[from..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak <= 0.2 + 1e-6, "nothing rose during the stop: {peak}");
}

#[test]
fn explorer_a_sweep_faster_than_the_slew_is_answered_at_the_slew() {
    let mut r = rig(1.0, 240.0);
    r.at_level(0.2);
    let rec = r.svc.sweep(SweepDirection::Up, 600.0).unwrap();
    match rec.kind {
        hertzforge_explorer_host::IntentKind::Sweep {
            rate_oct_per_min, ..
        } => {
            assert_eq!(rate_oct_per_min.asked, 600.0);
            assert_eq!(rate_oct_per_min.adopted, 60.0, "60 times 1 oct/s");
            assert!(rate_oct_per_min.clamped);
        }
        other => panic!("{other:?}"),
    }
    // The target never leads the tone by more than one frame's travel.
    let frame_oct = 1.0 / FS;
    for _ in 0..40 {
        let st = r.passes(1);
        let lead = (st.target_freq_hz / st.commanded_freq_hz).log2();
        assert!(
            lead <= frame_oct * 1.001,
            "the target leads the tone by {lead} octaves"
        );
    }
}

#[test]
fn explorer_a_hold_mid_sweep_leaves_the_tone_where_it_is() {
    let mut r = rig(1.0, 240.0);
    r.at_level(0.2);
    r.svc.sweep(SweepDirection::Up, 600.0).unwrap();
    let st = r.passes(8);
    let held_at = st.commanded_freq_hz;
    assert!(held_at > 100.0, "the tone has moved");
    let rec = r.svc.hold().unwrap();
    match rec.kind {
        hertzforge_explorer_host::IntentKind::Hold { hz } => {
            assert_eq!(hz, held_at, "the hold answers where the tone is")
        }
        other => panic!("{other:?}"),
    }
    let st = r.passes(40);
    let moved = (st.commanded_freq_hz / held_at).log2().abs();
    assert!(
        moved <= 1.0 / FS * 1.001,
        "the tone moved {moved} octaves after the hold ({held_at} to {} Hz)",
        st.commanded_freq_hz
    );
    assert_eq!(st.target_freq_hz, st.commanded_freq_hz);
}

#[test]
fn explorer_a_sample_over_the_ceiling_aborts_drive_vpk() {
    let mut r = rig(1.0, 240.0);
    // A step whose clamp is bypassed: every sample twice what the clamp allowed.
    r.svc.bypass_amplitude_clamp_for_test(2.0);
    assert_eq!(r.svc.set_amplitude_vpk(0.4).unwrap(), 0.4);
    let st = r.until(2_000, |st| st.state.is_terminal());
    let drive = r.drive();
    let peak = drive.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        peak <= 0.5f32,
        "a sample of {peak} V passed the 0.5 V ceiling to the station"
    );
    assert_eq!(st.state, RunState::Aborted, "the run ends at the first one");
    let end = st.end.expect("an end record");
    let trip: DriveTrip = end.drive.expect("the drive facts");
    assert_eq!(trip.ceiling_vpk, 0.5);
    assert!(trip.vpk > 0.5 && trip.vpk <= 0.8 + 1e-6, "{trip:?}");
    assert!(end.reason.contains("ceiling"), "{}", end.reason);
    // It ends before the drive could reach 0.8 V: at the first sample past 0.5 V.
    assert!(trip.vpk < 0.52, "{trip:?}");
}

#[test]
fn explorer_a_safe_stop_names_its_cause() {
    let mut reasons = Vec::new();
    for cause in [
        StopCause::Operator,
        StopCause::SafeStop,
        StopCause::Shutdown,
    ] {
        let mut r = rig(1.0, 240.0);
        r.at_level(0.2);
        r.svc.request_stop_for(cause);
        let st = r.until(2_000, |st| st.state.is_terminal());
        assert_eq!(st.state, RunState::Stopped);
        let end = st.end.unwrap();
        assert_eq!(end.stop, Some(cause), "{}", end.reason);
        assert!(end.trip.is_none());
        reasons.push(end.reason);
    }
    // The secondary monitor's breach: the controlled stop, with the breach as its trip.
    let mut r = rig(1.0, 240.0);
    r.at_level(0.2);
    let breach = AbortTrip {
        channel_index: 1,
        axis: AbortAxis::Velocity,
        value: 1.2,
        limit: 1.0,
        at_frame: 4_096,
    };
    r.svc.inject_observed_trip_for_test(breach);
    let st = r.until(2_000, |st| st.state.is_terminal());
    assert_eq!(st.state, RunState::Stopped, "ramped, not aborted");
    let end = st.end.unwrap();
    assert_eq!(end.stop, Some(StopCause::Limit), "{}", end.reason);
    assert_eq!(end.trip, Some(breach));
    reasons.push(end.reason);
    let distinct: std::collections::BTreeSet<&String> = reasons.iter().collect();
    assert_eq!(
        distinct.len(),
        4,
        "each cause in its own words: {reasons:?}"
    );
    // The steer that has nothing to do with a stop still works on a running run.
    let mut q = rig(1.0, 240.0);
    assert!(q
        .svc
        .steer(Some(FrequencyPart::Hz(300.0)), Some(LevelPart::Vpk(0.1)))
        .is_ok());
}
