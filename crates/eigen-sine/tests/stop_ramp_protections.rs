//! WHAT HAPPENS TO THE PROTECTIONS DURING A COMMANDED RAMP-DOWN.
//!
//! The chassis finding (sine closing sitting, 2026-08-09, NI-REAL wire probe):
//! a controller vanish MID-ENGAGED at 0.2 V reached `module_state = aborted` in
//! 8.6 s where the sysID-phase vanish had correctly RAMPED to `stopped` in 30.6 s.
//! The filed suspicion was the retune-settle class on the STOP path: a protection
//! (the low-side scalar abort, or the debounced servo-fault) tripping DURING the
//! deliberate ramp-down as the measured level walks out of the profile band.
//!
//! These tests are the in-tree reproduction of that suspicion, on the one plant
//! that can produce it: a UNITY LOOPBACK, where the measured response follows the
//! drive down instead of sitting at a constant like `stop_paths.rs`'s deliberately
//! drive-independent mock. On a constant-input plant the measured level does NOT
//! collapse when the drive ramps, so the whole class is invisible; the chassis has
//! AO0 wired to AI0 and 0.2 g == 0.2 V, which is exactly this plant.
//!
//! What they pin:
//!
//! 1. `a_commanded_ramp_down_on_a_unity_plant_lands_stopped_with_no_trip` - the
//!    daemon's own stop path (`request_stop` + `run_batches`, NOT the blocking
//!    `SineService::stop`), engaged and SETTLED so every protection is armed, on
//!    a plant whose measured level tracks the drive all the way to zero. It must
//!    land `Stopped` with `trip() == None`.
//! 2. `the_ramp_walks_the_drive_down_rather_than_cliffing_it` - the ramp is
//!    OBSERVED, not assumed: the drive scale is sampled across the stop and must
//!    decrease monotonically over several samples before it reaches zero. A stop
//!    that lands `Stopped` by cutting the drive would pass (1) and fail this.
//! 3. `a_genuine_hazard_during_the_ramp_still_aborts` - the anti-vacuity half.
//!    Suspending the profile-band protections for the ramp must not disarm the
//!    authorities that make a ramp safe: the engine's raw-ring accel watchdog is
//!    LIVE throughout, and a real over-level excursion injected while the ramp is
//!    running still ends the run `Aborted` with a trip recorded.
//!
//! **RED-MUTATION, OBSERVED (2026-08-09):** delete the `self.ramping_down` term
//! from `control_tick_body`'s step-3 early return (leaving only
//! `self.abort.is_tripped()`) and test (1) reds with
//! `ScalarAbort { side: "low", .. }` - the exact trip class the filed suspicion
//! named, on the exact path it named. It is green as shipped because step 3
//! already suspends every control-tick protection for the length of a commanded
//! ramp; the mutation is what proves the suspension is load-bearing rather than
//! incidental.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_dsp::tracking::BandwidthMode;
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use eigen_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use eigen_sine::{
    CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig, LadderConfig,
    NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const FS_HZ: f64 = 8_192.0;
const BLOCK: usize = 64;
const DWELL_HZ: f64 = 200.0;
/// The control target. 1 EU on a 1000 mV/EU channel is 1 V, so the loopback below
/// is numerically unity - the chassis's "0.2 g == 0.2 V on the jumper", scaled.
const TARGET_G: f64 = 1.0;

/// A UNITY LOOPBACK plant: whatever volts the drive wrote come back as the
/// control channel's engineering units, one block later.
///
/// This is the chassis condition the constant-input mock in `stop_paths.rs`
/// cannot express, and the difference is the whole point of this file: with a
/// constant input the measured level is independent of the drive, so a ramp-down
/// never takes the response out of the profile band and the stop-ramp abort class
/// cannot be reproduced at all.
///
/// The one-block delay is real too (the device plays what it was given, the ADC
/// reads it back on the next block), and it is what makes the collapse the
/// protections would see LAG the drive rather than track it exactly.
struct LoopbackBackend {
    n_in: usize,
    n_out: usize,
    /// Drive frames written but not yet read back (ch0 only).
    pending: Vec<f32>,
    /// Frames queued in the simulated output buffer, and its capacity.
    queued: usize,
    capacity: usize,
    /// Extra EU added to every input frame, for the mid-ramp hazard injection.
    inject: Arc<Mutex<f32>>,
    /// Set once the loopback has been asked for input at least once.
    ran: Arc<AtomicBool>,
}

impl LoopbackBackend {
    fn new(n_in: usize, n_out: usize, inject: Arc<Mutex<f32>>, ran: Arc<AtomicBool>) -> Self {
        Self {
            n_in,
            n_out,
            pending: Vec::new(),
            queued: 0,
            capacity: BLOCK * 8,
            inject,
            ran,
        }
    }
}

impl Backend for LoopbackBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: self.n_in as u32,
            max_output_channels: self.n_out as u32,
            abort_latency_bound: Some(std::time::Duration::from_millis(25)),
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: BLOCK as u32,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: FS_HZ,
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
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.ran.store(true, Ordering::Relaxed);
        let want = (out.len() / self.n_in).min(BLOCK);
        if want == 0 {
            return Ok(InputRead::NoDataYet);
        }
        let inject = *self.inject.lock().expect("inject poisoned");
        let have = self.pending.len().min(want);
        for f in 0..want {
            let v = if f < have { self.pending[f] } else { 0.0 };
            for ch in 0..self.n_in {
                out[f * self.n_in + ch] = v + inject;
            }
        }
        self.pending.drain(..have);
        self.queued = self.queued.saturating_sub(want);
        Ok(InputRead::Frames(want))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let offered = drive.len() / self.n_out;
        let room = self.capacity.saturating_sub(self.queued);
        let taken = offered.min(room);
        for f in 0..taken {
            self.pending.push(drive[f * self.n_out]);
        }
        self.queued += taken;
        Ok(taken)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.queued = self.queued.saturating_sub(BLOCK);
        Some(self.capacity.saturating_sub(self.queued))
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: true,
            faulted: false,
            message: String::new(),
            sample_counter: 0,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    // 1000 mV per EU: one volt of drive comes back as one g of measured response,
    // which is what makes this loopback UNITY in the units the profile is in.
    ctrl.sensitivity_mv_per_eu = 1000.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

fn cfg() -> SineTestConfig {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(100.0, TARGET_G));
    profile.push(ProfileRow::accel(400.0, TARGET_G));
    SineTestConfig {
        profile,
        mode: SineMode::Dwell(DwellPlan {
            freq_hz: DWELL_HZ,
            for_: DwellFor::Seconds(600.0),
        }),
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 0.0,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::FixedHz(20.0),
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
        drive_vpk_ceiling: 4.0,
        drive_vpk_abort: 8.0,
        // The DEBOUNCED SERVO-FAULT IS ARMED (5 ticks), because it is one of the
        // two protections the filed suspicion named. A file that left it at zero
        // would pass the ramp test with half the suspicion untested.
        fault_debounce_ticks: 5,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig {
            enabled: false,
            volts_pk: 1.0,
            waveform: eigen_sine::ColaWaveform::Sine,
        },
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: BLOCK,
        fs_hz: None,
        metadata: Vec::new(),
    }
}

struct Rig {
    svc: SineService,
    inject: Arc<Mutex<f32>>,
}

/// Arm + sysID + engage against the unity loopback, then run until the servo has
/// brought the measured response ONTO the profile band and the level-settle
/// window has expired - i.e. until every protection this file is about is ARMED.
///
/// Settling first is what makes the tests non-vacuous: a stop issued while the
/// settle window is still open would be suspended by the window rather than by
/// anything the stop path does, and the test would pass for the wrong reason.
fn engaged_and_settled() -> Rig {
    let inject = Arc::new(Mutex::new(0.0f32));
    let ran = Arc::new(AtomicBool::new(false));
    let mut svc = SineService::new(
        cfg(),
        table(),
        Box::new(LoopbackBackend::new(1, 1, inject.clone(), ran.clone())),
        eigen_module_abi::StationClass::NiReal,
        0x51E5_0117,
        None,
    )
    .expect("construction")
    .with_block_frames(BLOCK);
    svc.assess_feasibility().expect("no shaker configured");
    svc.arm().expect("shared clock + abort bound");
    svc.run_sysid(8192).expect("sysID");
    assert!(svc.coherence_gate().expect("gate"), "floor 0.0 passes");
    assert_eq!(svc.state(), RunState::Engaged);

    // Drive the loop until the response is ON the band and the settle window has
    // been spent. 4000 batches x 2 ticks x 64 frames / 8192 Hz = ~62 s of test
    // time, against a settle window of ~1 s at the Fast preset's 60 dB/s.
    for _ in 0..4_000 {
        if !svc.run_batches(1).expect("batches") {
            break;
        }
    }
    assert_eq!(
        svc.state(),
        RunState::Engaged,
        "the run must still be ENGAGED before the stop under test; trip {:?}",
        svc.trip()
    );
    assert!(
        svc.drive_scale() > 1e-3,
        "the servo must have the drive UP before a ramp-down is worth testing; got {}",
        svc.drive_scale()
    );
    Rig { svc, inject }
}

/// Pump the daemon's own two-phase stop to a terminal state, sampling the drive
/// scale on the way so the ramp can be OBSERVED rather than assumed.
fn pump_to_terminal(svc: &mut SineService, max_batches: usize) -> Vec<f64> {
    let mut trace = vec![svc.drive_scale()];
    for _ in 0..max_batches {
        match svc.run_batches(1) {
            Ok(true) => trace.push(svc.drive_scale()),
            Ok(false) | Err(_) => {
                trace.push(svc.drive_scale());
                break;
            }
        }
    }
    trace
}

/// **THE CHASSIS STOP-ABORT REPRODUCTION.** The daemon's stop path, mid-engaged,
/// settled, on a plant whose measured level collapses with the drive. It must land
/// `Stopped`.
///
/// RED-MUTATION (module doc): drop `self.ramping_down` from the step-3 early
/// return and this reds with `ScalarAbort { side: "low" }`.
#[test]
fn a_commanded_ramp_down_on_a_unity_plant_lands_stopped_with_no_trip() {
    let Rig { mut svc, .. } = engaged_and_settled();

    // THE DAEMON'S PATH, not `SineService::stop`: `SineHost::stop` posts
    // `request_stop` and returns, and the PUMP walks the ramp through
    // `run_batches`. The blocking `stop()` drives its own ramp and would not
    // exercise the loop the chassis was running.
    svc.request_stop().expect("request_stop");
    let trace = pump_to_terminal(&mut svc, 20_000);

    assert_eq!(
        svc.trip(),
        None,
        "a COMMANDED ramp-down must not trip a protection: the level leaving the \
         profile band is what the operator asked for. Trip: {:?}, drive trace tail \
         {:?}",
        svc.trip(),
        &trace[trace.len().saturating_sub(6)..]
    );
    assert_eq!(
        svc.state(),
        RunState::Stopped,
        "the mid-engaged commanded stop must land STOPPED; an `aborted` here is the \
         chassis stop-abort finding"
    );
    assert_eq!(svc.drive_scale(), 0.0, "the drive rode the ramp to zero");
}

/// The ramp is OBSERVED. A stop that reached zero by cutting the drive would pass
/// the test above and fail this one.
#[test]
fn the_ramp_walks_the_drive_down_rather_than_cliffing_it() {
    let Rig { mut svc, .. } = engaged_and_settled();
    let start = svc.drive_scale();
    svc.request_stop().expect("request_stop");
    let trace = pump_to_terminal(&mut svc, 20_000);
    assert_eq!(svc.state(), RunState::Stopped, "trip {:?}", svc.trip());

    // Every sample is below the one before it (or already at the floor), and the
    // walk takes SEVERAL samples - a cliff would be one sample from `start` to 0.
    let walking: Vec<f64> = trace.iter().copied().take_while(|&v| v > 0.0).collect();
    assert!(
        walking.len() >= 4,
        "the drive must WALK down over several batches, not cliff: trace {trace:?}"
    );
    for w in walking.windows(2) {
        assert!(
            w[1] < w[0],
            "the stop ramp must be monotone decreasing; got {w:?} in {trace:?}"
        );
    }
    assert!(
        walking[0] <= start && *walking.last().expect("non-empty") < start * 1e-2,
        "the walk must cover the range: {:?} -> {:?} from {start}",
        walking.first(),
        walking.last()
    );
}

/// **ANTI-VACUITY.** Suspending the profile-band protections for the ramp must
/// not disarm the authorities that make a ramp safe. The engine's raw-ring accel
/// watchdog is armed from the profile's abort band and stays LIVE through the
/// ramp; a real over-level excursion injected while the drive is walking down
/// still ends the run `Aborted`, with a trip recorded.
///
/// Without this test, "the ramp suspends the protections" could be implemented as
/// "the ramp suspends everything" and nothing here would notice.
#[test]
fn a_genuine_hazard_during_the_ramp_still_aborts() {
    let Rig { mut svc, inject } = engaged_and_settled();
    svc.request_stop().expect("request_stop");
    // A few batches INTO the ramp, so the hazard lands on the stop path and not
    // on an ordinary engaged tick.
    for _ in 0..3 {
        let _ = svc.run_batches(1);
    }
    assert!(
        svc.drive_scale() > 0.0 && svc.state() == RunState::Engaged,
        "the hazard must land WHILE the ramp is running: drive {} state {:?}",
        svc.drive_scale(),
        svc.state()
    );
    // WAY past the profile's abort band on the control channel: this is a plant
    // excursion, not a commanded level.
    *inject.lock().expect("inject poisoned") = 50.0;
    let _ = pump_to_terminal(&mut svc, 20_000);

    assert_eq!(
        svc.state(),
        RunState::Aborted,
        "a genuine over-level excursion during the ramp must still ABORT - the \
         suspension is for the commanded level walking down, not for the plant"
    );
    assert!(
        svc.trip().is_some(),
        "the abort must carry its trip record; an unexplained abort is a defect"
    );
}
