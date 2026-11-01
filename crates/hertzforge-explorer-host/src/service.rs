//! [`ExplorerHostService`] - the host-closes-loop Explorer run orchestration.
//!
//! # The shape, and what is load-bearing about it
//!
//! * **A persistent [`Engine`] run in BOUNDED BATCHES.** `run_ticks`, not `run` (whose
//!   epilogue stops the backend - on an NI station the AO does not come back from that,
//!   and the next batch dies on the stall deadman). One to two ticks per batch, with the
//!   command mailbox, the stop/abort flags and the health check between them. Not an
//!   internal thread: [`ExplorerHostService::run_batches`] is called BY the host's pump,
//!   so the caller owns the cadence and the shutdown.
//! * **Analysis is OFF the tick.** The process step copies each acquired block into a
//!   ring; [`ExplorerHostService::service_analysis`] drains it into the spectrum /
//!   derived / profile engines between batches. The measured display FFT is ~24 ms and
//!   the block period on the chassis is ~15 ms, so the alternative is a
//!   `SchedulingFault` per display update.
//! * **The rate is ADOPTED, then the gate is consulted with it.** `configure` answers
//!   with what the driver really programmed; the oscillator, the band, the slew
//!   increments and the display grid are all re-derived from that answer BEFORE
//!   [`hertzforge_module_abi::arm_gate`] is called, and the gate is passed `Some(rate)`,
//!   never `None`. This is Random's 6.25x lesson: it asked for 8192 Hz, the 9234
//!   programmed 51 200 Hz, and every profile frequency landed 6.25x high on the shaker
//!   because the loop kept synthesizing as if it played at 8192.
//! * **The pump body runs under `catch_unwind`.** A panic inside a tick must not leave
//!   the 9263 holding its last sample while the stack unwinds past every stop path.
//!
//! # Arm posture: 0.000 V, and why that is the review hold
//!
//! Every other module that drives a shaker gets a hold between "armed" and "moving" -
//! somewhere a human confirms before energy goes into the armature. Explorer has none,
//! and the reason is that its arm posture already IS one: the commanded drive at arm is
//! **exactly 0.000 V**, the lead-in written to the device before the clock starts is
//! zeros BY VALUE (they are what the step generates at that command, not a substituted
//! silence), and nothing moves until a human raises the level through
//! [`ExplorerHostService::set_amplitude_vpk`]. A hold in front of that would be a
//! confirmation that nothing is about to happen.
//!
//! What that posture does NOT buy is a quiet safety layer: the volts ceiling and the
//! abort path are live from the FIRST tick, not from the first non-zero command, because
//! the hazard begins the moment the AO clock runs.
//!
//! # Which monitor is AUTHORITATIVE
//!
//! **The engine's [`AbortWatchdog`](hertzforge_engine::AbortWatchdog) is.** It reads the raw
//! ring INSIDE the tick, sees every frame, and its trip stops the run and calls
//! `emergency_stop` before this service is next scheduled. The
//! [`AbortMonitor`](crate::guards::AbortMonitor) this service feeds from the analysis
//! ring in [`ExplorerHostService::service_analysis`] is a SECOND, SLOWER read of the
//! same limits, and it exists for one thing the watchdog's per-run integrator state does
//! not give: a settle window measured in high-pass time constants, applied to the
//! derived velocity/displacement axes only.
//!
//! So the two do not race for the same decision. A watchdog trip ABORTS (the engine has
//! already stopped the device by the time it is reported). An `AbortMonitor` trip is an
//! OBSERVATION: it is surfaced in the status and it requests a RAMPED stop, because by
//! construction it can only fire where the authoritative monitor deliberately did not.
//! Treating it as an abort would mean a slower, partial view of the same data
//! overriding the complete one.

use crate::analysis::{
    DerivedTracer, ProfilePoint, ProfileTrace, ProfileTracer, SpectrumEngine, SpectrumFrame,
    SpectrumKind,
};
use crate::floor::{FloorProbe, NoiseFloorProbe};
use crate::freq::WindowedFreqMeasure;
use crate::guards::{AbortMonitor, AbortMonitorError};
use crate::ring::SampleRing;
use crate::slew::SlewLimits;
use crate::step::{
    AmplitudeRamp, ExplorerProcessStep, StepBuild, StepHandles, SweepDirection, SweepState,
};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_engine::{AbortTrip, Engine, EngineConfig, StopReason, VecCaptureSink};
use hertzforge_hal::{Backend, ConfiguredActual};
use hertzforge_module_abi::StationClass;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ticks per `run_ticks` call.
///
/// The batch is BOUNDED and small on purpose. `run_ticks` has no cooperative stop, so
/// the batch length IS the worst-case latency from an operator intent (or a stop) to the
/// drive acting on it; at two ticks that is two block periods, ~30 ms on the chassis.
/// One tick would halve it and double the per-batch overhead the between-batch work
/// already has to fit inside; two sits inside the targeted 1-2.
pub(crate) const TICKS_PER_BATCH: u64 = 2;

/// The HERTZFORGE-SIM abort-watchdog debounce, in RAW FRAMES. Mirrors the sine service's
/// value; a frame count is only a TIME against a rate, which is why the hardware branch
/// converts instead of sharing this literal.
const SIM_WATCHDOG_SUSTAIN_FRAMES: u32 = 16;

/// The abort-watchdog debounce on a HARDWARE station, in SECONDS - the same quarter
/// millisecond, in the unit that survives a rate change.
const WATCHDOG_SUSTAIN_SECONDS: f64 = 0.00025;

/// High-pass corner for the derived velocity/displacement integrators, Hz.
const DERIVED_HP_HZ: f64 = 5.0;

/// High-pass time constants the derived-axis settle window spans.
const SETTLE_HP_CONSTANTS: f64 = 5.0;

/// Blocks of acquisition the analysis ring holds before it drops the oldest.
///
/// Sized in TIME, not blocks-as-a-number: 16 blocks at the chassis block
/// period (~15 ms) is ~0.25 s of runway for the pump. A pump that falls further behind
/// than a quarter second is not going to catch up by being given more memory, and the
/// drop counter says so.
const DEFAULT_RING_BLOCKS: usize = 16;

/// Driver diagnostics retained for the status surface, most recent kept.
///
/// Bounded because `take_warnings` is drained every batch and a driver that has
/// something to say every block would otherwise grow this without limit across a soak.
/// The BOUND is the honest part: a status that silently truncated would look like a
/// station with nothing to report.
pub const MAX_DRIVER_WARNINGS: usize = 32;

/// Errors from an [`ExplorerHostService`] operation.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceError {
    /// The configuration or the channel table was rejected.
    Config(String),
    /// The operation is not allowed in the current run state.
    BadState(String),
    /// The backend refused an operation, or the run faulted against it.
    Plant(String),
    /// The arm gate refused.
    ///
    /// The TYPED reason travels, not a formatted string: the string is what an operator
    /// reads (this error's `Display` prints it verbatim), and the variant is what a
    /// caller branches on - a UI panel that has to render a different affordance for
    /// "wrong backend" than for "no abort-latency bound", and a test that has to assert
    /// WHICH check refused rather than that some refusal text contained a word.
    ArmRefused(hertzforge_module_abi::UnsupportedReason),
    /// The pump body PANICKED. `emergency_stop` was called before this was returned.
    PumpPanicked(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Config(s) => write!(f, "config error: {s}"),
            ServiceError::BadState(s) => write!(f, "bad state: {s}"),
            ServiceError::Plant(s) => write!(f, "plant error: {s}"),
            ServiceError::ArmRefused(s) => write!(f, "arm refused: {s}"),
            ServiceError::PumpPanicked(s) => write!(
                f,
                "explorer pump panicked ({s}); emergency_stop was issued and the run is aborted"
            ),
        }
    }
}

impl std::error::Error for ServiceError {}

/// The run state machine. **Sweeping is a FLAG, not a state** - an operator starting or
/// stopping a sweep does not change what phase the run is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// Constructed; nothing opened.
    Idle,
    /// The station is open, configured, armed, prefilled and CLOCKED, with 0.000 V
    /// commanded. The pump has not been let loose yet.
    Armed,
    /// The pump may run tick batches.
    Running,
    /// A stop was requested: the drive is ramping to zero at the operator slew. The
    /// backend is still clocked - stopping it here is what would put the step into the
    /// armature that the ramp exists to avoid.
    Stopping,
    /// The ramp completed and the backend was stopped. Terminal.
    Stopped,
    /// Aborted. `emergency_stop` was issued. Terminal.
    Aborted,
}

impl RunState {
    /// Is the run over (no further batches will do anything)?
    pub fn is_terminal(self) -> bool {
        matches!(self, RunState::Stopped | RunState::Aborted)
    }
}

/// An operator intent posted into the mailbox from OUTSIDE the pump thread.
///
/// The mailbox exists so a command dispatcher never has to hold the service lock (the
/// Rule: run commands beside the frame forwarder, not in front of it). A caller
/// that already holds `&mut self` should use the named intent methods instead - they
/// return the CLAMPED value, which the mailbox path cannot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExplorerCommand {
    /// Go to an absolute frequency (Hz). Clamped to the band.
    SetFrequency(f64),
    /// Move the frequency by a delta (Hz), relative to the current TARGET.
    AdjustFrequency(f64),
    /// Set the drive amplitude (volts peak). Clamped to `[0, max_drive_vpk]`.
    SetAmplitudeVpk(f64),
    /// Start (or re-aim) a sweep.
    StartSweep {
        direction: SweepDirection,
        rate_oct_per_min: f64,
    },
    /// Stop the sweep, leaving the tone where it is.
    StopSweep,
    /// Ramp the drive to zero and end the run.
    RequestStop,
}

/// The lock-free-drain command mailbox: any thread posts, the pump drains BETWEEN tick
/// batches. The lock is held only for the O(1) push and the drain swap - never across a
/// batch, and never on the RT tick at all.
#[derive(Debug, Default)]
pub struct CommandMailbox {
    inner: Mutex<Vec<ExplorerCommand>>,
}

impl CommandMailbox {
    /// Post a command.
    pub fn post(&self, cmd: ExplorerCommand) {
        if let Ok(mut q) = self.inner.lock() {
            q.push(cmd);
        }
    }

    /// Drain all pending commands (pump side).
    pub fn drain(&self) -> Vec<ExplorerCommand> {
        match self.inner.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        }
    }
}

/// Why a run ended, latched for the status surface.
///
/// A run that ENDS must say so and must name what ended it - a still pane
/// is indistinguishable from a hung app.
#[derive(Debug, Clone, PartialEq)]
pub struct EndRecord {
    /// The state the run landed in.
    pub state: RunState,
    /// Operator-readable cause.
    pub reason: String,
    /// The limit breach, when a monitor is what ended it.
    pub trip: Option<AbortTrip>,
}

/// The abort latch: the pump sets it, any observer reads it without the service lock.
#[derive(Debug, Default)]
pub struct AbortLatch {
    tripped: AtomicBool,
    record: Mutex<Option<EndRecord>>,
}

impl AbortLatch {
    /// Whether the run has ended abnormally.
    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::Acquire)
    }

    /// The latched end record, if any (cloned, not taken).
    pub fn record(&self) -> Option<EndRecord> {
        self.record.lock().ok().and_then(|r| r.clone())
    }

    fn set(&self, record: EndRecord) {
        if let Ok(mut r) = self.record.lock() {
            if r.is_none() {
                *r = Some(record);
            }
        }
        self.tripped.store(true, Ordering::Release);
    }
}

/// The full status snapshot. Published by [`ExplorerHostService::service_analysis`] -
/// i.e. off the RT tick, from the same pass that produced the spectra it describes, so
/// the numbers on a panel all belong to one moment.
#[derive(Debug, Clone)]
pub struct ExplorerStatus {
    pub state: RunState,
    pub station: StationClass,
    /// The rate the driver ACTUALLY programmed. Everything below is denominated in it.
    pub actual_fs_hz: f64,
    /// The rate that was asked for. Kept beside the actual so a coercion is visible
    /// rather than merely absorbed.
    pub requested_fs_hz: f64,
    /// The band after narrowing to `0.45 * actual_fs`.
    pub band_hz: (f64, f64),
    /// The frequency the oscillator is generating right now.
    pub commanded_freq_hz: f64,
    /// Where the tone is travelling (an operator intent, or the sweep's).
    pub target_freq_hz: f64,
    /// The frequency MEASURED on the control channel, when a window has matured. The
    /// cross-check against `commanded_freq_hz`; `None` is the honest answer for a drive
    /// at zero or a window that has not filled. The window opens only once the drive in
    /// flight from before the tone settled has been acquired ([`Self::pipeline_frames`]),
    /// so the first reading after a retune is the NEW tone's, not a blend with the old.
    pub measured_freq_hz: Option<f64>,
    /// The amplitude being generated right now, volts peak.
    pub commanded_vpk: f64,
    /// The amplitude the operator asked for, volts peak.
    pub target_vpk: f64,
    /// The hard ceiling every amplitude intent is clamped to.
    pub max_drive_vpk: f64,
    /// The live sweep, when one is running (`halted` says it reached an edge).
    pub sweep: Option<SweepState>,
    /// RMS per acquired channel over the blocks this analysis pass consumed, in
    /// interleave order. Empty when the pass consumed nothing.
    pub per_channel_rms: Vec<f64>,
    /// The measured noise floor per acquired channel (fail-closed `Unknown` until it
    /// matures, and forever once a drive change contaminates it).
    pub floor: Vec<FloorProbe>,
    /// Whether the floor probe matured before the first drive change.
    pub floor_matured: bool,
    /// The control channel's derived `(velocity in/s, displacement mil)`.
    pub derived: (f64, f64),
    /// True while the derived integrators are still inside their startup transient. A
    /// UI must not plot this window as real motion.
    pub derived_settling: bool,
    /// Frames of drive the step has generated this run (run-relative, not a device
    /// counter).
    pub frames_generated: u64,
    /// Frames the analysis pass has consumed this run.
    pub frames_analyzed: u64,
    /// **The acquisition-vs-drive PIPELINE, in frames**: how many further input frames
    /// still carry the PREVIOUS tone at the instant a retune is commanded. See
    /// [`ExplorerHostService::pipeline_frames`] for the arithmetic and why it is the
    /// number the profile capture and the measured-frequency window are aligned to.
    pub pipeline_frames: usize,
    /// Acquired blocks the ring had to drop because the pump fell behind.
    pub blocks_dropped: u64,
    /// Engine ticks executed.
    pub engine_ticks: u64,
    /// Worst per-tick compute observed, and the budget it is measured against.
    pub worst_tick_compute: Duration,
    pub compute_budget: Duration,
    /// Scheduling-health faults so far.
    pub health_faults: u64,
    /// Raw-ring drops inside the engine (a limits-armed run FAILS on any).
    pub ring_overruns: usize,
    /// A limit breach the SECONDARY monitor observed. See the module docs for why this
    /// is an observation and not an abort.
    pub observed_trip: Option<AbortTrip>,
    /// Non-fatal DRIVER diagnostics collected since the run began (DAQmx underruns,
    /// coercions, late samples), most recent last and bounded to
    /// [`MAX_DRIVER_WARNINGS`].
    ///
    /// A warning the driver already reported and nothing surfaces is a signal that cannot
    /// reach anyone. `Backend::take_warnings` is DRAINING, so if this service does not
    /// pull them nobody does - they are simply lost, silently, and the first anyone hears
    /// of an underrun is a symptom.
    pub driver_warnings: Vec<String>,
    /// Set once the run has ended, naming what ended it.
    pub end: Option<EndRecord>,
}

/// The shared status cell an observer reads without touching the service.
#[derive(Debug, Default)]
pub struct StatusCell {
    inner: Mutex<Option<ExplorerStatus>>,
}

impl StatusCell {
    /// The latest published status, or `None` before the first analysis pass. `None` is
    /// deliberately not a zeroed default: a panel that has never been told anything must
    /// say so rather than render a plausible set of zeros.
    pub fn read(&self) -> Option<ExplorerStatus> {
        self.inner.lock().ok().and_then(|s| s.clone())
    }

    fn store(&self, s: ExplorerStatus) {
        if let Ok(mut cell) = self.inner.lock() {
            *cell = Some(s);
        }
    }
}

/// What the operator asked for, BEFORE the station answered.
///
/// `fs_hz` and `band_hz` are REQUESTS. After [`ExplorerHostService::arm`] the service's
/// own [`ExplorerHostService::actual_fs_hz`] / [`ExplorerHostService::band_hz`] are the
/// truth; this struct keeps its original values so a coercion stays visible.
#[derive(Debug, Clone)]
pub struct ExplorerHostConfig {
    /// Requested acquisition rate, Hz.
    pub fs_hz: f64,
    /// Requested frequency band `[lo, hi]`, Hz.
    pub band_hz: (f64, f64),
    /// Where the oscillator parks at arm.
    pub start_hz: f64,
    /// The hard volts-peak ceiling on the drive.
    pub max_drive_vpk: f64,
    /// Operator slew ceilings (octaves/s, dB/s).
    pub slew: SlewLimits,
    /// The zero-crossing rule for the amplitude ramp.
    pub ramp: AmplitudeRamp,
    /// FFT size for the DISPLAY spectra. Power of two.
    pub display_nfft: usize,
    /// Blocks of acquisition the analysis ring holds.
    pub ring_blocks: usize,
}

impl Default for ExplorerHostConfig {
    fn default() -> Self {
        Self {
            fs_hz: 8_192.0,
            band_hz: (10.0, 2_000.0),
            start_hz: 100.0,
            max_drive_vpk: 1.0,
            slew: SlewLimits::default(),
            ramp: AmplitudeRamp::default(),
            display_nfft: 4_096,
            ring_blocks: DEFAULT_RING_BLOCKS,
        }
    }
}

impl ExplorerHostConfig {
    fn validate(&self) -> Result<(), ServiceError> {
        let bad = |m: String| Err(ServiceError::Config(m));
        if !(self.fs_hz.is_finite() && self.fs_hz > 0.0) {
            return bad(format!(
                "fs_hz {} is not a positive finite rate",
                self.fs_hz
            ));
        }
        let (lo, hi) = self.band_hz;
        if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && hi > lo) {
            return bad(format!(
                "band {lo}..{hi} Hz is not a positive increasing band"
            ));
        }
        if hi > 0.45 * self.fs_hz {
            return bad(format!(
                "band high edge {hi} Hz exceeds 0.45 x the requested rate ({} Hz); raise \
                 the rate or lower the band",
                0.45 * self.fs_hz
            ));
        }
        if !(self.start_hz.is_finite() && (lo..=hi).contains(&self.start_hz)) {
            return bad(format!(
                "start {} Hz is outside the band {lo}..{hi} Hz",
                self.start_hz
            ));
        }
        if !(self.max_drive_vpk.is_finite() && self.max_drive_vpk > 0.0) {
            return bad(format!(
                "max_drive_vpk {} is not a positive finite volts ceiling",
                self.max_drive_vpk
            ));
        }
        if self.display_nfft < 2 || !self.display_nfft.is_power_of_two() {
            return bad(format!(
                "display_nfft {} must be a power of two >= 2",
                self.display_nfft
            ));
        }
        if self.ring_blocks == 0 {
            return bad("ring_blocks must be at least 1".into());
        }
        if !(self.slew.freq_oct_per_s.is_finite() && self.slew.freq_oct_per_s > 0.0) {
            return bad("slew.freq_oct_per_s must be positive and finite".into());
        }
        if !(self.slew.drive_db_per_s.is_finite() && self.slew.drive_db_per_s > 0.0) {
            return bad("slew.drive_db_per_s must be positive and finite".into());
        }
        if !(self.ramp.ramp_span_db.is_finite() && self.ramp.ramp_span_db > 0.0) {
            return bad("ramp.ramp_span_db must be positive and finite".into());
        }
        Ok(())
    }
}

/// The host-side Explorer run service. See the module docs for the shape.
pub struct ExplorerHostService {
    cfg: ExplorerHostConfig,
    table: ChannelTable,
    station: StationClass,
    state: RunState,

    /// The backend, held from construction. `None` only after an arm that failed
    /// partway: there is no re-arm path, and a second attempt says so rather than
    /// silently building a second station.
    backend: Option<Box<dyn Backend>>,
    /// What `configure` answered with. The authority for everything derived.
    configured: Option<ConfiguredActual>,
    /// The ADOPTED rate: `configured.actual_rate_hz`.
    fs_hz: f64,
    /// The band after narrowing to the adopted rate.
    band_hz: (f64, f64),

    step: Option<ExplorerProcessStep>,
    handles: Option<StepHandles>,
    engine: Option<Engine>,

    // --- station-derived engine knobs -----------------------------------------
    block_frames: usize,
    jitter_frac: f64,
    compute_budget: Option<Duration>,

    // --- acquired-frame geometry ----------------------------------------------
    /// Interleave width of an acquired frame: enabled non-Drive rows.
    n_interleave: usize,
    /// The control channel's slot in that frame.
    control_slot: usize,
    /// 1-based table indices of the acquired channels, in interleave order.
    input_indices: Vec<u32>,

    // --- off-RT analysis ------------------------------------------------------
    spectra: Option<SpectrumEngine>,
    latest_spectra: Vec<SpectrumFrame>,
    derived: Option<DerivedTracer>,
    profile: Option<ProfileTracer>,
    freq_meter: Option<WindowedFreqMeasure>,
    floor: Option<NoiseFloorProbe>,
    monitor: Option<AbortMonitor>,
    observed_trip: Option<AbortTrip>,
    /// Driver diagnostics drained from the backend after every batch.
    driver_warnings: Vec<String>,
    /// Reusable analysis scratch, so a pass does not allocate per block.
    drained: Vec<Vec<f32>>,
    scratch_f64: Vec<f64>,
    control_scratch: Vec<f64>,
    /// The profile's per-PASS control run (see `service_analysis_tapped`).
    profile_scratch: Vec<f64>,
    frames_analyzed: u64,
    /// The generated frequency at the previous analysis pass. A zero-crossing window is
    /// only meaningful over a STEADY tone, so a change here discards the window.
    last_tone_hz: f64,
    /// The ring's drop count at the previous analysis pass. A change means acquisition
    /// went missing between that pass's blocks and this one's, so the measured window must
    /// not test the interval across the gap as if it were one sample interval.
    ring_dropped_seen: u64,
    /// Input frames the measured-frequency window must still DISCARD before it may take a
    /// sample: the drive that was already in flight when the generator last changed state.
    /// Re-armed to [`Self::pipeline_frames`] by every pass that is not measurable.
    meter_skip: usize,

    // --- shared surfaces ------------------------------------------------------
    mailbox: Arc<CommandMailbox>,
    status_cell: Arc<StatusCell>,
    abort_latch: Arc<AbortLatch>,
    end: Option<EndRecord>,
}

impl ExplorerHostService {
    /// Build a service for `cfg` against `table` on `backend`.
    ///
    /// Mirrors `SineService::new`'s shape deliberately: a FINISHED `Box<dyn Backend>`
    /// plus the [`StationClass`] the factory bound it to. This crate never names a
    /// concrete backend and holds no dependency on `hertzforge-station` - a service that
    /// builds its own plant is a service that can substitute software for absent
    /// hardware without its caller knowing.
    pub fn new(
        cfg: ExplorerHostConfig,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        station: StationClass,
    ) -> Result<Self, ServiceError> {
        cfg.validate()?;
        table
            .validate()
            .map_err(|e| ServiceError::Config(format!("channel table invalid: {e}")))?;

        let enabled_inputs: Vec<&hertzforge_channel_table::Channel> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .collect();
        if enabled_inputs.is_empty() {
            return Err(ServiceError::Config(
                "no enabled input (control/monitor/reference) channels: an Explorer run with \
                 nothing to measure has no cross-check on the tone it commands"
                    .into(),
            ));
        }
        let n_drive = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count();
        if n_drive > 1 {
            return Err(ServiceError::Config(format!(
                "{n_drive} enabled Drive rows: Explorer synthesizes ONE tone and would leave \
                 the others silent, which is an output the operator configured and nothing \
                 commands. Disable the extra Drive row(s)."
            )));
        }
        let input_indices: Vec<u32> = enabled_inputs.iter().map(|c| c.index).collect();
        let n_interleave = enabled_inputs.len();
        // The control channel: the first enabled Control row, else the first enabled
        // input. Its slot is its POSITION among enabled inputs, which is what a
        // configured backend presents per frame.
        //
        // NB `AbortMonitor` derives ITS slots as `index - 1` instead (its own documented
        // convention, so that disabling a channel cannot silently re-point a limiter at a
        // different signal). The two agree for any table whose enabled input indices are
        // 1..=n, which is every table this path builds today; they are not the same rule
        // and neither is being changed here.
        let control_slot = enabled_inputs
            .iter()
            .position(|c| c.role == ChannelRole::Control)
            .unwrap_or(0);

        let fs_hz = cfg.fs_hz;
        let band_hz = cfg.band_hz;
        Ok(Self {
            block_frames: Self::default_block_frames(station, fs_hz),
            jitter_frac: Self::default_jitter_frac(station),
            compute_budget: Self::default_compute_budget(station),
            cfg,
            table,
            station,
            state: RunState::Idle,
            backend: Some(backend),
            configured: None,
            fs_hz,
            band_hz,
            step: None,
            handles: None,
            engine: None,
            n_interleave,
            control_slot,
            input_indices,
            spectra: None,
            latest_spectra: Vec::new(),
            derived: None,
            profile: None,
            freq_meter: None,
            floor: None,
            monitor: None,
            observed_trip: None,
            driver_warnings: Vec::new(),
            drained: Vec::new(),
            scratch_f64: Vec::new(),
            control_scratch: Vec::new(),
            profile_scratch: Vec::new(),
            frames_analyzed: 0,
            last_tone_hz: f64::NAN,
            ring_dropped_seen: 0,
            meter_skip: 0,
            mailbox: Arc::new(CommandMailbox::default()),
            status_cell: Arc::new(StatusCell::default()),
            abort_latch: Arc::new(AbortLatch::default()),
            end: None,
        })
    }

    /// The acquisition block this station should REQUEST at configure. Same station
    /// split the sine service documents: HERTZFORGE-SIM keeps the natural one-frame block so
    /// a frame count and a tick count coincide in the harness; hardware takes the
    /// rate-derived block the driver actually wants per read.
    fn default_block_frames(station: StationClass, fs_hz: f64) -> usize {
        if station.is_hardware() {
            hertzforge_engine::default_block_frames(fs_hz)
        } else {
            hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES
        }
    }

    /// The scheduling-health period-jitter allowance for this station.
    ///
    /// HERTZFORGE-SIM gets [`hertzforge_engine::TEST_DISABLE_JITTER_GUARD`] BY NAME: the harness
    /// executes the engine flat out and then stops between batches for the analysis
    /// pass, so the wall-clock interval between ticks bears no relation to the sample
    /// cadence and the guard would flag every batch boundary.
    ///
    /// A HARDWARE station gets the engine's own block-derived budget, where the tick
    /// cadence IS the sample cadence and period jitter is exactly what the guard is for.
    /// This is meant to be replaced by a figure MEASURED from a tick-period
    /// histogram on the chassis; until that measurement exists this inherits the engine
    /// default rather than inventing a number, and [`Self::with_jitter_frac`] is how the
    /// measured value gets in.
    fn default_jitter_frac(station: StationClass) -> f64 {
        if station.is_hardware() {
            EngineConfig::new(1, 1).jitter_frac
        } else {
            hertzforge_engine::TEST_DISABLE_JITTER_GUARD
        }
    }

    /// The per-tick COMPUTE budget for this station.
    ///
    /// HERTZFORGE-SIM gets [`hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET`] BY NAME; a HARDWARE
    /// station gets `None`, i.e. the tick period, because there a compute overrun means
    /// the drive really did arrive late.
    fn default_compute_budget(station: StationClass) -> Option<Duration> {
        if station.is_hardware() {
            None
        } else {
            Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET)
        }
    }

    /// Override the acquisition block requested at configure. Before `arm()`.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.block_frames = frames.max(1);
        self
    }

    /// Override the period-jitter allowance with a MEASURED one. Before `arm()`.
    pub fn with_jitter_frac(mut self, frac: f64) -> Self {
        self.jitter_frac = frac;
        self
    }

    /// The station class this service arms against.
    pub fn station(&self) -> StationClass {
        self.station
    }

    /// The run state.
    pub fn state(&self) -> RunState {
        self.state
    }

    /// The ADOPTED sample rate. Before `arm()` this is still the request.
    pub fn actual_fs_hz(&self) -> f64 {
        self.fs_hz
    }

    /// What `configure` answered with, once armed.
    pub fn configured(&self) -> Option<ConfiguredActual> {
        self.configured
    }

    /// The band after narrowing to the adopted rate.
    pub fn band_hz(&self) -> (f64, f64) {
        self.band_hz
    }

    /// The command mailbox, for a dispatcher that must not take the service lock.
    pub fn commands(&self) -> Arc<CommandMailbox> {
        self.mailbox.clone()
    }

    /// The status cell, for an observer that must not take the service lock.
    pub fn status_cell(&self) -> Arc<StatusCell> {
        self.status_cell.clone()
    }

    /// The abort latch, for an observer that must not take the service lock.
    pub fn abort_latch(&self) -> Arc<AbortLatch> {
        self.abort_latch.clone()
    }

    /// The end record, once the run has ended.
    pub fn end_record(&self) -> Option<&EndRecord> {
        self.end.as_ref()
    }

    /// The spectra produced by the most recent analysis pass that completed a window.
    pub fn latest_spectra(&self) -> &[SpectrumFrame] {
        &self.latest_spectra
    }

    /// The accumulated resonance profile (amplitude vs frequency) for the control
    /// channel.
    pub fn profile_trace(&self) -> Option<&ProfileTrace> {
        self.profile.as_ref().map(|p| p.trace())
    }

    /// Drain the profile points recorded since the last call - the streaming half of
    /// [`Self::profile_trace`].
    ///
    /// Each point is a `(commanded frequency, level)` pair on the profile channel, the
    /// level being the short-window RMS amplitude of the freshest signal at that tone
    /// (see [`crate::analysis::ProfileTracer::observe`]). At most one point per VISITED
    /// FREQUENCY, not one per pass: a dwell overwrites its own point rather than adding
    /// to it. The consumer that remembers them is the daemon's peak-profile raster;
    /// draining here is what keeps the tracer's vector from growing for the length of a
    /// survey.
    pub fn take_profile_points(&mut self) -> Vec<ProfilePoint> {
        self.profile
            .as_mut()
            .map(|p| p.take_points())
            .unwrap_or_default()
    }

    /// **THE PIPELINE, in input frames**: how many further acquired frames still carry the
    /// PREVIOUS tone at the instant the oscillator is retuned.
    ///
    /// # The arithmetic, and why it needs nothing new from the engine
    ///
    /// AI and AO ride one sample clock and start together, so output stream index `n` is
    /// converted at the same clock tick that input stream index `n` is acquired. The host
    /// is AHEAD of that point by construction: it has GENERATED `G` drive frames
    /// (`frames_generated`) and PULLED `F` input frames (`EngineHealth::frames_in`), and
    /// the difference is exactly the drive that is written-but-not-yet-converted - the
    /// engine's own delivery queue PLUS whatever the device's output FIFO is holding. On
    /// top of that the acquisition path delays what it reports by its own
    /// `group_delay_frames` (a delta-sigma decimation filter's), so:
    ///
    /// ```text
    /// pipeline = (frames_generated - frames_in) + group_delay_frames
    /// ```
    ///
    /// Nothing here introspects the device. `G - F` is invariant to how deep the AO FIFO
    /// is, because `F` counts what has been ACQUIRED and therefore what has been PLAYED;
    /// a deeper FIFO simply makes the host run further ahead, which is the difference.
    ///
    /// # What it is on each station
    ///
    /// The lead-in (`arm`) generates `2 x min_lookahead_frames`, writes half to the device
    /// and adopts half into the engine's queue; `OutputDelivery` then holds the queue at
    /// the contracted lookahead, and `NiBackend::start` tops the device side back up to
    /// the same. So the steady-state value is `2 x min_lookahead_frames + group_delay`.
    ///
    /// On the NI chassis the contract is `lookahead_blocks (2) x block_frames`, and
    /// `block_frames` is `hertzforge_hal::default_block_frames`, i.e. the power of two nearest
    /// `TARGET_BLOCK_SECONDS` (20 ms) of the REQUESTED rate. The pipeline is therefore
    /// **about four block periods, ~80 ms of signal, on any station** rather than a fixed
    /// frame count. MEASURED for the shipped Explorer profile (8192 Hz requested -> block
    /// 128, coerced to 8533.33 Hz by the 9234's ladder): `2 x (2 x 128) = 512` frames plus
    /// a 39-frame 9234 group delay = **551 frames = 64.6 ms**.
    ///
    /// On HERTZFORGE-SIM the contract is 4 frames, so it is ~1 ms - which is why no sim test
    /// could have found this, and why `tests/pipeline_alignment.rs` models the pipeline
    /// with a backend wrapper rather than trusting the Sim to have one.
    ///
    /// # Why `None` group delay reads as zero HERE
    ///
    /// `ConfiguredActual::group_delay_frames` is `None` for "not characterized", and a
    /// consumer needing a LATENCY BOUND must refuse rather than assume zero. This is not
    /// that consumer: it is a measurement ALIGNMENT, the queue term dominates it, and
    /// treating an uncharacterized path as zero makes this a LOWER bound on the true
    /// misalignment - conservative in the direction of measuring the old tone, never of
    /// claiming a settling that did not happen. The NI backend does characterize it.
    pub fn pipeline_frames(&self) -> usize {
        let generated = self
            .handles
            .as_ref()
            .map(|h| h.frames_generated.load(Ordering::Relaxed))
            .unwrap_or(0);
        let consumed = self
            .engine
            .as_ref()
            .map(|e| e.health().frames_in)
            .unwrap_or(0);
        let in_flight = generated.saturating_sub(consumed) as usize;
        let group = self
            .configured
            .as_ref()
            .and_then(|c| c.group_delay_frames)
            .unwrap_or(0) as usize;
        in_flight + group
    }

    /// The display FFT size this run was configured with. The display spectrum grid is
    /// `f0 = 0`, `df = actual_fs / display_nfft`, `n_bins = display_nfft / 2 + 1` - what
    /// [`crate::analysis::ChannelSpectrum`] emits, and what anything drawn against those
    /// spectra has to be rasterized onto.
    pub fn display_nfft(&self) -> usize {
        self.cfg.display_nfft
    }

    /// Acquisition blocks the analysis ring has DROPPED this run (drop-oldest overflow:
    /// the off-RT consumer fell behind). Per-run total, never reset.
    ///
    /// Readable without an analysis pass so a caller can attribute a drop to the window it
    /// happened in rather than to the window it was noticed in.
    pub fn blocks_dropped(&self) -> u64 {
        self.handles.as_ref().map(|h| h.ring.dropped()).unwrap_or(0)
    }

    // --- arm ---------------------------------------------------------------------

    /// Open, configure, ADOPT the actual rate, consult the arm gate, arm, prefill the
    /// device with a lead-in of the CURRENT drive, and start the clock.
    ///
    /// See the module docs for the 0.000 V arm posture and why there is no review hold.
    pub fn arm(&mut self) -> Result<(), ServiceError> {
        if self.state != RunState::Idle {
            return Err(ServiceError::BadState(format!("arm from {:?}", self.state)));
        }
        let mut backend = self.backend.take().ok_or_else(|| {
            ServiceError::BadState(
                "the station backend was consumed by an earlier arm attempt; rebuild the \
                 service to run again"
                    .into(),
            )
        })?;

        backend
            .open()
            .map_err(|e| ServiceError::Plant(format!("open: {e}")))?;
        let acq = hertzforge_hal::AcquisitionConfig::new(self.cfg.fs_hz, self.block_frames.max(1));
        let actual = backend
            .configure(&self.table, &acq)
            .map_err(|e| ServiceError::Plant(format!("configure: {e}")))?;
        self.configured = Some(actual);

        // ADOPT WHAT THE DEVICE PROGRAMMED, before anything is derived from the config.
        self.adopt_actual(&actual)?;

        // THE ARM GATE. Explorer requests `{HostSynthesized, None}` - the host
        // synthesizes every drive sample and nothing adapts it inside the run. The
        // operator is the loop, so there is no per-tick correction and therefore no
        // shared-clock precondition; what the gate still binds is that this backend HAS
        // a host drive path at all, and that a hardware station that drives an output
        // has an abort-latency bound.
        //
        // `Some(self.fs_hz)`, never `None`. `self.fs_hz` is the ADOPTED rate, so on a
        // station that reported a usable rate this comparison agrees by construction -
        // which is the point, not a vacuity: the adoption above is the regeneration a
        // `RateMismatch` would otherwise have asked the operator to perform by hand, and
        // passing the loop's own rate keeps the gate's statement TRUE rather than
        // absent. What it still catches is a backend that reports a rate nothing can be
        // denominated in (zero, negative, NaN): `rate_policy` refuses those outright,
        // and `None` would skip the check and arm into a station whose rate is a
        // fiction. That is the case the adoption deliberately does NOT claim - see
        // `adopt_actual` - so exactly one layer owns it.
        hertzforge_module_abi::arm_gate(
            &backend.caps(),
            self.station,
            hertzforge_module_abi::Resolution::host_open_loop(),
            Some(self.fs_hz),
            actual.actual_rate_hz,
        )
        .map_err(ServiceError::ArmRefused)?;

        // Build the step + analysis AT THE ADOPTED RATE.
        self.build_step_and_analysis(&actual, backend.as_ref())?;

        backend
            .arm()
            .map_err(|e| ServiceError::Plant(format!("arm: {e}")))?;

        // THE LEAD-IN. Load the device with the drive the step generates RIGHT NOW,
        // before the clock starts, so the first clocked sample is drive rather than a
        // substituted silence.
        //
        // At arm the commanded amplitude is 0.000 V, so the lead-in IS zeros - but they
        // are zeros BY VALUE, generated by the same synthesis path as every later
        // sample. That distinction is the whole point: the honest statement is "the
        // commanded drive is zero", not "we wrote silence because we had nothing yet".
        let lookahead = backend
            .caps()
            .output_buffer
            .map(|c| c.min_lookahead_frames as usize)
            .unwrap_or(4)
            .max(1);
        let out_ch = self.out_channels();
        let mut leadin = Vec::with_capacity(2 * lookahead * out_ch);
        if let Some(step) = self.step.as_mut() {
            use hertzforge_engine::ProcessStep;
            step.process(&[], 2 * lookahead, &mut leadin);
        }
        let mut written = 0usize;
        while written < lookahead {
            match backend.write_drive(&leadin[written * out_ch..lookahead * out_ch]) {
                Ok(0) | Err(_) => break,
                Ok(n) => written += n,
            }
        }
        if let Some(engine) = self.engine.as_mut() {
            if written > 0 {
                // The engine's own lazy ZERO prefill is now redundant, and its delivery
                // queue's zero priming would be delivered right behind what we just
                // wrote. Suppress the first and replace the second with the REST of the
                // generated lead-in, so the wire carries one continuous synthesis
                // stream.
                engine.mark_output_prefilled();
                let reserve: Vec<Vec<f32>> = leadin[written * out_ch..]
                    .chunks(out_ch)
                    .map(|f| f.to_vec())
                    .collect();
                engine.adopt_output_queue(reserve);
            }
        }

        backend
            .start()
            .map_err(|e| ServiceError::Plant(format!("start: {e}")))?;

        self.backend = Some(backend);
        self.state = RunState::Armed;
        Ok(())
    }

    /// Let the pump run tick batches. The device is already clocked by `arm()`, so this
    /// is a state transition, not a hardware action - keep the window between them short
    /// (the AO is playing the zero lead-in in the meantime).
    pub fn start(&mut self) -> Result<(), ServiceError> {
        match self.state {
            RunState::Armed => {
                self.state = RunState::Running;
                Ok(())
            }
            other => Err(ServiceError::BadState(format!("start from {other:?}"))),
        }
    }

    /// ADOPT the rate the driver programmed into everything derived from it.
    ///
    /// The band is re-narrowed here rather than at the gate, because a rate the band no
    /// longer fits inside is a CONFIGURATION problem with an operator-readable fix
    /// (raise the rate, lower the band), not a loop-resolution refusal.
    ///
    /// **What this deliberately does NOT refuse:** a rate that is zero, negative or
    /// NaN. That case belongs to the arm gate's rate policy, and it is the one case in
    /// which passing `Some(rate)` there rather than `None` changes the outcome. Claiming
    /// it here too would make the gate's rate argument unfalsifiable, which is how a
    /// check quietly becomes decoration.
    fn adopt_actual(&mut self, actual: &ConfiguredActual) -> Result<(), ServiceError> {
        let rate = actual.actual_rate_hz;
        self.fs_hz = rate;
        if !(rate.is_finite() && rate > 0.0) {
            return Ok(()); // the arm gate owns this one.
        }
        let guard = 0.45 * rate;
        let (lo, hi) = self.cfg.band_hz;
        if lo >= guard {
            return Err(ServiceError::Config(format!(
                "the station programmed {rate} Hz, whose usable ceiling is {guard} Hz, and the \
                 band starts at {lo} Hz: no part of the requested band survives the rate the \
                 driver actually gave us"
            )));
        }
        self.band_hz = (lo, hi.min(guard));
        Ok(())
    }

    /// Drive channels in the output frame.
    fn out_channels(&self) -> usize {
        self.table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count()
            .max(1)
    }

    /// Build the process step, the engine and every analysis object AT THE ADOPTED RATE.
    ///
    /// `backend` is passed in rather than read off `self`: `arm()` holds it in a local
    /// for the duration (so the rest of `self` stays borrowable and a failure part-way
    /// through drops it, which is what makes a second arm attempt say so instead of
    /// silently building a second station).
    fn build_step_and_analysis(
        &mut self,
        actual: &ConfiguredActual,
        backend: &dyn Backend,
    ) -> Result<(), ServiceError> {
        let fs = self.fs_hz;
        let out_ch = self.out_channels();
        let block = actual.block_frames.max(1);

        let (step, handles) = ExplorerProcessStep::new(StepBuild {
            fs_hz: fs,
            out_channels: out_ch,
            band_hz: self.band_hz,
            start_hz: self.cfg.start_hz,
            max_drive_vpk: self.cfg.max_drive_vpk,
            slew: self.cfg.slew,
            ramp: self.cfg.ramp,
            ring_blocks: self.cfg.ring_blocks,
            block_frames: block,
            n_interleave: self.n_interleave,
        })
        .map_err(|e| ServiceError::Config(e.to_string()))?;
        // The step narrowed the band again against the rate it was handed; take ITS
        // answer so the service and the step can never disagree about what a clamp does.
        self.band_hz = step.band_hz();
        self.step = Some(step);
        self.handles = Some(handles);

        let mut ecfg = EngineConfig::with_block(self.n_interleave, out_ch, actual);
        ecfg.watchdog_sustain = if self.station.is_hardware() {
            hertzforge_engine::watchdog_sustain_frames(WATCHDOG_SUSTAIN_SECONDS, fs)
        } else {
            SIM_WATCHDOG_SUSTAIN_FRAMES
        };
        ecfg.jitter_frac = self.jitter_frac;
        ecfg.compute_budget = self.compute_budget;
        let sustain = ecfg.watchdog_sustain;
        self.engine = Some(Engine::new(ecfg, &self.table, backend));

        self.spectra = Some(SpectrumEngine::new(
            &self.input_indices,
            fs,
            self.cfg.display_nfft,
        ));
        self.derived = Some(DerivedTracer::new(fs, DERIVED_HP_HZ, SETTLE_HP_CONSTANTS));
        // The profile tracer's tracking-filter bandwidth and decimation shape its
        // per-sample / chunked entry points, neither of which this service calls: it uses
        // `ProfileTracer::observe`, which measures the level over a short window of the
        // freshest signal. What DOES matter here is the acquisition block, because half
        // the post-retune transient guard is expressed in blocks.
        let profile_bw = (self.cfg.slew.freq_oct_per_s * self.band_hz.0).max(5.0);
        self.profile = Some(
            ProfileTracer::new(
                self.input_indices[self.control_slot.min(self.input_indices.len() - 1)],
                fs,
                profile_bw,
                1,
            )
            .with_block_frames(block),
        );
        // The amplitude gate stays CLOSED until the floor probe matures - a measured
        // frequency taken off the noise floor is a number with no referent.
        self.freq_meter = Some(WindowedFreqMeasure::new(
            fs,
            WindowedFreqMeasure::DEFAULT_WINDOW,
        ));
        self.floor = Some(NoiseFloorProbe::new(self.n_interleave));
        self.monitor = Some(
            AbortMonitor::new(
                &self.table,
                self.n_interleave,
                fs,
                DERIVED_HP_HZ,
                // The SAME debounce the engine's own watchdog was configured with, read
                // off that config rather than re-derived, so the authoritative monitor
                // and the secondary one cannot disagree about what a transient is.
                sustain,
                SETTLE_HP_CONSTANTS,
            )
            .map_err(|e: AbortMonitorError| ServiceError::Config(e.to_string()))?,
        );
        Ok(())
    }

    // --- operator intents ---------------------------------------------------------

    /// Command an absolute frequency. Returns the CLAMPED value actually adopted.
    pub fn set_frequency(&mut self, hz: f64) -> Result<f64, ServiceError> {
        let step = self.live_step()?;
        Ok(step.set_target_freq(hz))
    }

    /// Move the frequency by `delta_hz` from the current target. Returns the clamped
    /// value actually adopted.
    pub fn adjust_frequency(&mut self, delta_hz: f64) -> Result<f64, ServiceError> {
        let step = self.live_step()?;
        let next = step.target_freq_hz() + delta_hz;
        Ok(step.set_target_freq(next))
    }

    /// Command a drive amplitude in volts peak. Returns the clamped value adopted.
    ///
    /// Any change away from zero CONTAMINATES the noise-floor probe if it has not
    /// matured: the window it was accumulating no longer means "no drive".
    pub fn set_amplitude_vpk(&mut self, vpk: f64) -> Result<f64, ServiceError> {
        let step = self.live_step()?;
        let before = step.target_drive_vpk();
        let adopted = step.set_target_vpk(vpk);
        if (adopted - before).abs() > f64::EPSILON {
            if let Some(f) = self.floor.as_mut() {
                f.contaminate();
            }
        }
        Ok(adopted)
    }

    /// Start (or re-aim) a sweep between the band edges. Halt-and-dwell at the edge.
    pub fn start_sweep(
        &mut self,
        direction: SweepDirection,
        rate_oct_per_min: f64,
    ) -> Result<(), ServiceError> {
        let step = self.live_step()?;
        step.start_sweep(direction, rate_oct_per_min);
        Ok(())
    }

    /// Stop the sweep. The tone holds where it is.
    pub fn stop_sweep(&mut self) -> Result<(), ServiceError> {
        let step = self.live_step()?;
        step.stop_sweep();
        Ok(())
    }

    /// Ask for a RAMPED stop: the drive travels to zero at the operator drive slew, then
    /// the backend is stopped. Idempotent, and legal from `Armed` as well as `Running`.
    pub fn request_stop(&mut self) {
        if self.state.is_terminal() || self.state == RunState::Stopping {
            return;
        }
        if let Some(step) = self.step.as_mut() {
            step.stop_sweep();
            step.set_target_vpk(0.0);
        }
        self.state = RunState::Stopping;
    }

    /// ABORT: `emergency_stop` immediately, with no ramp.
    ///
    /// The absence of a ramp here is the point. A ramp is the right answer when the run
    /// is ENDING and the only question is how gracefully; an abort is the answer when
    /// something is already wrong, and the backend's own taper (the HAL contract's
    /// `RampDownThenAbort`) is the backstop that keeps `emergency_stop` from being a
    /// cliff. Trading abort latency for smoothness at the host layer is exactly the
    /// tradeoff that belongs to the operator per rig, not to this code.
    pub fn abort(&mut self, reason: &str) -> Result<(), ServiceError> {
        if self.state.is_terminal() {
            return Ok(());
        }
        let r = self.emergency_stop_now();
        self.finish(RunState::Aborted, format!("aborted: {reason}"), None);
        r
    }

    fn live_step(&mut self) -> Result<&mut ExplorerProcessStep, ServiceError> {
        if self.state.is_terminal() {
            return Err(ServiceError::BadState(format!(
                "the run has ended ({:?}); intents are no longer accepted",
                self.state
            )));
        }
        self.step
            .as_mut()
            .ok_or_else(|| ServiceError::BadState("not armed".into()))
    }

    fn emergency_stop_now(&mut self) -> Result<(), ServiceError> {
        match self.backend.as_mut() {
            Some(b) => b
                .emergency_stop()
                .map_err(|e| ServiceError::Plant(format!("emergency_stop: {e}"))),
            None => Ok(()),
        }
    }

    fn finish(&mut self, state: RunState, reason: String, trip: Option<AbortTrip>) {
        self.state = state;
        let record = EndRecord {
            state,
            reason,
            trip,
        };
        self.end = Some(record.clone());
        if state == RunState::Aborted {
            self.abort_latch.set(record);
        }
    }

    // --- the pump -----------------------------------------------------------------

    /// Run up to `batches` BOUNDED tick batches, servicing the mailbox and the stop /
    /// abort flags between them. Returns `true` while the run may continue.
    ///
    /// This is called BY the host's pump loop - it never spawns a thread. The caller
    /// therefore owns the interleaving of [`Self::service_analysis`] with these batches,
    /// which is what keeps a 24 ms FFT off a 15 ms tick without any thread handshake at
    /// all.
    ///
    /// # The `catch_unwind`
    ///
    /// The whole batch body runs inside `catch_unwind`, and a caught panic issues
    /// `emergency_stop` before propagating as [`ServiceError::PumpPanicked`]. Without
    /// it, a panic anywhere under `run_ticks` unwinds past every stop path in this file
    /// while the AO task keeps clocking whatever sample it last had - a drive frozen at
    /// its last value is the one failure mode a stopped host cannot correct. The panic
    /// itself is not swallowed: it is reported, the run is terminal, and there is no
    /// resume path.
    pub fn run_batches(&mut self, batches: u32) -> Result<bool, ServiceError> {
        if self.state.is_terminal() {
            return Ok(false);
        }
        if !matches!(self.state, RunState::Running | RunState::Stopping) {
            return Err(ServiceError::BadState(format!(
                "run_batches from {:?} (arm() then start() first)",
                self.state
            )));
        }
        for _ in 0..batches {
            self.apply_commands();
            if self.state.is_terminal() {
                return Ok(false);
            }
            // The RAMPED stop completes here, not in the tick: the drive is at zero, so
            // stopping the tasks can no longer put a step into anything.
            if self.state == RunState::Stopping && self.drive_is_zero() {
                if let Some(b) = self.backend.as_mut() {
                    b.stop()
                        .map_err(|e| ServiceError::Plant(format!("stop: {e}")))?;
                }
                self.finish(
                    RunState::Stopped,
                    "operator stop: drive ramped to zero, tasks stopped".into(),
                    None,
                );
                return Ok(false);
            }
            if !self.run_one_batch()? {
                return Ok(false);
            }
        }
        Ok(!self.state.is_terminal())
    }

    fn drive_is_zero(&self) -> bool {
        self.step.as_ref().map(|s| s.drive_vpk()).unwrap_or(0.0) == 0.0
    }

    /// Drain the mailbox and apply every intent. Between batches, never on the tick.
    fn apply_commands(&mut self) {
        for cmd in self.mailbox.drain() {
            match cmd {
                ExplorerCommand::SetFrequency(hz) => {
                    let _ = self.set_frequency(hz);
                }
                ExplorerCommand::AdjustFrequency(d) => {
                    let _ = self.adjust_frequency(d);
                }
                ExplorerCommand::SetAmplitudeVpk(v) => {
                    let _ = self.set_amplitude_vpk(v);
                }
                ExplorerCommand::StartSweep {
                    direction,
                    rate_oct_per_min,
                } => {
                    let _ = self.start_sweep(direction, rate_oct_per_min);
                }
                ExplorerCommand::StopSweep => {
                    let _ = self.stop_sweep();
                }
                ExplorerCommand::RequestStop => self.request_stop(),
            }
        }
    }

    /// One bounded batch of `run_ticks`, under `catch_unwind`.
    fn run_one_batch(&mut self) -> Result<bool, ServiceError> {
        let station = self.station;
        let (Some(backend), Some(engine), Some(step)) = (
            self.backend.as_mut(),
            self.engine.as_mut(),
            self.step.as_mut(),
        ) else {
            return Err(ServiceError::BadState("not armed".into()));
        };
        let mut sink = VecCaptureSink::new();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.run_ticks(backend.as_mut(), step, &mut sink, TICKS_PER_BATCH)
        }));
        let report = match caught {
            Ok(r) => r,
            Err(payload) => {
                let what = panic_message(&payload);
                let _ = self.emergency_stop_now();
                self.finish(
                    RunState::Aborted,
                    format!("pump panicked: {what}; emergency_stop issued"),
                    None,
                );
                return Err(ServiceError::PumpPanicked(what));
            }
        };

        // DRAIN the driver's own diagnostics every batch. `take_warnings` empties the
        // backend's buffer, so a caller that does not pull them is not "leaving them for
        // later" - it is discarding them. Drained here, right after the batch that produced
        // them, and before any of the fault arms below can return early: a batch that
        // FAULTED is exactly the one whose driver warnings matter most.
        if let Some(b) = self.backend.as_mut() {
            let fresh = b.take_warnings();
            if !fresh.is_empty() {
                self.driver_warnings.extend(fresh);
                let excess = self
                    .driver_warnings
                    .len()
                    .saturating_sub(MAX_DRIVER_WARNINGS);
                if excess > 0 {
                    self.driver_warnings.drain(..excess);
                }
            }
        }

        match report.reason {
            // The batch budget ran out: the ONLY healthy exit from a bounded batch on a
            // continuous acquisition. `InputEnded` here means `max_ticks`, not a stream
            // that finished.
            StopReason::InputEnded => Ok(true),
            StopReason::AbortTripped(trip) => {
                // The engine's watchdog already called `emergency_stop`.
                self.finish(
                    RunState::Aborted,
                    format!(
                        "abort watchdog: channel {} {:?} {:.4} past limit {:.4}",
                        trip.channel_index, trip.axis, trip.value, trip.limit
                    ),
                    Some(trip),
                );
                Ok(false)
            }
            StopReason::SchedulingFault(status) => {
                // No retry, on any station. A retry re-runs against a plant that has
                // already advanced, and on hardware the engine has issued
                // `emergency_stop` while a shaker was moving - re-arming and re-driving
                // is the opposite of the correct response.
                self.finish(
                    RunState::Aborted,
                    format!(
                        "scheduling fault ({status:?}) on station {station}, {} frame(s) \
                         acquired; the engine already called emergency_stop",
                        report.frames_in
                    ),
                    None,
                );
                Err(ServiceError::Plant(format!(
                    "scheduling fault ({status:?}) on station {station}"
                )))
            }
            StopReason::InputStalled { elapsed } => {
                self.finish(
                    RunState::Aborted,
                    format!("input stalled: no frames for {elapsed:?}"),
                    None,
                );
                Err(ServiceError::Plant(format!(
                    "input stalled: no frames for {elapsed:?}"
                )))
            }
            StopReason::Disconnected(m) => {
                self.finish(RunState::Aborted, format!("device link lost: {m}"), None);
                Err(ServiceError::Plant(format!(
                    "device link lost mid-run: {m}"
                )))
            }
            StopReason::DriveUnderrun(u) => {
                let _ = self.emergency_stop_now();
                self.finish(RunState::Aborted, format!("drive underrun: {u:?}"), None);
                Err(ServiceError::Plant(format!("drive underrun abort: {u:?}")))
            }
            StopReason::RingOverrun { frames } => {
                self.finish(
                    RunState::Aborted,
                    format!("raw ring overran by {frames} frame(s) on a limits-armed run"),
                    None,
                );
                Err(ServiceError::Plant(format!(
                    "raw ring overrun: {frames} frame(s) the abort watchdog never examined"
                )))
            }
            StopReason::BackendError(m) => {
                let _ = self.emergency_stop_now();
                self.finish(RunState::Aborted, format!("backend error: {m}"), None);
                Err(ServiceError::Plant(format!("engine backend error: {m}")))
            }
            StopReason::CaptureFatal(m) => {
                let _ = self.emergency_stop_now();
                self.finish(RunState::Aborted, format!("capture fatal: {m}"), None);
                Err(ServiceError::Plant(format!("capture fatal: {m}")))
            }
        }
    }

    // --- the off-RT analysis pass -------------------------------------------------

    /// Drain the acquisition ring into the analysis engines and publish a status
    /// snapshot. Call between tick batches; never from inside one.
    ///
    /// Everything expensive lives here: the display FFT, the derived integrators, the
    /// resonance profile, the measured-frequency window, the noise-floor probe and the
    /// secondary limit monitor. All of it is denominated in the ADOPTED rate.
    pub fn service_analysis(&mut self) -> ExplorerStatus {
        self.service_analysis_tapped(|_, _| {})
    }

    /// [`Self::service_analysis`] with a TIME-DOMAIN TAP: `on_block` is handed every
    /// drained acquisition block, in acquisition order, as `(interleaved samples, stride)`
    /// - the same bytes the analysis engines see, before they are recycled.
    ///
    /// # Why a callback and not a return value
    ///
    /// The obvious signature hands the blocks back. It cannot: those `Vec<f32>`s are the
    /// ring's RECYCLED buffers (`ring.rs` property 2), handed back to the producer at the
    /// end of this pass so the RT tick never reaches the allocator. A caller that took
    /// ownership would starve the pool and put an allocation on the drive clock - the one
    /// place in this crate that must not allocate. Borrowing them for the length of the
    /// call gives the caller the same data with none of that.
    ///
    /// The tap fires per BLOCK rather than per pass, so a consumer sees the samples at the
    /// cadence they were acquired at and can decide its own publish cadence. It runs
    /// inside the pass, which runs between tick batches - never on the RT tick.
    pub fn service_analysis_tapped<F>(&mut self, mut on_block: F) -> ExplorerStatus
    where
        F: FnMut(&[f32], usize),
    {
        let mut drained = std::mem::take(&mut self.drained);
        drained.clear();
        if let Some(h) = self.handles.as_ref() {
            h.ring.drain_into(&mut drained);
            // The ring drops the OLDEST block when it is full, so whatever it dropped since
            // the last pass sits between that pass's blocks and this pass's first one; the
            // blocks drained here are contiguous among themselves. See the measured window
            // below for why it has to know.
            let dropped = h.ring.dropped();
            if dropped != self.ring_dropped_seen {
                self.ring_dropped_seen = dropped;
                if let Some(fm) = self.freq_meter.as_mut() {
                    fm.break_continuity();
                }
            }
        }

        let stride = self.n_interleave.max(1);
        let control = self.control_slot.min(stride - 1);
        let mut sumsq = vec![0.0f64; stride];
        let mut frames_total = 0usize;
        let mut spectra_out = Vec::new();

        // The tone the profile tracer attributes this block's amplitude to. Read BEFORE
        // the loop: it is the frequency the step was generating while these blocks were
        // acquired, not whatever it has slewed to since.
        let inst_freq = self.step.as_ref().map(|s| s.freq_hz()).unwrap_or(0.0);
        // Whether the host is COMMANDING a tone right now. See the measured-frequency
        // gate below for why this is the load-bearing half of that decision.
        let driving = self.step.as_ref().map(|s| s.drive_vpk()).unwrap_or(0.0) > 0.0;
        // Whether the tone is STANDING STILL: at its target, with no sweep advancing it,
        // and at the same frequency it was at the previous pass.
        //
        // A zero-crossing count is a measurement of ONE tone over the whole window. Over
        // a window the tone SWEPT across, it is the average of everything in it and
        // belongs to no frequency at all - which is not merely imprecise, it is a number
        // the panel would print beside the commanded frequency as its cross-check.
        // MEASURED, 2026-08-08: without this gate a retune from 100 Hz to 400 Hz read
        // back as 232 Hz - the mean of the traversal, indistinguishable on the panel
        // from a device that had failed to follow the command.
        let steady_now = self
            .step
            .as_ref()
            .map(|s| {
                (s.freq_hz() - s.target_freq_hz()).abs() <= f64::EPSILON
                    && s.sweep().map(|w| w.halted).unwrap_or(true)
            })
            .unwrap_or(false);
        // Is the tone under a COMMANDED continuous travel? The profile capture needs this
        // to tell a SWEEP (which moves every pass by construction, and whose motion must
        // not re-arm the transient guard) from a RETUNE (an operator click, or the slew
        // walking toward a new target, which must).
        let sweeping = self
            .step
            .as_ref()
            .and_then(|s| s.sweep())
            .map(|w| !w.halted)
            .unwrap_or(false);
        // THE PIPELINE, read at the same instant as `inst_freq` and BEFORE the drain, so
        // the frames-generated and frames-in it is derived from belong to the same moment
        // as the tone the pass is about to attribute samples to.
        let pipeline = self.pipeline_frames();
        let tone_held = steady_now && (inst_freq - self.last_tone_hz).abs() <= f64::EPSILON;
        self.last_tone_hz = inst_freq;
        // Both halves must hold for a sample to enter the window: a tone that is not
        // being driven has nothing to measure, and a tone that is moving has no single
        // frequency to measure.
        let measurable = driving && tone_held;
        // THE PIPELINE, applied to the measured window - the half the gate above cannot do
        // on its own, because it reads the GENERATOR and the samples are the INPUT.
        // MEASURED, 2026-11-01 (the NI chassis gate): a retune 100 -> 400 Hz read back
        // 379.41 Hz, 5.15 % low, while a meter on the jumper read the tone at 400 Hz. A
        // manual retune lands in ONE generated frame and `measurable` held from the second
        // pass after it, but that pass's samples are the response to drive written up to
        // `pipeline_frames` earlier. On the chassis the pipeline is 551 frames and a pass
        // 256, so the window opened 295 frames before the new tone arrived: 295 frames of
        // 100 Hz in its 4352 left 387 crossings where 408 were due, 379.41 Hz (reproduced
        // to the hundredth in `tests/pipeline_alignment.rs`). The same arithmetic read a
        // level raised off 0 V at 100 Hz as 88 Hz: the zeros still in flight.
        //
        // The alignment is `ProfileTracer::observe_pipelined`'s, anchored at the OTHER
        // end. A change the generator made during the batch that ended at this pass
        // reaches the input between `pipeline` frames from the START of this pass's
        // samples and `pipeline` frames past their END. The profile anchors at the near
        // end and lets its post-arrival settle guard absorb the rest; this window has no
        // such guard, and a crossing count is a MEAN over everything in it, so it anchors
        // at the far end: every unmeasurable pass re-arms the skip to the full pipeline,
        // counted from the first frame AFTER this pass's samples (which an unmeasurable
        // pass never pushes). An operator intent is applied between batches and lands on
        // the first frame of the next one, so for a level raised off 0 V (the last
        // unmeasurable pass is the one before the command) the skip ends on exactly the
        // first lifted sample, and for a retune (unmeasurable until the tone has held for
        // a pass) it is conservative by one pass, ~30 ms on a 0.5 s window.
        if !measurable {
            self.meter_skip = pipeline;
        }

        let mut scratch = std::mem::take(&mut self.scratch_f64);
        let mut control_buf = std::mem::take(&mut self.control_scratch);
        // The profile's run for this pass: EVERY control sample the pass drained, at the
        // ONE frequency `inst_freq` names. Accumulated rather than pushed per block
        // because `inst_freq` is read once for the whole pass, so a per-block push would
        // claim a frequency for samples that were not measured against it.
        let mut profile_run = std::mem::take(&mut self.profile_scratch);
        profile_run.clear();
        for block in drained.iter() {
            let frames = block.len() / stride;
            if frames == 0 {
                continue;
            }
            frames_total += frames;

            // THE TIME-DOMAIN TAP, first - the same ordering rule the acquisition tap in
            // `ExplorerProcessStep::process` follows: a panic anywhere below still leaves
            // the display with the data that arrived.
            on_block(block, stride);

            // Fail-closed floor + the secondary limit monitor, both on the RAW f32.
            if let Some(f) = self.floor.as_mut() {
                f.push(block);
            }
            if let Some(m) = self.monitor.as_mut() {
                if let Some(trip) = m.feed(block) {
                    if self.observed_trip.is_none() {
                        self.observed_trip = Some(trip);
                    }
                }
            }

            scratch.clear();
            scratch.extend(block.iter().map(|&v| f64::from(v)));
            control_buf.clear();
            control_buf.extend(scratch.iter().skip(control).step_by(stride).copied());

            for (slot, acc) in sumsq.iter_mut().enumerate() {
                for f in 0..frames {
                    let v = scratch[f * stride + slot];
                    *acc += v * v;
                }
            }
            if let Some(sp) = self.spectra.as_mut() {
                let frames = sp.push_interleaved(&scratch, SpectrumKind::Accel);
                if !frames.is_empty() {
                    spectra_out = frames;
                }
            }
            if let Some(d) = self.derived.as_mut() {
                for &v in control_buf.iter() {
                    d.push(v);
                }
            }
            profile_run.extend_from_slice(&control_buf);
            if measurable {
                // CONTIGUOUS, not per chunk: these blocks are one unbroken acquisition
                // (a ring drop is the only gap, and it breaks the stream above), so the
                // interval between two blocks is a sample interval like any other and its
                // crossing must be counted. Fed through `push_chunk`, every block boundary
                // was time the window counted and a crossing it never tested - MEASURED
                // 2026-11-01 at the chassis's 8533.33 Hz / 128-frame block: a steady
                // 1066.67 Hz read 1033.33 Hz, every window, and 777 Hz read -0.83 %
                // (`tests/sim_end_to_end.rs`).
                //
                // The pipeline skip first. `control_buf` is ONE channel (de-interleaved
                // above), so its length is this block's frame count and the skip is spent
                // in frames, the unit `pipeline_frames` is denominated in. The window was
                // reset by the unmeasurable pass that armed the skip, so the first sample
                // past it starts a fresh stream - no carried sample, and no time added for
                // the interval before it, which was never tested.
                let skip = self.meter_skip.min(control_buf.len());
                self.meter_skip -= skip;
                if let Some(fm) = self.freq_meter.as_mut() {
                    fm.push_contiguous(&control_buf[skip..]);
                }
            }
        }
        // THE PROFILE LEVEL for this pass, aligned to the PIPELINE.
        //
        // `inst_freq` is the tone the step is generating right now, and these samples are
        // NOT its response: the drive is written ahead of the clock by the output
        // lookahead contract, so a retune commanded this pass is still `pipeline_frames`
        // of input away from being what the acquisition is carrying. `observe_pipelined`
        // queues the retune at its ARRIVAL and finalizes the outgoing tone's point on
        // DEPARTURE, so a dwell is measured over its whole length however soon the next
        // click comes. See `Self::pipeline_frames` for the arithmetic and
        // `ProfileTracer::observe_pipelined` for the capture rule.
        if let Some(p) = self.profile.as_mut() {
            p.observe_pipelined(&profile_run, inst_freq, pipeline, sweeping);
        }
        self.scratch_f64 = scratch;
        self.control_scratch = control_buf;
        self.profile_scratch = profile_run;

        // THE MEASURED-FREQUENCY GATE, in two halves.
        //
        // 1. **Are we driving at all?** The delegated path could not answer this - its drive
        //    was an open-loop nudge-count estimate, so the only defence against measuring
        //    the noise floor was an amplitude threshold, and a threshold needs a floor to
        //    be measured against. THIS path commands volts natively, so it KNOWS. While
        //    the commanded drive is zero there is nothing to measure and the window is
        //    reset rather than accumulated: a crossing count taken off the floor is a
        //    number with no referent (2026-07-16, a frozen-near-zero drive walked the
        //    displayed estimate 45.9 -> 44.4 -> 42.0 while the operator touched nothing).
        //    This is a STRONGER gate than the amplitude one, not a replacement for it,
        //    because it cannot be defeated by a loud noise floor.
        // 2. **Is the signal above the floor?** Applied ON TOP, as soon as the floor
        //    probe has a MEASURED value - never a guessed one. Three times the floor:
        //    comfortably above it, far below any tone worth measuring. If the probe never
        //    matures (it is contaminated by the operator's first drive change, by
        //    design) the first half still stands on its own.
        if !measurable {
            if let Some(fm) = self.freq_meter.as_mut() {
                fm.reset();
            }
        }
        if let (Some(fm), Some(fl)) = (self.freq_meter.as_mut(), self.floor.as_ref()) {
            if let FloorProbe::Measured(rms) = fl.floor(control) {
                fm.set_min_rms(3.0 * rms);
            } else {
                // No measured floor: the commanded-drive gate above is the whole gate,
                // so the amplitude one must not stay CLOSED (its constructor default) or
                // no measurement could ever mature on a bench whose floor probe was
                // contaminated - which is every bench where the operator touched the
                // level inside the first quarter second.
                fm.set_min_rms(0.0);
            }
        }
        let measured_freq_hz = if measurable {
            self.freq_meter.as_mut().and_then(|fm| fm.take())
        } else {
            None
        };

        if let Some(h) = self.handles.as_ref() {
            h.ring.recycle(&mut drained);
        }
        drained.clear();
        self.drained = drained;
        self.frames_analyzed += frames_total as u64;
        if !spectra_out.is_empty() {
            self.latest_spectra = spectra_out;
        }

        // A secondary-monitor trip is an OBSERVATION (see the module docs): surface it
        // and ask for a RAMPED stop, never an abort.
        if self.observed_trip.is_some() && matches!(self.state, RunState::Running | RunState::Armed)
        {
            self.request_stop();
        }

        let per_channel_rms: Vec<f64> = if frames_total > 0 {
            sumsq
                .iter()
                .map(|s| (s / frames_total as f64).max(0.0).sqrt())
                .collect()
        } else {
            Vec::new()
        };

        let status = self.snapshot(per_channel_rms, measured_freq_hz);
        self.status_cell.store(status.clone());
        status
    }

    fn snapshot(&self, per_channel_rms: Vec<f64>, measured_freq_hz: Option<f64>) -> ExplorerStatus {
        let health = self.engine.as_ref().map(|e| e.health());
        let floor: Vec<FloorProbe> = match self.floor.as_ref() {
            Some(f) => (0..self.n_interleave).map(|s| f.floor(s)).collect(),
            None => Vec::new(),
        };
        ExplorerStatus {
            state: self.state,
            station: self.station,
            actual_fs_hz: self.fs_hz,
            requested_fs_hz: self.cfg.fs_hz,
            band_hz: self.band_hz,
            commanded_freq_hz: self.step.as_ref().map(|s| s.freq_hz()).unwrap_or(0.0),
            target_freq_hz: self
                .step
                .as_ref()
                .map(|s| s.target_freq_hz())
                .unwrap_or(0.0),
            measured_freq_hz,
            commanded_vpk: self.step.as_ref().map(|s| s.drive_vpk()).unwrap_or(0.0),
            target_vpk: self
                .step
                .as_ref()
                .map(|s| s.target_drive_vpk())
                .unwrap_or(0.0),
            max_drive_vpk: self.cfg.max_drive_vpk,
            sweep: self.step.as_ref().and_then(|s| s.sweep()),
            per_channel_rms,
            floor,
            floor_matured: self.floor.as_ref().map(|f| f.matured()).unwrap_or(false),
            derived: self
                .derived
                .as_ref()
                .map(|d| d.last())
                .unwrap_or((0.0, 0.0)),
            derived_settling: self.derived.as_ref().map(|d| d.settling()).unwrap_or(false),
            frames_generated: self
                .step
                .as_ref()
                .map(|s| s.frames_generated())
                .unwrap_or(0),
            frames_analyzed: self.frames_analyzed,
            pipeline_frames: self.pipeline_frames(),
            blocks_dropped: self.handles.as_ref().map(|h| h.ring.dropped()).unwrap_or(0),
            engine_ticks: health.map(|h| h.ticks).unwrap_or(0),
            worst_tick_compute: health.map(|h| h.worst_compute).unwrap_or(Duration::ZERO),
            compute_budget: health.map(|h| h.compute_budget).unwrap_or(Duration::ZERO),
            health_faults: health.map(|h| h.health_faults).unwrap_or(0),
            ring_overruns: health.map(|h| h.ring_overruns).unwrap_or(0),
            observed_trip: self.observed_trip,
            driver_warnings: self.driver_warnings.clone(),
            end: self.end.clone(),
        }
    }

    /// The acquisition ring, for a caller that wants the raw tap (a capture sink, a
    /// probe). The service drains it in [`Self::service_analysis`]; a second consumer
    /// competes with that one.
    pub fn ring(&self) -> Option<Arc<SampleRing>> {
        self.handles.as_ref().map(|h| h.ring.clone())
    }
}

/// Best-effort text out of a caught panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_refuses_a_band_the_rate_cannot_carry() {
        let cfg = ExplorerHostConfig {
            fs_hz: 1_000.0,
            band_hz: (10.0, 900.0),
            ..Default::default()
        };
        let err = cfg.validate().expect_err("900 Hz needs > 2 kHz");
        assert!(format!("{err}").contains("0.45"), "{err}");
    }

    #[test]
    fn config_refuses_a_non_power_of_two_display_nfft() {
        let cfg = ExplorerHostConfig {
            display_nfft: 3_000,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_refuses_a_start_outside_the_band() {
        let cfg = ExplorerHostConfig {
            band_hz: (100.0, 200.0),
            start_hz: 20.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn the_default_config_is_valid() {
        ExplorerHostConfig::default().validate().unwrap();
    }

    #[test]
    fn a_terminal_state_is_terminal() {
        assert!(RunState::Stopped.is_terminal());
        assert!(RunState::Aborted.is_terminal());
        assert!(!RunState::Stopping.is_terminal());
        assert!(!RunState::Running.is_terminal());
    }

    #[test]
    fn the_mailbox_drains_in_order_and_empties() {
        let m = CommandMailbox::default();
        m.post(ExplorerCommand::SetFrequency(100.0));
        m.post(ExplorerCommand::RequestStop);
        let drained = m.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], ExplorerCommand::SetFrequency(100.0));
        assert!(m.drain().is_empty());
    }

    /// A panel that has never been told anything must SAY so, not render zeros.
    #[test]
    fn an_unpublished_status_cell_reads_none() {
        assert!(StatusCell::default().read().is_none());
    }
}
