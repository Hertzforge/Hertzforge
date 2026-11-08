//! The time between pulses is a floor on the PHYSICAL start-to-start: the time
//! between the instants the device's clock started two pulses, not between two
//! host timestamps taken somewhere near them.
//!
//! The plant here is the shipping [`HalPulsePlant`] over a loopback backend that
//! runs on the same virtual clock as the service's pacer. Each arm costs 15 ms
//! and 0 ms in turn, and the start itself takes 0.5 ms before the clock runs
//! and 0.5 ms after, so the work between the service's decision to fire and the
//! clock actually running is a different length on alternate pulses, as it is on
//! the real chassis. The backend records the instant its clock started.
//!
//! What must hold for every pair of pulses: the physical start-to-start is at
//! least the floor, and the start time the core reports is the physical start
//! give or take the start call itself (never before it).

mod common;

use common::*;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use hertzforge_srs::service::pacer::PulsePacer;
use hertzforge_srs::service::plant::HalPulsePlant;
use hertzforge_srs::service::report::RunState;
use hertzforge_srs::{FirePath, SrsShockService};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// One virtual clock, seconds, read by the pacer and moved by the backend.
type Clock = Arc<Mutex<f64>>;

/// The service's pacer on the shared clock: a wait jumps to its deadline, and a
/// pulse start costs nothing of its own (the backend charges the fire's time).
struct SharedPacer(Clock);

impl PulsePacer for SharedPacer {
    fn now_s(&mut self) -> f64 {
        *self.0.lock().unwrap()
    }
    fn sleep_until(&mut self, deadline_s: f64) {
        let mut t = self.0.lock().unwrap();
        if deadline_s > *t {
            *t = deadline_s;
        }
    }
}

/// Each arm's cost in turn, seconds.
const ARM_COST_S: [f64; 2] = [0.015, 0.0];
/// The start call's time before the clock runs, and after.
const START_PRE_S: f64 = 0.0005;
const START_POST_S: f64 = 0.0005;

/// A 1 g/V loopback on the shared clock that records when its clock started.
struct Loopback {
    clock: Clock,
    fs: f64,
    arms: usize,
    queued: VecDeque<f32>,
    /// The instant the clock started, every start.
    starts: Arc<Mutex<Vec<f64>>>,
}

impl Loopback {
    fn advance(&self, s: f64) {
        *self.clock.lock().unwrap() += s;
    }
}

impl Backend for Loopback {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 4,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: self.fs,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn configure(
        &mut self,
        _t: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.advance(ARM_COST_S[self.arms % ARM_COST_S.len()]);
        self.arms += 1;
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.advance(START_PRE_S);
        let t = *self.clock.lock().unwrap();
        self.starts.lock().unwrap().push(t);
        self.advance(START_POST_S);
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.queued.clear();
        Ok(())
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.queued.clear();
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let frames = out.len().min(self.queued.len());
        if frames == 0 {
            return Ok(InputRead::NoDataYet);
        }
        for v in out.iter_mut().take(frames) {
            *v = self.queued.pop_front().unwrap_or(0.0);
        }
        self.advance(frames as f64 / self.fs);
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.queued.extend(drive.iter().copied());
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
}

#[test]
fn physical_start_to_start_is_never_under_the_time_between_pulses() {
    let clock: Clock = Arc::default();
    let starts = Arc::new(Mutex::new(Vec::new()));
    let backend = Loopback {
        clock: Arc::clone(&clock),
        fs: FS,
        arms: 0,
        queued: VecDeque::new(),
        starts: Arc::clone(&starts),
    };
    let plant = HalPulsePlant::new(
        backend,
        siso_table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
    .with_block_frames(512);
    let cfg = closed_cfg(1.0);
    let floor = cfg.control.as_ref().unwrap().min_spacing_s();
    let mut s = SrsShockService::new(
        cfg,
        siso_table(),
        FirePath::Host(Box::new(plant)),
        Some(std::env::temp_dir().join(format!("hertzforge-srs-s2s-{}", std::process::id()))),
    )
    .unwrap();
    s.set_pacer(Box::new(SharedPacer(Arc::clone(&clock))));
    assert_eq!(run(&mut s), RunState::Stopped, "trip: {:?}", s.trip());

    // The first start is the identification burst; the rest are the pulses.
    let physical = starts.lock().unwrap()[1..].to_vec();
    let reported: Vec<f64> = s
        .report_data()
        .pulses
        .iter()
        .map(|r| r.at_s.expect("a closed-loop pulse has a start time"))
        .collect();
    assert_eq!(physical.len(), reported.len());
    assert!(physical.len() >= 4, "{} pulses", physical.len());

    for (k, w) in physical.windows(2).enumerate() {
        assert!(
            w[1] - w[0] >= floor - 1e-9,
            "pulses {k} and {} started {:.4} s apart on the device clock, under the \
             {floor} s floor (starts {physical:?})",
            k + 1,
            w[1] - w[0]
        );
    }
    for (k, (p, r)) in physical.iter().zip(&reported).enumerate() {
        assert!(
            *r >= p - 1e-9 && *r - p <= START_POST_S + 1e-9,
            "pulse {k}: reported start {r:.4} s against the device's {p:.4} s"
        );
    }
}
