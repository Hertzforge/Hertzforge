//! RecorderService end-to-end tests.
//!
//! Every engine-driven test wraps the pump in the segment-run loop the service
//! implements, and every test is FAST (< a few seconds under debug). The only
//! soak test is `#[ignore]`-tagged; a short `mini_soak_short` stands in for the
//! default suite.
//!
//! The scripted-backend tests (burst pre-trigger, biquad FRF) use a local
//! [`ScriptedBackend`] rather than the Sim plant, so the input is deterministic
//! per-channel (a burst at a known frame; y = biquad(x)). eigen-engine's own
//! MockBackend is `#[cfg(test)]` and not usable cross-crate, hence the local
//! impl.

use std::f64::consts::TAU;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use eigen_channel_table::{
    AbortLimits, Channel, ChannelRole, ChannelTable, Interface, Polarity, Units,
};
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use eigen_hal_sim::SimConfig;
use eigen_recorder::recording::RecordingReader;
use eigen_recorder::service::{RecState, RecorderConfig, RecorderService};
use eigen_recorder::trigger::{LevelTrigger, Slope, TriggerMode};

/// The earlier `RecorderService::new(cfg, table, sim_cfg, data_root)` shape.
///
/// That constructor was folded into `with_backend`: the service takes a finished
/// backend + the station class the arm gate is given, and it no longer sizes a
/// Sim to the table (sizing is the factory's job). This helper does both here, so
/// every test BODY below is unchanged.
fn sim_service(
    cfg: RecorderConfig,
    table: ChannelTable,
    sim_cfg: SimConfig,
    data_root: PathBuf,
) -> Result<RecorderService, eigen_recorder::service::RecorderError> {
    let n_input = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .count()
        .max(1);
    let backend = sim_cfg.max_input_channels(n_input as u32).build();
    RecorderService::with_backend(
        cfg,
        table,
        Box::new(backend),
        eigen_module_abi::StationClass::EigenSim,
        data_root,
    )
}

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/// Unique temp data-root per test (no external tempfile dep).
fn temp_root(tag: &str) -> PathBuf {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = std::env::temp_dir();
    p.push(format!("eigen_recorder_dr4_{tag}_{pid}_{n}"));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// An input (Monitor) channel with the given index, range, and optional accel
/// abort limit.
fn input_channel(index: u32, range_max: f64, abort_accel_g: Option<f64>) -> Channel {
    let mut c = Channel::new(index);
    c.role = ChannelRole::Monitor;
    c.enabled = true;
    c.units = Units::AccelG;
    c.interface = Interface::SingleEnded;
    c.polarity = Polarity::Positive;
    c.range_max = range_max;
    c.sensitivity_mv_per_eu = 100.0;
    c.abort = AbortLimits {
        accel_g: abort_accel_g,
        velocity_ips: None,
        displacement_mil: None,
    };
    c
}

/// An all-input table of `n` Monitor channels, range 100 g, no abort limits.
fn input_table(n: u32) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 1..=n {
        t.channels.push(input_channel(i, 100.0, None));
    }
    t
}

/// A fast acquisition config: native rate lowered so a few thousand frames span
/// enough analysis blocks, small block size, no decimation.
fn fast_acq() -> eigen_recorder::AcquisitionConfig {
    eigen_recorder::AcquisitionConfig {
        native_rate_hz: 8_192.0,
        block_size: 256,
        analysis_decimation: 1,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// The scripted backend: deterministic per-channel input frames.
// ---------------------------------------------------------------------------

/// A hardware-free backend that replays pre-computed interleaved input frames.
/// `read_input` delivers up to the requested frames from the script; when the
/// script is exhausted it returns `InputRead::Ended`. Drive writes are ignored.
/// A monotonic counter advances per delivered frame.
struct ScriptedBackend {
    n_input: usize,
    sample_rate_hz: f64,
    /// Interleaved input frames: `n_input` per frame.
    frames: Vec<f32>,
    /// Next frame index to deliver.
    pos: usize,
    counter: u64,
    state_open: bool,
    state_armed: bool,
    state_started: bool,
    estops: u32,
    /// Stand-in for the provisional `[H]` abort-latency bound the NI backend
    /// seeds. A hardware-station arm REFUSES
    /// without one, so the hardware watchdog tests must publish it; the
    /// EIGEN-SIM tests leave it `None` and are unaffected.
    abort_latency_bound: Option<std::time::Duration>,
}

impl ScriptedBackend {
    fn new(n_input: usize, sample_rate_hz: f64, frames: Vec<f32>) -> Self {
        assert_eq!(frames.len() % n_input, 0, "frames must be whole");
        Self {
            n_input,
            sample_rate_hz,
            frames,
            pos: 0,
            counter: 0,
            state_open: false,
            state_armed: false,
            state_started: false,
            estops: 0,
            abort_latency_bound: None,
        }
    }

    /// Publish an abort-latency bound, as an NI station does.
    fn with_abort_latency_bound(mut self, d: std::time::Duration) -> Self {
        self.abort_latency_bound = Some(d);
        self
    }
}

impl Backend for ScriptedBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: self.n_input as u32,
            max_output_channels: 4,
            abort_latency_bound: self.abort_latency_bound,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.sample_rate_hz,
            // A host-loop scripted double delegates
            // nothing and has one implicit clock group.
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.state_open = true;
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.state_open = false;
        Ok(())
    }
    fn configure(
        &mut self,
        _table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        // A scripted double never coerces.
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.state_armed = true;
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.state_started = true;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.state_started = false;
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let total_frames = self.frames.len() / self.n_input;
        if self.pos >= total_frames {
            return Ok(InputRead::Ended); // script exhausted: clean end of stream
        }
        let want = out.len() / self.n_input;
        let avail = total_frames - self.pos;
        let take = want.min(avail);
        if take == 0 {
            // Caller buffer below one frame (never happens through the engine);
            // there is data but nowhere to put it - not an end, keep the old
            // "nothing delivered" outcome without lying about the stream ending.
            return Ok(InputRead::NoDataYet);
        }
        let src = self.pos * self.n_input;
        out[..take * self.n_input].copy_from_slice(&self.frames[src..src + take * self.n_input]);
        self.pos += take;
        self.counter += take as u64;
        Ok(InputRead::Frames(take))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        // Lockstep no-op sink: the recorder's engine table injects ONE synthetic
        // drive channel, so each offered frame is 1 sample; accept them all.
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: self.state_open,
            faulted: false,
            message: String::new(),
            sample_counter: self.counter,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.estops += 1;
        self.state_started = false;
        Ok(())
    }
}

/// A one-pole/biquad-free "biquad" reference: a simple stable IIR
/// `y[n] = b0*x[n] + b1*x[n-1] - a1*y[n-1]` (a first-order low pass), used as a
/// KNOWN transfer for the FRF test. Returns the y stream given x.
fn biquad_lowpass(x: &[f64], b0: f64, b1: f64, a1: f64) -> Vec<f64> {
    let mut y = Vec::with_capacity(x.len());
    let mut x1 = 0.0;
    let mut y1 = 0.0;
    for &xn in x {
        let yn = b0 * xn + b1 * x1 - a1 * y1;
        y.push(yn);
        x1 = xn;
        y1 = yn;
    }
    y
}

/// The analytic complex response of `biquad_lowpass` at frequency `f` (rate
/// `fs`): `H(z) = (b0 + b1 z^-1) / (1 + a1 z^-1)`, `z = e^{j w}`.
fn biquad_response(f: f64, fs: f64, b0: f64, b1: f64, a1: f64) -> (f64, f64) {
    let w = TAU * f / fs;
    let (cw, sw) = (w.cos(), w.sin());
    // numerator b0 + b1 (cos - j sin)
    let nr = b0 + b1 * cw;
    let ni = -b1 * sw;
    // denominator 1 + a1 (cos - j sin)
    let dr = 1.0 + a1 * cw;
    let di = -a1 * sw;
    let den = dr * dr + di * di;
    ((nr * dr + ni * di) / den, (ni * dr - nr * di) / den)
}

// ===========================================================================
// TESTS
// ===========================================================================

/// An all-input user table on SimBackend runs N segments clean (no
/// BackendError - the SilentDrive workaround feeds the injected synthetic drive
/// channel zeros), and frames flow.
#[test]
fn silentdrive_on_simbackend_runs_clean() {
    let table = input_table(4);
    let cfg = RecorderConfig {
        acq: fast_acq(),
        trigger_mode: TriggerMode::Manual,
        ..Default::default()
    };
    let mut svc = sim_service(cfg, table, SimConfig::new(), temp_root("f1")).unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    assert_eq!(svc.state(), RecState::Monitoring);

    let mut total = 0u64;
    for _ in 0..6 {
        total += svc
            .pump_frames(2048)
            .expect("no BackendError with SilentDrive");
    }
    assert!(total >= 4096, "frames should have flowed, got {total}");
    assert_eq!(svc.state(), RecState::Monitoring, "clean monitor, no fault");
    svc.stop_monitor().unwrap();
    assert_eq!(svc.state(), RecState::Stopped);
}

/// The pump's segment budget is FRAMES, not engine ticks. The two were
/// numerically equal only while the block was the EIGEN-SIM natural 1 frame;
/// on a hardware-block station (with_block_frames) a ticks-denominated budget
/// inflates by the block factor - 16384 "ticks" x 1024-frame blocks is 5.5
/// MINUTES of driver-paced acquisition per pump() call with the service mutex
/// held throughout (found live on the NI-SIM rig, 2026-07-30). This pins the
/// conversion with a non-unit block: a 2048-frame budget at block 256 must
/// deliver ~2048 frames, not 2048 x 256.
#[test]
fn pump_budget_is_frames_not_ticks_under_a_hardware_block() {
    let table = input_table(2);
    let cfg = RecorderConfig {
        acq: fast_acq(),
        trigger_mode: TriggerMode::Manual,
        ..Default::default()
    };
    let mut svc = sim_service(cfg, table, SimConfig::new(), temp_root("ticks"))
        .unwrap()
        .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();

    let frames = svc.pump_frames(2048).expect("pump");
    // One block of slack for the final partial tick; the ticks-confused form
    // returns ~2048 x 256 frames and fails this by orders of magnitude.
    assert!(
        frames <= 2048 + 256,
        "budget must be frames: asked 2048, got {frames}"
    );
    assert!(frames >= 2048, "the budget should be met, got {frames}");
    svc.stop_monitor().unwrap();
}

/// With abort limits on the user table and `watchdog_enabled = false`
/// (default), a limit-violating signal does NOT trip (the engine table clone
/// strips the limits). With `watchdog_enabled = true` on a scripted backend
/// delivering a violating signal, the watchdog DOES trip -> Faulted.
#[test]
fn watchdog_off_by_default_strips_limits() {
    // ---- watchdog OFF: a big constant signal does not trip ----
    let mut table = input_table(2);
    // Tight abort limit on channel 0 (1 g); the Sim noise stays tiny, but even
    // a large signal must not trip because the clone strips limits.
    table.channels[0].abort = AbortLimits {
        accel_g: Some(0.001),
        velocity_ips: None,
        displacement_mil: None,
    };
    let cfg = RecorderConfig {
        acq: fast_acq(),
        watchdog_enabled: false,
        ..Default::default()
    };
    let mut svc = sim_service(cfg, table.clone(), SimConfig::new(), temp_root("f10off")).unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    for _ in 0..4 {
        svc.pump_frames(1024).expect("watchdog off: no trip");
    }
    assert_eq!(svc.state(), RecState::Monitoring, "off: no trip");
    svc.stop_monitor().unwrap();

    // ---- watchdog ON: a violating scripted signal trips -> Faulted ----
    let n = 2usize;
    let fs = 8_192.0;
    // 4096 frames of a large constant 50 g on both channels: well over the 1 g
    // accel limit, sustained, so the watchdog trips.
    let n_frames = 4096;
    let mut frames = Vec::with_capacity(n_frames * n);
    for _ in 0..n_frames {
        frames.push(50.0f32);
        frames.push(50.0f32);
    }
    let mut wtable = input_table(2);
    wtable.channels[0].abort = AbortLimits {
        accel_g: Some(1.0),
        velocity_ips: None,
        displacement_mil: None,
    };
    wtable.channels[1].abort = AbortLimits {
        accel_g: Some(1.0),
        velocity_ips: None,
        displacement_mil: None,
    };
    let cfg_on = RecorderConfig {
        acq: {
            let mut a = fast_acq();
            a.native_rate_hz = fs;
            a
        },
        watchdog_enabled: true,
        overload_fraction: 10.0, // keep the overload proxy out of the way
        ..Default::default()
    };
    let backend = ScriptedBackend::new(n, fs, frames);
    let mut svc = RecorderService::with_backend(
        cfg_on,
        wtable,
        backend,
        eigen_module_abi::StationClass::EigenSim,
        temp_root("f10on"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    let mut faulted = false;
    for _ in 0..8 {
        if svc.pump_frames(1024).is_err() {
            faulted = true;
            break;
        }
        if svc.state() == RecState::Faulted {
            faulted = true;
            break;
        }
    }
    assert!(faulted, "watchdog on: violating signal must trip");
    assert_eq!(svc.state(), RecState::Faulted, "trip records Faulted");
}

/// On a HARDWARE station, a config nobody touched
/// arms the watchdog when the table carries abort limits — and the arming is
/// real, not just a flag: the same violating signal that does NOT trip on
/// EIGEN-SIM (the test above) DOES trip here, through the same code path.
///
/// The pair matters more than either half. `watchdog_off_by_default_strips_limits`
/// pins the EIGEN-SIM behaviour unchanged; this pins that the hardware default is
/// the opposite; and both drive the SAME scripted signal, so the difference can
/// only be the station.
///
/// RED-MUTATION: delete the hardware arm in `config::resolve_watchdog` (flipping
/// the default back to off) and this test reds while the EIGEN-SIM one stays
/// green. Nothing else in the suite notices — which is exactly why this test has
/// to exist at the service level and not only at the resolver.
#[test]
fn a_hardware_station_arms_the_watchdog_with_no_operator_action() {
    let n = 2usize;
    let fs = 8_192.0;
    let n_frames = 4096;
    let mut frames = Vec::with_capacity(n_frames * n);
    for _ in 0..n_frames {
        frames.push(50.0f32);
        frames.push(50.0f32);
    }

    let mut table = input_table(2);
    for ch in table.channels.iter_mut() {
        ch.abort = AbortLimits {
            accel_g: Some(1.0),
            velocity_ips: None,
            displacement_mil: None,
        };
    }

    // The operator touched NOTHING: watchdog_enabled is the field default
    // (false) and watchdog_explicit is false. On EIGEN-SIM this is the
    // limits-stripped case; on a hardware station it must arm.
    let cfg = RecorderConfig {
        acq: {
            let mut a = fast_acq();
            a.native_rate_hz = fs;
            a
        },
        overload_fraction: 10.0, // keep the overload proxy out of the way
        ..Default::default()
    };
    assert!(!cfg.watchdog_enabled && !cfg.watchdog_explicit);

    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        // The provisional [H] bound an NI station publishes; without one the
        // arm gate refuses a hardware station outright (and rightly so).
        ScriptedBackend::new(n, fs, frames)
            .with_abort_latency_bound(std::time::Duration::from_millis(160)),
        eigen_module_abi::StationClass::NiSimulated,
        temp_root("p4_hw_watchdog"),
    )
    .unwrap();

    // The service reports what it decided, and why, before anything runs.
    let w = svc.watchdog().clone();
    assert!(
        w.armed,
        "an untouched config must arm on a hardware station"
    );
    assert!(!w.disarmed_with_limits);
    assert!(w.basis.contains("ARMED by default"), "{}", w.basis);

    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    let mut faulted = false;
    for _ in 0..8 {
        if svc.pump_frames(1024).is_err() || svc.state() == RecState::Faulted {
            faulted = true;
            break;
        }
    }
    assert!(
        faulted,
        "the armed watchdog must actually trip on the violating signal — a flag \
         that does not reach build_engine_table is not a watchdog"
    );
    assert_eq!(svc.state(), RecState::Faulted);
    assert!(
        svc.status(u64::MAX).watchdog_armed,
        "and the status says so"
    );
}

/// The disarm is still the operator's call — it simply announces itself.
#[test]
fn an_explicit_disarm_on_hardware_survives_into_the_status() {
    let mut table = input_table(2);
    table.channels[0].abort = AbortLimits {
        accel_g: Some(0.001),
        velocity_ips: None,
        displacement_mil: None,
    };
    let cfg = RecorderConfig {
        acq: fast_acq(),
        watchdog_enabled: false,
        watchdog_explicit: true,
        ..Default::default()
    };
    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        ScriptedBackend::new(2, 8_192.0, vec![50.0f32; 4096 * 2])
            .with_abort_latency_bound(std::time::Duration::from_millis(160)),
        eigen_module_abi::StationClass::NiReal,
        temp_root("p4_hw_disarm"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    for _ in 0..4 {
        svc.pump_frames(1024).expect("disarmed: no trip");
    }
    assert_eq!(
        svc.state(),
        RecState::Monitoring,
        "the operator's choice stands"
    );

    let s = svc.status(u64::MAX);
    assert!(!s.watchdog_armed);
    assert!(s.limits_disarmed, "the banner flag must reach the UI");
    assert!(
        s.watchdog_basis.contains("LIMITS DISARMED"),
        "{}",
        s.watchdog_basis
    );
}

/// A scripted burst at frame K with a level trigger + pre-trigger. The
/// recording must contain the pre-window intact, joined gap-free with the live
/// continuation (read back via RecordingReader; t=0 at the trigger sample).
#[test]
fn burst_trigger_with_pretrigger() {
    let n = 1usize;
    let fs = 8_192.0f64;
    // A ramp so every frame value is UNIQUE: frame i has value i*0.01 below the
    // burst. That makes the pre/live join checkable frame-by-frame. The burst
    // that fires the trigger is a jump to >= threshold at frame K, chosen AFTER
    // the ring has fully filled (K > ring depth).
    let pretrigger_s = 0.5f64; // 0.5 s * 8192 = 4096 frames of ring depth
    let ring_frames = (pretrigger_s * fs) as usize; // 4096
    let k = 6000usize; // trigger frame, well past the 4096-frame ring depth
    let total_frames = 8000usize;
    let threshold = 1_000_000.0f32;
    let mut frames = Vec::with_capacity(total_frames);
    for i in 0..total_frames {
        let v = if i < k {
            (i as f32) * 0.01 // slow ramp, well below threshold
        } else {
            threshold + (i - k) as f32 // >= threshold, still unique per frame
        };
        frames.push(v);
    }

    let table = input_table(1);
    let level = LevelTrigger {
        channel_index: 0,
        threshold_eu: threshold as f64,
        slope: Slope::Rising,
        hysteresis_eu: 1.0,
        holdoff_s: 0.0,
    };
    let cfg = RecorderConfig {
        acq: {
            let mut a = fast_acq();
            a.native_rate_hz = fs;
            a
        },
        trigger_mode: TriggerMode::Level(level),
        pretrigger_s: Some(pretrigger_s),
        overload_fraction: 1e12, // no overload interference
        ..Default::default()
    };
    let backend = ScriptedBackend::new(n, fs, frames.clone());
    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        backend,
        eigen_module_abi::StationClass::EigenSim,
        temp_root("f3burst"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.arm().unwrap();
    assert_eq!(svc.state(), RecState::Armed);

    // Pump segments until the recording fires and stops (script exhausts).
    for _ in 0..12 {
        if !svc.state().is_live() {
            break;
        }
        let _ = svc.pump_frames(1024);
    }
    // The recording should exist. Stop to finalize anything still open.
    if svc.state() == RecState::Recording {
        svc.stop_recording().unwrap();
    }
    svc.stop_monitor().unwrap();

    // Open the recording and verify the pre-window + join.
    let list = svc.recordings_list(None);
    assert!(!list.is_empty(), "a recording should have been produced");
    let rec_dir = svc.data_root().join(&list[0].name);
    let mut reader = RecordingReader::open(&rec_dir).unwrap();
    let info = reader.info().clone();
    assert_eq!(info.t0_convention, "trigger", "t=0 at the trigger sample");
    // The pre-window should be a FULL ring depth (the ring had long since
    // filled by frame K).
    assert_eq!(
        info.pretrigger_frames, ring_frames as u64,
        "pre-window should be the full ring depth"
    );
    // The trigger sample sits at file frame `pretrigger_frames`; its value is
    // the burst value at frame K (>= threshold).
    let at_trigger = reader
        .read_channel_region(0, info.pretrigger_frames, 1)
        .unwrap();
    assert!(
        at_trigger[0] as f64 >= threshold as f64,
        "trigger-sample value {} should be >= threshold {}",
        at_trigger[0],
        threshold
    );
    // The frame just BEFORE the trigger is the last pre-window sample: frame
    // K-1 of the source (value (K-1)*0.01, below threshold). Contiguity: the
    // pre-window's last frame and the trigger frame are source frames K-1 and K
    // (adjacent), so their values must be (K-1)*0.01 and threshold - a clean
    // frame-accurate join with no gap.
    let before = reader
        .read_channel_region(0, info.pretrigger_frames - 1, 1)
        .unwrap();
    let expected_before = ((k - 1) as f32) * 0.01;
    assert!(
        (before[0] - expected_before).abs() < 1e-3,
        "pre-window last frame {} should be source frame K-1 value {}",
        before[0],
        expected_before
    );
    // Time at the trigger frame is exactly 0.
    assert!(
        reader.time_of_frame(info.pretrigger_frames).abs() < 1e-12,
        "t=0 at the trigger frame"
    );
}

/// A timed recording of a fixed frame count is gap-free and holds exactly the
/// target frames.
#[test]
fn timed_recording_gap_free() {
    let table = input_table(2);
    let fs = 8_192.0f64;
    let record_s = 0.25f64; // 0.25 s * 8192 = 2048 frames
    let target = (record_s * fs).round() as u64;
    let cfg = RecorderConfig {
        acq: {
            let mut a = fast_acq();
            a.native_rate_hz = fs;
            a
        },
        trigger_mode: TriggerMode::Manual,
        timed_record_s: Some(record_s),
        overload_fraction: 1e12,
        ..Default::default()
    };
    let mut svc = sim_service(cfg, table, SimConfig::new(), temp_root("timed")).unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    assert_eq!(svc.state(), RecState::Recording);

    // Pump until the timed recording completes (returns to Monitoring).
    for _ in 0..20 {
        if svc.state() != RecState::Recording {
            break;
        }
        svc.pump_frames(512).unwrap();
    }
    assert_eq!(svc.state(), RecState::Monitoring, "timed record completed");
    svc.stop_monitor().unwrap();

    let list = svc.recordings_list(None);
    assert_eq!(list.len(), 1, "exactly one recording");
    assert!(list[0].gap_free, "timed recording must be gap-free");
    // The writer stops on the first frame >= target; because segments arrive in
    // whole ticks, the file holds AT LEAST the target (never fewer) and less
    // than target + one segment.
    assert!(
        list[0].frames >= target,
        "frames {} should be >= target {}",
        list[0].frames,
        target
    );
    assert!(
        list[0].frames < target + 512,
        "frames {} should be within one segment of the target {}",
        list[0].frames,
        target
    );
}

/// A scripted 2-channel backend with y = biquad(x). The live FRF |H1|
/// matches the analytic biquad response within tolerance, and coherence is
/// valid after >= 2 averages.
#[test]
fn frf_pair_through_scripted_biquad() {
    let n = 2usize;
    let fs = 8_192.0;
    // A broadband-ish deterministic x: sum of several sines so many FRF bins
    // carry energy. y = biquad(x). The FRF of a noiseless linear system is the
    // biquad response exactly.
    let (b0, b1, a1) = (0.3, 0.1, -0.5);
    let n_frames = 40_000usize; // plenty of blocks for >= 2 averages
    let mut x = Vec::with_capacity(n_frames);
    let tones = [200.0, 500.0, 900.0, 1500.0, 2500.0];
    for i in 0..n_frames {
        let t = i as f64 / fs;
        let mut s = 0.0;
        for (k, &f) in tones.iter().enumerate() {
            s += (TAU * f * t + 0.3 * k as f64).sin();
        }
        x.push(s);
    }
    let y = biquad_lowpass(&x, b0, b1, a1);
    // Interleave [x, y] per frame.
    let mut frames = Vec::with_capacity(n_frames * n);
    for i in 0..n_frames {
        frames.push(x[i] as f32);
        frames.push(y[i] as f32);
    }
    let table = input_table(2);
    let mut acq = fast_acq();
    acq.native_rate_hz = fs;
    acq.block_size = 1024;
    let cfg = RecorderConfig {
        acq: acq.clone(),
        trigger_mode: TriggerMode::Manual,
        frf_pair: Some((0, 1)), // x = ch0 (reference), y = ch1 (response)
        overload_fraction: 1e12,
        ..Default::default()
    };
    let backend = ScriptedBackend::new(n, fs, frames);
    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        backend,
        eigen_module_abi::StationClass::EigenSim,
        temp_root("frf"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    // Pump the whole script.
    for _ in 0..60 {
        if !svc.state().is_live() {
            break;
        }
        let _ = svc.pump_frames(2048);
    }
    let snap = svc.frf().expect("frf pair configured");
    assert!(
        snap.n_averages >= 2,
        "need >= 2 averages, got {}",
        snap.n_averages
    );
    assert!(snap.valid, "coherence valid after >= 2 averages");

    // Check |H1| at each tone bin against the analytic biquad magnitude.
    let df = snap.df_hz;
    for &f in &tones {
        let bin = (f / df).round() as usize;
        if bin >= snap.h1_mag.len() {
            continue;
        }
        let (hr, hi) = biquad_response(f, fs, b0, b1, a1);
        let analytic = (hr * hr + hi * hi).sqrt();
        let live = snap.h1_mag[bin];
        assert!(
            (live - analytic).abs() / analytic < 0.05,
            "FRF |H1| at {f} Hz (bin {bin}): live {live:.4} vs analytic {analytic:.4}"
        );
    }
}

/// Record via the service, then analyze_region over the same window: the
/// offline PSD equals the live pipeline within 1e-9 (service-level
/// live/offline equivalence).
#[test]
fn live_offline_equivalence_service_level() {
    let n = 1usize;
    let fs = 8_192.0;
    // A deterministic multi-tone x so the spectrum is rich and reproducible.
    let n_frames = 8192usize;
    let mut frames = Vec::with_capacity(n_frames);
    for i in 0..n_frames {
        let t = i as f64 / fs;
        let s = (TAU * 300.0 * t).sin() + 0.5 * (TAU * 1100.0 * t).sin();
        frames.push(s as f32);
    }
    let table = input_table(1);
    let mut acq = fast_acq();
    acq.native_rate_hz = fs;
    acq.block_size = 512;
    let cfg = RecorderConfig {
        acq: acq.clone(),
        trigger_mode: TriggerMode::Manual,
        overload_fraction: 1e12,
        ..Default::default()
    };
    let backend = ScriptedBackend::new(n, fs, frames);
    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        backend,
        eigen_module_abi::StationClass::EigenSim,
        temp_root("equiv"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    // Record the whole script (no timed cap -> record until stopped).
    for _ in 0..12 {
        if !svc.state().is_live() {
            break;
        }
        let _ = svc.pump_frames(2048);
    }
    // Snapshot the LIVE PSD before stopping.
    let live = svc.spectra();
    let live_psd = live[0].1.clone();
    if svc.state() == RecState::Recording {
        svc.stop_recording().unwrap();
    }
    svc.stop_monitor().unwrap();

    // Offline: analyze the WHOLE recorded region with the same config.
    let list = svc.recordings_list(None);
    assert_eq!(list.len(), 1);
    let dir = svc.data_root().join(&list[0].name);
    let mut reader = RecordingReader::open(&dir).unwrap();
    let frames_rec = reader.info().frames;
    let region =
        eigen_recorder::analyze_region(&mut reader, &[0], 0, frames_rec as usize, &acq).unwrap();
    let offline_psd = &region.per_channel[0].psd;

    // The live and offline pipelines average the SAME blocks over the SAME
    // frames, so the PSDs match to floating tolerance.
    assert_eq!(live_psd.len(), offline_psd.len(), "bin counts match");
    let mut max_abs = 0.0f64;
    for (a, b) in live_psd.iter().zip(offline_psd.iter()) {
        max_abs = max_abs.max((a - b).abs());
    }
    assert!(
        max_abs < 1e-9,
        "live vs offline PSD max abs diff {max_abs:e} should be < 1e-9"
    );
}

/// Auto-rearm honors the holdoff: two bursts closer than the ring depth do NOT
/// double-count, and the second recording's pre-window does not overlap the
/// first run's tail.
#[test]
fn auto_rearm_respects_holdoff() {
    let n = 1usize;
    let fs = 8_192.0f64;
    let threshold = 500.0f32;
    let pretrigger_s = 0.5f64;
    let ring_frames = (pretrigger_s * fs) as usize; // 4096
                                                    // Two bursts, the second only ~1000 frames after the first fires - CLOSER
                                                    // than the 4096-frame ring depth. The holdoff (>= ring depth) must suppress
                                                    // the second fire so it does not double-count.
    let total = 12000usize;
    let burst1 = 5000usize;
    let burst2 = 6000usize; // 1000 frames after burst1 (< ring depth)
    let mut frames = vec![0.0f32; total];
    for f in frames.iter_mut().skip(burst1).take(50) {
        *f = threshold + 10.0;
    }
    for f in frames.iter_mut().skip(burst2).take(50) {
        *f = threshold + 10.0;
    }
    let table = input_table(1);
    let level = LevelTrigger {
        channel_index: 0,
        threshold_eu: threshold as f64,
        slope: Slope::Rising,
        hysteresis_eu: 1.0,
        holdoff_s: 0.0, // engine floors it at the ring depth
    };
    let cfg = RecorderConfig {
        acq: {
            let mut a = fast_acq();
            a.native_rate_hz = fs;
            a
        },
        trigger_mode: TriggerMode::Level(level),
        pretrigger_s: Some(pretrigger_s),
        auto_rearm: 3, // allow several rearms; the holdoff still gates them
        timed_record_s: Some(0.03), // short timed records (~245 frames) so each fire yields a run
        overload_fraction: 1e12,
        ..Default::default()
    };
    let backend = ScriptedBackend::new(n, fs, frames);
    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        backend,
        eigen_module_abi::StationClass::EigenSim,
        temp_root("rearm"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.arm().unwrap();
    for _ in 0..40 {
        if !svc.state().is_live() {
            break;
        }
        let _ = svc.pump_frames(512);
    }
    if svc.state() == RecState::Recording {
        let _ = svc.stop_recording();
    }
    svc.stop_monitor().unwrap();

    // Only the FIRST burst should have fired: the second is inside the holdoff.
    let list = svc.recordings_list(None);
    assert_eq!(
        list.len(),
        1,
        "the second burst is within the ring-depth holdoff; only one run"
    );
    // And that one run must be gap-free with a full pre-window that does not
    // overlap anything earlier (there is no earlier run).
    let dir = svc.data_root().join(&list[0].name);
    let reader = RecordingReader::open(&dir).unwrap();
    assert_eq!(
        reader.info().pretrigger_frames,
        ring_frames as u64,
        "the pre-window is the full ring depth"
    );
}

/// serde round-trips for status + report, and the report carries the
/// concurrent-recording honesty line verbatim.
#[test]
fn status_and_report_serialize() {
    let table = input_table(3);
    let cfg = RecorderConfig {
        acq: fast_acq(),
        trigger_mode: TriggerMode::Manual,
        ..Default::default()
    };
    let mut svc = sim_service(cfg, table, SimConfig::new(), temp_root("serde")).unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    for _ in 0..3 {
        svc.pump_frames(1024).unwrap();
    }

    // Status round-trips.
    let status = svc.status(1_000_000_000);
    let sj = serde_json::to_string(&status).unwrap();
    let sback: eigen_recorder::RecStatus = serde_json::from_str(&sj).unwrap();
    assert_eq!(status, sback);
    assert!(
        status.disk_budget_seconds > 0,
        "a budget should be reported"
    );

    // Report round-trips and carries the concurrent-recording line.
    let report = svc.report_data();
    let rj = serde_json::to_string(&report).unwrap();
    let _rback: eigen_recorder::ReportData = serde_json::from_str(&rj).unwrap();
    assert!(
        report.honesty.concurrent.contains(
            "v1 records its own acquisition session; concurrent capture during another \
             module's test requires the shared-acquisition layer."
        ),
        "the concurrent-recording honesty line must be present verbatim, got: {}",
        report.honesty.concurrent
    );
    // Default metadata is present.
    let keys: Vec<&str> = report.metadata.iter().map(|m| m.key.as_str()).collect();
    assert!(keys.contains(&"run_number"));
    assert!(keys.contains(&"operator"));
    svc.stop_monitor().unwrap();
}

/// A FAST mini-soak: record ~1-2 seconds of frames and assert 0 drops. Stands
/// in for the ignored full soak in the default suite.
#[test]
fn mini_soak_short() {
    let table = input_table(8);
    let fs = 8_192.0;
    let record_s = 1.0; // 1 s * 8192 = 8192 frames, 8 ch
    let cfg = RecorderConfig {
        acq: {
            let mut a = fast_acq();
            a.native_rate_hz = fs;
            a
        },
        trigger_mode: TriggerMode::Manual,
        timed_record_s: Some(record_s),
        overload_fraction: 1e12,
        ..Default::default()
    };
    let mut svc = sim_service(cfg, table, SimConfig::new(), temp_root("minisoak")).unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    for _ in 0..40 {
        if svc.state() != RecState::Recording {
            break;
        }
        svc.pump_frames(1024).unwrap();
    }
    svc.stop_monitor().unwrap();
    let list = svc.recordings_list(None);
    assert_eq!(list.len(), 1);
    assert!(list[0].gap_free, "mini soak: 0 drops");
    assert_eq!(list[0].n_channels, 8, "8 channels recorded");
}

/// The full soak: >= 10 min native 8-ch, 0 drops, no discontinuities.
/// IGNORED by default (long wall-clock). Run with:
///   cargo test -p eigen-recorder --test service soak_native_8ch -- --ignored --nocapture
#[test]
#[ignore = "soak: >= 10 min native 8-ch; run explicitly with --ignored"]
fn soak_native_8ch() {
    let table = input_table(8);
    let fs = 65_536.0f64;
    let record_s = 600.0f64; // 10 minutes at native rate
    let acq = eigen_recorder::AcquisitionConfig {
        native_rate_hz: fs,
        block_size: 2048,
        ..Default::default()
    };
    let cfg = RecorderConfig {
        acq,
        trigger_mode: TriggerMode::Manual,
        timed_record_s: Some(record_s),
        overload_fraction: 1e12,
        ..Default::default()
    };
    let mut svc = sim_service(cfg, table, SimConfig::new(), temp_root("soak")).unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    let target = (record_s * fs) as u64;
    let mut guard = 0u64;
    let max_guard = target / DEFAULT_SEGMENT + 100;
    while svc.state() == RecState::Recording && guard < max_guard {
        svc.pump().unwrap();
        guard += 1;
    }
    svc.stop_monitor().unwrap();
    let list = svc.recordings_list(None);
    assert_eq!(list.len(), 1);
    assert!(list[0].gap_free, "soak: 0 drops and no discontinuities");
    assert!(list[0].frames >= target, "soak: full duration recorded");
}

/// The default segment size used by `pump()` (mirrors the service constant for
/// the soak guard math).
const DEFAULT_SEGMENT: u64 = 16_384;
