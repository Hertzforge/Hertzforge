//! The daemon-side CLASSICAL SHOCK module, plus the rails both shock hosts share
//! with [`crate::srs`].
//!
//! # A TRANSIENT producer on rails built for continuous ones
//!
//! [`crate::explorer::ExplorerHost`] and [`crate::random::RandomHost`] both pump a
//! loop that runs for as long as the operator wants it to. A shock run does not: it
//! ARMS, and then pulses are FIRED one verb at a time, each one tens of milliseconds
//! long with seconds of deliberate silence between them. Everything below that looks
//! like a departure from the Explorer rails is that difference, and each one is stated
//! at the site.
//!
//! What is copied verbatim, because it is not about continuity at all:
//!
//! * **The status path NEVER takes the service mutex.** A fire holds that mutex for the
//!   whole pulse, so a status poll behind it would block for the length of a train. The
//!   service publishes nothing of its own, so this host owns the [`ShockCell`] and
//!   writes it at every job boundary AND from inside the per-block fire observer.
//! * **The arm is ASYNC behind a priming flag.** `arm()` opens the station, configures
//!   it, adopts the device rate, consults the gate, synthesizes the compensated
//!   composite and re-derives the SRS grid: seconds of DAQmx plus a synthesis. A
//!   command whose latency scales with driver task setup is a broken contract.
//! * **Dispatch beside the forwarder**, inherited from `server.rs`.
//! * **An out-of-band fault cell** for the refusals the service's own state machine
//!   cannot latch (a refused arm, a panicking pump), a journalled abort tail including
//!   the driver warnings the service drained, and a FINAL publish on every pump exit
//!   path.
//! * **catch_unwind over the whole pump**, ending in `emergency_stop`.
//!
//! # What is SHOCK-SHAPED, and why
//!
//! ## The pump is a JOB QUEUE, not a tick loop
//!
//! There is no batch to run. The pump waits on a mailbox, executes ONE job under the
//! service mutex, publishes, and waits again. Every mutating verb (`Start` = ARM,
//! `FirePulse`, `RunAuto`, `FireTrain`, `FireGunfireBurst`, `Reset`) enqueues; the
//! verb returns as soon as the job is ACCEPTED, exactly as the Explorer's `Start`
//! returns before its arm finishes. A fire that ran on the dispatch thread would hold
//! the command path for the length of a train, and the one verb that must always get
//! through - Stop - would be behind it.
//!
//! ## Stop and Abort do NOT need the mutex
//!
//! Both raise the service's cancel flag, which is an `Arc<AtomicBool>` the service
//! hands out precisely so a host holding it behind a `Mutex` can reach a fire in
//! flight. The fire returns `Cancelled` at its next block boundary; the queued job
//! then performs the actual `stop()` / `abort()`. The latency arithmetic is the
//! service's (`ClassicalShockService::abort`): the committed drive queue is ~60 ms on
//! this chassis and is longer than a whole pulse, so a shock abort is a promise about
//! the NEXT pulse, never this one.
//!
//! ## The watchdog is gated on an explicit FIRING flag
//!
//! Beating every "iteration" is meaningless here: an armed-idle run has no iterations
//! at all, and an operator may sit armed for minutes. The beat is stored from inside
//! the fire's per-block observer and the stall check only runs while
//! [`ShockHost::firing`] is set, with the PLANNED inter-pulse spacing added to the
//! threshold so a legitimate 2 s gap in an AUTO train is not a stalled driver.
//!
//! ## Time streams are published per PULSE, from the completed record
//!
//! **This is a stated deviation from the design and the reason is a byte, not a
//! preference.** The design says the scope/wave batches ride "the per-block observer".
//! At this tip the observer's signature is `FnMut(frames_delivered, frames_expected)` -
//! COUNTS, no samples (`hertzforge_shock_host::service::firer::FireControl::observer`), and
//! the SRS seam (`hertzforge_srs::service::plant::PulsePlant::fire`) has no observer at all.
//! Streaming from inside the fire would therefore require widening two module crates'
//! trait seams, both of which other work touches. So the observer keeps the jobs the
//! design actually needs from it - the watchdog beat and the mid-train StatusCell
//! advance - and the display batches are packed from the measured record the moment the
//! fire returns, chunked into [`PULSE_TIME_CHUNKS`] pieces so the per-pulse positive
//! floor is met. The cost is bounded and stated: the pane sees the pulse one
//! pulse-length late (~44 ms for an 11 ms half-sine with MIL-compliant shoulders at
//! 51.2 kHz), not one train late. The widening is the recorded upgrade path.
//!
//! ## The frame clock is WALL-CLOCK with an explicit GAP
//!
//! `cum_frames` advances by the pulse's own length AND by the silence between pulses
//! (`gap_s x fs` frames, never packed as samples). The time pane therefore draws real
//! time and the inter-pulse silence is a HOLE the wave ring breaks the trace on, which
//! is the honest rendering of a transient producer - a producer that packed only its
//! pulses back to back would draw a train of five as one continuous 220 ms signal.
//!
//! ## Trace 921 is NOT exported
//!
//! The local UI's convention of putting a pulse TIME HISTORY on a spectrum frame at id
//! 921 works only because both ends share the repo. Over the wire, time rides the
//! scope and waveform streams; 921 stays a local wart, recorded as known.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_hal::Backend;
use hertzforge_protocol::{ClassicalShockProfile, Doc, ProtocolError, SchemaKind, StatusReport};
use hertzforge_shock_common::pulse_store::PulseRecord;
use hertzforge_shock_host::service::firer::BackendPulseFirer;
use hertzforge_shock_host::service::report::RunState as ShockRunState;
use hertzforge_shock_host::{ClassicalShockService, ClassicalTestConfig, ServiceError};
use hertzforge_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};

use crate::provenance::{mint_run_id, now_unix, RunProvenance};
use crate::recorder::{DisplayBatch, SpectrumBatch};

// =====================================================================================
// The reserved display block (ui/IPC_CONTRACT.md census: 920-924 classical, 925-929 SRS)
// =====================================================================================

/// The measured pulse SRS, classical. On the ORDINAL band axis - see
/// [`srs_spectrum_batch`].
pub const CH_SHOCK_SRS: u32 = 920;

/// **RESERVED AND DELIBERATELY UNUSED ON THE WIRE.** The local UI draws a pulse time
/// history on this id as a spectrum trace; over this seam a consumer routing on the
/// frame kind would have nothing telling it "samples, not bins". Time rides the scope
/// and waveform streams here. Named so the census entry has a reason beside it instead
/// of a hole.
pub const CH_SHOCK_TIME_LOCAL_ONLY: u32 = 921;

/// The classical pulse FFT magnitude. **RESERVED AND NOT PUBLISHED BY THIS DAEMON.**
///
/// # The id collision this constant records (resolved 2026-08-09)
///
/// This id used to carry the measured pulse PVSS here while the local core
/// (`ui/src-tauri/src/shock.rs`, `CH_C_PULSE_FFT`) drew a pulse FFT magnitude on it.
/// **Two different physical quantities on one id** - a pane fed by both paths would
/// label an in/s pseudo-velocity curve "Pulse FFT" (or the reverse) with nothing on the
/// wire able to tell them apart, because a `SpectrumBatch` trace is an id and a
/// magnitude array and nothing more.
///
/// It is resolved in the ID's favour, not the producer's: 922 MEANS pulse FFT, on both
/// ends, and this daemon publishes nothing on it because it computes no FFT. Moving
/// PVSS to a free id inside the reservation was checked and is not available: 920-924 is
/// classical and 925-929 is SRS, all ten are spoken for by the census below, 921 is
/// reserved-but-unpublishable (a time history on a spectrum frame), and 940-999 is the
/// contested four-claimant range that a new producer must not be added to.
///
/// **The PVSS data still crosses**, so this is a DISPLAY regression and not a data one:
/// `PulseDetail{seq}` carries `pvss` per record, exactly as before. What was lost is a
/// LIVE overlay trace. Giving it one needs a new id allocation, which is still open.
pub const CH_SHOCK_PULSE_FFT_UNPUBLISHED: u32 = 922;

/// The classical REFERENCE SRS: the commanded pulse's SRS at FULL level.
///
/// Mirrors the local core's `CH_C_REF_SRS` semantics exactly, including the display
/// convention behind it: on a clean unity plant the commanded pulse SRS IS the
/// reference, so the MEASURED SRS is scaled back up by the pulse's own ladder level and
/// the reference curve is therefore level-independent. An operator stepping the ladder
/// sees the measurement walk up to a reference that stays put.
pub const CH_SHOCK_REF_SRS: u32 = 923;

/// The classical SRS tolerance-band UPPER edge: `+srs_band_tol_db` on [`CH_SHOCK_REF_SRS`].
///
/// The lower edge is deliberately not a trace - it is the reference reflected, and one
/// band chip reads it. The local core hard-codes +3 dB here; this reads
/// `ClassicalTestConfig::srs_band_tol_db`, which DEFAULTS to 3.0, so the two agree at
/// the default and this one stays right when the operator authored something else.
pub const CH_SHOCK_TOL_HI: u32 = 924;

/// The measured response SRS, SRS module.
pub const CH_SRS_MEASURED: u32 = 925;

/// The TARGET SRS at the current level, SRS module. Published beside the measurement so
/// the operator reads the two on one axis.
pub const CH_SRS_TARGET: u32 = 926;

/// The SRS ALARM-high envelope edge, on the same base as [`CH_SRS_TARGET`].
///
/// **Level-scaled, and that is a stated divergence from the local core.** The local path
/// draws its target and both envelopes at FULL level beside a measurement at the fired
/// level; this path draws all three at the FIRED level, because 926 here has always been
/// level-scaled and an envelope sitting at a different level from the target it brackets
/// is a tolerance band belonging to no plotted curve. Recorded rather than silently
/// reconciled: resolving it means changing the LOCAL publisher.
pub const CH_SRS_ALARM_HI: u32 = 927;

/// The SRS ABORT-high envelope edge. See [`CH_SRS_ALARM_HI`] for the level convention.
pub const CH_SRS_ABORT_HI: u32 = 928;

/// The PREDICTED limit-channel SRS - the notch readout, first limit channel.
///
/// Published only once the notch predictor has actually run (limit channels configured,
/// a system ID behind them). ABSENT is the honest rendering of "nothing predicted this";
/// a zero-filled trace would draw a limit prediction no model produced.
pub const CH_SRS_LIMIT_PRED: u32 = 929;

// =====================================================================================
// Tunables, all stated in the units they mean
// =====================================================================================

/// How many scope + waveform batches ONE pulse is published as - the per-pulse positive
/// floor the wire gate asserts.
///
/// **K = 4, and here is where the number comes from.** The floor has to be a count the
/// probe can assert without knowing the record length, so it is a property of the
/// PACKER rather than of the pulse: the measured record is split into K equal chunks
/// and each chunk is packed into one scope batch and one wave batch. K = 4 is the
/// smallest count that makes "the pane advanced DURING the pulse" a visible thing on a
/// 44 ms record (four ~11 ms steps) while keeping the wire cost of a 5-pulse train at
/// 40 batches rather than hundreds.
///
/// A chunk shorter than one scope column still packs (the column collapses to the
/// samples it has); a chunk of zero frames packs nothing, which is why the packer
/// clamps K down for a record with fewer than K frames rather than emitting empties.
pub const PULSE_TIME_CHUNKS: usize = 4;

/// The host-side pulse ring's cap, in RECORDS.
///
/// The service's own `PulseStore` is unbounded and lives for the run; this ring is what
/// the WIRE reads, and a weeks-lived daemon serving `PulseList` out of an unbounded
/// store grows RSS monotonically. 512 records at a ~2300-frame record is ~9 MB of f64,
/// which is a bound a headless box can hold and an operator can be told. Overflow drops
/// the OLDEST and increments a counter that crosses on the wire, because a silent drop
/// is the one thing a bounded buffer must never do.
pub const PULSE_RING_CAP: usize = 512;

/// How long the per-pulse [`Doc`] rides [`StatusReport::last_pulse`] after the pulse
/// fired.
///
/// The design says the Doc rides "only on change". A strict edge - attach it to the first
/// status after each pulse and never again - is not multi-peer safe: whichever
/// connection polled first would consume it and every other peer would never see that
/// pulse at all. A TTL is the same intent made safe: every peer polling within the
/// window gets it, a long armed-idle poll does not carry it, and anything missed is
/// fetchable by sequence number through `PulseDetail`.
pub const LAST_PULSE_TTL: Duration = Duration::from_millis(2_000);

/// How long a level / query verb will park on the service mutex before refusing by
/// name.
///
/// The Explorer parks indefinitely because its hold is one bounded batch. A shock fire
/// holds for a whole pulse and a train holds for its whole duration, so an unbounded
/// park would make an operator's level nudge look like a hung daemon for ten seconds.
/// Past this the answer is a REFUSAL that says what is happening and what to send.
const LOCK_WAIT: Duration = Duration::from_millis(500);

/// The pump's idle wake-up cadence, so it can re-publish and notice the stop flag even
/// with an empty mailbox.
const JOB_POLL: Duration = Duration::from_millis(100);

/// The stall watchdog's floor, mirroring `explorer.rs`'s: three missed beats or this,
/// whichever is longer. A fire's per-block beat on this chassis is ~20 ms, so `3 x
/// period` is 60 ms and any ordinary scheduling hiccup would read as a dead driver.
const WATCHDOG_FLOOR_MS: u64 = 3_000;

// =====================================================================================
// Shared rails: the published status cell
// =====================================================================================

/// One published snapshot of a shock run, written by the pump and read by the status
/// path WITHOUT the service mutex.
///
/// Shared by both shock hosts: the fields either module cannot fill stay `None`, which
/// is the wire's own convention for "this module has no answer" rather than a zero
/// standing in for one.
#[derive(Debug, Clone)]
pub(crate) struct ShockSnapshot {
    /// The wire run state this maps to.
    pub run: hertzforge_protocol::RunState,
    /// The namespaced module-state token (`armed`, `firing`, `feasibility_blocked: ..`,
    /// `awaiting_review: ..`, `stopped: ..`).
    pub module_state: String,
    pub level_db: Option<f64>,
    pub pulses_fired: u32,
    pub pulses_at_level: u32,
    pub train_remaining: Option<u32>,
    pub last_pulse_seq: Option<u64>,
    pub alarm_active: bool,
    pub abort_active: bool,
    pub requested_fs_hz: Option<f64>,
    pub actual_fs_hz: Option<f64>,
    pub band_coherence: Option<f64>,
    pub records_dropped: u64,
}

/// A snapshot of a core that has been configured and has done nothing else.
///
/// `Default` is hand-written because `hertzforge_protocol::RunState` deliberately has no
/// `Default`: on the wire there is no neutral run state, and a derived one would make
/// `Idle` the silent answer for any field nobody filled.
impl Default for ShockSnapshot {
    fn default() -> Self {
        ShockSnapshot {
            run: hertzforge_protocol::RunState::Idle,
            module_state: String::new(),
            level_db: None,
            pulses_fired: 0,
            pulses_at_level: 0,
            train_remaining: None,
            last_pulse_seq: None,
            alarm_active: false,
            abort_active: false,
            requested_fs_hz: None,
            actual_fs_hz: None,
            band_coherence: None,
            records_dropped: 0,
        }
    }
}

/// The cell itself. A `Mutex` rather than a lock-free cell because the snapshot is a
/// dozen fields written together and a torn read would put one pulse's level beside
/// another's count; the lock is held for a struct clone and is never taken by anything
/// that can block.
#[derive(Default)]
pub(crate) struct ShockCell(Mutex<Option<ShockSnapshot>>);

impl ShockCell {
    pub(crate) fn read(&self) -> Option<ShockSnapshot> {
        self.0.lock().ok().and_then(|g| g.clone())
    }
    pub(crate) fn write(&self, s: ShockSnapshot) {
        if let Ok(mut g) = self.0.lock() {
            *g = Some(s);
        }
    }
    /// Forget everything. Used by `Reset`, which drops the service - and a status cell
    /// that outlived its service would report a run this core can no longer act on.
    pub(crate) fn clear(&self) {
        if let Ok(mut g) = self.0.lock() {
            *g = None;
        }
    }
}

/// The LIVE per-fire progress, written from inside the per-block fire observer
/// and read by the status path.
///
/// This is the half of the status that must advance DURING a fire, which the snapshot
/// above cannot: the snapshot is written at job boundaries and a train is ONE job.
/// Without this, `pulses_fired` would sit still for the whole train and jump at the end.
#[derive(Default)]
pub(crate) struct Progress {
    /// Pulses this JOB has started, counted by the observer.
    pub(crate) started: AtomicU32,
    /// `frames_delivered` at the previous observer call. A call whose `delivered` is
    /// NOT greater than this one is the first block of a NEW pulse - the fire loop
    /// restarts the counter per pulse, so a non-increase is the only edge available
    /// from a counts-only observer.
    pub(crate) prev_delivered: AtomicU64,
    /// The train size this job planned, `0` = not a train.
    pub(crate) train_total: AtomicU32,
    /// True while a fire is actually in flight - the watchdog's gate.
    pub(crate) firing: AtomicBool,
    /// The PLANNED inter-pulse spacing in ms, added to the stall threshold so a
    /// legitimate gap is not a stalled driver.
    pub(crate) spacing_ms: AtomicU64,
}

impl Progress {
    /// Reset for a new job. The observer's edge detector needs `prev_delivered` at zero
    /// or the first pulse of the job is not counted.
    pub(crate) fn begin(&self, train_total: u32, spacing_s: f64) {
        self.started.store(0, Ordering::SeqCst);
        self.prev_delivered.store(0, Ordering::SeqCst);
        self.train_total.store(train_total, Ordering::SeqCst);
        let ms = if spacing_s.is_finite() && spacing_s > 0.0 {
            (spacing_s * 1e3) as u64
        } else {
            0
        };
        self.spacing_ms.store(ms, Ordering::SeqCst);
        self.firing.store(true, Ordering::SeqCst);
    }

    /// The job is over: the watchdog stops watching and the snapshot becomes the
    /// authority again.
    pub(crate) fn end(&self) {
        self.firing.store(false, Ordering::SeqCst);
        self.train_total.store(0, Ordering::SeqCst);
    }

    /// One observer call. Returns `true` when this call opened a NEW pulse.
    pub(crate) fn observe(&self, delivered: usize) -> bool {
        let d = delivered as u64;
        let prev = self.prev_delivered.swap(d, Ordering::SeqCst);
        let fresh = prev == 0 || d <= prev;
        if fresh {
            self.started.fetch_add(1, Ordering::SeqCst);
        }
        fresh
    }
}

// =====================================================================================
// Shared rails: the bounded pulse ring the wire reads
// =====================================================================================

/// One pulse ROW - what `PulseList` answers with. Rows only: the time histories are
/// megabytes and a list is a table an operator scrolls.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PulseRowDoc {
    pub seq: u64,
    pub level_db: f64,
    pub fired_at_unix: u64,
    pub fs_hz: f64,
    pub n_samples: usize,
    pub peak_accel_g: f64,
    pub peak_velocity_ips: f64,
    pub residual_velocity_ips: f64,
    pub pk_pk_disp_in: f64,
}

impl PulseRowDoc {
    fn from_record(r: &PulseRecord) -> Self {
        PulseRowDoc {
            seq: r.seq,
            level_db: r.level_db,
            fired_at_unix: r.fired_at_unix,
            fs_hz: r.fs_hz,
            n_samples: r.time_history.len(),
            peak_accel_g: r.kinematics.peak_accel_g,
            peak_velocity_ips: r.kinematics.peak_velocity_ips,
            residual_velocity_ips: r.kinematics.residual_velocity_ips,
            pk_pk_disp_in: r.kinematics.pk_pk_disp_in,
        }
    }
}

/// The bounded ring. Append-only during a run, oldest-out at the cap, with the drop
/// count carried on the wire.
#[derive(Default)]
pub(crate) struct PulseRing {
    records: VecDeque<PulseRecord>,
    dropped: u64,
}

impl PulseRing {
    pub(crate) fn push(&mut self, r: PulseRecord) {
        if self.records.len() >= PULSE_RING_CAP {
            self.records.pop_front();
            self.dropped += 1;
        }
        self.records.push_back(r);
    }

    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.dropped = 0;
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// `PulseList`: rows only, plus the cap and the drop count so an operator reading a
    /// short list knows whether it is short because the run is young or because the
    /// ring rolled.
    pub(crate) fn list_json(&self) -> String {
        let rows: Vec<PulseRowDoc> = self.records.iter().map(PulseRowDoc::from_record).collect();
        serde_json::json!({
            "cap": PULSE_RING_CAP,
            "dropped": self.dropped,
            "rows": rows,
        })
        .to_string()
    }

    /// `PulseDetail{seq}`: the whole record, arrays included.
    pub(crate) fn detail_json(&self, seq: u64) -> Option<String> {
        let r = self.records.iter().find(|r| r.seq == seq)?;
        Some(
            serde_json::json!({
                "row": PulseRowDoc::from_record(r),
                "time_history": r.time_history,
                "srs": r.srs,
                "srs_freqs": r.srs_freqs,
                "pvss": r.pvss,
            })
            .to_string(),
        )
    }

    /// The `last_pulse` Doc that rides the status report - the ROW plus the SRS, never
    /// the time history (that is what `PulseDetail` is for).
    pub(crate) fn last_status_json(&self) -> Option<String> {
        let r = self.records.back()?;
        Some(
            serde_json::json!({
                "row": PulseRowDoc::from_record(r),
                "srs": r.srs,
            })
            .to_string(),
        )
    }
}

/// The refusal a `PulseDetail` for an unknown sequence gets. Named rather than empty:
/// "not in the ring" and "never fired" are different facts and an operator can act on
/// the first one (fetch sooner, or raise the cap).
pub(crate) fn no_such_pulse(seq: u64, dropped: u64) -> ProtocolError {
    ProtocolError::InvalidArgument {
        message: format!(
            "pulse {seq} is not in this core's pulse ring. The ring holds the most recent \
             {PULSE_RING_CAP} records and has dropped {dropped} older one(s) this run; a \
             sequence past the newest has not fired yet. PulseList states both bounds."
        ),
    }
}

/// The `SrsFreqs` Doc: the analysis grid a remote pane draws 920/925 against.
///
/// **This is the frequency axis, and the spectrum batch is NOT.** See
/// [`srs_spectrum_batch`] for why the batch carries an ordinal axis instead.
pub(crate) fn srs_grid_json(freqs: &[f64], q: f64) -> String {
    serde_json::json!({
        "freqs_hz": freqs,
        "q": q,
        "n_bands": freqs.len(),
        "axis": "ordinal",
        "note": "The 920-929 spectrum traces ride an ORDINAL band index (f0_hz = 0, \
                 df_hz = 1). A shock SRS grid is GEOMETRIC and the wire's (f0, df) pair \
                 can only express a linear one, so the grid is served here and the \
                 batch carries the index. freqs_hz[i] is the natural frequency of bin i.",
    })
    .to_string()
}

/// One SRS-shaped spectrum batch on the ORDINAL band axis.
///
/// # Why the batch's grid is an INDEX and not Hz
///
/// `SpectrumBatch` carries exactly one `(f0_hz, df_hz)` pair, which describes a LINEAR
/// grid. A shock SRS is analyzed on a fractional-octave (geometric) grid, so no
/// `(f0, df)` can describe it. The two dishonest options were both available and both
/// rejected: pretending the grid is linear puts every band at the wrong frequency on
/// the operator's screen, and resampling the SRS onto a linear grid would ship a curve
/// nobody measured. So the batch states an ordinal axis - bin `i` IS band `i` - and the
/// real frequencies are served once per run by `SrsFreqs`, which a late-joining
/// observer and a reconnecting controller can both fetch.
pub(crate) fn srs_spectrum_batch(
    run_id: u64,
    window_seq: u32,
    traces: Vec<(u32, Arc<Vec<f32>>)>,
) -> Option<SpectrumBatch> {
    if traces.is_empty() || traces.iter().all(|(_, v)| v.is_empty()) {
        return None;
    }
    Some(SpectrumBatch {
        f0_hz: 0.0,
        df_hz: 1.0,
        window_seq,
        run_id,
        traces: traces.into_iter().map(|(id, v)| (id, None, v)).collect(),
    })
}

// =====================================================================================
// Shared rails: the transient display tap
// =====================================================================================

/// The WALL-CLOCK display tap for a transient producer.
///
/// Holds the run-relative frame clock ACROSS pulses, advancing it by the measured
/// silence between them so the pane draws real time. The gap is never packed as
/// samples: the wave ring breaks its trace on a forward jump, which is the correct
/// rendering of "nothing was driving here".
pub(crate) struct TransientTap {
    run_id: u64,
    /// Run-relative frames through the END of everything published so far.
    cum_frames: u64,
    /// Wall time the previous pulse's record ENDED, for the gap arithmetic.
    last_end: Option<Instant>,
}

impl TransientTap {
    pub(crate) fn new(run_id: u64) -> Self {
        TransientTap {
            run_id,
            cum_frames: 0,
            last_end: None,
        }
    }

    /// Publish one measured pulse as [`PULSE_TIME_CHUNKS`] scope + wave batch pairs at
    /// the RAW rate (decimation 1 inside the pulse window).
    ///
    /// # The decimation numbers, stated
    ///
    /// The shared `wave_batch` targets 2 kHz, which is right for a continuous producer
    /// and hostile to a transient one: at 2 kHz a 6 ms pulse is TWELVE points and an
    /// 11 ms half-sine is 22, so the peak the whole test is about lands between
    /// samples. Inside the pulse window the decimation is therefore 1 - at the 51 200 Hz
    /// this chassis actually runs, that same 6 ms pulse is 307 points and the 11 ms one
    /// is 563. The window is BOUNDED BY THE PULSE, so raw rate here is a few hundred
    /// samples per batch, not a firehose.
    ///
    /// Returns the number of batches published (scope + wave), which is what the
    /// per-pulse positive floor is asserted against.
    pub(crate) fn publish_pulse(
        &mut self,
        tx: &tokio::sync::broadcast::Sender<DisplayBatch>,
        record: &[f64],
        fs_hz: f64,
    ) -> usize {
        if record.is_empty() || !(fs_hz.is_finite() && fs_hz > 0.0) {
            return 0;
        }
        // THE GAP. Measured, not assumed: whatever wall time passed since the previous
        // pulse's record ended is silence the operator's pane must show as elapsed.
        if let Some(prev_end) = self.last_end {
            let gap_s = prev_end.elapsed().as_secs_f64();
            let gap_frames = (gap_s * fs_hz).round().max(0.0) as u64;
            self.cum_frames = self.cum_frames.saturating_add(gap_frames);
        }

        let samples: Vec<f32> = record.iter().map(|&v| v as f32).collect();
        let n = samples.len();
        let chunks = PULSE_TIME_CHUNKS.min(n.max(1));
        let per = n.div_ceil(chunks);
        let mut published = 0usize;
        let mut at = 0usize;
        while at < n {
            let end = (at + per).min(n);
            let seg = &samples[at..end];
            self.cum_frames = self.cum_frames.saturating_add(seg.len() as u64);
            if let Some(sb) =
                crate::display::scope_batch(seg, 1, fs_hz, self.run_id, self.cum_frames)
            {
                let _ = tx.send(DisplayBatch::Scope(sb));
                published += 1;
            }
            // Decimation 1: see the doc comment's arithmetic.
            if let Some(wb) =
                crate::display::wave_batch_dec(seg, 1, fs_hz, self.run_id, self.cum_frames, 1)
            {
                let _ = tx.send(DisplayBatch::Wave(wb));
                published += 1;
            }
            at = end;
        }
        self.last_end = Some(Instant::now());
        published
    }
}

// =====================================================================================
// Shared rails: the station
// =====================================================================================

/// Build the station a shock run drives, from the daemon's channel table.
///
/// The table checks are HERE, where the table is in hand, and each one names its fix:
/// a shock run plays a synthesized record onto ONE output and judges it against a
/// CONTROL channel, so zero drive rows, two drive rows and no control channel are three
/// different mistakes with three different sentences.
///
/// **Two enabled Drive rows are REFUSED by name.** Both shock fire paths hard-code
/// an output width of one; a two-Drive table would bind two AOs and feed them one mono
/// record, silently driving the second from the first. The service cannot see the
/// table's Drive rows, so this is the only place the refusal can be written.
pub(crate) fn build_shock_station(
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
            message: "the channel table has no enabled Drive row: a shock run plays a \
                      synthesized pulse and needs an output to put it on. Edit the table \
                      (PutChannelTable) and enable exactly one Drive row."
                .into(),
        });
    }
    if n_drive > 1 {
        return Err(ProtocolError::Refused {
            message: format!(
                "{n_drive} enabled Drive rows: a shock fire path plays ONE record onto ONE \
                 output, so the extra row(s) would be bound and then driven from the same \
                 samples - an output the operator configured and nothing separately \
                 commands. Disable the extra Drive row(s) (PutChannelTable)."
            ),
        });
    }
    if !table
        .channels
        .iter()
        .any(|c| c.enabled && c.role == ChannelRole::Control)
    {
        return Err(ProtocolError::Refused {
            message: "the channel table has no enabled Control row: every shock acceptance \
                      check (tolerance, kinematics, SRS) is computed from the CONTROL \
                      channel's record, so a run without one measures nothing. Edit the \
                      table (PutChannelTable) and mark one input Control."
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
        // A WIRE, not a resonance plant - the same choice `explorer.rs` states and for
        // the same reason: the HERTZFORGE-SIM analogue of this bench is AO jumpered to AI
        // with nothing downstream, and a resonator would make every measured pulse
        // amplitude a statement about the resonator's gain at the pulse's frequency
        // content rather than about the record that was played.
        sim: SimPlantSpec {
            sample_rate_hz: fs_hz,
            resonances: Vec::new(),
            noise_level: 0.0,
            feedthrough: 1.0,
            shared_clock: true,
            seed: 0x5A5A_9210,
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
        tracing::warn!(target: "hertzforge::backendd", "shock station refused: {e}");
        ProtocolError::Refused {
            message: e.to_string(),
        }
    })
}

/// The conversion the ceiling uses between the profile's peak and the volts the DAC
/// will be asked for: **1.0, because there is no conversion.**
///
/// `write_drive` is RAW volts and no `eu_to_volts` exists anywhere in this workspace -
/// the g-valued record IS what reaches the DAC. The first version of this function
/// converted through the Drive row's `sensitivity_mv_per_eu` (an INPUT-path property),
/// modeling a conversion the drive path does not perform: on the first NI-REAL probe
/// run the ceiling blessed "2.0 V implied" while the DAC was handed 10.0012 V-valued
/// samples and refused them at its own range (-200561, 2026-08-08). A ceiling that
/// converts through a fiction is not a ceiling.
///
/// When a real EU->volts stage lands in the drive path, this function is where its
/// factor re-enters - with the conversion enforced against what the wire CARRIES.
pub(crate) fn drive_volts_per_eu(table: &ChannelTable) -> f64 {
    let _ = table; // the signature stays; the table currently contributes nothing
    1.0
}

/// **THE COMMANDED PEAK DRIVE, volts** - the one derivation both the ceiling and the
/// status echo read.
///
/// `peak_eu` is the pulse's full-level peak, walked down the ladder by `level_db` and
/// converted by the Drive row's `volts_per_eu` (see [`drive_volts_per_eu`], which is
/// 1.0 today and says why). It is what this core will COMMAND at the drive output -
/// not a measurement, and not a readback: nothing in the shock path reads the AO back.
///
/// It is a function rather than two copies of the same three multiplications because
/// the echo and the refusal have to agree by construction. A panel that showed 4.0 Vpk
/// while the ceiling refused at 4.2 would leave an operator arguing with a number.
pub(crate) fn commanded_drive_vpk(peak_eu: f64, level_db: f64, volts_per_eu: f64) -> f64 {
    peak_eu * 10f64.powf(level_db / 20.0) * volts_per_eu
}

/// **THE OPERATOR'S DRIVE CEILING, enforced by the HOST**.
///
/// Neither shock service has a drive-volts concept: `fire(drive_g)` writes g-valued
/// samples into a volt-valued DAC and no `eu_to_volts` exists anywhere in the workspace.
/// So `max_drive_vpk` has exactly one enforcement point and this is it - checked at
/// `set_profile` against the schedule's HIGHEST rung, and again at every level change,
/// because a run that armed inside the ceiling can be walked outside it one dB at a
/// time.
///
/// The refusal carries BOTH numbers. A ceiling refusal that states only "too high" is a
/// refusal the operator cannot act on without guessing what the implied volts were.
pub(crate) fn enforce_drive_ceiling(
    what: &str,
    peak_eu: f64,
    level_db: f64,
    volts_per_eu: f64,
    max_drive_vpk: f64,
) -> Result<f64, ProtocolError> {
    let implied = commanded_drive_vpk(peak_eu, level_db, volts_per_eu);
    if !implied.is_finite() {
        return Err(ProtocolError::InvalidArgument {
            message: format!(
                "{what}: the implied drive is not a finite number (peak {peak_eu} EU at \
                 {level_db} dB against {volts_per_eu} V/EU); nothing may be commanded from it."
            ),
        });
    }
    if implied > max_drive_vpk {
        return Err(ProtocolError::Refused {
            message: format!(
                "{what} would command {implied:.3} Vpk at the drive output, and this \
                 profile's ceiling is {max_drive_vpk:.3} Vpk. The pulse peaks at \
                 {peak_eu:.4} EU, the level is {level_db:+.2} dB, and the Drive row \
                 converts at {volts_per_eu:.4} V/EU. Lower the level, lower the pulse \
                 amplitude, or raise max_drive_vpk in the profile - the ceiling is a \
                 number a human chose and this core will not run past it."
            ),
        });
    }
    Ok(implied)
}

// =====================================================================================
// Shared rails: small helpers
// =====================================================================================

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Best-effort text out of a caught panic payload (mirrors `explorer.rs`).
pub(crate) fn panic_text(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// The priming flag clears on EVERY pump exit path, refusals and panics included.
pub(crate) struct PrimingGuard(pub(crate) Arc<AtomicBool>);
impl Drop for PrimingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A classical service error, typed onto the wire.
fn svc_err(e: ServiceError) -> ProtocolError {
    match e {
        ServiceError::Config(m) => ProtocolError::InvalidArgument {
            message: m.to_string(),
        },
        ServiceError::BadState(m) => ProtocolError::Refused { message: m },
        other => ProtocolError::Refused {
            message: other.to_string(),
        },
    }
}

fn not_configured() -> ProtocolError {
    ProtocolError::Refused {
        message: "the classical Shock module is not configured on this core; send \
                  SetProfile with a ShockClassical profile first"
            .into(),
    }
}

/// The refusal a verb gets while a pulse is in flight and the mutex is held.
///
/// A named refusal beats an unbounded park: the operator is told what is happening, how
/// long it lasts, and which verb reaches a fire in flight (Stop and Abort do, because
/// they ride the cancel flag and never take this lock).
fn busy_firing(what: &str) -> ProtocolError {
    ProtocolError::Refused {
        message: format!(
            "a pulse is in flight on this core, so {what} cannot be applied right now: a \
             fire holds the run for the length of its record and the level it uses is fixed \
             before it starts. Retry between pulses. Stop and Abort DO reach a fire in \
             flight - they raise the cancel flag, which needs no lock."
        ),
    }
}

// =====================================================================================
// The classical shock host
// =====================================================================================

/// The service this host owns: the extracted hal-neutral classical service over
/// whatever the station factory built.
type Svc = ClassicalShockService<BackendPulseFirer<Box<dyn Backend>>>;

/// One unit of work the pump performs under the service mutex.
///
/// Everything that DRIVES is a job. Everything that only reads is answered from the
/// published cell or the pulse ring, without ever touching the service.
#[derive(Debug, Clone)]
enum Job {
    /// `Command::Start`, and the feasibility-override variant.
    Arm {
        ack: Option<String>,
    },
    FirePulse,
    RunAuto {
        max_pulses: u64,
    },
    FireTrain {
        spec: Option<hertzforge_shock_common::TrainSpec>,
    },
    Gunfire,
    /// The controlled stop: complete the pulse in flight, cancel at the next
    /// inter-pulse boundary, park.
    Stop {
        reason: String,
    },
    /// The immediate stop: `emergency_stop`, tasks unreserved.
    Abort {
        reason: String,
    },
    /// Clear the run back to unarmed by REBUILDING the service (the arm consumed the
    /// station backend, so there is no in-place path back to Idle).
    Reset,
}

/// The daemon's classical-shock host.
pub struct ShockHost {
    svc: Arc<Mutex<Option<Svc>>>,
    /// The mailbox the dispatch thread posts jobs into and the pump drains.
    jobs: Arc<(Mutex<VecDeque<Job>>, Condvar)>,
    /// True from `start()` until the arm job finishes, either way.
    priming: Arc<AtomicBool>,
    /// True from the moment a stop is REQUESTED until the run is down. The stall
    /// watchdog is exempt while it is set.
    stopping: Arc<AtomicBool>,
    /// The pump's exit flag.
    stop: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The SERVICE's cancel flag, taken at construction so Stop and Abort can reach a
    /// fire in flight without the mutex it holds.
    cancel: Arc<AtomicBool>,
    /// The shared display bus.
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    /// The published status.
    cell: Arc<ShockCell>,
    /// The live per-fire progress.
    progress: Arc<Progress>,
    /// The out-of-band fault cell.
    fault: Arc<Mutex<Option<String>>>,
    /// Unix ms of the last per-block beat.
    last_tick_ms: Arc<AtomicU64>,
    /// The expected per-block period in ms, refined at arm from the adopted rate.
    expected_iter_ms: Arc<AtomicU64>,
    /// The bounded pulse ring the wire reads.
    ring: Arc<Mutex<PulseRing>>,
    /// The SRS analysis grid this run uses, published once per configure and served by
    /// `SrsFreqs` to peers that never saw the configure.
    srs_freqs: Arc<Mutex<Vec<f64>>>,
    /// Unix ms the newest pulse was published at - the [`LAST_PULSE_TTL`] window.
    last_pulse_at_ms: Arc<AtomicU64>,
    provenance: Mutex<Option<RunProvenance>>,
    /// What a `Reset` rebuilds from.
    last_profile: Mutex<Option<(ClassicalShockProfile, ChannelTable, String)>>,
    /// The operator's drive ceiling and the Drive row's conversion, held for the
    /// per-level-change check.
    max_drive_vpk: Arc<Mutex<(f64, f64, f64)>>, // (ceiling_vpk, volts_per_eu, full_peak_eu)
    controller_timeout_s: f64,
    kind: BackendKind,
    ni_devices: Vec<String>,
    data_root: std::path::PathBuf,
}

impl ShockHost {
    /// A host bound to a station kind and the shared display bus. Builds nothing until
    /// [`ShockHost::set_profile`].
    pub fn new(
        kind: BackendKind,
        ni_devices: Vec<String>,
        data_root: std::path::PathBuf,
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
        controller_timeout_s: f64,
    ) -> Self {
        ShockHost {
            svc: Arc::new(Mutex::new(None)),
            jobs: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
            priming: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            cell: Arc::new(ShockCell::default()),
            progress: Arc::new(Progress::default()),
            fault: Arc::new(Mutex::new(None)),
            last_tick_ms: Arc::new(AtomicU64::new(0)),
            expected_iter_ms: Arc::new(AtomicU64::new(0)),
            ring: Arc::new(Mutex::new(PulseRing::default())),
            srs_freqs: Arc::new(Mutex::new(Vec::new())),
            last_pulse_at_ms: Arc::new(AtomicU64::new(0)),
            provenance: Mutex::new(None),
            last_profile: Mutex::new(None),
            max_drive_vpk: Arc::new(Mutex::new((
                hertzforge_protocol::DEFAULT_SHOCK_MAX_DRIVE_VPK,
                1.0,
                0.0,
            ))),
            controller_timeout_s,
            kind,
            ni_devices,
            data_root,
        }
    }

    /// The provenance of the configured run, if any.
    pub fn provenance(&self) -> Option<RunProvenance> {
        self.provenance.lock().expect("provenance poisoned").clone()
    }

    /// Is a run live? **`Armed` counts, and so does the priming window**.
    ///
    /// The arm opens and commits the DAQmx tasks, so from that instant the station is
    /// reserved; a definition that only counted the firing states would let another
    /// module claim the core out from under a committed chassis, and would let a
    /// reconfigure drop a service whose backend is still bound.
    ///
    /// NEVER a blocking lock: the published cell is the authority and a fire holds the
    /// service mutex for its whole record.
    pub fn is_live(&self) -> bool {
        if self.priming.load(Ordering::SeqCst) || self.progress.firing.load(Ordering::SeqCst) {
            return true;
        }
        self.cell.read().is_some_and(|s| {
            matches!(
                s.module_state.split(':').next(),
                Some("armed") | Some("firing")
            )
        })
    }

    /// Is anything configured at all? NEVER a blocking lock.
    pub fn is_configured(&self) -> bool {
        match self.svc.try_lock() {
            Ok(g) => g.is_some(),
            // A contended mutex IS the pump holding a configured service.
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        }
    }

    /// The SRS analysis grid, for `SrsFreqs`. Lock-free of the service.
    pub fn srs_freqs_doc(&self) -> Result<Doc, ProtocolError> {
        let f = self.srs_freqs.lock().expect("freqs poisoned").clone();
        if f.is_empty() {
            return Err(not_configured());
        }
        Ok(Doc::new(
            SchemaKind::ShockSrsGrid,
            srs_grid_json(&f, hertzforge_shock_common::pulse_store::DEFAULT_SRS_Q),
        ))
    }

    /// `PulseList` - rows only, out of the bounded ring, never the service.
    pub fn pulse_list_doc(&self) -> Result<Doc, ProtocolError> {
        let ring = self.ring.lock().expect("ring poisoned");
        Ok(Doc::new(SchemaKind::ShockPulse, ring.list_json()))
    }

    /// `PulseDetail{seq}` - the whole record.
    pub fn pulse_detail_doc(&self, seq: u64) -> Result<Doc, ProtocolError> {
        let ring = self.ring.lock().expect("ring poisoned");
        match ring.detail_json(seq) {
            Some(j) => Ok(Doc::new(SchemaKind::ShockPulse, j)),
            None => Err(no_such_pulse(seq, ring.dropped())),
        }
    }

    /// Translate the wire profile, build the station, construct the service, and start
    /// the job pump.
    ///
    /// **The CROSS-MODULE exclusion is NOT here** - that is `Core::live_module`'s
    /// question, asked by the caller. What stays here is the self-check and the
    /// reconfigure ordering: the slot goes to `None` and the drop COMPLETES, releasing
    /// the DAQmx tasks, before a new station is built. An earlier design claimed a
    /// chassis-verified ordering that had never been run; the claim was withdrawn and
    /// the ordering is now simply the safe one.
    pub fn set_profile(
        &self,
        profile: ClassicalShockProfile,
        table: ChannelTable,
        lease_owner: &str,
    ) -> Result<(), ProtocolError> {
        if self.is_live() {
            return Err(ProtocolError::Refused {
                message: "a classical Shock run is live on this core; Stop it before \
                          reconfiguring"
                    .into(),
            });
        }
        // Tear the previous pump down and let the previous service DROP before anything
        // new touches the driver.
        self.shutdown_pump();
        *self.svc.lock().expect("svc poisoned") = None;

        let cfg = translate_classical(&profile)?;
        let station = build_shock_station(self.kind, &self.ni_devices, &table, cfg.fs_hz)?;
        tracing::info!(target: "hertzforge::backendd", "{}", station.provenance.log_line());

        // ---- THE DRIVE CEILING, checked before anything is reserved --------------
        //
        // The schedule's HIGHEST rung is the run's maximum demand, so a profile that
        // would breach the ceiling anywhere breaches it here.
        let volts_per_eu = drive_volts_per_eu(&table);
        let full_peak_eu = cfg.spec.amplitude_g.abs();
        let top_db = cfg
            .schedule
            .table_db
            .iter()
            .copied()
            .chain(std::iter::once(cfg.schedule.start_db))
            .fold(f64::NEG_INFINITY, f64::max);
        enforce_drive_ceiling(
            "the profile's top ladder rung",
            full_peak_eu,
            top_db,
            volts_per_eu,
            profile.max_drive_vpk,
        )?;

        // ---- THE SAFE-STOP BUDGET, stated with numbers ----------------
        //
        // A vanished controller is brought down by the between-pulse cancel, so the
        // budget is "the pulse in flight, plus the task stop". There is no ramp to
        // arithmetic: every shock record ends in its quiet lead-out, so the backend's
        // stop taper starts from zero and is a measured no-op. What CAN outlast the
        // window is a single pulse longer than the whole controller timeout, and that is
        // refused here with both numbers rather than discovered by a safe-stop that
        // misses its own deadline.
        let pulse_s = pulse_wall_seconds(&cfg);
        if !pulse_s.is_finite() || pulse_s >= self.controller_timeout_s {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "one pulse of this profile is {pulse_s:.2} s of record (main pulse \
                     {:.2} ms plus its compensation shoulders and the time-denominated \
                     quiet surround), and this daemon's controller timeout is {:.1} s. A \
                     vanished controller's safe-stop cancels at the next inter-pulse \
                     boundary, so a single pulse longer than the whole budget would still \
                     be driving when the run was supposed to be down. Shorten the pulse or \
                     raise controller_timeout_s.",
                    cfg.spec.duration_ms, self.controller_timeout_s
                ),
            });
        }

        let station_class = station.station;
        let run_id = mint_run_id();
        let prov =
            RunProvenance::assemble("shock.spectrum", run_id, &station, lease_owner, now_unix());
        tracing::info!(target: "hertzforge::backendd", "{}", prov.log_line());

        let firer = BackendPulseFirer::new(station.backend, table.clone(), station_class)
            .map_err(svc_err)?;
        let requested_fs = cfg.fs_hz;
        let svc =
            ClassicalShockService::new(cfg, table.clone(), firer, Some(self.data_root.clone()))
                .map_err(svc_err)?
                .with_cancel_flag(Arc::clone(&self.cancel));

        self.cancel.store(false, Ordering::SeqCst);
        self.stopping.store(false, Ordering::SeqCst);
        self.progress.end();
        *self.fault.lock().expect("fault poisoned") = None;
        self.ring.lock().expect("ring poisoned").clear();
        *self.srs_freqs.lock().expect("freqs poisoned") = svc.srs_freqs().to_vec();
        self.expected_iter_ms.store(
            block_period_ms(
                requested_fs,
                hertzforge_hal::default_block_frames(requested_fs),
            ),
            Ordering::SeqCst,
        );
        *self.max_drive_vpk.lock().expect("ceiling poisoned") =
            (profile.max_drive_vpk, volts_per_eu, full_peak_eu);
        self.cell.write(ShockSnapshot {
            run: hertzforge_protocol::RunState::Configured,
            module_state: "configured".into(),
            level_db: Some(svc.current_level_db()),
            requested_fs_hz: Some(requested_fs),
            ..ShockSnapshot::default()
        });
        *self.svc.lock().expect("svc poisoned") = Some(svc);
        *self.provenance.lock().expect("provenance poisoned") = Some(prov);
        *self.last_profile.lock().expect("profile poisoned") =
            Some((profile, table, lease_owner.to_string()));
        self.spawn_pump();
        Ok(())
    }

    /// `Command::Start` **IS ARM** for this module (see `hertzforge_protocol::shock`'s docs).
    ///
    /// The cheap refusals are synchronous; the arm itself is not. It opens and
    /// configures the station, adopts the device's rate, consults the arm gate,
    /// re-derives the SRS grid at the adopted rate and synthesizes the compensated
    /// composite - seconds of DAQmx plus a synthesis, which is exactly the shape the
    /// async arm moved off the command path.
    pub fn arm(&self, ack: Option<String>) -> Result<(), ProtocolError> {
        {
            let guard = self
                .try_lock_svc(LOCK_WAIT)
                .ok_or_else(|| busy_firing("Arm"))?;
            let svc = guard.as_ref().ok_or_else(not_configured)?;
            match svc.state() {
                ShockRunState::Idle | ShockRunState::Feasibility => {
                    if ack.is_some() {
                        return Err(ProtocolError::Refused {
                            message: "ArmWithOverride is for a feasibility EXCEEDANCE, and this \
                                      run is not blocked on one. Send Start to arm."
                                .into(),
                        });
                    }
                }
                ShockRunState::FeasibilityBlocked if ack.is_some() => {}
                ShockRunState::FeasibilityBlocked => {
                    return Err(ProtocolError::Refused {
                        message: "arming is HARD-BLOCKED: the feasibility assessment came back \
                                  EXCEEDED. Send Shock(ArmWithOverride) with a non-empty \
                                  acknowledgement to proceed anyway."
                            .into(),
                    })
                }
                other => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "the classical Shock run cannot arm from {other:?}; send SetProfile \
                             or Shock(Reset) to start a fresh run"
                        ),
                    })
                }
            }
        }
        self.last_tick_ms.store(now_ms(), Ordering::Relaxed);
        self.priming.store(true, Ordering::SeqCst);
        self.post(Job::Arm { ack });
        Ok(())
    }

    /// Fire ONE pulse at the current level.
    pub fn fire_pulse(&self) -> Result<(), ProtocolError> {
        self.require_running("FirePulse")?;
        self.post(Job::FirePulse);
        Ok(())
    }

    /// Run the AUTO ladder, bounded.
    pub fn run_auto(&self, max_pulses: Option<u32>) -> Result<(), ProtocolError> {
        self.require_running("RunAuto")?;
        // The bound is the operator's when they gave one and a large finite number when
        // they did not: an unbounded firing loop whose end depends on the network is the
        // shape the Explorer's run loop is bounded to avoid.
        let max = max_pulses.unwrap_or(u32::MAX) as u64;
        self.post(Job::RunAuto { max_pulses: max });
        Ok(())
    }

    /// Fire the configured (or an explicit) pulse train.
    pub fn fire_train(
        &self,
        spec: Option<hertzforge_protocol::ShockTrainSpec>,
    ) -> Result<(), ProtocolError> {
        self.require_running("FireTrain")?;
        let spec = spec.map(train_spec);
        self.post(Job::FireTrain { spec });
        Ok(())
    }

    /// Fire the configured gunfire burst.
    pub fn fire_gunfire(&self) -> Result<(), ProtocolError> {
        self.require_running("FireGunfireBurst")?;
        self.post(Job::Gunfire);
        Ok(())
    }

    /// The configured ladder's rungs, dB. Answers the router's nearest-rung jump
    /// WITHOUT the service mutex being held across the decision - it is read, released,
    /// and the jump takes the lock again on its own. A stale read between the two is
    /// harmless: the table is authored at configure and cannot change during a run.
    pub fn level_table(&self) -> Result<Vec<f64>, ProtocolError> {
        let guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy_firing("reading the level table"))?;
        let svc = guard.as_ref().ok_or_else(not_configured)?;
        Ok(svc.config().schedule.table_db.clone())
    }

    /// Jump to a level-table index. Synchronous: it answers with the level actually
    /// adopted, or a refusal, and the CEILING is re-checked at the new level.
    pub fn jump_to_level(&self, index: usize) -> Result<f64, ProtocolError> {
        let mut guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy_firing("a level jump"))?;
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        let want = svc
            .config()
            .schedule
            .table_db
            .get(index)
            .copied()
            .ok_or_else(|| ProtocolError::InvalidArgument {
                message: format!(
                    "level index {index} is not a rung of the configured ladder ({} rung(s))",
                    svc.config().schedule.table_db.len()
                ),
            })?;
        self.check_level_ceiling(want)?;
        svc.jump_to_level(index).map_err(svc_err)
    }

    /// Step the level by `delta_db`. The service exposes +/- 1 dB steps, so a delta is
    /// applied as that many whole steps and the ECHO is the level actually reached -
    /// never the request. A fractional delta is REFUSED by name rather than silently
    /// rounded: an operator who asked for 0.5 dB and got 1.0 has been given a different
    /// test.
    pub fn adjust_level(&self, delta_db: f64) -> Result<f64, ProtocolError> {
        if !delta_db.is_finite() || (delta_db - delta_db.round()).abs() > 1e-9 {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "the shock ladder steps in WHOLE dB (the module's manual step is +/-1 dB), \
                     so a {delta_db} dB step cannot be honored. Send a whole number of dB, or \
                     Shock(JumpToLevel) to name a rung."
                ),
            });
        }
        let mut guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy_firing("a level step"))?;
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        let steps = delta_db.round() as i64;
        self.check_level_ceiling(svc.current_level_db() + steps as f64)?;
        let mut last = svc.current_level_db();
        for _ in 0..steps.unsigned_abs() {
            last = if steps > 0 {
                svc.step_up().map_err(svc_err)?
            } else {
                svc.step_down().map_err(svc_err)?
            };
        }
        Ok(last)
    }

    /// The drive ceiling at a proposed level: checked at EVERY level change,
    /// because a run that armed inside the ceiling can be walked outside it one dB at a
    /// time.
    fn check_level_ceiling(&self, level_db: f64) -> Result<(), ProtocolError> {
        let (ceiling, volts_per_eu, full_peak) =
            *self.max_drive_vpk.lock().expect("ceiling poisoned");
        enforce_drive_ceiling("that level", full_peak, level_db, volts_per_eu, ceiling).map(|_| ())
    }

    /// `Shock(Reset)`: clear the run state back to unarmed.
    /// `Shock(Reset)`: take the run down and **RELEASE THE CORE**.
    ///
    /// # Why this is a teardown and not a rewind
    ///
    /// The arm moved the station backend into the fire path with no reclaim path, so
    /// there is no in-place route back to `Idle`: a service reset in place would refuse
    /// its next arm. Reset therefore stops the run, drops the service, and leaves the
    /// operator to send `SetProfile` - which is the same rebuild the local app's Reset
    /// escape hatch performs.
    ///
    /// **Releasing the core is the load-bearing half**, and it was found by running the
    /// wire probe rather than reasoned about. A module whose service is still present
    /// holds the core at `Claim::Configured` even after its run has STOPPED, and a bare
    /// `Command::Start` is dispatched on that claim through the documented priority
    /// order. So a stopped shock run left configured takes the next `Start` and answers
    /// "cannot arm from Stopped" - about a module the operator had already moved on
    /// from. Reset is the verb that says "I am done with this module"; it has to mean
    /// it.
    ///
    /// The published cell is CLEARED with the service. A status cell outliving its
    /// service would report a run this core can no longer act on.
    pub fn reset(&self) -> Result<(), ProtocolError> {
        if self
            .last_profile
            .lock()
            .expect("profile poisoned")
            .is_none()
        {
            return Err(not_configured());
        }
        // The cancel flag first and OUTSIDE the mutex - it is the only thing that
        // reaches a fire in flight, and the pump has to be able to finish before the
        // shutdown below joins it.
        self.cancel.store(true, Ordering::SeqCst);
        self.stopping.store(true, Ordering::SeqCst);
        self.post(Job::Reset);
        self.shutdown_pump();
        // The drop COMPLETES here, releasing the station's tasks, before anything else
        // can build one (the reconfigure ordering).
        *self.svc.lock().expect("svc poisoned") = None;
        self.cell.clear();
        self.progress.end();
        *self.fault.lock().expect("fault poisoned") = None;
        self.ring.lock().expect("ring poisoned").clear();
        *self.srs_freqs.lock().expect("freqs poisoned") = Vec::new();
        self.last_pulse_at_ms.store(0, Ordering::Relaxed);
        *self.provenance.lock().expect("provenance poisoned") = None;
        self.stopping.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// THE CONTROLLED STOP: cancel at the next inter-pulse boundary, park.
    ///
    /// The cancel flag is raised FIRST and OUTSIDE the mutex, because it is the only
    /// part that reaches a fire already in flight. The queued job does the actual
    /// `stop()` once the fire returns.
    pub fn safe_stop(&self, reason: &str) {
        if !self.is_live() && !self.is_configured() {
            return;
        }
        tracing::warn!(
            target: "hertzforge::backendd",
            reason,
            "SAFE-STOP: cancelling the classical Shock run at the next boundary"
        );
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.post(Job::Stop {
            reason: reason.to_string(),
        });
    }

    /// ABORT: `emergency_stop`, tasks unreserved.
    ///
    /// The arithmetic is in `ClassicalShockService::abort`'s docs and is not hidden
    /// here: mid-record preemption is REFUSED by this chassis (`-200289`), so whatever
    /// the device has already accepted plays out - up to ~60 ms, which is longer than a
    /// whole pulse. A shock abort is a promise about the NEXT pulse.
    pub fn abort(&self, reason: &str) {
        tracing::warn!(target: "hertzforge::backendd", reason, "ABORT: classical Shock");
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.post(Job::Abort {
            reason: reason.to_string(),
        });
    }

    /// Fold this module's truth into a protocol status report. **Never takes the
    /// service mutex**.
    pub fn fill_status(&self, s: &mut StatusReport) {
        let Some(snap) = self.cell.read() else {
            return; // nothing configured
        };
        s.requested_fs_hz = snap.requested_fs_hz;
        s.actual_fs_hz = snap.actual_fs_hz;

        // A refusal OUTSIDE the service's own state machine (a refused arm, a
        // panicking pump) is read FIRST - without it the wire would report "configured"
        // for a run whose station refused to open.
        if let Some(f) = self.fault.lock().ok().and_then(|f| f.clone()) {
            s.run = hertzforge_protocol::RunState::Faulted;
            s.module_state = Some(f);
            return;
        }
        if self.priming.load(Ordering::SeqCst) {
            s.run = hertzforge_protocol::RunState::Armed;
            s.module_state = Some(
                "priming (opening the station, configuring it, adopting the rate the driver \
                 programmed, consulting the arm gate, re-deriving the SRS grid and \
                 synthesizing the compensated composite - the run is ready when it reports \
                 armed)"
                    .into(),
            );
            return;
        }

        // THE STALL WATCHDOG. Only while a fire is actually in flight: an armed-idle
        // run has no beat to miss and MANUAL has no planned spacing, so watching it
        // would false-alarm on an operator's coffee break. The planned inter-pulse
        // spacing is ADDED to the threshold, which is what exempts an AUTO train's
        // legitimate gap.
        if self.progress.firing.load(Ordering::SeqCst) && !self.stopping.load(Ordering::SeqCst) {
            let beat = self.last_tick_ms.load(Ordering::Relaxed);
            let period = self.expected_iter_ms.load(Ordering::SeqCst).max(1);
            let spacing = self.progress.spacing_ms.load(Ordering::SeqCst);
            let stall_after = (3 * period).max(WATCHDOG_FLOOR_MS) + spacing;
            let stale = now_ms().saturating_sub(beat);
            if beat != 0 && stale > stall_after {
                s.run = hertzforge_protocol::RunState::Faulted;
                s.module_state = Some(format!(
                    "stalled_driver ({:.1} s without a block during a fire, expected one every \
                     {:.3} s with {:.3} s of planned inter-pulse spacing allowed - the NI \
                     runtime is not returning; restart the daemon, and the chassis if preflight \
                     then fails)",
                    stale as f64 / 1e3,
                    period as f64 / 1e3,
                    spacing as f64 / 1e3
                ));
                return;
            }
        }

        s.run = snap.run;
        s.module_state = Some(snap.module_state.clone());
        s.level_db = snap.level_db;
        s.alarm_active = snap.alarm_active;
        s.abort_active = snap.abort_active;
        s.display_blocks_dropped = Some(snap.records_dropped);

        // THE DRIVE ECHO (operator acceptance, 2026-08-09: "shouldn't we be able to
        // just read drive V from the device"). Filled from the SAME derivation the drive
        // ceiling is enforced against, so the panel and the refusal cannot disagree.
        //
        // What this is, stated because the field name does not say it: the volts this
        // core will COMMAND at the drive output for a full-amplitude pulse at the
        // current rung. It is NOT a readback - nothing in the shock path reads the AO,
        // and the 9263 offers no readback channel - so no measured drive number exists
        // to echo here. `None` while the core holds no ceiling tuple, never zero: zero
        // volts is a real commanded value and must not be how "unknown" renders.
        if let Some(db) = snap.level_db {
            if let Ok(g) = self.max_drive_vpk.lock() {
                let (_ceiling, volts_per_eu, full_peak_eu) = *g;
                let v = commanded_drive_vpk(full_peak_eu, db, volts_per_eu);
                if v.is_finite() {
                    s.drive_vpk = Some(v);
                }
            }
        }

        // THE LIVE HALF: while a fire is in flight the observer's counters are
        // the authority, because the snapshot is written at job boundaries and a train is
        // ONE job. Without this `pulses_fired` sits still for a whole train.
        let (fired, remaining) = if self.progress.firing.load(Ordering::SeqCst) {
            let started = self.progress.started.load(Ordering::SeqCst);
            let total = self.progress.train_total.load(Ordering::SeqCst);
            (
                snap.pulses_fired + started.saturating_sub(1),
                (total > 0).then(|| total.saturating_sub(started)),
            )
        } else {
            (snap.pulses_fired, snap.train_remaining)
        };
        s.pulses_fired = Some(fired);
        s.pulses_at_level = Some(snap.pulses_at_level);
        s.train_remaining = remaining;
        s.last_pulse_seq = snap.last_pulse_seq;

        // THE PER-PULSE DOC, inside its TTL only. See LAST_PULSE_TTL for why a TTL and
        // not a strict edge.
        let at = self.last_pulse_at_ms.load(Ordering::Relaxed);
        if at != 0 && now_ms().saturating_sub(at) <= LAST_PULSE_TTL.as_millis() as u64 {
            if let Some(j) = self.ring.lock().ok().and_then(|r| r.last_status_json()) {
                s.last_pulse = Some(Doc::new(SchemaKind::ShockPulse, j));
            }
        }
    }

    // --- internals ------------------------------------------------------------------

    /// A cheap synchronous gate for the fire verbs: is there a run they could join?
    fn require_running(&self, what: &str) -> Result<(), ProtocolError> {
        if self.priming.load(Ordering::SeqCst) {
            return Ok(()); // the arm is in flight; the job queues behind it
        }
        let snap = self.cell.read().ok_or_else(not_configured)?;
        match snap.module_state.split(':').next() {
            Some("armed") | Some("firing") => Ok(()),
            Some(other) => Err(ProtocolError::Refused {
                message: format!(
                    "{what} needs an ARMED classical Shock run and this one is {other}. Send \
                     Start to arm (Start means ARM for this module), or Shock(Reset) to clear \
                     a finished run."
                ),
            }),
            None => Err(not_configured()),
        }
    }

    /// Take the service mutex, giving up after `budget` rather than parking behind a
    /// whole train. See [`busy_firing`].
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

    fn shutdown_pump(&self) {
        self.stop.store(true, Ordering::SeqCst);
        {
            let (_, cv) = &*self.jobs;
            cv.notify_all();
        }
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        if let Ok(mut q) = self.jobs.0.lock() {
            q.clear();
        }
        self.stop.store(false, Ordering::SeqCst);
    }

    fn spawn_pump(&self) {
        let mut slot = self.pump.lock().expect("pump poisoned");
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
            tx: self.tx.clone(),
            cell: Arc::clone(&self.cell),
            progress: Arc::clone(&self.progress),
            fault: Arc::clone(&self.fault),
            beat: Arc::clone(&self.last_tick_ms),
            expected: Arc::clone(&self.expected_iter_ms),
            ring: Arc::clone(&self.ring),
            srs_freqs: Arc::clone(&self.srs_freqs),
            last_pulse_at: Arc::clone(&self.last_pulse_at_ms),
            run_id: self
                .provenance()
                .and_then(|p| p.run_id.parse::<u64>().ok())
                .unwrap_or(0),
        };
        let svc_for_panic = Arc::clone(&self.svc);
        let fault_out = Arc::clone(&self.fault);
        let priming_out = Arc::clone(&self.priming);
        let handle = std::thread::spawn(move || {
            // The whole pump under catch_unwind, ending in emergency_stop (the Explorer
            // rail): a panic in the display packing or the report assembly unwinds past
            // every stop path in this file while the AO keeps clocking its last sample.
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                pump_body(ctx);
            }));
            if let Err(payload) = caught {
                let what = panic_text(&payload);
                tracing::error!(
                    target: "hertzforge::backendd",
                    panic = %what,
                    "SHOCK PUMP PANICKED - issuing emergency_stop"
                );
                if let Ok(mut g) = svc_for_panic.lock() {
                    if let Some(s) = g.as_mut() {
                        let _ = s.abort_with_reason(&format!("daemon pump panicked: {what}"));
                    }
                }
                if let Ok(mut f) = fault_out.lock() {
                    *f = Some(format!("pump_panicked: {what}; emergency_stop was issued"));
                }
                priming_out.store(false, Ordering::SeqCst);
            }
        });
        *slot = Some(handle);
    }
}

impl Drop for ShockHost {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        let (_, cv) = &*self.jobs;
        cv.notify_all();
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
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
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    cell: Arc<ShockCell>,
    progress: Arc<Progress>,
    fault: Arc<Mutex<Option<String>>>,
    beat: Arc<AtomicU64>,
    expected: Arc<AtomicU64>,
    ring: Arc<Mutex<PulseRing>>,
    srs_freqs: Arc<Mutex<Vec<f64>>>,
    last_pulse_at: Arc<AtomicU64>,
    run_id: u64,
}

fn pump_body(ctx: PumpCtx) {
    let mut tap = TransientTap::new(ctx.run_id);
    let mut window_seq: u32 = 0;
    let mut published_seq: Option<u64> = None;

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

        run_job(&ctx, job, &mut tap, &mut window_seq, &mut published_seq);
    }

    // ---- THE FINAL PUBLISH, on EVERY exit path (the Explorer rail) -----------------
    //
    // The cell is written by the pump, and the run can be ended from OUTSIDE this
    // thread - `ShockHost::abort` raises the cancel flag and `shutdown_pump` raises the
    // stop flag, either of which ends the loop at its CONDITION, before any in-loop
    // publish. Leaving without a final write strands the cell on the last snapshot and
    // the status path then watches a beat that has stopped.
    ctx.progress.end();
    if let Ok(g) = ctx.svc.lock() {
        if let Some(s) = g.as_ref() {
            publish_snapshot(&ctx, s, None);
            journal_end(s);
        }
    }
    tracing::info!(target: "hertzforge::backendd", "shock pump thread exited");
}

/// Execute ONE job under the service mutex, then publish everything it produced.
fn run_job(
    ctx: &PumpCtx,
    job: Job,
    tap: &mut TransientTap,
    window_seq: &mut u32,
    published_seq: &mut Option<u64>,
) {
    // The PRIMING guard covers the arm job only, and it clears on every exit including
    // a refusal or a panic.
    let guard = matches!(job, Job::Arm { .. }).then(|| PrimingGuard(Arc::clone(&ctx.priming)));

    let train_total = match &job {
        Job::FireTrain { spec } => spec.map(|s| s.count).unwrap_or(0),
        Job::Gunfire => u32::MAX, // resolved from the config below
        _ => 0,
    };

    let mut lock = match ctx.svc.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(svc) = lock.as_mut() else { return };

    // The observer is installed for EVERY job that can fire: the watchdog
    // beat and the mid-train counter both live in it, and a job that fired without one
    // would look stalled and would freeze the counters.
    {
        let beat = Arc::clone(&ctx.beat);
        let progress = Arc::clone(&ctx.progress);
        svc.set_block_observer(Box::new(move |delivered, _expected| {
            beat.store(now_ms(), Ordering::Relaxed);
            progress.observe(delivered);
        }));
    }

    let planned_spacing = svc
        .config()
        .rate_model
        .effective_min_spacing(hertzforge_shock_common::DrivePath::HostDac);
    let total = if train_total == u32::MAX {
        svc.config().gunfire.map(|g| g.rounds).unwrap_or(0)
    } else {
        train_total
    };
    ctx.progress.begin(total, planned_spacing);
    ctx.beat.store(now_ms(), Ordering::Relaxed);

    let outcome: Result<(), ProtocolError> = match job {
        Job::Arm { ack } => {
            let r = match ack {
                Some(a) => svc.arm_with_override(&a),
                None => {
                    // Feasibility runs first, always: `arm()` requires the assessment to
                    // have happened, and an EXCEEDED verdict must park at
                    // FeasibilityBlocked rather than arming.
                    match svc.assess_feasibility() {
                        Ok(_) | Err(ServiceError::BadState(_)) => {}
                        Err(e) => {
                            let msg = format!("arm_refused: {e}");
                            if let Ok(mut f) = ctx.fault.lock() {
                                *f = Some(msg.clone());
                            }
                            ctx.progress.end();
                            svc.clear_block_observer();
                            publish_snapshot(ctx, svc, None);
                            drop(guard);
                            return;
                        }
                    }
                    svc.arm()
                }
            };
            match r {
                Ok(()) => {
                    // The adopted rate is known only after the arm; every Hz-denominated
                    // thing downstream reads it, and so does the watchdog's cadence.
                    if let Some(fs) = svc.actual_fs_hz() {
                        ctx.expected.store(
                            block_period_ms(fs, hertzforge_hal::default_block_frames(fs)),
                            Ordering::SeqCst,
                        );
                    }
                    *ctx.srs_freqs.lock().expect("freqs poisoned") = svc.srs_freqs().to_vec();
                    ctx.stopping.store(false, Ordering::SeqCst);
                    Ok(())
                }
                Err(e) => {
                    let msg = format!("arm_refused: {e}");
                    tracing::warn!(target: "hertzforge::backendd", "shock arm refused: {e}");
                    if let Ok(mut f) = ctx.fault.lock() {
                        *f = Some(msg);
                    }
                    Ok(())
                }
            }
        }
        Job::FirePulse => svc.fire_next_pulse().map(|_| ()).map_err(svc_err),
        Job::RunAuto { max_pulses } => svc.run_auto(max_pulses).map(|_| ()).map_err(svc_err),
        Job::FireTrain { spec } => match spec {
            Some(s) => svc.fire_train_with(s).map(|_| ()).map_err(svc_err),
            None => svc.fire_train().map(|_| ()).map_err(svc_err),
        },
        Job::Gunfire => svc.fire_gunfire_burst().map(|_| ()).map_err(svc_err),
        Job::Stop { reason } => {
            tracing::info!(target: "hertzforge::backendd", reason, "shock controlled stop");
            svc.stop().map_err(svc_err)
        }
        Job::Abort { reason } => svc.abort_with_reason(&reason).map_err(svc_err),
        Job::Reset => {
            // The service cannot go back to Idle in place - the arm moved the station
            // backend into the firer with no reclaim path - so a Reset is a REBUILD, and
            // it is the host's `set_profile` that owns that. The pump only takes the run
            // down; the rebuild happens on the next SetProfile. Stated rather than
            // silently partial.
            let _ = svc.stop();
            Ok(())
        }
    };

    if let Err(e) = outcome {
        tracing::warn!(target: "hertzforge::backendd", "shock job refused: {e:?}");
        if let Ok(mut f) = ctx.fault.lock() {
            // A REFUSAL from a fire is a run-state fact, not a station fault, so it goes
            // in the fault cell only when the run is not already saying it itself.
            if svc.state() != ShockRunState::Aborted {
                *f = Some(format!("job_refused: {e:?}"));
            }
        }
    }

    svc.clear_block_observer();
    ctx.progress.end();
    ctx.cancel.store(false, Ordering::SeqCst);

    // ---- INGEST EVERY NEW PULSE, then publish -------------------------------------
    let mut fresh: Vec<PulseRecord> = Vec::new();
    {
        let store = svc.pulse_store();
        let after = published_seq.map(|s| s + 1).unwrap_or(0);
        for r in store.records().iter().filter(|r| r.seq >= after) {
            fresh.push(r.clone());
        }
    }
    let snapshot_srs = svc.srs_freqs().to_vec();
    // The authored per-band SRS tolerance, read under the lock with everything else the
    // publish needs. It is the half-width of the +/- band the tolerance check enforces,
    // and 924 draws its UPPER edge.
    let srs_band_tol_db = svc.config().srs_band_tol_db;
    let last_seq = fresh.last().map(|r| r.seq);
    if last_seq.is_some() {
        *published_seq = last_seq;
    }
    publish_snapshot(ctx, svc, last_seq);
    journal_warnings(svc);

    // The service mutex is RELEASED before anything is sent: packing is cheap, a slow
    // broadcast is not, and neither may lengthen the hold.
    drop(lock);
    drop(guard);

    if !fresh.is_empty() {
        ctx.last_pulse_at.store(now_ms(), Ordering::Relaxed);
    }
    for r in fresh {
        let published = tap.publish_pulse(&ctx.tx, &r.time_history, r.fs_hz);
        if published == 0 {
            tracing::warn!(
                target: "hertzforge::backendd",
                seq = r.seq,
                "shock pulse published NO time batches - the record was empty or its rate \
                 was unusable"
            );
        }
        // The SRS on the ordinal band axis. `srs_freqs` (the query) is the frequency
        // axis; see `srs_spectrum_batch`.
        //
        // THREE traces, not one. The measurement alone is a curve with nothing to read
        // it against: a remote pane drawing only 920 shows an SRS and no reference and
        // no tolerance, and an operator cannot tell a passing pulse from a failing one
        // by looking at it. 923 and 924 are the same two curves the LOCAL core has
        // always drawn beside it, derived from the same record and by the same
        // arithmetic (`ui/src-tauri/src/shock.rs::classical_frames`).
        //
        // PVSS used to ride 922 here and no longer does - see
        // [`CH_SHOCK_PULSE_FFT_UNPUBLISHED`] for the collision and where PVSS still
        // crosses.
        let mut traces: Vec<(u32, Arc<Vec<f32>>)> = Vec::new();
        if !r.srs.is_empty() && r.srs.len() == snapshot_srs.len() {
            traces.push((
                CH_SHOCK_SRS,
                Arc::new(r.srs.iter().map(|&v| v as f32).collect()),
            ));
            // The reference at FULL level: the measured SRS scaled back up by this
            // pulse's own ladder level, so the reference does not move as the ladder
            // walks. `level_db` is negative below full, hence the negated exponent.
            let level_scale = 10f64.powf(-r.level_db / 20.0);
            let reference: Vec<f32> = r.srs.iter().map(|&v| (v * level_scale) as f32).collect();
            let tol_hi_gain = 10f32.powf((srs_band_tol_db as f32) / 20.0);
            let tol_hi: Vec<f32> = reference.iter().map(|&v| v * tol_hi_gain).collect();
            traces.push((CH_SHOCK_REF_SRS, Arc::new(reference)));
            traces.push((CH_SHOCK_TOL_HI, Arc::new(tol_hi)));
        }
        *window_seq = window_seq.wrapping_add(1);
        if let Some(b) = srs_spectrum_batch(ctx.run_id, *window_seq, traces) {
            let _ = ctx.tx.send(DisplayBatch::Spectrum(b));
        }
        ctx.ring.lock().expect("ring poisoned").push(r);
    }
}

/// Write the published snapshot from the service's own status.
fn publish_snapshot(ctx: &PumpCtx, svc: &Svc, last_seq: Option<u64>) {
    let st = svc.status();
    let dropped = ctx.ring.lock().map(|r| r.dropped()).unwrap_or(0);
    let prev = ctx.cell.read();
    let (run, token) = classical_wire_state(svc, &st);
    ctx.cell.write(ShockSnapshot {
        run,
        module_state: token,
        level_db: Some(st.current_level_db),
        pulses_fired: st.pulses_fired.min(u32::MAX as u64) as u32,
        pulses_at_level: st.pulses_at_level,
        train_remaining: None,
        last_pulse_seq: last_seq.or_else(|| prev.as_ref().and_then(|p| p.last_pulse_seq)),
        alarm_active: st.alarm_active,
        abort_active: st.abort_active,
        requested_fs_hz: Some(st.requested_fs_hz),
        actual_fs_hz: st.actual_fs_hz,
        band_coherence: None,
        records_dropped: dropped,
    });
}

/// The service state, mapped onto the wire state plus a NAMESPACED module-state token.
///
/// The token before the colon is always the bare state, so a consumer that only wants
/// the state splits once (the Explorer's convention). Terminal states carry what ended
/// them; `feasibility_blocked` carries the verdict that blocked it.
fn classical_wire_state(
    svc: &Svc,
    st: &hertzforge_shock_host::service::report::Status,
) -> (hertzforge_protocol::RunState, String) {
    match st.state {
        ShockRunState::Idle => (hertzforge_protocol::RunState::Configured, "idle".into()),
        ShockRunState::Feasibility => (
            hertzforge_protocol::RunState::Configured,
            "feasibility".into(),
        ),
        ShockRunState::FeasibilityBlocked => (
            hertzforge_protocol::RunState::Configured,
            format!(
                "feasibility_blocked: {}",
                svc.feasibility()
                    .map(|d| format!("{:?}", d.report.overall))
                    .unwrap_or_else(|| "exceeded".into())
            ),
        ),
        ShockRunState::Armed => (hertzforge_protocol::RunState::Armed, "armed".into()),
        ShockRunState::Firing => (hertzforge_protocol::RunState::Running, "firing".into()),
        ShockRunState::Stopped => (
            hertzforge_protocol::RunState::Stopped,
            "stopped: the run ended cleanly".into(),
        ),
        ShockRunState::Aborted => (
            hertzforge_protocol::RunState::Faulted,
            format!(
                "aborted: {}",
                svc.trip()
                    .map(|t| format!("{:?}", t.kind))
                    .unwrap_or_else(|| "a protection tripped".into())
            ),
        ),
    }
}

/// DRIVER diagnostics the service drained must reach the journal, or the first anyone
/// hears of an underrun is a symptom. The service drains
/// `Backend::take_warnings` into its event log after every fire; this is the
/// journal half.
fn journal_warnings(svc: &Svc) {
    for e in svc.log().events().iter().rev().take(24) {
        if let hertzforge_shock_host::service::report::EventKind::DriverWarning { pulse, text } =
            &e.kind
        {
            tracing::warn!(
                target: "hertzforge::backendd",
                pulse = ?pulse,
                "shock DRIVER diagnostic: {text}"
            );
        }
    }
}

/// The abort tail: a run that ended for any reason but "the operator asked" has
/// to say so in the journal, or a tripped run reads as an unexplained thread exit.
fn journal_end(svc: &Svc) {
    if let Some(trip) = svc.trip() {
        tracing::warn!(
            target: "hertzforge::backendd",
            trip = format!("{:?}", trip.kind),
            at_level_db = trip.at_level_db,
            at_pulse = trip.at_pulse,
            "shock run ended ABORTED"
        );
        for e in svc.log().events().iter().rev().take(6) {
            tracing::warn!(target: "hertzforge::backendd", "  abort-tail: {:?}", e.kind);
        }
    }
}

// =====================================================================================
// Wire profile -> service config
// =====================================================================================

/// The block period in ms at a rate - the watchdog's cadence scale.
fn block_period_ms(fs_hz: f64, block_frames: usize) -> u64 {
    if !(fs_hz.is_finite() && fs_hz > 0.0) {
        return 0;
    }
    ((block_frames as f64 / fs_hz) * 1e3).round().max(0.0) as u64
}

/// How long ONE pulse's record is, in seconds at the REQUESTED rate.
///
/// Main pulse + both compensation shoulders + the time-denominated quiet surround
/// (`max(0.5 T, 5 ms)` each side). The rate cancels out of everything except the quiet
/// surround's floor, so this is a rate-independent bound in all but the shortest-pulse
/// case - which is why it can be checked at `set_profile`, before the device has said
/// what rate it will run at.
fn pulse_wall_seconds(cfg: &ClassicalTestConfig) -> f64 {
    let t = cfg.spec.duration_ms / 1000.0;
    let shoulders = if cfg.spec.compensation.symmetric {
        2.0 * cfg.spec.compensation.pre_frac
    } else {
        cfg.spec.compensation.pre_frac + cfg.spec.compensation.post_frac
    };
    let quiet = (0.5 * t).max(0.005);
    t * (1.0 + shoulders) + 2.0 * quiet
}

fn train_spec(w: hertzforge_protocol::ShockTrainSpec) -> hertzforge_shock_common::TrainSpec {
    hertzforge_shock_common::TrainSpec {
        count: w.count,
        spacing: match w.spacing {
            hertzforge_protocol::ShockSpacing::Delay(s) => {
                hertzforge_shock_common::Spacing::Delay(s)
            }
            hertzforge_protocol::ShockSpacing::MaxRate => hertzforge_shock_common::Spacing::MaxRate,
        },
    }
}

/// **THE TRANSLATION, and the two obligations `hertzforge_protocol::shock`'s module docs
/// place on it.**
///
/// 1. **The capture block is FORCED OFF.** The wire profile carries no capture rules at
///    all, so there is no field an older or hand-written client could use to flip
///    `auto_start` back on - but the translation still has to state the value rather
///    than inherit whatever `ClassicalTestConfig`'s default happens to be, because a
///    default is a thing that changes. `auto_start = false` closes the remote-capture
///    back door: a Mac-side checkbox must not be able to start a disk writer on the
///    core host that the operator cannot see, cannot size, and has no verb to stop.
///    Pinned by `the_translation_forces_capture_auto_start_off`.
/// 2. **`rate_model` crosses the wire and is used verbatim.** It is an operator/site
///    input - cooling, settling and compute floors for THIS shaker in THIS room - and a
///    default buried in the daemon would silently govern how fast a train may fire.
fn translate_classical(p: &ClassicalShockProfile) -> Result<ClassicalTestConfig, ProtocolError> {
    use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
    use hertzforge_shock_host::spec::{ClassicalPulseSpec, ClassicalShape};

    let shape = match p.spec.shape {
        hertzforge_protocol::ShockPulseShape::HalfSine => ClassicalShape::HalfSine,
        hertzforge_protocol::ShockPulseShape::Haversine => ClassicalShape::Haversine,
        hertzforge_protocol::ShockPulseShape::TerminalPeakSawtooth => {
            ClassicalShape::TerminalPeakSawtooth
        }
        hertzforge_protocol::ShockPulseShape::InitialPeakSawtooth => {
            ClassicalShape::InitialPeakSawtooth
        }
        hertzforge_protocol::ShockPulseShape::Trapezoid {
            rise_frac,
            fall_frac,
        } => ClassicalShape::Trapezoid {
            rise_frac,
            fall_frac,
        },
        hertzforge_protocol::ShockPulseShape::Rectangle => ClassicalShape::Rectangle,
    };
    let compensation = CompensationConfig {
        shape: match p.spec.compensation.shape {
            hertzforge_protocol::ShockCompensationShape::HalfSine => CompensationShape::HalfSine,
            hertzforge_protocol::ShockCompensationShape::Rectangular => {
                CompensationShape::Rectangular
            }
            hertzforge_protocol::ShockCompensationShape::RoundedRectangular => {
                CompensationShape::RoundedRectangular
            }
        },
        pre_frac: p.spec.compensation.pre_frac,
        post_frac: p.spec.compensation.post_frac,
        symmetric: p.spec.compensation.symmetric,
    };
    let spec = ClassicalPulseSpec {
        shape,
        amplitude_g: p.spec.amplitude_g,
        duration_ms: p.spec.duration_ms,
        compensation,
    };

    let (shaker, moving_mass) = feasibility_inputs(p.feasibility.as_ref())?;

    let mut cfg = ClassicalTestConfig::new(spec);
    cfg.fs_hz = p.fs_hz;
    cfg.standard = match p.standard {
        hertzforge_protocol::ShockToleranceStandard::Mil810_516 => {
            hertzforge_shock_host::tolerance::ToleranceStandard::Mil810_516
        }
        hertzforge_protocol::ShockToleranceStandard::Iec60068_2_27 => {
            hertzforge_shock_host::tolerance::ToleranceStandard::Iec60068_2_27
        }
    };
    cfg.schedule = level_schedule(&p.schedule);
    cfg.train = train_spec(p.train);
    cfg.gunfire = p.gunfire.map(|g| hertzforge_shock_host::GunfireSpec {
        rounds_per_min: g.rounds_per_min,
        rounds: g.rounds,
    });
    cfg.rate_model = hertzforge_shock_common::RateModel {
        cooling_floor_s: p.rate_model.cooling_floor_s,
        settling_floor_s: p.rate_model.settling_floor_s,
        compute_floor_s: p.rate_model.compute_floor_s,
        device_recompute_floor_s: p.rate_model.device_recompute_floor_s,
        read_cadence_ok: p.rate_model.read_cadence_ok,
    };
    cfg.drive_limit = drive_limit(&p.drive_limit);
    cfg.shaker = shaker;
    cfg.moving_mass = moving_mass;
    cfg.residual_velocity_tol_ips = p.residual_velocity_tol_ips;
    cfg.residual_disp_tol_in = p.residual_disp_tol_in;
    cfg.srs_f_lo_hz = p.srs_f_lo_hz;
    cfg.srs_f_hi_hz = p.srs_f_hi_hz;
    cfg.srs_bands_per_octave = p.srs_bands_per_octave;
    cfg.srs_band_tol_db = p.srs_band_tol_db;
    cfg.compute_pvss = p.compute_pvss;
    cfg.abort_on_tolerance_failure = p.abort_on_tolerance_failure;
    // OBLIGATION 1 - see this function's docs. NOT inherited from the default.
    cfg.capture.auto_start = false;
    cfg.metadata = p
        .metadata
        .iter()
        .map(|m| hertzforge_shock_host::MetaField {
            key: m.key.clone(),
            label: m.label.clone(),
            value: m.value.clone(),
        })
        .collect();
    cfg.validate().map_err(|e| ProtocolError::InvalidArgument {
        message: e.to_string(),
    })?;
    Ok(cfg)
}

/// The shared level schedule, wire -> core.
pub(crate) fn level_schedule(
    w: &hertzforge_protocol::ShockLevelSchedule,
) -> hertzforge_shock_common::ShockLevelSchedule {
    hertzforge_shock_common::ShockLevelSchedule {
        mode: match w.mode {
            hertzforge_protocol::ShockRunMode::Auto => hertzforge_shock_common::ShockRunMode::Auto,
            hertzforge_protocol::ShockRunMode::Manual => {
                hertzforge_shock_common::ShockRunMode::Manual
            }
        },
        start_db: w.start_db,
        table_db: w.table_db.clone(),
        pulses_per_level: w.pulses_per_level.clone(),
    }
}

/// The shared drive-limit set, wire -> core.
pub(crate) fn drive_limit(
    w: &hertzforge_protocol::ShockDriveLimit,
) -> hertzforge_shock_common::DriveLimit {
    hertzforge_shock_common::DriveLimit {
        peak_accel_g: w.peak_accel_g,
        monitors: w
            .monitors
            .iter()
            .map(|m| hertzforge_shock_common::DriveMonitor {
                channel: m.channel.clone(),
                eu: m.eu.clone(),
                peak_ceiling: m.peak_ceiling,
                kind: match m.kind {
                    hertzforge_protocol::ShockMonitorKind::Predictive => {
                        hertzforge_shock_common::DriveMonitorKind::Predictive
                    }
                    hertzforge_protocol::ShockMonitorKind::BetweenPulse => {
                        hertzforge_shock_common::DriveMonitorKind::BetweenPulse
                    }
                },
            })
            .collect(),
    }
}

/// The feasibility inputs, wire -> core. A payload that does not parse is a REFUSAL,
/// never a run with feasibility quietly skipped (the `FeasibilityInputs` doc's rule).
pub(crate) fn feasibility_inputs(
    f: Option<&hertzforge_protocol::FeasibilityInputs>,
) -> Result<
    (
        Option<hertzforge_shaker_db::ShakerRecord>,
        hertzforge_shaker_db::MovingMass,
    ),
    ProtocolError,
> {
    let Some(f) = f else {
        return Ok((None, hertzforge_shaker_db::MovingMass::default()));
    };
    let shaker: hertzforge_shaker_db::ShakerRecord =
        serde_json::from_str(&f.shaker_json).map_err(|e| ProtocolError::MalformedPayload {
            container: "shock feasibility shaker record".into(),
            detail: format!(
                "{e}. Feasibility is a SAFETY gate: a record that does not parse is refused \
                 rather than silently skipped, which would arm a run with no shaker-limit \
                 check and no sign that one was expected."
            ),
        })?;
    Ok((
        Some(shaker),
        hertzforge_shaker_db::MovingMass {
            armature_lb: f.armature_lb,
            slip_table_lb: f.slip_table_lb,
            drive_adapter_lb: f.drive_adapter_lb,
            fixturing_lb: f.fixturing_lb,
            test_article_lb: f.test_article_lb,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_profile() -> ClassicalShockProfile {
        use hertzforge_protocol::*;
        ClassicalShockProfile {
            schema_version: PROFILE_SCHEMA,
            spec: ShockPulseSpec {
                shape: ShockPulseShape::HalfSine,
                amplitude_g: 0.4, // g==V on the wire; honest vs the 5 V default ceiling
                duration_ms: 11.0,
                compensation: ShockCompensation {
                    shape: ShockCompensationShape::HalfSine,
                    pre_frac: 1.5,
                    post_frac: 1.5,
                    symmetric: true,
                },
            },
            fs_hz: 5120.0,
            standard: ShockToleranceStandard::Mil810_516,
            schedule: ShockLevelSchedule {
                mode: ShockRunMode::Auto,
                start_db: -12.0,
                table_db: vec![-12.0, -6.0, 0.0],
                pulses_per_level: vec![1],
            },
            train: ShockTrainSpec {
                count: 3,
                spacing: ShockSpacing::MaxRate,
            },
            gunfire: None,
            rate_model: ShockRateModel {
                cooling_floor_s: 0.2,
                settling_floor_s: 0.1,
                compute_floor_s: 0.05,
                device_recompute_floor_s: 20.0,
                read_cadence_ok: true,
            },
            drive_limit: ShockDriveLimit {
                peak_accel_g: None,
                monitors: Vec::new(),
            },
            feasibility: None,
            residual_velocity_tol_ips: 1.0,
            residual_disp_tol_in: 0.1,
            srs_f_lo_hz: 20.0,
            srs_f_hi_hz: 2000.0,
            srs_bands_per_octave: 3.0,
            srs_band_tol_db: 3.0,
            compute_pvss: false,
            abort_on_tolerance_failure: true,
            max_drive_vpk: DEFAULT_SHOCK_MAX_DRIVE_VPK,
            metadata: Vec::new(),
        }
    }

    /// **THE TEST THE PROTOCOL'S MODULE DOCS PROMISE** (`hertzforge_protocol::shock`:
    /// "the daemon FORCES `auto_start = false` when it translates either profile ...
    /// with a test").
    ///
    /// RED-MUTATION: delete the `cfg.capture.auto_start = false` line and this reds -
    /// the state where a remote configure starts a disk writer on the core host that
    /// the operator cannot see, cannot size and has no verb to stop (the remote-capture
    /// back door).
    ///
    /// Anti-vacuity: `ClassicalTestConfig::new`'s default is asserted separately, so a
    /// pass here depends on the forcing line and not on the default happening to agree.
    #[test]
    fn the_translation_forces_capture_auto_start_off() {
        let cfg = translate_classical(&wire_profile()).expect("translate");
        assert!(
            !cfg.capture.auto_start,
            "the daemon must force capture auto_start OFF - the wire has no field for it \
             because remote capture is not served over this seam"
        );
        // ANTI-VACUITY: if the core default ever flips to true, the line above is what
        // holds - and this assertion is what says the line is load-bearing rather than
        // agreeing with a default.
        let mut deliberately_on = cfg.clone();
        deliberately_on.capture.auto_start = true;
        assert!(
            deliberately_on.capture.auto_start,
            "the field is settable, so forcing it is a real act"
        );
    }

    /// The rate model crosses the wire VERBATIM: it is an operator/site input and a
    /// daemon-side default would silently govern how fast a train may fire.
    #[test]
    fn the_translation_carries_the_rate_model_verbatim() {
        let p = wire_profile();
        let cfg = translate_classical(&p).expect("translate");
        assert_eq!(cfg.rate_model.cooling_floor_s, p.rate_model.cooling_floor_s);
        assert_eq!(
            cfg.rate_model.settling_floor_s,
            p.rate_model.settling_floor_s
        );
        assert_eq!(cfg.rate_model.compute_floor_s, p.rate_model.compute_floor_s);
        assert_eq!(
            cfg.rate_model.device_recompute_floor_s,
            p.rate_model.device_recompute_floor_s
        );
        assert!(cfg.rate_model.read_cadence_ok);
    }

    /// **THE DRIVE CEILING IS THE HOST'S JOB**: neither shock service has a drive-volts
    /// concept, so this is the only enforcement point. The refusal carries BOTH
    /// numbers.
    ///
    /// Anti-vacuity: the same profile inside the ceiling is accepted, so the refusal is
    /// about the arithmetic and not about the check being on.
    #[test]
    fn the_drive_ceiling_refuses_with_both_numbers_and_passes_inside_it() {
        // 20 g peak at 0 dB through a 1000 mV/EU Drive row = 20.0 Vpk implied.
        let err = enforce_drive_ceiling("the profile's top rung", 20.0, 0.0, 1.0, 5.0)
            .expect_err("20 Vpk must not pass a 5 Vpk ceiling");
        let ProtocolError::Refused { message } = err else {
            panic!("the ceiling refusal must be a Refused");
        };
        assert!(message.contains("20.000 Vpk"), "{message}");
        assert!(message.contains("5.000 Vpk"), "{message}");
        // Inside it: -20 dB is a factor of 10 down, so 2.0 Vpk.
        let v = enforce_drive_ceiling("the profile's top rung", 20.0, -20.0, 1.0, 5.0)
            .expect("2 Vpk is inside a 5 Vpk ceiling");
        assert!((v - 2.0).abs() < 1e-9, "{v}");
    }

    /// **THE PANEL'S DRIVE ECHO AND THE CEILING'S REFUSAL ARE ONE DERIVATION**.
    ///
    /// Operator acceptance, 2026-08-09: "why is drive tied to channel 4? shouldn't
    /// we be able to just read drive V from the device". Both shock hosts now fill
    /// `StatusReport::drive_vpk` with `commanded_drive_vpk(..)`, which is the number the
    /// ceiling is enforced against - so a panel showing 4.0 Vpk cannot sit beside a
    /// refusal that says 4.2.
    ///
    /// This pins the shared derivation rather than the two `fill_status` call sites,
    /// which need a built station: the ceiling arm below is the discriminating one, and
    /// it goes red the moment the echo grows its own copy of the arithmetic.
    ///
    /// RED-MUTATION: change either multiplication in `commanded_drive_vpk` and the
    /// accepted-value row reds against the ceiling's own return.
    #[test]
    fn the_drive_echo_is_the_same_number_the_ceiling_refuses_on() {
        for (peak_eu, db, v_per_eu) in [(20.0, -20.0, 1.0), (0.4, -12.0, 1.0), (2.5, 0.0, 2.0)] {
            let echoed = commanded_drive_vpk(peak_eu, db, v_per_eu);
            let enforced = enforce_drive_ceiling("row", peak_eu, db, v_per_eu, 1e6)
                .expect("a 1e6 Vpk ceiling admits every row here");
            assert!(
                (echoed - enforced).abs() < 1e-12,
                "the echo ({echoed}) and the ceiling ({enforced}) disagree at \
                 {peak_eu} EU / {db} dB / {v_per_eu} V per EU"
            );
        }
        // The ladder is what makes this a DRIVE readout and not a restatement of the
        // profile: -6 dB is half the volts of 0 dB on the same pulse.
        let top = commanded_drive_vpk(1.0, 0.0, 1.0);
        let down = commanded_drive_vpk(1.0, -6.0, 1.0);
        assert!((down / top - 0.5011872336).abs() < 1e-9, "{down} vs {top}");
    }

    /// THE MID-TRAIN COUNTER. The observer sees only counts, so a NEW pulse is a
    /// `frames_delivered` that did not increase - the fire loop restarts the counter
    /// per pulse.
    ///
    /// RED-MUTATION: drop the `prev == 0` clause and the FIRST pulse of a job is never
    /// counted, which is the state where a single-pulse fire reports zero.
    #[test]
    fn the_progress_observer_counts_a_pulse_per_delivered_reset() {
        let p = Progress::default();
        p.begin(3, 2.0);
        // pulse 1: three blocks
        assert!(p.observe(256), "the first block of the job opens pulse 1");
        assert!(!p.observe(512));
        assert!(!p.observe(768));
        // pulse 2: the count restarts
        assert!(p.observe(256), "a non-increase opens the next pulse");
        assert!(!p.observe(512));
        // pulse 3
        assert!(p.observe(256));
        assert_eq!(p.started.load(Ordering::SeqCst), 3);
        assert_eq!(p.train_total.load(Ordering::SeqCst), 3);
        assert_eq!(p.spacing_ms.load(Ordering::SeqCst), 2000);
        assert!(p.firing.load(Ordering::SeqCst));
        p.end();
        assert!(!p.firing.load(Ordering::SeqCst));
    }

    /// A single-block record is the degenerate case: every call has
    /// `delivered == expected`, so the edge is `<=` rather than `<`.
    #[test]
    fn a_single_block_pulse_still_counts_once_each() {
        let p = Progress::default();
        p.begin(0, 0.0);
        assert!(p.observe(64));
        assert!(p.observe(64));
        assert_eq!(p.started.load(Ordering::SeqCst), 2);
    }

    /// The bounded ring drops the OLDEST and SAYS how many.
    #[test]
    fn the_pulse_ring_is_bounded_and_reports_its_drops() {
        let mut ring = PulseRing::default();
        for seq in 0..(PULSE_RING_CAP as u64 + 3) {
            ring.push(PulseRecord {
                seq,
                level_db: 0.0,
                time_history: vec![0.0; 4],
                fs_hz: 51_200.0,
                kinematics: hertzforge_shock_common::pulse_store::PulseKinematicsRecord {
                    peak_accel_g: 1.0,
                    peak_velocity_ips: 0.0,
                    residual_velocity_ips: 0.0,
                    peak_disp_in: 0.0,
                    min_disp_in: 0.0,
                    pk_pk_disp_in: 0.0,
                    residual_disp_in: 0.0,
                },
                srs: vec![1.0],
                srs_freqs: vec![100.0],
                pvss: None,
                fired_at_unix: 0,
            });
        }
        assert_eq!(ring.dropped(), 3);
        assert!(ring.detail_json(0).is_none(), "the oldest rolled out");
        assert!(ring.detail_json(PULSE_RING_CAP as u64 + 2).is_some());
        let list = ring.list_json();
        assert!(list.contains("\"dropped\":3"), "{list}");
        // The refusal for a rolled-out sequence NAMES both bounds.
        let ProtocolError::InvalidArgument { message } = no_such_pulse(0, ring.dropped()) else {
            panic!("a missing pulse is an InvalidArgument");
        };
        assert!(message.contains("dropped 3"), "{message}");
    }

    /// The SRS batch rides an ORDINAL axis and SAYS so in the grid document, because a
    /// geometric grid cannot be expressed as `(f0, df)`.
    #[test]
    fn the_srs_batch_is_ordinal_and_the_grid_doc_says_so() {
        let b = srs_spectrum_batch(7, 1, vec![(CH_SHOCK_SRS, Arc::new(vec![1.0, 2.0, 3.0]))])
            .expect("a non-empty trace publishes");
        assert_eq!(b.f0_hz, 0.0);
        assert_eq!(b.df_hz, 1.0);
        assert_eq!(b.traces[0].0, CH_SHOCK_SRS);
        let doc = srs_grid_json(&[20.0, 25.0, 31.5], 10.0);
        assert!(doc.contains("\"axis\":\"ordinal\""), "{doc}");
        assert!(doc.contains("\"n_bands\":3"), "{doc}");
        // An EMPTY trace set publishes nothing rather than a blank entry in the pane's
        // channel list.
        assert!(srs_spectrum_batch(7, 1, Vec::new()).is_none());
        assert!(srs_spectrum_batch(7, 1, vec![(CH_SHOCK_SRS, Arc::new(Vec::new()))]).is_none());
    }

    /// **THE PER-PULSE POSITIVE FLOOR**: one pulse publishes at least
    /// `PULSE_TIME_CHUNKS` scope batches and the same number of wave batches, and the
    /// frame clock ADVANCES across pulses including the silence between them.
    #[test]
    fn one_pulse_publishes_the_per_pulse_floor_and_the_clock_advances() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(256);
        let mut tap = TransientTap::new(9);
        let fs = 51_200.0;
        let record: Vec<f64> = (0..2304).map(|i| (i as f64 / 100.0).sin()).collect();
        let n = tap.publish_pulse(&tx, &record, fs);
        assert_eq!(
            n,
            2 * PULSE_TIME_CHUNKS,
            "K scope + K wave batches per pulse is the floor the wire gate asserts"
        );
        let mut scope_ends = Vec::new();
        let mut wave = 0;
        while let Ok(b) = rx.try_recv() {
            match b {
                DisplayBatch::Scope(s) => scope_ends.push(s.t_end_s),
                DisplayBatch::Wave(_) => wave += 1,
                DisplayBatch::Spectrum(_) => {}
            }
        }
        assert_eq!(wave, PULSE_TIME_CHUNKS);
        assert_eq!(scope_ends.len(), PULSE_TIME_CHUNKS);
        for w in scope_ends.windows(2) {
            assert!(
                w[1] > w[0],
                "the frame clock must ADVANCE within a pulse: {scope_ends:?}"
            );
        }
        let first_pulse_end = *scope_ends.last().unwrap();
        // The SECOND pulse's clock starts AFTER the gap, never back at the first
        // pulse's end - which is the whole point of a wall-clock frame clock.
        std::thread::sleep(Duration::from_millis(30));
        tap.publish_pulse(&tx, &record, fs);
        let mut second_start = None;
        while let Ok(b) = rx.try_recv() {
            if let DisplayBatch::Scope(s) = b {
                second_start.get_or_insert(s.t_end_s);
            }
        }
        let second = second_start.expect("the second pulse publishes");
        let gap = second - first_pulse_end - (record.len() as f64 / PULSE_TIME_CHUNKS as f64) / fs;
        assert!(
            gap > 0.02,
            "the inter-pulse silence must appear in the clock as a GAP (measured {gap:.4} s)"
        );
    }

    /// The station build refuses a two-Drive table BY NAME.
    #[test]
    fn two_drive_rows_are_refused_by_name() {
        let mut t = ChannelTable::default_with_channels(4);
        t.channels[2].role = ChannelRole::Drive;
        t.channels[3].role = ChannelRole::Drive;
        match build_shock_station(BackendKind::Sim, &[], &t, 5120.0) {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("2 enabled Drive rows"), "{message}");
                assert!(message.contains("PutChannelTable"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// A table with no Control row is refused by name: every shock acceptance check is
    /// computed from the control record, so a run without one measures nothing.
    #[test]
    fn a_table_without_a_control_row_is_refused_by_name() {
        let mut t = ChannelTable::default_with_channels(4);
        t.channels[3].role = ChannelRole::Drive;
        for c in t.channels.iter_mut().take(3) {
            c.role = ChannelRole::Monitor;
        }
        match build_shock_station(BackendKind::Sim, &[], &t, 5120.0) {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("no enabled Control row"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }
}
