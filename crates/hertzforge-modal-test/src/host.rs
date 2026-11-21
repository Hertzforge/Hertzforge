//! The run host: the station opened once for the whole test, the arm gate, the engine
//! and the pump thread that runs it, in both modes.
//!
//! * **Impact mode drives nothing.** The engine is built with no outputs on the table
//!   with its drive rows left out, and the arm gate is asked for a capture-only run: no
//!   drive sample is ever written.
//! * **Shaker mode follows Playback's protections.** The source is filled ahead on its
//!   own thread before the station opens; the engine runs in batches of
//!   [`TICKS_PER_BATCH`] ticks under `catch_unwind`; between batches an operator's
//!   abort, a response abort, the source running empty, a ceiling trip and an input
//!   overload on a row with abort limits each take the emergency stop at once. A
//!   controlled stop ramps the drive [`crate::setup::STOP_RAMP_DEPTH_DB`] down over
//!   [`crate::setup::STOP_RAMP_S`], then zero. The drive ceiling is judged on every
//!   sample before it leaves.
//! * **Every input frame goes to the capture ring** (with the drive written on the same
//!   tick), for the run's thread to trigger, average and display from. The loop thread
//!   does no analysis and no file work.

use crate::setup::{STOP_RAMP_DEPTH_DB, STOP_RAMP_S};
use crate::source::{panic_words, Feeder, Frames, Source, SourceBuild};
use hertzforge_channel_table::{AbortLimits, ChannelRole, ChannelTable};
use hertzforge_engine::ring::{Consumer, Producer};
use hertzforge_engine::{AbortTrip, Engine, EngineConfig, ProcessStep, StopReason, VecCaptureSink};
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_module_abi::StationClass;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Ticks per batch: the most an abort waits for the pump to look.
pub const TICKS_PER_BATCH: u64 = 2;
const SIM_WATCHDOG_SUSTAIN_FRAMES: u32 = 16;
const WATCHDOG_SUSTAIN_SECONDS: f64 = 0.00025;
/// Batches an ended test waits, at most, for its record to be whole.
const ZERO_WAIT_BATCHES: u32 = 400;
/// Seconds of input frames the capture ring holds.
pub const CAPTURE_RING_S: f64 = 8.0;
/// Seconds of source the feeder keeps ahead.
pub const SOURCE_AHEAD_S: f64 = 2.0;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// How the pump turns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pacing {
    Station,
    WallClock { speed: f64 },
}

/// The drive's phase in shaker mode (impact mode stays [`DrivePhase::Idle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DrivePhase {
    /// Zero drive, the source still taken so frames stay aligned.
    Idle = 0,
    /// A start was asked; the ramp begins on the next frame's start.
    Waiting = 1,
    Ramping = 2,
    Driving = 3,
    Stopping = 4,
    /// Zeroed at once for good: an abort, an underrun or a ceiling trip.
    Zeroed = 5,
}

impl DrivePhase {
    pub fn from_u8(x: u8) -> DrivePhase {
        match x {
            0 => DrivePhase::Idle,
            1 => DrivePhase::Waiting,
            2 => DrivePhase::Ramping,
            3 => DrivePhase::Driving,
            4 => DrivePhase::Stopping,
            _ => DrivePhase::Zeroed,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DrivePhase::Idle => "idle",
            DrivePhase::Waiting => "waiting",
            DrivePhase::Ramping => "ramping",
            DrivePhase::Driving => "driving",
            DrivePhase::Stopping => "stopping",
            DrivePhase::Zeroed => "zeroed",
        }
    }
}

/// What the step and the host share.
#[derive(Debug, Default)]
pub struct StepShared {
    pub start: AtomicBool,
    pub stop: AtomicBool,
    pub abort: AtomicBool,
    /// Set once an end or an abort is asked: the step takes no drive start after it, so
    /// the drive never leaves [`DrivePhase::Idle`] or [`DrivePhase::Zeroed`] again.
    pub closed: AtomicBool,
    pub underrun: AtomicBool,
    pub ceiling_trip: AtomicBool,
    pub trip_drive: AtomicU32,
    pub trip_value: AtomicU64,
    pub capture_overruns: AtomicU64,
    /// Frames generated (the lead-in included).
    pub generated: AtomicU64,
    /// Input frames the step has been handed.
    pub frames_in: AtomicU64,
    /// The generated frame the latest ramp began at.
    pub ramp_from: AtomicU64,
    /// The generated frame the latest stop ramp began at.
    pub stop_from: AtomicU64,
    /// The generated frame the drive went back to zero at after the latest stop.
    pub quiet_from: AtomicU64,
    phase: AtomicU8,
}

impl StepShared {
    pub fn phase(&self) -> DrivePhase {
        DrivePhase::from_u8(self.phase.load(Ordering::Acquire))
    }
}

/// The shaker step's build.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveBuild {
    pub outputs: usize,
    /// The output slot of each source drive.
    pub slots: Vec<usize>,
    pub block: usize,
    pub ramp_frames: u64,
    pub ceiling_v: f64,
    pub stop_ramp_frames: u64,
}

/// The step: every input frame to the capture ring; in shaker mode the source's next
/// sample per drive, times the ramp's gain, judged against the ceiling.
pub struct ModalStep {
    drive: Option<(DriveBuild, Consumer, Arc<crate::source::FeedShared>)>,
    capture: Producer,
    shared: Arc<StepShared>,
    phase: DrivePhase,
    generated: u64,
    lead_in: u64,
    ramp_at: u64,
    stop_gain: f64,
    gain: f64,
    taken: u64,
    sample: Vec<f32>,
    out: Vec<f32>,
    frame: Vec<f32>,
}

impl ModalStep {
    pub fn new(
        drive: Option<(DriveBuild, Consumer, Arc<crate::source::FeedShared>)>,
        capture: Producer,
        lead_in: u64,
    ) -> (ModalStep, Arc<StepShared>) {
        let shared = Arc::new(StepShared::default());
        shared.generated.store(lead_in, Ordering::Release);
        let (outs, width) = drive
            .as_ref()
            .map_or((0, 0), |d| (d.0.outputs, d.0.slots.len()));
        let cw = capture.frame_len();
        (
            ModalStep {
                drive,
                capture,
                shared: shared.clone(),
                phase: DrivePhase::Idle,
                generated: lead_in,
                lead_in,
                ramp_at: 0,
                stop_gain: 0.0,
                gain: 0.0,
                taken: 0,
                sample: vec![0.0; width],
                out: vec![0.0; outs],
                frame: vec![0.0; cw],
            },
            shared,
        )
    }

    fn enter(&mut self, p: DrivePhase) {
        self.phase = p;
        self.shared.phase.store(p as u8, Ordering::Release);
    }

    fn zero(&mut self) {
        self.gain = 0.0;
        self.enter(DrivePhase::Zeroed);
    }

    /// The next drive frame into `self.out`.
    fn next_drive(&mut self) {
        self.out.iter_mut().for_each(|o| *o = 0.0);
        let Some((b, ring, feed)) = self.drive.as_ref() else {
            return;
        };
        let (block, ramp, stop_n, ceiling) = (
            b.block as u64,
            b.ramp_frames.max(1),
            b.stop_ramp_frames.max(1),
            b.ceiling_v,
        );
        if self.phase == DrivePhase::Zeroed {
            return;
        }
        // A failed source zeroes the drive on this sample, not when the loop next looks.
        if feed.failed.load(Ordering::Acquire) {
            self.zero();
            return;
        }
        if !ring.pop(&mut self.sample) {
            self.shared.underrun.store(true, Ordering::Release);
            self.zero();
            return;
        }
        feed.popped.fetch_add(1, Ordering::AcqRel);
        let at_frame_start = self.taken.is_multiple_of(block);
        self.taken += 1;
        match self.phase {
            DrivePhase::Waiting if at_frame_start => {
                self.shared
                    .ramp_from
                    .store(self.generated, Ordering::Release);
                self.ramp_at = 0;
                self.enter(DrivePhase::Ramping);
            }
            _ => {}
        }
        self.gain = match self.phase {
            DrivePhase::Idle | DrivePhase::Waiting | DrivePhase::Zeroed => 0.0,
            DrivePhase::Ramping => {
                let g = self.ramp_at as f64 / ramp as f64;
                self.ramp_at += 1;
                if self.ramp_at >= ramp {
                    self.enter(DrivePhase::Driving);
                }
                g
            }
            DrivePhase::Driving => 1.0,
            DrivePhase::Stopping => {
                if self.ramp_at >= stop_n {
                    self.shared
                        .quiet_from
                        .store(self.generated, Ordering::Release);
                    self.enter(DrivePhase::Idle);
                    0.0
                } else {
                    let g = self.stop_gain
                        * 10f64.powf(
                            -STOP_RAMP_DEPTH_DB * (self.ramp_at as f64 / stop_n as f64) / 20.0,
                        );
                    self.ramp_at += 1;
                    g
                }
            }
        };
        let Some((b, _, _)) = self.drive.as_ref() else {
            return;
        };
        for (k, slot) in b.slots.iter().enumerate() {
            // Judged as the f32 the station is handed, so the cast cannot round past it.
            let w = (f64::from(self.sample[k]) * self.gain) as f32;
            let v = f64::from(w);
            if v.abs() > ceiling || !v.is_finite() {
                self.shared.trip_drive.store(k as u32, Ordering::Relaxed);
                self.shared.trip_value.store(v.to_bits(), Ordering::Relaxed);
                self.shared.ceiling_trip.store(true, Ordering::Release);
                self.out.iter_mut().for_each(|o| *o = 0.0);
                self.zero();
                return;
            }
            if let Some(o) = self.out.get_mut(*slot) {
                *o = w;
            }
        }
    }
}

impl ProcessStep for ModalStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        drive_out.clear();
        let s = self.shared.clone();
        if s.abort.load(Ordering::Acquire) {
            if self.phase != DrivePhase::Zeroed {
                self.zero();
            }
        } else {
            if s.stop.swap(false, Ordering::AcqRel) {
                match self.phase {
                    DrivePhase::Ramping | DrivePhase::Driving => {
                        s.stop_from.store(self.generated, Ordering::Release);
                        self.stop_gain = self.gain;
                        self.ramp_at = 0;
                        self.enter(DrivePhase::Stopping);
                    }
                    DrivePhase::Waiting => self.enter(DrivePhase::Idle),
                    _ => {}
                }
            }
            let start = s.start.swap(false, Ordering::AcqRel);
            if s.closed.load(Ordering::Acquire) {
                // Ended: a start asked before or after the end is dropped.
                if self.phase == DrivePhase::Waiting {
                    self.enter(DrivePhase::Idle);
                }
            } else if start && self.phase == DrivePhase::Idle && self.drive.is_some() {
                self.enter(DrivePhase::Waiting);
            }
        }
        let in_w = input.len().checked_div(n_frames).unwrap_or(0);
        let cw = self.frame.len();
        for f in 0..n_frames {
            self.next_drive();
            drive_out.extend_from_slice(&self.out);
            self.generated += 1;
            let row = &input[f * in_w..(f + 1) * in_w];
            for (k, x) in row.iter().chain(self.out.iter()).take(cw).enumerate() {
                self.frame[k] = *x;
            }
            if !self.capture.push(&self.frame) {
                s.capture_overruns.fetch_add(1, Ordering::AcqRel);
            }
        }
        s.generated.store(self.generated, Ordering::Release);
        s.frames_in.fetch_add(n_frames as u64, Ordering::AcqRel);
        let _ = self.lead_in;
    }
}

/// How a host is built.
#[derive(Debug, Clone)]
pub struct HostBuild {
    pub fs_hz: f64,
    pub block: usize,
    /// Shaker mode: the source and the drive's build.
    pub drive: Option<(SourceBuild, DriveBuild)>,
    pub pacing: Pacing,
    /// Set to the frame the record of the drive ends before, at the test's end.
    pub record_until: Option<Arc<AtomicU64>>,
}

/// Why a start was refused. Nothing was driven.
#[derive(Debug, Clone, PartialEq)]
pub enum HostError {
    /// The station programmed another rate (`rate_mismatch`).
    Rate { asked: f64, got: f64 },
    /// The station refused, or the arm gate did.
    Arm(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Rate { asked, got } => write!(
                f,
                "the station programmed {got} Hz for the {asked} Hz asked; a Modal Test runs at the rate it asks"
            ),
            HostError::Arm(s) => write!(f, "the station refused to arm: {s}"),
        }
    }
}

/// How the host ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `ended`, `aborted`, `response_abort`, `drive_underrun`, `source_failed`,
    /// `drive_ceiling`, `input_overload`, `loop_faulted`, `loop_panicked` or
    /// `run_panicked`.
    pub outcome: &'static str,
    pub message: String,
    pub trip: Option<AbortTrip>,
    pub emergency_stop: bool,
    pub generated: u64,
}

/// The host's status, taken after every batch.
#[derive(Debug, Clone, PartialEq)]
pub struct HostStatus {
    pub phase: DrivePhase,
    pub fs_hz: f64,
    pub block_frames: usize,
    pub group_delay_frames: Option<u32>,
    pub lead_in_frames: u64,
    pub generated: u64,
    pub frames_in: u64,
    pub overloads: Vec<bool>,
    pub capture_overruns: u64,
    pub end: Option<Outcome>,
}

struct Inner {
    status: Mutex<HostStatus>,
    step: Arc<StepShared>,
    finish: AtomicBool,
    /// The outcome and words an abort ends with when it is not the operator's.
    why: Mutex<Option<(&'static str, String)>>,
}

/// How the host starts a thread: [`std_spawn`], or a test's own.
pub type Spawn =
    fn(std::thread::Builder, Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>;

/// The standard library's spawn.
pub fn std_spawn(
    b: std::thread::Builder,
    f: Box<dyn FnOnce() + Send>,
) -> std::io::Result<JoinHandle<()>> {
    b.spawn(f)
}

/// A started test.
pub struct ModalHost {
    inner: Arc<Inner>,
    pump: Mutex<Option<JoinHandle<()>>>,
    /// The capture ring's reading side: each frame is the inputs then the drives.
    pub capture: Mutex<Option<Consumer>>,
    pub inputs: usize,
    pub outputs: usize,
}

/// The table an impact test's engine is built on: the drive rows left out and the
/// abort limits stripped (nothing is driven, so a limit has nothing to stop).
pub fn capture_only_table(user: &ChannelTable) -> ChannelTable {
    let mut t = user.clone();
    t.channels.retain(|c| c.role != ChannelRole::Drive);
    for c in t.channels.iter_mut() {
        c.abort = AbortLimits::default();
    }
    t
}

impl ModalHost {
    /// Open the station, pass the arm gate, start the clock and the pump. In shaker
    /// mode the source is filled ahead first. `table` is the engine's table: for an
    /// impact test, [`capture_only_table`].
    pub fn start(
        b: HostBuild,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        station: StationClass,
    ) -> Result<ModalHost, HostError> {
        let frames = b.drive.as_ref().map(|(sb, _)| Source::new(sb.clone()));
        ModalHost::start_with_frames(b, frames, table, backend, station)
    }

    /// [`ModalHost::start`] with the drive's frames made by `frames` in place of the
    /// source `b.drive` names (its block and its drives).
    pub fn start_with_frames<F: Frames>(
        b: HostBuild,
        frames: Option<F>,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        station: StationClass,
    ) -> Result<ModalHost, HostError> {
        ModalHost::start_with_spawn(b, frames, table, backend, station, std_spawn)
    }

    /// [`ModalHost::start_with_frames`] with the pump's thread started by `spawn`. When
    /// it cannot start, the station is stopped and closed and the start refused.
    pub fn start_with_spawn<F: Frames>(
        b: HostBuild,
        frames: Option<F>,
        table: ChannelTable,
        mut backend: Box<dyn Backend>,
        station: StationClass,
        spawn: Spawn,
    ) -> Result<ModalHost, HostError> {
        let fs = b.fs_hz;
        let block = hertzforge_hal::default_block_frames(fs);
        let inputs = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .count();
        let outputs = b.drive.as_ref().map_or(0, |d| d.1.outputs);
        let mut feeder = None;
        let mut drive_parts = None;
        if let (Some((_, db)), Some(src)) = (&b.drive, frames) {
            let (block, width) = (src.block(), src.width());
            let ahead = ((SOURCE_AHEAD_S * fs) as u64).max(4 * block as u64);
            let (tx, rx) = hertzforge_engine::ring::channel(ahead as usize + block, width);
            let fd = Feeder::spawn(src, tx, ahead)
                .map_err(|e| HostError::Arm(format!("the source could not start: {e}")))?;
            drive_parts = Some((db.clone(), rx, fd.shared.clone()));
            feeder = Some(fd);
        }
        backend
            .open()
            .map_err(|e| HostError::Arm(format!("open: {e}")))?;
        let actual = match backend.configure(&table, &AcquisitionConfig::new(fs, block)) {
            Ok(a) => a,
            Err(e) => {
                let _ = backend.close();
                return Err(HostError::Arm(format!("configure: {e}")));
            }
        };
        if actual.actual_rate_hz != fs {
            let _ = backend.close();
            return Err(HostError::Rate {
                asked: fs,
                got: actual.actual_rate_hz,
            });
        }
        let resolution = if b.drive.is_some() {
            hertzforge_module_abi::Resolution::host_open_loop()
        } else {
            hertzforge_module_abi::Resolution::capture_only()
        };
        if let Err(e) = hertzforge_module_abi::arm_gate(
            &backend.caps(),
            station,
            resolution,
            Some(fs),
            actual.actual_rate_hz,
        ) {
            let _ = backend.close();
            return Err(HostError::Arm(format!("{e:?}")));
        }
        let mut ecfg = EngineConfig::with_block(inputs, outputs, &actual);
        if station.is_hardware() {
            ecfg.watchdog_sustain =
                hertzforge_engine::watchdog_sustain_frames(WATCHDOG_SUSTAIN_SECONDS, fs);
        } else {
            ecfg.watchdog_sustain = SIM_WATCHDOG_SUSTAIN_FRAMES;
            ecfg.jitter_frac = hertzforge_engine::TEST_DISABLE_JITTER_GUARD;
            ecfg.compute_budget = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);
        }
        let mut engine = Engine::new(ecfg, &table, backend.as_ref());
        let primed = engine.health().output_queued as u64;
        if let Err(e) = backend.arm() {
            let _ = backend.close();
            return Err(HostError::Arm(format!("arm: {e}")));
        }
        let prefilled = if outputs > 0 {
            match engine.prefill_output(backend.as_mut()) {
                Ok(n) => n as u64,
                Err(r) => {
                    let _ = backend.emergency_stop();
                    return Err(HostError::Arm(format!(
                        "the drive lead-in was refused: {r:?}"
                    )));
                }
            }
        } else {
            0
        };
        let lead_in = primed + prefilled;
        let (cap_tx, cap_rx) = hertzforge_engine::ring::channel(
            ((CAPTURE_RING_S * fs) as usize).max(4 * b.block),
            inputs + outputs,
        );
        let (step, step_shared) = ModalStep::new(drive_parts, cap_tx, lead_in);
        if let Err(e) = backend.start() {
            let _ = backend.emergency_stop();
            return Err(HostError::Arm(format!("start: {e}")));
        }
        let inner = Arc::new(Inner {
            status: Mutex::new(HostStatus {
                phase: DrivePhase::Idle,
                fs_hz: fs,
                block_frames: actual.block_frames,
                group_delay_frames: actual.group_delay_frames,
                lead_in_frames: lead_in,
                generated: lead_in,
                frames_in: 0,
                overloads: vec![false; inputs],
                capture_overruns: 0,
                end: None,
            }),
            step: step_shared,
            finish: AtomicBool::new(false),
            why: Mutex::new(None),
        });
        let limited: Vec<bool> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| c.has_armed_limit())
            .collect();
        let pump_inner = inner.clone();
        let pacing = b.pacing;
        let record_until = b.record_until.clone();
        let group = actual.group_delay_frames.unwrap_or(0) as u64;
        // The loop's parts wait in a slot its thread takes them from, so they come back
        // here, the station still open, when the thread cannot start.
        let slot = Arc::new(Mutex::new(Some((backend, engine, step, feeder))));
        let taken = slot.clone();
        let spawned = spawn(
            std::thread::Builder::new().name("modal-test-pump".into()),
            Box::new(move || {
                let Some((backend, mut engine, mut step, feeder)) = lock(&taken).take() else {
                    return;
                };
                let mut p = Pump {
                    inner: pump_inner,
                    backend,
                    engine: &mut engine,
                    step: &mut step,
                    feeder,
                    limited,
                    fs,
                    group,
                    pacing,
                    record_until,
                };
                p.run();
            }),
        );
        let pump = match spawned {
            Ok(t) => t,
            Err(e) => {
                if let Some((mut backend, _, _, feeder)) = lock(&slot).take() {
                    let _ = backend.emergency_stop();
                    let _ = backend.close();
                    if let Some(mut f) = feeder {
                        f.cancel();
                    }
                }
                return Err(HostError::Arm(format!(
                    "the test's loop could not start ({e}); the station was stopped and closed"
                )));
            }
        };
        Ok(ModalHost {
            inner,
            pump: Mutex::new(Some(pump)),
            capture: Mutex::new(Some(cap_rx)),
            inputs,
            outputs,
        })
    }

    pub fn status(&self) -> HostStatus {
        lock(&self.inner.status).clone()
    }

    pub fn shared(&self) -> Arc<StepShared> {
        self.inner.step.clone()
    }

    /// Start the drive: the ramp begins on the next frame's start. `false`, and nothing
    /// asked, once the test is ending or aborted.
    pub fn start_drive(&self) -> bool {
        if self.closed() {
            return false;
        }
        self.inner.step.start.store(true, Ordering::Release);
        true
    }

    /// Whether an end or an abort has been asked: no drive starts after it.
    pub fn closed(&self) -> bool {
        self.inner.step.closed.load(Ordering::Acquire)
    }

    /// The controlled stop of the drive: it ramps down, then idles at zero.
    pub fn stop_drive(&self) {
        self.inner.step.stop.store(true, Ordering::Release);
    }

    /// Abort: the drive goes to zero at once and the emergency stop is issued.
    pub fn abort(&self) {
        self.inner.step.closed.store(true, Ordering::Release);
        self.inner.step.abort.store(true, Ordering::Release);
    }

    /// Abort for a fault, not the operator: the test ends as `outcome` with `message`.
    pub fn abort_for(&self, outcome: &'static str, message: String) {
        lock(&self.inner.why).get_or_insert((outcome, message));
        self.abort();
    }

    /// Run `body`, the run's own thread, on guard: a panic in it aborts the host as
    /// `run_panicked` and waits for the loop to end in the emergency stop; the panic's
    /// words come back.
    pub fn guard(&self, body: impl FnOnce()) -> Result<(), String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
            Ok(()) => Ok(()),
            Err(p) => {
                let why = panic_words(p.as_ref());
                self.abort_for(
                    "run_panicked",
                    format!("the run's thread panicked ({why}); the drive went to zero at once"),
                );
                self.join(Duration::from_secs(10));
                Err(why)
            }
        }
    }

    /// End the test: once the drive is at zero and the record whole, the station stops.
    pub fn finish(&self) {
        self.inner.step.closed.store(true, Ordering::Release);
        self.inner.step.stop.store(true, Ordering::Release);
        self.inner.finish.store(true, Ordering::Release);
    }

    pub fn finished(&self) -> bool {
        lock(&self.inner.status).end.is_some()
            && lock(&self.pump).as_ref().is_none_or(|t| t.is_finished())
    }

    pub fn join(&self, wait: Duration) -> bool {
        let began = Instant::now();
        while began.elapsed() < wait {
            if lock(&self.pump).as_ref().is_none_or(|t| t.is_finished()) {
                if let Some(t) = lock(&self.pump).take() {
                    let _ = t.join();
                }
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

struct Pump<'a> {
    inner: Arc<Inner>,
    backend: Box<dyn Backend>,
    engine: &'a mut Engine,
    step: &'a mut ModalStep,
    feeder: Option<Feeder>,
    limited: Vec<bool>,
    fs: f64,
    group: u64,
    pacing: Pacing,
    record_until: Option<Arc<AtomicU64>>,
}

impl Pump<'_> {
    fn emergency(&mut self) -> bool {
        self.inner.step.closed.store(true, Ordering::Release);
        self.inner.step.abort.store(true, Ordering::Release);
        let _ = self.backend.emergency_stop();
        true
    }

    fn end(
        &mut self,
        outcome: &'static str,
        message: String,
        trip: Option<AbortTrip>,
        estop: bool,
    ) {
        let st = &self.inner.step;
        let mut s = lock(&self.inner.status);
        s.phase = if estop {
            DrivePhase::Zeroed
        } else {
            st.phase()
        };
        s.generated = st.generated.load(Ordering::Acquire);
        s.end = Some(Outcome {
            outcome,
            message,
            trip,
            emergency_stop: estop,
            generated: s.generated,
        });
    }

    fn record_whole(&self, until: u64) -> bool {
        let h = self.engine.health();
        let held = (h.output_queued + h.output_staged) as u64;
        let written = self
            .inner
            .step
            .generated
            .load(Ordering::Acquire)
            .saturating_sub(held);
        written >= until && h.frames_in >= until.saturating_add(self.group)
    }

    /// The loop, on guard: a panic anywhere in it, not only in the engine's ticks, ends
    /// in the emergency stop.
    fn run(&mut self) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.turn()));
        if let Err(p) = caught {
            let why = panic_words(p.as_ref());
            let e = self.emergency();
            self.end(
                "loop_panicked",
                format!("the test's loop panicked ({why}); the emergency stop was issued"),
                None,
                e,
            );
        }
        if let Some(f) = self.feeder.as_mut() {
            f.cancel();
        }
        let _ = self.backend.close();
    }

    fn turn(&mut self) {
        let began = Instant::now();
        let mut zero_wait = 0u32;
        let mut until: Option<u64> = None;
        loop {
            let st = self.inner.step.clone();
            if st.abort.load(Ordering::Acquire) {
                let e = self.emergency();
                let (outcome, message) = lock(&self.inner.why).take().unwrap_or((
                    "aborted",
                    "operator abort: the drive went to zero at once".into(),
                ));
                self.end(outcome, message, None, e);
                break;
            }
            let mut sink = VecCaptureSink::new();
            let (backend, engine, step) = (&mut self.backend, &mut *self.engine, &mut *self.step);
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.run_ticks(backend.as_mut(), step, &mut sink, TICKS_PER_BATCH)
            }));
            let report = match caught {
                Ok(r) => r,
                Err(p) => {
                    let e = self.emergency();
                    let why = panic_words(p.as_ref());
                    self.end(
                        "loop_panicked",
                        format!("the test's loop panicked ({why}); the emergency stop was issued"),
                        None,
                        e,
                    );
                    break;
                }
            };
            match report.reason {
                StopReason::InputEnded => {}
                StopReason::AbortTripped(t) => {
                    st.closed.store(true, Ordering::Release);
                    st.abort.store(true, Ordering::Release);
                    self.end(
                        "response_abort",
                        format!(
                            "response abort: channel {} {:?} {:.6} past its limit {:.6}; the drive went to zero at once",
                            t.channel_index, t.axis, t.value, t.limit
                        ),
                        Some(t),
                        true,
                    );
                    break;
                }
                other => {
                    let e = self.emergency();
                    self.end(
                        "loop_faulted",
                        format!("the test's loop stopped: {other:?}"),
                        None,
                        e,
                    );
                    break;
                }
            }
            let failed = self
                .feeder
                .as_ref()
                .filter(|f| f.shared.failed.load(Ordering::Acquire))
                .map(|f| {
                    lock(&f.shared.why)
                        .clone()
                        .unwrap_or_else(|| "no words".to_owned())
                });
            if let Some(why) = failed {
                let e = self.emergency();
                self.end(
                    "source_failed",
                    format!("the source thread failed ({why}); the drive went to zero at once"),
                    None,
                    e,
                );
                break;
            }
            if st.underrun.load(Ordering::Acquire) {
                let e = self.emergency();
                self.end(
                    "drive_underrun",
                    "drive underrun: the source ran empty; the drive went to zero at once".into(),
                    None,
                    e,
                );
                break;
            }
            if st.ceiling_trip.load(Ordering::Acquire) {
                let e = self.emergency();
                let k = st.trip_drive.load(Ordering::Relaxed);
                let v = f64::from_bits(st.trip_value.load(Ordering::Relaxed));
                self.end(
                    "drive_ceiling",
                    format!(
                        "drive ceiling: drive {k} asked {v:.6} V; the drive went to zero at once"
                    ),
                    None,
                    e,
                );
                break;
            }
            let overloads = self.backend.input_overloads();
            if let Some(k) = overloads
                .iter()
                .zip(&self.limited)
                .position(|(o, l)| *o && *l)
            {
                let e = self.emergency();
                self.end("input_overload", format!("input {k} (in table order) overloaded on a row with abort limits; the drive went to zero at once"), None, e);
                break;
            }
            let phase = st.phase();
            {
                let mut s = lock(&self.inner.status);
                s.phase = phase;
                s.generated = st.generated.load(Ordering::Acquire);
                s.frames_in = st.frames_in.load(Ordering::Acquire);
                s.capture_overruns = st.capture_overruns.load(Ordering::Acquire);
                if !overloads.is_empty() {
                    s.overloads = overloads;
                }
            }
            if self.inner.finish.load(Ordering::Acquire)
                && matches!(phase, DrivePhase::Idle | DrivePhase::Zeroed)
            {
                let u = *until.get_or_insert_with(|| {
                    let u = st.generated.load(Ordering::Acquire);
                    if let Some(r) = &self.record_until {
                        r.store(u, Ordering::Release);
                    }
                    u
                });
                zero_wait += 1;
                if self.record_whole(u) || zero_wait > ZERO_WAIT_BATCHES {
                    let _ = self.backend.stop();
                    self.end(
                        "ended",
                        "the test was ended with the drive at zero".into(),
                        None,
                        false,
                    );
                    break;
                }
            }
            if let Pacing::WallClock { speed } = self.pacing {
                let generated = st.generated.load(Ordering::Acquire) as f64;
                let due = Duration::from_secs_f64(generated / (self.fs * speed.max(1e-6)));
                let now = began.elapsed();
                if due > now {
                    std::thread::sleep((due - now).min(Duration::from_millis(50)));
                }
            }
        }
    }
}

/// The stop ramp's frames at `fs`.
pub fn stop_ramp_frames(fs_hz: f64) -> u64 {
    (STOP_RAMP_S * fs_hz).round().max(1.0) as u64
}
