//! The run host around [`ExplorerHostService`]: the pump thread, the asynchronous arm,
//! the lock-free status snapshot, the stall watchdog and the abort tail.
//!
//! Every front end that runs an Explorer in-process (the headless daemon, the desktop
//! app) needs the same machinery around the service, and each used to be one copy of it
//! away from drifting. A front end now supplies two things: the service, built on its
//! own station, and a [`HostSink`] for what the pump produces (display frames and journal
//! lines). Validation of its own payload, wording and wire vocabulary stay with it.
//!
//! # The shape
//!
//! * [`RunHost::install`] takes a built, idle service. Nothing is opened.
//! * [`RunHost::start`] refuses the cheap cases synchronously and moves the ARM to the
//!   pump thread behind a priming flag: `arm()` opens the station, configures it, adopts
//!   the programmed rate, consults the arm gate, builds the step and the analysis, writes
//!   the drive lead-in and starts the clock, and on an NI chassis that is seconds.
//! * The pump runs ONE bounded tick batch per iteration, then the off-RT analysis pass,
//!   and hands the drained blocks, the ring-drop holes and each new spectrum window to the
//!   sink while the service mutex is held; the sink sends after it is released.
//! * [`RunHost::snapshot`] never takes the service mutex: the service publishes into its
//!   own [`StatusCell`], and a driver stalled inside a read cannot wedge a status poll.
//!
//! # Failures outside the service's state machine
//!
//! A refused arm leaves the service at `Idle` with its backend consumed, and a panicking
//! pump unwinds past every stop path. Both are latched in a fault cell the snapshot reads
//! FIRST, so neither can be reported as "configured". A panic also aborts the service
//! (`emergency_stop`) before the thread exits.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eigen_channel_table::{ChannelRole, ChannelTable};

use crate::analysis::SpectrumFrame;
use crate::profile_raster::ProfileRaster;
use crate::service::{
    EndRecord, ExplorerHostService, ExplorerStatus, RunState, ServiceError, StatusCell,
    TICKS_PER_BATCH,
};
use crate::step::SweepDirection;

/// Tick batches per pump iteration.
///
/// One, so the loop's outer cadence is the service's own bounded batch and the mailbox /
/// stop checks between batches are not deferred behind a second batch's worth of ticks.
const BATCHES_PER_ITER: u32 = 1;

/// Pump iterations before the loop bails: the last-resort backstop. One iteration is one
/// bounded batch (~30 ms on the chassis at the shipped block), so this is ~33 hours: past
/// any survey session, and still finite.
const MAX_PUMP_ITERS: u64 = 4_000_000;

/// The display cadence, stated in TIME.
///
/// 40 ms = 25 spectrum windows per second at most, decimated producer-side: the pump
/// hands the sink at most one window per period, whatever the block period is. Stating it
/// in time rather than in blocks keeps the display load the same when the operator
/// changes the rate or the block size.
const DISPLAY_PERIOD: Duration = Duration::from_millis(40);

/// The pump's release window when nothing is parked on the service mutex.
///
/// The floor: std's mutex is unfair and a loop that re-acquires microseconds after
/// releasing starves every parked waiter for the length of the run.
const RELEASE_FLOOR: Duration = Duration::from_millis(1);

/// The release window when a verb IS parked. The pump's hold is short (one bounded batch
/// plus the analysis pass) and its re-acquire is immediate, so a waiter gets few chances
/// and each one is brief. Widening the window when, and only when, somebody is waiting
/// costs nothing on an idle console and hands the mutex over promptly when an operator
/// moves a knob.
const WAITER_RELEASE: Duration = Duration::from_millis(8);

/// The longest a wall-clock-paced pump sleeps in one go, so a stop, an abort or a parked
/// verb is never left waiting behind a long catch-up pause.
const MAX_PACE_PAUSE: Duration = Duration::from_millis(50);

/// What holds the pump to the signal's own time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pacing {
    /// The station's clock: a hardware read waits for the device's block, so the pump
    /// turns at the acquisition rate. A software plant under this pacing runs as fast as
    /// the pump turns.
    #[default]
    Station,
    /// The wall clock: the pump holds the frames it has generated to real time, so a
    /// software plant's tone, ramps and sweeps take the time they are stated in.
    WallClock,
}

/// How long a wall-clock-paced pump waits after an iteration: until `frames` at `fs_hz`
/// is due, measured from the start of the run, never longer than [`MAX_PACE_PAUSE`].
/// Zero when the run is behind (it catches up at the pump's own speed) or the rate does
/// not exist.
pub fn pace_pause(since_start: Duration, frames: u64, fs_hz: f64) -> Duration {
    if !(fs_hz.is_finite() && fs_hz > 0.0) {
        return Duration::ZERO;
    }
    let due = Duration::from_secs_f64(frames as f64 / fs_hz);
    due.saturating_sub(since_start).min(MAX_PACE_PAUSE)
}

/// The expected pump-iteration period in ms: one iteration is
/// `BATCHES_PER_ITER x TICKS_PER_BATCH` blocks of acquisition. `0` for a rate that does
/// not exist, rather than a fabricated cadence.
pub fn iter_period_ms(fs_hz: f64, block_frames: usize) -> u64 {
    if !(fs_hz.is_finite() && fs_hz > 0.0) {
        return 0;
    }
    let blocks = BATCHES_PER_ITER as f64 * TICKS_PER_BATCH as f64;
    ((blocks * block_frames as f64 / fs_hz) * 1e3)
        .round()
        .max(0.0) as u64
}

/// Why a channel table cannot carry an Explorer run.
///
/// Zero and two Drive rows are different mistakes: the service refuses two on its own,
/// and treats zero as one, which on a station with no drive row would build an output
/// frame nothing is bound to. Both are decided before a station is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableRefusal {
    /// No enabled Drive row: the tone has no output to go on.
    NoDriveRow,
    /// More than one enabled Drive row (the count): the extra outputs would stay silent.
    ExtraDriveRows(usize),
    /// No enabled input row: nothing measures the tone that is commanded.
    NoInputs,
}

/// The 1-based table indices of the acquired channels, in interleave order, or why the
/// table cannot carry an Explorer run.
pub fn acquired_inputs(table: &ChannelTable) -> Result<Vec<u32>, TableRefusal> {
    let n_drive = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        .count();
    if n_drive == 0 {
        return Err(TableRefusal::NoDriveRow);
    }
    if n_drive > 1 {
        return Err(TableRefusal::ExtraDriveRows(n_drive));
    }
    let inputs: Vec<u32> = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .map(|c| c.index)
        .collect();
    if inputs.is_empty() {
        return Err(TableRefusal::NoInputs);
    }
    Ok(inputs)
}

/// An operation the host refused.
#[derive(Debug, Clone, PartialEq)]
pub enum HostError {
    /// No service is installed.
    NotConfigured,
    /// A run is already under way (armed, running or stopping).
    AlreadyRunning,
    /// The installed service is in a state a start cannot leave; it must be rebuilt.
    CannotStart(RunState),
    /// The service refused.
    Service(ServiceError),
}

/// What the host is doing, as a status reader sees it. Read in this order: a latched
/// fault outranks the arm in progress, which outranks the stall watchdog, which outranks
/// the service's own published status.
#[derive(Debug, Clone)]
pub enum HostView {
    /// No service is installed.
    Unconfigured,
    /// A failure outside the service's own state machine ended the run (a refused arm or
    /// start, a panicked pump). The text names it, prefixed by its kind
    /// (`arm_refused:`, `start_refused:`, `pump_panicked:`).
    Faulted(String),
    /// The pump is arming the station.
    Priming,
    /// A live run has gone `stale_ms` without a pump iteration against an expected
    /// period of `period_ms`: three missed iterations, never less than 3 s.
    Stalled { stale_ms: u64, period_ms: u64 },
    /// Installed and never armed: the service has published nothing yet.
    Configured,
    /// The service's latest published status.
    Status(Box<ExplorerStatus>),
}

/// The status a reader gets without touching the service mutex.
#[derive(Debug, Clone)]
pub struct HostSnapshot {
    /// The rate the installed run asked for; `None` when nothing is installed.
    pub requested_fs_hz: Option<f64>,
    /// The rate the station programmed; `None` until the arm has adopted it.
    pub actual_fs_hz: Option<f64>,
    pub view: HostView,
}

/// A journal event from the pump. The sink decides where it goes and how it reads.
#[derive(Debug)]
pub enum HostNote<'a> {
    /// The arm refused; the run is latched faulted.
    ArmRefused(&'a ServiceError),
    /// The start after a good arm refused; the run is latched faulted.
    StartRefused(&'a ServiceError),
    /// The sweep the run was installed with could not begin; the run continues parked.
    InitialSweepRefused(&'a ServiceError),
    /// A tick batch failed; the service has latched a terminal state and its end record.
    LoopFaulted(&'a ServiceError),
    /// A driver diagnostic not journalled before.
    DriverWarning(&'a str),
    /// The analysis ring has dropped this many blocks so far (repeated while it grows).
    RingDropping(u64),
    /// The peak profile and the spectra are on different grids; the profile was withheld.
    ProfileGridMismatch {
        profile_bins: usize,
        spectrum_bins: usize,
    },
    /// The run ended aborted, with the last published status when there is one.
    EndedAborted {
        end: &'a EndRecord,
        tail: Option<&'a ExplorerStatus>,
    },
    /// The run ended any other way.
    Ended(&'a EndRecord),
    /// The pump thread is leaving after `iters` iterations.
    PumpExited { iters: u64 },
    /// The pump panicked; the service is being aborted.
    PumpPanicked(&'a str),
}

/// Where a running host's output goes. One sink per run, handed to [`RunHost::start`]
/// and owned by the pump thread.
///
/// The calls marked "lock held" run inside the service mutex, between tick batches, so
/// they pack and buffer; they never send. Sending happens in [`HostSink::publish`] and
/// [`HostSink::finish`], after the mutex is released, so a slow consumer can never
/// lengthen the hold.
pub trait HostSink: Send + 'static {
    /// The run armed at `fs_hz`, the rate the station programmed, with `display_nfft`
    /// point display spectra. Every later call is denominated in that rate. Lock held.
    fn armed(&mut self, fs_hz: f64, display_nfft: usize);

    /// One drained acquisition block, interleaved `stride` wide, in acquisition order.
    /// Lock held.
    fn block(&mut self, samples: &[f32], stride: usize);

    /// `frames` acquired frames the analysis ring dropped since the last call. They are
    /// older than anything still in the ring. Lock held.
    fn dropped(&mut self, frames: u64);

    /// A NEW spectrum window, one frame per acquired channel, and the peak-level sweep
    /// profile on the same grid once anything has been visited. Returns whether anything
    /// was packed to send; `false` leaves the window to be offered again. Lock held.
    fn spectra(&mut self, frames: &[SpectrumFrame], profile: Option<&[f32]>) -> bool;

    /// Send what the lock-held calls packed. Called once per pump iteration.
    fn publish(&mut self);

    /// The run ended: pack whatever is still buffered and send everything.
    fn finish(&mut self);

    /// A journal event.
    fn note(&mut self, note: HostNote<'_>);

    /// The abort reason recorded on the service when the pump panics.
    fn panic_reason(&self, what: &str) -> String {
        format!("pump panicked: {what}")
    }
}

/// The run host. See the module docs.
pub struct RunHost {
    svc: Arc<Mutex<Option<ExplorerHostService>>>,
    stop: Arc<AtomicBool>,
    /// True while the pump is arming (open -> configure -> adopt -> gate -> build ->
    /// prefill -> start). The snapshot answers "priming" instead of a stall read off a
    /// heartbeat that has not started.
    priming: Arc<AtomicBool>,
    /// True from the moment a ramped stop is requested until the pump exits. The stall
    /// watchdog is exempt while it is set: the stop path ends in `Backend::stop`, whose
    /// NI implementation generates and waits out a taper of up to ~2 s inside one
    /// iteration, which is not a stall.
    stopping: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Verbs parked on the service mutex right now. See [`WAITER_RELEASE`].
    waiters: Arc<AtomicU64>,
    /// The service's own published status, shared without the service lock. `None`
    /// until a service is installed.
    status_cell: Mutex<Option<Arc<StatusCell>>>,
    /// The pump's expected iteration period in ms, for the stall watchdog. Estimated at
    /// install from the requested rate and refined by the pump at arm from what the
    /// driver programmed.
    expected_iter_ms: Arc<AtomicU64>,
    /// Unix ms of the pump's last completed iteration. `0` = no pump.
    last_tick_ms: Arc<AtomicU64>,
    /// The rate the installed run asked for, f64 bits; `0` = nothing installed.
    requested_fs_bits: AtomicU64,
    /// The rate the station programmed, f64 bits; `0` = not yet known. Only the arm can
    /// fill it, and install clears it, so a reinstall never serves a previous arm's rate.
    actual_fs_bits: Arc<AtomicU64>,
    /// A failure that ended the run outside the service's state machine. See the module
    /// docs.
    fault: Arc<Mutex<Option<String>>>,
    /// A sweep to begin as soon as the run engages.
    initial_sweep: Mutex<Option<(SweepDirection, f64)>>,
    /// What paces the installed run's pump.
    pacing: Mutex<Pacing>,
}

impl Default for RunHost {
    fn default() -> Self {
        Self::new()
    }
}

impl RunHost {
    /// An empty host. Builds nothing until [`RunHost::install`].
    pub fn new() -> Self {
        RunHost {
            svc: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            priming: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            waiters: Arc::new(AtomicU64::new(0)),
            status_cell: Mutex::new(None),
            expected_iter_ms: Arc::new(AtomicU64::new(0)),
            last_tick_ms: Arc::new(AtomicU64::new(0)),
            requested_fs_bits: AtomicU64::new(0),
            actual_fs_bits: Arc::new(AtomicU64::new(0)),
            fault: Arc::new(Mutex::new(None)),
            initial_sweep: Mutex::new(None),
            pacing: Mutex::new(Pacing::Station),
        }
    }

    /// Is a run live (driving, or clocked and about to be able to drive) right now?
    ///
    /// `Armed` counts: the arm opens the AO clock and writes the drive lead-in, so from
    /// that instant the station is reserved and the output is running at 0.000 V. A
    /// definition that only counted `Running` would let another module claim the station
    /// out from under a clocked chassis.
    pub fn is_live(&self) -> bool {
        // The pump is inside `arm()`, which holds the mutex for its whole duration, so
        // this is checked before the try-lock.
        if self.priming.load(Ordering::SeqCst) {
            return true;
        }
        match self.svc.try_lock() {
            Ok(g) => g.as_ref().is_some_and(|s| {
                matches!(
                    s.state(),
                    RunState::Armed | RunState::Running | RunState::Stopping
                )
            }),
            // A contended mutex IS the pump mid-batch, which is the definition of live.
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        }
    }

    /// Is a service installed at all? Never a blocking lock.
    pub fn is_configured(&self) -> bool {
        match self.svc.try_lock() {
            Ok(g) => g.is_some(),
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        }
    }

    /// Install a built, idle service, replacing whatever was installed.
    ///
    /// The caller checks [`RunHost::is_live`] BEFORE it builds the station the service
    /// runs on: a second station opened beside a live run is the failure, and by the
    /// time the service exists it has already happened.
    pub fn install(&self, svc: ExplorerHostService, initial_sweep: Option<(SweepDirection, f64)>) {
        self.install_paced(svc, initial_sweep, Pacing::Station);
    }

    /// [`RunHost::install`] with an explicit [`Pacing`] for the run's pump.
    pub fn install_paced(
        &self,
        svc: ExplorerHostService,
        initial_sweep: Option<(SweepDirection, f64)>,
        pacing: Pacing,
    ) {
        *self.pacing.lock().expect("pacing poisoned") = pacing;
        // Before the arm the service's rate is still the request.
        let fs = svc.actual_fs_hz();
        // An ESTIMATE until the arm refines it from what the driver programmed; it only
        // has to be non-zero and roughly right for the window before the arm, during
        // which the 3 s floor is the whole threshold anyway.
        self.expected_iter_ms.store(
            iter_period_ms(fs, eigen_hal::default_block_frames(fs)),
            Ordering::SeqCst,
        );
        self.requested_fs_bits.store(fs.to_bits(), Ordering::SeqCst);
        // The station has not answered yet. Nothing may stand in for it.
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        *self.fault.lock().expect("fault poisoned") = None;
        self.stopping.store(false, Ordering::SeqCst);
        *self.initial_sweep.lock().expect("sweep poisoned") = initial_sweep;
        *self.status_cell.lock().expect("cell poisoned") = Some(svc.status_cell());
        *self.svc.lock().expect("svc poisoned") = Some(svc);
    }

    /// Start the installed run on a pump thread that sends its output to `sink`.
    ///
    /// The cheap refusals are synchronous (not installed, wrong state). The arm is not:
    /// it runs on the pump behind the priming flag, and a refused arm is latched as a
    /// fault the snapshot reports.
    pub fn start<S: HostSink>(&self, sink: S) -> Result<(), HostError> {
        {
            let guard = self.lock_svc();
            let svc = guard.as_ref().ok_or(HostError::NotConfigured)?;
            match svc.state() {
                RunState::Idle => {}
                RunState::Armed | RunState::Running | RunState::Stopping => {
                    return Err(HostError::AlreadyRunning)
                }
                other => return Err(HostError::CannotStart(other)),
            }
        }
        // The beat is reset BEFORE the flag so the stall arithmetic can never read the
        // previous run's heartbeat.
        self.last_tick_ms.store(now_ms(), Ordering::Relaxed);
        self.priming.store(true, Ordering::SeqCst);
        self.spawn_pump(sink);
        Ok(())
    }

    /// Absolute frequency intent. Answers with the CLAMPED value the step adopted.
    pub fn go_to_frequency(&self, hz: f64) -> Result<f64, HostError> {
        self.with_svc(|s| s.set_frequency(hz))
    }

    /// Relative frequency intent. Answers with the CLAMPED value the step adopted.
    pub fn adjust_frequency(&self, delta_hz: f64) -> Result<f64, HostError> {
        self.with_svc(|s| s.adjust_frequency(delta_hz))
    }

    /// Absolute drive intent, volts peak. Answers with the CLAMPED value.
    pub fn set_amplitude_vpk(&self, vpk: f64) -> Result<f64, HostError> {
        self.with_svc(|s| s.set_amplitude_vpk(vpk))
    }

    /// Begin (or re-aim) a sweep toward a band edge.
    pub fn start_sweep(
        &self,
        direction: SweepDirection,
        rate_oct_per_min: f64,
    ) -> Result<(), HostError> {
        self.with_svc(|s| s.start_sweep(direction, rate_oct_per_min))
    }

    /// Stop sweeping; the tone HOLDS where the sweep reached.
    pub fn stop_sweep(&self) -> Result<(), HostError> {
        self.with_svc(|s| s.stop_sweep())
    }

    /// The controlled stop: the drive RAMPS to zero at the operator's drive slew, and
    /// only then are the tasks stopped.
    ///
    /// The pump walks the ramp, so the stop flag is deliberately NOT raised for a live
    /// run: raising it would halt the pump mid-ramp and leave the last commanded sample on
    /// the output.
    pub fn stop(&self) {
        {
            let mut guard = self.lock_svc();
            match guard.as_mut() {
                Some(s) => match s.state() {
                    RunState::Armed | RunState::Running | RunState::Stopping => {
                        self.stopping.store(true, Ordering::SeqCst);
                        s.request_stop();
                        return;
                    }
                    // Idle (installed, never armed) or terminal: nothing is driving, so
                    // there is nothing to ramp. `request_stop` from Idle would park the
                    // service at `Stopping` with no pump to complete it, which reads as a
                    // live run forever.
                    RunState::Idle | RunState::Stopped | RunState::Aborted => {}
                },
                None => return,
            }
        }
        self.signal_and_reap();
    }

    /// ABORT: immediate, no ramp. The service issues `emergency_stop` before this
    /// returns; the backend's own taper is what keeps that from being a cliff.
    pub fn abort(&self, reason: &str) -> Result<(), HostError> {
        {
            let mut guard = self.lock_svc();
            if let Some(s) = guard.as_mut() {
                if !s.state().is_terminal() {
                    s.abort(reason).map_err(HostError::Service)?;
                }
            }
        }
        // Signal, do NOT join: the pump is at most one bounded batch from noticing the
        // terminal state, and joining here would make an abort wait on the driver it
        // just told to stop.
        self.stop.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Drop an installed-but-idle service so this host stops holding the station.
    /// Returns whether anything was released. A live run is never released here.
    pub fn release_idle(&self) -> bool {
        if self.is_live() || !self.is_configured() {
            return false;
        }
        // Signal first, then reap: an idle pump is at most one bounded batch from its
        // exit, and the stop flag is what tells it to take it.
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        // The drop completes here, releasing the station's tasks, before whatever
        // displaced this run builds its own.
        *self.svc.lock().expect("svc poisoned") = None;
        self.stop.store(false, Ordering::SeqCst);
        self.stopping.store(false, Ordering::SeqCst);
        // Every cache that outlives its service is a status path reporting a run this
        // host can no longer act on.
        if let Ok(mut c) = self.status_cell.lock() {
            *c = None;
        }
        if let Ok(mut f) = self.fault.lock() {
            *f = None;
        }
        if let Ok(mut s) = self.initial_sweep.lock() {
            *s = None;
        }
        self.requested_fs_bits.store(0, Ordering::SeqCst);
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        self.last_tick_ms.store(0, Ordering::Relaxed);
        self.expected_iter_ms.store(0, Ordering::SeqCst);
        true
    }

    /// The status, without the service mutex. See [`HostView`] for the precedence.
    pub fn snapshot(&self) -> HostSnapshot {
        let read = |bits: &AtomicU64| -> Option<f64> {
            let v = f64::from_bits(bits.load(Ordering::SeqCst));
            (v.is_finite() && v > 0.0).then_some(v)
        };
        HostSnapshot {
            requested_fs_hz: read(&self.requested_fs_bits),
            actual_fs_hz: read(&self.actual_fs_bits),
            view: self.view(),
        }
    }

    fn view(&self) -> HostView {
        let Some(cell) = self.status_cell.lock().ok().and_then(|c| c.clone()) else {
            return HostView::Unconfigured;
        };
        if let Some(f) = self.fault.lock().ok().and_then(|f| f.clone()) {
            return HostView::Faulted(f);
        }
        if self.priming.load(Ordering::SeqCst) {
            return HostView::Priming;
        }
        let published = cell.read();
        // THE STALL WATCHDOG. The beat is stored every pump iteration (a quiet read is
        // not a stall), and the threshold is three missed iterations with a 3 s floor.
        // The floor is what saves a fast loop: at a 30 ms iteration, three periods is
        // 90 ms and any scheduling hiccup would read as a dead driver.
        let running = published
            .as_ref()
            .is_some_and(|st| matches!(st.state, RunState::Armed | RunState::Running));
        if running && !self.stopping.load(Ordering::SeqCst) {
            let beat = self.last_tick_ms.load(Ordering::Relaxed);
            let period = self.expected_iter_ms.load(Ordering::SeqCst).max(1);
            let stall_after = (3 * period).max(3_000);
            let stale_ms = now_ms().saturating_sub(beat);
            if beat != 0 && stale_ms > stall_after {
                return HostView::Stalled {
                    stale_ms,
                    period_ms: period,
                };
            }
        }
        match published {
            Some(st) => HostView::Status(Box::new(st)),
            None => HostView::Configured,
        }
    }

    /// Run `f` on the installed service under the mutex.
    fn with_svc<T>(
        &self,
        f: impl FnOnce(&mut ExplorerHostService) -> Result<T, ServiceError>,
    ) -> Result<T, HostError> {
        let mut guard = self.lock_svc();
        let svc = guard.as_mut().ok_or(HostError::NotConfigured)?;
        f(svc).map_err(HostError::Service)
    }

    /// Take the service mutex, counting this thread as a parked waiter first so the pump
    /// widens its release window (see [`WAITER_RELEASE`]). A BLOCKING lock: callers run
    /// beside the display path, never in front of it, and blocking is what lets every
    /// intent answer with the value the step adopted.
    fn lock_svc(&self) -> std::sync::MutexGuard<'_, Option<ExplorerHostService>> {
        self.waiters.fetch_add(1, Ordering::Relaxed);
        let g = self.svc.lock().expect("svc poisoned");
        self.waiters.fetch_sub(1, Ordering::Relaxed);
        g
    }

    /// Tell the pump to leave and reap it if it already has.
    fn signal_and_reap(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let mut slot = self.pump.lock().expect("pump poisoned");
        if let Some(h) = slot.take() {
            if h.is_finished() {
                let _ = h.join();
            } else {
                *slot = Some(h);
            }
        }
    }

    /// The pump thread: arm, then batch / analyse / publish until the run ends.
    fn spawn_pump<S: HostSink>(&self, sink: S) {
        let mut slot = self.pump.lock().expect("pump poisoned");
        // A FINISHED handle is not a running pump: a run that ends on its own leaves its
        // handle in the slot, and reading that as "already pumping" would arm a service
        // nothing then ticks.
        if let Some(h) = slot.take() {
            if h.is_finished() {
                let _ = h.join();
            } else {
                *slot = Some(h);
                return; // genuinely already pumping
            }
        }
        self.stop.store(false, Ordering::SeqCst);
        self.stopping.store(false, Ordering::SeqCst);

        let svc = Arc::clone(&self.svc);
        let priming = Arc::clone(&self.priming);
        let fault_out = Arc::clone(&self.fault);
        let ctx = PumpCtx {
            svc: Arc::clone(&self.svc),
            stop: Arc::clone(&self.stop),
            priming: Arc::clone(&self.priming),
            beat: Arc::clone(&self.last_tick_ms),
            waiters: Arc::clone(&self.waiters),
            initial_sweep: *self.initial_sweep.lock().expect("sweep poisoned"),
            pacing: *self.pacing.lock().expect("pacing poisoned"),
            actual_fs: Arc::clone(&self.actual_fs_bits),
            expected: Arc::clone(&self.expected_iter_ms),
            fault: Arc::clone(&self.fault),
        };

        let handle = std::thread::spawn(move || {
            let mut sink = sink;
            // The whole pump body under `catch_unwind`. The service already catches a
            // panic inside its own tick batch (and issues `emergency_stop` from there);
            // this covers everything else the thread does - the analysis pass, the
            // sink's packing - because a panic in any of them unwinds past every stop
            // path while the AO keeps clocking its last sample.
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pump_body(ctx, &mut sink);
            }));
            if let Err(payload) = caught {
                let what = panic_text(&payload);
                sink.note(HostNote::PumpPanicked(&what));
                if let Ok(mut g) = svc.lock() {
                    if let Some(s) = g.as_mut() {
                        let _ = s.abort(&sink.panic_reason(&what));
                    }
                }
                if let Ok(mut f) = fault_out.lock() {
                    *f = Some(format!("pump_panicked: {what}; emergency_stop was issued"));
                }
                priming.store(false, Ordering::SeqCst);
            }
        });
        *slot = Some(handle);
    }
}

impl Drop for RunHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
    }
}

/// Everything the pump thread owns.
struct PumpCtx {
    pacing: Pacing,
    svc: Arc<Mutex<Option<ExplorerHostService>>>,
    stop: Arc<AtomicBool>,
    priming: Arc<AtomicBool>,
    beat: Arc<AtomicU64>,
    waiters: Arc<AtomicU64>,
    initial_sweep: Option<(SweepDirection, f64)>,
    actual_fs: Arc<AtomicU64>,
    expected: Arc<AtomicU64>,
    fault: Arc<Mutex<Option<String>>>,
}

/// The pump: arm, then batch / analyse / publish until the run ends.
fn pump_body<S: HostSink>(ctx: PumpCtx, sink: &mut S) {
    // The priming flag clears on EVERY exit path, refusals and panics included.
    struct PrimingGuard(Arc<AtomicBool>);
    impl Drop for PrimingGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    // ---- PRIMING: open -> configure -> adopt -> gate -> build -> prefill -> start ----
    //
    // The profile raster is built from the rate the driver ACTUALLY programmed and lives
    // for this run only: a new run spawns a new pump, and a reinstall builds a new
    // service, so it can never carry a previous grid. Every failure path here returns,
    // so it is never observed unbuilt.
    let (mut raster, block_frames, mut last_dropped, fs_hz) = {
        let guard = PrimingGuard(Arc::clone(&ctx.priming));
        let mut lock = match ctx.svc.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(s) = lock.as_mut() else { return };
        if let Err(e) = s.arm() {
            sink.note(HostNote::ArmRefused(&e));
            if let Ok(mut f) = ctx.fault.lock() {
                *f = Some(format!("arm_refused: {e}"));
            }
            return;
        }
        // THE ARM IS THE ONLY MOMENT THE ACTUAL RATE EXISTS: `arm()` adopts the
        // programmed rate into the oscillator, the band, the slew increments and the
        // display grid. Read on the success path only, so a refused arm reports "not
        // known".
        ctx.actual_fs
            .store(s.actual_fs_hz().to_bits(), Ordering::SeqCst);
        let mut block_frames: u64 = 0;
        if let Some(c) = s.configured() {
            ctx.expected.store(
                iter_period_ms(s.actual_fs_hz(), c.block_frames.max(1)),
                Ordering::SeqCst,
            );
            block_frames = c.block_frames.max(1) as u64;
        }
        // Both display grids are denominated in the ADOPTED rate: a delta-sigma front end
        // coerces a requested 8192 up to 8533.33, and a time base or a profile axis built
        // on the request would be wrong by that ratio while looking entirely normal.
        let fs = s.actual_fs_hz();
        let nfft = s.display_nfft().max(2);
        sink.armed(fs, nfft);
        let raster = ProfileRaster::new(0.0, fs / nfft as f64, nfft / 2 + 1);
        let dropped_at_arm = s.blocks_dropped();
        if let Err(e) = s.start() {
            sink.note(HostNote::StartRefused(&e));
            if let Ok(mut f) = ctx.fault.lock() {
                *f = Some(format!("start_refused: {e}"));
            }
            return;
        }
        // An installed sweep begins here, with the drive still at 0.000 V: the tone
        // travels, nothing drives, and the operator raises the level when ready.
        if let Some((direction, rate)) = ctx.initial_sweep {
            if let Err(e) = s.start_sweep(direction, rate) {
                sink.note(HostNote::InitialSweepRefused(&e));
            }
        }
        // A first snapshot, so a status read between the arm and the first batch sees
        // the armed run rather than "configured".
        let _ = s.service_analysis();
        drop(guard);
        (raster, block_frames, dropped_at_arm, fs)
    };
    ctx.beat.store(now_ms(), Ordering::Relaxed);
    let started = Instant::now();

    // ---- THE LOOP ----------------------------------------------------------------
    let mut iters: u64 = 0;
    let mut last_publish = Instant::now()
        .checked_sub(DISPLAY_PERIOD)
        .unwrap_or_else(Instant::now);
    let mut last_seq: u64 = 0;
    let mut last_logged_warning: Option<String> = None;

    while !ctx.stop.load(Ordering::Relaxed) && iters < MAX_PUMP_ITERS {
        // Producer-side decimation, decided before the lock so a window that is not due
        // is never offered to the sink.
        let due = last_publish.elapsed() >= DISPLAY_PERIOD;
        let outcome = {
            let mut lock = match ctx.svc.lock() {
                Ok(g) => g,
                Err(_) => break,
            };
            let Some(s) = lock.as_mut() else { break };
            if s.state().is_terminal() {
                break;
            }
            match s.run_batches(BATCHES_PER_ITER) {
                Ok(alive) => {
                    // THE RING-DROP HOLE, read here so it is attributed to the window it
                    // happened in: `run_batches` pushes blocks into the ring and is
                    // therefore what overflows it; the drain is the next line.
                    let dropped_now = s.blocks_dropped();
                    if dropped_now > last_dropped {
                        let lost = dropped_now
                            .saturating_sub(last_dropped)
                            .saturating_mul(block_frames);
                        last_dropped = dropped_now;
                        sink.dropped(lost);
                    }
                    // The off-RT analysis pass. It publishes the status cell the snapshot
                    // reads, and hands every drained block to the sink on the way past.
                    let st = s.service_analysis_tapped(|blk, stride| sink.block(blk, stride));
                    // THE PEAK PROFILE: every point the tracer recorded this pass, folded
                    // in at its commanded frequency. Drained, so the tracer stays bounded
                    // and the raster is the one thing that remembers.
                    for p in s.take_profile_points() {
                        raster.visit(p.freq_hz, p.amplitude);
                    }
                    let published =
                        due && offer_window(s.latest_spectra(), &mut last_seq, &raster, sink);
                    // Driver diagnostics, tracked by CONTENT, not by a count:
                    // `driver_warnings` is bounded and drains its oldest entries when it
                    // saturates, so a count watermark would stop advancing at the bound
                    // and stop journalling precisely when the driver is talking most.
                    let fresh: Vec<String> = match last_logged_warning.as_ref() {
                        Some(prev) => match st.driver_warnings.iter().rposition(|w| w == prev) {
                            Some(i) => st.driver_warnings[i + 1..].to_vec(),
                            // The watermark itself was drained: everything still held is
                            // new, and some were lost before they were seen.
                            None => st.driver_warnings.clone(),
                        },
                        None => st.driver_warnings.clone(),
                    };
                    if let Some(last) = fresh.last() {
                        last_logged_warning = Some(last.clone());
                    }
                    Some((
                        alive,
                        st.blocks_dropped,
                        st.frames_generated,
                        published,
                        fresh,
                    ))
                }
                Err(e) => {
                    // The service has already latched a terminal state and its end
                    // record, so the status will report it; this is the journal half.
                    sink.note(HostNote::LoopFaulted(&e));
                    // Tapped like the healthy path: this pass drains the ring, and an
                    // untapped one would swallow the last samples before the fault.
                    let _ = s.service_analysis_tapped(|blk, stride| sink.block(blk, stride));
                    None
                }
            }
        };
        // Sent with the service mutex RELEASED.
        sink.publish();
        let Some((alive, blocks_dropped, frames_generated, published, fresh_warnings)) = outcome
        else {
            break;
        };
        for w in &fresh_warnings {
            sink.note(HostNote::DriverWarning(w));
        }
        if blocks_dropped > 0 && iters.is_multiple_of(256) {
            sink.note(HostNote::RingDropping(blocks_dropped));
        }
        if published {
            last_publish = Instant::now();
        }
        // THE HEARTBEAT, every iteration. A beat that only fired on a productive
        // iteration would make an idle-but-healthy loop look like a dead driver.
        ctx.beat.store(now_ms(), Ordering::Relaxed);
        iters += 1;
        if !alive {
            break;
        }
        let release = if ctx.waiters.load(Ordering::Relaxed) > 0 {
            WAITER_RELEASE
        } else {
            RELEASE_FLOOR
        };
        std::thread::sleep(match ctx.pacing {
            Pacing::Station => release,
            Pacing::WallClock => {
                pace_pause(started.elapsed(), frames_generated, fs_hz).max(release)
            }
        });
    }

    // ---- THE FINAL PASS, on EVERY exit path ---------------------------------------
    //
    // The status cell is written only by an analysis pass, and the run can be ended from
    // OUTSIDE this thread: `RunHost::abort` aborts the service directly and raises the
    // stop flag, which ends the loop at its condition, before any in-loop pass. Leaving
    // without one strands the cell on the last running snapshot, and the stall watchdog
    // then reports a run that aborted cleanly as a dead driver.
    if let Ok(mut g) = ctx.svc.lock() {
        if let Some(s) = g.as_mut() {
            // The same pass drains the last blocks the ring holds, so the time stream's
            // final samples are the ones that were actually acquired last.
            let _ = s.service_analysis_tapped(|blk, stride| sink.block(blk, stride));
            // The profile's last points and one final window carrying the finished curve:
            // the raster dies with this thread, so a point recorded and never offered is
            // a measurement the operator took and never saw.
            for p in s.take_profile_points() {
                raster.visit(p.freq_hz, p.amplitude);
            }
            offer_window(s.latest_spectra(), &mut last_seq, &raster, sink);
        }
    }
    sink.finish();

    // ---- WHY did the loop end? ---------------------------------------------------
    //
    // A run that ended for any reason but "the operator asked" has to say so in the
    // journal, or a tripped run reads as an unexplained thread exit.
    if let Ok(g) = ctx.svc.lock() {
        if let Some(s) = g.as_ref() {
            match s.end_record() {
                Some(end) if end.state == RunState::Aborted => {
                    let tail = s.status_cell().read();
                    sink.note(HostNote::EndedAborted {
                        end,
                        tail: tail.as_ref(),
                    });
                }
                Some(end) => sink.note(HostNote::Ended(end)),
                None => {}
            }
        }
    }
    sink.note(HostNote::PumpExited { iters });
}

/// Offer the service's latest spectra to the sink when they are a NEW window, with the
/// peak profile when it is on their grid. Returns whether the sink packed anything.
///
/// Offering the same window again at the display cadence would put the same numbers out
/// under a fresh sequence number, which a consumer cannot tell from a live measurement
/// that stopped changing.
fn offer_window<S: HostSink>(
    frames: &[SpectrumFrame],
    last_seq: &mut u64,
    raster: &ProfileRaster,
    sink: &mut S,
) -> bool {
    let Some(first) = frames.first() else {
        return false;
    };
    if first.df_hz <= 0.0 || first.seq <= *last_seq {
        return false;
    }
    // The raster is built from `display_nfft` and the adopted rate, the same arithmetic
    // the spectrum engine does, so this is an equality and not a resample. A mismatch
    // means one grid moved: withhold the profile rather than draw it against frequencies
    // it was not measured at.
    let rendered = raster.render();
    let profile = match rendered.as_deref() {
        Some(mags) if mags.len() == first.mags.len() => Some(mags),
        Some(mags) => {
            sink.note(HostNote::ProfileGridMismatch {
                profile_bins: mags.len(),
                spectrum_bins: first.mags.len(),
            });
            None
        }
        None => None,
    };
    if !sink.spectra(frames, profile) {
        return false;
    }
    *last_seq = first.seq;
    true
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Best-effort text out of a caught panic payload.
fn panic_text(payload: &Box<dyn std::any::Any + Send>) -> String {
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

    /// A wall-clock-paced pump waits until the signal it generated is due, never longer
    /// than the cap, and never waits when it is behind.
    #[test]
    fn the_wall_clock_pause_holds_generated_frames_to_real_time() {
        // 0.25 s of signal generated 0.2 s into the run: 50 ms early.
        assert_eq!(
            pace_pause(Duration::from_millis(200), 2_048, 8_192.0),
            Duration::from_millis(50)
        );
        // 0.25 s of signal 0.24 s in: 10 ms early.
        let p = pace_pause(Duration::from_millis(240), 2_048, 8_192.0);
        assert!(
            (p.as_secs_f64() - 0.010).abs() < 1e-6,
            "10 ms early waits 10 ms: {p:?}"
        );
        // A full second ahead waits only the cap, so a stop is never held behind it.
        assert_eq!(pace_pause(Duration::ZERO, 8_192, 8_192.0), MAX_PACE_PAUSE);
        // Behind schedule, or a rate that does not exist: no wait.
        assert_eq!(
            pace_pause(Duration::from_secs(1), 4_096, 8_192.0),
            Duration::ZERO
        );
        assert_eq!(pace_pause(Duration::ZERO, 4_096, 0.0), Duration::ZERO);
        assert_eq!(pace_pause(Duration::ZERO, 4_096, f64::NAN), Duration::ZERO);
    }
}
