//! Test doubles shared by the recorder's tests: a scripted station and a record store
//! that blocks, fails or crawls on request.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use hertzforge_channel_table::{
    AbortLimits, Channel, ChannelRole, ChannelTable, Interface, Polarity, Units,
};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use hertzforge_recorder::recording::{GapRecord, RecordingSummary};
use hertzforge_recorder::store::{RecordStore, RecordingSpec};

/// A unique empty folder under the system's temporary folder.
pub fn temp_root(tag: &str) -> PathBuf {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "hertzforge_recorder_{tag}_{}_{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// An enabled input row in g.
pub fn input_row(index: u32, range_max: f64, accel_limit: Option<f64>) -> Channel {
    let mut c = Channel::new(index);
    c.role = ChannelRole::Monitor;
    c.enabled = true;
    c.units = Units::AccelG;
    c.interface = Interface::SingleEnded;
    c.polarity = Polarity::Positive;
    c.range_max = range_max;
    c.sensitivity_mv_per_eu = 100.0;
    c.abort = AbortLimits {
        accel_g: accel_limit,
        velocity_ips: None,
        displacement_mil: None,
    };
    c
}

/// `n` input rows, range 100 g, no limits.
pub fn inputs(n: u32) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 1..=n {
        t.channels.push(input_row(i, 100.0, None));
    }
    t
}

/// What a scripted station has been asked to do.
#[derive(Debug, Default)]
pub struct Calls {
    pub opens: AtomicU64,
    pub arms: AtomicU64,
    pub starts: AtomicU64,
    pub stops: AtomicU64,
    pub write_drives: AtomicU64,
    /// The rate the last configure asked, as f64 bits.
    pub asked_rate_bits: AtomicU64,
    /// The block the last configure asked.
    pub asked_block: AtomicU64,
    /// Enabled drive rows in the table the last configure was given.
    pub drive_rows: AtomicU64,
}

impl Calls {
    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::SeqCst)
    }
    pub fn asked_rate(&self) -> f64 {
        f64::from_bits(self.asked_rate_bits.load(Ordering::SeqCst))
    }
}

/// The signal a scripted station reads: `f(station frame, column)`.
pub type Signal = Box<dyn Fn(u64, usize) -> f32 + Send + Sync>;

/// A station that reads a stated signal at whatever rate it programs.
pub struct ScriptedStation {
    pub n_input: usize,
    /// The device's own rate (what `caps()` says before configure).
    pub device_rate_hz: f64,
    /// What the device programs for a rate asked.
    pub program: fn(f64) -> f64,
    /// The rate programmed by the last configure.
    pub rate_hz: f64,
    pub block_frames: usize,
    pub max_outputs: u32,
    /// Frames the station delivers before its stream ends; `None` for endless.
    pub total_frames: Option<u64>,
    /// Hold delivery to the wall clock at the programmed rate.
    pub paced: bool,
    pub signal: Signal,
    /// Answer `read_input` with an I/O error in these words once this many frames
    /// have been read.
    pub fail_after: Option<(u64, String)>,
    pub abort_latency_bound: Option<Duration>,
    /// Answer `write_drive` `Unsupported`, as an input-only task does.
    pub drive_unsupported: bool,
    pub calls: Arc<Calls>,
    pos: u64,
    started_at: Option<Instant>,
    open: bool,
}

/// The device programs whatever it is asked.
pub fn programs_as_asked(rate: f64) -> f64 {
    rate
}

/// An NI-style delta-sigma ladder: 13.1072 MHz / 256 / n, the nearest rate at or
/// above the request.
pub fn delta_sigma_ladder(rate: f64) -> f64 {
    let base = 13_107_200.0 / 256.0;
    let n = (base / rate).floor().clamp(1.0, 31.0);
    base / n
}

impl ScriptedStation {
    pub fn new(n_input: usize, rate_hz: f64, signal: Signal) -> Self {
        Self {
            n_input,
            device_rate_hz: rate_hz,
            program: programs_as_asked,
            rate_hz,
            block_frames: 256,
            max_outputs: 0,
            total_frames: None,
            paced: false,
            signal,
            fail_after: None,
            abort_latency_bound: None,
            drive_unsupported: false,
            calls: Arc::new(Calls::default()),
            pos: 0,
            started_at: None,
            open: false,
        }
    }

    pub fn frames(mut self, n: u64) -> Self {
        self.total_frames = Some(n);
        self
    }

    pub fn paced(mut self) -> Self {
        self.paced = true;
        self
    }

    pub fn calls(&self) -> Arc<Calls> {
        Arc::clone(&self.calls)
    }
}

impl Backend for ScriptedStation {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: self.n_input as u32,
            max_output_channels: self.max_outputs,
            abort_latency_bound: self.abort_latency_bound,
            output_buffer: (self.max_outputs > 0).then_some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.rate_hz,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.calls.opens.fetch_add(1, Ordering::SeqCst);
        self.open = true;
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.open = false;
        Ok(())
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        let drives = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count();
        self.calls.drive_rows.store(drives as u64, Ordering::SeqCst);
        self.calls
            .asked_rate_bits
            .store(acq.requested_rate_hz.to_bits(), Ordering::SeqCst);
        self.calls
            .asked_block
            .store(acq.block_frames as u64, Ordering::SeqCst);
        self.rate_hz = (self.program)(acq.requested_rate_hz);
        self.block_frames = acq.block_frames.max(1);
        Ok(ConfiguredActual {
            actual_rate_hz: self.rate_hz,
            block_frames: self.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.calls.arms.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.calls.starts.fetch_add(1, Ordering::SeqCst);
        if self.started_at.is_none() {
            self.started_at = Some(Instant::now());
        }
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.calls.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        if let Some((at, words)) = &self.fail_after {
            if self.pos >= *at {
                return Err(HalError::Io(words.clone()));
            }
        }
        let want = (out.len() / self.n_input) as u64;
        let left = self
            .total_frames
            .map_or(u64::MAX, |t| t.saturating_sub(self.pos));
        if left == 0 {
            return Ok(InputRead::Ended);
        }
        let mut take = want.min(left);
        if let Some((at, _)) = &self.fail_after {
            take = take.min(at.saturating_sub(self.pos).max(1));
        }
        if self.paced {
            let started = *self.started_at.get_or_insert_with(Instant::now);
            let due = Duration::from_secs_f64((self.pos + take) as f64 / self.rate_hz);
            let now = started.elapsed();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        for f in 0..take as usize {
            for c in 0..self.n_input {
                out[f * self.n_input + c] = (self.signal)(self.pos + f as u64, c);
            }
        }
        self.pos += take;
        Ok(InputRead::Frames(take as usize))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.calls.write_drives.fetch_add(1, Ordering::SeqCst);
        if self.drive_unsupported {
            return Err(HalError::Unsupported("this station has no output task"));
        }
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: self.open,
            faulted: false,
            message: String::new(),
            sample_counter: self.pos,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

/// One call the store double saw.
#[derive(Debug, Clone)]
pub enum StoreCall {
    Start {
        name: String,
        pre_frames: u64,
        thread: ThreadId,
    },
    Push {
        frames: usize,
        thread: ThreadId,
    },
    Finish {
        thread: ThreadId,
    },
}

/// What the store double has been handed.
#[derive(Debug, Default)]
pub struct StoreLog {
    pub calls: Vec<StoreCall>,
    /// Every frame pushed, interleaved, per recording.
    pub recordings: Vec<Vec<f32>>,
}

/// A record store that keeps everything in memory and blocks, fails or crawls on
/// request.
pub struct StoreDouble {
    pub log: Arc<Mutex<StoreLog>>,
    pub start_delay: Duration,
    pub finish_delay: Duration,
    /// Wall time each pushed frame costs, at `pace_fs_hz` frames a second times this
    /// factor (2.0 writes at half the stream's rate).
    pub slow_factor: Option<(f64, f64)>,
    /// Fail with this text once this many pushes have been taken.
    pub fail_after_pushes: Option<(usize, String)>,
    pub fault_now: Option<String>,
    n_channels: usize,
    on_disk: u64,
    pushes: usize,
    active: Option<String>,
}

impl StoreDouble {
    pub fn new(n_channels: usize) -> (Self, Arc<Mutex<StoreLog>>) {
        let log = Arc::new(Mutex::new(StoreLog::default()));
        (
            Self {
                log: Arc::clone(&log),
                start_delay: Duration::ZERO,
                finish_delay: Duration::ZERO,
                slow_factor: None,
                fail_after_pushes: None,
                fault_now: None,
                n_channels,
                on_disk: 0,
                pushes: 0,
                active: None,
            },
            log,
        )
    }
}

impl RecordStore for StoreDouble {
    fn start(&mut self, spec: RecordingSpec) -> Result<String, String> {
        let pre = spec.pretrigger.as_ref().map_or(0, |e| e.pretrigger_frames);
        self.log.lock().unwrap().calls.push(StoreCall::Start {
            name: spec.name.clone(),
            pre_frames: pre,
            thread: std::thread::current().id(),
        });
        std::thread::sleep(self.start_delay);
        let mut first = Vec::new();
        if let Some(ev) = &spec.pretrigger {
            first.extend_from_slice(&ev.pretrigger);
        }
        self.on_disk = pre;
        self.log.lock().unwrap().recordings.push(first);
        self.active = Some(spec.name.clone());
        Ok(spec.name)
    }

    fn push(&mut self, frames: &[f32], n_frames: usize) {
        self.pushes += 1;
        self.log.lock().unwrap().calls.push(StoreCall::Push {
            frames: n_frames,
            thread: std::thread::current().id(),
        });
        if let Some((fs, factor)) = self.slow_factor {
            std::thread::sleep(Duration::from_secs_f64(n_frames as f64 / fs * factor));
        }
        if let Some((after, text)) = &self.fail_after_pushes {
            if self.pushes > *after {
                self.fault_now = Some(text.clone());
            }
        }
        if self.fault_now.is_some() {
            return;
        }
        let mut log = self.log.lock().unwrap();
        if let Some(r) = log.recordings.last_mut() {
            r.extend_from_slice(&frames[..n_frames * self.n_channels]);
        }
        self.on_disk += n_frames as u64;
    }

    fn finish(&mut self) -> Result<RecordingSummary, String> {
        self.log.lock().unwrap().calls.push(StoreCall::Finish {
            thread: std::thread::current().id(),
        });
        std::thread::sleep(self.finish_delay);
        let name = self.active.take().unwrap_or_default();
        let frames = self.on_disk;
        self.on_disk = 0;
        Ok(RecordingSummary {
            run_dir: PathBuf::from(name),
            frames_written: frames,
            gaps: Vec::<GapRecord>::new(),
            gap_free: true,
            bytes: frames * self.n_channels as u64 * 4,
        })
    }

    fn fault(&self) -> Option<String> {
        self.fault_now.clone()
    }

    fn frames_on_disk(&self) -> u64 {
        self.on_disk
    }
}
