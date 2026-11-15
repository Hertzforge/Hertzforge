//! The run host: the reader filled ahead, the station opened at the file's rate, the
//! arm gate, the engine and the pump thread that runs it.
//!
//! # The shape
//!
//! * **The reader fills the ring before the station opens.** At least
//!   [`HostOptions::prefill_s`] of the file (or all of it) is in the ring before the
//!   first sample is generated.
//! * **The rate is the file's.** The station is asked for the file's rate; when it
//!   programs another the arm is refused (`rate_mismatch`) before anything is driven,
//!   naming both rates. A file is never resampled.
//! * **A persistent [`Engine`] run in bounded batches** of [`TICKS_PER_BATCH`] ticks
//!   on the pump thread, which is the loop thread. The engine's watchdog reads the
//!   channel table's abort limits on every raw sample and, on a breach, calls the
//!   emergency stop inside the tick: that is the response abort.
//! * **Between batches**, never inside one: an operator's abort, the step's underrun,
//!   ceiling trip and the reader's failure, an input overload on a limited row, and an
//!   engine stop other than a clean batch end each take the emergency stop at once.
//!   The completed play and the controlled stop end with the station stopped once
//!   the zeros the step wrote have played.
//! * **The batch runs under `catch_unwind`.** A panic issues the emergency stop
//!   before the run ends, so a drive is never left held at its last sample.

use crate::plan::{rate_refusal, CheckedDrive, STOP_RAMP_DEPTH_DB, STOP_RAMP_S};
use crate::source::DriveSource;
use crate::step::{Phase, PlaybackStep, StepBuild, StepShared};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_engine::{
    AbortTrip, Engine, EngineConfig, EngineHealth, StopReason, VecCaptureSink,
};
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_module_abi::StationClass;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Ticks per batch: the most an abort waits for the pump to look.
pub const TICKS_PER_BATCH: u64 = 2;
/// The simulator's watchdog debounce, frames (as Explorer's).
const SIM_WATCHDOG_SUSTAIN_FRAMES: u32 = 16;
/// The hardware watchdog debounce, seconds (as Explorer's).
const WATCHDOG_SUSTAIN_SECONDS: f64 = 0.00025;
/// Batches a stop waits, at most, for the zeros to play.
const ZERO_WAIT_BATCHES: u32 = 400;
/// Display frames per second taken from the loop.
pub const DISPLAY_HZ: f64 = 2000.0;
/// Seconds of display frames the display ring holds.
const DISPLAY_RING_S: f64 = 4.0;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// How the pump turns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pacing {
    /// The station's clock: hardware reads wait for the device.
    Station,
    /// The wall clock, `speed` station seconds per wall second: a software station
    /// runs no faster.
    WallClock { speed: f64 },
}

/// How a run is hosted.
#[derive(Debug, Clone, PartialEq)]
pub struct HostOptions {
    pub pacing: Pacing,
    /// Seconds of the file read ahead before the first sample (at least 2).
    pub prefill_s: f64,
    /// Seconds the read-ahead ring holds (at least the prefill and a block).
    pub ring_s: f64,
    /// How long the prefill may take.
    pub prefill_wait: Duration,
}

impl Default for HostOptions {
    fn default() -> Self {
        HostOptions {
            pacing: Pacing::Station,
            prefill_s: 2.0,
            ring_s: 4.0,
            prefill_wait: Duration::from_secs(30),
        }
    }
}

/// Why a start was refused. Nothing was driven.
#[derive(Debug, Clone, PartialEq)]
pub enum HostError {
    /// The file could not be read ahead (`drive_unreadable`).
    Source(String),
    /// The station programmed another rate (`rate_mismatch`).
    Rate(crate::file::FileError),
    /// The station refused to open, configure or arm, or the arm gate refused.
    Arm(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Source(s) => write!(f, "the drive file could not be read ahead: {s}"),
            HostError::Rate(e) => write!(f, "{}", e.message),
            HostError::Arm(s) => write!(f, "the station refused to arm: {s}"),
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `completed`, `stopped`, `aborted`, `response_abort`, `drive_underrun`,
    /// `drive_ceiling`, `reader_failed`, `input_overload`, `loop_faulted` or
    /// `loop_panicked`.
    pub outcome: &'static str,
    pub message: String,
    /// The breach of a response abort.
    pub trip: Option<AbortTrip>,
    /// Whether the emergency stop was issued.
    pub emergency_stop: bool,
    /// File frames played.
    pub played: u64,
    /// Frames generated, the rolls included.
    pub generated: u64,
}

/// What the run looks like now: taken by the pump after every batch.
#[derive(Debug, Clone, PartialEq)]
pub struct HostStatus {
    pub phase: Phase,
    pub fs_hz: f64,
    pub block_frames: usize,
    pub lookahead_frames: usize,
    pub group_delay_frames: Option<u32>,
    pub generated: u64,
    pub played: u64,
    pub samples: u64,
    pub pre_roll_frames: u64,
    pub post_roll_frames: u64,
    pub frames_in: u64,
    /// Frames in the read-ahead ring.
    pub buffered: u64,
    pub health: Option<EngineHealth>,
    /// Per acquired input, in table order: whether it has overloaded.
    pub overloads: Vec<bool>,
    pub stop_from: Option<u64>,
    pub end: Option<Outcome>,
}

struct Inner {
    status: Mutex<HostStatus>,
    step: Arc<StepShared>,
}

/// A started run.
pub struct PlaybackHost {
    inner: Arc<Inner>,
    pump: Mutex<Option<JoinHandle<()>>>,
    /// The display ring's reading side: each frame is the inputs then the drives.
    pub display: Mutex<Option<hertzforge_engine::ring::Consumer>>,
    pub display_every: u64,
}

impl PlaybackHost {
    /// Read ahead, open the station at the file's rate, pass the arm gate, start the
    /// clock and the pump. Returns once the pump runs (the pre-roll at zero).
    pub fn start(
        drive: CheckedDrive,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        station: StationClass,
        opts: HostOptions,
    ) -> Result<PlaybackHost, HostError> {
        let reader = drive
            .file
            .samples_reader()
            .map_err(|e| HostError::Source(e.message))?;
        Self::start_with_reader(drive, Box::new(reader), table, backend, station, opts)
    }

    /// [`PlaybackHost::start`] with the samples read from `reader` (positioned at the
    /// first sample) rather than from the file: how a test makes a slow or failing disk.
    pub fn start_with_reader(
        drive: CheckedDrive,
        reader: Box<dyn std::io::Read + Send>,
        table: ChannelTable,
        mut backend: Box<dyn Backend>,
        station: StationClass,
        opts: HostOptions,
    ) -> Result<PlaybackHost, HostError> {
        let fs = drive.file.rate_hz;
        let width = drive.file.channels.len();
        let prefill = (opts.prefill_s.max(2.0) * fs).ceil() as u64;
        let block = hertzforge_hal::default_block_frames(fs);
        let ring = ((opts.ring_s * fs).ceil() as u64)
            .max(prefill + 4 * block as u64)
            .min(drive.file.samples.max(1) + 1) as usize;
        let (source, ring_reader) = DriveSource::spawn(reader, width, drive.file.samples, ring)
            .map_err(|e| HostError::Source(e.to_string()))?;
        source
            .wait_prefill(prefill, opts.prefill_wait)
            .map_err(HostError::Source)?;

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
            return Err(HostError::Rate(rate_refusal(
                fs,
                actual.actual_rate_hz,
                &[actual.actual_rate_hz],
            )));
        }
        if let Err(e) = hertzforge_module_abi::arm_gate(
            &backend.caps(),
            station,
            hertzforge_module_abi::Resolution::host_open_loop(),
            Some(fs),
            actual.actual_rate_hz,
        ) {
            let _ = backend.close();
            return Err(HostError::Arm(format!("{e:?}")));
        }

        let inputs = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .count();
        let outputs = drive.rows.len();
        let display_every = (fs / DISPLAY_HZ).round().max(1.0) as u64;
        let (display_tx, display_rx) = hertzforge_engine::ring::channel(
            ((DISPLAY_RING_S * fs / display_every as f64) as usize).max(64),
            inputs + outputs,
        );
        let s = &drive.setup;
        let (mut step, step_shared) = PlaybackStep::new(
            StepBuild {
                out_channels: outputs,
                slots: drive.slots.clone(),
                samples: drive.file.samples,
                gain: s.gain(),
                ceiling_v: s.ceiling_v,
                pre_roll_frames: (s.pre_roll_s * fs).round() as u64,
                post_roll_frames: (s.post_roll_s * fs).round() as u64,
                stop_ramp_frames: (STOP_RAMP_S * fs).round().max(1.0) as u64,
                stop_ramp_depth_db: STOP_RAMP_DEPTH_DB,
                display_every,
            },
            ring_reader,
            Some(display_tx),
        );
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
        if let Err(e) = backend.arm() {
            let _ = backend.close();
            return Err(HostError::Arm(format!("arm: {e}")));
        }
        // The lead-in: the zeros the step generates at the start of its pre-roll.
        if let Err(r) = engine.prefill_output(backend.as_mut()) {
            let _ = backend.emergency_stop();
            return Err(HostError::Arm(format!(
                "the drive lead-in was refused: {r:?}"
            )));
        }
        if let Err(e) = backend.start() {
            let _ = backend.emergency_stop();
            return Err(HostError::Arm(format!("start: {e}")));
        }
        let lookahead = backend
            .caps()
            .output_buffer
            .map_or(0, |c| c.min_lookahead_frames as usize);
        let inner = Arc::new(Inner {
            status: Mutex::new(HostStatus {
                phase: Phase::PreRoll,
                fs_hz: fs,
                block_frames: actual.block_frames,
                lookahead_frames: lookahead,
                group_delay_frames: actual.group_delay_frames,
                generated: 0,
                played: 0,
                samples: drive.file.samples,
                pre_roll_frames: (s.pre_roll_s * fs).round() as u64,
                post_roll_frames: (s.post_roll_s * fs).round() as u64,
                frames_in: 0,
                buffered: source.shared.pushed.load(Ordering::Acquire),
                health: None,
                overloads: vec![false; inputs],
                stop_from: None,
                end: None,
            }),
            step: step_shared,
        });
        let limited: Vec<bool> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| {
                c.abort.accel_g.is_some()
                    || c.abort.velocity_ips.is_some()
                    || c.abort.displacement_mil.is_some()
            })
            .collect();
        let pump_inner = inner.clone();
        let pacing = opts.pacing;
        let pump = std::thread::Builder::new()
            .name("playback-pump".into())
            .spawn(move || {
                let mut p = Pump {
                    inner: pump_inner,
                    backend,
                    engine: &mut engine,
                    step: &mut step,
                    source,
                    limited,
                    fs,
                    group: actual.group_delay_frames.unwrap_or(0) as u64,
                    pacing,
                };
                p.run();
            })
            .map_err(|e| HostError::Arm(format!("the run's loop could not start: {e}")))?;
        Ok(PlaybackHost {
            inner,
            pump: Mutex::new(Some(pump)),
            display: Mutex::new(Some(display_rx)),
            display_every,
        })
    }

    pub fn status(&self) -> HostStatus {
        lock(&self.inner.status).clone()
    }

    /// Ask the controlled stop: the drive ramps down, then the station stops.
    pub fn stop(&self) {
        self.inner.step.stop.store(true, Ordering::Release);
    }

    /// Abort: the drive goes to zero at once and the emergency stop is issued.
    pub fn abort(&self) {
        self.inner.step.abort.store(true, Ordering::Release);
    }

    /// Whether the pump has ended.
    pub fn finished(&self) -> bool {
        lock(&self.inner.status).end.is_some()
            && lock(&self.pump).as_ref().is_none_or(|t| t.is_finished())
    }

    /// Wait for the pump to end, at most `wait`. `true` once it has.
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
    step: &'a mut PlaybackStep,
    source: DriveSource,
    limited: Vec<bool>,
    fs: f64,
    group: u64,
    pacing: Pacing,
}

impl Pump<'_> {
    fn emergency(&mut self) -> bool {
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
        // An emergency stop took the drive to zero, whatever the step last wrote.
        s.phase = if estop { Phase::Zeroed } else { st.phase() };
        s.generated = st.generated.load(Ordering::Acquire);
        s.played = st.played.load(Ordering::Acquire);
        s.end = Some(Outcome {
            outcome,
            message,
            trip,
            emergency_stop: estop,
            played: s.played,
            generated: s.generated,
        });
    }

    /// The zeros written from `quiet_from` on have played.
    fn zeros_played(&self) -> bool {
        let from = self.inner.step.quiet_from.load(Ordering::Acquire);
        self.engine.health().frames_in >= from + self.group
    }

    fn run(&mut self) {
        let began = Instant::now();
        let mut zero_wait = 0u32;
        loop {
            let st = self.inner.step.clone();
            // An operator's abort, before the next batch.
            if st.abort.load(Ordering::Acquire) {
                let e = self.emergency();
                self.end(
                    "aborted",
                    "operator abort: the drive went to zero at once".into(),
                    None,
                    e,
                );
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
                    let why = p
                        .downcast_ref::<&str>()
                        .map(|x| (*x).to_owned())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_default();
                    self.end(
                        "loop_panicked",
                        format!("the run's loop panicked ({why}); the emergency stop was issued"),
                        None,
                        e,
                    );
                    break;
                }
            };
            match report.reason {
                StopReason::InputEnded => {}
                StopReason::AbortTripped(t) => {
                    st.abort.store(true, Ordering::Release);
                    self.end(
                        "response_abort",
                        format!(
                            "response abort: channel {} {:?} {:.6} past its limit {:.6}; the \
                             drive went to zero at once",
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
                        format!("the run's loop stopped: {other:?}"),
                        None,
                        e,
                    );
                    break;
                }
            }
            if self.source.shared.failed.load(Ordering::Acquire) {
                let e = self.emergency();
                let why = self.source.shared.failure().unwrap_or_default();
                self.end(
                    "reader_failed",
                    format!(
                        "the drive file's reader failed ({why}); the drive went to zero at once"
                    ),
                    None,
                    e,
                );
                break;
            }
            if st.underrun.load(Ordering::Acquire) {
                let e = self.emergency();
                let at = st.played.load(Ordering::Acquire);
                self.end("drive_underrun", format!("drive underrun: the read-ahead ran empty at file sample {at}; the drive went to zero at once"), None, e);
                break;
            }
            if st.ceiling_trip.load(Ordering::Acquire) {
                let e = self.emergency();
                let ch = st.trip_channel.load(Ordering::Relaxed);
                let v = f64::from_bits(st.trip_value.load(Ordering::Relaxed));
                self.end("drive_ceiling", format!("drive ceiling: file channel {ch} asked {v:.6} V; the drive went to zero at once"), None, e);
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
                s.played = st.played.load(Ordering::Acquire);
                let h = self.engine.health();
                s.frames_in = h.frames_in;
                s.health = Some(h);
                s.buffered = self
                    .source
                    .shared
                    .pushed
                    .load(Ordering::Acquire)
                    .saturating_sub(self.source.shared.popped.load(Ordering::Acquire));
                if !overloads.is_empty() {
                    s.overloads = overloads;
                }
                if phase == Phase::Stopping || phase == Phase::Stopped {
                    s.stop_from
                        .get_or_insert(st.stop_from.load(Ordering::Acquire));
                }
            }
            if phase == Phase::Done || phase == Phase::Stopped {
                zero_wait += 1;
                if self.zeros_played() || zero_wait > ZERO_WAIT_BATCHES {
                    let _ = self.backend.stop();
                    if phase == Phase::Done {
                        self.end(
                            "completed",
                            "the file played to its end and the post-roll was recorded".into(),
                            None,
                            false,
                        );
                    } else {
                        self.end(
                            "stopped",
                            "operator stop: the drive ramped down to zero".into(),
                            None,
                            false,
                        );
                    }
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
        self.source.shared.cancel.store(true, Ordering::Release);
        let _ = self.backend.close();
    }
}
