//! The THREE terminal paths all take the drive off the device, and the stop is
//! two-phase.
//!
//! Converting the run loop from `Engine::run` to bounded `run_ticks` batches
//! DELETES Sine's only backend stop: `run`'s epilogue. Three paths needed it, not
//! two -
//!
//! 1. `stop()`, the operator's clean end;
//! 2. `finish_abort()`, reached from CONTROL-TICK trips (DriveVpk, scalar,
//!    kinematic, servo-fault) - none of which ever touched the backend, before or
//!    after the conversion, because they latch and return;
//! 3. the engine-watchdog `AbortTripped` path.
//!
//! One `finish(reason)` helper carries it for all three. These tests count the
//! calls at the backend, because that is the only place the question "did the
//! drive actually come off" can be answered.
//!
//! RED-MUTATIONS, each flipped and observed once (2026-08-08):
//!
//! * Delete the `b.stop()` call from `SineService::finish`'s non-abort arm:
//!   `an_operator_stop_stops_the_backend` reds - the run reported `Stopped` with
//!   the device still clocking.
//! * Make `finish_abort` transition directly instead of calling `finish`:
//!   `a_control_tick_trip_reaches_the_backend` reds - the abort recorded its trip
//!   and left the AO running, which is the pre-conversion behaviour.
//! * Drop the COLA half of `SineProcessStep::drive_is_zero` (return
//!   `self.current_drive_scale == 0.0`): `the_stop_waits_for_the_cola_aux_too`
//!   reds at `out_channels = 2` - the tasks stopped with the aux DAC still at
//!   9.6e-6 of its reference amplitude, which the device then holds forever.
//!
//! In all three cases exactly the named test reds and the other six stay green.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use hertzforge_sine::{
    CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig, LadderConfig,
    NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const FS_HZ: f64 = 8_192.0;
const BLOCK: usize = 64;
const DWELL_HZ: f64 = 200.0;

/// What the backend was asked to do, readable from the test after the service has
/// consumed the box.
#[derive(Default)]
struct Counters {
    stops: AtomicU32,
    estops: AtomicU32,
    /// Frames still queued in the simulated output buffer when `stop()` was
    /// called. The two-phase stop's whole claim is that this is ZERO.
    queued_at_stop: AtomicU32,
}

/// A backend that DELIVERS FRAMES (so the engine actually runs) and counts the
/// two ways a run can take the drive off it.
///
/// Its input is a constant, deliberately: these tests are about the stop paths,
/// and a plant whose response depended on the drive would make the trip they are
/// asserting depend on the servo's behaviour too.
struct CountingBackend {
    c: Arc<Counters>,
    n_in: usize,
    n_out: usize,
    /// Constant EU value every input frame carries.
    level: f32,
    /// Simulated output buffer: frames queued, and the capacity.
    queued: usize,
    capacity: usize,
    /// Frames the device plays out on every `read_input` (a DMA-clocked AO
    /// draining at the sample clock).
    drain_per_read: usize,
    started: bool,
}

impl CountingBackend {
    fn new(c: Arc<Counters>, n_in: usize, n_out: usize, level: f32) -> Self {
        Self {
            c,
            n_in,
            n_out,
            level,
            queued: 0,
            capacity: BLOCK * 8,
            drain_per_read: BLOCK,
            started: false,
        }
    }
}

impl Backend for CountingBackend {
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
        self.started = true;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.c.stops.fetch_add(1, Ordering::Relaxed);
        self.c
            .queued_at_stop
            .store(self.queued as u32, Ordering::Relaxed);
        self.started = false;
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let frames = (out.len() / self.n_in).min(BLOCK);
        if frames == 0 {
            return Ok(InputRead::NoDataYet);
        }
        for v in out[..frames * self.n_in].iter_mut() {
            *v = self.level;
        }
        self.queued = self.queued.saturating_sub(self.drain_per_read);
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let offered = drive.len() / self.n_out;
        let room = self.capacity.saturating_sub(self.queued);
        let taken = offered.min(room);
        self.queued += taken;
        Ok(taken)
    }
    /// The buffer drains on the DEVICE's own clock, not only when the host reads
    /// input - which is what makes the generation-drain wait terminate. A mock
    /// that only drained on `read_input` would make the wait time out, and the
    /// timeout is not the behaviour under test.
    fn output_space(&mut self) -> Option<usize> {
        self.queued = self.queued.saturating_sub(self.drain_per_read);
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
        self.c.estops.fetch_add(1, Ordering::Relaxed);
        self.started = false;
        Ok(())
    }
}

fn table(cola: bool) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.sensitivity_mv_per_eu = 1000.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    if cola {
        let mut aux = Channel::new(3);
        aux.role = ChannelRole::Drive;
        aux.units = Units::VoltageV;
        t.channels.push(aux);
    }
    t
}

fn cfg(cola: bool, drive_vpk_abort: f64) -> SineTestConfig {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(100.0, 1.0));
    profile.push(ProfileRow::accel(400.0, 1.0));
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
            per_rung: RungFor::Seconds(600.0),
            full_for: RungFor::Seconds(600.0),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.0,
        drive_vpk_ceiling: drive_vpk_abort * 0.9,
        drive_vpk_abort,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig {
            enabled: cola,
            volts_pk: 1.0,
            waveform: hertzforge_sine::ColaWaveform::Sine,
        },
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: BLOCK,
        fs_hz: None,
        metadata: Vec::new(),
    }
}

/// Arm + sysID + engage against a counting backend. Returns the service and the
/// counters (which outlive the boxed backend).
fn engaged(cola: bool, drive_vpk_abort: f64, level: f32) -> (SineService, Arc<Counters>) {
    let c = Arc::new(Counters::default());
    let n_in = 1;
    let n_out = if cola { 2 } else { 1 };
    let mut svc = SineService::new(
        cfg(cola, drive_vpk_abort),
        table(cola),
        Box::new(CountingBackend::new(c.clone(), n_in, n_out, level)),
        hertzforge_module_abi::StationClass::NiReal,
        0x51E5_0001,
        None,
    )
    .expect("construction")
    .with_block_frames(BLOCK);
    svc.assess_feasibility().expect("no shaker configured");
    svc.arm().expect("shared clock + abort bound");
    svc.run_sysid(4096).expect("sysID");
    assert!(svc.coherence_gate().expect("gate"), "floor 0.0 passes");
    assert_eq!(svc.state(), RunState::Engaged);
    (svc, c)
}

// =============================================================================
// Path 1: the operator stop
// =============================================================================

/// RED-MUTATION test (module doc). Also the TWO-PHASE assertion: the tasks are
/// stopped only once the queue has drained.
#[test]
fn an_operator_stop_stops_the_backend() {
    let (mut svc, c) = engaged(false, 10.0, 1.0);
    // Run a few batches so the loop is genuinely going before the stop.
    for _ in 0..4 {
        svc.run_batches(2).expect("batches");
    }
    let before = c.stops.load(Ordering::Relaxed);
    svc.stop().expect("stop");
    assert_eq!(svc.state(), RunState::Stopped);
    assert!(
        c.stops.load(Ordering::Relaxed) > before,
        "the stop path must reach Backend::stop; a run that reports Stopped with the \
         device still clocking is the defect the conversion introduced"
    );
    assert_eq!(
        c.estops.load(Ordering::Relaxed),
        0,
        "a clean stop is a RAMP, never an emergency stop"
    );
    assert_eq!(
        c.queued_at_stop.load(Ordering::Relaxed),
        0,
        "PHASE TWO: the tasks must not stop while the device still has frames to \
         play - it holds its last sample"
    );
    assert_eq!(svc.drive_scale(), 0.0, "the drive rode the ramp to zero");
}

/// RED-MUTATION test (module doc). At `out_channels = 2` the aux rides a SECOND
/// physical DAC, and a stop predicate that only reads ch0 leaves it holding.
#[test]
fn the_stop_waits_for_the_cola_aux_too() {
    let (mut svc, c) = engaged(true, 10.0, 1.0);
    for _ in 0..4 {
        svc.run_batches(2).expect("batches");
    }
    svc.stop().expect("stop");
    assert_eq!(svc.state(), RunState::Stopped);
    assert!(c.stops.load(Ordering::Relaxed) > 0);
    // BOTH outputs. The two scales do not arrive at zero together: the servo's
    // drive scale at the end of a run sits far below the COLA reference
    // amplitude, so it hits the -120 dB floor first and a ch0-only predicate
    // fires early.
    assert_eq!(svc.drive_scale(), 0.0, "ch0");
    assert_eq!(
        svc.cola_scale(),
        0.0,
        "the COLA aux must ALSO be at zero before the tasks stop"
    );
}

// =============================================================================
// Path 2: a control-tick trip
// =============================================================================

/// RED-MUTATION test (module doc). A DriveVpk trip is latched INSIDE the control
/// tick and the pre-conversion `finish_abort` never touched the backend.
#[test]
fn a_control_tick_trip_reaches_the_backend() {
    // A measured level of ~0 with a target of 1.0 g drives the servo up; a tiny
    // Vpk abort ceiling makes it trip within a few batches. The trip is a
    // CONTROL-TICK trip, which is the path under test.
    let (mut svc, c) = engaged(false, 1e-3, 0.0);
    let mut ended = false;
    for _ in 0..400 {
        match svc.run_batches(2) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                ended = true;
                break;
            }
        }
    }
    assert!(ended, "the drive guard must trip; state {:?}", svc.state());
    assert_eq!(svc.state(), RunState::Aborted);
    assert!(svc.trip().is_some(), "the trip must be recorded");
    assert!(
        c.estops.load(Ordering::Relaxed) > 0,
        "an ABORT takes emergency_stop - the backend's own 25 ms taper - not a ramp. \
         Before the conversion this path recorded the trip and left the AO running."
    );
}

// =============================================================================
// The batch pump's own contract
// =============================================================================

/// `InputEnded` at the batch budget is the EXPECTED per-iteration outcome, not a
/// run end. The pre-conversion loop's `_ => break` arm treated it as one, which
/// post-conversion would end every run after a single batch.
#[test]
fn a_batch_budget_exit_continues_the_run() {
    let (mut svc, _c) = engaged(false, 10.0, 1.0);
    for i in 0..20 {
        assert!(
            svc.run_batches(2).expect("batches"),
            "the run ended at batch group {i}; a bounded batch always exits InputEnded \
             on a continuous acquisition and that is not a run end"
        );
        assert_eq!(svc.state(), RunState::Engaged);
    }
}

/// The ring is fed by the step and drained by the service, in acquisition order.
#[test]
fn the_raw_ring_carries_acquired_blocks_to_the_tap() {
    let (mut svc, _c) = engaged(false, 0.5, 1.0);
    svc.run_batches(2).expect("batches");
    let mut frames = 0usize;
    let mut stride_seen = 0usize;
    svc.service_analysis_tapped(|block, stride| {
        stride_seen = stride;
        frames += block.len() / stride.max(1);
    });
    assert!(
        frames > 0,
        "the tap saw no blocks; sine had no ring at all before"
    );
    assert_eq!(stride_seen, 1, "one enabled input channel");
    assert_eq!(svc.blocks_dropped(), 0, "the drain kept up");
    // Draining twice must not re-deliver: the buffers went back to the producer.
    let mut again = 0usize;
    svc.service_analysis_tapped(|b, s| again += b.len() / s.max(1));
    assert_eq!(again, 0);
}

/// The ramp law is stated in dB/s, so its duration is computable - which is what
/// a host needs to check a profile's safe stop against its controller timeout.
#[test]
fn the_safe_stop_budget_is_computable_and_rate_independent() {
    use hertzforge_sine::service::engine::{SineProcessStep, RAMP_DB_PER_S, RAMP_FLOOR_SCALE};
    // Full span, floor to any starting scale, is span_dB / rate.
    let from = 1.0;
    let want = 20.0 * (from / RAMP_FLOOR_SCALE).log10() / RAMP_DB_PER_S;
    let got = SineProcessStep::safe_stop_seconds(from);
    assert!((got - want).abs() < 1e-12, "{got} vs {want}");
    // 120 dB at 120 dB/s is one second, at ANY sample rate. The law it replaces
    // (0.999 per FRAME) was -142 dB/s at 16 384 Hz and -445 dB/s at 51 200 - the
    // same code, a 3x different stop.
    assert!((got - 1.0).abs() < 1e-9, "{got}");
    // A drive already below the floor stops instantly.
    assert_eq!(
        SineProcessStep::safe_stop_seconds(RAMP_FLOOR_SCALE / 2.0),
        0.0
    );
}

/// `periodic_save_s` is an INTERVAL, and it was never read as one.
///
/// `service_capture` checked only that the field was SET and then saved a
/// snapshot every chunk. Pre-conversion that was one file per ~256 s on the
/// chassis, which looked plausible; post-conversion a chunk is 0.25 s of test
/// time, so the same code is 4 file writes a second of test time on the pump
/// thread. RED-MUTATION: replace the `due` computation with `let due = true;` -
/// this test reds with a save count in the dozens instead of at most a handful.
/// **Observed red 2026-08-08** (16 saves against an expected <= 3).
#[test]
fn periodic_save_honors_its_interval() {
    let c = Arc::new(Counters::default());
    let dir = std::env::temp_dir().join(format!("hertzforge-sine-psave-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut cfg = cfg(false, 10.0);
    // 1.0 s of TEST time between saves. A chunk is 0.25 s, so a correct
    // implementation saves on every FOURTH chunk and a broken one on every chunk.
    cfg.capture.periodic_save_s = Some(1.0);
    let mut svc = SineService::new(
        cfg,
        table(false),
        Box::new(CountingBackend::new(c, 1, 1, 1.0)),
        hertzforge_module_abi::StationClass::NiReal,
        0x51E5_0001,
        Some(dir.clone()),
    )
    .unwrap()
    .with_block_frames(BLOCK);
    svc.assess_feasibility().unwrap();
    svc.arm().unwrap();
    svc.run_sysid(4096).unwrap();
    assert!(svc.coherence_gate().unwrap());
    svc.start_capture("psave")
        .expect("armed, so the rate is honest");

    // 12 chunks = 3.0 s of test time. At a 1.0 s interval that is at most 3 saves
    // (the first is due one full interval in, so 2 or 3 depending on rounding).
    for _ in 0..12 {
        if !svc.run_chunk().unwrap_or(false) {
            break;
        }
    }
    let saves = svc
        .log()
        .events()
        .iter()
        .filter(|e| matches!(e.kind, hertzforge_sine::EventKind::Snapshot { .. }))
        .count();
    let _ = svc.stop();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        saves <= 3,
        "12 chunks of 0.25 s test time at a 1.0 s interval is at most 3 saves; got \
         {saves}. A save per chunk is the defect."
    );
}

// =============================================================================
// Capture
// =============================================================================

/// The capture sidecar stamps the sample rate, and until `configure()` has
/// answered that number is the REQUEST.
#[test]
fn capture_refuses_before_arm() {
    let c = Arc::new(Counters::default());
    let mut svc = SineService::new(
        cfg(false, 10.0),
        table(false),
        Box::new(CountingBackend::new(c, 1, 1, 1.0)),
        hertzforge_module_abi::StationClass::NiReal,
        0x51E5_0001,
        None,
    )
    .unwrap()
    .with_block_frames(BLOCK);
    let text = svc
        .start_capture("pre_arm")
        .expect_err("the rate is not known yet")
        .to_string();
    assert!(text.contains("before arm"), "{text}");
    assert!(text.contains("sample rate"), "{text}");
}
