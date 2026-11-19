//! The daemon-side CLOSED-LOOP SINE module: thin orchestration over `hertzforge-sine`
//! and the station factory, so a REMOTE front end can run a swept-sine test on
//! this host's hardware.
//!
//! # THE CONTROL LOOP NEVER CROSSES THE NETWORK
//!
//! Exactly as the Random and Explorer hosts: the pump thread owns the service,
//! steps `run_batches` on the hardware host, and publishes DECIMATED DISPLAY
//! products on the shared broadcast. What crosses the wire is operator INTENT
//! down and display frames back. A remote Stop/Abort is SUPERVISORY; the primary
//! abort authority is the in-process watchdog co-resident with the drive clock,
//! backed by the physical E-stop.
//!
//! # THE PUMP SHAPE IS RANDOM'S, NOT THE EXPLORER'S, AND THAT IS THE WHOLE DESIGN
//!
//! The Explorer primes (open -> configure -> adopt -> gate -> prefill -> start)
//! INSIDE a lock-held block and then loops without ever leaving. Sine cannot: it
//! has two PARKS a run legitimately sits at, and the verb that releases each one
//! needs the service mutex.
//!
//! * [`RunState::FeasibilityBlocked`] - released by `Sine(OverrideFeasibility)`.
//! * [`RunState::AwaitingReview`] - released by `ProceedAfterReview`.
//!
//! A lock-held priming block would hold the mutex ACROSS the park, so the release
//! verb would deadlock against the thread it is trying to release. Random solved
//! this already: the pump EXITS at a park and the release verb RESPAWNS it
//! (`random.rs`'s priming phase and its `proceed_after_review`). Both of Sine's
//! parks are modelled that way.
//!
//! [`RunState::CoherenceHold`] is the third park and it is DIFFERENT: it is the
//! gate FAILING, not passing-and-pausing. The pump exits there too, but nothing
//! respawns it - `SineService::proceed_after_review` refuses that state by name,
//! and this host surfaces the refusal VERBATIM rather than paraphrasing it. Only
//! a `SetProfile` (which rebuilds the service) clears it. A release verb wired to
//! that park would be a one-click bypass of a safety refusal.
//!
//! # WHY A LOCK-FREE PHASE MIRROR
//!
//! `SineService`'s [`StatusCell`] is written by the CONTROL TICK, so it carries
//! the numbers and not the state, and it does not exist at all until the step is
//! built - which is to say it is EMPTY across arm -> sysID -> gate, the exact
//! window an operator stares at for tens of seconds. The Explorer's cell carries
//! its whole status because its analysis pass writes it; Sine's does not.
//!
//! So this host keeps an [`AtomicU8`] PHASE beside the priming flag, written by
//! whoever moves the run and read without any lock at all. `fill_status` renders
//! the phase (and the `hold` reason that goes with a parked one) and takes the
//! NUMBERS from the cell once the engaged phase has one. Two snapshots, split by
//! who can write them - the Explorer's rationale, applied to a service whose cell
//! covers less.
//!
//! Without it, `fill_status` would early-return "priming" for the whole bring-up
//! and an operator watching a 27 s sysID could not tell it from a hung arm.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hertzforge_protocol::{LadderMode, LevelTarget, ProtocolError, SineWireProfile};
use hertzforge_sine::service::engine::{SineProcessStep, StatusCell};
use hertzforge_sine::{RunMode, RunState, SineService};
use hertzforge_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};

use crate::display::TimeTap;
use crate::provenance::{mint_run_id, now_unix, RunProvenance};
use crate::recorder::{DisplayBatch, SpectrumBatch};

mod convert;
pub use convert::config_from_wire;

// =====================================================================================
// Reserved trace ids
// =====================================================================================

// The SWEEP-OVERLAY block, byte-identical to the local app's sine ids
// (`ui/src-tauri/src/sine.rs`, pinned by `ui/IPC_CONTRACT.md`). The remote front end
// decodes these frames with the SAME decoder as the local path, so the ids must be the
// same numbers or the overlay renders the wrong trace.
/// Profile target, on the sweep raster.
pub const CH_PROFILE: u32 = 910;
/// Alarm band, upper edge.
pub const CH_ALARM_HI: u32 = 911;
/// Alarm band, lower edge.
pub const CH_ALARM_LO: u32 = 912;
/// Abort band, upper edge.
pub const CH_ABORT_HI: u32 = 913;
/// Abort band, lower edge.
pub const CH_ABORT_LO: u32 = 914;
/// Drive reference (the sysID `|H|` resampled onto the sweep raster).
pub const CH_DRIVE: u32 = 915;
/// Composite tracked amplitude.
pub const CH_COMPOSITE: u32 = 916;
/// Total harmonic distortion.
pub const CH_THD: u32 = 917;
/// COLA marker.
pub const CH_COLA: u32 = 918;
/// Per-channel BROADBAND rides `real channel id + this`, as it does locally.
pub const BROADBAND_OFFSET: u32 = 400;

/// The sysID `|H1|` magnitude, published AT the gate. DERIVED (not an overlay):
/// it is a measurement of the plant, on the sysID grid, and display averaging
/// over successive runs of it would be meaningless.
pub const CH_SYSID_H: u32 = 1150;
/// The sysID coherence, published AT the gate, on the same grid as
/// [`CH_SYSID_H`]. An OVERLAY - a `[0, 1]` quality curve, never averaged with a
/// magnitude.
pub const CH_SYSID_COH: u32 = 1151;

// =====================================================================================
// Cadence + safety constants
// =====================================================================================

/// Tick batches per pump iteration.
///
/// One, so the loop's outer cadence is the service's own bounded batch and the
/// mailbox / stop checks between batches are not deferred behind a second batch's
/// worth of ticks. Same reasoning, same value, as the Explorer's.
const BATCHES_PER_ITER: u32 = 1;

/// The service's ticks per batch, MIRRORED for the cadence arithmetic only.
///
/// `hertzforge_sine::service::TICKS_PER_BATCH` is private, so this is a copy and a copy
/// can drift. It is load-bearing for NOTHING except the stall threshold's scaled
/// term, which the 3 s floor dominates at every rate this daemon runs. Stated so
/// the drift, if it happens, is visible and harmless rather than invisible.
const ASSUMED_TICKS_PER_BATCH: u64 = 2;

/// Pump iterations before the loop bails - the last-resort backstop that mirrors
/// Random's `MAX_LOOP_TICKS` and the Explorer's `MAX_PUMP_ITERS`.
const MAX_PUMP_ITERS: u64 = 4_000_000;

/// The display cadence, stated in TIME.
///
/// 40 ms = 25 batches per second, decimated PRODUCER-side and decided BEFORE the
/// service mutex is taken, so a pass that is not due never pays for building a
/// snapshot it would have thrown away. Stating it in time rather than in blocks is
/// what keeps the wire load the same when the operator changes the rate or the
/// block size - an "every Nth block" rule silently becomes a different frame rate
/// the moment either moves.
const DISPLAY_PERIOD: Duration = Duration::from_millis(40);

/// The pump's release window. The floor: std's mutex is unfair and a loop
/// that re-acquires microseconds after releasing starves every parked waiter for
/// the length of the run.
const RELEASE_FLOOR: Duration = Duration::from_millis(1);

/// The release window when a verb IS parked on the service mutex.
const WAITER_RELEASE: Duration = Duration::from_millis(8);

/// What the two-phase stop's generation drain is allowed to take, on top of the
/// ramp itself, before the safe-stop budget is judged against the controller
/// timeout.
///
/// `hertzforge_sine`'s `DRAIN_DEADLINE` is 2 s and is private; this mirrors it. Unlike
/// [`ASSUMED_TICKS_PER_BATCH`] this one IS load-bearing (it is inside a refusal),
/// so it is stated as an ALLOWANCE rather than as a claim about the other crate:
/// the budget check only has to be conservative, and a drain that got faster
/// would leave this pessimistic rather than wrong.
const DRAIN_ALLOWANCE_S: f64 = 2.0;

// =====================================================================================
// The lock-free phase
// =====================================================================================

/// The bring-up / run PHASE, mirrored lock-free so `fill_status` never waits on
/// the pump. See the module docs for why the service's own cell cannot carry it.
///
/// Written by whoever MOVES the run: `start` (arming), the pump (sysid, gating,
/// the parks, engaged, aborted), the stop path (stopping, stopped) and the
/// release verbs. Read with `Ordering::SeqCst` on both sides - this is a status
/// path, not a hot loop, and the cost of a fence here is not measurable against
/// the cost of getting the operator's readout wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Phase {
    /// Configured; nothing armed.
    Idle = 0,
    /// Feasibility + arm are running (the station is opening).
    Arming = 1,
    /// The sysID burst is driving.
    SysId = 2,
    /// The coherence gate is being computed, with the device PARKED.
    Gating = 3,
    /// Gate PASSED, holding for the operator. Releasable.
    AwaitingReview = 4,
    /// Feasibility EXCEEDED, holding for an acknowledged override. Releasable.
    FeasibilityBlocked = 5,
    /// Gate FAILED. NOT releasable - see the module docs.
    CoherenceHold = 6,
    /// The loop is closed and climbing the ladder.
    Engaged = 7,
    /// The loop is closed at the full rung.
    FullLevel = 8,
    /// A stop was requested; the drive is ramping and the tasks come down after.
    Stopping = 9,
    /// Cleanly stopped.
    Stopped = 10,
    /// A protection engine tripped, or the operator aborted.
    Aborted = 11,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Arming,
            2 => Phase::SysId,
            3 => Phase::Gating,
            4 => Phase::AwaitingReview,
            5 => Phase::FeasibilityBlocked,
            6 => Phase::CoherenceHold,
            7 => Phase::Engaged,
            8 => Phase::FullLevel,
            9 => Phase::Stopping,
            10 => Phase::Stopped,
            11 => Phase::Aborted,
            // 0 and anything else. A phase byte this host never wrote is a bug in
            // this file, and reporting it as Idle is the least-wrong answer: it
            // claims nothing is happening, which is the direction that does not
            // invite an operator to leave a run unattended.
            _ => Phase::Idle,
        }
    }

    /// The module's OWN fine state, in the token the front end renders verbatim.
    /// Deliberately the same vocabulary Random's `apply_status` uses for the
    /// states the two share, so one panel can speak both.
    fn token(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Arming => "arming",
            Phase::SysId => "sysid",
            Phase::Gating => "coherence_gate",
            Phase::AwaitingReview => "awaiting_review",
            Phase::FeasibilityBlocked => "feasibility_blocked",
            Phase::CoherenceHold => "coherence_hold",
            Phase::Engaged => "engaged",
            Phase::FullLevel => "full_level",
            Phase::Stopping => "stopping",
            Phase::Stopped => "stopped",
            Phase::Aborted => "aborted",
        }
    }

    /// The coarse cross-module lifecycle this phase maps onto.
    fn run_state(self) -> hertzforge_protocol::RunState {
        match self {
            Phase::Idle => hertzforge_protocol::RunState::Configured,
            Phase::Arming
            | Phase::SysId
            | Phase::Gating
            | Phase::AwaitingReview
            | Phase::FeasibilityBlocked
            | Phase::CoherenceHold => hertzforge_protocol::RunState::Armed,
            Phase::Engaged | Phase::FullLevel => hertzforge_protocol::RunState::Running,
            Phase::Stopping => hertzforge_protocol::RunState::Stopping,
            Phase::Stopped => hertzforge_protocol::RunState::Stopped,
            Phase::Aborted => hertzforge_protocol::RunState::Faulted,
        }
    }

    /// Is the run DRIVING, or clocked and about to be able to?
    ///
    /// The three parks are deliberately NOT live: `park_after_sysid` stops the AO
    /// and the drive is at zero at every one of them, so a park is a run that is
    /// holding with nothing on the pin. sysID IS live - the AO is
    /// driving the burst.
    fn is_live(self) -> bool {
        matches!(
            self,
            Phase::Arming | Phase::SysId | Phase::Gating | Phase::Engaged | Phase::FullLevel
        )
    }

    /// Is the CONTROL LOOP closed right now? The only phases the stall watchdog
    /// can judge, because they are the only ones in which the pump iterates.
    fn is_pumping(self) -> bool {
        matches!(self, Phase::Engaged | Phase::FullLevel)
    }

    fn from_run_state(s: RunState) -> Phase {
        match s {
            RunState::Idle => Phase::Idle,
            RunState::Feasibility => Phase::Arming,
            RunState::FeasibilityBlocked => Phase::FeasibilityBlocked,
            RunState::SysId => Phase::SysId,
            RunState::CoherenceGate => Phase::Gating,
            RunState::CoherenceHold => Phase::CoherenceHold,
            RunState::AwaitingReview => Phase::AwaitingReview,
            RunState::Engaged => Phase::Engaged,
            RunState::FullLevel => Phase::FullLevel,
            RunState::Stopped => Phase::Stopped,
            RunState::Aborted => Phase::Aborted,
        }
    }
}

// =====================================================================================
// The host
// =====================================================================================

/// The daemon's closed-loop Sine host.
pub struct SineHost {
    svc: Arc<Mutex<Option<SineService>>>,
    stop: Arc<AtomicBool>,
    /// The lock-free phase mirror. See [`Phase`].
    phase: Arc<AtomicU8>,
    /// True while the arm block runs (feasibility -> arm -> sysID START).
    ///
    /// Kept BESIDE the phase rather than replaced by it: the phase says what the
    /// run is doing and this says that a bring-up call is in flight holding the
    /// mutex, which is an extra reason the stall watchdog must not judge a
    /// heartbeat. The [`PrimingGuard`] clears it on EVERY exit path, refusals and
    /// panics included.
    priming: Arc<AtomicBool>,
    /// True from the moment a ramped stop is REQUESTED until the pump exits.
    ///
    /// The stall watchdog is EXEMPT while this is set: the stop path ends
    /// in the two-phase stop, which waits out a generation drain of up to
    /// [`DRAIN_ALLOWANCE_S`] inside one pump iteration. That is not a stall.
    stopping: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Verbs parked on the service mutex right now. See [`WAITER_RELEASE`].
    waiters: Arc<AtomicU64>,
    /// The SHARED display bus (one per daemon).
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    /// A built service exists. Lock-free, because `Core::live_module` asks this on
    /// a tokio worker inside dispatch and the pump holds the svc mutex for nearly
    /// the whole of a long sysID (a blocking waiter there starved for the
    /// LENGTH OF THE RUN, measured 29 s on the Random path).
    configured: Arc<AtomicBool>,
    /// The SERVICE's own status cell, taken once the step exists. `None` across
    /// the whole bring-up, which is exactly why [`SineHost::phase`] exists.
    ///
    /// SHARED with the pump: the pump learns the cell at engage (it is created by
    /// `build_step`, which the gate or a release verb runs) and `fill_status` must
    /// read the same one. A private copy on each side is how a status path ends up
    /// serving a cell nothing writes.
    status_cell: Arc<Mutex<Option<Arc<StatusCell>>>>,
    /// The host-side snapshot for the phases the cell does not cover.
    ///
    /// Written at each bring-up transition (never per tick), so a status poll
    /// during sysID answers with the band coherence and level the run last
    /// published rather than with nothing.
    bringup_cache: Arc<Mutex<Option<hertzforge_sine::SineStatus>>>,
    /// Why the run is HOLDING, verbatim; `None` = not holding. Fills the wire's
    /// `hold` field. A hold is NOT a run state - the output continues at the held
    /// frequency and level - so it gets its own field and its own writer.
    hold: Arc<Mutex<Option<String>>>,
    /// The band coherence the gate measured, f64 bits. Lock-free so the status
    /// path can carry it through every phase after the gate.
    coherence_bits: Arc<AtomicU64>,
    /// The SERVICE's schedule-progress fraction, f64 bits; NaN = not determinable.
    ///
    /// Mirrored lock-free for the same reason as the coherence above: the engaged
    /// phases read the `StatusCell`, which carries the control tick's scalars and
    /// not the scheduler's, so without this the wire could only carry progress at
    /// bring-up transitions - i.e. never while the run is actually running, which
    /// is the only time an operator wants it. Written by the pump each iteration.
    ///
    /// It is MIRRORED rather than derived because it is the module's own number:
    /// `elapsed / (elapsed + remaining)` computed at the far end is a second
    /// authority that agrees only until either end changes how it counts held time.
    progress_bits: Arc<AtomicU64>,
    /// Blocks the raw acquisition ring has dropped this run.
    ///
    /// PUBLISHED on the wire, including ZERO: the time-domain tap packs from the
    /// blocks this ring hands over, so a drop is a HOLE in what the operator is
    /// watching, and a display that has silently lost blocks looks exactly like one
    /// that has not. The wire's absent case is reserved for "no module with such a
    /// ring is live", which is not this module's answer to give once it is running.
    display_dropped: Arc<AtomicU64>,
    /// The service's sysID CANCELLATION flag, cached at arm.
    ///
    /// Cached rather than fetched on demand, and that is the whole point of it
    /// being an `Arc<AtomicBool>` on the service side: the sysID burst holds the
    /// service mutex for its entire duration (tens of seconds), so a Stop that
    /// had to take the mutex to find the flag could not raise it until the burst
    /// it is trying to cancel had already finished. Read once, while the arm path
    /// holds the mutex anyway; raised from anywhere afterwards.
    sysid_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// The pump's EXPECTED iteration period in ms, for the stall watchdog.
    /// Estimated at `set_profile`, REFINED by the pump from what the driver
    /// actually programmed.
    expected_iter_ms: Arc<AtomicU64>,
    /// Unix ms of the pump's last completed iteration. `0` = none.
    last_tick_ms: Arc<AtomicU64>,
    /// The rate this profile ASKED the station for, f64 bits; `0` = nothing
    /// configured.
    requested_fs_bits: AtomicU64,
    /// The rate the station actually PROGRAMMED, f64 bits; `0` = not yet known.
    ///
    /// Only the ARM can fill this: `SineService::arm` adopts
    /// `ConfiguredActual::actual_rate_hz` into the service's `sample_rate_hz`,
    /// and before that moment the service's rate is still the request.
    /// Cleared at `set_profile` so a reconfigure can never serve the previous
    /// run's adopted rate.
    actual_fs_bits: Arc<AtomicU64>,
    /// A refusal that killed the run OUTSIDE the service's own state machine - a
    /// failed arm, a panicking pump (the out-of-band fault rule, applied to the
    /// paths the service's state machine cannot latch).
    fault: Arc<Mutex<Option<String>>>,
    /// WHY the run ABORTED, cached the moment it did, for the status path.
    ///
    /// `fill_status` may not take the service mutex (a 27 s sysID burst
    /// or a stalled DAQmx read would wedge every poll behind it), so it cannot ask
    /// `SineService::end_reason()` itself. The pump writes this cache on the way
    /// out and the status path reads it, exactly as `fault` and `bringup_cache`
    /// are written and read.
    ///
    /// Before it existed the wire said `module_state = "aborted"` and NOTHING
    /// else: the trip lived in the service, the reason lived in the event log, and
    /// the journal was the only place either could be seen. That is how the sine
    /// closing sitting's chassis observation - a mid-engaged controller vanish
    /// reaching `aborted` in 8.6 s - was recorded only as a guess about which
    /// protection fired, on a run whose remote operator had already gone.
    abort_detail: Arc<Mutex<Option<String>>>,
    provenance: Mutex<Option<RunProvenance>>,
    /// The daemon's controller timeout, seconds - the budget the safe-stop ramp
    /// has to finish inside. See [`SineHost::set_profile`] for the arithmetic.
    controller_timeout_s: f64,
    data_root: std::path::PathBuf,
    kind: BackendKind,
    ni_devices: Vec<String>,
}

/// Clears [`SineHost::priming`] on EVERY exit path, refusals and panics included.
struct PrimingGuard(Arc<AtomicBool>);

impl Drop for PrimingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl SineHost {
    /// A host bound to a station kind and the shared display bus. Builds nothing
    /// until [`SineHost::set_profile`].
    pub fn new(
        kind: BackendKind,
        ni_devices: Vec<String>,
        data_root: std::path::PathBuf,
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
        controller_timeout_s: f64,
    ) -> Self {
        SineHost {
            svc: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(AtomicU8::new(Phase::Idle as u8)),
            priming: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            waiters: Arc::new(AtomicU64::new(0)),
            tx,
            configured: Arc::new(AtomicBool::new(false)),
            status_cell: Arc::new(Mutex::new(None)),
            bringup_cache: Arc::new(Mutex::new(None)),
            hold: Arc::new(Mutex::new(None)),
            coherence_bits: Arc::new(AtomicU64::new(0)),
            progress_bits: Arc::new(AtomicU64::new(f64::NAN.to_bits())),
            display_dropped: Arc::new(AtomicU64::new(0)),
            sysid_cancel: Mutex::new(None),
            expected_iter_ms: Arc::new(AtomicU64::new(0)),
            last_tick_ms: Arc::new(AtomicU64::new(0)),
            requested_fs_bits: AtomicU64::new(0),
            actual_fs_bits: Arc::new(AtomicU64::new(0)),
            fault: Arc::new(Mutex::new(None)),
            abort_detail: Arc::new(Mutex::new(None)),
            provenance: Mutex::new(None),
            controller_timeout_s,
            data_root,
            kind,
            ni_devices,
        }
    }

    /// The provenance of the configured run, if any.
    pub fn provenance(&self) -> Option<RunProvenance> {
        self.provenance.lock().expect("provenance poisoned").clone()
    }

    fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::SeqCst))
    }

    fn set_phase(&self, p: Phase) {
        self.phase.store(p as u8, Ordering::SeqCst);
    }

    /// Is a run live (driving, or clocked and about to drive) right now?
    ///
    /// LOCK-FREE, off the phase mirror rather than off a `try_lock` on the
    /// service. Random reads a contended mutex as "live", which is right for it
    /// because its pump only holds the mutex while driving. Sine's pump holds it
    /// for the whole sysID burst AND across the gate compute with the device
    /// parked, so "contended" would report a parked run as live. The phase knows
    /// the difference.
    pub fn is_live(&self) -> bool {
        self.phase().is_live()
    }

    /// Is anything configured at all? Lock-free, so a status read never waits on the
    /// service mutex.
    pub fn is_configured(&self) -> bool {
        self.configured.load(Ordering::SeqCst)
    }

    /// Take the service mutex, counting this thread as a parked waiter first so
    /// the pump widens its release window (see [`WAITER_RELEASE`]).
    ///
    /// A BLOCKING lock, deliberately: command dispatch runs on a blocking task
    /// beside the frame forwarder (`server.rs`), so parking here costs one
    /// blocking thread and never a runtime worker - and it is what lets every
    /// intent answer with the value the service actually adopted instead of a
    /// locally recomputed guess.
    fn lock_svc(&self) -> std::sync::MutexGuard<'_, Option<SineService>> {
        self.waiters.fetch_add(1, Ordering::Relaxed);
        let g = self.svc.lock().expect("svc poisoned");
        self.waiters.fetch_sub(1, Ordering::Relaxed);
        g
    }

    fn set_hold(&self, why: impl Into<String>) {
        if let Ok(mut h) = self.hold.lock() {
            *h = Some(why.into());
        }
    }

    fn clear_hold(&self) {
        if let Ok(mut h) = self.hold.lock() {
            *h = None;
        }
    }

    // =================================================================================
    // Configure
    // =================================================================================

    /// Translate the wire profile, derive the station spec from the channel
    /// table, build the station, and construct the service over it.
    ///
    /// **The CROSS-MODULE exclusion is NOT here**, exactly as in the Random and
    /// Explorer hosts: whether some OTHER module owns the core is
    /// `Core::live_module`'s question, asked by the caller. What stays here is the
    /// self-check - only Sine knows what "already live" means for Sine.
    pub fn set_profile(
        &self,
        profile: SineWireProfile,
        table: hertzforge_channel_table::ChannelTable,
        lease_owner: &str,
    ) -> Result<(), ProtocolError> {
        if self.is_live() {
            return Err(ProtocolError::Refused {
                message: "a Sine run is live on this core; Stop it before reconfiguring".into(),
            });
        }

        // ---- the drive row: EXACTLY one (the Explorer's refusal set) -----------
        //
        // Zero and two are different mistakes and get different sentences. Sine
        // derives its output width from the COLA setting and floors at one, so a
        // station with no drive row would build an output frame nothing is bound
        // to; two drive rows with COLA off would leave one silent. Both are
        // decided HERE, where the channel table is in hand.
        let n_drive = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == hertzforge_channel_table::ChannelRole::Drive)
            .count();
        let cola = profile.cola.enabled;
        let want_drive = if cola { 2 } else { 1 };
        if n_drive == 0 {
            return Err(ProtocolError::Refused {
                message: "the channel table has no enabled Drive row: a Sine run synthesizes a \
                          tone and needs an output to put it on. Edit the table \
                          (PutChannelTable) and enable exactly one Drive row (two with COLA)."
                    .into(),
            });
        }
        if n_drive > want_drive {
            return Err(ProtocolError::Refused {
                message: format!(
                    "{n_drive} enabled Drive rows but this profile drives {want_drive} \
                     (COLA {}): the extra output is one the operator configured and nothing \
                     commands. Disable the extra Drive row(s) (PutChannelTable), or enable \
                     COLA if the second output is the reference channel.",
                    if cola { "ON" } else { "OFF" }
                ),
            });
        }
        if cola && n_drive < 2 {
            return Err(ProtocolError::Refused {
                message: "COLA is enabled and the channel table has one Drive row: the COLA \
                          reference is a SECOND drive channel, frequency-locked to the sweep. \
                          Enable a second Drive row (PutChannelTable) or turn COLA off."
                    .into(),
            });
        }
        let inputs: Vec<u32> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != hertzforge_channel_table::ChannelRole::Drive)
            .map(|c| c.index)
            .collect();
        if inputs.is_empty() {
            return Err(ProtocolError::Refused {
                message: "the channel table has no enabled input channel: a closed-loop sine \
                          run has nothing to control to. Edit the table (PutChannelTable) first."
                    .into(),
            });
        }

        // ---- wire -> config (the wire fidelity fence lives in `convert`) -------
        let cfg = convert::config_from_wire(profile)?;

        // ---- THE SAFE-STOP BUDGET ----------------------------------------------
        //
        // A vanished controller is brought down by the RAMPED stop, and the ramp
        // has to finish inside the window the daemon gives it. The arithmetic,
        // stated in the unit the law is now written in (the ramp was converted from
        // `scale *= 0.999` per FRAME - a rate-dependent law wearing a
        // rate-independent constant - to a dB/s law, which is what makes this
        // number computable at all):
        //
        //     t_ramp = 20 * log10(from_scale / RAMP_FLOOR_SCALE) / RAMP_DB_PER_S
        //
        // `from_scale` is the drive-guard ceiling, which is the same argument the
        // service's own `pump_ramp_to_zero` passes - one arithmetic, not two. At
        // the shipped 120 dB/s from a unit ceiling that is 1.0 s, plus the
        // two-phase stop's generation drain:
        //
        //     ramp 1.0 s + drain allowance 2.0 s = 3.0 s
        //     against controller_timeout_s 30 s  -> 27.0 s of margin
        //
        // Both numbers go in the refusal, because "the profile is refused" without
        // them leaves an operator with nothing to change.
        let t_ramp_s = SineProcessStep::safe_stop_seconds(cfg.drive_vpk_ceiling);
        let budget_s = t_ramp_s + DRAIN_ALLOWANCE_S;
        // `!is_finite()` FIRST. Not belt-and-braces as an ORDERING rule even where
        // it is currently unreachable: a NaN makes every comparison false, so a
        // plain `>=` lets the worst case through as "fits". `safe_stop_seconds`
        // clamps a non-finite input to 0.0 today, and this guard must not depend on
        // that staying true.
        if !budget_s.is_finite() || budget_s >= self.controller_timeout_s {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "the safe-stop budget for this profile is {budget_s:.2} s ({t_ramp_s:.2} s \
                     of drive ramp at {} dB/s from the {:.3} drive ceiling, plus {DRAIN_ALLOWANCE_S:.1} s \
                     of generation drain) and this daemon's controller timeout is {:.1} s: a \
                     vanished controller's safe-stop would still be ramping when the run was \
                     supposed to be down. Lower drive_vpk_ceiling, or raise the daemon's \
                     controller_timeout_s above {budget_s:.2} s.",
                    hertzforge_sine::service::engine::RAMP_DB_PER_S,
                    cfg.drive_vpk_ceiling,
                    self.controller_timeout_s
                ),
            });
        }
        if budget_s > 0.5 * self.controller_timeout_s {
            tracing::warn!(
                target: "hertzforge::backendd",
                budget_s = format!("{budget_s:.2}"),
                controller_timeout_s = self.controller_timeout_s,
                "sine safe-stop budget uses more than half the controller-timeout window"
            );
        }

        // ---- table -> StationSpec ----------------------------------------------
        //
        // The block MUST be derived from the rate the station will RUN at, not
        // from the software plant's (`ui/src-tauri/src/sine.rs`'s `sine_station`
        // learned this the expensive way: a block sized for a 16.384 kHz Sim
        // programmed onto a 51.2 kHz chassis makes the tick cadence, the ring
        // depth and the health budget all wrong in TIME). The service re-derives
        // its own block from what `configure()` ANSWERS, so this is the REQUEST -
        // but a request that is three times off makes the first pass wrong for no
        // reason.
        let n_inputs = inputs.len();
        let (phys_in, phys_out) = hertzforge_station::physical_from_table(&table);
        let station_rate = match self.kind {
            BackendKind::Ni => hertzforge_station::NI_BUILD_RATE_HZ,
            _ => SimPlantSpec::default().sample_rate_hz,
        };
        let spec = StationSpec {
            // The HERTZFORGE-SIM half is the LOCAL sine path's plant, transcribed
            // (`sine_sim_plant`): the demo resonance posture the SN4 end-to-end
            // gates were validated against, with `shared_clock` stated because
            // Sine arms as `{HostSynthesized, HostPerTick}` and the arm gate
            // (correctly) refuses per-tick correction without it.
            sim: SimPlantSpec {
                max_input_channels: (n_inputs.max(1)) as u32,
                max_output_channels: if cola { 2 } else { 1 },
                shared_clock: true,
                ..SimPlantSpec::default()
            },
            ni: NiSelection {
                inputs: phys_in,
                outputs: phys_out,
                derive_inputs: n_inputs.max(1),
                // COLA is a SECOND drive channel; the service re-checks the width
                // against `ConfiguredActual` at arm, so this is the request and the
                // answer still governs.
                derive_outputs: if cola { 2 } else { 1 },
                iepe: false,
                restrict_devices: self.ni_devices.clone(),
            },
            ni_block_frames: hertzforge_hal::default_block_frames(station_rate),
        };

        // THE ONE CONSTRUCTION POINT - a refusal is typed all the way to the
        // remote operator, never a Sim substitution.
        let station = try_build_station_backend(self.kind, &spec).map_err(|e| {
            tracing::warn!(target: "hertzforge::backendd", "sine station refused: {e}");
            ProtocolError::Refused {
                message: e.to_string(),
            }
        })?;
        tracing::info!(target: "hertzforge::backendd", "{}", station.provenance.log_line());
        let station_class = station.station;

        let run_id = mint_run_id();
        let prov = RunProvenance::assemble("sine", run_id, &station, lease_owner, now_unix());
        tracing::info!(target: "hertzforge::backendd", "{}", prov.log_line());

        let svc = SineService::new(
            cfg,
            table,
            station.backend,
            station_class,
            SYSID_SEED,
            Some(self.data_root.clone()),
        )
        .map_err(svc_err)?;

        // The REQUEST, taken from the service rather than from the wire: before
        // arm the service's rate is what the backend's caps advertised, which is
        // the number this run asked to be built at.
        self.requested_fs_bits
            .store(svc.sample_rate_hz().to_bits(), Ordering::SeqCst);
        // The station has not adopted yet. Nothing may stand in for it.
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        self.expected_iter_ms.store(
            iter_period_ms(
                svc.sample_rate_hz(),
                hertzforge_hal::default_block_frames(station_rate),
            ),
            Ordering::SeqCst,
        );
        self.coherence_bits.store(0, Ordering::SeqCst);
        // NaN, not zero: 0.0 is a LEGITIMATE progress reading (a run that just
        // started), so the "unknown" sentinel cannot be a value the field can take.
        self.progress_bits
            .store(f64::NAN.to_bits(), Ordering::SeqCst);
        self.display_dropped.store(0, Ordering::SeqCst);
        self.last_tick_ms.store(0, Ordering::Relaxed);
        self.stopping.store(false, Ordering::SeqCst);
        if let Ok(mut f) = self.fault.lock() {
            *f = None;
        }
        if let Ok(mut a) = self.abort_detail.lock() {
            *a = None; // the previous run's abort is not this one's
        }
        if let Ok(mut c) = self.status_cell.lock() {
            *c = None; // a fresh run must never serve the previous run's cell
        }
        if let Ok(mut c) = self.bringup_cache.lock() {
            *c = None;
        }
        if let Ok(mut c) = self.sysid_cancel.lock() {
            *c = None; // the previous run's flag cancels nothing in this one
        }
        self.clear_hold();
        self.set_phase(Phase::Idle);

        *self.svc.lock().expect("svc poisoned") = Some(svc);
        *self.provenance.lock().expect("provenance poisoned") = Some(prov);
        self.configured.store(true, Ordering::SeqCst);
        Ok(())
    }

    // =================================================================================
    // Start + the two releasable parks
    // =================================================================================

    /// Start the configured run.
    ///
    /// The CHEAP refusals are synchronous - not configured, wrong state,
    /// feasibility exceeded, arm failed - so the operator gets an immediate,
    /// specific "no" for everything decidable without running the plant.
    ///
    /// sysID is NOT synchronous, and this is Random's lesson taken as read: the
    /// burst is `~1600 / f_lo` seconds (about 27 s at a 60 Hz low edge, 80 s at
    /// 20 Hz) and the front end waits a fixed 60 s for this call. A command whose
    /// latency scales with the profile's band is a broken contract, so the burst
    /// runs on the pump thread and the PHASE reports its progress.
    pub fn start(&self) -> Result<(), ProtocolError> {
        {
            // The beat is reset BEFORE the flag so the stalled-driver math can
            // never read the PREVIOUS run's heartbeat through the window between
            // the two.
            self.last_tick_ms.store(now_ms(), Ordering::Relaxed);
            self.priming.store(true, Ordering::SeqCst);
            let _priming = PrimingGuard(Arc::clone(&self.priming));
            let mut guard = self.lock_svc();
            let svc = guard.as_mut().ok_or_else(not_configured)?;
            match svc.state() {
                RunState::Idle => {
                    self.set_phase(Phase::Arming);
                    svc.assess_feasibility().map_err(svc_err)?;
                    if matches!(svc.state(), RunState::FeasibilityBlocked) {
                        self.set_phase(Phase::FeasibilityBlocked);
                        let why = "feasibility EXCEEDED: the authored profile is outside the \
                                   selected shaker's ratings. The run is PARKED before it \
                                   drives. Review the profile against the shaker, or release \
                                   the park with Sine(OverrideFeasibility) carrying a \
                                   non-empty acknowledgement."
                            .to_string();
                        self.set_hold(why.clone());
                        self.cache_bringup(svc);
                        return Err(ProtocolError::Refused { message: why });
                    }
                    svc.arm().map_err(|e| {
                        // An arm that refused leaves the service Idle with its
                        // backend consumed; without latching it the wire would go
                        // on calling that "configured" (the out-of-band fault rule
                        // applied to a path the state machine cannot latch).
                        self.set_phase(Phase::Idle);
                        if let Ok(mut f) = self.fault.lock() {
                            *f = Some(format!("arm_refused: {e}"));
                        }
                        svc_err(e)
                    })?;
                    self.adopt_rate(svc);
                    // THE PHASE STAYS `arming` UNTIL THE PUMP ENTERS THE BURST.
                    //
                    // `arm()` leaves the SERVICE at SysId, and the obvious thing is to
                    // mirror that here - which is what this line used to do, and it
                    // was a lie with a consequence. The burst belongs to the pump,
                    // which has not necessarily been SCHEDULED yet, so a controller
                    // that saw `sysid` and sent Stop was stopping something that was
                    // not running. `sysid` now means the AO is driving the burst,
                    // which is what a reader of that token acts on.
                    self.cache_bringup(svc);
                }
                RunState::Engaged | RunState::FullLevel => { /* already engaged */ }
                other => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "Sine cannot start from {other:?}; reconfigure (SetProfile) to reset"
                        ),
                    });
                }
            }
        }
        self.spawn_pump();
        Ok(())
    }

    /// Release the [`RunState::AwaitingReview`] park (`ProceedAfterReview`).
    ///
    /// **The service's refusal is surfaced VERBATIM.** From `CoherenceHold` it
    /// answers with a sentence that says the gate FAILED, that this park has no
    /// release verb, and what to do instead; paraphrasing it here would be a
    /// second copy of a safety statement, and the copy is the one that drifts.
    pub fn proceed_after_review(&self) -> Result<(), ProtocolError> {
        {
            let mut guard = self.lock_svc();
            let svc = guard.as_mut().ok_or_else(not_configured)?;
            svc.proceed_after_review().map_err(svc_err)?;
            self.set_phase(Phase::from_run_state(svc.state()));
            self.take_status_cell(svc);
            self.cache_bringup(svc);
        }
        self.clear_hold();
        // The pump EXITED at the park; engaging again needs it back.
        self.spawn_pump();
        Ok(())
    }

    /// Release the [`RunState::FeasibilityBlocked`] park with an explicit,
    /// non-empty operator acknowledgement (`Sine(OverrideFeasibility)`).
    ///
    /// The ack is the record of WHO decided to drive past a refusal, so an empty
    /// one is refused by the service - a required field nobody has to fill in is
    /// not a gate.
    pub fn override_feasibility(&self, ack: &str) -> Result<(), ProtocolError> {
        {
            self.last_tick_ms.store(now_ms(), Ordering::Relaxed);
            self.priming.store(true, Ordering::SeqCst);
            let _priming = PrimingGuard(Arc::clone(&self.priming));
            let mut guard = self.lock_svc();
            let svc = guard.as_mut().ok_or_else(not_configured)?;
            self.set_phase(Phase::Arming);
            svc.arm_with_override(ack).map_err(|e| {
                // Back to the park: a refused override changed nothing, and
                // leaving the phase at Arming would report a run that is arming
                // when it is still blocked.
                self.set_phase(Phase::FeasibilityBlocked);
                svc_err(e)
            })?;
            self.adopt_rate(svc);
            // `arming` until the pump enters the burst - see `start`.
            self.cache_bringup(svc);
        }
        self.clear_hold();
        self.spawn_pump();
        Ok(())
    }

    /// ARM IS THE ONLY MOMENT THE ACTUAL RATE EXISTS: it adopts
    /// `ConfiguredActual::actual_rate_hz` into the service's own rate. Read
    /// on the success path only - a refused arm leaves the field cleared, which
    /// reports honestly as "not known".
    fn adopt_rate(&self, svc: &SineService) {
        // Cache the sysID cancellation handle while the arm path holds the mutex.
        // From here a Stop / Abort / safe-stop can pre-empt the burst without
        // touching the lock the burst is holding. See [`SineHost::sysid_cancel`].
        let flag = svc.sysid_cancel_flag();
        flag.store(false, Ordering::SeqCst);
        if let Ok(mut c) = self.sysid_cancel.lock() {
            *c = Some(flag);
        }
        let actual = svc.sample_rate_hz();
        self.actual_fs_bits
            .store(actual.to_bits(), Ordering::SeqCst);
        if let Some(c) = svc.configured() {
            self.expected_iter_ms.store(
                iter_period_ms(actual, c.block_frames.max(1)),
                Ordering::SeqCst,
            );
        }
        if svc.rate_was_coerced() {
            let requested = f64::from_bits(self.requested_fs_bits.load(Ordering::SeqCst));
            tracing::warn!(
                target: "hertzforge::backendd",
                requested_fs_hz = requested,
                actual_fs_hz = actual,
                "sine COERCION WARNING: the station programmed a different rate than the run \
                 asked for; every readout is denominated in the ACTUAL"
            );
        }
    }

    /// Snapshot the service status for the phases the [`StatusCell`] cannot cover.
    /// Called at TRANSITIONS, never per tick.
    fn cache_bringup(&self, svc: &SineService) {
        self.coherence_bits
            .store(svc.band_coherence().to_bits(), Ordering::SeqCst);
        if let Ok(mut c) = self.bringup_cache.lock() {
            *c = Some(svc.status());
        }
    }

    /// Cache WHY an aborted run aborted, for the lock-free status path.
    ///
    /// A no-op unless the service is actually `Aborted`, so it is safe to call on
    /// every terminal path without asking first. The string is the service's own
    /// [`SineService::end_reason`] - the sentence the event log and the journal
    /// carry - with the trip's WHERE appended when a control-tick protection left
    /// a record. The engine-level arms (underrun, ring overrun, scheduling fault,
    /// link lost) leave no record and their reason stands alone; that asymmetry is
    /// exactly why the reason, not the record, is the primary source here.
    fn cache_abort_detail(&self, svc: &SineService) {
        if let Some(d) = abort_detail_line(svc) {
            if let Ok(mut a) = self.abort_detail.lock() {
                *a = Some(d);
            }
        }
    }

    /// Take the service's own cell once the step exists.
    fn take_status_cell(&self, svc: &SineService) {
        if let Some(cell) = svc.status_cell() {
            if let Ok(mut slot) = self.status_cell.lock() {
                *slot = Some(cell);
            }
        }
    }

    // =================================================================================
    // Level / frequency / transport intents
    // =================================================================================

    /// Absolute level intent: dB, resolved to the NEAREST ladder rung.
    ///
    /// Returns the rung ACTUALLY applied, never the request. Random refuses an
    /// off-ladder dB by listing the ladder; Sine resolves instead, because its
    /// ladder is a continuum an operator reads off a slider and the service owns
    /// one nearest-rung expression that both the echo and the move read
    /// (`LadderPlan::nearest_rung`). An out-of-table request lands on the end rung
    /// it is nearest, which IS the clamp, expressed by the same arithmetic rather
    /// than by a second branch that could disagree with it.
    pub fn set_level(&self, level: LevelTarget) -> Result<f64, ProtocolError> {
        let db = match level {
            LevelTarget::Db(db) => db,
            LevelTarget::DriveVolts(_) => {
                return Err(ProtocolError::InvalidArgument {
                    message: "Sine levels are dB relative to the full profile; DriveVolts is the \
                              OPEN-LOOP basis and a closed-loop run has no referent for it. The \
                              drive volts a sine run puts out are what the servo decides."
                        .into(),
                })
            }
        };
        let mut guard = self.lock_svc();
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        svc.set_level_db(db).map_err(svc_err)
    }

    /// Relative level intent, dB.
    pub fn adjust_level(&self, delta_db: f64) -> Result<(), ProtocolError> {
        let mut guard = self.lock_svc();
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        svc.step_level(delta_db).map_err(svc_err)
    }

    /// LIVE ladder-mode takeover, both directions.
    ///
    /// Legal before the step exists, which is the point: a MANUAL run is chosen at
    /// Start and the step is not built until the gate releases, so a mode that
    /// could only be posted into the step's mailbox would arrive after the first
    /// rung rule had already been evaluated.
    pub fn set_ladder_mode(&self, ladder: LadderMode) -> Result<(), ProtocolError> {
        let mode = match ladder {
            LadderMode::Auto => RunMode::Auto,
            LadderMode::Manual => RunMode::Manual,
        };
        let mut guard = self.lock_svc();
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        svc.set_run_mode(mode).map_err(svc_err)
    }

    /// Absolute frequency intent. Returns the CLAMPED value the run adopts.
    pub fn go_to_frequency(&self, hz: f64) -> Result<f64, ProtocolError> {
        let guard = self.lock_svc();
        let svc = guard.as_ref().ok_or_else(not_configured)?;
        svc.go_to(hz).map_err(svc_err)
    }

    /// Relative frequency intent. Returns the CLAMPED value the run adopts.
    pub fn adjust_frequency(&self, delta_hz: f64) -> Result<f64, ProtocolError> {
        let guard = self.lock_svc();
        let svc = guard.as_ref().ok_or_else(not_configured)?;
        svc.jog(delta_hz).map_err(svc_err)
    }

    /// PAUSE test progress. Not a stop: the output CONTINUES at the held
    /// frequency and level, which is the whole point - an operator holds a sweep
    /// at the moment they see something, and moving the tone off that spot
    /// destroys what they stopped to look at.
    pub fn hold(&self) -> Result<(), ProtocolError> {
        {
            let guard = self.lock_svc();
            let svc = guard.as_ref().ok_or_else(not_configured)?;
            svc.hold().map_err(svc_err)?;
        }
        self.set_hold(
            "operator hold: the sweep / dwell / rung clocks are paused. The output CONTINUES \
             at the held frequency and level - this is not a stop.",
        );
        Ok(())
    }

    /// Resume test progress from a hold.
    pub fn resume(&self) -> Result<(), ProtocolError> {
        {
            let guard = self.lock_svc();
            let svc = guard.as_ref().ok_or_else(not_configured)?;
            svc.resume().map_err(svc_err)?;
        }
        self.clear_hold();
        Ok(())
    }

    /// Flip the sweep direction mid-pass.
    pub fn reverse(&self) -> Result<(), ProtocolError> {
        let guard = self.lock_svc();
        let svc = guard.as_ref().ok_or_else(not_configured)?;
        svc.reverse().map_err(svc_err)
    }

    /// Set the Hinv-feedforward vs feedback blend weight, live.
    pub fn set_blend_w(&self, w: f64) -> Result<(), ProtocolError> {
        let guard = self.lock_svc();
        let svc = guard.as_ref().ok_or_else(not_configured)?;
        svc.set_w(w).map_err(svc_err)
    }

    // =================================================================================
    // Stop / abort / safe-stop
    // =================================================================================

    /// The controlled stop (the operator's normal end-of-test).
    ///
    /// CONTROLLED: `request_stop` arms the ramp and the LOOP walks the drive to
    /// zero, then the two-phase stop waits for the generation to drain before the
    /// tasks come down. Killing the pump first would leave nobody to walk the
    /// ramp - a lesson from the Random host's audit, and here the device would hold
    /// whatever sample the ramp had reached.
    pub fn stop(&self) -> Result<(), ProtocolError> {
        // PRE-EMPT THE sysID BURST FIRST, before anything takes the service mutex.
        //
        // The burst holds that mutex for its whole duration, so the line below would
        // otherwise block until the thing this call is trying to stop had finished on
        // its own - and a Stop that waits out a 27 s burst is not a stop. Raising the
        // flag is free when no burst is running (nothing reads it) and is the only
        // way in when one is.
        //
        // It is done HERE and not only in `safe_stop` / `abort` so the answer does not
        // depend on which entry point the operator's Stop arrived through. That
        // asymmetry is exactly what made the pump's cancelled-burst verdict flap
        // between runs.
        self.raise_sysid_cancel();
        let driving = {
            let guard = self.lock_svc();
            match guard.as_ref() {
                Some(s) => {
                    let driving = matches!(s.state(), RunState::Engaged | RunState::FullLevel);
                    if driving {
                        s.request_stop().map_err(svc_err)?;
                    }
                    driving
                }
                None => return Ok(()),
            }
        };
        if driving {
            // The watchdog is exempt from here until the pump exits: the drain can
            // block one iteration for up to DRAIN_ALLOWANCE_S and that is not a
            // stall.
            self.stopping.store(true, Ordering::SeqCst);
            self.set_phase(Phase::Stopping);
            self.clear_hold();
            return Ok(());
        }
        // Nothing driving (idle, parked, already stopped): reap the pump. A park
        // that is stopped is STOPPED - the operator asked for the run to end and
        // the fact that it was holding does not make that a no-op.
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        {
            let mut guard = self.lock_svc();
            if let Some(s) = guard.as_mut() {
                if !matches!(s.state(), RunState::Stopped | RunState::Aborted) {
                    s.stop().map_err(svc_err)?;
                }
                self.set_phase(Phase::from_run_state(s.state()));
                self.cache_bringup(s);
                // A run this branch found ALREADY aborted (the pump tripped between
                // the operator pressing Stop and this taking the mutex) still owes
                // the wire its reason.
                self.cache_abort_detail(s);
            }
        }
        self.clear_hold();
        Ok(())
    }

    /// ABORT: `emergency_stop` immediately, no ramp.
    ///
    /// The absence of a ramp is the point. A ramp is the right answer when a run
    /// is ENDING and the only question is how gracefully; an abort is the answer
    /// when something is already wrong, and the backend's own taper (25 ms on the
    /// NI path) is the backstop that keeps it from being a cliff.
    pub fn abort(&self, reason: &str) -> Result<(), ProtocolError> {
        {
            let mut guard = self.lock_svc();
            if let Some(s) = guard.as_mut() {
                if !matches!(s.state(), RunState::Stopped | RunState::Aborted) {
                    s.abort(reason).map_err(svc_err)?;
                }
                self.set_phase(Phase::from_run_state(s.state()));
                self.cache_bringup(s);
                self.cache_abort_detail(s);
            }
        }
        // The sysID burst reads this flag once per BLOCK, so an abort raised while
        // the pump is inside `run_sysid` takes effect within one block instead of
        // waiting out the whole burst.
        self.raise_sysid_cancel();
        // Signal, do NOT join: the pump is at most one bounded batch from noticing
        // the terminal state, and joining here would make an ABORT the one verb
        // that waits on the driver it just told to stop.
        self.stop.store(true, Ordering::SeqCst);
        self.clear_hold();
        Ok(())
    }

    /// THE SAFE-STOP PATH, invoked when the controller times out.
    ///
    /// Same rule as Random's and the Explorer's: "nobody is watching" is a reason
    /// to stop SAFELY - the controlled ramp-down - never to slam the drive. Never
    /// fails.
    ///
    /// **It covers the sysID burst**, which is the case this module has that the
    /// others do not: during sysID the AO is driving at the sysID level, the
    /// service is inside a long blocking call, and a stop that could only be seen
    /// between control ticks would not be seen for the length of the burst. The
    /// cancellation flag is raised here and read once per block inside it.
    pub fn safe_stop(&self, reason: &str) {
        if !self.is_live() {
            return;
        }
        tracing::warn!(
            target: "hertzforge::backendd",
            reason,
            "SAFE-STOP: ramping the Sine run down"
        );
        self.raise_sysid_cancel();
        if let Err(e) = self.stop() {
            tracing::warn!(target: "hertzforge::backendd", "sine safe-stop: {e}");
        }
    }

    /// Drop a BUILT-BUT-IDLE Sine run so this host stops holding the core.
    ///
    /// Called only by [`crate::core::Core::release_idle_claims`], which asks
    /// `holds(Sine, Live)` first. The guard below is still real: the phase mirror is
    /// the authority on what "idle" means for Sine (a parked-but-clocked run reads
    /// LIVE), and a caller that got it wrong must not silently drop a clocked station.
    pub fn release_idle(&self) -> bool {
        if self.is_live() || !self.is_configured() {
            return false;
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        // The drop COMPLETES here, releasing the station's tasks, before the module
        // that displaced this one builds its own.
        *self.svc.lock().expect("svc poisoned") = None;
        self.configured.store(false, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
        self.stopping.store(false, Ordering::SeqCst);
        // Every cache that outlives its service is a status path reporting a run this
        // core can no longer act on - the same list `set_profile` clears.
        if let Ok(mut f) = self.fault.lock() {
            *f = None;
        }
        if let Ok(mut a) = self.abort_detail.lock() {
            *a = None;
        }
        if let Ok(mut c) = self.status_cell.lock() {
            *c = None;
        }
        if let Ok(mut c) = self.bringup_cache.lock() {
            *c = None;
        }
        if let Ok(mut c) = self.sysid_cancel.lock() {
            *c = None;
        }
        if let Ok(mut p) = self.provenance.lock() {
            *p = None;
        }
        self.clear_hold();
        self.set_phase(Phase::Idle);
        self.requested_fs_bits.store(0, Ordering::SeqCst);
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        self.coherence_bits.store(0, Ordering::SeqCst);
        self.display_dropped.store(0, Ordering::SeqCst);
        self.last_tick_ms.store(0, Ordering::Relaxed);
        self.expected_iter_ms.store(0, Ordering::SeqCst);
        true
    }

    /// Raise the service's sysID cancellation flag WITHOUT the service mutex.
    ///
    /// The flag is an `Arc<AtomicBool>` the service hands out precisely so a stop
    /// can be raised from another thread while the pump is inside the burst - the
    /// burst holds the service mutex for its whole duration, so a stop that had to
    /// take that mutex could not be seen until the thing it was cancelling had
    /// already finished. The handle is cached at arm; this raises it and touches
    /// nothing else.
    fn raise_sysid_cancel(&self) {
        if let Some(f) = self.sysid_cancel.lock().ok().and_then(|c| c.clone()) {
            f.store(true, Ordering::SeqCst);
        }
    }

    // =================================================================================
    // Status
    // =================================================================================

    /// Fold this module's truth into a protocol status report.
    ///
    /// **This path never takes the service mutex.** The phase is an atomic, the
    /// rate pair is two atomics, the coherence is an atomic, and the numbers come
    /// from the service's own [`StatusCell`] (which is written by the control tick
    /// under a lock held for three memcpys). A driver stalled inside a DAQmx read
    /// - or a 27 s sysID burst - can never wedge a status poll behind it.
    pub fn fill_status(&self, s: &mut hertzforge_protocol::StatusReport) {
        // BEFORE anything can return early: the rate pair describes a run whose
        // rates are already decided, and a status that drops them while a run arms
        // is exactly the window an operator stares at.
        self.fill_rates(s);
        if !self.is_configured() {
            return;
        }
        // A refusal OUTSIDE the service's own state machine (a failed arm, a
        // panicking pump) - the out-of-band fault rule applied to the paths it
        // cannot latch.
        if let Some(f) = self.fault.lock().ok().and_then(|f| f.clone()) {
            s.run = hertzforge_protocol::RunState::Faulted;
            s.module_state = Some(f);
            return;
        }
        let phase = self.phase();
        s.run = phase.run_state();
        // **AN ABORT NAMES ITSELF.** The token stays the token - consumers
        // split on the first colon (the `feasibility_blocked: <reason>` /
        // `coherence_hold: <reason>` convention this module already publishes on
        // the local path, and the one `shockRemote` documents) - and the tail is
        // the reason the service recorded when it ended the run.
        //
        // The bare token was the whole message until the sine closing sitting:
        // a mid-engaged controller vanish on the chassis reached `aborted` and the
        // only way to learn WHY was `journalctl` on the box. A wire that can say
        // `aborted` but not `aborted: drive underrun ...` sends every remote
        // diagnosis through a shell session the operator may not have.
        let why = self.abort_detail.lock().ok().and_then(|a| a.clone());
        s.module_state = Some(match why {
            Some(why) if phase == Phase::Aborted => format!("{}: {why}", phase.token()),
            _ => phase.token().to_string(),
        });
        s.hold = self.hold.lock().ok().and_then(|h| h.clone());
        let coh = f64::from_bits(self.coherence_bits.load(Ordering::SeqCst));
        if coh.is_finite() && coh > 0.0 {
            s.band_coherence = Some(coh);
        }
        // Schedule progress: finite = the module's own fraction, NaN = no determinable
        // end (a dwell, a step run). Zero is a REAL value here and crosses as one.
        let prog = f64::from_bits(self.progress_bits.load(Ordering::SeqCst));
        if prog.is_finite() {
            s.progress = Some(prog.clamp(0.0, 1.0));
        }
        s.abort_active = phase == Phase::Aborted;
        // Reported unconditionally once a run exists, zero included: on a live run
        // "0 dropped" is a MEASUREMENT, and a count that only reached the daemon's
        // log would say it to nobody at the screen.
        s.display_blocks_dropped = Some(self.display_dropped.load(Ordering::SeqCst));

        // THE STALL WATCHDOG, and it judges ONLY the phases in which the
        // pump iterates. The beat is stored every iteration of the driving loop; a
        // sysID burst or a park does not beat at all, and a threshold applied there
        // would report every healthy bring-up as a dead driver. `stopping` is
        // exempt for the length of the ramp plus the generation drain.
        if phase.is_pumping()
            && !self.stopping.load(Ordering::SeqCst)
            && !self.priming.load(Ordering::SeqCst)
        {
            let beat = self.last_tick_ms.load(Ordering::Relaxed);
            let period = self.expected_iter_ms.load(Ordering::SeqCst).max(1);
            let stall_after = (3 * period).max(3_000);
            let stale_ms = now_ms().saturating_sub(beat);
            if beat != 0 && stale_ms > stall_after {
                s.run = hertzforge_protocol::RunState::Faulted;
                s.module_state = Some(format!(
                    "stalled_driver ({:.1} s without a pump iteration, expected one every \
                     {:.3} s - the NI runtime is not returning; restart the daemon, and the \
                     chassis if preflight then fails)",
                    stale_ms as f64 / 1e3,
                    period as f64 / 1e3
                ));
                return;
            }
        }

        // THE NUMBERS. The engaged phases read the service's own cell (at most one
        // control tick old); every other phase reads the transition snapshot,
        // because the cell does not exist yet.
        if phase.is_pumping() {
            if let Some(cell) = self.status_cell.lock().ok().and_then(|c| c.clone()) {
                apply_control_snapshot(&cell.read(), s);
                return;
            }
        }
        if let Some(st) = self.bringup_cache.lock().ok().and_then(|c| c.clone()) {
            apply_service_status(&st, s);
        }
    }

    /// The requested/actual sample-rate pair, lock-free. `0` bits = unknown, which
    /// crosses the wire as `None` rather than as a number.
    fn fill_rates(&self, s: &mut hertzforge_protocol::StatusReport) {
        let read = |bits: &AtomicU64| -> Option<f64> {
            let v = f64::from_bits(bits.load(Ordering::SeqCst));
            (v.is_finite() && v > 0.0).then_some(v)
        };
        s.requested_fs_hz = read(&self.requested_fs_bits);
        s.actual_fs_hz = read(&self.actual_fs_bits);
    }
}

// =====================================================================================
// The pump
// =====================================================================================

/// Everything the pump thread owns.
struct PumpCtx {
    svc: Arc<Mutex<Option<SineService>>>,
    stop: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    priming: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    waiters: Arc<AtomicU64>,
    beat: Arc<AtomicU64>,
    coherence: Arc<AtomicU64>,
    progress: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    bringup: Arc<Mutex<Option<hertzforge_sine::SineStatus>>>,
    hold: Arc<Mutex<Option<String>>>,
    fault: Arc<Mutex<Option<String>>>,
    /// The abort's WHY, cached for the status path. The pump is the writer
    /// on the ordinary abort path - it is the thread that sees the run end.
    abort_detail: Arc<Mutex<Option<String>>>,
    cell: Arc<Mutex<Option<Arc<StatusCell>>>>,
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    run_id: u64,
}

impl SineHost {
    /// Spawn (or RE-spawn) the pump.
    ///
    /// Re-spawnable BY DESIGN: this is what makes the park model work. The pump
    /// exits at `AwaitingReview` and at `FeasibilityBlocked`, and the verb that
    /// releases each one calls this again for the driving phase. A pump that ran
    /// through a park would be holding the mutex the release verb needs.
    fn spawn_pump(&self) {
        let mut slot = self.pump.lock().expect("pump poisoned");
        // A FINISHED handle is not a running pump (found live on the Random
        // path 2026-08-01): a run that ends on its own leaves its `JoinHandle` in
        // the slot, and a spawn that read that as "already pumping" returned - the
        // service engaged, the servo answered, and not one loop iterated. Reap a
        // finished handle and spawn; only a LIVE pump short-circuits. This host
        // re-spawns at two PARKS as well, so the hazard is not hypothetical here.
        if let Some(h) = slot.take() {
            if h.is_finished() {
                let _ = h.join();
            } else {
                *slot = Some(h);
                return; // genuinely already pumping
            }
        }
        self.stop.store(false, Ordering::SeqCst);
        let run_id = self
            .provenance()
            .and_then(|p| p.run_id.parse::<u64>().ok())
            .unwrap_or(0);
        let ctx = PumpCtx {
            svc: Arc::clone(&self.svc),
            stop: Arc::clone(&self.stop),
            phase: Arc::clone(&self.phase),
            priming: Arc::clone(&self.priming),
            stopping: Arc::clone(&self.stopping),
            waiters: Arc::clone(&self.waiters),
            beat: Arc::clone(&self.last_tick_ms),
            coherence: Arc::clone(&self.coherence_bits),
            progress: Arc::clone(&self.progress_bits),
            dropped: Arc::clone(&self.display_dropped),
            bringup: Arc::clone(&self.bringup_cache),
            hold: Arc::clone(&self.hold),
            fault: Arc::clone(&self.fault),
            abort_detail: Arc::clone(&self.abort_detail),
            cell: Arc::clone(&self.status_cell),
            tx: self.tx.clone(),
            run_id,
        };
        *slot = Some(std::thread::spawn(move || pump_body(ctx)));
    }
}

/// The pump: run the sysID burst and the gate if they are still owed, then batch
/// and publish until the run ends or PARKS.
fn pump_body(ctx: PumpCtx) {
    // ---- THE BRING-UP HALF (sysID + gate), when it is still owed --------------
    //
    // `start` armed synchronously and left the service at `SysId`; the burst runs
    // HERE because its duration scales with the profile's low band edge
    // (`~1600 / f_lo` s) and no command may block for that long. A pump respawned
    // by a park-release verb finds the service already past this and skips it.
    //
    // ANSWERS with what the driving loop below needs, rather than assigning into
    // pre-declared slots: every failure path here `return`s, so the loop can only
    // ever observe values this block actually produced - the Explorer's shape, and
    // the reason its equivalent block is an expression too.
    let (mut tap, block_frames, mut last_dropped) = {
        let mut guard = match ctx.svc.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(s) = guard.as_mut() else { return };
        if matches!(s.state(), RunState::SysId) {
            // The PrimingGuard covers arm-through-sysID-START and no further. It
            // is dropped BEFORE the burst, because from the moment the burst is
            // running the PHASE says `sysid` and that is a better answer than
            // "priming" for the next 27 seconds; and it is dropped well before the
            // gate decision, so a park is never reported as a bring-up in flight.
            {
                let _priming = PrimingGuard(Arc::clone(&ctx.priming));
                ctx.phase.store(Phase::SysId as u8, Ordering::SeqCst);
            }
            if let Err(e) = s.run_sysid(0) {
                // A CANCELLED BURST IS NOT A FAULT.
                //
                // The cancellation flag is how a Stop / Abort / safe-stop reaches a
                // call that holds the service mutex for its whole duration, so the
                // burst returning `Err` is the NORMAL outcome of the operator
                // pressing Stop - and latching it as a fault told them, on the wire,
                // that something had broken. MEASURED, not reasoned: the wire probe's
                // Stop-during-sysID row reported
                // `sysid_failed: sysID burst CANCELLED after 23292 of 2097152 frames`
                // in the FAULTED state on its first green run, which is a correct
                // sentence attached to the wrong verdict.
                //
                // THE DISCRIMINATOR IS THE CANCEL FLAG, and getting this wrong once
                // is why the reasoning is spelled out. The first fix keyed on
                // `ctx.stop`, which LOSES THE RACE by construction: every stop path
                // raises the cancel flag and THEN takes the service mutex to set
                // `ctx.stop` - and the pump is holding that mutex for the whole
                // burst, so by the time `ctx.stop` is set the pump has already
                // observed the `Err` and decided. The probe caught it a second time
                // (`run='faulted'` again); the first run had PASSED only because that
                // burst completed instead of cancelling, which is the same Sim race
                // that makes the in-tree test non-discriminating.
                //
                // The cancel flag is raised BEFORE any lock is taken, by exactly the
                // paths that ask this burst to end and by nothing else. An `Err` with
                // it set is an ANSWERED REQUEST; an `Err` without it is a real
                // failure. `ctx.stop` stays in the condition as a belt for a stop
                // that somehow reached the flag-free path.
                let cancelled = s.sysid_cancel_flag().load(Ordering::SeqCst);
                if cancelled || ctx.stop.load(Ordering::SeqCst) {
                    tracing::info!(
                        target: "hertzforge::backendd",
                        "sine sysID CANCELLED by request (not a fault): {e}"
                    );
                    let _ = s.stop();
                } else {
                    tracing::warn!(target: "hertzforge::backendd", "sine sysID failed: {e}");
                    let _ = s.abort(&format!("sysID failed: {e}"));
                    if let Ok(mut f) = ctx.fault.lock() {
                        *f = Some(format!("sysid_failed: {e}"));
                    }
                }
                ctx.phase
                    .store(Phase::from_run_state(s.state()) as u8, Ordering::SeqCst);
                publish_bringup(&ctx, s);
                return;
            }
            ctx.phase.store(Phase::Gating as u8, Ordering::SeqCst);
            publish_bringup(&ctx, s);

            let gate = s.coherence_gate();
            ctx.coherence
                .store(s.band_coherence().to_bits(), Ordering::SeqCst);
            // THE sysID PAIR, published on PASS **and** on FAIL. A held gate is
            // exactly when an operator most needs to see the |H| and the coherence
            // that held it; publishing only on the happy path would hide the curve
            // at the one moment it is the whole diagnosis.
            publish_sysid_pair(&ctx, s);
            match gate {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(target: "hertzforge::backendd", "sine coherence gate: {e}");
                    if let Ok(mut f) = ctx.fault.lock() {
                        *f = Some(format!("coherence_gate_failed: {e}"));
                    }
                }
            }
            let state = s.state();
            ctx.phase
                .store(Phase::from_run_state(state) as u8, Ordering::SeqCst);
            publish_bringup(&ctx, s);
            match state {
                // THE TERMINAL PARK. The gate FAILED. No verb on this seam clears
                // it and `proceed_after_review` refuses it by name - only a
                // SetProfile (which rebuilds the service) does. The pump exits and
                // nothing respawns it.
                RunState::CoherenceHold => {
                    set_hold(
                        &ctx,
                        format!(
                            "COHERENCE HOLD: the sysID gate FAILED at band coherence {:.4} \
                             (floor {:.3}). The run is parked and will NOT auto-raise level. \
                             This park has no release verb, deliberately: it is a safety \
                             refusal, not a pause. Improve the setup and send SetProfile to \
                             reset.",
                            s.band_coherence(),
                            s.config().coherence_floor
                        ),
                    );
                    tracing::warn!(
                        target: "hertzforge::backendd",
                        coherence = s.band_coherence(),
                        floor = s.config().coherence_floor,
                        "sine parked at COHERENCE HOLD; the pump is exiting and no verb respawns it"
                    );
                    return;
                }
                // THE RELEASABLE PARK. The gate PASSED and the run is holding for
                // a human. `ProceedAfterReview` respawns this thread.
                RunState::AwaitingReview => {
                    set_hold(
                        &ctx,
                        format!(
                            "REVIEW HOLD: the sysID gate PASSED at band coherence {:.4} (floor \
                             {:.3}). The |H| and coherence are published; the loop is NOT \
                             closed and the drive is at zero with the tasks stopped until \
                             ProceedAfterReview.",
                            s.band_coherence(),
                            s.config().coherence_floor
                        ),
                    );
                    return;
                }
                RunState::Engaged | RunState::FullLevel => {}
                other => {
                    tracing::warn!(
                        target: "hertzforge::backendd",
                        state = format!("{other:?}"),
                        "sine bring-up ended without engaging"
                    );
                    return;
                }
            }
        }
        // Engaged from here (either straight through the gate or via a release
        // verb): the step exists, so the service's own cell does too.
        if let Some(cell) = s.status_cell() {
            if let Ok(mut c) = ctx.cell.lock() {
                *c = Some(cell);
            }
        }
        // THE DISPLAY GRID, denominated in the ADOPTED rate. The 9234 coerces a
        // requested 8192 up to 8533.33, and a time base built on the REQUEST would be
        // wrong by that ratio while looking entirely normal.
        (
            TimeTap::new(ctx.run_id, s.sample_rate_hz()),
            s.configured().map(|c| c.block_frames.max(1)).unwrap_or(1) as u64,
            s.blocks_dropped(),
        )
    };
    ctx.beat.store(now_ms(), Ordering::Relaxed);

    // ---- THE DRIVING LOOP ------------------------------------------------------
    let mut iters: u64 = 0;
    let mut window_seq: u32 = 0;
    let mut last_logged_warning: Option<String> = None;
    let mut log_len: usize = 0;
    // Start DUE, so the first completed batch publishes rather than waiting out a
    // display period on a pane that currently has nothing on it.
    let mut last_publish = Instant::now()
        .checked_sub(DISPLAY_PERIOD)
        .unwrap_or_else(Instant::now);

    while !ctx.stop.load(Ordering::Relaxed) && iters < MAX_PUMP_ITERS {
        // PRODUCER-SIDE decimation, decided BEFORE the lock so a pass that is not due
        // to publish never pays for building a snapshot it would have thrown away.
        let due = last_publish.elapsed() >= DISPLAY_PERIOD;
        let outcome = {
            let mut guard = match ctx.svc.lock() {
                Ok(g) => g,
                Err(_) => break,
            };
            let Some(s) = guard.as_mut() else { break };
            if !matches!(s.state(), RunState::Engaged | RunState::FullLevel) {
                break;
            }
            match s.run_batches(BATCHES_PER_ITER) {
                Ok(alive) => {
                    ctx.phase
                        .store(Phase::from_run_state(s.state()) as u8, Ordering::SeqCst);
                    // THE RING-DROP HOLE, read here rather than off a later status, so
                    // it is attributed to the window it happened in. `run_batches` is
                    // what pushes blocks into the ring and therefore what overflows it;
                    // the drain is the next line. The clock advances PAST the hole, so
                    // the loss surfaces as a gap in the time pane rather than as
                    // silently compressed time.
                    // The SCHEDULE fraction, mirrored while the mutex is already
                    // held. The StatusCell carries the control tick's scalars, not the
                    // scheduler's, so without this mirror the wire could only carry
                    // progress at bring-up transitions - never during the run, which
                    // is the only time it is worth anything.
                    store_progress(&ctx, s);
                    let dropped_now = s.blocks_dropped();
                    if dropped_now > last_dropped {
                        let lost = dropped_now
                            .saturating_sub(last_dropped)
                            .saturating_mul(block_frames);
                        last_dropped = dropped_now;
                        tap.skip(lost);
                    }
                    // THE TIME-DOMAIN TAP. The blocks the drain hands over are packed
                    // on the way past, so the time panes advance on the acquisition
                    // beat instead of waiting for a display window. This is the drain
                    // the raw ring exists for - without it `service_analysis_tapped`
                    // has nothing to feed and the ring just overflows.
                    s.service_analysis_tapped(|blk, stride| tap.push(blk, stride));
                    // SCALARS ride the StatusCell (the status path stays lock-free);
                    // TRACES ride this, and ONLY when a publish is due. No ReportData
                    // is constructed at batch cadence - that is the whole point of the
                    // accessor.
                    let snap = due.then(|| s.display_snapshot());
                    let fresh = fresh_driver_warnings(s, &mut last_logged_warning, &mut log_len);
                    Some((alive, snap, fresh, dropped_now))
                }
                Err(e) => {
                    // The loop is dead - SAY SO on the wire. Before this
                    // rule existed the state stayed `engaged` after the thread
                    // exited, and a run that drove nothing and streamed nothing
                    // reported itself healthy for the rest of the session.
                    tracing::warn!(target: "hertzforge::backendd", "sine loop faulted: {e}");
                    // TAPPED like the healthy path: this pass drains the ring, so an
                    // untapped one here would swallow the last samples before the
                    // fault - exactly the ones an operator scrolls back to look at.
                    s.service_analysis_tapped(|blk, stride| tap.push(blk, stride));
                    let _ = s.abort(&format!("loop fault: {e}"));
                    ctx.phase
                        .store(Phase::from_run_state(s.state()) as u8, Ordering::SeqCst);
                    if let Ok(mut f) = ctx.fault.lock() {
                        *f = Some(format!("loop_faulted: {e}"));
                    }
                    None
                }
            }
        };
        // THE TIME BATCHES, sent with the service mutex RELEASED: packing happened
        // inside the pass, sending must not lengthen the hold.
        tap.publish(&ctx.tx);
        let Some((alive, snap, fresh, dropped_now)) = outcome else {
            break;
        };
        ctx.dropped.store(dropped_now, Ordering::SeqCst);
        for w in fresh {
            tracing::warn!(target: "hertzforge::backendd", "sine DRIVER diagnostic: {w}");
        }
        if let Some(snap) = snap {
            last_publish = Instant::now();
            window_seq = window_seq.wrapping_add(1);
            if let Some(b) = sweep_batch(&snap, window_seq, ctx.run_id) {
                let _ = ctx.tx.send(DisplayBatch::Spectrum(b));
            }
        }
        // THE HEARTBEAT, every iteration. A quiet DAQ read is not a stall,
        // and a beat that only fired on a "productive" iteration would make an
        // idle-but-healthy loop indistinguishable from a dead driver.
        ctx.beat.store(now_ms(), Ordering::Relaxed);
        iters += 1;
        if !alive {
            break;
        }
        std::thread::sleep(if ctx.waiters.load(Ordering::Relaxed) > 0 {
            WAITER_RELEASE
        } else {
            RELEASE_FLOOR
        });
    }

    // ---- THE FINAL PUBLISH, on EVERY exit path --------------------------------
    //
    // The run can be ended from OUTSIDE this thread: `abort` calls the service's
    // abort directly and raises the stop flag (an abort that waited for the pump
    // to notice would not be an abort), which ends the loop at its CONDITION,
    // before any in-loop publish. Leaving without this strands the status on the
    // last RUNNING snapshot and the status path then watches a heartbeat that has
    // stopped and reports `stalled_driver` for a run that ended cleanly - measured
    // on the Explorer path, not reasoned.
    if let Ok(mut g) = ctx.svc.lock() {
        if let Some(s) = g.as_mut() {
            ctx.phase
                .store(Phase::from_run_state(s.state()) as u8, Ordering::SeqCst);
            publish_bringup(&ctx, s);
            // The same pass drains the last blocks the ring holds; tapping it means
            // the time pane's final samples are the ones that were actually acquired
            // last, rather than whatever happened to clear the span floor.
            s.service_analysis_tapped(|blk, stride| tap.push(blk, stride));
            // ONE final trace batch: a measurement the operator took and never saw is
            // the same as one that was never taken.
            window_seq = window_seq.wrapping_add(1);
            if let Some(b) = sweep_batch(&s.display_snapshot(), window_seq, ctx.run_id) {
                let _ = ctx.tx.send(DisplayBatch::Spectrum(b));
            }
            // WHY did the loop end? An abort's trip record used to live only in the
            // in-memory event log, so a tripped run read as an unexplained thread
            // exit in the journal (the Random path's 2026-08-03 lesson: three
            // deploy cycles blind).
            if matches!(s.state(), RunState::Aborted) {
                // The SAME tail the journal gets, cached for the wire. Until
                // this line existed the journal was the ONLY place an abort could be
                // explained, and a remote operator whose controller had gone (the
                // one case where an abort is most likely and least watched) had
                // nothing but the word `aborted`.
                cache_abort_detail(&ctx, s);
                tracing::warn!(
                    target: "hertzforge::backendd",
                    trip = format!("{:?}", s.trip()),
                    "sine loop ended ABORTED"
                );
                for ev in s.log().events().iter().rev().take(6) {
                    tracing::warn!(target: "hertzforge::backendd", "  abort-tail event: {ev:?}");
                }
            }
        }
    }
    tap.flush();
    tap.publish(&ctx.tx);
    ctx.stopping.store(false, Ordering::SeqCst);
    tracing::info!(target: "hertzforge::backendd", iters, "sine pump thread exited");
}

fn set_hold(ctx: &PumpCtx, why: String) {
    if let Ok(mut h) = ctx.hold.lock() {
        *h = Some(why);
    }
}

/// Mirror the service's schedule-progress fraction into the lock-free status path.
/// `None` stores NaN, which `fill_status` reads back as an absent wire field.
fn store_progress(ctx: &PumpCtx, s: &SineService) {
    ctx.progress
        .store(s.progress().unwrap_or(f64::NAN).to_bits(), Ordering::SeqCst);
}

/// Refresh the host-side bring-up snapshot from the service. Called at
/// TRANSITIONS only - the engaged phases read the service's own cell instead.
fn publish_bringup(ctx: &PumpCtx, s: &SineService) {
    ctx.coherence
        .store(s.band_coherence().to_bits(), Ordering::SeqCst);
    store_progress(ctx, s);
    if let Ok(mut c) = ctx.bringup.lock() {
        *c = Some(s.status());
    }
}

/// DRIVER diagnostics the service drained must reach the journal, or the first
/// anyone hears of an underrun is a symptom.
///
/// Tracked by CONTENT, not by a count, and the reason is the Explorer's: a count
/// watermark stops advancing the moment the source buffer saturates and silently
/// stops logging - precisely when the driver is talking most. `hertzforge-sine` folds
/// driver warnings into its event log as `driver: ...` notes; the log is
/// append-only today, but a watermark that depends on that staying true is the
/// same latent fault in a new place.
fn fresh_driver_warnings(
    s: &SineService,
    last: &mut Option<String>,
    log_len: &mut usize,
) -> Vec<String> {
    let events = s.log().events();
    if events.len() == *log_len {
        return Vec::new(); // nothing was appended; do not re-scan
    }
    *log_len = events.len();
    let all: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.kind {
            hertzforge_sine::EventKind::Note { text } if text.starts_with("driver: ") => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    let fresh: Vec<String> = match last.as_ref() {
        Some(prev) => match all.iter().rposition(|w| w == prev) {
            Some(i) => all[i + 1..].to_vec(),
            // The watermark itself is gone: everything held is new to the journal,
            // and some were lost before we saw them.
            None => all.clone(),
        },
        None => all.clone(),
    };
    if let Some(l) = fresh.last() {
        *last = Some(l.clone());
    }
    fresh
}

/// One display batch from a [`SineDisplaySnapshot`]: the sweep overlays on their
/// reserved ids, plus the per-channel tracked / broadband curves on their REAL
/// table indices.
///
/// The snapshot arrives already rastered by `hertzforge-sine` - one grid function shared
/// with the local Tauri packer - so this is a MAPPING from products to ids and
/// nothing more. That split is deliberate: the arithmetic that decides what a curve
/// looks like belongs beside the run, and the id census belongs beside the wire.
///
/// `None` when the raster is not usable, which is a configured-but-never-armed run.
/// A batch with a zero `df_hz` would put every trace on one frequency.
fn sweep_batch(
    snap: &hertzforge_sine::SineDisplaySnapshot,
    window_seq: u32,
    run_id: u64,
) -> Option<SpectrumBatch> {
    if !(snap.df_hz.is_finite() && snap.df_hz > 0.0) {
        return None;
    }
    let mut traces: Vec<(u32, Option<u32>, Arc<Vec<f32>>)> = Vec::new();
    let mut push = |ch: u32, v: &[f32]| {
        if !v.is_empty() {
            traces.push((ch, None, Arc::new(v.to_vec())));
        }
    };
    push(CH_PROFILE, &snap.profile);
    push(CH_ALARM_HI, &snap.alarm_hi);
    push(CH_ALARM_LO, &snap.alarm_lo);
    push(CH_ABORT_HI, &snap.abort_hi);
    push(CH_ABORT_LO, &snap.abort_lo);
    // EMPTY until a sysID has run, and empty means NOT PUBLISHED: a flat zero curve
    // on the drive id reads as "the drive is dead" rather than as "there is no plant
    // estimate yet".
    push(CH_DRIVE, &snap.drive);
    push(CH_COMPOSITE, &snap.composite);
    for c in &snap.channels {
        push(c.channel_index, &c.tracked);
        push(c.channel_index + BROADBAND_OFFSET, &c.broadband);
    }
    // THD and COLA are live SCALARS, not swept logs, so they ride the whole raster as
    // flat markers - the local packer's shape, kept so the two paths draw the same
    // thing. THD crosses as a PERCENT here, matching the status field's unit.
    let n = snap.profile.len().max(snap.composite.len());
    if let Some(thd) = snap.thd {
        if n > 0 {
            traces.push((CH_THD, None, Arc::new(vec![(thd * 100.0) as f32; n])));
        }
    }
    if let Some(cola) = snap.cola {
        if n > 0 {
            traces.push((CH_COLA, None, Arc::new(vec![cola as f32; n])));
        }
    }
    if traces.is_empty() {
        return None;
    }
    Some(SpectrumBatch {
        f0_hz: snap.f0_hz,
        df_hz: snap.df_hz,
        window_seq,
        run_id,
        traces,
    })
}

/// The sysID `|H1|` and coherence, on their reserved ids, at the gate.
///
/// Published on PASS and on FAIL alike. The grid is the sysID's own frequency
/// vector, which is uniform by construction, so `(f0, df)` reconstructs it exactly;
/// a non-uniform vector would be skipped rather than drawn against frequencies it
/// was not measured at.
fn publish_sysid_pair(ctx: &PumpCtx, s: &SineService) {
    let Some(frf) = s.frf() else { return };
    if frf.freqs.len() < 2 || frf.h_mag.len() != frf.freqs.len() {
        return;
    }
    let f0 = frf.freqs[0];
    let df = frf.freqs[1] - f0;
    if !(df.is_finite() && df > 0.0) {
        tracing::warn!(
            target: "hertzforge::backendd",
            "sine sysID grid is not uniform; the |H| / coherence pair is NOT published (a \
             curve drawn against the wrong frequencies is worse than no curve)"
        );
        return;
    }
    let mut traces: Vec<(u32, Option<u32>, Arc<Vec<f32>>)> = Vec::with_capacity(2);
    traces.push((
        CH_SYSID_H,
        None,
        Arc::new(frf.h_mag.iter().map(|&v| v as f32).collect::<Vec<f32>>()),
    ));
    if frf.coherence.len() == frf.freqs.len() {
        traces.push((
            CH_SYSID_COH,
            None,
            Arc::new(
                frf.coherence
                    .iter()
                    .map(|&v| v as f32)
                    .collect::<Vec<f32>>(),
            ),
        ));
    }
    let _ = ctx.tx.send(DisplayBatch::Spectrum(SpectrumBatch {
        f0_hz: f0,
        df_hz: df,
        window_seq: 0,
        run_id: ctx.run_id,
        traces,
    }));
}

/// The sysID PRNG seed, transcribed from the local host (`ui/src-tauri/src/sine.rs`).
///
/// A CONTROL-side determinism knob, deliberately distinct from the Sim plant's noise
/// seed: the two used to ride the same field, which coupled a control-repeatability
/// choice to a plant detail for no reason.
const SYSID_SEED: u64 = 0xE16E_5148_0BAD_5EED;

/// Map one CONTROL-tick snapshot onto the wire report (the engaged phases).
fn apply_control_snapshot(
    c: &hertzforge_sine::service::engine::ControlSnapshot,
    s: &mut hertzforge_protocol::StatusReport,
) {
    s.frequency_hz = Some(c.freq_hz);
    s.level_db = Some(c.level_db);
    s.sweeps_done = Some(c.sweeps_done);
    s.elapsed_s = Some(c.elapsed_test_s);
    s.alarm_active = c.alarm_active;
    s.notch_active = Some(c.notch_binding);
    // THE CLOSED-LOOP PAIR (v6). The cell has carried both since the service landed and
    // neither crossed the wire, so the remote run rail's TARGET and MEASURED rows were
    // blank for the whole of every run - the two numbers that say whether the loop is
    // holding. Non-finite is dropped rather than sent as NaN: `None` renders as a dash,
    // which is what "not measured" looks like; NaN renders as whatever the formatter
    // does with it.
    if c.target_g.is_finite() {
        s.target_g = Some(c.target_g);
    }
    if c.measured_g.is_finite() {
        s.measured_g = Some(c.measured_g);
    }
    // THD crosses as a PERCENT (the wire field's unit); the core carries a
    // fraction. Converted at the one place that knows both, never twice.
    if c.thd.is_finite() {
        s.thd_pct = Some(c.thd * 100.0);
    }
}

/// Map one SERVICE status onto the wire report (the bring-up phases and the
/// terminal ones, where no control tick has written a cell).
/// The one-line WHY for an aborted run, or `None` when the run is not aborted.
///
/// The service's own [`SineService::end_reason`] is the primary source
/// because it is the only one that covers every terminal path: a control-tick
/// protection leaves a [`hertzforge_sine::TripRecord`], but the engine-level arms
/// (scheduling fault, input stall, device link lost, drive underrun, ring overrun,
/// backend/capture fatal) and the pump-panic arm leave a reason string and no
/// record - and those are precisely the classes a real chassis produces and an
/// HERTZFORGE-SIM does not. The trip's WHERE (frequency, level, tick) is appended when
/// there is a record, because "which protection" without "at what frequency and
/// level" still sends the reader to the journal.
fn abort_detail_line(svc: &SineService) -> Option<String> {
    if !matches!(svc.state(), RunState::Aborted) {
        return None;
    }
    let mut detail = svc
        .end_reason()
        .unwrap_or("aborted with no reason recorded")
        .to_string();
    if let Some(r) = svc.trip() {
        detail.push_str(&format!(
            " @ {:.2} Hz, {:.1} dB, tick {}",
            r.at_freq_hz, r.at_level_db, r.at_tick
        ));
    }
    // ONE LINE on the wire: `module_state` is rendered inline by every consumer
    // (the panel's state chip, the probe's row detail, the daemon's log line), and
    // a newline in it turns a chip into a paragraph.
    Some(detail.replace(['\n', '\r'], " "))
}

/// The pump's half of [`SineHost::cache_abort_detail`] - same cache, reached
/// through [`PumpCtx`] because the pump is a free function and does not hold the
/// host.
fn cache_abort_detail(ctx: &PumpCtx, svc: &SineService) {
    if let Some(d) = abort_detail_line(svc) {
        if let Ok(mut a) = ctx.abort_detail.lock() {
            *a = Some(d);
        }
    }
}

fn apply_service_status(
    st: &hertzforge_sine::SineStatus,
    s: &mut hertzforge_protocol::StatusReport,
) {
    s.frequency_hz = Some(st.freq_hz);
    s.level_db = Some(st.level_db);
    s.sweeps_done = Some(st.sweeps_done);
    s.elapsed_s = Some(st.elapsed_test_s);
    s.remaining_s = st.remaining_test_s;
    s.alarm_active = st.alarm_active;
    s.notch_active = Some(st.notch_binding);
    // The same v6 pair from the bring-up/terminal side. `measured_g` is already an
    // Option here - the service says "not measured yet" rather than inventing a zero -
    // and that distinction is preserved all the way to the dash on the rail.
    if st.target_g.is_finite() {
        s.target_g = Some(st.target_g);
    }
    if let Some(m) = st.measured_g.filter(|v| v.is_finite()) {
        s.measured_g = Some(m);
    }
    if let Some(thd) = st.thd {
        s.thd_pct = Some(thd * 100.0);
    }
    if st.band_coherence.is_finite() && st.band_coherence > 0.0 {
        s.band_coherence = Some(st.band_coherence);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn not_configured() -> ProtocolError {
    ProtocolError::Refused {
        message: "Sine is not configured on this core; send SetProfile first".into(),
    }
}

fn svc_err(e: hertzforge_sine::ServiceError) -> ProtocolError {
    ProtocolError::Refused {
        message: e.to_string(),
    }
}

/// The expected pump-iteration period in ms: one iteration is
/// `BATCHES_PER_ITER x ticks-per-batch` blocks of acquisition.
fn iter_period_ms(fs_hz: f64, block_frames: usize) -> u64 {
    if !(fs_hz.is_finite() && fs_hz > 0.0) {
        return 0;
    }
    let blocks = BATCHES_PER_ITER as f64 * ASSUMED_TICKS_PER_BATCH as f64;
    ((blocks * block_frames as f64 / fs_hz) * 1e3)
        .round()
        .max(0.0) as u64
}

impl Drop for SineHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.raise_sysid_cancel();
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_channel_table::ChannelTable;
    use std::time::Instant;

    /// Three inputs and ONE Drive row - the shape a real operator (or the wire
    /// probe) has to `PutChannelTable` before configuring. The daemon's shipped
    /// seed table has no Drive row at all, which this host refuses by name.
    fn table() -> ChannelTable {
        let mut t = ChannelTable::default_with_channels(4);
        t.channels[3].role = hertzforge_channel_table::ChannelRole::Drive;
        t.channels[3].units = hertzforge_channel_table::Units::VoltageV;
        t
    }

    /// A minimal but VALID v5 sine profile, with the review hold OFF.
    ///
    /// Off because these tests exercise the DRIVING path and the hold's own
    /// behaviour (and its default-ON policy) is covered where the state machine
    /// lives - plus, in this file, by `a_passing_gate_parks_at_awaiting_review`,
    /// which turns it back on precisely to test it.
    fn profile() -> SineWireProfile {
        use hertzforge_protocol::*;
        SineWireProfile {
            schema_version: PROFILE_SCHEMA,
            profile: SineControlProfile {
                label: "host test".into(),
                rows: vec![
                    SineProfileRow {
                        freq_hz: 60.0,
                        quantity: SineQuantity::AccelGPk,
                        value: 0.2,
                        alarm_db: None,
                        abort_db: None,
                    },
                    SineProfileRow {
                        freq_hz: 500.0,
                        quantity: SineQuantity::AccelGPk,
                        value: 0.2,
                        alarm_db: None,
                        abort_db: None,
                    },
                ],
                global_alarm_db: (3.0, 3.0),
                global_abort_db: (6.0, 6.0),
            },
            mode: SineModeSpec::Sweep(SineSweepSpec {
                band_hz: (60.0, 500.0),
                rate: SineSweepRate::LogOctPerMin(60.0),
                start_hz: 60.0,
                initial_direction: SweepDirection::Up,
                target: SineSweepTarget::Sweeps(1),
            }),
            control: SineControlSet {
                channels: vec![SineControlChannel {
                    index: 1,
                    weight: 1.0,
                }],
                mode: SineCompositeMode::WeightedAvg,
            },
            limit_profiles: Vec::new(),
            blend_w: 0.5,
            compression: SineCompression::Fast,
            tracking_bw: SineBandwidthMode::FixedHz(10.0),
            feedforward: SineFeedforwardConfig {
                coherence_floor: 0.5,
                h_floor_frac: 0.05,
            },
            notch: SineNotchConfig {
                drive_floor: 0.01,
                smoothing_ticks: 3.0,
                release_ramp_db_per_s: 20.0,
            },
            ladder: SineLadderConfig {
                sysid_level_db: -20.0,
                start_db: -12.0,
                table_db: vec![-12.0, -6.0, 0.0],
                per_rung: SineRungFor::Sweeps(1),
                full_for: SineRungFor::Sweeps(1),
            },
            sysid_level_db: -20.0,
            coherence_floor: 0.5,
            drive_vpk_ceiling: 1.0,
            drive_vpk_abort: 2.0,
            fault_debounce_ticks: 5,
            kinematic_monitor: false,
            capture: SineCaptureConfig {
                rule: SineCaptureRule::Off,
                level_settle_s: 1.0,
                periodic_save_s: None,
            },
            cola: SineColaConfig {
                enabled: false,
                volts_pk: 1.0,
                waveform: SineColaWaveform::Sine,
            },
            feasibility: None,
            control_tick_frames: 64,
            fs_hz: None,
            review_hold: false,
            metadata: Vec::new(),
        }
    }

    fn host() -> SineHost {
        let root = std::env::temp_dir().join(format!(
            "hertzforge-backendd-sine-{}-{}",
            std::process::id(),
            now_unix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (tx, _) = tokio::sync::broadcast::channel(256);
        SineHost::new(BackendKind::Sim, Vec::new(), root, tx, 30.0)
    }

    /// Block until the bring-up leaves a phase, or give up. BOUNDED so a genuine
    /// failure fails the test instead of hanging.
    fn wait_until(h: &SineHost, secs: u64, f: impl Fn(Phase) -> bool) -> Phase {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let p = h.phase();
            if f(p) || Instant::now() > deadline {
                return p;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The wire profile translates, the station builds, and the service configures
    /// - on the Sim station, hardware-free.
    #[test]
    fn a_wire_profile_configures_a_service() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        assert!(h.is_configured());
        assert!(!h.is_live(), "configured is not live");
        let p = h.provenance().expect("provenance recorded");
        assert_eq!(p.module, "sine");
    }

    /// Start before SetProfile is a refusal that says what to send first.
    #[test]
    fn start_before_configure_says_what_to_send_first() {
        let h = host();
        match h.start() {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("SetProfile"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// The channel-table refusal set, one row per mistake, each named.
    ///
    /// Zero drive rows and too many are DIFFERENT mistakes with different fixes, so
    /// they get different sentences; a COLA profile against a one-output table is a
    /// third. Anti-vacuity is the test above: the same profile on a correct table
    /// configures, so these refusals are about the table and not about the profile.
    #[test]
    fn the_table_refusals_name_the_mistake() {
        let h = host();
        let mut no_drive = table();
        no_drive.channels[3].role = hertzforge_channel_table::ChannelRole::Monitor;
        match h.set_profile(profile(), no_drive, "test") {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("no enabled Drive row"), "{message}");
                assert!(message.contains("PutChannelTable"), "{message}");
            }
            other => panic!("a table with no Drive row: {other:?}"),
        }

        let mut two_drive = table();
        two_drive.channels[2].role = hertzforge_channel_table::ChannelRole::Drive;
        match h.set_profile(profile(), two_drive, "test") {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("2 enabled Drive rows"), "{message}");
                assert!(message.contains("COLA"), "{message}");
            }
            other => panic!("two Drive rows with COLA off: {other:?}"),
        }

        let mut cola = profile();
        cola.cola.enabled = true;
        match h.set_profile(cola, table(), "test") {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("COLA"), "{message}");
                assert!(message.contains("SECOND drive channel"), "{message}");
            }
            other => panic!("COLA against a one-output table: {other:?}"),
        }
    }

    /// **THE SAFE-STOP BUDGET IS CHECKED, WITH BOTH NUMBERS IN THE REFUSAL.**
    ///
    /// A drive ceiling large enough to make the ramp outlast the controller timeout
    /// is refused BEFORE the station is built, because the failure it produces
    /// otherwise is a daemon that declares the controller gone and then rides a
    /// ramp past its own deadline.
    ///
    /// Anti-vacuity: the shipped ceiling (1.0) passes on the same host, so this row
    /// depends on the arithmetic and not on `set_profile` refusing everything.
    ///
    /// RED-MUTATION: delete the budget check and the refusal arm reds.
    #[test]
    fn a_ramp_that_outlasts_the_controller_timeout_is_refused_with_both_numbers() {
        // `t_ramp = 20 * log10(ceiling / 1e-6) / 120`. A 30 s timeout minus the 2 s
        // drain allowance leaves 28 s of ramp, which needs
        // `ceiling > 1e-6 * 10^(28 * 120 / 20)` - astronomically large. So the
        // honest way to exercise the guard is a SHORT timeout, which is a real
        // configuration (`controller_timeout_s` is operator-set).
        let root = std::env::temp_dir().join(format!(
            "hertzforge-backendd-sine-budget-{}-{}",
            std::process::id(),
            now_unix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (tx, _) = tokio::sync::broadcast::channel(16);
        let tight = SineHost::new(BackendKind::Sim, Vec::new(), root, tx, 2.5);
        match tight.set_profile(profile(), table(), "test") {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("safe-stop budget"), "{message}");
                assert!(
                    message.contains("2.5"),
                    "the TIMEOUT must be stated: {message}"
                );
                assert!(
                    message.contains("3.00") || message.contains("3.0"),
                    "the BUDGET must be stated: {message}"
                );
                assert!(message.contains("controller_timeout_s"), "{message}");
            }
            other => panic!("a budget that does not fit must be refused: {other:?}"),
        }
        // ANTI-VACUITY: the same profile at the shipped 30 s timeout configures.
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("the shipped budget fits");
    }

    /// **THE WHOLE BRING-UP, on the Sim station**: configure -> start -> the phase
    /// walks arming -> sysid -> ... -> engaged, and a stop lands.
    ///
    /// This is the hardware-free proof that a remote peer's verb sequence drives a
    /// real service through a real station, and that the LOCK-FREE phase mirror
    /// actually moves - the thing an operator watches for tens of seconds while
    /// `fill_status` has nothing else to say.
    #[test]
    fn the_sim_station_walks_the_bringup_phases_and_engages() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        assert_eq!(h.phase(), Phase::Idle);
        h.start().expect("start");
        // `start` returns with the arm DONE and the burst owed to the PUMP, so the
        // phase is `arming` until that thread actually enters it. It must NOT already
        // say `sysid`: that token means the AO is driving the burst, and a controller
        // that saw it early would send a Stop at something not yet running.
        assert!(
            matches!(
                h.phase(),
                Phase::Arming | Phase::SysId | Phase::Gating | Phase::Engaged
            ),
            "start must leave the run past the arm, got {:?}",
            h.phase()
        );
        assert!(h.is_live(), "an armed run with the AO clocked is LIVE");

        // The rate pair is filled from the arm and never before it.
        let mut st = hertzforge_protocol::StatusReport::idle();
        h.fill_status(&mut st);
        assert!(
            st.actual_fs_hz.is_some(),
            "arm is the only moment the actual rate exists, and it has happened"
        );
        assert!(st.requested_fs_hz.is_some());

        let phase = wait_until(&h, 120, |p| {
            matches!(
                p,
                Phase::Engaged
                    | Phase::FullLevel
                    | Phase::CoherenceHold
                    | Phase::Aborted
                    | Phase::Stopped
            )
        });
        assert!(
            matches!(phase, Phase::Engaged | Phase::FullLevel),
            "the run must ENGAGE on the Sim plant; got {phase:?}"
        );

        let mut st = hertzforge_protocol::StatusReport::idle();
        h.fill_status(&mut st);
        assert_eq!(st.run, hertzforge_protocol::RunState::Running);
        assert_eq!(st.module_state.as_deref(), Some(phase.token()));
        assert!(st.hold.is_none(), "an engaged run is not holding");
        assert!(
            st.band_coherence.is_some(),
            "the gate measured a coherence and the status must carry it"
        );

        // THE RAMPED STOP, and the budget is DERIVED rather than guessed.
        //
        // The ramp is `safe_stop_seconds(start scale)` of SIGNAL - about 0.9 s from
        // the -12 dB start rung at 120 dB/s - plus the generation drain. What makes
        // it minutes rather than seconds here is the HERTZFORGE-SIM station's block: it
        // is ONE FRAME (`SIM_NATURAL_BLOCK_FRAMES`), so a pump iteration advances two
        // frames and then yields the mutex for a millisecond, which is ~20-35x slower
        // than real time in a debug build. MEASURED at 35.3 s. That pacing is a
        // property of the Sim, not of this code - it is the same ratio the wire
        // probe's TIME_SCALE knob exists to declare - so the budget carries slack
        // over the measurement rather than sitting on it.
        h.stop().expect("stop");
        let ended = wait_until(&h, 180, |p| {
            matches!(p, Phase::Stopped | Phase::Aborted | Phase::Idle)
        });
        assert!(
            matches!(ended, Phase::Stopped | Phase::Aborted),
            "the ramped stop must land; got {ended:?}"
        );
        assert_eq!(
            ended,
            Phase::Stopped,
            "a controlled stop must land STOPPED, never faulted - an abort would mean the \
             ramp was not walked"
        );
        assert!(!h.is_live(), "stopped is not live");
    }

    /// **A PASSING GATE PARKS AT `AwaitingReview`, AND THE PUMP EXITS THERE.**
    ///
    /// The park is the whole reason this host copies Random's pump shape instead of
    /// the Explorer's: `ProceedAfterReview` needs the service mutex, and a lock-held
    /// priming block would still be holding it. So the assertions are BOTH halves -
    /// the run parks (it did not engage itself past the review) AND the release verb
    /// answers rather than deadlocking, which it can only do if the pump left.
    ///
    /// RED-MUTATION: hold the service mutex across the park (the Explorer's shape)
    /// and `proceed_after_review` never returns - this test hangs and then fails on
    /// its own bounded wait rather than passing.
    #[test]
    fn a_passing_gate_parks_at_awaiting_review_and_proceed_releases_it() {
        let h = host();
        let mut p = profile();
        p.review_hold = true; // the shipped default; ON deliberately here
        h.set_profile(p, table(), "test").expect("configure");
        h.start().expect("start");

        let phase = wait_until(&h, 120, |p| {
            matches!(
                p,
                Phase::AwaitingReview | Phase::CoherenceHold | Phase::Engaged | Phase::Aborted
            )
        });
        assert_eq!(
            phase,
            Phase::AwaitingReview,
            "a PASSING gate with review_hold on must PARK, not engage"
        );
        assert!(
            !h.is_live(),
            "the park stops the AO and the drive is at zero - a park is not live"
        );

        let mut st = hertzforge_protocol::StatusReport::idle();
        h.fill_status(&mut st);
        assert_eq!(st.module_state.as_deref(), Some("awaiting_review"));
        let hold = st.hold.expect("a parked run must say WHY on the wire");
        assert!(hold.contains("REVIEW HOLD"), "{hold}");
        assert!(
            hold.contains("PASSED"),
            "the reason must distinguish this park from the gate FAILING: {hold}"
        );

        // The release verb ANSWERS - it does not deadlock against the pump.
        h.proceed_after_review().expect("the park releases");
        let engaged = wait_until(&h, 30, |p| {
            matches!(p, Phase::Engaged | Phase::FullLevel | Phase::Aborted)
        });
        assert!(
            matches!(engaged, Phase::Engaged | Phase::FullLevel),
            "ProceedAfterReview must engage; got {engaged:?}"
        );
        let mut st = hertzforge_protocol::StatusReport::idle();
        h.fill_status(&mut st);
        assert!(st.hold.is_none(), "the hold clears when the park releases");
        // ABORT, not Stop, for teardown: `Drop` joins the pump, and a RAMPED stop on
        // the Sim's one-frame block costs ~35 s of wall clock (see
        // `the_sim_station_walks_the_bringup_phases_and_engages`, which is where the
        // ramp is the row under test). Every other test here pays that for nothing.
        h.abort("test teardown").expect("abort");
        wait_until(&h, 60, |p| !p.is_live());
    }

    /// **AN ABORTED RUN NAMES ITSELF ON THE WIRE.**
    ///
    /// The chassis finding this closes: the sine closing sitting's NI-REAL wire
    /// probe watched a mid-engaged controller vanish reach `module_state =
    /// "aborted"` and could say nothing else about it, because the bare token was
    /// the entire message. The trip lived in the service, the reason lived in the
    /// event log, and the only way to see either was `journalctl` on the box - so
    /// the record of it had to GUESS which protection had fired.
    ///
    /// The reason travels namespaced (`aborted: <why>`), which is the convention
    /// this module already uses for its parks and which every consumer already
    /// splits on the first colon.
    ///
    /// **RED-MUTATION, OBSERVED (2026-08-09):** restore the old
    /// `s.module_state = Some(phase.token().to_string())` in `fill_status` and this
    /// reds on `starts_with("aborted: ")` - the exact silence the chassis row hit.
    #[test]
    fn an_aborted_run_carries_its_reason_in_module_state() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        let phase = wait_until(&h, 120, |p| {
            matches!(p, Phase::Engaged | Phase::FullLevel | Phase::Aborted)
        });
        assert!(
            matches!(phase, Phase::Engaged | Phase::FullLevel),
            "the run must ENGAGE before it can be aborted mid-run; got {phase:?}"
        );

        h.abort("supervisory abort from the test").expect("abort");
        let ended = wait_until(&h, 60, |p| matches!(p, Phase::Aborted | Phase::Stopped));
        assert_eq!(ended, Phase::Aborted, "an abort lands ABORTED");

        let mut st = hertzforge_protocol::StatusReport::idle();
        h.fill_status(&mut st);
        let ms = st
            .module_state
            .clone()
            .expect("a configured run reports a state");
        // THE TOKEN IS STILL THE TOKEN. Everything downstream - the probe rows, the
        // panel's transport surface, the poll's terminal rule - reads the part
        // before the first colon, and a change that renamed the state would break
        // all three while looking like a message improvement.
        assert_eq!(
            ms.split(':').next(),
            Some("aborted"),
            "the token must survive the namespacing: {ms}"
        );
        assert!(
            ms.starts_with("aborted: "),
            "an abort must say WHY on the wire, not only in the journal: {ms}"
        );
        assert!(
            ms.contains("supervisory abort from the test"),
            "the reason must be the one the service recorded, verbatim: {ms}"
        );
        assert!(
            st.abort_active,
            "the abort flag and the reason are the same fact and must agree"
        );

        // ANTI-VACUITY: a run that has NOT aborted carries the bare token, so the
        // assertion above is about aborts and not about every status ever sent.
        let h2 = host();
        h2.set_profile(profile(), table(), "test")
            .expect("configure");
        let mut st2 = hertzforge_protocol::StatusReport::idle();
        h2.fill_status(&mut st2);
        assert_eq!(
            st2.module_state.as_deref(),
            Some("idle"),
            "a configured-but-idle run has no reason to carry"
        );
    }

    /// **`ProceedAfterReview` FROM AN IDLE RUN IS THE SERVICE'S OWN REFUSAL**, passed
    /// through verbatim.
    ///
    /// The negative control for the release path. The host adds no state machine of
    /// its own on top of the service's, so a release sent at the wrong moment gets
    /// the service's sentence - which is what makes the `CoherenceHold` refusal
    /// (whose text says the gate FAILED and that the park has no release verb)
    /// reach the operator unparaphrased. A second copy here would be a second
    /// statement of a safety rule, and the copy is the one that drifts.
    #[test]
    fn proceed_after_review_at_the_wrong_moment_carries_the_services_own_words() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        match h.proceed_after_review() {
            Err(ProtocolError::Refused { message }) => {
                assert!(
                    message.contains("proceed_after_review from"),
                    "the SERVICE's own text, not the host's: {message}"
                );
                assert!(message.contains("AwaitingReview"), "{message}");
            }
            other => panic!("a release with nothing parked: {other:?}"),
        }
    }

    /// The sysID `|H1|` + coherence pair reaches the display bus AT the gate.
    ///
    /// Asserted at the PRODUCER, before the frame is packed, for the same reason the
    /// Explorer's trace-id row is: a wire probe counts frames and does not decode the
    /// packed layout, so the id census has to be pinned where the ids are chosen.
    #[test]
    fn the_sysid_pair_reaches_the_bus_on_its_reserved_ids() {
        let h = host();
        let mut rx = h.tx.subscribe();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        wait_until(&h, 120, |p| {
            matches!(
                p,
                Phase::Engaged | Phase::FullLevel | Phase::CoherenceHold | Phase::Aborted
            )
        });

        let mut ids: Vec<u32> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(DisplayBatch::Spectrum(b)) => {
                    ids.extend(b.traces.iter().map(|(ch, _, _)| *ch));
                    if ids.contains(&CH_SYSID_H) && ids.contains(&CH_SYSID_COH) {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
        assert!(
            ids.contains(&CH_SYSID_H),
            "the sysID |H1| must be published at the gate: {ids:?}"
        );
        assert!(
            ids.contains(&CH_SYSID_COH),
            "the sysID coherence must ride the same batch: {ids:?}"
        );
        h.abort("test teardown").expect("abort"); // see the note on the park test
        wait_until(&h, 60, |p| !p.is_live());
    }

    /// **THE SWEEP OVERLAYS AND THE TIME STREAMS REACH THE BUS**.
    ///
    /// Asserted at the PRODUCER, before anything is packed into a binary frame, for
    /// the reason the Explorer's equivalent row gives: a wire probe counts frames
    /// and does not decode the packed `SpectrumFrame` layout, so the id census has
    /// to be pinned where the ids are chosen.
    ///
    /// The three claims, each of which has its own way of being wrong:
    ///
    /// 1. The reserved 910-918 overlays ride the batch, on a raster whose `f0` is
    ///    the run band's LOW edge - not an FFT grid starting at DC, which would put
    ///    the whole test in the left-hand tenth of the pane.
    /// 2. Per-channel curves ride their REAL table indices, with broadband at
    ///    `index + 400`. A slot number here would label the wrong channel the
    ///    moment an operator disabled a row.
    /// 3. SCOPE and WAVEFORM batches flow, with a RUN-RELATIVE `t_end_s` that
    ///    ADVANCES. A packer handed the block length instead stamps a constant
    ///    `t_end_s` on every batch and every follow window freezes - 48/48 wire
    ///    frames measured that way on the Random path, which is why the tap is
    ///    shared rather than copied.
    #[test]
    fn the_sweep_overlays_and_time_streams_reach_the_bus() {
        let h = host();
        let mut rx = h.tx.subscribe();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        wait_until(&h, 120, |p| {
            matches!(
                p,
                Phase::Engaged | Phase::FullLevel | Phase::CoherenceHold | Phase::Aborted
            )
        });

        let mut ids: Vec<u32> = Vec::new();
        let mut f0 = f64::NAN;
        let mut df = f64::NAN;
        let mut scope_t: Vec<f64> = Vec::new();
        let mut wave_t: Vec<f64> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(DisplayBatch::Spectrum(b)) => {
                    // The sysID pair rides its OWN grid (the sysID's), so only a
                    // batch carrying a sweep overlay describes the sweep raster.
                    if b.traces.iter().any(|(ch, _, _)| *ch == CH_PROFILE) {
                        f0 = b.f0_hz;
                        df = b.df_hz;
                    }
                    ids.extend(b.traces.iter().map(|(ch, _, _)| *ch));
                }
                Ok(DisplayBatch::Scope(b)) => scope_t.push(b.t_end_s),
                Ok(DisplayBatch::Wave(b)) => wave_t.push(b.t_end_s),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
            if scope_t.len() >= 3
                && wave_t.len() >= 3
                && ids.contains(&CH_COMPOSITE)
                && ids.contains(&(1 + BROADBAND_OFFSET))
            {
                break;
            }
        }

        for want in [
            CH_PROFILE,
            CH_ALARM_HI,
            CH_ALARM_LO,
            CH_ABORT_HI,
            CH_ABORT_LO,
            CH_COMPOSITE,
        ] {
            assert!(ids.contains(&want), "overlay {want} missing: {ids:?}");
        }
        assert!(
            ids.contains(&1),
            "the control channel's TRACKED curve rides its real table index: {ids:?}"
        );
        assert!(
            ids.contains(&(1 + BROADBAND_OFFSET)),
            "broadband rides index + {BROADBAND_OFFSET}: {ids:?}"
        );

        // BAND-SPANNING: `f0` is the band's low edge (60 Hz), and 256 points span it.
        assert!(
            (f0 - 60.0).abs() < 1e-6,
            "the sweep raster starts at the band's LOW edge, not at DC: f0={f0}"
        );
        let expect_df = (500.0 - 60.0) / (hertzforge_sine::SWEEP_RASTER_POINTS as f64 - 1.0);
        assert!(
            (df - expect_df).abs() < 1e-9,
            "the raster spans the band in {} points: df={df} want {expect_df}",
            hertzforge_sine::SWEEP_RASTER_POINTS
        );

        // TIME STREAMS, with an ADVANCING run-relative clock.
        assert!(scope_t.len() >= 2, "scope batches: {}", scope_t.len());
        assert!(wave_t.len() >= 2, "waveform batches: {}", wave_t.len());
        assert!(
            scope_t.windows(2).all(|w| w[1] >= w[0]),
            "the scope clock must never run backwards: {scope_t:?}"
        );
        assert!(
            scope_t.last().copied().unwrap_or(0.0) > scope_t[0],
            "the scope clock must ADVANCE (a constant t_end freezes the pane): \
             {scope_t:?}"
        );
        assert!(
            wave_t.last().copied().unwrap_or(0.0) > wave_t[0],
            "the waveform clock must ADVANCE: {wave_t:?}"
        );

        // The ring's loss reaches the WIRE, zero included.
        let mut st = hertzforge_protocol::StatusReport::idle();
        h.fill_status(&mut st);
        assert!(
            st.display_blocks_dropped.is_some(),
            "a live run must report its display ring's loss, including zero"
        );

        h.abort("test teardown").expect("abort"); // see the note on the park test
        wait_until(&h, 60, |p| !p.is_live());
    }

    /// RECONFIGURE while THIS module's own run is live is refused by name.
    ///
    /// The CROSS-module half of the exclusion lives in `Core::live_module`; what
    /// stays here is the self-check - only Sine knows what "already live" means for
    /// Sine.
    #[test]
    fn reconfigure_refuses_while_this_modules_own_run_is_live() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        match h.set_profile(profile(), table(), "test") {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("live"), "{message}");
                assert!(message.contains("Stop"), "{message}");
            }
            other => panic!("must refuse: {other:?}"),
        }
        h.abort("test teardown").expect("abort"); // see the note on the park test
        wait_until(&h, 60, |p| !p.is_live());
    }

    /// **A STOP DURING THE sysID BURST IS SEEN INSIDE THE BURST**, not after it.
    ///
    /// THE SAFETY ROW OF THIS FILE. During sysID the AO is driving at the sysID level
    /// and the service holds its own mutex for the whole burst - tens of seconds on
    /// a real band. A stop that had to take that mutex could not be seen until the
    /// thing it was cancelling had finished, which is a live unwatched drive for the
    /// length of a burst. The cancellation flag is an `Arc<AtomicBool>` precisely so
    /// this path does not need the lock.
    ///
    /// The assertion is on the OUTCOME (the run leaves the driving states promptly),
    /// not on a wall-clock bound: the Sim station is not paced by a driver, so a
    /// timing bound here would be measuring the test host. The wire probe's
    /// `row_stop_during_sysid_answers_inside_one_block` is where the TIME assertion
    /// belongs, against a station whose clock is real.
    ///
    /// RED-MUTATION: stop caching the cancel handle (fetch it under the service
    /// mutex instead) and `safe_stop` blocks until the burst ends - the phase is
    /// still `sysid` when the bounded wait expires.
    #[test]
    fn a_stop_during_sysid_is_seen_without_the_service_mutex() {
        let h = host();
        let mut p = profile();
        // A LOW band edge makes the burst long: `run_sysid` sizes nfft to resolve a
        // sharp peak at the low edge, so the burst runs ~1600 / f_lo seconds of
        // signal. 20 Hz is the slowest this profile family gets, which is what
        // gives the stop something to interrupt.
        p.mode = hertzforge_protocol::SineModeSpec::Sweep(hertzforge_protocol::SineSweepSpec {
            band_hz: (20.0, 500.0),
            rate: hertzforge_protocol::SineSweepRate::LogOctPerMin(60.0),
            start_hz: 20.0,
            initial_direction: hertzforge_protocol::SweepDirection::Up,
            target: hertzforge_protocol::SineSweepTarget::Sweeps(1),
        });
        p.profile.rows[0].freq_hz = 20.0;
        h.set_profile(p, table(), "test").expect("configure");
        h.start().expect("start");
        // WAIT for the pump to actually enter the burst. `start` leaves the phase at
        // `arming`; `sysid` means the AO is driving, and stopping before that is
        // stopping something that has not started - which is precisely the window
        // that used to swallow the request (the burst cleared its own cancellation
        // flag on entry; `arm_inner` owns that reset now).
        let entered = wait_until(&h, 30, |p| p != Phase::Arming);
        assert_eq!(entered, Phase::SysId, "the pump must enter the burst");

        h.safe_stop("controller vanished during sysID");
        let ended = wait_until(&h, 90, |p| !p.is_live());
        assert!(
            !ended.is_live(),
            "a stop raised during sysID must be honored; the run is still {ended:?}"
        );
        // AND IT LANDS `stopped`, NOT FAULTED - a stop the operator asked for must
        // never be reported as a failure.
        //
        // `run_sysid` returns an `Err` when the burst is cancelled mid-flight,
        // because that IS how the cancellation is delivered, and the first version of
        // this host latched that `Err` as a fault: an operator who pressed Stop was
        // told on the wire that something had broken.
        //
        // **RED-MUTATION, OBSERVED:** replace the pump's cancelled-burst condition
        // with `false` and this reds `left: Aborted, right: Stopped`.
        //
        // It did NOT red until two other things were fixed, and both are worth
        // knowing because each made this assertion vacuous in a different way (hard
        // rule 3): the burst used to clear its own cancellation flag on entry, so a
        // request raised before it started was swallowed; and `start` used to set the
        // phase to `sysid` before the pump had entered the burst, so the `wait_until`
        // above returned immediately and the stop landed in that same swallowed
        // window. With both corrected, the burst is genuinely running when the stop
        // arrives and this test exercises the arm it names.
        let settled = wait_until(&h, 30, |p| matches!(p, Phase::Stopped | Phase::Aborted));
        assert_eq!(
            settled,
            Phase::Stopped,
            "an operator stop must land STOPPED however the burst ended"
        );
        let mut st = hertzforge_protocol::StatusReport::idle();
        h.fill_status(&mut st);
        assert_eq!(
            st.run,
            hertzforge_protocol::RunState::Stopped,
            "module_state={:?} - a stop the operator asked for must not report Faulted",
            st.module_state
        );
    }
}
