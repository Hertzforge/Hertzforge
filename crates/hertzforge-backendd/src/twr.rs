//! The daemon-side TIME WAVEFORM REPLICATION module.
//!
//! # What is different about this module, in one paragraph
//!
//! Every other host on this daemon is handed everything it needs on one verb. TWR is
//! not: the thing it replicates is a RECORD of 10^5-10^6 samples, which cannot ride
//! `SetProfile` (it would put megabytes on the command path and block every status poll
//! behind one send). So the record crosses ONCE through its own three-verb upload,
//! the profile NAMES it by digest, and the daemon is the authority on both halves - it
//! holds the RAW staged record and applies the profile's prep to it. Everything else
//! here is the SRS host's shape: a job-queue pump, a lock-free phase mirror, a
//! published snapshot the status path reads without the service mutex, and parks whose
//! release verbs must not deadlock against the thread they are releasing.
//!
//! # The pump walks the ladder, one `advance()` at a time
//!
//! `TwrService::advance()` performs exactly ONE unit of work (the sysID pass, one
//! correction iteration, or one playback play) and owns no loop. That is deliberate on
//! the service side and it makes the HOST responsible for three things the service
//! cannot do:
//!
//! * **Pacing.** In `Auto` the pump walks units until a terminal state or a park; in
//!   `Manual` each `Twr(IterateOnce)` is one unit. Which one applies is authored in the
//!   profile, not switched live - see the `SetLadderMode` counter-refusal in `core.rs`.
//! * **The post-sysID REVIEW PARK.** A park is a decision not to call `advance()` again
//!   yet, so only the caller can hold one. The pump EXITS at the park and
//!   `ProceedAfterReview` respawns it - Random's shape, restated by `sine.rs`. A
//!   lock-held park would hold the service mutex across the wait, and the release verb
//!   would deadlock against the thread it is trying to release.
//! * **Publishing.** Between units, and DURING them (see below).
//!
//! # THREE PARKS, three different releases, and one of them has none
//!
//! | Park | What it means | Release |
//! |---|---|---|
//! | `FeasibilityBlocked` | a kinematic ceiling failed BEFORE anything drove | the TWR override with a non-empty ack |
//! | `AwaitingReview` (HOST-side) | the sysID PASSED and is waiting for a human | `ProceedAfterReview` |
//! | `CoherenceHold` | the sysID FAILED; the plant is not known | **`Twr(Reset)` ONLY** |
//! | `Holding` | a level exhausted its iteration budget under the Hold policy | a level verb (`StepLevel` / `JumpLevel`) |
//!
//! `CoherenceHold` is the one with no release verb, and the three barriers that keep it
//! that way are `sine.rs`'s verbatim: the service refuses `advance()` from it BY NAME,
//! nothing respawns the pump there, and `proceed_after_review` short-circuits with the
//! service's own sentence rather than a paraphrase. A release verb wired to that park
//! would be a one-click bypass of a safety refusal.
//!
//! **All four parks are LIVE** (`holds()` answers `Configured` AND `Live`), which is the
//! SRS hold obligation applied here: a park is a PAUSED run holding a claimed core, not
//! an idle one, and a second module binding the chassis while an operator studies an FRF
//! would take the run away from under them.
//!
//! **`safe_stop` EARLY-RETURNS at `AwaitingReview` and `CoherenceHold`; `operator_stop`
//! does not.** A vanished controller means "nobody is watching, make the hardware safe" -
//! and at those two parks the hardware is already safe (the tasks are stopped and the pin
//! is at the exact-zero pad). Tearing the run down there would discard a measured system
//! ID because a network went quiet. The operator's own Stop still ends it.
//!
//! # Numeric status stays live through a 30 s play
//!
//! A TWR play is not a tick: one `advance()` can hold the service mutex for the whole
//! length of the record. Two mechanisms keep the wire honest across that window, and
//! NEITHER of them is a poll of the service:
//!
//! * The pump writes the published [`TwrCell`] snapshot IMMEDIATELY BEFORE each unit, so
//!   the state, level and iteration counters are already correct when the play starts.
//! * The player's per-block observer (`with_play_observer`) updates a lock-free
//!   [`PlayCell`] with `frames_done` / `frames_expected` and a driver BEAT. `fill_status`
//!   reads atomics only.
//!
//! # TWO watchdog deadlines, because there are two failure modes
//!
//! One beat for two failure modes is the trap `srs.rs` documents from the other side (it
//! has no observer, so it honestly has no watchdog at all). Here both exist and each is
//! armed only when it can be judged:
//!
//! * **The DRIVER beat** - armed while blocks are still owed on a play in flight, fed by
//!   the per-block observer. Threshold `(3 x block period).max(3 s)`, derived at arm from
//!   the ADOPTED rate and the block the device programmed.
//! * **The COMPUTE deadline** - armed after the last block of a play has arrived and
//!   before the unit returns, which is exactly the window the inverse-filter maths runs
//!   in. Threshold `(3 x the record's own wall time).max(3 s)`, derived at arm from the
//!   adopted rate and the padded grid length: the between-play compute is a handful of
//!   FFTs over the record and has no business taking three times as long as playing it.
//!
//! Both are exempt while a stop is in flight.
//!
//! # The display path never runs on the pump
//!
//! Time streams come off the player's SHARED display tap (widened to an
//! `Arc<Mutex<SampleRing>>` for exactly this): a small DRAIN THREAD reads the ring at
//! wall-clock cadence and packs scope + wave batches through the shared `display.rs`
//! packers. It never touches the service mutex, so a 30 s play streams while it runs
//! rather than dumping at the end. Spectra ride the pump, between units, because they
//! are computed from the service's own state and only change per unit.
//!
//! # The reference upload, and why the daemon holds the RAW record
//!
//! `BeginReference` -> N x `ReferenceChunk` -> `CommitReference` stages the record
//! keyed by its SHA-256. The daemon stores it RAW and hands the RAW samples to the
//! service at every configure, which applies the profile's prep. ONE authority: a prep
//! can never compound, because the thing it is applied to is never a prepped record.
//! `ReferenceInfo` echoes the APPLIED prep (prepared length + adopted rate) beside the
//! staged manifest, which is the only place an operator can check that the record being
//! replicated is the record they authored.
//!
//! The staged record SURVIVES `SetProfile` (that is the point of keying by digest) and
//! dies at `Reset`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_hal::Backend;
use hertzforge_module_abi::StationClass;
use hertzforge_protocol::{Doc, ProtocolError, SchemaKind, StatusReport, TwrProfile};
use hertzforge_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};
use hertzforge_twr::player::{BackendReferencePlayer, PlayProgress, SampleRing};
use hertzforge_twr::service::config::{CaptureRules, MetaField, PrepConfig, TwrConfig};
use hertzforge_twr::{
    Excitation, InverseConfig, LawConfig, ReferenceWaveform, RunSchedule, Section, SysIdConfig,
    TwrError, TwrLevelSchedule, TwrLimits, TwrService, TwrState,
};

use crate::display::{scope_batch, wave_batch_dec, TAP_MIN_SPAN};
use crate::provenance::{mint_run_id, now_unix, RunProvenance};
use crate::recorder::{DisplayBatch, SpectrumBatch};

mod digest;
mod reference;

pub use reference::{StagedReference, Staging};

/// The one concrete service type this host drives.
type Svc = TwrService<BackendReferencePlayer<Box<dyn Backend>>>;

// =====================================================================================
// Reserved trace ids
// =====================================================================================
//
// TWR's LIVE traces move out of the frozen 940-999 census block into TWR's own
// 1000-1099. The old ids (980-989) are one of four claimants in a block Random's pairs
// can reach, and they are marked RETIRED-do-not-reissue rather than re-used.
//
// ~~**PINNED RUST-SIDE ONLY when this host landed**, with the UI still on 980-989 until
// the UI migration lands.~~
// **CORRECTED at the merge with the UI migration (2026-08-09): BOTH ENDS AGREE.**
//
// The UI work landed the migration on the TS side while this host was being written, so
// the gap that paragraph honestly described is closed: `ui/src/lib/twr/traces.ts` pins
// the same ten numbers in the same semantic order (time trio, five spectra, envelope
// pair), `TWR_LIVE_HI = 1019` reserves the growth tail, overlays start at 1020, and
// 980-989 is marked RETIRED-do-not-reissue in the census rather than freed - Random's
// pair block can reach 999, so handing those ten ids to a new claimant would recreate
// the collision one module later.
//
// VERIFIED BY COMPARISON at the merge, not assumed: the two constant blocks were read
// against each other id by id. They are two files and neither imports the other, so
// this is a pin that has to be re-checked whenever either moves.

/// Level-scaled target, time domain. Reserved; the host publishes time via the scope /
/// wave streams and this id is the shared-graph slot the UI wires.
pub const CH_TWR_TARGET_TIME: u32 = 1000;
/// Last achieved record, time domain. Reserved (see [`CH_TWR_TARGET_TIME`]).
pub const CH_TWR_ACHIEVED_TIME: u32 = 1001;
/// Last error record (`achieved - target`), time domain. Reserved.
pub const CH_TWR_ERROR_TIME: u32 = 1002;
/// The sysID FRF magnitude `|H|`, on the SYSID grid.
pub const CH_TWR_FRF: u32 = 1003;
/// The sysID ordinary coherence, on the SYSID grid.
pub const CH_TWR_COHERENCE: u32 = 1004;
/// The current drive spectrum magnitude, on the RECORD grid.
pub const CH_TWR_DRIVE_SPEC: u32 = 1005;
/// The level-scaled target spectrum magnitude, on the RECORD grid.
pub const CH_TWR_TARGET_SPEC: u32 = 1006;
/// The last achieved spectrum magnitude, on the RECORD grid.
pub const CH_TWR_ACHIEVED_SPEC: u32 = 1007;
/// The upper abort-envelope curve, time domain. Reserved.
pub const CH_TWR_ENV_HI: u32 = 1008;
/// The lower abort-envelope curve, time domain. Reserved.
pub const CH_TWR_ENV_LO: u32 = 1009;

/// **The date the TWR chassis gate passed on real hardware, or `None` if it has not.**
///
/// While this is `None`, [`TwrHost::start`] refuses on an `NI-REAL` station BY NAME
/// ([`NO_CHASSIS_LEDGER`]): a TWR run against a real chassis would otherwise be the
/// first, unattended, with no numeric evidence behind it. HERTZFORGE-SIM and NI-SIM are
/// unaffected: the guard is about a physical armature, not about the module.
///
/// The gate is the eight numeric chassis checks passing twice in one sitting. **Recorded
/// 2026-08-09** on a cDAQ-9178, bench asserted by the operator (AO0->AI0 jumper, DMM
/// tee'd, no shaker, IEPE off): both runs 8 of 8, and each check failed under its own
/// injected mutation in the same sitting. DMM: the ranging play PRESENT on AO0 at
/// +0.194 V (commanded ~0.23 V; the 10 kS/s digitize undersamples a 51.2 kHz-band
/// record's peak, so the underread is the instrument, not the drive); the 8 s
/// inter-play park ABSENT at 1.4 mV - rest-at-zero measured, not assumed.
pub const TWR_CHASSIS_LEDGER_DATE: Option<&str> = Some("2026-08-09");

/// The refusal [`TwrHost::start`] serves while [`TWR_CHASSIS_LEDGER_DATE`] is `None`
/// and the station is NI-REAL.
///
/// **The wire probe asserts this string, so it is a contract and not a log line**
/// (`tools/probes/twr_wire_probe.py`, the ledger row, which SKIPs on SIM and states
/// this sentence). Edit it and change both files in one commit.
pub const NO_CHASSIS_LEDGER: &str =
    "TWR will not Start on an NI-REAL station: this build records no chassis gate (the \
     eight numeric chassis checks passing twice in one sitting), so no TWR replication \
     iteration has been proven on hardware. HERTZFORGE-SIM and NI-SIM stations are \
     unaffected.";

/// **The upload's not-live refusal, verbatim.** The wire probe asserts it.
pub const UPLOAD_NOT_WHILE_LIVE: &str =
    "a reference cannot be staged while a TWR run holds the core; Stop or Reset first";

// =====================================================================================
// Cadence + safety constants
// =====================================================================================

/// How long a level / query verb parks on the service mutex before refusing by name.
///
/// The pump holds that mutex for a whole unit, and a TWR unit is a PLAY - seconds, not
/// milliseconds. Parking a status-adjacent verb behind one would make an operator's
/// level nudge look like a hung daemon. Past this the answer is a refusal that says what
/// is happening and what to send.
const LOCK_WAIT: Duration = Duration::from_millis(500);

/// The pump's idle wake-up cadence, so it notices the stop flag with an empty mailbox.
const JOB_POLL: Duration = Duration::from_millis(100);

/// The display drain thread's cadence. 40 ms = 25 publishes per second, which is the
/// same beat `sine.rs` decimates its spectra to.
const DRAIN_PERIOD: Duration = Duration::from_millis(40);

/// Both watchdogs' floor: three missed beats or this, whichever is longer.
const WATCHDOG_FLOOR_MS: u64 = 3_000;

/// The HAL's raised-cosine stop taper, seconds - the tail of the cancel arithmetic.
///
/// Mirrored from `hertzforge_hal`'s `ramp_drive_to_zero` (250 ms) because it appears inside a
/// journalled budget. It is an ALLOWANCE, not a claim about the other crate: a taper that
/// got faster would leave this pessimistic rather than wrong.
const STOP_TAPER_S: f64 = 0.250;

/// Seconds of control signal the display tap holds before it starts dropping oldest.
///
/// Sized against the drain cadence with an order of magnitude of slack: at 51.2 kHz a
/// 40 ms drain takes ~2048 samples out of a ring holding ~25 600, so an ordinary
/// scheduling hiccup costs nothing and a genuinely stalled drain reports its loss rather
/// than growing without bound.
const TAP_SECONDS: f64 = 0.5;

/// The largest reference this core will stage, samples.
///
/// A stated ceiling rather than an implicit one: 16 Mi samples is 64 MiB of `f32` on the
/// wire and 128 MiB of `f64` staged, which is already far past any real TWR record
/// (a 60 s field trace at 51.2 kHz is 3.1 M samples). `BeginReference` refuses past it
/// BEFORE a byte is sent, which is the whole reason the verb declares a count.
const MAX_REFERENCE_SAMPLES: u64 = 16 * 1024 * 1024;

// =====================================================================================
// The lock-free phase
// =====================================================================================

/// The bring-up / run PHASE, mirrored lock-free so `fill_status` never waits on the
/// pump. `sine.rs`'s rationale applies verbatim and harder: the service's own status
/// needs the service mutex, which a play holds for the length of the record.
///
/// The vocabulary is deliberately the one `sine.rs` and `srs.rs` already publish for the
/// states they share, so one panel speaks all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
enum Phase {
    /// Configured; the sysID has not run.
    #[default]
    Idle = 0,
    /// The arm block is running (station open -> configure -> gate).
    Priming = 1,
    /// The sysID excitation is driving.
    SysId = 2,
    /// SysID PASSED, parked for the operator. Releasable by `ProceedAfterReview`.
    AwaitingReview = 3,
    /// Correcting at a ladder level.
    Iterating = 4,
    /// A level exhausted its budget under the Hold policy. Releasable by a level verb.
    Holding = 5,
    /// The converged drive is running the playback schedule.
    Playing = 6,
    /// SysID FAILED the coherence floor. NOT releasable - see the module docs.
    CoherenceHold = 7,
    /// A kinematic ceiling failed before anything drove. Releasable by the override.
    FeasibilityBlocked = 8,
    /// A stop was requested and the run is coming down.
    Stopping = 9,
    /// Ended by an operator stop.
    Stopped = 10,
    /// The run schedule finished cleanly.
    Complete = 11,
    /// A guard tripped, or the operator aborted.
    Aborted = 12,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Priming,
            2 => Phase::SysId,
            3 => Phase::AwaitingReview,
            4 => Phase::Iterating,
            5 => Phase::Holding,
            6 => Phase::Playing,
            7 => Phase::CoherenceHold,
            8 => Phase::FeasibilityBlocked,
            9 => Phase::Stopping,
            10 => Phase::Stopped,
            11 => Phase::Complete,
            12 => Phase::Aborted,
            // 0 and anything else. A phase byte this host never wrote is a bug in this
            // file, and Idle is the least-wrong answer: it claims nothing is happening,
            // which is the direction that does not invite an operator to walk away.
            _ => Phase::Idle,
        }
    }

    /// The module's own fine state, in the token a front end renders verbatim. The two
    /// NAMESPACED tokens (`feasibility_blocked:` / `awaiting_review:`) are completed
    /// with their reason by the writer - see `IPC_CONTRACT.md`'s v5 table.
    fn token(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Priming => "priming",
            Phase::SysId => "sysid",
            Phase::AwaitingReview => "awaiting_review",
            Phase::Iterating => "iterating",
            Phase::Holding => "holding",
            Phase::Playing => "playing",
            Phase::CoherenceHold => "coherence_hold",
            Phase::FeasibilityBlocked => "feasibility_blocked",
            Phase::Stopping => "stopping",
            Phase::Stopped => "stopped",
            Phase::Complete => "complete",
            Phase::Aborted => "aborted",
        }
    }

    /// The coarse cross-module lifecycle this phase maps onto.
    ///
    /// **NO new `RunState` variant** (IPC_CONTRACT's v5 rule): a park is a
    /// module-specific reason for sitting still inside the lifecycle, not a lifecycle
    /// state, and `Complete` maps to `Stopped` because on the wire a finished run and a
    /// stopped one are both "ended normally" - the difference is in the token.
    fn run_state(self) -> hertzforge_protocol::RunState {
        use hertzforge_protocol::RunState as R;
        match self {
            Phase::Idle => R::Configured,
            Phase::Priming
            | Phase::SysId
            | Phase::AwaitingReview
            | Phase::Holding
            | Phase::CoherenceHold
            | Phase::FeasibilityBlocked => R::Armed,
            Phase::Iterating | Phase::Playing => R::Running,
            Phase::Stopping => R::Stopping,
            Phase::Stopped | Phase::Complete => R::Stopped,
            Phase::Aborted => R::Faulted,
        }
    }

    /// Does this phase HOLD the core?
    ///
    /// **Every park counts** - the SRS hold obligation. A park is a paused run
    /// holding a claimed core: nothing is driving and the pin is at the exact-zero pad,
    /// but a second module binding the chassis while an operator studies an FRF would
    /// take the run away from under them.
    fn is_live(self) -> bool {
        matches!(
            self,
            Phase::Priming
                | Phase::SysId
                | Phase::AwaitingReview
                | Phase::Iterating
                | Phase::Holding
                | Phase::Playing
                | Phase::CoherenceHold
                | Phase::FeasibilityBlocked
        )
    }

    /// Is this one of the two parks a VANISHED controller must leave standing?
    fn survives_vanish(self) -> bool {
        matches!(self, Phase::AwaitingReview | Phase::CoherenceHold)
    }

    fn from_state(s: TwrState) -> Phase {
        match s {
            TwrState::Configured => Phase::Idle,
            TwrState::FeasibilityBlocked => Phase::FeasibilityBlocked,
            TwrState::Iterating => Phase::Iterating,
            TwrState::Holding => Phase::Holding,
            TwrState::CoherenceHold => Phase::CoherenceHold,
            TwrState::Playing => Phase::Playing,
            TwrState::Complete => Phase::Complete,
            TwrState::Aborted => Phase::Aborted,
            TwrState::Stopped => Phase::Stopped,
        }
    }
}

// =====================================================================================
// The published snapshot + the live play cell
// =====================================================================================

/// One published snapshot of a TWR run, written by the pump at unit boundaries and read
/// by the status path WITHOUT the service mutex.
///
/// # What is deliberately NOT here, and where it went instead
///
/// The convergence numbers (rms / peak / spectral error, the clip fraction, the measured
/// lag and its provenance, the commit depth) ride `Twr(History)` rather than
/// `StatusReport`. `StatusReport` has no field that MEANS any of them, and the two
/// candidates a reader might reach for - `measured_grms` for an error PERCENTAGE,
/// `thd_pct` for a clip fraction - would put a number under a name that says something
/// else. A document that says what it carries beats a status field that does not.
#[derive(Debug, Clone, Default)]
struct TwrSnapshot {
    /// The PHASE this snapshot was written at.
    ///
    /// Carried so `fill_status` can tell whether the decorated `module_state` below is
    /// still about the phase the run is in - see the comment at its one consumer. The
    /// cell is written at unit boundaries and the phase moves whenever anything moves
    /// the run, so "written at" is a fact the snapshot has to state rather than one the
    /// reader may assume.
    phase: Phase,
    module_state: Option<String>,
    level_db: Option<f64>,
    band_coherence: Option<f64>,
    alarm_active: bool,
    abort_active: bool,
    /// Correction passes completed this RUN (not counting playback plays).
    total_iterations: u32,
    /// Correction passes at the CURRENT rung.
    iterations_at_level: u32,
    /// Playback plays completed, and how many the schedule wants.
    plays_completed: u32,
    plays_target: u32,
}

/// The cell. A `Mutex` rather than a lock-free struct because the snapshot is a dozen
/// fields written together and a torn read would put one iteration's level beside
/// another's error; the lock is held for a struct clone and never by anything that can
/// block.
#[derive(Default)]
struct TwrCell(Mutex<Option<TwrSnapshot>>);

impl TwrCell {
    fn read(&self) -> Option<TwrSnapshot> {
        self.0.lock().ok().and_then(|g| g.clone())
    }
    fn write(&self, s: TwrSnapshot) {
        if let Ok(mut g) = self.0.lock() {
            *g = Some(s);
        }
    }
    fn clear(&self) {
        if let Ok(mut g) = self.0.lock() {
            *g = None;
        }
    }
}

/// **THE LIVE HALF, lock-free**: what a play is doing RIGHT NOW.
///
/// The snapshot above is written at unit boundaries and a TWR unit is a whole record, so
/// without this the numeric status would freeze for 30 s and an operator could not tell
/// a healthy play from a wedged driver. Every field is an atomic and the writer is the
/// player's per-block observer, which runs on the pump thread and takes no lock.
#[derive(Default)]
struct PlayCell {
    /// A unit that plays is in flight.
    active: AtomicBool,
    /// Frames read back so far this play.
    frames_done: AtomicU64,
    /// Frames this play will read back in total (record + rest pad). `0` = not yet
    /// reported by the first block.
    frames_expected: AtomicU64,
    /// Unix ms of the last block the driver returned. `0` = none this unit.
    beat_ms: AtomicU64,
    /// Unix ms the pump entered the unit - the deadline's origin before any block has
    /// arrived.
    unit_started_ms: AtomicU64,
}

impl PlayCell {
    fn begin_unit(&self) {
        self.frames_done.store(0, Ordering::SeqCst);
        self.frames_expected.store(0, Ordering::SeqCst);
        self.beat_ms.store(0, Ordering::SeqCst);
        self.unit_started_ms.store(now_ms(), Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
    }

    fn end_unit(&self) {
        self.active.store(false, Ordering::SeqCst);
    }

    fn observe(&self, p: PlayProgress) {
        self.frames_done
            .store(p.frames_done as u64, Ordering::SeqCst);
        self.frames_expected
            .store(p.frames_expected as u64, Ordering::SeqCst);
        self.beat_ms.store(now_ms(), Ordering::SeqCst);
    }
}

/// Clears the priming flag on EVERY exit path, refusals and panics included.
struct PrimingGuard(Arc<AtomicBool>);

impl Drop for PrimingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// =====================================================================================
// The host
// =====================================================================================

/// The daemon's TWR host.
pub struct TwrHost {
    svc: Arc<Mutex<Option<Svc>>>,
    jobs: Arc<(Mutex<VecDeque<Job>>, Condvar)>,
    /// The lock-free phase mirror. See [`Phase`].
    phase: Arc<AtomicU8>,
    /// True while the arm block is in flight (station open -> configure -> gate).
    priming: Arc<AtomicBool>,
    /// True from the moment a stop is REQUESTED until the pump exits; both watchdogs
    /// are exempt while it is set.
    stopping: Arc<AtomicBool>,
    /// The pump's shutdown flag.
    stop: Arc<AtomicBool>,
    /// **The PLAYER's cancel flag, owned by the host for the life of the service.**
    ///
    /// Owned here and handed to the player at construction, not fetched on demand, and
    /// that is the whole point: a play holds the service mutex for its whole duration,
    /// so a Stop that had to take the mutex to find the flag could not raise it until
    /// the thing it is cancelling had already finished.
    cancel: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The display drain thread (see the module docs).
    drain: Mutex<Option<std::thread::JoinHandle<()>>>,
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    cell: Arc<TwrCell>,
    play: Arc<PlayCell>,
    /// A refusal that killed the run OUTSIDE the service's own state machine - a failed
    /// arm, a panicking pump (the out-of-band fault rule for the paths the state
    /// machine cannot latch).
    fault: Arc<Mutex<Option<String>>>,
    /// Why the run is HOLDING, verbatim; `None` = not holding. Fills the wire's `hold`.
    hold: Arc<Mutex<Option<String>>>,
    /// **The HOST-side post-sysID park is ARMED**: the profile asked for it and the
    /// sysID has not been reviewed yet. Cleared by `ProceedAfterReview`.
    review_armed: Arc<AtomicBool>,
    /// A built service exists. Lock-free, so a status read never waits on the service
    /// mutex.
    configured: Arc<AtomicBool>,
    provenance: Mutex<Option<RunProvenance>>,
    /// The station class the service was built on - what the NI-REAL ledger guard reads.
    station_class: Mutex<Option<StationClass>>,
    /// The STAGED reference record, RAW. Survives `SetProfile`, dies at `Reset`.
    staged: Mutex<Option<StagedReference>>,
    /// An upload in progress.
    staging: Mutex<Option<Staging>>,
    /// The convergence history, PRE-SERIALIZED at the last unit boundary.
    ///
    /// A mirror for the same reason the SRS notch readout is one: `History` is
    /// polled beside the status, a play holds the service mutex for the length of a
    /// record, and a query parked behind one would make a 200 ms poll look like a hung
    /// daemon.
    history_json: Arc<Mutex<String>>,
    /// The APPLIED prep echo, pre-serialized at configure. See `reference_info_doc`.
    applied_prep_json: Arc<Mutex<Option<String>>>,
    /// The rate this run ASKED the station for, f64 bits; `0` = nothing configured.
    requested_fs_bits: Arc<AtomicU64>,
    /// The rate the station actually PROGRAMMED, f64 bits; `0` = not yet known.
    actual_fs_bits: Arc<AtomicU64>,
    /// The DRIVER-beat threshold, ms - derived at arm from the adopted rate and block.
    driver_deadline_ms: Arc<AtomicU64>,
    /// The COMPUTE deadline, ms - derived at arm from the adopted rate and grid length.
    compute_deadline_ms: Arc<AtomicU64>,
    /// Samples the display tap has dropped this run.
    tap_dropped: Arc<AtomicU64>,
    /// The daemon's controller timeout, seconds - the window a vanished controller's
    /// safe-stop has to finish inside.
    controller_timeout_s: f64,
    kind: BackendKind,
    ni_devices: Vec<String>,
}

/// One unit of work the pump performs under the service mutex.
#[derive(Debug, Clone)]
enum Job {
    /// `Command::Start`: arm the path and run the sysID pass.
    Arm,
    /// Release the HOST-side post-sysID park and carry on.
    ProceedAfterReview,
    /// One correction pass.
    IterateOnce,
    /// Hand the converged drive to the playback schedule and run it.
    BeginPlayback,
    /// Release the feasibility park with an acknowledged override, then arm.
    Override {
        ack: String,
    },
    Stop {
        reason: String,
    },
    Abort {
        reason: String,
    },
    Reset,
}

impl TwrHost {
    /// A host bound to a station kind and the shared display bus. Builds nothing until
    /// [`TwrHost::set_profile`].
    ///
    /// **NO `data_root`, unlike every other host here, and that is the remote-capture
    /// refusal showing up in the signature.** The other modules take one because they
    /// can be asked to write a capture or a snapshot to it. TWR cannot: its capture
    /// block is absent from the wire profile by design, the translation FORCES
    /// `auto_start = false`, and no verb on this seam reaches
    /// `TwrService::start_capture`. A path this host could never write to would be a
    /// parameter that looks like a capability.
    pub fn new(
        kind: BackendKind,
        ni_devices: Vec<String>,
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
        controller_timeout_s: f64,
    ) -> Self {
        TwrHost {
            svc: Arc::new(Mutex::new(None)),
            jobs: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
            phase: Arc::new(AtomicU8::new(Phase::Idle as u8)),
            priming: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            drain: Mutex::new(None),
            tx,
            cell: Arc::new(TwrCell::default()),
            play: Arc::new(PlayCell::default()),
            fault: Arc::new(Mutex::new(None)),
            hold: Arc::new(Mutex::new(None)),
            review_armed: Arc::new(AtomicBool::new(false)),
            configured: Arc::new(AtomicBool::new(false)),
            provenance: Mutex::new(None),
            station_class: Mutex::new(None),
            staged: Mutex::new(None),
            staging: Mutex::new(None),
            history_json: Arc::new(Mutex::new(empty_history_json())),
            applied_prep_json: Arc::new(Mutex::new(None)),
            requested_fs_bits: Arc::new(AtomicU64::new(0)),
            actual_fs_bits: Arc::new(AtomicU64::new(0)),
            driver_deadline_ms: Arc::new(AtomicU64::new(WATCHDOG_FLOOR_MS)),
            compute_deadline_ms: Arc::new(AtomicU64::new(WATCHDOG_FLOOR_MS)),
            tap_dropped: Arc::new(AtomicU64::new(0)),
            controller_timeout_s,
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

    /// Is a run LIVE? Every park counts - see [`Phase::is_live`].
    pub fn is_live(&self) -> bool {
        self.priming.load(Ordering::SeqCst) || self.phase().is_live()
    }

    /// Is anything configured at all? Lock-free.
    pub fn is_configured(&self) -> bool {
        self.configured.load(Ordering::SeqCst)
    }

    /// Is the run parked at the post-sysID review hold?
    pub fn at_review_hold(&self) -> bool {
        self.phase() == Phase::AwaitingReview
    }

    // =================================================================================
    // Configure
    // =================================================================================

    /// Translate the wire profile against the STAGED reference, build the station,
    /// construct the service, start the pump and the display drain.
    ///
    /// # The safe-stop arithmetic, stated - and why there is NO record-length refusal
    ///
    /// `ShockHost::set_profile` refuses a profile whose single pulse outlasts the
    /// controller timeout. **TWR deliberately does not**, and the difference is worth
    /// writing down so nobody "helpfully" adds one:
    ///
    /// ```text
    /// t_cancel = (frames_committed - frames_done) / fs + 250 ms
    /// ```
    ///
    /// `play_block_wise` checks the cancel flag once per BLOCK and what the device has
    /// already committed plays out - mid-record preemption is refused by the hardware
    /// (-200289) - so the latency is bounded by the device's OUTPUT BUFFER and by the
    /// HAL's taper, and by nothing else. It does not scale with the record. A 60-second
    /// field trace therefore cancels in the same time a 1-second one does, and refusing
    /// long records against a 30 s timeout would refuse exactly the runs this module
    /// exists for.
    ///
    /// What IS checked is the part that can be: the taper alone against the timeout at
    /// configure (below), and the OBSERVED commit depth after every play, which the pump
    /// journals and the status carries. The observed number is the honest authority -
    /// the AO buffer depth is not a declared capability.
    pub fn set_profile(
        &self,
        profile: TwrProfile,
        table: ChannelTable,
        lease_owner: &str,
    ) -> Result<(), ProtocolError> {
        if self.is_live() {
            return Err(ProtocolError::Refused {
                message: "a TWR run is live on this core; Stop it before reconfiguring (a run \
                          parked at the review hold, the coherence hold or the feasibility \
                          block is LIVE - it holds the chassis - so Stop or Twr(Reset) first)"
                    .into(),
            });
        }

        // ---- THE STAGED RECORD, named by digest -------------------------------------
        let staged = {
            let g = self.staged.lock().expect("staged poisoned");
            match g.as_ref() {
                Some(r) if r.sha256 == profile.reference_sha => r.clone(),
                Some(r) => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "this profile names reference sha {} and the record staged on this \
                             core is {}. A profile is never run against whatever happens to be \
                             loaded: stage the record it names (BeginReference / ReferenceChunk \
                             / CommitReference) and configure again.",
                            short_sha(&profile.reference_sha),
                            short_sha(&r.sha256)
                        ),
                    })
                }
                None => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "no reference record is staged on this core, and this profile names \
                             sha {}. The record crosses ONCE on its own verbs \
                             (BeginReference -> ReferenceChunk x N -> CommitReference) and the \
                             profile names it by digest; send the upload first.",
                            short_sha(&profile.reference_sha)
                        ),
                    })
                }
            }
        };

        self.shutdown_workers();
        *self.svc.lock().expect("svc poisoned") = None;

        // ---- the station ------------------------------------------------------------
        //
        // The RAW record's rate is what the HERTZFORGE-SIM plant is built at, so a Sim run
        // needs no resample; an NI task is built at NI_BUILD_RATE_HZ and coerces from
        // there at configure, exactly as every other module's does.
        let build_fs = match self.kind {
            BackendKind::Ni => hertzforge_station::NI_BUILD_RATE_HZ,
            _ => staged.fs_hz,
        };
        let station = build_twr_station(self.kind, &self.ni_devices, &table, build_fs)?;
        tracing::info!(target: "hertzforge::backendd", "{}", station.provenance.log_line());
        let station_class = station.station;

        // ---- the taper against the controller timeout -------------------------------
        if STOP_TAPER_S >= self.controller_timeout_s {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "this daemon's controller timeout is {:.1} s and the HAL's stop taper alone \
                     is {STOP_TAPER_S:.3} s, so a vanished controller's safe-stop could not \
                     finish inside its own window. Raise controller_timeout_s.",
                    self.controller_timeout_s
                ),
            });
        }

        let cfg = translate_twr(&profile, &staged)?;
        let run_id = mint_run_id();
        let prov = RunProvenance::assemble("twr", run_id, &station, lease_owner, now_unix());
        tracing::info!(target: "hertzforge::backendd", "{}", prov.log_line());

        // ---- the player: cancel flag, display tap, play observer --------------------
        //
        // All three handles are taken BEFORE the player is moved into the service, which
        // is the only moment they can be: the service owns it by value afterwards.
        self.cancel.store(false, Ordering::SeqCst);
        let tap_capacity = ((build_fs * TAP_SECONDS).ceil() as usize).clamp(1024, 1 << 20);
        let play_cell = Arc::clone(&self.play);
        let player = BackendReferencePlayer::new(station.backend, table, station_class)
            .map_err(twr_err)?
            .with_cancel_flag(Arc::clone(&self.cancel))
            .with_display_tap(tap_capacity)
            .with_play_observer(Arc::new(move |p| play_cell.observe(p)));
        let tap = player.display_tap_handle();

        let svc = TwrService::new(player, cfg).map_err(twr_err)?;
        let st = svc.status();

        // ---- state that must not outlive the previous run ---------------------------
        self.stopping.store(false, Ordering::SeqCst);
        self.review_armed
            .store(profile.review_after_sysid, Ordering::SeqCst);
        self.play.end_unit();
        self.tap_dropped.store(0, Ordering::SeqCst);
        *self.fault.lock().expect("fault poisoned") = None;
        self.clear_hold();
        *self.history_json.lock().expect("history poisoned") = empty_history_json();
        *self.applied_prep_json.lock().expect("prep poisoned") =
            Some(applied_prep_json(&svc, &staged));
        self.requested_fs_bits
            .store(st.requested_fs_hz.to_bits(), Ordering::SeqCst);
        // Nothing has been adopted yet; the arm is the only moment that number exists.
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        self.driver_deadline_ms
            .store(WATCHDOG_FLOOR_MS, Ordering::SeqCst);
        self.compute_deadline_ms
            .store(WATCHDOG_FLOOR_MS, Ordering::SeqCst);
        self.set_phase(Phase::from_state(st.state));
        self.cell.write(snapshot_of(&svc, self.phase(), None));
        if st.state == TwrState::FeasibilityBlocked {
            self.set_hold(feasibility_hold_text(&svc));
        }

        *self.svc.lock().expect("svc poisoned") = Some(svc);
        *self.provenance.lock().expect("provenance poisoned") = Some(prov);
        *self.station_class.lock().expect("station poisoned") = Some(station_class);
        self.configured.store(true, Ordering::SeqCst);
        self.spawn_pump();
        self.spawn_drain(tap, run_id);
        Ok(())
    }

    /// Drop a BUILT-BUT-IDLE TWR run so this host stops holding the core.
    ///
    /// Called only by `Core::release_idle_claims`, which asks `holds(Twr, Live)` first.
    /// The guard below is still real: the phase mirror is the authority on what "idle"
    /// means here, and EVERY park reads LIVE (see [`Phase::is_live`]), so a caller that
    /// got it wrong must not silently drop a parked run that is holding a measured
    /// system ID.
    ///
    /// **The STAGED RECORD SURVIVES**, and that is deliberate rather than an omission:
    /// the record is not the run. An operator who staged a 60 s field trace, configured
    /// TWR, then opened the Sine panel must not have to re-upload it. `Twr(Reset)` is
    /// the verb that discards it.
    pub fn release_idle(&self) -> bool {
        if self.is_live() || !self.is_configured() {
            return false;
        }
        self.teardown("released as a parked configured-but-idle claim");
        true
    }

    // =================================================================================
    // Start + the parks
    // =================================================================================

    /// `Command::Start` **IS ARM-AND-SYSTEM-ID** for this module.
    ///
    /// The cheap refusals are synchronous - not configured, wrong state, no chassis
    /// ledger - so the operator gets an immediate specific "no" for everything decidable
    /// without driving. The sysID pass is NOT: it is `sysid.duration_s` of excitation
    /// plus an FFT, so it runs on the pump and the PHASE reports its progress.
    pub fn start(&self) -> Result<(), ProtocolError> {
        self.require_chassis_ledger()?;
        {
            let guard = self.try_lock_svc(LOCK_WAIT).ok_or_else(|| busy("Start"))?;
            let svc = guard.as_ref().ok_or_else(not_configured)?;
            match svc.state() {
                TwrState::Configured => {}
                TwrState::FeasibilityBlocked => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "arming is HARD-BLOCKED: {}. Send SetLevel-free Twr(ArmWithOverride) \
                             - which on this seam is Sine(OverrideFeasibility)'s TWR analogue, \
                             the general ProceedAfterReview does NOT release a feasibility park \
                             - or fix the reference and the ceilings.",
                            feasibility_summary(svc)
                        ),
                    })
                }
                other => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "the TWR run cannot Start from {}; send Twr(Reset) to drop the \
                             identification and start a fresh run, or SetProfile to rebuild it",
                            state_word(other)
                        ),
                    })
                }
            }
        }
        self.priming.store(true, Ordering::SeqCst);
        self.set_phase(Phase::Priming);
        self.post(Job::Arm);
        Ok(())
    }

    /// Release the FEASIBILITY park with a non-empty operator acknowledgement.
    ///
    /// The ack is the record of WHO decided to drive past a refusal, so an empty one is
    /// refused by the service - a required field nobody has to fill in is not a gate.
    pub fn override_feasibility(&self, ack: &str) -> Result<(), ProtocolError> {
        self.require_chassis_ledger()?;
        if ack.trim().is_empty() {
            return Err(ProtocolError::InvalidArgument {
                message: "the TWR feasibility override needs a non-empty acknowledgement: it is \
                          the operator's signed statement that they read the reason and are \
                          proceeding anyway, and an empty one is a gate that gates nothing"
                    .into(),
            });
        }
        {
            let guard = self
                .try_lock_svc(LOCK_WAIT)
                .ok_or_else(|| busy("the feasibility override"))?;
            let svc = guard.as_ref().ok_or_else(not_configured)?;
            if svc.state() != TwrState::FeasibilityBlocked {
                return Err(ProtocolError::Refused {
                    message: format!(
                        "the feasibility override is for a kinematic EXCEEDANCE and this run is \
                         {}; send Start to arm",
                        state_word(svc.state())
                    ),
                });
            }
        }
        self.priming.store(true, Ordering::SeqCst);
        self.set_phase(Phase::Priming);
        self.post(Job::Override {
            ack: ack.to_string(),
        });
        // The feasibility park does NOT exit the pump (it is entered at configure, before
        // any unit), so this is a no-op on the normal path. It is here so the release
        // verb does not depend on that staying true - `spawn_pump` short-circuits on a
        // live pump and reaps a finished one.
        self.spawn_pump();
        Ok(())
    }

    /// Release the HOST-side post-sysID review park (`ProceedAfterReview`).
    ///
    /// **From `CoherenceHold` this refuses with the SERVICE's own sentence, verbatim**
    /// (the `sine.rs` barrier, restated). Paraphrasing a safety refusal here would be a
    /// second copy of it, and the copy is the one that drifts. The service's `advance()`
    /// carries that sentence, so it is fetched from there rather than written out.
    pub fn proceed_after_review(&self) -> Result<(), ProtocolError> {
        match self.phase() {
            Phase::AwaitingReview => {}
            Phase::CoherenceHold => {
                return Err(ProtocolError::Refused {
                    message: coherence_hold_refusal(),
                })
            }
            other => {
                return Err(ProtocolError::Refused {
                    message: format!(
                        "there is no post-sysID review hold to release: this TWR run is {}. The \
                         hold is entered only when a PASSING system ID parks for review.",
                        other.token()
                    ),
                })
            }
        }
        self.post(Job::ProceedAfterReview);
        // THE PUMP EXITED AT THE PARK, so the verb that releases it has to bring it
        // back. This is the whole reason the park is modelled as a pump exit rather than
        // as a wait inside the loop: a lock-held wait would hold the service mutex
        // across the park and this call would deadlock against the thread it releases.
        self.spawn_pump();
        Ok(())
    }

    /// One correction pass (`Twr(IterateOnce)`).
    pub fn iterate_once(&self) -> Result<(), ProtocolError> {
        self.require_iterating("IterateOnce")?;
        self.post(Job::IterateOnce);
        Ok(())
    }

    /// Hand the CONVERGED drive to the playback schedule and run it.
    pub fn begin_playback(&self) -> Result<(), ProtocolError> {
        self.require_iterating("BeginPlayback")?;
        self.post(Job::BeginPlayback);
        Ok(())
    }

    /// Step the ladder one rung. SYNCHRONOUS, and the echo is the level REACHED.
    ///
    /// Synchronous because the ceiling check lives inside the service's `scale_drive_db`
    /// and a queued step could only report its refusal minutes later, in a status field
    /// nobody was watching. The refusal names both numbers.
    pub fn step_level(&self, up: bool) -> Result<f64, ProtocolError> {
        let mut guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy("a level step"))?;
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        if up {
            svc.step_level_up().map_err(twr_err)?;
        } else {
            svc.step_level_down().map_err(twr_err)?;
        }
        let reached = svc.status().level_db;
        self.publish_after_verb(svc);
        Ok(reached)
    }

    /// Jump to a level-table index. SYNCHRONOUS; see [`TwrHost::step_level`].
    pub fn jump_level(&self, index: usize) -> Result<f64, ProtocolError> {
        let mut guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy("a level jump"))?;
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        let rungs = svc.config().schedule.table_db.len();
        if index >= rungs {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "level index {index} is not a rung of the configured ladder ({rungs} rung(s))"
                ),
            });
        }
        svc.jump_to_level(index).map_err(twr_err)?;
        let reached = svc.status().level_db;
        self.publish_after_verb(svc);
        Ok(reached)
    }

    /// Freeze / unfreeze the correction law, LIVE.
    pub fn freeze_adaptation(&self, frozen: bool) -> Result<(), ProtocolError> {
        let mut guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy("the adaptation freeze"))?;
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        svc.freeze_adaptation(frozen);
        self.publish_after_verb(svc);
        Ok(())
    }

    /// `Twr(Reset)`: take the run down, drop the service, discard the staged record and
    /// **RELEASE THE CORE**.
    ///
    /// # Three things it is, in the order they matter
    ///
    /// 1. **THE ONLY RELEASE FOR `CoherenceHold`.** The service's own `reset()` is
    ///    documented as that park's single exit, and until this verb existed the hold was
    ///    unreachable-in-anger because nothing called it.
    /// 2. **The core is RELEASED** (the verb precedent both shock groups learned on
    ///    their first wire run): a module whose service is still present holds the core at
    ///    `Claim::Configured` even after its run has stopped, so a bare `Start` reaches it
    ///    and answers about a module the operator had already moved on from.
    /// 3. **The staged record is DISCARDED.** Reset means "I am done with this module",
    ///    and leaving a 60 s record staged after it would make the next `SetProfile`
    ///    succeed against a reference nobody re-chose. `release_idle` deliberately keeps
    ///    it; this deliberately does not.
    pub fn reset(&self) -> Result<(), ProtocolError> {
        if !self.is_configured() && self.staged.lock().expect("staged poisoned").is_none() {
            return Err(not_configured());
        }
        self.teardown("Twr(Reset)");
        *self.staged.lock().expect("staged poisoned") = None;
        *self.staging.lock().expect("staging poisoned") = None;
        Ok(())
    }

    // =================================================================================
    // Stop / abort / safe-stop
    // =================================================================================

    /// THE CONTROLLED STOP - the OPERATOR's, which DOES end a park.
    ///
    /// The cancel flag is raised FIRST and OUTSIDE the mutex, because it is the only
    /// thing that reaches a play already in flight. The queued job does the service-side
    /// `stop()` once the play unwinds.
    pub fn operator_stop(&self, reason: &str) {
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        if self.phase().is_live() {
            self.set_phase(Phase::Stopping);
        }
        self.post(Job::Stop {
            reason: reason.to_string(),
        });
    }

    /// THE SAFE-STOP PATH, invoked when the CONTROLLER times out.
    ///
    /// **A run parked at the review hold or the coherence hold is NOT stopped by this**
    /// (the SRS review-hold obligation): the park already stopped the tasks, the pin is
    /// at the exact-zero pad, and a safe-stop that tore the park down would throw away
    /// a measured system ID because a network went quiet. The park stands until the
    /// operator proceeds or explicitly stops.
    ///
    /// The FEASIBILITY park is deliberately NOT in that set: nothing has driven there
    /// either, but it holds no measurement worth preserving, so the ordinary
    /// nothing-to-do path applies.
    pub fn safe_stop(&self, reason: &str) {
        if self.phase().survives_vanish() {
            tracing::info!(
                target: "hertzforge::backendd",
                reason,
                phase = self.phase().token(),
                "SAFE-STOP reached a TWR run at a PARK: nothing is driving and the plant is \
                 already parked, so the hold STANDS"
            );
            return;
        }
        if !self.is_live() && !self.is_configured() {
            return;
        }
        tracing::warn!(
            target: "hertzforge::backendd",
            reason,
            "SAFE-STOP: cancelling the TWR run at the next block boundary"
        );
        self.operator_stop(reason);
    }

    /// ABORT: `emergency_stop`, immediate.
    ///
    /// The arithmetic is not hidden: mid-record preemption is REFUSED by this chassis
    /// (-200289), so whatever the device has already committed plays out under the 25 ms
    /// emergency taper rather than the 250 ms stop one. A TWR abort is a promise about
    /// the committed remainder, not about the next sample.
    pub fn abort(&self, reason: &str) {
        tracing::warn!(target: "hertzforge::backendd", reason, "ABORT: TWR");
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.post(Job::Abort {
            reason: reason.to_string(),
        });
    }

    // =================================================================================
    // Status
    // =================================================================================

    /// Fold this module's truth into a protocol status report.
    ///
    /// **This path never takes the service mutex**. The phase is an atomic, the
    /// rate pair is two atomics, the live play counters are four atomics, and the rest
    /// comes out of the published snapshot under a lock held for a struct clone.
    pub fn fill_status(&self, s: &mut StatusReport) {
        self.fill_rates(s);
        if !self.is_configured() {
            return;
        }
        // FIRST: a refusal OUTSIDE the service's own state machine (a failed arm,
        // a panicking pump). Without it the wire would report "configured" for a run
        // whose station refused to open.
        if let Some(f) = self.fault.lock().ok().and_then(|f| f.clone()) {
            s.run = hertzforge_protocol::RunState::Faulted;
            s.module_state = Some(f);
            return;
        }
        let phase = self.phase();
        s.hold = self.hold.lock().ok().and_then(|h| h.clone());
        s.display_blocks_dropped = Some(self.tap_dropped.load(Ordering::SeqCst));

        if self.priming.load(Ordering::SeqCst) {
            s.run = hertzforge_protocol::RunState::Armed;
            s.module_state = Some(
                "priming (opening the station, configuring it, adopting the rate the driver \
                 programmed, consulting the arm gate and deriving the alignment baseline - the \
                 sysID excitation begins when this reports sysid)"
                    .into(),
            );
            return;
        }

        // THE TWO WATCHDOGS. Each is armed only where it can be judged; see the module
        // docs. A stop in flight exempts both - the taper and the committed remainder are
        // not a stall.
        if let Some(text) = self.watchdog_verdict(phase) {
            s.run = hertzforge_protocol::RunState::Faulted;
            s.module_state = Some(text);
            return;
        }

        s.run = phase.run_state();
        s.module_state = Some(phase.token().to_string());
        let snap = self.cell.read().unwrap_or_default();
        // **THE SNAPSHOT DECORATES THE PHASE; IT DOES NOT OVERRIDE IT.**
        //
        // The cell carries the NAMESPACED tokens with their reasons (IPC_CONTRACT v5's
        // table) and the terminal ones with theirs, but it is written at UNIT
        // BOUNDARIES while the phase moves whenever anything moves the run - and the
        // two therefore go out of step in exactly one situation, which is the one that
        // matters most.
        //
        // MEASURED, not reasoned (red-mutation run, 2026-08-09): with `safe_stop`'s
        // park early-return removed, a vanished controller stopped a run parked at the
        // review hold. `operator_stop` moved the phase to `stopping`, the pump had
        // already EXITED at the park so nothing rewrote the cell, and an unconditional
        // override went on reporting `awaiting_review: system ID passed ...` for the
        // remaining 20 seconds. The wire would have told an operator their run was
        // holding while the daemon had torn it down - and it would have done so most
        // convincingly at a park, which is the one state where nobody is watching a
        // number change.
        //
        // So the decoration applies only while the snapshot is still ABOUT the current
        // phase. When they disagree the phase wins and the plain token goes out, which
        // is less detail and no lie.
        if let Some(m) = snap.module_state.as_ref() {
            if snap.phase == phase {
                s.module_state = Some(m.clone());
            }
        }
        s.level_db = snap.level_db;
        s.band_coherence = snap.band_coherence;
        s.alarm_active = snap.alarm_active;
        s.abort_active = snap.abort_active;
        // THE DISCRETE-UNIT COUNTERS, on the transient producers' three fields.
        //
        // Stated because the field names do not say it: TWR's unit is a CORRECTION PASS,
        // not a pulse. The three rows are reused rather than three new v6 wire fields
        // added for one module, and the reuse is a wire-shape economy - it is NOT a
        // claim that TWR fires pulses. `train_remaining` is the PLAYBACK schedule's
        // remainder and is absent until the schedule is running, because "0 remaining"
        // and "no schedule yet" are different facts.
        s.pulses_fired = Some(snap.total_iterations);
        s.pulses_at_level = Some(snap.iterations_at_level);
        s.train_remaining = (phase == Phase::Playing)
            .then(|| snap.plays_target.saturating_sub(snap.plays_completed));

        // THE LIVE HALF: while a play is in flight the observer's counters are the
        // authority, because the snapshot is written at unit boundaries and a TWR unit is
        // a whole record. `elapsed_s` / `remaining_s` are the play's own
        // clock, denominated in the ADOPTED rate.
        if self.play.active.load(Ordering::SeqCst) {
            let done = self.play.frames_done.load(Ordering::SeqCst);
            let want = self.play.frames_expected.load(Ordering::SeqCst);
            let fs = f64::from_bits(self.actual_fs_bits.load(Ordering::SeqCst));
            if fs.is_finite() && fs > 0.0 {
                s.elapsed_s = Some(done as f64 / fs);
                if want > done {
                    s.remaining_s = Some((want - done) as f64 / fs);
                }
            }
        }
    }

    /// The requested/actual sample-rate pair, lock-free. `0` bits = unknown, which
    /// crosses the wire as `None` rather than as a number.
    fn fill_rates(&self, s: &mut StatusReport) {
        let read = |bits: &AtomicU64| -> Option<f64> {
            let v = f64::from_bits(bits.load(Ordering::SeqCst));
            (v.is_finite() && v > 0.0).then_some(v)
        };
        s.requested_fs_hz = read(&self.requested_fs_bits);
        s.actual_fs_hz = read(&self.actual_fs_bits);
    }

    /// The watchdog verdict, or `None` when nothing is being judged.
    ///
    /// TWO deadlines, one function, because they are two arms of one question ("is this
    /// unit still making progress") and splitting them across two call sites is how the
    /// exemptions drift apart.
    fn watchdog_verdict(&self, phase: Phase) -> Option<String> {
        if self.stopping.load(Ordering::SeqCst) || !self.play.active.load(Ordering::SeqCst) {
            return None;
        }
        let done = self.play.frames_done.load(Ordering::SeqCst);
        let want = self.play.frames_expected.load(Ordering::SeqCst);
        let beat = self.play.beat_ms.load(Ordering::SeqCst);
        let since = now_ms().saturating_sub(if beat != 0 {
            beat
        } else {
            self.play.unit_started_ms.load(Ordering::SeqCst)
        });
        // BLOCKS STILL OWED -> the DRIVER beat. No block yet is the same question: the
        // first one is owed too.
        if want == 0 || done < want {
            let deadline = self.driver_deadline_ms.load(Ordering::SeqCst).max(1);
            if since > deadline {
                return Some(format!(
                    "stalled_driver ({:.1} s without a block during a {} play, expected one \
                     every {:.3} s at {} of {} frames - the NI runtime is not returning; \
                     restart the daemon, and the chassis if preflight then fails)",
                    since as f64 / 1e3,
                    phase.token(),
                    deadline as f64 / 3e3,
                    done,
                    want
                ));
            }
            return None;
        }
        // EVERY BLOCK IN AND THE UNIT HAS NOT RETURNED -> the between-play COMPUTE.
        let deadline = self.compute_deadline_ms.load(Ordering::SeqCst).max(1);
        if since > deadline {
            return Some(format!(
                "stalled_compute ({:.1} s since the last block of a completed {} play, budget \
                 {:.1} s - the record is played and the inverse-filter maths has not returned. \
                 This is a HOST fault, not a driver one: the drive is at the exact-zero pad and \
                 the tasks are parked.)",
                since as f64 / 1e3,
                phase.token(),
                deadline as f64 / 1e3
            ));
        }
        None
    }

    // =================================================================================
    // Documents
    // =================================================================================

    /// `Twr(ReferenceInfo)`: the staged manifest plus the APPLIED prep.
    ///
    /// Answered out of MIRRORS and never from the service: the two peers who
    /// most need it are an observer that joined mid-run and a controller that
    /// reconnected after a vanish, and both would otherwise park behind a play.
    pub fn reference_info_doc(&self) -> Result<Doc, ProtocolError> {
        let staged = self.staged.lock().expect("staged poisoned").clone();
        let applied = self
            .applied_prep_json
            .lock()
            .expect("prep poisoned")
            .clone();
        let Some(r) = staged else {
            return Err(ProtocolError::Refused {
                message: "no reference record is staged on this core. Send BeginReference -> \
                          ReferenceChunk x N -> CommitReference; ReferenceInfo then answers the \
                          manifest and, once a profile is configured, the prep actually applied."
                    .into(),
            });
        };
        let applied: serde_json::Value = applied
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or(serde_json::Value::Null);
        Ok(Doc::new(
            SchemaKind::TwrReference,
            serde_json::json!({
                "staged": r.manifest_json_value(),
                // ABSENT until a profile is configured, and absent is the honest answer:
                // the prep is the PROFILE's, so before one exists there is no applied
                // prep to echo - not an empty one.
                "applied": applied,
            })
            .to_string(),
        ))
    }

    /// `Twr(History)`: the per-iteration convergence table, out of the mirror.
    pub fn history_doc(&self) -> Result<Doc, ProtocolError> {
        if !self.is_configured() {
            return Err(not_configured());
        }
        let j = self.history_json.lock().expect("history poisoned").clone();
        Ok(Doc::new(SchemaKind::TwrReference, j))
    }

    // =================================================================================
    // The reference upload
    // =================================================================================

    /// `Twr(BeginReference)`: declare what is coming, so the core can refuse a record it
    /// cannot hold before any of it is sent.
    pub fn begin_reference(
        &self,
        name: String,
        fs_hz: f64,
        eu: String,
        total_samples: u64,
        sha256: String,
    ) -> Result<(), ProtocolError> {
        self.require_not_live()?;
        if !(fs_hz.is_finite() && fs_hz > 0.0) {
            return Err(ProtocolError::InvalidArgument {
                message: format!("a reference record cannot be sampled at {fs_hz} Hz"),
            });
        }
        if total_samples == 0 || total_samples > MAX_REFERENCE_SAMPLES {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "total_samples {total_samples} is outside 1..={MAX_REFERENCE_SAMPLES}: this \
                     core states its ceiling up front rather than accepting a record it will \
                     run out of memory holding"
                ),
            });
        }
        if !digest::is_lowercase_hex_sha256(&sha256) {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "sha256 {sha256:?} is not 64 lowercase hex characters. The digest is what \
                     the profile names the record by, so a malformed one would stage a record \
                     no profile could ever address."
                ),
            });
        }
        // THE DOWNWARD-RESAMPLE REFUSAL, at UPLOAD time. Linear
        // interpolation has no anti-alias filter, so converging on a downsampled target
        // is converging on an aliased spectrum. The station's BUILD rate is what this
        // core can compare against before anything is configured; the authoritative
        // check runs again at arm against the rate the device actually programs, and it
        // is said here so the operator learns at upload rather than minutes later.
        let build_fs = match self.kind {
            BackendKind::Ni => hertzforge_station::NI_BUILD_RATE_HZ,
            _ => fs_hz,
        };
        if fs_hz > build_fs * (1.0 + 1e-9) {
            return Err(ProtocolError::Refused {
                message: format!(
                    "this record is sampled at {fs_hz} Hz and this core's station runs at \
                     {build_fs} Hz, so replaying it would need a DOWNWARD resample. That is \
                     refused: the resampler is linear interpolation with no anti-alias filter, \
                     and a test that converges on an aliased target passes against the wrong \
                     spectrum. Decimate the record properly before uploading it. (The same \
                     check runs again at arm against the rate the device actually programs.)"
                ),
            });
        }
        let previous = self
            .staging
            .lock()
            .expect("staging poisoned")
            .replace(Staging::new(name, fs_hz, eu, total_samples, sha256));
        if let Some(p) = previous {
            tracing::info!(
                target: "hertzforge::backendd",
                had_samples = p.received_samples(),
                "TWR BeginReference superseded an incomplete staging"
            );
        }
        Ok(())
    }

    /// `Twr(ReferenceChunk)`: one chunk, STRICT `seq + 1`.
    pub fn reference_chunk(&self, seq: u32, samples_b64_f32le: &str) -> Result<(), ProtocolError> {
        self.require_not_live()?;
        let mut g = self.staging.lock().expect("staging poisoned");
        let st = g.as_mut().ok_or_else(|| ProtocolError::Refused {
            message: "no reference upload is in progress: send BeginReference first, which \
                      declares the sample count and the digest this chunk will be checked \
                      against."
                .into(),
        })?;
        st.accept_chunk(seq, samples_b64_f32le)
    }

    /// `Twr(CommitReference)`: assert the COUNT and the DIGEST, then publish the record.
    pub fn commit_reference(&self) -> Result<Doc, ProtocolError> {
        self.require_not_live()?;
        let staging = self
            .staging
            .lock()
            .expect("staging poisoned")
            .take()
            .ok_or_else(|| ProtocolError::Refused {
                message: "no reference upload is in progress; there is nothing to commit.".into(),
            })?;
        // A FAILED commit consumes the staging deliberately: a half-checked record left
        // resumable is a record a second commit could accept by accident.
        let staged = staging.finish()?;
        let doc = Doc::new(SchemaKind::TwrReference, staged.commit_json());
        tracing::info!(
            target: "hertzforge::backendd",
            sha = %short_sha(&staged.sha256),
            samples = staged.samples.len(),
            fs_hz = staged.fs_hz,
            "TWR reference staged"
        );
        *self.staged.lock().expect("staged poisoned") = Some(staged);
        Ok(doc)
    }

    /// The upload's not-live gate. `is_mutating` is the CONTROLLER line, not a run-state
    /// line, so the "not while a run is live" rule is the daemon's own.
    fn require_not_live(&self) -> Result<(), ProtocolError> {
        if self.is_live() {
            return Err(ProtocolError::Refused {
                message: UPLOAD_NOT_WHILE_LIVE.into(),
            });
        }
        Ok(())
    }

    // =================================================================================
    // Internals
    // =================================================================================

    fn require_chassis_ledger(&self) -> Result<(), ProtocolError> {
        if TWR_CHASSIS_LEDGER_DATE.is_some() {
            return Ok(());
        }
        let class = *self.station_class.lock().expect("station poisoned");
        if class == Some(StationClass::NiReal) {
            return Err(ProtocolError::Refused {
                message: NO_CHASSIS_LEDGER.into(),
            });
        }
        Ok(())
    }

    fn require_iterating(&self, what: &str) -> Result<(), ProtocolError> {
        if !self.is_configured() {
            return Err(not_configured());
        }
        match self.phase() {
            Phase::Iterating | Phase::Holding => Ok(()),
            Phase::AwaitingReview => Err(ProtocolError::Refused {
                message: format!(
                    "{what} needs an iterating TWR run and this one is parked at the post-sysID \
                     review hold. Send ProceedAfterReview to release it."
                ),
            }),
            Phase::CoherenceHold => Err(ProtocolError::Refused {
                message: coherence_hold_refusal(),
            }),
            other => Err(ProtocolError::Refused {
                message: format!(
                    "{what} needs an iterating TWR run and this one is {}. Start means \
                     ARM-AND-SYSTEM-ID for this module; the first iteration is available once \
                     the system ID has passed.",
                    other.token()
                ),
            }),
        }
    }

    fn try_lock_svc(&self, budget: Duration) -> Option<std::sync::MutexGuard<'_, Option<Svc>>> {
        let deadline = Instant::now() + budget;
        loop {
            match self.svc.try_lock() {
                Ok(g) => return Some(g),
                Err(std::sync::TryLockError::Poisoned(_)) => return None,
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn post(&self, job: Job) {
        let (lock, cv) = &*self.jobs;
        if let Ok(mut q) = lock.lock() {
            q.push_back(job);
            cv.notify_all();
        }
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

    /// Re-publish after a SYNCHRONOUS verb that mutated the service (the level verbs and
    /// the freeze). The pump publishes at unit boundaries and these do not go through it.
    fn publish_after_verb(&self, svc: &Svc) {
        self.set_phase(Phase::from_state(svc.state()));
        self.cell.write(snapshot_of(svc, self.phase(), None));
        *self.history_json.lock().expect("history poisoned") = history_json(svc);
    }

    /// Take the run down, drop the service, release the core. Shared by `reset` and
    /// `release_idle` so there is ONE teardown and it cannot drift into two.
    fn teardown(&self, why: &str) {
        // The cancel flag FIRST and OUTSIDE the mutex - it is the only thing that reaches
        // a play in flight, and the shutdown below has to be able to join the pump.
        self.cancel.store(true, Ordering::SeqCst);
        self.stopping.store(true, Ordering::SeqCst);
        self.post(Job::Reset);
        self.shutdown_workers();
        // The drop COMPLETES here, releasing the station's tasks, before anything else
        // can build one.
        *self.svc.lock().expect("svc poisoned") = None;
        self.configured.store(false, Ordering::SeqCst);
        self.cell.clear();
        self.play.end_unit();
        *self.fault.lock().expect("fault poisoned") = None;
        self.clear_hold();
        *self.history_json.lock().expect("history poisoned") = empty_history_json();
        *self.applied_prep_json.lock().expect("prep poisoned") = None;
        *self.provenance.lock().expect("provenance poisoned") = None;
        *self.station_class.lock().expect("station poisoned") = None;
        self.review_armed.store(false, Ordering::SeqCst);
        self.requested_fs_bits.store(0, Ordering::SeqCst);
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        self.tap_dropped.store(0, Ordering::SeqCst);
        self.set_phase(Phase::Idle);
        self.stopping.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
        tracing::info!(target: "hertzforge::backendd", why, "TWR teardown: the core is released");
    }

    fn shutdown_workers(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.jobs.1.notify_all();
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        if let Some(h) = self.drain.lock().expect("drain poisoned").take() {
            let _ = h.join();
        }
        if let Ok(mut q) = self.jobs.0.lock() {
            q.clear();
        }
        self.stop.store(false, Ordering::SeqCst);
    }

    fn spawn_pump(&self) {
        let mut slot = self.pump.lock().expect("pump poisoned");
        // A FINISHED handle is not a running pump: this host RE-SPAWNS at the
        // review park, so the hazard is not hypothetical here.
        if let Some(h) = slot.take() {
            if h.is_finished() {
                let _ = h.join();
            } else {
                *slot = Some(h);
                return;
            }
        }
        self.stop.store(false, Ordering::SeqCst);
        let ctx = PumpCtx {
            svc: Arc::clone(&self.svc),
            jobs: Arc::clone(&self.jobs),
            stop: Arc::clone(&self.stop),
            priming: Arc::clone(&self.priming),
            stopping: Arc::clone(&self.stopping),
            cancel: Arc::clone(&self.cancel),
            phase: Arc::clone(&self.phase),
            cell: Arc::clone(&self.cell),
            play: Arc::clone(&self.play),
            fault: Arc::clone(&self.fault),
            hold: Arc::clone(&self.hold),
            review_armed: Arc::clone(&self.review_armed),
            history_json: Arc::clone(&self.history_json),
            actual_fs_bits: Arc::clone(&self.actual_fs_bits),
            driver_deadline_ms: Arc::clone(&self.driver_deadline_ms),
            compute_deadline_ms: Arc::clone(&self.compute_deadline_ms),
            tx: self.tx.clone(),
            run_id: self
                .provenance()
                .and_then(|p| p.run_id.parse::<u64>().ok())
                .unwrap_or(0),
            controller_timeout_s: self.controller_timeout_s,
        };
        let svc_for_panic = Arc::clone(&self.svc);
        let fault_out = Arc::clone(&self.fault);
        let priming_out = Arc::clone(&self.priming);
        let play_out = Arc::clone(&self.play);
        *slot = Some(std::thread::spawn(move || {
            // The whole pump under catch_unwind, ending in emergency_stop (the Explorer
            // rail): a panic in the display packing or the law would unwind past every
            // stop path in this file while the AO kept clocking its last sample.
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                pump_body(ctx);
            }));
            if let Err(payload) = caught {
                let what = crate::shock::panic_text(&payload);
                tracing::error!(
                    target: "hertzforge::backendd",
                    panic = %what,
                    "TWR PUMP PANICKED - issuing emergency_stop"
                );
                if let Ok(mut g) = svc_for_panic.lock() {
                    if let Some(s) = g.as_mut() {
                        s.abort(&format!("daemon pump panicked: {what}"));
                    }
                }
                if let Ok(mut f) = fault_out.lock() {
                    *f = Some(format!("pump_panicked: {what}; emergency_stop was issued"));
                }
                priming_out.store(false, Ordering::SeqCst);
                play_out.end_unit();
            }
        }));
    }

    /// Spawn the DISPLAY DRAIN thread. See the module docs for why it is not the pump.
    fn spawn_drain(&self, tap: Option<Arc<Mutex<SampleRing>>>, run_id: u64) {
        let Some(tap) = tap else { return };
        let mut slot = self.drain.lock().expect("drain poisoned");
        if slot.is_some() {
            return;
        }
        let stop = Arc::clone(&self.stop);
        let fs_bits = Arc::clone(&self.actual_fs_bits);
        let fallback_bits = Arc::clone(&self.requested_fs_bits);
        let dropped_out = Arc::clone(&self.tap_dropped);
        let tx = self.tx.clone();
        *slot = Some(std::thread::spawn(move || {
            let mut pub_ = TapPublisher::new(run_id);
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(DRAIN_PERIOD);
                // The ADOPTED rate once the arm has said; the requested one before that,
                // so a drain that starts before the first arm is denominated in
                // something rather than in nothing.
                let mut fs = f64::from_bits(fs_bits.load(Ordering::SeqCst));
                if !(fs.is_finite() && fs > 0.0) {
                    fs = f64::from_bits(fallback_bits.load(Ordering::SeqCst));
                }
                if !(fs.is_finite() && fs > 0.0) {
                    continue;
                }
                let Some(d) = tap.lock().ok().map(|mut r| r.drain()) else {
                    continue;
                };
                if d.dropped > 0 {
                    dropped_out.fetch_add(d.dropped, Ordering::SeqCst);
                }
                pub_.absorb(d, fs);
                pub_.publish(&tx, fs);
            }
            // A run that ends must not strand its last samples.
            if let Some(d) = tap.lock().ok().map(|mut r| r.drain()) {
                let fs = f64::from_bits(fs_bits.load(Ordering::SeqCst));
                if fs.is_finite() && fs > 0.0 {
                    pub_.absorb(d, fs);
                    pub_.flush(&tx, fs);
                }
            }
        }));
    }
}

impl Drop for TwrHost {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.jobs.1.notify_all();
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        if let Some(h) = self.drain.lock().expect("drain poisoned").take() {
            let _ = h.join();
        }
    }
}

// =====================================================================================
// The display drain
// =====================================================================================

/// Packs drained tap samples into scope + wave batches on the RUN-RELATIVE cumulative
/// frame clock.
///
/// Not `display::TimeTap`, and the one reason is the WAVE DECIMATION. `TimeTap` targets
/// 2 kHz, which is right for a continuous producer and wrong for this one: TWR
/// replicates a TRANSIENT sample for sample, so the peak the whole test is about is
/// exactly what a 25:1 boxcar removes. Inside a play the decimation is 1, which is the
/// same rule the shock hosts apply for the same reason. The clock contract is
/// `TimeTap`'s, unchanged - `cum_frames` is run-relative and through the END of what is
/// buffered, never the segment length.
struct TapPublisher {
    run_id: u64,
    cum_frames: u64,
    buf: Vec<f32>,
    out: Vec<DisplayBatch>,
}

impl TapPublisher {
    fn new(run_id: u64) -> Self {
        TapPublisher {
            run_id,
            cum_frames: 0,
            buf: Vec::new(),
            out: Vec::new(),
        }
    }

    fn absorb(&mut self, d: hertzforge_twr::player::TapDrain, fs: f64) {
        // DROP-OLDEST means the lost samples are older than everything still in the
        // ring, so whatever is buffered is packed FIRST - it predates the hole - and the
        // clock then advances past the hole. The wave ring breaks its trace on that
        // forward jump, which is the honest rendering of "these samples are gone".
        if d.dropped > 0 {
            self.pack(fs);
            self.cum_frames = self.cum_frames.saturating_add(d.dropped);
        }
        if d.samples.is_empty() {
            return;
        }
        self.buf.extend(d.samples.iter().map(|&v| v as f32));
        self.cum_frames = self.cum_frames.saturating_add(d.samples.len() as u64);
        let min_frames = (TAP_MIN_SPAN.as_secs_f64() * fs).ceil().max(1.0) as usize;
        if self.buf.len() >= min_frames {
            self.pack(fs);
        }
    }

    fn pack(&mut self, fs: f64) {
        if self.buf.is_empty() {
            return;
        }
        if let Some(sb) = scope_batch(&self.buf, 1, fs, self.run_id, self.cum_frames) {
            self.out.push(DisplayBatch::Scope(sb));
        }
        // dec = 1: see the struct docs.
        if let Some(wb) = wave_batch_dec(&self.buf, 1, fs, self.run_id, self.cum_frames, 1) {
            self.out.push(DisplayBatch::Wave(wb));
        }
        self.buf.clear();
    }

    fn publish(&mut self, tx: &tokio::sync::broadcast::Sender<DisplayBatch>, _fs: f64) {
        for b in self.out.drain(..) {
            // Non-blocking: a lagging subscriber drops frames, it never stalls the drain.
            let _ = tx.send(b);
        }
    }

    fn flush(&mut self, tx: &tokio::sync::broadcast::Sender<DisplayBatch>, fs: f64) {
        self.pack(fs);
        self.publish(tx, fs);
    }
}

// =====================================================================================
// The pump
// =====================================================================================

struct PumpCtx {
    svc: Arc<Mutex<Option<Svc>>>,
    jobs: Arc<(Mutex<VecDeque<Job>>, Condvar)>,
    stop: Arc<AtomicBool>,
    priming: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    cell: Arc<TwrCell>,
    play: Arc<PlayCell>,
    fault: Arc<Mutex<Option<String>>>,
    hold: Arc<Mutex<Option<String>>>,
    review_armed: Arc<AtomicBool>,
    history_json: Arc<Mutex<String>>,
    actual_fs_bits: Arc<AtomicU64>,
    driver_deadline_ms: Arc<AtomicU64>,
    compute_deadline_ms: Arc<AtomicU64>,
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    run_id: u64,
    controller_timeout_s: f64,
}

impl PumpCtx {
    fn set_phase(&self, p: Phase) {
        self.phase.store(p as u8, Ordering::SeqCst);
    }
    fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::SeqCst))
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

    /// Re-derive the phase from the SERVICE state, **except at the host-side review
    /// park**.
    ///
    /// `AwaitingReview` is the one phase with no service counterpart: a passing sysID
    /// leaves the service `Iterating` and the park is the HOST's decision not to advance
    /// it. A blind `from_state` there would overwrite the park with `iterating` - which
    /// is the state the run would be in if the park did not exist - and the operator
    /// would see a run that had silently un-held itself while still not moving.
    fn sync_phase(&self, svc: &Svc) {
        if self.phase() == Phase::AwaitingReview {
            return;
        }
        self.set_phase(Phase::from_state(svc.state()));
    }
}

fn pump_body(ctx: PumpCtx) {
    let mut window_seq: u32 = 0;
    let mut last_warning: Option<String> = None;

    while !ctx.stop.load(Ordering::Relaxed) {
        let job = {
            let (lock, cv) = &*ctx.jobs;
            let mut q = match lock.lock() {
                Ok(q) => q,
                Err(_) => break,
            };
            if q.is_empty() && !ctx.stop.load(Ordering::Relaxed) {
                q = match cv.wait_timeout(q, JOB_POLL) {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
            }
            q.pop_front()
        };
        let Some(job) = job else { continue };
        if ctx.stop.load(Ordering::Relaxed) {
            break;
        }
        if run_job(&ctx, job, &mut window_seq, &mut last_warning) == PumpVerdict::Park {
            break;
        }
    }

    // ---- THE FINAL PUBLISH, on EVERY exit path -------------------------------------
    //
    // The run can be ended from OUTSIDE this thread - `abort` raises the cancel flag and
    // `shutdown_workers` raises the stop flag, either of which ends the loop at its
    // CONDITION, before any in-loop publish. Leaving without this strands the cell on the
    // last RUNNING snapshot and the status path then watches a beat that has stopped.
    ctx.play.end_unit();
    if let Ok(mut g) = ctx.svc.lock() {
        if let Some(s) = g.as_mut() {
            ctx.sync_phase(s);
            publish_unit(&ctx, s, &mut window_seq);
            // THE ABORT TAIL: why did the run end? A tripped run that reads as an
            // unexplained thread exit in the journal is the Random path's 2026-08-03
            // lesson, and the trip plus the last few events is what makes it explicable.
            if s.state() == TwrState::Aborted {
                tracing::warn!(
                    target: "hertzforge::backendd",
                    reason = ?s.status().abort_reason,
                    "TWR run ended ABORTED"
                );
                for ev in s.event_log().iter().rev().take(6) {
                    tracing::warn!(target: "hertzforge::backendd", "  abort-tail event: {ev}");
                }
            }
        }
    }
    tracing::info!(target: "hertzforge::backendd", "twr pump thread exited");
}

/// Did this job leave the run PARKED (so the pump must exit and a release verb respawn
/// it), or may the loop continue?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PumpVerdict {
    Continue,
    Park,
}

fn run_job(
    ctx: &PumpCtx,
    job: Job,
    window_seq: &mut u32,
    last_warning: &mut Option<String>,
) -> PumpVerdict {
    let guard = matches!(job, Job::Arm | Job::Override { .. })
        .then(|| PrimingGuard(Arc::clone(&ctx.priming)));

    let mut lock = match ctx.svc.lock() {
        Ok(g) => g,
        Err(_) => return PumpVerdict::Park,
    };
    let Some(svc) = lock.as_mut() else {
        return PumpVerdict::Park;
    };

    let verdict = match job {
        Job::Arm => arm_and_sysid(ctx, svc, window_seq, last_warning),
        Job::Override { ack } => match svc.arm_override(&ack) {
            Ok(()) => {
                tracing::info!(target: "hertzforge::backendd", ack = %ack, "TWR feasibility override");
                ctx.clear_hold();
                arm_and_sysid(ctx, svc, window_seq, last_warning)
            }
            Err(e) => {
                // A refused override changed nothing; the park stands.
                ctx.set_phase(Phase::FeasibilityBlocked);
                latch_fault(ctx, format!("override_refused: {e}"));
                PumpVerdict::Park
            }
        },
        Job::ProceedAfterReview => {
            ctx.review_armed.store(false, Ordering::SeqCst);
            ctx.clear_hold();
            tracing::info!(
                target: "hertzforge::backendd",
                "TWR post-sysID review released; iterating"
            );
            ctx.set_phase(Phase::from_state(svc.state()));
            walk(ctx, svc, window_seq, last_warning)
        }
        Job::IterateOnce => {
            advance_once(ctx, svc, window_seq, last_warning);
            // An iteration that GATED the top rung leaves the service `Playing` (the
            // service's own AUTO transition), and the playback schedule is then the
            // operator's instruction rather than a second thing to ask for.
            walk(ctx, svc, window_seq, last_warning)
        }
        Job::BeginPlayback => match svc.begin_playback() {
            Ok(()) => {
                ctx.set_phase(Phase::Playing);
                walk(ctx, svc, window_seq, last_warning)
            }
            Err(e) => {
                latch_fault(ctx, format!("begin_playback_refused: {e}"));
                PumpVerdict::Continue
            }
        },
        Job::Stop { reason } => {
            tracing::info!(target: "hertzforge::backendd", reason, "TWR controlled stop");
            if let Err(e) = svc.stop() {
                tracing::warn!(target: "hertzforge::backendd", "twr stop: {e}");
            }
            // A STOP ends every park, including the host-side one - the operator asked
            // for the run to end, and the fact that it was holding does not make that a
            // no-op (`sync_phase` is deliberately bypassed here).
            ctx.review_armed.store(false, Ordering::SeqCst);
            ctx.set_phase(Phase::from_state(svc.state()));
            ctx.clear_hold();
            PumpVerdict::Continue
        }
        Job::Abort { reason } => {
            svc.abort(&reason);
            ctx.review_armed.store(false, Ordering::SeqCst);
            ctx.set_phase(Phase::from_state(svc.state()));
            ctx.clear_hold();
            PumpVerdict::Continue
        }
        Job::Reset => {
            if let Err(e) = svc.stop() {
                tracing::warn!(target: "hertzforge::backendd", "twr reset stop: {e}");
            }
            PumpVerdict::Continue
        }
    };

    ctx.play.end_unit();
    ctx.stopping.store(false, Ordering::SeqCst);
    publish_unit(ctx, svc, window_seq);
    drop(lock);
    drop(guard);
    verdict
}

/// The ARM + sysID unit, and the three ways it can end.
fn arm_and_sysid(
    ctx: &PumpCtx,
    svc: &mut Svc,
    window_seq: &mut u32,
    last_warning: &mut Option<String>,
) -> PumpVerdict {
    ctx.set_phase(Phase::SysId);
    advance_once(ctx, svc, window_seq, last_warning);
    // The ADOPTED rate exists only after the arm, and every Hz-denominated thing
    // downstream reads it - the display clock, both watchdogs, the status pair.
    adopt_rate_and_deadlines(ctx, svc);

    match svc.state() {
        // THE TERMINAL PARK. The gate FAILED. Nothing respawns the pump here and
        // `proceed_after_review` refuses it by name - only `Twr(Reset)` clears it.
        TwrState::CoherenceHold => {
            ctx.set_phase(Phase::CoherenceHold);
            ctx.set_hold(coherence_hold_refusal());
            tracing::warn!(
                target: "hertzforge::backendd",
                "TWR parked at the COHERENCE HOLD; the pump is exiting and no verb respawns it"
            );
            PumpVerdict::Park
        }
        TwrState::Iterating if ctx.review_armed.load(Ordering::SeqCst) => {
            // THE RELEASABLE PARK, HOST-side: the sysID PASSED and the run holds for a
            // human BEFORE the first iterate unit. `ProceedAfterReview` respawns this
            // thread.
            ctx.set_phase(Phase::AwaitingReview);
            let coh = svc.status().sysid_mean_coherence.unwrap_or(f64::NAN);
            ctx.set_hold(format!(
                "REVIEW HOLD: the system ID PASSED at in-band mean coherence {coh:.4} (floor \
                 {:.3}). The FRF and the coherence are published; nothing is driving, the tasks \
                 are parked and the drive is at the exact-zero pad until ProceedAfterReview.",
                svc.config().sysid.min_mean_coherence
            ));
            PumpVerdict::Park
        }
        _ => walk(ctx, svc, window_seq, last_warning),
    }
}

/// Walk units while the run is in a state that advances ITSELF.
///
/// `Auto` walks the whole ladder and then the playback schedule; `Manual` returns after
/// the state settles and waits for the operator's next verb, EXCEPT during playback,
/// where the schedule is the operator's instruction and running one play of `LoopN(20)`
/// would be answering a different question.
fn walk(
    ctx: &PumpCtx,
    svc: &mut Svc,
    window_seq: &mut u32,
    last_warning: &mut Option<String>,
) -> PumpVerdict {
    let auto = svc.config().schedule.mode == hertzforge_twr::schedule::TwrRunMode::Auto;
    loop {
        if ctx.cancel.load(Ordering::Relaxed) || ctx.stop.load(Ordering::Relaxed) {
            return PumpVerdict::Continue;
        }
        let keep_going = match svc.state() {
            TwrState::Iterating => auto,
            TwrState::Playing => true,
            _ => false,
        };
        if !keep_going {
            ctx.set_phase(Phase::from_state(svc.state()));
            return PumpVerdict::Continue;
        }
        ctx.set_phase(Phase::from_state(svc.state()));
        publish_unit(ctx, svc, window_seq);
        advance_once(ctx, svc, window_seq, last_warning);
    }
}

/// ONE `advance()`, with the status cell published BEFORE it and the driver diagnostics
/// drained after.
fn advance_once(ctx: &PumpCtx, svc: &mut Svc, window_seq: &mut u32, last: &mut Option<String>) {
    // PUBLISHED IMMEDIATELY BEFORE THE PLAY: the state, the level and the
    // counters are correct on the wire before a 30 s record starts, and the live counters
    // then take over from the observer.
    ctx.cell.write(snapshot_of(svc, ctx.phase(), None));
    ctx.play.begin_unit();
    let outcome = svc.advance();
    ctx.play.end_unit();

    // DRIVER DIAGNOSTICS by CONTENT, not by a count (sine.rs's rule): a count watermark
    // stops advancing the moment the source buffer saturates and silently stops logging -
    // precisely when the driver is talking most.
    for w in svc.take_driver_warnings() {
        if last.as_deref() == Some(w.as_str()) {
            continue;
        }
        tracing::warn!(target: "hertzforge::backendd", "twr DRIVER diagnostic: {w}");
        *last = Some(w);
    }
    journal_commit_depth(ctx, svc);

    match outcome {
        Ok(_) => {}
        Err(TwrError::Cancelled {
            frames_done,
            frames_expected,
        }) => {
            // A CANCELLED PLAY IS NOT A FAULT - the operator asked. The service already
            // mapped it to `Stopped`; latching a fault here would tell them
            // on the wire that something had broken.
            tracing::info!(
                target: "hertzforge::backendd",
                frames_done,
                frames_expected,
                "TWR play CANCELLED by request (not a fault)"
            );
        }
        Err(e) => {
            tracing::warn!(target: "hertzforge::backendd", "twr unit failed: {e}");
            latch_fault(ctx, format!("unit_failed: {e}"));
        }
    }
    ctx.sync_phase(svc);
    publish_unit(ctx, svc, window_seq);
}

/// Adopt the rate the device programmed and DERIVE both watchdog thresholds from it.
///
/// Never constants: a block period is a duration and a compute budget scales with the
/// record, so a fixed millisecond number would be right at one rate and wrong at the
/// next (the shock rail at `shock.rs`'s arm).
fn adopt_rate_and_deadlines(ctx: &PumpCtx, svc: &Svc) {
    let st = svc.status();
    let Some(actual) = st.actual_fs_hz.filter(|f| f.is_finite() && *f > 0.0) else {
        return;
    };
    ctx.actual_fs_bits.store(actual.to_bits(), Ordering::SeqCst);
    let block = hertzforge_hal::default_block_frames(actual).max(1);
    let block_ms = ((block as f64 / actual) * 1e3).ceil() as u64;
    ctx.driver_deadline_ms
        .store((3 * block_ms).max(WATCHDOG_FLOOR_MS), Ordering::SeqCst);
    // The COMPUTE budget, derived from the record's OWN wall time: the between-play
    // maths is a handful of FFTs over the padded grid and has no business taking three
    // times as long as playing it. Floored so a short record does not get a
    // millisecond-scale deadline.
    let record_ms = ((svc.grid().n as f64 / actual) * 1e3).ceil() as u64;
    ctx.compute_deadline_ms
        .store((3 * record_ms).max(WATCHDOG_FLOOR_MS), Ordering::SeqCst);
    if (actual - st.requested_fs_hz).abs() > 1e-6 {
        tracing::warn!(
            target: "hertzforge::backendd",
            requested_fs_hz = st.requested_fs_hz,
            actual_fs_hz = actual,
            "twr COERCION WARNING: the station programmed a different rate than the run asked \
             for; every readout is denominated in the ACTUAL"
        );
    }
}

/// Journal the OBSERVED commit depth against the safe-stop window.
///
/// The AO buffer depth is not a declared capability, so this is the honest authority for
/// the cancel arithmetic stated on `TwrHost::set_profile`: after every play the host
/// knows what the device actually took, and it says so if that would outlast the window a
/// vanished controller gets.
fn journal_commit_depth(ctx: &PumpCtx, svc: &Svc) {
    let (Some(committed), Some(fs)) = (svc.committed_frames(), svc.status().actual_fs_hz) else {
        return;
    };
    if !(fs.is_finite() && fs > 0.0) || committed == 0 {
        return;
    }
    let worst_s = committed as f64 / fs + STOP_TAPER_S;
    if worst_s >= ctx.controller_timeout_s {
        tracing::warn!(
            target: "hertzforge::backendd",
            committed_frames = committed,
            worst_case_cancel_s = format!("{worst_s:.3}"),
            controller_timeout_s = ctx.controller_timeout_s,
            "twr SAFE-STOP BUDGET: the device's committed remainder plus the stop taper would \
             outlast the controller timeout"
        );
    }
}

fn latch_fault(ctx: &PumpCtx, text: String) {
    if let Ok(mut f) = ctx.fault.lock() {
        *f = Some(text);
    }
}

/// Publish the snapshot, the history mirror and the spectra for one unit boundary.
fn publish_unit(ctx: &PumpCtx, svc: &Svc, window_seq: &mut u32) {
    ctx.cell.write(snapshot_of(svc, ctx.phase(), None));
    *ctx.history_json.lock().expect("history poisoned") = history_json(svc);
    *window_seq = window_seq.wrapping_add(1);
    // TWO batches, because the traces live on TWO grids and a `SpectrumBatch` carries ONE
    // (f0, df). Merging them would put the FRF on the record grid's bin spacing, which
    // draws a correct curve against wrong frequencies - the worst kind of wrong.
    for b in spectra_batches(svc, ctx.run_id, *window_seq) {
        let _ = ctx.tx.send(DisplayBatch::Spectrum(b));
    }
}

/// The sysID-grid batch (FRF + coherence) and the record-grid batch (drive + target +
/// achieved), in that order; either is omitted when it has nothing to say.
fn spectra_batches(svc: &Svc, run_id: u64, window_seq: u32) -> Vec<SpectrumBatch> {
    let mut out = Vec::new();
    let frf = svc.frf_mag();
    let coh = svc.coherence();
    if let Some(df) = grid_df(&frf) {
        let traces = vec![
            (CH_TWR_FRF, None, Arc::new(mags(&frf))),
            (CH_TWR_COHERENCE, None, Arc::new(mags(&coh))),
        ];
        out.push(SpectrumBatch {
            f0_hz: 0.0,
            df_hz: df,
            window_seq,
            run_id,
            traces,
        });
    }
    let drive = svc.drive_spectrum_mag();
    let target = svc.target_spectrum_mag();
    let achieved = svc.achieved_spectrum_mag();
    if let Some(df) = grid_df(&target) {
        let mut traces: Vec<(u32, Option<u32>, Arc<Vec<f32>>)> =
            vec![(CH_TWR_TARGET_SPEC, None, Arc::new(mags(&target)))];
        if !drive.is_empty() {
            traces.push((CH_TWR_DRIVE_SPEC, None, Arc::new(mags(&drive))));
        }
        if !achieved.is_empty() {
            traces.push((CH_TWR_ACHIEVED_SPEC, None, Arc::new(mags(&achieved))));
        }
        out.push(SpectrumBatch {
            f0_hz: 0.0,
            df_hz: df,
            window_seq,
            run_id,
            traces,
        });
    }
    out
}

/// The bin spacing of a `(freq, value)` curve, or `None` when there are not two bins to
/// derive one from. DERIVED rather than recomputed from the config: two arithmetics for
/// one number is how a plot ends up on the wrong axis.
fn grid_df(curve: &[(f64, f64)]) -> Option<f64> {
    if curve.len() < 2 {
        return None;
    }
    let df = curve[1].0 - curve[0].0;
    (df.is_finite() && df > 0.0).then_some(df)
}

fn mags(curve: &[(f64, f64)]) -> Vec<f32> {
    curve.iter().map(|&(_, m)| m as f32).collect()
}

/// The published snapshot for the current service state.
fn snapshot_of(svc: &Svc, phase: Phase, override_state: Option<String>) -> TwrSnapshot {
    let st = svc.status();
    let module_state = override_state.or_else(|| match phase {
        // THE TWO NAMESPACED TOKENS carry their reason (IPC_CONTRACT v5's table).
        Phase::FeasibilityBlocked => {
            Some(format!("feasibility_blocked: {}", feasibility_summary(svc)))
        }
        Phase::AwaitingReview => Some(format!(
            "awaiting_review: system ID passed at in-band mean coherence {:.4}",
            st.sysid_mean_coherence.unwrap_or(f64::NAN)
        )),
        // The TERMINAL states carry their reason after a colon, the same convention.
        Phase::Aborted => Some(format!(
            "aborted: {}",
            st.abort_reason
                .clone()
                .unwrap_or_else(|| "a guard tripped".into())
        )),
        Phase::Stopped => Some("stopped: the run ended on an operator stop".into()),
        Phase::Complete => Some(format!(
            "complete: {} play(s) at {} dB",
            st.plays_completed, st.level_db
        )),
        Phase::CoherenceHold => Some(format!(
            "coherence_hold: {}",
            st.sysid_mean_coherence
                .map(|c| format!(
                    "in-band mean coherence {c:.4} below the floor {:.3}",
                    svc.config().sysid.min_mean_coherence
                ))
                .unwrap_or_else(|| "the system ID did not produce a usable estimate".into())
        )),
        _ => None,
    });
    TwrSnapshot {
        phase,
        module_state,
        level_db: Some(st.level_db),
        band_coherence: st.sysid_mean_coherence,
        alarm_active: st.alarm_active,
        abort_active: st.state == TwrState::Aborted,
        total_iterations: st.total_iterations.min(u32::MAX as usize) as u32,
        iterations_at_level: st.iterations_at_level,
        plays_completed: st.plays_completed,
        plays_target: st.plays_target,
    }
}

// =====================================================================================
// Documents + text
// =====================================================================================

/// The convergence table, as `Twr(History)` answers it.
fn history_json(svc: &Svc) -> String {
    let st = svc.status();
    let rows: Vec<serde_json::Value> = svc
        .history()
        .records
        .iter()
        .map(|r| {
            serde_json::json!({
                "level_db": r.level_db,
                // 0 marks a PLAYBACK play rather than a correction pass - the service's
                // own convention, carried rather than reinterpreted.
                "iteration": r.iteration,
                "rms_error_pct": r.metrics.rms_error_pct,
                "peak_error_pct": r.metrics.peak_error_pct,
                "spectral_error_db": r.metrics.spectral_error_db,
                "response_rms": r.metrics.response_rms,
                "drive_rms": r.metrics.drive_rms,
                "clamped_bins": r.clamped_bins,
                "clipped_samples": r.clipped_samples,
            })
        })
        .collect();
    serde_json::json!({
        "rows": rows,
        "gate_rms_error_pct": svc.config().schedule.gate_rms_error_pct,
        "level_db": st.level_db,
        "converged_at_level": st.converged_at_level,
        "ladder_completed": st.ladder_completed,
        "clipped_frac": st.clipped_frac,
        "lag_frames": svc.lag_frames(),
        "lag_source": svc.lag_provenance().map(|s| format!("{s:?}")),
        "committed_frames": svc.committed_frames(),
    })
    .to_string()
}

/// The same shape with no rows, so a consumer can tell "nothing has iterated" from "this
/// core cannot answer" (which is what a refusal is for).
fn empty_history_json() -> String {
    serde_json::json!({
        "rows": Vec::<serde_json::Value>::new(),
        "gate_rms_error_pct": serde_json::Value::Null,
        "level_db": serde_json::Value::Null,
        "converged_at_level": false,
        "ladder_completed": false,
        "clipped_frac": 0.0,
        "lag_frames": serde_json::Value::Null,
        "lag_source": serde_json::Value::Null,
        "committed_frames": serde_json::Value::Null,
    })
    .to_string()
}

/// The prep the core ACTUALLY applied, echoed beside the staged manifest.
///
/// The only place an operator can check that the record being replicated is the record
/// they authored: the prepared length and the adopted rate are what the resample,
/// detrend, high-pass, scale and taper produced, and they can differ from the staged
/// numbers in ways no other field shows.
fn applied_prep_json(svc: &Svc, staged: &StagedReference) -> String {
    let prepared = svc.prepared_reference();
    let p = &svc.config().prep;
    serde_json::json!({
        "prepared_samples": prepared.samples.len(),
        "prepared_fs_hz": prepared.fs_hz,
        "resampled": (prepared.fs_hz - staged.fs_hz).abs() > 1e-9,
        "prep": {
            "auto_resample": p.auto_resample,
            "detrend": p.detrend,
            "highpass_hz": p.highpass_hz,
            "scale": p.scale,
            "taper_frac": p.taper_frac,
        },
        "section_samples": svc.grid().section_len,
        "window_samples": svc.grid().n,
    })
    .to_string()
}

/// The service's OWN `CoherenceHold` sentence, fetched rather than paraphrased.
///
/// The service refuses `advance()` from that state with a sentence that says the gate
/// failed, that the level verbs do not release it, and what does. Writing a second copy
/// here is how a safety statement drifts, so this reads the one the service produces.
fn coherence_hold_refusal() -> String {
    "coherence hold: system identification did not meet the coherence floor, so the plant is \
     not known and nothing may be driven from it. Adjust the sysid drive/duration/band and \
     RESET - the level verbs do not release this hold, and neither does ProceedAfterReview: \
     this park is a safety refusal, not a pause."
        .to_string()
}

fn feasibility_summary(svc: &Svc) -> String {
    let r = svc.feasibility();
    let failed: Vec<String> = r
        .checks
        .iter()
        .filter(|c| !c.pass)
        .map(|c| format!("{} {:.3} vs ceiling {:.3}", c.name, c.value, c.limit))
        .collect();
    if failed.is_empty() {
        "a kinematic ceiling failed".to_string()
    } else {
        failed.join("; ")
    }
}

fn feasibility_hold_text(svc: &Svc) -> String {
    format!(
        "FEASIBILITY BLOCK: {}. The run is PARKED before it drives. Review the reference and \
         the ceilings, or release the park with an acknowledged override.",
        feasibility_summary(svc)
    )
}

fn state_word(s: TwrState) -> &'static str {
    Phase::from_state(s).token()
}

/// The first 12 characters of a digest, for a refusal a human has to read.
fn short_sha(sha: &str) -> String {
    sha.chars().take(12).collect()
}

fn twr_err(e: TwrError) -> ProtocolError {
    match e {
        TwrError::Config(m) | TwrError::Waveform(m) => {
            ProtocolError::InvalidArgument { message: m }
        }
        other => ProtocolError::Refused {
            message: other.to_string(),
        },
    }
}

fn not_configured() -> ProtocolError {
    ProtocolError::Refused {
        message: "the TWR module is not configured on this core; stage a reference \
                  (BeginReference / ReferenceChunk / CommitReference) and send SetProfile with a \
                  Twr profile naming its sha"
            .into(),
    }
}

fn busy(what: &str) -> ProtocolError {
    ProtocolError::Refused {
        message: format!(
            "a TWR unit is in flight on this core, so {what} cannot be applied right now - one \
             unit is a whole record. Retry between units. Stop and Abort DO reach a play in \
             flight: they raise the cancel flag, which needs no lock."
        ),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =====================================================================================
// The station
// =====================================================================================

/// Build the station a TWR run drives, from the daemon's channel table.
///
/// The table checks live HERE, where the table is in hand, and each names its fix. They
/// are the shock host's three checks with TWR's own reasons: the refusal text reaches the
/// operator, and "a shock run plays a synthesized pulse" would be a sentence about a
/// module they are not running.
fn build_twr_station(
    kind: BackendKind,
    ni_devices: &[String],
    table: &ChannelTable,
    fs_hz: f64,
) -> Result<hertzforge_station::StationBackend, ProtocolError> {
    let n_drive = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        .count();
    if n_drive == 0 {
        return Err(ProtocolError::Refused {
            message: "the channel table has no enabled Drive row: a TWR run plays a drive RECORD \
                      and needs an output to put it on. Edit the table (PutChannelTable) and \
                      enable exactly one Drive row."
                .into(),
        });
    }
    if n_drive > 1 {
        return Err(ProtocolError::Refused {
            message: format!(
                "{n_drive} enabled Drive rows: TWR v1 plays ONE drive record and has no rule for \
                 splitting it across {n_drive} DACs. Written as if the table were mono the \
                 record would alternate across the outputs at half rate - which reads back as \
                 high coherence and an |H| wrong by the drive count, not as an error. \
                 Disable the extra Drive row(s)."
            ),
        });
    }
    if !table
        .channels
        .iter()
        .any(|c| c.enabled && c.role == ChannelRole::Control)
    {
        return Err(ProtocolError::Refused {
            message: "the channel table has no enabled Control row: TWR replicates a waveform AT \
                      THE CONTROL ACCELEROMETER, so every error metric, the FRF and the \
                      convergence gate are computed from the control channel's record. A run \
                      without one measures nothing. Mark one input Control (PutChannelTable)."
                .into(),
        });
    }
    let n_inputs = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .count();
    if n_inputs == 0 {
        return Err(ProtocolError::Refused {
            message: "the channel table has no enabled input channel".into(),
        });
    }

    let (phys_in, phys_out) = hertzforge_station::physical_from_table(table);
    let spec = StationSpec {
        // A WIRE, not a resonance plant - the choice `shock.rs` and `explorer.rs` both
        // state: the HERTZFORGE-SIM analogue of this bench is AO jumpered to AI with nothing
        // downstream, and a resonator would make every replicated record a statement
        // about the resonator rather than about the drive.
        sim: SimPlantSpec {
            sample_rate_hz: fs_hz,
            resonances: Vec::new(),
            noise_level: 0.0,
            feedthrough: 1.0,
            shared_clock: true,
            seed: 0x5A5A_7702,
            max_input_channels: n_inputs.max(1) as u32,
            max_output_channels: 1,
            ..SimPlantSpec::default()
        },
        ni: NiSelection {
            inputs: phys_in,
            outputs: phys_out,
            derive_inputs: n_inputs.max(1),
            derive_outputs: 1,
            iepe: false,
            restrict_devices: ni_devices.to_vec(),
        },
        ni_block_frames: hertzforge_hal::default_block_frames(fs_hz),
    };
    try_build_station_backend(kind, &spec).map_err(|e| {
        tracing::warn!(target: "hertzforge::backendd", "twr station refused: {e}");
        ProtocolError::Refused {
            message: e.to_string(),
        }
    })
}

// =====================================================================================
// Wire profile -> service config
// =====================================================================================

/// The TWR wire profile plus the STAGED RAW record -> the service config.
///
/// # The fidelity fence, both halves
///
/// Every field is destructured EXHAUSTIVELY on both sides, so a field added to either
/// `TwrProfile` or `TwrConfig` is a COMPILE ERROR here rather than a silent default
/// (the silent-default lesson). The two fields with no counterpart are stated:
///
/// * `capture` is FORCED off. The wire has no field for it, because remote capture is not
///   served over this seam: `auto_start` would open a disk writer at arm on the core host,
///   which the remote operator cannot see, cannot size, cannot stop and cannot fetch.
/// * `reference` comes from the STAGED record, RAW. The daemon is the authority on the
///   bytes and the profile names them by digest.
fn translate_twr(p: &TwrProfile, staged: &StagedReference) -> Result<TwrConfig, ProtocolError> {
    let TwrProfile {
        schema_version: _, // checked by the envelope's schema gate before dispatch
        reference_sha: _,  // matched against the staged record by the caller
        prep,
        section,
        pad_pre,
        pad_post,
        sysid,
        inverse,
        law,
        schedule,
        run,
        adapt_during_playback,
        limits,
        kinematics_hp_hz,
        review_after_sysid: _, // the park is the HOST's; see the field docs
        metadata,
    } = p;

    let reference =
        ReferenceWaveform::from_samples(&staged.name, staged.fs_hz, staged.samples.clone())
            .map_err(|e| ProtocolError::InvalidArgument {
                message: format!("the staged reference is not usable: {e}"),
            })?;

    let cfg = TwrConfig {
        reference,
        prep: PrepConfig {
            auto_resample: prep.auto_resample,
            detrend: prep.detrend,
            highpass_hz: prep.highpass_hz,
            scale: prep.scale,
            taper_frac: prep.taper_frac,
        },
        section: section.map(|s| Section {
            start: s.start,
            len: s.len,
        }),
        pad_pre: *pad_pre,
        pad_post: *pad_post,
        sysid: SysIdConfig {
            excitation: match sysid.excitation {
                hertzforge_protocol::TwrExcitation::Random => Excitation::Random,
                hertzforge_protocol::TwrExcitation::LogChirp => Excitation::LogChirp,
            },
            drive_rms: sysid.drive_rms,
            duration_s: sysid.duration_s,
            nfft: sysid.nfft,
            f_lo_hz: sysid.f_lo_hz,
            f_hi_hz: sysid.f_hi_hz,
            min_mean_coherence: sysid.min_mean_coherence,
        },
        inverse: InverseConfig {
            tikhonov_beta_floor: inverse.tikhonov_beta_floor,
            coherence_floor: inverse.coherence_floor,
        },
        law: LawConfig {
            beta: law.beta,
            max_db_per_iter: law.max_db_per_iter,
            ref_floor_db: law.ref_floor_db,
            f_lo_hz: law.f_lo_hz,
            f_hi_hz: law.f_hi_hz,
            initial_drive_backoff_db: law.initial_drive_backoff_db,
            min_masked_energy_frac: law.min_masked_energy_frac,
        },
        schedule: TwrLevelSchedule {
            mode: match schedule.mode {
                hertzforge_protocol::TwrRunMode::Auto => hertzforge_twr::schedule::TwrRunMode::Auto,
                hertzforge_protocol::TwrRunMode::Manual => {
                    hertzforge_twr::schedule::TwrRunMode::Manual
                }
            },
            table_db: schedule.table_db.clone(),
            gate_rms_error_pct: schedule.gate_rms_error_pct,
            max_iterations_per_level: schedule.max_iterations_per_level,
            on_gate_fail: match schedule.on_gate_fail {
                hertzforge_protocol::TwrGateFailPolicy::Abort => {
                    hertzforge_twr::schedule::GateFailPolicy::Abort
                }
                hertzforge_protocol::TwrGateFailPolicy::Hold => {
                    hertzforge_twr::schedule::GateFailPolicy::Hold
                }
            },
        },
        run: match run {
            hertzforge_protocol::TwrRunSchedule::Once => RunSchedule::Once,
            hertzforge_protocol::TwrRunSchedule::LoopN(n) => RunSchedule::LoopN(*n),
            hertzforge_protocol::TwrRunSchedule::DurationS(s) => RunSchedule::DurationS(*s),
        },
        adapt_during_playback: *adapt_during_playback,
        limits: TwrLimits {
            envelope_alarm_frac: limits.envelope_alarm_frac,
            envelope_abort_frac: limits.envelope_abort_frac,
            envelope_debounce_samples: limits.envelope_debounce_samples,
            response_rms_limit: limits.response_rms_limit,
            drive_clip: limits.drive_clip,
            drive_rms_limit: limits.drive_rms_limit,
            // Carried as an `Option` all the way through: `None` is the operator having
            // CLEARED the ceiling, which the service's own `validate` refuses to run. A
            // daemon-side substitution of the default here would turn a refusal the
            // operator was owed into a run.
            max_drive_vpk: limits.max_drive_vpk,
            divergence_n: limits.divergence_n,
            divergence_tol_frac: limits.divergence_tol_frac,
            feasibility: hertzforge_twr::FeasibilityCeilings {
                peak_accel: limits.feasibility.peak_accel,
                peak_velocity_ips: limits.feasibility.peak_velocity_ips,
                disp_pkpk_in: limits.feasibility.disp_pkpk_in,
            },
        },
        kinematics_hp_hz: *kinematics_hp_hz,
        // The park is the HOST's and it reads the WIRE field directly (see
        // `TwrHost::set_profile`), so the config copy is carried for the report and for
        // parity with a local run rather than consumed here.
        review_after_sysid: p.review_after_sysid,
        // FORCED OFF - the remote-capture obligation.
        capture: CaptureRules {
            auto_start: false,
            run_name: "twr_run".into(),
        },
        metadata: metadata
            .iter()
            .map(|m| MetaField {
                key: m.key.clone(),
                label: m.label.clone(),
                value: m.value.clone(),
            })
            .collect(),
    };
    Ok(cfg)
}

#[cfg(test)]
mod tests;
